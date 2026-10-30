//! Real Docker qualification of the interactive CLI through an isolated PTY.
//! Ignored by default: it starts retained worker instances on an explicitly
//! selected local daemon. It never invokes a provider or uses real credentials.
use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    control::{
        AllocationProvenanceV1, QuotaPolicyV1, RuntimeStatusV1, WorkerMutationV1, WorkerStateV1,
        WorkerTransitionV1,
    },
    operations::initialize_config,
    policy::Policy,
    redaction::Redactor,
    runner::{AuditClient, read_token},
};
use anyhow::{Context, Result, ensure};
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::IntoResponse,
};
use std::os::unix::{net::UnixStream, process::ExitStatusExt};
use std::{
    fs,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{fs::PermissionsExt, process::CommandExt},
    },
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const DOCKER: &str = "/opt/homebrew/bin/docker";
const ENDPOINT: &str = "unix:///Users/louis/.docker/run/docker.sock";
const DEADLINE: Duration = Duration::from_secs(150);
const PTY_CAPTURE_LIMIT: usize = 2 * 1024 * 1024;
const PTY_PUMP_READ_BUDGET: usize = 4 * 1024;
const PTY_FLOOD_BYTES: usize = 64 * 1024 * 1024;
const PTY_FLOOD_TEST_CAPTURE_LIMIT: usize = 16 * 1024;
const PTY_FLOOD_TEST_READ_BUDGET: usize = 64;

#[derive(Clone, Debug)]
struct TerminalFacts {
    logs_none: bool,
    log_options_empty: bool,
    tty: bool,
    open_stdin: bool,
    stdin_once: bool,
    running: bool,
    exit_code: i64,
}

async fn inspect_terminal_before_ack(endpoint: &str, container_id: &str) -> Result<TerminalFacts> {
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(DOCKER)
            .args([
                "--host",
                endpoint,
                "inspect",
                "--format",
                "{{.HostConfig.LogConfig.Type}}\n{{json .HostConfig.LogConfig.Config}}\n{{.Config.Tty}} {{.Config.OpenStdin}} {{.Config.StdinOnce}}\n{{.State.Running}} {{.State.ExitCode}}",
                container_id,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output(),
    )
    .await
    .context("pre-ACK Docker inspect exceeded deadline")??;
    ensure!(output.status.success(), "pre-ACK Docker inspect failed");
    let lines: Vec<_> = std::str::from_utf8(&output.stdout)?.lines().collect();
    ensure!(
        lines.len() == 4,
        "pre-ACK inspect returned an unexpected scalar shape"
    );
    let streams: Vec<_> = lines[2].split_whitespace().collect();
    let state: Vec<_> = lines[3].split_whitespace().collect();
    ensure!(
        streams.len() == 3 && state.len() == 2,
        "pre-ACK inspect returned malformed scalar facts"
    );
    Ok(TerminalFacts {
        logs_none: lines[0] == "none",
        log_options_empty: matches!(lines[1], "{}" | "null"),
        tty: streams[0] == "true",
        open_stdin: streams[1] == "true",
        stdin_once: streams[2] == "true",
        running: state[0] == "true",
        exit_code: state[1].parse()?,
    })
}

struct HostPty {
    master: fs::File,
    slave: OwnedFd,
    original: libc::termios,
}

impl HostPty {
    fn new() -> Result<Self> {
        let mut master = -1;
        let mut slave = -1;
        let mut size = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: openpty initializes both descriptors and receives a valid winsize pointer.
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        ensure!(rc == 0, "could not allocate private PTY");
        // SAFETY: successful openpty returned owned descriptors.
        let master = unsafe { fs::File::from_raw_fd(master) };
        // SAFETY: successful openpty returned owned descriptors.
        let slave = unsafe { OwnedFd::from_raw_fd(slave) };
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: slave is a valid tty descriptor and output pointer is writable.
        ensure!(
            unsafe { libc::tcgetattr(slave.as_raw_fd(), original.as_mut_ptr()) } == 0,
            "could not read isolated PTY termios"
        );
        // SAFETY: tcgetattr initialized the termios value.
        let original = unsafe { original.assume_init() };
        Ok(Self {
            master,
            slave,
            original,
        })
    }

    fn verify_broker_restored(&self) -> Result<()> {
        let mut current = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: output points to writable termios storage.
        // SAFETY: slave remains open while the host driver retains the PTY session.
        let result = unsafe { libc::tcgetattr(self.slave.as_raw_fd(), current.as_mut_ptr()) };
        ensure!(
            result == 0,
            "could not verify restored PTY termios: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: tcgetattr initialized current.
        let current = unsafe { current.assume_init() };
        #[cfg(target_os = "macos")]
        let pending_input = libc::PENDIN;
        #[cfg(not(target_os = "macos"))]
        let pending_input = 0;
        let lflag_delta = current.c_lflag ^ self.original.c_lflag;
        ensure!(
            lflag_delta & !pending_input == 0
                && current.c_iflag == self.original.c_iflag
                && current.c_oflag == self.original.c_oflag,
            "broker did not restore host PTY modes before cleanup (ICANON/ECHO and other flags must match; Darwin may add PENDIN when canonical mode resumes)"
        );
        ensure!(
            current.c_cflag == self.original.c_cflag && current.c_cc == self.original.c_cc,
            "broker did not restore host PTY control characters or control flags"
        );
        Ok(())
    }
}

impl Drop for HostPty {
    fn drop(&mut self) {
        // Cleanup only after `verify_broker_restored` had the chance to inspect
        // the broker's state. This fallback applies solely to the private PTY.
        // SAFETY: slave is a valid PTY and original was read from that descriptor.
        unsafe {
            libc::tcsetattr(self.slave.as_raw_fd(), libc::TCSANOW, &self.original);
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct BrokerStatus {
    code: Option<i32>,
    signal: Option<i32>,
}

fn broker_status_text(status: BrokerStatus) -> String {
    format!("code={:?},signal={:?}", status.code, status.signal)
}

fn try_wait_child_until(
    child: &mut std::process::Child,
    deadline: Instant,
) -> Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        thread::sleep(remaining.min(Duration::from_millis(10)));
    }
}

fn terminate_broker_group(
    child: &mut std::process::Child,
    group: libc::pid_t,
) -> Result<ExitStatus> {
    // This targets only the broker process group. Runtime instances are left
    // untouched; their state and volumes remain available for diagnosis.
    unsafe { libc::kill(-group, libc::SIGTERM) };
    if let Some(status) = try_wait_child_until(child, Instant::now() + Duration::from_secs(2))? {
        return Ok(status);
    }
    unsafe { libc::kill(-group, libc::SIGKILL) };
    try_wait_child_until(child, Instant::now() + Duration::from_secs(2))?
        .context("broker process did not exit after bounded process-group termination")
}

struct PtySession {
    master: fs::File,
    control: UnixStream,
    capture_limit: usize,
    read_budget: usize,
    captured: Vec<u8>,
    control_bytes: Vec<u8>,
    broker_status: Option<BrokerStatus>,
    driver_error: Option<String>,
    overflow: bool,
}

struct FloodWriter {
    stop: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<Result<()>>>,
    saturated: mpsc::Receiver<Result<()>>,
    progress: mpsc::Receiver<()>,
}

impl FloodWriter {
    fn start(slave_fd: libc::c_int) -> Result<Self> {
        // SAFETY: dup returns an independent descriptor owned by the File below.
        let writer_fd = unsafe { libc::dup(slave_fd) };
        ensure!(
            writer_fd >= 0,
            "could not duplicate PTY slave for flood producer"
        );
        // SAFETY: successful dup returned a descriptor with unique ownership here.
        let mut writer = unsafe { fs::File::from_raw_fd(writer_fd) };
        let flags = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) };
        ensure!(flags >= 0, "could not inspect flood writer flags");
        ensure!(
            unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                == 0,
            "could not make flood writer nonblocking"
        );
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = running.clone();
        let (ready_tx, saturated) = mpsc::sync_channel(1);
        let (progress_tx, progress) = mpsc::sync_channel(1);
        let join = thread::spawn(move || -> Result<()> {
            let chunk = [b'F'; 4096];
            let mut total = 0;
            while !thread_stop.load(Ordering::Acquire) && total < PTY_FLOOD_BYTES {
                match writer.write(&chunk) {
                    Ok(0) => thread::yield_now(),
                    Ok(count) => {
                        total += count;
                        let _ = progress_tx.try_send(());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        let _ = ready_tx.try_send(Ok(()));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        let _ = ready_tx.try_send(Err(anyhow::anyhow!(
                            "PTY flood producer write failed: {error}"
                        )));
                        thread_running.store(false, Ordering::Release);
                        return Err(error.into());
                    }
                }
            }
            thread_running.store(false, Ordering::Release);
            Ok(())
        });
        Ok(Self {
            stop,
            running,
            join: Some(join),
            saturated,
            progress,
        })
    }

    fn wait_until_saturated(&self) -> Result<()> {
        self.saturated
            .recv_timeout(Duration::from_secs(3))
            .context("PTY flood producer did not fill the output queue")??;
        Ok(())
    }

    fn clear_saturation(&self) {
        while self.saturated.try_recv().is_ok() {}
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn clear_progress(&self) {
        while self.progress.try_recv().is_ok() {}
    }

    fn wait_for_progress_until(&self, deadline: Instant) -> Result<()> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(
            !remaining.is_zero(),
            "PTY flood progress exceeded its overall deadline"
        );
        self.progress
            .recv_timeout(remaining)
            .context("flood producer did not resume after PTY output was read")?;
        Ok(())
    }

    fn stop_and_join(&mut self) -> Result<()> {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            join.join()
                .map_err(|_| anyhow::anyhow!("PTY flood producer panicked"))??;
        }
        Ok(())
    }
}

impl Drop for FloodWriter {
    fn drop(&mut self) {
        let _ = self.stop_and_join();
    }
}

impl PtySession {
    fn new(master: fs::File, control: UnixStream) -> Result<Self> {
        Self::with_limits(master, control, PTY_PUMP_READ_BUDGET, PTY_CAPTURE_LIMIT)
    }

    fn with_limits(
        master: fs::File,
        control: UnixStream,
        read_budget: usize,
        capture_limit: usize,
    ) -> Result<Self> {
        ensure!(
            read_budget > 0 && capture_limit > 0,
            "PTY session limits must be nonzero"
        );
        control.set_nonblocking(true)?;
        let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
        ensure!(flags >= 0, "could not read PTY master flags");
        ensure!(
            unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                == 0,
            "could not make PTY master nonblocking"
        );
        let checked = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
        ensure!(
            checked >= 0 && checked & libc::O_NONBLOCK != 0,
            "PTY master did not retain O_NONBLOCK"
        );
        Ok(Self {
            master,
            control,
            capture_limit,
            read_budget,
            captured: Vec::new(),
            control_bytes: Vec::new(),
            broker_status: None,
            driver_error: None,
            overflow: false,
        })
    }

    fn pump(&mut self, timeout: Duration) -> Result<bool> {
        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        let mut descriptors = [
            libc::pollfd {
                fd: self.master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.control.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: both pollfd values are initialized and the wait has a finite timeout.
        let ready =
            unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as _, timeout_ms) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(error.into());
        }
        let mut got_pty_bytes = false;
        if descriptors[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            let mut buffer = [0u8; 8192];
            let mut budget_remaining = self.read_budget;
            while budget_remaining > 0 {
                let read_size = buffer.len().min(budget_remaining);
                match self.master.read(&mut buffer[..read_size]) {
                    Ok(0) => break,
                    Ok(count) => {
                        got_pty_bytes = true;
                        budget_remaining -= count;
                        if self.captured.len() + count <= self.capture_limit {
                            self.captured.extend_from_slice(&buffer[..count]);
                        } else {
                            self.overflow = true;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error.into()),
                }
            }
        }
        if descriptors[1].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            let mut buffer = [0u8; 64];
            loop {
                match self.control.read(&mut buffer) {
                    Ok(0) => {
                        if self.broker_status.is_none() {
                            self.driver_error =
                                Some("private driver control pipe closed before status".into());
                        }
                        break;
                    }
                    Ok(count) => {
                        self.control_bytes.extend_from_slice(&buffer[..count]);
                        ensure!(
                            self.control_bytes.len() <= 128,
                            "driver status exceeded its scalar bound"
                        );
                        if self.control_bytes.contains(&b'\n') {
                            self.parse_driver_status()?;
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(got_pty_bytes)
    }

    fn parse_driver_status(&mut self) -> Result<()> {
        let line = std::str::from_utf8(&self.control_bytes)?;
        let line = line.trim();
        if let Some(error) = line.strip_prefix("DRIVER_ERROR ") {
            self.driver_error = Some(error.to_owned());
            return Ok(());
        }
        let (code, signal) = line
            .strip_prefix("BROKER_EXIT ")
            .context("host driver status was malformed")?
            .split_once(' ')
            .context("host driver status omitted signal field")?;
        self.broker_status = Some(BrokerStatus {
            code: (code != "none").then(|| code.parse()).transpose()?,
            signal: (signal != "none").then(|| signal.parse()).transpose()?,
        });
        Ok(())
    }

    fn wait_for(&mut self, marker: &[u8], deadline: Instant, phase: &str) -> Result<()> {
        loop {
            if self
                .captured
                .windows(marker.len())
                .any(|window| window == marker)
            {
                return Ok(());
            }
            if let Some(status) = self.broker_status {
                anyhow::bail!(
                    "broker exited before {phase}: {}",
                    broker_status_text(status)
                );
            }
            if let Some(error) = &self.driver_error {
                anyhow::bail!("host driver failed before {phase}: {error}");
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            ensure!(
                !remaining.is_zero(),
                "timed out waiting for PTY phase {phase}"
            );
            self.pump(remaining.min(Duration::from_millis(25)))?;
        }
    }

    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.master.write_all(bytes)?;
        Ok(())
    }

    fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: master is the test-owned PTY and size is initialized.
        ensure!(
            unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) } == 0,
            "PTY resize failed"
        );
        Ok(())
    }

    fn drain_available(&mut self) -> Result<()> {
        for _ in 0..1000 {
            if !self.pump(Duration::ZERO)? {
                return Ok(());
            }
        }
        anyhow::bail!("PTY queue did not drain within its bounded poll count")
    }
}

fn host_driver_main() -> Result<()> {
    let descriptor: i32 = std::env::var("AI_GUARD_INTERACTIVE_HOST_CONTROL_FD")?
        .parse()
        .context("invalid host driver control descriptor")?;
    // SAFETY: parent passed this inherited UnixStream descriptor for this child.
    let mut control = unsafe { UnixStream::from_raw_fd(descriptor) };
    let descriptor_flags = unsafe { libc::fcntl(control.as_raw_fd(), libc::F_GETFD) };
    ensure!(
        descriptor_flags >= 0,
        "host driver could not inspect control descriptor"
    );
    ensure!(
        unsafe {
            libc::fcntl(
                control.as_raw_fd(),
                libc::F_SETFD,
                descriptor_flags | libc::FD_CLOEXEC,
            )
        } == 0,
        "host driver could not protect control descriptor from broker inheritance"
    );
    control.set_nonblocking(true)?;
    let broker = std::env::var("AI_GUARD_INTERACTIVE_BROKER")?;
    let args: Vec<String> = serde_json::from_str(&std::env::var("AI_GUARD_INTERACTIVE_ARGS")?)?;
    let mut command = Command::new(broker);
    command
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    // Place the broker in its own foreground process group before exec, while
    // this session-leader driver keeps the controlling PTY alive after exit.
    // SAFETY: only async-signal-safe libc calls run in this pre-exec callback.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut block = std::mem::zeroed::<libc::sigset_t>();
            let mut previous = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut block);
            libc::sigaddset(&mut block, libc::SIGTTOU);
            if libc::sigprocmask(libc::SIG_BLOCK, &block, &mut previous) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let foreground = libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpgrp());
            let foreground_error = (foreground < 0).then(std::io::Error::last_os_error);
            let restored = libc::sigprocmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
            if let Some(error) = foreground_error {
                return Err(error);
            }
            if restored < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            control.set_nonblocking(false)?;
            control.write_all(b"DRIVER_ERROR spawn\n")?;
            return Ok(());
        }
    };
    let child_group = child.id() as libc::pid_t;
    let deadline = Instant::now() + Duration::from_secs(140);
    let status: ExitStatus = 'status: loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let mut request = libc::pollfd {
            fd: control.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized pollfd and finite timeout.
        if unsafe { libc::poll(&mut request, 1, 25) } > 0 && request.revents & libc::POLLIN != 0 {
            let mut bytes = [0u8; 32];
            let count = control.read(&mut bytes)?;
            if bytes[..count].starts_with(b"ABORT") {
                break 'status terminate_broker_group(&mut child, child_group)?;
            }
        }
        if Instant::now() >= deadline {
            break 'status terminate_broker_group(&mut child, child_group)?;
        }
    };
    let code = status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "none".into());
    let signal = status
        .signal()
        .map(|signal| signal.to_string())
        .unwrap_or_else(|| "none".into());
    control.set_nonblocking(false)?;
    control.write_all(format!("BROKER_EXIT {code} {signal}\n").as_bytes())?;
    control.set_read_timeout(Some(Duration::from_secs(20)))?;
    let mut ack = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        control.read_exact(&mut byte)?;
        ack.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
        ensure!(
            ack.len() <= 16,
            "host driver control message exceeded bound"
        );
    }
    // Restore the foreground process group before the driver session exits.
    // SAFETY: sigemptyset immediately initializes the opaque signal-set value.
    let mut block = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    // SAFETY: sigprocmask writes the previous signal mask before it is read.
    let mut previous = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    // SAFETY: block SIGTTOU around tcsetpgrp while this driver is background.
    unsafe {
        libc::sigemptyset(&mut block);
        libc::sigaddset(&mut block, libc::SIGTTOU);
        ensure!(
            libc::sigprocmask(libc::SIG_BLOCK, &block, &mut previous) == 0,
            "host driver could not block SIGTTOU"
        );
        let result = libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpgrp());
        let saved = std::io::Error::last_os_error();
        let restored = libc::sigprocmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
        ensure!(
            result == 0,
            "host driver could not restore foreground process group: {saved}"
        );
        ensure!(restored == 0, "host driver could not restore signal mask");
    }
    Ok(())
}

#[test]
#[ignore = "internal host PTY session driver; invoked only by the real interactive qualification"]
fn interactive_host_driver() {
    if std::env::var("AI_GUARD_INTERACTIVE_HOST_DRIVER").as_deref() != Ok("1") {
        return;
    }
    if host_driver_main().is_err() {
        std::process::exit(90);
    }
}

struct CliRun {
    broker_status: BrokerStatus,
    broker_status_reported_at: Instant,
    bytes: Vec<u8>,
}

fn run_on_pty(
    args: &[String],
    actions: impl FnOnce(&mut PtySession) -> Result<()>,
) -> Result<CliRun> {
    let pty = HostPty::new()?;
    for fd in [pty.master.as_raw_fd(), pty.slave.as_raw_fd()] {
        // SAFETY: descriptors are live and only close-on-exec is changed.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        ensure!(flags >= 0, "could not inspect PTY descriptor flags");
        ensure!(
            unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == 0,
            "could not protect PTY descriptors across exec"
        );
    }
    let (mut parent_control, driver_control) = UnixStream::pair()?;
    let control_flags = unsafe { libc::fcntl(driver_control.as_raw_fd(), libc::F_GETFD) };
    ensure!(control_flags >= 0, "could not inspect driver control fd");
    ensure!(
        unsafe {
            libc::fcntl(
                driver_control.as_raw_fd(),
                libc::F_SETFD,
                control_flags & !libc::FD_CLOEXEC,
            )
        } == 0,
        "could not pass private control socket to host driver"
    );
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            "--ignored",
            "--exact",
            "interactive_host_driver",
            "--nocapture",
        ])
        .env("AI_GUARD_INTERACTIVE_HOST_DRIVER", "1")
        .env(
            "AI_GUARD_INTERACTIVE_HOST_CONTROL_FD",
            driver_control.as_raw_fd().to_string(),
        )
        .env(
            "AI_GUARD_INTERACTIVE_BROKER",
            env!("CARGO_BIN_EXE_agentsandbox"),
        )
        .env("AI_GUARD_INTERACTIVE_ARGS", serde_json::to_string(args)?)
        .stdin(Stdio::from(pty.slave.try_clone()?))
        .stdout(Stdio::from(pty.slave.try_clone()?))
        .stderr(Stdio::from(pty.slave.try_clone()?));
    // This session leader retains the controlling PTY after broker exit.
    // SAFETY: this child alone creates the session and claims the passed PTY.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as libc::c_ulong, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut driver_child = command
        .spawn()
        .context("could not launch PTY host driver")?;
    let driver_pid = driver_child.id();
    eprintln!("interactive_qa phase=driver_started driver_pid={driver_pid}");
    drop(driver_control);
    let mut session = PtySession::new(pty.master.try_clone()?, parent_control.try_clone()?)?;
    let action_result = actions(&mut session);
    if action_result.is_err() {
        parent_control.set_nonblocking(true)?;
        let _ = parent_control.write_all(b"ABORT\n");
    }
    let status_deadline = Instant::now() + DEADLINE;
    while session.broker_status.is_none() && session.driver_error.is_none() {
        if let Some(exit) = driver_child.try_wait()? {
            anyhow::bail!(
                "PTY host driver exited before broker status (driver code {:?})",
                exit.code()
            );
        }
        if Instant::now() >= status_deadline {
            let _ = parent_control.write_all(b"ABORT\n");
            anyhow::bail!(
                "PTY host driver exceeded broker status deadline; driver_pid={driver_pid}"
            );
        }
        session.pump(Duration::from_millis(25))?;
    }
    if let Some(error) = &session.driver_error {
        anyhow::bail!("PTY host driver failure; driver_pid={driver_pid}; {error}");
    }
    let status = session
        .broker_status
        .context("host driver did not report broker status")?;
    let broker_status_reported_at = Instant::now();
    eprintln!(
        "interactive_qa phase=broker_status driver_pid={driver_pid} {}",
        broker_status_text(status)
    );
    session.drain_available()?;
    eprintln!(
        "interactive_qa phase=drain_complete captured_bytes={}",
        session.captured.len()
    );
    let termios_error = pty
        .verify_broker_restored()
        .err()
        .map(|error| format!("{error:#}"));
    eprintln!(
        "interactive_qa phase=termios_checked result={}",
        if termios_error.is_none() {
            "pass"
        } else {
            "fail"
        }
    );
    parent_control.set_nonblocking(false)?;
    parent_control.set_write_timeout(Some(Duration::from_secs(2)))?;
    parent_control.write_all(b"ACK\n")?;
    eprintln!("interactive_qa phase=ack_sent driver_pid={driver_pid}");
    let driver_deadline = Instant::now() + Duration::from_secs(10);
    let driver_status = loop {
        if let Some(status) = driver_child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= driver_deadline {
            let _ = driver_child.kill();
            break try_wait_child_until(
                &mut driver_child,
                Instant::now() + Duration::from_secs(2),
            )?;
        }
        session.pump(Duration::from_millis(20))?;
    };
    eprintln!(
        "interactive_qa phase=driver_wait_complete driver_pid={driver_pid} exited={}",
        driver_status.is_some()
    );
    if let Err(action_error) = action_result {
        anyhow::bail!(
            "PTY action failed: {action_error:#}; broker {}; termios verification: {}",
            broker_status_text(status),
            termios_error.as_deref().unwrap_or("PASS")
        );
    }
    if let Some(error) = termios_error {
        anyhow::bail!(
            "broker {}; PTY termios verification failed: {error}",
            broker_status_text(status)
        );
    }
    let driver_status = driver_status.context("PTY host driver did not exit within its bound")?;
    ensure!(
        driver_status.success(),
        "PTY host driver exited unsuccessfully"
    );
    ensure!(
        !session.overflow,
        "PTY capture exceeded 2 MiB; evidence is incomplete"
    );
    let bytes = std::mem::take(&mut session.captured);
    Ok(CliRun {
        broker_status: status,
        broker_status_reported_at,
        bytes,
    })
}

#[test]
fn pty_pump_services_control_during_flood_and_caps_capture() -> Result<()> {
    let mut pty = HostPty::new()?;
    let mut flood = FloodWriter::start(pty.slave.as_raw_fd())?;
    flood.wait_until_saturated()?;
    let master_flags = unsafe { libc::fcntl(pty.master.as_raw_fd(), libc::F_GETFL) };
    ensure!(master_flags >= 0, "could not inspect PTY master flags");
    ensure!(
        unsafe {
            libc::fcntl(
                pty.master.as_raw_fd(),
                libc::F_SETFL,
                master_flags | libc::O_NONBLOCK,
            )
        } == 0,
        "could not make PTY master nonblocking for flood readiness probe"
    );
    let mut sampled_bytes = 0;
    let sample_deadline = Instant::now() + Duration::from_secs(3);
    let mut sample = [0u8; 1024];
    while sampled_bytes <= PTY_FLOOD_TEST_READ_BUDGET {
        match pty.master.read(&mut sample) {
            Ok(0) => break,
            Ok(count) => sampled_bytes += count,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                ensure!(
                    Instant::now() < sample_deadline,
                    "PTY flood produced no readable bytes"
                );
                thread::yield_now();
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    ensure!(
        sampled_bytes > PTY_FLOOD_TEST_READ_BUDGET,
        "PTY flood did not produce more than one pump budget"
    );
    flood.clear_saturation();
    flood.wait_until_saturated()?;
    let (control_reader, mut control_writer) = UnixStream::pair()?;
    control_writer.write_all(b"BROKER_EXIT 0 none\n")?;
    let session = PtySession::with_limits(
        pty.master.try_clone()?,
        control_reader,
        PTY_FLOOD_TEST_READ_BUDGET,
        PTY_FLOOD_TEST_CAPTURE_LIMIT,
    )?;
    flood.clear_progress();
    let progress_deadline = Instant::now() + Duration::from_secs(10);
    let (pump_tx, pump_rx) = mpsc::sync_channel(1);
    let pump_thread = thread::spawn(move || {
        let mut session = session;
        let result = session.pump(Duration::from_millis(100));
        let _ = pump_tx.send((result, session));
    });
    let first_pump = match pump_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(pump) => pump,
        Err(error) => {
            flood.stop_and_join()?;
            if pump_rx.recv_timeout(Duration::from_secs(3)).is_ok() {
                let _ = pump_thread.join();
            }
            anyhow::bail!("PTY pump did not return during continuous flood: {error}");
        }
    };
    pump_thread
        .join()
        .map_err(|_| anyhow::anyhow!("PTY pump thread panicked"))?;
    let (first_result, mut session) = first_pump;
    let verification = (|| -> Result<()> {
        ensure!(
            first_result?,
            "PTY flood was not consumed by the initial pump"
        );
        flood.wait_for_progress_until(progress_deadline)?;
        ensure!(
            flood.is_running(),
            "producer stopped before pump processed the control notification"
        );
        ensure!(
            session
                .broker_status
                .is_some_and(|status| status.code == Some(0)),
            "control status was not processed while PTY output was flooding"
        );
        ensure!(
            session.captured.len() <= PTY_FLOOD_TEST_READ_BUDGET,
            "one pump call exceeded its fixed PTY read budget"
        );
        for _ in 0..16_384 {
            if session.overflow {
                break;
            }
            flood.clear_progress();
            let before = session.captured.len();
            session.pump(Duration::ZERO)?;
            flood.wait_for_progress_until(progress_deadline)?;
            ensure!(
                session.captured.len().saturating_sub(before) <= PTY_FLOOD_TEST_READ_BUDGET,
                "a pump call exceeded its fixed PTY read budget"
            );
            thread::yield_now();
        }
        ensure!(
            session.overflow,
            "sustained PTY output did not exercise the capture cap (captured_bytes={}, producer_running={})",
            session.captured.len(),
            flood.is_running()
        );
        ensure!(
            session.captured.len() <= PTY_FLOOD_TEST_CAPTURE_LIMIT,
            "PTY capture exceeded its memory limit"
        );
        ensure!(
            flood.is_running(),
            "producer stopped before bounded capture behavior was verified"
        );
        Ok(())
    })();
    flood.stop_and_join()?;
    verification?;
    eprintln!(
        "PASS PTY flood regression: pump returned and parsed control status under active flood; read budget and capture cap held"
    );
    Ok(())
}

#[test]
fn local_pty_host_driver_help_smoke() -> Result<()> {
    let args = vec!["--help".to_owned()];
    let result = run_on_pty(&args, |session| {
        session.wait_for(
            b"Usage:",
            Instant::now() + Duration::from_secs(5),
            "CLI help output",
        )
    })?;
    ensure!(
        result.broker_status.code == Some(0),
        "CLI --help did not exit successfully"
    );
    ensure!(
        result
            .bytes
            .windows(b"Usage:".len())
            .any(|part| part == b"Usage:"),
        "CLI --help output was not captured by the private PTY"
    );
    eprintln!("interactive_qa phase=help_smoke_complete result=pass");
    Ok(())
}

async fn private_gateway() -> Result<(
    std::path::PathBuf,
    String,
    Arc<Store>,
    tokio::task::JoinHandle<Result<()>>,
    Arc<Mutex<Vec<Option<TerminalFacts>>>>,
)> {
    let root = tempfile::Builder::new()
        .prefix("ai-guard-interactive-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()?
        .keep();
    // Keep all generated keys and evidence in a private, retained directory.
    eprintln!("retained interactive QA evidence: {}", root.display());
    let config = root.join("config");
    initialize_config(&config)?;
    let runner_token = read_token(&config.join("secrets/runner"))?;
    let viewer_token = read_token(&config.join("secrets/viewer"))?;
    let redactor = Redactor::new(vec![runner_token.clone(), viewer_token.clone()]);
    let database = root.join("audit.sqlite");
    let store = Arc::new(Store::open_for_tenant(
        &database,
        "local",
        redactor.clone(),
        1_000,
    )?);
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "local".into(),
            store: store.clone(),
            runner_token,
            viewer_token,
        }],
        Policy::default(),
        None,
        redactor,
    )?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let inspections = Arc::new(Mutex::new(Vec::<Option<TerminalFacts>>::new()));
    let captured_inspections = inspections.clone();
    let endpoint = ENDPOINT.to_owned();
    let app =
        gateway_router(state).layer(middleware::from_fn(move |request: Request, next: Next| {
            let captured = captured_inspections.clone();
            let endpoint = endpoint.clone();
            async move {
                if request.method() != Method::POST
                    || !request.uri().path().starts_with("/api/v1/workers/")
                    || !request.uri().path().ends_with("/transitions")
                {
                    return next.run(request).await;
                }
                let (parts, body) = request.into_parts();
                let Ok(bytes) = to_bytes(body, 128 * 1024).await else {
                    return StatusCode::PAYLOAD_TOO_LARGE.into_response();
                };
                if let Ok(mutation) = serde_json::from_slice::<WorkerMutationV1>(&bytes)
                    && let WorkerTransitionV1::Observe { observation } = &mutation.transition
                    && observation.status == RuntimeStatusV1::Exited
                    && observation.running == Some(false)
                {
                    // Read only the exact terminal configuration before the
                    // real gateway commits/ACKs it. Forward original bytes and
                    // response unchanged; this middleware never delays cleanup.
                    let facts = inspect_terminal_before_ack(
                        &endpoint,
                        &observation.inspection.container_id,
                    )
                    .await
                    .ok();
                    if let Ok(mut results) = captured.lock() {
                        results.push(facts);
                    }
                }
                next.run(Request::from_parts(parts, Body::from(bytes)))
                    .await
            }
        }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.map_err(Into::into) });
    Ok((root, url, store, server, inspections))
}

fn assert_filtered(bytes: &[u8]) -> Result<()> {
    for forbidden in [b"\x1b]52;".as_slice(), b"\x1bP", b"\x1b]0;", b"\x1b[5i"] {
        ensure!(
            !bytes
                .windows(forbidden.len())
                .any(|window| window == forbidden),
            "broker re-emitted a prohibited terminal sequence"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicitly selected local Docker, pinned worker image, and retains QA evidence and volumes"]
async fn docker_interactive_cli_forwards_tty_input_resize_and_worker_exit() -> Result<()> {
    ensure!(
        std::path::Path::new(DOCKER).is_file(),
        "expected explicit Docker executable is unavailable"
    );
    let endpoint =
        std::env::var("AGENTSANDBOX_TEST_RUNTIME_ENDPOINT").unwrap_or_else(|_| ENDPOINT.into());
    ensure!(
        endpoint == ENDPOINT,
        "this qualification is pinned to the authorized local Docker socket"
    );
    let image = std::env::var("AGENTSANDBOX_TEST_INTERACTIVE_IMAGE")
        .context("select the separately built interactive probe image after approval")?;
    let (root, gateway, store, server, inspections) = private_gateway().await?;
    let config = root.join("config");
    let project = root.join("project");
    fs::create_dir(&project)?;
    fs::write(project.join("original.txt"), "unchanged\n")?;
    let args = vec![
        "run".into(),
        "--project".into(),
        project.to_string_lossy().into_owned(),
        "--config-dir".into(),
        config.to_string_lossy().into_owned(),
        "--gateway".into(),
        gateway.clone(),
        "--runtime".into(),
        "docker".into(),
        "--runtime-program".into(),
        DOCKER.into(),
        "--runtime-endpoint".into(),
        endpoint,
        "--image".into(),
        image,
        "--profile".into(),
        "observe".into(),
        "--network".into(),
        "none".into(),
        "--allow-rootful-runtime".into(),
        "--timeout".into(),
        "90".into(),
        "--provider".into(),
        "generic".into(),
        "--interactive".into(),
        "--".into(),
        "/usr/local/bin/ai-guard-interactive-probe".into(),
        "exit7".into(),
    ];
    let result = run_on_pty(&args, |session| {
        session.wait_for(
            b"PROBE_READY",
            Instant::now() + Duration::from_secs(100),
            "worker keyboard probe",
        )?;
        session.resize(40, 120)?;
        session.write(b"keyboard-proof\n")?;
        session.wait_for(
            b"SAFE_AFTER",
            Instant::now() + Duration::from_secs(15),
            "sanitized terminal frame",
        )?;
        assert_filtered(&session.captured)?;
        ensure!(
            session
                .captured
                .windows(b"SAFE_BEFORE".len())
                .any(|w| w == b"SAFE_BEFORE"),
            "safe frame start was not observed before allowing the probe to exit"
        );
        session.write(b"exit\n")?;
        Ok(())
    })?;
    ensure!(
        result.broker_status.code == Some(1),
        "CLI must report worker exit 7 as a failed command (CLI code {:?})",
        result.broker_status.code
    );
    let visible = String::from_utf8_lossy(&result.bytes);
    ensure!(
        visible.contains("PROBE_INITIAL_SIZE:24 80")
            && visible.contains("PROBE_READY")
            && visible.contains("PROBE_ACK:keyboard-proof:40 120")
            && visible.contains("SAFE_BEFORE")
            && visible.contains("SAFE_AFTER"),
        "initial size, keyboard input or resized dimensions were not observed through the real TTY"
    );
    assert_filtered(&result.bytes)?;
    ensure!(
        fs::read_to_string(project.join("original.txt"))? == "unchanged\n",
        "source project was modified"
    );
    let token = read_token(&config.join("secrets/viewer"))?;
    let client = AuditClient::new(&gateway, &token)?;
    let integrity = client.get_json("api/integrity").await?;
    ensure!(
        integrity["valid"].as_bool() == Some(true),
        "gateway audit integrity failed"
    );
    let workers = store.workers()?;
    ensure!(workers.len() == 1, "expected exactly one admitted worker");
    let worker = &workers[0];
    let worker_id = worker.worker_id.clone();
    let terminal = {
        let inspections = inspections
            .lock()
            .map_err(|_| anyhow::anyhow!("pre-ACK inspection capture poisoned"))?;
        ensure!(
            inspections.len() == 1,
            "expected one independent pre-ACK Docker inspection"
        );
        inspections[0]
            .as_ref()
            .context("pre-ACK Docker inspection failed")?
            .clone()
    };
    ensure!(
        terminal.logs_none
            && terminal.log_options_empty
            && terminal.tty
            && terminal.open_stdin
            && terminal.stdin_once
            && !terminal.running
            && terminal.exit_code == 7,
        "pre-ACK Docker facts differed from the authorized interactive terminal contract"
    );
    let usage = store.worker_usage()?;
    ensure!(
        usage.len() == 1
            && !usage[0].billable
            && usage[0].source.provenance == AllocationProvenanceV1::Observed,
        "expected one observed non-billable allocation"
    );
    ensure!(
        store.worker_quota(&QuotaPolicyV1::default())?.held_workers == 0,
        "terminal ACK did not release quota"
    );
    let events = store.events(&EventQuery::default())?;
    ensure!(
        events
            .iter()
            .any(|event| event.event.kind == "command.failed"),
        "worker exit 7 was not durably audited as failure"
    );
    server.abort();
    let _ = server.await;
    drop(store);
    let reopened = Store::open_for_tenant(
        &root.join("audit.sqlite"),
        "local",
        Redactor::new(vec![]),
        1_000,
    )?;
    ensure!(
        reopened.integrity()?.valid,
        "audit chain invalid after reopening"
    );
    ensure!(
        reopened
            .workers()?
            .iter()
            .any(|worker| worker.worker_id == worker_id),
        "worker disappeared after reopening"
    );
    eprintln!(
        "PASS interactive Docker run: worker exit 7 preserved; tty input/resize observed; prohibited terminal sequences filtered; one observed billable=false allocation; quota released; audit reopened valid; source unchanged"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicitly selected local Docker, pinned raw-key probe image, and retains QA evidence and volumes"]
async fn docker_interactive_raw_keys_preserve_control_and_utf8_bytes() -> Result<()> {
    ensure!(
        std::path::Path::new(DOCKER).is_file(),
        "expected explicit Docker executable is unavailable"
    );
    let endpoint =
        std::env::var("AGENTSANDBOX_TEST_RUNTIME_ENDPOINT").unwrap_or_else(|_| ENDPOINT.into());
    ensure!(
        endpoint == ENDPOINT,
        "this qualification is pinned to the authorized local Docker socket"
    );
    let image = std::env::var("AGENTSANDBOX_TEST_INTERACTIVE_IMAGE")
        .context("select the separately built raw-key probe image after approval")?;
    let (root, gateway, store, server, inspections) = private_gateway().await?;
    let config = root.join("config");
    let project = root.join("project");
    fs::create_dir(&project)?;
    fs::write(project.join("original.txt"), "unchanged\n")?;
    let args = vec![
        "run".into(),
        "--project".into(),
        project.to_string_lossy().into_owned(),
        "--config-dir".into(),
        config.to_string_lossy().into_owned(),
        "--gateway".into(),
        gateway.clone(),
        "--runtime".into(),
        "docker".into(),
        "--runtime-program".into(),
        DOCKER.into(),
        "--runtime-endpoint".into(),
        endpoint,
        "--image".into(),
        image,
        "--profile".into(),
        "observe".into(),
        "--network".into(),
        "none".into(),
        "--allow-rootful-runtime".into(),
        "--timeout".into(),
        "90".into(),
        "--provider".into(),
        "generic".into(),
        "--interactive".into(),
        "--".into(),
        "/usr/local/bin/ai-guard-interactive-probe".into(),
        "raw-keys".into(),
    ];
    const RAW_BYTES: [u8; 8] = [0x0d, 0x0a, 0x03, 0xc3, 0xa9, 0x1b, 0x5b, 0x41];
    const RAW_REPORT: &[u8] = b"RAW_KEYS_RECEIVED:0d,0a,03,c3,a9,1b,5b,41";
    let result = run_on_pty(&args, |session| {
        session.wait_for(
            b"RAW_KEYS_READY",
            Instant::now() + Duration::from_secs(45),
            "remote raw terminal readiness",
        )?;
        session.write(&RAW_BYTES)?;
        session.wait_for(
            RAW_REPORT,
            Instant::now() + Duration::from_secs(15),
            "exact remote raw key report",
        )?;
        session.wait_for(
            b"RAW_KEYS_DONE",
            Instant::now() + Duration::from_secs(5),
            "remote raw key completion",
        )?;
        Ok(())
    })?;
    ensure!(
        result.broker_status.code == Some(0),
        "raw key probe did not exit successfully: {:?}",
        result.broker_status.code
    );
    let visible = String::from_utf8_lossy(&result.bytes);
    ensure!(
        visible.contains("RAW_KEYS_READY")
            && visible.contains("RAW_KEYS_RECEIVED:0d,0a,03,c3,a9,1b,5b,41")
            && visible.contains("RAW_KEYS_DONE"),
        "remote helper did not report the exact public raw key byte sequence"
    );
    ensure!(
        fs::read_to_string(project.join("original.txt"))? == "unchanged\n",
        "source project was modified"
    );
    let workers = store.workers()?;
    ensure!(workers.len() == 1, "expected exactly one admitted worker");
    let worker_id = workers[0].worker_id.clone();
    ensure!(
        workers[0].state == WorkerStateV1::Terminal && !workers[0].quota_held,
        "raw-key probe did not reach a terminal state and release quota"
    );
    let terminal = {
        let inspections = inspections
            .lock()
            .map_err(|_| anyhow::anyhow!("pre-ACK inspection capture poisoned"))?;
        ensure!(
            inspections.len() == 1,
            "expected one pre-ACK Docker inspection"
        );
        inspections[0]
            .as_ref()
            .context("pre-ACK Docker inspection failed")?
            .clone()
    };
    ensure!(
        terminal.logs_none
            && terminal.log_options_empty
            && terminal.tty
            && terminal.open_stdin
            && terminal.stdin_once
            && !terminal.running
            && terminal.exit_code == 0,
        "pre-ACK Docker facts differed from the raw interactive terminal contract"
    );
    let usage = store.worker_usage()?;
    ensure!(
        usage.len() == 1
            && !usage[0].billable
            && usage[0].source.provenance == AllocationProvenanceV1::Observed,
        "expected one observed non-billable allocation"
    );
    ensure!(
        store.worker_quota(&QuotaPolicyV1::default())?.held_workers == 0,
        "terminal ACK did not release quota"
    );
    server.abort();
    let _ = server.await;
    drop(store);
    let reopened = Store::open_for_tenant(
        &root.join("audit.sqlite"),
        "local",
        Redactor::new(vec![]),
        1_000,
    )?;
    ensure!(
        reopened.integrity()?.valid,
        "audit chain invalid after reopening"
    );
    ensure!(
        reopened
            .workers()?
            .iter()
            .any(|worker| worker.worker_id == worker_id),
        "worker disappeared after reopening"
    );
    eprintln!(
        "PASS raw-key Docker run: CR/LF/Ctrl-C/UTF-8/arrow bytes preserved exactly; worker exited cleanly; quota released; audit reopened valid; source unchanged"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicitly selected local Docker, pinned interactive probe image, and retains QA evidence and volumes"]
async fn docker_interactive_double_ctrl_x_stops_only_the_owned_worker() -> Result<()> {
    let endpoint =
        std::env::var("AGENTSANDBOX_TEST_RUNTIME_ENDPOINT").unwrap_or_else(|_| ENDPOINT.into());
    ensure!(
        endpoint == ENDPOINT,
        "this qualification is pinned to the authorized local Docker socket"
    );
    let image = std::env::var("AGENTSANDBOX_TEST_INTERACTIVE_IMAGE")
        .context("select the separately built interactive probe image after approval")?;
    let (root, gateway, store, server, inspections) = private_gateway().await?;
    let config = root.join("config");
    let project = root.join("project");
    fs::create_dir(&project)?;
    fs::write(project.join("original.txt"), "unchanged\n")?;
    let args = vec![
        "run".into(),
        "--project".into(),
        project.to_string_lossy().into_owned(),
        "--config-dir".into(),
        config.to_string_lossy().into_owned(),
        "--gateway".into(),
        gateway,
        "--runtime".into(),
        "docker".into(),
        "--runtime-program".into(),
        DOCKER.into(),
        "--runtime-endpoint".into(),
        endpoint.clone(),
        "--image".into(),
        image,
        "--profile".into(),
        "observe".into(),
        "--network".into(),
        "none".into(),
        "--allow-rootful-runtime".into(),
        "--timeout".into(),
        "120".into(),
        "--provider".into(),
        "generic".into(),
        "--interactive".into(),
        "--".into(),
        "/usr/local/bin/ai-guard-interactive-probe".into(),
        "wait".into(),
    ];
    let mut second_ctrl_x_sent_at = None;
    let result = run_on_pty(&args, |session| {
        session.wait_for(
            b"STOP_READY",
            Instant::now() + Duration::from_secs(100),
            "worker stop probe",
        )?;
        session.write(&[0x18])?;
        thread::sleep(Duration::from_millis(250));
        session.write(&[0x18])?;
        second_ctrl_x_sent_at = Some(Instant::now());
        Ok(())
    })?;
    let second_ctrl_x_sent_at = second_ctrl_x_sent_at.context("second Ctrl-X was not sent")?;
    let stop_response = result
        .broker_status_reported_at
        .checked_duration_since(second_ctrl_x_sent_at)
        .context("broker status preceded the second Ctrl-X")?;
    ensure!(
        stop_response < Duration::from_secs(30),
        "broker did not finish promptly after double Ctrl-X ({stop_response:?})"
    );
    ensure!(
        result.broker_status.code != Some(0),
        "interactive stop must report the interrupted command as unsuccessful"
    );
    let visible = String::from_utf8_lossy(&result.bytes);
    ensure!(
        visible.contains("STOP_READY"),
        "stop probe never reached its running state"
    );
    assert_filtered(&result.bytes)?;
    ensure!(
        fs::read_to_string(project.join("original.txt"))? == "unchanged\n",
        "source project was modified"
    );
    let workers = store.workers()?;
    ensure!(
        workers.len() == 1,
        "stop must not reserve or start a replacement worker"
    );
    let worker = &workers[0];
    ensure!(
        worker.attempt.start_operation_id.is_some(),
        "worker has no durable start claim"
    );
    ensure!(
        worker.state == WorkerStateV1::Terminal,
        "double Ctrl-X did not leave the worker in a terminal state"
    );
    ensure!(!worker.quota_held, "confirmed stop did not release quota");
    let usage = store.worker_usage()?;
    ensure!(
        usage.len() == 1
            && !usage[0].billable
            && usage[0].source.provenance == AllocationProvenanceV1::Observed,
        "expected exactly one observed non-billable allocation for stopped worker"
    );
    let terminal = {
        let inspections = inspections
            .lock()
            .map_err(|_| anyhow::anyhow!("pre-ACK inspection capture poisoned"))?;
        ensure!(
            inspections.len() == 1,
            "expected one independent pre-ACK Docker inspection"
        );
        inspections[0]
            .as_ref()
            .context("pre-ACK Docker inspection failed")?
            .clone()
    };
    ensure!(
        terminal.logs_none
            && terminal.log_options_empty
            && terminal.tty
            && terminal.open_stdin
            && terminal.stdin_once
            && !terminal.running,
        "pre-ACK Docker facts did not confirm terminal settings and stopped worker"
    );
    let event_count = store
        .events(&EventQuery::default())?
        .iter()
        .filter(|event| event.event.kind == "runtime.terminal.attach.authorized")
        .count();
    ensure!(
        event_count == 1,
        "double Ctrl-X caused more than one attach/start authorization"
    );
    server.abort();
    let _ = server.await;
    drop(store);
    let reopened = Store::open_for_tenant(
        &root.join("audit.sqlite"),
        "local",
        Redactor::new(vec![]),
        1_000,
    )?;
    ensure!(
        reopened.integrity()?.valid,
        "audit chain invalid after reopening"
    );
    let durable_events = reopened.events(&EventQuery::default())?;
    let failed_events: Vec<_> = durable_events
        .iter()
        .filter(|event| event.event.kind == "command.failed")
        .collect();
    ensure!(
        failed_events.len() == 1,
        "double Ctrl-X must produce exactly one durable command.failed event"
    );
    let failed_result = failed_events[0]
        .event
        .result
        .as_ref()
        .context("command.failed event omitted its durable result")?;
    ensure!(
        failed_result["transport_stop_reason"].as_str()
            == Some("operator requested worker stop with Ctrl-X Ctrl-X"),
        "command.failed event did not record the exact Ctrl-X stop reason"
    );
    ensure!(
        failed_result["timed_out"] == false,
        "double Ctrl-X was reported as a timed-out command"
    );
    eprintln!(
        "PASS double Ctrl-X: one claimed worker stopped promptly; durable Ctrl-X reason and timed_out=false; Docker confirms terminal state; one observed allocation; quota released; no second start authorization; audit reopened valid; source unchanged"
    );
    Ok(())
}
