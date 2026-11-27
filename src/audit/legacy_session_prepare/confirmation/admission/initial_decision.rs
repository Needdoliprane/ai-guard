//! Durable Initial policy evidence constructed from a retained confirmation.
use super::*;
use crate::{
    audit::{
        decision_codec::{self, LEGACY_NAMED_ORIGIN},
        worker_admission,
    },
    control::{AdmissionV1, DecisionReceiptV1, session_control::LegacyNamedInitialDecisionV1},
    model::DecisionRequest,
};

pub(crate) enum LegacyNamedInitialDecision {
    Evaluate(DecisionRequest),
    Recorded(LegacyNamedInitialDecisionV1),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Origin {
    version: u32,
    binding_ref: Uuid,
    confirmation_hash: String,
    launch_spec_hash: String,
    authorization_revision: u64,
    actor: AdmissionActor,
}

fn decision_id(binding_ref: Uuid) -> String {
    key("ai-guard:legacy-named-initial-policy-event:v1", binding_ref)
}

fn read_origin(
    stored: &StoredEvent,
    plan: &LegacyConfirmedPlanV1,
    confirmed: &StoredEvent,
    request: &DecisionRequest,
) -> anyhow::Result<Origin> {
    let origin: Origin = serde_json::from_value(stored.event.data[LEGACY_NAMED_ORIGIN].clone())
        .map_err(|_| ControlFailure::Conflict)?;
    conflict(
        origin.version == 1
            && origin.binding_ref == plan.binding_ref
            && origin.confirmation_hash == confirmed.hash
            && origin.launch_spec_hash == plan.launch_spec_hash
            && stored.seq > confirmed.seq
            && stored.event.kind == decision_codec::KIND
            && stored.event.id == decision_id(plan.binding_ref)
            && stored.event.data["request_id"].as_str() == Some(request.request_id.as_str())
            && stored.event.session_id == request.session_id
            && stored.event.correlation_id == request.correlation_id
            && stored.event.command.as_ref() == Some(&request.command),
    )?;
    Ok(origin)
}

fn require_current_origin(
    stored: &StoredEvent,
    resolved: &ResolvedLegacyConfirmedPlan,
    broker: &BrokerIdentityV1,
    request: &DecisionRequest,
) -> anyhow::Result<()> {
    let origin = read_origin(stored, &resolved.plan, &resolved.confirmed, request)?;
    conflict(
        origin.authorization_revision >= resolved.initiator.authorization_revision
            && origin.authorization_revision <= resolved.authorization_revision
            && origin.actor.matches_grant(broker),
    )
}

fn lookup_named_decision(
    db: &Connection,
    binding_ref: Uuid,
    request: &DecisionRequest,
    hash: &str,
) -> anyhow::Result<Option<StoredEvent>> {
    let Some(stored) = event_by_id(db, &decision_id(binding_ref))? else {
        return Ok(None);
    };
    let decision =
        decision_codec::find_receipt([stored], request, hash)?.ok_or(ControlFailure::Conflict)?;
    Ok(Some(decision))
}

fn receipt_source(db: &Connection, receipt: &DecisionReceiptV1) -> anyhow::Result<StoredEvent> {
    db.query_row(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE seq=?1",
        [receipt.audit_seq],
        row_event,
    )
    .optional()?
    .ok_or(ControlFailure::InvalidRequest.into())
}

/// Older callers can still present their existing retained policy decision.
/// The named policy uses its own deterministic event lookup and scope.
pub(super) fn require_admission_decision(
    tx: &CheckedTransaction<'_>,
    broker: &BrokerIdentityV1,
    resolved: &ResolvedLegacyConfirmedPlan,
    admission: &AdmissionV1,
) -> anyhow::Result<()> {
    let request = &admission.decision_request;
    let hash = &admission.receipt.request_hash;
    let source = receipt_source(tx, &admission.receipt)?;
    let stored = if source.event.data.get(LEGACY_NAMED_ORIGIN).is_some() {
        let expected = resolved.plan.initial_decision_request(&resolved.launch)?;
        conflict(hash == &policy::request_hash(&expected))?;
        let stored = lookup_named_decision(tx, resolved.plan.binding_ref, &expected, hash)?
            .ok_or(ControlFailure::InvalidRequest)?;
        require_current_origin(&stored, resolved, broker, &expected)?;
        stored
    } else {
        Store::lookup_decision(tx, request, hash)?.ok_or(ControlFailure::InvalidRequest)?
    };
    worker_admission::require_receipt(&admission.receipt, &stored)
}

/// Reconstruct and verify scoped policy evidence at the exact durable prefix.
pub(super) fn validate_historical_admission(
    db: &Connection,
    plan: &LegacyConfirmedPlanV1,
    confirmed: &StoredEvent,
    fact: &AdmissionFact,
    admission: &AdmissionV1,
) -> anyhow::Result<()> {
    let source = receipt_source(db, &admission.receipt)?;
    if source.event.data.get(LEGACY_NAMED_ORIGIN).is_none() {
        return Ok(());
    }
    let expected = plan.initial_decision_request(&admission.launch)?;
    let hash = policy::request_hash(&expected);
    conflict(admission.receipt.request_hash == hash)?;
    let stored = lookup_named_decision(db, plan.binding_ref, &expected, &hash)?
        .ok_or(ControlFailure::Conflict)?;
    let origin = read_origin(&stored, plan, confirmed, &expected)?;
    let at = crate::broker_registry::parse_time(&stored.event.timestamp)?;
    conflict(
        stored.seq < fact.admission.audit_seq
            && at < plan.expires_at
            && origin.authorization_revision
                == identity::authorization_revision_before(db, stored.seq)?
            && origin.authorization_revision <= fact.authorization_revision
            && origin
                .actor
                .matches_grant(&fact.actor.historical_identity()?),
    )?;
    workload::validate_broker_identity_at(
        db,
        &origin.actor.historical_identity()?,
        at,
        stored.seq,
    )?;
    worker_admission::require_receipt(&admission.receipt, &stored)
}

pub(in crate::audit) fn require_named_admission_link(
    db: &Connection,
    decision: &StoredEvent,
    admission_seq: i64,
) -> anyhow::Result<()> {
    let origin: Origin = serde_json::from_value(decision.event.data[LEGACY_NAMED_ORIGIN].clone())?;
    let stored = event_by_id(db, &fact_id(origin.binding_ref))?.ok_or(ControlFailure::Conflict)?;
    let fact: AdmissionFact = decoded(&stored, ADMITTED)?;
    conflict(
        fact.binding_ref == origin.binding_ref
            && fact.admission.audit_seq == admission_seq
            && decision.event.id == decision_id(origin.binding_ref),
    )
}

fn recorded(
    stored: StoredEvent,
    request: DecisionRequest,
    request_hash: String,
    binding_ref: Uuid,
    replayed: bool,
) -> anyhow::Result<LegacyNamedInitialDecision> {
    let result = LegacyNamedInitialDecisionV1 {
        version: 1,
        binding_ref,
        receipt: DecisionReceiptV1 {
            version: 1,
            request_id: request.request_id.clone(),
            request_hash,
            phase: DecisionPhaseV1::Initial,
            audit_seq: stored.seq,
            audit_hash: stored.hash,
        },
        request,
        decision: stored.event.decision.ok_or(ControlFailure::Conflict)?,
        replayed,
    };
    result.validate()?;
    Ok(LegacyNamedInitialDecision::Recorded(result))
}

impl Store {
    /// Resolve an exact policy input or replay its existing result. Evaluate
    /// outside this call, then present the hash to the checked write.
    pub(crate) fn prepare_legacy_named_initial_decision<T>(
        &self,
        broker: &BrokerIdentityV1,
        binding_ref: Uuid,
        proposed: Option<(String, Decision, Value)>,
        prepare: impl FnOnce(LegacyNamedInitialDecision) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
        let tx = self.begin_checked(&mut db)?;
        Self::require_legacy_named_admission_format(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        let resolved = self.resolve_legacy_confirmed_plan(&tx, broker, binding_ref, &clock)?;
        let request = resolved.plan.initial_decision_request(&resolved.launch)?;
        let hash = decision_codec::validate_request(&request, &self.redactor)?;
        let value = serde_json::to_value(&request)?;
        crate::control::invalid(
            self.redactor.value_unclipped(&value) == value
                && serde_json::to_vec(&value)?.len() <= LEGACY_NAMED_ADMISSION_REQUEST_BYTES,
        )?;
        conflict(
            proposed
                .as_ref()
                .is_none_or(|(expected, _, _)| expected == &hash),
        )?;
        let result = if let Some(stored) = lookup_named_decision(&tx, binding_ref, &request, &hash)?
        {
            require_current_origin(&stored, &resolved, broker, &request)?;
            let result = recorded(stored, request, hash, binding_ref, true)?;
            self.commit_authorized(tx, &clock)?;
            result
        } else {
            anyhow::ensure!(tx.lease.active, ControlFailure::SupervisionInactive);
            conflict(event_by_id(&tx, &fact_id(binding_ref))?.is_none())?;
            if let Some((_, decision, jev)) = proposed {
                let mut event = decision_codec::build_event(&request, &hash, decision, jev)?;
                event.id = decision_id(binding_ref);
                event.timestamp = crate::broker_registry::format_time(clock.at());
                let origin = serde_json::to_value(Origin {
                    version: 1,
                    binding_ref,
                    confirmation_hash: resolved.confirmed.hash.clone(),
                    launch_spec_hash: resolved.plan.launch_spec_hash.clone(),
                    authorization_revision: resolved.authorization_revision,
                    actor: AdmissionActor::capture(broker),
                })?;
                event.data[LEGACY_NAMED_ORIGIN] = origin.clone();
                let (event, payload) = self.prepare_event(event)?;
                conflict(event.data[LEGACY_NAMED_ORIGIN] == origin)?;
                let stored = self.append_with_authorized_capacity_class_at(
                    &tx,
                    event,
                    payload,
                    capacity::Class::OrdinaryWrite,
                    &clock,
                    Utc::now(),
                )?;
                require_current_origin(&stored, &resolved, broker, &request)?;
                let result = recorded(stored, request, hash, binding_ref, false)?;
                self.commit_legacy_named_control(tx, &clock, |tx, current| {
                    let current =
                        self.resolve_legacy_confirmed_plan(tx, broker, binding_ref, current)?;
                    conflict(
                        current.confirmed.hash == resolved.confirmed.hash
                            && current.plan == resolved.plan,
                    )?;
                    Ok((Some(current.plan.expires_at), current.broker_deadline))
                })?;
                result
            } else {
                self.commit_authorized(tx, &clock)?;
                LegacyNamedInitialDecision::Evaluate(request)
            }
        };
        drop(db);
        let response = prepare(result)?;
        resolved.release_at(Utc::now())?;
        Ok(response)
    }
}
