//! Regression coverage for stopping only a container whose inspected snapshot
//! still matches the immutable launch authority.
use agentsandbox::runtime::{
    ContainerPurpose, ContainerSpec, ManagedRuntime, NetworkMode, Runtime, RuntimeKind,
    StartDirective,
};
use std::{fs, process::Command, time::Duration};
use uuid::Uuid;

const CHILD: &str = "AI_GUARD_STOP_SNAPSHOT_CHILD";

#[test]
fn divergent_running_container_is_retained_without_kill() {
    if let Ok(case) = std::env::var(CHILD) {
        let root =
            std::path::PathBuf::from(std::env::var_os("AI_GUARD_STOP_SNAPSHOT_ROOT").unwrap());
        let runtime = Runtime::with_program(
            RuntimeKind::Docker,
            &std::path::PathBuf::from(std::env::var_os("AI_GUARD_RUNTIME_PROGRAM").unwrap()),
        )
        .unwrap();
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        executor.block_on(async {
            let managed = ManagedRuntime::connect_docker_with_runtime(
                Uuid::new_v4(),
                "unix:///fixture/docker.sock",
                runtime,
            )
            .await
            .unwrap();
            let spec = ContainerSpec::with_operation(
                Uuid::new_v4(),
                ContainerPurpose::Run,
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                vec!["/usr/bin/id".into()],
                NetworkMode::None,
                Uuid::new_v4(),
            )
            .unwrap()
            .with_managed_names("aig-fixture-worker", "aig-fixture-worker-work")
            .unwrap();
            if matches!(case.as_str(), "config_changed" | "logging_changed") {
                let outcome = managed
                    .execute_retained(
                        &spec,
                        None,
                        Duration::from_millis(100),
                        1024,
                        |_| async { Ok(StartDirective::EmitOnce) },
                        |_| async { Ok(()) },
                    )
                    .await;
                assert!(
                    outcome.is_err(),
                    "divergent running configuration must not be accepted"
                );
            } else {
                let mut captured = None;
                let outcome = managed
                    .execute_retained(
                        &spec,
                        None,
                        Duration::from_secs(1),
                        1024,
                        |snapshot| {
                            captured = Some(snapshot);
                            async { Ok(StartDirective::ObserveOnly) }
                        },
                        |_| async { Ok(()) },
                    )
                    .await;
                assert!(
                    outcome.is_err(),
                    "ObserveOnly must retain the created instance"
                );
                let snapshot = captured.expect("created snapshot must be delivered once");
                match case.as_str() {
                    "exact_running" => fs::write(root.join("state"), "running").unwrap(),
                    "unknown_running" => {
                        fs::write(root.join("state"), "running").unwrap();
                        fs::write(root.join("inspect_unknown"), "1").unwrap();
                    }
                    "inspect_refused" => fs::write(root.join("inspect_refused"), "1").unwrap(),
                    "created_unallocatable" => {
                        fs::write(root.join("created_unallocatable"), "1").unwrap()
                    }
                    _ => unreachable!(),
                }
                let stopped = managed.stop_snapshot_owned(&snapshot).await;
                if case == "exact_running" {
                    let observation = stopped.expect("exact running snapshot may be stopped");
                    assert_eq!(observation.process.status.as_deref(), Some("exited"));
                } else {
                    assert!(
                        stopped.is_err(),
                        "untrusted stop observation must be refused"
                    );
                }
            }
        });

        let log = fs::read_to_string(root.join("log")).unwrap();
        let kill_count = log.lines().filter(|line| line.starts_with("kill ")).count();
        if case == "exact_running" {
            assert_eq!(kill_count, 1, "exact owned snapshot should stop once");
        } else {
            assert_eq!(
                kill_count, 0,
                "untrusted or divergent evidence must never kill ({case}); log={log}"
            );
        }
        if matches!(case.as_str(), "config_changed" | "logging_changed") {
            assert_eq!(fs::read_to_string(root.join("state")).unwrap(), "running");
        }
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let source = include_str!("runtime_fixtures/managed.rs");
    let held_source = source.replace(
        "case == \"timeout\" || case == \"start_callback_error\"",
        "case == \"timeout\" || case == \"start_callback_error\" || matches!(case.as_str(), \"config_changed\" | \"logging_changed\")",
    );
    let held_source = held_source.replace(
        "            println!(\"{json}\");",
        r##"            if root.join("inspect_refused").exists() { std::process::exit(125); }
            if root.join("inspect_unknown").exists() { json = json.replace("\"Running\":true,", ""); }
            if root.join("created_unallocatable").exists() { json = json.replace("\"Memory\":1073741824", "\"Memory\":0"); }
            println!("{json}");"##,
    );
    assert_ne!(source, held_source, "fixture adaptation must apply");
    let fixture_source = temp.path().join("managed_fixture.rs");
    fs::write(&fixture_source, held_source).unwrap();
    let fixture = temp.path().join("docker");
    let output = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .args(["--edition=2024", "--crate-name", "managed_fixture"])
        .arg(&fixture_source)
        .arg("-o")
        .arg(&fixture)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fixture compiler failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut failures = Vec::new();
    for case in [
        "config_changed",
        "logging_changed",
        "exact_running",
        "inspect_refused",
        "unknown_running",
        "created_unallocatable",
    ] {
        let root = temp.path().join(case);
        fs::create_dir(&root).unwrap();
        let metadata = fixture.with_extension("fixture");
        fs::create_dir_all(&metadata).unwrap();
        fs::write(
            metadata.join("AI_GUARD_MANAGED_ROOT"),
            root.to_string_lossy().as_bytes(),
        )
        .unwrap();
        fs::write(metadata.join("AI_GUARD_MANAGED_CHILD"), case).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "divergent_running_container_is_retained_without_kill",
                "--nocapture",
            ])
            .env(CHILD, case)
            .env("AI_GUARD_STOP_SNAPSHOT_ROOT", &root)
            .env("AI_GUARD_RUNTIME_PROGRAM", &fixture)
            .output()
            .unwrap();
        if !output.status.success() {
            failures.push(format!(
                "case {case}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
