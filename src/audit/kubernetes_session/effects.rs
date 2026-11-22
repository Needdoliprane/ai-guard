//! Tenant effect ledger of V2 Kubernetes sessions (G11: R1, Q7, settlement).
//!
//! A capability family gated by its own genesis marker, without a
//! `user_version` change: its tables exist exactly when the marker exists,
//! and the schema validator checks both. Every claim, result, terminal and
//! settlement is one audited event appended in the same transaction as its
//! projection rows. Quota stays held by the reservation row until a
//! settlement; the shared quota readers exclude settled reservations.
//!
//! The ledger re-derives every request digest it can (claim and Pod CREATE,
//! gate and finalizer PATCH, DELETE) from the confirmed intent and the
//! retained object UIDs before granting `EmitOnce`. An attach digest is a
//! declared broker commitment bound to the exact Pod UID.
use super::*;
use crate::control::kubernetes_session::effects::*;
use crate::runtime::kubernetes::{
    session_claim::KubernetesSessionClaimRequestV2,
    session_pod::{
        SessionPodPurposeV2, effectful_request_bytes, session_export_pod_name, session_pod_name,
        session_workspace_claim_name,
    },
    session_requests::{
        delete_claim_request, delete_pod_request, release_finalizer_request, release_gate_request,
    },
};
use KubernetesSessionEffectActionV2 as Action;
use KubernetesSessionEffectSubjectV2 as Subject;

pub(crate) const GENESIS_KIND: &str = "control.kubernetes.session_effect.genesis";
pub(crate) const PREFIX: &str = "control.kubernetes.session_effect.";
const CLAIMED: &str = "control.kubernetes.session_effect.claimed";
const RESULT: &str = "control.kubernetes.session_effect.result";
const TERMINAL: &str = "control.kubernetes.session_effect.terminal";
const SETTLED: &str = "control.kubernetes.session_effect.settled";

pub(crate) const SCHEMA: [(&str, &str, &str); 4] = [
    (
        "table",
        "kubernetes_session_effect_meta",
        "CREATE TABLE kubernetes_session_effect_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),genesis_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),genesis_hash TEXT NOT NULL)",
    ),
    (
        "table",
        "kubernetes_session_effects",
        "CREATE TABLE kubernetes_session_effects(reservation_id TEXT NOT NULL REFERENCES kubernetes_session_reservations(reservation_id),step INTEGER NOT NULL CHECK(step>=1 AND step<=32),operation_id TEXT NOT NULL UNIQUE,input_hash TEXT NOT NULL,subject TEXT NOT NULL CHECK(subject IN ('workspace_claim','session_pod','export_pod')),action TEXT NOT NULL CHECK(action IN ('create','release_gate','attach','release_finalizer','delete')),request_sha256 TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN ('claimed','applied','not_applied','uncertain')),claim_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),result_seq INTEGER UNIQUE REFERENCES events(seq),PRIMARY KEY(reservation_id,step))",
    ),
    (
        "table",
        "kubernetes_session_claims",
        "CREATE TABLE kubernetes_session_claims(claim_uid TEXT PRIMARY KEY NOT NULL,reservation_id TEXT NOT NULL UNIQUE REFERENCES kubernetes_session_reservations(reservation_id),bound_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
    (
        "table",
        "kubernetes_session_effect_states",
        "CREATE TABLE kubernetes_session_effect_states(reservation_id TEXT PRIMARY KEY NOT NULL REFERENCES kubernetes_session_reservations(reservation_id),last_step INTEGER NOT NULL CHECK(last_step>=1 AND last_step<=32),settled INTEGER NOT NULL CHECK(settled IN (0,1)),state_json TEXT NOT NULL,last_event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
];
pub(crate) const AUTO_INDEX_COUNTS: [(&str, u32); 4] = [
    ("kubernetes_session_effect_meta", 1),
    ("kubernetes_session_effects", 4),
    ("kubernetes_session_claims", 3),
    ("kubernetes_session_effect_states", 2),
];

/// True when the effect capability marker exists. Only then may the family's
/// tables and events exist.
pub(crate) fn enabled(db: &Connection) -> anyhow::Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM events WHERE kind=?1)",
        [GENESIS_KIND],
        |row| row.get(0),
    )?)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Genesis {
    version: u32,
    protocol: String,
    tenant_id: String,
    control_namespace_uuid: String,
    reservation_genesis_seq: i64,
    previous_seq: i64,
    previous_hash: String,
}

/// One audited change of the ledger. The tenant and broker come from the
/// authenticated identity of the transaction, never from a body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case", deny_unknown_fields)]
enum Change {
    Claim {
        request: KubernetesSessionEffectRequestV2,
        issued_at: String,
        not_after: String,
    },
    Result {
        result: KubernetesSessionEffectResultV2,
    },
    Terminal {
        terminal: KubernetesSessionTerminalV2,
    },
    Settle {
        settlement: KubernetesSessionSettlementV2,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    tenant_id: String,
    broker_id: String,
    incarnation_id: String,
    reservation_id: String,
    input_hash: String,
    change: Change,
    post_state: KubernetesSessionEffectStateV2,
}
impl Change {
    fn kind(&self) -> &'static str {
        match self {
            Self::Claim { .. } => CLAIMED,
            Self::Result { .. } => RESULT,
            Self::Terminal { .. } => TERMINAL,
            Self::Settle { .. } => SETTLED,
        }
    }
    fn operation_id(&self) -> &str {
        match self {
            Self::Claim { request, .. } => &request.operation_id,
            Self::Result { result } => &result.operation_id,
            Self::Terminal { terminal } => &terminal.operation_id,
            Self::Settle { settlement } => &settlement.operation_id,
        }
    }
    fn input_hash(&self) -> anyhow::Result<String> {
        match self {
            Self::Claim { request, .. } => request.input_hash(),
            Self::Result { result } => result.input_hash(),
            Self::Terminal { terminal } => terminal.input_hash(),
            Self::Settle { settlement } => settlement.input_hash(),
        }
    }
}

/// One retained step, as the ledger knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Step {
    step: u32,
    subject: Subject,
    action: Action,
    request_sha256: String,
    object_uid: Option<String>,
    outcome: Option<KubernetesSessionEffectOutcomeV2>,
}

fn subject_name(subject: Subject) -> &'static str {
    match subject {
        Subject::WorkspaceClaim => "workspace_claim",
        Subject::SessionPod => "session_pod",
        Subject::ExportPod => "export_pod",
    }
}
fn action_name(action: Action) -> &'static str {
    match action {
        Action::Create => "create",
        Action::ReleaseGate => "release_gate",
        Action::Attach => "attach",
        Action::ReleaseFinalizer => "release_finalizer",
        Action::Delete => "delete",
    }
}
fn outcome_state(outcome: Option<&KubernetesSessionEffectOutcomeV2>) -> &'static str {
    match outcome {
        None => "claimed",
        Some(KubernetesSessionEffectOutcomeV2::Applied { .. }) => "applied",
        Some(KubernetesSessionEffectOutcomeV2::NotApplied { .. }) => "not_applied",
        Some(KubernetesSessionEffectOutcomeV2::Uncertain { .. }) => "uncertain",
    }
}

fn initial_state(reservation_id: &str) -> KubernetesSessionEffectStateV2 {
    KubernetesSessionEffectStateV2 {
        version: 1,
        reservation_id: reservation_id.into(),
        last_step: 0,
        workspace_claim_uid: None,
        session_pod_uid: None,
        export_pod_uid: None,
        session_terminal: false,
        export_terminal: false,
        unresolved_uncertain_steps: 0,
        settled: false,
    }
}
fn pod_uid(state: &KubernetesSessionEffectStateV2, subject: Subject) -> Option<&str> {
    match subject {
        Subject::WorkspaceClaim => state.workspace_claim_uid.as_deref(),
        Subject::SessionPod => state.session_pod_uid.as_deref(),
        Subject::ExportPod => state.export_pod_uid.as_deref(),
    }
}
fn terminal(state: &KubernetesSessionEffectStateV2, subject: Subject) -> bool {
    match subject {
        Subject::WorkspaceClaim => false,
        Subject::SessionPod => state.session_terminal,
        Subject::ExportPod => state.export_terminal,
    }
}
fn any(steps: &[Step], subject: Subject, action: Action) -> bool {
    steps
        .iter()
        .any(|step| step.subject == subject && step.action == action)
}
fn applied(steps: &[Step], subject: Subject, action: Action) -> bool {
    steps.iter().any(|step| {
        step.subject == subject
            && step.action == action
            && matches!(
                step.outcome,
                Some(KubernetesSessionEffectOutcomeV2::Applied { .. })
            )
    })
}

/// Ordering of the closed effect chain. Pure: the projection and the
/// retained steps decide; nothing here reads a clock or the cluster.
fn admit_order(
    state: &KubernetesSessionEffectStateV2,
    steps: &[Step],
    request: &KubernetesSessionEffectRequestV2,
) -> anyhow::Result<()> {
    require(request.step == state.last_step + 1)?;
    // A step whose result is still pending blocks every later claim.
    conflict(steps.iter().all(|step| step.outcome.is_some()))?;
    let subject = request.subject;
    let cleanup = matches!(request.action, Action::Delete | Action::ReleaseFinalizer);
    if state.settled {
        conflict(cleanup)?;
    }
    let uid_matches = || pod_uid(state, subject) == request.object_uid.as_deref();
    match (subject, request.action) {
        (Subject::WorkspaceClaim, Action::Create) => conflict(steps.is_empty()),
        (Subject::WorkspaceClaim, _) => {
            // The session Pod never ran when no gate release of it was
            // applied or may have been: its workspace holds no session data.
            let session_never_ran = !steps.iter().any(|step| {
                step.subject == Subject::SessionPod
                    && step.action == Action::ReleaseGate
                    && !matches!(
                        step.outcome,
                        Some(KubernetesSessionEffectOutcomeV2::NotApplied { .. })
                    )
            });
            conflict(
                state.workspace_claim_uid.is_some()
                    && uid_matches()
                    // Nothing can still write to the claim, and its data was
                    // exported or never produced by a session.
                    && (state.session_pod_uid.is_none() || state.session_terminal)
                    && (state.export_pod_uid.is_none() || state.export_terminal)
                    && (state.export_terminal || session_never_ran),
            )
        }
        (Subject::SessionPod, Action::Create) => conflict(
            state.workspace_claim_uid.is_some() && !any(steps, Subject::SessionPod, Action::Create),
        ),
        (Subject::ExportPod, Action::Create) => conflict(
            state.workspace_claim_uid.is_some()
                && state.session_terminal
                && !any(steps, Subject::ExportPod, Action::Create),
        ),
        (_, Action::ReleaseGate) => conflict(
            uid_matches()
                && pod_uid(state, subject).is_some()
                && !terminal(state, subject)
                && !any(steps, subject, Action::ReleaseGate),
        ),
        (_, Action::Attach) => conflict(
            uid_matches()
                && !terminal(state, subject)
                && applied(steps, subject, Action::ReleaseGate)
                // stdinOnce: one attach per Pod, ever.
                && !any(steps, subject, Action::Attach),
        ),
        (_, Action::ReleaseFinalizer) => {
            conflict(uid_matches() && pod_uid(state, subject).is_some() && terminal(state, subject))
        }
        (_, Action::Delete) => conflict(uid_matches() && pod_uid(state, subject).is_some()),
    }
}

/// Central re-derivation of the claimed digest from retained facts.
fn rederive(
    intent: &KubernetesSessionReservationIntentV2,
    request: &KubernetesSessionEffectRequestV2,
    storage_policy: Option<&KubernetesSessionStoragePolicyV2>,
) -> anyhow::Result<()> {
    let authority = intent.authority();
    let ids = intent.ids()?;
    let pod_name = |subject| match subject {
        Subject::ExportPod => session_export_pod_name(ids.attempt_id),
        _ => session_pod_name(ids.attempt_id),
    };
    let uid = || request.object_uid.as_deref().ok_or(Failure::InvalidRequest);
    let refused = |_| Failure::InvalidRequest;
    let expected = match (request.subject, request.action) {
        (Subject::WorkspaceClaim, Action::Create) => {
            let storage = request.storage.as_ref().ok_or(Failure::InvalidRequest)?;
            permission(
                storage_policy
                    .ok_or(Failure::PermissionDenied)?
                    .approves(storage)?,
            )?;
            KubernetesSessionClaimRequestV2::render(intent, storage)
                .map_err(|_| Failure::InvalidRequest)?
                .target()
                .request_sha256()
                .to_owned()
        }
        (Subject::WorkspaceClaim, _) => {
            delete_claim_request(
                authority,
                &session_workspace_claim_name(ids.attempt_id),
                uid()?,
            )
            .map_err(refused)?
            .request_sha256
        }
        (subject, Action::Create) => {
            let purpose = if subject == Subject::ExportPod {
                SessionPodPurposeV2::Export
            } else {
                SessionPodPurposeV2::Session
            };
            effectful_request_bytes(intent, purpose)
                .map_err(|_| Failure::InvalidRequest)?
                .1
        }
        (subject, Action::ReleaseGate) => {
            release_gate_request(
                authority,
                &pod_name(subject),
                uid()?,
                request
                    .object_resource_version
                    .as_deref()
                    .ok_or(Failure::InvalidRequest)?,
            )
            .map_err(refused)?
            .request_sha256
        }
        (subject, Action::ReleaseFinalizer) => {
            release_finalizer_request(authority, &pod_name(subject), uid()?)
                .map_err(refused)?
                .request_sha256
        }
        (subject, Action::Delete) => {
            delete_pod_request(
                authority,
                &pod_name(subject),
                uid()?,
                request.object_resource_version.as_deref(),
            )
            .map_err(refused)?
            .request_sha256
        }
        // Identity bytes come from the broker's two observations: declared.
        (_, Action::Attach) => request.request_sha256.clone(),
    };
    conflict(expected == request.request_sha256)
}

fn load_state(
    db: &Connection,
    reservation_id: &str,
) -> anyhow::Result<Option<(KubernetesSessionEffectStateV2, i64)>> {
    let row: Option<(String, i64, i64, i64)> = db
        .query_row(
            "SELECT state_json,last_step,settled,last_event_seq FROM kubernetes_session_effect_states WHERE reservation_id=?1",
            [reservation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    row.map(|(json, last_step, settled, seq)| {
        let value =
            crate::control::kubernetes_session::validation::unique(json.as_bytes(), 16 * 1024)?;
        let state: KubernetesSessionEffectStateV2 = serde_json::from_value(value)?;
        conflict(
            state.reservation_id == reservation_id
                && i64::from(state.last_step) == last_step
                && i64::from(state.settled) == settled,
        )?;
        Ok((state, seq))
    })
    .transpose()
}
fn load_steps(db: &Connection, reservation_id: &str) -> anyhow::Result<Vec<Step>> {
    let mut statement = db.prepare(
        "SELECT e.payload FROM kubernetes_session_effects k JOIN events e ON e.seq=k.claim_seq WHERE k.reservation_id=?1 ORDER BY k.step",
    )?;
    let claims = statement
        .query_map([reservation_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut steps = Vec::with_capacity(claims.len());
    for payload in claims {
        let event: Event = serde_json::from_str(&payload)?;
        let record = record(&event)?;
        let Change::Claim { request, .. } = record.change else {
            return Err(Failure::Conflict.into());
        };
        steps.push(Step {
            step: request.step,
            subject: request.subject,
            action: request.action,
            request_sha256: request.request_sha256,
            object_uid: request.object_uid,
            outcome: None,
        });
    }
    let mut statement = db.prepare(
        "SELECT e.payload FROM kubernetes_session_effects k JOIN events e ON e.seq=k.result_seq WHERE k.reservation_id=?1 AND k.result_seq IS NOT NULL ORDER BY k.step",
    )?;
    for payload in statement
        .query_map([reservation_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
    {
        let event: Event = serde_json::from_str(&payload)?;
        let Change::Result { result } = record(&event)?.change else {
            return Err(Failure::Conflict.into());
        };
        let step = steps
            .iter_mut()
            .find(|step| step.step == result.step)
            .ok_or(Failure::Conflict)?;
        step.outcome = Some(result.outcome);
    }
    Ok(steps)
}

fn record(event: &Event) -> anyhow::Result<Record> {
    conflict(event.kind.starts_with(PREFIX) && event.kind != GENESIS_KIND)?;
    let value = serde_json::to_value(&event.data)?;
    let record: Record = serde_json::from_value(value).map_err(|_| Failure::Conflict)?;
    conflict(
        record.version == 1
            && event.kind == record.change.kind()
            && event.id == record.change.operation_id()
            && record.input_hash == record.change.input_hash()?
            && record.post_state.reservation_id == record.reservation_id
            && event.correlation_id.as_deref() == Some(record.reservation_id.as_str()),
    )?;
    Ok(record)
}
fn event(record: &Record, at: DateTime<Utc>) -> anyhow::Result<Event> {
    let mut event = Event::new(record.change.kind());
    event.id = record.change.operation_id().into();
    event.timestamp = format_time(at);
    event.correlation_id = Some(record.reservation_id.clone());
    event.data = serde_json::to_value(record)?;
    self::record(&event)?;
    Ok(event)
}

/// The retained operation of this id, if any. The same id with another
/// input or reservation is a conflict; an exact match is a replay.
fn exact_operation(
    db: &Connection,
    operation_id: &str,
    input_hash: &str,
    reservation_id: &str,
    tenant: &str,
) -> anyhow::Result<Option<(StoredEvent, Record)>> {
    let stored = db
        .query_row(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE id=?1",
            [operation_id],
            row_event,
        )
        .optional()?;
    stored
        .map(|stored| {
            let record = record(&stored.event)?;
            conflict(
                record.input_hash == input_hash
                    && record.reservation_id == reservation_id
                    && record.tenant_id == tenant,
            )?;
            Ok((stored, record))
        })
        .transpose()
}

fn receipt(
    stored: &StoredEvent,
    record: &Record,
    namespace: &str,
) -> KubernetesSessionEffectReceiptV2 {
    KubernetesSessionEffectReceiptV2 {
        version: 1,
        operation_id: record.change.operation_id().into(),
        input_hash: record.input_hash.clone(),
        control_namespace_uuid: namespace.into(),
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
    }
}

/// The capability marker: one gateway-observed genesis after the reservation
/// genesis, with no session or command field.
fn genesis_marker(event: &Event) -> anyhow::Result<()> {
    let marker: Genesis = serde_json::from_value(event.data.clone())?;
    crate::tenancy::validate_tenant_id(&marker.tenant_id)?;
    uuid(&marker.control_namespace_uuid)?;
    hash(&marker.previous_hash)?;
    conflict(
        marker.version == 1
            && marker.protocol == "kubernetes_session_effect_ledger_v1"
            && marker.reservation_genesis_seq > 0
            && marker.previous_seq >= marker.reservation_genesis_seq
            && event.kind == GENESIS_KIND
            && event.session_id.is_none()
            && event.correlation_id.is_none()
            && event.command.is_none()
            && event.decision.is_none(),
    )
}

/// Capacity class of a ledger event: the broker named by its record.
pub(super) fn workload_audit_identity(
    event: &Event,
) -> anyhow::Result<Option<capacity::WorkloadAuditPrincipalV1>> {
    if !event.kind.starts_with(PREFIX) {
        return Ok(None);
    }
    if event.kind == GENESIS_KIND {
        genesis_marker(event)?;
        return Ok(None);
    }
    let record = record(event)?;
    uuid(&record.broker_id)?;
    Ok(Some(capacity::WorkloadAuditPrincipalV1::Broker {
        broker_id: record.broker_id,
    }))
}

/// Reservations settled by an event before `before`: their quota was
/// released at that point of the history.
pub(super) fn settled_before(
    db: &Connection,
    before: i64,
) -> anyhow::Result<std::collections::BTreeSet<String>> {
    let mut settled = std::collections::BTreeSet::new();
    if !enabled(db)? {
        return Ok(settled);
    }
    let mut statement =
        db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind=?1 AND seq<?2 ORDER BY seq")?;
    for stored in statement.query_map(params![SETTLED, before], row_event)? {
        let record = record(&stored?.event)?;
        conflict(record.post_state.settled && settled.insert(record.reservation_id))?;
    }
    Ok(settled)
}

/// Settled reservations hold no quota; used by the shared quota readers.
pub(crate) fn settled_clause(db: &Connection) -> anyhow::Result<&'static str> {
    Ok(if enabled(db)? {
        " AND reservation_id NOT IN (SELECT reservation_id FROM kubernetes_session_effect_states WHERE settled=1)"
    } else {
        ""
    })
}

/// Additive DDL and genesis marker of the capability, inside the backed-up
/// maintenance transaction (`Store::enable_kubernetes_session_effects_v1`).
pub(crate) fn initialize(store: &Store, tx: &Transaction<'_>) -> anyhow::Result<()> {
    require_schema(tx)?;
    conflict(!enabled(tx)?)?;
    let (reservation_genesis_seq, _) = migration::genesis(tx, &store.tenant_id)?;
    for (_, _, sql) in SCHEMA {
        tx.execute_batch(sql)?;
    }
    let (previous_seq, previous_hash) = audit_head(tx)?;
    let namespace: String = tx.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let mut event = Event::new(GENESIS_KIND);
    event.data = serde_json::to_value(Genesis {
        version: 1,
        protocol: "kubernetes_session_effect_ledger_v1".into(),
        tenant_id: store.tenant_id.clone(),
        control_namespace_uuid: namespace,
        reservation_genesis_seq,
        previous_seq,
        previous_hash,
    })?;
    genesis_marker(&event)?;
    let original = serde_json::to_value(&event)?;
    let (event, payload) = store.prepare_event(event)?;
    permission(serde_json::to_value(&event)? == original)?;
    // Only this exact maintenance marker uses the existing control reserve.
    let stored = store.append_in_transaction(tx, event, payload)?;
    tx.execute(
        "INSERT INTO kubernetes_session_effect_meta(singleton,genesis_seq,genesis_hash) VALUES(1,?1,?2)",
        params![stored.seq, stored.hash],
    )?;
    Ok(())
}

impl Store {
    /// Claim of one exact effect request (R1). `EmitOnce` is returned once;
    /// any replay of the same step returns its historical receipt with
    /// `ObserveOnly`.
    pub(crate) fn claim_kubernetes_session_effect(
        &self,
        identity: &BrokerIdentityV1,
        request: &KubernetesSessionEffectRequestV2,
        storage_policy: Option<&KubernetesSessionStoragePolicyV2>,
        window_seconds: u32,
    ) -> anyhow::Result<KubernetesSessionEffectClaimAckV2> {
        request.validate()?;
        require((1..=60).contains(&window_seconds))?;
        let cleanup = matches!(request.action, Action::Delete | Action::ReleaseFinalizer);
        let input_hash = request.input_hash()?;
        self.mutate(
            identity,
            &request.reservation_id,
            &request.operation_id,
            &input_hash,
            cleanup,
            |tx, intent, state, steps, at| {
                require(request.reservation_input_hash == intent.input_hash(&self.tenant_id)?)?;
                admit_order(state, steps, request)?;
                rederive(intent, request, storage_policy)?;
                let not_after = at
                    .checked_add_signed(chrono::Duration::seconds(i64::from(window_seconds)))
                    .ok_or(Failure::Unavailable)?;
                let _ = tx;
                let mut post = state.clone();
                post.last_step = request.step;
                Ok((
                    Change::Claim {
                        request: request.clone(),
                        issued_at: format_time(at),
                        not_after: format_time(not_after),
                    },
                    post,
                ))
            },
        )
        .and_then(|(stored, record, replayed, namespace)| {
            let Change::Claim {
                request: claimed,
                issued_at,
                not_after,
            } = record.change.clone()
            else {
                return Err(Failure::Conflict.into());
            };
            let ack = KubernetesSessionEffectClaimAckV2 {
                version: 1,
                request: claimed,
                disposition: if replayed {
                    KubernetesSessionEffectDispositionV2::ObserveOnly
                } else {
                    KubernetesSessionEffectDispositionV2::EmitOnce
                },
                issued_at,
                not_after,
                receipt: receipt(&stored, &record, &namespace),
                replayed,
            };
            ack.validate()?;
            Ok(ack)
        })
    }

    /// Observed result of one claimed step. `Applied` on a CREATE binds the
    /// object UID; a workspace claim UID is bound once across all
    /// reservations of the tenant (Q7).
    pub(crate) fn record_kubernetes_session_effect_result(
        &self,
        identity: &BrokerIdentityV1,
        result: &KubernetesSessionEffectResultV2,
    ) -> anyhow::Result<KubernetesSessionEffectStateV2> {
        result.validate()?;
        let input_hash = result.input_hash()?;
        self.mutate(
            identity,
            &result.reservation_id,
            &result.operation_id,
            &input_hash,
            true,
            |tx, _, state, steps, _| {
            let step = steps
                .iter()
                .find(|step| step.step == result.step)
                .ok_or(Failure::NotFound)?;
            let claim_operation: String = tx.query_row(
                "SELECT operation_id FROM kubernetes_session_effects WHERE reservation_id=?1 AND step=?2",
                params![result.reservation_id, result.step],
                |row| row.get(0),
            )?;
            conflict(
                step.outcome.is_none()
                    && claim_operation == result.claim_operation_id
                    && step.request_sha256 == result.request_sha256,
            )?;
            let mut post = state.clone();
            match (&result.outcome, step.action) {
                (KubernetesSessionEffectOutcomeV2::Applied { object_uid, .. }, Action::Create) => {
                    let slot = match step.subject {
                        Subject::WorkspaceClaim => &mut post.workspace_claim_uid,
                        Subject::SessionPod => &mut post.session_pod_uid,
                        Subject::ExportPod => &mut post.export_pod_uid,
                    };
                    conflict(slot.is_none())?;
                    *slot = Some(object_uid.clone());
                }
                (KubernetesSessionEffectOutcomeV2::Applied { object_uid, .. }, _) => {
                    conflict(step.object_uid.as_deref() == Some(object_uid.as_str()))?;
                }
                (KubernetesSessionEffectOutcomeV2::Uncertain { .. }, Action::Create) => {
                    post.unresolved_uncertain_steps += 1;
                }
                _ => {}
            }
            Ok((Change::Result { result: result.clone() }, post))
        })
        .map(|(_, record, _, _)| record.post_state)
    }

    /// Runtime-observed end of one Pod, by its exact UID.
    pub(crate) fn record_kubernetes_session_terminal(
        &self,
        identity: &BrokerIdentityV1,
        observed: &KubernetesSessionTerminalV2,
    ) -> anyhow::Result<KubernetesSessionEffectStateV2> {
        observed.validate()?;
        let input_hash = observed.input_hash()?;
        self.mutate(
            identity,
            &observed.reservation_id,
            &observed.operation_id,
            &input_hash,
            true,
            |_, _, state, steps, _| {
                conflict(
                    pod_uid(state, observed.subject) == Some(observed.pod_uid.as_str())
                        && !terminal(state, observed.subject),
                )?;
                if observed.end == KubernetesSessionPodEndV2::NeverStarted {
                    // A Pod whose gate release was applied, or may have been, may
                    // have run: only refused releases leave it never started.
                    conflict(!steps.iter().any(|step| {
                        step.subject == observed.subject
                            && step.action == Action::ReleaseGate
                            && !matches!(
                                step.outcome,
                                Some(KubernetesSessionEffectOutcomeV2::NotApplied { .. })
                            )
                    }))?;
                }
                let mut post = state.clone();
                match observed.subject {
                    Subject::SessionPod => post.session_terminal = true,
                    Subject::ExportPod => post.export_terminal = true,
                    Subject::WorkspaceClaim => return Err(Failure::InvalidRequest.into()),
                }
                Ok((
                    Change::Terminal {
                        terminal: observed.clone(),
                    },
                    post,
                ))
            },
        )
        .map(|(_, record, _, _)| record.post_state)
    }

    /// Release of the attempt's quota once every created Pod has an
    /// observed end and no CREATE remains uncertain.
    pub(crate) fn settle_kubernetes_session(
        &self,
        identity: &BrokerIdentityV1,
        settlement: &KubernetesSessionSettlementV2,
    ) -> anyhow::Result<KubernetesSessionEffectStateV2> {
        settlement.validate()?;
        let input_hash = settlement.input_hash()?;
        self.mutate(
            identity,
            &settlement.reservation_id,
            &settlement.operation_id,
            &input_hash,
            true,
            |_, intent, state, steps, _| {
                require(settlement.reservation_input_hash == intent.input_hash(&self.tenant_id)?)?;
                conflict(
                    !state.settled
                        && settlement.expected_last_step == state.last_step
                        && state.unresolved_uncertain_steps == 0
                        && steps.iter().all(|step| step.outcome.is_some())
                        && (state.session_pod_uid.is_none() || state.session_terminal)
                        && (state.export_pod_uid.is_none() || state.export_terminal),
                )?;
                let mut post = state.clone();
                post.settled = true;
                Ok((
                    Change::Settle {
                        settlement: settlement.clone(),
                    },
                    post,
                ))
            },
        )
        .map(|(_, record, _, _)| record.post_state)
    }

    /// Current effect position of one reservation of the authenticated broker.
    pub(crate) fn kubernetes_session_effect_state(
        &self,
        identity: &BrokerIdentityV1,
        reservation_id: &str,
    ) -> anyhow::Result<KubernetesSessionEffectStateV2> {
        uuid(reservation_id)?;
        let mut db = self.db.lock().map_err(|_| Failure::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        permission(enabled(&tx)?)?;
        let clock = workload::capture_control_time(&tx)?;
        let (_, original) = replay::reservation(&tx, reservation_id, &self.tenant_id)?;
        authorization::current(
            &tx,
            identity,
            &original.intent,
            &self.tenant_id,
            true,
            clock.at(),
        )?;
        let state = load_state(&tx, reservation_id)?
            .map_or_else(|| initial_state(reservation_id), |(state, _)| state);
        self.commit_authorized(tx, &clock)?;
        Ok(state)
    }

    /// Shared transaction of every ledger change: fresh broker identity,
    /// exact replay, one audited event and its projection rows.
    fn mutate(
        &self,
        identity: &BrokerIdentityV1,
        reservation_id: &str,
        operation_id: &str,
        input_hash: &str,
        successor_allowed: bool,
        decide: impl FnOnce(
            &Connection,
            &KubernetesSessionReservationIntentV2,
            &KubernetesSessionEffectStateV2,
            &[Step],
            DateTime<Utc>,
        ) -> anyhow::Result<(Change, KubernetesSessionEffectStateV2)>,
    ) -> anyhow::Result<(StoredEvent, Record, bool, String)> {
        let mut db = self.db.lock().map_err(|_| Failure::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        if !enabled(&tx)? {
            return Err(Failure::MigrationRequired.into());
        }
        let clock = workload::capture_control_time(&tx)?;
        let (_, original) = replay::reservation(&tx, reservation_id, &self.tenant_id)?;
        let intent = original.intent.clone();
        let broker = authorization::current(
            &tx,
            identity,
            &intent,
            &self.tenant_id,
            successor_allowed,
            clock.at(),
        )?;
        let namespace = intent.ids()?.control_namespace_uuid.to_string();
        let (snapshot, _) = replay::load_snapshot(&tx, reservation_id, &self.tenant_id)?
            .ok_or(Failure::NotFound)?;
        // A reservation closed before any effect never gains one.
        conflict(
            snapshot.state.state == KubernetesSessionReservationStateV2::Reserved
                && snapshot.state.quota_held,
        )?;
        let (state, previous_seq) = match load_state(&tx, reservation_id)? {
            Some((state, seq)) => (state, Some(seq)),
            None => (initial_state(reservation_id), None),
        };
        if let Some((stored, record)) = exact_operation(
            &tx,
            operation_id,
            input_hash,
            reservation_id,
            &self.tenant_id,
        )? {
            self.commit_authorized(tx, &clock)?;
            return Ok((stored, record, true, namespace));
        }
        let steps = load_steps(&tx, reservation_id)?;
        let (change, post_state) = decide(&tx, &intent, &state, &steps, clock.at())?;
        conflict(change.operation_id() == operation_id && change.input_hash()? == input_hash)?;
        let record = Record {
            version: 1,
            tenant_id: self.tenant_id.clone(),
            broker_id: identity.broker_id().into(),
            incarnation_id: broker.incarnation_id.clone(),
            reservation_id: reservation_id.into(),
            input_hash: change.input_hash()?,
            change,
            post_state,
        };
        let event = event(&record, clock.at())?;
        require(serde_json::to_vec(&event)?.len() <= 64 * 1024)?;
        let original_event = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        permission(serde_json::to_value(&event)? == original_event)?;
        let stored = self.append_authorized_workload_in_transaction(
            &tx,
            event,
            payload,
            capacity::WorkloadAuditPrincipalV1::Broker {
                broker_id: identity.broker_id().into(),
            },
            &clock,
        )?;
        write_projection(&tx, &record, &stored, previous_seq)?;
        self.commit_authorized(tx, &clock)?;
        Ok((stored, record, false, namespace))
    }
}

/// Projection rows of one appended change, checked against the record.
fn write_projection(
    tx: &Connection,
    record: &Record,
    stored: &StoredEvent,
    previous_seq: Option<i64>,
) -> anyhow::Result<()> {
    match &record.change {
        Change::Claim { request, .. } => {
            conflict(
                tx.execute(
                    "INSERT INTO kubernetes_session_effects(reservation_id,step,operation_id,input_hash,subject,action,request_sha256,state,claim_seq,result_seq) VALUES(?1,?2,?3,?4,?5,?6,?7,'claimed',?8,NULL)",
                    params![
                        record.reservation_id,
                        request.step,
                        request.operation_id,
                        record.input_hash,
                        subject_name(request.subject),
                        action_name(request.action),
                        request.request_sha256,
                        stored.seq
                    ],
                )? == 1,
            )?;
        }
        Change::Result { result } => {
            conflict(
                tx.execute(
                    "UPDATE kubernetes_session_effects SET state=?3,result_seq=?4 WHERE reservation_id=?1 AND step=?2 AND state='claimed' AND result_seq IS NULL",
                    params![
                        record.reservation_id,
                        result.step,
                        outcome_state(Some(&result.outcome)),
                        stored.seq
                    ],
                )? == 1,
            )?;
            if let KubernetesSessionEffectOutcomeV2::Applied { object_uid, .. } = &result.outcome {
                if record.post_state.workspace_claim_uid.as_deref() == Some(object_uid.as_str()) {
                    let fresh: bool = tx.query_row(
                        "SELECT NOT EXISTS(SELECT 1 FROM kubernetes_session_claims WHERE reservation_id=?1)",
                        [&record.reservation_id],
                        |row| row.get(0),
                    )?;
                    if fresh {
                        // The primary key refuses any claim UID already bound
                        // to another reservation: a claim is never reused (Q7).
                        tx.execute(
                            "INSERT INTO kubernetes_session_claims(claim_uid,reservation_id,bound_seq) VALUES(?1,?2,?3)",
                            params![object_uid, record.reservation_id, stored.seq],
                        )
                        .map_err(|_| Failure::Conflict)?;
                    }
                }
            }
        }
        Change::Terminal { .. } | Change::Settle { .. } => {}
    }
    let json = String::from_utf8(crate::control::kubernetes_session::validation::canonical(
        &record.post_state,
    )?)?;
    let changed = match previous_seq {
        Some(previous) => tx.execute(
            "UPDATE kubernetes_session_effect_states SET last_step=?2,settled=?3,state_json=?4,last_event_seq=?5 WHERE reservation_id=?1 AND last_event_seq=?6",
            params![
                record.reservation_id,
                record.post_state.last_step,
                i64::from(record.post_state.settled),
                json,
                stored.seq,
                previous
            ],
        )?,
        None => tx.execute(
            "INSERT INTO kubernetes_session_effect_states(reservation_id,last_step,settled,state_json,last_event_seq) VALUES(?1,?2,?3,?4,?5)",
            params![
                record.reservation_id,
                record.post_state.last_step,
                i64::from(record.post_state.settled),
                json,
                stored.seq
            ],
        )?,
    };
    conflict(changed == 1)
}

/// Complete replay of the family against its projection rows.
pub(crate) fn validate(db: &Connection) -> anyhow::Result<()> {
    if !enabled(db)? {
        let present: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE kind GLOB 'control.kubernetes.session_effect.*')",
            [],
            |row| row.get(0),
        )?;
        return conflict(!present);
    }
    let marker: (i64, String) = db.query_row(
        "SELECT genesis_seq,genesis_hash FROM kubernetes_session_effect_meta WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let genesis: (i64, String) = db.query_row(
        "SELECT seq,hash FROM events WHERE kind=?1",
        [GENESIS_KIND],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    conflict(marker == genesis)?;
    let mut states = BTreeMap::<String, (KubernetesSessionEffectStateV2, i64)>::new();
    let mut statement = db.prepare(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind GLOB 'control.kubernetes.session_effect.*' AND kind<>?1 ORDER BY seq",
    )?;
    for stored in statement.query_map([GENESIS_KIND], row_event)? {
        let stored = stored?;
        conflict(stored.seq > marker.0)?;
        let record = record(&stored.event)?;
        let previous = states
            .get(&record.reservation_id)
            .map(|(state, _)| state.clone());
        let before = previous.unwrap_or_else(|| initial_state(&record.reservation_id));
        // Each change only moves the projection the way its kind allows.
        conflict(match &record.change {
            Change::Claim { request, .. } => record.post_state.last_step == request.step,
            _ => record.post_state.last_step == before.last_step,
        })?;
        states.insert(
            record.reservation_id.clone(),
            (record.post_state.clone(), stored.seq),
        );
    }
    let mut query = db.prepare(
        "SELECT reservation_id,state_json,last_event_seq FROM kubernetes_session_effect_states ORDER BY reservation_id",
    )?;
    let actual = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let expected = states
        .iter()
        .map(|(id, (state, seq))| {
            Ok((
                id.clone(),
                String::from_utf8(crate::control::kubernetes_session::validation::canonical(
                    state,
                )?)?,
                *seq,
            ))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    conflict(actual == expected)
}

#[cfg(test)]
#[path = "effects_tests.rs"]
mod tests;
