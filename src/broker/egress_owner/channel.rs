//! One broker-owned egress channel: the trusted end of a worker helper pipe.
//!
//! Order per flow: validate the requested authority, obtain a durable gateway
//! decision, then (allow only) resolve with full address validation and
//! connect once to a pinned numeric address. No worker byte reaches upstream
//! before that. Every upstream socket is owned here; dropping a flow closes
//! it. Revocation, expiry, a lost lease, helper EOF or a protocol violation
//! close the affected sockets. The channel never reattaches an old epoch.

use super::resolve::{ResolveError, Resolver};
use crate::control::egress_decision::*;
use crate::egress::{
    Admission, Authority,
    channel_protocol::{
        self, CommonEgressHandshakeV1, EgressFrameReader, FRAME_TIME, HANDSHAKE_TIME,
    },
    ids::{CHANNEL_CONTRACT, IdV1},
    protocol::{
        DirectionV1, EgressCapabilityV1, EgressChannelState, EgressMessageV1, EgressReasonV1,
        MAX_DATA_BYTES,
    },
};
use std::{
    collections::{HashMap, HashSet},
    future::{Future, poll_fn},
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::Poll,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpSocket, TcpStream},
    sync::{Semaphore, mpsc, oneshot, watch},
    task::JoinSet,
    time::{Instant, MissedTickBehavior, sleep_until, timeout_at},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ControlError {
    /// The gateway or its durable audit could not answer.
    Unavailable,
    /// The gateway refused the request itself (binding, schema, permission).
    Refused,
}

pub(crate) type ControlFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ControlError>> + Send + 'a>>;

/// Gateway port of the egress owner. Each call is one durable exchange.
pub(crate) trait EgressControl: Send + Sync + 'static {
    fn decide(&self, request: EgressDecideRequestV1) -> ControlFuture<'_, EgressDecisionV1>;
    fn observe(&self, request: EgressObserveRequestV1) -> ControlFuture<'_, ()>;
    fn renew(&self, request: EgressRenewRequestV1) -> ControlFuture<'_, EgressRenewResponseV1>;
}

/// Published time bounds. Product values; tests may only shorten them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ChannelTimes {
    /// A live flow without a successful renewal for this long is closed.
    pub(crate) lease: Duration,
    pub(crate) renew_every: Duration,
    /// Decision + resolution + connect, inside the ledger's 10 s open time.
    pub(crate) open_budget: Duration,
    pub(crate) connect_attempt: Duration,
}
impl Default for ChannelTimes {
    fn default() -> Self {
        Self {
            lease: Duration::from_secs(5),
            renew_every: Duration::from_secs(2),
            open_budget: Duration::from_secs(8),
            connect_attempt: Duration::from_secs(3),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChannelExit {
    /// The helper closed its pipe (worker ended or helper killed).
    HelperClosed,
    /// HELLO/READY failed or the helper violated the protocol.
    Protocol,
    /// `egress.channel.opened` could not be recorded; no flow was admitted.
    AuditUnavailable,
    /// The owner was asked to stop (worker terminal or broker shutdown).
    Stopped,
}
impl ChannelExit {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::HelperClosed => "helper_exit",
            Self::Protocol => "protocol",
            Self::AuditUnavailable => "audit_unavailable",
            Self::Stopped => "owner_exit",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalQuiescence {
    Confirmed,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChannelTermination {
    pub(crate) cause: ChannelExit,
    /// Covers the broker's local egress owners and socket handles only.
    /// Unknown blocks epoch handover pending reconciliation.
    pub(crate) local_quiescence: LocalQuiescence,
}

impl ChannelTermination {
    fn without_flows(cause: ChannelExit) -> Self {
        Self {
            cause,
            local_quiescence: LocalQuiescence::Confirmed,
        }
    }
}

const COMMAND_SLOTS: usize = 32;
const OBSERVATION_SLOTS: usize = 8;
const OBSERVATION_DRAIN: Duration = Duration::from_secs(5);

enum Command {
    Write(Vec<u8>),
    Credit(usize),
    ShutdownWrite,
}

struct Opened {
    stream: TcpStream,
    peer: SocketAddr,
    connected_at: Instant,
    valid_until: Instant,
    resolver: &'static str,
}

struct PendingOpen {
    socket: SocketControl,
    peer: SocketAddr,
    connected_at: Instant,
    resolver: &'static str,
}

struct SocketControl {
    authority: Arc<watch::Sender<SocketAuthority>>,
    commands: mpsc::Sender<Command>,
    closed: oneshot::Receiver<SocketClosure>,
    valid_until: Instant,
}

type ObservedStop = (Instant, EgressCloseReasonV1);

#[derive(Clone, Copy, Default)]
struct SocketAuthority {
    lease_until: Option<Instant>,
    stop: Option<ObservedStop>,
}

impl SocketAuthority {
    fn deadline(self, valid_until: Instant) -> (Instant, EgressCloseReasonV1) {
        match self.lease_until {
            Some(lease) if lease < valid_until => (lease, EgressCloseReasonV1::LeaseLost),
            _ => (valid_until, EgressCloseReasonV1::Expired),
        }
    }

    fn loss(self, valid_until: Instant, now: Instant) -> Option<EgressCloseReasonV1> {
        let (deadline, expiry) = self.deadline(valid_until);
        if let Some((at, reason)) = self.stop
            && at < deadline
        {
            return Some(reason);
        }
        (now >= deadline).then_some(expiry)
    }
}

#[derive(Default)]
struct SocketTraffic {
    bytes_up: u64,
    bytes_down: u64,
}

struct SocketClosure {
    at: Instant,
    reason: EgressCloseReasonV1,
    traffic: SocketTraffic,
}

struct ClosingFlow {
    flow_id: u64,
    decision_id: IdV1,
    connected_at: Instant,
    closed: oneshot::Receiver<SocketClosure>,
}

struct OpeningHandle {
    owner: OpeningOwner,
    outcome: oneshot::Receiver<OpeningOutcome>,
}

struct OpeningOutcome {
    result: Result<PendingOpen, Refusal>,
    socket_unobserved: bool,
}

struct OpeningOwner(watch::Sender<Option<ObservedStop>>);

impl OpeningOwner {
    fn new() -> (Self, watch::Receiver<Option<ObservedStop>>) {
        let (sender, receiver) = watch::channel(None);
        (Self(sender), receiver)
    }

    fn retire(&self, reason: EgressCloseReasonV1) {
        self.0.send_if_modified(|stop| {
            if stop.is_some() {
                return false;
            }
            *stop = Some((Instant::now(), reason));
            true
        });
    }
}

impl Drop for OpeningOwner {
    fn drop(&mut self) {
        self.retire(EgressCloseReasonV1::OwnerExit);
    }
}

pub(crate) struct ChannelStop(watch::Sender<ChannelStopState>);

pub(crate) struct ChannelStopState(bool);

impl ChannelStop {
    pub(crate) fn new() -> (Self, watch::Receiver<ChannelStopState>) {
        let (sender, receiver) = watch::channel(ChannelStopState(false));
        (Self(sender), receiver)
    }

    pub(crate) fn stop(&self) {
        self.0.send_replace(ChannelStopState(true));
    }
}

impl Drop for ChannelStop {
    fn drop(&mut self) {
        self.stop();
    }
}

struct OpeningLifetime {
    owner: watch::Receiver<Option<ObservedStop>>,
    stop: watch::Receiver<ChannelStopState>,
}

impl OpeningLifetime {
    fn loss(&self) -> Option<ObservedStop> {
        let retired = *self.owner.borrow();
        retired.or_else(|| {
            (self.owner.has_changed().is_err()
                || self.stop.borrow().0
                || !matches!(self.stop.has_changed(), Ok(false)))
            .then(|| (Instant::now(), EgressCloseReasonV1::OwnerExit))
        })
    }

    fn poll_connect<T>(
        &self,
        deadline: Instant,
        poll: impl FnOnce() -> Poll<T>,
    ) -> Option<Poll<T>> {
        // Both invalidators publish through these watch values before dropping.
        // Hold the snapshots through one synchronous poll only.
        let stop = self.stop.borrow();
        let owner = self.owner.borrow();
        if stop.0
            || owner.is_some()
            || !matches!(self.stop.has_changed(), Ok(false))
            || !matches!(self.owner.has_changed(), Ok(false))
            || Instant::now() >= deadline
        {
            return None;
        }
        Some(poll())
    }

    async fn cancelled(&self) {
        if self.loss().is_none() {
            let mut owner = self.owner.clone();
            let mut stop = self.stop.clone();
            tokio::select! {
                _ = owner.changed() => {},
                _ = stop.changed() => {},
            }
        }
    }
}

#[derive(Default)]
struct ConnectProgress {
    pending: bool,
    unobserved: bool,
}

enum RetiredFlow {
    Socket(ClosingFlow),
    Opening {
        flow_id: u64,
        decision_id: IdV1,
        reason: EgressCloseReasonV1,
        outcome: oneshot::Receiver<OpeningOutcome>,
    },
}

#[derive(Clone, Copy)]
enum TerminalFrame {
    Reset(EgressReasonV1),
    Rejected(EgressReasonV1),
}

impl TerminalFrame {
    fn message(self, flow_id: u64, confirmed: Option<EgressCloseReasonV1>) -> EgressMessageV1 {
        let reason =
            |fallback| confirmed.map_or(fallback, |reason| wire_close_reason(reason, fallback));
        match self {
            Self::Reset(fallback) => EgressMessageV1::Reset {
                flow_id,
                reason: reason(fallback),
            },
            Self::Rejected(fallback) => EgressMessageV1::Rejected {
                flow_id,
                reason: reason(fallback),
            },
        }
    }
}

struct SocketLifetime {
    valid_until: Instant,
    opening: Option<OpeningLifetime>,
}

impl Opened {
    fn start_pump(
        self,
        flow_id: u64,
        events: mpsc::Sender<Event>,
        opening: OpeningLifetime,
    ) -> PendingOpen {
        let Self {
            stream,
            peer,
            connected_at,
            valid_until,
            resolver,
        } = self;
        // The socket owner runs independently of the frame writer and event
        // queue. No data can move until the channel owner sends commands.
        PendingOpen {
            socket: SocketControl::spawn(flow_id, stream, events, valid_until, Some(opening)),
            peer,
            connected_at,
            resolver,
        }
    }
}

impl PendingOpen {
    fn available(&self) -> bool {
        Instant::now() < self.socket.valid_until && !self.socket.commands.is_closed()
    }
}

impl SocketControl {
    fn spawn<IO>(
        flow_id: u64,
        stream: IO,
        events: mpsc::Sender<Event>,
        valid_until: Instant,
        opening: Option<OpeningLifetime>,
    ) -> Self
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (commands, receiver) = mpsc::channel(COMMAND_SLOTS);
        let (authority, updates) = watch::channel(SocketAuthority::default());
        let (closed_sender, closed) = oneshot::channel();
        tokio::spawn(drive_socket(
            flow_id,
            stream,
            receiver,
            events,
            SocketLifetime {
                valid_until,
                opening,
            },
            updates,
            closed_sender,
        ));
        Self {
            authority: Arc::new(authority),
            commands,
            closed,
            valid_until,
        }
    }

    fn arm_lease(&self, lease_until: Instant) -> bool {
        Instant::now() < self.valid_until
            && self
                .authority
                .send(SocketAuthority {
                    lease_until: Some(lease_until),
                    stop: None,
                })
                .is_ok()
    }

    fn renew_lease(&self, lease_until: Instant) {
        self.authority.send_if_modified(|state| {
            let now = Instant::now();
            if now < self.valid_until
                && state.stop.is_none()
                && state
                    .lease_until
                    .is_some_and(|prior| now < prior && prior < lease_until)
            {
                state.lease_until = Some(lease_until);
                return true;
            }
            false
        });
    }

    fn active_io_deadline(&self) -> Option<Instant> {
        let state = *self.authority.borrow();
        state.lease_until?;
        if self.commands.is_closed() || state.loss(self.valid_until, Instant::now()).is_some() {
            return None;
        }
        Some(state.deadline(self.valid_until).0)
    }

    fn stop(
        self,
        flow_id: u64,
        decision_id: IdV1,
        connected_at: Instant,
        reason: EgressCloseReasonV1,
    ) -> ClosingFlow {
        let at = Instant::now();
        signal_loss(&self.authority, at, reason);
        ClosingFlow {
            flow_id,
            decision_id,
            connected_at,
            closed: self.closed,
        }
    }
}

fn signal_loss(
    authority: &watch::Sender<SocketAuthority>,
    observed_at: Instant,
    reason: EgressCloseReasonV1,
) {
    authority.send_if_modified(|state| {
        if state.stop.is_some() {
            return false;
        }
        state.stop = Some((observed_at, reason));
        true
    });
}

struct Refusal {
    reason: EgressReasonV1,
    /// Present when a durable allow preceded a resolution/connect failure.
    failure: Option<(&'static str, &'static str)>,
}

enum Event {
    Opened {
        flow_id: u64,
        decision_id: IdV1,
    },
    Upstream {
        flow_id: u64,
        data: Vec<u8>,
    },
    UpstreamEof {
        flow_id: u64,
    },
    SocketClosed {
        flow_id: u64,
        reason: EgressCloseReasonV1,
    },
    Written {
        flow_id: u64,
        bytes: usize,
    },
    Renewed {
        sent: Instant,
        decisions: Vec<IdV1>,
        result: Result<EgressRenewResponseV1, ControlError>,
    },
}

struct Flow {
    decision_id: IdV1,
    opening: Option<OpeningHandle>,
    socket: Option<SocketControl>,
    started: Instant,
    worker_closed: bool,
    upstream_closed: bool,
    in_flight_up: usize,
}

impl Flow {
    fn stop(self, flow_id: u64, reason: EgressCloseReasonV1) -> Option<RetiredFlow> {
        if let Some(socket) = self.socket {
            return Some(RetiredFlow::Socket(socket.stop(
                flow_id,
                self.decision_id,
                self.started,
                reason,
            )));
        }
        let opening = self.opening?;
        opening.owner.retire(reason);
        Some(RetiredFlow::Opening {
            flow_id,
            decision_id: self.decision_id,
            reason,
            outcome: opening.outcome,
        })
    }
}

struct Owner<W> {
    output: Option<W>,
    stop: watch::Receiver<ChannelStopState>,
    stopped_during_write: bool,
    binding: EgressBindingV1,
    control: Arc<dyn EgressControl>,
    resolver: Arc<Resolver>,
    times: ChannelTimes,
    ledger: EgressChannelState,
    started: Instant,
    flows: HashMap<u64, Flow>,
    events: mpsc::Sender<Event>,
    observations: JoinSet<bool>,
    observation_slots: Arc<Semaphore>,
    observations_lost: u64,
    retirement_unconfirmed: bool,
    renewing: bool,
}

/// Run one channel until the helper closes, a violation occurs or `stop`.
/// `io` is the helper's stdio (Docker/Podman exec) or any equivalent stream.
pub(crate) async fn run<IO>(
    io: IO,
    binding: EgressBindingV1,
    control: Arc<dyn EgressControl>,
    resolver: Arc<Resolver>,
    times: ChannelTimes,
    mut stop: watch::Receiver<ChannelStopState>,
) -> ChannelTermination
where
    IO: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut input, mut output) = tokio::io::split(io);
    let Ok(binding_id) = IdV1::parse(&binding.attempt_id) else {
        return ChannelTermination::without_flows(ChannelExit::Protocol);
    };
    let hello = CommonEgressHandshakeV1 {
        version: 1,
        protocol: CHANNEL_CONTRACT.into(),
        claim_id: binding.channel_id.clone(),
        binding_id,
        transport_epoch: binding.epoch,
        context_digest: binding.context_digest.clone(),
        challenge: IdV1::random(),
        helper_version: 1,
        capability: EgressCapabilityV1::default(),
    };
    let deadline = Instant::now() + HANDSHAKE_TIME;
    if channel_protocol::write_handshake(&mut output, &hello, deadline)
        .await
        .is_err()
        || channel_protocol::read_handshake(&mut input, deadline)
            .await
            .ok()
            .as_ref()
            != Some(&hello)
    {
        return ChannelTermination::without_flows(ChannelExit::Protocol);
    }
    // The channel is durably known before any flow can be admitted.
    let opened = EgressObservationV1::ChannelOpened {
        binding: binding.clone(),
    };
    if control.observe(observation(opened)).await.is_err() {
        return ChannelTermination::without_flows(ChannelExit::AuditUnavailable);
    }
    let started = Instant::now();
    let mut ledger = EgressChannelState::new(0);
    if ledger
        .accept_capability(Some(&hello.capability), 0)
        .is_err()
    {
        return ChannelTermination::without_flows(ChannelExit::Protocol);
    }
    let (events, mut incoming) = mpsc::channel(64);
    let mut owner = Owner {
        output: Some(output),
        stop: stop.clone(),
        stopped_during_write: false,
        binding,
        control,
        resolver,
        times,
        ledger,
        started,
        flows: HashMap::new(),
        events,
        observations: JoinSet::new(),
        observation_slots: Arc::new(Semaphore::new(OBSERVATION_SLOTS)),
        observations_lost: 0,
        retirement_unconfirmed: false,
        renewing: false,
    };
    let mut reader = EgressFrameReader::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut renew = tokio::time::interval(times.renew_every);
    renew.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let exit = loop {
        tokio::select! {
            biased;
            _ = stop.changed() => break ChannelExit::Stopped,
            frame = reader.read(&mut input) => match frame {
                Ok(Some(message)) => {
                    if owner.worker_frame(message).await.is_err() { break ChannelExit::Protocol; }
                }
                Ok(None) => break ChannelExit::HelperClosed,
                Err(_) => break ChannelExit::Protocol,
            },
            Some(event) = incoming.recv() => {
                if owner.event(event).await.is_err() { break ChannelExit::Protocol; }
            }
            _ = tick.tick() => {
                if owner.deadlines().await.is_err() { break ChannelExit::Protocol; }
            }
            _ = renew.tick() => owner.renew(),
        }
    };
    let exit = if owner.stopped_during_write {
        ChannelExit::Stopped
    } else {
        exit
    };
    // A timed-out write may have emitted only a frame prefix. Close both
    // halves before waiting for the audit work, without sending another frame.
    drop(input);
    drop(owner.output.take());
    drop(incoming);
    owner
        .shutdown(exit, Instant::now() + OBSERVATION_DRAIN)
        .await
}

fn observation(observation: EgressObservationV1) -> EgressObserveRequestV1 {
    EgressObserveRequestV1 {
        version: 1,
        observation_id: IdV1::random(),
        observation,
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

impl<W: AsyncWrite + Unpin + Send> Owner<W> {
    fn now_ms(&self) -> u64 {
        millis(self.started.elapsed())
    }

    /// Send one frame after the ledger admitted it. Stale means the worker
    /// already retired that flow; nothing is written.
    async fn send(&mut self, message: EgressMessageV1) -> Result<Admission, ()> {
        self.send_before(message, Instant::now() + FRAME_TIME).await
    }

    async fn send_before(
        &mut self,
        message: EgressMessageV1,
        deadline: Instant,
    ) -> Result<Admission, ()> {
        let deadline = deadline.min(Instant::now() + FRAME_TIME);
        if Instant::now() >= deadline {
            return Err(());
        }
        let now = self.now_ms();
        let admission = self.ledger.on_broker(&message, now).map_err(|_| ())?;
        if admission == Admission::Applied {
            let output = self.output.as_mut().ok_or(())?;
            tokio::select! {
                biased;
                _ = self.stop.changed() => {
                    self.stopped_during_write = true;
                    return Err(());
                }
                _ = sleep_until(deadline) => return Err(()),
                written = channel_protocol::write_message(output, &message, deadline) => {
                    written.map_err(|_| ())?;
                }
            }
        }
        Ok(admission)
    }

    async fn send_active(&mut self, message: EgressMessageV1) -> Result<Admission, ()> {
        let Some(socket) = self
            .flows
            .get(&message.flow_id())
            .and_then(|flow| flow.socket.as_ref())
        else {
            return Ok(Admission::Stale);
        };
        let Some(deadline) = socket.active_io_deadline() else {
            return Ok(Admission::Stale);
        };
        let commands = socket.commands.clone();
        // Closing a socket during publication can leave a partial frame. An
        // error abandons the channel rather than reusing that transport.
        tokio::select! {
            biased;
            _ = commands.closed() => Err(()),
            result = self.send_before(message, deadline) => result,
        }
    }

    fn record(&mut self, observation_value: EgressObservationV1) {
        let Ok(slot) = self.observation_slots.clone().try_acquire_owned() else {
            self.observations_lost += 1;
            return;
        };
        let control = self.control.clone();
        self.observations.spawn(async move {
            let _slot = slot;
            control
                .observe(observation(observation_value))
                .await
                .is_ok()
        });
        while let Some(done) = self.observations.try_join_next() {
            if !matches!(done, Ok(true)) {
                self.observations_lost += 1;
            }
        }
    }

    fn mark_retirement_unconfirmed(&mut self) {
        self.retirement_unconfirmed = true;
        self.observations_lost += 1;
    }

    async fn record_closed(
        &mut self,
        closing: ClosingFlow,
        deadline: Instant,
    ) -> Result<EgressCloseReasonV1, ()> {
        let Ok(Ok(closed)) = timeout_at(deadline, closing.closed).await else {
            self.mark_retirement_unconfirmed();
            return Err(());
        };
        self.record(EgressObservationV1::Closed {
            binding: self.binding.clone(),
            decision_id: closing.decision_id,
            flow_id: closing.flow_id,
            reason: closed.reason,
            bytes_up: closed.traffic.bytes_up,
            bytes_down: closed.traffic.bytes_down,
            duration_ms: millis(closed.at.saturating_duration_since(closing.connected_at)),
        });
        Ok(closed.reason)
    }

    async fn collect_retired(
        &mut self,
        retired: RetiredFlow,
        deadline: Instant,
    ) -> Result<Option<EgressCloseReasonV1>, ()> {
        match retired {
            RetiredFlow::Socket(closing) => self.record_closed(closing, deadline).await.map(Some),
            RetiredFlow::Opening {
                flow_id,
                decision_id,
                reason,
                outcome,
            } => {
                let Ok(Ok(outcome)) = timeout_at(deadline, outcome).await else {
                    self.mark_retirement_unconfirmed();
                    return Err(());
                };
                self.observe_opening(flow_id, &decision_id, &outcome);
                if let Ok(opened) = outcome.result {
                    let closing =
                        opened
                            .socket
                            .stop(flow_id, decision_id, opened.connected_at, reason);
                    return self.record_closed(closing, deadline).await.map(Some);
                }
                Ok(None)
            }
        }
    }

    fn observe_opening(&mut self, flow_id: u64, decision_id: &IdV1, outcome: &OpeningOutcome) {
        if outcome.socket_unobserved {
            self.observations_lost += 1;
        }
        let (connected, failure, peer, resolver) = match &outcome.result {
            Ok(opened) => (true, None, Some(opened.peer.to_string()), opened.resolver),
            Err(refusal) if !outcome.socket_unobserved => {
                let Some((failure, resolver)) = refusal.failure else {
                    return;
                };
                (false, Some(failure.into()), None, resolver)
            }
            Err(_) => return,
        };
        self.record(EgressObservationV1::Connection {
            binding: self.binding.clone(),
            decision_id: decision_id.clone(),
            flow_id,
            connected,
            failure,
            peer,
            resolver: resolver.into(),
        });
    }

    /// Reset a flow toward the worker and close its upstream socket.
    async fn reset(
        &mut self,
        flow_id: u64,
        wire: EgressReasonV1,
        reason: EgressCloseReasonV1,
    ) -> Result<(), ()> {
        self.retire_flows([(flow_id, TerminalFrame::Reset(wire), reason)])
            .await
    }

    async fn retire_flows(
        &mut self,
        targets: impl IntoIterator<Item = (u64, TerminalFrame, EgressCloseReasonV1)>,
    ) -> Result<(), ()> {
        // Signal every affected owner before any confirmation or helper I/O.
        let deadline = Instant::now() + OBSERVATION_DRAIN;
        let stopped: Vec<_> = targets
            .into_iter()
            .filter_map(|(flow_id, wire, reason)| {
                self.flows
                    .remove(&flow_id)
                    .map(|flow| (flow_id, wire, flow.stop(flow_id, reason)))
            })
            .collect();
        let mut notices = Vec::with_capacity(stopped.len());
        let mut closed = Ok(());
        for (flow_id, wire, closing) in stopped {
            let confirmed = match closing {
                Some(closing) => match self.collect_retired(closing, deadline).await {
                    Ok(confirmed) => confirmed,
                    Err(()) => {
                        closed = Err(());
                        continue;
                    }
                },
                None => None,
            };
            notices.push(wire.message(flow_id, confirmed));
        }
        closed?;
        for notice in notices {
            self.send_before(notice, deadline).await?;
        }
        Ok(())
    }

    async fn worker_frame(&mut self, message: EgressMessageV1) -> Result<(), ()> {
        let now = self.now_ms();
        if self.ledger.on_worker(&message, now).map_err(|_| ())? == Admission::Stale {
            return Ok(());
        }
        match message {
            EgressMessageV1::Open { flow_id, authority } => self.open(flow_id, &authority).await,
            data @ EgressMessageV1::Data { .. } => {
                let flow_id = data.flow_id();
                let bytes = data.decoded_data().map_err(|_| ())?.ok_or(())?;
                let Some(flow) = self.flows.get_mut(&flow_id) else {
                    return Err(());
                };
                let Some(socket) = &flow.socket else {
                    return Err(());
                };
                flow.in_flight_up += bytes.len();
                if socket.commands.try_send(Command::Write(bytes)).is_err() {
                    return self
                        .reset(
                            flow_id,
                            EgressReasonV1::Capacity,
                            EgressCloseReasonV1::Protocol,
                        )
                        .await;
                }
                Ok(())
            }
            EgressMessageV1::Credit { flow_id, bytes } => {
                if let Some(commands) = self
                    .flows
                    .get(&flow_id)
                    .and_then(|flow| flow.socket.as_ref())
                    .map(|socket| &socket.commands)
                    && commands.try_send(Command::Credit(bytes as usize)).is_err()
                {
                    return self
                        .reset(
                            flow_id,
                            EgressReasonV1::Capacity,
                            EgressCloseReasonV1::Protocol,
                        )
                        .await;
                }
                Ok(())
            }
            EgressMessageV1::HalfClose { flow_id } => {
                let Some(flow) = self.flows.get_mut(&flow_id) else {
                    return Err(());
                };
                flow.worker_closed = true;
                if let Some(socket) = &flow.socket {
                    let _ = socket.commands.try_send(Command::ShutdownWrite);
                }
                self.finish_if_done(flow_id).await
            }
            EgressMessageV1::Reset { flow_id, .. } => {
                // The ledger already retired the flow; close the socket only.
                if let Some(retired) = self
                    .flows
                    .remove(&flow_id)
                    .and_then(|flow| flow.stop(flow_id, EgressCloseReasonV1::WorkerReset))
                {
                    self.collect_retired(retired, Instant::now() + OBSERVATION_DRAIN)
                        .await?;
                }
                Ok(())
            }
            _ => Err(()),
        }
    }

    async fn open(&mut self, flow_id: u64, raw: &str) -> Result<(), ()> {
        let decision_id = IdV1::random();
        let Ok(authority) = Authority::parse_connect(raw) else {
            self.flows.insert(flow_id, self.pending_flow(decision_id));
            return self.reset_pending(flow_id, EgressReasonV1::Protocol).await;
        };
        let (owner, owned) = OpeningOwner::new();
        let (reported, outcome) = oneshot::channel();
        let mut flow = self.pending_flow(decision_id.clone());
        flow.opening = Some(OpeningHandle { owner, outcome });
        self.flows.insert(flow_id, flow);
        let request = EgressDecideRequestV1 {
            version: 1,
            decision_id: decision_id.clone(),
            binding: self.binding.clone(),
            flow_id,
            authority: format!("{}:{}", authority.name(), authority.port()),
        };
        let control = self.control.clone();
        let resolver = self.resolver.clone();
        let events = self.events.clone();
        let times = self.times;
        let lifetime = OpeningLifetime {
            owner: owned,
            stop: self.stop.clone(),
        };
        tokio::spawn(async move {
            let mut progress = ConnectProgress::default();
            let result = tokio::select! {
                biased;
                _ = lifetime.cancelled() => Err(cancelled_opening()),
                result = open_upstream(control, resolver, times, request, authority, &lifetime, &mut progress) => result,
            };
            let socket_unobserved = progress.pending || progress.unobserved;
            let result = result.map(|opened| opened.start_pump(flow_id, events.clone(), lifetime));
            // The flow owns the sole result. The queue only wakes its owner.
            if reported
                .send(OpeningOutcome {
                    result,
                    socket_unobserved,
                })
                .is_ok()
            {
                let _ = events
                    .send(Event::Opened {
                        flow_id,
                        decision_id,
                    })
                    .await;
            }
        });
        Ok(())
    }

    fn pending_flow(&self, decision_id: IdV1) -> Flow {
        let now = Instant::now();
        Flow {
            decision_id,
            opening: None,
            socket: None,
            started: now,
            worker_closed: false,
            upstream_closed: false,
            in_flight_up: 0,
        }
    }

    async fn reset_pending(&mut self, flow_id: u64, reason: EgressReasonV1) -> Result<(), ()> {
        self.retire_flows([(
            flow_id,
            TerminalFrame::Rejected(reason),
            EgressCloseReasonV1::Protocol,
        )])
        .await
    }

    async fn event(&mut self, event: Event) -> Result<(), ()> {
        match event {
            Event::Opened {
                flow_id,
                decision_id,
            } => self.opened(flow_id, decision_id).await,
            Event::Upstream { flow_id, data } => {
                let length = data.len();
                let message = EgressMessageV1::data(flow_id, &data).map_err(|_| ())?;
                if self.send_active(message).await? == Admission::Applied {
                    self.ledger
                        .release_buffer(flow_id, DirectionV1::BrokerToWorker, length)
                        .map_err(|_| ())?;
                }
                Ok(())
            }
            Event::Written { flow_id, bytes } => {
                let Some(flow) = self.flows.get_mut(&flow_id) else {
                    return Ok(());
                };
                flow.in_flight_up = flow.in_flight_up.saturating_sub(bytes);
                let worker_closed = flow.worker_closed;
                self.ledger
                    .release_buffer(flow_id, DirectionV1::WorkerToBroker, bytes)
                    .map_err(|_| ())?;
                if !worker_closed {
                    let bytes = u32::try_from(bytes).map_err(|_| ())?;
                    self.send_active(EgressMessageV1::Credit { flow_id, bytes })
                        .await?;
                }
                self.finish_if_done(flow_id).await
            }
            Event::UpstreamEof { flow_id } => {
                let Some(flow) = self.flows.get_mut(&flow_id) else {
                    return Ok(());
                };
                flow.upstream_closed = true;
                self.send_active(EgressMessageV1::HalfClose { flow_id })
                    .await?;
                self.finish_if_done(flow_id).await
            }
            Event::SocketClosed { flow_id, reason } => {
                self.reset(
                    flow_id,
                    wire_close_reason(reason, EgressReasonV1::Transport),
                    reason,
                )
                .await
            }
            Event::Renewed {
                sent,
                decisions,
                result,
            } => {
                self.renewing = false;
                let Ok(answer) = result else { return Ok(()) };
                let leased_until = sent + self.times.lease;
                let mut stopped = Vec::new();
                for (flow_id, flow) in &self.flows {
                    if answer.revoked.contains(&flow.decision_id) {
                        stopped.push((
                            *flow_id,
                            TerminalFrame::Reset(EgressReasonV1::Revoked),
                            EgressCloseReasonV1::Revoked,
                        ));
                    } else if answer.expired.contains(&flow.decision_id) {
                        stopped.push((
                            *flow_id,
                            TerminalFrame::Reset(EgressReasonV1::Expired),
                            EgressCloseReasonV1::Expired,
                        ));
                    } else if decisions.contains(&flow.decision_id)
                        && let Some(socket) = &flow.socket
                    {
                        socket.renew_lease(leased_until);
                    }
                }
                self.retire_flows(stopped).await
            }
        }
    }

    async fn opened(&mut self, flow_id: u64, decision_id: IdV1) -> Result<(), ()> {
        let Some(OpeningHandle {
            owner: _keep_open,
            mut outcome,
        }) = self
            .flows
            .get_mut(&flow_id)
            .filter(|flow| flow.decision_id == decision_id)
            .and_then(|flow| flow.opening.take())
        else {
            return Ok(());
        };
        // The wakeup is sent only after the result is available.
        let Ok(outcome) = outcome.try_recv() else {
            self.mark_retirement_unconfirmed();
            return Err(());
        };
        self.observe_opening(flow_id, &decision_id, &outcome);
        match outcome.result {
            Ok(opened) => {
                if !opened.available() {
                    let closed = self
                        .close_opening(flow_id, decision_id, opened, EgressCloseReasonV1::Protocol)
                        .await?;
                    let reason = wire_close_reason(closed, EgressReasonV1::Transport);
                    return self.reset_pending(flow_id, reason).await;
                }
                match self.publish_opening(flow_id, &opened).await {
                    Ok(Admission::Applied) if self.arm_opening(&opened) => {
                        let flow = self.flows.get_mut(&flow_id).ok_or(())?;
                        flow.socket = Some(opened.socket);
                        flow.started = opened.connected_at;
                        Ok(())
                    }
                    Ok(Admission::Stale) => {
                        self.flows.remove(&flow_id);
                        self.close_opening(
                            flow_id,
                            decision_id,
                            opened,
                            EgressCloseReasonV1::WorkerReset,
                        )
                        .await
                        .map(|_| ())
                    }
                    _ => {
                        self.flows.remove(&flow_id);
                        let reason = if self.stopped_during_write || self.stop_requested() {
                            self.stopped_during_write = true;
                            EgressCloseReasonV1::OwnerExit
                        } else {
                            EgressCloseReasonV1::Protocol
                        };
                        self.close_opening(flow_id, decision_id, opened, reason)
                            .await?;
                        Err(())
                    }
                }
            }
            Err(refusal) => self.reset_pending(flow_id, refusal.reason).await,
        }
    }

    fn stop_requested(&self) -> bool {
        self.stop.borrow().0 || !matches!(self.stop.has_changed(), Ok(false))
    }

    fn arm_opening(&self, opened: &PendingOpen) -> bool {
        // Hold the Stop snapshot through this synchronous handoff.
        let stop = self.stop.borrow();
        !stop.0
            && matches!(self.stop.has_changed(), Ok(false))
            && opened.socket.arm_lease(Instant::now() + self.times.lease)
    }

    async fn publish_opening(
        &mut self,
        flow_id: u64,
        opened: &PendingOpen,
    ) -> Result<Admission, ()> {
        let deadline = opened.socket.valid_until;
        let commands = &opened.socket.commands;
        tokio::select! {
            biased;
            _ = commands.closed() => Err(()),
            result = self.publish_opening_frames(flow_id, deadline, commands) => result,
        }
    }

    async fn publish_opening_frames(
        &mut self,
        flow_id: u64,
        deadline: Instant,
        commands: &mpsc::Sender<Command>,
    ) -> Result<Admission, ()> {
        if self
            .send_before(EgressMessageV1::Accepted { flow_id }, deadline)
            .await?
            == Admission::Stale
        {
            return Ok(Admission::Stale);
        }
        if commands.is_closed() {
            return Err(());
        }
        let credit = EgressMessageV1::Credit {
            flow_id,
            bytes: MAX_DATA_BYTES as u32,
        };
        match self.send_before(credit, deadline).await? {
            Admission::Applied => Ok(Admission::Applied),
            // Accepted has already been published. Abandon the channel if
            // it cannot deliver the rest of a usable opening.
            Admission::Stale => Err(()),
        }
    }

    async fn close_opening(
        &mut self,
        flow_id: u64,
        decision_id: IdV1,
        opened: PendingOpen,
        reason: EgressCloseReasonV1,
    ) -> Result<EgressCloseReasonV1, ()> {
        let closing = opened
            .socket
            .stop(flow_id, decision_id, opened.connected_at, reason);
        self.record_closed(closing, Instant::now() + OBSERVATION_DRAIN)
            .await
    }

    async fn finish_if_done(&mut self, flow_id: u64) -> Result<(), ()> {
        let done = self.flows.get(&flow_id).is_some_and(|flow| {
            flow.worker_closed
                && flow.upstream_closed
                && flow.in_flight_up == 0
                && flow
                    .socket
                    .as_ref()
                    .and_then(SocketControl::active_io_deadline)
                    .is_some()
        });
        if done
            && let Some(closing) = self
                .flows
                .remove(&flow_id)
                .and_then(|flow| flow.stop(flow_id, EgressCloseReasonV1::EndOfStream))
        {
            let reason = self
                .collect_retired(closing, Instant::now() + OBSERVATION_DRAIN)
                .await?
                .ok_or(())?;
            let message = if reason == EgressCloseReasonV1::EndOfStream {
                EgressMessageV1::Closed {
                    flow_id,
                    reason: EgressReasonV1::EndOfStream,
                }
            } else {
                EgressMessageV1::Reset {
                    flow_id,
                    reason: wire_close_reason(reason, EgressReasonV1::Transport),
                }
            };
            self.send(message).await?;
        }
        Ok(())
    }

    async fn deadlines(&mut self) -> Result<(), ()> {
        let now_ms = self.now_ms();
        let mut stopped = Vec::new();
        for flow_id in self.ledger.check_deadlines(now_ms).map_err(|_| ())? {
            match self.flows.get(&flow_id) {
                Some(flow) => {
                    let wire = if flow.socket.is_some() {
                        TerminalFrame::Reset(EgressReasonV1::Timeout)
                    } else {
                        TerminalFrame::Rejected(EgressReasonV1::Timeout)
                    };
                    stopped.push((flow_id, wire, EgressCloseReasonV1::IdleTimeout));
                }
                None => {}
            }
        }
        self.retire_flows(stopped).await
    }

    fn renew(&mut self) {
        if self.renewing {
            return;
        }
        let sockets: Vec<_> = self
            .flows
            .values()
            .filter_map(|flow| {
                flow.socket
                    .as_ref()
                    .map(|socket| (flow.decision_id.clone(), Arc::downgrade(&socket.authority)))
            })
            .collect();
        if sockets.is_empty() {
            return;
        }
        let decisions: Vec<_> = sockets.iter().map(|(id, _)| id.clone()).collect();
        self.renewing = true;
        let request = EgressRenewRequestV1 {
            version: 1,
            binding: self.binding.clone(),
            decisions: decisions.clone(),
        };
        let control = self.control.clone();
        let events = self.events.clone();
        tokio::spawn(async move {
            let sent = Instant::now();
            let result = control.renew(request).await.and_then(|answer| {
                if !valid_renewal(&answer, &decisions) {
                    return Err(ControlError::Refused);
                }
                let observed = Instant::now();
                for (id, socket) in sockets {
                    let reason = if answer.revoked.contains(&id) {
                        EgressCloseReasonV1::Revoked
                    } else if answer.expired.contains(&id) {
                        EgressCloseReasonV1::Expired
                    } else {
                        continue;
                    };
                    // A response only narrows authority on the exact socket
                    // generation named when that renewal was sent.
                    if let Some(authority) = socket.upgrade() {
                        signal_loss(&authority, observed, reason);
                    }
                }
                Ok(answer)
            });
            let _ = events
                .send(Event::Renewed {
                    sent,
                    decisions,
                    result,
                })
                .await;
        });
    }

    async fn shutdown(&mut self, exit: ChannelExit, deadline: Instant) -> ChannelTermination {
        let reason = match exit {
            ChannelExit::Protocol => EgressCloseReasonV1::Protocol,
            _ => EgressCloseReasonV1::OwnerExit,
        };
        let closing: Vec<_> = self
            .flows
            .drain()
            .filter_map(|(flow_id, flow)| flow.stop(flow_id, reason))
            .collect();
        for flow in closing {
            let _ = self.collect_retired(flow, deadline).await;
        }
        self.ledger.close();
        if exit != ChannelExit::AuditUnavailable {
            let closed = EgressObservationV1::ChannelClosed {
                binding: self.binding.clone(),
                reason: exit.code().into(),
                observations_lost: self.observations_lost,
            };
            self.record(closed);
        }
        while let Ok(Some(done)) = timeout_at(deadline, self.observations.join_next()).await {
            if !matches!(done, Ok(true)) {
                self.observations_lost += 1;
            }
        }
        self.observations.abort_all();
        ChannelTermination {
            cause: exit,
            local_quiescence: if self.retirement_unconfirmed {
                LocalQuiescence::Unknown
            } else {
                LocalQuiescence::Confirmed
            },
        }
    }
}

fn wire_close_reason(reason: EgressCloseReasonV1, fallback: EgressReasonV1) -> EgressReasonV1 {
    match reason {
        EgressCloseReasonV1::Revoked => EgressReasonV1::Revoked,
        EgressCloseReasonV1::Expired => EgressReasonV1::Expired,
        EgressCloseReasonV1::LeaseLost => EgressReasonV1::AuditUnavailable,
        EgressCloseReasonV1::IdleTimeout => EgressReasonV1::Timeout,
        EgressCloseReasonV1::WorkerReset | EgressCloseReasonV1::OwnerExit => {
            EgressReasonV1::Cancelled
        }
        EgressCloseReasonV1::UpstreamError => EgressReasonV1::Transport,
        // The socket cannot observe the channel's queue capacity.
        EgressCloseReasonV1::Protocol if fallback == EgressReasonV1::Capacity => fallback,
        EgressCloseReasonV1::Protocol => EgressReasonV1::Protocol,
        // The worker accepts EndOfStream only in Closed.
        EgressCloseReasonV1::EndOfStream => EgressReasonV1::Transport,
    }
}

fn valid_renewal(answer: &EgressRenewResponseV1, decisions: &[IdV1]) -> bool {
    let mut seen = HashSet::new();
    answer.version == 1
        && answer
            .revoked
            .iter()
            .chain(&answer.expired)
            .all(|id| decisions.contains(id) && seen.insert(id))
}

fn cancelled_opening() -> Refusal {
    Refusal {
        reason: EgressReasonV1::Cancelled,
        failure: None,
    }
}

/// Durable decision first; resolution and connect only after an allow.
async fn open_upstream(
    control: Arc<dyn EgressControl>,
    resolver: Arc<Resolver>,
    times: ChannelTimes,
    request: EgressDecideRequestV1,
    authority: Authority,
    lifetime: &OpeningLifetime,
    progress: &mut ConnectProgress,
) -> Result<Opened, Refusal> {
    if lifetime.loss().is_some() {
        return Err(cancelled_opening());
    }
    let sent = Instant::now();
    let deadline = sent + times.open_budget;
    let decision = match timeout_at(deadline, control.decide(request.clone())).await {
        Ok(Ok(decision)) => decision,
        _ => {
            return Err(Refusal {
                reason: EgressReasonV1::AuditUnavailable,
                failure: None,
            });
        }
    };
    if !decision.allows(&request) {
        let reason = match decision.outcome {
            EgressOutcomeV1::Review => EgressReasonV1::Review,
            _ => EgressReasonV1::Denied,
        };
        return Err(Refusal {
            reason,
            failure: None,
        });
    }
    let valid_until = sent
        .checked_add(Duration::from_millis(decision.remaining_ms.unwrap_or(0)))
        .ok_or(Refusal {
            reason: EgressReasonV1::Denied,
            failure: None,
        })?;
    let (deadline, elapsed_reason) = if valid_until <= deadline {
        (valid_until, EgressReasonV1::Expired)
    } else {
        (deadline, EgressReasonV1::Timeout)
    };
    if Instant::now() >= deadline {
        return Err(Refusal {
            reason: elapsed_reason,
            failure: None,
        });
    }
    if lifetime.loss().is_some() {
        return Err(cancelled_opening());
    }
    let resolution = match resolver.resolve(&authority, deadline).await {
        Ok(resolution) => resolution,
        Err(error) => {
            let reason = match error {
                ResolveError::Timeout => elapsed_reason,
                ResolveError::RefusedAddress => EgressReasonV1::Denied,
                _ => EgressReasonV1::Unavailable,
            };
            return Err(Refusal {
                reason,
                failure: Some((error.code(), "host_resolver")),
            });
        }
    };
    let source = resolution.source.code();
    // Keep the resolver's validated order. This permit authorizes one connect.
    let Some(address) = resolution.addresses.first() else {
        return Err(Refusal {
            reason: EgressReasonV1::Unavailable,
            failure: None,
        });
    };
    if lifetime.loss().is_some() {
        return Err(cancelled_opening());
    }
    let now = Instant::now();
    if now >= deadline {
        return Err(Refusal {
            reason: elapsed_reason,
            failure: None,
        });
    }
    let target = SocketAddr::new(*address, authority.port());
    let attempt = (now + times.connect_attempt).min(deadline);
    let mut attempted = false;
    let mut connecting = std::pin::pin!(async {
        let socket = match target {
            SocketAddr::V4(_) => TcpSocket::new_v4()?,
            SocketAddr::V6(_) => TcpSocket::new_v6()?,
        };
        // Tokio's TcpSocket issues the nonblocking connect on its first poll.
        // Later polls only wait for the result of this same attempt.
        socket.connect(target).await
    });
    let connect = poll_fn(|cx| {
        if let Some(polled) = lifetime.poll_connect(attempt, || {
            attempted = true;
            progress.pending = true;
            connecting.as_mut().poll(cx)
        }) {
            return polled.map(|result| {
                progress.pending = false;
                result.ok()
            });
        }
        progress.unobserved |= progress.pending;
        progress.pending = false;
        Poll::Ready(None)
    });
    // An elapsed attempt takes precedence over a ready connect future.
    let stream = tokio::select! {
        biased;
        _ = sleep_until(attempt) => {
            // The wait ended without an observed OS result.
            progress.unobserved |= progress.pending;
            progress.pending = false;
            None
        }
        stream = connect => stream,
    };
    if let Some(stream) = stream {
        if stream.peer_addr().is_ok_and(|peer| peer == target) {
            let _ = stream.set_nodelay(true);
            return Ok(Opened {
                stream,
                peer: target,
                connected_at: Instant::now(),
                valid_until,
                resolver: source,
            });
        }
        progress.unobserved = true;
    }
    Err(Refusal {
        reason: if Instant::now() >= deadline {
            elapsed_reason
        } else {
            EgressReasonV1::Unavailable
        },
        failure: attempted.then_some(("connect_failed", source)),
    })
}

/// Cancel the pump by dropping it, including any pending socket or event I/O.
async fn drive_socket<IO>(
    flow_id: u64,
    stream: IO,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
    lifetime: SocketLifetime,
    mut authority: watch::Receiver<SocketAuthority>,
    closed: oneshot::Sender<SocketClosure>,
) where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    let SocketLifetime {
        valid_until,
        opening,
    } = lifetime;
    let mut traffic = SocketTraffic::default();
    let reason = {
        let pump_authority = authority.clone();
        let io = pump(
            flow_id,
            stream,
            commands,
            events.clone(),
            &mut traffic,
            &pump_authority,
            valid_until,
        );
        tokio::pin!(io);
        loop {
            let mut state = *authority.borrow_and_update();
            if state.lease_until.is_none()
                && let Some(stop) = opening.as_ref().and_then(OpeningLifetime::loss)
                && state.stop.is_none_or(|existing| stop.0 < existing.0)
            {
                state.stop = Some(stop);
            }
            if let Some(reason) = state.loss(valid_until, Instant::now()) {
                break reason;
            }
            let (deadline, _) = state.deadline(valid_until);
            tokio::select! {
                biased;
                changed = authority.changed() => {
                    if changed.is_err() {
                        break EgressCloseReasonV1::OwnerExit;
                    }
                }
                _ = async {
                    if let Some(opening) = &opening { opening.cancelled().await; }
                }, if state.lease_until.is_none() && opening.is_some() => {},
                _ = sleep_until(deadline) => {},
                result = &mut io => break result,
            }
        }
    };
    let at = Instant::now();
    let active = authority.borrow().lease_until.is_some();
    let _ = closed.send(SocketClosure {
        at,
        reason,
        traffic,
    });
    if active {
        let _ = events.send(Event::SocketClosed { flow_id, reason }).await;
    }
}

/// Sole owner of one upstream socket. Reads only within the worker's credit.
async fn pump<IO>(
    flow_id: u64,
    stream: IO,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
    traffic: &mut SocketTraffic,
    authority: &watch::Receiver<SocketAuthority>,
    valid_until: Instant,
) -> EgressCloseReasonV1
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    let (mut read, mut write) = tokio::io::split(stream);
    let mut budget = 0usize;
    let mut eof = false;
    let mut buffer = vec![0u8; MAX_DATA_BYTES];
    let loss = || authority.borrow().loss(valid_until, Instant::now());
    loop {
        tokio::select! {
            biased;
            command = commands.recv() => match command {
                Some(Command::Write(bytes)) => {
                    let mut written = 0;
                    while written < bytes.len() {
                        if let Some(reason) = loss() { return reason; }
                        let count = match write.write(&bytes[written..]).await {
                            Ok(count) if count > 0 => count,
                            _ => return EgressCloseReasonV1::UpstreamError,
                        };
                        written += count;
                        traffic.bytes_up = traffic.bytes_up.saturating_add(count as u64);
                    }
                    if events.send(Event::Written { flow_id, bytes: written }).await.is_err() {
                        return EgressCloseReasonV1::OwnerExit;
                    }
                }
                Some(Command::Credit(bytes)) => budget = budget.saturating_add(bytes),
                Some(Command::ShutdownWrite) => {
                    if let Some(reason) = loss() { return reason; }
                    let _ = write.shutdown().await;
                }
                None => return EgressCloseReasonV1::OwnerExit,
            },
            read_result = async {
                if let Some(reason) = loss() { return Err(reason); }
                read.read(&mut buffer[..budget.min(MAX_DATA_BYTES)]).await.map_err(|_| EgressCloseReasonV1::UpstreamError)
            }, if budget > 0 && !eof => match read_result {
                Ok(0) => {
                    eof = true;
                    if events.send(Event::UpstreamEof { flow_id }).await.is_err() {
                        return EgressCloseReasonV1::OwnerExit;
                    }
                }
                Ok(count) => {
                    budget -= count;
                    traffic.bytes_down = traffic.bytes_down.saturating_add(count as u64);
                    if events.send(Event::Upstream { flow_id, data: buffer[..count].to_vec() }).await.is_err() {
                        return EgressCloseReasonV1::OwnerExit;
                    }
                }
                Err(reason) => return reason,
            },
        }
    }
}

#[cfg(test)]
#[path = "channel_tests.rs"]
mod tests;
