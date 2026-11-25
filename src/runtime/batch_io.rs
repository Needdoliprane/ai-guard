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
    JoinFailed(Arc<JoinError>),
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
            Self::JoinFailed(error) => Some(error.as_ref()),
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
    control_task: Option<JoinHandle<()>>,
    control_joined: bool,
    events: mpsc::Receiver<BatchIoEvent>,
    requests: watch::Sender<Requests>,
    collected: bool,
    unstarted_control: Option<RetainedControlInput>,
    retained_control: Option<Arc<tokio::sync::Mutex<ControlResources>>>,
    failed_join: Option<Arc<JoinError>>,
}
impl fmt::Debug for OwnedBatchIo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedBatchIo")
            .field("unstarted", &self.unstarted.is_some())
            .field(
                "task_retained",
                &(self.task.is_some() || self.control_task.is_some()),
            )
            .field("collected", &self.collected)
            .finish()
    }
}
impl OwnedBatchIo {
    #[cfg(test)]
    pub(super) fn abort_retained_control_for_test(&self) {
        self.control_task
            .as_ref()
            .expect("fixture has an actual native IO task")
            .abort();
    }
    #[cfg(test)]
    pub(super) async fn retained_control_identity_for_test(&self) -> (Option<u32>, bool, bool) {
        let resources = self
            .retained_control
            .as_ref()
            .expect("retained control fixture")
            .lock()
            .await;
        (
            resources.child.id(),
            resources.stdout.is_some(),
            resources.stderr.is_some(),
        )
    }
    #[cfg(test)]
    pub(super) async fn collect_aborted_control_fixture(&mut self) {
        let mut resources = self
            .retained_control
            .as_ref()
            .expect("retained control fixture")
            .lock()
            .await;
        let ControlResources {
            child,
            stdout,
            stderr,
            out,
            err,
            ..
        } = &mut *resources;
        let _ = child.start_kill();
        child.wait().await.expect("collect the exact fixture Child");
        assert!(capture_retained(stdout, out, 256 * 1024).await);
        assert!(capture_retained(stderr, err, 64 * 1024).await);
        // Fixture cleanup only: the application's Unknown phase and original
        // failed join are unchanged, and no settled native proof is produced.
        self.collected = true;
    }
    pub(super) fn is_collected(&self) -> bool {
        self.collected
    }
    pub(super) fn collection_unknown(&self) -> bool {
        self.failed_join.is_some()
    }
    /// Native-control variant: actual Child, pipes and partial captures live
    /// outside the task future. A panic/abort of that task cannot drop them.
    pub(super) fn own_cli_retained_observed(
        mut child: Child,
        stdout_limit: usize,
        stderr_limit: usize,
        observation: Option<super::observation::Scope>,
    ) -> Self {
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        // Ordinary control commands never have an archive/stdin writer.
        drop(child.stdin.take());
        let resources = Arc::new(tokio::sync::Mutex::new(ControlResources {
            child,
            stdout,
            stderr,
            out: empty_capture(stdout_limit),
            err: empty_capture(stderr_limit),
            causes: Vec::new(),
            completed: None,
        }));
        let (send, events) = mpsc::channel(4);
        let (requests, receive) = watch::channel(Requests::default());
        let mut owned = Self {
            unstarted: None,
            task: None,
            control_task: None,
            control_joined: false,
            events,
            requests,
            collected: false,
            unstarted_control: Some(RetainedControlInput {
                resources: Arc::clone(&resources),
                stdout_limit,
                stderr_limit,
                events: send,
                requests: receive,
                observation,
            }),
            retained_control: Some(resources),
            failed_join: None,
        };
        let _ = owned.start_if_possible();
        owned
    }
    /// Infallible transfer. Without a current runtime, Child and archive remain
    /// in unstarted until an async observer can start the same owner task.
    // Closed batch IO owner constructor; production wiring pending.
    #[allow(dead_code)]
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
            control_task: None,
            control_joined: false,
            events,
            requests,
            collected: false,
            unstarted_control: None,
            retained_control: None,
            failed_join: None,
        };
        let _ = owned.start_if_possible();
        owned
    }
    fn start_if_possible(&mut self) -> Result<(), BatchIoCollectError> {
        if self.collected {
            return Err(BatchIoCollectError::AlreadyCollected);
        }
        if let Some(error) = &self.failed_join {
            return Err(BatchIoCollectError::JoinFailed(Arc::clone(error)));
        }
        if self.task.is_none() && self.control_task.is_none() {
            let runtime = tokio::runtime::Handle::try_current()
                .map_err(|_| BatchIoCollectError::NoRuntime)?;
            if let Some(input) = self.unstarted.take() {
                self.task = Some(runtime.spawn(run(input)));
            } else if let Some(input) = self.unstarted_control.take() {
                self.control_task = Some(runtime.spawn(run_retained_control(input)));
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
        if self.control_task.is_some() || self.control_joined {
            if !self.control_joined {
                let task = self
                    .control_task
                    .as_mut()
                    .ok_or(BatchIoCollectError::AlreadyCollected)?;
                match task.await {
                    Ok(()) => {
                        // Remember the real join before another await; cancelled
                        // collection must never repoll a Ready JoinHandle.
                        self.control_joined = true;
                    }
                    Err(error) => {
                        let error = Arc::new(error);
                        self.failed_join = Some(Arc::clone(&error));
                        return Err(BatchIoCollectError::JoinFailed(error));
                    }
                }
            }
            let resources = self
                .retained_control
                .as_ref()
                .ok_or(BatchIoCollectError::AlreadyCollected)?;
            let outcome = resources
                .lock()
                .await
                .completed
                .take()
                .ok_or(BatchIoCollectError::AlreadyCollected)?;
            self.control_task.take();
            self.collected = true;
            return Ok(outcome);
        }
        let joined = match self.task.as_mut() {
            Some(task) => task.await,
            None => return Err(BatchIoCollectError::AlreadyCollected),
        };
        match joined {
            Ok(outcome) => {
                // No await between a real successful join and its consumption.
                self.task.take();
                self.collected = true;
                Ok(outcome)
            }
            Err(error) => {
                // Keep the original failed handle and physical resources. A
                // Ready JoinError is sticky unknown, not a repollable receipt.
                let error = Arc::new(error);
                self.failed_join = Some(Arc::clone(&error));
                Err(BatchIoCollectError::JoinFailed(error))
            }
        }
    }
}

impl Drop for OwnedBatchIo {
    fn drop(&mut self) {
        if self.retained_control.is_some() && !self.collected {
            // Affine native custody cannot be released by an observer's Drop.
            // This pins one already charged call; it creates no replacement.
            let retained = (
                self.retained_control.take(),
                self.unstarted_control.take(),
                self.task.take(),
                self.control_task.take(),
                self.failed_join.take(),
            );
            std::mem::forget(retained);
        }
    }
}

struct ReadCapture {
    capture: Capture,
    eof: bool,
    error: Option<io::Error>,
}
fn empty_capture(limit: usize) -> ReadCapture {
    ReadCapture {
        capture: Capture {
            bytes: Vec::with_capacity(limit.min(16 * 1024)),
            total_bytes: 0,
            truncated: false,
        },
        eof: false,
        error: None,
    }
}
struct ControlResources {
    child: Child,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
    out: ReadCapture,
    err: ReadCapture,
    causes: Vec<anyhow::Error>,
    completed: Option<BatchIoCollected>,
}
struct RetainedControlInput {
    resources: Arc<tokio::sync::Mutex<ControlResources>>,
    stdout_limit: usize,
    stderr_limit: usize,
    events: mpsc::Sender<BatchIoEvent>,
    requests: watch::Receiver<Requests>,
    observation: Option<super::observation::Scope>,
}
async fn capture_retained<R: AsyncRead + Unpin>(
    reader: &mut Option<R>,
    captured: &mut ReadCapture,
    limit: usize,
) -> bool {
    let Some(reader) = reader else {
        captured.error = Some(io::Error::other("batch capture pipe missing"));
        return false;
    };
    let mut buffer = [0; 16 * 1024];
    loop {
        let count = match reader.read(&mut buffer).await {
            Ok(0) => {
                captured.eof = true;
                return true;
            }
            Ok(count) => count,
            Err(error) => {
                captured.error = Some(error);
                return false;
            }
        };
        captured.capture.total_bytes = captured.capture.total_bytes.saturating_add(count as u64);
        let retained = limit
            .saturating_sub(captured.capture.bytes.len())
            .min(count);
        captured
            .capture
            .bytes
            .extend_from_slice(&buffer[..retained]);
        captured.capture.truncated |= retained < count;
    }
}
async fn run_retained_control(input: RetainedControlInput) {
    let RetainedControlInput {
        resources,
        stdout_limit,
        stderr_limit,
        events,
        mut requests,
        observation,
    } = input;
    let mut resources = resources.lock().await;
    let status = {
        let ControlResources {
            child,
            stdout,
            stderr,
            out,
            err,
            causes,
            ..
        } = &mut *resources;
        let output = capture_retained(stdout, out, stdout_limit);
        let errors = capture_retained(stderr, err, stderr_limit);
        tokio::pin!(output, errors);
        let (mut out_done, mut err_done) = (false, false);
        let mut status = None;
        let mut wait_uncertain = false;
        let mut fault_sent = false;
        let mut controls_open = true;
        let mut control = *requests.borrow_and_update();
        let mut kill_requested = false;
        let mut attach_expired = false;
        let mut capture_expired = false;
        let mut waited_at = None;
        loop {
            if status.is_some() && out_done && err_done {
                break;
            }
            if control.kill_transport && status.is_none() && !wait_uncertain {
                transport_kill(child, &mut kill_requested, causes, &events, &mut fault_sent);
            }
            let attach_until = if status.is_none() && !attach_expired {
                control.drain_started.map(|t| t + ATTACH_DRAIN)
            } else {
                None
            };
            let capture_until = if (!out_done || !err_done) && !capture_expired {
                waited_at.map(|t: Instant| t + CAPTURE_DRAIN)
            } else {
                None
            };
            tokio::select! {
                changed = requests.changed(), if controls_open => {
                    if changed.is_err() { controls_open = false; } else { control = *requests.borrow_and_update(); }
                }
                result = child.wait(), if status.is_none() && !wait_uncertain => match result {
                    Ok(exit) => { super::record_cli_exit(&observation, Some(exit)); status = Some(exit); waited_at = Some(Instant::now()); let _ = events.try_send(BatchIoEvent::Exited(Ok(exit))); }
                    Err(error) => { super::observation::record(&observation, super::observation::Phase::StopCommandWaitFailed); wait_uncertain = true; let error = Arc::new(error); causes.push(anyhow::Error::new(SharedWaitError(error.clone()))); let _ = events.try_send(BatchIoEvent::Exited(Err(error))); fault(&events, &mut fault_sent); }
                },
                eof = &mut output, if !out_done => { out_done = true; if !eof { causes.push(anyhow::anyhow!("stdout capture failed; partial capture retained")); fault(&events, &mut fault_sent); } }
                eof = &mut errors, if !err_done => { err_done = true; if !eof { causes.push(anyhow::anyhow!("stderr capture failed; partial capture retained")); fault(&events, &mut fault_sent); } }
                _ = until(attach_until), if attach_until.is_some() => {
                    attach_expired = true; causes.push(anyhow::anyhow!("CLI attach drain exceeded 10 seconds; collection remains pending")); fault(&events, &mut fault_sent);
                    if status.is_none() && !wait_uncertain { transport_kill(child, &mut kill_requested, causes, &events, &mut fault_sent); }
                }
                _ = until(capture_until), if capture_until.is_some() => { capture_expired = true; causes.push(anyhow::anyhow!("capture drain exceeded 10 seconds; collection remains pending")); fault(&events, &mut fault_sent); }
                else => pending::<()>().await,
            }
        }
        status
    };
    let mut causes = std::mem::take(&mut resources.causes);
    if let Some(error) = resources.out.error.take() {
        causes.push(anyhow::Error::new(error).context("stdout read cause"));
    }
    if let Some(error) = resources.err.error.take() {
        causes.push(anyhow::Error::new(error).context("stderr read cause"));
    }
    let completed = BatchIoCollected {
        stdout: std::mem::replace(&mut resources.out.capture, empty_capture(0).capture),
        stderr: std::mem::replace(&mut resources.err.capture, empty_capture(0).capture),
        stdout_eof: resources.out.eof,
        stderr_eof: resources.err.eof,
        status,
        errors: causes,
        retained_input: None,
    };
    // Publish the exact captures/causes outside the task's return value before
    // returning. A failed task join cannot discard this completed payload.
    resources.completed = Some(completed);
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
