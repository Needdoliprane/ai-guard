//! Worker-local helper: `ai-guard-activity serve-v1`.
//!
//! The broker starts it with a fixed non-TTY exec whose stdin/stdout carry the
//! broker side of the protocol. The helper binds the fixed private socket,
//! accepts hook clients, forwards each submission to the broker with a new
//! sequence number, routes the broker's ack back to the waiting client, and
//! reports loss counts. It holds no credential, network, HTTP, tenant, audit
//! or runtime authority; the broker owns recording and its durability.
//!
//! Resource bounds: [`MAX_PENDING_HOOKS`] connections at once (a further one is
//! refused immediately and counted), a 5 s budget to read one submission, the
//! submit frame bound of the wire codec, a 5 s budget per broker write and per
//! reply. The helper itself sets no deadline on the broker's ack; a stalled
//! broker fills the pending capacity, after which every hook is refused.
//!
//! When the broker closes stdin, waiting hooks are refused, the socket is
//! removed and the helper exits. Filesystem checks guard against other UIDs
//! only: the agent shares this UID (see the module documentation).
use super::{
    LOSS_DIRECTORY, SOCKET_NAME, WORKER_ACTIVITY_ROOT,
    hook::peer_is_self,
    wire::{
        BrokerFrameV1, ClientFrameV1, FrameReader, FrameWriter, HelloV1, HelperFrameV1, HookV1,
        LossReasonV1, LossV1, PROTOCOL_VERSION, ReplyFrameV1, ReplyV1, StatusV1, SubmitV1,
        WireError, encode_framed, write_frame,
    },
};
use crate::control_fs::Directory;
use std::{
    collections::HashMap,
    ffi::{CString, OsStr},
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{UnixListener, UnixStream},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot},
    task::JoinSet,
    time::{Instant, MissedTickBehavior, timeout, timeout_at},
};

/// Hook connections served at once, from accept until their reply.
pub const MAX_PENDING_HOOKS: usize = 16;
const HANDSHAKE_TIME: Duration = Duration::from_secs(5);
const CONNECTION_READ_TIME: Duration = Duration::from_secs(5);
const REPLY_WRITE_TIME: Duration = Duration::from_secs(5);
const SWEEP_INTERVAL: Duration = Duration::from_secs(1);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);
/// Directory entries examined per loss sweep; the rest wait for the next one.
const MAX_SWEEP_ENTRIES: usize = 4096;
// The common Linux/Darwin pathname bound, including room for a terminating NUL.
const MAX_SOCKET_PATH: usize = 103;

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
    /// Fixed stderr code, at most 32 bytes.
    pub fn diagnostic(self) -> &'static str {
        match self {
            Self::Protocol => "activity:protocol\n",
            Self::Timeout => "activity:timeout\n",
            Self::Unavailable => "activity:unavailable\n",
            Self::UnsafeFilesystem => "activity:unsafe_filesystem\n",
            Self::Capacity => "activity:capacity\n",
        }
    }
}
fn wire(error: WireError) -> HelperError {
    match error {
        WireError::Timeout => HelperError::Timeout,
        WireError::Io => HelperError::Unavailable,
        WireError::Capacity => HelperError::Capacity,
        WireError::Closed
        | WireError::PartialFrame
        | WireError::FrameSize
        | WireError::Protocol => HelperError::Protocol,
    }
}

fn effective_uid() -> u32 {
    // SAFETY: read-only process credential query.
    unsafe { libc::geteuid() }
}

/// An exclusive flock on an independent open of the root: one helper serves a
/// root at a time, and a dead helper's lease ends with its process.
fn exclusive(root: &Directory) -> Result<File, HelperError> {
    let parent = root
        .try_clone_file()
        .map_err(|_| HelperError::UnsafeFilesystem)?;
    // SAFETY: opening "." under the retained directory creates an independent
    // open-file description for flock (dup would share the original flock).
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
    // SAFETY: flock on the owned descriptor; no lock file is created.
    if unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(HelperError::Unavailable);
    }
    root.revalidate()
        .map_err(|_| HelperError::UnsafeFilesystem)?;
    Ok(lease)
}

/// No-follow facts of one entry under a retained directory; `None` for ENOENT.
fn entry_facts(directory: &File, name: &CString) -> Result<Option<libc::stat>, HelperError> {
    let mut facts = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: retained directory FD, one NUL-terminated component and a
    // writable stat buffer; AT_SYMLINK_NOFOLLOW never follows the entry.
    let result = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            facts.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            Ok(None)
        } else {
            Err(HelperError::UnsafeFilesystem)
        };
    }
    // SAFETY: successful fstatat initialized the complete structure.
    Ok(Some(unsafe { facts.assume_init() }))
}

fn own_socket(facts: &libc::stat) -> bool {
    facts.st_mode & libc::S_IFMT == libc::S_IFSOCK
        && facts.st_uid == effective_uid()
        && facts.st_nlink == 1
}

/// The fixed socket, bound only while this helper holds the root lease.
struct Endpoint {
    root: Directory,
    name: CString,
    listener: Option<UnixListener>,
    device: u64,
    inode: u64,
}
impl Endpoint {
    fn bind(root: Directory) -> Result<Self, HelperError> {
        let name = CString::new(SOCKET_NAME).map_err(|_| HelperError::Protocol)?;
        let path = root.path().join(SOCKET_NAME);
        if path.as_os_str().as_bytes().len() > MAX_SOCKET_PATH {
            return Err(HelperError::UnsafeFilesystem);
        }
        let parent = root
            .try_clone_file()
            .map_err(|_| HelperError::UnsafeFilesystem)?;
        match entry_facts(&parent, &name)? {
            None => {}
            // Under the lease, an existing own socket belongs to a dead helper.
            // Anything else is refused and left untouched for diagnosis.
            Some(facts) if own_socket(&facts) => {
                // SAFETY: retained directory FD and the fixed component only.
                if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                    return Err(HelperError::UnsafeFilesystem);
                }
            }
            Some(_) => return Err(HelperError::UnsafeFilesystem),
        }
        let listener =
            std::os::unix::net::UnixListener::bind(path).map_err(|_| HelperError::Unavailable)?;
        // SAFETY: no-follow metadata change of the fixed leaf under its retained
        // private directory; no caller pathname is eligible.
        if unsafe {
            libc::fchmodat(
                parent.as_raw_fd(),
                name.as_ptr(),
                0o600,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(HelperError::UnsafeFilesystem);
        }
        let facts = entry_facts(&parent, &name)?.ok_or(HelperError::UnsafeFilesystem)?;
        if !own_socket(&facts) || facts.st_mode & 0o7777 != 0o600 {
            return Err(HelperError::UnsafeFilesystem);
        }
        listener
            .set_nonblocking(true)
            .map_err(|_| HelperError::Unavailable)?;
        let listener = UnixListener::from_std(listener).map_err(|_| HelperError::Unavailable)?;
        #[allow(clippy::unnecessary_cast)]
        let endpoint = Self {
            root,
            name,
            listener: Some(listener),
            device: facts.st_dev as u64,
            inode: facts.st_ino as u64,
        };
        endpoint.revalidate()?;
        Ok(endpoint)
    }

    /// The root and the exact socket inode must still be in place.
    fn revalidate(&self) -> Result<(), HelperError> {
        self.root
            .revalidate()
            .map_err(|_| HelperError::UnsafeFilesystem)?;
        let parent = self
            .root
            .try_clone_file()
            .map_err(|_| HelperError::UnsafeFilesystem)?;
        let facts = entry_facts(&parent, &self.name)?.ok_or(HelperError::UnsafeFilesystem)?;
        #[allow(clippy::unnecessary_cast)]
        if !own_socket(&facts)
            || facts.st_mode & 0o7777 != 0o600
            || facts.st_dev as u64 != self.device
            || facts.st_ino as u64 != self.inode
        {
            return Err(HelperError::UnsafeFilesystem);
        }
        Ok(())
    }

    async fn accept(&self) -> std::io::Result<UnixStream> {
        match &self.listener {
            Some(listener) => listener.accept().await.map(|(stream, _)| stream),
            None => std::future::pending().await,
        }
    }

    /// Stop accepting, then remove the socket only if it is still this inode.
    fn close(&mut self) {
        self.listener = None;
        if self.revalidate().is_ok()
            && let Ok(parent) = self.root.try_clone_file()
        {
            // SAFETY: retained directory FD and the fixed component only, after
            // checking it is still the socket this helper bound.
            unsafe {
                libc::unlinkat(parent.as_raw_fd(), self.name.as_ptr(), 0);
            }
        }
    }
}

fn marker_name(name: &OsStr) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 34
        && bytes.starts_with(b"u-")
        && bytes[2..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

/// Count and remove own regular `u-<32 hex>` markers. Other entries are left
/// in place and not counted.
fn sweep(loss: &Directory) -> Result<u64, HelperError> {
    loss.revalidate()
        .map_err(|_| HelperError::UnsafeFilesystem)?;
    let parent = loss
        .try_clone_file()
        .map_err(|_| HelperError::UnsafeFilesystem)?;
    let entries = std::fs::read_dir(loss.path()).map_err(|_| HelperError::UnsafeFilesystem)?;
    let mut count = 0u64;
    for entry in entries.take(MAX_SWEEP_ENTRIES) {
        let entry = entry.map_err(|_| HelperError::UnsafeFilesystem)?;
        let file_name = entry.file_name();
        if !marker_name(&file_name) {
            continue;
        }
        let name = CString::new(file_name.as_bytes()).map_err(|_| HelperError::Protocol)?;
        let Some(facts) = entry_facts(&parent, &name)? else {
            continue;
        };
        if facts.st_mode & libc::S_IFMT != libc::S_IFREG || facts.st_uid != effective_uid() {
            continue;
        }
        // SAFETY: retained directory FD and a checked fixed-shape component.
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } == 0 {
            count += 1;
        }
    }
    loss.revalidate()
        .map_err(|_| HelperError::UnsafeFilesystem)?;
    Ok(count)
}

enum Event {
    Submitted {
        submit: SubmitV1,
        reply: oneshot::Sender<StatusV1>,
    },
    Loss(LossReasonV1),
}

#[derive(Default)]
struct Losses(HashMap<LossReasonV1, u64>);
impl Losses {
    fn add(&mut self, reason: LossReasonV1, count: u64) {
        if count > 0 {
            let entry = self.0.entry(reason).or_default();
            *entry = entry.saturating_add(count);
        }
    }
}

/// Broker-side state of the single owner task.
struct Stream {
    seq: u64,
    pending: HashMap<u64, oneshot::Sender<StatusV1>>,
    losses: Losses,
    writer: FrameWriter,
}
impl Stream {
    fn next_seq(&mut self) -> Result<u64, HelperError> {
        self.seq = self.seq.checked_add(1).ok_or(HelperError::Capacity)?;
        Ok(self.seq)
    }

    async fn forward<W: AsyncWrite + Unpin>(
        &mut self,
        output: &mut W,
        submit: SubmitV1,
        reply: oneshot::Sender<StatusV1>,
    ) -> Result<(), HelperError> {
        let seq = self.next_seq()?;
        self.writer
            .write(output, &HelperFrameV1::Hook(HookV1 { seq, submit }))
            .await
            .map_err(wire)?;
        self.pending.insert(seq, reply);
        Ok(())
    }

    /// Report every non-zero counter, in the fixed reason order.
    async fn flush<W: AsyncWrite + Unpin>(&mut self, output: &mut W) -> Result<(), HelperError> {
        for reason in LossReasonV1::ALL {
            let count = self.losses.0.get(&reason).copied().unwrap_or(0);
            if count == 0 {
                continue;
            }
            let seq = self.next_seq()?;
            self.writer
                .write(output, &HelperFrameV1::Loss(LossV1 { seq, count, reason }))
                .await
                .map_err(wire)?;
            self.losses.0.remove(&reason);
        }
        Ok(())
    }
}

async fn reply(stream: &mut UnixStream, status: StatusV1) {
    let frame = ReplyFrameV1::Reply(ReplyV1 {
        version: PROTOCOL_VERSION,
        status,
    });
    let _ = timeout(REPLY_WRITE_TIME, write_frame(stream, &frame)).await;
}

/// One hook connection: peer check, one bounded submission, then its reply.
async fn connection(
    mut stream: UnixStream,
    _permit: OwnedSemaphorePermit,
    events: mpsc::Sender<Event>,
) {
    if !peer_is_self(&stream) {
        let _ = events.send(Event::Loss(LossReasonV1::PeerRefused)).await;
        return;
    }
    let deadline = Instant::now() + CONNECTION_READ_TIME;
    let mut reader = FrameReader::<ClientFrameV1>::new();
    let submit = match timeout_at(deadline, reader.read(&mut stream)).await {
        Ok(Ok(Some(ClientFrameV1::Submit(submit)))) => submit,
        Ok(Err(WireError::FrameSize)) => {
            let _ = events.send(Event::Loss(LossReasonV1::OversizedFrame)).await;
            reply(&mut stream, StatusV1::Refused).await;
            return;
        }
        _ => {
            let _ = events.send(Event::Loss(LossReasonV1::InvalidFrame)).await;
            reply(&mut stream, StatusV1::Refused).await;
            return;
        }
    };
    let (sender, receiver) = oneshot::channel();
    let status = match events
        .send(Event::Submitted {
            submit,
            reply: sender,
        })
        .await
    {
        Ok(()) => receiver.await.unwrap_or(StatusV1::Refused),
        Err(_) => StatusV1::Refused,
    };
    reply(&mut stream, status).await;
}

/// Serve one broker channel on `input`/`output` with the given private root.
///
/// Returns `Ok` when the broker closes its side, before or after `ready`.
/// Every error ends the channel: no frame is resynchronized and no waiting
/// hook is reported recorded. Dropping this future aborts every connection.
pub async fn serve<R, W>(root: Directory, mut input: R, mut output: W) -> Result<(), HelperError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let _lease = exclusive(&root)?;
    let mut reader = FrameReader::<BrokerFrameV1>::new();
    let mut stream = Stream {
        seq: 0,
        pending: HashMap::new(),
        losses: Losses::default(),
        writer: FrameWriter::new(),
    };
    let handshake = Instant::now() + HANDSHAKE_TIME;
    let hello = HelperFrameV1::Hello(HelloV1 {
        version: PROTOCOL_VERSION,
    });
    timeout_at(handshake, stream.writer.write(&mut output, &hello))
        .await
        .map_err(|_| HelperError::Timeout)?
        .map_err(wire)?;
    match timeout_at(handshake, reader.read(&mut input))
        .await
        .map_err(|_| HelperError::Timeout)?
        .map_err(wire)?
    {
        None => return Ok(()),
        Some(BrokerFrameV1::Ready(_)) => {}
        Some(BrokerFrameV1::Ack(_)) => return Err(HelperError::Protocol),
    }
    // Neither the loss directory nor the socket exists before `ready`.
    let loss = root
        .child(LOSS_DIRECTORY)
        .map_err(|_| HelperError::UnsafeFilesystem)?;
    let mut endpoint = Endpoint::bind(root)?;
    let result = serve_open(
        &mut endpoint,
        &loss,
        &mut reader,
        &mut stream,
        &mut input,
        &mut output,
    )
    .await;
    endpoint.close();
    result
}

async fn serve_open<R, W>(
    endpoint: &mut Endpoint,
    loss: &Directory,
    reader: &mut FrameReader<BrokerFrameV1>,
    stream: &mut Stream,
    input: &mut R,
    output: &mut W,
) -> Result<(), HelperError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let refused = encode_framed(&ReplyFrameV1::Reply(ReplyV1 {
        version: PROTOCOL_VERSION,
        status: StatusV1::Refused,
    }))
    .map_err(wire)?;
    stream
        .losses
        .add(LossReasonV1::ClientUnrecorded, sweep(loss)?);
    stream.flush(output).await?;
    let permits = Arc::new(Semaphore::new(MAX_PENDING_HOOKS));
    let (sender, mut events) = mpsc::channel::<Event>(MAX_PENDING_HOOKS * 2);
    let mut tasks = JoinSet::new();
    let mut tick = tokio::time::interval_at(Instant::now() + SWEEP_INTERVAL, SWEEP_INTERVAL);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let result = loop {
        tokio::select! {
            biased;
            frame = reader.read(input) => match frame {
                Ok(Some(BrokerFrameV1::Ack(ack))) => match stream.pending.remove(&ack.seq) {
                    Some(waiting) => {
                        let _ = waiting.send(ack.status);
                    }
                    // An ack for a loss, an unissued or an already acked seq.
                    None => break Err(HelperError::Protocol),
                },
                Ok(Some(BrokerFrameV1::Ready(_))) => break Err(HelperError::Protocol),
                Ok(None) => break Ok(()),
                Err(error) => break Err(wire(error)),
            },
            event = events.recv() => match event {
                Some(Event::Submitted { submit, reply }) => {
                    if let Err(error) = stream.forward(output, submit, reply).await {
                        break Err(error);
                    }
                }
                Some(Event::Loss(reason)) => stream.losses.add(reason, 1),
                None => break Err(HelperError::Unavailable),
            },
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            accepted = endpoint.accept() => match accepted {
                Ok(client) => match Arc::clone(&permits).try_acquire_owned() {
                    Ok(permit) => {
                        tasks.spawn(connection(client, permit, sender.clone()));
                    }
                    Err(_) => {
                        // A small reply into an empty socket buffer; never waits.
                        let _ = client.try_write(&refused);
                        stream.losses.add(LossReasonV1::HelperCapacity, 1);
                    }
                },
                Err(_) => break Err(HelperError::Unavailable),
            },
            _ = tick.tick() => {
                if let Err(error) = endpoint.revalidate() {
                    break Err(error);
                }
                match sweep(loss) {
                    Ok(count) => stream.losses.add(LossReasonV1::ClientUnrecorded, count),
                    Err(error) => break Err(error),
                }
                if let Err(error) = stream.flush(output).await {
                    break Err(error);
                }
            }
        }
    };
    // From here no hook is recorded: stop accepting and refuse every waiter.
    endpoint.close();
    for (_, waiting) in stream.pending.drain() {
        let _ = waiting.send(StatusV1::Refused);
    }
    drop(sender);
    events.close();
    while let Ok(event) = events.try_recv() {
        match event {
            Event::Submitted { reply, .. } => {
                let _ = reply.send(StatusV1::Refused);
            }
            Event::Loss(reason) => stream.losses.add(reason, 1),
        }
    }
    let _ = timeout(SHUTDOWN_GRACE, async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    tasks.abort_all();
    if result.is_ok() {
        // Best effort: the broker may still drain the final counts.
        if let Ok(count) = sweep(loss) {
            stream.losses.add(LossReasonV1::ClientUnrecorded, count);
        }
        let _ = stream.flush(output).await;
    }
    result
}

/// Internal binary entry point: fixed stdin/stdout and the fixed worker root;
/// no path, secret, endpoint or command comes from the caller.
pub async fn serve_stdio() -> Result<(), HelperError> {
    use crate::relay::channel::helper::NonblockingFile;
    let duplicate = |original| {
        // SAFETY: F_DUPFD_CLOEXEC returns a separately owned descriptor; the
        // standard-stream descriptor itself is never adopted or closed here.
        let fd = unsafe { libc::fcntl(original, libc::F_DUPFD_CLOEXEC, 3) };
        if fd < 0 {
            return Err(HelperError::Unavailable);
        }
        // SAFETY: a successful duplicate transfers ownership of this new FD.
        NonblockingFile::new(unsafe { File::from_raw_fd(fd) }).map_err(|_| HelperError::Unavailable)
    };
    let input = duplicate(0)?;
    let output = duplicate(1)?;
    let root = Directory::open_or_create(Path::new(WORKER_ACTIVITY_ROOT))
        .map_err(|_| HelperError::UnsafeFilesystem)?;
    serve(root, input, output).await
}

/// Emit one fixed code (at most 32 bytes), best effort and nonblocking.
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

/// Binary entry point of `serve-v1`; returns the process exit code.
pub fn main_v1() -> i32 {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let result = match runtime {
        Ok(runtime) => runtime.block_on(serve_stdio()),
        Err(_) => Err(HelperError::Unavailable),
    };
    match result {
        Ok(()) => 0,
        Err(error) => {
            emit_diagnostic(error);
            126
        }
    }
}
