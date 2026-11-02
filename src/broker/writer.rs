//! A bounded typed queue around the existing journal. Only the dedicated OS
//! thread performs journal work; a caller timeout never cancels queued work.
#[path = "writer_retirement.rs"]
mod retirement;
pub(crate) use retirement::{AuthorizedRetirement, BeginUnusedRetirement, PendingUnusedRetirement};

use super::{
    checkpoint::{BrokerCheckpointV2, SupervisionCheckpointV1},
    journal::{
        AcceptedJobV1, CancelStopProjectionV1, CheckpointReceiptV1, ControlSecrets, FsExclusions,
        Journal, JournalError, JournalLimitsV1, JournalPageV1, JournalResult, JournalScopeV1,
        JournalStatusV1, RejectionCodeV1, RejectionPhaseV1, RejectionReceiptV1, ReservationTicket,
        ReserveResult, RevisionRefV1, StopJobSnapshotV1, StopJournal,
        recovery::{StopRecoveryReaderV1, StopRecoveryReportV1},
    },
    lifecycle::{BrokerCheckpointV3, LocalCommandReceiptV1, LocalCommandRequestV1, LocalCommandV1},
    outbox::{BrokerCheckpointV4, OutboxEventV1, OutboxReceiptV1},
};
use crate::{control_fs::Directory, model::Event, runner::broker::PreparedSubmission};
use std::{
    collections::HashSet,
    fmt,
    sync::{Arc, mpsc},
    time::Duration,
};
use tokio::sync::oneshot;
use uuid::Uuid;

type Reply<T> = oneshot::Sender<JournalResult<T>>;

pub struct AcceptInput {
    ticket: ReservationTicket,
    prepared: Box<PreparedSubmission>,
}
impl AcceptInput {
    pub fn into_parts(self) -> (ReservationTicket, PreparedSubmission) {
        (self.ticket, *self.prepared)
    }
}
pub struct RejectInput {
    ticket: ReservationTicket,
    phase: RejectionPhaseV1,
    code: RejectionCodeV1,
}
impl RejectInput {
    pub fn into_parts(self) -> (ReservationTicket, RejectionPhaseV1, RejectionCodeV1) {
        (self.ticket, self.phase, self.code)
    }
}
/// A cancellation identity is data, never a business or start capability.
pub struct CancelInputV1 {
    operation_id: Uuid,
    command_id: Uuid,
}
impl CancelInputV1 {
    pub fn new(operation_id: Uuid, command_id: Uuid) -> JournalResult<Self> {
        if operation_id.is_nil() || command_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        Ok(Self {
            operation_id,
            command_id,
        })
    }
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }
    pub fn command_id(&self) -> Uuid {
        self.command_id
    }
}
/// Ownership is returned only when the completion never entered the queue.
/// An unknown outcome may already have committed and cannot return its input.
pub enum CompletionFailure<T> {
    NotQueued { error: JournalError, input: T },
    Unknown { error: JournalError },
}
impl<T> CompletionFailure<T> {
    pub fn error(&self) -> JournalError {
        match self {
            Self::NotQueued { error, .. } | Self::Unknown { error } => *error,
        }
    }
    pub fn into_input(self) -> Option<T> {
        match self {
            Self::NotQueued { input, .. } => Some(input),
            Self::Unknown { .. } => None,
        }
    }
}
impl<T> fmt::Debug for CompletionFailure<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotQueued { .. } => "CompletionFailure::NotQueued(<private>)",
            Self::Unknown { .. } => "CompletionFailure::Unknown(<private>)",
        })
    }
}
impl<T> fmt::Display for CompletionFailure<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotQueued { .. } => "journal completion was not queued",
            Self::Unknown { .. } => "journal completion outcome is unknown",
        })
    }
}
impl<T> std::error::Error for CompletionFailure<T> {}
macro_rules! opaque_input {
    ($kind:ty) => {
        impl fmt::Debug for $kind {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($kind), "(<private>)"))
            }
        }
    };
}
opaque_input!(AcceptInput);
opaque_input!(RejectInput);
opaque_input!(CancelInputV1);

enum Command {
    #[cfg(test)]
    TestPause {
        release: mpsc::Receiver<()>,
        active: Arc<std::sync::atomic::AtomicBool>,
        reply: Reply<()>,
    },
    PendingRetirementEvents {
        operation_id: Uuid,
        reply: Reply<Vec<OutboxEventV1>>,
    },
    AcknowledgeRetirementEvents {
        operation_id: Uuid,
        acknowledgements: Vec<(Uuid, String, i64)>,
        reply: Reply<Vec<OutboxReceiptV1>>,
    },
    BeginUnusedRetirement {
        input: BeginUnusedRetirement,
        reply: Reply<PendingUnusedRetirement>,
    },
    AuthorizeUnusedRetirement {
        pending: PendingUnusedRetirement,
        reply: Reply<AuthorizedRetirement>,
    },
    BindFull3 {
        operation_id: Uuid,
        expected: RevisionRefV1,
        binding: Box<super::retirement_checkpoint::Full3RegistrationBindingV1>,
        reply: Reply<CheckpointReceiptV1>,
    },
    FinalizeRetirement {
        receipt: Box<super::enforcer::retirement::VerifiedRetirementReceipt>,
        reply: Reply<CheckpointReceiptV1>,
    },
    Reserve {
        operation_id: Uuid,
        submit: Vec<u8>,
        reply: Reply<ReserveResult>,
    },
    Reject {
        input: RejectInput,
        reply: Reply<RejectionReceiptV1>,
    },
    AcceptPrepared {
        input: AcceptInput,
        reply: Reply<AcceptedJobV1>,
    },
    Load {
        operation_id: Uuid,
        reply: Reply<Option<AcceptedJobV1>>,
    },
    Status {
        operation_id: Uuid,
        reply: Reply<Option<JournalStatusV1>>,
    },
    List {
        after: Option<Uuid>,
        limit: u32,
        reply: Reply<JournalPageV1>,
    },
    AdvanceLifecycle {
        operation_id: Uuid,
        expected: RevisionRefV1,
        next: Box<BrokerCheckpointV3>,
        reply: Reply<CheckpointReceiptV1>,
    },
    AppendLocal {
        operation_id: Uuid,
        command_id: Uuid,
        request: Box<LocalCommandRequestV1>,
        reply: Reply<LocalCommandReceiptV1>,
    },
    CancelStopProjection {
        operation_id: Uuid,
        command_id: Uuid,
        reply: Reply<Option<CancelStopProjectionV1>>,
    },
    AppendCancel {
        input: CancelInputV1,
        reply: Reply<CancelStopProjectionV1>,
    },
    RecoveryStatus {
        operation_id: Uuid,
        reply: Reply<Option<StopJobSnapshotV1>>,
    },
    EnqueueEvents {
        operation_id: Uuid,
        events: Vec<Event>,
        reply: Reply<Vec<OutboxReceiptV1>>,
    },
    AcknowledgeEvents {
        operation_id: Uuid,
        acknowledgements: Vec<(Uuid, i64)>,
        reply: Reply<Vec<OutboxReceiptV1>>,
    },
}

struct ClientState {
    sender: mpsc::SyncSender<Command>,
    scope: JournalScopeV1,
    timeout: Duration,
}

enum Backend {
    Full(Journal),
    Stops(StopJournal),
}
impl Backend {
    fn scope(&self) -> &JournalScopeV1 {
        match self {
            Self::Full(journal) => journal.scope(),
            Self::Stops(journal) => journal.scope(),
        }
    }
    fn full(&self) -> JournalResult<&Journal> {
        match self {
            Self::Full(journal) => Ok(journal),
            // Even an incorrectly routed private message cannot turn a
            // retained cancellation lease into a business capability.
            Self::Stops(_) => Err(JournalError::InvalidInput),
        }
    }
}

fn validate_queue(capacity: u32, timeout: Duration) -> JournalResult<()> {
    if !(1..=256).contains(&capacity) || timeout.is_zero() || timeout > Duration::from_secs(60) {
        return Err(JournalError::InvalidInput);
    }
    Ok(())
}

impl ClientState {
    fn start(backend: Backend, capacity: u32, timeout: Duration) -> JournalResult<Arc<Self>> {
        validate_queue(capacity, timeout)?;
        let scope = backend.scope().clone();
        let (sender, receiver) = mpsc::sync_channel(capacity as usize);
        // Closing the queue is graceful. No client Drop joins a blocked fsync;
        // the actor owns its exact existing lease until queued work drains.
        let _thread = std::thread::Builder::new()
            .name("ai-guard-journal-writer".into())
            .spawn(move || serve_backend(backend, receiver))
            .map_err(|_| JournalError::Io)?;
        Ok(Arc::new(Self {
            sender,
            scope,
            timeout,
        }))
    }
    async fn call<T: Send + 'static>(
        &self,
        command: impl FnOnce(Reply<T>) -> Command + Send,
    ) -> JournalResult<T> {
        tokio::runtime::Handle::try_current().map_err(|_| JournalError::Io)?;
        let (reply, receive) = oneshot::channel();
        match self.sender.try_send(command(reply)) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => return Err(JournalError::Busy),
            Err(mpsc::TrySendError::Disconnected(_)) => return Err(JournalError::Io),
        }
        // A lost reply never removes or cancels the actor's typed command.
        self.receive(receive).await
    }
    async fn receive<T>(&self, receive: oneshot::Receiver<JournalResult<T>>) -> JournalResult<T> {
        tokio::time::timeout(self.timeout, receive)
            .await
            .map_err(|_| JournalError::Io)?
            .map_err(|_| JournalError::Io)?
    }
}

/// A restricted facade over the same actor. It has no conversion to a full
/// writer, no business load, and no arbitrary mutation entry point.
#[derive(Clone)]
pub struct StopWriterClient(Arc<ClientState>);
opaque_input!(StopWriterClient);

/// Profile admission chooses a capability once without releasing its lease.
/// A stop-only profile never gains a full writer through this facade.
#[derive(Clone)]
pub struct ProfileWriterCapabilities {
    full: Option<WriterClient>,
    stops: StopWriterClient,
}
opaque_input!(ProfileWriterCapabilities);

impl ProfileWriterCapabilities {
    pub fn from_full(writer: WriterClient) -> Self {
        let stops = writer.stops();
        Self {
            full: Some(writer),
            stops,
        }
    }
    pub fn open_existing(
        root: &Directory,
        scope: JournalScopeV1,
        exclusions: &FsExclusions,
        limits: JournalLimitsV1,
        control: ControlSecrets,
        capacity: u32,
        timeout: Duration,
    ) -> JournalResult<Option<Self>> {
        validate_queue(capacity, timeout)?;
        let Some(reader) =
            StopRecoveryReaderV1::open_existing(root, scope, exclusions, limits, control)?
        else {
            return Ok(None);
        };
        let report = reader.scan();
        Self::from_recovery_report(&report, false, capacity, timeout).map(Some)
    }
    pub fn from_recovery_report(
        report: &StopRecoveryReportV1,
        force_stops: bool,
        capacity: u32,
        timeout: Duration,
    ) -> JournalResult<Self> {
        validate_queue(capacity, timeout)?;
        match super::journal::admit_recovery_report(report, force_stops)? {
            super::journal::ExistingProfileV1::Full(journal) => {
                WriterClient::start(journal, capacity, timeout).map(Self::from_full)
            }
            super::journal::ExistingProfileV1::Stops(journal) => Ok(Self {
                full: None,
                stops: StopWriterClient(ClientState::start(
                    Backend::Stops(journal),
                    capacity,
                    timeout,
                )?),
            }),
        }
    }
    pub fn full(&self) -> Option<WriterClient> {
        self.full.clone()
    }
    pub fn stops(&self) -> StopWriterClient {
        self.stops.clone()
    }
    pub fn is_stop_only(&self) -> bool {
        self.full.is_none()
    }
}

impl StopWriterClient {
    pub fn scope(&self) -> &JournalScopeV1 {
        &self.0.scope
    }
    pub async fn append_cancel(
        &self,
        input: CancelInputV1,
    ) -> Result<CancelStopProjectionV1, CompletionFailure<CancelInputV1>> {
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(CompletionFailure::NotQueued {
                error: JournalError::Io,
                input,
            });
        }
        let (reply, receive) = oneshot::channel();
        match self
            .0
            .sender
            .try_send(Command::AppendCancel { input, reply })
        {
            Ok(()) => self
                .0
                .receive(receive)
                .await
                .map_err(|error| CompletionFailure::Unknown { error }),
            Err(mpsc::TrySendError::Full(Command::AppendCancel { input, .. })) => {
                Err(CompletionFailure::NotQueued {
                    error: JournalError::Busy,
                    input,
                })
            }
            Err(mpsc::TrySendError::Disconnected(Command::AppendCancel { input, .. })) => {
                Err(CompletionFailure::NotQueued {
                    error: JournalError::Io,
                    input,
                })
            }
            Err(_) => Err(CompletionFailure::Unknown {
                error: JournalError::Io,
            }),
        }
    }
    pub async fn cancel_stop_projection(
        &self,
        operation_id: Uuid,
        command_id: Uuid,
    ) -> JournalResult<Option<CancelStopProjectionV1>> {
        self.0
            .call(move |reply| Command::CancelStopProjection {
                operation_id,
                command_id,
                reply,
            })
            .await
    }
    pub async fn recovery_status(
        &self,
        operation_id: Uuid,
    ) -> JournalResult<Option<StopJobSnapshotV1>> {
        self.0
            .call(move |reply| Command::RecoveryStatus {
                operation_id,
                reply,
            })
            .await
    }
}

/// Clones share one queue and one thread. Dropping the final client closes the
/// queue; the thread drains accepted commands before dropping its journal lease.
/// No Drop waits for a possibly blocked filesystem operation.
#[derive(Clone)]
pub struct WriterClient(Arc<ClientState>);

/// Controls only a test pause in the real actor, without acquiring its journal
/// mutex or replacing its backend, queue, or lease. Drop releases the actor.
#[cfg(test)]
pub(crate) struct WriterPauseGuard {
    release: mpsc::Sender<()>,
    active: Arc<std::sync::atomic::AtomicBool>,
}
#[cfg(test)]
impl WriterPauseGuard {
    pub(crate) fn is_paused(&self) -> bool {
        self.active.load(std::sync::atomic::Ordering::SeqCst)
    }
}
#[cfg(test)]
impl Drop for WriterPauseGuard {
    fn drop(&mut self) {
        let _ = self.release.send(());
    }
}

impl fmt::Debug for WriterClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WriterClient(<private>)")
    }
}

impl WriterClient {
    #[cfg(test)]
    pub(crate) async fn pause_for_test(&self) -> JournalResult<WriterPauseGuard> {
        let (release, wait) = mpsc::channel();
        let active = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Construct before the fallible call: even a queued command whose ACK
        // is lost will be released when this guard drops on the error path.
        let guard = WriterPauseGuard {
            release,
            active: Arc::clone(&active),
        };
        self.call(move |reply| Command::TestPause {
            release: wait,
            active,
            reply,
        })
        .await?;
        Ok(guard)
    }

    pub fn start(journal: Journal, capacity: u32, timeout: Duration) -> JournalResult<Self> {
        ClientState::start(Backend::Full(journal), capacity, timeout).map(Self)
    }

    pub fn scope(&self) -> &JournalScopeV1 {
        &self.0.scope
    }
    pub fn stops(&self) -> StopWriterClient {
        StopWriterClient(Arc::clone(&self.0))
    }

    async fn call<T: Send + 'static>(
        &self,
        command: impl FnOnce(Reply<T>) -> Command + Send,
    ) -> JournalResult<T> {
        self.0.call(command).await
    }

    async fn receive<T>(&self, receive: oneshot::Receiver<JournalResult<T>>) -> JournalResult<T> {
        self.0.receive(receive).await
    }

    pub async fn reserve(
        &self,
        operation_id: Uuid,
        submit: Vec<u8>,
    ) -> JournalResult<ReserveResult> {
        if submit.len() > 256 * 1024 {
            return Err(JournalError::TooLarge);
        }
        self.call(move |reply| Command::Reserve {
            operation_id,
            submit,
            reply,
        })
        .await
    }
    pub async fn reject(
        &self,
        ticket: ReservationTicket,
        phase: RejectionPhaseV1,
        code: RejectionCodeV1,
    ) -> Result<RejectionReceiptV1, CompletionFailure<RejectInput>> {
        let input = RejectInput {
            ticket,
            phase,
            code,
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(CompletionFailure::NotQueued {
                error: JournalError::Io,
                input,
            });
        }
        let (reply, receive) = oneshot::channel();
        match self.0.sender.try_send(Command::Reject { input, reply }) {
            Ok(()) => self
                .receive(receive)
                .await
                .map_err(|error| CompletionFailure::Unknown { error }),
            Err(mpsc::TrySendError::Full(Command::Reject { input, .. })) => {
                Err(CompletionFailure::NotQueued {
                    error: JournalError::Busy,
                    input,
                })
            }
            Err(mpsc::TrySendError::Disconnected(Command::Reject { input, .. })) => {
                Err(CompletionFailure::NotQueued {
                    error: JournalError::Io,
                    input,
                })
            }
            // This private sender was given only the variant above. Keep a
            // conservative failure if that invariant is changed in the future.
            Err(_) => Err(CompletionFailure::Unknown {
                error: JournalError::Io,
            }),
        }
    }
    pub async fn accept_prepared(
        &self,
        ticket: ReservationTicket,
        prepared: PreparedSubmission,
    ) -> Result<AcceptedJobV1, CompletionFailure<AcceptInput>> {
        let input = AcceptInput {
            ticket,
            prepared: Box::new(prepared),
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(CompletionFailure::NotQueued {
                error: JournalError::Io,
                input,
            });
        }
        let (reply, receive) = oneshot::channel();
        match self
            .0
            .sender
            .try_send(Command::AcceptPrepared { input, reply })
        {
            Ok(()) => self
                .receive(receive)
                .await
                .map_err(|error| CompletionFailure::Unknown { error }),
            Err(mpsc::TrySendError::Full(Command::AcceptPrepared { input, .. })) => {
                Err(CompletionFailure::NotQueued {
                    error: JournalError::Busy,
                    input,
                })
            }
            Err(mpsc::TrySendError::Disconnected(Command::AcceptPrepared { input, .. })) => {
                Err(CompletionFailure::NotQueued {
                    error: JournalError::Io,
                    input,
                })
            }
            Err(_) => Err(CompletionFailure::Unknown {
                error: JournalError::Io,
            }),
        }
    }
    pub async fn load(&self, operation_id: Uuid) -> JournalResult<Option<AcceptedJobV1>> {
        self.call(move |reply| Command::Load {
            operation_id,
            reply,
        })
        .await
    }
    pub async fn cancel_stop_projection(
        &self,
        operation_id: Uuid,
        command_id: Uuid,
    ) -> JournalResult<Option<CancelStopProjectionV1>> {
        self.call(move |reply| Command::CancelStopProjection {
            operation_id,
            command_id,
            reply,
        })
        .await
    }
    pub async fn status(&self, operation_id: Uuid) -> JournalResult<Option<JournalStatusV1>> {
        self.call(move |reply| Command::Status {
            operation_id,
            reply,
        })
        .await
    }
    pub async fn list(&self, after: Option<Uuid>, limit: u32) -> JournalResult<JournalPageV1> {
        self.call(move |reply| Command::List {
            after,
            limit,
            reply,
        })
        .await
    }
    pub async fn advance_lifecycle(
        &self,
        operation_id: Uuid,
        expected: RevisionRefV1,
        next: BrokerCheckpointV3,
    ) -> JournalResult<CheckpointReceiptV1> {
        self.call(move |reply| Command::AdvanceLifecycle {
            operation_id,
            expected,
            next: Box::new(next),
            reply,
        })
        .await
    }
    pub async fn append_command(
        &self,
        operation_id: Uuid,
        command_id: Uuid,
        request: LocalCommandRequestV1,
    ) -> JournalResult<LocalCommandReceiptV1> {
        self.call(move |reply| Command::AppendLocal {
            operation_id,
            command_id,
            request: Box::new(request),
            reply,
        })
        .await
    }
    pub async fn enqueue_event(
        &self,
        operation_id: Uuid,
        event: Event,
    ) -> JournalResult<OutboxReceiptV1> {
        self.enqueue_events(operation_id, vec![event])
            .await?
            .into_iter()
            .next()
            .ok_or(JournalError::Corrupt)
    }
    /// Every new event is pending in one durable revision before this returns.
    /// Replayed entries return their original pending receipts, including after ACK.
    pub async fn enqueue_events(
        &self,
        operation_id: Uuid,
        events: Vec<Event>,
    ) -> JournalResult<Vec<OutboxReceiptV1>> {
        if operation_id.is_nil() || events.is_empty() {
            return Err(JournalError::InvalidInput);
        }
        if events.len() > 512 {
            return Err(JournalError::LimitExceeded);
        }
        // Bound the queued data as well as the eventual checkpoint. Structural,
        // contextual and secret checks still happen before mutation in the writer.
        if super::journal::canonical(&events)?.len() > 4 * 1024 * 1024 {
            return Err(JournalError::TooLarge);
        }
        for event in &events {
            if super::journal::canonical(event)?.len() > 120 * 1024 {
                return Err(JournalError::TooLarge);
            }
        }
        self.call(move |reply| Command::EnqueueEvents {
            operation_id,
            events,
            reply,
        })
        .await
    }
    pub async fn acknowledge_event(
        &self,
        operation_id: Uuid,
        event_id: Uuid,
        audit_seq: i64,
    ) -> JournalResult<OutboxReceiptV1> {
        self.acknowledge_events(operation_id, vec![(event_id, audit_seq)])
            .await?
            .into_iter()
            .next()
            .ok_or(JournalError::Corrupt)
    }
    pub async fn acknowledge_events(
        &self,
        operation_id: Uuid,
        acknowledgements: Vec<(Uuid, i64)>,
    ) -> JournalResult<Vec<OutboxReceiptV1>> {
        if operation_id.is_nil()
            || acknowledgements.is_empty()
            || acknowledgements
                .iter()
                .any(|(id, seq)| id.is_nil() || *seq <= 0)
        {
            return Err(JournalError::InvalidInput);
        }
        if acknowledgements.len() > 512 {
            return Err(JournalError::LimitExceeded);
        }
        self.call(move |reply| Command::AcknowledgeEvents {
            operation_id,
            acknowledgements,
            reply,
        })
        .await
    }
}

#[cfg(test)]
fn serve(journal: Journal, receiver: mpsc::Receiver<Command>) {
    serve_backend(Backend::Full(journal), receiver);
}

fn serve_backend(backend: Backend, receiver: mpsc::Receiver<Command>) {
    while let Ok(command) = receiver.recv() {
        match command {
            #[cfg(test)]
            Command::TestPause {
                release,
                active,
                reply,
            } => {
                active.store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = reply.send(Ok(()));
                // Fail-safe bound belongs only to the test seam; no productive
                // queue, RPC, Arm, or journal timeout is changed.
                let _ = release.recv_timeout(Duration::from_secs(10));
                active.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            Command::PendingRetirementEvents {
                operation_id,
                reply,
            } => {
                let result = match &backend {
                    Backend::Full(journal) => {
                        StopJournal::from_full(journal).pending_retirement_events(operation_id)
                    }
                    Backend::Stops(journal) => journal.pending_retirement_events(operation_id),
                };
                let _ = reply.send(result);
            }
            Command::AcknowledgeRetirementEvents {
                operation_id,
                acknowledgements,
                reply,
            } => {
                let result = match &backend {
                    Backend::Full(journal) => StopJournal::from_full(journal)
                        .acknowledge_retirement_events(operation_id, acknowledgements),
                    Backend::Stops(journal) => {
                        journal.acknowledge_retirement_events(operation_id, acknowledgements)
                    }
                };
                let _ = reply.send(result);
            }
            Command::BeginUnusedRetirement { input, reply } => {
                let _ = reply.send(
                    backend
                        .full()
                        .and_then(|journal| retirement::begin(journal, input)),
                );
            }
            Command::AuthorizeUnusedRetirement { pending, reply } => {
                let _ = reply.send(
                    backend
                        .full()
                        .and_then(|journal| retirement::authorize(journal, pending)),
                );
            }
            Command::BindFull3 {
                operation_id,
                expected,
                binding,
                reply,
            } => {
                let _ = reply.send(backend.full().and_then(|journal| {
                    journal.bind_full3_registration(operation_id, expected, *binding)
                }));
            }
            Command::FinalizeRetirement { receipt, reply } => {
                let result = match &backend {
                    Backend::Full(journal) => {
                        StopJournal::from_full(journal).finalize_retirement(*receipt)
                    }
                    Backend::Stops(journal) => journal.finalize_retirement(*receipt),
                };
                let _ = reply.send(result);
            }
            Command::Reserve {
                operation_id,
                submit,
                reply,
            } => {
                let _ = reply.send(
                    backend
                        .full()
                        .and_then(|journal| journal.reserve(operation_id, &submit)),
                );
            }
            Command::Reject { input, reply } => {
                let result = backend.full().and_then(|journal| {
                    let (ticket, phase, code) = input.into_parts();
                    journal.reject(ticket, phase, code)
                });
                let _ = reply.send(result);
            }
            Command::AcceptPrepared { input, reply } => {
                let result = backend.full().and_then(|journal| {
                    let (ticket, prepared) = input.into_parts();
                    prepared.initial_checkpoint().and_then(|initial| {
                        journal.accept_with_launch_profile(
                            ticket,
                            prepared.intent(),
                            prepared.binding_inputs(),
                            &initial,
                            prepared.launch_selection(),
                        )
                    })
                });
                let _ = reply.send(result);
            }
            Command::Load {
                operation_id,
                reply,
            } => {
                let _ = reply.send(
                    backend
                        .full()
                        .and_then(|journal| journal.load(operation_id)),
                );
            }
            Command::Status {
                operation_id,
                reply,
            } => {
                let _ = reply.send(
                    backend
                        .full()
                        .and_then(|journal| journal.status(operation_id)),
                );
            }
            Command::List {
                after,
                limit,
                reply,
            } => {
                let _ = reply.send(
                    backend
                        .full()
                        .and_then(|journal| journal.list(after, limit)),
                );
            }
            Command::AdvanceLifecycle {
                operation_id,
                expected,
                next,
                reply,
            } => {
                let _ =
                    reply.send(backend.full().and_then(|journal| {
                        advance_lifecycle(journal, operation_id, expected, &next)
                    }));
            }
            Command::AppendLocal {
                operation_id,
                command_id,
                request,
                reply,
            } => {
                let _ = reply.send(backend.full().and_then(|journal| {
                    append_command(journal, operation_id, command_id, *request)
                }));
            }
            Command::CancelStopProjection {
                operation_id,
                command_id,
                reply,
            } => {
                let result = match &backend {
                    Backend::Full(journal) => StopJournal::from_full(journal)
                        .cancel_stop_projection(operation_id, command_id),
                    Backend::Stops(journal) => {
                        journal.cancel_stop_projection(operation_id, command_id)
                    }
                };
                let _ = reply.send(result);
            }
            Command::AppendCancel { input, reply } => {
                let result = match &backend {
                    Backend::Full(journal) => StopJournal::from_full(journal)
                        .append_cancel(input.operation_id, input.command_id),
                    Backend::Stops(journal) => {
                        journal.append_cancel(input.operation_id, input.command_id)
                    }
                };
                let _ = reply.send(result);
            }
            Command::RecoveryStatus {
                operation_id,
                reply,
            } => {
                let result = match &backend {
                    Backend::Full(journal) => {
                        StopJournal::from_full(journal).recovery_status(operation_id)
                    }
                    Backend::Stops(journal) => journal.recovery_status(operation_id),
                };
                let _ = reply.send(result);
            }
            Command::EnqueueEvents {
                operation_id,
                events,
                reply,
            } => {
                let _ = reply.send(
                    backend
                        .full()
                        .and_then(|journal| enqueue_events(journal, operation_id, events)),
                );
            }
            Command::AcknowledgeEvents {
                operation_id,
                acknowledgements,
                reply,
            } => {
                let _ = reply.send(backend.full().and_then(|journal| {
                    acknowledge_events(journal, operation_id, acknowledgements)
                }));
            }
        }
    }
}

fn append_command(
    journal: &Journal,
    operation_id: Uuid,
    command_id: Uuid,
    request: LocalCommandRequestV1,
) -> JournalResult<LocalCommandReceiptV1> {
    if operation_id.is_nil() || command_id.is_nil() {
        return Err(JournalError::InvalidInput);
    }
    let job = journal
        .load(operation_id)?
        .ok_or(JournalError::InvalidInput)?;
    let command = LocalCommandV1::new(operation_id, job.intent_hash(), command_id, request)?;
    let mut commands = job
        .broker_checkpoint_v3()
        .map_or_else(Vec::new, |value| value.commands().to_vec());
    if let Some(existing) = commands
        .iter()
        .find(|existing| existing.command_id() == command_id)
    {
        if existing.input_hash() != command.input_hash() {
            return Err(JournalError::Conflict);
        }
        return journal
            .command_receipt(operation_id, command_id)?
            .ok_or(JournalError::Corrupt);
    }
    let broker = match job.broker_checkpoint_v2() {
        Some(value) => value.clone(),
        None => BrokerCheckpointV2::new(
            job.checkpoint().clone(),
            SupervisionCheckpointV1::new(
                job.scope().control_namespace_uuid(),
                Vec::new(),
                Vec::new(),
            )?,
        )?,
    };
    commands.push(command);
    let next = BrokerCheckpointV3::new(broker, commands)?;
    advance_lifecycle(journal, operation_id, job.head().clone(), &next)?;
    journal
        .command_receipt(operation_id, command_id)?
        .ok_or(JournalError::Corrupt)
}

fn advance_lifecycle(
    journal: &Journal,
    operation_id: Uuid,
    expected: RevisionRefV1,
    next: &BrokerCheckpointV3,
) -> JournalResult<CheckpointReceiptV1> {
    journal.advance_lifecycle_preserving_outbox(operation_id, expected, next)
}

fn lifecycle(job: &AcceptedJobV1) -> JournalResult<BrokerCheckpointV3> {
    if let Some(current) = job.broker_checkpoint_v3() {
        return Ok(current.clone());
    }
    let broker = match job.broker_checkpoint_v2() {
        Some(value) => value.clone(),
        None => BrokerCheckpointV2::new(
            job.checkpoint().clone(),
            SupervisionCheckpointV1::new(
                job.scope().control_namespace_uuid(),
                Vec::new(),
                Vec::new(),
            )?,
        )?,
    };
    BrokerCheckpointV3::new(broker, Vec::new())
}

fn receipts(
    job: &AcceptedJobV1,
    states: &[(Uuid, Option<i64>)],
) -> JournalResult<Vec<OutboxReceiptV1>> {
    states
        .iter()
        .map(|(id, seq)| {
            job.outbox_receipt(*id, *seq)
                .cloned()
                .ok_or(JournalError::Corrupt)
        })
        .collect()
}

fn enqueue_events(
    journal: &Journal,
    operation_id: Uuid,
    events: Vec<Event>,
) -> JournalResult<Vec<OutboxReceiptV1>> {
    let mut job = journal
        .load(operation_id)?
        .ok_or(JournalError::InvalidInput)?;
    let mut next = job
        .broker_checkpoint_v4()
        .map_or_else(Vec::new, |value| value.events().to_vec());
    let mut states = Vec::with_capacity(events.len());
    let mut ids = HashSet::new();
    let mut changed = false;
    for event in events {
        let entry = OutboxEventV1::new(operation_id, job.intent_hash(), event)?;
        let id = entry.event_id();
        if !ids.insert(id) {
            return Err(JournalError::Conflict);
        }
        if let Some(existing) = next.iter().find(|existing| existing.event_id() == id) {
            if existing.input_hash() != entry.input_hash()
                || super::journal::canonical(existing.event())?
                    != super::journal::canonical(entry.event())?
            {
                return Err(JournalError::Conflict);
            }
        } else {
            next.push(entry);
            changed = true;
        }
        states.push((id, None));
    }
    if changed {
        let next = BrokerCheckpointV4::new(lifecycle(&job)?, next)?;
        journal.advance_outbox(operation_id, job.head().clone(), &next)?;
        job = journal.load(operation_id)?.ok_or(JournalError::Corrupt)?;
    }
    receipts(&job, &states)
}

fn acknowledge_events(
    journal: &Journal,
    operation_id: Uuid,
    acknowledgements: Vec<(Uuid, i64)>,
) -> JournalResult<Vec<OutboxReceiptV1>> {
    let mut job = journal
        .load(operation_id)?
        .ok_or(JournalError::InvalidInput)?;
    let current = job
        .broker_checkpoint_v4()
        .ok_or(JournalError::InvalidInput)?;
    let mut next = current.events().to_vec();
    let mut states = Vec::with_capacity(acknowledgements.len());
    let mut ids = HashSet::new();
    let mut changed = false;
    for (id, seq) in acknowledgements {
        if !ids.insert(id) {
            return Err(JournalError::Conflict);
        }
        let entry = next
            .iter_mut()
            .find(|entry| entry.event_id() == id)
            .ok_or(JournalError::InvalidInput)?;
        changed |= entry.audit_seq().is_none();
        *entry = entry.acknowledged(seq)?;
        states.push((id, Some(seq)));
    }
    if changed {
        let next = BrokerCheckpointV4::new(current.lifecycle().clone(), next)?;
        journal.advance_outbox(operation_id, job.head().clone(), &next)?;
        job = journal.load(operation_id)?.ok_or(JournalError::Corrupt)?;
    }
    receipts(&job, &states)
}

#[cfg(all(test, unix))]
#[path = "writer_retirement_live_tests.rs"]
mod retirement_live_tests;

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::control_fs::Directory;
    use std::os::unix::fs::PermissionsExt;
    use tokio::sync::oneshot;

    fn private_journal() -> (tempfile::TempDir, Journal) {
        let root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let directory = Directory::open_existing(&root.path().canonicalize().unwrap()).unwrap();
        let scope = JournalScopeV1::new(
            Uuid::from_u128(0x11111111111141118111111111111111),
            "tenant-writer-ownership-test",
            "http://127.0.0.1:8787",
            Uuid::from_u128(0x22222222222242228222222222222222),
            "writer_test",
        )
        .unwrap();
        let journal = Journal::open(
            &directory,
            scope,
            &super::super::journal::FsExclusions::new(),
            super::super::journal::JournalLimitsV1::default(),
            super::super::journal::ControlSecrets::new(Vec::new()).unwrap(),
        )
        .unwrap();
        (root, journal)
    }

    fn paused_client(
        journal: &Journal,
        capacity: usize,
        timeout: Duration,
    ) -> (WriterClient, std::sync::mpsc::Receiver<Command>) {
        let (sender, receiver) = std::sync::mpsc::sync_channel(capacity);
        (
            WriterClient(std::sync::Arc::new(ClientState {
                sender,
                scope: journal.scope().clone(),
                timeout,
            })),
            receiver,
        )
    }

    fn reserved_ticket(journal: &Journal, operation_id: Uuid, submit: &[u8]) -> ReservationTicket {
        match journal.reserve(operation_id, submit).unwrap() {
            ReserveResult::New(ticket) => ticket,
            _ => panic!("fresh operation must yield a new durable ticket"),
        }
    }

    fn check_rejection(receipt: &RejectionReceiptV1, operation_id: Uuid) {
        assert_eq!(receipt.operation_id(), operation_id);
        assert_eq!(receipt.phase(), RejectionPhaseV1::SubmissionPreparation);
        assert_eq!(receipt.code(), RejectionCodeV1::PreparationFailed);
    }

    #[tokio::test]
    async fn full_queue_returns_reject_ticket_for_retry() {
        let (_root, journal) = private_journal();
        let operation_id = Uuid::from_u128(0x33333333333343338333333333333333);
        let submit = br#"{"request":"writer-backpressure-one"}"#.to_vec();
        let ticket = reserved_ticket(&journal, operation_id, &submit);
        let (writer, receiver) = paused_client(&journal, 1, Duration::from_secs(2));

        let (barrier_reply, barrier_result) = oneshot::channel();
        assert!(
            writer
                .0
                .sender
                .try_send(Command::Status {
                    operation_id: Uuid::from_u128(0x44444444444444448444444444444444),
                    reply: barrier_reply,
                })
                .is_ok()
        );

        let failure = writer
            .reject(
                ticket,
                RejectionPhaseV1::SubmissionPreparation,
                RejectionCodeV1::PreparationFailed,
            )
            .await
            .unwrap_err();
        assert_eq!(failure.error(), JournalError::Busy);
        let input = match failure {
            CompletionFailure::NotQueued { input, .. } => input,
            CompletionFailure::Unknown { .. } => {
                panic!("a command rejected by a full queue must return its input")
            }
        };
        let (ticket, phase, code) = input.into_parts();
        assert_eq!(phase, RejectionPhaseV1::SubmissionPreparation);
        assert_eq!(code, RejectionCodeV1::PreparationFailed);

        let worker_journal = journal.clone();
        let worker = std::thread::spawn(move || serve(worker_journal, receiver));
        assert!(barrier_result.await.unwrap().unwrap().is_none());
        let receipt = writer.reject(ticket, phase, code).await.unwrap();
        check_rejection(&receipt, operation_id);

        let replay = writer.reserve(operation_id, submit).await.unwrap();
        let replayed = match replay {
            ReserveResult::Rejected(replayed) => replayed,
            _ => panic!("retry must replay the durable rejection"),
        };
        check_rejection(&replayed, operation_id);
        assert_eq!(replayed.record_hash(), receipt.record_hash());
        let status = writer.status(operation_id).await.unwrap();
        assert!(matches!(
            status,
            Some(JournalStatusV1::Rejected(ref rejected))
                if rejected.record_hash() == receipt.record_hash()
        ));

        drop(writer);
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn timed_out_queued_reject_keeps_unknown_input_and_replays_after_recovery() {
        let (_root, journal) = private_journal();
        let operation_id = Uuid::from_u128(0x55555555555545558555555555555555);
        let submit = br#"{"request":"writer-backpressure-two"}"#.to_vec();
        let ticket = reserved_ticket(&journal, operation_id, &submit);
        let (writer, receiver) = paused_client(&journal, 2, Duration::from_millis(10));

        let failure = writer
            .reject(
                ticket,
                RejectionPhaseV1::SubmissionPreparation,
                RejectionCodeV1::PreparationFailed,
            )
            .await
            .unwrap_err();
        assert_eq!(failure.error(), JournalError::Io);
        assert!(matches!(&failure, CompletionFailure::Unknown { .. }));
        assert!(failure.into_input().is_none());

        let queued = receiver
            .try_recv()
            .expect("the timed-out rejection remains owned by the queue");
        match &queued {
            Command::Reject { input, .. } => {
                assert_eq!(input.phase, RejectionPhaseV1::SubmissionPreparation);
                assert_eq!(input.code, RejectionCodeV1::PreparationFailed);
            }
            _ => panic!("the queued command must be this operation's rejection"),
        }
        assert!(writer.0.sender.try_send(queued).is_ok());

        let recovered_writer = WriterClient(std::sync::Arc::new(ClientState {
            sender: writer.0.sender.clone(),
            scope: writer.0.scope.clone(),
            timeout: Duration::from_secs(2),
        }));
        let worker = std::thread::spawn(move || serve(journal, receiver));

        let status = recovered_writer.status(operation_id).await.unwrap();
        let receipt = match status {
            Some(JournalStatusV1::Rejected(receipt)) => receipt,
            _ => panic!("worker recovery must finish the already queued rejection"),
        };
        check_rejection(&receipt, operation_id);
        let replay = recovered_writer
            .reserve(operation_id, submit)
            .await
            .unwrap();
        let replayed = match replay {
            ReserveResult::Rejected(replayed) => replayed,
            _ => panic!("replay after recovery must not reserve or reject twice"),
        };
        check_rejection(&replayed, operation_id);
        assert_eq!(replayed.record_hash(), receipt.record_hash());
        let page = recovered_writer.list(None, 10).await.unwrap();
        assert_eq!(page.entries().len(), 1);
        assert_eq!(page.entries()[0].operation_id(), operation_id);
        assert_eq!(
            page.entries()[0].kind(),
            super::super::journal::JournalEntryKindV1::Rejected
        );

        drop(writer);
        drop(recovered_writer);
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn stop_append_unknown_retains_queued_identity_and_replays_once() {
        use crate::{broker::intent::ExecutionIntentV1, runner::CheckpointPayloadV1};
        use serde_json::{Value, json};

        let (_root, journal) = private_journal();
        let operation_id = Uuid::from_u128(0x33333333333343338333333333333333);
        let mut plan: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/broker_intent-plan-v3.json"
        )))
        .unwrap();
        plan["version"] = json!(1);
        plan["operation_id"] = json!(operation_id.to_string());
        plan["options"]["profile"] = json!("strict");
        plan["options"]["network"] = json!("none");
        plan["options"]["provider"] = json!("generic");
        plan["options"]["argv"] = json!(["/usr/bin/id"]);
        for field in [
            "client",
            "auth_mode",
            "client_profile",
            "providers_file",
            "prompt",
            "model",
        ] {
            plan["options"].as_object_mut().unwrap().remove(field);
        }
        plan["request"]["profile"] = json!("strict");
        plan["request"]["agent_id"] = json!("generic-22222222-2222-4222-8222-222222222222");
        plan["request"]["command"]["argv"] = json!(["/usr/bin/id"]);
        plan["request"]
            .as_object_mut()
            .unwrap()
            .remove("provider_resolution");
        plan["provider_input"] = Value::Null;
        plan["provider_credential"] = Value::Null;
        let intent =
            ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap();
        let ticket = reserved_ticket(&journal, operation_id, br#"{"stop_queue_test":true}"#);
        let initial = CheckpointPayloadV1::initial(&intent).unwrap();
        journal
            .accept(
                ticket,
                &intent,
                super::super::journal::BindingInputs::None,
                &initial,
            )
            .unwrap();

        let first = Uuid::from_u128(0x66666666666646668666666666666666);
        let second = Uuid::from_u128(0x77777777777747778777777777777777);
        let (writer, receiver) = paused_client(&journal, 1, Duration::from_millis(10));
        let failure = writer
            .stops()
            .append_cancel(CancelInputV1::new(operation_id, first).unwrap())
            .await
            .err()
            .unwrap();
        assert_eq!(failure.error(), JournalError::Io);
        assert!(matches!(&failure, CompletionFailure::Unknown { .. }));
        assert!(failure.into_input().is_none());
        let queued = receiver
            .try_recv()
            .expect("a timed-out append remains queued");
        match &queued {
            Command::AppendCancel { input, .. } => {
                assert_eq!(input.operation_id(), operation_id);
                assert_eq!(input.command_id(), first);
            }
            _ => panic!("the queue must retain the exact cancellation input"),
        }
        assert!(writer.0.sender.try_send(queued).is_ok());
        let failure = writer
            .stops()
            .append_cancel(CancelInputV1::new(operation_id, second).unwrap())
            .await
            .err()
            .unwrap();
        assert_eq!(failure.error(), JournalError::Busy);
        assert!(matches!(&failure, CompletionFailure::NotQueued { .. }));
        let retry = failure.into_input().unwrap();
        assert_eq!(
            (retry.operation_id(), retry.command_id()),
            (operation_id, second)
        );
        assert_eq!(
            journal
                .load(operation_id)
                .unwrap()
                .unwrap()
                .head()
                .revision(),
            0
        );
        assert!(
            journal
                .command_receipt(operation_id, first)
                .unwrap()
                .is_none()
        );
        assert!(
            journal
                .command_receipt(operation_id, second)
                .unwrap()
                .is_none()
        );

        let recovered_writer = WriterClient(Arc::new(ClientState {
            sender: writer.0.sender.clone(),
            scope: writer.0.scope.clone(),
            timeout: Duration::from_secs(2),
        }));
        let worker_journal = journal.clone();
        let worker = std::thread::spawn(move || serve(worker_journal, receiver));
        let (reply, result) = oneshot::channel();
        assert!(
            recovered_writer
                .0
                .sender
                .send(Command::Status {
                    operation_id,
                    reply
                })
                .is_ok()
        );
        assert!(result.await.unwrap().unwrap().is_some());
        let projection = recovered_writer
            .stops()
            .append_cancel(CancelInputV1::new(operation_id, first).unwrap())
            .await
            .unwrap();
        let receipt = projection.receipt();
        assert_eq!(receipt.command_id(), first);
        assert_eq!(receipt.head().revision(), 1);
        assert_eq!(
            journal
                .command_receipt(operation_id, first)
                .unwrap()
                .unwrap()
                .head(),
            receipt.head()
        );
        assert_eq!(
            journal
                .load(operation_id)
                .unwrap()
                .unwrap()
                .broker_checkpoint_v3()
                .unwrap()
                .commands()
                .len(),
            1
        );
        let second_projection = recovered_writer.stops().append_cancel(retry).await.unwrap();
        assert_eq!(second_projection.receipt().command_id(), second);
        assert_eq!(second_projection.receipt().head().revision(), 2);
        assert_eq!(
            journal
                .load(operation_id)
                .unwrap()
                .unwrap()
                .broker_checkpoint_v3()
                .unwrap()
                .commands()
                .len(),
            2
        );
        drop(writer);
        drop(recovered_writer);
        worker.join().unwrap();
    }
}
