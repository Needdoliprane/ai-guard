//! Unprivileged worker-local bridge. Files and peer UID are discovery/transport
//! guards only; the retained control pipe, outside this process, supplies worker
//! authority. No HTTP, credential, tenant, runtime invocation or audit is here.
use super::{
    FrameLimit, FrameV1, HandshakeV1, MAX_CONTROL_FRAME_BYTES, MAX_CORRELATIONS,
    MAX_REQUEST_FRAME_BYTES, MAX_RESPONSE_FRAME_BYTES, ReplyV1, RequestPayloadV1, RequestV1,
    ResponseResultV1, ResponseV1,
};
use crate::{
    broker::local_ipc::{EndpointRecordV1, PrivateListener},
    control_fs::{Directory, PrivateFile},
    relay::validation,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd},
    pin::Pin,
    task::{Context, Poll, ready},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, unix::AsyncFd},
    net::{
        UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
    time::{Instant, timeout_at},
};

const FRAME_TIME: Duration = Duration::from_secs(5);
const IDLE_TIME: Duration = Duration::from_secs(2);
const OPERATION_TIME: Duration = Duration::from_secs(45);
const RECORD_NAME: &str = "relay-v1.json";
pub const WORKER_RELAY_ROOT: &str = "/home/agent/.ai-guard-relay";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperError {
    Protocol,
    Timeout,
    Unavailable,
    UnsafeFilesystem,
    Capacity,
}
impl std::fmt::Display for HelperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.diagnostic())
    }
}
impl std::error::Error for HelperError {}
impl HelperError {
    pub fn diagnostic(self) -> &'static str {
        match self {
            Self::Protocol => "relay_helper:protocol\n",
            Self::Timeout => "relay_helper:timeout\n",
            Self::Unavailable => "relay_helper:unavailable\n",
            Self::UnsafeFilesystem => "relay_helper:unsafe_filesystem\n",
            Self::Capacity => "relay_helper:capacity\n",
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordV1 {
    version: u32,
    channel_id: String,
    epoch: u64,
    endpoint: EndpointRecordV1,
}

/// Discovery retains the immutable record FD. It supplies no tenant or agent
/// identity and never selects a credential or a control destination.
pub struct DiscoveredHelper {
    record: RecordV1,
    file: PrivateFile,
}
impl DiscoveredHelper {
    pub fn channel_id(&self) -> &str {
        &self.record.channel_id
    }
    pub fn epoch(&self) -> u64 {
        self.record.epoch
    }
    pub fn directory_name(&self) -> &str {
        self.record.endpoint.directory_name()
    }

    pub async fn connect_checked(&self, root: &Directory) -> Result<UnixStream, HelperError> {
        self.file
            .revalidate()
            .map_err(|_| HelperError::UnsafeFilesystem)?;
        let stream = self
            .record
            .endpoint
            .connect_checked(root)
            .await
            .map_err(|_| HelperError::UnsafeFilesystem)?;
        self.file
            .revalidate()
            .map_err(|_| HelperError::UnsafeFilesystem)?;
        Ok(stream)
    }
}

/// Open an explicit generation only. No scan chooses "latest" or reactivates an
/// old listener. The binary never accepts a root or namespace from its argv/env.
pub fn discover(root: &Directory, generation: &str) -> Result<DiscoveredHelper, HelperError> {
    let directory = root
        .persistent_child(generation, false)
        .map_err(|_| HelperError::UnsafeFilesystem)?
        .ok_or(HelperError::UnsafeFilesystem)?;
    let file = directory
        .persistent_read(RECORD_NAME, MAX_CONTROL_FRAME_BYTES as u64, &[])
        .map_err(|_| HelperError::UnsafeFilesystem)?
        .ok_or(HelperError::UnsafeFilesystem)?;
    let record: RecordV1 =
        validation::decode(file.bytes(), None).map_err(|_| HelperError::Protocol)?;
    super::protocol::header(record.version, &record.channel_id, record.epoch)
        .map_err(|_| HelperError::Protocol)?;
    if record.endpoint.directory_name() != generation {
        return Err(HelperError::UnsafeFilesystem);
    }
    record
        .endpoint
        .validate_at(root)
        .map_err(|_| HelperError::UnsafeFilesystem)?;
    Ok(DiscoveredHelper { record, file })
}

struct Namespace {
    root: Directory,
    listener: PrivateListener,
    record: PrivateFile,
}
impl Namespace {
    fn publish(root: Directory, hello: &HandshakeV1) -> Result<Self, HelperError> {
        let listener =
            PrivateListener::bind_new(&root).map_err(|_| HelperError::UnsafeFilesystem)?;
        let directory = root
            .persistent_child(listener.endpoint().directory_name(), false)
            .map_err(|_| HelperError::UnsafeFilesystem)?
            .ok_or(HelperError::UnsafeFilesystem)?;
        let bytes = serde_json::to_vec(&RecordV1 {
            version: 1,
            channel_id: hello.channel_id.clone(),
            epoch: hello.epoch,
            endpoint: listener.endpoint().clone(),
        })
        .map_err(|_| HelperError::Protocol)?;
        if bytes.len() > MAX_CONTROL_FRAME_BYTES {
            return Err(HelperError::Capacity);
        }
        let record = directory
            .persistent_create(RECORD_NAME, &bytes)
            .map_err(|_| HelperError::UnsafeFilesystem)?;
        let namespace = Self {
            root,
            listener,
            record,
        };
        namespace.revalidate()?;
        Ok(namespace)
    }

    fn revalidate(&self) -> Result<(), HelperError> {
        self.root
            .revalidate()
            .map_err(|_| HelperError::UnsafeFilesystem)?;
        self.record
            .revalidate()
            .map_err(|_| HelperError::UnsafeFilesystem)?;
        self.listener
            .endpoint()
            .validate_at(&self.root)
            .map_err(|_| HelperError::UnsafeFilesystem)
    }
}

fn exclusive(root: &Directory) -> Result<File, HelperError> {
    let parent = root
        .try_clone_file()
        .map_err(|_| HelperError::UnsafeFilesystem)?;
    // SAFETY: opening "." under the retained directory creates an independent
    // open-file description for flock (dup would share the original flock).
    // No pathname outside the retained directory is followed or created.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            c".".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(HelperError::UnsafeFilesystem);
    }
    // SAFETY: successful openat transfers one independently owned descriptor.
    let lease = unsafe { File::from_raw_fd(fd) };
    // SAFETY: the retained directory FD is owned here. No lock file is adopted,
    // created or overwritten. The descriptor remains owned until serve returns.
    if unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(HelperError::Unavailable);
    }
    root.revalidate()
        .map_err(|_| HelperError::UnsafeFilesystem)?;
    Ok(lease)
}

struct Application {
    input: OwnedReadHalf,
    output: OwnedWriteHalf,
    reader: AsyncFrameReader,
}
impl Application {
    fn new(stream: UnixStream) -> Self {
        let (input, output) = stream.into_split();
        Self {
            input,
            output,
            reader: AsyncFrameReader::new(),
        }
    }
}

/// One async owner and no spawned tasks: returning or dropping this future
/// closes every client, listener and supplied control IO object. Retained
/// filesystem entries remain for diagnosis; no previous generation is removed.
///
/// A request forwarded to control may already have an effect. Timeout, local
/// disconnect and EOF never establish rollback, worker death or quota release.
pub async fn serve<R, W>(root: Directory, mut input: R, mut output: W) -> Result<(), HelperError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let _lease = exclusive(&root)?;
    let mut control = AsyncFrameReader::new();
    let handshake_deadline = Instant::now() + FRAME_TIME;
    let first = timeout_at(
        handshake_deadline,
        control.read(&mut input, FrameLimit::Control),
    )
    .await
    .map_err(|_| HelperError::Timeout)??;
    let Some(first) = first else {
        return Ok(());
    };
    let FrameV1::Hello(hello) = first else {
        return Err(HelperError::Protocol);
    };
    timeout_at(
        handshake_deadline,
        write_async(&mut output, &FrameV1::Ready(hello.clone())),
    )
    .await
    .map_err(|_| HelperError::Timeout)??;
    // Neither listener nor discovery record exists before this complete write.
    let namespace = Namespace::publish(root, &hello)?;
    let mut active: Option<Application> = None;
    let mut seen = HashSet::new();
    loop {
        if seen.len() >= MAX_CORRELATIONS {
            return Err(HelperError::Capacity);
        }
        namespace.revalidate()?;
        if active.is_none() {
            tokio::select! {
                biased;
                incoming = control.read(&mut input, FrameLimit::Response) => {
                    control_end(incoming?, &hello)?;
                    return Ok(());
                }
                client = namespace.listener.accept_checked() => {
                    active = Some(Application::new(client.map_err(|_| HelperError::UnsafeFilesystem)?));
                }
            }
            continue;
        }
        let application = active.as_mut().ok_or(HelperError::Unavailable)?;
        let frame = tokio::select! {
            biased;
            incoming = control.read(&mut input, FrameLimit::Response) => {
                control_end(incoming?, &hello)?;
                return Ok(());
            }
            frame = application.reader.read(&mut application.input, FrameLimit::Request) => frame?,
            extra = namespace.listener.accept_checked() => {
                drop(extra.map_err(|_| HelperError::UnsafeFilesystem)?);
                continue;
            }
        };
        let Some(FrameV1::Request(request)) = frame else {
            active = None;
            continue;
        };
        binding(&hello, &request.channel_id, request.epoch)?;
        if !seen.insert(request.correlation_id.clone()) {
            return Err(HelperError::Protocol);
        }
        namespace.revalidate()?;
        let operation_deadline = Instant::now() + OPERATION_TIME;
        // Bounded write; a failure ends the owner without a fabricated response.
        write_async(&mut output, &FrameV1::Request(request.clone())).await?;
        let response = timeout_at(
            operation_deadline,
            await_response(
                &namespace,
                &hello,
                &request,
                &mut control,
                &mut input,
                &mut active,
            ),
        )
        .await
        .map_err(|_| HelperError::Timeout)??;
        let Some(response) = response else {
            return Ok(());
        };
        namespace.revalidate()?;
        if let Some(application) = active.as_mut() {
            let delivery = timeout_at(
                operation_deadline,
                deliver_response(
                    &namespace,
                    &hello,
                    &response,
                    &mut control,
                    &mut input,
                    application,
                ),
            )
            .await
            .map_err(|_| HelperError::Timeout)??;
            match delivery {
                Delivery::Written => {}
                Delivery::ClientClosed => active = None,
                Delivery::ControlClosed => return Ok(()),
            }
        }
    }
}

async fn await_response<R: AsyncRead + Unpin>(
    namespace: &Namespace,
    hello: &HandshakeV1,
    request: &RequestV1,
    control: &mut AsyncFrameReader,
    input: &mut R,
    active: &mut Option<Application>,
) -> Result<Option<FrameV1>, HelperError> {
    loop {
        let mut probe = [0_u8; 1];
        tokio::select! {
            biased;
            incoming = control.read(input, FrameLimit::Response) => {
                let frame = incoming?;
                match frame {
                    Some(FrameV1::Response(response)) => {
                        response_matches(hello, request, &response)?;
                        return Ok(Some(FrameV1::Response(response)));
                    }
                    end => { control_end(end, hello)?; return Ok(None); }
                }
            }
            _activity = async {
                match active.as_mut() {
                    Some(client) => { let _ = client.input.read(&mut probe).await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                // Consume at most one prefix byte, never a second JSON object.
                // Keep following the first exchange after its caller vanishes.
                *active = None;
            }
            extra = namespace.listener.accept_checked() => {
                drop(extra.map_err(|_| HelperError::UnsafeFilesystem)?);
            }
        }
    }
}

enum Delivery {
    Written,
    ClientClosed,
    ControlClosed,
}

async fn deliver_response<R: AsyncRead + Unpin>(
    namespace: &Namespace,
    hello: &HandshakeV1,
    response: &FrameV1,
    control: &mut AsyncFrameReader,
    input: &mut R,
    application: &mut Application,
) -> Result<Delivery, HelperError> {
    let writing = write_async(&mut application.output, response);
    tokio::pin!(writing);
    loop {
        let mut probe = [0_u8; 1];
        tokio::select! {
            biased;
            incoming = control.read(input, FrameLimit::Response) => {
                control_end(incoming?, hello)?;
                return Ok(Delivery::ControlClosed);
            }
            _activity = application.input.read(&mut probe) => return Ok(Delivery::ClientClosed),
            result = &mut writing => { result?; return Ok(Delivery::Written); }
            extra = namespace.listener.accept_checked() => {
                drop(extra.map_err(|_| HelperError::UnsafeFilesystem)?);
            }
        }
    }
}

fn control_end(frame: Option<FrameV1>, hello: &HandshakeV1) -> Result<(), HelperError> {
    match frame {
        None => Ok(()),
        Some(FrameV1::Close(close)) => binding(hello, &close.channel_id, close.epoch),
        _ => Err(HelperError::Protocol),
    }
}

fn binding(hello: &HandshakeV1, channel: &str, epoch: u64) -> Result<(), HelperError> {
    if channel == hello.channel_id && epoch == hello.epoch {
        Ok(())
    } else {
        Err(HelperError::Protocol)
    }
}

fn response_matches(
    hello: &HandshakeV1,
    request: &RequestV1,
    response: &ResponseV1,
) -> Result<(), HelperError> {
    binding(hello, &response.channel_id, response.epoch)?;
    if request.version != response.version
        || request.correlation_id != response.correlation_id
        || request.operation() != response.operation
    {
        return Err(HelperError::Protocol);
    }
    response.validate().map_err(|_| HelperError::Protocol)?;
    if let ResponseResultV1::Ok(reply) = &response.result {
        let result = match (&request.payload, reply) {
            (RequestPayloadV1::Send(input), ReplyV1::Send(receipt)) => {
                validation::validate_receipt(receipt, &input.operation_id, &input.operation_id)
            }
            (RequestPayloadV1::Ack(input), ReplyV1::Ack(receipt)) => {
                validation::validate_receipt(receipt, &input.operation_id, &input.message_id)
            }
            (RequestPayloadV1::Inbox(_), ReplyV1::Inbox(_)) => Ok(()),
            _ => return Err(HelperError::Protocol),
        };
        result.map_err(|_| HelperError::Protocol)?;
    }
    Ok(())
}

/// Cancellation-safe across select branches: every completed read advances the
/// retained prefix/body offsets before another await. A partial frame keeps its
/// original absolute and inactivity deadlines, not a fresh budget after wakeup.
struct AsyncFrameReader {
    prefix: [u8; 4],
    prefix_used: usize,
    body: Vec<u8>,
    body_used: usize,
    absolute: Option<Instant>,
    idle: Option<Instant>,
    closed: bool,
}
impl AsyncFrameReader {
    fn new() -> Self {
        Self {
            prefix: [0; 4],
            prefix_used: 0,
            body: Vec::new(),
            body_used: 0,
            absolute: None,
            idle: None,
            closed: false,
        }
    }

    async fn read<R: AsyncRead + Unpin>(
        &mut self,
        input: &mut R,
        limit: FrameLimit,
    ) -> Result<Option<FrameV1>, HelperError> {
        if self.closed {
            return Err(HelperError::Protocol);
        }
        let result = self.read_inner(input, limit).await;
        if !matches!(&result, Ok(Some(_))) {
            self.closed = true;
        }
        result
    }

    async fn read_inner<R: AsyncRead + Unpin>(
        &mut self,
        input: &mut R,
        limit: FrameLimit,
    ) -> Result<Option<FrameV1>, HelperError> {
        while self.prefix_used < 4 {
            let deadline = self.deadline();
            let count = read_piece(input, &mut self.prefix[self.prefix_used..], deadline).await?;
            if count == 0 {
                return if self.prefix_used == 0 {
                    Ok(None)
                } else {
                    Err(HelperError::Protocol)
                };
            }
            self.progress();
            self.prefix_used += count;
        }
        if self.body.is_empty() {
            let length = u32::from_be_bytes(self.prefix) as usize;
            let maximum = match limit {
                FrameLimit::Control => MAX_CONTROL_FRAME_BYTES,
                FrameLimit::Request => MAX_REQUEST_FRAME_BYTES,
                FrameLimit::Response => MAX_RESPONSE_FRAME_BYTES,
            };
            if length == 0 || length > maximum {
                return Err(HelperError::Protocol);
            }
            self.body
                .try_reserve_exact(length)
                .map_err(|_| HelperError::Capacity)?;
            self.body.resize(length, 0);
        }
        while self.body_used < self.body.len() {
            let deadline = self.deadline();
            let count = read_piece(input, &mut self.body[self.body_used..], deadline).await?;
            if count == 0 {
                return Err(HelperError::Protocol);
            }
            self.progress();
            self.body_used += count;
        }
        let frame = FrameV1::decode(&self.body, limit).map_err(|_| HelperError::Protocol)?;
        *self = Self::new();
        Ok(Some(frame))
    }

    fn deadline(&self) -> Option<Instant> {
        self.absolute
            .zip(self.idle)
            .map(|(absolute, idle)| absolute.min(idle))
    }
    fn progress(&mut self) {
        let now = Instant::now();
        self.absolute.get_or_insert(now + FRAME_TIME);
        self.idle = Some(now + IDLE_TIME);
    }
}

async fn read_piece<R: AsyncRead + Unpin>(
    input: &mut R,
    bytes: &mut [u8],
    deadline: Option<Instant>,
) -> Result<usize, HelperError> {
    match deadline {
        Some(deadline) => {
            if Instant::now() >= deadline {
                return Err(HelperError::Timeout);
            }
            timeout_at(deadline, input.read(bytes))
                .await
                .map_err(|_| HelperError::Timeout)?
                .map_err(|_| HelperError::Unavailable)
        }
        None => input
            .read(bytes)
            .await
            .map_err(|_| HelperError::Unavailable),
    }
}

async fn write_async<W: AsyncWrite + Unpin>(
    output: &mut W,
    frame: &FrameV1,
) -> Result<(), HelperError> {
    let absolute = Instant::now() + FRAME_TIME;
    let bytes = frame.encode().map_err(|_| HelperError::Protocol)?;
    let prefix = (bytes.len() as u32).to_be_bytes();
    for part in [&prefix[..], &bytes[..]] {
        let mut offset = 0;
        while offset < part.len() {
            let deadline = absolute.min(Instant::now() + IDLE_TIME);
            if Instant::now() >= deadline {
                return Err(HelperError::Timeout);
            }
            let count = timeout_at(deadline, output.write(&part[offset..]))
                .await
                .map_err(|_| HelperError::Timeout)?
                .map_err(|_| HelperError::Unavailable)?;
            if count == 0 {
                return Err(HelperError::Unavailable);
            }
            offset += count;
        }
    }
    timeout_at(absolute.min(Instant::now() + IDLE_TIME), output.flush())
        .await
        .map_err(|_| HelperError::Timeout)?
        .map_err(|_| HelperError::Unavailable)
}

/// A nonblocking owned pipe/socket FD, registered directly with Tokio. Unlike
/// TokioStdin, cancellation never leaves a blocking stdin task behind.
pub(crate) struct NonblockingFile(AsyncFd<File>);
impl NonblockingFile {
    pub(crate) fn new(file: File) -> Result<Self, HelperError> {
        let mut facts = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: a live owned FD and a writable stat buffer.
        if unsafe { libc::fstat(file.as_raw_fd(), facts.as_mut_ptr()) } != 0 {
            return Err(HelperError::Unavailable);
        }
        // SAFETY: successful fstat initialized the structure.
        let kind = unsafe { facts.assume_init() }.st_mode & libc::S_IFMT;
        if kind != libc::S_IFIFO && kind != libc::S_IFSOCK {
            return Err(HelperError::Protocol);
        }
        // SAFETY: fcntl queries/sets flags on this retained descriptor only.
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                != 0
        {
            return Err(HelperError::Unavailable);
        }
        // SAFETY: this owned File keeps the same valid, nonblocking FD until
        // AsyncFd deregisters and drops it. This private wrapper only borrows
        // &File for IO; it never replaces, extracts or closes a registered FD.
        unsafe { AsyncFd::register(file) }
            .map(Self)
            .map_err(|_| HelperError::Unavailable)
    }
}
impl AsyncRead for NonblockingFile {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = ready!(self.0.poll_read_ready(cx))?;
            let result = guard
                .try_io(|inner| io::Read::read(&mut inner.get_ref(), buffer.initialize_unfilled()));
            match result {
                Ok(Ok(count)) => {
                    buffer.advance(count);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(error)) => return Poll::Ready(Err(error)),
                Err(_) => continue,
            }
        }
    }
}
impl AsyncWrite for NonblockingFile {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.0.poll_write_ready(cx))?;
            match guard.try_io(|inner| io::Write::write(&mut inner.get_ref(), bytes)) {
                Ok(result) => return Poll::Ready(result),
                Err(_) => continue,
            }
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Internal binary entry point. Fixed stdin/stdout and fixed worker-home root;
/// no paths, secrets, endpoints or business commands are taken from the caller.
pub async fn serve_stdio() -> Result<(), HelperError> {
    let duplicate = |original| {
        // SAFETY: F_DUPFD_CLOEXEC returns a separately owned descriptor; the
        // standard-stream descriptor itself is never adopted or closed here.
        let fd = unsafe { libc::fcntl(original, libc::F_DUPFD_CLOEXEC, 3) };
        if fd < 0 {
            return Err(HelperError::Unavailable);
        }
        // SAFETY: a successful duplicate transfers ownership of this new FD.
        NonblockingFile::new(unsafe { File::from_raw_fd(fd) })
    };
    let input = duplicate(0)?;
    let output = duplicate(1)?;
    let root = Directory::open_or_create(std::path::Path::new(WORKER_RELAY_ROOT))
        .map_err(|_| HelperError::UnsafeFilesystem)?;
    serve(root, input, output).await
}

/// Emit one fixed code (at most 32 bytes), best effort and nonblocking. The
/// broker's separate stderr drain/cap remains its own ownership requirement.
pub fn emit_diagnostic(error: HelperError) {
    let bytes = error.diagnostic().as_bytes();
    // SAFETY: fixed stderr descriptor; no caller bytes or format string. A full
    // pipe is skipped rather than blocking shutdown behind an undrained log.
    unsafe {
        let flags = libc::fcntl(2, libc::F_GETFL);
        if flags >= 0 && libc::fcntl(2, libc::F_SETFL, flags | libc::O_NONBLOCK) == 0 {
            let _ = libc::write(2, bytes.as_ptr().cast(), bytes.len());
        }
    }
}

#[cfg(test)]
mod tests;
