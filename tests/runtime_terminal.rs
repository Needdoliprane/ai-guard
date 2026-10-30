//! External Rust CLI fixture: proves validation in Runtime::execute, not engine isolation.
#![cfg(unix)]

use agentsandbox::runtime::{ContainerPurpose, ContainerSpec, NetworkMode, Runtime, RuntimeKind};
use std::{fs, path::PathBuf, time::Duration};

const FIXTURE: &str = r#"
use std::{fs, io::Write};
fn fixture(key: &str) -> String { fs::read_to_string(std::env::current_exe().unwrap().with_extension("fixture").join(key)).unwrap() }
fn main() {
    let operation = std::env::args().nth(1).unwrap();
    let mut log = fs::OpenOptions::new().create(true).append(true)
        .open(fixture("AI_GUARD_TERMINAL_LOG")).unwrap();
    writeln!(log, "{operation}").unwrap();
    match operation.as_str() {
        "create" => println!("fixture-container"),
        "start" | "rm" => {},
        "inspect" => {
            if std::env::args().any(|arg| arg == "{{json .}}") {
                println!("{{\"Config\":{{\"Tty\":false,\"OpenStdin\":false,\"StdinOnce\":false}},\"HostConfig\":{{\"LogConfig\":{{\"Type\":\"none\",\"Config\":{{}}}}}}}}");
            } else {
                println!("{}", fixture("AI_GUARD_TERMINAL_STATE"));
            }
        },
        _ => std::process::exit(126),
    }
}
"#;

fn state(case: &str, kind: RuntimeKind) -> (&'static str, bool, i64) {
    match case {
        "stopped_success" => (
            r#"{"Status":"stopped","Running":false,"StartedAt":"2026-10-29T10:42:00Z","FinishedAt":"2026-10-29T10:42:01Z","ExitCode":0}"#,
            kind == RuntimeKind::Podman,
            0,
        ),
        "stopped_failure" => (
            r#"{"Status":"stopped","Running":false,"StartedAt":"2026-10-29T10:42:00Z","FinishedAt":"2026-10-29T10:42:01Z","ExitCode":7}"#,
            kind == RuntimeKind::Podman,
            7,
        ),
        "negative_exit" => (
            r#"{"Status":"exited","Running":false,"StartedAt":"2026-10-29T10:42:00Z","FinishedAt":"2026-10-29T10:42:01Z","ExitCode":-1}"#,
            false,
            0,
        ),
        "stopped_negative_exit" => (
            r#"{"Status":"stopped","Running":false,"StartedAt":"2026-10-29T10:42:00Z","FinishedAt":"2026-10-29T10:42:01Z","ExitCode":-1}"#,
            false,
            0,
        ),
        "stopped_running" => (
            r#"{"Status":"stopped","Running":true,"StartedAt":"2026-10-29T10:42:00Z","FinishedAt":"2026-10-29T10:42:01Z","ExitCode":0}"#,
            false,
            0,
        ),
        "stopped_missing_finish" => (
            r#"{"Status":"stopped","Running":false,"StartedAt":"2026-10-29T10:42:00Z","ExitCode":0}"#,
            false,
            0,
        ),
        "stopped_reversed_finish" => (
            r#"{"Status":"stopped","Running":false,"StartedAt":"2026-10-29T10:42:00Z","FinishedAt":"2026-10-29T10:41:59Z","ExitCode":0}"#,
            false,
            0,
        ),
        "missing_exit_code" => (
            r#"{"Status":"exited","Running":false,"StartedAt":"2026-10-29T10:42:00Z","FinishedAt":"2026-10-29T10:42:01Z"}"#,
            false,
            0,
        ),
        "still_running" => (
            r#"{"Status":"running","Running":true,"StartedAt":"2026-10-29T10:42:00Z","FinishedAt":"2026-10-29T10:42:01Z","ExitCode":0}"#,
            false,
            0,
        ),
        "inconsistent_running" => (
            r#"{"Status":"exited","Running":true,"StartedAt":"2026-10-29T10:42:00Z","FinishedAt":"2026-10-29T10:42:01Z","ExitCode":0}"#,
            false,
            0,
        ),
        "inconsistent_status" => (
            r#"{"Status":"running","Running":false,"StartedAt":"2026-10-29T10:42:00Z","FinishedAt":"2026-10-29T10:42:01Z","ExitCode":0}"#,
            false,
            0,
        ),
        "missing_running" => (
            r#"{"Status":"exited","StartedAt":"2026-10-29T10:42:00Z","FinishedAt":"2026-10-29T10:42:01Z","ExitCode":0}"#,
            false,
            0,
        ),
        "missing_finish" => (
            r#"{"Status":"exited","Running":false,"StartedAt":"2026-10-29T10:42:00Z","ExitCode":0}"#,
            false,
            0,
        ),
        "invalid_finish" => (
            r#"{"Status":"exited","Running":false,"StartedAt":"2026-10-29T10:42:00Z","FinishedAt":"not-a-timestamp","ExitCode":0}"#,
            false,
            0,
        ),
        "reversed_finish" => (
            r#"{"Status":"exited","Running":false,"StartedAt":"2026-10-29T10:42:00Z","FinishedAt":"2026-10-29T10:41:59Z","ExitCode":0}"#,
            false,
            0,
        ),
        "fast_success" => (
            r#"{"Status":"exited","Running":false,"StartedAt":"2026-10-29T10:42:00Z","FinishedAt":"2026-10-29T10:42:00.001Z","ExitCode":0}"#,
            true,
            0,
        ),
        "fast_failure" => (
            r#"{"Status":"exited","Running":false,"StartedAt":"2026-10-29T10:42:00Z","FinishedAt":"2026-10-29T10:42:00.001Z","ExitCode":7}"#,
            true,
            7,
        ),
        "lowercase_aliases" => (
            r#"{"status":"exited","running":false,"startedAt":"2026-10-29T10:42:00Z","finishedAt":"2026-10-29T10:42:00.001Z","exitCode":0}"#,
            true,
            0,
        ),
        _ => panic!("unknown test case"),
    }
}

#[test]
fn only_observed_terminal_results_are_returned_and_fast_exits_remain_valid() {
    const CHILD: &str = "AI_GUARD_TERMINAL_CHILD";
    if let Some(case) = std::env::var_os(CHILD) {
        let kind = match std::env::var("AI_GUARD_TERMINAL_KIND").as_deref() {
            Ok("docker") => RuntimeKind::Docker,
            Ok("podman") => RuntimeKind::Podman,
            _ => panic!("missing fixture runtime kind"),
        };
        let (expected_state, accepted, exit_code) = state(case.to_str().unwrap(), kind);
        assert_eq!(
            std::env::var("AI_GUARD_TERMINAL_STATE").unwrap(),
            expected_state
        );
        let runtime = Runtime::with_program(
            kind,
            &PathBuf::from(std::env::var_os("AI_GUARD_RUNTIME_PROGRAM").unwrap()),
        )
        .unwrap();
        let spec = ContainerSpec::new(
            uuid::Uuid::new_v4(),
            ContainerPurpose::Run,
            "worker:fixture",
            vec!["/usr/bin/id".into()],
            NetworkMode::None,
        )
        .unwrap();
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut starts = Vec::new();
        let outcome = executor.block_on(runtime.execute(
            &spec,
            None,
            Duration::from_secs(2),
            1024,
            |observed| {
                starts.push(observed);
                async { Ok(()) }
            },
        ));
        if accepted {
            let result = outcome.expect("observed fast terminal result must remain supported");
            assert_eq!(result.state.exit_code, exit_code);
            assert!(!result.state.running);
            assert_eq!(
                starts.len(),
                1,
                "historical start must still be observed for fast exits"
            );
            assert!(starts[0].has_started());
        } else {
            assert!(
                outcome.is_err(),
                "unproven terminal result was accepted: {outcome:?}"
            );
        }
        let log = fs::read_to_string(std::env::var_os("AI_GUARD_TERMINAL_LOG").unwrap()).unwrap();
        assert_eq!(log.lines().filter(|line| *line == "create").count(), 1);
        assert_eq!(
            log.lines().filter(|line| *line == "rm").count(),
            1,
            "owned fixture container must follow the existing cleanup path"
        );
        return;
    }

    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("terminal_fixture.rs");
    let binary = temporary.path().join("docker");
    fs::write(&source, FIXTURE).unwrap();
    let rustc = std::env::var_os("RUSTC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("rustc"));
    let compiled = std::process::Command::new(rustc)
        .args(["--edition=2024", "--crate-name", "terminal_fixture"])
        .arg(source)
        .arg("-o")
        .arg(binary)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    fs::copy(
        temporary.path().join("docker"),
        temporary.path().join("podman"),
    )
    .unwrap();
    let mut failures = Vec::new();
    for kind in [RuntimeKind::Docker, RuntimeKind::Podman] {
        for case in [
            "stopped_success",
            "stopped_failure",
            "negative_exit",
            "stopped_negative_exit",
            "stopped_running",
            "stopped_missing_finish",
            "stopped_reversed_finish",
            "missing_exit_code",
            "still_running",
            "inconsistent_running",
            "inconsistent_status",
            "missing_running",
            "missing_finish",
            "invalid_finish",
            "reversed_finish",
            "fast_success",
            "fast_failure",
            "lowercase_aliases",
        ] {
            let log = temporary.path().join(format!("{kind}-{case}.log"));
            let program = temporary.path().join(kind.program());
            let metadata = program.with_extension("fixture");
            fs::create_dir_all(&metadata).unwrap();
            fs::write(
                metadata.join("AI_GUARD_TERMINAL_LOG"),
                log.to_str().unwrap(),
            )
            .unwrap();
            fs::write(
                metadata.join("AI_GUARD_TERMINAL_STATE"),
                state(case, kind).0,
            )
            .unwrap();
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "only_observed_terminal_results_are_returned_and_fast_exits_remain_valid",
                    "--nocapture",
                ])
                .env(CHILD, case)
                .env("AI_GUARD_TERMINAL_KIND", kind.program())
                .env("AI_GUARD_TERMINAL_STATE", state(case, kind).0)
                .env("AI_GUARD_TERMINAL_LOG", log)
                .env("AI_GUARD_RUNTIME_PROGRAM", program)
                .output()
                .unwrap();
            eprintln!(
                "case {kind}/{case}: {}",
                if child.status.success() {
                    "PASS"
                } else {
                    "FAIL"
                }
            );
            if !child.status.success() {
                failures.push(format!(
                    "{kind}/{case}:\n{}\n{}",
                    String::from_utf8_lossy(&child.stdout),
                    String::from_utf8_lossy(&child.stderr)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
