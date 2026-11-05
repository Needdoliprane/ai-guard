//! Pure consistency checks on retained facts and descriptive control data.
//! A successful check is not authentication, a start permission or a journal ACK.
use super::{
    checkpoint::SupervisionOperationV1,
    intent::ExecutionIntentV1,
    journal::{JournalError, JournalResult, JournalScopeV1},
    outbox::BrokerCheckpointV4,
    terminal_facts::{BatchResultFactsV1, CoverageFactsV1},
};
use crate::{
    control::{
        ArmSubjectV1, NetworkV1, RuntimeKindV1, WorkerMutationV1, WorkerStateV1,
        WorkerTransitionV1, WorkerV1,
    },
    model::{DecisionRequest, Event},
    runner::broker_observation_data,
    runtime::{NetworkMode, RuntimeKind},
};
use anyhow::{Context, ensure};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use uuid::Uuid;

/// Borrowed data only. The eventual owner must authenticate the same control
/// client independently, then match this scope before it can send any event.
#[derive(Clone, Copy)]
pub(in crate::broker) struct RetainedTerminalFacts<'a> {
    pub(in crate::broker) scope: &'a JournalScopeV1,
    pub(in crate::broker) intent: &'a ExecutionIntentV1,
    pub(in crate::broker) checkpoint: &'a BrokerCheckpointV4,
}

pub(in crate::broker) fn check_retained_terminal(
    facts: RetainedTerminalFacts<'_>,
    worker: &WorkerV1,
) -> JournalResult<()> {
    // Diagnostics never return data from either the retained records or the
    // descriptive Worker. This does not authenticate the source of either.
    check(facts, worker).map_err(|_| JournalError::Conflict)
}

fn check(facts: RetainedTerminalFacts<'_>, worker: &WorkerV1) -> anyhow::Result<()> {
    let RetainedTerminalFacts {
        scope,
        intent,
        checkpoint,
    } = facts;
    intent.validate()?;
    checkpoint.validate_context(intent)?;
    let intent_hash = intent.intent_hash()?;
    checkpoint
        .lifecycle()
        .validate_context(intent.operation_id(), &intent_hash)?;
    let broker = checkpoint.lifecycle().broker();
    let supervision = broker.supervision();
    let context = broker.runner().broker_context();
    ensure!(
        context.managed
            && !context.legacy_pending
            && !context.legacy_import
            && context.tenant_id == Some(scope.tenant_id())
            && supervision.control_namespace_uuid() == scope.control_namespace_uuid()
    );
    let retained_origin =
        reqwest::Url::parse(context.gateway_origin.context("missing retained origin")?)?;
    let scope_origin = reqwest::Url::parse(scope.gateway_origin())?;
    let options = intent.initial_options().run_options();
    ensure!(
        retained_origin == scope_origin
            && reqwest::Url::parse(&options.gateway)?.origin() == scope_origin.origin()
    );
    let mut initial_request = context.request.clone();
    let expected_request_hash = crate::policy::request_hash(intent.request_data());
    ensure!(if initial_request.human_approved {
        initial_request.approval_hash.as_deref() == Some(expected_request_hash.as_str())
    } else {
        initial_request.approval_hash.is_none()
    });
    initial_request.human_approved = false;
    initial_request.approval_hash = None;
    same(&initial_request, intent.request_data())?;

    let admission = context.admission.context("missing retained admission")?;
    let retained = context.worker.context("missing retained worker")?;
    let binding = context.binding.context("missing retained authority")?;
    let owned = context.owned.context("missing retained ownership")?;
    admission.validate()?;
    same(&admission.decision_request, context.request)?;
    same(&admission.launch, &retained.launch)?;
    same(&admission.receipt, &retained.receipt)?;
    let kind = match options.runtime {
        RuntimeKind::Docker => RuntimeKindV1::Docker,
        RuntimeKind::Podman => RuntimeKindV1::Podman,
    };
    let network = match options.network {
        NetworkMode::None => NetworkV1::None,
        NetworkMode::Bridge => NetworkV1::Bridge,
    };
    ensure!(
        admission.operation_id == intent.operation_id().to_string()
            && retained.admission_operation_id == admission.operation_id
            && retained.request_id == context.request.request_id
            && admission.launch.session_id == intent.session().to_string()
            && admission.launch.command.argv == options.argv
            && admission.launch.command.cwd == "/workspace"
            && admission.launch.profile == options.profile
            && admission.launch.runtime_kind == kind
            && admission.launch.network == network
            && u64::from(admission.launch.timeout_seconds) == options.timeout
            && admission.launch.image_ref == options.image
            && admission.launch.provider == options.provider
            && admission.launch.capture_output == options.capture_output
            && admission.launch.security_preset.is_interactive() == options.interactive
            && retained.spec_hash == admission.launch.spec_hash()?
            && retained.spec_hash == retained.launch.spec_hash()?
    );
    // Bind all immutable fields before using a projection that copies spec,
    // authority, volume and network from a Worker supplied by the caller.
    immutable_worker(retained, worker)?;
    ensure!(
        retained.attempt == worker.attempt
            && worker.revision >= retained.revision
            && retained.revision > 0
    );
    ensure!(
        worker.state == WorkerStateV1::Terminal
            && !worker.quota_held
            && !worker.allocation_inconsistent
    );
    allocation_state(retained)?;
    canonical_uuid(&worker.worker_id)?;
    canonical_uuid(&worker.attempt.attempt_id)?;
    super::journal::valid_hash(&worker.spec_hash)?;
    super::journal::valid_hash(&worker.policy_hash)?;
    let expected_name = format!(
        "aig-{}-{}",
        scope.control_namespace_uuid().simple(),
        Uuid::parse_str(&worker.worker_id)?.simple()
    );
    ensure!(
        worker.attempt.stable_name == expected_name
            && worker.attempt.volume_name == format!("{}-work", worker.attempt.stable_name)
    );
    binding.validate_docker_launch_profile()?;
    ensure!(
        !binding.authority_id.is_nil()
            && binding.kind == options.runtime
            && binding.authority_id.to_string() == retained.launch.authority_id
            && binding.invocation.is_some()
            && !binding.endpoint.is_empty()
            && match binding.kind {
                RuntimeKind::Docker => binding.engine_id.as_ref().is_some_and(|id| !id.is_empty()),
                // Podman retains its private storage/marker binding in the
                // authority fingerprint, and has no Docker engine ID.
                RuntimeKind::Podman => binding.engine_id.is_none(),
            }
            && options
                .runtime_endpoint
                .as_ref()
                .is_none_or(|endpoint| endpoint == &binding.endpoint)
            && owned.handle.operation_id() == intent.operation_id()
            && owned.handle.session_id() == intent.session()
            && owned.handle.authority_fingerprint() == binding.fingerprint()
            && owned.handle.id()
                == worker
                    .attempt
                    .container_id
                    .as_deref()
                    .context("missing retained CID")?
            && owned.handle.name() == worker.attempt.stable_name
            && owned.observation.id.as_deref() == Some(owned.handle.id())
            && owned.observation.name == owned.handle.name()
            && owned.observation.image_id.as_deref() == Some(retained.launch.image_id.as_str())
    );
    owned
        .observation
        .validate_identity_allocation(&owned.observation)?;
    let created = worker
        .attempt
        .created_inspection
        .as_ref()
        .context("missing created inspection")?;
    let retained_created = broker_observation_data(retained, &owned.observation)?;
    ensure!(created == &retained_created.inspection);

    let mut arms =
        supervision.operations().iter().enumerate().filter_map(
            |(index, operation)| match operation {
                SupervisionOperationV1::Arm {
                    parent,
                    owned,
                    request,
                    response,
                } if matches!(request.subject, ArmSubjectV1::Worker) => {
                    Some((index, parent, owned, request, response))
                }
                _ => None,
            },
        );
    let (arm_index, parent, armed_owned, arm, response) =
        arms.next().context("missing worker Arm")?;
    ensure!(arms.next().is_none());
    let armed = response.as_ref().context("missing Arm ACK")?;
    arm.validate_for_parent(parent)?;
    armed.receipt.validate_for_arm(arm)?;
    same(&armed.arm, arm)?;
    same(armed_owned, owned)?;
    immutable_worker(parent, worker)?;
    ensure!(
        arm.intent_hash == intent_hash
            && arm.inspection == *created
            && arm.authority_binding_hash == binding.fingerprint()
            && armed.receipt.control_namespace_uuid == scope.control_namespace_uuid().to_string()
            && owned.handle.fingerprint_version() == arm.configuration_fingerprint_version
            && owned
                .observation
                .configuration_fingerprint_version
                .unwrap_or(1)
                == arm.configuration_fingerprint_version
            && owned.observation.configuration_fingerprint.as_deref()
                == Some(arm.configuration_fingerprint.as_str())
    );

    let mut claims = supervision.operations().iter().enumerate().filter_map(
        |(index, operation)| match operation {
            SupervisionOperationV1::WorkerClaim { request, response } => {
                Some((index, request, response))
            }
            _ => None,
        },
    );
    let (claim_index, claim, response) = claims.next().context("missing worker claim")?;
    ensure!(claims.next().is_none() && claim_index > arm_index);
    let claimed = response.as_ref().context("missing worker claim ACK")?;
    claim.validate_for_arm(arm)?;
    // A historical Claim is the exact deterministic Store transition from
    // its retained Created parent, not any later allocation-holding state or
    // a revision merely inside the central Worker's interval.
    let expected_claim = crate::audit::expected_worker_transition(
        parent,
        &WorkerMutationV1 {
            version: 1,
            operation_id: claim.operation_id.clone(),
            worker_id: claim.worker_id.clone(),
            attempt_id: claim.attempt_id.clone(),
            expected_revision: claim.expected_revision,
            transition: WorkerTransitionV1::StartClaim {
                emitter_id: claim.emitter_id.clone(),
            },
        },
    )?;
    same(&expected_claim, &claimed.worker)?;
    same(&claim.arm_receipt, &armed.receipt)?;
    immutable_worker(&claimed.worker, worker)?;
    allocation_state(&claimed.worker)?;
    super::journal::valid_hash(&claimed.receipt.audit_hash)?;
    canonical_uuid(&claim.operation_id)?;
    canonical_uuid(&claim.emitter_id)?;
    ensure!(
        claim.worker_id == worker.worker_id
            && claim.attempt_id == worker.attempt.attempt_id
            && worker.attempt.start_operation_id.as_deref() == Some(claim.operation_id.as_str())
            && worker.attempt.start_emitter_id.as_deref() == Some(claim.emitter_id.as_str())
            && claimed.disposition.is_some()
            && claimed.worker.attempt == worker.attempt
            && claimed.worker.revision > parent.revision
            && claimed.worker.revision <= worker.revision
            && claimed.receipt.operation_id == claim.operation_id
            && claimed.receipt.worker_id.as_deref() == Some(worker.worker_id.as_str())
            && claimed.receipt.revision == Some(claimed.worker.revision)
            && claimed.receipt.audit_seq > 0
            && claimed.receipt.input_hash == claim.input_hash()?
    );

    let (coverage_event, result_event) = result_pair(checkpoint, context.request)?;
    let result: BatchResultFactsV1 = closed_data(&result_event.data)?;
    let coverage: CoverageFactsV1 = closed_data(&coverage_event.data)?;
    ensure!(
        result.version == 1
            && result.worker_id == worker.worker_id
            && result.attempt_id == worker.attempt.attempt_id
    );
    result
        .observation
        .validate_terminal(binding.kind, &owned.observation)?;
    let projection = broker_observation_data(retained, &result.observation)?;
    ensure!(worker.last_observation.as_ref() == Some(&projection));
    let started = result
        .observation
        .process
        .started_at
        .context("missing terminal start")?;
    let central_started = chrono::DateTime::parse_from_rfc3339(
        worker
            .observed_started_at
            .as_deref()
            .context("missing central start")?,
    )?;
    ensure!(started.timestamp() > 0 && central_started == started);
    if let Some(value) = &retained.observed_started_at {
        ensure!(chrono::DateTime::parse_from_rfc3339(value)? == started);
    }
    if retained.state == WorkerStateV1::Terminal {
        ensure!(retained.last_observation.as_ref() == Some(&projection));
    }
    ensure!(
        coverage.version == 1
            && coverage.maximum_events == crate::adapters::MAX_EVENTS_PER_CAPTURE
            && !coverage.nested_authorization
            && coverage.raw_output_capture == admission.launch.capture_output
    );
    if let Some(transport) = &result.transport {
        let duration = transport.duration_ms_decimal.parse::<u128>()?;
        ensure!(transport.duration_ms_decimal == duration.to_string());
        let count = coverage
            .normalized_events
            .context("missing capture coverage count")?;
        ensure!(
            count <= coverage.maximum_events
                && coverage.event_limit_reached == Some(count == coverage.maximum_events)
                && coverage.stdout_truncated == Some(transport.stdout.truncated)
                && coverage.unterminated_final_line_omitted.is_some()
                && transport.stdout.text.is_some() == coverage.raw_output_capture
                && transport.stdout.text_clipped.is_some() == coverage.raw_output_capture
                && transport.stderr.text.is_some() == coverage.raw_output_capture
                && transport.stderr.text_clipped.is_some() == coverage.raw_output_capture
                && result_event.command.as_ref() == Some(&context.request.command)
        );
    } else {
        ensure!(
            coverage.normalized_events.is_none()
                && coverage.event_limit_reached.is_none()
                && coverage.stdout_truncated.is_none()
                && coverage.unterminated_final_line_omitted.is_none()
                && result_event.command.is_none()
        );
    }
    Ok(())
}

fn immutable_worker(left: &WorkerV1, right: &WorkerV1) -> anyhow::Result<()> {
    ensure!(
        left.worker_id == right.worker_id
            && left.admission_operation_id == right.admission_operation_id
            && left.request_id == right.request_id
            && left.spec_hash == right.spec_hash
            && left.policy_hash == right.policy_hash
            && left.attempt.attempt_id == right.attempt.attempt_id
            && left.attempt.stable_name == right.attempt.stable_name
            && left.attempt.volume_name == right.attempt.volume_name
            && left.attempt.container_id == right.attempt.container_id
            && left.attempt.created_inspection == right.attempt.created_inspection
            && left.attempt.create_operation_id == right.attempt.create_operation_id
    );
    same(&left.launch, &right.launch)?;
    same(&left.receipt, &right.receipt)
}

fn allocation_state(worker: &WorkerV1) -> anyhow::Result<()> {
    ensure!(
        !worker.allocation_inconsistent
            && match worker.state {
                WorkerStateV1::StartIssued | WorkerStateV1::Running | WorkerStateV1::Stopping =>
                    worker.quota_held,
                WorkerStateV1::Terminal => !worker.quota_held,
                _ => false,
            }
    );
    Ok(())
}

fn same<T: Serialize>(left: &T, right: &T) -> anyhow::Result<()> {
    ensure!(serde_json::to_value(left)? == serde_json::to_value(right)?);
    Ok(())
}

fn canonical_uuid(value: &str) -> anyhow::Result<()> {
    let id = Uuid::parse_str(value)?;
    ensure!(!id.is_nil() && id.to_string() == value);
    Ok(())
}

fn closed_data<T: DeserializeOwned + Serialize>(data: &Value) -> anyhow::Result<T> {
    let mut ignored = false;
    let value: T = serde_ignored::deserialize(data, |_| ignored = true)?;
    ensure!(!ignored && serde_json::to_value(&value)? == *data);
    Ok(value)
}

fn result_pair<'a>(
    checkpoint: &'a BrokerCheckpointV4,
    request: &DecisionRequest,
) -> anyhow::Result<(&'a Event, &'a Event)> {
    let mut coverage = None;
    let mut result = None;
    let mut pending = 0;
    for entry in checkpoint.events() {
        let event = entry.event();
        let target = match event.kind.as_str() {
            "observer.coverage" => &mut coverage,
            "broker.batch.result" => &mut result,
            _ => {
                ensure!(entry.audit_seq().is_some());
                continue;
            }
        };
        ensure!(target.replace(event).is_none());
        canonical_uuid(&event.id)?;
        ensure!(
            event.source == "runner"
                && event.provenance == "observed"
                && event.session_id == request.session_id
                && event.project_id == request.project_id
                && event.sandbox_id == request.sandbox_id
                && event.agent_id == request.agent_id
                && event.correlation_id == request.correlation_id
                && event.trace_id == request.trace_id
                && event.parent_agent_id.is_none()
                && event.decision.is_none()
                && event.result.is_none()
        );
        pending += usize::from(entry.audit_seq().is_none());
    }
    let coverage = coverage.context("missing retained coverage")?;
    let result = result.context("missing retained result")?;
    ensure!(
        (1..=2).contains(&pending)
            && coverage.coverage == "partial"
            && coverage.command.is_none()
            && result.coverage == "observed"
    );
    Ok((coverage, result))
}

#[cfg(test)]
#[path = "cold_terminal_tests.rs"]
mod tests;
