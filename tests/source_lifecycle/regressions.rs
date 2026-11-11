//! R4b–R8: what the prepared source of `agentsandbox build`, `up` and `down`
//! does beyond the reproduced defects: a copy that exists when a signal
//! arrives between engine commands, a consumer that ignores the forwarded
//! signal, repeated signals, a signal inherited as ignored, a termination that
//! cannot be established, sources, temporary directories and a signal
//! disposition that are refused, and a prepared copy altered or replaced
//! before or during its consumption.
//!
//! The oracles are those of R1–R5: a probe answered by the fixture process
//! itself, the liveness lock the kernel releases, and what is really left in
//! the test's private temporary directory. Signals go to the control process
//! only. The grace and observation durations are the product's own, in real
//! time.
use crate::harness::{
    COMPOSE_FILE, Exit, FixtureState, Liveness, Operator, Running, Spawn, TreeEntry, build_inputs,
    install_engine, missed_ready, tail, tree_manifest,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::{self, File},
    io,
    os::unix::{
        fs::{PermissionsExt, symlink},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Test-side observation windows. They are not product deadlines.
const READY_WINDOW: Duration = Duration::from_secs(60);
const EXIT_WINDOW: Duration = Duration::from_secs(60);
const PROBE_WINDOW: Duration = Duration::from_secs(3);
/// A probe asked while a product grace runs must be answered well inside it.
const GRACE_PROBE_WINDOW: Duration = Duration::from_millis(1500);
const END_WINDOW: Duration = Duration::from_secs(10);

/// The product's documented durations, in real time: the grace a forwarded
/// signal is given, the interval below which a repeated signal is the same
/// request, and the time collection is given after SIGKILL.
const INTERRUPT_GRACE: Duration = Duration::from_secs(10);
const REPEAT_MIN_INTERVAL: Duration = Duration::from_secs(1);
const STOP_OBSERVATION: Duration = Duration::from_secs(2);

const RETAINED_ANCHOR: &str = "docs/setup/local/README.md#prepared-source-retained";

/// The error the command reported, for the run's log.
pub(crate) fn report(exit: &Exit) {
    let message = exit
        .stderr
        .lines()
        .find(|line| line.starts_with("Error: "))
        .unwrap_or("no error line");
    eprintln!("message: {message}");
}

fn sleep_until(instant: Instant) {
    std::thread::sleep(instant.saturating_duration_since(Instant::now()));
}

/// Waits until `role` of a `build` consumer published its readiness.
pub(crate) fn await_ready(scenario: &Operator, running: &mut Running, role: &str) {
    if scenario.state.wait_ready(role, READY_WINDOW).is_none() {
        let product = running.product();
        panic!(
            "{}",
            missed_ready(
                &scenario.state,
                role,
                "build",
                running.spawned_at,
                READY_WINDOW,
                &product
            )
        );
    }
}

/// The first build is consuming: its leader read its inputs, answers a probe
/// and the command is pending.
pub(crate) fn await_holding_build(scenario: &Operator, running: &mut Running) {
    await_ready(scenario, running, "leader");
    assert!(
        scenario.state.probe("leader", PROBE_WINDOW),
        "SETUP: the build consumer did not answer a probe before the signal; {}",
        scenario.state.describe()
    );
    if let Some(early) = running.poll() {
        panic!(
            "FAIL: the command ended while its build consumer was holding: {}",
            early.describe()
        );
    }
}

pub(crate) fn single_prepared(scenario: &Operator) -> PathBuf {
    let mut prepared = scenario.prepared_directories();
    assert_eq!(
        prepared.len(),
        1,
        "FAIL: expected exactly one prepared source while a build consumes it: {:?}",
        scenario.temporary_entries()
    );
    prepared.remove(0)
}

/// Relative path, mode and bytes of each entry of a tree.
type Tree = BTreeMap<String, (u32, Option<Vec<u8>>)>;

/// Mode and bytes of every entry below `root`; a directory has no bytes. An
/// error is returned, not turned into a panic: a tree that disappears is an
/// observation of the test.
fn snapshot(root: &Path) -> io::Result<Tree> {
    fn visit(root: &Path, relative: &Path, entries: &mut Tree) -> io::Result<()> {
        for entry in fs::read_dir(root.join(relative))? {
            let relative = relative.join(entry?.file_name());
            let path = root.join(&relative);
            let metadata = fs::symlink_metadata(&path)?;
            let bytes = if metadata.is_file() {
                Some(fs::read(&path)?)
            } else {
                None
            };
            entries.insert(
                relative.to_string_lossy().into_owned(),
                (metadata.permissions().mode() & 0o7777, bytes),
            );
            if metadata.is_dir() {
                visit(root, &relative, entries)?;
            }
        }
        Ok(())
    }
    let mut entries = BTreeMap::new();
    visit(root, Path::new(""), &mut entries)?;
    Ok(entries)
}

pub(crate) fn no_material_marker(scenario: &Operator, role: &str) {
    for kind in ["material-missing", "material-changed"] {
        let marker = format!("{kind}-{role}");
        assert!(
            !scenario.state.exists(&marker),
            "FAIL: the {role} recorded {marker}: {:?}",
            scenario.state.read(&marker)
        );
    }
}

/// What a sample saw during the grace of the product. The product gives its
/// consumer time until `INTERRUPT_GRACE` after the signal at least; a sample
/// that overran that time (a stalled host) says nothing about it and is
/// inconclusive, not a product failure.
fn observed(sent: Instant, what: String) -> ! {
    let after = sent.elapsed();
    if after < INTERRUPT_GRACE {
        panic!("FAIL: {after:?} after the signal, {what}");
    }
    panic!("SETUP: a sample overran the grace of the product ({after:?} after the signal): {what}");
}

/// While the command is pending after a signal: its consumer is alive, reads
/// the same bytes by pathname, and the prepared source is what it was.
fn still_consuming(
    scenario: &Operator,
    running: &mut Running,
    prepared: &Path,
    captured: &Tree,
    sent: Instant,
) {
    let pending = |running: &mut Running| {
        if let Some(early) = running.poll() {
            observed(
                sent,
                format!(
                    "the command ended while its consumer was given time: {}",
                    early.describe()
                ),
            );
        }
    };
    pending(running);
    if !scenario.state.probe("leader", GRACE_PROBE_WINDOW) {
        observed(
            sent,
            format!(
                "the build consumer did not answer a probe; {}",
                scenario.state.describe()
            ),
        );
    }
    no_material_marker(scenario, "leader");
    match snapshot(prepared) {
        Ok(now) if now == *captured => {}
        Ok(_) => observed(
            sent,
            "the prepared source changed while its consumer is alive".to_owned(),
        ),
        Err(error) => observed(
            sent,
            format!("the prepared source is unavailable while its consumer is alive: {error}"),
        ),
    }
    pending(running);
}

pub(crate) fn await_exit(scenario: &Operator, running: &mut Running, what: &str) -> Exit {
    running.wait(EXIT_WINDOW).unwrap_or_else(|| {
        panic!(
            "FAIL: the command is still pending {EXIT_WINDOW:?} after {what}; {}",
            scenario.state.describe()
        )
    })
}

/// The command stopped a consumer that had ignored the forwarded SIGTERM.
fn ended_by_kill_after_sigterm(scenario: &Operator, exit: &Exit) {
    report(exit);
    assert_eq!(
        exit.status.code(),
        Some(1),
        "FAIL: unexpected status: {}",
        exit.describe()
    );
    assert!(
        !exit.stderr.contains("termination is unknown"),
        "FAIL: unexpected unknown termination: {}",
        exit.describe()
    );
    for text in [
        "interrupted by SIGTERM",
        ", then SIGKILL was sent",
        "signal: 9",
    ] {
        assert!(
            exit.stderr.contains(text),
            "FAIL: the message does not say {text:?}; stderr tail:\n{}",
            tail(&exit.stderr, 12)
        );
    }
    assert!(
        scenario.state.ended("leader", END_WINDOW),
        "FAIL: the command returned but its build consumer was not proven ended; {}",
        scenario.state.describe()
    );
    no_material_marker(scenario, "leader");
    assert_eq!(
        scenario.state.effects("build"),
        1,
        "FAIL: the second image build was started after the interruption; calls: {}",
        scenario.state.calls()
    );
    let left = scenario.temporary_entries();
    assert!(
        left.is_empty(),
        "FAIL: the private temporary directory is not empty: {left:?}"
    );
}

#[test]
fn r5b_a_consumer_that_ignores_the_forwarded_signal_keeps_its_source_for_the_whole_grace() {
    let scenario = Operator::prepare();
    scenario.state.plan(&[
        ("hold", "consume"),
        ("ignore-signals", "leader"),
        ("deadline-seconds", "120"),
    ]);
    let mut running = scenario.spawn("build", &[], &Spawn::default());
    await_holding_build(&scenario, &mut running);
    let prepared = single_prepared(&scenario);
    let captured = snapshot(&prepared).expect("SETUP: the prepared source can be read");

    running.signal(libc::SIGTERM);
    let sent = Instant::now();
    for tenths in [2, 5, 8] {
        sleep_until(sent + INTERRUPT_GRACE * tenths / 10);
        still_consuming(&scenario, &mut running, &prepared, &captured, sent);
    }
    let exit = await_exit(&scenario, &mut running, "SIGTERM");
    let waited = sent.elapsed();
    assert!(
        waited >= INTERRUPT_GRACE,
        "FAIL: the command ended {waited:?} after SIGTERM, before the grace of {INTERRUPT_GRACE:?}"
    );
    eprintln!("latency: SIGTERM to the end of the command {waited:?}");
    ended_by_kill_after_sigterm(&scenario, &exit);
}

#[test]
fn r5c_paired_signals_are_one_request_and_a_later_one_ends_the_grace() {
    let scenario = Operator::prepare();
    scenario.state.plan(&[
        ("hold", "consume"),
        ("ignore-signals", "leader"),
        ("deadline-seconds", "120"),
    ]);
    let mut running = scenario.spawn("build", &[], &Spawn::default());
    await_holding_build(&scenario, &mut running);
    let prepared = single_prepared(&scenario);
    let captured = snapshot(&prepared).expect("SETUP: the prepared source can be read");

    // Two deliveries of one stop, as a shell and a kernel, or a supervisor, send them.
    running.signal(libc::SIGTERM);
    let first = Instant::now();
    std::thread::sleep(Duration::from_millis(50));
    running.signal(libc::SIGTERM);
    sleep_until(first + Duration::from_secs(2));
    still_consuming(&scenario, &mut running, &prepared, &captured, first);

    // A further signal, later than the minimum interval, escalates.
    assert!(
        first.elapsed() >= REPEAT_MIN_INTERVAL + Duration::from_millis(200),
        "SETUP: the third signal would not be a later request"
    );
    running.signal(libc::SIGTERM);
    let exit = await_exit(&scenario, &mut running, "a repeated SIGTERM");
    let waited = first.elapsed();
    assert!(
        waited < INTERRUPT_GRACE - Duration::from_secs(2),
        "FAIL: a repeated signal did not end the grace: the command ended {waited:?} after the \
         first signal"
    );
    eprintln!("latency: first SIGTERM to the end of the command {waited:?}");
    ended_by_kill_after_sigterm(&scenario, &exit);
}

#[test]
fn r5d_a_signal_inherited_as_ignored_stays_ignored_and_the_command_completes() {
    let scenario = Operator::prepare();
    scenario
        .state
        .plan(&[("hold", "consume"), ("deadline-seconds", "120")]);
    let options = Spawn {
        ignored: vec![libc::SIGHUP],
        ..Spawn::default()
    };
    let mut running = scenario.spawn("build", &[], &options);
    await_holding_build(&scenario, &mut running);

    running.signal(libc::SIGHUP);
    std::thread::sleep(Duration::from_secs(2));
    if let Some(early) = running.poll() {
        panic!(
            "FAIL: the command ended on a signal it had inherited as ignored: {}",
            early.describe()
        );
    }
    assert!(
        scenario.state.probe("leader", PROBE_WINDOW),
        "FAIL: the build consumer stopped answering after an ignored SIGHUP; {}",
        scenario.state.describe()
    );
    no_material_marker(&scenario, "leader");

    scenario.state.release();
    let exit = await_exit(&scenario, &mut running, "the consumers were released");
    assert!(
        exit.status.success(),
        "FAIL: the command did not complete after an ignored SIGHUP: {}",
        exit.describe()
    );
    assert!(
        !exit.stderr.contains("interrupted"),
        "FAIL: an ignored signal was reported as an interruption; stderr tail:\n{}",
        tail(&exit.stderr, 12)
    );
    assert_eq!(
        scenario.state.effects("build"),
        2,
        "FAIL: expected both image builds; calls: {}",
        scenario.state.calls()
    );
    let left = scenario.temporary_entries();
    assert!(
        left.is_empty(),
        "FAIL: the private temporary directory is not empty: {left:?}"
    );
}

#[test]
fn r6_unknown_termination_keeps_the_prepared_source_and_says_where() {
    let scenario = Operator::prepare();
    scenario.state.plan(&[
        ("hold", "consume"),
        ("descendant", "own-group"),
        ("deadline-seconds", "120"),
    ]);
    let mut running = scenario.spawn("build", &[], &Spawn::default());
    await_ready(&scenario, &mut running, "leader");
    await_ready(&scenario, &mut running, "descendant");
    for role in ["leader", "descendant"] {
        assert!(
            scenario.state.probe(role, PROBE_WINDOW),
            "SETUP: the {role} did not answer a probe before the signal; {}",
            scenario.state.describe()
        );
    }
    if let Some(early) = running.poll() {
        panic!(
            "FAIL: the command ended while its build consumers were holding: {}",
            early.describe()
        );
    }
    let prepared = single_prepared(&scenario);
    let leader: u64 = scenario
        .state
        .read("ready-leader")
        .and_then(|pid| pid.trim().parse().ok())
        .expect("SETUP: the leader published its PID");

    running.signal(libc::SIGTERM);
    let sent = Instant::now();
    let exit = await_exit(&scenario, &mut running, "SIGTERM");
    let waited = sent.elapsed();
    eprintln!("latency: SIGTERM to the end of the command {waited:?}");
    report(&exit);

    assert_eq!(
        exit.status.code(),
        Some(1),
        "FAIL: unexpected status: {}",
        exit.describe()
    );
    assert!(
        exit.stderr.contains("termination is unknown"),
        "FAIL: the command ended without saying that the termination is unknown while a \
         process holds its output: {}; {}",
        exit.describe(),
        scenario.state.describe()
    );
    assert!(
        waited >= INTERRUPT_GRACE + STOP_OBSERVATION,
        "FAIL: unknown termination announced {waited:?} after SIGTERM, before the grace and \
         the observation of the product"
    );
    let named = prepared.to_string_lossy().into_owned();
    assert!(
        exit.stderr.contains(&named) && exit.stderr.contains(RETAINED_ANCHOR),
        "FAIL: the message does not name {named} and {RETAINED_ANCHOR}; stderr tail:\n{}",
        tail(&exit.stderr, 12)
    );
    assert!(
        exit.stderr.contains("is not removed by ai-guard")
            && exit.stderr.contains("SIGTERM was forwarded")
            && exit.stderr.contains("then SIGKILL was sent"),
        "FAIL: the message does not state what was sent and that the copy stays; stderr \
         tail:\n{}",
        tail(&exit.stderr, 12)
    );

    // The copy is still there, alone, and still is what its manifest says.
    assert_eq!(
        scenario.prepared_directories(),
        vec![prepared.clone()],
        "FAIL: the retained prepared source is not the only one left"
    );
    assert_eq!(scenario.temporary_entries().len(), 1);
    let read_json = |name: &str| -> serde_json::Value {
        let bytes = fs::read(prepared.join(name)).unwrap_or_else(|error| {
            panic!("FAIL: {name} is missing in the retained copy: {error}")
        });
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("FAIL: {name} is not JSON: {error}"))
    };
    let manifest = read_json("manifest.json");
    let context = snapshot(&prepared.join("context"))
        .unwrap_or_else(|error| panic!("FAIL: the retained copy cannot be read: {error}"));
    let files: BTreeMap<&str, &Vec<u8>> = context
        .iter()
        .filter_map(|(path, (_, bytes))| Some((path.as_str(), bytes.as_ref()?)))
        .collect();
    let entries = manifest["entries"]
        .as_array()
        .expect("FAIL: the manifest lists no entries");
    assert_eq!(
        entries.len(),
        files.len(),
        "FAIL: the retained copy and its manifest list different files"
    );
    assert_eq!(entries.len(), build_inputs().len());
    for entry in entries {
        let path = entry["path"].as_str().unwrap_or_default();
        let bytes = files
            .get(path)
            .unwrap_or_else(|| panic!("FAIL: {path} of the manifest is not in the retained copy"));
        assert_eq!(entry["bytes"], bytes.len() as u64, "FAIL: length of {path}");
        assert_eq!(
            entry["sha256"],
            hex::encode(Sha256::digest(bytes)),
            "FAIL: content of {path}"
        );
    }
    let marker = read_json("RETAINED-UNKNOWN.json");
    assert!(
        marker["stdout_open"] == true || marker["stderr_open"] == true,
        "FAIL: the marker names no open pipe: {marker}"
    );
    assert_eq!(marker["version"], 1, "{marker}");
    assert_eq!(marker["verb"], "build", "{marker}");
    assert_eq!(marker["runtime"], "docker", "{marker}");
    assert_eq!(marker["scope"], "build", "{marker}");
    assert_eq!(marker["sha256"], manifest["sha256"], "{marker}");
    assert_eq!(marker["control_pid"], running.pid, "{marker}");
    assert_eq!(marker["leader_pid"], leader, "{marker}");
    assert_eq!(marker["process_group"], leader, "{marker}");
    assert_eq!(marker["forwarded_signal"], "SIGTERM", "{marker}");
    assert_eq!(marker["kill_sent"], true, "{marker}");
    assert_eq!(marker["wait_uncertain"], false, "{marker}");

    // The process that still holds the output reads the same bytes by pathname.
    assert!(
        scenario.state.ended("leader", END_WINDOW),
        "FAIL: the CLI process that was signalled was not proven ended; {}",
        scenario.state.describe()
    );
    assert_eq!(
        scenario.state.liveness("descendant"),
        Liveness::Alive,
        "SETUP: the process holding the output is gone; {}",
        scenario.state.describe()
    );
    assert!(
        scenario.state.probe("descendant", PROBE_WINDOW),
        "FAIL: the process holding the output does not answer; {}",
        scenario.state.describe()
    );
    std::thread::sleep(Duration::from_millis(500));
    no_material_marker(&scenario, "descendant");
    assert_eq!(
        scenario.state.effects("build"),
        1,
        "FAIL: the second image build was started; calls: {}",
        scenario.state.calls()
    );

    // Nothing of this test outlives it.
    scenario.state.quit();
    assert!(
        scenario.state.ended("descendant", END_WINDOW),
        "SETUP: the process holding the output did not leave; {}",
        scenario.state.describe()
    );
}

/// The command was refused with status 1, said why, left nothing in the
/// private temporary directory and did not touch the original.
fn refused(
    scenario: &Operator,
    exit: &Exit,
    original: &BTreeMap<String, TreeEntry>,
    says: &[&str],
) {
    report(exit);
    assert_eq!(
        exit.status.code(),
        Some(1),
        "FAIL: unexpected status of a refused command: {}",
        exit.describe()
    );
    for text in says {
        assert!(
            exit.stderr.contains(text),
            "FAIL: the refusal does not say {text:?}; stderr tail:\n{}",
            tail(&exit.stderr, 12)
        );
    }
    let left = scenario.temporary_entries();
    assert!(
        left.is_empty(),
        "FAIL: a refused command left a prepared source behind: {left:?}"
    );
    assert!(
        tree_manifest(&scenario.source) == *original,
        "FAIL: the original source tree changed"
    );
}

pub(crate) fn no_engine_call(scenario: &Operator) {
    assert!(
        scenario.state.invocations().is_empty() && scenario.state.calls().is_empty(),
        "FAIL: the engine CLI was started before the refusal; calls: {}",
        scenario.state.calls()
    );
}

/// `build` on a source prepared by `arrange` is refused before any engine call.
fn build_refused(arrange: impl FnOnce(&Operator), says: &[&str]) {
    let scenario = Operator::prepare();
    arrange(&scenario);
    let original = tree_manifest(&scenario.source);
    let exit = scenario.run("build", &[], &Spawn::default());
    refused(&scenario, &exit, &original, says);
    no_engine_call(&scenario);
}

/// `down` on a source prepared by `arrange` is refused before the Compose
/// command; the engine probes that select the runtime come first.
fn down_refused(arrange: impl FnOnce(&Operator), says: &[&str]) {
    let scenario = Operator::prepare();
    scenario.initialize_config();
    arrange(&scenario);
    let original = tree_manifest(&scenario.source);
    let config = scenario.config.to_string_lossy().into_owned();
    let exit = scenario.run("down", &["--config-dir", &config], &Spawn::default());
    refused(&scenario, &exit, &original, says);
    assert!(
        scenario.state.effects("compose") == 0 && scenario.state.consumers("compose").is_empty(),
        "FAIL: a Compose command was started; calls: {}",
        scenario.state.calls()
    );
}

/// How one `agentsandbox build` is started when the defaults of the harness
/// do not fit the scenario.
#[derive(Default)]
pub(crate) struct Start<'a> {
    /// The value of `TMPDIR`. `None` leaves the variable unset: the default
    /// temporary directory of the platform applies.
    pub(crate) tmpdir: Option<&'a OsStr>,
    /// The working directory of the control process.
    pub(crate) directory: Option<&'a Path>,
    /// Soft and hard limit of open file descriptors of the control process
    /// and of the processes it starts.
    pub(crate) descriptors: Option<u64>,
    /// Signals the control process inherits as ignored.
    pub(crate) ignored: &'a [i32],
}

/// Runs `agentsandbox build --runtime-program <fixture> --source <synthetic
/// source>` to its end, started as `start` says.
pub(crate) fn build_started_with(scenario: &Operator, start: &Start) -> Exit {
    started_with(scenario, "build", &[], start)
}

/// Runs `agentsandbox <action> --runtime-program <fixture> --source <synthetic
/// source> <further arguments>` to its end, started as `start` says.
fn started_with(scenario: &Operator, action: &str, further: &[&str], start: &Start) -> Exit {
    let base = scenario.source.parent().expect("source has a parent");
    let stdout = base.join("build-started-with.stdout");
    let stderr = base.join("build-started-with.stderr");
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .arg(action)
        .arg("--runtime-program")
        .arg(&scenario.program)
        .arg("--source")
        .arg(&scenario.source)
        .args(further)
        .stdin(Stdio::null())
        .stdout(File::create(&stdout).expect("SETUP: command output file"))
        .stderr(File::create(&stderr).expect("SETUP: command output file"))
        .process_group(0);
    match start.tmpdir {
        Some(value) => command.env("TMPDIR", value),
        None => command.env_remove("TMPDIR"),
    };
    if let Some(directory) = start.directory {
        command.current_dir(directory);
    }
    if let Some(descriptors) = start.descriptors {
        let limit = libc::rlimit {
            rlim_cur: descriptors as libc::rlim_t,
            rlim_max: descriptors as libc::rlim_t,
        };
        // SAFETY: the hook only calls async-signal-safe `setrlimit` and reads
        // a value owned by the closure.
        unsafe {
            command.pre_exec(move || {
                if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) == 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            });
        }
    }
    if !start.ignored.is_empty() {
        let ignored = start.ignored.to_vec();
        // SAFETY: the hook only calls async-signal-safe `signal` and reads
        // memory owned by the closure.
        unsafe {
            command.pre_exec(move || {
                for signal in &ignored {
                    if libc::signal(*signal, libc::SIG_IGN) == libc::SIG_ERR {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
    }
    let started = Instant::now();
    let mut child = command
        .spawn()
        .expect("SETUP: agentsandbox was not started");
    let status = loop {
        if let Some(status) = child.try_wait().expect("SETUP: child status") {
            break status;
        }
        if started.elapsed() >= 2 * EXIT_WINDOW {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "UNKNOWN: agentsandbox {action} is still pending after {:?}; calls: {:?}; {}",
                2 * EXIT_WINDOW,
                scenario.state.calls(),
                scenario.state.describe()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Exit {
        status,
        stdout: fs::read_to_string(&stdout).unwrap_or_default(),
        stderr: fs::read_to_string(&stderr).unwrap_or_default(),
        elapsed: started.elapsed(),
    }
}

#[test]
fn r7a_a_linked_build_input_is_refused_by_name_before_any_engine_call() {
    build_refused(
        |scenario| {
            symlink(
                scenario.source.join("Cargo.toml"),
                scenario.source.join("src/linked.rs"),
            )
            .expect("SETUP: linked build input");
        },
        &["must not be a symbolic link", "src/linked.rs"],
    );
}

#[test]
fn r7b_a_linked_source_directory_is_refused_by_name_before_any_engine_call() {
    // Below a required directory: the prepared source refuses it instead of
    // leaving its files out.
    build_refused(
        |scenario| {
            let linked = scenario.source.join("front/src/views");
            fs::remove_dir_all(&linked).expect("SETUP: source directory");
            symlink(scenario.source.join("front/src/theme"), linked)
                .expect("SETUP: linked source directory");
        },
        &["must not be a symbolic link", "front/src/views"],
    );
    // A required directory itself: the structural check of the selected
    // source refuses it first, with its own message.
    build_refused(
        |scenario| {
            let moved = scenario.source.join("front/moved");
            fs::rename(scenario.source.join("front/src"), &moved).expect("SETUP: moved directory");
            symlink(&moved, scenario.source.join("front/src")).expect("SETUP: linked directory");
        },
        &["must not be a symbolic link", "front/src"],
    );
}

#[test]
fn r7c_a_special_file_among_the_build_inputs_is_refused_by_name_without_blocking() {
    build_refused(
        |scenario| {
            let pipe = std::ffi::CString::new(
                scenario
                    .source
                    .join("src/pipe.rs")
                    .to_str()
                    .expect("SETUP: UTF-8 test path"),
            )
            .expect("SETUP: path without NUL");
            // SAFETY: `pipe` is a NUL-terminated path owned by this test.
            let made = unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) };
            assert_eq!(made, 0, "SETUP: the FIFO could not be created");
        },
        &["must be a regular file", "src/pipe.rs"],
    );
}

#[test]
fn r7d_a_linked_compose_file_is_refused_by_name_before_the_compose_command() {
    down_refused(
        |scenario| {
            let file = scenario.source.join(COMPOSE_FILE);
            fs::rename(&file, file.with_file_name("real.yaml")).expect("SETUP: Compose file");
            symlink("real.yaml", &file).expect("SETUP: linked Compose file");
        },
        &["must not be a symbolic link", COMPOSE_FILE],
    );
}

#[test]
fn r7e_a_temporary_directory_writable_by_others_is_refused_and_named() {
    let scenario = Operator::prepare();
    fs::set_permissions(&scenario.tmp, fs::Permissions::from_mode(0o777))
        .expect("SETUP: temporary directory mode");
    let original = tree_manifest(&scenario.source);
    let exit = scenario.run("build", &[], &Spawn::default());
    let named = scenario.tmp.to_string_lossy().into_owned();
    refused(
        &scenario,
        &exit,
        &original,
        &["TMPDIR", &named, "group/other"],
    );
    no_engine_call(&scenario);
}

#[test]
fn r7f_a_source_path_deeper_than_the_bound_is_refused() {
    build_refused(
        |scenario| {
            let directory = scenario.source.join(format!("src/{}", "d/".repeat(64)));
            fs::create_dir_all(&directory).expect("SETUP: deep source directory");
            fs::write(directory.join("deep.rs"), "deep\n").expect("SETUP: deep source file");
        },
        &["exceeds the bound of 64 components", "src/d/d/"],
    );
}

#[test]
fn r7g_an_oversized_compose_file_is_refused_by_name_before_the_compose_command() {
    down_refused(
        |scenario| {
            fs::write(
                scenario.source.join(COMPOSE_FILE),
                vec![b'#'; 1024 * 1024 + 1],
            )
            .expect("SETUP: oversized Compose file");
        },
        &["per-file bound of 1048576 bytes", COMPOSE_FILE],
    );
}

#[test]
fn r7h_an_oversized_build_input_and_too_many_entries_are_refused() {
    build_refused(
        |scenario| {
            fs::write(
                scenario.source.join("src/big.rs"),
                vec![b'/'; 32 * 1024 * 1024 + 1],
            )
            .expect("SETUP: oversized build input");
        },
        &["per-file bound of 33554432 bytes", "src/big.rs"],
    );
    build_refused(
        |scenario| {
            let directory = scenario.source.join("src/many");
            fs::create_dir(&directory).expect("SETUP: crowded source directory");
            for number in 0..=16_384 {
                fs::write(directory.join(format!("{number}.txt")), b"")
                    .expect("SETUP: crowded source entry");
            }
        },
        &["bound of 16384 entries"],
    );
}

#[test]
fn r7i_unrelated_links_and_inputs_writable_by_group_do_not_refuse_a_build() {
    let scenario = Operator::prepare();
    for relative in build_inputs() {
        fs::set_permissions(
            scenario.source.join(relative),
            fs::Permissions::from_mode(0o644),
        )
        .expect("SETUP: build input mode");
    }
    fs::set_permissions(
        scenario.source.join("src/lib.rs"),
        fs::Permissions::from_mode(0o664),
    )
    .expect("SETUP: build input mode");
    // Not build inputs: the allowlist admits none of these paths.
    symlink("private-notes.txt", scenario.source.join("notes-link")).expect("SETUP: link");
    symlink("guide.md", scenario.source.join("docs/linked.md")).expect("SETUP: link");
    symlink("../LICENSE", scenario.source.join("policies/license.txt")).expect("SETUP: link");
    let original = tree_manifest(&scenario.source);

    let exit = scenario.run("build", &[], &Spawn::default());
    assert!(
        exit.status.success(),
        "FAIL: agentsandbox build did not succeed: {}",
        exit.describe()
    );
    let reported: Vec<&str> = exit
        .stderr
        .lines()
        .filter(|line| line.starts_with("Prepared public distribution (build): "))
        .collect();
    assert_eq!(reported.len(), 1, "FAIL: provenance lines: {reported:?}");
    assert!(
        reported[0]
            .split_whitespace()
            .any(|field| field == "shared_writable=1"),
        "FAIL: the provenance line does not count the input writable by group: {}",
        reported[0]
    );
    let builds = scenario.state.consumers("build");
    assert_eq!(
        builds.len(),
        2,
        "FAIL: expected exactly two image builds; calls: {}",
        scenario.state.calls()
    );
    let consumed = builds[0].context_files();
    assert_eq!(
        consumed.keys().map(String::as_str).collect::<Vec<_>>(),
        {
            let mut inputs = build_inputs();
            inputs.sort_unstable();
            inputs
        },
        "FAIL: the build context is not exactly the build inputs"
    );
    assert!(scenario.temporary_entries().is_empty());
    assert!(
        tree_manifest(&scenario.source) == original,
        "FAIL: the original source tree changed"
    );
}

/// The fixture altered the prepared copy; `SETUP:` otherwise.
fn altered(scenario: &Operator) -> String {
    scenario.state.read("altered").unwrap_or_else(|| {
        panic!(
            "SETUP: the fixture did not alter the prepared copy; calls: {}",
            scenario.state.calls()
        )
    })
}

#[test]
fn r8a_a_copy_altered_before_its_build_is_refused_before_that_build_starts() {
    let scenario = Operator::prepare();
    let temporary = scenario.tmp.to_string_lossy().into_owned();
    let directive = format!("info:{temporary}");
    scenario
        .state
        .plan(&[("alter-prepared", directive.as_str())]);
    let original = tree_manifest(&scenario.source);

    let exit = scenario.run("build", &[], &Spawn::default());
    let target = altered(&scenario);
    refused(
        &scenario,
        &exit,
        &original,
        &[
            "the prepared copy changed before docker build started",
            "prepared copy content changed: Cargo.toml",
            "docker build was not started",
        ],
    );
    assert!(
        scenario.state.effects("build") == 0 && scenario.state.consumers("build").is_empty(),
        "FAIL: a build was started on an altered copy ({target}); calls: {}",
        scenario.state.calls()
    );
}

#[test]
fn r8b_a_copy_altered_during_its_build_fails_the_command_after_that_build() {
    let scenario = Operator::prepare();
    let temporary = scenario.tmp.to_string_lossy().into_owned();
    let directive = format!("build:{temporary}");
    scenario
        .state
        .plan(&[("alter-prepared", directive.as_str())]);
    let original = tree_manifest(&scenario.source);

    let exit = scenario.run("build", &[], &Spawn::default());
    let target = altered(&scenario);
    refused(
        &scenario,
        &exit,
        &original,
        &[
            "the prepared copy changed while docker build was running",
            "prepared copy content changed: Cargo.toml",
            "exit status: 0",
            "is not bound to the reported digest",
        ],
    );
    assert_eq!(
        scenario.state.effects("build"),
        1,
        "FAIL: expected exactly the build during which {target} was altered; calls: {}",
        scenario.state.calls()
    );
}

#[test]
fn r4b_a_prepared_source_that_exists_when_a_signal_arrives_between_engine_commands_is_removed() {
    let scenario = Operator::prepare();
    scenario
        .state
        .plan(&[("hold", "first-call"), ("deadline-seconds", "120")]);
    let original = tree_manifest(&scenario.source);
    let mut running = scenario.spawn("build", &[], &Spawn::default());

    // The first engine call is a probe, and it holds: no build command runs.
    if scenario.state.wait_ready("leader", READY_WINDOW).is_none() {
        let product = running.product();
        panic!(
            "{}",
            missed_ready(
                &scenario.state,
                "leader",
                "context",
                running.spawned_at,
                READY_WINDOW,
                &product
            )
        );
    }
    if let Some(early) = running.poll() {
        panic!(
            "SETUP: the command ended before the signal was sent: {}",
            early.describe()
        );
    }
    // The scenario: a complete prepared source exists before the signal.
    let mut prepared = scenario.prepared_directories();
    assert_eq!(
        prepared.len(),
        1,
        "SETUP: expected exactly one prepared source while the first engine call holds: {:?}",
        scenario.temporary_entries()
    );
    let prepared = prepared.remove(0);
    assert!(
        prepared.join("manifest.json").is_file() && prepared.join("context/Cargo.toml").is_file(),
        "SETUP: the prepared source is not complete while the first engine call holds: {}",
        prepared.display()
    );

    running.signal(libc::SIGTERM);
    let exit = await_exit(&scenario, &mut running, "SIGTERM");
    report(&exit);
    assert_eq!(
        exit.status.code(),
        Some(1),
        "FAIL: unexpected status after an interruption: {}",
        exit.describe()
    );
    assert!(
        exit.stderr.lines().any(|line| line
            == "Error: interrupted by SIGTERM: no engine build or Compose command was running"),
        "FAIL: the interruption result is not explicit; stderr tail:\n{}",
        tail(&exit.stderr, 12)
    );
    assert!(
        !prepared.exists(),
        "FAIL: the prepared source that existed when the signal arrived was not removed: {}",
        prepared.display()
    );
    let left = scenario.temporary_entries();
    assert!(
        left.is_empty(),
        "FAIL: the private temporary directory is not empty: {left:?}"
    );
    assert!(
        scenario.state.effects("build") == 0 && scenario.state.consumers("build").is_empty(),
        "FAIL: a build was started after the interruption; calls: {}",
        scenario.state.calls()
    );
    assert!(
        tree_manifest(&scenario.source) == original,
        "FAIL: the original source tree changed"
    );
}

#[test]
fn r7j_a_build_source_whose_dockerignore_differs_from_the_compiled_list_is_refused() {
    let shipped = fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join(".dockerignore"))
        .expect("SETUP: repository build allowlist");
    // One rule more than the shipped file, and the file it admits.
    let mut rules = shipped.clone();
    rules.extend_from_slice(b"!extra/\n");
    let message = format!(
        "Error: the build input list of this CLI mirrors a .dockerignore whose SHA-256 is {}; \
         the .dockerignore of the selected distribution has SHA-256 {}. Build with the CLI built \
         from that distribution, or run the engine build directly",
        hex::encode(Sha256::digest(&shipped)),
        hex::encode(Sha256::digest(&rules))
    );
    build_refused(
        |scenario| {
            fs::write(scenario.source.join(".dockerignore"), &rules)
                .expect("SETUP: differing build allowlist");
            fs::create_dir(scenario.source.join("extra")).expect("SETUP: admitted directory");
            fs::write(scenario.source.join("extra/ca.pem"), "admitted here only\n")
                .expect("SETUP: admitted file");
        },
        &[&message],
    );
}

#[test]
fn r7k_a_build_source_with_a_per_dockerfile_ignore_file_is_refused_by_name() {
    build_refused(
        |scenario| {
            fs::write(
                scenario
                    .source
                    .join("images/control.Dockerfile.dockerignore"),
                "**\n",
            )
            .expect("SETUP: per-Dockerfile ignore file");
        },
        &[
            "Error: the build input list of this CLI mirrors .dockerignore only: the \
             per-Dockerfile ignore file images/control.Dockerfile.dockerignore of the selected \
             distribution is not reconciled with it, and a builder may apply that file instead. \
             Remove it, or run the engine build directly",
        ],
    );
}

#[test]
fn r7l_an_empty_or_relative_temporary_directory_is_refused_and_nothing_is_created() {
    for value in ["", "relative-tmp"] {
        let scenario = Operator::prepare();
        let original = tree_manifest(&scenario.source);
        // The working directory is the source, as in the documented commands:
        // that is where such a value would have placed the copy.
        let exit = build_started_with(
            &scenario,
            &Start {
                tmpdir: Some(OsStr::new(value)),
                directory: Some(&scenario.source),
                ..Start::default()
            },
        );
        let message = format!(
            "Error: the temporary directory must be an absolute path, but TMPDIR is \"{value}\"; \
             set TMPDIR to an absolute directory outside the selected source"
        );
        refused(&scenario, &exit, &original, &[&message]);
        no_engine_call(&scenario);
    }
}

#[test]
fn r7m_a_temporary_directory_inside_the_selected_source_is_refused_and_nothing_is_created() {
    // A directory the capture does not enter, and one it does enter.
    for inside in [".tmp", "src/.tmp"] {
        let mut scenario = Operator::prepare();
        scenario.tmp = scenario.source.join(inside);
        fs::create_dir(&scenario.tmp).expect("SETUP: temporary directory inside the source");
        fs::set_permissions(&scenario.tmp, fs::Permissions::from_mode(0o700))
            .expect("SETUP: temporary directory mode");
        let original = tree_manifest(&scenario.source);
        let exit = scenario.run("build", &[], &Spawn::default());
        let message = format!(
            "Error: the temporary directory {} (TMPDIR) is the selected source {} or lies \
             inside it; set TMPDIR to a directory outside the source",
            scenario.tmp.display(),
            scenario.source.display()
        );
        refused(&scenario, &exit, &original, &[&message]);
        no_engine_call(&scenario);
    }
}

#[test]
fn r8c_a_copy_whose_path_was_given_to_another_directory_is_reported_and_nothing_is_removed() {
    const PLACED: &[u8] = b"placed by the test\n";
    let scenario = Operator::prepare();
    scenario
        .state
        .plan(&[("hold", "consume"), ("deadline-seconds", "120")]);
    let mut running = scenario.spawn("build", &[], &Spawn::default());
    await_holding_build(&scenario, &mut running);
    let prepared = single_prepared(&scenario);

    // While the build holds, the copy is moved out of the temporary directory
    // and another private directory takes its path.
    let moved = scenario.source.with_file_name("moved-copy");
    fs::rename(&prepared, &moved).expect("SETUP: moved prepared source");
    fs::create_dir(&prepared).expect("SETUP: directory at the path of the copy");
    fs::set_permissions(&prepared, fs::Permissions::from_mode(0o700))
        .expect("SETUP: mode of the directory at the path of the copy");
    let sentinel = prepared.join("not-the-copy.txt");
    fs::write(&sentinel, PLACED).expect("SETUP: sentinel");
    let captured = snapshot(&moved).expect("SETUP: the moved copy can be read");

    scenario.state.release();
    let exit = await_exit(&scenario, &mut running, "the consumer was released");
    report(&exit);

    assert_eq!(
        exit.status.code(),
        Some(1),
        "FAIL: unexpected status: {}",
        exit.describe()
    );
    // The change is found when the build ended, before its result is used.
    for text in [
        "the prepared copy changed while docker build was running",
        "control directory path identity changed",
        "is not bound to the reported digest",
    ] {
        assert!(
            exit.stderr.contains(text),
            "FAIL: the message does not say {text:?}; stderr tail:\n{}",
            tail(&exit.stderr, 12)
        );
    }
    // Nothing is removed through a path that names something else.
    assert_eq!(
        fs::read(&sentinel).ok().as_deref(),
        Some(PLACED),
        "FAIL: the directory that took the path of the prepared copy was removed"
    );
    let warning = format!(
        "warning: prepared source at {} was not removed: ",
        prepared.display()
    );
    assert!(
        exit.stderr.lines().any(|line| line.starts_with(&warning)),
        "FAIL: the copy that was not removed is not reported; stderr tail:\n{}",
        tail(&exit.stderr, 12)
    );
    assert_eq!(
        scenario.prepared_directories(),
        vec![prepared],
        "FAIL: the directory at the path of the copy is not the only thing left"
    );
    assert_eq!(scenario.temporary_entries().len(), 1);
    // The copy itself is whole where it was moved.
    match snapshot(&moved) {
        Ok(now) if now == captured => {}
        Ok(_) => panic!("FAIL: the moved prepared copy changed"),
        Err(error) => panic!("FAIL: the moved prepared copy is unavailable: {error}"),
    }
    assert_eq!(
        scenario.state.effects("build"),
        1,
        "FAIL: expected exactly the build during which the copy was replaced; calls: {}",
        scenario.state.calls()
    );
    assert!(
        scenario.state.ended("leader", END_WINDOW),
        "FAIL: the command returned but its build consumer was not proven ended; {}",
        scenario.state.describe()
    );
}

#[test]
fn r7n_a_source_with_more_selected_directories_than_the_descriptor_limit_is_built() {
    const DIRECTORIES: usize = 300;
    const DESCRIPTORS: u64 = 128;
    let scenario = Operator::prepare();
    let mut expected: BTreeMap<String, Vec<u8>> = build_inputs()
        .into_iter()
        .map(|relative| (relative.to_owned(), scenario.source_file(relative)))
        .collect();
    for number in 0..DIRECTORIES {
        let relative = format!("src/wide/d{number:03}/unit.rs");
        let content = format!("// synthetic build input {number}\n");
        let path = scenario.source.join(&relative);
        fs::create_dir_all(path.parent().expect("relative file has a parent"))
            .expect("SETUP: selected source directory");
        fs::write(&path, &content).expect("SETUP: selected source file");
        expected.insert(relative, content.into_bytes());
    }
    let original = tree_manifest(&scenario.source);

    let exit = build_started_with(
        &scenario,
        &Start {
            tmpdir: Some(scenario.tmp.as_os_str()),
            descriptors: Some(DESCRIPTORS),
            ..Start::default()
        },
    );
    assert!(
        exit.status.success(),
        "FAIL: agentsandbox build did not succeed on {DIRECTORIES} selected directories with a \
         limit of {DESCRIPTORS} descriptors: {}",
        exit.describe()
    );
    let builds = scenario.state.consumers("build");
    assert_eq!(
        builds.len(),
        2,
        "FAIL: expected exactly two image builds; calls: {}",
        scenario.state.calls()
    );
    for build in &builds {
        assert!(
            build.context_files() == expected,
            "FAIL: build #{} context differs from the build inputs",
            build.number
        );
    }
    let left = scenario.temporary_entries();
    assert!(
        left.is_empty(),
        "FAIL: the private temporary directory is not empty: {left:?}"
    );
    assert!(
        tree_manifest(&scenario.source) == original,
        "FAIL: the original source tree changed"
    );
}

#[test]
fn r7o_a_source_too_deep_for_the_descriptor_limit_is_refused_and_the_limit_is_named() {
    // Each level of the walk and each level of the copy holds one descriptor:
    // this depth needs more of them than this limit allows.
    const DEPTH: usize = 40;
    const DESCRIPTORS: u64 = 64;
    let scenario = Operator::prepare();
    let directory = scenario.source.join(format!("src/{}", "d/".repeat(DEPTH)));
    fs::create_dir_all(&directory).expect("SETUP: deep source directory");
    fs::write(directory.join("deep.rs"), "deep\n").expect("SETUP: deep source file");
    let original = tree_manifest(&scenario.source);

    let exit = build_started_with(
        &scenario,
        &Start {
            tmpdir: Some(scenario.tmp.as_os_str()),
            descriptors: Some(DESCRIPTORS),
            ..Start::default()
        },
    );
    assert!(
        exit.stderr.contains("os error"),
        "SETUP: the command met no operating system error under a limit of {DESCRIPTORS} \
         descriptors: {}",
        exit.describe()
    );
    refused(
        &scenario,
        &exit,
        &original,
        &[
            "Error: the open-file limit of this process or of the system was reached while the \
             source was being prepared; raise the descriptor limit (`ulimit -n`) and run the \
             command again",
        ],
    );
    no_engine_call(&scenario);
}

#[test]
fn r7p_a_cli_started_with_sigchld_ignored_is_refused_before_anything_is_prepared_or_started() {
    const REFUSAL: &str = "Error: SIGCHLD is ignored in this process, inherited from the program \
                           that started this CLI: the wait status of an engine CLI process \
                           could not be obtained, so the end of an engine command could not be \
                           established. Start this CLI with the default SIGCHLD disposition. \
                           Nothing was prepared and no engine command was started";
    const PODMAN_COMPOSE: &str = "deploy/podman/compose.yaml";
    let ignored = [libc::SIGCHLD];
    let refused_before_anything = |scenario: &Operator, action: &str, further: &[&str]| {
        let original = tree_manifest(&scenario.source);
        let exit = started_with(
            scenario,
            action,
            further,
            &Start {
                tmpdir: Some(scenario.tmp.as_os_str()),
                ignored: &ignored,
                ..Start::default()
            },
        );
        // POSIX leaves it to the platform whether an ignored SIGCHLD stays
        // ignored through exec. Where it does not, the command runs as usual
        // and this scenario was not reached.
        if exit.status.success() {
            panic!(
                "SETUP: agentsandbox {action} ran to its end: an ignored SIGCHLD did not reach \
                 the control process through exec on this platform; {}",
                exit.describe()
            );
        }
        refused(scenario, &exit, &original, &[REFUSAL]);
        assert!(
            exit.stderr.lines().any(|line| line == REFUSAL),
            "FAIL: the refusal of {action} is not that single line; stderr tail:\n{}",
            tail(&exit.stderr, 12)
        );
        no_engine_call(scenario);
    };

    // `down` with a Podman-kind engine: the Compose command is the first
    // process this command starts, and its wait status is the one that could
    // not be obtained.
    let mut scenario = Operator::prepare();
    let control = scenario
        .program
        .parent()
        .expect("the engine program has a directory")
        .to_path_buf();
    scenario.program = install_engine(&control, "podman");
    scenario.state = FixtureState::for_program(&scenario.program);
    let compose = scenario.source.join(PODMAN_COMPOSE);
    fs::create_dir_all(compose.parent().expect("relative file has a parent"))
        .expect("SETUP: Podman Compose directory");
    fs::write(&compose, "services: {}\n").expect("SETUP: Podman Compose file");
    scenario.initialize_config();
    let config = scenario.config.to_string_lossy().into_owned();
    refused_before_anything(
        &scenario,
        "down",
        &["--runtime", "podman", "--config-dir", &config],
    );

    // `build`: nothing is copied, and the engine is not asked about itself.
    let scenario = Operator::prepare();
    refused_before_anything(&scenario, "build", &[]);

    // `up`: refused before the configuration directory is created or refreshed.
    let scenario = Operator::prepare();
    let config = scenario.config.to_string_lossy().into_owned();
    refused_before_anything(&scenario, "up", &["--config-dir", &config]);
    assert!(
        !scenario.config.exists(),
        "FAIL: a refused up created its configuration directory: {}",
        scenario.config.display()
    );
}
