//! Durable Initial policy evidence constructed from a retained confirmation.
use super::*;
use crate::{
    audit::{decision_codec, worker_admission},
    control::{AdmissionV1, DecisionReceiptV1, session_control::LegacyNamedInitialDecisionV1},
    model::DecisionRequest,
};

const ORIGIN: &str = "legacy_named_confirmation";

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

fn request_id(binding_ref: Uuid) -> String {
    key(
        "ai-guard:legacy-named-initial-policy-request:v1",
        binding_ref,
    )
}

fn decision_id(binding_ref: Uuid) -> String {
    key("ai-guard:legacy-named-initial-policy-event:v1", binding_ref)
}

fn request_for(resolved: &ResolvedLegacyConfirmedPlan) -> anyhow::Result<DecisionRequest> {
    resolved.launch.validate()?;
    resolved.launch.validate_supervised()?;
    Ok(DecisionRequest {
        request_id: request_id(resolved.plan.binding_ref),
        profile: resolved.launch.profile.clone(),
        command: resolved.launch.command.clone(),
        project_id: None,
        sandbox_id: None,
        agent_id: None,
        session_id: Some(resolved.plan.session_id.to_string()),
        correlation_id: Some(resolved.plan.binding_ref.to_string()),
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: resolved.launch.provider_resolution.clone(),
    })
}

fn require_origin(
    stored: &StoredEvent,
    resolved: &ResolvedLegacyConfirmedPlan,
    broker: &BrokerIdentityV1,
    request: &DecisionRequest,
) -> anyhow::Result<()> {
    let origin: Origin = serde_json::from_value(stored.event.data[ORIGIN].clone())
        .map_err(|_| ControlFailure::Conflict)?;
    conflict(
        origin.version == 1
            && origin.binding_ref == resolved.plan.binding_ref
            && origin.confirmation_hash == resolved.confirmed.hash
            && origin.launch_spec_hash == resolved.plan.launch_spec_hash
            && origin.authorization_revision >= resolved.initiator.authorization_revision
            && origin.authorization_revision <= resolved.authorization_revision
            && origin.actor.matches_grant(broker)
            && stored.seq > resolved.confirmed.seq
            && stored.event.id == decision_id(resolved.plan.binding_ref)
            && stored.event.session_id == request.session_id
            && stored.event.correlation_id == request.correlation_id
            && stored.event.command.as_ref() == Some(&request.command),
    )
}

/// Earlier private callers can still present their own retained policy
/// decision. The server-derived identity is reserved to the named issuer.
pub(super) fn require_named_origin_when_used(
    tx: &CheckedTransaction<'_>,
    broker: &BrokerIdentityV1,
    resolved: &ResolvedLegacyConfirmedPlan,
    admission: &AdmissionV1,
) -> anyhow::Result<()> {
    if admission.decision_request.request_id != request_id(resolved.plan.binding_ref) {
        return Ok(());
    }
    let expected = request_for(resolved)?;
    let hash = policy::request_hash(&expected);
    conflict(admission.receipt.request_hash == hash)?;
    let stored =
        Store::lookup_decision(tx, &expected, &hash)?.ok_or(ControlFailure::InvalidRequest)?;
    require_origin(&stored, resolved, broker, &expected)?;
    worker_admission::require_receipt(&admission.receipt, &stored)
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
        let request = request_for(&resolved)?;
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
        let result = if let Some(stored) = Self::lookup_decision(&tx, &request, &hash)? {
            require_origin(&stored, &resolved, broker, &request)?;
            let result = recorded(stored, request, hash, binding_ref, true)?;
            self.commit_authorized(tx, &clock)?;
            result
        } else {
            anyhow::ensure!(tx.lease.active, ControlFailure::SupervisionInactive);
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
                event.data[ORIGIN] = origin.clone();
                let (event, payload) = self.prepare_event(event)?;
                conflict(event.data[ORIGIN] == origin)?;
                let stored = self.append_with_authorized_capacity_class_at(
                    &tx,
                    event,
                    payload,
                    capacity::Class::OrdinaryWrite,
                    &clock,
                    Utc::now(),
                )?;
                require_origin(&stored, &resolved, broker, &request)?;
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
