//! R9–R18: paths of `agentsandbox build`, `up` and `down` that R1–R8 do not
//! reach: a successful `up`, a Podman-kind engine, a signal that lands while
//! the source is being prepared, the total bound of a prepared copy, an unknown
//! termination whose marker cannot be written, a copy that cannot be removed,
//! a Compose command whose output is held past the product's own bound or
//! until an operator signal, an engine command that fails by itself, the
//! default temporary directory, and a build log beyond its bound.
//!
//! The oracles are those of R1–R8: what the fixture engine really received and
//! read, a probe answered by the fixture process itself, the liveness lock the
//! kernel releases, and what is really left in the test's private temporary
//! directory. Signals go to the control process only, and every duration of
//! the product is its own, in real time.
use crate::{
    harness::{
        COMPOSE_FILE, Exit, FixtureState, Liveness, Operator, PREPARED_PREFIX, Spawn, build_inputs,
        distribution_digest, install_engine, missed_ready, provenance_lines, tail, tree_manifest,
    },
    regressions::{
        Start, await_exit, await_holding_build, await_ready, build_started_with, no_engine_call,
        no_material_marker, report, single_prepared,
    },
};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Test-side observation windows. They are not product deadlines.
const READY_WINDOW: Duration = Duration::from_secs(60);
const EXIT_WINDOW: Duration = Duration::from_secs(60);
const PROBE_WINDOW: Duration = Duration::from_secs(3);
const END_WINDOW: Duration = Duration::from_secs(10);
const TICK: Duration = Duration::from_millis(1);

/// The product's documented durations, in real time: the grace a forwarded
/// signal is given, the time collection is given after SIGKILL, and the bound
/// of one Compose command.
const INTERRUPT_GRACE: Duration = Duration::from_secs(10);
const STOP_OBSERVATION: Duration = Duration::from_secs(2);
const COMPOSE_BOUND: Duration = Duration::from_secs(180);

const RETAINED_ANCHOR: &str = "docs/setup/local/README.md#prepared-source-retained";
const UNKNOWN_MARKER: &str = "RETAINED-UNKNOWN.json";

fn succeed(action: &str, exit: &Exit) {
    assert!(
        exit.status.success(),
        "FAIL: agentsandbox {action} did not succeed: {}",
        exit.describe()
    );
}

/// An absolute path below the test's private temporary directory, so neither
/// the live source nor anything beside it.
fn prepared_path(scenario: &Operator, path: &str) -> bool {
    Path::new(path).is_absolute()
        && Path::new(path).starts_with(&scenario.tmp)
        && !Path::new(path).starts_with(&scenario.source)
}

/// The default loopback ports `agentsandbox up` asks for readiness: the gateway
/// and the dashboard.
const READINESS_PORTS: [u16; 2] = [8787, 4000];

/// The viewer identity request `up` waits for on both origins.
const READINESS_REQUEST: &str = "GET /api/v1/identity HTTP/1.1";

/// One readiness request: where it arrived, its request line, and what the
/// test's private temporary directory held at that moment.
struct Asked {
    port: u16,
    request: String,
    /// Whether the request carried the viewer key of the control configuration.
    authorized: bool,
    temporary: Vec<String>,
}

/// Answers the viewer identity request on the readiness ports for the time of one test: `200`
/// with a viewer identity when the request carries the key stored in the control configuration
/// (read when the request arrives, because `up` may create it), `401` otherwise. It stands for
/// the gateway and the dashboard a real Compose provider would have started; it is neither.
struct Readiness {
    ports: [u16; 2],
    stop: Arc<AtomicBool>,
    asked: Arc<Mutex<Vec<Asked>>>,
    servers: Vec<JoinHandle<()>>,
}

impl Readiness {
    fn listen(ports: [u16; 2], temporary: &Path, viewer_file: &Path) -> Self {
        let listeners = ports.map(|port| {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap_or_else(|error| {
                panic!("SETUP: cannot bind readiness port {port}: {error}")
            });
            listener
                .set_nonblocking(true)
                .expect("SETUP: readiness listener mode");
            listener
        });
        let ports = listeners
            .each_ref()
            .map(|listener| listener.local_addr().unwrap().port());
        let stop = Arc::new(AtomicBool::new(false));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let mut servers = Vec::new();
        for (listener, port) in listeners.into_iter().zip(ports) {
            let (stop, asked, temporary, viewer_file) = (
                Arc::clone(&stop),
                Arc::clone(&asked),
                temporary.to_owned(),
                viewer_file.to_owned(),
            );
            servers.push(std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => answer(stream, port, &temporary, &viewer_file, &asked),
                        Err(_) => std::thread::sleep(TICK),
                    }
                }
            }));
        }
        Self {
            ports,
            stop,
            asked,
            servers,
        }
    }

    /// Stops answering and returns every request that arrived.
    fn finish(mut self) -> Vec<Asked> {
        self.close();
        std::mem::take(&mut *self.asked.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn close(&mut self) {
        self.stop.store(true, Ordering::Release);
        for server in self.servers.drain(..) {
            let _ = server.join();
        }
    }
}

impl Drop for Readiness {
    fn drop(&mut self) {
        self.close();
    }
}

fn answer(
    mut stream: TcpStream,
    port: u16,
    temporary: &Path,
    viewer_file: &Path,
    asked: &Mutex<Vec<Asked>>,
) {
    // An accepted socket may inherit the listener's non-blocking mode.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") && request.len() < 16 * 1024 {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => request.extend_from_slice(&buffer[..count]),
        }
    }
    let mut names: Vec<String> = fs::read_dir(temporary)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let text = String::from_utf8_lossy(&request).into_owned();
    let request = text.lines().next().unwrap_or_default();
    let viewer = fs::read_to_string(viewer_file).unwrap_or_default();
    let authorized = !viewer.trim().is_empty()
        && text.lines().any(|line| {
            let prefix = "authorization: bearer ";
            line.to_ascii_lowercase().starts_with(prefix)
                && line[prefix.len()..].trim() == viewer.trim()
        });
    asked
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(Asked {
            port,
            request: request.to_owned(),
            authorized,
            temporary: names,
        });
    let body = "{\"tenant_id\":\"local\",\"role\":\"viewer\"}";
    let reply = if request != READINESS_REQUEST {
        "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned()
    } else if authorized {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    } else {
        "HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned()
    };
    let _ = stream.write_all(reply.as_bytes());
}

#[test]
fn r9_up_hands_the_prepared_compose_file_to_the_engine_and_ends_when_the_stack_answers() {
    let scenario = Operator::prepare();
    let original = tree_manifest(&scenario.source);
    let compose_bytes = scenario.source_file(COMPOSE_FILE);
    let readiness = Readiness::listen(
        READINESS_PORTS,
        &scenario.tmp,
        &scenario.config.join("secrets/viewer"),
    );

    let config = scenario.config.to_string_lossy().into_owned();
    let started = scenario.run("up", &["--config-dir", &config], &Spawn::default());
    let asked = readiness.finish();
    succeed("up", &started);
    for line in [
        "Stack: agentsandbox",
        "Gateway: http://127.0.0.1:8787",
        "Dashboard: http://127.0.0.1:4000",
    ] {
        assert!(
            started.stdout.contains(line),
            "FAIL: up did not report {line:?}; stdout: {}",
            started.stdout
        );
    }

    let mut commands = scenario.state.consumers("compose");
    assert_eq!(
        commands.len(),
        1,
        "FAIL: expected exactly one Compose command; calls: {}",
        scenario.state.calls()
    );
    let compose = commands.remove(0);
    let environment = compose.value("--env-file").unwrap_or_default();
    let file = compose.value("--file").unwrap_or_default();
    let wanted = [
        "compose",
        "--project-name",
        "agentsandbox",
        "--env-file",
        environment,
        "--file",
        file,
        "up",
        "--detach",
        "--force-recreate",
    ];
    assert_eq!(
        compose.engine_arguments(),
        &wanted[..],
        "FAIL: unexpected Compose arguments"
    );
    assert!(
        prepared_path(&scenario, environment) && prepared_path(&scenario, file),
        "FAIL: the Compose command was not given prepared paths: {environment} and {file}"
    );
    assert_eq!(
        compose.observed("env-file").as_deref(),
        Some(&b""[..]),
        "FAIL: the Compose env file is not empty"
    );
    assert_eq!(
        compose.observed("compose-file").as_ref(),
        Some(&compose_bytes),
        "FAIL: the Compose consumer did not read the original file byte for byte"
    );
    let reported = provenance_lines(&started.stderr);
    assert_eq!(
        reported.len(),
        1,
        "FAIL: up printed {} provenance lines; stderr tail:\n{}",
        reported.len(),
        tail(&started.stderr, 12)
    );
    assert_eq!(reported[0].scope, "compose");
    assert_eq!(
        reported[0].digest,
        distribution_digest(&BTreeMap::from([(COMPOSE_FILE.to_owned(), compose_bytes)])),
        "FAIL: the Compose provenance does not describe the consumed file"
    );
    assert_eq!(
        (
            scenario.state.effects("compose"),
            scenario.state.effects("build")
        ),
        (1, 0),
        "FAIL: unexpected engine effects; calls: {}",
        scenario.state.calls()
    );

    // The usual command carries exactly the environment it always had: the control
    // directory alone, and no publication port.
    assert_eq!(
        compose.compose_environment(),
        vec![format!("AGENTSANDBOX_CONFIG_DIR={config}")],
        "FAIL: unexpected Compose environment"
    );

    // Readiness was really asked of both endpoints with the viewer key, and only once the
    // Compose command was collected: its prepared copy was already gone.
    for port in READINESS_PORTS {
        assert!(
            asked.iter().any(|asked| asked.port == port
                && asked.request == READINESS_REQUEST
                && asked.authorized),
            "FAIL: up succeeded without asking 127.0.0.1:{port} for viewer access"
        );
    }
    for asked in &asked {
        assert!(
            asked.temporary.is_empty(),
            "FAIL: a prepared source outlived its Compose command: {:?} when {} was asked {:?}",
            asked.temporary,
            asked.port,
            asked.request
        );
    }
    let left = scenario.temporary_entries();
    assert!(
        left.is_empty(),
        "FAIL: the private temporary directory is not empty after up: {left:?}"
    );
    assert!(
        tree_manifest(&scenario.source) == original,
        "FAIL: the original source tree changed"
    );
}

#[test]
fn r10_a_podman_kind_engine_reads_the_prepared_source_without_an_engine_global_option() {
    const PODMAN_COMPOSE: &str = "deploy/podman/compose.yaml";
    let mut scenario = Operator::prepare();
    let control = scenario
        .program
        .parent()
        .expect("the engine program has a directory")
        .to_path_buf();
    scenario.program = install_engine(&control, "podman");
    scenario.state = FixtureState::for_program(&scenario.program);
    let compose_bytes = b"services: {}\n# podman kind\n".to_vec();
    let compose_path = scenario.source.join(PODMAN_COMPOSE);
    fs::create_dir_all(compose_path.parent().expect("relative file has a parent"))
        .expect("SETUP: Podman Compose directory");
    fs::write(&compose_path, &compose_bytes).expect("SETUP: Podman Compose file");
    scenario.initialize_config();
    let original = tree_manifest(&scenario.source);
    let expected: BTreeMap<String, Vec<u8>> = build_inputs()
        .into_iter()
        .map(|relative| (relative.to_owned(), scenario.source_file(relative)))
        .collect();

    let built = scenario.run("build", &["--runtime", "podman"], &Spawn::default());
    succeed("build", &built);
    let left_after_build = scenario.temporary_entries();
    let config = scenario.config.to_string_lossy().into_owned();
    let stopped = scenario.run(
        "down",
        &["--runtime", "podman", "--config-dir", &config],
        &Spawn::default(),
    );
    succeed("down", &stopped);

    // The engine was asked for its information in the Podman form.
    assert!(
        scenario
            .state
            .calls()
            .lines()
            .any(|line| line == r#"["info","--format","json"]"#),
        "SETUP: the fixture was not driven as a Podman-kind engine; calls: {}",
        scenario.state.calls()
    );
    assert!(
        built.stderr.contains("with podman"),
        "FAIL: the build was not announced for podman; stderr tail:\n{}",
        tail(&built.stderr, 12)
    );

    let builds = scenario.state.consumers("build");
    assert_eq!(
        builds.len(),
        2,
        "FAIL: expected exactly two image builds; calls: {}",
        scenario.state.calls()
    );
    for (build, dockerfile) in builds
        .iter()
        .zip(["images/control.Dockerfile", "images/worker.Dockerfile"])
    {
        // Nothing precedes the verb: a Podman command carries no `--host`.
        assert_eq!(
            build.arguments.first().map(String::as_str),
            Some("build"),
            "FAIL: build #{} starts with an engine-global option: {:?}",
            build.number,
            build.arguments
        );
        let context = build
            .arguments
            .last()
            .expect("a recorded build has arguments");
        assert!(
            prepared_path(&scenario, context),
            "FAIL: the build context is not a prepared path: {context}"
        );
        assert_eq!(
            build.value("--file"),
            Some(format!("{context}/{dockerfile}").as_str()),
            "FAIL: build #{} was not given its prepared Dockerfile",
            build.number
        );
        assert_eq!(
            build.context_files(),
            expected,
            "FAIL: build #{} context differs from the build inputs",
            build.number
        );
    }
    let reported = provenance_lines(&built.stderr);
    assert_eq!(reported.len(), 1, "FAIL: build provenance lines");
    assert_eq!(reported[0].digest, distribution_digest(&expected));

    let mut commands = scenario.state.consumers("compose");
    assert_eq!(
        commands.len(),
        1,
        "FAIL: expected exactly one Compose command; calls: {}",
        scenario.state.calls()
    );
    let compose = commands.remove(0);
    let environment = compose.value("--env-file").unwrap_or_default();
    let file = compose.value("--file").unwrap_or_default();
    let wanted = [
        "compose",
        "--project-name",
        "agentsandbox",
        "--env-file",
        environment,
        "--file",
        file,
        "down",
    ];
    assert_eq!(
        compose.arguments, wanted,
        "FAIL: unexpected arguments of the Podman Compose command"
    );
    assert!(
        prepared_path(&scenario, environment)
            && prepared_path(&scenario, file)
            && file.ends_with(&format!("/context/{PODMAN_COMPOSE}")),
        "FAIL: the Compose command was not given the prepared Podman file: {environment} and \
         {file}"
    );
    assert_eq!(compose.observed("env-file").as_deref(), Some(&b""[..]));
    assert_eq!(
        compose.observed("compose-file").as_ref(),
        Some(&compose_bytes),
        "FAIL: the Compose consumer did not read the Podman file byte for byte"
    );
    let reported = provenance_lines(&stopped.stderr);
    assert_eq!(reported.len(), 1, "FAIL: down provenance lines");
    assert_eq!(
        reported[0].digest,
        distribution_digest(&BTreeMap::from([(
            PODMAN_COMPOSE.to_owned(),
            compose_bytes
        )])),
        "FAIL: the Compose provenance does not describe the consumed file"
    );

    let left = scenario.temporary_entries();
    assert!(
        left_after_build.is_empty() && left.is_empty(),
        "FAIL: the private temporary directory is not empty after the commands: \
         {left_after_build:?} then {left:?}"
    );
    assert!(
        tree_manifest(&scenario.source) == original,
        "FAIL: the original source tree changed"
    );
}

#[test]
fn r11_operator_signal_while_the_source_is_prepared_stops_the_copy_before_any_engine_command() {
    // Enough build inputs for the copy to last far longer than a signal takes
    // to arrive; well below the bound of 16 384 entries.
    const CROWD: usize = 6_000;
    let scenario = Operator::prepare();
    let crowd = scenario.source.join("src/crowd");
    fs::create_dir(&crowd).expect("SETUP: crowded source directory");
    for number in 0..CROWD {
        fs::write(
            crowd.join(format!("unit{number}.rs")),
            format!("// synthetic build input {number}\n"),
        )
        .expect("SETUP: crowded source entry");
    }
    let original = tree_manifest(&scenario.source);
    let mut running = scenario.spawn("build", &[], &Spawn::default());

    // The private directory of the copy appears when the copy starts.
    let waiting = Instant::now();
    let prepared: PathBuf = loop {
        if let Some(directory) = scenario.prepared_directories().pop() {
            break directory;
        }
        if let Some(early) = running.poll() {
            panic!(
                "FAIL: the command ended before it prepared its source: {}",
                early.describe()
            );
        }
        assert!(
            waiting.elapsed() < READY_WINDOW,
            "SETUP: no prepared source appeared within {READY_WINDOW:?}"
        );
        std::thread::sleep(TICK);
    };
    // A directory that does not exist, yet or any more, holds nothing.
    let crowd_copied =
        || fs::read_dir(prepared.join("context/src/crowd")).map_or(0, Iterator::count);
    running.signal(libc::SIGINT);
    let sent = Instant::now();
    // Counted after the signal was sent, so at most what was copied by then.
    let copied = crowd_copied();
    // Sampled until the command ends: a copy that stopped adds next to nothing
    // to what it held when the signal was sent.
    let mut peak = copied;
    let exit = loop {
        peak = peak.max(crowd_copied());
        if let Some(exit) = running.poll() {
            break exit;
        }
        assert!(
            sent.elapsed() < EXIT_WINDOW,
            "FAIL: the command is still pending {EXIT_WINDOW:?} after SIGINT; {}",
            scenario.state.describe()
        );
        std::thread::sleep(TICK);
    };
    eprintln!(
        "latency: spawn to prepared directory {:?}; SIGINT to the end of the command {:?}; \
         {copied} of {CROWD} crowd files copied when the signal was sent, {peak} at most until \
         the command ended",
        waiting.elapsed().saturating_sub(sent.elapsed()),
        sent.elapsed()
    );
    report(&exit);

    // The scenario needs most of the copy still to do when the signal is sent.
    assert!(
        copied < CROWD / 2,
        "SETUP: the signal came too late: {copied} of {CROWD} crowd files were copied when it \
         was sent: {}",
        exit.describe()
    );
    // A removal that follows a complete copy can hide its last entries from a
    // sample, but not the half of them.
    assert!(
        peak < CROWD / 2,
        "FAIL: the copy went on after SIGINT: {copied} of {CROWD} crowd files were copied when \
         it was sent and {peak} were seen afterwards: {}",
        exit.describe()
    );
    assert_eq!(
        exit.status.code(),
        Some(1),
        "FAIL: unexpected status after an interruption: {}",
        exit.describe()
    );
    assert!(
        exit.stderr.lines().any(|line| line
            == "Error: interrupted by SIGINT while the source was being prepared; no engine \
                build or Compose command was started"),
        "FAIL: the interruption result is not explicit; stderr tail:\n{}",
        tail(&exit.stderr, 12)
    );
    assert!(
        provenance_lines(&exit.stderr).is_empty(),
        "FAIL: a provenance line was printed for a copy that was never completed"
    );
    no_engine_call(&scenario);
    let left = scenario.temporary_entries();
    assert!(
        left.is_empty(),
        "FAIL: an interrupted preparation left its copy behind: {left:?}"
    );
    assert!(
        tree_manifest(&scenario.source) == original,
        "FAIL: the original source tree changed"
    );
}

#[test]
fn r12_build_inputs_beyond_the_total_bound_are_refused_and_their_partial_copy_is_removed() {
    // Nine inputs of 30 MiB: each is under the per-file bound of 32 MiB, and
    // together they pass the bound of 256 MiB of one prepared copy.
    const PART: u64 = 30 * 1024 * 1024;
    const PARTS: usize = 9;
    let scenario = Operator::prepare();
    let parts: Vec<PathBuf> = (0..PARTS)
        .map(|number| scenario.source.join(format!("src/bulk{number}.rs")))
        .collect();
    for part in &parts {
        // Sparse: the bounds are those of the length the descriptor reports.
        File::create(part)
            .and_then(|file| file.set_len(PART))
            .expect("SETUP: large build input");
    }
    let before: Vec<_> = parts
        .iter()
        .map(|part| {
            let facts = fs::metadata(part).expect("SETUP: large build input metadata");
            (facts.len(), facts.modified().ok())
        })
        .collect();

    let exit = scenario.run("build", &[], &Spawn::default());
    eprintln!("latency: refused after {:?}", exit.elapsed);
    report(&exit);
    assert_eq!(
        exit.status.code(),
        Some(1),
        "FAIL: unexpected status of a refused command: {}",
        exit.describe()
    );
    assert!(
        exit.stderr.contains(
            "exceed the total bound of 268435456 bytes of the prepared copy at: src/bulk"
        ),
        "FAIL: the refusal does not name the total bound and the input that passed it; stderr \
         tail:\n{}",
        tail(&exit.stderr, 12)
    );
    no_engine_call(&scenario);
    // More than 224 MiB had been copied when the last input was refused.
    let left = scenario.temporary_entries();
    assert!(
        left.is_empty(),
        "FAIL: a refused command left its partial copy behind: {left:?}"
    );
    let after: Vec<_> = parts
        .iter()
        .map(|part| {
            let facts = fs::metadata(part).expect("FAIL: a large build input disappeared");
            (facts.len(), facts.modified().ok())
        })
        .collect();
    assert_eq!(after, before, "FAIL: a large build input changed");
}

#[test]
fn r13_unknown_termination_whose_marker_cannot_be_written_still_keeps_the_copy_and_says_so() {
    const PLANTED: &[u8] = b"written by the test before the termination became unknown\n";
    let scenario = Operator::prepare();
    scenario.state.plan(&[
        ("hold", "consume"),
        ("descendant", "own-group"),
        ("deadline-seconds", "120"),
    ]);
    let mut running = scenario.spawn("build", &[], &Spawn::default());
    await_ready(&scenario, &mut running, "leader");
    await_ready(&scenario, &mut running, "descendant");
    assert!(
        scenario.state.probe("descendant", PROBE_WINDOW),
        "SETUP: the descendant did not answer a probe before the signal; {}",
        scenario.state.describe()
    );
    if let Some(early) = running.poll() {
        panic!(
            "FAIL: the command ended while its build consumers were holding: {}",
            early.describe()
        );
    }
    let prepared = single_prepared(&scenario);
    // The marker's name is taken: the product creates it exclusively and
    // never replaces existing data.
    let marker = prepared.join(UNKNOWN_MARKER);
    fs::write(&marker, PLANTED).expect("SETUP: occupied marker name");

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
        waited >= INTERRUPT_GRACE + STOP_OBSERVATION,
        "FAIL: unknown termination announced {waited:?} after SIGTERM, before the grace and \
         the observation of the product"
    );
    let named = prepared.to_string_lossy().into_owned();
    assert!(
        exit.stderr.contains("termination is unknown")
            && exit.stderr.contains(&named)
            && exit.stderr.contains("is not removed by ai-guard")
            && exit.stderr.contains(RETAINED_ANCHOR),
        "FAIL: the unknown termination is not reported with the retained copy; stderr tail:\n{}",
        tail(&exit.stderr, 12)
    );
    let warning = format!("warning: {UNKNOWN_MARKER} could not be written in {named}: ");
    assert!(
        exit.stderr.lines().any(|line| line.starts_with(&warning)),
        "FAIL: the marker that could not be written is not reported; stderr tail:\n{}",
        tail(&exit.stderr, 12)
    );
    assert_eq!(
        fs::read(&marker).ok().as_deref(),
        Some(PLANTED),
        "FAIL: existing data under the marker's name was replaced"
    );

    // Retention does not depend on the marker: the copy is there, alone, and
    // the process that holds the output still reads the same bytes.
    assert_eq!(
        scenario.prepared_directories(),
        vec![prepared.clone()],
        "FAIL: the retained prepared source is not the only one left"
    );
    for relative in build_inputs() {
        assert_eq!(
            fs::read(prepared.join("context").join(relative)).ok(),
            Some(scenario.source_file(relative)),
            "FAIL: {relative} of the retained copy is not what was prepared"
        );
    }
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

    // Nothing of this test outlives it.
    scenario.state.quit();
    assert!(
        scenario.state.ended("descendant", END_WINDOW),
        "SETUP: the process holding the output did not leave; {}",
        scenario.state.describe()
    );
}

/// Gives a directory back its private mode when the test ends, so the test's
/// own directory can be removed.
struct Unlocked(PathBuf);

impl Drop for Unlocked {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
    }
}

#[test]
fn r14_a_prepared_copy_that_cannot_be_removed_is_reported_and_left_in_place() {
    let scenario = Operator::prepare();
    scenario
        .state
        .plan(&[("hold", "consume"), ("deadline-seconds", "120")]);
    let mut running = scenario.spawn("build", &[], &Spawn::default());
    await_holding_build(&scenario, &mut running);
    let prepared = single_prepared(&scenario);

    // While the build reads the copy, one of its directories loses its write
    // permission: its entries can no longer be unlinked by this user.
    let locked = prepared.join("context/policies");
    let kept = locked.join("default.json");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o500))
        .expect("SETUP: prepared directory mode");
    let _unlocked = Unlocked(locked.clone());
    let probe = locked.join("probe");
    if File::create(&probe).is_ok() {
        let _ = fs::remove_file(&probe);
        panic!(
            "SETUP: this user writes into a directory without write permission (privileged \
             process): a removal failure cannot be produced here"
        );
    }

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
        "prepared copy directory changed: policies",
        "is not bound to the reported digest",
    ] {
        assert!(
            exit.stderr.contains(text),
            "FAIL: the message does not say {text:?}; stderr tail:\n{}",
            tail(&exit.stderr, 12)
        );
    }
    assert_eq!(
        scenario.state.effects("build"),
        1,
        "FAIL: expected exactly the build during which the copy was altered; calls: {}",
        scenario.state.calls()
    );
    // The removal failed, visibly: a warning names the directory, and what
    // could not be removed is still there.
    let warning = format!(
        "warning: prepared source at {} could not be removed: ",
        prepared.display()
    );
    assert!(
        exit.stderr.lines().any(|line| line.starts_with(&warning)),
        "FAIL: the failed removal of the prepared copy is not reported; stderr tail:\n{}",
        tail(&exit.stderr, 12)
    );
    assert!(
        kept.is_file(),
        "FAIL: an entry of a directory without write permission was removed: {}",
        kept.display()
    );
    assert_eq!(
        scenario.prepared_directories(),
        vec![prepared],
        "FAIL: the copy that could not be removed is not the only thing left"
    );
    assert!(
        scenario.state.ended("leader", END_WINDOW),
        "FAIL: the command returned but its build consumer was not proven ended; {}",
        scenario.state.describe()
    );
}

#[test]
#[ignore = "waits for the product's real 180 s Compose bound: run it by name with --ignored"]
fn r15_compose_command_whose_output_is_held_past_its_real_bound_is_killed_and_reported() {
    let scenario = Operator::prepare();
    scenario.initialize_config();
    scenario.state.plan(&[
        ("descendant", "same-group"),
        ("leader-exit", "0"),
        ("deadline-seconds", "300"),
    ]);
    let config = scenario.config.to_string_lossy().into_owned();
    let mut running = scenario.spawn("down", &["--config-dir", &config], &Spawn::default());

    if scenario
        .state
        .wait_ready("descendant", READY_WINDOW)
        .is_none()
    {
        let product = running.product();
        panic!(
            "{}",
            missed_ready(
                &scenario.state,
                "descendant",
                "compose",
                running.spawned_at,
                READY_WINDOW,
                &product
            )
        );
    }
    let held = Instant::now();
    assert!(
        scenario.state.ended("leader", END_WINDOW),
        "SETUP: the Compose CLI process did not exit by itself; {}",
        scenario.state.describe()
    );
    let prepared = single_prepared(&scenario);

    // The CLI process is gone and its output is held: the command is not
    // finished, and its prepared file stays for the whole bound.
    for seconds in [60, 120, 170] {
        std::thread::sleep(
            (held + Duration::from_secs(seconds)).saturating_duration_since(Instant::now()),
        );
        if let Some(early) = running.poll() {
            panic!(
                "FAIL: the command ended {:?} after its output was held, before its bound: {}",
                held.elapsed(),
                early.describe()
            );
        }
        assert!(
            scenario.state.probe("descendant", PROBE_WINDOW),
            "FAIL: the process holding the output does not answer; {}",
            scenario.state.describe()
        );
        no_material_marker(&scenario, "descendant");
        assert!(
            prepared.join("context").join(COMPOSE_FILE).is_file(),
            "FAIL: the prepared Compose file is gone while its command is pending"
        );
    }

    let exit = await_exit(&scenario, &mut running, "its output was held");
    eprintln!(
        "latency: the command ended {:?} after it was started and {:?} after its output was held",
        exit.elapsed,
        held.elapsed()
    );
    report(&exit);
    assert!(
        exit.elapsed >= COMPOSE_BOUND,
        "FAIL: the command ended after {:?}, before the bound of its Compose command",
        exit.elapsed
    );
    assert!(
        held.elapsed() < COMPOSE_BOUND + Duration::from_secs(20),
        "FAIL: the command was not ended at the bound of its Compose command: {:?}",
        held.elapsed()
    );
    assert_eq!(
        exit.status.code(),
        Some(1),
        "FAIL: unexpected status: {}",
        exit.describe()
    );
    for text in [
        "docker compose exceeded its 180 s bound",
        "SIGKILL was sent to the process group of the CLI process for docker compose",
        "exit status: 0",
        "The CLI process had already exited when SIGKILL was sent",
    ] {
        assert!(
            exit.stderr.contains(text),
            "FAIL: the message does not say {text:?}; stderr tail:\n{}",
            tail(&exit.stderr, 12)
        );
    }
    assert!(
        !exit.stderr.contains("termination is unknown"),
        "FAIL: unexpected unknown termination: {}",
        exit.describe()
    );
    assert!(
        scenario.state.ended("descendant", END_WINDOW),
        "FAIL: the command returned but the process holding its output was not proven ended; {}",
        scenario.state.describe()
    );
    no_material_marker(&scenario, "descendant");
    assert_eq!(scenario.state.effects("compose"), 1);
    let left = scenario.temporary_entries();
    assert!(
        left.is_empty(),
        "FAIL: the private temporary directory is not empty: {left:?}"
    );
}

#[test]
fn r16_an_engine_command_that_fails_by_itself_is_reported_with_its_status_and_leaves_nothing() {
    const FAILURE: &str = "Error: docker command failed (exit status: 7)";

    // The first image build reads its inputs, writes its output and exits 7.
    let scenario = Operator::prepare();
    scenario.state.plan(&[("exit", "7")]);
    let original = tree_manifest(&scenario.source);
    let exit = scenario.run("build", &[], &Spawn::default());
    report(&exit);
    assert_eq!(
        exit.status.code(),
        Some(1),
        "FAIL: unexpected status of a failed build: {}",
        exit.describe()
    );
    assert!(
        exit.stderr.lines().any(|line| line == FAILURE),
        "FAIL: the failed build is not reported with its status; stderr tail:\n{}",
        tail(&exit.stderr, 12)
    );
    assert_eq!(
        provenance_lines(&exit.stderr).len(),
        1,
        "FAIL: provenance lines of a build whose engine command failed"
    );
    let builds = scenario.state.consumers("build");
    assert_eq!(
        (builds.len(), scenario.state.effects("build")),
        (1, 1),
        "FAIL: expected exactly the build that failed; calls: {}",
        scenario.state.calls()
    );
    let context = builds[0].arguments.last().map_or("", String::as_str);
    assert!(
        prepared_path(&scenario, context),
        "FAIL: the build that failed was not given a prepared path: {context}"
    );
    assert!(
        scenario.state.ended("leader", END_WINDOW),
        "FAIL: the command returned but its build consumer was not proven ended; {}",
        scenario.state.describe()
    );
    let left = scenario.temporary_entries();
    assert!(
        left.is_empty(),
        "FAIL: a failed build left its prepared source behind: {left:?}"
    );
    assert!(
        tree_manifest(&scenario.source) == original,
        "FAIL: the original source tree changed"
    );

    // The same for the Compose command of `down`.
    let scenario = Operator::prepare();
    scenario.initialize_config();
    scenario.state.plan(&[("exit", "7")]);
    let config = scenario.config.to_string_lossy().into_owned();
    let exit = scenario.run("down", &["--config-dir", &config], &Spawn::default());
    report(&exit);
    assert_eq!(
        exit.status.code(),
        Some(1),
        "FAIL: unexpected status of a failed Compose command: {}",
        exit.describe()
    );
    assert!(
        exit.stderr.lines().any(|line| line == FAILURE),
        "FAIL: the failed Compose command is not reported with its status; stderr tail:\n{}",
        tail(&exit.stderr, 12)
    );
    let commands = scenario.state.consumers("compose");
    assert_eq!(
        (commands.len(), scenario.state.effects("compose")),
        (1, 1),
        "FAIL: expected exactly the Compose command that failed; calls: {}",
        scenario.state.calls()
    );
    let file = commands[0].value("--file").unwrap_or_default();
    assert!(
        prepared_path(&scenario, file),
        "FAIL: the Compose command that failed was not given a prepared path: {file}"
    );
    let left = scenario.temporary_entries();
    assert!(
        left.is_empty(),
        "FAIL: a failed Compose command left its prepared source behind: {left:?}"
    );
}

#[test]
fn r17_without_tmpdir_the_copy_is_made_in_the_default_temporary_directory_and_removed() {
    let scenario = Operator::prepare();
    let original = tree_manifest(&scenario.source);
    let expected: BTreeMap<String, Vec<u8>> = build_inputs()
        .into_iter()
        .map(|relative| (relative.to_owned(), scenario.source_file(relative)))
        .collect();

    // `TMPDIR` is not set for this command.
    let exit = build_started_with(&scenario, &Start::default());
    report(&exit);
    if exit
        .stderr
        .contains("cannot create the private prepared-source directory under")
    {
        panic!(
            "SETUP: the default temporary directory of this host is not accepted for a prepared \
             source: {}",
            exit.describe()
        );
    }
    succeed("build", &exit);

    let builds = scenario.state.consumers("build");
    assert_eq!(
        builds.len(),
        2,
        "FAIL: expected exactly two image builds; calls: {}",
        scenario.state.calls()
    );
    for build in &builds {
        // `<default temporary directory>/ai-guard-distribution-<random>/context`
        let context = Path::new(build.arguments.last().map_or("", String::as_str));
        let root = context.parent().unwrap_or(Path::new(""));
        assert!(
            context.is_absolute()
                && context.ends_with("context")
                && root
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(PREPARED_PREFIX)),
            "FAIL: build #{} was not given a prepared path: {}",
            build.number,
            context.display()
        );
        assert!(
            !context.starts_with(&scenario.tmp) && !context.starts_with(&scenario.source),
            "FAIL: build #{} did not read from the default temporary directory: {}",
            build.number,
            context.display()
        );
        #[cfg(target_os = "linux")]
        assert_eq!(
            root.parent(),
            Some(Path::new("/tmp")),
            "FAIL: without TMPDIR the copy of build #{} is not directly under /tmp",
            build.number
        );
        assert_eq!(
            build.context_files(),
            expected,
            "FAIL: build #{} context differs from the build inputs",
            build.number
        );
        assert!(
            !root.exists(),
            "FAIL: a prepared source was left in the default temporary directory: {}",
            root.display()
        );
    }
    eprintln!(
        "default temporary directory: {}",
        Path::new(builds[0].arguments.last().map_or("", String::as_str))
            .ancestors()
            .nth(2)
            .unwrap_or(Path::new(""))
            .display()
    );
    assert_eq!(provenance_lines(&exit.stderr).len(), 1);
    assert!(
        scenario.temporary_entries().is_empty(),
        "FAIL: the temporary directory of the other scenarios was used"
    );
    assert!(
        tree_manifest(&scenario.source) == original,
        "FAIL: the original source tree changed"
    );
}

#[test]
fn r15b_compose_command_whose_cli_exited_while_its_output_is_held_ends_on_the_forwarded_signal() {
    const INTERRUPTED: &str = "Error: interrupted by SIGTERM: SIGTERM was forwarded to the process \
                               group of the CLI process for docker compose; that process exited \
                               (exit status: 0) and both of its output pipes reached end of file";
    let scenario = Operator::prepare();
    scenario.initialize_config();
    scenario.state.plan(&[
        ("descendant", "same-group"),
        ("leader-exit", "0"),
        ("deadline-seconds", "120"),
    ]);
    let config = scenario.config.to_string_lossy().into_owned();
    let mut running = scenario.spawn("down", &["--config-dir", &config], &Spawn::default());

    if scenario
        .state
        .wait_ready("descendant", READY_WINDOW)
        .is_none()
    {
        let product = running.product();
        panic!(
            "{}",
            missed_ready(
                &scenario.state,
                "descendant",
                "compose",
                running.spawned_at,
                READY_WINDOW,
                &product
            )
        );
    }
    assert!(
        scenario.state.ended("leader", END_WINDOW),
        "SETUP: the Compose CLI process did not exit by itself; {}",
        scenario.state.describe()
    );
    let prepared = single_prepared(&scenario);

    // The CLI process is gone and a process of its group holds its output:
    // the command is not finished, and its prepared file stays.
    std::thread::sleep(Duration::from_secs(1));
    if let Some(early) = running.poll() {
        panic!(
            "FAIL: the command ended while a process of its Compose command holds its output: {}",
            early.describe()
        );
    }
    assert!(
        scenario.state.probe("descendant", PROBE_WINDOW),
        "SETUP: the process holding the output does not answer; {}",
        scenario.state.describe()
    );
    no_material_marker(&scenario, "descendant");
    assert!(
        prepared.join("context").join(COMPOSE_FILE).is_file(),
        "FAIL: the prepared Compose file is gone while its command is pending"
    );

    running.signal(libc::SIGTERM);
    let sent = Instant::now();
    let exit = await_exit(&scenario, &mut running, "SIGTERM");
    eprintln!(
        "latency: SIGTERM to the end of the command {:?}",
        sent.elapsed()
    );
    report(&exit);

    assert_eq!(
        exit.status.code(),
        Some(1),
        "FAIL: unexpected status: {}",
        exit.describe()
    );
    // The signal reached the group of the exited, unreaped CLI process: the
    // holder ended on it, and the command was collected with the status the
    // CLI process had really exited with. No SIGKILL was needed.
    assert!(
        exit.stderr.lines().any(|line| line == INTERRUPTED),
        "FAIL: the command was not collected on the forwarded signal; stderr tail:\n{}",
        tail(&exit.stderr, 12)
    );
    assert!(
        !exit.stderr.contains("termination is unknown") && !exit.stderr.contains("SIGKILL"),
        "FAIL: unexpected unknown termination or SIGKILL: {}",
        exit.describe()
    );
    for role in ["leader", "descendant"] {
        assert!(
            scenario.state.ended(role, END_WINDOW),
            "FAIL: the command returned but the {role} was not proven ended; {}",
            scenario.state.describe()
        );
    }
    // The copy outlived the process that was reading it.
    no_material_marker(&scenario, "descendant");
    assert_eq!(scenario.state.effects("compose"), 1);
    let left = scenario.temporary_entries();
    assert!(
        left.is_empty(),
        "FAIL: the private temporary directory is not empty: {left:?}"
    );
}

#[test]
fn r18_a_build_log_beyond_its_bound_is_printed_up_to_256_kib_and_the_cut_is_announced() {
    /// The documented bound of the standard output kept of one engine command.
    const BOUND: usize = 256 * 1024;
    /// The line the fixture engine repeats to fill a burst of standard output.
    const BURST_LINE: &[u8] = b"fixture-stdout-line\n";
    const NOTICE: &str =
        "Build log was truncated to 256 KiB; runtime reported successful completion.";
    let scenario = Operator::prepare();
    // Each image build writes twice the bound, then exits 0.
    let burst = format!("{},0", 2 * BOUND);
    scenario.state.plan(&[("burst", burst.as_str())]);
    let original = tree_manifest(&scenario.source);

    let exit = scenario.run("build", &[], &Spawn::default());
    succeed("build", &exit);
    assert_eq!(
        scenario.state.effects("build"),
        2,
        "FAIL: expected both image builds; calls: {}",
        scenario.state.calls()
    );
    // What is printed of each build: the first 256 KiB its engine command
    // wrote, then the line end the CLI adds.
    let mut log: Vec<u8> = BURST_LINE.iter().copied().cycle().take(BOUND).collect();
    log.push(b'\n');
    assert!(
        exit.stdout.as_bytes() == [log.as_slice(), log.as_slice()].concat(),
        "FAIL: the printed build logs are not, twice, the first 256 KiB of a build: {} bytes \
         were printed",
        exit.stdout.len()
    );
    assert_eq!(
        exit.stderr.lines().filter(|line| *line == NOTICE).count(),
        2,
        "FAIL: each cut build log is not announced once; stderr tail:\n{}",
        tail(&exit.stderr, 12)
    );
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
fn r19_named_stack_uses_the_selected_ports_and_stops_the_same_project() {
    let scenario = Operator::prepare();
    scenario.initialize_config();
    let mut original_config = tree_manifest(&scenario.config);
    let readiness = Readiness::listen(
        [0, 0],
        &scenario.tmp,
        &scenario.config.join("secrets/viewer"),
    );
    let [gateway_port, dashboard_port] = readiness.ports;
    let config = scenario.config.to_string_lossy().into_owned();
    let (gateway_port_text, dashboard_port_text) =
        (gateway_port.to_string(), dashboard_port.to_string());
    let started = scenario.run(
        "up",
        &[
            "--config-dir",
            &config,
            "--stack-name",
            "qa-second",
            "--gateway-port",
            &gateway_port_text,
            "--dashboard-port",
            &dashboard_port_text,
        ],
        &Spawn::default(),
    );
    let asked = readiness.finish();
    succeed("up", &started);
    let started_config = tree_manifest(&scenario.config);
    // Up refreshes the mounted copies, while the private sources keep their metadata.
    for projection in [
        "mounted-secrets",
        "mounted-secrets/runner",
        "mounted-secrets/viewer",
    ] {
        original_config.get_mut(projection).unwrap().modified = started_config[projection].modified;
    }
    assert!(
        started_config == original_config,
        "FAIL: up changed the control configuration beyond refreshing the mounted projections"
    );
    for line in [
        "Stack: qa-second".to_owned(),
        format!("Gateway: http://127.0.0.1:{gateway_port}"),
        format!("Dashboard: http://127.0.0.1:{dashboard_port}"),
    ] {
        assert!(
            started.stdout.contains(&line),
            "FAIL: up did not report {line:?}; stdout: {}",
            started.stdout
        );
    }
    let commands = scenario.state.consumers("compose");
    assert_eq!(commands.len(), 1, "calls: {}", scenario.state.calls());
    let up = &commands[0];
    assert_eq!(up.value("--project-name"), Some("qa-second"));
    assert_eq!(
        up.compose_environment(),
        vec![
            format!("AGENTSANDBOX_CONFIG_DIR={config}"),
            format!("AI_GUARD_DASHBOARD_PORT={dashboard_port}"),
            format!("AI_GUARD_GATEWAY_PORT={gateway_port}"),
        ],
        "FAIL: unexpected Compose environment"
    );
    for port in [gateway_port, dashboard_port] {
        assert!(
            asked.iter().any(|asked| asked.port == port
                && asked.request == READINESS_REQUEST
                && asked.authorized),
            "FAIL: up did not ask 127.0.0.1:{port} for viewer access"
        );
    }

    let stopped = scenario.run(
        "down",
        &["--config-dir", &config, "--stack-name", "qa-second"],
        &Spawn::default(),
    );
    succeed("down", &stopped);
    assert!(
        stopped
            .stdout
            .contains("Control-plane stack qa-second stopped")
    );
    let commands = scenario.state.consumers("compose");
    assert_eq!(commands.len(), 2, "calls: {}", scenario.state.calls());
    let down = &commands[1];
    assert_eq!(down.value("--project-name"), up.value("--project-name"));
    assert_eq!(down.engine_arguments()[7..], ["down"]);
    assert_eq!(
        down.compose_environment(),
        [format!("AGENTSANDBOX_CONFIG_DIR={config}")]
    );
    assert!(
        tree_manifest(&scenario.config) == started_config,
        "FAIL: down changed the control configuration"
    );
}

#[test]
fn r20_stack_options_are_refused_before_configuration_or_engine_effects() {
    let scenario = Operator::prepare();
    let config = scenario.config.to_string_lossy().into_owned();
    let refused = scenario.run(
        "up",
        &[
            "--config-dir",
            &config,
            "--gateway-port",
            "9100",
            "--dashboard-port",
            "9100",
        ],
        &Spawn::default(),
    );
    assert!(!refused.status.success(), "FAIL: equal ports were accepted");
    assert!(
        refused.stderr.contains("must be distinct"),
        "FAIL: refusal text: {}",
        refused.stderr
    );
    for (action, flag, value) in [
        ("up", "--stack-name", "Upper"),
        ("up", "--gateway-port", "0"),
        ("up", "--dashboard-port", "65536"),
        ("up", "--runtime", "kubernetes"),
        ("down", "--stack-name", "Upper"),
    ] {
        let exit = scenario.run(
            action,
            &["--config-dir", &config, flag, value],
            &Spawn::default(),
        );
        assert_eq!(
            exit.status.code(),
            Some(2),
            "FAIL: {flag} {value}: {}",
            exit.describe()
        );
    }
    no_engine_call(&scenario);
    assert!(
        !scenario.config.exists(),
        "FAIL: a refused command initialized a configuration"
    );
}
