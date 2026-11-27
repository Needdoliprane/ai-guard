//! Bidirectional conversation adapter for SendSessionMessage, over the
//! documented Codex app-server JSON-RPC stdio protocol (`turn/start` request,
//! typed response carrying the turn id, correlated `turn/completed`
//! notification). See `docs/engineering/evidence/2026-11-25/provider-conversation.fr.md`.
//! The [claude] module correlates Claude Code stream-json on its own wire format.
//!
//! This module is the correlation and framing layer only. It never opens a
//! network socket, never enables Bridge, never loosens a preset and never falls
//! back to a TTY or a substituted command: it writes one complete framed turn
//! to the exact owned stdio channel and reconciles the exact correlated
//! response. The durable control plane (enqueue, emission issue, transmission,
//! acknowledgement) lives in `audit::session_message`; this adapter decides
//! whether an observed frame is a valid acknowledgement of the exact issued
//! instruction. Real Codex/Claude CLI reception is a separate qualification
//! (NOT_RUN here): the behavior below is proven against a controlled stdio
//! fixture child, not a provider.
//!
//! Truthful delivery: a complete write is `transmitted`; only an exact
//! correlated typed response is `acknowledged`; a discordant id, a divergent
//! provenance or a duplicate never advances the state. After a possible
//! emission with no reconcilable response, delivery stays unknown and is never
//! automatically resent.

use super::{bounded, content_hash, invalid};
use serde_json::Value;
use std::{
    io::{self, Write},
    os::fd::AsRawFd,
    time::{Duration, Instant},
};

pub mod claude;

/// Upper bound of one framed conversation line (request or response). A frame
/// beyond this bound is refused, never partially written or read.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
const MAX_WRITE_CHUNK_BYTES: usize = 4 * 1024;
const JSON_FRAME_END: &[u8; 2] = b"}\n";
const FRAME_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_RETRY_INTERVAL: Duration = Duration::from_millis(5);
const CHILD_REAP_TIMEOUT: Duration = Duration::from_secs(2);
const CHILD_REAP_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The exact `turn/start` request the control writes for one instruction. The
/// control correlates it locally by `rpc_id`; the operation identity and the
/// content fingerprint never leave the trusted control toward the provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnRequestV1 {
    pub rpc_id: u64,
    pub thread_id: String,
    pub text: String,
}

impl TurnRequestV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        bounded(&self.thread_id, 128)?;
        invalid(
            !self.text.is_empty()
                && self.text.len() <= super::session_control::SESSION_MESSAGE_MAX_CONTENT_BYTES,
        )?;
        Ok(())
    }

    /// The complete JSON-RPC frame, newline-terminated by the transport.
    pub fn frame(&self) -> anyhow::Result<String> {
        self.validate()?;
        let value = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.rpc_id,
            "method": "turn/start",
            "params": {
                "threadId": self.thread_id,
                "input": [{"type": "text", "text": self.text}],
            },
        });
        let frame = serde_json::to_string(&value)?;
        invalid(frame.len() <= MAX_FRAME_BYTES)?;
        Ok(frame)
    }
}

/// What one observed response frame means for the exact issued turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AckObservationV1 {
    /// The typed response to the exact issued RPC, carrying the provider turn
    /// id. This is the only observation that may record an acknowledgement.
    Acknowledged { turn_id: String },
    /// A response whose RPC id does not match the issued request: it can never
    /// acknowledge this instruction.
    DiscordantId,
    /// The provider refused the turn (typed JSON-RPC error).
    Rejected,
    /// A well-formed response that carries no usable turn id.
    Malformed,
    /// Not a response to a pending request (notification or unrelated frame).
    NotAResponse,
}

/// What one observed `turn/completed` notification means for the acknowledged
/// turn. The turn result is a distinct fact from the input acknowledgement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutcomeObservationV1 {
    /// The notification correlates to the exact acknowledged turn.
    Completed { turn_id: String, status: String },
    /// A notification for a different turn: divergent provenance, ignored.
    DivergentProvenance,
    /// Not a turn/completed notification.
    NotAnOutcome,
}

/// Parse one observed response frame against the exact issued RPC id. Pure:
/// the caller decides durably whether to record the acknowledgement.
pub fn observe_response(frame: &Value, issued_rpc_id: u64) -> AckObservationV1 {
    if frame.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return AckObservationV1::NotAResponse;
    }
    let Some(id) = frame.get("id").and_then(Value::as_u64) else {
        return AckObservationV1::NotAResponse;
    };
    if id != issued_rpc_id {
        return AckObservationV1::DiscordantId;
    }
    if frame.get("error").is_some() {
        return AckObservationV1::Rejected;
    }
    let turn_id = frame
        .get("result")
        .and_then(|result| result.get("turn"))
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str);
    match turn_id {
        Some(turn_id) if !turn_id.is_empty() && turn_id.len() <= 128 => {
            AckObservationV1::Acknowledged {
                turn_id: turn_id.to_string(),
            }
        }
        _ => AckObservationV1::Malformed,
    }
}

/// Parse one observed notification against the exact acknowledged turn id.
pub fn observe_outcome(frame: &Value, acknowledged_turn_id: &str) -> OutcomeObservationV1 {
    if frame.get("method").and_then(Value::as_str) != Some("turn/completed") {
        return OutcomeObservationV1::NotAnOutcome;
    }
    let turn = frame.get("params").and_then(|params| params.get("turn"));
    let turn_id = turn.and_then(|turn| turn.get("id")).and_then(Value::as_str);
    match turn_id {
        Some(turn_id) if turn_id == acknowledged_turn_id => OutcomeObservationV1::Completed {
            turn_id: turn_id.to_string(),
            status: turn
                .and_then(|turn| turn.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
        },
        _ => OutcomeObservationV1::DivergentProvenance,
    }
}

/// The correlation state of one in-flight conversation turn. At most one turn
/// is in flight: a second instruction before the first turn's outcome is
/// refused, never interleaved. The content fingerprint binds the exact text.
#[derive(Clone, Debug)]
pub struct ConversationTurnStateV1 {
    pub operation_id: uuid::Uuid,
    pub rpc_id: u64,
    pub thread_id: String,
    pub content_hash: String,
    /// Set only by an exact correlated typed response.
    pub acknowledged_turn_id: Option<String>,
    /// Set only by a correlated turn/completed notification.
    pub completed: bool,
}

/// Drives one conversation channel: issues a single in-flight turn, then
/// reconciles responses and outcomes. It holds no durable state itself; the
/// trusted control records each durable fact through `audit::session_message`.
#[derive(Default)]
pub struct ConversationAdapterV1 {
    next_rpc_id: u64,
    pending: Option<ConversationTurnStateV1>,
}

/// Why a new turn cannot be issued now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnRefusalV1 {
    /// A previous turn is still in flight (one turn at a time).
    TurnInFlight,
}
impl std::fmt::Display for TurnRefusalV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "conversation turn refused: {self:?}")
    }
}
impl std::error::Error for TurnRefusalV1 {}

impl ConversationAdapterV1 {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pending(&self) -> Option<&ConversationTurnStateV1> {
        self.pending.as_ref()
    }

    /// Issue one turn. Refuses while another turn is in flight. Returns the
    /// exact frame to write; the caller writes it completely, then records the
    /// transmission durably.
    pub fn issue_turn(
        &mut self,
        operation_id: uuid::Uuid,
        thread_id: &str,
        text: &str,
    ) -> anyhow::Result<TurnRequestV1> {
        anyhow::ensure!(self.pending.is_none(), TurnRefusalV1::TurnInFlight);
        let content_hash = content_hash("ai-guard:session-message-content:v1", &text)?;
        self.next_rpc_id = self
            .next_rpc_id
            .checked_add(1)
            .ok_or(super::ControlFailure::InvalidRequest)?;
        let request = TurnRequestV1 {
            rpc_id: self.next_rpc_id,
            thread_id: thread_id.to_string(),
            text: text.to_string(),
        };
        request.validate()?;
        self.pending = Some(ConversationTurnStateV1 {
            operation_id,
            rpc_id: request.rpc_id,
            thread_id: request.thread_id.clone(),
            content_hash,
            acknowledged_turn_id: None,
            completed: false,
        });
        Ok(request)
    }

    /// Reconcile one observed response frame. Only an exact correlated typed
    /// response sets the acknowledged turn id; a duplicate after the ack is
    /// ignored (returns `None`), never a second acknowledgement.
    pub fn reconcile_response(&mut self, frame: &Value) -> Option<AckObservationV1> {
        let pending = self.pending.as_mut()?;
        let observation = observe_response(frame, pending.rpc_id);
        if let AckObservationV1::Acknowledged { turn_id } = &observation {
            if pending.acknowledged_turn_id.is_some() {
                // Duplicate acknowledgement of the same turn: no new fact.
                return None;
            }
            pending.acknowledged_turn_id = Some(turn_id.clone());
        }
        Some(observation)
    }

    /// Reconcile one observed notification. Only the exact acknowledged turn's
    /// completion closes the turn; a duplicate completion is ignored.
    pub fn reconcile_outcome(&mut self, frame: &Value) -> Option<OutcomeObservationV1> {
        let pending = self.pending.as_mut()?;
        let turn_id = pending.acknowledged_turn_id.as_deref()?;
        let observation = observe_outcome(frame, turn_id);
        if let OutcomeObservationV1::Completed { .. } = &observation {
            if pending.completed {
                return None;
            }
            pending.completed = true;
        }
        Some(observation)
    }

    /// Close the current turn after its outcome (or an explicit stop), freeing
    /// the channel for the next instruction. Returns the closed turn state.
    pub fn close_turn(&mut self) -> Option<ConversationTurnStateV1> {
        self.pending.take()
    }
}

/// A complete framed write to the exact owned stdio channel, or a read of one
/// bounded response line with a deadline. The transport never interprets the
/// instruction as a shell command and never opens a network socket: it is a
/// pipe to the provider client process the trusted broker already owns.
pub struct StdioConversation {
    child: Option<std::process::Child>,
    retire: std::sync::mpsc::Sender<std::process::Child>,
    stdin: Option<std::process::ChildStdin>,
    lines: std::sync::mpsc::Receiver<std::io::Result<String>>,
}

/// A borrowed, currently usable nonblocking FIFO writer. Preparation does
/// not write to the pipe and precedes the durable emission latch.
pub(crate) struct ConversationInput<'a>(&'a mut std::process::ChildStdin);

impl StdioConversation {
    /// Spawn the provider client by program and separate arguments (no shell),
    /// with piped stdin/stdout. A background thread reads bounded lines so the
    /// caller can apply a deadline without blocking forever.
    pub fn spawn(program: &std::ffi::OsStr, args: &[std::ffi::OsString]) -> std::io::Result<Self> {
        Self::spawn_with_env(program, args, &[])
    }

    /// Spawn with explicit child environment entries, so a controlled fixture
    /// peer can be configured per channel without mutating the parent process.
    pub fn spawn_with_env(
        program: &std::ffi::OsStr,
        args: &[std::ffi::OsString],
        env: &[(std::ffi::OsString, std::ffi::OsString)],
    ) -> std::io::Result<Self> {
        require_waitable_children()?;
        let mut command = std::process::Command::new(program);
        command
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        for (key, value) in env {
            command.env(key, value);
        }
        // Reserve a reaper before spawning the child. An abandoned channel
        // hands off its child without waiting under the caller's locks.
        let (retire, abandoned) = std::sync::mpsc::channel::<std::process::Child>();
        let reaper = std::thread::Builder::new().spawn(move || {
            if let Ok(mut child) = abandoned.recv()
                && matches!(child.try_wait(), Ok(None))
            {
                let _ = child.kill();
                let _ = child.wait();
            }
        })?;
        drop(reaper);
        let mut child = command.spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let (sender, lines) = std::sync::mpsc::channel();
        let channel = Self {
            child: Some(child),
            retire,
            stdin,
            lines,
        };
        let stdout = stdout.ok_or_else(|| io::Error::other("conversation stdout unavailable"))?;
        let reader = std::thread::Builder::new().spawn(move || {
            use std::io::{BufRead, Read};
            let mut reader = std::io::BufReader::new(stdout);
            loop {
                let mut line = String::new();
                // Bound the read so a malicious or broken peer cannot exhaust
                // memory with an unterminated frame.
                let mut limited = (&mut reader).take(MAX_FRAME_BYTES as u64 + 1);
                match limited.read_line(&mut line) {
                    Ok(0) => {
                        let _ = sender.send(Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "conversation channel closed",
                        )));
                        break;
                    }
                    Ok(_) => {
                        if line.len() > MAX_FRAME_BYTES {
                            let _ = sender.send(Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "conversation frame exceeds its bound",
                            )));
                            break;
                        }
                        if !line.ends_with('\n') {
                            let _ = sender.send(Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "conversation response ended before the frame delimiter",
                            )));
                            break;
                        }
                        if sender.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
        })?;
        drop(reader);
        Ok(channel)
    }

    // The fixture can retain the peer past child teardown to observe every
    // byte already submitted to the stream, followed by EOF.
    #[cfg(test)]
    pub(crate) fn with_stdin_for_test(mut self, stdin: std::process::ChildStdin) -> Self {
        self.stdin = Some(stdin);
        self
    }

    /// Write one complete newline-terminated frame. A partial or
    /// failed write leaves delivery unknown; the caller never resends blindly.
    pub fn write_frame(&mut self, frame: &str) -> io::Result<()> {
        self.prepare_input()?
            .write_frame_guarded(frame, |_, _| Ok(()), |&()| Ok(()))
    }

    pub(crate) fn prepare_input(&mut self) -> io::Result<ConversationInput<'_>> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "stdin closed"))?;
        ConversationInput::certify_pipe(stdin)?;
        Ok(ConversationInput(stdin))
    }

    pub(crate) fn close_input(&mut self) {
        self.stdin = None;
    }

    /// Read the next non-empty line within the deadline. Harness noise lines
    /// that are not JSON objects are skipped by the caller, not here.
    pub fn read_line(&mut self, deadline: std::time::Duration) -> std::io::Result<String> {
        use std::sync::mpsc::RecvTimeoutError;

        self.lines
            .recv_timeout(deadline)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => {
                    io::Error::new(io::ErrorKind::TimedOut, "conversation read timed out")
                }
                RecvTimeoutError::Disconnected => {
                    io::Error::new(io::ErrorKind::UnexpectedEof, "conversation channel closed")
                }
            })?
    }

    /// Close the write side and reap the child. Used by STOP and by teardown.
    pub fn close(&mut self) {
        let _ = self.close_and_reap();
    }

    pub(crate) fn close_and_reap(&mut self) -> io::Result<()> {
        self.close_input();
        let Some(child) = &mut self.child else {
            return Ok(());
        };
        if child.try_wait()?.is_none()
            && let Err(error) = child.kill()
            && child.try_wait()?.is_none()
        {
            return Err(error);
        }
        let deadline = Instant::now() + CHILD_REAP_TIMEOUT;
        while child.try_wait()?.is_none() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "conversation child exit was not observed",
                ));
            }
            std::thread::sleep(remaining.min(CHILD_REAP_POLL_INTERVAL));
        }
        self.child = None;
        Ok(())
    }
}

/// Retaining a Child must retain the exited process's status and PID until
/// its owner waits. Query the inherited disposition without changing it.
fn require_waitable_children() -> io::Result<()> {
    let mut action = std::mem::MaybeUninit::<libc::sigaction>::uninit();
    // SAFETY: the null action queries SIGCHLD into the writable out-pointer.
    if unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), action.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful query initialized action.
    let action = unsafe { action.assume_init() };
    if action.sa_sigaction == libc::SIG_IGN || action.sa_flags & libc::SA_NOCLDWAIT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "conversation requires waitable children (SIGCHLD)",
        ));
    }
    Ok(())
}

impl ConversationInput<'_> {
    /// Enter the effect guard for each syscall, including the newline. Frame
    /// preparation and any wait for pipe capacity precede guard acquisition.
    pub(crate) fn write_frame_guarded<G, E>(
        &mut self,
        frame: &str,
        mut enter_effect: impl FnMut(usize, Instant) -> Result<G, E>,
        check_effect: impl Fn(&G) -> Result<(), E>,
    ) -> Result<(), E>
    where
        E: From<io::Error>,
    {
        if frame.len() > MAX_FRAME_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "frame too large").into());
        }
        let prefix = frame
            .strip_suffix('}')
            .filter(|prefix| {
                prefix.starts_with('{')
                    && !prefix.contains(['\n', '\r'])
                    && serde_json::from_str::<Value>(frame).is_ok_and(|value| value.is_object())
            })
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid JSON frame"))?
            .as_bytes();
        let deadline = Instant::now() + FRAME_WRITE_TIMEOUT;
        let mut written = 0;
        loop {
            let final_chunk = written == prefix.len();
            // An EOF reader may parse its last line without a newline. Keep
            // the JSON object unparseable until the atomic pipe write of }+LF.
            let chunk = if final_chunk {
                JSON_FRAME_END
            } else {
                &prefix[written..(written + MAX_WRITE_CHUNK_BYTES).min(prefix.len())]
            };
            let result = {
                let effect = enter_effect(written, deadline)?;
                check_effect(&effect)?;
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "conversation write timed out",
                    )
                    .into());
                }
                // ChildStdin is unbuffered. This is one nonblocking write;
                // the guard drops before retry or any wait.
                self.0.write(chunk)
            };
            match result {
                Ok(count) if final_chunk && count == JSON_FRAME_END.len() => return Ok(()),
                Ok(_) if final_chunk => {
                    return Err(
                        io::Error::other("incomplete atomic conversation terminator").into(),
                    );
                }
                Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero).into()),
                Ok(count) => written += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let delay = deadline.saturating_duration_since(Instant::now());
                    std::thread::sleep(WRITE_RETRY_INTERVAL.min(delay));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn certify_pipe(stdin: &std::process::ChildStdin) -> io::Result<()> {
        let fd = stdin.as_raw_fd();
        let mut facts = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: fd is owned and live, and facts is writable.
        if unsafe { libc::fstat(fd, facts.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful fstat initialized facts; fd is still live.
        let kind = unsafe { facts.assume_init() }.st_mode & libc::S_IFMT;
        // SAFETY: the descriptor remains owned for the path configuration query.
        if kind != libc::S_IFIFO
            || unsafe { libc::fpathconf(fd, libc::_PC_PIPE_BUF) }
                <= JSON_FRAME_END.len() as libc::c_long
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "input pipe atomicity unavailable",
            ));
        }
        // SAFETY: the owned ChildStdin keeps fd live for fcntl and poll.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if flags & libc::O_ACCMODE != libc::O_WRONLY {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "input pipe is not write-only",
            ));
        }
        // SAFETY: fd is retained and this sets only the nonblocking status flag.
        if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut readiness = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: one live descriptor, one writable pollfd and no wait.
        if unsafe { libc::poll(&mut readiness, 1, 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if readiness.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "stdin peer closed",
            ));
        }
        Ok(())
    }
}

impl Drop for StdioConversation {
    fn drop(&mut self) {
        self.close_input();
        if let Some(child) = self.child.take() {
            // The receiver waits for this single handoff until Drop.
            let _ = self.retire.send(child);
        }
    }
}

#[cfg(test)]
#[path = "session_message_tests.rs"]
mod tests;
