//! Bounded blocking journal segments. Retention belongs to the broker owner
//! registry; cancelling a serve/observer future never drops its pending handle.
use super::*;
use crate::broker::discovery::RoleLease;
use crate::broker::relay_owner::journal::{
    ChannelChangeV2, ChannelFact, ChannelHistoryV2, ChannelJournal,
};
use tokio::{sync::Mutex, task::JoinHandle};

struct IoBudget {
    tenant: Arc<Semaphore>,
    global: Arc<Semaphore>,
}
struct IoResources {
    role: Arc<RoleLease>,
}
// Bounded by-value IO command; keep value semantics across the channel.
#[allow(clippy::large_enum_variant)]
pub(super) enum IoCommand {
    Check,
    Prepare(
        RequestPayloadV1,
        Arc<Binding>,
        Arc<WorkerRelayClient>,
        Arc<Secrets>,
    ),
    Dispatched(RequestPayloadV1, Arc<Binding>, Arc<Secrets>),
    Confirmed(RequestPayloadV1, crate::relay::RelayReceiptV1, Arc<Secrets>),
    // Closed relay channel read command; production wiring pending.
    #[allow(dead_code)]
    ChannelGet(uuid::Uuid),
    ChannelSave(ChannelFact, Arc<Secrets>),
    ChannelHistory(uuid::Uuid),
    ChannelChange(uuid::Uuid, ChannelChangeV2, Arc<Secrets>),
    LoadBinding(std::path::PathBuf, crate::broker::journal::JournalScopeV1),
    CheckBinding(Arc<crate::workload_cli::broker_binding::RetainedWorkerBinding>),
    BindingClient(
        Arc<crate::workload_cli::broker_binding::RetainedWorkerBinding>,
        crate::broker::journal::JournalScopeV1,
        crate::tls::ClientTrust,
    ),
}
// Bounded by-value IO reply; keep value semantics across the channel.
#[allow(clippy::large_enum_variant)]
pub(super) enum IoReply {
    Unit,
    Channel(Option<ChannelFact>),
    ChannelHistory(Option<ChannelHistoryV2>),
    Binding(Arc<crate::workload_cli::broker_binding::RetainedWorkerBinding>),
    AwaitingBinding,
    Client(Arc<WorkerRelayClient>),
}
impl std::fmt::Debug for IoReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IoReply([private])")
    }
}
impl PartialEq for IoReply {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Unit, Self::Unit) | (Self::AwaitingBinding, Self::AwaitingBinding)
        )
    }
}
type IoJoinResult = (
    Outbox,
    Option<ChannelJournal>,
    Result<IoReply, ControlError>,
);

struct IoState {
    ready: Option<Outbox>,
    channels: Option<ChannelJournal>,
    pending: Option<JoinHandle<IoJoinResult>>,
    poisoned: bool,
}

/// The service registry retains this Arc
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
    pub(super) fn open(
        outbox: Outbox,
        channels: Option<ChannelJournal>,
        role: Arc<RoleLease>,
        tenant: Arc<Semaphore>,
        global: Arc<Semaphore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(IoState {
                ready: Some(outbox),
                channels,
                pending: None,
                poisoned: false,
            }),
            resources: Arc::new(IoResources { role }),
            budget: IoBudget { tenant, global },
            #[cfg(test)]
            pause: std::sync::Mutex::new(None),
        })
    }
    pub(super) async fn run(
        &self,
        command: IoCommand,
        reservation: Arc<Reservation>,
    ) -> Result<IoReply, ControlError> {
        if let Ok(state) = self.state.try_lock() {
            if state.poisoned {
                return Err(ControlError::Persistence);
            }
            if state.pending.is_some() {
                return Err(ControlError::NeedsReconciliation);
            }
        }
        // Admission bounds queued AND running segments before waiting on the
        // tenant journal mutex. A cancelled prior wait leaves a pending handle
        // that must be explicitly joined, never an overlapping writer.
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
        let mut state = self.state.lock().await;
        if state.poisoned {
            return Err(ControlError::Persistence);
        }
        if state.pending.is_some() {
            return Err(ControlError::NeedsReconciliation);
        }
        let mut outbox = state
            .ready
            .take()
            .ok_or(ControlError::NeedsReconciliation)?;
        let mut channels = state.channels.take();
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
            if resources.role.revalidate().is_err() {
                return (outbox, channels, Err(ControlError::AuthorityUnavailable));
            }
            #[cfg(test)]
            if let Some((started, finish)) = pause {
                let _ = started.send(());
                if finish.recv_timeout(FRAME_TIME).is_err() {
                    return (outbox, channels, Err(ControlError::Persistence));
                }
            }
            let result = match command {
                IoCommand::Check => outbox.revalidate().map(|_| IoReply::Unit),
                IoCommand::Prepare(input, binding, client, secrets) => outbox
                    .prepare(&binding, &input, &client, &secrets)
                    .map(|_| IoReply::Unit),
                IoCommand::Dispatched(input, binding, secrets) => outbox
                    .dispatched(&binding, &input, &secrets)
                    .map(|_| IoReply::Unit),
                IoCommand::Confirmed(input, receipt, secrets) => outbox
                    .confirmed(&input, &receipt, &secrets)
                    .map(|_| IoReply::Unit),
                IoCommand::ChannelGet(job) => channels
                    .as_ref()
                    .ok_or(ControlError::Persistence)
                    .and_then(|journal| {
                        journal
                            .get(job)
                            .map(IoReply::Channel)
                            .map_err(|_| ControlError::Persistence)
                    }),
                IoCommand::ChannelSave(fact, secrets) => channels
                    .as_mut()
                    .ok_or(ControlError::Persistence)
                    .and_then(|journal| {
                        journal
                            .append(fact, &secrets.0)
                            .map(|fact| IoReply::Channel(Some(fact)))
                            .map_err(|_| ControlError::Persistence)
                    }),
                IoCommand::ChannelHistory(job) => channels
                    .as_ref()
                    .ok_or(ControlError::Persistence)
                    .and_then(|journal| {
                        journal
                            .history(job)
                            .map(IoReply::ChannelHistory)
                            .map_err(|_| ControlError::Persistence)
                    }),
                IoCommand::ChannelChange(job, change, secrets) => channels
                    .as_mut()
                    .ok_or(ControlError::Persistence)
                    .and_then(|journal| {
                        journal
                            .append_v2(job, change, &secrets.0)
                            .map(|history| IoReply::ChannelHistory(Some(history)))
                            .map_err(|_| ControlError::Persistence)
                    }),
                IoCommand::LoadBinding(path, scope) => {
                    use crate::workload_cli::WorkloadCliError as Error;
                    match crate::workload_cli::broker_binding::RetainedWorkerBinding::open(
                        &path, &scope,
                    ) {
                        Ok(binding) => Ok(IoReply::Binding(Arc::new(binding))),
                        Err(Error::IncompletePreparation | Error::Busy) => {
                            Ok(IoReply::AwaitingBinding)
                        }
                        Err(_) => Err(ControlError::Binding),
                    }
                }
                IoCommand::CheckBinding(binding) => binding
                    .revalidate()
                    .map(|_| IoReply::Unit)
                    .map_err(|_| ControlError::Binding),
                IoCommand::BindingClient(binding, scope, gateway_trust) => binding
                    .client(&scope, &gateway_trust)
                    .map(|client| IoReply::Client(Arc::new(client)))
                    .map_err(|_| ControlError::Binding),
            };
            (outbox, channels, result)
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
            Self::join(&mut state).await.map(|_| ())
        } else {
            Ok(())
        }
    }

    async fn join(state: &mut IoState) -> Result<IoReply, ControlError> {
        let pending = state
            .pending
            .as_mut()
            .ok_or(ControlError::NeedsReconciliation)?;
        let result = pending.await;
        state.pending.take();
        match result {
            Ok((outbox, channels, result)) => {
                state.ready = Some(outbox);
                state.channels = channels;
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
        _binding: Arc<Binding>,
        _client: Arc<WorkerRelayClient>,
        _secrets: Arc<Secrets>,
        role: Arc<RoleLease>,
        tenant: Arc<Semaphore>,
        global: Arc<Semaphore>,
    ) -> Arc<Self> {
        Self::open(outbox, None, role, tenant, global)
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
