//! Exclusive stdin for the prearmed, non-TTY start transport.
//! The direct profile requires Tty=false and OpenStdin=true. The preflight
//! observed AttachStdin and StdinOnce both true on Docker and both false on
//! Podman. The managed caller must attest these facts before handing off.
use super::{
    ExecutionResult, ManagedRuntime, OwnedContainerSnapshot, TransportTasks, capture_stream,
    capture_stream_tapped, ensure_prearmed_live, wait_prearmed_stop,
};
use crate::broker::enforcer::CancelGeneration;
use anyhow::{Context, Result, ensure};
use std::{
    fmt,
    future::Future,
    io,
    os::fd::{AsRawFd, OwnedFd},
    pin::Pin,
    time::Duration,
};
use tokio::{
    io::{Interest, unix::AsyncFd},
    process::Child,
    sync::watch,
    time::{Instant, timeout, timeout_at},
};

/// Keep the single write within the POSIX minimum atomic pipe write size.
pub(crate) const MAX_BOOTSTRAP_STDIN_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BootstrapStdinSubmitError {
    /// The guard refused the submission before any write.
    Refused,
    /// The write did not report a complete submission. Reconcile the
    /// original instance and keep the original deadline.
    DeliveryUnknown,
}

impl fmt::Display for BootstrapStdinSubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Refused => "bootstrap stdin submission refused",
            Self::DeliveryUnknown => "bootstrap stdin submission requires reconciliation",
        })
    }
}

impl std::error::Error for BootstrapStdinSubmitError {}

/// Owns the only write endpoint provided by the newly spawned CLI. Dropping
/// it closes that endpoint, including on refusal or cancelled preparation.
pub(super) struct PrearmedBootstrapStdin(AsyncFd<OwnedFd>);

impl PrearmedBootstrapStdin {
    /// Take the stdin pipe immediately after spawning the admitted prearmed
    /// start and before transferring Child to an IO owner. The caller must use
    /// this only for the attested direct PID 1 transport, without archive or TTY.
    pub(super) fn take(child: &mut Child) -> Result<Self> {
        let stdin = child
            .stdin
            .take()
            .context("missing prearmed bootstrap stdin")?;
        let fd = stdin.into_owned_fd()?;
        // Tokio's conversion makes this endpoint blocking. Restore nonblocking
        // mode before registering it and before it can reach a release guard.
        // SAFETY: fd is still live and both fcntl commands use their integer ABI.
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
        if flags == -1
            || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            return Err(io::Error::last_os_error()).context("configure prearmed bootstrap stdin");
        }
        Ok(Self(AsyncFd::with_interest(fd, Interest::WRITABLE)?))
    }

    /// Wait outside the cancellation gate, within the caller's preparation
    /// bound. This is local pipe readiness. Attachment and the container
    /// incarnation require a separate runtime observation.
    pub(crate) async fn wait_writable(&self, until: Instant) -> Result<()> {
        let ready = timeout_at(until, self.0.writable())
            .await
            .context("prearmed bootstrap stdin readiness timed out")??;
        ensure!(
            ready.ready().is_writable() && !ready.ready().is_write_closed(),
            "prearmed bootstrap stdin closed before readiness"
        );
        Ok(())
    }

    /// Consume the only write right. The broker supplies the final synchronous
    /// check of the live channel, current authority, inspected incarnation
    /// and original native deadline. Readiness, inspection and durable
    /// acknowledgements must already have completed outside the gate.
    ///
    /// Success establishes only submission to the local CLI pipe.
    pub(super) fn submit_once(
        self,
        frame: &[u8],
        generation: &CancelGeneration,
        expected: u64,
        check_release: impl FnOnce() -> Result<()>,
    ) -> Result<(), BootstrapStdinSubmitError> {
        use BootstrapStdinSubmitError::{DeliveryUnknown, Refused};

        if frame.is_empty() || frame.len() > MAX_BOOTSTRAP_STDIN_BYTES {
            return Err(Refused);
        }
        let fd = self.0.get_ref().as_raw_fd();
        let result = generation.with_current_generation(expected, || {
            check_release().map_err(|_| Refused)?;
            // SAFETY: self owns the live, nonblocking pipe and frame is
            // readable for its bounded length. The Rust broker uses the
            // default ignored SIGPIPE disposition. A broken pipe is an error.
            let written = unsafe { libc::write(fd, frame.as_ptr().cast(), frame.len()) };
            if written == frame.len() as libc::ssize_t {
                Ok(())
            } else {
                Err(DeliveryUnknown)
            }
        });
        // Deregistration and close stay outside the generation gate.
        drop(self);
        result.map_err(|_| Refused)?
    }
}

/// The callback is scoped to the owned transport. A pending handoff and its
/// stdin are dropped if STOP wins or the original CLI exits.
pub(crate) type BootstrapReady = Box<
    dyn for<'a> FnOnce(
            PreparedBootstrapStdin<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>
        + Send,
>;

/// A single write right for the inspected running incarnation. The snapshot
/// and STOP receiver borrow the supervised handoff's lifetime.
pub(crate) struct PreparedBootstrapStdin<'a> {
    observed: &'a OwnedContainerSnapshot,
    stop: &'a watch::Receiver<bool>,
    stdin: PrearmedBootstrapStdin,
}

impl PreparedBootstrapStdin<'_> {
    #[allow(dead_code)] // Consumed by the separate broker composition.
    pub(crate) fn owned(&self) -> &OwnedContainerSnapshot {
        self.observed
    }

    #[allow(dead_code)] // Consumed by the separate broker composition.
    pub(crate) fn submit_once(
        self,
        frame: &[u8],
        generation: &CancelGeneration,
        expected: u64,
        check_release: impl FnOnce() -> Result<()>,
    ) -> Result<(), BootstrapStdinSubmitError> {
        let Self { stdin, stop, .. } = self;
        stdin.submit_once(frame, generation, expected, || {
            ensure_prearmed_live(stop)?;
            check_release()
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn supervise(
    authority: &ManagedRuntime,
    created: &OwnedContainerSnapshot,
    mut child: Child,
    stdin: Result<PrearmedBootstrapStdin>,
    on_ready: BootstrapReady,
    mut stop: watch::Receiver<bool>,
    stdout_limit: usize,
    stdout_tap: Option<crate::provider_activity::StdoutTap>,
) -> Result<ExecutionResult> {
    let result = async move {
        let stdin = stdin.context("cannot retain the prearmed bootstrap stdin")?;
        let start = Instant::now();
        let stdout_release = stdout_tap
            .as_ref()
            .map(crate::provider_activity::StdoutTap::stall_release);
        let mut tasks = TransportTasks::default();
        let stdout = tokio::spawn(capture_stream_tapped(
            child.stdout.take().context("missing stdout")?,
            stdout_limit,
            stdout_tap,
        ));
        tasks.0.push(stdout.abort_handle());
        let stderr = tokio::spawn(capture_stream(
            child.stderr.take().context("missing stderr")?,
            256 * 1024,
        ));
        tasks.0.push(stderr.abort_handle());

        let ready_stop = stop.clone();
        tokio::select! {
            biased;
            stopped = wait_prearmed_stop(&mut stop, stdout_release.as_ref()) => {
                stopped?;
                anyhow::bail!("stop signalled before the retained bootstrap handoff completed");
            }
            status = child.wait() => {
                status.context("retained bootstrap CLI wait failed")?;
                anyhow::bail!("retained bootstrap CLI exited before handoff completed");
            }
            ready = async move {
                let mut interval = tokio::time::interval(Duration::from_millis(200));
                let observed = loop {
                    interval.tick().await;
                    if let Some(observed) = authority.inspect_bootstrap_running(created).await? {
                        break observed;
                    }
                };
                stdin.wait_writable(Instant::now() + Duration::from_secs(5)).await?;
                on_ready(PreparedBootstrapStdin {
                    observed: &observed,
                    stop: &ready_stop,
                    stdin,
                })
                .await
            } => ready?,
        }

        let finish = async {
            let status = child.wait().await?;
            if let Some(release) = &stdout_release {
                release.send_replace(true);
            }
            let state = authority
                .inspect_process(created.handle.id(), &created.observation)
                .await?;
            state.validate_terminal(authority.binding().kind)?;
            let stdout = timeout(Duration::from_secs(10), stdout)
                .await
                .context("stdout pipe did not close")???;
            let stderr = timeout(Duration::from_secs(10), stderr)
                .await
                .context("stderr pipe did not close")???;
            Ok(ExecutionResult {
                stdout,
                stderr,
                state,
                exit_code: status.code(),
                timed_out: false,
                duration_ms: start.elapsed().as_millis(),
                transport_stop_reason: None,
            })
        };
        tokio::select! {
            biased;
            stopped = wait_prearmed_stop(&mut stop, stdout_release.as_ref()) => {
                stopped?;
                anyhow::bail!("stop signalled during retained bootstrap execution");
            }
            result = finish => result,
        }
    }
    .await;

    // The owned handoff and attach CLI have been dropped. Request stop only
    // for the snapshot of this attempt. An uncertain effect stays with the
    // independently armed enforcer and reconciler.
    if let Err(error) = result {
        return match authority.stop_snapshot_owned(created).await {
            Ok(_) => Err(error.context("retained bootstrap failed; owned snapshot retained")),
            Err(_) => {
                Err(error.context("retained bootstrap failed; exact-instance stop unconfirmed"))
            }
        };
    }
    result
}
