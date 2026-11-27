//! Observation-only convergence for an owned worker's terminal.
use super::*;

pub(super) enum WorkerTerminalSource<'a> {
    DurableCancellation,
    JoinedPrearmedStop {
        runtime: &'a ManagedRuntime,
        interruption: &'a crate::runtime::JoinedPrearmedStop,
    },
}

impl Execution<'_> {
    pub(super) async fn reconcile_retained_worker_terminal(
        &self,
        source: WorkerTerminalSource<'_>,
    ) -> Result<()> {
        self.stage(LocalFailurePhaseV1::Worker);
        let (_, draft) = self.load().await?;
        match &source {
            WorkerTerminalSource::DurableCancellation => {
                ensure!(draft.cancelled(), "durable cancellation is missing");
            }
            WorkerTerminalSource::JoinedPrearmedStop { .. } => {
                self.generation_current()?;
                ensure!(
                    !draft.cancelled(),
                    "durable cancellation owns reconciliation"
                );
            }
        }
        let worker = draft.worker()?;
        ensure!(
            matches!(
                worker.state,
                WorkerStateV1::Running | WorkerStateV1::Stopping
            ) && worker.quota_held
                && !worker.allocation_inconsistent,
            "worker has no reconcilable running allocation"
        );
        let checkpoint = draft
            .run
            .plan
            .managed
            .as_ref()
            .context("managed state is missing")?;
        let binding = checkpoint
            .binding
            .as_ref()
            .context("retained runtime authority is missing")?;
        let owned = checkpoint
            .owned
            .as_ref()
            .context("retained worker ownership is missing")?;
        let created = worker
            .attempt
            .created_inspection
            .as_ref()
            .context("created inspection is missing")?;
        ensure!(
            binding.kind == self.context.runtime().kind
                && binding.invocation.as_ref() == Some(self.context.runtime().invocation())
                && binding.authority_id.to_string() == worker.launch.authority_id
                && owned.handle.operation_id() == self.operation
                && owned.handle.session_id() == draft.run.session()
                && owned.handle.authority_fingerprint() == binding.fingerprint()
                && worker.attempt.container_id.as_deref() == Some(owned.handle.id())
                && created.container_id == owned.handle.id()
                && created.stable_name == owned.handle.name(),
            "retained worker authority or ownership changed"
        );
        let (parent, armed_owned, arm, armed) = draft
            .operations
            .iter()
            .find_map(|operation| match operation {
                Operation::Arm {
                    parent,
                    owned,
                    request,
                    response: Some(response),
                } if matches!(request.subject, ArmSubjectV1::Worker)
                    && request.worker_id == worker.worker_id
                    && request.attempt_id == worker.attempt.attempt_id =>
                {
                    Some((parent, owned, request, response))
                }
                _ => None,
            })
            .context("acknowledged worker arm is missing")?;
        arm.validate_for_parent(parent)?;
        ensure!(
            same(armed_owned, owned)?
                && arm.inspection == *created
                && arm.authority_binding_hash == binding.fingerprint()
                && same(&armed.arm, arm)?
                && armed.receipt.arm_id == arm.operation_id
                && armed.receipt.worker_id == worker.worker_id
                && armed.receipt.attempt_id == worker.attempt.attempt_id
                && armed.receipt.control_namespace_uuid
                    == self.context.scope().control_namespace_uuid().to_string(),
            "retained worker arm differs from its ownership"
        );
        let (claim, claimed) = draft
            .operations
            .iter()
            .find_map(|operation| match operation {
                Operation::WorkerClaim {
                    request,
                    response: Some(response),
                } if request.worker_id == worker.worker_id
                    && request.attempt_id == worker.attempt.attempt_id
                    && worker.attempt.start_operation_id.as_deref()
                        == Some(request.operation_id.as_str()) =>
                {
                    Some((request, response))
                }
                _ => None,
            })
            .context("acknowledged historical worker claim is missing")?;
        claim.validate_for_arm(arm)?;
        ensure!(
            same(&claim.arm_receipt, &armed.receipt)?
                && worker.attempt.start_emitter_id.as_deref() == Some(claim.emitter_id.as_str())
                && claimed.disposition.is_some()
                && claimed.receipt.operation_id == claim.operation_id
                && claimed.worker.worker_id == worker.worker_id
                && claimed.worker.attempt.attempt_id == worker.attempt.attempt_id
                && claimed.worker.attempt.start_operation_id == worker.attempt.start_operation_id,
            "historical worker claim does not match the allocation"
        );
        let started = chrono::DateTime::parse_from_rfc3339(
            worker
                .observed_started_at
                .as_deref()
                .context("historical worker start is unknown")?,
        )?;
        ensure!(
            started.timestamp() > 0,
            "historical worker start is invalid"
        );

        let reopened;
        let runtime = match source {
            WorkerTerminalSource::DurableCancellation => {
                reopened = ManagedRuntime::reopen_with_runtime(
                    binding.clone(),
                    self.context.runtime().clone(),
                )
                .await?;
                &reopened
            }
            WorkerTerminalSource::JoinedPrearmedStop {
                runtime,
                interruption,
            } => {
                ensure!(
                    runtime.binding() == binding
                        && interruption.container_id() == owned.handle.id(),
                    "joined transport differs from the retained worker"
                );
                runtime
            }
        };
        let terminal = runtime.resume_terminal(&owned.handle).await?;
        terminal.validate_terminal(binding.kind, &owned.observation)?;
        let observed_start = terminal
            .process
            .started_at
            .context("terminal start is unknown")?;
        ensure!(
            observed_start.with_timezone(&chrono::Utc) == started.with_timezone(&chrono::Utc),
            "runtime start differs from the centrally observed allocation"
        );

        if !draft.cancelled() {
            self.generation_current()?;
        }
        let terminal = Box::pin(self.save_observed_result(&terminal)).await?;
        let acknowledged = Box::pin(self.mutate(WorkerTransitionV1::Observe {
            observation: managed::observation_dto(worker, &terminal)?,
        }))
        .await?;
        ensure!(
            acknowledged.worker_id == worker.worker_id
                && acknowledged.attempt.attempt_id == worker.attempt.attempt_id
                && acknowledged.state == WorkerStateV1::Terminal
                && !acknowledged.quota_held,
            "worker terminal is not centrally acknowledged"
        );
        Box::pin(self.post_worker_result()).await
    }
}
