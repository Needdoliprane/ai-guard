//! Bounded blocking journal segments. Retention belongs to the broker owner
//! registry; cancelling a serve/observer future never drops its pending handle.
use super::*;
use crate::broker::discovery::RoleLease;
use tokio::{sync::Mutex, task::JoinHandle};

struct IoBudget {
    tenant: Arc<Semaphore>,
    global: Arc<Semaphore>,
}
struct IoResources {
    binding: Arc<Binding>,
    client: Arc<WorkerRelayClient>,
    secrets: Arc<Secrets>,
    role: Arc<RoleLease>,
}
pub(super) enum IoCommand {
    Check,
    Prepare(RequestPayloadV1),
    Dispatched(RequestPayloadV1),
    Confirmed(RequestPayloadV1, crate::relay::RelayReceiptV1),
}
struct IoState {
    ready: Option<Outbox>,
    pending: Option<JoinHandle<(Outbox, Result<(), ControlError>)>>,
    poisoned: bool,
}

/// No production constructor. The future service registry must retain this Arc
/// until explicit join, including after deadline/NeedsReconciliation. The real
/// RoleLease and journal flock remain owned throughout that retention.
pub(super) struct IoOwner {
    state: Mutex<IoState>,
    resources: Arc<IoResources>,
    budget: IoBudget,
    #[cfg(test)]
    pause: std::sync::Mutex<Option<(oneshot::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
}
impl IoOwner {
    pub(super) async fn run(
        &self,
        command: IoCommand,
        reservation: Arc<Reservation>,
    ) -> Result<(), ControlError> {
        // Never queue an IO admission or permit a second operation while an
        // earlier caller was cancelled. join_pending must resolve that handle.
        let mut state = self.state.try_lock().map_err(|_| ControlError::Busy)?;
        if state.poisoned {
            return Err(ControlError::Persistence);
        }
        if state.pending.is_some() {
            return Err(ControlError::NeedsReconciliation);
        }
        let tenant = self
            .budget
            .tenant
            .clone()
            .try_acquire_owned()
            .map_err(|_| ControlError::Capacity)?;
        let global = self
            .budget
            .global
            .clone()
            .try_acquire_owned()
            .map_err(|_| ControlError::Capacity)?;
        let mut outbox = state
            .ready
            .take()
            .ok_or(ControlError::NeedsReconciliation)?;
        let resources = self.resources.clone();
        #[cfg(test)]
        let pause = self
            .pause
            .lock()
            .map_err(|_| ControlError::Persistence)?
            .take();
        state.pending = Some(tokio::task::spawn_blocking(move || {
            // Admission bounds include queued tasks and survive cancellation.
            // Actual release happens after the synchronous operation returns.
            let _retained = (tenant, global, reservation, resources.role.clone());
            #[cfg(test)]
            if let Some((started, finish)) = pause {
                let _ = started.send(());
                if finish.recv_timeout(FRAME_TIME).is_err() {
                    return (outbox, Err(ControlError::Persistence));
                }
            }
            let result = match command {
                IoCommand::Check => outbox.revalidate(),
                IoCommand::Prepare(input) => outbox.prepare(
                    &resources.binding,
                    &input,
                    &resources.client,
                    &resources.secrets,
                ),
                IoCommand::Dispatched(input) => {
                    outbox.dispatched(&resources.binding, &input, &resources.secrets)
                }
                IoCommand::Confirmed(input, receipt) => {
                    outbox.confirmed(&input, &receipt, &resources.secrets)
                }
            };
            (outbox, result)
        }));
        // The JoinHandle stays inside retained state while awaited. Dropping
        // this future drops only the mutex guard, preserving the handle.
        Self::join(&mut state).await
    }

    pub(super) async fn join_pending(&self) -> Result<(), ControlError> {
        let mut state = self.state.lock().await;
        if state.poisoned {
            return Err(ControlError::Persistence);
        }
        if state.pending.is_some() {
            Self::join(&mut state).await
        } else {
            Ok(())
        }
    }

    async fn join(state: &mut IoState) -> Result<(), ControlError> {
        let pending = state
            .pending
            .as_mut()
            .ok_or(ControlError::NeedsReconciliation)?;
        let result = pending.await;
        state.pending.take();
        match result {
            Ok((outbox, result)) => {
                state.ready = Some(outbox);
                result
            }
            Err(_) => {
                state.poisoned = true;
                Err(ControlError::NeedsReconciliation)
            }
        }
    }

    #[cfg(test)]
    pub(super) fn fixture(
        outbox: Outbox,
        binding: Arc<Binding>,
        client: Arc<WorkerRelayClient>,
        secrets: Arc<Secrets>,
        role: Arc<RoleLease>,
        tenant: Arc<Semaphore>,
        global: Arc<Semaphore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(IoState {
                ready: Some(outbox),
                pending: None,
                poisoned: false,
            }),
            resources: Arc::new(IoResources {
                binding,
                client,
                secrets,
                role,
            }),
            budget: IoBudget { tenant, global },
            pause: std::sync::Mutex::new(None),
        })
    }

    #[cfg(test)]
    pub(super) fn pause_next(
        &self,
        started: oneshot::Sender<()>,
        finish: std::sync::mpsc::Receiver<()>,
    ) {
        *self.pause.lock().unwrap() = Some((started, finish));
    }
}
