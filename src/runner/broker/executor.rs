//! Batch execution from a durable intention. Journal I/O is delegated to the
//! bounded writer; the enforcer owns stop independently of that writer.
macro_rules! continue_step {
    ($step:expr) => {
        match $step {
            ControlFlow::Continue(value) => value,
            ControlFlow::Break(abandoned) => return Ok(ControlFlow::Break(abandoned)),
        }
    };
}

mod cancelled;
mod diagnostics;
mod helpers;
mod journal;
mod results;
mod retirement;
#[cfg(all(test, unix))]
mod retirement_live_tests;
use super::{BrokerRunState, FreshStartPermission, credentials_from_job};
use crate::{
    broker::{
        checkpoint::{
            BrokerCheckpointV2, DecisionAnswerV1, HelperPreparationV1, SupervisionCheckpointV1,
            SupervisionOperationV1 as Operation,
        },
        clock::{HostClock, checked_deadline},
        enforcer::{CancelGeneration, EnforcedSlot, ExecutionEnforcerV1, RegistrationV1},
        journal::{AcceptedJobV1, CancelStopProjectionV1, JournalError, JournalScopeV1},
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
use futures_util::future::BoxFuture;
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
pub(crate) use journal::{ColdRecoverySeed, terminal_pending_seed_for_cold_audit_test};
use journal::{operation_key, same};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    ops::ControlFlow,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;

const CANCEL_FANOUT: usize = 16;
const CANCEL_TOTAL_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone)]
pub(crate) struct BatchExecutor {
    writer: WriterClient,
    enforcer: ExecutionEnforcerV1,
    clock: Arc<dyn HostClock>,
    relays: Option<Arc<crate::broker::relay_owner::RelayRegistry>>,
}
impl fmt::Debug for BatchExecutor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BatchExecutor(<private>)")
    }
}

/// Disposition of this invocation. Detailed results are already persisted in
/// the journal and audit; the service reads those facts for status responses.
pub(crate) enum BatchOutcome {
    AbandonedBeforeClaim(Box<AbandonedBatch>),
    AwaitingReview,
    Denied,
    AdmissionRefused,
    Cancelled,
    Terminal,
}

pub(crate) struct AbandonedBatch {
    pub(crate) owned: OwnedContainerSnapshot,
    pub(crate) retirement: crate::runtime::AbandonedBeforeClaim,
}

pub(crate) type BatchStep<T> = ControlFlow<Box<AbandonedBatch>, T>;

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
    fn checkpoint(&self, namespace: Uuid) -> Result<BrokerCheckpointV3> {
        let runner = CheckpointPayloadV1(CheckpointDataV1 {
            version: 1,
            current_request: self.run.plan.request.clone(),
            managed: self.run.plan.managed.clone(),
        });
        Ok(BrokerCheckpointV3::new(
            BrokerCheckpointV2::new(
                runner,
                SupervisionCheckpointV1::new(
                    namespace,
                    self.helpers.clone(),
                    self.operations.clone(),
                )?,
            )?,
            self.commands.clone(),
        )?)
    }
}

/// Confirmed runner data at this exact head, never a latest-state or start
/// capability. V4 outbox data is deliberately absent from Draft.
struct CommittedDraftAtHead {
    operation: Uuid,
    scope: JournalScopeV1,
    head: crate::broker::journal::RevisionRefV1,
    draft: Draft,
}
impl CommittedDraftAtHead {
    fn require_context(&self, operation: Uuid, scope: &JournalScopeV1) -> Result<()> {
        ensure!(
            self.operation == operation
                && self.draft.run.operation_id() == operation
                && same(&self.scope, scope)?,
            "confirmed draft context changed"
        );
        Ok(())
    }
    fn into_parts(
        self: Box<Self>,
        operation: Uuid,
        scope: &JournalScopeV1,
    ) -> Result<(crate::broker::journal::RevisionRefV1, Draft)> {
        self.require_context(operation, scope)?;
        let Self { head, draft, .. } = *self;
        Ok((head, draft))
    }
}
impl fmt::Debug for CommittedDraftAtHead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CommittedDraftAtHead(<private confirmed data>)")
    }
}

struct Execution<'a> {
    executor: &'a BatchExecutor,
    context: &'a AuthenticatedBatchContext,
    operation: Uuid,
    client: AuditClient,
    generation: u64,
    phase: AtomicU8,
    launch_profile: Option<crate::runtime::launch_profile::RetainedDockerLaunchProfile>,
}

// These owned snapshots retain the original inputs across sequential phases.
// They are data, never a replacement for the checked loads in each phase.
struct PreparedRunPhase {
    job: Box<AcceptedJobV1>,
    draft: Box<Draft>,
}

struct ActiveRuntimePhase {
    runtime: ManagedRuntime,
    _initial_draft: Box<Draft>,
}

struct ActiveWorkerPhase {
    runtime: Box<ActiveRuntimePhase>,
    _draft: Box<Draft>,
    worker: Box<WorkerV1>,
}

// Construct the concrete phase future inside a separate call boundary before
// boxing it. Boxing an already constructed aggregate can leave its temporary
// storage in the caller's debug poll frame. This invokes construct immediately;
// it does not poll, spawn, defer argument evaluation or change cancellation.
#[inline(never)]
fn boxed_execution_phase<F: std::future::Future>(
    construct: impl FnOnce() -> F,
) -> std::pin::Pin<Box<F>> {
    Box::pin(construct())
}

impl BatchExecutor {
    pub(crate) fn new(
        writer: WriterClient,
        enforcer: impl Into<ExecutionEnforcerV1>,
        clock: Arc<dyn HostClock>,
    ) -> Self {
        Self {
            writer,
            enforcer: enforcer.into(),
            clock,
            relays: None,
        }
    }
    pub(crate) fn with_relays(
        mut self,
        relays: Option<Arc<crate::broker::relay_owner::RelayRegistry>>,
    ) -> Self {
        self.relays = relays;
        self
    }
    pub(crate) async fn run(
        &self,
        operation: Uuid,
        context: &AuthenticatedBatchContext,
    ) -> std::result::Result<BatchOutcome, BatchFailure> {
        let execution = Execution::open(self, operation, context)
            .await
            .map_err(|_| BatchFailure)?;
        match boxed_execution_phase(|| execution.run()).await {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                execution.failure(execution.phase(), &error).await;
                Err(BatchFailure)
            }
        }
    }
    pub(crate) async fn export(
        &self,
        operation: Uuid,
        target: ValidatedExportTarget,
        context: &AuthenticatedBatchContext,
    ) -> std::result::Result<BatchStep<ExportCompletion>, BatchFailure> {
        let execution = Execution::open(self, operation, context)
            .await
            .map_err(|_| BatchFailure)?;
        match execution.export(target).await {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                execution.failure(LocalFailurePhaseV1::Export, &error).await;
                Err(BatchFailure)
            }
        }
    }
}

/// Stop-only delivery independent of a writer, execution context or host clock.
/// The caller has authenticated the expected scope; the projection proves the
/// immutable Cancel prefix. Known targets are attempted before partial coverage
/// is reported, without changing a job generation, deadline or start permission.
pub(crate) async fn deliver_cancellation(
    enforcer: &ExecutionEnforcerV1,
    expected_scope: &JournalScopeV1,
    projection: CancelStopProjectionV1,
    cancel: &CancelGeneration,
) -> Result<()> {
    ensure!(
        same(projection.scope(), expected_scope)?,
        "cancellation scope mismatch"
    );
    ensure!(
        !projection.operation_id().is_nil()
            && !projection.command_id().is_nil()
            && projection.receipt().command_id() == projection.command_id(),
        "invalid durable cancellation projection"
    );
    let origin = hex::encode(Sha256::digest(
        projection.scope().gateway_origin().as_bytes(),
    ));
    let expires = tokio::time::Instant::now() + CANCEL_TOTAL_TIMEOUT;
    let batches = projection.stops().len().div_ceil(CANCEL_FANOUT);
    let mut failed = false;
    for (index, stops) in projection.stops().chunks(CANCEL_FANOUT).enumerate() {
        // Divide the remaining total deadline fairly between batches. An
        // unavailable early target cannot consume every later attempt.
        let budget = expires.saturating_duration_since(tokio::time::Instant::now())
            / u32::try_from(batches - index).context("cancellation inventory exceeds its bound")?;
        let mut attempts = Vec::with_capacity(stops.len());
        for stop in stops {
            let registration = RegistrationV1::new(
                origin.clone(),
                projection.scope().tenant_id().into(),
                stop.arm().clone(),
                stop.receipt().clone(),
                stop.authority().clone(),
                stop.owned().clone(),
            );
            let registration = match registration {
                Ok(registration) => registration,
                Err(_) => {
                    failed = true;
                    continue;
                }
            };
            let binding = stop.full3_registration_binding().cloned();
            let enforcer = enforcer.clone();
            let cancel = cancel.clone();
            attempts.push(async move {
                tokio::time::timeout(budget, async {
                    match (enforcer.full3(), binding.as_ref()) {
                        (Some(_), Some(binding)) => {
                            enforcer
                                .cancel_retained_bound(registration, cancel, binding)
                                .await
                        }
                        (Some(_), None) => {
                            Err(crate::broker::enforcer::EnforcerError::Unauthorized)
                        }
                        (None, Some(_)) => {
                            Err(crate::broker::enforcer::EnforcerError::Unauthorized)
                        }
                        (None, None) => enforcer.cancel_retained(registration, cancel).await,
                    }
                })
                .await
            });
        }
        let outcomes = futures_util::future::join_all(attempts).await;
        failed |= outcomes.iter().any(|result| !matches!(result, Ok(Ok(()))));
    }
    ensure!(
        !failed && !projection.incomplete(),
        "cancellation delivery is incomplete; retained targets require reconciliation"
    );
    Ok(())
}

impl<'a> Execution<'a> {
    // Bound inline composition of heavy phase futures: a debug poll reserves
    // its frame before the cancellation branch can return early.
    async fn run(&self) -> Result<BatchOutcome> {
        self.stage(LocalFailurePhaseV1::Decision);
        let prepared = self.prepare_run_phase().await?;
        if prepared.draft.cancelled() {
            return self.cancelled_run_phase(prepared).await;
        }
        let PreparedRunPhase { job, draft } = *prepared;
        boxed_execution_phase(|| self.run_active(job, draft)).await
    }

    fn prepare_run_phase(&self) -> BoxFuture<'_, Result<Box<PreparedRunPhase>>> {
        Box::pin(async move {
            self.replay_events().await?;
            self.reconcile_pending().await?;
            let (job, draft) = self.load().await?;
            Ok(Box::new(PreparedRunPhase {
                job: Box::new(job),
                draft: Box::new(draft),
            }))
        })
    }

    fn cancelled_run_phase(
        &self,
        prepared: Box<PreparedRunPhase>,
    ) -> BoxFuture<'_, Result<BatchOutcome>> {
        Box::pin(async move {
            let draft = &prepared.draft;
            let worker = draft
                .run
                .plan
                .managed
                .as_ref()
                .and_then(|m| m.worker.clone());
            match worker.as_ref().map(|worker| worker.state) {
                Some(WorkerStateV1::Reserved) => {
                    self.mutate(WorkerTransitionV1::CancelReserved).await?;
                }
                Some(WorkerStateV1::Running | WorkerStateV1::Stopping) => {
                    Box::pin(self.reconcile_cancelled_worker()).await?;
                }
                Some(WorkerStateV1::Terminal) => {
                    if draft.run.plan.managed.as_ref().is_some_and(|managed| {
                        managed.output_event.is_some() && !managed.output_acknowledged
                    }) {
                        self.stage(LocalFailurePhaseV1::Worker);
                        Box::pin(self.post_worker_result()).await?;
                    }
                }
                None | Some(WorkerStateV1::Cancelled) => {}
                Some(_) => return Err(anyhow!("cancelled worker requires reconciliation")),
            }
            if let Some(relays) = &self.executor.relays {
                let (current, latest) = self.load().await?;
                if latest.worker().is_ok_and(|worker| {
                    worker.state == WorkerStateV1::Terminal && !worker.quota_held
                }) {
                    relays
                        .retire(self.context, current, self.executor.clock.clone())
                        .await?;
                }
            }
            Ok(BatchOutcome::Cancelled)
        })
    }

    async fn run_active(
        &self,
        job: Box<AcceptedJobV1>,
        mut draft: Box<Draft>,
    ) -> Result<BatchOutcome> {
        self.generation_current()?;
        if draft.run.plan.managed.is_none() {
            let answer = match boxed_execution_phase(|| self.decide_active(&draft)).await? {
                ControlFlow::Break(outcome) => return Ok(outcome),
                ControlFlow::Continue(answer) => answer,
            };
            draft = boxed_execution_phase(|| self.prepare_active(&job, answer)).await?;
        }
        let outcome = boxed_execution_phase(|| self.execute_active(draft)).await;
        // Keep the accepted job and its retained files through every phase,
        // just as the original single run_active body did.
        drop(job);
        outcome
    }

    // Each sequential phase has its own heap-owned future and poll frame. The
    // orchestrator retains the existing owner; these methods spawn no work.
    async fn decide_active(
        &self,
        draft: &Draft,
    ) -> Result<ControlFlow<BatchOutcome, DecisionAnswerV1>> {
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
            let approval = current
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
                return Ok(ControlFlow::Break(BatchOutcome::AwaitingReview));
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
            Outcome::Deny => return Ok(ControlFlow::Break(BatchOutcome::Denied)),
            Outcome::Review => {
                return Ok(ControlFlow::Break(BatchOutcome::AwaitingReview));
            }
            Outcome::Allow => {}
        }
        Ok(ControlFlow::Continue(answer))
    }

    async fn prepare_active(
        &self,
        job: &AcceptedJobV1,
        answer: DecisionAnswerV1,
    ) -> Result<Box<Draft>> {
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
        let managed = boxed_execution_phase(|| {
            managed::broker_prepare(managed::BrokerPreparation {
                plan: &current.run.plan,
                scope: self.context.scope(),
                binding: current_job.binding_manifest(),
                answer: &answer,
                runtime: self.context.runtime(),
                workspace,
                credentials: credentials.as_ref().map(|c| &c.credential),
                launch_profile: self.launch_profile.as_ref(),
            })
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
        let (_, draft) = self.load().await?;
        Ok(Box::new(draft))
    }

    async fn execute_active(&self, draft: Box<Draft>) -> Result<BatchOutcome> {
        let runtime = match self.active_runtime_phase(draft).await? {
            ControlFlow::Break(outcome) => return Ok(outcome),
            ControlFlow::Continue(runtime) => runtime,
        };
        let prepared = match self.active_import_phase(runtime).await? {
            ControlFlow::Break(abandoned) => {
                return Ok(BatchOutcome::AbandonedBeforeClaim(abandoned));
            }
            ControlFlow::Continue(prepared) => prepared,
        };
        let prepared = match self.active_worker_phase(prepared).await? {
            ControlFlow::Break(abandoned) => {
                return Ok(BatchOutcome::AbandonedBeforeClaim(abandoned));
            }
            ControlFlow::Continue(prepared) => prepared,
        };
        self.active_terminal_phase(prepared).await
    }

    fn active_runtime_phase(
        &self,
        draft: Box<Draft>,
    ) -> BoxFuture<'_, Result<ControlFlow<BatchOutcome, Box<ActiveRuntimePhase>>>> {
        Box::pin(async move {
            if let Some(response) = draft.operations.iter().find_map(|op| match op {
                Operation::Admission {
                    response: Some(result),
                    ..
                } if !result.accepted => Some(result),
                _ => None,
            }) {
                response.refusal.context("admission refusal is missing")?;
                return Ok(ControlFlow::Break(BatchOutcome::AdmissionRefused));
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
                self.launch_profile.as_ref(),
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
            Ok(ControlFlow::Continue(Box::new(ActiveRuntimePhase {
                runtime,
                _initial_draft: draft,
            })))
        })
    }

    fn active_import_phase(
        &self,
        runtime: Box<ActiveRuntimePhase>,
    ) -> BoxFuture<'_, Result<BatchStep<Box<ActiveWorkerPhase>>>> {
        Box::pin(async move {
            let (_, draft) = self.load().await?;
            let mut worker = draft.worker()?.clone();
            if worker.state == WorkerStateV1::Reserved {
                worker = self.mutate(WorkerTransitionV1::Prepare).await?;
            }
            if worker.state == WorkerStateV1::Preparing
                && worker.attempt.create_operation_id.is_none()
            {
                if let ControlFlow::Break(abandoned) =
                    boxed_execution_phase(|| self.import(&runtime.runtime)).await?
                {
                    return Ok(ControlFlow::Break(abandoned));
                }
                worker = self.mutate(WorkerTransitionV1::CreateIssued).await?;
            }
            Ok(ControlFlow::Continue(Box::new(ActiveWorkerPhase {
                runtime,
                _draft: Box::new(draft),
                worker: Box::new(worker),
            })))
        })
    }

    fn active_worker_phase(
        &self,
        prepared: Box<ActiveWorkerPhase>,
    ) -> BoxFuture<'_, Result<BatchStep<Box<ActiveWorkerPhase>>>> {
        Box::pin(async move {
            let worker = &prepared.worker;
            if ((worker.state == WorkerStateV1::Preparing
                && worker.attempt.create_operation_id.is_some())
                || matches!(
                    worker.state,
                    WorkerStateV1::Created
                        | WorkerStateV1::StartIssued
                        | WorkerStateV1::Running
                        | WorkerStateV1::InDoubt
                        | WorkerStateV1::Stopping
                ))
                && let ControlFlow::Break(abandoned) =
                    boxed_execution_phase(|| self.execute_worker(&prepared.runtime.runtime)).await?
            {
                return Ok(ControlFlow::Break(abandoned));
            }
            Ok(ControlFlow::Continue(prepared))
        })
    }

    fn active_terminal_phase(
        &self,
        prepared: Box<ActiveWorkerPhase>,
    ) -> BoxFuture<'_, Result<BatchOutcome>> {
        Box::pin(async move {
            let (_, draft) = self.load().await?;
            let worker = draft.worker()?.clone();
            ensure!(
                worker.state == WorkerStateV1::Terminal && !worker.quota_held,
                "runtime terminal is not centrally acknowledged"
            );
            self.post_worker_result().await?;
            if let Some(relays) = &self.executor.relays {
                let (current, _) = self.load().await?;
                relays
                    .retire(self.context, current, self.executor.clock.clone())
                    .await?;
            }
            if let ControlFlow::Break(abandoned) =
                boxed_execution_phase(|| self.final_scan(&prepared.runtime.runtime)).await?
            {
                return Ok(BatchOutcome::AbandonedBeforeClaim(abandoned));
            }
            Ok(BatchOutcome::Terminal)
        })
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
    async fn import(&self, runtime: &ManagedRuntime) -> Result<BatchStep<()>> {
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
        let imported = continue_step!(
            self.helper(
                runtime,
                HelperPurposeV1::Import,
                HelperPermissionV1::Admission { receipt: admission },
                None,
            )
            .await?
        );
        let verification = HelperPermissionV1::ImportVerification {
            import_helper_id: imported.policy.helper_id.clone(),
            import_arm: imported.arm,
            import_terminal: imported.receipt,
            manifest_digest: worker.launch.workspace_manifest_digest.clone(),
        };
        continue_step!(
            self.helper(
                runtime,
                HelperPurposeV1::ImportVerification,
                verification,
                None,
            )
            .await?
        );
        Ok(ControlFlow::Continue(()))
    }

    async fn execute_worker(
        &self,
        runtime: &ManagedRuntime,
    ) -> Result<BatchStep<ContainerObservation>> {
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
            self.restore_assigned_slot(RegistrationV1::new(
                hex::encode(Sha256::digest(
                    self.context.scope().gateway_origin().as_bytes(),
                )),
                self.context.scope().tenant_id().into(),
                arm.0,
                arm.1,
                runtime.binding().clone(),
                owned.clone(),
            )?)
            .await?;
            if let Some(relays) = &self.executor.relays {
                let (current, _) = self.load().await?;
                relays
                    .started(
                        self.context,
                        current,
                        runtime.clone(),
                        spec.clone(),
                        self.executor.clock.clone(),
                    )
                    .await?;
            }
            let terminal = runtime.resume_terminal(&owned.handle).await?;
            self.save_observed_result(&terminal).await?;
            self.mutate(WorkerTransitionV1::Observe {
                observation: managed::observation_dto(&worker, &terminal)?,
            })
            .await?;
            self.post_worker_result().await?;
            if let Some(relays) = &self.executor.relays {
                let (current, _) = self.load().await?;
                relays
                    .retire(self.context, current, self.executor.clock.clone())
                    .await?;
            }
            return Ok(ControlFlow::Continue(terminal));
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
            .await
            .context(diagnostics::BrokerStep::OwnershipCommit)?;
            let (_, draft) = self
                .load()
                .await
                .context(diagnostics::BrokerStep::OwnershipReload)?;
            if draft.worker()?.attempt.created_inspection.is_none() {
                self.mutate(WorkerTransitionV1::Created {
                    inspection: managed::inspection(draft.worker()?, &owned.observation)
                        .context(diagnostics::BrokerStep::CreatedInspection)?,
                })
                .await
                .context(diagnostics::BrokerStep::CreatedExchange)?;
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
        let relay_spec = spec.clone();
        let started = |observation: ContainerObservation| {
            let relay_spec = relay_spec.clone();
            async move {
                let (_, draft) = self.load().await?;
                self.mutate(WorkerTransitionV1::Observe {
                    observation: managed::observation_dto(draft.worker()?, &observation)?,
                })
                .await?;
                if let Some(relays) = &self.executor.relays {
                    let (current, _) = self.load().await?;
                    relays
                        .started(
                            self.context,
                            current,
                            runtime.clone(),
                            relay_spec,
                            self.executor.clock.clone(),
                        )
                        .await?;
                }
                Ok(())
            }
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
        let retained = match retained {
            crate::runtime::ManagedPrearmedOutcome::Executed(retained) => retained,
            crate::runtime::ManagedPrearmedOutcome::Abandoned { owned, retirement } => {
                return Ok(ControlFlow::Break(Box::new(AbandonedBatch {
                    owned,
                    retirement,
                })));
            }
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
        let relay_joined = if let Some(relays) = &self.executor.relays {
            let (current, _) = self.load().await?;
            relays.worker_terminal(&current).await.is_ok()
        } else {
            true
        };
        let removed = relay_joined
            && runtime
                .cleanup_terminal(retained.handle(), retained.terminal())
                .await
                .is_ok();
        self.mutate(WorkerTransitionV1::CleanupResult {
            removed,
            error: (!removed).then(|| "runtime cleanup was not confirmed".into()),
        })
        .await?;
        ensure!(removed, "worker cleanup requires reconciliation");
        Ok(ControlFlow::Continue(retained.terminal().clone()))
    }
}
