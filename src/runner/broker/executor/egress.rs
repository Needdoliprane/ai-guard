//! Preparation of one held bootstrap. The next composition lot keeps the
//! returned owners through release and supervision.
#![allow(dead_code)] // The profile remains closed until the callback is connected.

use super::{Execution, Operation, WorkerMutationV1, WorkerStateV1, WorkerTransitionV1};
use super::{boxed_execution_phase, managed, same};
use crate::{
    broker::{
        egress_owner::channel::{
            self, ChannelAuditClosure, ChannelStop, EgressControl, OpenedChannel,
        },
        journal::RevisionRefV1,
    },
    control::{MutationResultV1, egress_decision::EgressChannelOpeningRequestV1},
    egress::ids::IdV1,
    runner::{
        egress_control::GatewayEgressControl,
        egress_release::{EgressReleaseIntentV1, EgressReleaseTargetV1},
    },
    runtime::{
        ContainerSpec, EgressExecIo, EgressExecOwner, ManagedRuntime, RelayExecState,
        bootstrap_stdin::PreparedBootstrapStdin,
    },
};
use anyhow::{Context, Result, anyhow, ensure};
use futures_util::future::BoxFuture;
use std::sync::Arc;
use uuid::Uuid;

/// Survives a dropped preparation future when retained outside that future.
/// The request identifies possible historical audit even if its reply was lost.
pub(super) struct EgressOpeningEvidence {
    pub(super) request: EgressChannelOpeningRequestV1,
    pub(super) audit: ChannelAuditClosure,
}

/// The exact local writer acknowledgement and the intent from its committed draft.
/// Persisted intent alone cannot reconstruct this in-process value.
pub(super) struct CommittedEgressReleaseIntent {
    head: RevisionRefV1,
    intent: Box<EgressReleaseIntentV1>,
}

impl CommittedEgressReleaseIntent {
    pub(super) fn head(&self) -> &RevisionRefV1 {
        &self.head
    }

    pub(super) fn intent(&self) -> &EgressReleaseIntentV1 {
        &self.intent
    }
}

#[must_use = "retain the local egress owners through release and reconciliation"]
pub(super) struct PreparedEgressRelease {
    pub(super) bootstrap: Box<PreparedBootstrapStdin>,
    pub(super) channel: OpenedChannel<EgressExecIo>,
    pub(super) exec: EgressExecOwner,
    pub(super) stop: ChannelStop,
    pub(super) control: Arc<dyn EgressControl>,
    pub(super) committed: CommittedEgressReleaseIntent,
}

#[must_use = "supervise the opened channel through worker stop and reconciliation"]
pub(super) struct ReleasedEgressChannel {
    pub(super) channel: OpenedChannel<EgressExecIo>,
    pub(super) exec: EgressExecOwner,
    pub(super) stop: ChannelStop,
    pub(super) control: Arc<dyn EgressControl>,
}

struct RunningPlan {
    target: EgressReleaseTargetV1,
    request: WorkerMutationV1,
}

impl Execution<'_> {
    /// Revalidate the committed intent and the live incarnation before the
    /// original native authorization arbitrates STOP and the one pipe write.
    /// The returned owners keep the helper and its channel open for supervision.
    pub(super) fn submit_prepared_egress_release<'ready>(
        &'ready self,
        runtime: &'ready ManagedRuntime,
        ready: Box<PreparedEgressRelease>,
    ) -> BoxFuture<'ready, Result<Box<ReleasedEgressChannel>>> {
        boxed_execution_phase(|| async move {
            self.require_egress_live(&ready.exec)?;
            let (job, draft) = boxed_execution_phase(|| self.load()).await?;
            ensure!(
                same(job.head(), ready.committed.head())?
                    && !draft.cancelled()
                    && draft.egress_release.as_deref() == Some(ready.committed.intent())
                    && ready.committed.intent().matches_running(draft.worker()?),
                "confirmed egress release intent is no longer current"
            );
            drop(draft);
            drop(job);

            let reinspected =
                boxed_execution_phase(|| runtime.observe(&ready.bootstrap.owned().handle))
                    .await
                    .context("cannot revalidate the held bootstrap incarnation")?;

            let PreparedEgressRelease {
                bootstrap,
                channel,
                exec,
                stop,
                control,
                ..
            } = *ready;
            // The native authorization supplies the original generation and
            // deadline. This veto has local checks only.
            bootstrap.submit_once(runtime.binding(), &reinspected, || {
                self.require_egress_live(&exec)
            })?;
            Ok(Box::new(ReleasedEgressChannel {
                channel,
                exec,
                stop,
                control,
            }))
        })
    }

    /// Use only inside the runtime's supervised BootstrapReady callback.
    /// Consumes the one-shot stdin owner and returns it untouched on success.
    /// Keep opening evidence outside the callback to reconcile ambiguous POSTs.
    pub(super) fn prepare_egress_release<'ready>(
        &'ready self,
        runtime: &'ready ManagedRuntime,
        spec: &'ready ContainerSpec,
        bootstrap: PreparedBootstrapStdin,
        opening_evidence: &'ready mut Option<EgressOpeningEvidence>,
    ) -> BoxFuture<'ready, Result<Box<PreparedEgressRelease>>> {
        let bootstrap = Box::new(bootstrap);
        boxed_execution_phase(|| async move {
            ensure!(
                opening_evidence.is_none(),
                "egress opening already attempted"
            );
            let plan =
                boxed_execution_phase(|| self.egress_running_plan(runtime, &bootstrap)).await?;
            // Only the first channel of this live bootstrap is prepared here.
            let binding = plan.target.binding(IdV1::random(), 1)?;

            self.generation_current()?;
            let (exec, io) =
                boxed_execution_phase(|| runtime.open_owned_egress_proxy(&bootstrap, spec))
                    .await
                    .context("retained egress helper exec unavailable")?;
            self.require_egress_live(&exec)?;
            let (stop, stop_receiver) = ChannelStop::new();
            let prepared = boxed_execution_phase(|| channel::prepare(io, binding, stop_receiver))
                .await
                .map_err(|failure| {
                    anyhow!("egress helper READY failed ({})", failure.cause.code())
                })?;

            self.require_egress_live(&exec)?;
            let running = boxed_execution_phase(|| self.acknowledge_egress_running(&plan.request))
                .await
                .context("retained egress Running acknowledgement unavailable")?;
            self.require_egress_live(&exec)?;

            boxed_execution_phase(|| {
                self.record_egress_opening(&plan.target, prepared.opening_request(), &running)
            })
            .await
            .context("retained egress opening request acknowledgement unavailable")?;
            self.require_egress_live(&exec)?;

            let control: Arc<dyn EgressControl> =
                Arc::new(GatewayEgressControl::new(self.client.clone()));
            let evidence = opening_evidence.insert(EgressOpeningEvidence {
                request: prepared.opening_request().clone(),
                audit: ChannelAuditClosure::Unknown {
                    observations_lost: 0,
                    observations_unconfirmed: 1,
                },
            });
            let channel = match boxed_execution_phase(|| prepared.open(control.as_ref())).await {
                Ok(opened) => {
                    evidence.audit = ChannelAuditClosure::OpeningAcknowledged;
                    opened
                }
                Err(failure) => {
                    evidence.audit = failure.audit;
                    return Err(anyhow!(
                        "retained egress opening failed ({})",
                        failure.cause.code()
                    ));
                }
            };

            self.require_egress_live(&exec)?;
            let mut committed = boxed_execution_phase(|| {
                self.record_egress_release_intent(plan.target, &channel, &running)
            })
            .await
            .context("retained egress intent acknowledgement unavailable")?;
            self.require_egress_live(&exec)?;
            committed.require_context(self.operation, self.context.scope())?;
            let intent = committed
                .draft
                .egress_release
                .take()
                .context("acknowledged checkpoint has no egress release intent")?;
            Ok(Box::new(PreparedEgressRelease {
                bootstrap,
                channel,
                exec,
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
