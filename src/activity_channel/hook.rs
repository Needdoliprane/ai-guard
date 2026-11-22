//! Short-lived hook client: `ai-guard-activity hook-v1 <client> <event>`.
//!
//! The provider runs it from its managed hook configuration and hands it the
//! hook input on stdin. The client keeps at most [`MAX_PAYLOAD_BYTES`] of that
//! input, counts the rest within a bounded drain, submits one frame to the
//! worker-local helper and waits for its reply.
//!
//! Exit contract (stdout is never written; no permission decision is printed):
//! - gate events (`PreToolUse`, `UserPromptSubmit`): exit 0 only when the
//!   reply is `recorded`; otherwise exit 2 with one fixed stderr line, so the
//!   provider blocks the action;
//! - other events: always exit 0 (exit 2 would change provider behaviour, for
//!   example keep a `Stop` hook running); an unrecorded submission leaves an
//!   empty loss marker for the helper to count.
//!
//! No environment variable is read or forwarded: the provider environment can
//! hold API keys. The root path is fixed in production and injectable only
//! through the crate-internal [`HookSettings`].
use super::{
    LOSS_DIRECTORY, SOCKET_NAME, WORKER_ACTIVITY_ROOT,
    wire::{
        ClientFrameV1, ClientV1, EventV1, FrameReader, FrameWriter, MAX_PAYLOAD_BYTES,
        PROTOCOL_VERSION, ReplyFrameV1, StatusV1, SubmitV1, bounded_payload,
    },
};
use crate::control_fs::Directory;
use std::{
    ffi::{CString, OsString},
    fs::File,
    io::{ErrorKind, Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::{
    net::UnixStream,
    time::{Instant, timeout_at},
};

/// The only text this client ever writes, to stderr, for a blocked gate.
pub const BLOCKED_LINE: &[u8] = b"ai-guard: activity audit unavailable; action blocked\n";
pub const EXIT_CONTINUE: i32 = 0;
/// The provider's blocking exit code for hook commands.
pub const EXIT_BLOCK: i32 = 2;
/// An unusable invocation that names no gate event.
pub const EXIT_USAGE: i32 = 64;
/// Total time of a gate invocation, including connection retries. It stays
/// below the 30 s command timeout of the managed hook configuration.
pub const GATE_DEADLINE: Duration = Duration::from_secs(10);
/// Total time of a non-gate invocation.
pub const OBSERVE_DEADLINE: Duration = Duration::from_secs(3);
/// Stdin drain budget inside the total deadline.
pub const STDIN_DRAIN: Duration = Duration::from_secs(2);
const CONNECT_RETRY: Duration = Duration::from_millis(100);
const READ_CHUNK: usize = 64 * 1024;
const STDIN_THREAD_STACK: usize = 256 * 1024;

/// Crate-internal parameters; production uses [`HookSettings::production`].
#[derive(Clone, Debug)]
pub(crate) struct HookSettings {
    pub(crate) root: PathBuf,
    pub(crate) gate_deadline: Duration,
    pub(crate) observe_deadline: Duration,
    pub(crate) stdin_drain: Duration,
}
impl HookSettings {
    pub(crate) fn production() -> Self {
        Self {
            root: PathBuf::from(WORKER_ACTIVITY_ROOT),
            gate_deadline: GATE_DEADLINE,
            observe_deadline: OBSERVE_DEADLINE,
            stdin_drain: STDIN_DRAIN,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Invocation {
    Valid {
        client: ClientV1,
        event: EventV1,
    },
    /// Unusable arguments; `gate` when they still name a gate event.
    Invalid {
        gate: bool,
    },
}

/// `arguments` follow `hook-v1`: exactly `<client> <event>`.
pub(crate) fn parse(arguments: &[OsString]) -> Invocation {
    let event = arguments
        .get(1)
        .and_then(|value| value.to_str())
        .and_then(EventV1::parse);
    let gate = event.is_some_and(EventV1::is_gate);
    if arguments.len() != 2 {
        return Invocation::Invalid { gate };
    }
    match (arguments[0].to_str().and_then(ClientV1::parse), event) {
        (Some(client), Some(event)) if client.supports(event) => {
            Invocation::Valid { client, event }
        }
        _ => Invocation::Invalid { gate },
    }
}

/// Binary entry point with the fixed root and real standard streams.
pub fn main_v1(arguments: &[OsString]) -> i32 {
    run(
        &HookSettings::production(),
        arguments,
        std::io::stdin(),
        &mut std::io::stderr(),
    )
}

pub(crate) fn run<R: Read + Send + 'static>(
    settings: &HookSettings,
    arguments: &[OsString],
    stdin: R,
    stderr: &mut dyn Write,
) -> i32 {
    let (client, event) = match parse(arguments) {
        Invocation::Valid { client, event } => (client, event),
        Invocation::Invalid { gate: true } => return blocked(stderr),
        Invocation::Invalid { gate: false } => return EXIT_USAGE,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let status = match runtime {
        Ok(runtime) => runtime.block_on(submit(settings, client, event, stdin)),
        Err(_) => None,
    };
    conclude(settings, event, status, stderr)
}

/// Same contract as [`run`] for a caller already inside a Tokio runtime.
#[cfg(test)]
pub(crate) async fn run_async<R: Read + Send + 'static>(
    settings: &HookSettings,
    arguments: &[OsString],
    stdin: R,
    stderr: &mut (dyn Write + Send),
) -> i32 {
    let (client, event) = match parse(arguments) {
        Invocation::Valid { client, event } => (client, event),
        Invocation::Invalid { gate: true } => return blocked(stderr),
        Invocation::Invalid { gate: false } => return EXIT_USAGE,
    };
    let status = submit(settings, client, event, stdin).await;
    conclude(settings, event, status, stderr)
}

fn conclude(
    settings: &HookSettings,
    event: EventV1,
    status: Option<StatusV1>,
    stderr: &mut dyn Write,
) -> i32 {
    if status == Some(StatusV1::Recorded) {
        EXIT_CONTINUE
    } else if event.is_gate() {
        blocked(stderr)
    } else {
        leave_loss_marker(&settings.root);
        EXIT_CONTINUE
    }
}

fn blocked(stderr: &mut dyn Write) -> i32 {
    let _ = stderr.write_all(BLOCKED_LINE);
    let _ = stderr.flush();
    EXIT_BLOCK
}

async fn submit<R: Read + Send + 'static>(
    settings: &HookSettings,
    client: ClientV1,
    event: EventV1,
    stdin: R,
) -> Option<StatusV1> {
    let started = Instant::now();
    let total = if event.is_gate() {
        settings.gate_deadline
    } else {
        settings.observe_deadline
    };
    let deadline = started + total;
    let captured = drain(stdin, (started + settings.stdin_drain).min(deadline)).await;
    let submission = submission(client, event, &captured);
    exchange(&settings.root, submission, deadline).await
}

pub(crate) struct Captured {
    pub(crate) kept: Vec<u8>,
    pub(crate) total: u64,
    /// EOF was reached before the drain deadline without a read error.
    pub(crate) complete: bool,
}

#[derive(Default)]
struct Drain {
    kept: Vec<u8>,
    total: u64,
    complete: bool,
}

fn lock(shared: &Mutex<Drain>) -> MutexGuard<'_, Drain> {
    match shared.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Read stdin on a dedicated thread: a blocking pipe read cannot be cancelled,
/// so the client stops waiting at the deadline and the thread ends with the
/// process. Only the first [`MAX_PAYLOAD_BYTES`] are kept; the rest is counted.
async fn drain<R: Read + Send + 'static>(mut input: R, deadline: Instant) -> Captured {
    let shared = Arc::new(Mutex::new(Drain::default()));
    let (done, finished) = tokio::sync::oneshot::channel::<()>();
    let worker = Arc::clone(&shared);
    // An explicit stack size: std then never consults RUST_MIN_STACK.
    let spawned = std::thread::Builder::new()
        .stack_size(STDIN_THREAD_STACK)
        .spawn(move || {
            let mut chunk = vec![0u8; READ_CHUNK];
            loop {
                match input.read(&mut chunk) {
                    Ok(0) => {
                        lock(&worker).complete = true;
                        break;
                    }
                    Ok(count) => {
                        let mut state = lock(&worker);
                        state.total = state.total.saturating_add(count as u64);
                        let room = MAX_PAYLOAD_BYTES - state.kept.len();
                        let kept = count.min(room);
                        state.kept.extend_from_slice(&chunk[..kept]);
                    }
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            let _ = done.send(());
        });
    if spawned.is_ok() {
        let _ = timeout_at(deadline, finished).await;
    }
    let mut state = lock(&shared);
    Captured {
        kept: std::mem::take(&mut state.kept),
        total: state.total,
        complete: state.complete,
    }
}

/// Build the submission without parsing the provider JSON. A UTF-8 sequence
/// cut by the byte bound or the deadline is dropped from the end; any other
/// invalid UTF-8 yields an empty payload and the explicit flag.
pub(crate) fn submission(client: ClientV1, event: EventV1, captured: &Captured) -> SubmitV1 {
    let partial = !captured.complete || captured.total > captured.kept.len() as u64;
    let (text, invalid, utf8_cut) = match std::str::from_utf8(&captured.kept) {
        Ok(text) => (text, false, false),
        Err(error) if partial && error.error_len().is_none() => (
            std::str::from_utf8(&captured.kept[..error.valid_up_to()]).unwrap_or_default(),
            false,
            true,
        ),
        Err(_) => ("", true, false),
    };
    let (payload, escape_cut) = bounded_payload(text);
    SubmitV1 {
        version: PROTOCOL_VERSION,
        client,
        event,
        gate: event.is_gate(),
        stdin_total_bytes: captured.total,
        stdin_truncated: partial || utf8_cut || escape_cut,
        payload_invalid_utf8: invalid,
        payload: payload.to_owned(),
    }
}

/// One submit/reply exchange. Every failure is `None`: absent or foreign
/// socket, deadline, refused, closed or malformed reply.
async fn exchange(root: &Path, submission: SubmitV1, deadline: Instant) -> Option<StatusV1> {
    let frame = ClientFrameV1::Submit(submission);
    let mut stream = connect(root, deadline).await?;
    let mut writer = FrameWriter::new();
    timeout_at(deadline, writer.write(&mut stream, &frame))
        .await
        .ok()?
        .ok()?;
    let mut reader = FrameReader::<ReplyFrameV1>::new();
    match timeout_at(deadline, reader.read(&mut stream)).await {
        Ok(Ok(Some(ReplyFrameV1::Reply(reply)))) => Some(reply.status),
        _ => None,
    }
}

/// Retry until the deadline while the private root or its socket is absent or
/// refuses connections. A connected peer with another UID ends the attempt.
async fn connect(root: &Path, deadline: Instant) -> Option<UnixStream> {
    loop {
        if Instant::now() >= deadline {
            return None;
        }
        if let Ok(directory) = Directory::open_existing(root) {
            let path = directory.path().join(SOCKET_NAME);
            if let Ok(Ok(stream)) = timeout_at(deadline, UnixStream::connect(path)).await {
                return peer_is_self(&stream).then_some(stream);
            }
        }
        let pause = (Instant::now() + CONNECT_RETRY).min(deadline);
        tokio::time::sleep_until(pause).await;
    }
}

pub(crate) fn peer_is_self(stream: &UnixStream) -> bool {
    // SAFETY: read-only process credential query.
    let own = unsafe { libc::geteuid() };
    stream
        .peer_cred()
        .is_ok_and(|credentials| credentials.uid() == own)
}

/// Best effort: an empty `u-<32 hex>` file created exclusively, no-follow and
/// 0600 in the private loss directory. Failure leaves nothing to clean up.
pub(crate) fn leave_loss_marker(root: &Path) {
    let Ok(root) = Directory::open_or_create(root) else {
        return;
    };
    let Ok(loss) = root.child(LOSS_DIRECTORY) else {
        return;
    };
    let Ok(parent) = loss.try_clone_file() else {
        return;
    };
    let Ok(name) = CString::new(format!("u-{}", uuid::Uuid::new_v4().simple())) else {
        return;
    };
    // SAFETY: retained checked private directory FD, one fixed-shape component
    // and an exclusive no-follow creation. The descriptor is immediately owned.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd >= 0 {
        // SAFETY: a successful openat transfers ownership of one live FD.
        drop(unsafe { File::from_raw_fd(fd) });
    }
}
