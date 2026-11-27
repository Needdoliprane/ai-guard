use crate::{
    model::{Decision, DecisionRequest, Event, ExecutionModeV1, Outcome, StoredEvent},
    policy,
    redaction::Redactor,
};
use serde_json::{Value, json};

pub(crate) const KIND: &str = "security.decision";
pub(crate) const LEGACY_NAMED_ORIGIN: &str = "legacy_named_confirmation";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionFailure {
    Conflict,
    AmbiguousHistory,
    ApprovalRequired,
    InvalidRequest,
}
impl std::fmt::Display for DecisionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Conflict => "request identity already belongs to different content",
            Self::AmbiguousHistory => "decision history has missing or ambiguous metadata",
            Self::ApprovalRequired => "approved phase requires a durable review and its exact hash",
            Self::InvalidRequest => "invalid request identity or approval phase",
        })
    }
}
impl std::error::Error for DecisionFailure {}
pub(crate) fn validate_request(
    request: &DecisionRequest,
    redactor: &Redactor,
) -> anyhow::Result<String> {
    if uuid::Uuid::parse_str(&request.request_id).is_err()
        || (!request.human_approved && request.approval_hash.is_some())
        || !policy::valid_provider_context(request)
    {
        return Err(DecisionFailure::InvalidRequest.into());
    }
    let hash = policy::request_hash(request);
    // Check the approval before even considering a cached allow.
    if request.human_approved && request.approval_hash.as_deref() != Some(hash.as_str()) {
        return Err(DecisionFailure::ApprovalRequired.into());
    }
    // Never persist or replay a key that masking would make unfindable.
    // Keep masking enabled, and reject this request instead of weakening
    // secret protection or manufacturing a second durable identity.
    let mut identity = json!({"kind":"security.decision","source":"gateway","provenance":"observed","request_id":request.request_id,"request_hash":hash,"profile":request.profile,"human_approved":request.human_approved});
    if let Some(mode) = request.execution_mode {
        // Absence retains the exact historical batch identity and payload.
        identity["execution_mode"] = serde_json::to_value(mode)?;
    }
    if let Some(resolution) = &request.provider_resolution {
        identity["provider_resolution"] = serde_json::to_value(resolution)?;
    }
    if redactor.value(&identity) != identity {
        return Err(DecisionFailure::InvalidRequest.into());
    }
    Ok(hash)
}

pub(crate) fn build_event(
    request: &DecisionRequest,
    hash: &str,
    decision: Decision,
    jev: Value,
) -> anyhow::Result<Event> {
    let mut event = Event::new(KIND);
    event.project_id = request.project_id.clone();
    event.sandbox_id = request.sandbox_id.clone();
    event.agent_id = request.agent_id.clone();
    event.session_id = request.session_id.clone();
    event.correlation_id = request.correlation_id.clone();
    event.trace_id = request.trace_id.clone();
    event.command = Some(request.command.clone());
    event.decision = Some(decision);
    event.data = json!({"request_id":request.request_id,"request_hash":hash,"profile":request.profile,"human_approved":request.human_approved,"jev":jev});
    if let Some(mode) = request.execution_mode {
        event.data["execution_mode"] = serde_json::to_value(mode)?;
    }
    if let Some(resolution) = &request.provider_resolution {
        event.data["provider_resolution"] = serde_json::to_value(resolution)?;
    }
    Ok(event)
}

/// Missing is the historical batch form. A present unknown or null mode is
/// ambiguous history, never permission to reinterpret it as batch.
pub(super) fn decision_execution_mode(data: &Value) -> anyhow::Result<Option<ExecutionModeV1>> {
    match data.get("execution_mode") {
        None => Ok(None),
        Some(value) => serde_json::from_value::<ExecutionModeV1>(value.clone())
            .map(Some)
            .map_err(|_| DecisionFailure::AmbiguousHistory.into()),
    }
}

pub(super) fn decision_provider_resolution(
    data: &Value,
) -> anyhow::Result<Option<crate::provider::ProviderResolutionV1>> {
    match data.get("provider_resolution") {
        None => Ok(None),
        Some(value) => {
            let resolution: crate::provider::ProviderResolutionV1 =
                serde_json::from_value(value.clone())
                    .map_err(|_| DecisionFailure::AmbiguousHistory)?;
            resolution
                .validate()
                .map_err(|_| DecisionFailure::AmbiguousHistory)?;
            Ok(Some(resolution))
        }
    }
}

pub(crate) fn find_receipt(
    events: impl IntoIterator<Item = StoredEvent>,
    request: &DecisionRequest,
    hash: &str,
) -> anyhow::Result<Option<StoredEvent>> {
    let mut initial: Option<StoredEvent> = None;
    let mut approved: Option<StoredEvent> = None;
    for stored in events {
        let event = &stored.event;
        let data = &event.data;
        let Some(old_hash) = data.get("request_hash").and_then(Value::as_str) else {
            return Err(DecisionFailure::AmbiguousHistory.into());
        };
        if old_hash != hash {
            return Err(DecisionFailure::Conflict.into());
        }
        let Some(phase) = data.get("human_approved").and_then(Value::as_bool) else {
            return Err(DecisionFailure::AmbiguousHistory.into());
        };
        if event.source != "gateway"
            || event.provenance != "observed"
            || event.decision.is_none()
            || data.get("profile").and_then(Value::as_str) != Some(request.profile.as_str())
            || data.get("jev").is_none()
            || decision_execution_mode(data)? != request.execution_mode
            || decision_provider_resolution(data)? != request.provider_resolution
        {
            return Err(DecisionFailure::AmbiguousHistory.into());
        }
        let slot = if phase { &mut approved } else { &mut initial };
        if slot.replace(stored).is_some() {
            return Err(DecisionFailure::AmbiguousHistory.into());
        }
    }
    if approved.is_some()
        && !initial.as_ref().is_some_and(|event| {
            event
                .event
                .decision
                .as_ref()
                .is_some_and(|decision| decision.outcome == Outcome::Review)
        })
    {
        return Err(DecisionFailure::AmbiguousHistory.into());
    }
    if let (Some(initial), Some(approved)) = (&initial, &approved)
        && approved.seq <= initial.seq
    {
        return Err(DecisionFailure::AmbiguousHistory.into());
    }
    if request.human_approved {
        if !initial.as_ref().is_some_and(|event| {
            event
                .event
                .decision
                .as_ref()
                .is_some_and(|decision| decision.outcome == Outcome::Review)
        }) {
            return Err(DecisionFailure::ApprovalRequired.into());
        }
        Ok(approved)
    } else {
        Ok(initial)
    }
}
