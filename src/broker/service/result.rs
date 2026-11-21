//! Read-only batch result from a confirmed business journal head.
use super::{jobs, protocol::*};
use crate::{
    broker::{
        checkpoint::SupervisionOperationV1,
        journal::{AcceptedJobV1, JournalStatusV1, valid_hash},
        lifecycle::LocalCommandRequestV1,
        outbox::OutboxEventV1,
        terminal_facts::BatchResultFactsV1,
    },
    control::{RuntimeStatusV1, WorkerStateV1, WorkerTransitionV1, WorkerV1},
    model::Outcome,
    runner::broker_observation_data,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

#[cfg(test)]
#[path = "result_tests.rs"]
mod tests;

pub(super) fn project(
    operation_id: Uuid,
    durable: Option<JournalStatusV1>,
    execution: (bool, Option<BrokerErrorCodeV1>),
) -> JobResultV1 {
    use JobResultOutcomeV1 as State;

    let mut cancel_requested = false;
    let outcome = match durable {
        None => unknown(BrokerErrorCodeV1::NeedsReconciliation),
        Some(JournalStatusV1::ReservedInProcess) => State::Pending,
        Some(JournalStatusV1::Interrupted) => unknown(BrokerErrorCodeV1::Interrupted),
        Some(JournalStatusV1::Rejected(rejection)) => State::Refused {
            reason: JobRefusalV1::Submission {
                phase: rejection.phase(),
                code: rejection.code(),
            },
        },
        Some(JournalStatusV1::Accepted(job)) => {
            cancel_requested = job.broker_checkpoint_v3().is_some_and(|checkpoint| {
                checkpoint.commands().iter().any(|command| {
                    matches!(command.request(), LocalCommandRequestV1::CancelRequested)
                })
            });
            accepted(&job, cancel_requested).unwrap_or_else(|| match execution {
                (true, _) => State::Pending,
                (false, error) => unknown(error.unwrap_or(BrokerErrorCodeV1::NeedsReconciliation)),
            })
        }
    };
    JobResultV1 {
        version: JOB_RESULT_VERSION,
        operation_id,
        cancel_requested,
        outcome,
    }
}

fn unknown(reason: BrokerErrorCodeV1) -> JobResultOutcomeV1 {
    JobResultOutcomeV1::Ambiguous { reason }
}

fn accepted(job: &AcceptedJobV1, cancel_requested: bool) -> Option<JobResultOutcomeV1> {
    use JobResultOutcomeV1 as State;

    let operations = job
        .broker_checkpoint_v2()
        .map(|checkpoint| checkpoint.supervision().operations())
        .unwrap_or_default();
    let context = job.checkpoint().broker_context();
    if let Some(worker) = context.worker
        && let Some(checkpoint) = job.broker_checkpoint_v4()
        && let Some(result) = terminal(
            job.intent().session(),
            worker,
            operations,
            checkpoint.events(),
        )
    {
        return Some(State::Terminal { result });
    }
    if job
        .broker_checkpoint_v5()
        .is_some_and(|checkpoint| !checkpoint.retirements().is_empty())
    {
        // An explicit pre-claim abandonment has its own settlement protocol.
        // While that work is active the batch result remains pending. A
        // retained retirement by itself confirms neither exit nor cancellation.
        return None;
    }
    if cancel_requested {
        let no_runtime = match context.worker {
            Some(worker) => worker.state == WorkerStateV1::Cancelled && !worker.quota_held,
            None => !operations.iter().any(|operation| {
                matches!(operation, SupervisionOperationV1::Admission { response, .. }
                    if response.as_ref().is_none_or(|answer| answer.accepted))
            }),
        };
        return no_runtime.then_some(State::Cancelled);
    }
    if let Some(code) = operations.iter().find_map(|operation| match operation {
        SupervisionOperationV1::Admission {
            response: Some(answer),
            ..
        } if !answer.accepted => answer.refusal,
        _ => None,
    }) {
        return Some(State::Refused {
            reason: JobRefusalV1::Admission { code },
        });
    }
    let decision = jobs::last_decision(job)?;
    match decision.outcome {
        Outcome::Deny => Some(State::Refused {
            reason: JobRefusalV1::Decision { decision },
        }),
        Outcome::Review => {
            if job.broker_checkpoint_v3().is_some_and(|checkpoint| {
                checkpoint.commands().iter().any(|command| {
                    matches!(
                        command.request(),
                        LocalCommandRequestV1::ApprovalSubmitted { .. }
                    )
                })
            }) {
                return None;
            }
            let (request_hash, _) = jobs::review(job)?;
            Some(State::Review {
                request_hash,
                decision,
            })
        }
        Outcome::Allow => None,
    }
}

fn terminal(
    session_id: Uuid,
    worker: &WorkerV1,
    operations: &[SupervisionOperationV1],
    events: &[OutboxEventV1],
) -> Option<TerminalJobResultV1> {
    if worker.state != WorkerStateV1::Terminal
        || worker.quota_held
        || worker.cleanup_removed != Some(true)
    {
        return None;
    }
    let container_id = worker.attempt.container_id.as_deref()?;
    let observation = worker.last_observation.as_ref()?;
    let exit_code = observation.exit_code?;
    if !matches!(
        observation.status,
        RuntimeStatusV1::Exited | RuntimeStatusV1::Dead
    ) || observation.running != Some(false)
        || observation.inspection.container_id != container_id
    {
        return None;
    }
    let cleanup_audit_seq = operations
        .iter()
        .rev()
        .find_map(|operation| match operation {
            SupervisionOperationV1::WorkerMutation {
                request,
                response: Some(answer),
            } if matches!(
                request.transition,
                WorkerTransitionV1::CleanupResult {
                    removed: true,
                    error: None
                }
            ) && request.worker_id == worker.worker_id
                && request.attempt_id == worker.attempt.attempt_id
                && answer.worker.revision == worker.revision =>
            {
                Some(answer.receipt.audit_seq)
            }
            _ => None,
        })?;
    if cleanup_audit_seq <= 0 {
        return None;
    }

    let (result_audit_seq, data) = batch_event(events, "broker.batch.result", &worker.worker_id)?;
    let result = BatchResultFactsV1::deserialize(data).ok()?;
    if result.attempt_id != worker.attempt.attempt_id {
        return None;
    }
    let mut retained = broker_observation_data(worker, &result.observation).ok()?;
    // Reconciliation may observe the same finished allocation at a later time.
    retained.observed_at.clone_from(&observation.observed_at);
    if retained != *observation {
        return None;
    }

    let (scan_audit_seq, scan) = batch_event(events, "broker.batch.scan", &worker.worker_id)?;
    let changed_files = usize::try_from(scan.get("changed_files")?.as_u64()?).ok()?;
    let manifest_digest = scan.get("manifest_digest")?.as_str()?;
    valid_hash(manifest_digest).ok()?;

    Some(TerminalJobResultV1 {
        session_id,
        volume: worker.attempt.volume_name.clone(),
        container_id: container_id.to_owned(),
        exit_code,
        oom_killed: observation.oom_killed,
        changed_files,
        manifest_digest: manifest_digest.to_owned(),
        output_captured: worker.launch.capture_output
            && result.transport.is_some_and(|transport| {
                transport.stdout.text.is_some() && transport.stderr.text.is_some()
            }),
        result_audit_seq,
        scan_audit_seq,
        cleanup_audit_seq,
    })
}

fn batch_event<'a>(
    events: &'a [OutboxEventV1],
    kind: &str,
    worker_id: &str,
) -> Option<(i64, &'a Value)> {
    events.iter().rev().find_map(|entry| {
        let event = entry.event();
        let seq = entry.audit_seq()?;
        (event.kind == kind
            && event.data.get("version")?.as_u64()? == 1
            && event.data.get("worker_id")?.as_str()? == worker_id)
            .then_some((seq, &event.data))
    })
}
