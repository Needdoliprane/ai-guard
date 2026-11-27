//! Linux custody of the original detached guardian in the local test fixture.
//! Once captured, an owned pidfd identifies the process across broker restart.
#![cfg(target_os = "linux")]

use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, File},
    io::{self, Read},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::MetadataExt,
    },
    path::Path,
    time::{Duration, Instant},
};
use tokio::net::UnixStream;

#[derive(PartialEq, Eq)]
struct ProcessFacts {
    parent: libc::pid_t,
    session: libc::pid_t,
    start_ticks: u64,
    executable: (u64, u64),
}

fn facts(pid: libc::pid_t) -> Result<ProcessFacts> {
    ensure!(pid > 1, "category=invalid_pid");
    let mut stat = String::new();
    File::open(format!("/proc/{pid}/stat"))?
        .take(4097)
        .read_to_string(&mut stat)?;
    ensure!(stat.len() <= 4096, "category=process_stat_bound");
    let (_, fields) = stat
        .rsplit_once(')')
        .context("category=invalid_process_stat")?;
    let mut fields = fields.split_whitespace();
    let state = fields.next().context("category=missing_process_state")?;
    ensure!(!matches!(state, "Z" | "X" | "x"), "category=process_exited");
    let parent = fields
        .next()
        .context("category=missing_process_parent")?
        .parse()?;
    let session = fields
        .nth(1)
        .context("category=missing_process_session")?
        .parse()?;
    let start_ticks = fields
        .nth(15)
        .context("category=missing_process_start")?
        .parse()?;
    let executable = fs::metadata(format!("/proc/{pid}/exe"))?;
    Ok(ProcessFacts {
        parent,
        session,
        start_ticks,
        executable: (executable.dev(), executable.ino()),
    })
}

fn peer_pid(stream: &UnixStream) -> Result<libc::pid_t> {
    let credentials = stream.peer_cred()?;
    // SAFETY: read-only effective UID of the current fixture process.
    ensure!(
        credentials.uid() == unsafe { libc::geteuid() },
        "category=guardian_peer_uid"
    );
    let pid = credentials
        .pid()
        .context("category=guardian_peer_pid_unavailable")?;
    ensure!(pid > 1, "category=invalid_guardian_peer_pid");
    Ok(pid)
}

pub(super) struct LinuxIdentity {
    pid: libc::pid_t,
    start_ticks: u64,
    executable: (u64, u64),
    pidfd: OwnedFd,
}

impl LinuxIdentity {
    pub(super) fn capture(stream: &UnixStream, broker_pid: u32, expected: &Path) -> Result<Self> {
        let broker_pid = libc::pid_t::try_from(broker_pid)?;
        let broker = facts(broker_pid)?;
        let pid = peer_pid(stream)?;
        let guardian = facts(pid)?;
        let expected = fs::metadata(expected)?;
        ensure!(expected.is_file(), "category=expected_fixture_executable");
        let executable = (expected.dev(), expected.ino());
        ensure!(
            broker.parent == libc::pid_t::try_from(std::process::id())?
                && broker.executable == executable
                && guardian.parent == broker_pid
                && guardian.session == pid
                && guardian.start_ticks >= broker.start_ticks
                && guardian.executable == executable,
            "category=guardian_not_owned_detached_child"
        );
        // SAFETY: pidfd_open retains a kernel handle for this observed child.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        ensure!(fd >= 0, "category=guardian_pidfd_unavailable");
        // SAFETY: success returned a new owned descriptor.
        let pidfd = unsafe { OwnedFd::from_raw_fd(libc::c_int::try_from(fd)?) };
        let identity = Self {
            pid,
            start_ticks: guardian.start_ticks,
            executable,
            pidfd,
        };
        identity.revalidate_retained_peer(stream)?;
        ensure!(
            facts(pid)? == guardian && facts(broker_pid)? == broker,
            "category=guardian_spawn_changed"
        );
        Ok(identity)
    }

    pub(super) fn exited(&self) -> Result<bool> {
        let mut poll = libc::pollfd {
            fd: self.pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one owned descriptor and one writable pollfd. Zero wait.
        let ready = unsafe { libc::poll(&mut poll, 1, 0) };
        if ready == -1 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        ensure!(
            ready >= 0 && poll.revents & (libc::POLLERR | libc::POLLNVAL) == 0,
            "category=guardian_exit_observation_unavailable"
        );
        Ok(ready == 1 && poll.revents & (libc::POLLIN | libc::POLLHUP) != 0)
    }

    pub(super) fn revalidate_retained_peer(&self, stream: &UnixStream) -> Result<()> {
        ensure!(
            peer_pid(stream)? == self.pid && !self.exited()?,
            "category=guardian_peer_identity_changed"
        );
        let guardian = facts(self.pid)?;
        ensure!(
            guardian.start_ticks == self.start_ticks
                && guardian.session == self.pid
                && guardian.executable == self.executable,
            "category=guardian_process_identity_changed"
        );
        Ok(())
    }

    pub(super) fn signal_retained_peer(&self, stream: &UnixStream) -> Result<()> {
        self.revalidate_retained_peer(stream)?;
        // SAFETY: only the retained pidfd is targeted, even if its PID is reused.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.pidfd.as_raw_fd(),
                libc::SIGTERM,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        ensure!(rc == 0, "category=guardian_identity_signal_refused");
        Ok(())
    }

    pub(super) async fn wait_exit(&self) -> Result<()> {
        let until = Instant::now() + Duration::from_secs(5);
        while !self.exited()? {
            ensure!(Instant::now() < until, "category=guardian_exit_unconfirmed");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }
}
