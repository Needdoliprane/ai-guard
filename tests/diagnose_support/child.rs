//! Bounded child processes. Each child has a cleared environment, a private
//! working directory, a closed stdin and bounded captures. A test kills and
//! reaps only its own child. A wall-clock overrun is a harness timeout: it is
//! inconclusive and never a product failure.

use std::{
    io::{BufRead, BufReader, Read},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// The shared QA container has three CPUs; `setup prepare` creates several
/// SQLite stores and the gateway verifies each one before it listens.
pub const DIAGNOSE_BOUND: Duration = Duration::from_secs(20);
pub const BUILDER_BOUND: Duration = Duration::from_secs(60);
pub const GATEWAY_BOUND: Duration = Duration::from_secs(60);
/// Above the 524 288-byte bound of a report, so an oversized report is seen.
pub const DIAGNOSE_STDOUT_CAP: usize = 1_048_576;
pub const SMALL_CAP: usize = 65_536;

pub struct Output {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn stdout_text(&self) -> String {
        String::from_utf8(self.stdout.clone()).expect("child stdout is UTF-8")
    }

    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Kills and reaps this test's own child when the test unwinds.
struct Owned(Child);

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn capture(mut reader: impl Read + Send + 'static, cap: usize) -> JoinHandle<(Vec<u8>, bool)> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut truncated = false;
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    // Keep draining past the cap so the child never blocks on
                    // a full pipe; the overflow fails the test afterwards.
                    let room = cap - bytes.len();
                    truncated |= count > room;
                    bytes.extend_from_slice(&buffer[..count.min(room)]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        (bytes, truncated)
    })
}

fn isolated(command: &mut Command, directory: &Path) {
    command
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
}

/// Run a child to its end within `bound`. The caller clears or sets the
/// environment; this function never inherits a terminal.
pub fn run(
    mut command: Command,
    directory: &Path,
    bound: Duration,
    stdout_cap: usize,
    stderr_cap: usize,
) -> Output {
    isolated(&mut command, directory);
    let mut child = Owned(command.spawn().expect("spawn the test's own child"));
    let stdout = capture(child.0.stdout.take().unwrap(), stdout_cap);
    let stderr = capture(child.0.stderr.take().unwrap(), stderr_cap);
    let status = wait_within(child, bound);
    let (stdout, stdout_truncated) = stdout.join().expect("stdout capture");
    let (stderr, stderr_truncated) = stderr.join().expect("stderr capture");
    assert!(
        !stdout_truncated && !stderr_truncated,
        "a child wrote more than its capture bound"
    );
    Output {
        code: status.code(),
        stdout,
        stderr,
    }
}

/// Wait for a child within `bound`, then reap it.
fn wait_within(mut child: Owned, bound: Duration) -> ExitStatus {
    let deadline = Instant::now() + bound;
    loop {
        if let Some(status) = child.0.try_wait().expect("observe the test's own child") {
            return status;
        }
        if Instant::now() >= deadline {
            drop(child);
            panic!("harness timeout: inconclusive; the child exceeded {bound:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Run a child whose standard output is a pipe that nobody reads, so that
/// its first write to it fails. Standard error is captured as in `run`.
pub fn run_without_stdout_reader(
    mut command: Command,
    directory: &Path,
    bound: Duration,
    stderr_cap: usize,
) -> Output {
    let (reader, writer) = std::io::pipe().expect("create the pipe of the test's own child");
    drop(reader);
    command
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(Stdio::piped());
    let mut child = Owned(command.spawn().expect("spawn the test's own child"));
    let stderr = capture(child.0.stderr.take().unwrap(), stderr_cap);
    let status = wait_within(child, bound);
    let (stderr, stderr_truncated) = stderr.join().expect("stderr capture");
    assert!(
        !stderr_truncated,
        "a child wrote more than its capture bound"
    );
    Output {
        code: status.code(),
        stdout: Vec::new(),
        stderr,
    }
}

/// What a real gateway did with one command line.
pub enum GatewayStart {
    /// It verified its stores and printed its listening line; then this test
    /// stopped it.
    Listening,
    /// It exited before listening.
    Refused { code: Option<i32>, stderr: String },
}

impl GatewayStart {
    pub fn listening(&self) -> bool {
        matches!(self, Self::Listening)
    }

    /// What a gateway that exited before listening wrote on its standard
    /// error: its own statement of the cause. `None` when it listened.
    pub fn refusal(&self) -> Option<&str> {
        match self {
            Self::Listening => None,
            Self::Refused { stderr, .. } => Some(stderr),
        }
    }

    /// For assertion messages. The gateway masks its own diagnostics, and
    /// every credential of these fixtures is synthetic.
    pub fn describe(&self) -> String {
        match self {
            Self::Listening => "listening".to_owned(),
            Self::Refused { code, stderr } => {
                format!("exited with {code:?} before listening: {stderr}")
            }
        }
    }
}

/// A real gateway that printed its listening line and still runs: it holds
/// its stores open. Dropping it stops it.
pub struct RunningGateway {
    child: Owned,
    // Reads the standard error of the gateway for as long as it runs, so
    // that a later line of the gateway never meets a closed pipe.
    reader: JoinHandle<String>,
}

impl RunningGateway {
    /// Whether the gateway process is still there.
    pub fn still_runs(&mut self) -> bool {
        matches!(self.child.0.try_wait(), Ok(None))
    }
}

/// Start a real gateway and wait for its listening line or its exit. The
/// gateway opens and may create stores: call it after the manifest
/// comparison of the scenario.
pub fn gateway_start(command: Command, directory: &Path) -> GatewayStart {
    match gateway_wait(command, directory) {
        Ok(running) => {
            // Stopped here: its standard error ends, and so does its reader.
            let RunningGateway { child, reader } = running;
            drop(child);
            let _ = reader.join();
            GatewayStart::Listening
        }
        Err(refused) => refused,
    }
}

/// Start a real gateway and keep it running once it listens, for a test
/// that inspects an installation while its gateway holds the stores. `Err`
/// says what a gateway that does not run did: its refusal, or `Listening`
/// when it exited right after its listening line.
pub fn gateway_hold(command: Command, directory: &Path) -> Result<RunningGateway, GatewayStart> {
    gateway_wait(command, directory)
}

fn gateway_wait(mut command: Command, directory: &Path) -> Result<RunningGateway, GatewayStart> {
    isolated(&mut command, directory);
    command.stdout(Stdio::null());
    let mut child = Owned(command.spawn().expect("spawn the test's own gateway"));
    let stderr = child.0.stderr.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut captured = String::new();
        let mut listening = false;
        for line in BufReader::new(stderr.take(SMALL_CAP as u64)).lines() {
            let Ok(line) = line else { break };
            // Past the listening line the gateway is only read, not kept.
            if listening {
                continue;
            }
            listening = line.contains("audit vérifié");
            captured.push_str(&line);
            captured.push('\n');
            if listening {
                let _ = sender.send(());
            }
        }
        captured
    });
    let deadline = Instant::now() + GATEWAY_BOUND;
    loop {
        if receiver.try_recv().is_ok() {
            return Ok(RunningGateway { child, reader });
        }
        if let Some(status) = child.0.try_wait().expect("observe the test's own gateway") {
            let stderr = reader.join().expect("gateway stderr capture");
            // The line may have been printed just before a late exit.
            if receiver.try_recv().is_ok() {
                return Err(GatewayStart::Listening);
            }
            return Err(GatewayStart::Refused {
                code: status.code(),
                stderr,
            });
        }
        if Instant::now() >= deadline {
            drop(child);
            panic!("harness timeout: inconclusive; the gateway neither listened nor exited");
        }
        thread::sleep(Duration::from_millis(20));
    }
}
