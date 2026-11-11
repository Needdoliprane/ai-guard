//! R1–R5: what `agentsandbox build` and `down` hand to the engine CLI, and what
//! happens to it when the operator interrupts the command.
//!
//! Each test carries its complete oracle list. On a product that still names
//! the live source to the engine, R2, R3 and the three R5 stop at their
//! `DEFECT:` assertion and R1 and R4 stop at their `CONTRACT:` assertion.
use crate::harness::{
    COMPOSE_FILE, Exit, Invocation, Liveness, Operator, POLL, Spawn, build_inputs,
    distribution_digest, live_version, missed_ready, provenance_lines, signal_name, tail,
    tree_manifest,
};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::process::ExitStatusExt,
    path::Path,
    time::{Duration, Instant},
};

/// Test-side observation windows. They are not product deadlines.
const READY_WINDOW: Duration = Duration::from_secs(60);
const EXIT_WINDOW: Duration = Duration::from_secs(60);
const PROBE_WINDOW: Duration = Duration::from_secs(3);
const END_WINDOW: Duration = Duration::from_secs(10);

const CANARIES: [&str; 4] = [
    "images/control.Dockerfile",
    "images/worker.Dockerfile",
    "Cargo.toml",
    "src/main.rs",
];

fn succeed(action: &str, exit: &Exit) {
    assert!(
        exit.status.success(),
        "FAIL: agentsandbox {action} did not succeed: {}",
        exit.describe()
    );
}

fn two_builds(scenario: &Operator) -> Vec<Invocation> {
    let builds = scenario.state.consumers("build");
    assert_eq!(
        builds.len(),
        2,
        "FAIL: expected exactly two image builds; calls: {}",
        scenario.state.calls()
    );
    builds
}

fn one_compose(scenario: &Operator) -> Invocation {
    let mut commands = scenario.state.consumers("compose");
    assert_eq!(
        commands.len(),
        1,
        "FAIL: expected exactly one Compose command; calls: {}",
        scenario.state.calls()
    );
    commands.remove(0)
}

fn no_failed_mutation(scenario: &Operator) {
    let failed = scenario.state.names("mutate-failed-");
    assert!(
        failed.is_empty(),
        "SETUP: the fixture could not rewrite the live source: {failed:?}"
    );
}

fn outside_source(scenario: &Operator, path: &str) -> bool {
    Path::new(path).is_absolute() && !Path::new(path).starts_with(&scenario.source)
}

fn version_of(what: &str, bytes: Option<Vec<u8>>) -> u64 {
    let bytes = bytes.unwrap_or_else(|| panic!("FAIL: the consumer did not read {what}"));
    live_version(&bytes).unwrap_or_else(|| {
        panic!(
            "FAIL: {what} is not a canary version: {:?}",
            String::from_utf8_lossy(&bytes)
        )
    })
}

#[test]
fn r1_build_and_compose_consumers_read_exactly_the_prepared_inputs_from_private_paths() {
    for umask in [0o022, 0o000] {
        let scenario = Operator::prepare();
        scenario.initialize_config();
        let original = tree_manifest(&scenario.source);
        let expected: BTreeMap<String, Vec<u8>> = build_inputs()
            .into_iter()
            .map(|relative| (relative.to_owned(), scenario.source_file(relative)))
            .collect();
        let options = Spawn {
            umask,
            ..Spawn::default()
        };

        let built = scenario.run("build", &[], &options);
        succeed("build", &built);
        let left_after_build = scenario.temporary_entries();
        let config = scenario.config.to_string_lossy().into_owned();
        let stopped = scenario.run("down", &["--config-dir", &config], &options);
        succeed("down", &stopped);
        let left_after_down = scenario.temporary_entries();

        let builds = two_builds(&scenario);
        let compose = one_compose(&scenario);
        for build in &builds {
            let consumed = build.context_files();
            for (relative, bytes) in &expected {
                assert_eq!(
                    consumed.get(relative),
                    Some(bytes),
                    "FAIL: build #{} did not read the build input {relative} byte for byte",
                    build.number
                );
            }
        }

        for build in &builds {
            let context = build
                .arguments
                .last()
                .expect("a recorded build has arguments");
            assert!(
                outside_source(&scenario, context),
                "CONTRACT: build context is the live source path: {context}"
            );
            let dockerfile = build.value("--file").unwrap_or_default();
            assert!(
                outside_source(&scenario, dockerfile),
                "CONTRACT: the Dockerfile argument is a live source path: {dockerfile}"
            );
        }
        let compose_file = compose.value("--file").unwrap_or_default();
        assert!(
            outside_source(&scenario, compose_file),
            "CONTRACT: the Compose file argument is a live source path: {compose_file}"
        );

        // The context holds the build inputs and nothing else, in private modes.
        for build in &builds {
            let consumed = build.context_files();
            assert_eq!(
                consumed.keys().collect::<Vec<_>>(),
                expected.keys().collect::<Vec<_>>(),
                "FAIL: build #{} saw a different set of files than the build inputs",
                build.number
            );
            assert_eq!(
                consumed, expected,
                "FAIL: build #{} context differs",
                build.number
            );
            let entries = build.entries();
            assert!(
                entries.iter().any(|(kind, _, _)| kind == "root")
                    && entries.iter().any(|(kind, _, _)| kind == "parent"),
                "SETUP: the fixture recorded no context modes for build #{}",
                build.number
            );
            for (kind, mode, relative) in entries {
                let private = match kind.as_str() {
                    "root" | "parent" | "dir" => 0o700,
                    "file" => 0o600,
                    other => panic!("FAIL: the prepared context holds a {other}: {relative}"),
                };
                assert_eq!(
                    mode, private,
                    "FAIL: prepared {kind} {relative} has mode {mode:o} under umask {umask:o}"
                );
            }
        }

        let environment = compose.value("--env-file").unwrap_or_default();
        let wanted = [
            "compose",
            "--project-name",
            "agentsandbox",
            "--env-file",
            environment,
            "--file",
            compose_file,
            "down",
        ];
        assert_eq!(
            compose.engine_arguments(),
            &wanted[..],
            "FAIL: unexpected Compose arguments"
        );
        assert!(
            outside_source(&scenario, environment),
            "FAIL: the Compose env file is not a private absolute path: {environment}"
        );
        assert_eq!(
            compose.observed("env-file").as_deref(),
            Some(&b""[..]),
            "FAIL: the Compose env file is not empty"
        );
        let compose_bytes = scenario.source_file(COMPOSE_FILE);
        assert_eq!(
            compose.observed("compose-file").as_ref(),
            Some(&compose_bytes),
            "FAIL: the Compose consumer did not read the original file byte for byte"
        );

        // Each command reports the digest of exactly what its consumers read.
        let reported = provenance_lines(&built.stderr);
        assert_eq!(
            reported.len(),
            1,
            "FAIL: build printed {} provenance lines; stderr tail:\n{}",
            reported.len(),
            tail(&built.stderr, 12)
        );
        assert_eq!(
            reported[0].digest,
            distribution_digest(&expected),
            "FAIL: the build provenance does not describe the consumed files"
        );
        let reported = provenance_lines(&stopped.stderr);
        assert_eq!(
            reported.len(),
            1,
            "FAIL: down printed {} provenance lines; stderr tail:\n{}",
            reported.len(),
            tail(&stopped.stderr, 12)
        );
        assert_eq!(
            reported[0].digest,
            distribution_digest(&BTreeMap::from([(COMPOSE_FILE.to_owned(), compose_bytes)])),
            "FAIL: the Compose provenance does not describe the consumed file"
        );

        assert!(
            left_after_build.is_empty() && left_after_down.is_empty(),
            "FAIL: the private temporary directory is not empty after the commands: \
             {left_after_build:?} then {left_after_down:?}"
        );
        assert_eq!(
            tree_manifest(&scenario.source),
            original,
            "FAIL: the original source tree changed"
        );
    }
}

#[test]
fn r2_build_consumers_keep_prepared_bytes_when_the_original_changes_after_preparation() {
    let scenario = Operator::prepare();
    for relative in CANARIES {
        fs::write(scenario.source.join(relative), "live-version 0\n")
            .expect("SETUP: canary build input");
    }
    let source = scenario.source.to_string_lossy().into_owned();
    let canaries = CANARIES.join(",");
    scenario
        .state
        .plan(&[("source", source.as_str()), ("mutate", canaries.as_str())]);

    let built = scenario.run("build", &[], &Spawn::default());
    succeed("build", &built);
    no_failed_mutation(&scenario);
    let builds = two_builds(&scenario);

    for (index, build) in builds.iter().enumerate() {
        let consumed = version_of("its Dockerfile", build.observed("dockerfile"));
        assert!(
            consumed == 0,
            "DEFECT: build #{} consumed live-version {consumed} (N >= 1), written after validation",
            index + 1
        );
    }
    for (index, build) in builds.iter().enumerate() {
        let context = build.context_files();
        for relative in CANARIES {
            let consumed = version_of(relative, context.get(relative).cloned());
            assert!(
                consumed == 0,
                "FAIL: build #{} read live-version {consumed} of {relative} in its context",
                index + 1
            );
            let live = version_of(
                "the live canary",
                build.observed(&format!("live/{relative}")),
            );
            assert!(
                live >= 1,
                "SETUP: the live {relative} had not moved on when build #{} ran",
                index + 1
            );
        }
    }
    assert_eq!(
        builds[0].context_files(),
        builds[1].context_files(),
        "FAIL: the two image builds did not consume the same bytes"
    );
}

#[test]
fn r3_compose_consumer_keeps_the_prepared_file_when_the_original_changes_after_preparation() {
    let scenario = Operator::prepare();
    scenario.initialize_config();
    fs::write(scenario.source.join(COMPOSE_FILE), "live-version 0\n")
        .expect("SETUP: canary Compose file");
    let source = scenario.source.to_string_lossy().into_owned();
    scenario
        .state
        .plan(&[("source", source.as_str()), ("mutate", COMPOSE_FILE)]);

    let config = scenario.config.to_string_lossy().into_owned();
    let stopped = scenario.run("down", &["--config-dir", &config], &Spawn::default());
    succeed("down", &stopped);
    no_failed_mutation(&scenario);
    let compose = one_compose(&scenario);

    let bytes = compose
        .observed("compose-file")
        .expect("FAIL: the Compose consumer did not read its file");
    let consumed = version_of("the Compose file", Some(bytes.clone()));
    let live = version_of(
        "the live Compose file",
        compose.observed(&format!("live/{COMPOSE_FILE}")),
    );
    assert!(
        consumed < live,
        "DEFECT: Compose consumed the live file as of consumption time (live-version {live})"
    );

    let reported = provenance_lines(&stopped.stderr);
    assert_eq!(
        reported.len(),
        1,
        "FAIL: down printed {} provenance lines; stderr tail:\n{}",
        reported.len(),
        tail(&stopped.stderr, 12)
    );
    assert_eq!(
        reported[0].digest,
        distribution_digest(&BTreeMap::from([(COMPOSE_FILE.to_owned(), bytes)])),
        "FAIL: the consumed Compose file is not the one named by the provenance digest"
    );
}

#[test]
fn r4_operator_signal_before_the_engine_command_starts_runs_no_consumer_and_leaves_no_prepared_source()
 {
    let scenario = Operator::prepare();
    scenario
        .state
        .plan(&[("hold", "first-call"), ("deadline-seconds", "120")]);
    let mut running = scenario.spawn("build", &[], &Spawn::default());

    let Some(ready) = scenario.state.wait_ready("leader", READY_WINDOW) else {
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
    };
    eprintln!("latency: spawn to ready-leader (first engine call) {ready:?}");
    if let Some(early) = running.poll() {
        panic!(
            "SETUP: the command ended before the signal was sent: {}",
            early.describe()
        );
    }
    running.signal(libc::SIGTERM);
    let exit = running.wait(EXIT_WINDOW).unwrap_or_else(|| {
        panic!(
            "FAIL: the command is still pending {EXIT_WINDOW:?} after SIGTERM; {}",
            scenario.state.describe()
        )
    });

    assert_eq!(
        scenario.state.effects("build"),
        0,
        "FAIL: a build reached the engine; calls: {}",
        scenario.state.calls()
    );
    assert!(
        scenario.state.consumers("build").is_empty(),
        "FAIL: a build consumer was started; calls: {}",
        scenario.state.calls()
    );
    let left = scenario.temporary_entries();
    assert!(
        left.is_empty(),
        "FAIL: a prepared source was left behind: {left:?}"
    );

    if let Some(signal) = exit.status.signal() {
        panic!(
            "CONTRACT: command ended by signal {signal} without an explicit interruption result"
        );
    }
    assert_eq!(
        exit.status.code(),
        Some(1),
        "FAIL: unexpected status after an interruption: {}",
        exit.describe()
    );
    assert!(
        exit.stderr.contains("interrupted by SIGTERM")
            && exit
                .stderr
                .contains("no engine build or Compose command was running"),
        "FAIL: the interruption result is not explicit; stderr tail:\n{}",
        tail(&exit.stderr, 12)
    );
}

/// `signal: <n>` for exactly this number: `signal: 1` is not found in
/// `signal: 15`.
fn names_signal(text: &str, signal: i32) -> bool {
    let wanted = format!("signal: {signal}");
    text.match_indices(&wanted)
        .any(|(at, _)| !text[at + wanted.len()..].starts_with(|next: char| next.is_ascii_digit()))
}

#[test]
fn r5_oracle_tells_a_signal_number_from_a_longer_one() {
    let hangup = "that process exited (signal: 1 (SIGHUP)) and both";
    let terminate = "that process exited (signal: 15 (SIGTERM)) and both";
    assert!(names_signal(hangup, libc::SIGHUP) && names_signal("(signal: 1)", libc::SIGHUP));
    assert!(names_signal(terminate, libc::SIGTERM));
    assert!(
        !names_signal(terminate, libc::SIGHUP),
        "FAIL: an end by SIGTERM is read as an end by SIGHUP"
    );
    assert!(!names_signal(hangup, libc::SIGTERM) && !names_signal("signal 1", libc::SIGHUP));
}

#[test]
fn r5_sigterm_during_consumption_returns_only_after_the_consumer_ended() {
    interrupt_during_build(libc::SIGTERM);
}

#[test]
fn r5_sigint_during_consumption_returns_only_after_the_consumer_ended() {
    interrupt_during_build(libc::SIGINT);
}

#[test]
fn r5_sighup_during_consumption_returns_only_after_the_consumer_ended() {
    interrupt_during_build(libc::SIGHUP);
}

/// The signal goes to the control process only. A terminal that signals a
/// whole foreground process group is not reproduced here.
fn interrupt_during_build(signal: i32) {
    let name = signal_name(signal);
    let scenario = Operator::prepare();
    scenario
        .state
        .plan(&[("hold", "consume"), ("deadline-seconds", "120")]);
    let mut running = scenario.spawn("build", &[], &Spawn::default());

    let Some(ready) = scenario.state.wait_ready("leader", READY_WINDOW) else {
        let product = running.product();
        panic!(
            "{}",
            missed_ready(
                &scenario.state,
                "leader",
                "build",
                running.spawned_at,
                READY_WINDOW,
                &product
            )
        );
    };
    eprintln!("latency: spawn to ready-leader (first build) {ready:?}");
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
    let prepared = scenario.prepared_directories();

    running.signal(signal);
    let sent = Instant::now();
    let exit = loop {
        if let Some(exit) = running.poll() {
            break exit;
        }
        // The lock is tried after the directory is seen missing: a consumer
        // that is still alive then was alive when its input disappeared.
        for directory in &prepared {
            if !directory.exists() && scenario.state.liveness("leader") == Liveness::Alive {
                panic!(
                    "FAIL: prepared source removed while its consumer is alive: {}",
                    directory.display()
                );
            }
        }
        if sent.elapsed() >= EXIT_WINDOW {
            panic!(
                "FAIL: the command is still pending {EXIT_WINDOW:?} after {name}; {}",
                scenario.state.describe()
            );
        }
        std::thread::sleep(POLL);
    };

    let unknown = exit.stderr.contains("termination is unknown");
    if scenario.state.probe("leader", PROBE_WINDOW) {
        assert!(
            unknown,
            "DEFECT: command ended ({}) while its build consumer still answers a probe; \
             no unknown state was reported",
            match exit.status.signal() {
                Some(number) => format!("signal {number}"),
                None => exit.status.to_string(),
            }
        );
    }
    assert!(
        !unknown,
        "FAIL: unexpected unknown termination {:?} after {name}: {}",
        sent.elapsed(),
        exit.describe()
    );
    assert!(
        scenario.state.ended("leader", END_WINDOW),
        "FAIL: the command returned but its build consumer was not proven ended; {}",
        scenario.state.describe()
    );
    assert_eq!(
        exit.status.code(),
        Some(1),
        "FAIL: unexpected status after {name}: {}",
        exit.describe()
    );
    assert!(
        exit.stderr.contains(&format!("interrupted by {name}"))
            && names_signal(&exit.stderr, signal),
        "FAIL: the message does not name {name} and the CLI process's end by signal {signal}; \
         stderr tail:\n{}",
        tail(&exit.stderr, 12)
    );
    for marker in ["material-missing-leader", "material-changed-leader"] {
        assert!(
            !scenario.state.exists(marker),
            "FAIL: the build consumer recorded {marker}: {:?}",
            scenario.state.read(marker)
        );
    }
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
