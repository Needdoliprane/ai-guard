//! Lifecycle ordering only: external Rust CLI fixture, never an isolation proof.
#![cfg(unix)]

use agentsandbox::runtime::{ContainerPurpose, ContainerSpec, NetworkMode, Runtime, RuntimeKind};
use std::{fs, io::Write, path::PathBuf, time::Duration};

const FIXTURE: &str = r###"
use std::{fs, io::Write};
fn set_state(path: &std::ffi::OsStr, value: &str) {
    let path = std::path::Path::new(path);
    let temporary = path.with_extension(format!("state-{}", std::process::id()));
    fs::write(&temporary, value).unwrap();
    fs::rename(temporary, path).unwrap();
}
fn fixture(key: &str) -> String { fs::read_to_string(std::env::current_exe().unwrap().with_extension("fixture").join(key)).unwrap() }
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let operation = args.first().unwrap();
    let mut marker = std::ffi::OsString::from(fixture("AI_GUARD_RETAIN_INSTANCE"));
    let unavailable = std::ffi::OsString::from(fixture("AI_GUARD_RETAIN_UNAVAILABLE"));
    let case = fixture("AI_GUARD_RETAIN_CHILD");
    let alias = std::ffi::OsString::from(fixture("AI_GUARD_RETAIN_ALIAS"));
    let name = std::ffi::OsString::from(fixture("AI_GUARD_RETAIN_NAME"));
    let mut log = fs::OpenOptions::new().create(true).append(true)
        .open(std::ffi::OsString::from(fixture("AI_GUARD_RETAIN_LOG"))).unwrap();
    writeln!(log, "{} {}", operation, args.last().unwrap()).unwrap();
    if std::path::Path::new(&unavailable).exists() {
        eprintln!("fixture engine unavailable"); std::process::exit(125);
    }
    if std::path::Path::new(&alias).exists()
        && fs::read_to_string(&name).ok().as_ref() == args.last() {
        marker = std::ffi::OsString::from(fixture("AI_GUARD_RETAIN_OTHER"));
    }
    match operation.as_str() {
        "create" => {
            if case == "create_conflict" { std::process::exit(125); }
            let index = args.iter().position(|arg| arg == "--name").unwrap();
            fs::write(name, &args[index + 1]).unwrap();
            set_state(&marker, "created");
            if case.starts_with("unusable_") {
                fs::write(alias, "name now belongs to successor").unwrap();
                if case.ends_with("malformed") { println!("not a usable reference"); }
                if case.ends_with("truncated") { println!("{}", "x".repeat(256 * 1024 + 1)); }
                return;
            }
            println!("fixture-owned-id");
        },
        "start" => {
            set_state(&marker, if case.starts_with("callback_") { "running" } else { "exited" });
            std::thread::sleep(std::time::Duration::from_millis(50));
        },
        "inspect" => {
            if args.iter().any(|arg| arg == "{{json .}}") {
                println!(r#"{{"Config":{{"Tty":false,"OpenStdin":false,"StdinOnce":false}},"HostConfig":{{"LogConfig":{{"Type":"none","Config":{{}}}}}}}}"#);
                return;
            }
            let state = fs::read_to_string(marker).unwrap_or_default();
            let (status, running, start, finish) = match state.as_str() {
                "created" => ("created", false, "0001-01-01T00:00:00Z", "0001-01-01T00:00:00Z"),
                "running" => ("running", true, "2026-10-29T10:42:00Z", "0001-01-01T00:00:00Z"),
                "exited" => ("exited", false, "2026-10-29T10:42:00Z", "2026-10-29T10:42:01Z"),
                _ => std::process::exit(125),
            };
            println!(r#"{{"Status":"{status}","Running":{running},"StartedAt":"{start}","FinishedAt":"{finish}","ExitCode":0}}"#);
        },
        "kill" => { set_state(&marker, "exited"); },
        "rm" => {
            if case == "rm_failure" { std::process::exit(125); }
            fs::remove_file(marker).unwrap();
        },
        _ => std::process::exit(126),
    }
}
"###;

#[test]
fn terminal_instances_survive_until_explicit_cleanup_and_failed_start_is_stopped() {
    const CHILD: &str = "AI_GUARD_RETAIN_CHILD";
    if let Ok(case) = std::env::var(CHILD) {
        let marker = PathBuf::from(std::env::var_os("AI_GUARD_RETAIN_INSTANCE").unwrap());
        let unavailable = PathBuf::from(std::env::var_os("AI_GUARD_RETAIN_UNAVAILABLE").unwrap());
        let log_path = PathBuf::from(std::env::var_os("AI_GUARD_RETAIN_LOG").unwrap());
        let runtime = Runtime::with_program(
            RuntimeKind::Docker,
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
        if case.starts_with("unusable_") {
            let outcome = if case.starts_with("unusable_legacy_") {
                executor
                    .block_on(runtime.execute(
                        &spec,
                        None,
                        Duration::from_secs(2),
                        1024,
                        |_| async { Ok(()) },
                    ))
                    .map(|_| ())
            } else {
                executor
                    .block_on(runtime.execute_retained(
                        &spec,
                        None,
                        Duration::from_secs(2),
                        1024,
                        |_| async { Ok(()) },
                    ))
                    .map(|_| ())
            };
            let error = outcome.expect_err("unusable create reference must not launch");
            let log = fs::read_to_string(&log_path).unwrap();
            assert_eq!(
                log.lines().count(),
                1,
                "unknown ownership must not inspect, start, kill or remove a successor by name: {log}"
            );
            assert_eq!(
                fs::read_to_string(std::env::var_os("AI_GUARD_RETAIN_OTHER").unwrap()).unwrap(),
                "running"
            );
            assert_eq!(fs::read_to_string(&marker).unwrap(), "created");
            let message = format!("{error:#}");
            assert!(
                message.contains(&spec.name()),
                "diagnostic must identify the requested name: {message}"
            );
            assert!(message.contains("ownership unknown"), "{message}");
            return;
        }
        if case == "legacy_cleanup" {
            let result = executor
                .block_on(
                    runtime.execute(&spec, None, Duration::from_secs(2), 1024, |_| async {
                        Ok(())
                    }),
                )
                .unwrap();
            assert_eq!(result.state.exit_code, 0);
            assert!(
                !marker.exists(),
                "legacy execute must keep automatic cleanup"
            );
            return;
        }
        let mut start_seen = false;
        let outcome = executor.block_on(runtime.execute_retained(
            &spec,
            None,
            Duration::from_secs(2),
            1024,
            |_| {
                start_seen = true;
                let unavailable = unavailable.clone();
                let case = case.clone();
                async move {
                    if case == "callback_engine_unavailable" {
                        fs::write(unavailable, "unavailable")?;
                    }
                    anyhow::ensure!(
                        !case.starts_with("callback_"),
                        "fixture start audit refused"
                    );
                    Ok(())
                }
            },
        ));
        if case == "create_conflict" {
            assert!(outcome.is_err());
            assert_eq!(
                fs::read_to_string(marker).unwrap(),
                "belongs to another operation"
            );
            let log = fs::read_to_string(log_path).unwrap();
            assert_eq!(
                log.lines().count(),
                1,
                "failed create must not inspect, adopt, stop or remove"
            );
            return;
        }
        if case.starts_with("callback_") {
            let error = outcome.expect_err("start audit failure must abort the execution");
            assert!(
                start_seen,
                "fixture must reach the intended start callback: {error:#}"
            );
            assert!(
                marker.exists(),
                "failed execution must retain its created instance"
            );
            if case == "callback_engine_unavailable" {
                let message = format!("{error:#}");
                assert!(message.contains("stop unknown"), "{message}");
            } else {
                assert_eq!(
                    fs::read_to_string(marker).unwrap(),
                    "exited",
                    "owned running instance must be stopped"
                );
            }
            let log = fs::read_to_string(log_path).unwrap();
            assert!(!log.lines().any(|line| line.starts_with("rm ")));
            return;
        }
        let retained = outcome.expect("known terminal result must remain accessible");
        assert_eq!(retained.result().state.exit_code, 0);
        assert_eq!(retained.container_name(), spec.name());
        assert!(
            marker.exists(),
            "no rm before the caller commits its terminal result"
        );
        let before_ack = fs::read_to_string(&log_path).unwrap();
        assert!(!before_ack.lines().any(|line| line.starts_with("rm ")));
        assert!(
            executor.block_on(runtime.inspect(&spec)).is_ok(),
            "retained terminal must remain inspectable"
        );
        if case == "audit_failure" {
            let finalization: anyhow::Result<()> =
                Err(anyhow::anyhow!("fixture terminal audit refused"));
            assert!(finalization.is_err());
            assert!(
                marker.exists(),
                "failed finalization must not delete evidence"
            );
            drop(retained);
            assert!(marker.exists(), "drop must not remove a retained instance");
            return;
        }
        if case == "rm_failure" {
            assert!(executor.block_on(runtime.cleanup(&retained)).is_err());
            assert_eq!(retained.result().state.exit_code, 0);
            assert!(marker.exists());
            return;
        }
        let mut log = fs::OpenOptions::new().append(true).open(&log_path).unwrap();
        writeln!(log, "audit-ack").unwrap();
        if case == "name_substitution" {
            fs::write(
                std::env::var_os("AI_GUARD_RETAIN_ALIAS").unwrap(),
                "swapped",
            )
            .unwrap();
        }
        executor.block_on(runtime.cleanup(&retained)).unwrap();
        assert_eq!(retained.result().state.exit_code, 0);
        assert!(!marker.exists());
        if case == "name_substitution" {
            assert_eq!(
                fs::read_to_string(std::env::var_os("AI_GUARD_RETAIN_OTHER").unwrap()).unwrap(),
                "other instance unchanged"
            );
        }
        let log = fs::read_to_string(&log_path).unwrap();
        let lines: Vec<_> = log.lines().collect();
        assert!(
            lines.iter().position(|line| *line == "audit-ack").unwrap()
                < lines
                    .iter()
                    .position(|line| line.starts_with("rm "))
                    .unwrap()
        );
        assert!(
            lines
                .iter()
                .filter(|line| line.starts_with("start ")
                    || line.starts_with("kill ")
                    || line.starts_with("rm "))
                .all(|line| line.ends_with("fixture-owned-id")),
            "mutating operations must target the returned created reference"
        );
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("retention_fixture.rs");
    fs::write(&source, FIXTURE).unwrap();
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let compiled = std::process::Command::new(rustc)
        .args(["--edition=2024", "--crate-name", "retention_fixture"])
        .arg(source)
        .arg("-o")
        .arg(temp.path().join("docker"))
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let mut failures = Vec::new();
    for case in [
        "terminal_before_ack",
        "audit_failure",
        "rm_failure",
        "create_conflict",
        "callback_stop",
        "callback_engine_unavailable",
        "legacy_cleanup",
        "name_substitution",
        "unusable_retained_empty",
        "unusable_retained_malformed",
        "unusable_retained_truncated",
        "unusable_legacy_empty",
        "unusable_legacy_malformed",
        "unusable_legacy_truncated",
    ] {
        let marker = temp.path().join(format!("{case}.instance"));
        if case == "create_conflict" {
            fs::write(&marker, "belongs to another operation").unwrap();
        }
        let other = temp.path().join(format!("{case}.other"));
        fs::write(
            &other,
            if case.starts_with("unusable_") {
                "running"
            } else {
                "other instance unchanged"
            },
        )
        .unwrap();
        let metadata = temp.path().join("docker").with_extension("fixture");
        fs::create_dir_all(&metadata).unwrap();
        for (key, value) in [
            ("AI_GUARD_RETAIN_CHILD", PathBuf::from(case)),
            ("AI_GUARD_RETAIN_INSTANCE", marker.clone()),
            ("AI_GUARD_RETAIN_OTHER", other.clone()),
            (
                "AI_GUARD_RETAIN_LOG",
                temp.path().join(format!("{case}.log")),
            ),
            (
                "AI_GUARD_RETAIN_UNAVAILABLE",
                temp.path().join(format!("{case}.unavailable")),
            ),
            (
                "AI_GUARD_RETAIN_ALIAS",
                temp.path().join(format!("{case}.alias")),
            ),
            (
                "AI_GUARD_RETAIN_NAME",
                temp.path().join(format!("{case}.name")),
            ),
        ] {
            fs::write(metadata.join(key), value.to_str().unwrap()).unwrap();
        }
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "terminal_instances_survive_until_explicit_cleanup_and_failed_start_is_stopped",
                "--nocapture",
            ])
            .env(CHILD, case)
            .env("AI_GUARD_RETAIN_INSTANCE", marker)
            .env(
                "AI_GUARD_RETAIN_LOG",
                temp.path().join(format!("{case}.log")),
            )
            .env(
                "AI_GUARD_RETAIN_UNAVAILABLE",
                temp.path().join(format!("{case}.unavailable")),
            )
            .env(
                "AI_GUARD_RETAIN_ALIAS",
                temp.path().join(format!("{case}.alias")),
            )
            .env(
                "AI_GUARD_RETAIN_NAME",
                temp.path().join(format!("{case}.name")),
            )
            .env("AI_GUARD_RETAIN_OTHER", other)
            .env("AI_GUARD_RUNTIME_PROGRAM", temp.path().join("docker"))
            .output()
            .unwrap();
        eprintln!(
            "case {case}: {}",
            if child.status.success() {
                "PASS"
            } else {
                "FAIL"
            }
        );
        if !child.status.success() {
            failures.push(format!(
                "{case}:\n{}\n{}",
                String::from_utf8_lossy(&child.stdout),
                String::from_utf8_lossy(&child.stderr)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
