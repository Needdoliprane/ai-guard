//! What the retained owner decides without the fixture engine: refusals
//! before anything is spawned, how the arguments are taken, how stop requests
//! coalesce, how holds count, how ends map to results, what it does with a
//! process that no longer holds its output pipes, and the durations it stops
//! a command with.
//!
//! These use the public API only. The engines of the tests that start a
//! command are a few lines of Rust compiled at test time. The first records
//! what it was given, so "nothing was started" is read from the absence of its
//! record. The second gives up its two output pipes and lives on.
use crate::{
    adapter,
    harness::{FixtureState, Liveness, private_tempdir},
};
use agentsandbox::runtime::{
    Capture, Runtime, RuntimeCommandFailure, RuntimeKind,
    retained_cli::{
        Collected, INTERRUPT_GRACE, Interruption, InvocationEnd, InvocationFailure,
        InvocationStopped, MAX_DURATION, REPEAT_MIN_INTERVAL, RetainedInput,
        RuntimeInvocationOwner, STOP_OBSERVATION, Stop, StopReason, UnknownEnd,
    },
};
use anyhow::Context;
use std::{
    os::unix::{fs::PermissionsExt, process::ExitStatusExt},
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

/// At most one scenario of this module waits for the binary's turn.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Test-side observation windows. They are not product deadlines.
const SLOT_WINDOW: Duration = Duration::from_secs(60);
const MINUTE: Duration = Duration::from_secs(60);
const READY_WINDOW: Duration = Duration::from_secs(30);
const PROBE_WINDOW: Duration = Duration::from_secs(3);
const END_WINDOW: Duration = Duration::from_secs(10);
/// Bound handed to a command that the scenario ends in another way.
const LONG: Duration = Duration::from_secs(120);

/// The documented bounds of one retained command.
const MAX_ARGUMENTS: usize = 128;
const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
const MAX_ENVIRONMENT_VALUE_BYTES: usize = 4096;

const CANARY: &str = "CAPTURED-OUTPUT-CANARY";

/// Records that it ran, with what it was given, in its working directory.
const ENGINE: &str = r#"fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let bytes: usize = arguments.iter().map(String::len).sum();
    let directory = std::env::var("AGENTSANDBOX_CONFIG_DIR").map_or(0, |value| value.len());
    let head: Vec<&str> = arguments.iter().take(3).map(String::as_str).collect();
    let record = format!("{} {bytes} {directory} {}", arguments.len(), head.join(" "));
    std::fs::write("spawned", record).unwrap();
    let ports = ["AI_GUARD_GATEWAY_PORT", "AI_GUARD_DASHBOARD_PORT"]
        .map(|name| std::env::var(name).unwrap_or_default());
    if ports.iter().any(|port| !port.is_empty()) {
        std::fs::write("ports", ports.join(" ")).unwrap();
    }
    println!("ok");
}"#;

/// An engine CLI whose processes can live without the two output pipes of the
/// control process. Its arguments are a mode and its state directory, relative
/// to its working directory:
///
/// - `quiet-leader`: the CLI process itself replaces its standard output and
///   error by `/dev/null`, holds, and exits 3 when released;
/// - `quiet-helper`: the CLI process starts a helper in its own process group
///   whose output is `/dev/null` from the start, then exits 0;
/// - `quiet-helper-and-wait`: the same, and the CLI process then holds with
///   its pipes.
///
/// A holding process speaks the protocol of the fixture engine, so the same
/// oracles read it: the lock on `alive-<role>` for its whole life,
/// `ready-<role>` with its PID, `ack-<role>-<k>` for each `probe-<k>`, and it
/// leaves at `release` or `quit`. The helper ignores SIGINT, so a signal
/// forwarded to the process group ends the CLI process and not the helper.
const QUIET_ENGINE: &str = r#"use std::{
    fs::{self, File, OpenOptions},
    os::fd::AsRawFd,
    path::Path,
    process::{Command, Stdio, exit},
    time::{Duration, Instant},
};

unsafe extern "C" {
    fn dup2(from: i32, to: i32) -> i32;
    fn signal(signal: i32, handler: usize) -> usize;
}

const SIGINT: i32 = 2;
const SIG_IGN: usize = 1;
const TICK: Duration = Duration::from_millis(10);
const LIFE: Duration = Duration::from_secs(90);
const EXIT_USAGE: i32 = 64;
const EXIT_QUIT: i32 = 96;
const EXIT_SETUP: i32 = 97;
const EXIT_LIFE: i32 = 98;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [mode, state] = arguments.as_slice() else {
        exit(EXIT_USAGE)
    };
    let state = Path::new(state);
    match mode.as_str() {
        "quiet-leader" => {
            let null = OpenOptions::new().write(true).open("/dev/null");
            let Ok(null) = null else { exit(EXIT_SETUP) };
            for descriptor in [1, 2] {
                // SAFETY: both descriptors are open and owned by this process.
                if unsafe { dup2(null.as_raw_fd(), descriptor) } != descriptor {
                    exit(EXIT_SETUP);
                }
            }
            let _alive = hold(state, "leader");
            exit(3);
        }
        "quiet-helper" => {
            start_helper(state);
            exit(0);
        }
        "quiet-helper-and-wait" => {
            start_helper(state);
            let _alive = hold(state, "leader");
            exit(0);
        }
        "helper" => {
            // SAFETY: only replaces a disposition by SIG_IGN.
            unsafe {
                signal(SIGINT, SIG_IGN);
            }
            let _alive = hold(state, "descendant");
            exit(0);
        }
        _ => exit(EXIT_USAGE),
    }
}

/// Starts the helper in this process group, without the pipes of this process,
/// and returns once it holds its lock.
fn start_helper(state: &Path) {
    publish(state, "leader-pid", &std::process::id().to_string());
    let Ok(program) = std::env::current_exe() else {
        exit(EXIT_SETUP)
    };
    let started = Command::new(program)
        .arg("helper")
        .arg(state)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if started.is_err() {
        exit(EXIT_SETUP);
    }
    let waiting = Instant::now();
    while !state.join("ready-descendant").exists() {
        if waiting.elapsed() > Duration::from_secs(20) {
            exit(EXIT_SETUP);
        }
        std::thread::sleep(TICK);
    }
}

/// Takes the lock of `role`, publishes its readiness, answers probes and
/// returns at `release`. The returned lock lives until the process is gone.
fn hold(state: &Path, role: &str) -> File {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join(format!("alive-{role}")));
    let Ok(lock) = lock else { exit(EXIT_SETUP) };
    if lock.lock().is_err() {
        exit(EXIT_SETUP);
    }
    publish(state, &format!("ready-{role}"), &std::process::id().to_string());
    let started = Instant::now();
    let mut answered = 0_u32;
    while started.elapsed() < LIFE {
        if state.join("quit").exists() || !state.exists() {
            exit(EXIT_QUIT);
        }
        if state.join("release").exists() {
            return lock;
        }
        while state.join(format!("probe-{}", answered + 1)).exists() {
            answered += 1;
            publish(state, &format!("ack-{role}-{answered}"), "");
        }
        std::thread::sleep(TICK);
    }
    exit(EXIT_LIFE)
}

/// Readers never see a partial file: temporary name, then rename.
fn publish(state: &Path, name: &str, content: &str) {
    let staged = state.join(format!(".{name}.{}.tmp", std::process::id()));
    if fs::write(&staged, content).is_err() || fs::rename(&staged, state.join(name)).is_err() {
        exit(EXIT_SETUP);
    }
}
"#;

struct Engine {
    directory: tempfile::TempDir,
    runtime: Runtime,
    // Declared last: the turn passes on after the engine directory is removed.
    _turn: adapter::Turn,
}

impl Engine {
    async fn compile() -> Self {
        Self::compile_from(ENGINE).await
    }

    /// Compiles `text` as the approved engine CLI of a scenario that holds
    /// the binary's turn.
    async fn compile_from(text: &str) -> Self {
        let directory = private_tempdir();
        let source = directory.path().join("engine.rs");
        let binary = directory.path().join("docker");
        std::fs::write(&source, text).expect("SETUP: engine source");
        let compiled =
            std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
                .args(["--edition=2024", "--crate-name", "retained_cli_engine"])
                .arg(&source)
                .arg("-o")
                .arg(&binary)
                .output()
                .expect("SETUP: the Rust compiler is unavailable at test time");
        assert!(
            compiled.status.success(),
            "SETUP: the engine stand-in does not compile: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let runtime = Runtime::with_program(RuntimeKind::Docker, &binary).unwrap_or_else(|error| {
            panic!("SETUP: the engine stand-in was not approved: {error:#}")
        });
        let turn = adapter::admit(&runtime, SLOT_WINDOW)
            .await
            .unwrap_or_else(|busy| {
                panic!("SETUP: retained slot still busy since the previous test: {busy}")
            });
        Self {
            directory,
            runtime,
            _turn: turn,
        }
    }

    /// What the engine recorded when it ran. `None`: it was never started.
    fn spawned(&self) -> Option<String> {
        std::fs::read_to_string(self.directory.path().join("spawned")).ok()
    }

    /// The path the engine was approved at.
    fn program(&self) -> PathBuf {
        self.directory.path().join("docker")
    }

    /// The oracles of one command of the quiet engine, and its arguments for
    /// a mode. The state directory of the command is `<name>.fixture`, beside
    /// the engine: that is where the engine process runs.
    fn quiet_command(&self, name: &str) -> (FixtureState, impl Fn(&str) -> Vec<String>) {
        let state = FixtureState::for_program(&self.directory.path().join(name));
        let directory = format!("{name}.fixture");
        (state, move |mode: &str| {
            vec![mode.to_owned(), directory.clone()]
        })
    }
}

/// An input that counts how often its arguments are taken and records its drop.
struct Probe {
    arguments: Vec<String>,
    taken: Arc<AtomicUsize>,
    dropped: Arc<AtomicBool>,
}

impl Probe {
    fn new(arguments: Vec<String>) -> (Self, Arc<AtomicUsize>, Arc<AtomicBool>) {
        let taken = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        let probe = Self {
            arguments,
            taken: Arc::clone(&taken),
            dropped: Arc::clone(&dropped),
        };
        (probe, taken, dropped)
    }
}

impl RetainedInput for Probe {
    fn arguments(&self) -> Vec<String> {
        self.taken.fetch_add(1, Ordering::SeqCst);
        self.arguments.clone()
    }
    fn verify_before_spawn(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

async fn end_of(owner: &mut RuntimeInvocationOwner) -> InvocationEnd {
    tokio::time::timeout(MINUTE, owner.join())
        .await
        .expect("FAIL: a command that is refused or exits at once has no end after a minute")
}

fn refusal(end: InvocationEnd) -> InvocationFailure {
    let InvocationEnd::Refused(error) = end else {
        panic!("FAIL: the command was not refused: {end:?}");
    };
    error
        .downcast::<InvocationFailure>()
        .unwrap_or_else(|other| panic!("FAIL: the refusal has no typed reason: {other:#}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_cli_rejects_unbounded_arguments_and_unapproved_environment_before_spawn() {
    let _serial = SERIAL.lock().await;
    let engine = Engine::compile().await;
    let words = |count: usize| vec!["a".to_owned(); count];
    let host = || vec!["--host".to_owned(), "unix:///fixture.sock".to_owned()];
    let long_directory = format!("/{}", "c".repeat(MAX_ENVIRONMENT_VALUE_BYTES));
    let control = "AGENTSANDBOX_CONFIG_DIR";
    type Case<'a> = (
        &'a str,
        Vec<String>,
        Vec<String>,
        Duration,
        Vec<(&'a str, &'a str)>,
    );
    let refused: Vec<Case<'_>> = vec![
        (
            "more than 128 arguments, prefix included",
            host(),
            words(MAX_ARGUMENTS - 1),
            MINUTE,
            vec![],
        ),
        (
            "more than 64 KiB of arguments",
            vec![],
            vec!["a".repeat(MAX_ARGUMENT_BYTES + 1)],
            MINUTE,
            vec![],
        ),
        (
            "a NUL byte in an argument",
            vec![],
            vec!["version\0".to_owned()],
            MINUTE,
            vec![],
        ),
        (
            "an environment name other than the control directory",
            vec![],
            words(1),
            MINUTE,
            vec![("PATH", "/usr/bin")],
        ),
        (
            "a relative control directory",
            vec![],
            words(1),
            MINUTE,
            vec![(control, "relative")],
        ),
        (
            "a control character in the control directory",
            vec![],
            words(1),
            MINUTE,
            vec![(control, "/control\n")],
        ),
        (
            "a control directory over 4096 bytes",
            vec![],
            words(1),
            MINUTE,
            vec![(control, long_directory.as_str())],
        ),
        (
            "two environment entries",
            vec![],
            words(1),
            MINUTE,
            vec![(control, "/first"), (control, "/second")],
        ),
        ("a zero bound", vec![], words(1), Duration::ZERO, vec![]),
        (
            "a bound over 30 minutes",
            vec![],
            words(1),
            MAX_DURATION + Duration::from_nanos(1),
            vec![],
        ),
    ];
    for (case, prefix, arguments, duration, environment) in refused {
        let interruption = Interruption::new();
        let (input, taken, dropped) = Probe::new(arguments);
        let mut owner = engine.runtime.cli_env_retained(
            prefix,
            Box::new(input),
            duration,
            &environment,
            Some(interruption.clone()),
        );
        assert!(
            dropped.load(Ordering::SeqCst),
            "FAIL: {case}: the input was not dropped on the caller's task"
        );
        assert_eq!(
            interruption.active(),
            1,
            "FAIL: {case}: the hold ended before the end was taken"
        );
        assert_eq!(
            refusal(end_of(&mut owner).await),
            InvocationFailure::InvalidInput,
            "FAIL: {case}"
        );
        assert_eq!(
            interruption.active(),
            0,
            "FAIL: {case}: a refusal keeps a hold"
        );
        assert_eq!(taken.load(Ordering::SeqCst), 1, "FAIL: {case}");
        assert_eq!(
            engine.spawned(),
            None,
            "FAIL: {case}: the engine was started"
        );
    }

    // A refusal whose observer is dropped without `join` keeps no hold either:
    // nothing was admitted.
    let interruption = Interruption::new();
    let (input, _, _) = Probe::new(words(1));
    drop(engine.runtime.cli_env_retained(
        vec![],
        Box::new(input),
        Duration::ZERO,
        &[],
        Some(interruption.clone()),
    ));
    assert_eq!(
        interruption.active(),
        0,
        "FAIL: a refusal that nobody took keeps a hold"
    );

    // The bounds themselves are approved, and the same engine then really runs:
    // the absence of its record above is not vacuous.
    let mut arguments = words(MAX_ARGUMENTS - 1);
    arguments.push("b".repeat(MAX_ARGUMENT_BYTES - (MAX_ARGUMENTS - 1)));
    let directory = format!("/{}", "c".repeat(MAX_ENVIRONMENT_VALUE_BYTES - 1));
    let (input, _, dropped) = Probe::new(arguments);
    let mut owner = engine.runtime.cli_env_retained(
        vec![],
        Box::new(input),
        MAX_DURATION,
        &[(control, directory.as_str())],
        None,
    );
    let end = end_of(&mut owner).await;
    let InvocationEnd::Collected(collected) = end else {
        panic!("FAIL: a command at its bounds was not collected: {end:?}");
    };
    assert!(collected.status.success() && collected.stopped.is_none());
    assert!(collected.input_released && dropped.load(Ordering::SeqCst));
    assert_eq!(collected.stdout.bytes, b"ok\n");
    assert_eq!(
        engine.spawned().as_deref(),
        Some("128 65536 4096 a a a"),
        "FAIL: the engine did not receive the approved arguments and control directory"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_cli_limits_port_overrides_to_canonical_compose_up() {
    let _serial = SERIAL.lock().await;
    let engine = Engine::compile().await;
    let config = "AGENTSANDBOX_CONFIG_DIR";
    let gateway = "AI_GUARD_GATEWAY_PORT";
    let dashboard = "AI_GUARD_DASHBOARD_PORT";
    let compose = |action: &[&str]| {
        [
            "compose",
            "--project-name",
            "qa",
            "--env-file",
            "/copy/empty.env",
            "--file",
            "/copy/compose.yaml",
        ]
        .into_iter()
        .chain(action.iter().copied())
        .map(str::to_owned)
        .collect::<Vec<_>>()
    };
    let up = || compose(&["up", "--detach", "--force-recreate"]);
    let mut refused = vec![
        (compose(&["down"]), vec![(gateway, "9001")]),
        (vec!["build".into()], vec![(dashboard, "9002")]),
        (compose(&["up"]), vec![(gateway, "9001")]),
        (up(), vec![("AGENTSANDBOX_GATEWAY_PORT", "9001")]),
        (up(), vec![(gateway, "9001"), (gateway, "9002")]),
        (up(), vec![(dashboard, "9001"), (dashboard, "9002")]),
        (
            up(),
            vec![(config, "/first"), (gateway, "9001"), (config, "/second")],
        ),
        (up(), vec![(gateway, "9001"), (dashboard, "9001")]),
        (up(), vec![(gateway, "4000")]),
    ];
    for invalid in ["", "0", "65536", "+9001", "09001", "9001 "] {
        refused.push((up(), vec![(gateway, invalid)]));
        refused.push((up(), vec![(dashboard, invalid)]));
    }
    for (arguments, environment) in refused {
        let (input, _, _) = Probe::new(arguments);
        let mut owner =
            engine
                .runtime
                .cli_env_retained(vec![], Box::new(input), MINUTE, &environment, None);
        assert_eq!(
            refusal(end_of(&mut owner).await),
            InvocationFailure::InvalidInput,
            "{environment:?}"
        );
        assert_eq!(engine.spawned(), None);
    }

    for (prefix, environment, observed) in [
        (
            vec!["--host".into(), "unix:///fixture.sock".into()],
            vec![(config, "/control"), (gateway, "1"), (dashboard, "65535")],
            "1 65535",
        ),
        (
            vec![],
            vec![(config, "/control"), (gateway, "9100")],
            "9100 ",
        ),
    ] {
        let (input, _, _) = Probe::new(up());
        let mut owner =
            engine
                .runtime
                .cli_env_retained(prefix, Box::new(input), MINUTE, &environment, None);
        let result = end_of(&mut owner).await;
        let InvocationEnd::Collected(collected) = result else {
            panic!("FAIL: the Compose startup was not collected: {result:?}");
        };
        assert!(collected.status.success() && collected.input_released);
        assert_eq!(
            std::fs::read_to_string(engine.directory.path().join("ports")).unwrap(),
            observed
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arguments_are_taken_once() {
    let _serial = SERIAL.lock().await;
    let engine = Engine::compile().await;
    let prefix = vec!["--host".to_owned(), "unix:///fixture.sock".to_owned()];

    // Refused in the owner thread: its interruption was requested beforehand.
    let interruption = Interruption::new();
    interruption.request(libc::SIGINT);
    let (input, taken, dropped) = Probe::new(vec!["version".to_owned()]);
    let mut owner = engine.runtime.cli_env_retained(
        prefix.clone(),
        Box::new(input),
        MINUTE,
        &[],
        Some(interruption.clone()),
    );
    assert_eq!(
        refusal(end_of(&mut owner).await),
        InvocationFailure::Interrupted {
            signal: libc::SIGINT
        }
    );
    assert_eq!(taken.load(Ordering::SeqCst), 1);
    assert!(
        dropped.load(Ordering::SeqCst),
        "FAIL: a refused input was not dropped before its end was sent"
    );
    assert_eq!(interruption.active(), 0);
    assert_eq!(engine.spawned(), None);

    // Started and collected: the prefix precedes the arguments, taken once.
    let (input, taken, dropped) = Probe::new(vec!["version".to_owned()]);
    let mut owner = engine
        .runtime
        .cli_env_retained(prefix, Box::new(input), MINUTE, &[], None);
    let end = end_of(&mut owner).await;
    let InvocationEnd::Collected(collected) = end else {
        panic!("FAIL: not collected: {end:?}");
    };
    assert!(collected.status.success() && collected.input_released);
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(taken.load(Ordering::SeqCst), 1);
    assert_eq!(
        engine.spawned().as_deref(),
        Some("3 33 0 --host unix:///fixture.sock version")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_that_cannot_be_executed_refuses_the_command_and_releases_its_input() {
    let _serial = SERIAL.lock().await;
    let directory = private_tempdir();
    let program = directory.path().join("docker");
    // A regular executable file, so it is approved; its interpreter does not
    // exist, so executing it fails.
    std::fs::write(&program, "#!/nonexistent/ai-guard-fixture-interpreter\n")
        .expect("SETUP: engine file");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
        .expect("SETUP: engine file mode");
    let runtime = Runtime::with_program(RuntimeKind::Docker, &program)
        .unwrap_or_else(|error| panic!("SETUP: the engine file was not approved: {error:#}"));
    let _turn = adapter::admit(&runtime, SLOT_WINDOW)
        .await
        .unwrap_or_else(|busy| {
            panic!("SETUP: retained slot still busy since the previous test: {busy}")
        });

    let interruption = Interruption::new();
    let (input, _, dropped) = Probe::new(vec!["version".to_owned()]);
    let mut owner = runtime.cli_env_retained(
        vec![],
        Box::new(input),
        MINUTE,
        &[],
        Some(interruption.clone()),
    );
    let end = end_of(&mut owner).await;
    let InvocationEnd::Refused(error) = end else {
        panic!("FAIL: a command whose engine cannot be executed was reported as {end:?}");
    };
    let text = format!("{error:#}");
    assert!(
        text.contains("docker is unavailable"),
        "FAIL: the refusal does not say that the engine CLI is unavailable: {text}"
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "FAIL: the input of a command that was never started was not dropped"
    );
    assert_eq!(interruption.active(), 0, "FAIL: a refusal keeps a hold");
    assert!(
        matches!(
            adapter::slot_refusal(&runtime).await,
            Ok(InvocationFailure::Interrupted { .. })
        ),
        "FAIL: the admission slot is not free after a refusal"
    );
}

#[test]
fn interruption_coalesces_requests_inside_the_minimum_interval() {
    let interruption = Interruption::new();
    assert_eq!(interruption.requested(), None);
    assert!(!interruption.escalated());

    interruption.request(libc::SIGHUP);
    // Paired deliveries of one stop, of the same signal or not, are one request.
    interruption.request(libc::SIGHUP);
    interruption.request(libc::SIGTERM);
    assert_eq!(interruption.requested(), Some(libc::SIGHUP));
    assert!(!interruption.escalated());

    std::thread::sleep(REPEAT_MIN_INTERVAL + Duration::from_millis(50));
    interruption.request(libc::SIGINT);
    assert_eq!(
        interruption.requested(),
        Some(libc::SIGHUP),
        "FAIL: the first signal did not stay the requested one"
    );
    assert!(interruption.escalated());
    assert!(
        interruption.clone().escalated(),
        "FAIL: clones do not share one request"
    );

    // Only the three operator signals are ever forwarded to a process group.
    for other in [libc::SIGKILL, libc::SIGSTOP, libc::SIGQUIT, 0, -1] {
        let interruption = Interruption::new();
        interruption.request(other);
        assert_eq!(interruption.requested(), Some(libc::SIGTERM));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hold_keep_never_becomes_idle() {
    let brief = Duration::from_millis(100);
    let interruption = Interruption::new();
    tokio::time::timeout(brief, interruption.idle())
        .await
        .expect("FAIL: an interruption without a hold is not idle");

    let first = interruption.hold();
    let second = interruption.hold();
    assert_eq!(interruption.active(), 2);
    drop(first);
    assert_eq!(interruption.active(), 1);
    assert!(
        tokio::time::timeout(brief, interruption.idle())
            .await
            .is_err(),
        "FAIL: one live hold is idle"
    );

    // A waiter that started before the last release is woken by it.
    let waiter = tokio::spawn({
        let interruption = interruption.clone();
        async move { interruption.idle().await }
    });
    tokio::time::sleep(brief).await;
    assert!(!waiter.is_finished());
    drop(second);
    tokio::time::timeout(MINUTE, waiter)
        .await
        .expect("FAIL: the last release did not wake the waiter")
        .expect("the waiter does not panic");
    assert_eq!(interruption.active(), 0);

    // Kept: the termination of the work it counts is unknown.
    interruption.hold().keep();
    assert_eq!(interruption.active(), 1);
    assert!(
        tokio::time::timeout(brief, interruption.idle())
            .await
            .is_err(),
        "FAIL: a kept hold became idle"
    );
}

fn collected(status: i32, stopped: Option<Stop>, changed: Option<anyhow::Error>) -> InvocationEnd {
    let capture = || Capture {
        bytes: CANARY.as_bytes().to_vec(),
        total_bytes: CANARY.len() as u64,
        truncated: false,
    };
    InvocationEnd::Collected(Collected {
        status: ExitStatus::from_raw(status),
        stdout: capture(),
        stderr: capture(),
        stopped,
        input_released: true,
        input_changed: changed,
    })
}

/// Wait status of a process that exited by itself with `code`.
fn exited(code: i32) -> i32 {
    code << 8
}

fn formatted(error: &anyhow::Error) -> String {
    let text = format!("{error:#}\n{error:?}");
    assert!(
        !text.contains(CANARY),
        "FAIL: captured output entered an error: {text}"
    );
    text
}

#[test]
fn ends_map_to_typed_results_without_captured_output() {
    let docker = RuntimeKind::Docker;

    let stdout = collected(exited(0), None, None)
        .into_result(docker)
        .expect("a self-ended success has a result");
    assert_eq!(stdout.bytes, CANARY.as_bytes());

    // A self-ended failure is the existing typed error with its bounded stderr.
    let error = collected(exited(7), None, None)
        .into_result(docker)
        .expect_err("a failed command has no result");
    let failure = error
        .downcast_ref::<RuntimeCommandFailure>()
        .unwrap_or_else(|| panic!("FAIL: a self-ended failure mapped to {error:#}"));
    assert_eq!(failure.stderr_capture().bytes, CANARY.as_bytes());
    assert!(formatted(&error).contains("exit status: 7"));

    // A stop by the owner wins over the status, even a successful one.
    let stop = Stop {
        reason: StopReason::TimedOut,
        forwarded: None,
        kill_sent: true,
    };
    let error = collected(exited(0), Some(stop), None)
        .into_result(RuntimeKind::Podman)
        .expect_err("a stopped command has no result");
    let stopped = error
        .downcast_ref::<InvocationStopped>()
        .unwrap_or_else(|| panic!("FAIL: a stopped command mapped to {error:#}"));
    assert_eq!((stopped.stop, stopped.status.code()), (stop, Some(0)));
    let text = formatted(&error);
    for fact in [
        "podman",
        "its bound elapsed",
        "SIGKILL was sent",
        "exit status: 0",
    ] {
        assert!(text.contains(fact), "FAIL: {fact} is missing from: {text}");
    }

    let stop = Stop {
        reason: StopReason::Interrupted {
            signal: libc::SIGINT,
        },
        forwarded: Some(libc::SIGINT),
        kill_sent: false,
    };
    let error = collected(libc::SIGINT, Some(stop), None)
        .into_result(docker)
        .expect_err("a stopped command has no result");
    let text = formatted(&error);
    for fact in ["interrupted by SIGINT", "SIGINT was forwarded", "signal: 2"] {
        assert!(text.contains(fact), "FAIL: {fact} is missing from: {text}");
    }
    assert!(
        !text.contains("SIGKILL"),
        "FAIL: a SIGKILL that was not sent is reported: {text}"
    );

    // A result whose input failed its check after collection is not bound to
    // that input, whatever the status.
    for status in [exited(0), exited(1)] {
        let changed = anyhow::anyhow!("prepared copy differs: Cargo.toml");
        let error = collected(status, None, Some(changed))
            .into_result(docker)
            .expect_err("a result is not bound to material that failed its check");
        assert_eq!(
            error.downcast_ref::<InvocationFailure>(),
            Some(&InvocationFailure::InputChanged)
        );
        assert!(formatted(&error).contains("prepared copy differs: Cargo.toml"));
    }

    let error = InvocationEnd::Refused(InvocationFailure::Capacity.into())
        .into_result(docker)
        .expect_err("a refused command has no result");
    assert_eq!(
        error.downcast_ref::<InvocationFailure>(),
        Some(&InvocationFailure::Capacity)
    );
    let unknown = InvocationEnd::Unknown(UnknownEnd {
        runtime: docker,
        leader_pid: 4242,
        process_group: 4242,
        stop,
        leader_reaped: true,
        wait_uncertain: false,
        stdout_open: true,
        stderr_open: false,
        retained_at: None,
    });
    assert_eq!(
        unknown
            .into_result(docker)
            .expect_err("an unknown termination has no result")
            .downcast_ref::<InvocationFailure>(),
        Some(&InvocationFailure::Unknown)
    );
    let aborted = InvocationEnd::Aborted {
        input_released: false,
    };
    assert_eq!(
        aborted
            .into_result(docker)
            .expect_err("an aborted owner has no result")
            .downcast_ref::<InvocationFailure>(),
        Some(&InvocationFailure::Unavailable)
    );

    // The debug form of an end gives sizes, never the captured bytes.
    let debug = format!("{:?}", collected(exited(7), None, None));
    assert!(debug.contains("stderr_bytes: 22"), "FAIL: {debug}");
    assert!(
        !debug.contains(CANARY) && !debug.contains("67, 65, 80"),
        "FAIL: captured bytes entered a debug form: {debug}"
    );
}

/// The durations the operator guide quotes. A change of one of them changes
/// `docs/setup/local/README.md#interrupt-build-or-compose` and the same anchor
/// of `docs/setup/local/README.fr.md` in the same commit: both state the
/// 10 seconds a forwarded signal is given, the one second below which a
/// repeated signal is the same request, the 2 seconds collection is given
/// after SIGKILL, and 30 minutes for one image build. That last figure is
/// the largest bound a retained command may be given; `operations` passes
/// the bound of a build, and the 180 seconds of a Compose command, as its own
/// literals, which this test does not read.
#[test]
fn stop_durations_are_the_documented_ones() {
    assert_eq!(
        INTERRUPT_GRACE,
        Duration::from_secs(10),
        "FAIL: the grace of a forwarded signal is not the documented one"
    );
    assert_eq!(
        REPEAT_MIN_INTERVAL,
        Duration::from_secs(1),
        "FAIL: the interval of a repeated stop request is not the documented one"
    );
    assert_eq!(
        STOP_OBSERVATION,
        Duration::from_secs(2),
        "FAIL: the observation after SIGKILL is not the documented one"
    );
    assert_eq!(
        MAX_DURATION,
        Duration::from_secs(30 * 60),
        "FAIL: the largest bound of a retained command is not the documented one"
    );
}

/// A probe whose last check counts its calls, then does what the test says.
struct Checking {
    probe: Probe,
    checked: Arc<AtomicUsize>,
    during: Option<Box<dyn Fn() -> anyhow::Result<()> + Send>>,
}

impl RetainedInput for Checking {
    fn arguments(&self) -> Vec<String> {
        self.probe.arguments()
    }
    fn verify_before_spawn(&self) -> anyhow::Result<()> {
        self.checked.fetch_add(1, Ordering::SeqCst);
        self.during.as_ref().map_or(Ok(()), |during| during())
    }
}

/// Puts another file, with the same bytes and mode, at the path of the engine.
fn replace_engine(program: &Path) -> anyhow::Result<()> {
    let staged = program.with_extension("replacement");
    std::fs::copy(program, &staged).context("SETUP: the engine executable could not be copied")?;
    std::fs::rename(&staged, program).context("SETUP: the engine executable could not be replaced")
}

/// The command was refused because its engine executable is no longer the
/// approved one: nothing ran, and nothing of the command is left.
async fn refused_for_its_replaced_engine(
    engine: &Engine,
    owner: &mut RuntimeInvocationOwner,
    interruption: &Interruption,
    dropped: &AtomicBool,
) {
    let end = end_of(owner).await;
    let InvocationEnd::Refused(error) = end else {
        panic!(
            "FAIL: a command whose engine executable was replaced after its approval was \
             reported as {end:?}; record of the engine: {:?}",
            engine.spawned()
        );
    };
    let text = format!("{error:#}");
    if let Some(setup) = text.find("SETUP:") {
        panic!("{}", &text[setup..]);
    }
    assert!(
        text.contains("approved runtime executable identity or content changed"),
        "FAIL: the refusal does not name the replaced engine executable: {text}"
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "FAIL: the input of a command that was never started was not dropped"
    );
    assert_eq!(interruption.active(), 0, "FAIL: a refusal keeps a hold");
    assert_eq!(
        engine.spawned(),
        None,
        "FAIL: the replaced engine executable was started"
    );
    assert!(
        matches!(
            adapter::slot_refusal(&engine.runtime).await,
            Ok(InvocationFailure::Interrupted { .. })
        ),
        "FAIL: the admission slot is not free after a refusal"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_replaced_after_its_approval_refuses_the_command_before_its_input_is_checked() {
    let _serial = SERIAL.lock().await;
    let engine = Engine::compile().await;
    replace_engine(&engine.program()).unwrap_or_else(|error| panic!("{error:#}"));

    let interruption = Interruption::new();
    let (probe, _, dropped) = Probe::new(vec!["version".to_owned()]);
    let checked = Arc::new(AtomicUsize::new(0));
    let input = Checking {
        probe,
        checked: Arc::clone(&checked),
        during: None,
    };
    let mut owner = engine.runtime.cli_env_retained(
        vec![],
        Box::new(input),
        MINUTE,
        &[],
        Some(interruption.clone()),
    );
    refused_for_its_replaced_engine(&engine, &mut owner, &interruption, &dropped).await;
    // The executable is verified before the caller's material: this refusal
    // is the first of the two, and the input was never asked.
    assert_eq!(
        checked.load(Ordering::SeqCst),
        0,
        "FAIL: the input was checked before the engine executable was verified"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_replaced_during_the_last_check_of_its_input_refuses_the_command_before_spawn() {
    let _serial = SERIAL.lock().await;
    let engine = Engine::compile().await;

    let interruption = Interruption::new();
    let (probe, _, dropped) = Probe::new(vec!["version".to_owned()]);
    let checked = Arc::new(AtomicUsize::new(0));
    let program = engine.program();
    let input = Checking {
        probe,
        checked: Arc::clone(&checked),
        // The executable has passed its first verification when this runs:
        // only the verification that follows the input's check can refuse.
        during: Some(Box::new(move || replace_engine(&program))),
    };
    let mut owner = engine.runtime.cli_env_retained(
        vec![],
        Box::new(input),
        MINUTE,
        &[],
        Some(interruption.clone()),
    );
    refused_for_its_replaced_engine(&engine, &mut owner, &interruption, &dropped).await;
    assert_eq!(
        checked.load(Ordering::SeqCst),
        1,
        "SETUP: the check that replaces the engine executable did not run exactly once"
    );
}

async fn end_after(owner: &mut RuntimeInvocationOwner, what: &str) -> InvocationEnd {
    tokio::time::timeout(MINUTE, owner.join())
        .await
        .unwrap_or_else(|_| panic!("FAIL: no end was reported a minute after {what}"))
}

/// Waits until `role` of a quiet command published its readiness. A command
/// that never gets there did not reach its scenario.
async fn quiet_ready(state: &FixtureState, owner: &mut RuntimeInvocationOwner, role: &str) {
    if state.wait_ready(role, READY_WINDOW).is_some() {
        return;
    }
    let product = match tokio::time::timeout(Duration::from_millis(1), owner.join()).await {
        Ok(end) => format!("the command ended: {end:?}"),
        Err(_) => "the command is pending".to_owned(),
    };
    panic!(
        "SETUP: the quiet engine did not publish ready-{role} within {READY_WINDOW:?} \
         ({product}); {}",
        state.describe()
    );
}

/// For `windows` half-seconds in a row: no end is reported, the input is held
/// and the CLI process answers a probe asked in that window.
async fn owned_while_its_cli_process_lives(
    state: &FixtureState,
    owner: &mut RuntimeInvocationOwner,
    dropped: &AtomicBool,
    windows: usize,
) {
    for window in 1..=windows {
        if let Ok(end) = tokio::time::timeout(Duration::from_millis(500), owner.join()).await {
            // An end is a product failure only while the process is alive.
            let class = match state.liveness("leader") {
                Liveness::Alive => {
                    "FAIL: an end was reported while the CLI process lives with both of its \
                     output pipes at end of file"
                }
                _ => "SETUP: the CLI process of the quiet engine ended during the scenario",
            };
            panic!("{class} (window {window}): {end:?}; {}", state.describe());
        }
        assert!(
            !dropped.load(Ordering::SeqCst),
            "FAIL: window {window}: the input was released while the CLI process lives"
        );
        assert!(
            state.probe("leader", PROBE_WINDOW),
            "FAIL: window {window}: the CLI process does not answer a probe; {}",
            state.describe()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_whose_output_is_at_end_of_file_stays_owned_while_its_cli_process_lives() {
    let _serial = SERIAL.lock().await;
    let engine = Engine::compile_from(QUIET_ENGINE).await;

    // Left alone, the command is collected only once its CLI process exited.
    let (state, arguments) = engine.quiet_command("released");
    let interruption = Interruption::new();
    let (input, _, dropped) = Probe::new(arguments("quiet-leader"));
    let mut owner = engine.runtime.cli_env_retained(
        vec![],
        Box::new(input),
        LONG,
        &[],
        Some(interruption.clone()),
    );
    // Published after the process replaced both descriptors: from here on no
    // process holds a write end of either pipe.
    quiet_ready(&state, &mut owner, "leader").await;
    owned_while_its_cli_process_lives(&state, &mut owner, &dropped, 3).await;
    assert_eq!(
        interruption.active(),
        1,
        "FAIL: the hold of a pending command ended"
    );
    state.release();
    let end = end_after(&mut owner, "the CLI process was released").await;
    let InvocationEnd::Collected(collected) = end else {
        panic!("FAIL: a command whose CLI process exited by itself was reported as {end:?}");
    };
    assert_eq!(
        state.liveness("leader"),
        Liveness::Ended,
        "FAIL: the end was reported while the CLI process is alive"
    );
    assert_eq!(
        collected.status.code(),
        Some(3),
        "FAIL: the status is not the one the CLI process exited with: {collected:?}"
    );
    assert!(
        collected.stopped.is_none() && collected.input_changed.is_none(),
        "FAIL: {collected:?}"
    );
    assert!(
        collected.input_released && dropped.load(Ordering::SeqCst),
        "FAIL: the input of a collected command was not released"
    );
    assert_eq!(
        interruption.active(),
        0,
        "FAIL: a collected command keeps a hold"
    );
    drop(state);

    // A stop is still heard in that state: the process group is killed and
    // the command is collected with the status of that kill.
    let (state, arguments) = engine.quiet_command("cancelled");
    let (input, _, dropped) = Probe::new(arguments("quiet-leader"));
    let mut owner = engine
        .runtime
        .cli_env_retained(vec![], Box::new(input), LONG, &[], None);
    quiet_ready(&state, &mut owner, "leader").await;
    owned_while_its_cli_process_lives(&state, &mut owner, &dropped, 1).await;
    owner.cancel();
    let end = end_after(&mut owner, "the command was cancelled").await;
    let InvocationEnd::Collected(collected) = end else {
        panic!("FAIL: a cancelled command whose pipes were at end of file was reported as {end:?}");
    };
    assert_eq!(
        state.liveness("leader"),
        Liveness::Ended,
        "FAIL: the end was reported while the CLI process is alive"
    );
    assert_eq!(
        collected.status.signal(),
        Some(libc::SIGKILL),
        "FAIL: the CLI process did not end by the SIGKILL of the stop: {collected:?}"
    );
    assert_eq!(
        collected.stopped,
        Some(Stop {
            reason: StopReason::Cancelled,
            forwarded: None,
            kill_sent: true
        })
    );
    assert!(
        collected.input_released && dropped.load(Ordering::SeqCst),
        "FAIL: the input of a collected command was not released"
    );
}

/// What the stated limit says of a collected command: its helper, which holds
/// neither output pipe, is alive in the command's process group after the
/// input was released. The helper is then told to leave.
fn helper_outlived_the_collection(state: &FixtureState) {
    let published = |name: &str| -> libc::pid_t {
        state
            .read(name)
            .and_then(|pid| pid.trim().parse().ok())
            .unwrap_or_else(|| {
                panic!(
                    "SETUP: the quiet engine did not publish {name}; {}",
                    state.describe()
                )
            })
    };
    let (leader, helper) = (published("leader-pid"), published("ready-descendant"));
    assert!(
        state.probe("descendant", PROBE_WINDOW),
        "CONTRACT: the stated limit no longer holds: a process of the command that holds \
         neither output pipe did not outlive the collection. If the owner now ends or waits \
         for such a process, change the module documentation of `runtime::retained_cli` and \
         the limits under docs/setup/local/README.md#interrupt-build-or-compose (EN and FR) \
         with this test; {}",
        state.describe()
    );
    // SAFETY: `getpgid` only reads. The helper just answered, so its PID
    // names it; the group of a live process keeps its identifier.
    let group = unsafe { libc::getpgid(helper) };
    assert_eq!(
        group, leader,
        "SETUP: the helper is not in the process group the command was started in"
    );
    state.release();
    assert!(
        state.ended("descendant", END_WINDOW),
        "SETUP: the helper did not leave; {}",
        state.describe()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stated_limit_a_process_without_the_output_pipes_outlives_the_collection_of_its_command() {
    let _serial = SERIAL.lock().await;
    let engine = Engine::compile_from(QUIET_ENGINE).await;

    // The command ends by itself: nothing is sent to its process group.
    let (state, arguments) = engine.quiet_command("self-ended");
    let (input, _, dropped) = Probe::new(arguments("quiet-helper"));
    let mut owner = engine
        .runtime
        .cli_env_retained(vec![], Box::new(input), LONG, &[], None);
    let end = end_after(&mut owner, "a command that exits by itself was started").await;
    let InvocationEnd::Collected(collected) = end else {
        panic!("FAIL: a command whose CLI process exited by itself was reported as {end:?}");
    };
    assert_eq!(
        collected.status.code(),
        Some(0),
        "SETUP: the quiet engine did not start its helper: {collected:?}; {}",
        state.describe()
    );
    assert!(collected.stopped.is_none(), "FAIL: {collected:?}");
    assert!(
        collected.input_released && dropped.load(Ordering::SeqCst),
        "FAIL: the input of a collected command was not released"
    );
    helper_outlived_the_collection(&state);
    drop(state);

    // The command ends on the forwarded signal, which its helper ignores:
    // nothing else is sent to the process group.
    let (state, arguments) = engine.quiet_command("interrupted");
    let interruption = Interruption::new();
    let (input, _, dropped) = Probe::new(arguments("quiet-helper-and-wait"));
    let mut owner = engine.runtime.cli_env_retained(
        vec![],
        Box::new(input),
        LONG,
        &[],
        Some(interruption.clone()),
    );
    quiet_ready(&state, &mut owner, "leader").await;
    interruption.request(libc::SIGINT);
    let end = end_after(&mut owner, "SIGINT was requested").await;
    let InvocationEnd::Collected(collected) = end else {
        panic!("FAIL: a command that died on the forwarded signal was reported as {end:?}");
    };
    assert_eq!(
        state.liveness("leader"),
        Liveness::Ended,
        "FAIL: the end was reported while the CLI process is alive"
    );
    assert_eq!(
        collected.status.signal(),
        Some(libc::SIGINT),
        "FAIL: the CLI process did not end by the forwarded signal: {collected:?}"
    );
    assert_eq!(
        collected.stopped,
        Some(Stop {
            reason: StopReason::Interrupted {
                signal: libc::SIGINT
            },
            forwarded: Some(libc::SIGINT),
            kill_sent: false
        })
    );
    assert!(
        collected.input_released && dropped.load(Ordering::SeqCst),
        "FAIL: the input of a collected command was not released"
    );
    assert_eq!(interruption.active(), 0);
    helper_outlived_the_collection(&state);
}
