//! Batch execution from a durable intention. Journal I/O is delegated to the
//! bounded writer; the enforcer owns stop independently of that writer.
mod helpers;
mod journal;
mod results;
use super::{BrokerRunState, FreshStartPermission, credentials_from_job};
use crate::{
    broker::{
        checkpoint::{
            BrokerCheckpointV2, DecisionAnswerV1, HelperPreparationV1, SupervisionCheckpointV1,
            SupervisionOperationV1 as Operation,
        },
        clock::{HostClock, checked_deadline},
        enforcer::{CancelGeneration, EnforcedSlot, EnforcerClient, RegistrationV1},
        journal::{AcceptedJobV1, JournalError},
        lifecycle::{
            BrokerCheckpointV3, LocalCommandRequestV1, LocalCommandV1, LocalFailureCodeV1,
            LocalFailurePhaseV1,
        },
        service::{AuthenticatedBatchContext, ValidatedExportTarget},
        writer::WriterClient,
    },
    control::*,
    model::Outcome,
    runner::{AuditClient, CheckpointDataV1, CheckpointPayloadV1, managed},
    runtime::{
        ContainerObservation, ManagedPrearmedExecution, ManagedRuntime, OwnedContainerSnapshot,
        PrearmedStartDirective,
    },
    workspace::{self, Limits},
};
use anyhow::{Context, Result, anyhow, ensure};
use journal::{operation_key, same};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct BatchExecutor {
    writer: WriterClient,
    enforcer: EnforcerClient,
    clock: Arc<dyn HostClock>,
}
impl fmt::Debug for BatchExecutor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BatchExecutor(<private>)")
    }
}

/// Disposition of this invocation. Detailed results are already persisted in
/// the journal and audit; the service reads those facts for status responses.
pub(crate) enum BatchOutcome {
    AwaitingReview,
    Denied,
    AdmissionRefused,
    Cancelled,
    Terminal,
}
impl fmt::Debug for BatchOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BatchOutcome(<private observed state>)")
    }
}
pub(crate) struct ExportCompletion {
    pub(crate) request_id: Uuid,
    pub(crate) manifest_digest: String,
    pub(crate) files: usize,
}
impl fmt::Debug for ExportCompletion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExportCompletion(<private>)")
    }
}
pub(crate) struct BatchFailure;
impl fmt::Display for BatchFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("broker batch requires reconciliation; inspect its durable status")
    }
}
impl fmt::Debug for BatchFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl std::error::Error for BatchFailure {}

struct Draft {
    run: BrokerRunState,
    helpers: Vec<HelperPreparationV1>,
    operations: Vec<Operation>,
    commands: Vec<LocalCommandV1>,
}
impl Draft {
    fn from_job(job: &AcceptedJobV1) -> Result<Self> {
        let supervision = job.broker_checkpoint_v2().map(|cp| cp.supervision());
        Ok(Self {
            run: BrokerRunState::from_job(job)?,
            helpers: supervision
                .map(|s| s.helpers().to_vec())
                .unwrap_or_default(),
            operations: supervision
                .map(|s| s.operations().to_vec())
                .unwrap_or_default(),
            commands: job
                .broker_checkpoint_v3()
                .map(|cp| cp.commands().to_vec())
                .unwrap_or_default(),
        })
    }
    fn cancelled(&self) -> bool {
        self.commands
            .iter()
            .any(|c| matches!(c.request(), LocalCommandRequestV1::CancelRequested))
    }
    fn worker(&self) -> Result<&WorkerV1> {
        self.run
            .plan
            .managed
            .as_ref()
            .and_then(|m| m.worker.as_ref())
            .context("worker is not admitted")
    }
    fn checkpoint(self, namespace: Uuid) -> Result<BrokerCheckpointV3> {
        let runner = CheckpointPayloadV1(CheckpointDataV1 {
            version: 1,
            current_request: self.run.plan.request,
            managed: self.run.plan.managed,
        });
        Ok(BrokerCheckpointV3::new(
            BrokerCheckpointV2::new(
                runner,
                SupervisionCheckpointV1::new(namespace, self.helpers, self.operations)?,
            )?,
            self.commands,
        )?)
    }
}

struct Execution<'a> {
    executor: &'a BatchExecutor,
    context: &'a AuthenticatedBatchContext,
    operation: Uuid,
    client: AuditClient,
    generation: u64,
    phase: AtomicU8,
}

impl BatchExecutor {
    pub(crate) fn new(
        writer: WriterClient,
        enforcer: EnforcerClient,
        clock: Arc<dyn HostClock>,
    ) -> Self {
        Self {
            writer,
            enforcer,
            clock,
        }
    }
    pub(crate) async fn run(
        &self,
        operation: Uuid,
        context: &AuthenticatedBatchContext,
    ) -> std::result::Result<BatchOutcome, BatchFailure> {
        let execution = Execution::open(self, operation, context)
            .await
            .map_err(|_| BatchFailure)?;
        match execution.run().await {
            Ok(outcome) => Ok(outcome),
            Err(_) => {
                execution.failure(execution.phase()).await;
                Err(BatchFailure)
            }
        }
    }
    pub(crate) async fn export(
        &self,
        operation: Uuid,
        target: ValidatedExportTarget,
        context: &AuthenticatedBatchContext,
    ) -> std::result::Result<ExportCompletion, BatchFailure> {
        let execution = Execution::open(self, operation, context)
            .await
            .map_err(|_| BatchFailure)?;
        match execution.export(target).await {
            Ok(outcome) => Ok(outcome),
            Err(_) => {
                execution.failure(LocalFailurePhaseV1::Export).await;
                Err(BatchFailure)
            }
        }
    }
    /// The caller first invalidates this same shared generation and durably
    /// appends CancelRequested. This method only restores and stops old targets.
    /// Success acknowledges delivery to known armed targets; it proves neither
    /// runtime terminal/absence nor central quota release.
    pub(crate) async fn cancel(
        &self,
        operation: Uuid,
        generation: u64,
        cancel: &CancelGeneration,
    ) -> Result<()> {
        ensure!(
            generation != u64::MAX && generation == cancel.current(),
            "invalid cancellation generation"
        );
        let job = self
            .writer
            .load(operation)
            .await?
            .context("cancellation job is unavailable")?;
        ensure!(
            same(job.scope(), self.writer.scope())?,
            "cancellation scope mismatch"
        );
        ensure!(
            job.broker_checkpoint_v3()
                .is_some_and(
                    |checkpoint| checkpoint.commands().iter().any(|command| matches!(
                        command.request(),
                        LocalCommandRequestV1::CancelRequested
                    ))
                ),
            "cancellation is not durable"
        );
        let registrations = retained_registrations(&job)?;
        // Every target gets an attempt, even if another target is unavailable.
        // The list is bounded by the validated checkpoint operation limit.
        let mut tasks = tokio::task::JoinSet::new();
        for registration in registrations {
            let enforcer = self.enforcer.clone();
            let cancel = cancel.clone();
            tasks.spawn(async move {
                let slot = enforcer.restore(registration, cancel.clone()).await?;
                slot.cancel(cancel.current()).await
            });
        }
        let mut failed = false;
        while let Some(result) = tasks.join_next().await {
            if !matches!(result, Ok(Ok(()))) {
                failed = true;
            }
        }
        ensure!(
            !failed,
            "one or more cancellation targets require reconciliation"
        );
        Ok(())
    }
}

/// Extracts already acknowledged arms only. This is a pure stop inventory,
/// never an authentication check, a fresh arm, or a start permission.
pub(crate) fn retained_registrations(job: &AcceptedJobV1) -> Result<Vec<RegistrationV1>> {
    let Some(checkpoint) = job.broker_checkpoint_v2() else {
        return Ok(Vec::new());
    };
    let arms = checkpoint
        .supervision()
        .operations()
        .iter()
        .filter_map(|operation| match operation {
            Operation::Arm {
                owned,
                request,
                response: Some(response),
                ..
            } => Some((owned, request, response)),
            _ => None,
        })
        .collect::<Vec<_>>();
    if arms.is_empty() {
        return Ok(Vec::new());
    }
    let context = job.checkpoint().broker_context();
    let authority = context
        .binding
        .context("retained arm has no durable runtime binding")?;
    let origin = hex::encode(Sha256::digest(job.scope().gateway_origin().as_bytes()));
    arms.into_iter()
        .map(|(owned, request, response)| {
            ensure!(
                same(request, &response.arm)?,
                "retained arm acknowledgement differs"
            );
            Ok(RegistrationV1::new(
                origin.clone(),
                job.scope().tenant_id().into(),
                request.clone(),
                response.receipt.clone(),
                authority.clone(),
                owned.clone(),
            )?)
        })
        .collect()
}

impl<'a> Execution<'a> {
    async fn run(&self) -> Result<BatchOutcome> {
        self.stage(LocalFailurePhaseV1::Decision);
        self.replay_events().await?;
        self.reconcile_pending().await?;
        let (job, mut draft) = self.load().await?;
        if draft.cancelled() {
            let worker = draft
                .run
                .plan
                .managed
                .as_ref()
                .and_then(|m| m.worker.clone());
            if worker
                .as_ref()
                .is_some_and(|w| w.state == WorkerStateV1::Reserved)
            {
                self.mutate(WorkerTransitionV1::CancelReserved).await?;
            }
            return Ok(BatchOutcome::Cancelled);
        }
        self.generation_current()?;
        if draft.run.plan.managed.is_none() {
            self.executor.enforcer.require_admission_ready().await?;
            let mut request = draft.run.plan.request.clone();
            let (answer, _) = self
                .rpc(Operation::Decision {
                    request: request.clone(),
                    response: None,
                })
                .await?;
            let Operation::Decision {
                response: Some(mut answer),
                ..
            } = answer
            else {
                return Err(anyhow!("decision acknowledgement is missing"));
            };
            if answer.decision().outcome == Outcome::Review {
                let (_, current) = self.load().await?;
                let approval =
                    current
                        .commands
                        .iter()
                        .find_map(|command| match command.request() {
                            LocalCommandRequestV1::ApprovalSubmitted {
                                request_hash,
                                review_receipt,
                            } if same(review_receipt, answer.receipt()).unwrap_or(false) => {
                                Some(request_hash.clone())
                            }
                            _ => None,
                        });
                let Some(hash) = approval else {
                    return Ok(BatchOutcome::AwaitingReview);
                };
                request.human_approved = true;
                request.approval_hash = Some(hash);
                self.patch(|draft| {
                    ensure!(
                        !draft.cancelled() && draft.run.plan.managed.is_none(),
                        "approval context changed"
                    );
                    draft.run.plan.request = request.clone();
                    let pending = Operation::Decision {
                        request: request.clone(),
                        response: None,
                    };
                    if !draft
                        .operations
                        .iter()
                        .any(|op| operation_key(op) == operation_key(&pending))
                    {
                        draft.operations.push(pending);
                    }
                    Ok(())
                })
                .await?;
                let (approved, _) = self
                    .rpc(Operation::Decision {
                        request,
                        response: None,
                    })
                    .await?;
                let Operation::Decision {
                    response: Some(approved),
                    ..
                } = approved
                else {
                    return Err(anyhow!("approved decision acknowledgement is missing"));
                };
                answer = approved;
            }
            match answer.decision().outcome {
                Outcome::Deny => return Ok(BatchOutcome::Denied),
                Outcome::Review => {
                    return Ok(BatchOutcome::AwaitingReview);
                }
                Outcome::Allow => {}
            }
            self.generation_current()?;
            self.stage(LocalFailurePhaseV1::WorkspacePreparation);
            self.executor.enforcer.require_admission_ready().await?;
            let (current_job, current) = self.load().await?;
            ensure!(
                !current.cancelled(),
                "cancelled before workspace preparation"
            );
            let config = current.run.options().config_dir.clone();
            let operation = self.operation;
            tokio::task::spawn_blocking(move || {
                managed::broker_require_new_archive(&config, operation)
            })
            .await??;
            let source = self
                .context
                .source_handle()
                .context("authorized source is unavailable for initial preparation")?;
            let exclusions = self
                .context
                .excluded_roots()
                .iter()
                .map(|file| file.try_clone())
                .collect::<std::io::Result<Vec<_>>>()?;
            let workspace = tokio::task::spawn_blocking(move || {
                let exclusions = exclusions.iter().collect::<Vec<_>>();
                source.prepare(Limits::default(), &exclusions)
            })
            .await??;
            let credentials = credentials_from_job(&current_job, self.context.config())?;
            let managed = managed::broker_prepare(managed::BrokerPreparation {
                plan: &current.run.plan,
                scope: self.context.scope(),
                binding: current_job.binding_manifest(),
                answer: &answer,
                runtime: self.context.runtime(),
                workspace,
                credentials: credentials.as_ref().map(|c| &c.credential),
            })
            .await
            .map_err(|_| anyhow!("immutable workspace preparation requires reconciliation"))?;
            let request = SupervisedAdmissionV1 {
                version: 1,
                protocol: SupervisionProtocolV1::BrokerSupervisionV1,
                intent_hash: job.intent_hash().into(),
                admission: managed.admission.clone(),
            };
            self.stage(LocalFailurePhaseV1::Admission);
            self.executor.enforcer.require_admission_ready().await?;
            self.patch(|draft| {
                ensure!(
                    !draft.cancelled() && draft.run.plan.managed.is_none(),
                    "admission context changed"
                );
                self.generation_current()?;
                draft.run.plan.managed = Some(managed.clone());
                draft.operations.push(Operation::Admission {
                    request: request.clone(),
                    response: None,
                });
                Ok(())
            })
            .await?;
            self.rpc(Operation::Admission {
                request,
                response: None,
            })
            .await?;
            (_, draft) = self.load().await?;
        }
        if let Some(response) = draft.operations.iter().find_map(|op| match op {
            Operation::Admission {
                response: Some(result),
                ..
            } if !result.accepted => Some(result),
            _ => None,
        }) {
            response.refusal.context("admission refusal is missing")?;
            return Ok(BatchOutcome::AdmissionRefused);
        }
        self.generation_current()?;
        let checkpoint = draft
            .run
            .plan
            .managed
            .as_ref()
            .context("managed preparation is missing")?;
        let runtime = managed::broker_runtime(
            checkpoint,
            self.context.runtime(),
            draft.run.options().allow_rootful_runtime,
        )
        .await?;
        let binding = runtime.binding().clone();
        if checkpoint.binding.is_none() {
            self.patch(|draft| {
                self.generation_current()?;
                draft
                    .run
                    .plan
                    .managed
                    .as_mut()
                    .context("managed state disappeared")?
                    .binding = Some(binding.clone());
                Ok(())
            })
            .await?;
        }
        let (_, draft) = self.load().await?;
        let mut worker = draft.worker()?.clone();
        if worker.state == WorkerStateV1::Reserved {
            worker = self.mutate(WorkerTransitionV1::Prepare).await?;
        }
        if worker.state == WorkerStateV1::Preparing && worker.attempt.create_operation_id.is_none()
        {
            self.import(&runtime).await?;
            worker = self.mutate(WorkerTransitionV1::CreateIssued).await?;
        }
        if (worker.state == WorkerStateV1::Preparing
            && worker.attempt.create_operation_id.is_some())
            || matches!(
                worker.state,
                WorkerStateV1::Created
                    | WorkerStateV1::StartIssued
                    | WorkerStateV1::Running
                    | WorkerStateV1::InDoubt
                    | WorkerStateV1::Stopping
            )
        {
            self.execute_worker(&runtime).await?;
        }
        let (_, draft) = self.load().await?;
        let worker = draft.worker()?.clone();
        ensure!(
            worker.state == WorkerStateV1::Terminal && !worker.quota_held,
            "runtime terminal is not centrally acknowledged"
        );
        self.post_worker_result().await?;
        self.final_scan(&runtime).await?;
        Ok(BatchOutcome::Terminal)
    }

    async fn admission_receipt(&self) -> Result<MutationReceiptV1> {
        let (_, draft) = self.load().await?;
        draft
            .operations
            .iter()
            .find_map(|op| match op {
                Operation::Admission {
                    response: Some(response),
                    ..
                } if response.accepted => Some(response.receipt.clone()),
                _ => None,
            })
            .context("accepted admission receipt is missing")
    }
    async fn import(&self, runtime: &ManagedRuntime) -> Result<()> {
        let (_, draft) = self.load().await?;
        let worker = draft.worker()?.clone();
        // Repeating volume creation is safe only through the existing runtime's
        // exact control ownership labels. No external volume is adopted.
        self.generation_current()?;
        let namespace = worker
            .attempt
            .stable_name
            .strip_prefix("aig-")
            .and_then(|name| name.split('-').next())
            .context("worker namespace is missing")?;
        runtime
            .create_named_volume(
                &worker.attempt.volume_name,
                &[
                    (
                        "agentsandbox.authority".into(),
                        worker.launch.authority_id.clone(),
                    ),
                    ("agentsandbox.worker".into(), worker.worker_id.clone()),
                    ("agentsandbox.namespace".into(), namespace.into()),
                ],
            )
            .await?;
        let admission = self.admission_receipt().await?;
        let imported = self
            .helper(
                runtime,
                HelperPurposeV1::Import,
                HelperPermissionV1::Admission { receipt: admission },
                None,
            )
            .await?;
        let verification = HelperPermissionV1::ImportVerification {
            import_helper_id: imported.policy.helper_id.clone(),
            import_arm: imported.arm,
            import_terminal: imported.receipt,
            manifest_digest: worker.launch.workspace_manifest_digest.clone(),
        };
        self.helper(
            runtime,
            HelperPurposeV1::ImportVerification,
            verification,
            None,
        )
        .await?;
        Ok(())
    }

    async fn execute_worker(&self, runtime: &ManagedRuntime) -> Result<ContainerObservation> {
        self.stage(LocalFailurePhaseV1::Worker);
        let (job, draft) = self.load().await?;
        let worker = draft.worker()?.clone();
        let credentials = credentials_from_job(&job, self.context.config())?;
        if let Some(credentials) = &credentials {
            credentials.validate()?;
        }
        let spec = managed::worker_spec(
            &draft.run.plan,
            &worker,
            credentials.as_ref().map(|c| &c.credential),
        )?;
        let owned = draft
            .run
            .plan
            .managed
            .as_ref()
            .and_then(|m| m.owned.clone());
        let owned = match owned {
            Some(owned) => Some(owned),
            None => runtime.lookup_created(&spec).await?,
        };
        if let Some(owned) = &owned
            && draft.operations.iter().any(|op| {
                matches!(
                    op,
                    Operation::WorkerClaim {
                        response: Some(_),
                        ..
                    }
                )
            })
        {
            // The runtime explicitly refuses ObserveOnly in its start API.
            // Reconciliation therefore uses its read-only terminal observer.
            // Active/unknown remains an error with quota held until a later
            // observation; no timer or new claim is manufactured here.
            let arm = draft
                .operations
                .iter()
                .find_map(|op| match op {
                    Operation::Arm {
                        request,
                        response: Some(response),
                        ..
                    } if matches!(request.subject, ArmSubjectV1::Worker) => {
                        Some((request.clone(), response.receipt.clone()))
                    }
                    _ => None,
                })
                .context("historical worker arm is missing")?;
            self.executor
                .enforcer
                .restore(
                    RegistrationV1::new(
                        hex::encode(Sha256::digest(
                            self.context.scope().gateway_origin().as_bytes(),
                        )),
                        self.context.scope().tenant_id().into(),
                        arm.0,
                        arm.1,
                        runtime.binding().clone(),
                        owned.clone(),
                    )?,
                    self.context.cancel_generation().clone(),
                )
                .await?;
            let terminal = runtime.resume_terminal(&owned.handle).await?;
            self.save_observed_result(&terminal).await?;
            self.mutate(WorkerTransitionV1::Observe {
                observation: managed::observation_dto(&worker, &terminal)?,
            })
            .await?;
            self.post_worker_result().await?;
            return Ok(terminal);
        }
        let callback = |owned: OwnedContainerSnapshot| async move {
            self.patch(|draft| {
                let managed = draft
                    .run
                    .plan
                    .managed
                    .as_mut()
                    .context("managed state disappeared")?;
                if let Some(previous) = &managed.owned {
                    ensure!(
                        same(&previous.handle, &owned.handle)?,
                        "owned worker changed"
                    );
                    owned
                        .observation
                        .validate_identity_allocation(&previous.observation)?;
                } else {
                    managed.owned = Some(owned.clone());
                }
                Ok(())
            })
            .await?;
            let (_, draft) = self.load().await?;
            if draft.worker()?.attempt.created_inspection.is_none() {
                self.mutate(WorkerTransitionV1::Created {
                    inspection: managed::inspection(draft.worker()?, &owned.observation)?,
                })
                .await?;
            }
            let durable = draft
                .run
                .plan
                .managed
                .as_ref()
                .and_then(|m| m.owned.clone())
                .context("worker ownership was not persisted")?;
            self.arm_and_claim(runtime, durable, None).await
        };
        let started = |observation: ContainerObservation| async move {
            let (_, draft) = self.load().await?;
            self.mutate(WorkerTransitionV1::Observe {
                observation: managed::observation_dto(draft.worker()?, &observation)?,
            })
            .await?;
            Ok(())
        };
        let retained = if let Some(owned) = owned {
            // An owned instance is always reopened by immutable CID. A stored
            // claim is only observed; the callback cannot reconstruct its permit.
            runtime
                .start_owned_prearmed_retained(&spec, &owned, None, 1024 * 1024, callback, started)
                .await?
        } else {
            self.generation_current()?;
            runtime
                .execute_prearmed_retained(&spec, None, 1024 * 1024, callback, started)
                .await?
        };
        self.save_worker_result(&retained).await?;
        let (_, draft) = self.load().await?;
        self.mutate(WorkerTransitionV1::Observe {
            observation: managed::observation_dto(draft.worker()?, retained.terminal())?,
        })
        .await?;
        let (_, draft) = self.load().await?;
        ensure!(
            draft.worker()?.state == WorkerStateV1::Terminal && !draft.worker()?.quota_held,
            "terminal acknowledgement is uncertain"
        );
        self.post_worker_result().await?;
        // Enforcer registrations remain held. Cleanup is an independent fact;
        // no disarm or capacity release is inferred from this central terminal.
        let removed = runtime
            .cleanup_terminal(retained.handle(), retained.terminal())
            .await
            .is_ok();
        self.mutate(WorkerTransitionV1::CleanupResult {
            removed,
            error: (!removed).then(|| "runtime cleanup was not confirmed".into()),
        })
        .await?;
        ensure!(removed, "worker cleanup requires reconciliation");
        Ok(retained.terminal().clone())
    }
}
