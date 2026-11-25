//! Execution of one held bootstrap with its broker-owned egress channel.

use super::{Execution, Operation, WorkerMutationV1, WorkerStateV1, WorkerTransitionV1};
use super::{boxed_execution_phase, managed, same};
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
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle, time::timeout};
use uuid::Uuid;

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
                self.executor.writer.fence_egress_release_head(
                    self.operation,
                    ready.committed.head(),
                    ready.committed.intent(),
                )
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
            bootstrap.submit_once(runtime.binding(), &reinspected, head, || {
                // The only channel-stop sender stays here through the write.
                // The native owner orders its independent STOP with submission.
                ensure!(!stop.is_stopped(), "retained egress channel has stopped");
                self.require_egress_live(exec)
            })?;
            let stop = Arc::new(stop);
            let task_stop = Arc::clone(&stop);
            let task = tokio::spawn(async move {
                let mut run = boxed_execution_phase(|| {
                    channel::run_opened(channel, control, resolver, ChannelTimes::default())
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
            self.require_egress_live(exec)?;

            let control: Arc<dyn EgressControl> =
                Arc::new(GatewayEgressControl::new(self.client.clone()));
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
