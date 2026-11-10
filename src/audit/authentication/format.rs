//! Closed authentication format and replay; no tenant or execution metadata.
use super::*;
use anyhow::{Result, ensure};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const LEGACY_VERSION: i64 = 1_000_001;
pub(super) const VERIFIED_INITIAL_VERSION: i64 = 1_000_002;
pub(super) const VERSION: i64 = 1_000_003;
pub(super) const TENANT_CAPACITY_VERSION: i64 = 1_000_004;
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

#[derive(Clone, Serialize, Deserialize)]
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
    /// V2 only. The verifier's identity and consumed proof are committed with
    /// the initial intention, before any membership lookup or session install.
    VerifiedInitial {
        record: AuthenticationRecordV1,
        principal: PrincipalRefV1,
        access_fingerprint: AuthenticationTokenFingerprintV1,
        membership_recovery: bool,
    },
    Session {
        mutation: Box<SessionMutation>,
    },
    CapacityPlan {
        plan: AuthenticationCapacityPlanV1,
    },
    Charged {
        charge: budget::CapacityOwner,
        entry: Box<Entry>,
    },
}

/// The audit record and its session transition are one closed, hash-chained
/// payload. There is no independently writable session projection to trust.
#[derive(Clone, Serialize, Deserialize)]
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
    BeginNativeLogoutDuringRefresh {
        record: AuthenticationRecordV1,
        expected_binding: AuthenticationSessionBindingV1,
        refresh_operation_id: Uuid,
        refresh_intention: AuthenticationReceiptV1,
    },
    RecordNativeLogoutAfterRefreshClosed {
        record: AuthenticationRecordV1,
        expected_binding: AuthenticationSessionBindingV1,
        refresh_operation_id: Uuid,
        refresh_intention: AuthenticationReceiptV1,
        refresh_result: AuthenticationReceiptV1,
    },
}

impl SessionMutation {
    pub fn record(&self) -> &AuthenticationRecordV1 {
        match self {
            Self::Admit { record, .. }
            | Self::RefreshPending { record, .. }
            | Self::Rotate { record, .. }
            | Self::Close { record, .. }
            | Self::BeginNativeLogoutDuringRefresh { record, .. }
            | Self::RecordNativeLogoutAfterRefreshClosed { record, .. } => record,
        }
    }
}

impl Entry {
    pub fn record(&self) -> Option<&AuthenticationRecordV1> {
        match self {
            Self::Authentication { record } | Self::VerifiedInitial { record, .. } => Some(record),
            Self::Session { mutation } => Some(mutation.record()),
            Self::Charged { entry, .. } => entry.record(),
            Self::Genesis { .. } | Self::CapacityPlan { .. } => None,
        }
    }
}

pub(super) struct Replay {
    pub version: i64,
    pub verified: AuthenticationVerificationV1,
    phases: BTreeMap<(Uuid, u8), (String, AuthenticationReceiptV1)>,
    intentions: BTreeMap<Uuid, (AuthenticationRecordV1, AuthenticationReceiptV1)>,
    receipts: BTreeMap<u64, AuthenticationReceiptV1>,
    sessions: BTreeMap<Uuid, AuthenticationSessionCurrentV1>,
    fingerprints: BTreeSet<AuthenticationTokenFingerprintV1>,
    initial_proofs: BTreeMap<Uuid, InitialProof>,
    // Rebuilt from validated initial entries in this replay, never persisted
    // separately or accepted as an independent authority.
    initial_fingerprints: BTreeSet<AuthenticationTokenFingerprintV1>,
    subject_usage: BTreeMap<PrincipalRefV1, (u64, u64)>,
    // Only a validated V3 BEGIN creates a promise; only its END consumes it.
    // One entry per durable session, bounded by the same record/byte policy.
    native_logouts: BTreeMap<Uuid, NativeLogoutPromise>,
    pub budget: budget::BudgetReplay,
}

struct NativeLogoutPromise {
    logout_operation_id: Uuid,
    logout_intention: AuthenticationReceiptV1,
    refresh_operation_id: Uuid,
    refresh_intention: AuthenticationReceiptV1,
}

struct InitialProof {
    principal: PrincipalRefV1,
    fingerprint: AuthenticationTokenFingerprintV1,
    reserved_bytes: u64,
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

pub(super) fn schema_version(db: &Connection) -> Result<i64> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let application: i64 = db.pragma_query_value(None, "application_id", |row| row.get(0))?;
    ensure!(
        matches!(
            version,
            LEGACY_VERSION | VERIFIED_INITIAL_VERSION | VERSION | TENANT_CAPACITY_VERSION
        ) && application == APPLICATION_ID,
        AuthenticationAuditFailureV1::Format
    );
    Ok(version)
}

pub(super) fn validate_schema(db: &Connection) -> Result<()> {
    schema_version(db)?;
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
    /// Admission-only accounting, rebuilt from the validated durable session
    /// states. Historical underfunded Pending entries remain readable; they
    /// never justify admitting another operation into their terminal budget.
    pub fn check_terminal_capacity(
        &self,
        entry: &Entry,
        records: u64,
        bytes: u64,
        policy: &AuthenticationAuditPolicyV1,
    ) -> Result<()> {
        if self.budget.plan.is_some() {
            // V4 validates every charge and historical ticket separately.
            // There is no second, transferable global reservation pool.
            ensure!(
                records <= policy.max_records && bytes <= policy.max_serialized_bytes,
                AuthenticationAuditFailureV1::Capacity
            );
            return Ok(());
        }
        self.check_terminal_capacity_for_mutation(
            match entry {
                Entry::Session { mutation } => Some(mutation.as_ref()),
                _ => None,
            },
            records,
            bytes,
            policy,
        )
    }

    fn check_terminal_capacity_for_mutation(
        &self,
        mutation: Option<&SessionMutation>,
        records: u64,
        bytes: u64,
        policy: &AuthenticationAuditPolicyV1,
    ) -> Result<()> {
        let pending = u64::try_from(
            self.sessions
                .values()
                .filter(|current| {
                    matches!(
                        current.state,
                        AuthenticationSessionStateV1::RefreshPending { .. }
                    )
                })
                .count(),
        )
        .map_err(|_| AuthenticationAuditFailureV1::Capacity)?;
        // append_entry has already validated the exact CAS, phase and causal
        // receipt. Only an actual transition out of this Pending consumes its
        // single allowance; exact replays return before this admission check.
        let remaining = match mutation {
            Some(mutation) => match mutation {
                SessionMutation::RefreshPending { .. } => pending
                    .checked_add(1)
                    .ok_or(AuthenticationAuditFailureV1::Capacity)?,
                SessionMutation::Rotate { record, .. } | SessionMutation::Close { record, .. }
                    if record
                        .session_id
                        .and_then(|id| self.sessions.get(&id))
                        .is_some_and(|current| {
                            matches!(
                                current.state,
                                AuthenticationSessionStateV1::RefreshPending { .. }
                            )
                        }) =>
                {
                    pending
                        .checked_sub(1)
                        .ok_or(AuthenticationAuditFailureV1::Integrity)?
                }
                _ => pending,
            },
            _ => pending,
        };
        let logouts = u64::try_from(self.native_logouts.len())
            .map_err(|_| AuthenticationAuditFailureV1::Capacity)?;
        let remaining_logouts = match mutation {
            Some(SessionMutation::BeginNativeLogoutDuringRefresh { .. }) => logouts
                .checked_add(1)
                .ok_or(AuthenticationAuditFailureV1::Capacity)?,
            Some(SessionMutation::RecordNativeLogoutAfterRefreshClosed { .. }) => logouts
                .checked_sub(1)
                .ok_or(AuthenticationAuditFailureV1::Integrity)?,
            _ => logouts,
        };
        let remaining = remaining
            .checked_add(remaining_logouts)
            .ok_or(AuthenticationAuditFailureV1::Capacity)?;
        // checked_json applies this absolute bound to every terminal payload,
        // including its session mutation/binding wrapper, not just the record.
        let terminal_bytes = remaining
            .checked_mul(MAX_PAYLOAD_BYTES as u64)
            .ok_or(AuthenticationAuditFailureV1::Capacity)?;
        let committed_and_reserved_records = records
            .checked_add(remaining)
            .ok_or(AuthenticationAuditFailureV1::Capacity)?;
        let committed_and_reserved_bytes = bytes
            .checked_add(terminal_bytes)
            .ok_or(AuthenticationAuditFailureV1::Capacity)?;
        ensure!(
            committed_and_reserved_records <= policy.max_records
                && committed_and_reserved_bytes <= policy.max_serialized_bytes,
            AuthenticationAuditFailureV1::Capacity
        );
        Ok(())
    }

    fn check_native_logout_prefix(
        &self,
        mutation: Option<&SessionMutation>,
        payload: &str,
        policy: &AuthenticationAuditPolicyV1,
    ) -> Result<()> {
        if self.budget.plan.is_some() {
            return Ok(());
        }
        if self.native_logouts.is_empty()
            && !matches!(
                mutation,
                Some(
                    SessionMutation::BeginNativeLogoutDuringRefresh { .. }
                        | SessionMutation::RecordNativeLogoutAfterRefreshClosed { .. }
                )
            )
        {
            // Historical underfunded refreshes remain readable. No V3 logout
            // promise exists here that could be retroactively declared funded.
            return Ok(());
        }
        let records = self
            .verified
            .records
            .checked_add(1)
            .ok_or(AuthenticationAuditFailureV1::Integrity)?;
        let bytes = self
            .verified
            .serialized_bytes
            .checked_add(payload.len() as u64)
            .ok_or(AuthenticationAuditFailureV1::Integrity)?;
        self.check_terminal_capacity_for_mutation(mutation, records, bytes, policy)
            .map_err(|_| AuthenticationAuditFailureV1::Integrity.into())
    }

    pub fn session_current(&self, session_id: Uuid) -> Option<AuthenticationSessionCurrentV1> {
        self.sessions.get(&session_id).cloned()
    }

    pub fn fingerprint_seen(&self, fingerprint: &AuthenticationTokenFingerprintV1) -> bool {
        self.fingerprints.contains(fingerprint) || self.initial_fingerprints.contains(fingerprint)
    }

    pub fn validate_initial(
        &self,
        record: &AuthenticationRecordV1,
        principal: &PrincipalRefV1,
        fingerprint: AuthenticationTokenFingerprintV1,
        membership_recovery: bool,
        payload: &str,
        policy: &AuthenticationAuditPolicyV1,
    ) -> Result<u64> {
        ensure!(
            matches!(
                self.version,
                VERIFIED_INITIAL_VERSION | VERSION | TENANT_CAPACITY_VERSION
            ),
            AuthenticationAuditFailureV1::Format
        );
        ensure!(
            record.phase == AuthenticationPhaseV1::Intention
                && matches!(
                    record.action,
                    AuthenticationActionV1::Login | AuthenticationActionV1::SessionAdmission
                )
                && record.session_id.is_some()
                && matches!(principal, PrincipalRefV1::HumanOidc { issuer, .. } if issuer == &record.issuer)
                && fingerprint.domain == FingerprintDomainV1::Access,
            AuthenticationAuditFailureV1::InvalidInput
        );
        principal
            .validate()
            .map_err(|_| AuthenticationAuditFailureV1::InvalidInput)?;
        ensure!(
            !self.fingerprint_seen(&fingerprint),
            AuthenticationAuditFailureV1::Conflict
        );
        // Count a pending attempt until a durably admitted session proves its
        // success. A crash, failed lookup or rejected result cannot release it.
        // The result allowance includes the largest initial rejection shape;
        // its decimal sequence is sized at the policy's maximum, not today's.
        let mut terminal = record.clone();
        terminal.phase = AuthenticationPhaseV1::Result {
            outcome: AuthenticationOutcomeV1::Rejected,
            reason: AuthenticationReasonV1::NoMembership,
        };
        terminal.principal = Some(principal.clone());
        terminal.causal = Some(AuthenticationReceiptV1 {
            namespace_id: self.verified.namespace_id,
            sequence: policy.max_records,
            hash: ZERO_HASH.into(),
        });
        let reserved_bytes = (payload.len() as u64)
            .checked_add(
                checked_json(
                    &Entry::Authentication { record: terminal },
                    &Redactor::new(vec![]),
                )?
                .len() as u64,
            )
            .ok_or(AuthenticationAuditFailureV1::Capacity)?;
        let used = self
            .subject_usage
            .get(principal)
            .copied()
            .unwrap_or_default();
        // Lifetime of this ledger, all client IDs and session kinds combined.
        // An unapproved subject gets at most 64 failed/pending attempts and
        // 1/8 of logical capacity (one auditable attempt on a smaller policy).
        // No expiry or operator archive is implied by this finite budget.
        // A fresh trusted membership preflight can recover an exhausted user;
        // that observation is explicit and never replaces normal authorization.
        let attempts = ((policy.max_records.saturating_sub(1)) / 16).clamp(1, 64);
        let bytes = (policy.max_serialized_bytes / 8).max(reserved_bytes);
        ensure!(
            membership_recovery
                || (used.0 < attempts
                    && used
                        .1
                        .checked_add(reserved_bytes)
                        .is_some_and(|next| next <= bytes)),
            AuthenticationAuditFailureV1::SubjectCapacity
        );
        Ok(reserved_bytes)
    }

    fn accept_initial(
        &mut self,
        entry: Entry,
        payload: String,
        receipt: AuthenticationReceiptV1,
        policy: &AuthenticationAuditPolicyV1,
    ) -> Result<()> {
        let Entry::VerifiedInitial {
            record,
            principal,
            access_fingerprint: fingerprint,
            membership_recovery,
        } = entry
        else {
            return Err(AuthenticationAuditFailureV1::InvalidInput.into());
        };
        let reserved_bytes = self.validate_initial(
            &record,
            &principal,
            fingerprint,
            membership_recovery,
            &payload,
            policy,
        )?;
        let used = self.subject_usage.entry(principal.clone()).or_default();
        used.0 = used
            .0
            .checked_add(1)
            .ok_or(AuthenticationAuditFailureV1::Integrity)?;
        used.1 = used
            .1
            .checked_add(reserved_bytes)
            .ok_or(AuthenticationAuditFailureV1::Integrity)?;
        self.initial_proofs.insert(
            record.operation_id,
            InitialProof {
                principal,
                fingerprint,
                reserved_bytes,
            },
        );
        self.initial_fingerprints.insert(fingerprint);
        self.accept(record, payload, receipt)
    }

    /// A tracked lifecycle phase must update its session in the same entry.
    /// Otherwise a plain record would consume the unique phase while leaving
    /// an Active/Pending projection that can no longer complete its transition.
    pub fn validate_generic_lane(&self, record: &AuthenticationRecordV1) -> Result<()> {
        // A verified initial success must atomically install its binding.
        if self.initial_proofs.contains_key(&record.operation_id) {
            ensure!(
                !matches!(
                    record.phase,
                    AuthenticationPhaseV1::Result {
                        outcome: AuthenticationOutcomeV1::Succeeded,
                        ..
                    }
                ),
                AuthenticationAuditFailureV1::PhaseOrder
            );
        }
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

    fn durable_phase_entry(
        &self,
        operation: Uuid,
        phase: u8,
        expected: &AuthenticationReceiptV1,
    ) -> Result<Entry> {
        let (payload, receipt) = self
            .phases
            .get(&(operation, phase))
            .ok_or(AuthenticationAuditFailureV1::PhaseOrder)?;
        ensure!(
            expected.namespace_id == self.verified.namespace_id && receipt == expected,
            AuthenticationAuditFailureV1::PhaseOrder
        );
        // phases contains only canonical payloads from the validated prefix.
        let entry: Entry =
            serde_json::from_str(payload).map_err(|_| AuthenticationAuditFailureV1::Integrity)?;
        Ok(match entry {
            Entry::Charged { entry, .. } => *entry,
            other => other,
        })
    }

    fn validate_native_logout_context(
        &self,
        record: &AuthenticationRecordV1,
        binding: &AuthenticationSessionBindingV1,
        refresh_operation_id: Uuid,
        refresh_intention: &AuthenticationReceiptV1,
        policy: &AuthenticationAuditPolicyV1,
    ) -> Result<()> {
        ensure!(
            matches!(self.version, VERSION | TENANT_CAPACITY_VERSION),
            AuthenticationAuditFailureV1::Format
        );
        validate_binding(binding, policy)?;
        record_matches_binding(record, binding)?;
        let current = self.current_for(record, binding.generation, binding.access_fingerprint)?;
        ensure!(
            binding.kind == OidcSessionKindV1::NativeCli
                && current.binding == *binding
                && !refresh_operation_id.is_nil()
                && refresh_operation_id != record.operation_id,
            AuthenticationAuditFailureV1::PhaseOrder
        );
        let Entry::Session { mutation } =
            self.durable_phase_entry(refresh_operation_id, 0, refresh_intention)?
        else {
            return Err(AuthenticationAuditFailureV1::PhaseOrder.into());
        };
        let SessionMutation::RefreshPending {
            record: refresh,
            expected_generation,
            expected_access_fingerprint,
        } = *mutation
        else {
            return Err(AuthenticationAuditFailureV1::PhaseOrder.into());
        };
        record_matches_binding(&refresh, binding)?;
        ensure!(
            refresh.operation_id == refresh_operation_id
                && refresh.action == AuthenticationActionV1::Refresh
                && refresh.phase == AuthenticationPhaseV1::Intention
                && expected_generation == binding.generation
                && expected_access_fingerprint == binding.access_fingerprint,
            AuthenticationAuditFailureV1::PhaseOrder
        );
        Ok(())
    }

    fn validate_native_logout_begin(
        &self,
        record: &AuthenticationRecordV1,
        binding: &AuthenticationSessionBindingV1,
        refresh_operation_id: Uuid,
        refresh_intention: &AuthenticationReceiptV1,
        existing: Option<&AuthenticationReceiptV1>,
        policy: &AuthenticationAuditPolicyV1,
    ) -> Result<()> {
        self.validate_native_logout_context(
            record,
            binding,
            refresh_operation_id,
            refresh_intention,
            policy,
        )?;
        ensure!(
            record.action == AuthenticationActionV1::Logout
                && record.phase == AuthenticationPhaseV1::Intention
                && record.transaction_id.is_none()
                && record.causal.is_none(),
            AuthenticationAuditFailureV1::PhaseOrder
        );
        let current = self.current_for(record, binding.generation, binding.access_fingerprint)?;
        if let Some(receipt) = existing {
            ensure!(
                refresh_intention.sequence < receipt.sequence,
                AuthenticationAuditFailureV1::PhaseOrder
            );
            if let Some(promise) = self.native_logouts.get(&binding.session_id) {
                ensure!(
                    promise.logout_operation_id == record.operation_id
                        && promise.logout_intention == *receipt
                        && promise.refresh_operation_id == refresh_operation_id
                        && promise.refresh_intention == *refresh_intention
                        && matches!(
                            current.state,
                            AuthenticationSessionStateV1::RefreshPending { .. }
                                | AuthenticationSessionStateV1::Closed { .. }
                        ),
                    AuthenticationAuditFailureV1::PhaseOrder
                );
                return Ok(());
            }
            // A consumed promise is proved by its actual END, never by absence.
            let (_, end_receipt) = self
                .phases
                .get(&(record.operation_id, 1))
                .ok_or(AuthenticationAuditFailureV1::PhaseOrder)?;
            let Entry::Session { mutation } =
                self.durable_phase_entry(record.operation_id, 1, end_receipt)?
            else {
                return Err(AuthenticationAuditFailureV1::PhaseOrder.into());
            };
            let SessionMutation::RecordNativeLogoutAfterRefreshClosed {
                record: end,
                expected_binding,
                refresh_operation_id: end_refresh,
                refresh_intention: end_intention,
                refresh_result,
            } = *mutation
            else {
                return Err(AuthenticationAuditFailureV1::PhaseOrder.into());
            };
            ensure!(
                expected_binding == *binding
                    && end_refresh == refresh_operation_id
                    && end_intention == *refresh_intention
                    && end.causal.as_ref() == Some(receipt),
                AuthenticationAuditFailureV1::PhaseOrder
            );
            return self.validate_native_logout_end(
                &end,
                binding,
                refresh_operation_id,
                refresh_intention,
                &refresh_result,
                end_receipt.sequence,
                false,
                policy,
            );
        }
        ensure!(
            matches!(&current.state, AuthenticationSessionStateV1::RefreshPending {
                operation_id, intention
            } if *operation_id == refresh_operation_id && intention == refresh_intention)
                && !self.native_logouts.contains_key(&binding.session_id),
            AuthenticationAuditFailureV1::PhaseOrder
        );
        Ok(())
    }

    fn validate_native_logout_end(
        &self,
        record: &AuthenticationRecordV1,
        binding: &AuthenticationSessionBindingV1,
        refresh_operation_id: Uuid,
        refresh_intention: &AuthenticationReceiptV1,
        refresh_result: &AuthenticationReceiptV1,
        result_sequence: u64,
        require_promise: bool,
        policy: &AuthenticationAuditPolicyV1,
    ) -> Result<()> {
        self.validate_native_logout_context(
            record,
            binding,
            refresh_operation_id,
            refresh_intention,
            policy,
        )?;
        self.validate_transition(record)?;
        ensure!(
            record.action == AuthenticationActionV1::Logout
                && record.phase
                    == (AuthenticationPhaseV1::Result {
                        outcome: AuthenticationOutcomeV1::Succeeded,
                        reason: AuthenticationReasonV1::UserRequested,
                    })
                && record.transaction_id.is_none(),
            AuthenticationAuditFailureV1::PhaseOrder
        );
        let logout_intention = record
            .causal
            .as_ref()
            .ok_or(AuthenticationAuditFailureV1::PhaseOrder)?;
        let Entry::Session { mutation } =
            self.durable_phase_entry(record.operation_id, 0, logout_intention)?
        else {
            return Err(AuthenticationAuditFailureV1::PhaseOrder.into());
        };
        let SessionMutation::BeginNativeLogoutDuringRefresh {
            record: begin,
            expected_binding,
            refresh_operation_id: begin_refresh,
            refresh_intention: begin_intention,
        } = *mutation
        else {
            return Err(AuthenticationAuditFailureV1::PhaseOrder.into());
        };
        record_matches_binding(&begin, binding)?;
        ensure!(
            begin.operation_id == record.operation_id
                && begin.action == AuthenticationActionV1::Logout
                && begin.phase == AuthenticationPhaseV1::Intention
                && begin.transaction_id.is_none()
                && begin.causal.is_none()
                && expected_binding == *binding
                && begin_refresh == refresh_operation_id
                && begin_intention == *refresh_intention,
            AuthenticationAuditFailureV1::PhaseOrder
        );
        let Entry::Session { mutation } =
            self.durable_phase_entry(refresh_operation_id, 1, refresh_result)?
        else {
            return Err(AuthenticationAuditFailureV1::PhaseOrder.into());
        };
        let SessionMutation::Close {
            record: refresh,
            expected_generation,
            expected_access_fingerprint,
        } = *mutation
        else {
            return Err(AuthenticationAuditFailureV1::PhaseOrder.into());
        };
        record_matches_binding(&refresh, binding)?;
        let AuthenticationPhaseV1::Result { outcome, reason } = refresh.phase else {
            return Err(AuthenticationAuditFailureV1::PhaseOrder.into());
        };
        let current = self.current_for(record, binding.generation, binding.access_fingerprint)?;
        ensure!(
            refresh.operation_id == refresh_operation_id
                && refresh.action == AuthenticationActionV1::Refresh
                && outcome != AuthenticationOutcomeV1::Succeeded
                && refresh.causal.as_ref() == Some(refresh_intention)
                && expected_generation == binding.generation
                && expected_access_fingerprint == binding.access_fingerprint
                && current.state == (AuthenticationSessionStateV1::Closed { reason })
                && refresh_intention.sequence < logout_intention.sequence
                && logout_intention.sequence < refresh_result.sequence
                && refresh_result.sequence < result_sequence,
            AuthenticationAuditFailureV1::PhaseOrder
        );
        if require_promise {
            let promise = self
                .native_logouts
                .get(&binding.session_id)
                .ok_or(AuthenticationAuditFailureV1::PhaseOrder)?;
            ensure!(
                promise.logout_operation_id == record.operation_id
                    && promise.logout_intention == *logout_intention
                    && promise.refresh_operation_id == refresh_operation_id
                    && promise.refresh_intention == *refresh_intention,
                AuthenticationAuditFailureV1::PhaseOrder
            );
        }
        Ok(())
    }

    /// Exact replay is an observation, never a second exchange grant or a
    /// reactivation of a subsequently advanced/closed session.
    pub fn validate_mutation_replay(
        &self,
        mutation: &SessionMutation,
        receipt: &AuthenticationReceiptV1,
        policy: &AuthenticationAuditPolicyV1,
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
            SessionMutation::BeginNativeLogoutDuringRefresh {
                record,
                expected_binding,
                refresh_operation_id,
                refresh_intention,
            } => self.validate_native_logout_begin(
                record,
                expected_binding,
                *refresh_operation_id,
                refresh_intention,
                Some(receipt),
                policy,
            )?,
            SessionMutation::RecordNativeLogoutAfterRefreshClosed {
                record,
                expected_binding,
                refresh_operation_id,
                refresh_intention,
                refresh_result,
            } => self.validate_native_logout_end(
                record,
                expected_binding,
                *refresh_operation_id,
                refresh_intention,
                refresh_result,
                receipt.sequence,
                false,
                policy,
            )?,
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
                if let Some(proof) = self.initial_proofs.get(&record.operation_id) {
                    ensure!(
                        proof.principal == binding.principal
                            && proof.fingerprint == binding.access_fingerprint,
                        AuthenticationAuditFailureV1::PhaseOrder
                    );
                } else {
                    ensure!(
                        !self
                            .initial_fingerprints
                            .contains(&binding.access_fingerprint),
                        AuthenticationAuditFailureV1::PhaseOrder
                    );
                }
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
                    !self.native_logouts.contains_key(&old.session_id)
                        && record.action == A::Refresh
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
                        && !self.fingerprint_seen(&binding.access_fingerprint),
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
                if let Some(promise) = self.native_logouts.get(&current.binding.session_id) {
                    ensure!(
                        record.action == A::Refresh
                            && record.operation_id == promise.refresh_operation_id
                            && record.causal.as_ref() == Some(&promise.refresh_intention),
                        AuthenticationAuditFailureV1::PhaseOrder
                    );
                }
            }
            SessionMutation::BeginNativeLogoutDuringRefresh {
                record,
                expected_binding,
                refresh_operation_id,
                refresh_intention,
            } => self.validate_native_logout_begin(
                record,
                expected_binding,
                *refresh_operation_id,
                refresh_intention,
                None,
                policy,
            )?,
            SessionMutation::RecordNativeLogoutAfterRefreshClosed {
                record,
                expected_binding,
                refresh_operation_id,
                refresh_intention,
                refresh_result,
            } => self.validate_native_logout_end(
                record,
                expected_binding,
                *refresh_operation_id,
                refresh_intention,
                refresh_result,
                self.verified
                    .records
                    .checked_add(1)
                    .ok_or(AuthenticationAuditFailureV1::Capacity)?,
                true,
                policy,
            )?,
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
        // Check the candidate prefix before applying either state/promise
        // delta. This also covers the END consuming the last live promise.
        self.check_native_logout_prefix(Some(&mutation), &payload, policy)?;
        // All checks precede projection changes. Replaying a damaged file
        // cannot partially authorize a binding or fingerprint.
        match &mutation {
            SessionMutation::Admit { binding, .. } | SessionMutation::Rotate { binding, .. } => {
                if let SessionMutation::Admit { record, .. } = &mutation
                    && let Some(proof) = self.initial_proofs.get(&record.operation_id)
                {
                    let used = self
                        .subject_usage
                        .get_mut(&proof.principal)
                        .ok_or(AuthenticationAuditFailureV1::Integrity)?;
                    used.0 = used
                        .0
                        .checked_sub(1)
                        .ok_or(AuthenticationAuditFailureV1::Integrity)?;
                    used.1 = used
                        .1
                        .checked_sub(proof.reserved_bytes)
                        .ok_or(AuthenticationAuditFailureV1::Integrity)?;
                }
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
            SessionMutation::BeginNativeLogoutDuringRefresh {
                record,
                expected_binding,
                refresh_operation_id,
                refresh_intention,
            } => {
                self.native_logouts.insert(
                    expected_binding.session_id,
                    NativeLogoutPromise {
                        logout_operation_id: record.operation_id,
                        logout_intention: receipt.clone(),
                        refresh_operation_id: *refresh_operation_id,
                        refresh_intention: refresh_intention.clone(),
                    },
                );
            }
            SessionMutation::RecordNativeLogoutAfterRefreshClosed {
                expected_binding, ..
            } => {
                self.native_logouts
                    .remove(&expected_binding.session_id)
                    .ok_or(AuthenticationAuditFailureV1::Integrity)?;
            }
        }
        let record = match mutation {
            SessionMutation::Admit { record, .. }
            | SessionMutation::RefreshPending { record, .. }
            | SessionMutation::Rotate { record, .. }
            | SessionMutation::Close { record, .. }
            | SessionMutation::BeginNativeLogoutDuringRefresh { record, .. }
            | SessionMutation::RecordNativeLogoutAfterRefreshClosed { record, .. } => record,
        };
        self.accept(record, payload, receipt)
    }

    /// Compare the requested logical entry with its exact stored representation
    /// before considering any current membership candidates or capacity.
    pub fn existing_entry(&self, entry: &Entry) -> Result<Option<AuthenticationReceiptV1>> {
        let record = entry
            .record()
            .ok_or(AuthenticationAuditFailureV1::InvalidInput)?;
        let Some((payload, receipt)) = self
            .phases
            .get(&(record.operation_id, phase_key(record.phase).1))
        else {
            return Ok(None);
        };
        let stored: Entry =
            serde_json::from_str(payload).map_err(|_| AuthenticationAuditFailureV1::Integrity)?;
        let logical = match &stored {
            Entry::Charged { entry, .. } => entry.as_ref(),
            other => other,
        };
        ensure!(
            checked_json(logical, &Redactor::new(vec![]))?
                == checked_json(entry, &Redactor::new(vec![]))?,
            AuthenticationAuditFailureV1::Conflict
        );
        Ok(Some(receipt.clone()))
    }

    pub fn charge_entry(
        &self,
        entry: Entry,
        candidates: Option<&AuthenticationTenantCandidatesV1>,
        redactor: &Redactor,
    ) -> Result<Entry> {
        if self.version != TENANT_CAPACITY_VERSION {
            ensure!(candidates.is_none(), AuthenticationAuditFailureV1::Format);
            return Ok(entry);
        }
        ensure!(
            self.budget.plan.is_some(),
            AuthenticationAuditFailureV1::Format
        );
        let owner = if matches!(entry, Entry::VerifiedInitial { .. }) {
            Some(self.budget.choose_initial(
                &entry,
                candidates.ok_or(AuthenticationAuditFailureV1::Format)?,
                redactor,
            )?)
        } else {
            ensure!(
                candidates.is_none(),
                AuthenticationAuditFailureV1::InvalidInput
            );
            self.budget.owner(&entry)?
        };
        if let Some(owner) = &owner {
            self.validate_future_terminal(&entry, owner)?;
        }
        Ok(match owner {
            Some(charge) => Entry::Charged {
                charge,
                entry: Box::new(entry),
            },
            None => entry, // Only the exact historical terminal can own None.
        })
    }

    /// Before promising an effect, size all terminal layouts with the retained
    /// immutable identity and worst-width numeric/fingerprint fields. These
    /// are sizing templates, never persisted events or claimed observations.
    /// Unknown future credentials affect only fixed-size digests. Every copied
    /// identity string comes from this binding and keeps its exact JSON escape
    /// cost; no guess about a maximum unescaped string is required.
    fn validate_future_terminal(&self, entry: &Entry, owner: &budget::CapacityOwner) -> Result<()> {
        let record = entry
            .record()
            .ok_or(AuthenticationAuditFailureV1::InvalidInput)?;
        if record.phase != AuthenticationPhaseV1::Intention {
            return Ok(());
        }
        let principal = match entry {
            Entry::VerifiedInitial { principal, .. } => principal.clone(),
            _ => self
                .sessions
                .get(
                    &record
                        .session_id
                        .ok_or(AuthenticationAuditFailureV1::PhaseOrder)?,
                )
                .ok_or(AuthenticationAuditFailureV1::PhaseOrder)?
                .binding
                .principal
                .clone(),
        };
        let receipt = AuthenticationReceiptV1 {
            namespace_id: self.verified.namespace_id,
            sequence: u64::MAX,
            hash: "f".repeat(64),
        };
        let access = AuthenticationTokenFingerprintV1 {
            domain: FingerprintDomainV1::Access,
            digest: [255; 32],
        };
        let binding = AuthenticationSessionBindingV1 {
            session_id: record
                .session_id
                .ok_or(AuthenticationAuditFailureV1::PhaseOrder)?,
            generation: u32::MAX,
            principal: principal.clone(),
            issuer: record.issuer.clone(),
            client_id: record.client_id.clone(),
            kind: OidcSessionKindV1::NativeCli,
            access_fingerprint: access,
            refresh_fingerprint: Some(AuthenticationTokenFingerprintV1 {
                domain: FingerprintDomainV1::Refresh,
                digest: [255; 32],
            }),
            expires_at_unix_seconds: i64::MIN,
        };
        let terminal = AuthenticationRecordV1 {
            // This is the longest action/phase/reason representation, including
            // optional context even when the concrete transition needs less.
            action: AuthenticationActionV1::SessionAdmission,
            phase: AuthenticationPhaseV1::Result {
                outcome: AuthenticationOutcomeV1::Succeeded,
                reason: AuthenticationReasonV1::ProviderUnavailable,
            },
            principal: Some(principal),
            transaction_id: Some(record.operation_id),
            causal: Some(receipt.clone()),
            ..record.clone()
        };
        let mutations = [
            SessionMutation::Admit {
                record: terminal.clone(),
                binding: binding.clone(),
            },
            SessionMutation::Rotate {
                record: terminal.clone(),
                binding: binding.clone(),
                expected_generation: u32::MAX,
                expected_access_fingerprint: access,
            },
            SessionMutation::Close {
                record: terminal.clone(),
                expected_generation: u32::MAX,
                expected_access_fingerprint: access,
            },
            SessionMutation::RecordNativeLogoutAfterRefreshClosed {
                record: terminal,
                expected_binding: binding,
                refresh_operation_id: record.operation_id,
                refresh_intention: receipt.clone(),
                refresh_result: receipt,
            },
        ];
        for mutation in mutations {
            checked_json(
                &Entry::Charged {
                    charge: owner.clone(),
                    entry: Box::new(Entry::Session {
                        mutation: Box::new(mutation),
                    }),
                },
                &Redactor::new(vec![]),
            )?;
        }
        Ok(())
    }

    pub fn validate_charge(&self, entry: &Entry, bytes: u64) -> Result<()> {
        if self.budget.plan.is_none() {
            return Ok(());
        }
        let (logical, owner) = match entry {
            Entry::Charged { entry, charge } => (entry.as_ref(), Some(charge)),
            other => (other, None),
        };
        // Use the same replay transition before inserting, so a routine
        // capacity refusal never latches the live handle as damaged.
        self.budget.validate(logical, owner, bytes)
    }

    pub fn install_plan(
        &mut self,
        plan: AuthenticationCapacityPlanV1,
        payload_bytes: u64,
        policy: &AuthenticationAuditPolicyV1,
    ) -> Result<()> {
        ensure!(
            self.version == TENANT_CAPACITY_VERSION,
            AuthenticationAuditFailureV1::Format
        );
        self.budget.install(
            plan,
            self.intentions
                .iter()
                .filter(|(operation, _)| !self.phases.contains_key(&(**operation, 1)))
                .map(|(_, (record, receipt))| {
                    (
                        record,
                        receipt,
                        record
                            .session_id
                            .and_then(|id| self.sessions.get(&id))
                            .map(|current| &current.binding),
                    )
                }),
            self.verified
                .records
                .checked_add(1)
                .ok_or(AuthenticationAuditFailureV1::Capacity)?,
            self.verified
                .serialized_bytes
                .checked_add(payload_bytes)
                .ok_or(AuthenticationAuditFailureV1::Capacity)?,
            policy,
        )
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
                if let Some(proof) = self.initial_proofs.get(&record.operation_id) {
                    ensure!(
                        record
                            .principal
                            .as_ref()
                            .is_none_or(|principal| *principal == proof.principal),
                        AuthenticationAuditFailureV1::PhaseOrder
                    );
                }
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
        // V1 denied results have verified subjects but no retained proof.
        // Preserve every byte while charging those historical denials against
        // their subject after migration; never invent fingerprints for them.
        if matches!(
            record.phase,
            AuthenticationPhaseV1::Result {
                outcome: AuthenticationOutcomeV1::Rejected,
                reason: AuthenticationReasonV1::NoMembership,
            }
        ) && matches!(
            record.action,
            AuthenticationActionV1::Login | AuthenticationActionV1::SessionAdmission
        ) && !self.initial_proofs.contains_key(&record.operation_id)
        {
            let principal = record
                .principal
                .clone()
                .ok_or(AuthenticationAuditFailureV1::Integrity)?;
            let initial = self
                .phases
                .get(&(record.operation_id, 0))
                .ok_or(AuthenticationAuditFailureV1::Integrity)?;
            let bytes = (initial.0.len() as u64)
                .checked_add(payload.len() as u64)
                .ok_or(AuthenticationAuditFailureV1::Integrity)?;
            let used = self.subject_usage.entry(principal).or_default();
            used.0 = used
                .0
                .checked_add(1)
                .ok_or(AuthenticationAuditFailureV1::Integrity)?;
            used.1 = used
                .1
                .checked_add(bytes)
                .ok_or(AuthenticationAuditFailureV1::Integrity)?;
        }
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
        version: schema_version(db)?,
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
        initial_proofs: BTreeMap::new(),
        initial_fingerprints: BTreeSet::new(),
        subject_usage: BTreeMap::new(),
        native_logouts: BTreeMap::new(),
        budget: budget::BudgetReplay::default(),
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
        // A single bounded wrapper is permitted only after the unique plan.
        // Historical terminals retain their exact V3 bytes and receipts.
        let (entry, charge) = match entry {
            Entry::Charged { charge, entry } => {
                ensure!(
                    replay.version == TENANT_CAPACITY_VERSION
                        && replay.budget.plan.is_some()
                        && matches!(
                            entry.as_ref(),
                            Entry::Authentication { .. }
                                | Entry::VerifiedInitial { .. }
                                | Entry::Session { .. }
                        ),
                    AuthenticationAuditFailureV1::Integrity
                );
                (*entry, Some(charge))
            }
            other => (other, None),
        };
        if replay.budget.plan.is_some() && entry.record().is_some() {
            if let Some(owner) = charge.as_ref() {
                replay.validate_future_terminal(&entry, owner)?;
            }
            replay
                .budget
                .accept(&entry, charge.as_ref(), payload.len() as u64, &receipt)?;
        }
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
                replay.check_native_logout_prefix(None, &payload, policy)?;
                replay.accept(record, payload, receipt.clone())?;
            }
            entry @ Entry::VerifiedInitial { .. } => {
                let record = entry
                    .record()
                    .ok_or(AuthenticationAuditFailureV1::Integrity)?;
                ensure!(
                    seq > 1
                        && matches!(
                            replay.version,
                            VERIFIED_INITIAL_VERSION | VERSION | TENANT_CAPACITY_VERSION
                        )
                        && projected_operation.as_deref()
                            == Some(record.operation_id.to_string().as_str())
                        && projected_phase.as_deref() == Some(phase_key(record.phase).0),
                    AuthenticationAuditFailureV1::Integrity
                );
                validate_record(record, policy)?;
                replay.check_native_logout_prefix(None, &payload, policy)?;
                replay.accept_initial(entry, payload, receipt.clone(), policy)?;
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
            Entry::CapacityPlan { plan } => {
                ensure!(
                    seq > 1 && projected_operation.is_none() && projected_phase.is_none(),
                    AuthenticationAuditFailureV1::Integrity
                );
                replay.install_plan(plan, payload.len() as u64, policy)?;
                replay.receipts.insert(expected, receipt.clone());
            }
            Entry::Charged { .. } => return Err(AuthenticationAuditFailureV1::Integrity.into()),
        }
        replay.verified.records = expected;
        replay.verified.serialized_bytes = bytes;
        replay.verified.head = receipt;
    }
    ensure!(
        replay.verified.records > 0
            && ((replay.version == TENANT_CAPACITY_VERSION) == replay.budget.plan.is_some()),
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

    // Isolate the final-END arithmetic guard using a real, valid stored prefix
    // and one deliberately inconsistent in-memory counter. No database bytes
    // are changed: the separate cold-open oracle covers persisted prefixes.
    #[test]
    fn last_logout_end_checks_the_remaining_refresh_reservation() {
        let (_root, path, policy, installation, entry) =
            super::super::native_logout_tests::last_end_fixture();
        let db =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let mut checked = replay(&db, installation, &policy, &Redactor::new(vec![]), None).unwrap();
        let Entry::Session { mutation } = &entry else {
            panic!("test END fixture")
        };
        let payload = checked_json(&entry, &Redactor::new(vec![])).unwrap();
        checked.validate_mutation(mutation, &policy).unwrap();
        assert_eq!(checked.native_logouts.len(), 1);
        checked
            .check_native_logout_prefix(Some(mutation), &payload, &policy)
            .unwrap();
        // END consumes the last L, but the other session's R still requires
        // 32768 bytes. Merely checking committed payload bytes would pass.
        checked.verified.serialized_bytes =
            policy.max_serialized_bytes - MAX_PAYLOAD_BYTES as u64 - payload.len() as u64 + 1;
        assert!(
            checked.verified.serialized_bytes + (payload.len() as u64)
                < policy.max_serialized_bytes
        );
        let error = checked
            .check_native_logout_prefix(Some(mutation), &payload, &policy)
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<AuthenticationAuditFailureV1>(),
            Some(&AuthenticationAuditFailureV1::Integrity)
        );
        assert_eq!(
            checked.native_logouts.len(),
            1,
            "refusal must not consume L"
        );
    }

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
