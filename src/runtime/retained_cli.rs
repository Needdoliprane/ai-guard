//! Retained engine CLI consumer.
//!
//! One engine command that reads prepared material (`build`, Compose) is owned
//! by a dedicated OS thread, independent of the asynchronous task that observes
//! it. The thread holds the material until the command is *collected*: the real
//! wait status of the CLI process is known **and** both of its output pipes
//! reached end of file. A dropped observer, an elapsed bound, a signal that was
//! sent and the exit of the CLI process alone are not collection.
//!
//! When collection cannot be established the end is *unknown*. It is announced
//! once and, from that announcement on, this process never releases the
//! material, even if the command is collected later.
//!
//! Everything reported here is a fact about the local CLI process and its
//! process group: a wait status, end of file on two pipes, signals this process
//! sent. It establishes nothing about an engine daemon, or about a process that
//! no longer holds either output pipe, inside or outside the process group: a
//! command that ends by itself, or on the forwarded signal, is collected
//! without the group being signalled again or inspected. After SIGKILL the same
//! holds for a process that left the group or that this UID may not signal.
use super::*;
use std::{
    borrow::Cow,
    io::Read,
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::PathBuf,
    process::{Child, ChildStderr, ChildStdout, ExitStatus},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Instant,
};
use tokio::sync::{Notify, mpsc};

/// Capture bounds, equal to those of `Runtime::cli_env`.
pub const STDOUT_LIMIT: usize = 256 * 1024;
pub const STDERR_LIMIT: usize = 64 * 1024;
/// Largest bound a retained command may be given.
pub const MAX_DURATION: Duration = Duration::from_secs(30 * 60);
/// Time a forwarded operator signal is given before the process group is killed.
pub const INTERRUPT_GRACE: Duration = Duration::from_secs(10);
/// A stop request repeated sooner than this is the same request: a hang-up
/// reported by both kernel and shell, or a signal relayed by a wrapper, arrives
/// twice within milliseconds.
pub const REPEAT_MIN_INTERVAL: Duration = Duration::from_secs(1);
/// Time collection is given after SIGKILL before the end is announced unknown.
pub const STOP_OBSERVATION: Duration = Duration::from_secs(2);
const MAX_ARGUMENTS: usize = 128;
const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
const MAX_ENVIRONMENT_VALUE_BYTES: usize = 4096;
const POLL: Duration = Duration::from_millis(5);
/// Reads of one pipe between two looks at the stop requests.
const DRAIN_READS: usize = 16;
/// Attempts of a non-blocking system call that a signal interrupted.
const INTERRUPTED_RETRIES: usize = 16;

/// Material a retained engine command reads. The owner thread holds it until
/// collection; it is the only source of the arguments that name that material.
pub trait RetainedInput: Send + 'static {
    /// Engine arguments naming only the retained material. Called exactly once,
    /// on the caller's task, before the owner thread exists.
    fn arguments(&self) -> Vec<String>;
    /// Last synchronous check, in the owner thread, immediately before spawn.
    fn verify_before_spawn(&self) -> Result<()>;
    /// In the owner thread, after collection and before the input is dropped.
    fn verify_after_collection(&self) -> Result<()> {
        Ok(())
    }
    /// Called at most once when termination becomes unknown. Must not release
    /// anything. Returns where the material stays. After this call the owner
    /// never drops the input.
    fn termination_unknown(&self, _end: &UnknownEnd) -> Option<PathBuf> {
        None
    }
}

/// `SIGINT`, `SIGTERM`, `SIGHUP` or `SIGKILL` by name; any other number as is.
pub fn signal_name(signal: i32) -> Cow<'static, str> {
    match signal {
        libc::SIGHUP => "SIGHUP".into(),
        libc::SIGINT => "SIGINT".into(),
        libc::SIGTERM => "SIGTERM".into(),
        libc::SIGKILL => "SIGKILL".into(),
        other => format!("signal {other}").into(),
    }
}

#[derive(Default)]
struct Request {
    first: Option<(i32, Instant)>,
    escalated: bool,
}

#[derive(Default)]
struct InterruptionState {
    request: Mutex<Request>,
    active: AtomicUsize,
    released: Notify,
}

/// Operator stop request shared between a command scope and its retained commands.
#[derive(Clone, Default)]
pub struct Interruption(Arc<InterruptionState>);
impl Interruption {
    pub fn new() -> Self {
        Self::default()
    }
    fn request_state(&self) -> std::sync::MutexGuard<'_, Request> {
        self.0
            .request
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
    /// First call: records `signal` and the instant. A call less than
    /// `REPEAT_MIN_INTERVAL` after the first is ignored. A later call sets the
    /// escalation (SIGKILL).
    ///
    /// Only SIGINT, SIGTERM and SIGHUP are forwarded to an engine process
    /// group: any other value is recorded as SIGTERM, so a stop request is
    /// never lost and never turns into another signal.
    pub fn request(&self, signal: i32) {
        let signal = match signal {
            libc::SIGINT | libc::SIGHUP => signal,
            _ => libc::SIGTERM,
        };
        let now = Instant::now();
        let mut request = self.request_state();
        match request.first {
            None => request.first = Some((signal, now)),
            Some((_, first)) if now.duration_since(first) >= REPEAT_MIN_INTERVAL => {
                request.escalated = true;
            }
            Some(_) => {}
        }
    }
    /// The first requested signal.
    pub fn requested(&self) -> Option<i32> {
        self.request_state().first.map(|(signal, _)| signal)
    }
    pub fn escalated(&self) -> bool {
        self.request_state().escalated
    }
    /// Counts work that must finish before the command future may be dropped.
    /// Create and drop it on the task that awaits `idle()`.
    pub fn hold(&self) -> Hold {
        self.0.active.fetch_add(1, Ordering::AcqRel);
        Hold {
            state: Arc::clone(&self.0),
            kept: false,
        }
    }
    /// Live holds.
    pub fn active(&self) -> usize {
        self.0.active.load(Ordering::Acquire)
    }
    /// Resolves when `active()` is zero.
    pub async fn idle(&self) {
        loop {
            let released = self.0.released.notified();
            tokio::pin!(released);
            // Registered before the count is read: a release in between wakes it.
            released.as_mut().enable();
            if self.active() == 0 {
                return;
            }
            released.await;
        }
    }
}

/// One unit of work counted by an `Interruption`. Dropping it ends the count.
pub struct Hold {
    state: Arc<InterruptionState>,
    kept: bool,
}
impl Hold {
    /// Never released: the termination of the work it counts is unknown.
    pub fn keep(mut self) {
        self.kept = true;
    }
}
impl Drop for Hold {
    fn drop(&mut self) {
        if !self.kept {
            self.state.active.fetch_sub(1, Ordering::AcqRel);
            self.state.released.notify_waiters();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The observer cancelled the command or was dropped.
    Cancelled,
    /// An operator stop request; `signal` is the first one requested.
    Interrupted { signal: i32 },
    /// The command's own bound elapsed.
    TimedOut,
    /// An output pipe could not be read.
    OutputUnavailable,
    /// The wait status of the CLI process could not be obtained. No signal is
    /// sent for this reason: the process group can no longer be named safely.
    WaitUnavailable,
}

/// What the owner did to stop the command. Observed facts only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stop {
    /// The first reason; a later one does not replace it.
    pub reason: StopReason,
    /// The operator signal the process group accepted, if any.
    pub forwarded: Option<i32>,
    /// SIGKILL was accepted by the process group.
    pub kill_sent: bool,
}

/// Proof of end: real wait status and EOF on both pipes.
pub struct Collected {
    pub status: ExitStatus,
    pub stdout: Capture,
    pub stderr: Capture,
    /// `None`: the command ended by itself.
    pub stopped: Option<Stop>,
    /// `false` only after an announced `Unknown`.
    pub input_released: bool,
    /// `verify_after_collection` failed.
    pub input_changed: Option<anyhow::Error>,
}
impl std::fmt::Debug for Collected {
    // Captured bytes never enter a formatted value.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Collected")
            .field("status", &self.status)
            .field("stdout_bytes", &self.stdout.bytes.len())
            .field("stdout_total_bytes", &self.stdout.total_bytes)
            .field("stdout_truncated", &self.stdout.truncated)
            .field("stderr_bytes", &self.stderr.bytes.len())
            .field("stderr_total_bytes", &self.stderr.total_bytes)
            .field("stderr_truncated", &self.stderr.truncated)
            .field("stopped", &self.stopped)
            .field("input_released", &self.input_released)
            .field("input_changed", &self.input_changed)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct UnknownEnd {
    pub runtime: RuntimeKind,
    pub leader_pid: u32,
    /// Equals the leader PID.
    pub process_group: u32,
    pub stop: Stop,
    pub leader_reaped: bool,
    pub wait_uncertain: bool,
    pub stdout_open: bool,
    pub stderr_open: bool,
    pub retained_at: Option<PathBuf>,
}

#[derive(Debug)]
pub enum InvocationEnd {
    /// Nothing was spawned; the input was dropped.
    Refused(anyhow::Error),
    Collected(Collected),
    /// The input is kept for good; the owner thread keeps the admission slot
    /// until a late collection, if any.
    Unknown(UnknownEnd),
    /// The owner thread ended without a report (internal failure). It ends only
    /// before spawn or after collection.
    Aborted {
        input_released: bool,
    },
}
impl InvocationEnd {
    /// `Ok(stdout)` for a self-ended success; `RuntimeCommandFailure` for a
    /// self-ended non-zero exit; `InvocationStopped` when the owner stopped the
    /// command, whatever its status; otherwise `InvocationFailure`. A result
    /// whose input failed its check after collection is `InputChanged`, with
    /// the input's own error as source, whatever the status.
    pub fn into_result(self, runtime: RuntimeKind) -> Result<Capture> {
        match self {
            Self::Refused(error) => Err(error),
            Self::Unknown(_) => Err(InvocationFailure::Unknown.into()),
            Self::Aborted { .. } => Err(InvocationFailure::Unavailable.into()),
            Self::Collected(collected) => {
                if let Some(stop) = collected.stopped {
                    return Err(InvocationStopped {
                        runtime,
                        stop,
                        status: collected.status,
                    }
                    .into());
                }
                if let Some(changed) = collected.input_changed {
                    return Err(changed.context(InvocationFailure::InputChanged));
                }
                if !collected.status.success() {
                    return Err(RuntimeCommandFailure {
                        runtime,
                        status: collected.status,
                        stderr: collected.stderr,
                        private_redactions: Vec::new(),
                    }
                    .into());
                }
                Ok(collected.stdout)
            }
        }
    }
    /// The input was dropped: nothing of this command is left to wait for.
    fn releases_input(&self) -> bool {
        match self {
            Self::Refused(_) => true,
            Self::Collected(collected) => collected.input_released,
            Self::Aborted { input_released } => *input_released,
            Self::Unknown(_) => false,
        }
    }
}

/// Why a retained command has no collected result. Never formats captured output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvocationFailure {
    InvalidInput,
    Capacity,
    Cancelled,
    Interrupted { signal: i32 },
    Unavailable,
    InputChanged,
    Unknown,
}
impl std::fmt::Display for InvocationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput => {
                f.write_str("retained engine CLI arguments, environment or bound are not approved")
            }
            Self::Capacity => f.write_str(
                "another retained engine CLI command is still owned by this control process",
            ),
            Self::Cancelled => f.write_str("the engine CLI command was cancelled before it started"),
            Self::Interrupted { signal } => write!(f, "interrupted by {}", signal_name(*signal)),
            Self::Unavailable => f.write_str("the retained engine CLI owner is unavailable"),
            Self::InputChanged => {
                f.write_str("the material retained for the engine CLI command failed its check")
            }
            Self::Unknown => f.write_str(
                "engine CLI termination is unknown; its material is kept and is not removed by ai-guard",
            ),
        }
    }
}
impl std::error::Error for InvocationFailure {}

/// A collected command that the owner stopped. Observed facts only.
#[derive(Debug)]
pub struct InvocationStopped {
    pub runtime: RuntimeKind,
    pub stop: Stop,
    pub status: ExitStatus,
}
impl std::fmt::Display for InvocationStopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} command was stopped (", self.runtime)?;
        match self.stop.reason {
            StopReason::Cancelled => f.write_str("cancelled")?,
            StopReason::Interrupted { signal } => {
                write!(f, "interrupted by {}", signal_name(signal))?;
            }
            StopReason::TimedOut => f.write_str("its bound elapsed")?,
            StopReason::OutputUnavailable => f.write_str("its output could not be read")?,
            StopReason::WaitUnavailable => f.write_str("its wait status was unavailable")?,
        }
        if let Some(signal) = self.stop.forwarded {
            write!(
                f,
                "; {} was forwarded to its process group",
                signal_name(signal)
            )?;
        }
        if self.stop.kill_sent {
            f.write_str("; SIGKILL was sent to its process group")?;
        }
        write!(
            f,
            "); the CLI process exited ({}) and both of its output pipes reached end of file",
            self.status
        )
    }
}
impl std::error::Error for InvocationStopped {}

/// One retained engine command per control process.
static ADMITTED: AtomicBool = AtomicBool::new(false);
struct Slot(());
impl Slot {
    fn take() -> Option<Self> {
        ADMITTED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(()))
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        ADMITTED.store(false, Ordering::Release);
    }
}

/// Observer of one retained command. Not Clone. Dropping it requests a stop and
/// detaches; it never aborts the owner thread or releases the input.
#[must_use = "dropping the observer cancels the retained command"]
pub struct RuntimeInvocationOwner {
    cancel: Arc<AtomicBool>,
    keep: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    ends: mpsc::Receiver<InvocationEnd>,
    hold: Option<Hold>,
}
impl RuntimeInvocationOwner {
    /// Asks the owner to kill the command's process group. The end is still
    /// reported by `join()`, after collection or as unknown.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }
    /// Yields `Unknown` at most once, then the later end if one exists.
    /// Releases the hold when it returns `Refused`, `Aborted { input_released: true }`
    /// or `Collected { input_released: true }`. On `Unknown`, and when the observer
    /// is dropped before such an end was sent, the hold is kept for good
    /// (`Hold::keep`). A call after the last end reports `Aborted` with the
    /// recorded release state.
    pub async fn join(&mut self) -> InvocationEnd {
        let end = match self.ends.recv().await {
            Some(end) => end,
            None => self.aborted(),
        };
        self.settle(&end);
        end
    }
    /// Every sender is gone, so the owner thread has run its last release.
    fn aborted(&self) -> InvocationEnd {
        InvocationEnd::Aborted {
            input_released: !self.keep.load(Ordering::Acquire),
        }
    }
    fn settle(&mut self, end: &InvocationEnd) {
        if !matches!(end, InvocationEnd::Unknown(_))
            && let Some(worker) = self.worker.take()
        {
            // Brief by construction, on an asynchronous task and in `Drop`
            // alike. A last end is sent by the final statement of the owner
            // thread, after the input and the slot were released; a closed
            // channel means the thread dropped its sender, which it also does
            // after both. What is left of the thread is the drop of its own
            // handles, never work.
            let _ = worker.join();
        }
        if end.releases_input() {
            self.hold = None;
        } else if let Some(hold) = self.hold.take() {
            hold.keep();
        }
    }
}
impl Drop for RuntimeInvocationOwner {
    fn drop(&mut self) {
        self.cancel();
        // An end that was already sent still settles the hold. Without one the
        // command may be running: its hold is kept for good, even if the owner
        // sends its end an instant later, because nothing here waits for it.
        // A caller whose scope must become idle again takes the end through
        // `join()` instead of dropping the observer.
        let end = match self.ends.try_recv() {
            Ok(end) => Some(end),
            Err(mpsc::error::TryRecvError::Disconnected) => Some(self.aborted()),
            Err(mpsc::error::TryRecvError::Empty) => None,
        };
        match end {
            Some(end) => self.settle(&end),
            None => {
                if let Some(hold) = self.hold.take() {
                    hold.keep();
                }
            }
        }
    }
}

impl Runtime {
    /// Starts `prefix` followed by the input's own arguments as one retained
    /// engine command, bounded by `duration` from its spawn.
    ///
    /// Refused before anything is spawned: more than 128 arguments, more than
    /// 64 KiB of arguments or a NUL byte; an environment other than the
    /// distinct entries `AGENTSANDBOX_CONFIG_DIR` (absolute, at most 4096 bytes, no control
    /// characters), `AI_GUARD_GATEWAY_PORT` and `AI_GUARD_DASHBOARD_PORT` (decimal `1..=65535`, no leading zero); a zero or over-30-minute bound; a retained command already
    /// owned by this process. Capacity is decided before a stop request is
    /// looked at.
    pub fn cli_env_retained(
        &self,
        prefix: Vec<String>,
        input: Box<dyn RetainedInput>,
        duration: Duration,
        environment: &[(&str, &str)],
        interruption: Option<Interruption>,
    ) -> RuntimeInvocationOwner {
        // Taken before anything else: from here on the command scope sees work
        // it must not abandon.
        let hold = interruption.as_ref().map(Interruption::hold);
        let cancel = Arc::new(AtomicBool::new(false));
        let keep = Arc::new(AtomicBool::new(false));
        // At most one `Unknown` and one last end are ever sent.
        let (sender, ends) = mpsc::channel(2);
        let links = Links {
            control: Control {
                cancel: Arc::clone(&cancel),
                interruption,
            },
            keep: Arc::clone(&keep),
            sender: sender.clone(),
        };
        let worker = match admit(self, prefix, input, duration, environment, links) {
            Ok(worker) => Some(worker),
            Err(failure) => {
                let _ = sender.try_send(InvocationEnd::Refused(failure.into()));
                None
            }
        };
        RuntimeInvocationOwner {
            cancel,
            keep,
            worker,
            ends,
            hold,
        }
    }
}

/// The stop requests an owner thread looks at.
struct Control {
    cancel: Arc<AtomicBool>,
    interruption: Option<Interruption>,
}
impl Control {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
    fn interrupted(&self) -> Option<i32> {
        self.interruption.as_ref().and_then(Interruption::requested)
    }
    fn escalated(&self) -> bool {
        self.interruption
            .as_ref()
            .is_some_and(Interruption::escalated)
    }
    /// A stop requested before spawn refuses the command.
    fn refusal(&self) -> Option<InvocationFailure> {
        if self.cancelled() {
            return Some(InvocationFailure::Cancelled);
        }
        self.interrupted()
            .map(|signal| InvocationFailure::Interrupted { signal })
    }
}

/// The environment a control CLI command may carry. Each name appears at most once and carries one
/// value of a closed kind: the absolute control configuration directory of the operator, or one of
/// the two host loopback publication ports that `deploy/*/compose.yaml` reads (`1..=65535`, decimal
/// digits only). Nothing else reaches the engine CLI: its environment is otherwise empty.
pub(crate) fn approved_environment(environment: &[(&str, &str)]) -> bool {
    const NAMES: [&str; 3] = [
        "AGENTSANDBOX_CONFIG_DIR",
        "AI_GUARD_GATEWAY_PORT",
        "AI_GUARD_DASHBOARD_PORT",
    ];
    environment.len() <= NAMES.len()
        && environment
            .iter()
            .enumerate()
            .all(|(index, (key, _))| !environment[..index].iter().any(|(seen, _)| seen == key))
        && environment.iter().all(|(key, value)| match *key {
            "AGENTSANDBOX_CONFIG_DIR" => {
                value.len() <= MAX_ENVIRONMENT_VALUE_BYTES
                    && std::path::Path::new(value).is_absolute()
                    && !value.chars().any(char::is_control)
            }
            "AI_GUARD_GATEWAY_PORT" | "AI_GUARD_DASHBOARD_PORT" => {
                // No sign, no leading zero (it would be ambiguous for Compose), 1..=65535.
                value.len() <= 5
                    && !value.starts_with('0')
                    && value.bytes().all(|byte| byte.is_ascii_digit())
                    && value.parse::<u16>().is_ok()
            }
            _ => false,
        })
}

fn approved(argv: &[String], duration: Duration, environment: &[(&str, &str)]) -> bool {
    argv.len() <= MAX_ARGUMENTS
        && argv.iter().map(String::len).sum::<usize>() <= MAX_ARGUMENT_BYTES
        && argv.iter().all(|argument| !argument.contains('\0'))
        && approved_environment(environment)
        && !duration.is_zero()
        && duration <= MAX_DURATION
}

#[cfg(test)]
mod approved_environment_tests {
    use super::approved_environment;

    #[test]
    fn the_control_directory_and_the_two_publication_ports_are_approved() {
        assert!(approved_environment(&[]));
        assert!(approved_environment(&[(
            "AGENTSANDBOX_CONFIG_DIR",
            "/control"
        )]));
        assert!(approved_environment(&[("AI_GUARD_GATEWAY_PORT", "18787")]));
        assert!(approved_environment(&[
            ("AGENTSANDBOX_CONFIG_DIR", "/control"),
            ("AI_GUARD_GATEWAY_PORT", "1"),
            ("AI_GUARD_DASHBOARD_PORT", "65535"),
        ]));
    }

    #[test]
    fn any_other_name_port_form_or_repetition_is_refused() {
        for refused in [
            vec![("PATH", "/usr/bin")],
            vec![("AGENTSANDBOX_GATEWAY_PORT", "8787")],
            vec![("AI_GUARD_GATEWAY_PORT", "0")],
            vec![("AI_GUARD_GATEWAY_PORT", "65536")],
            vec![("AI_GUARD_GATEWAY_PORT", "+8787")],
            vec![("AI_GUARD_GATEWAY_PORT", "0x1f90")],
            vec![("AI_GUARD_GATEWAY_PORT", "")],
            vec![("AI_GUARD_GATEWAY_PORT", "00001")],
            vec![("AI_GUARD_DASHBOARD_PORT", "4000\n")],
            vec![("AGENTSANDBOX_CONFIG_DIR", "relative")],
            vec![("AGENTSANDBOX_CONFIG_DIR", "/control\n")],
            vec![
                ("AI_GUARD_GATEWAY_PORT", "8787"),
                ("AI_GUARD_GATEWAY_PORT", "8788"),
            ],
            vec![
                ("AGENTSANDBOX_CONFIG_DIR", "/a"),
                ("AI_GUARD_GATEWAY_PORT", "8787"),
                ("AI_GUARD_DASHBOARD_PORT", "4000"),
                ("PATH", "/usr/bin"),
            ],
        ] {
            assert!(!approved_environment(&refused), "{refused:?}");
        }
    }
}

/// What an owner thread shares with its observer.
struct Links {
    control: Control,
    keep: Arc<AtomicBool>,
    sender: mpsc::Sender<InvocationEnd>,
}

/// Everything decided on the caller's task. On a refusal the input is dropped
/// here, before the caller gets its observer back.
fn admit(
    runtime: &Runtime,
    prefix: Vec<String>,
    input: Box<dyn RetainedInput>,
    duration: Duration,
    environment: &[(&str, &str)],
    links: Links,
) -> Result<thread::JoinHandle<()>, InvocationFailure> {
    let mut argv = prefix;
    argv.extend(input.arguments());
    if !approved(&argv, duration, environment) {
        return Err(InvocationFailure::InvalidInput);
    }
    let slot = Slot::take().ok_or(InvocationFailure::Capacity)?;
    let runtime = runtime.clone();
    let environment: Vec<(String, String)> = environment
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    thread::Builder::new()
        .name("ai-guard-cli-owner".into())
        .spawn(move || {
            // Declaration order is the unwind contract. A panic first drops the
            // frame of `run`, which collects the child; then `held`, which
            // drops or keeps the input; then the slot; the channel closes only
            // after both.
            let Links {
                control,
                keep,
                sender,
            } = links;
            let slot = slot;
            let held = Held {
                input: Some(input),
                keep,
            };
            // Returns only before spawn or after collection.
            let end = match run(
                &runtime,
                argv,
                environment,
                duration,
                &control,
                &held,
                &sender,
            ) {
                Ran::Refused(error) => InvocationEnd::Refused(error),
                Ran::Collected(mut collected) => {
                    if held.kept() {
                        collected.input_released = false;
                    } else {
                        collected.input_changed = held.input().verify_after_collection().err();
                        collected.input_released = true;
                    }
                    InvocationEnd::Collected(collected)
                }
            };
            drop(held);
            drop(slot);
            // Must stay the last statement: the observer joins this thread
            // from an asynchronous task, or from its `Drop`, as soon as it
            // holds this end. Anything that takes time belongs above.
            let _ = sender.try_send(end);
        })
        .map_err(|_| InvocationFailure::Unavailable)
}

/// The input and the flag that decides its fate when the thread lets go of it.
struct Held {
    input: Option<Box<dyn RetainedInput>>,
    keep: Arc<AtomicBool>,
}
impl Held {
    fn input(&self) -> &dyn RetainedInput {
        // Present from construction until `drop`, the only place that takes it.
        self.input
            .as_deref()
            .expect("the retained input is held until the owner lets go of it")
    }
    fn kept(&self) -> bool {
        self.keep.load(Ordering::Acquire)
    }
}
impl Drop for Held {
    fn drop(&mut self) {
        if let Some(input) = self.input.take() {
            if self.kept() {
                // Unknown termination was announced: a process may still read
                // this material, so it is never released by this process.
                std::mem::forget(input);
            } else {
                drop(input);
            }
        }
    }
}

enum Ran {
    Refused(anyhow::Error),
    Collected(Collected),
}

fn run(
    runtime: &Runtime,
    argv: Vec<String>,
    environment: Vec<(String, String)>,
    duration: Duration,
    control: &Control,
    held: &Held,
    sender: &mpsc::Sender<InvocationEnd>,
) -> Ran {
    if let Some(reason) = control.refusal() {
        return Ran::Refused(reason.into());
    }
    // Same order as `Runtime::cli_env`: approved executable, then the caller's
    // material, then the executable once more as the last check.
    let mut command = match runtime.command() {
        Ok(command) => command,
        Err(error) => return Ran::Refused(error),
    };
    if let Err(error) = held.input().verify_before_spawn() {
        return Ran::Refused(error.context(InvocationFailure::InputChanged));
    }
    if let Err(error) = runtime.anchor.verify(&runtime.invocation, runtime.kind) {
        return Ran::Refused(error);
    }
    command
        .as_std_mut()
        .args(argv)
        .envs(environment)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own group: a stop reaches the CLI and the helpers it started,
        // and never the control process.
        .process_group(0);
    if let Some(reason) = control.refusal() {
        return Ran::Refused(reason.into());
    }
    let child = match command.as_std_mut().spawn() {
        Ok(child) => child,
        Err(error) => {
            return Ran::Refused(anyhow::Error::new(error).context(format!(
                "{} is unavailable; install and start a container runtime. No local fallback exists",
                runtime.kind
            )));
        }
    };
    let mut work = ChildWork::own(runtime.kind, child, duration, held, sender);
    loop {
        let more = work.poll();
        if let Some(collected) = work.take_collected() {
            return Ran::Collected(collected);
        }
        let now = Instant::now();
        work.steer(control, now);
        if work.unknown_due(now) {
            work.announce_unknown(Some(held.input()));
        }
        if !more {
            // The same period applies after an announced unknown end: this
            // thread, the only one the admission slot allows, keeps looking
            // for a late collection until the control process ends. After an
            // uncertain wait there is none to find, and the slot stays taken.
            thread::park_timeout(POLL);
        }
    }
}

struct Pipe<R> {
    reader: R,
    capture: Capture,
    limit: usize,
    nonblocking: bool,
    eof: bool,
}
impl<R: Read + AsRawFd> Pipe<R> {
    fn new(reader: R, limit: usize) -> Self {
        Self {
            reader,
            capture: empty_capture(),
            limit,
            nonblocking: false,
            eof: false,
        }
    }
    /// Reads what is available now. `Ok(true)`: the read quantum was used up
    /// and more may be waiting. End of file is recorded only from a real
    /// zero-length read.
    fn drain(&mut self) -> std::io::Result<bool> {
        if self.eof {
            return Ok(false);
        }
        if !self.nonblocking {
            let descriptor = self.reader.as_raw_fd();
            // SAFETY: this thread exclusively owns the open read end. The flag
            // belongs to that open file description; the writers keep theirs.
            let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
            if flags < 0
                || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            self.nonblocking = true;
        }
        let mut buffer = [0_u8; 16 * 1024];
        for _ in 0..DRAIN_READS {
            match self.reader.read(&mut buffer) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(false);
                }
                Ok(count) => {
                    self.capture.total_bytes =
                        self.capture.total_bytes.saturating_add(count as u64);
                    let retain = count.min(self.limit.saturating_sub(self.capture.bytes.len()));
                    self.capture.bytes.extend_from_slice(&buffer[..retain]);
                    self.capture.truncated |= retain < count;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }
}

fn empty_capture() -> Capture {
    Capture {
        bytes: Vec::new(),
        total_bytes: 0,
        truncated: false,
    }
}

#[derive(Clone, Copy)]
enum Phase {
    Running,
    /// The operator signal was forwarded once; SIGKILL follows at `until`.
    Grace {
        until: Instant,
    },
    /// SIGKILL was requested once; unknown is announced at `observe_until`.
    Kill {
        observe_until: Instant,
    },
}

/// The spawned command and everything needed to collect it. Its `Drop` is the
/// unwind path: the frame that owns it cannot be left before collection.
struct ChildWork<'a> {
    runtime: RuntimeKind,
    child: Child,
    // `None`: the pipe was never handed over; it then never counts as closed.
    stdout: Option<Pipe<ChildStdout>>,
    stderr: Option<Pipe<ChildStderr>>,
    /// The real wait status of the leader.
    status: Option<ExitStatus>,
    wait_uncertain: bool,
    /// Bound of the command, armed at spawn. `None` is an elapsed bound.
    until: Option<Instant>,
    phase: Phase,
    stop: Option<Stop>,
    unknown_announced: bool,
    held: &'a Held,
    sender: &'a mpsc::Sender<InvocationEnd>,
}
impl<'a> ChildWork<'a> {
    fn own(
        runtime: RuntimeKind,
        mut child: Child,
        duration: Duration,
        held: &'a Held,
        sender: &'a mpsc::Sender<InvocationEnd>,
    ) -> Self {
        let spawned = Instant::now();
        let stdout = child
            .stdout
            .take()
            .map(|pipe| Pipe::new(pipe, STDOUT_LIMIT));
        let stderr = child
            .stderr
            .take()
            .map(|pipe| Pipe::new(pipe, STDERR_LIMIT));
        let mut work = Self {
            runtime,
            child,
            stdout,
            stderr,
            status: None,
            wait_uncertain: false,
            until: spawned.checked_add(duration),
            phase: Phase::Running,
            stop: None,
            unknown_announced: false,
            held,
            sender,
        };
        if work.stdout.is_none() || work.stderr.is_none() {
            work.kill(StopReason::OutputUnavailable, spawned);
        }
        work
    }
    fn pipes_closed(&self) -> bool {
        self.stdout.as_ref().is_some_and(|pipe| pipe.eof)
            && self.stderr.as_ref().is_some_and(|pipe| pipe.eof)
    }
    fn kill_requested(&self) -> bool {
        matches!(self.phase, Phase::Kill { .. })
    }
    /// Collection: real wait status **and** end of file on both pipes.
    fn take_collected(&mut self) -> Option<Collected> {
        let status = self.status.filter(|_| self.pipes_closed())?;
        let take = |capture: Option<&mut Capture>| {
            capture.map_or_else(empty_capture, |kept| {
                std::mem::replace(kept, empty_capture())
            })
        };
        Some(Collected {
            status,
            stdout: take(self.stdout.as_mut().map(|pipe| &mut pipe.capture)),
            stderr: take(self.stderr.as_mut().map(|pipe| &mut pipe.capture)),
            stopped: self.stop,
            input_released: false,
            input_changed: None,
        })
    }
    /// Whether the leader is still this process's unreaped child, running or
    /// exited. Only then does its PID name the command's process group. The
    /// status is left in place: this never reaps.
    fn leader_unreaped(leader: libc::pid_t) -> bool {
        // SAFETY: an all-zero `siginfo_t` is a valid value for the kernel to
        // overwrite.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let options = libc::WEXITED | libc::WNOHANG | libc::WNOWAIT;
        for _ in 0..INTERRUPTED_RETRIES {
            // SAFETY: `info` is a live, exclusive out-pointer for this call.
            if unsafe { libc::waitid(libc::P_PID, leader as libc::id_t, &mut info, options) } == 0 {
                return true;
            }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return false;
            }
        }
        false
    }
    /// Sends `signal` to the command's process group. Never after a reap or an
    /// uncertain wait, and only once the leader is seen as this process's
    /// unreaped child: a leader reaped by anything else leaves a group
    /// identifier that may name other processes, and makes the wait uncertain.
    fn signal_group(&mut self, signal: i32) -> bool {
        if self.status.is_some() || self.wait_uncertain {
            return false;
        }
        let leader = match libc::pid_t::try_from(self.child.id()) {
            // 0 and 1 would address other processes when negated into a group.
            Ok(leader) if leader > 1 && Self::leader_unreaped(leader) => leader,
            _ => {
                self.wait_uncertain = true;
                return false;
            }
        };
        // SAFETY: `process_group(0)` made the child the leader of its own
        // group, and it was just seen unreaped. Unless its status is taken
        // between that look and this call, its PID cannot have been given to
        // another process, so the negated PID names that group only. The
        // kernel does not take it by itself in an operator command: the
        // interruption scope refuses a process whose SIGCHLD is ignored or
        // carries SA_NOCLDWAIT.
        unsafe { libc::kill(-leader, signal) == 0 }
    }
    /// Requests SIGKILL once. From here on the leader may be reaped: no later
    /// signal needs its PID.
    fn kill(&mut self, reason: StopReason, now: Instant) {
        if self.kill_requested() {
            return;
        }
        let kill_sent = self.signal_group(libc::SIGKILL);
        let stop = self.stop.get_or_insert(Stop {
            reason,
            forwarded: None,
            kill_sent: false,
        });
        stop.kill_sent = kill_sent;
        self.phase = Phase::Kill {
            observe_until: now + STOP_OBSERVATION,
        };
    }
    /// Drains both pipes and, when allowed, asks for the leader's status.
    /// Returns whether a pipe may hold more bytes right now.
    fn poll(&mut self) -> bool {
        let stdout = self.stdout.as_mut().map(Pipe::drain);
        let stderr = self.stderr.as_mut().map(Pipe::drain);
        if matches!(stdout, Some(Err(_))) || matches!(stderr, Some(Err(_))) {
            // The pipe stays open in the record: unread output is not an end.
            self.kill(StopReason::OutputUnavailable, Instant::now());
        }
        // A leader that exited while a pipe is open stays unreaped until SIGKILL
        // was requested: its PID keeps naming the group the signals are for.
        if self.status.is_none()
            && !self.wait_uncertain
            && (self.kill_requested() || self.pipes_closed())
        {
            match self.child.try_wait() {
                Ok(Some(status)) => self.status = Some(status),
                Ok(None) => {}
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => self.wait_uncertain = true,
            }
        }
        matches!(stdout, Some(Ok(true))) || matches!(stderr, Some(Ok(true)))
    }
    /// Acts on the stop requests and on the bounds. Nothing is signalled after
    /// an uncertain wait or after unknown was announced.
    fn steer(&mut self, control: &Control, now: Instant) {
        if self.unknown_announced || self.wait_uncertain {
            return;
        }
        let expired = self.until.is_none_or(|until| now >= until);
        match self.phase {
            Phase::Running => {
                if control.cancelled() {
                    self.kill(StopReason::Cancelled, now);
                } else if expired {
                    self.kill(StopReason::TimedOut, now);
                } else if let Some(signal) = control.interrupted() {
                    // With its own process group the CLI no longer receives the
                    // operator's signal by itself: it is forwarded first.
                    let forwarded = self.signal_group(signal).then_some(signal);
                    self.stop = Some(Stop {
                        reason: StopReason::Interrupted { signal },
                        forwarded,
                        kill_sent: false,
                    });
                    self.phase = Phase::Grace {
                        until: now + INTERRUPT_GRACE,
                    };
                }
            }
            Phase::Grace { until } => {
                if control.cancelled() || control.escalated() || expired || now >= until {
                    // The interruption stays the recorded reason.
                    self.kill(StopReason::Cancelled, now);
                }
            }
            Phase::Kill { .. } => {}
        }
    }
    fn unknown_due(&self, now: Instant) -> bool {
        !self.unknown_announced
            && (self.wait_uncertain
                || matches!(self.phase, Phase::Kill { observe_until } if now >= observe_until))
    }
    /// Announces unknown termination once. The keep flag is set before anything
    /// else: whatever happens next, the input is not dropped by this thread.
    fn announce_unknown(&mut self, input: Option<&dyn RetainedInput>) {
        self.held.keep.store(true, Ordering::Release);
        let mut end = UnknownEnd {
            runtime: self.runtime,
            leader_pid: self.child.id(),
            process_group: self.child.id(),
            stop: self.stop.unwrap_or(Stop {
                reason: StopReason::WaitUnavailable,
                forwarded: None,
                kill_sent: false,
            }),
            leader_reaped: self.status.is_some(),
            wait_uncertain: self.wait_uncertain,
            stdout_open: !self.stdout.as_ref().is_some_and(|pipe| pipe.eof),
            stderr_open: !self.stderr.as_ref().is_some_and(|pipe| pipe.eof),
            retained_at: None,
        };
        if let Some(input) = input {
            end.retained_at = input.termination_unknown(&end);
        }
        self.unknown_announced = true;
        let _ = self.sender.try_send(InvocationEnd::Unknown(end));
    }
}
impl Drop for ChildWork<'_> {
    fn drop(&mut self) {
        if self.status.is_some() && self.pipes_closed() {
            return;
        }
        // Unwinding: nothing may release the input ahead of the uncollected
        // command, and the observer must not wait without bound. No code of
        // the input runs here.
        if self.held.kept() && !self.unknown_announced {
            self.announce_unknown(None);
        }
        if !self.unknown_announced {
            self.kill(StopReason::Cancelled, Instant::now());
        }
        loop {
            self.poll();
            if self.status.is_some() && self.pipes_closed() {
                return;
            }
            if self.unknown_due(Instant::now()) {
                self.announce_unknown(None);
            }
            thread::park_timeout(POLL);
        }
    }
}
