//! Operator interruption scope of one `build`, `up` or `down` command.
//!
//! The scope turns SIGINT, SIGTERM and SIGHUP into one stop request shared by
//! everything the command does with a prepared source. While work is counted
//! by a hold (a preparation, a retained engine command), the command future is
//! never dropped: that work ends by itself and reports what was observed. With
//! no hold, nothing consumes prepared material and the command is dropped.
use crate::runtime::retained_cli::{Interruption, signal_name};
use anyhow::{Context, Result, anyhow, bail};
use std::{future::Future, task::Poll};
use tokio::signal::unix::{Signal, SignalKind, signal};

tokio::task_local! {
    static SCOPE: Interruption;
}

/// Inside a scope.
pub(super) fn active() -> bool {
    SCOPE.try_with(|_| ()).is_ok()
}

/// The stop request of the scope this task runs in.
pub(super) fn current() -> Option<Interruption> {
    SCOPE.try_with(Clone::clone).ok()
}

/// Where stop requests come from. The scope logic is the same for the
/// process's signals and for a source a test drives.
pub(super) trait SignalSource {
    /// The next signal number. Never resolves when none can arrive.
    async fn next(&mut self) -> i32;
}

/// SIGINT, SIGTERM and SIGHUP of this process, except those it inherited as
/// ignored.
pub(super) struct OsSignals {
    streams: Vec<(i32, Signal)>,
}

impl OsSignals {
    /// Tokio installs its handler for the life of the process and would also
    /// replace an ignored disposition. A signal the process inherited as
    /// ignored (`nohup`, a supervisor) is therefore left alone: it stays
    /// ignored here and, through exec, in the engine CLI.
    ///
    /// A process whose children cannot be waited for is refused first, before
    /// any handler is installed and before the command does anything.
    pub(super) fn install() -> Result<Self> {
        require_waitable_children()?;
        let mut streams = Vec::new();
        for (number, kind) in [
            (libc::SIGINT, SignalKind::interrupt()),
            (libc::SIGTERM, SignalKind::terminate()),
            (libc::SIGHUP, SignalKind::hangup()),
        ] {
            if inherited_as_ignored(number)? {
                continue;
            }
            let stream =
                signal(kind).with_context(|| format!("cannot observe {}", signal_name(number)))?;
            streams.push((number, stream));
        }
        Ok(Self { streams })
    }
}

/// The disposition this process has for `number`. Nothing is installed.
fn disposition(number: i32) -> std::io::Result<libc::sigaction> {
    // SAFETY: an all-zero `sigaction` is a valid value for the kernel to
    // overwrite.
    let mut current: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: a null new action only queries the disposition into `current`,
    // which is valid for writes; nothing is installed.
    if unsafe { libc::sigaction(number, std::ptr::null(), &mut current) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(current)
}

fn inherited_as_ignored(number: i32) -> Result<bool> {
    let current = disposition(number)
        .with_context(|| format!("cannot read the disposition of {}", signal_name(number)))?;
    Ok(current.sa_sigaction == libc::SIG_IGN)
}

/// Refuses a process whose SIGCHLD is ignored or carries SA_NOCLDWAIT.
///
/// The kernel then discards the status of a child that exits. The owner of a
/// retained engine command could never collect it: the command would run to
/// its end and be reported as an unknown termination, with its prepared copy
/// kept, and the PID of its CLI process could be given to another process
/// before its group is signalled. The disposition belongs to whoever started
/// this process: it is read, never changed.
fn require_waitable_children() -> Result<()> {
    let current = disposition(libc::SIGCHLD).context("cannot read the disposition of SIGCHLD")?;
    let state = if current.sa_sigaction == libc::SIG_IGN {
        "is ignored"
    } else if current.sa_flags & libc::SA_NOCLDWAIT != 0 {
        "has SA_NOCLDWAIT set"
    } else {
        return Ok(());
    };
    bail!(
        "SIGCHLD {state} in this process, inherited from the program that started this CLI: the \
         wait status of an engine CLI process could not be obtained, so the end of an engine \
         command could not be established. Start this CLI with the default SIGCHLD disposition. \
         Nothing was prepared and no engine command was started"
    )
}

impl SignalSource for OsSignals {
    async fn next(&mut self) -> i32 {
        std::future::poll_fn(|context| {
            for (number, stream) in &mut self.streams {
                if let Poll::Ready(Some(())) = stream.poll_recv(context) {
                    return Poll::Ready(*number);
                }
            }
            Poll::Pending
        })
        .await
    }
}

/// Runs `command` inside a new scope fed by `signals`.
///
/// - The command's own result always wins: it is polled first.
/// - After a stop request, the command is dropped only once no hold exists.
///   Holds are created and released on the command's own task, so the count
///   changes only while the command is being polled, and a retained command
///   whose end was sent but not yet taken still counts.
/// - A termination announced as unknown keeps its hold for good; the command
///   then returns its own error and the scope never reports "nothing running".
pub(super) async fn scope_with<S, F>(mut signals: S, command: F) -> Result<()>
where
    S: SignalSource,
    F: Future<Output = Result<()>>,
{
    let interruption = Interruption::new();
    SCOPE
        .scope(interruption.clone(), async move {
            let mut command = std::pin::pin!(command);
            loop {
                match interruption.requested() {
                    None => tokio::select! {
                        biased;
                        result = &mut command => return result,
                        number = signals.next() => interruption.request(number),
                    },
                    Some(first) => tokio::select! {
                        biased;
                        result = &mut command => return result,
                        () = interruption.idle() => {
                            // Leaving this block drops the command future and
                            // with it every prepared source it owns, before
                            // the caller sees the result.
                            return Err(anyhow!(
                                "interrupted by {}: no engine build or Compose command was running",
                                signal_name(first)
                            ));
                        }
                        // A repeated request is merged or escalated by `Interruption`.
                        number = signals.next() => interruption.request(number),
                    },
                }
            }
        })
        .await
}
