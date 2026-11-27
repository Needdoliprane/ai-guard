use super::worker_admission::{ControlRecord, receipt};
use crate::{
    control::{
        AllocationProvenanceV1, AllocationSourceV1, AllocationV1, ClaimDispositionV1,
        ControlFailure, MutationResultV1, RuntimeInspectionV1, RuntimeObservationV1,
        RuntimeStatusV1, WorkerMutationV1, WorkerStateV1, WorkerTransitionV1, WorkerV1, bounded,
        conflict, content_hash, hash, invalid, uuid,
    },
    model::StoredEvent,
    redaction::Redactor,
};
use anyhow::{Context, Result};

pub(super) fn validate_identity(mutation: &WorkerMutationV1) -> Result<()> {
    invalid(mutation.version == 1 && mutation.expected_revision > 0)?;
    uuid(&mutation.operation_id)?;
    uuid(&mutation.worker_id)?;
    uuid(&mutation.attempt_id)?;
    Ok(())
}

fn mutation_hash(mutation: &WorkerMutationV1) -> Result<String> {
    content_hash("ai-guard:worker-mutation:v1", mutation)
}

pub(crate) fn validate_input(mutation: &WorkerMutationV1, redactor: &Redactor) -> Result<String> {
    validate_identity(mutation)?;
    let value = serde_json::to_value(mutation)?;
    invalid(redactor.value(&value) == value && serde_json::to_vec(&value)?.len() <= 65536)?;
    mutation_hash(mutation)
}

pub(crate) fn action(mutation: &WorkerMutationV1) -> &'static str {
    match &mutation.transition {
        WorkerTransitionV1::Prepare => "prepare",
        WorkerTransitionV1::CreateIssued => "create_issued",
        WorkerTransitionV1::Created { .. } => "created",
        WorkerTransitionV1::StartClaim { .. } => "start_claim",
        WorkerTransitionV1::Observe { .. } => "observe",
        WorkerTransitionV1::Unknown { .. } => "unknown",
        WorkerTransitionV1::Stop => "stop",
        WorkerTransitionV1::CancelReserved => "cancel_reserved",
        WorkerTransitionV1::CleanupResult { .. } => "cleanup_result",
    }
}

pub(crate) fn apply(
    mut worker: WorkerV1,
    mutation: &WorkerMutationV1,
) -> Result<(WorkerV1, Option<RuntimeObservationV1>, Option<AllocationV1>)> {
    conflict(
        worker.worker_id == mutation.worker_id
            && worker.attempt.attempt_id == mutation.attempt_id
            && worker.revision == mutation.expected_revision,
    )?;
    let (observation, allocation) = apply_worker_transition(&mut worker, mutation)?;
    Ok((worker, observation, allocation))
}

pub(crate) fn build_record(
    worker: WorkerV1,
    mutation: &WorkerMutationV1,
    input_hash: String,
) -> Result<ControlRecord> {
    let (worker, observation, allocation) = apply(worker, mutation)?;
    Ok(ControlRecord {
        version: 1,
        operation_id: mutation.operation_id.clone(),
        action: action(mutation).into(),
        input_hash,
        previous_revision: Some(mutation.expected_revision),
        snapshot_hash: Some(content_hash("ai-guard:worker-snapshot:v1", &worker)?),
        worker: Some(worker),
        observation,
        allocation,
        refusal: None,
        admission_policy: None,
    })
}

pub(crate) fn replay(
    mutation: &WorkerMutationV1,
    digest: &str,
    record: &ControlRecord,
    stored: &StoredEvent,
) -> Result<MutationResultV1> {
    conflict(record.input_hash == digest && record.action == action(mutation))?;
    result(stored, record, ClaimDispositionV1::AlreadyConsumed)
}

/// The storage backend may call this only after confirming a fresh commit.
pub(crate) fn committed_result(
    stored: &StoredEvent,
    record: &ControlRecord,
) -> Result<MutationResultV1> {
    result(stored, record, ClaimDispositionV1::EmitOnce)
}

fn result(
    stored: &StoredEvent,
    record: &ControlRecord,
    claim_disposition: ClaimDispositionV1,
) -> Result<MutationResultV1> {
    Ok(MutationResultV1 {
        worker: record.worker.clone().ok_or(ControlFailure::Conflict)?,
        receipt: receipt(stored, record),
        disposition: (record.action == "start_claim").then_some(claim_disposition),
    })
}

/// Derive checkpoint facts from an acknowledged parent without storage or IO.
/// The returned clone is data, never a start or mutation permission.
pub(crate) fn expected_worker_transition(
    parent: &WorkerV1,
    mutation: &WorkerMutationV1,
) -> anyhow::Result<WorkerV1> {
    validate_identity(mutation)?;
    invalid(serde_json::to_vec(mutation)?.len() <= 65536)?;
    let (expected, _, _) = apply(parent.clone(), mutation)?;
    Ok(expected)
}

/// Deterministic state change shared by live mutations and journal replay.
/// This function performs no database or runtime operation.
pub(super) fn apply_worker_transition(
    worker: &mut WorkerV1,
    mutation: &WorkerMutationV1,
) -> anyhow::Result<(Option<RuntimeObservationV1>, Option<AllocationV1>)> {
    let mut observation = None;
    let mut allocation = None;
    match &mutation.transition {
        WorkerTransitionV1::Prepare => {
            prepare_worker(worker)?;
        }
        WorkerTransitionV1::CreateIssued => {
            conflict(
                worker.state == WorkerStateV1::Preparing
                    && worker.attempt.create_operation_id.is_none(),
            )?;
            worker.attempt.create_operation_id = Some(mutation.operation_id.clone());
        }
        WorkerTransitionV1::Created { inspection } => {
            record_created_worker(worker, inspection)?;
        }
        WorkerTransitionV1::StartClaim { emitter_id } => {
            uuid(emitter_id)?;
            conflict(
                worker.state == WorkerStateV1::Created
                    && worker.quota_held
                    && worker.attempt.container_id.is_some()
                    && worker.attempt.start_operation_id.is_none(),
            )?;
            worker.attempt.start_operation_id = Some(mutation.operation_id.clone());
            worker.attempt.start_emitter_id = Some(emitter_id.clone());
            worker.state = WorkerStateV1::StartIssued;
        }
        WorkerTransitionV1::Observe {
            observation: incoming,
        } => {
            conflict(
                worker.quota_held
                    && worker.attempt.container_id.is_some()
                    && ![
                        WorkerStateV1::Reserved,
                        WorkerStateV1::Preparing,
                        WorkerStateV1::Terminal,
                        WorkerStateV1::Cancelled,
                    ]
                    .contains(&worker.state),
            )?;
            validate_inspection(&incoming.inspection)?;
            chrono::DateTime::parse_from_rfc3339(&incoming.observed_at)
                .map_err(|_| ControlFailure::InvalidRequest)?;
            for date in [&incoming.started_at, &incoming.finished_at]
                .into_iter()
                .flatten()
            {
                bounded(date, 64)?;
            }
            // Later inspection cannot erase an observed restart or an
            // identity/configuration divergence during this allocation.
            if !inspection_matches(worker, &incoming.inspection, true)
                || incoming.restart_count.is_some_and(|count| count > 0)
                || worker
                    .observed_started_at
                    .as_ref()
                    .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
                    .zip(
                        incoming
                            .started_at
                            .as_ref()
                            .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok()),
                    )
                    .is_some_and(|(first, current)| first != current)
            {
                worker.allocation_inconsistent = true;
            }
            if worker.observed_started_at.is_none()
                && !worker.allocation_inconsistent
                && inspection_matches(worker, &incoming.inspection, true)
                && incoming.restart_count == Some(0)
                && worker.attempt.start_operation_id.is_some()
                && ((incoming.status == RuntimeStatusV1::Running && incoming.running == Some(true))
                    || ([RuntimeStatusV1::Exited, RuntimeStatusV1::Dead]
                        .contains(&incoming.status)
                        && incoming.running == Some(false)))
                && let Some(started) = incoming
                    .started_at
                    .as_ref()
                    .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
                    .filter(|date| date.timestamp() > 0)
            {
                // A credible historical start remains evidence even when
                // finish time or exit code is not yet observable.
                worker.observed_started_at = Some(
                    started
                        .with_timezone(&chrono::Utc)
                        .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
                );
            }
            let diagnosis = classify_observation(worker, incoming);
            match diagnosis {
                ObservationClass::Running => {
                    worker.state = if worker.state == WorkerStateV1::Stopping {
                        WorkerStateV1::Stopping
                    } else {
                        WorkerStateV1::Running
                    };
                    worker.unknown_reason = None;
                    if worker.observed_started_at.is_none() {
                        worker.observed_started_at = incoming.started_at.clone();
                    }
                }
                ObservationClass::Terminal(ref started, ref finished, ns) => {
                    let cpu = ns
                        .checked_mul(u128::from(worker.launch.cpu_millis))
                        .context("allocation CPU overflow")?;
                    let ram = ns
                        .checked_mul(u128::from(worker.launch.memory_bytes))
                        .context("allocation memory overflow")?;
                    allocation = Some(AllocationV1 {
                        attempt_id: worker.attempt.attempt_id.clone(),
                        worker_id: worker.worker_id.clone(),
                        terminal_observation_id: mutation.operation_id.clone(),
                        started_at: started.clone(),
                        finished_at: finished.clone(),
                        duration_ns: ns.to_string(),
                        cpu_millicore_nanoseconds: cpu.to_string(),
                        memory_byte_nanoseconds: ram.to_string(),
                        method_version: 1,
                        billable: false,
                        source: allocation_source(incoming),
                    });
                    worker.state = WorkerStateV1::Terminal;
                    worker.quota_held = false;
                    worker.unknown_reason = None;
                    if worker.observed_started_at.is_none() {
                        worker.observed_started_at = Some(started.clone());
                    }
                }
                ObservationClass::Unknown(reason) => {
                    worker.state = WorkerStateV1::InDoubt;
                    worker.unknown_reason = Some(reason.into());
                }
            }
            worker.last_observation = Some(incoming.clone());
            observation = Some(incoming.clone());
        }
        WorkerTransitionV1::Unknown { reason } => {
            bounded(reason, 1024)?;
            conflict(worker.quota_held)?;
            worker.state = WorkerStateV1::InDoubt;
            worker.unknown_reason = Some(reason.clone());
        }
        WorkerTransitionV1::Stop => {
            conflict(worker.quota_held && worker.attempt.container_id.is_some())?;
            worker.state = WorkerStateV1::Stopping;
        }
        WorkerTransitionV1::CancelReserved => {
            conflict(
                worker.state == WorkerStateV1::Reserved
                    && worker.attempt.create_operation_id.is_none()
                    && worker.attempt.start_operation_id.is_none()
                    && worker.quota_held,
            )?;
            worker.state = WorkerStateV1::Cancelled;
            worker.quota_held = false;
        }
        WorkerTransitionV1::CleanupResult { removed, error } => {
            conflict(worker.state == WorkerStateV1::Terminal && !worker.quota_held)?;
            invalid((*removed && error.is_none()) || (!*removed && error.is_some()))?;
            if let Some(error) = error {
                bounded(error, 1024)?;
            }
            worker.cleanup_removed = Some(*removed);
            worker.cleanup_error = error.clone();
        }
    }
    worker.revision = worker
        .revision
        .checked_add(1)
        .context("worker revision exhausted")?;
    Ok((observation, allocation))
}

/// Verify the mutation input and the resulting snapshot, observation and allocation.
pub(crate) fn replay_record(previous: &WorkerV1, record: &ControlRecord) -> Result<()> {
    let worker = record
        .worker
        .as_ref()
        .context("worker mutation snapshot missing")?;
    // Version 1 stores each mutation's full input across its identity, action,
    // snapshot and observation. Rebuild that input without trusting its result.
    // Admission inputs have a different contract and are validated separately.
    let transition = match record.action.as_str() {
        "prepare" => WorkerTransitionV1::Prepare,
        "create_issued" => WorkerTransitionV1::CreateIssued,
        "created" => WorkerTransitionV1::Created {
            inspection: worker
                .attempt
                .created_inspection
                .clone()
                .context("created transition has no inspection")?,
        },
        "start_claim" => WorkerTransitionV1::StartClaim {
            emitter_id: worker
                .attempt
                .start_emitter_id
                .clone()
                .context("start claim has no emitter")?,
        },
        "observe" => WorkerTransitionV1::Observe {
            observation: record
                .observation
                .clone()
                .context("observe transition has no observation")?,
        },
        "unknown" => WorkerTransitionV1::Unknown {
            reason: worker
                .unknown_reason
                .clone()
                .context("unknown transition has no reason")?,
        },
        "stop" => WorkerTransitionV1::Stop,
        "cancel_reserved" => WorkerTransitionV1::CancelReserved,
        "cleanup_result" => WorkerTransitionV1::CleanupResult {
            removed: worker
                .cleanup_removed
                .context("cleanup result is missing")?,
            error: worker.cleanup_error.clone(),
        },
        _ => anyhow::bail!("unsupported worker mutation action"),
    };
    let mutation = WorkerMutationV1 {
        version: 1,
        operation_id: record.operation_id.clone(),
        worker_id: previous.worker_id.clone(),
        attempt_id: previous.attempt.attempt_id.clone(),
        expected_revision: previous.revision,
        transition,
    };
    anyhow::ensure!(
        record.previous_revision == Some(previous.revision)
            && mutation_hash(&mutation)? == record.input_hash
            && record.refusal.is_none()
            && record.admission_policy.is_none(),
        "worker mutation input or metadata differs from journal"
    );
    let mut expected = previous.clone();
    let (observation, allocation) = apply_worker_transition(&mut expected, &mutation)?;
    anyhow::ensure!(
        serde_json::to_value(expected)? == serde_json::to_value(worker)?
            && observation == record.observation
            && allocation == record.allocation,
        "worker mutation result differs from journal"
    );
    Ok(())
}

fn prepare_worker(worker: &mut WorkerV1) -> anyhow::Result<()> {
    conflict(
        [WorkerStateV1::Reserved, WorkerStateV1::InDoubt].contains(&worker.state)
            && worker.quota_held
            && worker.attempt.create_operation_id.is_none()
            && worker.attempt.container_id.is_none()
            && worker.attempt.created_inspection.is_none()
            && worker.attempt.start_operation_id.is_none()
            && !worker.allocation_inconsistent,
    )?;
    worker.state = WorkerStateV1::Preparing;
    worker.unknown_reason = None;
    Ok(())
}

fn record_created_worker(
    worker: &mut WorkerV1,
    inspection: &RuntimeInspectionV1,
) -> anyhow::Result<()> {
    conflict(
        [WorkerStateV1::Preparing, WorkerStateV1::InDoubt].contains(&worker.state)
            && worker.quota_held
            && worker.attempt.create_operation_id.is_some()
            && worker.attempt.container_id.is_none()
            && worker.attempt.created_inspection.is_none()
            && worker.attempt.start_operation_id.is_none()
            && !worker.allocation_inconsistent,
    )?;
    validate_inspection(inspection)?;
    invalid(inspection_matches(worker, inspection, false))?;
    worker.attempt.container_id = Some(inspection.container_id.clone());
    worker.attempt.created_inspection = Some(inspection.clone());
    worker.state = WorkerStateV1::Created;
    worker.unknown_reason = None;
    Ok(())
}

fn validate_inspection(inspection: &RuntimeInspectionV1) -> anyhow::Result<()> {
    bounded(&inspection.authority_id, 256)?;
    bounded(&inspection.stable_name, 128)?;
    bounded(&inspection.volume_name, 128)?;
    hash(&inspection.container_id)?;
    hash(&inspection.spec_hash)?;
    hash(
        inspection
            .image_id
            .strip_prefix("sha256:")
            .ok_or(ControlFailure::InvalidRequest)?,
    )?;
    Ok(())
}
fn inspection_matches(worker: &WorkerV1, i: &RuntimeInspectionV1, known_cid: bool) -> bool {
    i.authority_id == worker.launch.authority_id
        && i.runtime_kind == worker.launch.runtime_kind
        && i.stable_name == worker.attempt.stable_name
        && i.volume_name == worker.attempt.volume_name
        && i.image_id == worker.launch.image_id
        && i.spec_hash == worker.spec_hash
        && i.cpu_millis == worker.launch.cpu_millis
        && i.memory_bytes == worker.launch.memory_bytes
        && i.pids_limit == worker.launch.pids_limit
        && i.network == worker.launch.network
        && i.security_preset == worker.launch.security_preset
        && i.restart_policy == worker.launch.restart_policy
        && (!known_cid || worker.attempt.container_id.as_deref() == Some(i.container_id.as_str()))
}
pub(super) enum ObservationClass {
    Running,
    Terminal(String, String, u128),
    Unknown(&'static str),
}
pub(super) fn allocation_source(observation: &RuntimeObservationV1) -> AllocationSourceV1 {
    AllocationSourceV1 {
        authority_id: observation.inspection.authority_id.clone(),
        runtime_kind: observation.inspection.runtime_kind,
        container_id: observation.inspection.container_id.clone(),
        spec_hash: observation.inspection.spec_hash.clone(),
        provenance: AllocationProvenanceV1::Observed,
    }
}
pub(super) fn classify_observation(
    worker: &WorkerV1,
    observation: &RuntimeObservationV1,
) -> ObservationClass {
    if worker.allocation_inconsistent {
        return ObservationClass::Unknown(
            "allocation history is inconsistent; explicit reconciliation required",
        );
    }
    if !inspection_matches(worker, &observation.inspection, true) {
        return ObservationClass::Unknown("runtime identity or inspected configuration differs");
    }
    if worker.attempt.start_operation_id.is_none() {
        return ObservationClass::Unknown("runtime start was not claimed by control");
    }
    if observation.restart_count != Some(0) {
        return ObservationClass::Unknown("runtime restart count is missing or nonzero");
    }
    let Some(started) = observation
        .started_at
        .as_ref()
        .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
    else {
        return ObservationClass::Unknown("runtime start time is missing or invalid");
    };
    if started.timestamp() <= 0 {
        return ObservationClass::Unknown("runtime start time is not an observed execution time");
    }
    if let Some(previous) = worker
        .observed_started_at
        .as_ref()
        .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
        && previous.timestamp() > 0
        && previous != started
    {
        return ObservationClass::Unknown("runtime start time changed");
    }
    if observation.status == RuntimeStatusV1::Running && observation.running == Some(true) {
        return ObservationClass::Running;
    }
    if ![RuntimeStatusV1::Exited, RuntimeStatusV1::Dead].contains(&observation.status)
        || observation.running != Some(false)
        || observation.exit_code.is_none()
    {
        return ObservationClass::Unknown("runtime terminal state or exit code is incomplete");
    }
    let Some(finished) = observation
        .finished_at
        .as_ref()
        .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
    else {
        return ObservationClass::Unknown("runtime finish time is missing or invalid");
    };
    if finished < started {
        return ObservationClass::Unknown("runtime finish precedes start");
    }
    let Some(duration) = (finished - started)
        .num_nanoseconds()
        .and_then(|ns| u128::try_from(ns).ok())
    else {
        return ObservationClass::Unknown("runtime interval cannot be represented exactly");
    };
    ObservationClass::Terminal(
        started
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        finished
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        duration,
    )
}
