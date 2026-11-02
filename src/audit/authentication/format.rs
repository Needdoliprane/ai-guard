//! Closed authentication format and replay; no tenant or execution metadata.
use super::*;
use anyhow::{Result, ensure};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const VERSION: i64 = 1_000_001;
pub(super) const APPLICATION_ID: i64 = 0x4149_4741; // AIGA
const PURPOSE: &str = "authentication_audit_v1";
const MAX_PAYLOAD_BYTES: usize = 32_768;
const ZERO_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

const SCHEMA: [(&str, &str); 3] = [
    (
        "authentication_meta",
        "CREATE TABLE authentication_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),purpose TEXT NOT NULL CHECK(purpose='authentication_audit_v1'),installation_id TEXT NOT NULL,namespace_uuid TEXT NOT NULL,genesis_hash TEXT NOT NULL)",
    ),
    (
        "authentication_entries",
        "CREATE TABLE authentication_entries(seq INTEGER PRIMARY KEY,received_at TEXT NOT NULL,prev_hash TEXT NOT NULL,hash TEXT NOT NULL UNIQUE,payload TEXT NOT NULL,operation_id TEXT,phase TEXT CHECK(phase IN('intention','result')),CHECK((operation_id IS NULL AND phase IS NULL) OR (operation_id IS NOT NULL AND phase IS NOT NULL)),UNIQUE(operation_id,phase))",
    ),
    (
        "authentication_quota",
        "CREATE TABLE authentication_quota(singleton INTEGER PRIMARY KEY CHECK(singleton=1),records INTEGER NOT NULL CHECK(records>=1),bytes INTEGER NOT NULL CHECK(bytes>=0))",
    ),
];

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Entry {
    Genesis {
        format: u32,
        purpose: String,
        installation_id: Uuid,
        namespace_id: Uuid,
        policy: AuthenticationAuditPolicyV1,
    },
    Authentication {
        record: AuthenticationRecordV1,
    },
    Session {
        mutation: Box<SessionMutation>,
    },
}

/// The audit record and its session transition are one closed, hash-chained
/// payload. There is no independently writable session projection to trust.
#[derive(Serialize, Deserialize)]
#[serde(tag = "transition", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum SessionMutation {
    Admit {
        record: AuthenticationRecordV1,
        binding: AuthenticationSessionBindingV1,
    },
    RefreshPending {
        record: AuthenticationRecordV1,
        expected_generation: u32,
        expected_access_fingerprint: AuthenticationTokenFingerprintV1,
    },
    Rotate {
        record: AuthenticationRecordV1,
        binding: AuthenticationSessionBindingV1,
        expected_generation: u32,
        expected_access_fingerprint: AuthenticationTokenFingerprintV1,
    },
    Close {
        record: AuthenticationRecordV1,
        expected_generation: u32,
        expected_access_fingerprint: AuthenticationTokenFingerprintV1,
    },
}

impl SessionMutation {
    pub fn record(&self) -> &AuthenticationRecordV1 {
        match self {
            Self::Admit { record, .. }
            | Self::RefreshPending { record, .. }
            | Self::Rotate { record, .. }
            | Self::Close { record, .. } => record,
        }
    }
}

impl Entry {
    pub fn record(&self) -> Option<&AuthenticationRecordV1> {
        match self {
            Self::Authentication { record } => Some(record),
            Self::Session { mutation } => Some(mutation.record()),
            Self::Genesis { .. } => None,
        }
    }
}

pub(super) struct Replay {
    pub verified: AuthenticationVerificationV1,
    phases: BTreeMap<(Uuid, u8), (String, AuthenticationReceiptV1)>,
    intentions: BTreeMap<Uuid, (AuthenticationRecordV1, AuthenticationReceiptV1)>,
    receipts: BTreeMap<u64, AuthenticationReceiptV1>,
    sessions: BTreeMap<Uuid, AuthenticationSessionCurrentV1>,
    fingerprints: BTreeSet<AuthenticationTokenFingerprintV1>,
}

pub(super) fn validate_policy(
    installation_id: Uuid,
    policy: &AuthenticationAuditPolicyV1,
    redactor: &Redactor,
) -> Result<()> {
    ensure!(
        !installation_id.is_nil(),
        AuthenticationAuditFailureV1::InvalidInput
    );
    ensure!(
        policy.max_records > 0
            && policy.max_records <= i64::MAX as u64
            && policy.max_serialized_bytes > 0
            && policy.max_serialized_bytes <= i64::MAX as u64
            && !policy.approved_clients.is_empty()
            && policy.approved_clients.len() <= 64,
        AuthenticationAuditFailureV1::InvalidInput
    );
    let mut unique = BTreeSet::new();
    for client in &policy.approved_clients {
        validate_client(&client.issuer, &client.client_id)?;
        ensure!(
            unique.insert((&client.issuer, &client.client_id)),
            AuthenticationAuditFailureV1::InvalidInput
        );
    }
    ensure!(
        checked_json(policy, redactor)?.len() <= 16_384,
        AuthenticationAuditFailureV1::InvalidInput
    );
    // Genesis persists both UUIDs in canonical 36-byte form. Sizing with a
    // non-nil placeholder namespace is exact, before storage creation. The
    // empty redactor is intentional: the future random namespace must not
    // inherit a spurious redaction collision from the placeholder value.
    genesis_payload(
        installation_id,
        Uuid::from_u128(1),
        policy,
        &Redactor::new(vec![]),
    )?;
    Ok(())
}

fn bounded_text(value: &str, max: usize) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control),
        AuthenticationAuditFailureV1::InvalidInput
    );
    Ok(())
}

fn validate_client(issuer: &str, client_id: &str) -> Result<()> {
    bounded_text(issuer, 2048)?;
    bounded_text(client_id, 512)?;
    let principal = PrincipalRefV1::HumanOidc {
        issuer: issuer.to_owned(),
        subject: "validation".into(),
    };
    principal
        .validate()
        .map_err(|_| AuthenticationAuditFailureV1::InvalidInput)?;
    Ok(())
}

/// Reject a redaction collision instead of changing a signed identity. Compare
/// decoded strings so JSON escaping cannot hide a literal known secret.
pub(super) fn checked_json<T: Serialize>(value: &T, redactor: &Redactor) -> Result<String> {
    let value = serde_json::to_value(value)?;
    ensure!(
        redactor.value_unclipped(&value) == value,
        AuthenticationAuditFailureV1::InvalidInput
    );
    let payload = serde_json::to_string(&value)?;
    ensure!(
        payload.len() <= MAX_PAYLOAD_BYTES,
        AuthenticationAuditFailureV1::InvalidInput
    );
    Ok(payload)
}

pub(super) fn validate_record(
    record: &AuthenticationRecordV1,
    policy: &AuthenticationAuditPolicyV1,
) -> Result<()> {
    use AuthenticationActionV1 as A;
    use AuthenticationOutcomeV1 as O;
    use AuthenticationPhaseV1 as P;
    use AuthenticationReasonV1 as R;
    ensure!(
        !record.operation_id.is_nil()
            && record.transaction_id.is_none_or(|id| !id.is_nil())
            && record.session_id.is_none_or(|id| !id.is_nil())
            && policy.approved_clients.iter().any(
                |client| client.issuer == record.issuer && client.client_id == record.client_id
            ),
        AuthenticationAuditFailureV1::InvalidInput
    );
    validate_client(&record.issuer, &record.client_id)?;
    if let Some(principal) = &record.principal {
        ensure!(
            matches!(principal, PrincipalRefV1::HumanOidc { issuer, .. } if issuer == &record.issuer),
            AuthenticationAuditFailureV1::InvalidInput
        );
        principal
            .validate()
            .map_err(|_| AuthenticationAuditFailureV1::InvalidInput)?;
    }
    if let Some(receipt) = &record.causal {
        ensure!(
            !receipt.namespace_id.is_nil() && receipt.sequence > 0 && valid_hash(&receipt.hash),
            AuthenticationAuditFailureV1::InvalidInput
        );
    }
    let initial = matches!(record.action, A::Login | A::SessionAdmission);
    match record.phase {
        P::Intention => {
            ensure!(
                if initial {
                    record.principal.is_none()
                } else {
                    record.principal.is_some() && record.session_id.is_some()
                },
                AuthenticationAuditFailureV1::InvalidInput
            );
        }
        P::Result { outcome, reason } => {
            let allowed = match (outcome, reason) {
                (O::Succeeded, R::Verified) | (O::Rejected, R::NoMembership) => {
                    matches!(record.action, A::Login | A::SessionAdmission | A::Refresh)
                        && record.principal.is_some()
                }
                (O::Succeeded, R::UserRequested) => {
                    record.action == A::Logout && record.principal.is_some()
                }
                (O::Succeeded, R::Expired) => {
                    record.action == A::Expiration && record.principal.is_some()
                }
                (O::Succeeded, R::Revoked) => {
                    matches!(record.action, A::Logout | A::Expiration) && record.principal.is_some()
                }
                (
                    O::Rejected,
                    R::InvalidResponse | R::ProviderRejected | R::Expired | R::Revoked,
                ) => record.action != A::Expiration && (!initial || record.principal.is_none()),
                (O::Failed | O::Unknown, R::ProviderUnavailable) => {
                    record.action != A::Expiration && (!initial || record.principal.is_none())
                }
                (O::Unknown, R::Interrupted) => !initial || record.principal.is_none(),
                _ => false,
            };
            ensure!(allowed, AuthenticationAuditFailureV1::InvalidInput);
        }
    }
    Ok(())
}

fn validate_binding(
    binding: &AuthenticationSessionBindingV1,
    policy: &AuthenticationAuditPolicyV1,
) -> Result<()> {
    ensure!(
        !binding.session_id.is_nil()
            && binding.generation > 0
            && binding.expires_at_unix_seconds > 0
            && binding.access_fingerprint.domain == FingerprintDomainV1::Access
            && binding
                .refresh_fingerprint
                .is_none_or(|fingerprint| fingerprint.domain == FingerprintDomainV1::Refresh)
            && matches!(&binding.principal, PrincipalRefV1::HumanOidc { issuer, .. } if issuer == &binding.issuer)
            && policy
                .approved_clients
                .iter()
                .any(|client| client.issuer == binding.issuer
                    && client.client_id == binding.client_id),
        AuthenticationAuditFailureV1::InvalidInput
    );
    validate_client(&binding.issuer, &binding.client_id)?;
    binding
        .principal
        .validate()
        .map_err(|_| AuthenticationAuditFailureV1::InvalidInput)?;
    Ok(())
}

fn record_matches_binding(
    record: &AuthenticationRecordV1,
    binding: &AuthenticationSessionBindingV1,
) -> Result<()> {
    ensure!(
        record.session_id == Some(binding.session_id)
            && record.principal.as_ref() == Some(&binding.principal)
            && record.issuer == binding.issuer
            && record.client_id == binding.client_id,
        AuthenticationAuditFailureV1::PhaseOrder
    );
    Ok(())
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) fn phase_key(phase: AuthenticationPhaseV1) -> (&'static str, u8) {
    match phase {
        AuthenticationPhaseV1::Intention => ("intention", 0),
        AuthenticationPhaseV1::Result { .. } => ("result", 1),
    }
}

pub(super) fn digest(seq: u64, received: &str, previous: &str, payload: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"ai-guard:authentication-audit:v1\0");
    hash.update(seq.to_be_bytes());
    for value in [received, previous, payload] {
        hash.update(value.as_bytes());
        hash.update([0]);
    }
    hex::encode(hash.finalize())
}

pub(super) fn validate_schema(db: &Connection) -> Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let application: i64 = db.pragma_query_value(None, "application_id", |row| row.get(0))?;
    ensure!(
        version == VERSION && application == APPLICATION_ID,
        AuthenticationAuditFailureV1::Format
    );
    let mut statement = db.prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema")?;
    let objects = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(objects.len() == 5, AuthenticationAuditFailureV1::Format);
    for (kind, name, table, sql) in objects {
        if let Some((_, expected)) = SCHEMA.iter().find(|(expected, _)| *expected == name) {
            ensure!(
                kind == "table"
                    && table == name
                    && sql.is_some_and(|sql| super::super::normalized_schema(&sql)
                        == super::super::normalized_schema(expected)),
                AuthenticationAuditFailureV1::Format
            );
        } else {
            ensure!(
                kind == "index"
                    && table == "authentication_entries"
                    && matches!(
                        name.as_str(),
                        "sqlite_autoindex_authentication_entries_1"
                            | "sqlite_autoindex_authentication_entries_2"
                    )
                    && sql.is_none(),
                AuthenticationAuditFailureV1::Format
            );
        }
    }
    Ok(())
}

fn genesis_payload(
    installation_id: Uuid,
    namespace_id: Uuid,
    policy: &AuthenticationAuditPolicyV1,
    redactor: &Redactor,
) -> Result<String> {
    let entry = Entry::Genesis {
        format: 1,
        purpose: PURPOSE.into(),
        installation_id,
        namespace_id,
        policy: policy.clone(),
    };
    let payload = checked_json(&entry, redactor)?;
    ensure!(
        payload.len() as u64 <= policy.max_serialized_bytes,
        AuthenticationAuditFailureV1::Capacity
    );
    Ok(payload)
}

pub(super) fn initialize(
    db: &Connection,
    installation_id: Uuid,
    namespace_id: Uuid,
    policy: &AuthenticationAuditPolicyV1,
    redactor: &Redactor,
) -> Result<()> {
    let payload = genesis_payload(installation_id, namespace_id, policy, redactor)?;
    let received = timestamp();
    let hash = digest(1, &received, ZERO_HASH, &payload);
    for (_, sql) in SCHEMA {
        db.execute_batch(sql)?;
    }
    db.execute("INSERT INTO authentication_meta(singleton,purpose,installation_id,namespace_uuid,genesis_hash) VALUES(1,?1,?2,?3,?4)", params![PURPOSE, installation_id.to_string(), namespace_id.to_string(), hash])?;
    db.execute("INSERT INTO authentication_entries(seq,received_at,prev_hash,hash,payload) VALUES(1,?1,?2,?3,?4)", params![received, ZERO_HASH, hash, payload])?;
    db.execute(
        "INSERT INTO authentication_quota(singleton,records,bytes) VALUES(1,1,?1)",
        [payload.len() as i64],
    )?;
    db.pragma_update(None, "application_id", APPLICATION_ID)?;
    db.pragma_update(None, "user_version", VERSION)?;
    Ok(())
}

pub(super) fn timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

impl Replay {
    pub fn session_current(&self, session_id: Uuid) -> Option<AuthenticationSessionCurrentV1> {
        self.sessions.get(&session_id).cloned()
    }

    pub fn fingerprint_seen(&self, fingerprint: &AuthenticationTokenFingerprintV1) -> bool {
        self.fingerprints.contains(fingerprint)
    }

    /// A tracked lifecycle phase must update its session in the same entry.
    /// Otherwise a plain record would consume the unique phase while leaving
    /// an Active/Pending projection that can no longer complete its transition.
    pub fn validate_generic_lane(&self, record: &AuthenticationRecordV1) -> Result<()> {
        let tracked = record
            .session_id
            .is_some_and(|id| self.sessions.contains_key(&id));
        let requires_transition = matches!(record.action, AuthenticationActionV1::Refresh)
            || (matches!(
                record.action,
                AuthenticationActionV1::Logout | AuthenticationActionV1::Expiration
            ) && matches!(record.phase, AuthenticationPhaseV1::Result { .. }));
        ensure!(
            !tracked || !requires_transition,
            AuthenticationAuditFailureV1::PhaseOrder
        );
        Ok(())
    }

    fn current_for(
        &self,
        record: &AuthenticationRecordV1,
        expected_generation: u32,
        expected_access_fingerprint: AuthenticationTokenFingerprintV1,
    ) -> Result<&AuthenticationSessionCurrentV1> {
        ensure!(
            expected_generation > 0
                && expected_access_fingerprint.domain == FingerprintDomainV1::Access,
            AuthenticationAuditFailureV1::InvalidInput
        );
        let current = record
            .session_id
            .and_then(|id| self.sessions.get(&id))
            .ok_or(AuthenticationAuditFailureV1::PhaseOrder)?;
        record_matches_binding(record, &current.binding)?;
        ensure!(
            current.binding.generation == expected_generation
                && current.binding.access_fingerprint == expected_access_fingerprint,
            AuthenticationAuditFailureV1::Conflict
        );
        Ok(current)
    }

    /// Exact replay is an observation, never a second exchange grant or a
    /// reactivation of a subsequently advanced/closed session.
    pub fn validate_mutation_replay(
        &self,
        mutation: &SessionMutation,
        receipt: &AuthenticationReceiptV1,
    ) -> Result<()> {
        match mutation {
            SessionMutation::RefreshPending {
                record,
                expected_generation,
                expected_access_fingerprint,
            } => {
                let current =
                    self.current_for(record, *expected_generation, *expected_access_fingerprint)?;
                ensure!(
                    matches!(&current.state, AuthenticationSessionStateV1::RefreshPending { operation_id, intention } if *operation_id == record.operation_id && intention == receipt),
                    AuthenticationAuditFailureV1::PhaseOrder
                );
            }
            SessionMutation::Admit { binding, .. } | SessionMutation::Rotate { binding, .. } => {
                ensure!(
                    self.sessions
                        .get(&binding.session_id)
                        .is_some_and(|current| current.binding == *binding
                            && current.state == AuthenticationSessionStateV1::Active),
                    AuthenticationAuditFailureV1::PhaseOrder
                );
            }
            SessionMutation::Close { .. } => {}
        }
        Ok(())
    }

    pub fn validate_mutation(
        &self,
        mutation: &SessionMutation,
        policy: &AuthenticationAuditPolicyV1,
    ) -> Result<()> {
        use AuthenticationActionV1 as A;
        use AuthenticationOutcomeV1 as O;
        use AuthenticationPhaseV1 as P;
        use AuthenticationReasonV1 as R;
        match mutation {
            SessionMutation::Admit { record, binding } => {
                validate_binding(binding, policy)?;
                record_matches_binding(record, binding)?;
                ensure!(
                    binding.generation == 1
                        && matches!(
                            (binding.kind, record.action),
                            (OidcSessionKindV1::WebPkce, A::Login)
                                | (OidcSessionKindV1::NativeCli, A::SessionAdmission)
                        )
                        && record.phase
                            == (P::Result {
                                outcome: O::Succeeded,
                                reason: R::Verified
                            })
                        && !self.sessions.contains_key(&binding.session_id)
                        && !self.fingerprints.contains(&binding.access_fingerprint),
                    AuthenticationAuditFailureV1::PhaseOrder
                );
            }
            SessionMutation::RefreshPending {
                record,
                expected_generation,
                expected_access_fingerprint,
            } => {
                let current =
                    self.current_for(record, *expected_generation, *expected_access_fingerprint)?;
                ensure!(
                    record.action == A::Refresh
                        && record.phase == P::Intention
                        && current.state == AuthenticationSessionStateV1::Active,
                    AuthenticationAuditFailureV1::PhaseOrder
                );
            }
            SessionMutation::Rotate {
                record,
                binding,
                expected_generation,
                expected_access_fingerprint,
            } => {
                validate_binding(binding, policy)?;
                let current =
                    self.current_for(record, *expected_generation, *expected_access_fingerprint)?;
                let old = &current.binding;
                ensure!(
                    record.action == A::Refresh
                        && record.phase
                            == (P::Result {
                                outcome: O::Succeeded,
                                reason: R::Verified
                            })
                        && matches!(&current.state, AuthenticationSessionStateV1::RefreshPending { operation_id, intention } if *operation_id == record.operation_id && record.causal.as_ref() == Some(intention))
                        && binding.session_id == old.session_id
                        && binding.principal == old.principal
                        && binding.issuer == old.issuer
                        && binding.client_id == old.client_id
                        && binding.kind == old.kind
                        && old.generation.checked_add(1) == Some(binding.generation)
                        && !self.fingerprints.contains(&binding.access_fingerprint),
                    AuthenticationAuditFailureV1::PhaseOrder
                );
            }
            SessionMutation::Close {
                record,
                expected_generation,
                expected_access_fingerprint,
            } => {
                let current =
                    self.current_for(record, *expected_generation, *expected_access_fingerprint)?;
                let P::Result { outcome, .. } = record.phase else {
                    return Err(AuthenticationAuditFailureV1::PhaseOrder.into());
                };
                ensure!(
                    !matches!(current.state, AuthenticationSessionStateV1::Closed { .. })
                        && (matches!(record.action, A::Logout | A::Expiration)
                            || (record.action == A::Refresh
                                && outcome != O::Succeeded
                                && matches!(&current.state, AuthenticationSessionStateV1::RefreshPending { operation_id, intention } if *operation_id == record.operation_id && record.causal.as_ref() == Some(intention)))),
                    AuthenticationAuditFailureV1::PhaseOrder
                );
            }
        }
        Ok(())
    }

    fn accept_mutation(
        &mut self,
        mutation: SessionMutation,
        payload: String,
        receipt: AuthenticationReceiptV1,
        policy: &AuthenticationAuditPolicyV1,
    ) -> Result<()> {
        self.validate_transition(mutation.record())?;
        self.validate_mutation(&mutation, policy)?;
        // All checks precede projection changes. Replaying a damaged file
        // cannot partially authorize a binding or fingerprint.
        match &mutation {
            SessionMutation::Admit { binding, .. } | SessionMutation::Rotate { binding, .. } => {
                self.fingerprints.insert(binding.access_fingerprint);
                if let Some(fingerprint) = binding.refresh_fingerprint {
                    self.fingerprints.insert(fingerprint);
                }
                self.sessions.insert(
                    binding.session_id,
                    AuthenticationSessionCurrentV1 {
                        binding: binding.clone(),
                        state: AuthenticationSessionStateV1::Active,
                    },
                );
            }
            SessionMutation::RefreshPending { record, .. } => {
                let current = self
                    .sessions
                    .get_mut(
                        &record
                            .session_id
                            .ok_or(AuthenticationAuditFailureV1::Integrity)?,
                    )
                    .ok_or(AuthenticationAuditFailureV1::Integrity)?;
                current.state = AuthenticationSessionStateV1::RefreshPending {
                    operation_id: record.operation_id,
                    intention: receipt.clone(),
                };
            }
            SessionMutation::Close { record, .. } => {
                let AuthenticationPhaseV1::Result { reason, .. } = record.phase else {
                    return Err(AuthenticationAuditFailureV1::Integrity.into());
                };
                let current = self
                    .sessions
                    .get_mut(
                        &record
                            .session_id
                            .ok_or(AuthenticationAuditFailureV1::Integrity)?,
                    )
                    .ok_or(AuthenticationAuditFailureV1::Integrity)?;
                current.state = AuthenticationSessionStateV1::Closed { reason };
            }
        }
        let record = match mutation {
            SessionMutation::Admit { record, .. }
            | SessionMutation::RefreshPending { record, .. }
            | SessionMutation::Rotate { record, .. }
            | SessionMutation::Close { record, .. } => record,
        };
        self.accept(record, payload, receipt)
    }

    pub fn existing(
        &self,
        record: &AuthenticationRecordV1,
        payload: &str,
    ) -> Result<Option<AuthenticationReceiptV1>> {
        match self
            .phases
            .get(&(record.operation_id, phase_key(record.phase).1))
        {
            Some((old, receipt)) => {
                ensure!(old == payload, AuthenticationAuditFailureV1::Conflict);
                Ok(Some(receipt.clone()))
            }
            None => Ok(None),
        }
    }

    pub fn validate_transition(&self, record: &AuthenticationRecordV1) -> Result<()> {
        match record.phase {
            AuthenticationPhaseV1::Intention => {
                ensure!(
                    !self.intentions.contains_key(&record.operation_id),
                    AuthenticationAuditFailureV1::PhaseOrder
                );
                if let Some(causal) = &record.causal {
                    ensure!(
                        causal.sequence > 1 && self.receipts.get(&causal.sequence) == Some(causal),
                        AuthenticationAuditFailureV1::PhaseOrder
                    );
                }
            }
            AuthenticationPhaseV1::Result { .. } => {
                let Some((intention, receipt)) = self.intentions.get(&record.operation_id) else {
                    return Err(AuthenticationAuditFailureV1::PhaseOrder.into());
                };
                ensure!(
                    record.causal.as_ref() == Some(receipt)
                        && record.action == intention.action
                        && record.issuer == intention.issuer
                        && record.client_id == intention.client_id
                        && record.transaction_id == intention.transaction_id
                        && record.session_id == intention.session_id
                        && intention
                            .principal
                            .as_ref()
                            .is_none_or(|principal| record.principal.as_ref() == Some(principal)),
                    AuthenticationAuditFailureV1::PhaseOrder
                );
            }
        }
        Ok(())
    }

    fn accept(
        &mut self,
        record: AuthenticationRecordV1,
        payload: String,
        receipt: AuthenticationReceiptV1,
    ) -> Result<()> {
        ensure!(
            !self
                .phases
                .contains_key(&(record.operation_id, phase_key(record.phase).1)),
            AuthenticationAuditFailureV1::Integrity
        );
        self.validate_transition(&record)?;
        self.phases.insert(
            (record.operation_id, phase_key(record.phase).1),
            (payload, receipt.clone()),
        );
        if record.phase == AuthenticationPhaseV1::Intention {
            self.intentions
                .insert(record.operation_id, (record, receipt.clone()));
        }
        self.receipts.insert(receipt.sequence, receipt);
        Ok(())
    }
}

/// Run inside one SQLite snapshot. Replays every typed record and checks both
/// projections and durable counters; a valid hash alone never grants a phase.
pub(super) fn replay(
    db: &Connection,
    installation_id: Uuid,
    policy: &AuthenticationAuditPolicyV1,
    redactor: &Redactor,
    floor: Option<&AuthenticationReceiptV1>,
) -> Result<Replay> {
    validate_schema(db)?;
    let meta_count: i64 = db.query_row("SELECT count(*) FROM authentication_meta", [], |row| {
        row.get(0)
    })?;
    let meta_lengths: (i64, i64, i64, i64) = db.query_row("SELECT length(CAST(purpose AS BLOB)),length(CAST(installation_id AS BLOB)),length(CAST(namespace_uuid AS BLOB)),length(CAST(genesis_hash AS BLOB)) FROM authentication_meta WHERE singleton=1", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?;
    ensure!(
        meta_count == 1 && meta_lengths == (PURPOSE.len() as i64, 36, 36, 64),
        AuthenticationAuditFailureV1::Format
    );
    let (purpose, installation, namespace, genesis): (String, String, String, String) = db.query_row("SELECT purpose,installation_id,namespace_uuid,genesis_hash FROM authentication_meta WHERE singleton=1", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?;
    let namespace_id =
        Uuid::parse_str(&namespace).map_err(|_| AuthenticationAuditFailureV1::Format)?;
    ensure!(
        meta_count == 1
            && purpose == PURPOSE
            && installation == installation_id.to_string()
            && !namespace_id.is_nil()
            && namespace_id.to_string() == namespace
            && valid_hash(&genesis),
        AuthenticationAuditFailureV1::Format
    );
    let mut replay = Replay {
        verified: AuthenticationVerificationV1 {
            namespace_id,
            records: 0,
            serialized_bytes: 0,
            head: AuthenticationReceiptV1 {
                namespace_id,
                sequence: 0,
                hash: ZERO_HASH.into(),
            },
        },
        phases: BTreeMap::new(),
        intentions: BTreeMap::new(),
        receipts: BTreeMap::new(),
        sessions: BTreeMap::new(),
        fingerprints: BTreeSet::new(),
    };
    let mut statement = db.prepare("SELECT seq,received_at,prev_hash,hash,length(CAST(payload AS BLOB)),payload,operation_id,phase,length(CAST(received_at AS BLOB)),length(CAST(prev_hash AS BLOB)),length(CAST(hash AS BLOB)),length(CAST(operation_id AS BLOB)),length(CAST(phase AS BLOB)) FROM authentication_entries ORDER BY seq")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let seq: i64 = row.get(0)?;
        // Inspect SQL lengths before allocating strings from a damaged file.
        let received_len: i64 = row.get(8)?;
        let previous_len: i64 = row.get(9)?;
        let hash_len: i64 = row.get(10)?;
        let operation_len: Option<i64> = row.get(11)?;
        let phase_len: Option<i64> = row.get(12)?;
        ensure!(
            (20..=40).contains(&received_len)
                && previous_len == 64
                && hash_len == 64
                && operation_len.is_none_or(|len| len == 36)
                && phase_len.is_none_or(|len| (1..=9).contains(&len)),
            AuthenticationAuditFailureV1::Integrity
        );
        let received: String = row.get(1)?;
        let previous: String = row.get(2)?;
        let hash: String = row.get(3)?;
        let length: i64 = row.get(4)?;
        ensure!(
            length >= 0 && length <= MAX_PAYLOAD_BYTES as i64,
            AuthenticationAuditFailureV1::Integrity
        );
        let payload: String = row.get(5)?;
        let projected_operation: Option<String> = row.get(6)?;
        let projected_phase: Option<String> = row.get(7)?;
        let expected = replay
            .verified
            .records
            .checked_add(1)
            .ok_or(AuthenticationAuditFailureV1::Integrity)?;
        let bytes = replay
            .verified
            .serialized_bytes
            .checked_add(payload.len() as u64)
            .ok_or(AuthenticationAuditFailureV1::Integrity)?;
        ensure!(
            seq > 0
                && seq as u64 == expected
                && expected <= policy.max_records
                && bytes <= policy.max_serialized_bytes
                && previous == replay.verified.head.hash
                && hash == digest(expected, &received, &previous, &payload),
            AuthenticationAuditFailureV1::Integrity
        );
        let time = chrono::DateTime::parse_from_rfc3339(&received)
            .map_err(|_| AuthenticationAuditFailureV1::Integrity)?;
        ensure!(
            time.with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
                == received,
            AuthenticationAuditFailureV1::Integrity
        );
        let entry: Entry =
            serde_json::from_str(&payload).map_err(|_| AuthenticationAuditFailureV1::Integrity)?;
        ensure!(
            checked_json(&entry, redactor)? == payload,
            AuthenticationAuditFailureV1::Integrity
        );
        let receipt = AuthenticationReceiptV1 {
            namespace_id,
            sequence: expected,
            hash: hash.clone(),
        };
        match entry {
            Entry::Genesis {
                format,
                purpose,
                installation_id: bound,
                namespace_id: bound_namespace,
                policy: bound_policy,
            } => {
                ensure!(
                    seq == 1
                        && format == 1
                        && purpose == PURPOSE
                        && bound == installation_id
                        && bound_namespace == namespace_id
                        && bound_policy == *policy
                        && hash == genesis
                        && projected_operation.is_none()
                        && projected_phase.is_none(),
                    AuthenticationAuditFailureV1::Integrity
                );
                replay.receipts.insert(1, receipt.clone());
            }
            Entry::Authentication { record } => {
                ensure!(
                    seq > 1
                        && projected_operation.as_deref()
                            == Some(record.operation_id.to_string().as_str())
                        && projected_phase.as_deref() == Some(phase_key(record.phase).0),
                    AuthenticationAuditFailureV1::Integrity
                );
                validate_record(&record, policy)?;
                replay.validate_generic_lane(&record)?;
                replay.accept(record, payload, receipt.clone())?;
            }
            Entry::Session { mutation } => {
                let record = mutation.record();
                ensure!(
                    seq > 1
                        && projected_operation.as_deref()
                            == Some(record.operation_id.to_string().as_str())
                        && projected_phase.as_deref() == Some(phase_key(record.phase).0),
                    AuthenticationAuditFailureV1::Integrity
                );
                validate_record(record, policy)?;
                replay.accept_mutation(*mutation, payload, receipt.clone(), policy)?;
            }
        }
        replay.verified.records = expected;
        replay.verified.serialized_bytes = bytes;
        replay.verified.head = receipt;
    }
    ensure!(
        replay.verified.records > 0,
        AuthenticationAuditFailureV1::Integrity
    );
    let quota_rows: i64 = db.query_row("SELECT count(*) FROM authentication_quota", [], |row| {
        row.get(0)
    })?;
    let quota: (i64, i64) = db.query_row(
        "SELECT records,bytes FROM authentication_quota WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    ensure!(
        quota_rows == 1
            && quota
                == (
                    replay.verified.records as i64,
                    replay.verified.serialized_bytes as i64
                ),
        AuthenticationAuditFailureV1::Integrity
    );
    if let Some(floor) = floor {
        ensure!(
            replay.receipts.get(&floor.sequence) == Some(floor),
            AuthenticationAuditFailureV1::Integrity
        );
    }
    Ok(replay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_capacity_accepts_exact_serialized_boundary_and_rejects_one_byte_less() {
        let installation = Uuid::from_u128(0x11111111111141118111111111111111);
        let namespace = Uuid::from_u128(0x22222222222242228222222222222222);
        let redactor = Redactor::new(vec![]);
        let mut policy = AuthenticationAuditPolicyV1 {
            max_records: 1,
            max_serialized_bytes: 512,
            approved_clients: vec![ApprovedAuthenticationClientV1 {
                issuer: "https://identity.example.test/realm".into(),
                client_id: "public-web".into(),
            }],
        };
        // Independent wire-shape oracle, including the capacity's own decimal
        // digits. Stabilize that value to obtain an exact byte boundary.
        let wire = |policy: &AuthenticationAuditPolicyV1| {
            serde_json::json!({
                "kind":"genesis", "format":1, "purpose":"authentication_audit_v1",
                "installation_id":installation, "namespace_id":namespace, "policy":policy
            })
            .to_string()
        };
        for _ in 0..4 {
            let required = wire(&policy).len() as u64;
            if required == policy.max_serialized_bytes {
                break;
            }
            policy.max_serialized_bytes = required;
        }
        assert_eq!(wire(&policy).len() as u64, policy.max_serialized_bytes);
        validate_policy(installation, &policy, &redactor).unwrap();
        assert_eq!(
            genesis_payload(installation, namespace, &policy, &redactor).unwrap(),
            wire(&policy)
        );
        let boundary = policy.max_serialized_bytes;
        policy.max_serialized_bytes -= 1;
        assert_eq!(wire(&policy).len() as u64, boundary);
        let error = validate_policy(installation, &policy, &redactor).unwrap_err();
        assert_eq!(
            error.downcast_ref::<AuthenticationAuditFailureV1>(),
            Some(&AuthenticationAuditFailureV1::Capacity)
        );
    }
}
