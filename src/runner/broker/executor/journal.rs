//! Private batch journal operations.
use super::*;

async fn load_draft(
    writer: &WriterClient,
    operation: Uuid,
    scope: &JournalScopeV1,
) -> Result<(AcceptedJobV1, Draft)> {
    ensure!(same(writer.scope(), scope)?, "writer scope changed");
    let job = writer.load(operation).await?.context("job disappeared")?;
    ensure!(
        job.operation_id() == operation && same(job.scope(), scope)?,
        "job scope changed"
    );
    let draft = Draft::from_job(&job)?;
    Ok((job, draft))
}

async fn commit_patch(
    writer: &WriterClient,
    operation: Uuid,
    scope: &JournalScopeV1,
    change: impl Fn(&mut Draft) -> Result<()>,
) -> Result<Box<CommittedDraftAtHead>> {
    for _ in 0..16 {
        let (job, mut draft) = load_draft(writer, operation, scope).await?;
        change(&mut draft)?;
        let next = draft.checkpoint(scope.control_namespace_uuid())?;
        #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
        tests::before_commit()?;
        match writer
            .advance_lifecycle(operation, job.head().clone(), next)
            .await
        {
            Ok(receipt) => {
                ensure!(
                    receipt.operation_id() == operation,
                    "checkpoint receipt operation changed"
                );
                // Only an acknowledged exact input produces this value. An
                // error, including an unknown publication, returns no Draft.
                return Ok(Box::new(CommittedDraftAtHead {
                    operation,
                    scope: writer.scope().clone(),
                    head: receipt.head().clone(),
                    draft,
                }));
            }
            Err(JournalError::Conflict) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(anyhow!("journal contention requires reconciliation"))
}

impl<'a> Execution<'a> {
    pub(super) async fn open(
        executor: &'a BatchExecutor,
        operation: Uuid,
        context: &'a AuthenticatedBatchContext,
    ) -> Result<Self> {
        ensure!(
            context.expected_generation() != u64::MAX
                && context.cancel_generation().current() == context.expected_generation(),
            "authenticated execution context was cancelled"
        );
        ensure!(
            !operation.is_nil() && same(executor.writer.scope(), context.scope())?,
            "scope mismatch"
        );
        let job = executor
            .writer
            .load(operation)
            .await?
            .context("job is unavailable")?;
        ensure!(same(job.scope(), context.scope())?, "job scope mismatch");
        if let Some(profile) = job.launch_profile_binding() {
            ensure!(
                profile.invocation_fingerprint == context.runtime().invocation().fingerprint(),
                "accepted Docker start profile invocation changed"
            );
        }
        let credentials = credentials_from_job(&job, context.config())?;
        let client = context.client().clone().with_secrets(
            credentials
                .as_ref()
                .map(|c| c.secrets().to_vec())
                .unwrap_or_default(),
        );
        let state = BrokerRunState::from_job(&job)?;
        ensure!(!state.options().interactive, "batch mode required");
        ensure!(
            state.options().config_dir == context.config().path()
                && state.options().runtime == context.runtime().kind,
            "trusted runtime mismatch"
        );
        Ok(Self {
            executor,
            context,
            operation,
            client,
            generation: context.expected_generation(),
            phase: AtomicU8::new(0),
            launch_profile: job.launch_profile().cloned(),
        })
    }

    pub(super) async fn load(&self) -> Result<(AcceptedJobV1, Draft)> {
        load_draft(&self.executor.writer, self.operation, self.context.scope()).await
    }

    pub(super) fn generation_current(&self) -> Result<()> {
        ensure!(
            self.generation != u64::MAX
                && self.context.cancel_generation().current() == self.generation,
            "execution generation was cancelled"
        );
        Ok(())
    }

    pub(super) fn stage(&self, phase: LocalFailurePhaseV1) {
        self.phase.store(
            match phase {
                LocalFailurePhaseV1::Decision => 0,
                LocalFailurePhaseV1::WorkspacePreparation => 1,
                LocalFailurePhaseV1::Admission => 2,
                LocalFailurePhaseV1::Import => 3,
                LocalFailurePhaseV1::ImportVerification => 4,
                LocalFailurePhaseV1::Worker => 5,
                LocalFailurePhaseV1::FinalScan => 6,
                LocalFailurePhaseV1::Export => 7,
                LocalFailurePhaseV1::Cleanup => 8,
            },
            Ordering::Relaxed,
        );
    }

    pub(super) fn phase(&self) -> LocalFailurePhaseV1 {
        match self.phase.load(Ordering::Relaxed) {
            0 => LocalFailurePhaseV1::Decision,
            1 => LocalFailurePhaseV1::WorkspacePreparation,
            2 => LocalFailurePhaseV1::Admission,
            3 => LocalFailurePhaseV1::Import,
            4 => LocalFailurePhaseV1::ImportVerification,
            6 => LocalFailurePhaseV1::FinalScan,
            7 => LocalFailurePhaseV1::Export,
            8 => LocalFailurePhaseV1::Cleanup,
            _ => LocalFailurePhaseV1::Worker,
        }
    }

    pub(super) async fn patch(&self, change: impl Fn(&mut Draft) -> Result<()>) -> Result<()> {
        self.patch_committed(change).await.map(drop)
    }

    async fn patch_committed(
        &self,
        change: impl Fn(&mut Draft) -> Result<()>,
    ) -> Result<Box<CommittedDraftAtHead>> {
        commit_patch(
            &self.executor.writer,
            self.operation,
            self.context.scope(),
            change,
        )
        .await
    }

    pub(super) async fn failure(&self, phase: LocalFailurePhaseV1, error: &anyhow::Error) {
        // A transport/IO error is a local observation. It never fabricates a
        // terminal worker and never releases central quota.
        let _ = self
            .executor
            .writer
            .append_command(
                self.operation,
                Uuid::new_v4(),
                LocalCommandRequestV1::FailureObserved {
                    phase,
                    code: LocalFailureCodeV1::Unavailable,
                },
            )
            .await;
        // Reporting the observed error does not assert that its lifecycle
        // append succeeded; that publication may itself be unknown.
        let diagnostic = diagnostics::failure_report(phase, error);
        if self
            .fact_event("broker.batch.failure", &diagnostic)
            .await
            .is_err()
        {
            // Enqueue may already be durable when HTTP/ACK delivery fails.
            // Do not claim that it is absent, retry the start, or print the
            // underlying error (which may carry a provider's private text).
            eprintln!(
                "broker batch diagnostic delivery incomplete; inspect pending journal events"
            );
        }
    }

    pub(super) async fn rpc(&self, pending: Operation) -> Result<(Operation, bool)> {
        self.rpc_checked(pending, None).await
    }

    pub(super) async fn rpc_checked(
        &self,
        pending: Operation,
        ready: Option<&EnforcedSlot>,
    ) -> Result<(Operation, bool)> {
        let (completed, fresh, _) = self.rpc_checked_at_head(pending, ready).await?;
        Ok((completed, fresh))
    }

    pub(super) async fn rpc_checked_at_head(
        &self,
        pending: Operation,
        ready: Option<&EnforcedSlot>,
    ) -> Result<(Operation, bool, Box<CommittedDraftAtHead>)> {
        let key = operation_key(&pending);
        let (job, draft) = self.load().await?;
        let existing = draft
            .operations
            .iter()
            .find(|op| operation_key(op) == key)
            .cloned();
        let fresh = existing.is_none();
        let input = if let Some(existing) = existing {
            ensure!(
                same(&without_response(&existing), &pending)?,
                "operation identity conflicts"
            );
            if acknowledged(&existing) {
                // The strict Full load already confirmed this historical head.
                // Its data does not become current if another mutation follows.
                return Ok((
                    existing,
                    false,
                    Box::new(CommittedDraftAtHead {
                        operation: self.operation,
                        scope: self.executor.writer.scope().clone(),
                        head: job.head().clone(),
                        draft,
                    }),
                ));
            }
            existing
        } else {
            self.patch(|draft| {
                if let Some(existing) = draft.operations.iter().find(|op| operation_key(op) == key)
                {
                    ensure!(
                        same(&without_response(existing), &pending)?,
                        "pending operation conflicts"
                    );
                    return Ok(());
                }
                draft.operations.push(pending.clone());
                Ok(())
            })
            .await?;
            pending
        };
        drop(job);
        drop(draft);
        if let Some(slot) = ready {
            self.generation_current()?;
            slot.check_ready_for_claim_diagnosed(self.generation)
                .context(diagnostics::BrokerStep::ReadinessBeforeClaim)?;
        }
        let completed = self.call(input.clone()).await?;
        let committed = self
            .patch_committed(|draft| {
                let existing = draft
                    .operations
                    .iter_mut()
                    .find(|op| operation_key(op) == key)
                    .context("pending request disappeared")?;
                ensure!(
                    same(&without_response(existing), &input)?,
                    "pending request changed"
                );
                if acknowledged(existing) {
                    ensure!(same(existing, &completed)?, "acknowledgement changed");
                }
                *existing = completed.clone();
                if let Some(worker) = response_worker(&completed) {
                    let managed = draft
                        .run
                        .plan
                        .managed
                        .as_mut()
                        .context("worker without managed state")?;
                    if managed
                        .worker
                        .as_ref()
                        .is_none_or(|old| old.revision <= worker.revision)
                    {
                        managed.worker = Some(worker.clone());
                    }
                }
                Ok(())
            })
            .await?;
        Ok((completed, fresh, committed))
    }

    pub(super) async fn call(&self, input: Operation) -> Result<Operation> {
        Ok(match input {
            Operation::Decision { request, .. } => {
                let answer = self.client.decide(&request).await?;
                let response = DecisionAnswerV1::new(
                    serde_json::from_value(answer["decision"].clone())?,
                    serde_json::from_value(answer["receipt"].clone())?,
                )?;
                Operation::Decision {
                    request,
                    response: Some(response),
                }
            }
            Operation::Admission { request, .. } => {
                // An idempotent POST whose first outcome is unknown can still
                // create a reservation. Preserve pending until supervision is
                // healthy; a stored ACK is returned earlier without this call.
                self.executor.enforcer.require_admission_ready().await?;
                self.generation_current()?;
                let response = self.client.admit_supervised(&request).await?;
                Operation::Admission {
                    request,
                    response: Some(response),
                }
            }
            Operation::WorkerMutation { request, .. } => {
                let response = self
                    .client
                    .post_control(
                        &format!("api/v1/workers/{}/transitions", request.worker_id),
                        &request,
                    )
                    .await?;
                Operation::WorkerMutation {
                    request,
                    response: Some(response),
                }
            }
            Operation::Arm {
                parent,
                owned,
                request,
                ..
            } => {
                let response = self
                    .client
                    .post_control(
                        &format!("api/v1/workers/{}/arms", request.worker_id),
                        &request,
                    )
                    .await?;
                Operation::Arm {
                    parent,
                    owned,
                    request,
                    response: Some(response),
                }
            }
            Operation::WorkerClaim { request, .. } => {
                let response = self
                    .client
                    .post_control(
                        &format!("api/v1/workers/{}/armed-start-claims", request.worker_id),
                        &request,
                    )
                    .await?;
                Operation::WorkerClaim {
                    request,
                    response: Some(response),
                }
            }
            Operation::HelperClaim { request, .. } => {
                let response = self
                    .client
                    .post_control(
                        &format!(
                            "api/v1/workers/{}/helper-start-claims",
                            request.arm_receipt.worker_id
                        ),
                        &request,
                    )
                    .await?;
                Operation::HelperClaim {
                    request,
                    response: Some(response),
                }
            }
            Operation::HelperObservation { request, .. } => {
                let response = self
                    .client
                    .post_control(
                        &format!(
                            "api/v1/workers/{}/helper-observations",
                            request.arm_receipt.worker_id
                        ),
                        &request,
                    )
                    .await?;
                Operation::HelperObservation {
                    request,
                    response: Some(response),
                }
            }
            Operation::HelperCleanup { request, .. } => {
                let response = self
                    .client
                    .post_control(
                        &format!(
                            "api/v1/workers/{}/helper-cleanup",
                            request.arm_receipt.worker_id
                        ),
                        &request,
                    )
                    .await?;
                Operation::HelperCleanup {
                    request,
                    response: Some(response),
                }
            }
            Operation::Export { request, .. } => {
                let response = self
                    .client
                    .post_control(
                        &format!("api/v1/workers/{}/export-requests", request.worker_id),
                        &request,
                    )
                    .await?;
                Operation::Export {
                    request,
                    response: Some(response),
                }
            }
        })
    }

    pub(super) async fn reconcile_pending(&self) -> Result<()> {
        let (_, draft) = self.load().await?;
        for operation in draft.operations.into_iter().filter(|op| !acknowledged(op)) {
            // Even a freshly returned EmitOnce here is data only. No runtime
            // callback receives permission from replaying this list.
            self.rpc(operation).await?;
        }
        Ok(())
    }

    pub(super) async fn mutate(&self, transition: WorkerTransitionV1) -> Result<WorkerV1> {
        if matches!(
            transition,
            WorkerTransitionV1::Prepare | WorkerTransitionV1::CreateIssued
        ) {
            self.generation_current()?;
        }
        let (_, draft) = self.load().await?;
        let worker = draft.worker()?;
        let request = WorkerMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            expected_revision: worker.revision,
            transition,
        };
        let (ack, _) = self
            .rpc(Operation::WorkerMutation {
                request,
                response: None,
            })
            .await?;
        response_worker(&ack)
            .cloned()
            .context("worker mutation has no result")
    }
}

pub(super) fn same(left: &impl Serialize, right: &impl Serialize) -> Result<bool> {
    Ok(serde_json::to_value(left)? == serde_json::to_value(right)?)
}
pub(super) fn response_worker(operation: &Operation) -> Option<&WorkerV1> {
    match operation {
        Operation::Admission {
            response: Some(response),
            ..
        } => response.worker.as_ref(),
        Operation::WorkerMutation {
            response: Some(response),
            ..
        }
        | Operation::WorkerClaim {
            response: Some(response),
            ..
        } => Some(&response.worker),
        _ => None,
    }
}
pub(super) fn operation_key(operation: &Operation) -> String {
    match operation {
        Operation::Decision { request, .. } => {
            format!("decision:{}:{}", request.request_id, request.human_approved)
        }
        Operation::Admission { request, .. } => request.admission.operation_id.clone(),
        Operation::WorkerMutation { request, .. } => request.operation_id.clone(),
        Operation::Arm { request, .. } => request.operation_id.clone(),
        Operation::WorkerClaim { request, .. } => request.operation_id.clone(),
        Operation::HelperClaim { request, .. } => request.operation_id.clone(),
        Operation::HelperObservation { request, .. } => request.operation_id.clone(),
        Operation::HelperCleanup { request, .. } => request.operation_id.clone(),
        Operation::Export { request, .. } => request.operation_id.clone(),
    }
}
pub(super) fn acknowledged(operation: &Operation) -> bool {
    match operation {
        Operation::Decision { response, .. } => response.is_some(),
        Operation::Admission { response, .. } => response.is_some(),
        Operation::WorkerMutation { response, .. } | Operation::WorkerClaim { response, .. } => {
            response.is_some()
        }
        Operation::Arm { response, .. } => response.is_some(),
        Operation::HelperClaim { response, .. } => response.is_some(),
        Operation::HelperObservation { response, .. }
        | Operation::HelperCleanup { response, .. }
        | Operation::Export { response, .. } => response.is_some(),
    }
}
pub(super) fn without_response(operation: &Operation) -> Operation {
    let mut operation = operation.clone();
    match &mut operation {
        Operation::Decision { response, .. } => *response = None,
        Operation::Admission { response, .. } => *response = None,
        Operation::WorkerMutation { response, .. } | Operation::WorkerClaim { response, .. } => {
            *response = None
        }
        Operation::Arm { response, .. } => *response = None,
        Operation::HelperClaim { response, .. } => *response = None,
        Operation::HelperObservation { response, .. }
        | Operation::HelperCleanup { response, .. }
        | Operation::Export { response, .. } => *response = None,
    }
    operation
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
#[path = "journal_tests.rs"]
mod tests;
