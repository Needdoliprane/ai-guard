//! Worker waiting mode of the existing `ai-guard-provider` binary (Kubernetes
//! V2, stage S2b; Root decision Q6).
//!
//! A session Pod starts the bootstrap program with the single argument
//! `SESSION_WAIT_MODE_ARG`. The mode answers one Hello with one Ready on its
//! standard streams and then accepts nothing: this stage of the protocol has
//! no Release, Start, credential handoff or business frame (R4 is not
//! delivered). It runs no program and reads no credential. The Ready echo is
//! declared metadata, never an identity proof. Hello and Ready carry
//! identifiers and digests, no secret; the Ready goes to standard output,
//! which the node runtime log keeps outside ai-guard.
//!
//! Environment: no code of this module reads an environment variable or
//! calls the provider path, so the provider handoff variable is never read.
//! The refusal of extra arguments builds no runtime. Serving the mode builds
//! a Tokio runtime, and the constructor of that runtime reads one Tokio
//! setting of its own, `TOKIO_UNSTABLE_SHARDED_BLOCKING_QUEUE` (tokio 1.54.1,
//! src/runtime/builder.rs:253-258 and 375).
//!
//! The mode never exits 0. An input that ends without a handoff is the
//! expected end of a preamble-only attach, and it is not the success of a
//! session.
//!
//! Arguments: the mode is recognized on the raw first argument, before any
//! text decoding. Once it is recognized, every following argument is refused
//! with `EXIT_PROTOCOL`, valid UTF-8 or not, and nothing reaches the provider
//! path. A first argument that is not valid UTF-8 is not the mode.
//!
//! Every item here has a caller outside tests through `dispatch_waiting_mode`,
//! which is public in the library, so the module carries no `dead_code`
//! attribute. The call from `src/bin/ai-guard-provider.rs` is a separate wire
//! patch that this lot does not apply.
use super::{
    bootstrap::{
        BootstrapCapacity, BootstrapError, MAX_HANDSHAKE_TIME, RESERVED_BYTES, WaitingForAttachV2,
        bootstrap_waiting_for_attach,
    },
    session_pod::SESSION_WAIT_MODE_ARG,
};
use std::{ffi::OsStr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    sync::Semaphore,
};

/// Refused input: a frame that is not the expected one, oversized, truncated
/// or followed by any byte after Ready, and any argument after the mode.
pub const EXIT_PROTOCOL: i32 = 64;
/// The Hello was not complete, or the Ready not written, within the handshake
/// window that the first input byte arms.
pub const EXIT_DEADLINE: i32 = 65;
/// The input ended and no handoff happened. Never a session success.
pub const EXIT_NO_HANDOFF: i32 = 66;
/// A read or write failed for another reason than the end of the input, or
/// the local runtime or reservation was unavailable.
pub const EXIT_IO: i32 = 70;

/// The two ends of the waiting mode that are not errors. Neither is a start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerBootstrapEndV2 {
    /// The input ended before any byte: nobody attached, or the attach closed
    /// before it wrote.
    EofBeforeHello,
    /// One Hello was answered by one Ready, then the input ended.
    ReadyThenEof,
}

/// What the arguments after the program name ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaitingModeRequest {
    /// Not the waiting mode: the caller keeps its existing path.
    Other,
    /// The mode alone.
    Serve,
    /// The mode followed by anything.
    Refused,
}

/// Only the first argument selects the mode, and only its exact spelling. The
/// comparison is on the raw value: the arguments that follow are counted,
/// never decoded, so one that is not valid UTF-8 is refused like any other.
fn waiting_mode_request<A: AsRef<OsStr>>(args: &[A]) -> WaitingModeRequest {
    let Some((mode, extra)) = args.split_first() else {
        return WaitingModeRequest::Other;
    };
    let mode: &OsStr = mode.as_ref();
    if mode != OsStr::new(SESSION_WAIT_MODE_ARG) {
        WaitingModeRequest::Other
    } else if extra.is_empty() {
        WaitingModeRequest::Serve
    } else {
        WaitingModeRequest::Refused
    }
}

/// Entry for the provider binary, called on its raw arguments (the values of
/// `std::env::args_os` after the program name), before it decodes them and
/// before its provider path. A slice of `String` is accepted as well.
/// `Some(code)` only when `args[0]` is exactly `SESSION_WAIT_MODE_ARG`:
/// `[mode]` runs the waiting mode on the process standard streams, and
/// `[mode, ..more]` returns `Some(EXIT_PROTOCOL)` without building a runtime or
/// reading anything, whether or not `more` is valid UTF-8. Every other argv
/// returns `None`, including one whose first argument is not valid UTF-8.
///
/// Once the mode is recognized the answer is always `Some`: the caller exits
/// with that code and never falls back to the provider path. Nothing here
/// reads the provider environment (see the module documentation for the one
/// Tokio setting that building the runtime of `[mode]` reads), and nothing
/// printed carries an identifier, an argument or frame content.
pub fn dispatch_waiting_mode<A: AsRef<OsStr>>(args: &[A]) -> Option<i32> {
    match waiting_mode_request(args) {
        WaitingModeRequest::Other => None,
        WaitingModeRequest::Refused => {
            diagnose("kubernetes session waiting mode: argument after the mode; refused");
            Some(EXIT_PROTOCOL)
        }
        WaitingModeRequest::Serve => Some(run_on_process_stdio()),
    }
}

/// Standard output carries only the Ready frame, flushed by the bootstrap
/// writer; the one diagnostic line goes to standard error.
fn run_on_process_stdio() -> i32 {
    // Only the time driver is needed: Tokio serves the standard streams from
    // its blocking pool, not from the IO driver.
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    else {
        diagnose("kubernetes session waiting mode: local runtime unavailable");
        return EXIT_IO;
    };
    let result = runtime.block_on(async {
        let capacity = worker_capacity()?;
        serve_waiting_bootstrap(tokio::io::stdin(), tokio::io::stdout(), capacity).await
    });
    // Tokio reads the process stdin with an ordinary blocking read on a pool
    // thread, and that read cannot be cancelled (tokio 1.54.1,
    // src/io/stdin.rs:15-19). A handshake deadline returns while the read is
    // still parked in the kernel with stdin open. Dropping the runtime would
    // then wait for the pool without limit (`BlockingPool::drop` calls
    // `shutdown(None)`, src/runtime/blocking/pool.rs:343-347): the process
    // would hang and its exit code would never be delivered.
    // `shutdown_background` (src/runtime/runtime.rs:494) does not wait. It
    // cannot lose the Ready frame: `write_frame` returns only after its flush
    // completed on that pool.
    runtime.shutdown_background();
    diagnose(waiting_diagnostic(&result));
    waiting_exit_code(&result)
}

/// One private pool sized for exactly one exchange: this process serves one
/// preamble and never a second one.
fn worker_capacity() -> Result<BootstrapCapacity, BootstrapError> {
    let permit = Arc::new(Semaphore::new(RESERVED_BYTES as usize))
        .try_acquire_many_owned(RESERVED_BYTES)
        .map_err(|_| BootstrapError::Capacity)?;
    BootstrapCapacity::new(permit)
}

/// Serves one waiting-mode exchange on any pair of streams.
///
/// - The input ends before its first byte: `Ok(EofBeforeHello)`.
/// - A Hello is answered by exactly one Ready. Then nothing is executed and
///   no frame is accepted: the end of the input is `Ok(ReadyThenEof)` and any
///   further byte is `Err(Protocol)`.
///
/// The wait for the first byte and the wait after Ready have no local
/// timeout: the contract gives the global duration to the Pod's
/// `activeDeadlineSeconds`. Only the handshake between them is bounded, by
/// `MAX_HANDSHAKE_TIME` from the first byte. Neither `Ok` value is a session
/// success, and `waiting_exit_code` maps none of them to 0.
pub(crate) async fn serve_waiting_bootstrap<R, W>(
    input: R,
    output: W,
    capacity: BootstrapCapacity,
) -> Result<WorkerBootstrapEndV2, BootstrapError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    serve_within(input, output, capacity, MAX_HANDSHAKE_TIME).await
}

/// `serve_waiting_bootstrap` with its handshake window as a parameter, so that
/// tests exercise the same code with a shorter window. The bootstrap reader
/// refuses a window above `MAX_HANDSHAKE_TIME`, so this cannot extend it.
async fn serve_within<R, W>(
    mut input: R,
    mut output: W,
    capacity: BootstrapCapacity,
    handshake: Duration,
) -> Result<WorkerBootstrapEndV2, BootstrapError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // `output` and `capacity` stay owned until the end: the stream is not
    // closed and the reservation is not returned before the exchange is over.
    match bootstrap_waiting_for_attach(&mut input, &mut output, &capacity, handshake).await? {
        WaitingForAttachV2::EofBeforeHello => Ok(WorkerBootstrapEndV2::EofBeforeHello),
        WaitingForAttachV2::ReadyWritten => end_after_ready(&mut input).await,
    }
}

/// After Ready there is no frame to accept and no timer to arm. One byte is
/// enough to refuse: nothing that follows it would be read or acted upon.
async fn end_after_ready<R: AsyncRead + Unpin>(
    input: &mut R,
) -> Result<WorkerBootstrapEndV2, BootstrapError> {
    let mut next = [0u8; 1];
    match input.read(&mut next).await {
        Ok(0) => Ok(WorkerBootstrapEndV2::ReadyThenEof),
        Ok(_) => Err(BootstrapError::Protocol),
        Err(_) => Err(BootstrapError::Io),
    }
}

/// Exit code of one end. The match has no wildcard: a new `BootstrapError`
/// variant fails to compile here until it is given a code. No value maps to 0.
pub(crate) fn waiting_exit_code(result: &Result<WorkerBootstrapEndV2, BootstrapError>) -> i32 {
    match result {
        Ok(WorkerBootstrapEndV2::EofBeforeHello | WorkerBootstrapEndV2::ReadyThenEof) => {
            EXIT_NO_HANDOFF
        }
        Err(
            BootstrapError::Protocol
            | BootstrapError::FrameSize
            | BootstrapError::PartialFrame
            | BootstrapError::Binding,
        ) => EXIT_PROTOCOL,
        Err(BootstrapError::Deadline) => EXIT_DEADLINE,
        Err(BootstrapError::Io | BootstrapError::Capacity | BootstrapError::Unavailable) => EXIT_IO,
    }
}

/// One fixed sentence per end, without wildcard for the same reason. A
/// `&'static str` cannot carry an identifier or a byte of a frame.
fn waiting_diagnostic(result: &Result<WorkerBootstrapEndV2, BootstrapError>) -> &'static str {
    match result {
        Ok(WorkerBootstrapEndV2::EofBeforeHello) => {
            "kubernetes session waiting mode: input ended before any frame; no handoff"
        }
        Ok(WorkerBootstrapEndV2::ReadyThenEof) => {
            "kubernetes session waiting mode: input ended after Ready; no handoff in this stage"
        }
        Err(BootstrapError::Protocol) => {
            "kubernetes session waiting mode: unexpected frame or byte; refused"
        }
        Err(BootstrapError::FrameSize) => {
            "kubernetes session waiting mode: frame length outside its bound; refused"
        }
        Err(BootstrapError::PartialFrame) => {
            "kubernetes session waiting mode: input ended inside a frame; refused"
        }
        Err(BootstrapError::Binding) => {
            "kubernetes session waiting mode: bootstrap metadata differs; refused"
        }
        Err(BootstrapError::Deadline) => {
            "kubernetes session waiting mode: handshake deadline elapsed"
        }
        Err(BootstrapError::Io) => "kubernetes session waiting mode: stream read or write failed",
        Err(BootstrapError::Capacity) => {
            "kubernetes session waiting mode: local reservation unavailable"
        }
        Err(BootstrapError::Unavailable) => {
            "kubernetes session waiting mode: local runtime unavailable"
        }
    }
}

/// Writes one fixed sentence to standard error. A failed write is ignored:
/// the exit code is the contract, and `eprintln!` would panic on a closed
/// stream and replace that code.
fn diagnose(sentence: &'static str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr().lock(), "{sentence}");
}

#[cfg(test)]
#[path = "session_worker_tests.rs"]
mod tests;
