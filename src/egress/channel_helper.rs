//! Worker-local, untrusted CONNECT adapter for the broker-owned egress channel.
//! The sole async owner retains every accepted socket. It spawns no tasks, does
//! no DNS, opens no upstream socket and holds no credential or control path.
//! One bounded frame/write is in flight at a time; backpressure never creates
//! an unbounded task or payload queue. Dropping the owner closes all its IO.

mod stdio;
#[cfg(test)]
mod tests;

use super::{
    Authority,
    channel_protocol::{
        self, CommonEgressChannelError, EgressFrameReader, FRAME_TIME, HANDSHAKE_TIME,
    },
    frontend::ConnectState,
    protocol::{
        Admission, DirectionV1, EgressChannelState, EgressMessageV1, EgressProtocolError,
        EgressReasonV1, MAX_DATA_BYTES,
    },
};
use std::{
    ffi::OsString,
    future::poll_fn,
    io,
    net::{Ipv4Addr, SocketAddrV4},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf},
    net::{TcpListener, TcpStream},
    time::{Instant, MissedTickBehavior, timeout_at},
};

pub use stdio::{emit_diagnostic, serve_stdio};
const MAX_CLIENTS: usize = 16;
const MAX_PENDING: usize = 4;
const MAX_HEAD_BYTES: usize = 16_384;
const HEAD_TIME: Duration = Duration::from_secs(5);
const OPEN_INTERVAL: Duration = Duration::from_millis(250);
const TICK_TIME: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommonEgressHelperError {
    Protocol,
    Timeout,
    Unavailable,
    Capacity,
}
impl CommonEgressHelperError {
    pub fn diagnostic(self) -> &'static str {
        match self {
            Self::Protocol => "egress_helper:protocol\n",
            Self::Timeout => "egress_helper:timeout\n",
            Self::Unavailable => "egress_helper:unavailable\n",
            Self::Capacity => "egress_helper:capacity\n",
        }
    }
}
impl std::fmt::Display for CommonEgressHelperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.diagnostic())
    }
}
impl std::error::Error for CommonEgressHelperError {}
impl From<CommonEgressChannelError> for CommonEgressHelperError {
    fn from(error: CommonEgressChannelError) -> Self {
        match error {
            CommonEgressChannelError::Timeout => Self::Timeout,
            CommonEgressChannelError::Capacity => Self::Capacity,
            CommonEgressChannelError::Io => Self::Unavailable,
            _ => Self::Protocol,
        }
    }
}
impl From<EgressProtocolError> for CommonEgressHelperError {
    fn from(error: EgressProtocolError) -> Self {
        match error {
            EgressProtocolError::Timeout => Self::Timeout,
            EgressProtocolError::Capacity => Self::Capacity,
            _ => Self::Protocol,
        }
    }
}
type Result<T> = std::result::Result<T, CommonEgressHelperError>;

/// The only configurable endpoint is a nonzero TCP port on IPv4 loopback.
/// It selects a local listener, never an upstream or a control destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopbackConfig {
    port: u16,
}
impl LoopbackConfig {
    pub fn new(port: u16) -> Result<Self> {
        if port == 0 {
            Err(CommonEgressHelperError::Protocol)
        } else {
            Ok(Self { port })
        }
    }
    pub fn port(self) -> u16 {
        self.port
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperMode {
    RelayV1,
    EgressV1(LoopbackConfig),
}

const EGRESS_MODE: &str = "serve-egress-v1";

/// True only for the exact egress mode token; used to select its diagnostics.
pub fn is_egress_mode(mode: &std::ffi::OsStr) -> bool {
    mode == EGRESS_MODE
}

/// Exact argv profiles only. Extra/non-UTF8 arguments and unknown modes fail
/// before creating a runtime, reading HELLO or writing any protocol stdout.
pub fn parse_mode(arguments: impl IntoIterator<Item = OsString>) -> Result<HelperMode> {
    let mut arguments = arguments.into_iter();
    let mode = arguments.next().ok_or(CommonEgressHelperError::Protocol)?;
    if mode == "serve-v1" {
        return if arguments.next().is_none() {
            Ok(HelperMode::RelayV1)
        } else {
            Err(CommonEgressHelperError::Protocol)
        };
    }
    if mode != EGRESS_MODE || arguments.next().as_deref() != Some(std::ffi::OsStr::new("--port")) {
        return Err(CommonEgressHelperError::Protocol);
    }
    Ok(HelperMode::EgressV1(LoopbackConfig::new(parse_port(
        arguments.next(),
    )?)?))
    .and_then(|mode| {
        if arguments.next().is_some() {
            Err(CommonEgressHelperError::Protocol)
        } else {
            Ok(mode)
        }
    })
}

/// Canonical decimal nonzero port: no sign, leading zero, address or suffix.
pub(crate) fn parse_port(raw: Option<OsString>) -> Result<u16> {
    let port = raw.ok_or(CommonEgressHelperError::Protocol)?;
    let raw = port.to_str().ok_or(CommonEgressHelperError::Protocol)?;
    if raw.is_empty()
        || raw.len() > 5
        || raw.starts_with('0')
        || !raw.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(CommonEgressHelperError::Protocol);
    }
    raw.parse::<u16>()
        .map_err(|_| CommonEgressHelperError::Protocol)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Head,
    Pending,
    Tunnel,
}
struct Client {
    socket: TcpStream,
    state: ConnectState,
    phase: Phase,
    head: Vec<u8>,
    head_deadline: Instant,
    flow_id: Option<u64>,
    send_credit: usize,
    read_closed: bool,
}
impl Client {
    fn new(socket: TcpStream, now: Instant, now_ms: u64) -> Result<Self> {
        let mut head = Vec::new();
        head.try_reserve_exact(MAX_HEAD_BYTES)
            .map_err(|_| CommonEgressHelperError::Capacity)?;
        Ok(Self {
            socket,
            state: ConnectState::new(now_ms),
            phase: Phase::Head,
            head,
            head_deadline: now + HEAD_TIME,
            flow_id: None,
            send_credit: 0,
            read_closed: false,
        })
    }
}

struct LocalOwner {
    clients: [Option<Client>; MAX_CLIENTS],
    ledger: EgressChannelState,
    started: Instant,
    last_flow_id: u64,
    poll_start: usize,
    next_open: Instant,
}
impl LocalOwner {
    fn new(started: Instant) -> Self {
        Self {
            clients: std::array::from_fn(|_| None),
            ledger: EgressChannelState::new(0),
            started,
            last_flow_id: 0,
            poll_start: 0,
            next_open: started,
        }
    }
    fn now_ms(&self) -> Result<u64> {
        u64::try_from(self.started.elapsed().as_millis())
            .map_err(|_| CommonEgressHelperError::Timeout)
    }
    fn flow_index(&self, id: u64) -> Result<usize> {
        self.clients
            .iter()
            .position(|client| {
                client
                    .as_ref()
                    .is_some_and(|client| client.flow_id == Some(id))
            })
            .ok_or(CommonEgressHelperError::Protocol)
    }
    fn pending_count(&self) -> usize {
        self.clients
            .iter()
            .flatten()
            .filter(|client| client.phase == Phase::Pending)
            .count()
    }
    fn has_capacity(&self) -> bool {
        self.clients.iter().any(Option::is_none)
    }
    fn accept(&mut self, socket: TcpStream) -> Result<()> {
        let index = self
            .clients
            .iter()
            .position(Option::is_none)
            .ok_or(CommonEgressHelperError::Capacity)?;
        socket
            .set_nodelay(true)
            .map_err(|_| CommonEgressHelperError::Unavailable)?;
        self.clients[index] = Some(Client::new(socket, Instant::now(), self.now_ms()?)?);
        Ok(())
    }

    /// No payload queue: each frame is fully written before another is read.
    /// One bounded message, its <=24576-byte encoded frame and the fixed
    /// <=16384-byte scratch are retained; the ledger also enforces byte credit.
    async fn emit<W: AsyncWrite + Unpin>(
        &mut self,
        output: &mut W,
        message: EgressMessageV1,
    ) -> Result<()> {
        // A frame for a flow the broker already retired is not sent.
        if self.ledger.on_worker(&message, self.now_ms()?)? == Admission::Stale {
            return Ok(());
        }
        channel_protocol::write_message(output, &message, Instant::now() + FRAME_TIME).await?;
        if let EgressMessageV1::Data {
            flow_id,
            data_b64: _,
        } = &message
        {
            let bytes = message
                .decoded_data()?
                .ok_or(CommonEgressHelperError::Protocol)?
                .len();
            self.ledger
                .release_buffer(*flow_id, DirectionV1::WorkerToBroker, bytes)?;
        }
        Ok(())
    }

    async fn cancel<W: AsyncWrite + Unpin>(&mut self, index: usize, output: &mut W) -> Result<()> {
        if let Some(id) = self.clients[index]
            .as_ref()
            .and_then(|client| client.flow_id)
        {
            self.emit(
                output,
                EgressMessageV1::Reset {
                    flow_id: id,
                    reason: EgressReasonV1::Cancelled,
                },
            )
            .await?;
        }
        self.clients[index] = None;
        Ok(())
    }

    async fn client_event<W: AsyncWrite + Unpin>(
        &mut self,
        index: usize,
        read: io::Result<usize>,
        bytes: &[u8],
        output: &mut W,
    ) -> Result<()> {
        let phase = self.clients[index]
            .as_ref()
            .ok_or(CommonEgressHelperError::Protocol)?
            .phase;
        let count = match read {
            Ok(count) => count,
            Err(_) => return self.cancel(index, output).await,
        };
        if phase == Phase::Head {
            if count == 0 {
                self.clients[index] = None;
                return Ok(());
            }
            let now_ms = self.now_ms()?;
            let client = self.clients[index]
                .as_mut()
                .ok_or(CommonEgressHelperError::Protocol)?;
            if Instant::now() >= client.head_deadline
                || client.head.len().saturating_add(count) > MAX_HEAD_BYTES
            {
                self.clients[index] = None;
                return Ok(());
            }
            client.head.extend_from_slice(&bytes[..count]);
            let authority = match client.state.receive_head(&client.head, now_ms) {
                Ok(None) => return Ok(()),
                Ok(Some(head)) => match Authority::parse_connect(head.raw_authority()) {
                    Ok(authority) => authority,
                    Err(_) => {
                        self.clients[index] = None;
                        return Ok(());
                    }
                },
                Err(_) => {
                    self.clients[index] = None;
                    return Ok(());
                }
            };
            let mut canonical = String::new();
            canonical
                .try_reserve_exact(authority.name().len() + 6)
                .map_err(|_| CommonEgressHelperError::Capacity)?;
            use std::fmt::Write as _;
            write!(&mut canonical, "{}:{}", authority.name(), authority.port())
                .map_err(|_| CommonEgressHelperError::Capacity)?;
            let flow_id = self
                .last_flow_id
                .checked_add(1)
                .ok_or(CommonEgressHelperError::Capacity)?;
            self.last_flow_id = flow_id;
            let client = self.clients[index]
                .as_mut()
                .ok_or(CommonEgressHelperError::Protocol)?;
            client.flow_id = Some(flow_id);
            client.phase = Phase::Pending;
            client.head.clear();
            self.next_open = Instant::now() + OPEN_INTERVAL;
            return self
                .emit(
                    output,
                    EgressMessageV1::Open {
                        flow_id,
                        authority: canonical,
                    },
                )
                .await;
        }
        if phase == Phase::Pending {
            // Even one premature tunnel byte is refused by cancelling the flow.
            // A concurrent late acceptance is never treated as a new flow.
            return self.cancel(index, output).await;
        }
        let now_ms = self.now_ms()?;
        let client = self.clients[index]
            .as_mut()
            .ok_or(CommonEgressHelperError::Protocol)?;
        let id = client.flow_id.ok_or(CommonEgressHelperError::Protocol)?;
        if count == 0 {
            client.read_closed = true;
            return self
                .emit(output, EgressMessageV1::HalfClose { flow_id: id })
                .await;
        }
        client
            .state
            .tunnel_data(&bytes[..count], now_ms)
            .map_err(|_| CommonEgressHelperError::Protocol)?;
        client.send_credit = client
            .send_credit
            .checked_sub(count)
            .ok_or(CommonEgressHelperError::Protocol)?;
        self.emit(output, EgressMessageV1::data(id, &bytes[..count])?)
            .await
    }

    async fn broker_event<W: AsyncWrite + Unpin>(
        &mut self,
        message: EgressMessageV1,
        output: &mut W,
    ) -> Result<()> {
        // A broker frame crossing this side's Reset names a retired flow.
        if self.ledger.on_broker(&message, self.now_ms()?)? == Admission::Stale {
            return Ok(());
        }
        let id = message.flow_id();
        let index = self.flow_index(id)?;
        let now_ms = self.now_ms()?;
        match message {
            EgressMessageV1::Accepted { .. } => {
                let client = self.clients[index]
                    .as_mut()
                    .ok_or(CommonEgressHelperError::Protocol)?;
                let response = client
                    .state
                    .broker_accepted(now_ms)
                    .map_err(|_| CommonEgressHelperError::Protocol)?;
                if write_client(&mut client.socket, response).await.is_err() {
                    return self.cancel(index, output).await;
                }
                client.phase = Phase::Tunnel;
                self.emit(
                    output,
                    EgressMessageV1::Credit {
                        flow_id: id,
                        bytes: MAX_DATA_BYTES as u32,
                    },
                )
                .await
            }
            EgressMessageV1::Rejected { .. } => {
                let mut client = self.clients[index]
                    .take()
                    .ok_or(CommonEgressHelperError::Protocol)?;
                let response = client
                    .state
                    .broker_rejected(now_ms)
                    .map_err(|_| CommonEgressHelperError::Protocol)?;
                let _ = write_client(&mut client.socket, response).await;
                Ok(())
            }
            EgressMessageV1::Credit { bytes, .. } => {
                let client = self.clients[index]
                    .as_mut()
                    .ok_or(CommonEgressHelperError::Protocol)?;
                client.send_credit = client
                    .send_credit
                    .checked_add(bytes as usize)
                    .ok_or(CommonEgressHelperError::Protocol)?;
                Ok(())
            }
            data @ EgressMessageV1::Data { .. } => {
                let bytes = data
                    .decoded_data()?
                    .ok_or(CommonEgressHelperError::Protocol)?;
                let client = self.clients[index]
                    .as_mut()
                    .ok_or(CommonEgressHelperError::Protocol)?;
                client
                    .state
                    .tunnel_data(&bytes, now_ms)
                    .map_err(|_| CommonEgressHelperError::Protocol)?;
                if write_client(&mut client.socket, &bytes).await.is_err() {
                    return self.cancel(index, output).await;
                }
                self.ledger
                    .release_buffer(id, DirectionV1::BrokerToWorker, bytes.len())?;
                self.emit(
                    output,
                    EgressMessageV1::Credit {
                        flow_id: id,
                        bytes: bytes.len() as u32,
                    },
                )
                .await
            }
            EgressMessageV1::HalfClose { .. } => {
                let client = self.clients[index]
                    .as_mut()
                    .ok_or(CommonEgressHelperError::Protocol)?;
                if !matches!(
                    timeout_at(Instant::now() + FRAME_TIME, client.socket.shutdown()).await,
                    Ok(Ok(()))
                ) {
                    return self.cancel(index, output).await;
                }
                Ok(())
            }
            EgressMessageV1::Reset { .. } | EgressMessageV1::Closed { .. } => {
                self.clients[index] = None;
                Ok(())
            }
            EgressMessageV1::Open { .. } => Err(CommonEgressHelperError::Protocol),
        }
    }

    fn poll_client(
        &mut self,
        cx: &mut Context<'_>,
        scratch: &mut [u8; MAX_DATA_BYTES],
    ) -> Poll<(usize, io::Result<usize>)> {
        let head_reads = self.pending_count() < MAX_PENDING && Instant::now() >= self.next_open;
        for offset in 0..MAX_CLIENTS {
            let index = (self.poll_start + offset) % MAX_CLIENTS;
            let Some(client) = self.clients[index].as_mut() else {
                continue;
            };
            let maximum = match client.phase {
                Phase::Head if head_reads => MAX_HEAD_BYTES
                    .saturating_sub(client.head.len())
                    .min(MAX_DATA_BYTES),
                Phase::Pending => 1,
                Phase::Tunnel if !client.read_closed => client.send_credit.min(MAX_DATA_BYTES),
                _ => 0,
            };
            if maximum == 0 {
                continue;
            }
            let mut buffer = ReadBuf::new(&mut scratch[..maximum]);
            match Pin::new(&mut client.socket).poll_read(cx, &mut buffer) {
                Poll::Ready(result) => {
                    self.poll_start = (index + 1) % MAX_CLIENTS;
                    return Poll::Ready((index, result.map(|()| buffer.filled().len())));
                }
                Poll::Pending => {}
            }
        }
        Poll::Pending
    }

    async fn tick<W: AsyncWrite + Unpin>(&mut self, output: &mut W) -> Result<()> {
        // An expired flow is cancelled alone; other flows keep the epoch.
        for flow_id in self.ledger.check_deadlines(self.now_ms()?)? {
            if let Ok(index) = self.flow_index(flow_id) {
                self.cancel(index, output).await?;
            }
        }
        let now = Instant::now();
        for client in &mut self.clients {
            if client
                .as_ref()
                .is_some_and(|client| client.phase == Phase::Head && now >= client.head_deadline)
            {
                *client = None;
            }
        }
        Ok(())
    }
}

async fn write_client(socket: &mut TcpStream, bytes: &[u8]) -> Result<()> {
    timeout_at(Instant::now() + FRAME_TIME, socket.write_all(bytes))
        .await
        .map_err(|_| CommonEgressHelperError::Timeout)?
        .map_err(|_| CommonEgressHelperError::Unavailable)
}

/// Bind only after valid HELLO, and echo READY only after a successful bind.
/// The broker still gates all admission on its independent Attached commit.
pub async fn serve<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    config: LoopbackConfig,
    mut input: R,
    output: W,
) -> Result<()> {
    let started = Instant::now();
    let deadline = started + HANDSHAKE_TIME;
    let hello = channel_protocol::read_handshake(&mut input, deadline).await?;
    let listener = timeout_at(
        deadline,
        TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, config.port)),
    )
    .await
    .map_err(|_| CommonEgressHelperError::Timeout)?
    .map_err(|_| CommonEgressHelperError::Unavailable)?;
    run(hello, started, deadline, listener, input, output).await
}

async fn run<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    hello: channel_protocol::CommonEgressHandshakeV1,
    started: Instant,
    deadline: Instant,
    listener: TcpListener,
    mut input: R,
    mut output: W,
) -> Result<()> {
    if !listener
        .local_addr()
        .map_err(|_| CommonEgressHelperError::Unavailable)?
        .ip()
        .is_loopback()
    {
        return Err(CommonEgressHelperError::Protocol);
    }
    channel_protocol::write_handshake(&mut output, &hello, deadline).await?;
    let mut owner = LocalOwner::new(started);
    owner
        .ledger
        .accept_capability(Some(&hello.capability), owner.now_ms()?)?;
    let mut reader = EgressFrameReader::new();
    let mut scratch = [0_u8; MAX_DATA_BYTES];
    let mut tick = tokio::time::interval(TICK_TIME);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            incoming = reader.read(&mut input) => {
                let Some(message) = incoming? else { return Ok(()); };
                owner.broker_event(message, &mut output).await?;
            }
            _ = tick.tick() => owner.tick(&mut output).await?,
            accepted = listener.accept(), if owner.has_capacity() => {
                let (socket, peer) = accepted.map_err(|_| CommonEgressHelperError::Unavailable)?;
                if !peer.ip().is_loopback() { drop(socket); continue; }
                owner.accept(socket)?;
            }
            (index, result) = poll_fn(|cx| owner.poll_client(cx, &mut scratch)) => {
                owner.client_event(index, result, &scratch, &mut output).await?;
            }
        }
    }
}
