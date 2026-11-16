//! One legacy control observer and its final, restrictive start veto.
//! No DTO/receipt constructor and no Native17/prearmed authority.
use super::super::*;
use std::sync::Arc;
use tokio::{sync::watch, time::Instant};

struct Status {
    monitor_alive: bool,
    ready: bool,
    admitted: bool,
    completed: bool,
    primary: Option<Arc<anyhow::Error>>,
    latest_stop_error: Option<Arc<anyhow::Error>>,
}
/// Retains the same attempt-lock FD, authenticated client, plan and runtime.
/// Construction is private to this legacy launch path, after its real checks.
pub(crate) struct PrestartOwner {
    runtime: ManagedRuntime,
    client: AuditClient,
    state: PlanState,
    guard: AttemptGuard,
    worker: String,
    attempt: String,
    session: String,
    origin: String,
    tenant: String,
    deadline: Instant,
    status: Mutex<Status>,
    changed: watch::Sender<()>,
}
pub(crate) struct ControlFailure {
    owner: Arc<PrestartOwner>,
    primary: Arc<anyhow::Error>,
}
impl std::fmt::Debug for ControlFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlFailure")
            .field(
                "stop_observation_failed",
                &self
                    .owner
                    .status
                    .lock()
                    .map(|status| status.latest_stop_error.is_some())
                    .unwrap_or(true),
            )
            .finish_non_exhaustive()
    }
}
impl std::fmt::Display for ControlFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "legacy control supervision failed: {}",
            self.owner
                .client
                .redactor
                .text(&format!("{:#}", self.primary))
        )
    }
}
impl std::error::Error for ControlFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.primary.as_ref().as_ref())
    }
}
/// A runtime result/error is never discarded to report the monitor failure.
pub(crate) struct RejectedLaunch {
    pub(crate) cause: anyhow::Error,
    pub(crate) diagnostic: String,
    pub(crate) execution_error: Option<anyhow::Error>,
    pub(crate) execution: Option<crate::runtime::ManagedRetainedExecution>,
    pub(crate) summary: Option<RunSummary>,
    pub(crate) finalization_error: Option<anyhow::Error>,
    pub(crate) audit_error: Option<anyhow::Error>,
}
impl std::fmt::Debug for RejectedLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RejectedLaunch")
            .field("execution_error_retained", &self.execution_error.is_some())
            .field("execution_result_retained", &self.execution.is_some())
            .field("summary_retained", &self.summary.is_some())
            .field(
                "finalization_error_retained",
                &self.finalization_error.is_some(),
            )
            .field("audit_error_retained", &self.audit_error.is_some())
            .finish_non_exhaustive()
    }
}
impl std::fmt::Display for RejectedLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.diagnostic)
    }
}
impl std::error::Error for RejectedLaunch {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}
struct StopFailure(Arc<anyhow::Error>);
impl std::fmt::Debug for StopFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StopFailure([private native error and IO])")
    }
}
impl std::fmt::Display for StopFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("owned stop remains unconfirmed; native error and IO retained")
    }
}
impl std::error::Error for StopFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref().as_ref())
    }
}
pub(crate) struct MonitorLease {
    owner: Arc<PrestartOwner>,
}
impl Drop for MonitorLease {
    fn drop(&mut self) {
        let incomplete = {
            let mut status = self
                .owner
                .status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            status.monitor_alive = false;
            !status.completed
        };
        if incomplete {
            // Synchronous revocation still fences an unadmitted start. External
            // cancellation after admission is not claimed safe by this Drop.
            self.owner.revoke(anyhow::anyhow!(
                "control observation owner ended before execution result"
            ));
        }
        self.owner.changed.send_replace(());
    }
}
impl PrestartOwner {
    pub(crate) fn new(
        runtime: &ManagedRuntime,
        client: &AuditClient,
        state: &PlanState,
        guard: &AttemptGuard,
        deadline: Instant,
    ) -> Result<Arc<Self>> {
        guard.ensure_current()?;
        let worker = state.worker()?;
        validate_worker(&state.snapshot()?, &worker)?;
        let checkpoint = state.checkpoint()?;
        let (changed, _) = watch::channel(());
        Ok(Arc::new(Self {
            runtime: runtime.clone(),
            client: client.clone(),
            state: state.clone(),
            guard: guard.clone(),
            worker: worker.worker_id,
            attempt: worker.attempt.attempt_id,
            session: worker.launch.session_id,
            origin: checkpoint.gateway_origin,
            tenant: checkpoint.tenant_id,
            deadline,
            status: Mutex::new(Status {
                monitor_alive: false,
                ready: false,
                admitted: false,
                completed: false,
                primary: None,
                latest_stop_error: None,
            }),
            changed,
        }))
    }
    fn subject(&self) -> Result<WorkerV1> {
        let worker = self.state.worker()?;
        let checkpoint = self.state.checkpoint()?;
        anyhow::ensure!(
            worker.worker_id == self.worker
                && worker.attempt.attempt_id == self.attempt
                && worker.launch.session_id == self.session
                && checkpoint.gateway_origin == self.origin
                && checkpoint.tenant_id == self.tenant,
            "legacy control observer subject changed"
        );
        Ok(worker)
    }
    pub(crate) fn require_context(
        &self,
        runtime: &ManagedRuntime,
        state: &PlanState,
        guard: &AttemptGuard,
    ) -> Result<()> {
        anyhow::ensure!(
            Arc::ptr_eq(&self.state.plan, &state.plan)
                && self.guard.same_owner(guard)
                && self.runtime.binding().fingerprint() == runtime.binding().fingerprint(),
            "control observer belongs to another retained actor/runtime/attempt"
        );
        Ok(())
    }
    pub(crate) fn own_monitor(self: &Arc<Self>) -> Result<MonitorLease> {
        let mut status = self
            .status
            .lock()
            .map_err(|_| anyhow::anyhow!("control observer state poisoned"))?;
        anyhow::ensure!(
            !status.monitor_alive && !status.completed && status.primary.is_none(),
            "control observer already owned or closed"
        );
        status.monitor_alive = true;
        Ok(MonitorLease {
            owner: self.clone(),
        })
    }
    pub(crate) fn confirm_read(self: &Arc<Self>) -> Result<()> {
        let worker = self.subject()?;
        anyhow::ensure!(
            worker.state != WorkerStateV1::Stopping,
            "durable stop was observed before start acknowledgement"
        );
        let mut status = self
            .status
            .lock()
            .map_err(|_| anyhow::anyhow!("control observer state poisoned"))?;
        if let Some(primary) = status.primary.clone() {
            drop(status);
            return Err(ControlFailure {
                owner: self.clone(),
                primary,
            }
            .into());
        }
        anyhow::ensure!(status.monitor_alive, "control observation owner is absent");
        status.ready = true;
        self.changed.send_replace(());
        Ok(())
    }
    pub(crate) fn revoked(&self) -> bool {
        self.status
            .lock()
            .map(|status| status.primary.is_some())
            .unwrap_or(true)
    }
    pub(crate) fn ensure_live(self: &Arc<Self>) -> Result<()> {
        let worker = self
            .subject()
            .map_err(|error| anyhow::Error::new(self.revoke(error)))?;
        let result = (|| -> Result<()> {
            let status = self
                .status
                .lock()
                .map_err(|_| anyhow::anyhow!("control observer state poisoned"))?;
            if let Some(primary) = status.primary.clone() {
                drop(status);
                return Err(ControlFailure {
                    owner: self.clone(),
                    primary,
                }
                .into());
            }
            anyhow::ensure!(
                status.monitor_alive && status.ready && !status.completed,
                "legacy start has no ready control observation owner"
            );
            anyhow::ensure!(
                Instant::now() < self.deadline,
                "original execution deadline expired before start"
            );
            anyhow::ensure!(
                worker.state != WorkerStateV1::Stopping,
                "durable stop was observed before start"
            );
            Ok(())
        })();
        result.map_err(|error| self.revoke(error).into())
    }
    pub(crate) async fn wait_ready(self: &Arc<Self>) -> Result<()> {
        let mut changed = self.changed.subscribe();
        loop {
            {
                let status = self
                    .status
                    .lock()
                    .map_err(|_| anyhow::anyhow!("control observer state poisoned"))?;
                if let Some(primary) = status.primary.clone() {
                    drop(status);
                    return Err(ControlFailure {
                        owner: self.clone(),
                        primary,
                    }
                    .into());
                }
                if status.ready && status.monitor_alive {
                    return Ok(());
                }
            }
            changed
                .changed()
                .await
                .context("control observation readiness owner disappeared")?;
        }
    }
    pub(crate) fn revoke(self: &Arc<Self>, error: anyhow::Error) -> ControlFailure {
        let primary = {
            let mut status = self
                .status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            status
                .primary
                .get_or_insert_with(|| Arc::new(error))
                .clone()
        };
        self.changed.send_replace(());
        ControlFailure {
            owner: self.clone(),
            primary,
        }
    }
    pub(crate) fn execution_finished(&self) {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .completed = true;
        self.changed.send_replace(());
    }
    pub(crate) fn veto(
        self: &Arc<Self>,
        created: OwnedContainerSnapshot,
    ) -> Result<crate::runtime::LegacyStartVeto> {
        let result = (|| -> Result<crate::runtime::LegacyStartVeto> {
            self.ensure_live()?;
            let worker = self.subject()?;
            let claim = worker
                .attempt
                .start_operation_id
                .context("legacy start claim is absent")?;
            anyhow::ensure!(
                worker.state == WorkerStateV1::StartIssued
                    && self.runtime.binding().fingerprint()
                        == created.handle.authority_fingerprint()
                    && worker.attempt.container_id.as_deref() == Some(created.handle.id()),
                "legacy start claim differs from its real created instance"
            );
            Ok(crate::runtime::LegacyStartVeto::new(Box::new(FinalVeto {
                owner: self.clone(),
                created,
                claim,
            })))
        })();
        result.map_err(|error| self.revoke(error).into())
    }
    pub(crate) async fn stop_owned(&self) -> Result<()> {
        // An uncertain native stop is retained once, not blindly reissued on
        // its Err. The execution's existing native owner/deadline also remains
        // alive; no uncertainty becomes terminal or releases its hold.
        if let Some(error) = self
            .status
            .lock()
            .map_err(|_| anyhow::anyhow!("control observer state poisoned"))?
            .latest_stop_error
            .clone()
        {
            return Err(StopFailure(error).into());
        }
        let result = async {
            if let Some(owned) = self.state.checkpoint()?.owned {
                self.runtime.stop_snapshot_owned(&owned).await?;
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        result.map_err(|error| {
            let retained = Arc::new(error);
            self.status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .latest_stop_error = Some(retained.clone());
            anyhow::Error::new(StopFailure(retained))
        })
    }
    pub(crate) async fn retain_stop_until_completed(
        self: &Arc<Self>,
        mut completed: watch::Receiver<bool>,
    ) {
        let mut stopped_once = false;
        loop {
            if *completed.borrow() {
                return;
            }
            let (admitted, stop_unconfirmed) = self
                .status
                .lock()
                .map(|status| (status.admitted, status.latest_stop_error.is_some()))
                .unwrap_or((true, true));
            if !stop_unconfirmed && (!stopped_once || admitted) {
                // Each call re-observes the exact owned instance before any
                // targeted kill. Successful Created is not a final result for
                // an emission already admitted; keep its owner until done.
                let _ = self.stop_owned().await;
                stopped_once = true;
            }
            tokio::select! {
                changed = completed.changed() => {
                    if changed.is_err() { return; }
                },
                _ = tokio::time::sleep(super::POLL) => {},
            }
        }
    }
}
struct FinalVeto {
    owner: Arc<PrestartOwner>,
    created: OwnedContainerSnapshot,
    claim: String,
}
impl crate::runtime::BeforeSpawnGuard for FinalVeto {
    fn check_before_spawn(self: Box<Self>, target: &str) -> Result<()> {
        // Full FD/subject checks happen in the caller's blocking spawn unit,
        // outside the tiny revocation gate. No gate is held across filesystem IO.
        let checked = (|| -> Result<()> {
            self.owner.guard.ensure_current()?;
            let worker = self.owner.subject()?;
            let owned = self
                .owner
                .state
                .checkpoint()?
                .owned
                .context("retained created instance is absent")?;
            anyhow::ensure!(
                target == self.created.handle.id()
                    && owned.handle.id() == target
                    && self.owner.runtime.binding().fingerprint()
                        == self.created.handle.authority_fingerprint()
                    && owned.handle.authority_fingerprint()
                        == self.created.handle.authority_fingerprint()
                    && owned.handle.session_id() == self.created.handle.session_id()
                    && owned.handle.operation_id() == self.created.handle.operation_id()
                    && worker.state == WorkerStateV1::StartIssued
                    && worker.attempt.start_operation_id.as_deref() == Some(self.claim.as_str())
                    && worker.attempt.container_id.as_deref() == Some(target),
                "legacy start veto differs from the retained actor/claim/instance"
            );
            owned
                .observation
                .validate_identity_allocation(&self.created.observation)?;
            Ok(())
        })();
        if let Err(error) = checked {
            return Err(self.owner.revoke(error).into());
        }
        let result = (|| -> Result<()> {
            let mut status = self
                .owner
                .status
                .lock()
                .map_err(|_| anyhow::anyhow!("control observer state poisoned"))?;
            if let Some(primary) = status.primary.clone() {
                drop(status);
                return Err(ControlFailure {
                    owner: self.owner.clone(),
                    primary,
                }
                .into());
            }
            anyhow::ensure!(
                status.ready && status.monitor_alive && !status.completed && !status.admitted,
                "legacy start observer is absent, closed or already consumed"
            );
            anyhow::ensure!(
                Instant::now() < self.owner.deadline,
                "original execution deadline expired before start"
            );
            // Linearization of this emission admission. A later revocation
            // retains enforcement until execution/IO/callbacks actually return.
            status.admitted = true;
            Ok(())
        })();
        result.map_err(|error| self.owner.revoke(error).into())
    }
}
