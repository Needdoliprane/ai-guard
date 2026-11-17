//! Physical batch CLI ownership only. Never a worker-terminal or stop proof.
//!
//! The independent task owns Child, both capture futures and the archive writer
//! before its first await. Internal deadlines record errors; they never discard
//! an unfinished future or manufacture EOF. The caller retains this observer and
//! calls collect even after a fault. External observer/runtime cancellation is
//! outside this local collection contract; dropping a task handle is not a join.
use super::Capture;
use std::{fmt, fs::File, future::pending, io, process::ExitStatus, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, ChildStdin},
    sync::{mpsc, watch},
    task::{JoinError, JoinHandle},
    time::Instant,
};

const ATTACH_DRAIN: Duration = Duration::from_secs(10);
const WRITER_DRAIN: Duration = Duration::from_secs(5);
const CAPTURE_DRAIN: Duration = Duration::from_secs(10);
const STDERR_LIMIT: usize = 256 * 1024;

pub(super) enum BatchIoEvent {
    Exited(Result<ExitStatus, Arc<io::Error>>),
    Fault,
    /// The event producer finished. Only collect supplies its actual outcome.
    OwnerFinished,
}
impl fmt::Debug for BatchIoEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exited(Ok(status)) => f.debug_tuple("Exited").field(status).finish(),
            Self::Exited(Err(_)) => f.write_str("Exited(wait unavailable)"),
            Self::Fault => f.write_str("Fault"),
            Self::OwnerFinished => f.write_str("OwnerFinished"),
        }
    }
}

pub(super) struct BatchIoCollected {
    pub(super) stdout: Capture,
    pub(super) stderr: Capture,
    pub(super) stdout_eof: bool,
    pub(super) stderr_eof: bool,
    pub(super) status: Option<ExitStatus>,
    pub(super) errors: Vec<anyhow::Error>,
    /// The original archive FD, including on IO success. Only the caller knows
    /// whether its callback/native/global result permits releasing this owner.
    /// Never a reopened path.
    pub(super) retained_input: Option<File>,
}
impl fmt::Debug for BatchIoCollected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchIoCollected")
            .field("stdout_bytes", &self.stdout.bytes.len())
            .field("stdout_total_bytes", &self.stdout.total_bytes)
            .field("stdout_truncated", &self.stdout.truncated)
            .field("stdout_eof", &self.stdout_eof)
            .field("stderr_bytes", &self.stderr.bytes.len())
            .field("stderr_total_bytes", &self.stderr.total_bytes)
            .field("stderr_truncated", &self.stderr.truncated)
            .field("stderr_eof", &self.stderr_eof)
            .field("status", &self.status)
            .field("error_count", &self.errors.len())
            .field("input_retained", &self.retained_input.is_some())
            .finish()
    }
}

pub(super) enum BatchIoCollectError {
    NoRuntime,
    AlreadyCollected,
    JoinFailed(JoinError),
}
impl fmt::Display for BatchIoCollectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoRuntime => "batch IO owner needs a Tokio runtime; original resources retained",
            Self::AlreadyCollected => "batch IO outcome was already collected",
            Self::JoinFailed(_) => "batch IO owner failed; no collected outcome is established",
        })
    }
}
impl fmt::Debug for BatchIoCollectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl std::error::Error for BatchIoCollectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::JoinFailed(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Requests {
    drain_started: Option<Instant>,
    kill_transport: bool,
}
struct Input {
    child: Child,
    archive: Option<File>,
    stdout_limit: usize,
    stderr_limit: usize,
    events: mpsc::Sender<BatchIoEvent>,
    requests: watch::Receiver<Requests>,
    observation: Option<super::observation::Scope>,
    cli_observation: Option<super::observation::Scope>,
}

/// NonClone observer. Its JoinHandle remains here across cancelled collect waits.
/// No callback, runtime authority, worker ID or engine command is accepted here.
pub(super) struct OwnedBatchIo {
    unstarted: Option<Input>,
    task: Option<JoinHandle<BatchIoCollected>>,
    events: mpsc::Receiver<BatchIoEvent>,
    requests: watch::Sender<Requests>,
    collected: bool,
}
impl fmt::Debug for OwnedBatchIo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedBatchIo")
            .field("unstarted", &self.unstarted.is_some())
            .field("task_retained", &self.task.is_some())
            .field("collected", &self.collected)
            .finish()
    }
}
impl OwnedBatchIo {
    /// Infallible transfer. Without a current runtime, Child and archive remain
    /// in unstarted until an async observer can start the same owner task.
    pub(super) fn own(child: Child, input: Option<File>, stdout_limit: usize) -> Self {
        Self::own_with_input(child, input, stdout_limit, STDERR_LIMIT, None, None)
    }
    /// The same physical owner for an ordinary control CLI command. The caller
    /// supplies its existing capture limits (256/64 KiB for capture_cli).
    pub(super) fn own_cli(child: Child, stdout_limit: usize, stderr_limit: usize) -> Self {
        Self::own_with_input(child, None, stdout_limit, stderr_limit, None, None)
    }
    pub(super) fn own_cli_observed(
        child: Child,
        stdout_limit: usize,
        stderr_limit: usize,
        observation: Option<super::observation::Scope>,
    ) -> Self {
        if observation.is_none() {
            Self::own_cli(child, stdout_limit, stderr_limit)
        } else {
            Self::own_with_input(child, None, stdout_limit, stderr_limit, None, observation)
        }
    }
    pub(super) fn own_observed(
        child: Child,
        input: Option<File>,
        stdout_limit: usize,
        observation: Option<super::observation::Scope>,
    ) -> Self {
        Self::own_with_input(child, input, stdout_limit, STDERR_LIMIT, observation, None)
    }
    fn own_with_input(
        child: Child,
        input: Option<File>,
        stdout_limit: usize,
        stderr_limit: usize,
        observation: Option<super::observation::Scope>,
        cli_observation: Option<super::observation::Scope>,
    ) -> Self {
        // At most one coalesced Fault and either a real exit or a wait error.
        // No notification send may block the physical owner.
        let (send, events) = mpsc::channel(4);
        let (requests, receive) = watch::channel(Requests::default());
        let mut owned = Self {
            unstarted: Some(Input {
                child,
                archive: input,
                stdout_limit,
                stderr_limit,
                events: send,
                requests: receive,
                observation,
                cli_observation,
            }),
            task: None,
            events,
            requests,
            collected: false,
        };
        let _ = owned.start_if_possible();
        owned
    }
    fn start_if_possible(&mut self) -> Result<(), BatchIoCollectError> {
        if self.collected {
            return Err(BatchIoCollectError::AlreadyCollected);
        }
        if self.task.is_none() {
            let runtime = tokio::runtime::Handle::try_current()
                .map_err(|_| BatchIoCollectError::NoRuntime)?;
            if let Some(input) = self.unstarted.take() {
                self.task = Some(runtime.spawn(run(input)));
            }
        }
        Ok(())
    }
    pub(super) async fn wait_event(&mut self) -> BatchIoEvent {
        if self.collected {
            return BatchIoEvent::OwnerFinished;
        }
        if self.start_if_possible().is_err() {
            return BatchIoEvent::Fault;
        }
        self.events
            .recv()
            .await
            .unwrap_or(BatchIoEvent::OwnerFinished)
    }
    /// Arm only once, at the original call Instant. Does not certify native STOP.
    pub(super) fn begin_drain(&mut self) {
        self.requests.send_modify(|request| {
            if request.drain_started.is_none() {
                request.drain_started = Some(Instant::now());
            }
        });
    }
    /// Requests termination of the CLI Child only. Never issues an engine kill.
    /// A prior uncertain wait permanently forbids using its PID again.
    pub(super) fn request_transport_kill(&mut self) {
        self.requests
            .send_modify(|request| request.kill_transport = true);
    }
    pub(super) async fn collect(&mut self) -> Result<BatchIoCollected, BatchIoCollectError> {
        self.start_if_possible()?;
        let joined = match self.task.as_mut() {
            Some(task) => task.await,
            None => return Err(BatchIoCollectError::AlreadyCollected),
        };
        // No await between observing Ready and removing the completed handle.
        self.task.take();
        self.collected = true;
        joined.map_err(BatchIoCollectError::JoinFailed)
    }
}

struct ReadCapture {
    capture: Capture,
    eof: bool,
    error: Option<io::Error>,
}
async fn capture<R: AsyncRead + Unpin>(reader: Option<R>, limit: usize) -> ReadCapture {
    let mut capture = Capture {
        bytes: Vec::with_capacity(limit.min(16 * 1024)),
        total_bytes: 0,
        truncated: false,
    };
    let Some(mut reader) = reader else {
        return ReadCapture {
            capture,
            eof: false,
            error: Some(io::Error::other("batch capture pipe missing")),
        };
    };
    let mut buffer = [0; 16 * 1024];
    loop {
        let count = match reader.read(&mut buffer).await {
            Ok(0) => {
                return ReadCapture {
                    capture,
                    eof: true,
                    error: None,
                };
            }
            Ok(count) => count,
            Err(error) => {
                return ReadCapture {
                    capture,
                    eof: false,
                    error: Some(error),
                };
            }
        };
        capture.total_bytes = capture.total_bytes.saturating_add(count as u64);
        let retained = count.min(limit.saturating_sub(capture.bytes.len()));
        capture.bytes.extend_from_slice(&buffer[..retained]);
        capture.truncated |= retained < count;
    }
}
async fn write_archive(
    source: io::Result<Option<File>>,
    sink: Option<ChildStdin>,
) -> io::Result<()> {
    let Some(source) = source? else {
        drop(sink);
        return Ok(());
    };
    let mut source = tokio::fs::File::from_std(source);
    let Some(mut sink) = sink else {
        // Join any file work before releasing the cloned descriptor.
        drop(source.into_std().await);
        return Err(io::Error::other("archive stdin missing"));
    };
    let copied = tokio::io::copy(&mut source, &mut sink).await;
    drop(sink);
    // into_std waits for any outstanding Tokio file operation. The original
    // archive FD is independently retained by run and returned in its outcome.
    drop(source.into_std().await);
    copied.map(|_| ())
}
async fn until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => pending::<()>().await,
    }
}
fn fault(events: &mpsc::Sender<BatchIoEvent>, sent: &mut bool) {
    if !*sent {
        *sent = true;
        let _ = events.try_send(BatchIoEvent::Fault);
    }
}
fn transport_kill(
    child: &mut Child,
    requested: &mut bool,
    errors: &mut Vec<anyhow::Error>,
    events: &mpsc::Sender<BatchIoEvent>,
    fault_sent: &mut bool,
) {
    if *requested {
        return;
    }
    *requested = true;
    if let Err(error) = child.start_kill() {
        errors.push(anyhow::Error::new(error).context("CLI transport kill request failed"));
        fault(events, fault_sent);
    }
}
#[derive(Debug)]
struct SharedWaitError(Arc<io::Error>);
impl fmt::Display for SharedWaitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CLI wait failed; child identity uncertain and retained")
    }
}
impl std::error::Error for SharedWaitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

async fn run(input: Input) -> BatchIoCollected {
    let Input {
        mut child,
        archive,
        stdout_limit,
        stderr_limit,
        events,
        mut requests,
        observation,
        cli_observation,
    } = input;
    // All original native owners are already in this future. Extract all pipes
    // and build all owning futures before the first await, including error paths.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdin = child.stdin.take();
    let source = archive.as_ref().map(File::try_clone).transpose();
    let out = capture(stdout, stdout_limit);
    let err = capture(stderr, stderr_limit);
    let writer = write_archive(source, stdin);
    tokio::pin!(out, err, writer);
    let mut out_result = None;
    let mut err_result = None;
    let mut writer_done = false;
    let mut status = None;
    // A failed wait can no longer justify any PID operation. Keep Child and
    // pending collection, while the already-owned capture/writer IO drains.
    let mut wait_uncertain = false;
    let mut errors = Vec::new();
    let mut fault_sent = false;
    let mut controls_open = true;
    let mut control = *requests.borrow_and_update();
    let mut kill_requested = false;
    let mut attach_expired = false;
    let mut writer_expired = false;
    let mut capture_expired = false;
    let mut waited_at = None;
    loop {
        if status.is_some() && out_result.is_some() && err_result.is_some() && writer_done {
            break;
        }
        if control.kill_transport && status.is_none() && !wait_uncertain {
            transport_kill(
                &mut child,
                &mut kill_requested,
                &mut errors,
                &events,
                &mut fault_sent,
            );
        }
        let attach_until = if status.is_none() && !attach_expired {
            control.drain_started.map(|start| start + ATTACH_DRAIN)
        } else {
            None
        };
        let writer_until = if !writer_done && !writer_expired {
            waited_at.map(|start: Instant| start + WRITER_DRAIN)
        } else {
            None
        };
        let capture_until = if (out_result.is_none() || err_result.is_none()) && !capture_expired {
            waited_at.map(|start: Instant| start + CAPTURE_DRAIN)
        } else {
            None
        };
        tokio::select! {
            changed = requests.changed(), if controls_open => {
                if changed.is_err() { controls_open = false; }
                else { control = *requests.borrow_and_update(); }
            }
            result = child.wait(), if status.is_none() && !wait_uncertain => {
                match result {
                    Ok(exit) => {
                        super::observation::record(&observation, super::observation::Phase::AttachExited);
                        super::record_cli_exit(&cli_observation, Some(exit));
                        status = Some(exit);
                        waited_at = Some(Instant::now());
                        let _ = events.try_send(BatchIoEvent::Exited(Ok(exit)));
                    }
                    Err(error) => {
                        super::observation::record(&observation, super::observation::Phase::AttachWaitFailed);
                        super::observation::record(&cli_observation, super::observation::Phase::StopCommandWaitFailed);
                        wait_uncertain = true;
                        let error = Arc::new(error);
                        errors.push(anyhow::Error::new(SharedWaitError(error.clone())));
                        let _ = events.try_send(BatchIoEvent::Exited(Err(error)));
                        fault(&events, &mut fault_sent);
                    }
                }
            }
            result = &mut out, if out_result.is_none() => {
                super::observation::record(&observation, if result.eof { super::observation::Phase::StdoutEof } else { super::observation::Phase::StdoutReadFailed });
                if let Some(error) = result.error.as_ref() {
                    let kind = error.kind();
                    errors.push(anyhow::anyhow!("stdout capture failed ({kind:?}); partial capture retained"));
                    fault(&events, &mut fault_sent);
                }
                out_result = Some(result);
            }
            result = &mut err, if err_result.is_none() => {
                super::observation::record(&observation, if result.eof { super::observation::Phase::StderrEof } else { super::observation::Phase::StderrReadFailed });
                if let Some(error) = result.error.as_ref() {
                    let kind = error.kind();
                    errors.push(anyhow::anyhow!("stderr capture failed ({kind:?}); partial capture retained"));
                    fault(&events, &mut fault_sent);
                }
                err_result = Some(result);
            }
            result = &mut writer, if !writer_done => {
                writer_done = true;
                if let Err(error) = result {
                    errors.push(anyhow::Error::new(error).context("archive writer failed; original FD retained"));
                    fault(&events, &mut fault_sent);
                }
            }
            _ = until(attach_until), if attach_until.is_some() => {
                attach_expired = true;
                errors.push(anyhow::anyhow!("CLI attach drain exceeded 10 seconds; collection remains pending"));
                fault(&events, &mut fault_sent);
                if status.is_none() && !wait_uncertain {
                    transport_kill(&mut child, &mut kill_requested, &mut errors, &events, &mut fault_sent);
                }
            }
            _ = until(writer_until), if writer_until.is_some() => {
                writer_expired = true;
                errors.push(anyhow::anyhow!("archive writer drain exceeded 5 seconds; collection remains pending"));
                fault(&events, &mut fault_sent);
            }
            _ = until(capture_until), if capture_until.is_some() => {
                capture_expired = true;
                errors.push(anyhow::anyhow!("capture drain exceeded 10 seconds; collection remains pending"));
                fault(&events, &mut fault_sent);
            }
            // In particular, a wait fault plus drained pipes plus a dropped
            // observer must neither panic nor pretend that Child was joined.
            else => pending::<()>().await,
        }
    }
    // Only completed futures reach this point. Read errors retain their exact
    // cause here and explicitly do not acquire an EOF flag.
    let out = out_result.expect("capture completed before outcome construction");
    let err = err_result.expect("capture completed before outcome construction");
    if let Some(error) = out.error {
        errors.push(anyhow::Error::new(error).context("stdout read cause"));
    }
    if let Some(error) = err.error {
        errors.push(anyhow::Error::new(error).context("stderr read cause"));
    }
    BatchIoCollected {
        stdout: out.capture,
        stderr: err.capture,
        stdout_eof: out.eof,
        stderr_eof: err.eof,
        status,
        errors,
        retained_input: archive,
    }
}

#[cfg(test)]
#[path = "batch_io_tests.rs"]
mod tests;
