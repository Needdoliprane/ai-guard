//! Execution of one held bootstrap with its broker-owned egress channel.

use super::{Execution, Operation, WorkerMutationV1, WorkerStateV1, WorkerTransitionV1};
use super::{boxed_execution_phase, managed, same};
#[cfg(test)]
use crate::broker::writer::WriterCallFailure;
use crate::{
    broker::{
        egress_owner::{
            channel::{
                self, ChannelStop, ChannelTermination, ChannelTimes, EgressControl,
                LocalQuiescence, OpenedChannel,
            },
            resolve::Resolver,
        },
        journal::RevisionRefV1,
    },
    control::MutationResultV1,
    egress::ids::IdV1,
    provider_activity::session::HookAttacher,
    runner::{
        egress_control::GatewayEgressControl,
        egress_release::{EgressReleaseIntentV1, EgressReleaseTargetV1},
    },
    runtime::{
        ContainerSpec, EgressExecIo, EgressExecOwner, ManagedPrearmedOutcome, ManagedRuntime,
        OwnedContainerSnapshot, PrearmedStartDirective, RelayExecState,
        bootstrap_stdin::PreparedBootstrapStdin,
    },
};
use anyhow::{Context, Result, anyhow, ensure};
use futures_util::future::BoxFuture;
#[cfg(test)]
use std::sync::OnceLock;
use std::{future::Future, sync::Arc, time::Duration};
#[cfg(test)]
use tokio::{
    sync::Notify,
    time::{Instant, sleep_until},
};
use tokio::{sync::watch, task::JoinHandle, time::timeout};
use uuid::Uuid;

#[cfg(test)]
#[derive(Default)]
pub(super) struct EgressOpeningProgress {
    started: OnceLock<Instant>,
    changed: Notify,
    pub(super) ready: OnceLock<Instant>,
    pub(super) running_http: OnceLock<(u128, u16)>,
    pub(super) running_decoded: OnceLock<Instant>,
    pub(super) running_ack: OnceLock<Instant>,
    pub(super) opening_key_ack: OnceLock<Instant>,
    pub(super) opening_attempt: OnceLock<Instant>,
    pub(super) opening_http_enter: OnceLock<Instant>,
    pub(super) opening_http: OnceLock<(u128, u16)>,
    pub(super) opening_receipt_held: OnceLock<Instant>,
    pub(super) release_intent_ack: OnceLock<Instant>,
    pub(super) release_submitted: OnceLock<Instant>,
    pub(super) close_release_stdin: Notify,
    pub(super) stop: OnceLock<Instant>,
    pub(super) callback_failed: OnceLock<Instant>,
    writer_failure: OnceLock<(Instant, EgressWitnessWriterFailure)>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(super) struct EgressWitnessWriterFailure {
    phase: &'static str,
    call: &'static str,
    cause: WriterCallFailure,
}

#[cfg(test)]
impl std::fmt::Display for EgressWitnessWriterFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cause = match self.cause {
            WriterCallFailure::QueueFull => "queue_full",
            WriterCallFailure::ReplyTimeout => "reply_timeout",
            WriterCallFailure::Unavailable => "unavailable",
            WriterCallFailure::Actor(_) => "actor_error",
        };
        write!(
            f,
            "writer phase={} call={} cause={cause}",
            self.phase, self.call
        )
    }
}

#[cfg(test)]
impl std::error::Error for EgressWitnessWriterFailure {}

#[cfg(test)]
impl EgressOpeningProgress {
    pub(super) fn start(&self) -> Instant {
        *self.started.get_or_init(Instant::now)
    }

    pub(super) fn elapsed_ms(&self) -> u128 {
        self.started.get_or_init(Instant::now).elapsed().as_millis()
    }

    pub(super) fn mark(&self, milestone: &OnceLock<Instant>) {
        if milestone.set(Instant::now()).is_ok() {
            // There is one oracle. A stored permit covers a mark before its next wait.
            self.changed.notify_one();
        }
    }

    pub(super) fn writer_failed(&self, call: &'static str, failure: &WriterCallFailure) {
        let phase = if self.ready.get().is_none() {
            "ready"
        } else if self.running_decoded.get().is_none() {
            "running_before_decode"
        } else if self.running_ack.get().is_none() {
            "running_ack"
        } else if self.opening_key_ack.get().is_none() {
            "opening_key_ack"
        } else if self.release_intent_ack.get().is_none() {
            "release_intent_ack"
        } else {
            return;
        };
        self.record_writer_failure(phase, call, *failure);
    }

    pub(super) fn record_writer_failure(
        &self,
        phase: &'static str,
        call: &'static str,
        cause: WriterCallFailure,
    ) -> EgressWitnessWriterFailure {
        let failure = EgressWitnessWriterFailure { phase, call, cause };
        if self.writer_failure.set((Instant::now(), failure)).is_ok() {
            self.changed.notify_one();
        }
        failure
    }

    pub(super) fn writer_failure_before(
        &self,
        deadline: Instant,
    ) -> Option<EgressWitnessWriterFailure> {
        self.writer_failure
            .get()
            .and_then(|&(at, failure)| (at < deadline).then_some(failure))
    }

    pub(super) async fn wait_for(
        &self,
        milestone: &OnceLock<Instant>,
        deadline: Instant,
    ) -> Result<Instant> {
        loop {
            // Sample expiry before reading the marks. A delayed poll can still
            // observe a mark published before the original deadline.
            let expired = Instant::now() >= deadline;
            if let Some(&at) = milestone.get()
                && at < deadline
            {
                return Ok(at);
            }
            if let Some(failure) = self.writer_failure_before(deadline) {
                return Err(failure.into());
            }
            if self.callback_failed.get().is_some_and(|&at| at < deadline) {
                anyhow::bail!("callback failed before this milestone was observed");
            }
            ensure!(!expired, "harness phase deadline elapsed");
            tokio::select! {
                () = self.changed.notified() => {},
                () = sleep_until(deadline) => {},
            }
        }
    }

    pub(super) async fn observe_release<T>(
        &self,
        deadline: Instant,
        observation: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        let mut observation = std::pin::pin!(observation);
        loop {
            if let Some(failure) = self.writer_failure_before(deadline) {
                return Err(failure.into());
            }
            tokio::select! {
                biased;
                result = observation.as_mut() => {
                    if let Some(failure) = self.writer_failure_before(deadline) {
                        return Err(failure.into());
                    }
                    return result;
                }
                () = sleep_until(deadline) => {
                    if let Some(failure) = self.writer_failure_before(deadline) {
                        return Err(failure.into());
                    }
                    anyhow::bail!("harness release observation deadline elapsed");
                }
                () = self.changed.notified() => {},
            }
        }
    }

    pub(super) fn report(&self) -> String {
        let started = self.start();
        let elapsed = |point: &OnceLock<Instant>| {
            point
                .get()
                .map(|at| at.saturating_duration_since(started).as_millis())
        };
        format!(
            "ready_ms={:?} running_http_ms_status={:?} running_decoded_ms={:?} running_ack_ms={:?} opening_key_ack_ms={:?} opening_attempt_ms={:?} opening_http_enter_ms={:?} opening_http_ms_status={:?} opening_receipt_held_ms={:?} release_intent_ack_ms={:?} stop_ms={:?} writer_failure_ms={:?}",
            elapsed(&self.ready),
            self.running_http.get(),
            elapsed(&self.running_decoded),
            elapsed(&self.running_ack),
            elapsed(&self.opening_key_ack),
            elapsed(&self.opening_attempt),
            elapsed(&self.opening_http_enter),
            self.opening_http.get(),
            elapsed(&self.opening_receipt_held),
            elapsed(&self.release_intent_ack),
            elapsed(&self.stop),
            self.writer_failure.get().map(|(at, failure)| (
                at.saturating_duration_since(started).as_millis(),
                failure.to_string()
            )),
        )
    }
}

#[cfg(test)]
tokio::task_local! {
    pub(super) static EGRESS_OPENING_PROGRESS: Arc<EgressOpeningProgress>;
}

// The channel owner bounds its local retirement and audit drain to five seconds.
const CHANNEL_JOIN_TIMEOUT: Duration = Duration::from_secs(6);

/// The exact local writer acknowledgement and the intent from its committed draft.
/// Persisted intent alone cannot reconstruct this in-process value.
struct CommittedEgressReleaseIntent {
    head: RevisionRefV1,
    intent: Box<EgressReleaseIntentV1>,
}

impl CommittedEgressReleaseIntent {
    fn head(&self) -> &RevisionRefV1 {
        &self.head
    }

    fn intent(&self) -> &EgressReleaseIntentV1 {
        &self.intent
    }
}

#[must_use = "retain the local egress owners through release and reconciliation"]
struct PreparedEgressRelease {
    bootstrap: Box<PreparedBootstrapStdin>,
    channel: OpenedChannel<EgressExecIo>,
    stop: ChannelStop,
    control: Arc<dyn EgressControl>,
    committed: CommittedEgressReleaseIntent,
}

/// The local owners stay with the worker phase across runtime success or error.
struct ActiveEgressChannel {
    task: JoinHandle<ChannelTermination>,
    stop: Arc<ChannelStop>,
}

impl ActiveEgressChannel {
    async fn close(mut self) -> bool {
        self.stop.stop();
        let channel = timeout(CHANNEL_JOIN_TIMEOUT, &mut self.task).await;
        matches!(channel, Ok(Ok(termination)) if termination.local_quiescence == LocalQuiescence::Confirmed)
    }
}

impl Drop for ActiveEgressChannel {
    fn drop(&mut self) {
        self.stop.stop();
        self.task.abort();
    }
}

struct RunningPlan {
    target: EgressReleaseTargetV1,
    request: WorkerMutationV1,
}

impl Execution<'_> {
    pub(super) async fn execute_held_egress<C, CFut>(
        &self,
        runtime: &ManagedRuntime,
        spec: &ContainerSpec,
        owned: Option<&OwnedContainerSnapshot>,
        on_created: C,
        hooks: Option<HookAttacher>,
    ) -> Result<(ManagedPrearmedOutcome, bool)>
    where
        C: FnOnce(OwnedContainerSnapshot) -> CFut,
        CFut: Future<Output = Result<PrearmedStartDirective>>,
    {
        let resolver = self
            .executor
            .egress_resolver
            .clone()
            .context("broker egress requires a retained operator-approved resolver")?;
        let mut channel = None;
        let mut exec = None;
        let active = &mut channel;
        let exec_owner = &mut exec;
        let on_ready: crate::runtime::bootstrap_stdin::BootstrapReady<'_> =
            Box::new(move |bootstrap| {
                boxed_execution_phase(|| async move {
                    let ready = self
                        .prepare_egress_release(runtime, spec, bootstrap, exec_owner)
                        .await?;
                    if let Some(hooks) = hooks {
                        hooks
                            .attach(runtime, ready.bootstrap.owned().clone(), spec)
                            .await;
                    }
                    let exec = exec_owner
                        .as_ref()
                        .context("missing retained egress helper after preparation")?;
                    *active = Some(
                        self.submit_prepared_egress_release(runtime, ready, exec, resolver)
                            .await?,
                    );
                    Ok(())
                })
            });
        let outcome = if let Some(owned) = owned {
            boxed_execution_phase(|| {
                runtime.start_owned_prearmed_bootstrap_retained(
                    spec,
                    owned,
                    1024 * 1024,
                    on_created,
                    on_ready,
                )
            })
            .await
        } else {
            self.generation_current()?;
            boxed_execution_phase(|| {
                runtime.execute_prearmed_bootstrap_retained(spec, 1024 * 1024, on_created, on_ready)
            })
            .await
        };
        let (channel_joined, exec_joined) = tokio::join!(
            async move {
                match channel {
                    Some(channel) => channel.close().await,
                    None => true,
                }
            },
            async move {
                match exec {
                    Some(mut exec) => exec.close_and_join().await.local_joined,
                    None => true,
                }
            }
        );
        Ok((outcome?, channel_joined && exec_joined))
    }

    /// Revalidate the committed intent and the live incarnation before the
    /// original native authorization arbitrates STOP and the one pipe write.
    /// The returned owners keep the helper and its channel open for supervision.
    fn submit_prepared_egress_release<'ready>(
        &'ready self,
        runtime: &'ready ManagedRuntime,
        ready: Box<PreparedEgressRelease>,
        exec: &'ready EgressExecOwner,
        resolver: Arc<Resolver>,
    ) -> BoxFuture<'ready, Result<ActiveEgressChannel>> {
        boxed_execution_phase(|| async move {
            self.require_egress_live(exec)?;
            let reinspected =
                boxed_execution_phase(|| runtime.observe(&ready.bootstrap.owned().handle))
                    .await
                    .context("cannot revalidate the held bootstrap incarnation")?;

            let head = boxed_execution_phase(|| {
                #[cfg(not(test))]
                {
                    self.executor.writer.fence_egress_release_head(
                        self.operation,
                        ready.committed.head(),
                        ready.committed.intent(),
                    )
                }
                #[cfg(test)]
                async {
                    self.executor
                        .writer
                        .fence_egress_release_head_diagnosed_for_test(
                            self.operation,
                            ready.committed.head(),
                            ready.committed.intent(),
                        )
                        .await
                        .map_err(|failure| {
                            let _ = EGRESS_OPENING_PROGRESS.try_with(|trace| {
                                trace.record_writer_failure("release_fence", "fence", failure)
                            });
                            failure.journal_error()
                        })
                }
            })
            .await
            .context("confirmed egress release intent is no longer current")?;

            let PreparedEgressRelease {
                bootstrap,
                channel,
                stop,
                control,
                ..
            } = *ready;
            // The native authorization supplies the original generation and
            // deadline. This veto has local checks only.
            let mut native_stop: watch::Receiver<bool> = bootstrap.stop_receiver();
            let submitted = bootstrap.submit_once(runtime.binding(), &reinspected, head, || {
                // The only channel-stop sender stays here through the write.
                // The native owner orders its independent STOP with submission.
                ensure!(!stop.is_stopped(), "retained egress channel has stopped");
                self.require_egress_live(exec)
            })?;
            #[cfg(test)]
            if let Ok(progress) = EGRESS_OPENING_PROGRESS.try_with(Arc::clone) {
                progress.mark(&progress.release_submitted);
                progress.close_release_stdin.notified().await;
            }
            // The batch release protocol requires EOF before the worker proceeds.
            let authorization = Arc::from(submitted.close_stdin());
            let stop = Arc::new(stop);
            let task_stop = Arc::clone(&stop);
            let task = tokio::spawn(async move {
                let mut run = boxed_execution_phase(|| {
                    channel::run_opened(
                        channel,
                        control,
                        resolver,
                        ChannelTimes::default(),
                        authorization,
                    )
                });
                tokio::select! {
                    biased;
                    () = async { let _ = native_stop.wait_for(|stopped| *stopped).await; } => {
                        task_stop.stop();
                        run.await
                    }
                    ended = &mut run => ended,
                }
            });
            Ok(ActiveEgressChannel { task, stop })
        })
    }

    /// Use only inside the runtime's supervised BootstrapReady callback.
    /// Consumes the one-shot stdin owner and returns it untouched on success.
    /// The journal retains the exact request for reconciliation of an ambiguous POST.
    fn prepare_egress_release<'ready>(
        &'ready self,
        runtime: &'ready ManagedRuntime,
        spec: &'ready ContainerSpec,
        bootstrap: PreparedBootstrapStdin,
        exec_owner: &'ready mut Option<EgressExecOwner>,
    ) -> BoxFuture<'ready, Result<Box<PreparedEgressRelease>>> {
        let bootstrap = Box::new(bootstrap);
        boxed_execution_phase(|| async move {
            ensure!(exec_owner.is_none(), "egress helper already retained");
            let plan =
                boxed_execution_phase(|| self.egress_running_plan(runtime, &bootstrap)).await?;
            // Only the first channel of this live bootstrap is prepared here.
            let binding = plan.target.binding(IdV1::random(), 1)?;

            self.generation_current()?;
            let (exec, io) =
                boxed_execution_phase(|| runtime.open_owned_egress_proxy(&bootstrap, spec))
                    .await
                    .context("retained egress helper exec unavailable")?;
            // The outer worker phase joins this exact exec even if STOP drops
            // this callback while a broker RPC or the handoff is in flight.
            let exec = exec_owner.insert(exec);
            self.require_egress_live(exec)?;
            let (stop, stop_receiver) = ChannelStop::new();
            let prepared = boxed_execution_phase(|| channel::prepare(io, binding, stop_receiver))
                .await
                .map_err(|failure| {
                    anyhow!("egress helper READY failed ({})", failure.cause.code())
                })?;
            #[cfg(test)]
            let _ = EGRESS_OPENING_PROGRESS.try_with(|trace| trace.mark(&trace.ready));

            self.require_egress_live(exec)?;
            let running = boxed_execution_phase(|| self.acknowledge_egress_running(&plan.request))
                .await
                .context("retained egress Running acknowledgement unavailable")?;
            self.require_egress_live(exec)?;

            boxed_execution_phase(|| {
                self.record_egress_opening(&plan.target, prepared.opening_request(), &running)
            })
            .await
            .context("retained egress opening request acknowledgement unavailable")?;
            #[cfg(test)]
            let _ = EGRESS_OPENING_PROGRESS.try_with(|trace| trace.mark(&trace.opening_key_ack));
            self.require_egress_live(exec)?;

            let control: Arc<dyn EgressControl> =
                Arc::new(GatewayEgressControl::new(self.client.clone()));
            #[cfg(test)]
            let _ = EGRESS_OPENING_PROGRESS.try_with(|trace| trace.mark(&trace.opening_attempt));
            let channel = boxed_execution_phase(|| prepared.open(control.as_ref()))
                .await
                .map_err(|failure| {
                    anyhow!("retained egress opening failed ({})", failure.cause.code())
                })?;

            self.require_egress_live(exec)?;
            let mut committed = boxed_execution_phase(|| {
                self.record_egress_release_intent(plan.target, &channel, &running)
            })
            .await
            .context("retained egress intent acknowledgement unavailable")?;
            #[cfg(test)]
            let _ = EGRESS_OPENING_PROGRESS.try_with(|trace| trace.mark(&trace.release_intent_ack));
            self.require_egress_live(exec)?;
            committed.require_context(self.operation, self.context.scope())?;
            let intent = committed
                .draft
                .egress_release
                .take()
                .context("acknowledged checkpoint has no egress release intent")?;
            Ok(Box::new(PreparedEgressRelease {
                bootstrap,
                channel,
                stop,
                control,
                committed: CommittedEgressReleaseIntent {
                    head: committed.head.clone(),
                    intent,
                },
            }))
        })
    }

    async fn egress_running_plan(
        &self,
        runtime: &ManagedRuntime,
        bootstrap: &PreparedBootstrapStdin,
    ) -> Result<Box<RunningPlan>> {
        self.generation_current()?;
        let (_, draft) = self.load().await?;
        let worker = draft.worker()?;
        ensure!(
            !draft.cancelled()
                && draft.egress_opening.is_none()
                && draft.egress_release.is_none()
                && worker.state == WorkerStateV1::StartIssued
                && worker.quota_held
                && !worker.allocation_inconsistent
                && bootstrap.attempt().as_str() == worker.attempt.attempt_id,
            "journal does not retain an eligible held bootstrap attempt",
        );
        let created = draft
            .run
            .plan
            .managed
            .as_ref()
            .and_then(|managed| managed.owned.as_ref())
            .context("journal lacks the retained Created snapshot")?;
        let observation = &bootstrap.owned().observation;
        let target = EgressReleaseTargetV1::from_observation(
            self.context.scope().tenant_id(),
            self.generation,
            worker,
            runtime.binding(),
            created,
            observation,
        )?;
        Ok(Box::new(RunningPlan {
            target,
            request: WorkerMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                worker_id: worker.worker_id.clone(),
                attempt_id: worker.attempt.attempt_id.clone(),
                expected_revision: worker.revision,
                transition: WorkerTransitionV1::Observe {
                    observation: managed::observation_dto(worker, observation)?,
                },
            },
        }))
    }

    async fn acknowledge_egress_running(
        &self,
        request: &WorkerMutationV1,
    ) -> Result<Box<MutationResultV1>> {
        let WorkerTransitionV1::Observe { observation } = &request.transition else {
            anyhow::bail!("retained egress requires its Running observation");
        };
        let (ack, fresh, committed) = self
            .rpc_checked_at_head(
                Operation::WorkerMutation {
                    request: request.clone(),
                    response: None,
                },
                None,
            )
            .await?;
        let Operation::WorkerMutation {
            response: Some(running),
            ..
        } = ack
        else {
            anyhow::bail!("retained egress worker mutation lacks an acknowledgement");
        };
        committed.require_context(self.operation, self.context.scope())?;
        ensure!(
            fresh
                && !committed.draft.cancelled()
                && committed.draft.egress_opening.is_none()
                && committed.draft.egress_release.is_none()
                && running.worker.state == WorkerStateV1::Running
                && running.worker.quota_held
                && !running.worker.allocation_inconsistent
                && running.worker.last_observation.as_ref() == Some(observation)
                && same(committed.draft.worker()?, &running.worker)?,
            "retained egress Running acknowledgement differs from its observation or journal head",
        );
        #[cfg(test)]
        let _ = EGRESS_OPENING_PROGRESS.try_with(|trace| trace.mark(&trace.running_ack));
        Ok(Box::new(running))
    }

    fn require_egress_live(&self, exec: &EgressExecOwner) -> Result<()> {
        self.generation_current()?;
        ensure!(
            exec.state() == RelayExecState::Open,
            "retained egress helper pipes closed"
        );
        Ok(())
    }
}
