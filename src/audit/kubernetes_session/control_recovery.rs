//! Trusted-control recovery of a retained, strictly effect-free reservation.
//! This module issues no runtime, transport, CREATE, release or Reserve proof.
use super::*;
use crate::api::AuthenticatedControlRecoveryV2;
use model::{Change, Creator, Record};
use std::time::{Duration, Instant};

/// Preserve the exact broker evidence wire shape while adding a separate
/// closed control variant. Deserialization restores descriptive facts only.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum JournalAuthorization {
    Broker(authorization::Evidence),
    Control(ControlEvidence),
}
impl From<authorization::Evidence> for JournalAuthorization {
    fn from(value: authorization::Evidence) -> Self {
        Self::Broker(value)
    }
}
impl JournalAuthorization {
    pub(super) fn broker(&self) -> anyhow::Result<&authorization::Evidence> {
        match self {
            Self::Broker(value) => Ok(value),
            Self::Control(_) => Err(Failure::PermissionDenied.into()),
        }
    }
    pub(super) fn control(&self) -> Option<&ControlEvidence> {
        match self {
            Self::Control(value) => Some(value),
            Self::Broker(_) => None,
        }
    }
}

/// Observed shared-control authentication, never a person or broker identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ControlActor {
    pub kind: String,
    pub tenant_id: String,
    pub mechanism: String,
    pub role: String,
    pub auth_origin: String,
    pub auth_epoch_id: String,
    pub request_auth_id: String,
    pub authenticated_at: String,
}
impl ControlActor {
    fn capture(auth: &AuthenticatedControlRecoveryV2) -> anyhow::Result<Self> {
        let actor = Self {
            kind: "control_shared_runner".into(),
            tenant_id: auth.tenant_id().into(),
            mechanism: "legacy_runner_bearer".into(),
            role: "runner".into(),
            auth_origin: "observed_gateway_verification".into(),
            auth_epoch_id: auth.auth_epoch_id().to_string(),
            request_auth_id: auth.request_auth_id().to_string(),
            authenticated_at: auth.authenticated_at().into(),
        };
        actor.validate()?;
        Ok(actor)
    }
    pub(super) fn validate(&self) -> anyhow::Result<()> {
        crate::tenancy::validate_tenant_id(&self.tenant_id)?;
        uuid(&self.auth_epoch_id)?;
        uuid(&self.request_auth_id)?;
        canonical_time(&self.authenticated_at)?;
        permission(
            self.kind == "control_shared_runner"
                && self.mechanism == "legacy_runner_bearer"
                && self.role == "runner"
                && self.auth_origin == "observed_gateway_verification",
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ControlScope {
    ControlRecoverBeforeEffect,
    ControlRecoveryReceiptLookup,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ControlAction {
    ObserveReserved,
    CloseReserved,
    LookupCloseReceipt,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ControlEvidence {
    pub version: u32,
    pub scope: ControlScope,
    pub actor: ControlActor,
    pub control_namespace_uuid: String,
    pub creator_reference: Creator,
    pub reservation_seq: i64,
    pub reservation_hash: String,
    pub reservation_input_hash: String,
    pub session_hash: String,
    pub attempt_hash: String,
    pub profile_hash: String,
    pub origin_seq: i64,
    pub origin_hash: String,
    pub state: KubernetesSessionReservationStateV2,
    pub revision: u64,
    pub effect_fence: KubernetesSessionEffectFenceV2,
    pub action: ControlAction,
    pub target_hash: String,
    pub policy_snapshot: KubernetesSessionControlRecoveryPolicyV2,
    pub policy_hash: String,
    pub issued_at: String,
    pub expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close_seq: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close_hash: Option<String>,
    pub decision_hash: String,
}
fn canonical_time(value: &str) -> anyhow::Result<DateTime<Utc>> {
    let at = parse_time(value)?;
    permission(value.len() == 24 && format_time(at) == value)?;
    Ok(at)
}
fn original_creator(creator: &Creator) -> anyhow::Result<(i64, &str, &model::IdentityEvidence)> {
    let Creator::Reservation {
        reservation_seq,
        reservation_hash,
        actor,
    } = creator
    else {
        return Err(Failure::PermissionDenied.into());
    };
    actor.identity()?;
    hash(reservation_hash)?;
    permission(*reservation_seq > 0)?;
    Ok((*reservation_seq, reservation_hash, actor))
}
impl ControlEvidence {
    fn commitment(&self) -> anyhow::Result<String> {
        match self.scope {
            ControlScope::ControlRecoverBeforeEffect => digest(
                b"ai-guard:kubernetes-session-control-recover-before-effect:v2\0",
                &json!([2,self.actor,self.control_namespace_uuid,self.creator_reference,self.reservation_seq,
                    self.reservation_hash,self.reservation_input_hash,self.session_hash,self.attempt_hash,self.profile_hash,
                    self.origin_seq,self.origin_hash,"reserved",1,{"state":"never_issued","revision":0},
                    self.action,self.target_hash,self.policy_snapshot,self.policy_hash,self.issued_at,self.expires_at]),
            ),
            ControlScope::ControlRecoveryReceiptLookup => digest(
                b"ai-guard:kubernetes-session-control-recovery-receipt:v2\0",
                &json!([
                    2,
                    self.actor,
                    self.control_namespace_uuid,
                    self.creator_reference,
                    self.reservation_input_hash,
                    self.target_hash,
                    self.close_seq,
                    self.close_hash,
                    self.origin_seq,
                    self.origin_hash,
                    self.policy_hash,
                    self.issued_at,
                    self.expires_at
                ]),
            ),
        }
    }
    pub(super) fn validate(
        &self,
        intent: &KubernetesSessionReservationIntentV2,
        at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        self.actor.validate()?;
        intent.validate()?;
        let issued = canonical_time(&self.issued_at)?;
        let expires = canonical_time(&self.expires_at)?;
        let authenticated = canonical_time(&self.actor.authenticated_at)?;
        let ids = intent.ids()?;
        let (seq, reserve_hash, creator) = original_creator(&self.creator_reference)?;
        for value in [
            &self.reservation_hash,
            &self.reservation_input_hash,
            &self.session_hash,
            &self.attempt_hash,
            &self.profile_hash,
            &self.origin_hash,
            &self.target_hash,
            &self.policy_hash,
            &self.decision_hash,
        ] {
            hash(value)?;
        }
        permission(
            self.version == 2
                && self.control_namespace_uuid == ids.control_namespace_uuid.to_string()
                && creator.tenant_id == self.actor.tenant_id
                && creator.broker_id == ids.broker_id.to_string()
                && creator.incarnation_id == ids.incarnation_id.to_string()
                && self.reservation_seq == seq
                && self.reservation_hash == reserve_hash
                && seq <= self.origin_seq
                && self.reservation_input_hash == intent.input_hash(&self.actor.tenant_id)?
                && self.session_hash == intent.session_hash(&self.actor.tenant_id)?
                && self.attempt_hash == intent.attempt_hash(&self.actor.tenant_id)?
                && self.profile_hash == intent.profile_hash()?
                && self.policy_snapshot == self.policy_snapshot.canonicalized()?
                && self.policy_hash == self.policy_snapshot.policy_hash()?
                && self.policy_snapshot.approves(&creator.broker_id)?
                && authenticated <= issued
                && issued <= at
                && at < expires
                && expires
                    <= issued
                        .checked_add_signed(chrono::Duration::seconds(i64::from(
                            self.policy_snapshot.decision_ttl_seconds,
                        )))
                        .ok_or(Failure::Unavailable)?
                && expires
                    <= authenticated
                        .checked_add_signed(chrono::Duration::seconds(60))
                        .ok_or(Failure::Unavailable)?
                && self.effect_fence == KubernetesSessionEffectFenceV2::NeverIssued { revision: 0 }
                && self.decision_hash == self.commitment()?,
        )?;
        match self.scope {
            ControlScope::ControlRecoverBeforeEffect => permission(
                matches!(
                    self.action,
                    ControlAction::ObserveReserved | ControlAction::CloseReserved
                ) && self.state == KubernetesSessionReservationStateV2::Reserved
                    && self.revision == 1
                    && self.close_seq.is_none()
                    && self.close_hash.is_none(),
            ),
            ControlScope::ControlRecoveryReceiptLookup => {
                let close_seq = self.close_seq.ok_or(Failure::PermissionDenied)?;
                hash(
                    self.close_hash
                        .as_deref()
                        .ok_or(Failure::PermissionDenied)?,
                )?;
                permission(
                    self.action == ControlAction::LookupCloseReceipt
                        && self.state == KubernetesSessionReservationStateV2::ClosedBeforeEffect
                        && self.revision == 2
                        && seq < close_seq
                        && close_seq <= self.origin_seq,
                )
            }
        }
    }
}

/// These borrow the actual proof, never the issuer's short-lived Usage guard.
pub(crate) struct ControlRecoverBeforeEffectV2<'auth> {
    auth: &'auth AuthenticatedControlRecoveryV2,
    evidence: ControlEvidence,
    monotone_deadline: Instant,
}
pub(crate) struct ControlRecoveryReceiptLookupV2<'auth> {
    auth: &'auth AuthenticatedControlRecoveryV2,
    evidence: ControlEvidence,
    monotone_deadline: Instant,
}
pub(crate) enum KubernetesSessionControlRecoveryAuthorizationV2<'auth> {
    BeforeEffect(ControlRecoverBeforeEffectV2<'auth>),
    ReceiptLookup(ControlRecoveryReceiptLookupV2<'auth>),
}

fn target_id(target: &KubernetesSessionControlRecoveryTargetV2) -> &str {
    match target {
        KubernetesSessionControlRecoveryTargetV2::ObserveReserved { reservation_id, .. }
        | KubernetesSessionControlRecoveryTargetV2::LookupCloseReceipt { reservation_id, .. } => {
            reservation_id
        }
        KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect { close } => {
            &close.reservation_id
        }
    }
}
fn validate_target(target: &KubernetesSessionControlRecoveryTargetV2) -> anyhow::Result<()> {
    uuid(target_id(target))?;
    match target {
        KubernetesSessionControlRecoveryTargetV2::ObserveReserved {
            reservation_input_hash,
            ..
        } => hash(reservation_input_hash),
        KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect { close } => close.validate(),
        KubernetesSessionControlRecoveryTargetV2::LookupCloseReceipt {
            reservation_input_hash,
            close_input_hash,
            operation_id,
            ..
        } => {
            hash(reservation_input_hash)?;
            hash(close_input_hash)?;
            uuid(operation_id)?;
            permission(operation_id != target_id(target))
        }
    }
}
fn reserved(snapshot: &KubernetesSessionReservationSnapshotV2) -> anyhow::Result<()> {
    snapshot.state.validate()?;
    conflict(
        snapshot.state.state == KubernetesSessionReservationStateV2::Reserved
            && snapshot.state.revision == 1
            && snapshot.state.quota_held
            && snapshot.state.effect_fence
                == KubernetesSessionEffectFenceV2::NeverIssued { revision: 0 },
    )
}
fn origin(db: &Connection, evidence: &ControlEvidence, before: Option<i64>) -> anyhow::Result<()> {
    let (marker, _) = migration::genesis(db, &evidence.actor.tenant_id)?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let actual: Option<String> = db
        .query_row(
            "SELECT hash FROM events WHERE seq=?1",
            [evidence.origin_seq],
            |row| row.get(0),
        )
        .optional()?;
    permission(
        namespace == evidence.control_namespace_uuid
            && evidence.origin_seq >= marker
            && evidence.origin_seq >= evidence.reservation_seq
            && before.is_none_or(|seq| evidence.origin_seq < seq)
            && actual.as_deref() == Some(evidence.origin_hash.as_str()),
    )
}
fn matches_current_auth(
    auth: &AuthenticatedControlRecoveryV2,
    evidence: &ControlEvidence,
) -> anyhow::Result<()> {
    permission(ControlActor::capture(auth)? == evidence.actor)
}
fn live_deadline(evidence: &ControlEvidence, monotone_deadline: Instant) -> anyhow::Result<()> {
    let now = Utc::now();
    permission(
        canonical_time(&evidence.issued_at)? <= now
            && now < canonical_time(&evidence.expires_at)?
            && Instant::now() < monotone_deadline,
    )
}
fn current_scope(
    db: &Connection,
    auth: &AuthenticatedControlRecoveryV2,
    evidence: &ControlEvidence,
    intent: &KubernetesSessionReservationIntentV2,
    policy: &KubernetesSessionControlRecoveryPolicyV2,
    at: DateTime<Utc>,
    deadline: Instant,
) -> anyhow::Result<()> {
    permission(
        auth.tenant_id() == evidence.actor.tenant_id
            && policy.policy_hash()? == evidence.policy_hash,
    )?;
    matches_current_auth(auth, evidence)?;
    evidence.validate(intent, at)?;
    origin(db, evidence, None)?;
    live_deadline(evidence, deadline)
}
fn creator(db: &Connection, id: &str, tenant: &str) -> anyhow::Result<Creator> {
    let (stored, original) = replay::reservation(db, id, tenant)?;
    Ok(Creator::Reservation {
        reservation_seq: stored.seq,
        reservation_hash: stored.hash,
        actor: original.actor.identity()?,
    })
}

/// Called by the common model parser for the genuinely distinct control arm.
pub(super) fn validate_record(record: &Record) -> anyhow::Result<()> {
    let evidence = record
        .authorization
        .control()
        .ok_or(Failure::PermissionDenied)?;
    let actor: ControlActor = serde_json::from_value(serde_json::to_value(&record.actor)?)?;
    permission(
        actor == evidence.actor
            && record.creator == evidence.creator_reference
            && record.previous_revision == 1
            && evidence.scope == ControlScope::ControlRecoverBeforeEffect
            && evidence.action == ControlAction::CloseReserved,
    )?;
    evidence.validate(&record.intent, canonical_time(&record.received_at)?)?;
    let Change::CloseBeforeEffect { close } = &record.change else {
        return Err(Failure::PermissionDenied.into());
    };
    permission(
        close.input_hash()? == evidence.target_hash && record.input_hash == evidence.target_hash,
    )?;
    let before = KubernetesSessionReservationStateDataV2 {
        state: KubernetesSessionReservationStateV2::Reserved,
        revision: 1,
        quota_held: true,
        effect_fence: KubernetesSessionEffectFenceV2::NeverIssued { revision: 0 },
        last_close: None,
    };
    let after = model::transition(
        &record.intent,
        Some(&before),
        &record.change,
        &evidence.actor.tenant_id,
    )?;
    conflict(after == record.post_state)
}
/// Historical validation intentionally does not require a live creator grant
/// or reconstruct the gateway's past shared credential from descriptive data.
pub(super) fn validate_historical(
    db: &Connection,
    stored: &StoredEvent,
    record: &Record,
    tenant: &str,
) -> anyhow::Result<()> {
    validate_record(record)?;
    let evidence = record
        .authorization
        .control()
        .ok_or(Failure::PermissionDenied)?;
    permission(
        tenant == evidence.actor.tenant_id
            && canonical_time(&record.received_at)? <= parse_time(&stored.received_at)?,
    )?;
    origin(db, evidence, Some(stored.seq))?;
    let (original_stored, original) =
        replay::reservation(db, record.intent.reservation_id(), tenant)?;
    let (seq, reserve_hash, actor) = original_creator(&record.creator)?;
    permission(
        seq == original_stored.seq
            && reserve_hash == original_stored.hash
            && seq < stored.seq
            && *actor == original.actor.identity()?
            && original.intent.same(&record.intent)?,
    )?;
    replay::ensure_no_legacy_identity(db, &record.intent)
}

fn prepare_close(
    store: &Store,
    db: &Connection,
    evidence: &ControlEvidence,
    intent: &KubernetesSessionReservationIntentV2,
    close: &KubernetesSessionReservationCloseV2,
    at: DateTime<Utc>,
) -> anyhow::Result<(Record, Event, String)> {
    evidence.validate(intent, at)?;
    permission(
        evidence.scope == ControlScope::ControlRecoverBeforeEffect
            && evidence.action == ControlAction::CloseReserved
            && evidence.target_hash == close.input_hash()?,
    )?;
    let (previous, _) =
        replay::load_snapshot(db, intent.reservation_id(), &evidence.actor.tenant_id)?
            .ok_or(Failure::NotFound)?;
    reserved(&previous)?;
    conflict(previous.intent.same(intent)?)?;
    let change = Change::CloseBeforeEffect {
        close: close.clone(),
    };
    let post_state = model::transition(
        intent,
        Some(&previous.state),
        &change,
        &evidence.actor.tenant_id,
    )?;
    let record = Record {
        version: 2,
        input_hash: close.input_hash()?,
        received_at: format_time(at),
        actor: serde_json::from_value(serde_json::to_value(&evidence.actor)?)?,
        creator: evidence.creator_reference.clone(),
        authorization: JournalAuthorization::Control(evidence.clone()),
        intent: intent.clone(),
        previous_revision: 1,
        change,
        post_state,
    };
    let event = model::event(&record)?;
    require(serde_json::to_vec(&event)?.len() <= 128 * 1024)?;
    let original = serde_json::to_value(&event)?;
    let (event, payload) = store.prepare_event(event)?;
    permission(serde_json::to_value(&event)? == original)?;
    require(payload.len() <= 128 * 1024)?;
    Ok((record, event, payload))
}
fn exact_close(
    db: &Connection,
    snapshot: &KubernetesSessionReservationSnapshotV2,
    close: &KubernetesSessionReservationCloseV2,
    tenant: &str,
) -> anyhow::Result<(StoredEvent, Record)> {
    let (stored, record) = replay::operation(db, &close.operation_id)?.ok_or(Failure::NotFound)?;
    conflict(
        record.change
            == Change::CloseBeforeEffect {
                close: close.clone(),
            }
            && record.intent.same(&snapshot.intent)?
            && record.post_state == snapshot.state
            && snapshot.state.state == KubernetesSessionReservationStateV2::ClosedBeforeEffect
            && snapshot.state.last_close.as_ref() == Some(close),
    )?;
    replay::validate_historical(db, &stored, &record, tenant)?;
    Ok((stored, record))
}

impl Store {
    pub(crate) fn authorize_kubernetes_session_control_recovery<'auth>(
        &self,
        auth: &'auth AuthenticatedControlRecoveryV2,
        target: &KubernetesSessionControlRecoveryTargetV2,
        policy: &KubernetesSessionControlRecoveryPolicyV2,
    ) -> anyhow::Result<KubernetesSessionControlRecoveryAuthorizationV2<'auth>> {
        validate_target(target)?;
        let policy = policy.canonicalized()?;
        // Reauth/collision scans may acquire Stores. No SQLite mutex yet.
        let _usage = auth.acquire_usage(self)?;
        permission(auth.tenant_id() == self.tenant_id)?;
        let actor = ControlActor::capture(auth)?;
        let mut db = self.db.lock().map_err(|_| Failure::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        let (snapshot, _) = replay::load_snapshot(&tx, target_id(target), &self.tenant_id)?
            .ok_or(Failure::NotFound)?;
        let creator = creator(&tx, target_id(target), &self.tenant_id)?;
        let (reserve_seq, reserve_hash, original_actor) = original_creator(&creator)?;
        permission(policy.approves(&original_actor.broker_id)?)?;
        replay::ensure_no_legacy_identity(&tx, &snapshot.intent)?;
        let (action, target_hash, close_ref) = match target {
            KubernetesSessionControlRecoveryTargetV2::ObserveReserved {
                reservation_input_hash,
                ..
            } => {
                permission(*reservation_input_hash == snapshot.input_hash)?;
                reserved(&snapshot)?;
                (
                    ControlAction::ObserveReserved,
                    reservation_input_hash.clone(),
                    None,
                )
            }
            KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect { close } => {
                permission(close.reservation_input_hash == snapshot.input_hash)?;
                if snapshot.state.state == KubernetesSessionReservationStateV2::ClosedBeforeEffect {
                    let (stored, _) = exact_close(&tx, &snapshot, close, &self.tenant_id)?;
                    (
                        ControlAction::LookupCloseReceipt,
                        close.input_hash()?,
                        Some((stored.seq, stored.hash)),
                    )
                } else {
                    reserved(&snapshot)?;
                    (ControlAction::CloseReserved, close.input_hash()?, None)
                }
            }
            KubernetesSessionControlRecoveryTargetV2::LookupCloseReceipt {
                reservation_input_hash,
                close_input_hash,
                operation_id,
                ..
            } => {
                permission(*reservation_input_hash == snapshot.input_hash)?;
                let close = snapshot
                    .state
                    .last_close
                    .as_ref()
                    .ok_or(Failure::Conflict)?;
                conflict(
                    close.operation_id == *operation_id && close.input_hash()? == *close_input_hash,
                )?;
                let (stored, _) = exact_close(&tx, &snapshot, close, &self.tenant_id)?;
                (
                    ControlAction::LookupCloseReceipt,
                    close_input_hash.clone(),
                    Some((stored.seq, stored.hash)),
                )
            }
        };
        let (origin_seq, origin_hash) = audit_head(&tx)?;
        let ttl = Duration::from_secs(u64::from(policy.decision_ttl_seconds));
        let expires = clock
            .at()
            .checked_add_signed(chrono::Duration::seconds(i64::from(
                policy.decision_ttl_seconds,
            )))
            .ok_or(Failure::Unavailable)?
            .min(
                canonical_time(&actor.authenticated_at)?
                    .checked_add_signed(chrono::Duration::seconds(60))
                    .ok_or(Failure::Unavailable)?,
            );
        let monotone_deadline = Instant::now()
            .checked_add(ttl)
            .ok_or(Failure::Unavailable)?
            .min(auth.monotone_deadline());
        let is_lookup = close_ref.is_some();
        let mut evidence = ControlEvidence {
            version: 2,
            scope: if is_lookup {
                ControlScope::ControlRecoveryReceiptLookup
            } else {
                ControlScope::ControlRecoverBeforeEffect
            },
            actor,
            control_namespace_uuid: snapshot.intent.ids()?.control_namespace_uuid.to_string(),
            creator_reference: creator.clone(),
            reservation_seq: reserve_seq,
            reservation_hash: reserve_hash.into(),
            reservation_input_hash: snapshot.input_hash.clone(),
            session_hash: snapshot.session_hash.clone(),
            attempt_hash: snapshot.attempt_hash.clone(),
            profile_hash: snapshot.profile_hash.clone(),
            origin_seq,
            origin_hash,
            state: snapshot.state.state,
            revision: snapshot.state.revision,
            effect_fence: snapshot.state.effect_fence.clone(),
            action,
            target_hash,
            policy_hash: policy.policy_hash()?,
            policy_snapshot: policy,
            issued_at: format_time(clock.at()),
            expires_at: format_time(expires),
            close_seq: close_ref.as_ref().map(|(seq, _)| *seq),
            close_hash: close_ref.map(|(_, hash)| hash),
            decision_hash: String::new(),
        };
        evidence.decision_hash = evidence.commitment()?;
        evidence.validate(&snapshot.intent, clock.at())?;
        origin(&tx, &evidence, None)?;
        if let KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect { close } = target
            && !is_lookup
        {
            conflict(replay::operation(&tx, &close.operation_id)?.is_none())?;
            prepare_close(self, &tx, &evidence, &snapshot.intent, close, clock.at())?;
        }
        live_deadline(&evidence, monotone_deadline)?;
        self.commit_authorized(tx, &clock)?;
        Ok(if is_lookup {
            KubernetesSessionControlRecoveryAuthorizationV2::ReceiptLookup(
                ControlRecoveryReceiptLookupV2 {
                    auth,
                    evidence,
                    monotone_deadline,
                },
            )
        } else {
            KubernetesSessionControlRecoveryAuthorizationV2::BeforeEffect(
                ControlRecoverBeforeEffectV2 {
                    auth,
                    evidence,
                    monotone_deadline,
                },
            )
        })
    }

    pub(crate) fn observe_kubernetes_session_control_recovery(
        &self,
        auth: &AuthenticatedControlRecoveryV2,
        cap: &ControlRecoverBeforeEffectV2<'_>,
        policy: &KubernetesSessionControlRecoveryPolicyV2,
    ) -> anyhow::Result<KubernetesSessionReservationCurrentV2> {
        let _usage = auth.acquire_usage(self)?;
        permission(
            std::ptr::eq(auth, cap.auth)
                && cap.evidence.scope == ControlScope::ControlRecoverBeforeEffect
                && cap.evidence.action == ControlAction::ObserveReserved,
        )?;
        let mut db = self.db.lock().map_err(|_| Failure::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        let (snapshot, seq) = replay::load_snapshot(
            &tx,
            cap.evidence
                .creator_reservation_id(&tx, &self.tenant_id)?
                .as_str(),
            &self.tenant_id,
        )?
        .ok_or(Failure::NotFound)?;
        current_scope(
            &tx,
            auth,
            &cap.evidence,
            &snapshot.intent,
            policy,
            clock.at(),
            cap.monotone_deadline,
        )?;
        permission(snapshot.input_hash == cap.evidence.target_hash)?;
        reserved(&snapshot)?;
        let hash: String = tx.query_row("SELECT hash FROM events WHERE seq=?1", [seq], |row| {
            row.get(0)
        })?;
        live_deadline(&cap.evidence, cap.monotone_deadline)?;
        self.commit_authorized(tx, &clock)?;
        Ok(KubernetesSessionReservationCurrentV2 {
            version: 2,
            snapshot,
            last_event_seq: seq,
            last_event_hash: hash,
        })
    }

    pub(crate) fn close_kubernetes_session_control_before_effect(
        &self,
        auth: &AuthenticatedControlRecoveryV2,
        cap: &ControlRecoverBeforeEffectV2<'_>,
        close: &KubernetesSessionReservationCloseV2,
        policy: &KubernetesSessionControlRecoveryPolicyV2,
    ) -> anyhow::Result<KubernetesSessionReservationMutationResponseV2> {
        close.validate()?;
        let _usage = auth.acquire_usage(self)?;
        permission(
            std::ptr::eq(auth, cap.auth)
                && cap.evidence.scope == ControlScope::ControlRecoverBeforeEffect
                && cap.evidence.action == ControlAction::CloseReserved
                && close.input_hash()? == cap.evidence.target_hash,
        )?;
        let mut db = self.db.lock().map_err(|_| Failure::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        let (snapshot, _) = replay::load_snapshot(&tx, &close.reservation_id, &self.tenant_id)?
            .ok_or(Failure::NotFound)?;
        current_scope(
            &tx,
            auth,
            &cap.evidence,
            &snapshot.intent,
            policy,
            clock.at(),
            cap.monotone_deadline,
        )?;
        if snapshot.state.state == KubernetesSessionReservationStateV2::ClosedBeforeEffect {
            let (stored, record) = exact_close(&tx, &snapshot, close, &self.tenant_id)?;
            let response = KubernetesSessionReservationMutationResponseV2 {
                version: 2,
                receipt: model::receipt(&stored, &record)?,
                snapshot,
                replayed: true,
            };
            live_deadline(&cap.evidence, cap.monotone_deadline)?;
            self.commit_authorized(tx, &clock)?;
            return Ok(response);
        }
        reserved(&snapshot)?;
        replay::ensure_no_legacy_identity(&tx, &snapshot.intent)?;
        // Global operation identity collision is checked before append too.
        conflict(replay::operation(&tx, &close.operation_id)?.is_none())?;
        let (record, event, payload) = prepare_close(
            self,
            &tx,
            &cap.evidence,
            &snapshot.intent,
            close,
            clock.at(),
        )?;
        live_deadline(&cap.evidence, cap.monotone_deadline)?;
        let (_, _, historical_actor) = original_creator(&cap.evidence.creator_reference)?;
        let stored = self.append_authorized_workload_in_transaction(
            &tx,
            event,
            payload,
            capacity::WorkloadAuditPrincipalV1::Broker {
                broker_id: historical_actor.broker_id.clone(),
            },
            &clock,
        )?;
        let snapshot = model::snapshot(&record.intent, record.post_state.clone(), &self.tenant_id)?;
        replay::write_snapshot(&tx, &snapshot, stored.seq, Some(1), &self.tenant_id)?;
        #[cfg(test)]
        pause_before_commit_for_test()?;
        live_deadline(&cap.evidence, cap.monotone_deadline)?;
        let response = KubernetesSessionReservationMutationResponseV2 {
            version: 2,
            receipt: model::receipt(&stored, &record)?,
            snapshot,
            replayed: false,
        };
        self.commit_authorized(tx, &clock)?;
        Ok(response)
    }

    pub(crate) fn lookup_kubernetes_session_control_recovery_receipt(
        &self,
        auth: &AuthenticatedControlRecoveryV2,
        cap: &ControlRecoveryReceiptLookupV2<'_>,
        policy: &KubernetesSessionControlRecoveryPolicyV2,
    ) -> anyhow::Result<KubernetesSessionReservationMutationResponseV2> {
        let _usage = auth.acquire_usage(self)?;
        permission(
            std::ptr::eq(auth, cap.auth)
                && cap.evidence.scope == ControlScope::ControlRecoveryReceiptLookup,
        )?;
        let mut db = self.db.lock().map_err(|_| Failure::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        let id = cap.evidence.creator_reservation_id(&tx, &self.tenant_id)?;
        let (snapshot, _) =
            replay::load_snapshot(&tx, &id, &self.tenant_id)?.ok_or(Failure::NotFound)?;
        current_scope(
            &tx,
            auth,
            &cap.evidence,
            &snapshot.intent,
            policy,
            clock.at(),
            cap.monotone_deadline,
        )?;
        let close = snapshot
            .state
            .last_close
            .as_ref()
            .ok_or(Failure::Conflict)?;
        conflict(close.input_hash()? == cap.evidence.target_hash)?;
        let (stored, record) = exact_close(&tx, &snapshot, close, &self.tenant_id)?;
        conflict(
            Some(stored.seq) == cap.evidence.close_seq
                && Some(&stored.hash) == cap.evidence.close_hash.as_ref(),
        )?;
        let response = KubernetesSessionReservationMutationResponseV2 {
            version: 2,
            receipt: model::receipt(&stored, &record)?,
            snapshot,
            replayed: true,
        };
        live_deadline(&cap.evidence, cap.monotone_deadline)?;
        self.commit_authorized(tx, &clock)?;
        Ok(response)
    }
}
impl ControlEvidence {
    fn creator_reservation_id(&self, db: &Connection, tenant: &str) -> anyhow::Result<String> {
        let stored = db.query_row(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE seq=?1",
            [self.reservation_seq],
            row_event,
        )?;
        permission(
            stored.hash == self.reservation_hash
                && stored.event.kind == "control.kubernetes.session.reserved",
        )?;
        let record = model::record(&stored.event)?;
        replay::validate_historical(db, &stored, &record, tenant)?;
        permission(record.intent.input_hash(tenant)? == self.reservation_input_hash)?;
        Ok(record.intent.reservation_id().into())
    }
}

#[cfg(test)]
thread_local! {
    static BEFORE_COMMIT: std::cell::RefCell<Option<(std::sync::mpsc::Sender<()>,std::sync::mpsc::Receiver<()>)>> = const {std::cell::RefCell::new(None)};
}
#[cfg(test)]
fn pause_before_commit_for_test() -> anyhow::Result<()> {
    BEFORE_COMMIT.with(|hook| {
        if let Some((ready, resume)) = hook.borrow_mut().take() {
            ready.send(()).map_err(|_| Failure::Unavailable)?;
            resume
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| Failure::Unavailable)?;
        }
        Ok(())
    })
}
#[cfg(test)]
#[path = "control_recovery_tests.rs"]
mod tests;
