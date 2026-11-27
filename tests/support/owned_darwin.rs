//! Process ownership checks for the disposable Darwin Full3 test fixture.
//!
//! The caller holds the original broker Child and validates a fresh
//! connection to the same published endpoint before closing. This module
//! compares the complete audit token and never signals by PID. The local ABI
//! and its behaviour under Seatbelt were checked by a separate Darwin probe.
#![cfg(target_os = "macos")]

use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsStr,
    io,
    mem::{MaybeUninit, size_of, size_of_val},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
    ptr,
    time::{Duration, Instant},
};
use tokio::net::UnixStream;

const CHILDREN_CAPACITY: usize = 8;
const EXIT_BUDGET: Duration = Duration::from_secs(5);
const REAP_BUDGET: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct AuditToken([libc::c_uint; 8]);

#[link(name = "bsm")]
unsafe extern "C" {
    fn audit_token_to_pid(token: AuditToken) -> libc::pid_t;
    fn audit_token_to_euid(token: AuditToken) -> libc::uid_t;
    fn audit_token_to_pidversion(token: AuditToken) -> libc::c_int;
}

#[link(name = "proc")]
unsafe extern "C" {
    fn proc_pidpath_audittoken(
        token: *mut AuditToken,
        buffer: *mut libc::c_void,
        size: u32,
    ) -> libc::c_int;
    fn proc_signal_with_audittoken(token: *mut AuditToken, signal: libc::c_int) -> libc::c_int;
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ProcessIdentity {
    pid: libc::pid_t,
    parent: libc::pid_t,
    uid: libc::uid_t,
    start: (u64, u64),
}

fn pid(native_id: u32) -> Result<libc::pid_t> {
    let value = libc::pid_t::try_from(native_id).context("category=invalid_pid")?;
    ensure!(value > 1, "category=invalid_pid");
    Ok(value)
}

fn own_uid() -> libc::uid_t {
    // SAFETY: reads only the effective identity of this test process.
    unsafe { libc::geteuid() }
}

fn process_identity(process: libc::pid_t) -> Result<ProcessIdentity> {
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: proc_pidinfo receives exactly one writable proc_bsdinfo buffer.
    let written = unsafe {
        libc::proc_pidinfo(
            process,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    ensure!(written == size, "category=bsd_process_identity_unavailable");
    // SAFETY: a full native proc_bsdinfo record was returned.
    let info = unsafe { info.assume_init() };
    ensure!(
        pid(info.pbi_pid)? == process
            && info.pbi_status != 0
            && info.pbi_status != libc::SZOMB
            && info.pbi_start_tvsec != 0
            && info.pbi_start_tvusec < 1_000_000,
        "category=bsd_process_identity_invalid"
    );
    Ok(ProcessIdentity {
        pid: process,
        parent: pid(info.pbi_ppid)?,
        uid: info.pbi_uid,
        start: (info.pbi_start_tvsec, info.pbi_start_tvusec),
    })
}

fn broker_identity(broker: libc::pid_t) -> Result<ProcessIdentity> {
    let identity = process_identity(broker)?;
    // SAFETY: reads only the PID of the current test process.
    let fixture = unsafe { libc::getpid() };
    ensure!(
        identity.parent == fixture && identity.uid == own_uid(),
        "category=broker_not_owned"
    );
    Ok(identity)
}

fn sole_child(broker: libc::pid_t) -> Result<Option<libc::pid_t>> {
    let mut children = [0 as libc::pid_t; CHILDREN_CAPACITY];
    // SAFETY: errno is thread-local. The fixed array is writable for its
    // whole byte size and only the call below may modify it.
    let (count, errno) = unsafe {
        let errno = libc::__error();
        *errno = 0;
        let count = libc::proc_listchildpids(
            broker,
            children.as_mut_ptr().cast(),
            size_of_val(&children) as libc::c_int,
        );
        (count, *errno)
    };
    match count {
        0 if errno == 0 && children.iter().all(|&child| child == 0) => Ok(None),
        1 if children[0] > 1 && children[1..].iter().all(|&child| child == 0) => {
            Ok(Some(children[0]))
        }
        _ => anyhow::bail!("category=broker_children_unknown"),
    }
}

fn read_peer_option<T>(stream: &UnixStream, option: libc::c_int) -> Result<T> {
    let mut value = MaybeUninit::<T>::uninit();
    let mut size = size_of::<T>() as libc::socklen_t;
    // SAFETY: UNIX socket options below write exactly pid_t or audit_token_t
    // to correctly sized output, with all bytes initialized on success.
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            option,
            value.as_mut_ptr().cast(),
            &mut size,
        )
    };
    ensure!(
        status == 0 && size as usize == size_of::<T>(),
        "category=guardian_peer_identity_unavailable"
    );
    // SAFETY: called only for exact native integer-record options.
    Ok(unsafe { value.assume_init() })
}

fn peer_identity(stream: &UnixStream) -> Result<(AuditToken, libc::pid_t)> {
    let audit = read_peer_option::<AuditToken>(stream, libc::LOCAL_PEERTOKEN)?;
    let pid = read_peer_option::<libc::pid_t>(stream, libc::LOCAL_PEERPID)?;
    // SAFETY: BSM decodes the native token obtained from this UNIX socket.
    let (token_pid, euid, version) = unsafe {
        (
            audit_token_to_pid(audit),
            audit_token_to_euid(audit),
            audit_token_to_pidversion(audit),
        )
    };
    ensure!(
        pid > 1 && token_pid == pid && euid == own_uid() && version != 0,
        "category=guardian_peer_identity_invalid"
    );
    Ok((audit, pid))
}

fn compare_executable(process: libc::pid_t, mut token: AuditToken, expected: &Path) -> Result<()> {
    let mut from_pid = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let mut from_token = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let size = from_pid.len() as u32;
    // SAFETY: both calls receive initialized buffers with the exact declared
    // capacity. The second additionally verifies the native process version.
    let (pid_size, token_size) = unsafe {
        (
            libc::proc_pidpath(process, from_pid.as_mut_ptr().cast(), size),
            proc_pidpath_audittoken(&mut token, from_token.as_mut_ptr().cast(), size),
        )
    };
    for (bytes, written) in [(&from_pid, pid_size), (&from_token, token_size)] {
        ensure!(
            written > 0 && (written as usize) < bytes.len(),
            "category=guardian_executable_unavailable"
        );
        let end = bytes
            .iter()
            .position(|&byte| byte == 0)
            .context("category=guardian_executable_invalid")?;
        ensure!(
            Path::new(OsStr::from_bytes(&bytes[..end])) == expected,
            "category=guardian_executable_changed"
        );
    }
    Ok(())
}

fn require_broker_executable(broker: libc::pid_t, expected: &Path) -> Result<()> {
    let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the exact declared capacity is available in this local buffer.
    let written =
        unsafe { libc::proc_pidpath(broker, path.as_mut_ptr().cast(), path.len() as u32) };
    ensure!(
        written > 0 && (written as usize) < path.len(),
        "category=broker_executable_unavailable"
    );
    let end = path
        .iter()
        .position(|&byte| byte == 0)
        .context("category=broker_executable_invalid")?;
    ensure!(
        Path::new(OsStr::from_bytes(&path[..end])) == expected,
        "category=broker_executable_changed"
    );
    Ok(())
}

struct ExitObservation {
    queue: OwnedFd,
    pid: libc::pid_t,
}

impl ExitObservation {
    fn arm(pid: libc::pid_t) -> Result<Self> {
        // SAFETY: kqueue creates a descriptor owned by this fixture.
        let fd = unsafe { libc::kqueue() };
        ensure!(fd >= 0, "category=guardian_exit_watch_unavailable");
        // SAFETY: the prior call returned a newly opened descriptor.
        let queue = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: use the standard close-on-exec flag on our own descriptor.
        let flags = unsafe { libc::fcntl(queue.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        ensure!(flags == 0, "category=guardian_exit_watch_setup");
        // SAFETY: kevent is a native integer/pointer record; zero is valid
        // for unused event fields including the null userdata pointer.
        let mut subscription: libc::kevent = unsafe { std::mem::zeroed() };
        subscription.ident = pid as libc::uintptr_t;
        subscription.filter = libc::EVFILT_PROC;
        subscription.flags = libc::EV_ADD | libc::EV_ONESHOT;
        subscription.fflags = libc::NOTE_EXIT;
        // SAFETY: the subscription buffer contains exactly one event. No
        // output is requested, so this only submits the registration.
        let rc = unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                &subscription,
                1,
                ptr::null_mut(),
                0,
                ptr::null(),
            )
        };
        ensure!(rc == 0, "category=guardian_exit_watch_setup");
        Ok(Self { queue, pid })
    }

    fn exited(&self) -> Result<bool> {
        let mut event = MaybeUninit::<libc::kevent>::uninit();
        let immediate = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: request at most one event into its exact writable buffer;
        // the nonblocking timeout applies only to this fixture queue.
        let rc = unsafe {
            libc::kevent(
                self.queue.as_raw_fd(),
                ptr::null(),
                0,
                event.as_mut_ptr(),
                1,
                &immediate,
            )
        };
        if rc == 0 || (rc == -1 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted)
        {
            return Ok(false);
        }
        ensure!(rc == 1, "category=guardian_exit_watch_failed");
        // SAFETY: exactly one initialized kevent has been returned.
        let event = unsafe { event.assume_init() };
        let identity = event.ident;
        let filter = event.filter;
        let flags = event.flags;
        let notes = event.fflags;
        ensure!(
            identity == self.pid as libc::uintptr_t
                && filter == libc::EVFILT_PROC
                && flags & libc::EV_ERROR == 0
                && notes & libc::NOTE_EXIT != 0,
            "category=guardian_exit_event_unexpected"
        );
        Ok(true)
    }

    async fn wait(&self) -> Result<()> {
        let deadline = Instant::now() + EXIT_BUDGET;
        while !self.exited()? {
            ensure!(
                Instant::now() < deadline,
                "category=guardian_exit_unconfirmed"
            );
            tokio::time::sleep(POLL).await;
        }
        Ok(())
    }
}

pub(super) struct DarwinIdentity {
    token: AuditToken,
    broker: ProcessIdentity,
    guardian: ProcessIdentity,
    exe: PathBuf,
    exit: ExitObservation,
}

impl DarwinIdentity {
    pub(super) async fn wait_exit(&self) -> Result<()> {
        self.exit.wait().await
    }

    pub(super) fn exited(&self) -> Result<bool> {
        self.exit.exited()
    }

    /// Legacy restart can reparent the same detached child. Verify its
    /// original kernel token and executable on the retained endpoint.
    pub(super) fn revalidate_retained_peer(&self, stream: &UnixStream) -> Result<()> {
        let (token, pid) = peer_identity(stream)?;
        ensure!(
            token == self.token && pid == self.guardian.pid && !self.exit.exited()?,
            "category=guardian_peer_identity_changed"
        );
        compare_executable(pid, token, &self.exe)
    }

    pub(super) fn signal_retained_peer(&self, stream: &UnixStream) -> Result<()> {
        self.revalidate_retained_peer(stream)?;
        self.signal_token()
    }

    pub(super) fn capture(
        stream: &UnixStream,
        broker_pid: u32,
        expected_exe: &Path,
    ) -> Result<Self> {
        let broker = broker_identity(pid(broker_pid)?)?;
        let (token, guardian_pid) = peer_identity(stream)?;
        let guardian = process_identity(guardian_pid)?;
        // SAFETY: getsid is identity-only and the child PID was validated.
        let session = unsafe { libc::getsid(guardian.pid) };
        ensure!(
            guardian.parent == broker.pid
                && guardian.uid == broker.uid
                && guardian.start >= broker.start
                && session == guardian.pid,
            "category=guardian_not_owned_detached_child"
        );
        let exe = expected_exe
            .canonicalize()
            .context("category=expected_executable_unavailable")?;
        let identity = Self {
            token,
            broker,
            guardian,
            exe,
            exit: ExitObservation::arm(guardian.pid)?,
        };
        identity.revalidate(stream, broker_pid, expected_exe)?;
        Ok(identity)
    }

    fn revalidate(&self, stream: &UnixStream, broker_pid: u32, expected_exe: &Path) -> Result<()> {
        ensure!(
            pid(broker_pid)? == self.broker.pid
                && expected_exe
                    .canonicalize()
                    .context("category=expected_executable_unavailable")?
                    == self.exe,
            "category=guardian_binding_changed"
        );
        let (token, guardian) = peer_identity(stream)?;
        ensure!(
            token == self.token && guardian == self.guardian.pid,
            "category=guardian_peer_identity_changed"
        );
        self.require_same_broker()?;
        ensure!(
            process_identity(guardian)? == self.guardian
                && sole_child(self.broker.pid)? == Some(guardian),
            "category=guardian_child_identity_changed"
        );
        compare_executable(guardian, token, &self.exe)?;
        require_broker_executable(self.broker.pid, &self.exe)?;
        self.require_same_broker()
    }

    fn require_same_broker(&self) -> Result<()> {
        ensure!(
            broker_identity(self.broker.pid)? == self.broker,
            "category=broker_identity_changed"
        );
        Ok(())
    }

    fn signal(&self, stream: &UnixStream, broker_pid: u32, expected_exe: &Path) -> Result<()> {
        self.revalidate(stream, broker_pid, expected_exe)?;
        ensure!(!self.exit.exited()?, "category=guardian_already_exited");
        self.signal_token()
    }

    fn signal_token(&self) -> Result<()> {
        let mut exact_token = self.token;
        // SAFETY: the native kernel operation validates the process identity
        // from the token. The PID is never used as a signal argument.
        let rc = unsafe { proc_signal_with_audittoken(&mut exact_token, libc::SIGTERM) };
        ensure!(
            rc == 0,
            "category=guardian_identity_signal_refused code={rc}"
        );
        Ok(())
    }

    async fn wait_reaped(&self) -> Result<()> {
        self.exit.wait().await?;
        let deadline = Instant::now() + REAP_BUDGET;
        loop {
            self.require_same_broker()?;
            match sole_child(self.broker.pid)? {
                None => return self.require_same_broker(),
                Some(child) if child == self.guardian.pid => {}
                Some(_) => anyhow::bail!("category=guardian_replacement_unconfirmed"),
            }
            ensure!(
                Instant::now() < deadline,
                "category=guardian_reap_unconfirmed"
            );
            tokio::time::sleep(POLL).await;
        }
    }

    pub(super) async fn close(
        self,
        stream: &UnixStream,
        broker_pid: u32,
        expected_exe: &Path,
    ) -> Result<()> {
        // Child processes used only for the synthetic runtime can take a
        // moment to exit after that runtime's socket and task have closed.
        // Retry proof only. The native signal itself is issued once.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.revalidate(stream, broker_pid, expected_exe) {
                Ok(()) => break,
                Err(error) if Instant::now() >= deadline => return Err(error),
                Err(_) => tokio::time::sleep(POLL).await,
            }
        }
        self.signal(stream, broker_pid, expected_exe)?;
        self.wait_reaped().await
    }
}
