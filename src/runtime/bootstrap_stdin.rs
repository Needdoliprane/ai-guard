//! Exclusive stdin for the prearmed, non-TTY start transport.
//! The direct profile requires Tty=false and OpenStdin=true. The preflight
//! observed AttachStdin and StdinOnce both true on Docker and both false on
//! Podman. The managed caller must attest these facts before handing off.
use crate::broker::enforcer::CancelGeneration;
use anyhow::{Context, Result, ensure};
use std::{
    fmt, io,
    os::fd::{AsRawFd, OwnedFd},
};
use tokio::{
    io::{Interest, unix::AsyncFd},
    process::Child,
    time::{Instant, timeout_at},
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
pub(crate) struct PrearmedBootstrapStdin(AsyncFd<OwnedFd>);

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
    pub(crate) fn submit_once(
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
