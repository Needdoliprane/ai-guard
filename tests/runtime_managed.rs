//! Native inspect fixtures exercise normalization; they do not qualify an engine.
use agentsandbox::runtime::{
    ContainerPurpose, ContainerSpec, NetworkMode, Runtime, RuntimeKind, parse_container_observation,
};
use serde_json::{Value, json};
use uuid::Uuid;

fn inspect_fixture() -> Value {
    json!({
        "Id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "Name":"/agentsandbox-fixture", "Image":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "RestartCount":0,
        "State":{"Status":"exited","Running":false,"StartedAt":"2026-10-29T12:00:00Z","FinishedAt":"2026-10-29T12:00:01.500Z","ExitCode":0,"OOMKilled":false},
        "HostConfig":{"NanoCpus":2000000000_u64,"CpuQuota":200000,"CpuPeriod":100000,"Memory":1073741824_u64,"PidsLimit":256},
        "Config":{"Env":["PROVIDER_KEY=fixture-private-value"]}
    })
}
fn parse(value: Value, kind: RuntimeKind) -> agentsandbox::runtime::ContainerObservation {
    parse_container_observation(&serde_json::to_vec(&value).unwrap(), kind).unwrap()
}

#[test]
fn persisted_operation_keeps_the_same_name_and_create_identity() {
    let session = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
    let operation = Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap();
    for _ in 0..2 {
        let spec = ContainerSpec::with_operation(
            session,
            ContainerPurpose::Run,
            "worker:fixture",
            vec!["/usr/bin/id".into()],
            NetworkMode::None,
            operation,
        )
        .unwrap();
        assert_eq!(
            spec.name(),
            "agentsandbox-11111111-1111-4111-8111-111111111111-run-22222222222242228222222222222222"
        );
        assert_eq!(spec.operation_id(), operation);
    }
}

#[test]
fn native_snapshots_keep_observed_dates_limits_and_ignore_environment() {
    for kind in [RuntimeKind::Docker, RuntimeKind::Podman] {
        let mut value = inspect_fixture();
        if kind == RuntimeKind::Podman {
            value["State"]["Status"] = json!("stopped");
            value["HostConfig"]["NanoCpus"] = json!(0);
            value["HostConfig"]["CpuQuota"] = json!(100000);
            value["HostConfig"]["CpuPeriod"] = json!(50000);
        }
        let observed = parse(value, kind);
        assert_eq!(observed.name, "agentsandbox-fixture");
        assert_eq!(observed.restart_count, Some(0));
        let cpu = observed.allocation.cpu.as_ref().unwrap();
        assert_eq!((cpu.quota, cpu.period), (2, 1));
        assert_eq!(observed.allocation.memory_bytes, Some(1073741824));
        assert_eq!(observed.allocation.pids_limit, Some(256));
        assert_eq!(observed.process.exit_code, Some(0));
        assert_eq!(
            observed.process.finished_at.unwrap().timestamp_millis()
                - observed.process.started_at.unwrap().timestamp_millis(),
            1500
        );
        observed.validate_terminal(kind, &observed).unwrap();
        let stored = serde_json::to_string(&observed).unwrap();
        assert!(!stored.contains("PROVIDER_KEY"));
        assert!(!stored.contains("fixture-private-value"));
    }
}

#[test]
fn incomplete_or_inconsistent_observation_never_becomes_terminal_usage() {
    for field in ["RestartCount", "Image", "Id"] {
        let mut value = inspect_fixture();
        value.as_object_mut().unwrap().remove(field);
        let observed = parse(value, RuntimeKind::Docker);
        assert!(
            observed
                .validate_terminal(RuntimeKind::Docker, &observed)
                .is_err(),
            "missing {field}"
        );
    }
    for field in ["Running", "ExitCode", "StartedAt", "FinishedAt"] {
        let mut value = inspect_fixture();
        value["State"].as_object_mut().unwrap().remove(field);
        let observed = parse(value, RuntimeKind::Docker);
        assert!(
            observed
                .validate_terminal(RuntimeKind::Docker, &observed)
                .is_err(),
            "missing {field}"
        );
    }
    for (field, replacement) in [
        ("ExitCode", json!(-1)),
        ("Running", json!(true)),
        ("StartedAt", json!("malformed")),
        ("FinishedAt", json!("2026-10-29T11:59:59Z")),
    ] {
        let mut value = inspect_fixture();
        value["State"][field] = replacement;
        let observed = parse(value, RuntimeKind::Docker);
        assert!(
            observed
                .validate_terminal(RuntimeKind::Docker, &observed)
                .is_err(),
            "invalid {field}"
        );
    }
    for field in ["Memory", "PidsLimit", "NanoCpus", "CpuPeriod"] {
        let mut value = inspect_fixture();
        value["HostConfig"][field] = json!(0);
        if field == "NanoCpus" {
            value["HostConfig"]["CpuQuota"] = json!(-1);
        }
        let observed = parse(value, RuntimeKind::Docker);
        assert!(
            observed
                .validate_terminal(RuntimeKind::Docker, &observed)
                .is_err(),
            "unbounded {field}"
        );
        assert!(!observed.diagnostics.is_empty());
    }
    let initial = parse(inspect_fixture(), RuntimeKind::Docker);
    for (field, replacement) in [
        (
            "Image",
            json!("sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"),
        ),
        ("RestartCount", json!(1)),
    ] {
        let mut value = inspect_fixture();
        value[field] = replacement;
        let changed = parse(value, RuntimeKind::Docker);
        assert!(
            changed
                .validate_terminal(RuntimeKind::Docker, &initial)
                .is_err(),
            "changed {field}"
        );
    }
    let mut divergent = inspect_fixture();
    divergent["HostConfig"]["CpuQuota"] = json!(100000);
    let observed = parse(divergent, RuntimeKind::Docker);
    assert!(observed.allocation.cpu.is_none());
    assert!(
        observed
            .validate_terminal(RuntimeKind::Docker, &initial)
            .is_err()
    );
}

#[test]
fn managed_launch_requires_created_ack_and_keeps_owned_evidence() {
    use agentsandbox::runtime::{ManagedRuntime, StartDirective};
    use std::{fs, time::Duration};
    const CHILD: &str = "AI_GUARD_MANAGED_CHILD";
    if let Ok(case) = std::env::var(CHILD) {
        let root = std::path::PathBuf::from(std::env::var_os("AI_GUARD_MANAGED_ROOT").unwrap());
        let run = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        run.block_on(async {
            let managed = ManagedRuntime::connect_docker_with_runtime(
                Uuid::new_v4(),
                "unix:///fixture/docker.sock",
                fixture_runtime(RuntimeKind::Docker),
            )
            .await
            .unwrap();
            assert!(
                managed.doctor(false).await.is_err(),
                "the bound runtime did not prove rootless mode"
            );
            assert!(!managed.doctor(true).await.unwrap().rootless);
            if case == "volume_collision" || case == "volume_bind" {
                assert!(
                    managed
                        .create_named_volume(
                            "aig-fixture-worker-work",
                            &[("agentsandbox.owner".into(), "expected".into())]
                        )
                        .await
                        .is_err(),
                    "an existing volume with wrong ownership or host options must not be adopted"
                );
                let log = fs::read_to_string(root.join("log")).unwrap();
                assert!(!log.lines().any(|line| line.starts_with("start ")
                    || line.starts_with("kill ")
                    || line.starts_with("rm ")));
                return;
            }
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
            let mut created_handle = None;
            let outcome = managed
                .execute_retained(
                    &spec,
                    None,
                    if case == "timeout" {
                        Duration::from_millis(50)
                    } else {
                        Duration::from_secs(2)
                    },
                    1024,
                    |snapshot| {
                        created_handle = Some(snapshot.handle);
                        let case = case.clone();
                        async move {
                            anyhow::ensure!(
                                !matches!(
                                    case.as_str(),
                                    "refused_created" | "recovered_created" | "recovered_consumed"
                                ),
                                "durable created acknowledgement refused"
                            );
                            Ok(if case == "observe_only" {
                                StartDirective::ObserveOnly
                            } else {
                                StartDirective::EmitOnce
                            })
                        }
                    },
                    |observation| {
                        if case == "missing_oom" {
                            assert!(
                                observation.process.oom_killed.is_none(),
                                "an absent OOM fact must remain unknown in callbacks"
                            );
                        }
                        let case = case.clone();
                        async move {
                            anyhow::ensure!(
                                case != "start_callback_error",
                                "durable running observation refused"
                            );
                            Ok(())
                        }
                    },
                )
                .await;
            if case == "legacy_observe" {
                assert!(
                    outcome.is_err(),
                    "v2 must refuse the legacy local logging driver"
                );
                let legacy = spec
                    .clone()
                    .with_security_preset(agentsandbox::control::SecurityPresetV1::FixedSandboxV1);
                let owned = managed.lookup_created(&legacy).await.unwrap().unwrap();
                assert!(
                    owned
                        .observation
                        .configuration_fingerprint_version
                        .is_none()
                );
                assert!(
                    !serde_json::to_string(&owned)
                        .unwrap()
                        .contains("configuration_fingerprint_version")
                );
                let observed = managed.observe(&owned.handle).await.unwrap();
                assert_eq!(observed.process.status.as_deref(), Some("created"));
                assert!(
                    observed
                        .validate_terminal(RuntimeKind::Docker, &owned.observation)
                        .is_err()
                );
                managed.stop_owned(&owned.handle).await.unwrap();
                let before = fs::read_to_string(root.join("log")).unwrap();
                let mut claim_called = false;
                let refused = managed
                    .start_owned_retained(
                        &legacy,
                        &owned,
                        None,
                        Duration::from_secs(1),
                        1024,
                        |_| {
                            claim_called = true;
                            async { Ok(StartDirective::EmitOnce) }
                        },
                        |_| async { Ok(()) },
                    )
                    .await;
                assert!(
                    refused
                        .unwrap_err()
                        .to_string()
                        .contains("fixed_sandbox_v1")
                );
                assert!(
                    !claim_called,
                    "a legacy first start must not reach the claim callback"
                );
                assert_eq!(before, fs::read_to_string(root.join("log")).unwrap());
                // Model a separate historical execution already completed by
                // the old broker; this fixture state write emits no CLI start.
                fs::write(root.join("state"), "exited").unwrap();
                let terminal = managed.observe(&owned.handle).await.unwrap();
                terminal
                    .validate_terminal(RuntimeKind::Docker, &owned.observation)
                    .unwrap();
                managed.stop_owned(&owned.handle).await.unwrap();
                let after = fs::read_to_string(root.join("log")).unwrap();
                assert!(
                    !after
                        .lines()
                        .any(|line| line.starts_with("start ") || line.starts_with("rm "))
                );
                return;
            }
            let outcome = if matches!(case.as_str(), "recovered_created" | "recovered_consumed") {
                assert!(outcome.is_err());
                let owned = managed.lookup_created(&spec).await.unwrap().unwrap();
                let consumed = case == "recovered_consumed";
                managed
                    .start_owned_retained(
                        &spec,
                        &owned,
                        None,
                        Duration::from_secs(1),
                        1024,
                        |_| async move {
                            Ok(if consumed {
                                StartDirective::ObserveOnly
                            } else {
                                StartDirective::EmitOnce
                            })
                        },
                        |_| async { Ok(()) },
                    )
                    .await
            } else {
                outcome
            };
            if case == "observe_only" {
                let recovered = managed.lookup_created(&spec).await.unwrap().unwrap();
                assert_eq!(
                    recovered.handle.id(),
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                );
                fs::write(root.join("lookup_foreign_labels"), "other-attempt").unwrap();
                assert!(
                    managed.lookup_created(&spec).await.is_err(),
                    "a matching name and spec with foreign attempt labels must not be adopted"
                );
                fs::remove_file(root.join("lookup_foreign_labels")).unwrap();
                fs::write(root.join("inventory_unavailable"), "unavailable").unwrap();
                assert!(
                    managed.lookup_created(&spec).await.is_err(),
                    "an inventory failure must never become absence"
                );
            }
            let log = fs::read_to_string(root.join("log")).unwrap();
            let start_count = log.lines().filter(|l| l.starts_with("start ")).count();
            assert_eq!(
                log.lines()
                    .filter(|line| line.starts_with("create "))
                    .count(),
                1,
                "a recovery must never create another instance"
            );
            if [
                "refused_created",
                "observe_only",
                "create_bad",
                "create_failed",
                "engine_changed",
                "bad_mount",
                "bad_privileged",
                "bad_command",
                "bad_image",
                "bad_tmpfs",
                "bad_log_driver",
                "missing_log_driver",
                "bad_log_options",
                "bad_tty",
                "bad_open_stdin",
                "bad_stdin_once",
                "recovered_consumed",
            ]
            .contains(&case.as_str())
            {
                assert!(outcome.is_err());
                assert_eq!(
                    start_count, 0,
                    "start emitted without a fresh durable created acknowledgement"
                );
            } else if matches!(
                case.as_str(),
                "start_callback_error" | "config_changed" | "logging_changed"
            ) {
                assert!(outcome.is_err());
                assert_eq!(start_count, 1);
                assert_eq!(fs::read_to_string(root.join("state")).unwrap(), "exited");
            } else {
                let retained = outcome.unwrap();
                assert_eq!(start_count, 1);
                assert_eq!(retained.terminal().restart_count, Some(0));
                assert_eq!(retained.result().timed_out, case == "timeout");
                assert!(root.join("state").exists());
                assert!(!log.lines().any(|l| l.starts_with("rm ")));
                fs::write(root.join("terminal_ack"), "durable").unwrap();
                if case == "name_swap" {
                    assert!(
                        managed.cleanup(&retained).await.is_err(),
                        "changed instance metadata requires diagnosis"
                    );
                    assert_eq!(
                        fs::read_to_string(root.join("other_instance")).unwrap(),
                        "running"
                    );
                    assert!(root.join("state").exists());
                } else {
                    managed.cleanup(&retained).await.unwrap();
                    assert!(!root.join("state").exists());
                }
            }
            let log = fs::read_to_string(root.join("log")).unwrap();
            if case == "create_bad" || case == "create_failed" {
                assert!(created_handle.is_none());
                assert!(!log.lines().any(|l| l.starts_with("inspect ")
                    || l.starts_with("kill ")
                    || l.starts_with("rm ")));
            }
            assert!(
                log.lines()
                    .filter(|l| l.starts_with("inspect ")
                        || l.starts_with("kill ")
                        || l.starts_with("rm ")
                        || l.starts_with("start "))
                    .all(|l| l.ends_with(
                        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    )),
                "owned mutations must never follow a reassigned name"
            );
            if outcome_is_expected_retained(&case) {
                assert!(root.join("state").exists());
                assert!(
                    !log.lines().any(|line| line.starts_with("rm ")),
                    "an unacknowledged/replayed attempt must not emit cleanup"
                );
            }
        });
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("managed_fixture.rs");
    fs::write(&source, include_str!("runtime_fixtures/managed.rs")).unwrap();
    let output =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", "--crate-name", "managed_fixture"])
            .arg(source)
            .arg("-o")
            .arg(temp.path().join("docker"))
            .output()
            .unwrap();
    assert!(output.status.success(), "fixture compiler failed");
    for case in [
        "refused_created",
        "observe_only",
        "create_bad",
        "create_failed",
        "start_ok",
        "start_callback_error",
        "timeout",
        "name_swap",
        "engine_changed",
        "bad_mount",
        "bad_privileged",
        "bad_command",
        "bad_image",
        "bad_tmpfs",
        "bad_log_driver",
        "missing_log_driver",
        "bad_log_options",
        "bad_tty",
        "bad_open_stdin",
        "bad_stdin_once",
        "logging_changed",
        "legacy_observe",
        "volume_collision",
        "volume_bind",
        "config_changed",
        "missing_oom",
        "recovered_created",
        "recovered_consumed",
    ] {
        let root = temp.path().join(case);
        fs::create_dir(&root).unwrap();
        write_fixture_metadata(
            &temp.path().join("docker"),
            &[
                ("AI_GUARD_MANAGED_ROOT", root.to_str().unwrap()),
                ("AI_GUARD_MANAGED_CHILD", case),
            ],
        );
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "managed_launch_requires_created_ack_and_keeps_owned_evidence",
                "--nocapture",
            ])
            .env(CHILD, case)
            .env("AI_GUARD_MANAGED_ROOT", root)
            .env("AI_GUARD_RUNTIME_PROGRAM", temp.path().join("docker"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "case {case}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
fn outcome_is_expected_retained(case: &str) -> bool {
    matches!(
        case,
        "refused_created"
            | "observe_only"
            | "create_bad"
            | "start_callback_error"
            | "engine_changed"
            | "bad_mount"
            | "bad_privileged"
            | "bad_command"
            | "bad_image"
            | "bad_tmpfs"
            | "bad_log_driver"
            | "missing_log_driver"
            | "bad_log_options"
            | "bad_tty"
            | "bad_open_stdin"
            | "bad_stdin_once"
            | "logging_changed"
            | "config_changed"
            | "recovered_consumed"
    )
}

#[test]
fn managed_volume_names_are_validated_and_shared_by_helpers_without_session_collision() {
    let session = Uuid::new_v4();
    let spec = ContainerSpec::new(
        session,
        ContainerPurpose::Run,
        "worker:fixture",
        vec!["/usr/bin/id".into()],
        NetworkMode::None,
    )
    .unwrap()
    .with_managed_names("aig-tenant-worker", "aig-tenant-worker-work")
    .unwrap();
    assert_eq!(spec.name(), "aig-tenant-worker");
    assert_eq!(spec.volume(), "aig-tenant-worker-work");
    let args = spec.create_args(RuntimeKind::Docker).unwrap();
    assert!(
        args.iter()
            .any(|arg| arg == "type=volume,source=aig-tenant-worker-work,target=/workspace")
    );
    for invalid in [
        "",
        "../outside",
        "contains,comma",
        "x/y",
        "-option",
        "contains space",
    ] {
        assert!(
            spec.clone()
                .with_managed_names(invalid, "valid-volume")
                .is_err()
        );
        assert!(
            spec.clone()
                .with_managed_names("valid-name", invalid)
                .is_err()
        );
    }
}

#[test]
fn podman_authority_bootstrap_is_explicit_and_marker_loss_remains_unknown() {
    use agentsandbox::runtime::{ManagedRuntime, ManagedRuntimeError};
    use std::fs;
    const CHILD: &str = "AI_GUARD_PODMAN_AUTHORITY_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let root = std::path::PathBuf::from(std::env::var_os("AI_GUARD_MANAGED_ROOT").unwrap());
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let id = Uuid::new_v4();
                let error = ManagedRuntime::connect_podman(id, "local")
                    .await
                    .unwrap_err();
                assert!(matches!(
                    error.downcast_ref::<ManagedRuntimeError>(),
                    Some(ManagedRuntimeError::BootstrapRequired)
                ));
                assert!(
                    !root.join("log").exists(),
                    "connecting must not bootstrap infrastructure"
                );
                fs::write(root.join("marker_create_error"), "once").unwrap();
                let failed = ManagedRuntime::bootstrap_podman_with_runtime(
                    Uuid::new_v4(),
                    "local",
                    fixture_runtime(RuntimeKind::Podman),
                )
                .await
                .unwrap_err();
                let nonce = fs::read_to_string(root.join("marker_nonce")).unwrap();
                let name = fs::read_to_string(root.join("marker_name")).unwrap();
                let stderr = failed
                    .downcast_ref::<agentsandbox::runtime::RuntimeCommandFailure>()
                    .expect("bootstrap preserves its typed CLI error")
                    .redacted_stderr(&agentsandbox::redaction::Redactor::new(vec![]));
                for value in [&nonce, &name] {
                    assert!(
                        !stderr.contains(value.as_str()),
                        "bootstrap diagnostic must hide its private values before binding exists"
                    );
                    assert!(!format!("{failed:#}").contains(value.as_str()));
                    assert!(!format!("{failed:?}").contains(value.as_str()));
                }
                assert!(stderr.contains("private marker") && stderr.contains("[REDACTED]"));
                fs::remove_file(root.join("marker_create_error")).unwrap();
                let managed = ManagedRuntime::bootstrap_podman_with_runtime(
                    id,
                    "local",
                    fixture_runtime(RuntimeKind::Podman),
                )
                .await
                .unwrap();
                assert!(
                    managed.binding().engine_id.is_none(),
                    "Podman has no invented native engine ID"
                );
                let binding = managed.binding().clone();
                let private_nonce = fs::read_to_string(root.join("marker_nonce")).unwrap();
                assert!(
                    !format!("{binding:?}").contains(&private_nonce),
                    "private marker must not escape Debug"
                );
                let reopened = ManagedRuntime::reopen(binding.clone()).await.unwrap();
                assert_eq!(reopened.binding().fingerprint(), binding.fingerprint());
                let mut historical = binding.clone();
                historical.invocation = None;
                let historical_fingerprint = historical.fingerprint();
                let adopted = ManagedRuntime::adopt_invocation(
                    historical,
                    fixture_runtime(RuntimeKind::Podman),
                )
                .await
                .unwrap();
                assert_eq!(adopted.binding().fingerprint(), historical_fingerprint);
                fs::write(root.join("marker_nonce"), Uuid::new_v4().to_string()).unwrap();
                assert!(
                    ManagedRuntime::reopen(binding.clone()).await.is_err(),
                    "a replacement marker must not be adopted"
                );
                fs::write(root.join("marker_missing"), "missing").unwrap();
                assert!(
                    ManagedRuntime::reopen(binding).await.is_err(),
                    "an inaccessible marker must remain unknown"
                );
                let log = fs::read_to_string(root.join("log")).unwrap();
                assert_eq!(
                    log.lines()
                        .filter(|l| l.starts_with("volume create "))
                        .count(),
                    2
                );
                assert!(!log.lines().any(|l| l.starts_with("kill ")
                    || l.starts_with("rm ")
                    || l.starts_with("start ")));
            });
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("managed_fixture.rs");
    fs::write(&source, include_str!("runtime_fixtures/managed.rs")).unwrap();
    let output =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", "--crate-name", "managed_fixture"])
            .arg(source)
            .arg("-o")
            .arg(temp.path().join("podman"))
            .output()
            .unwrap();
    assert!(output.status.success(), "fixture compiler failed");
    let root = temp.path().join("authority");
    fs::create_dir(&root).unwrap();
    write_fixture_metadata(
        &temp.path().join("podman"),
        &[
            ("AI_GUARD_MANAGED_ROOT", root.to_str().unwrap()),
            ("AI_GUARD_MANAGED_CHILD", "podman_authority"),
        ],
    );
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "podman_authority_bootstrap_is_explicit_and_marker_loss_remains_unknown",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("AI_GUARD_MANAGED_CHILD", "podman_authority")
        .env("AI_GUARD_MANAGED_ROOT", root)
        .env("AI_GUARD_RUNTIME_PROGRAM", temp.path().join("podman"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn default_invocation_never_executes_a_project_path_candidate() {
    const CHILD: &str = "AI_GUARD_INVOCATION_PATH_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                match agentsandbox::runtime::Runtime::new(RuntimeKind::Docker) {
                    Ok(runtime) => {
                        runtime
                            .cli(&["--version".into()], std::time::Duration::from_secs(5))
                            .await
                    }
                    Err(error) => Err(error),
                }
            });
        assert!(
            result.is_err()
                || !result
                    .unwrap()
                    .bytes
                    .starts_with(b"PROJECT_RUNTIME_EXECUTED"),
            "the default factory executed a program supplied by the project PATH"
        );
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("hostile.rs");
    std::fs::write(&source, "fn main(){println!(\"PROJECT_RUNTIME_EXECUTED\")}").unwrap();
    let compiled =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .arg(&source)
            .arg("-o")
            .arg(temporary.path().join("docker"))
            .output()
            .unwrap();
    assert!(compiled.status.success());
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "default_invocation_never_executes_a_project_path_candidate",
            "--nocapture",
        ])
        .current_dir(temporary.path())
        .env(CHILD, "1")
        .env("PATH", ".")
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{} {}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
}

fn fixture_runtime(kind: RuntimeKind) -> Runtime {
    Runtime::with_program(
        kind,
        &std::path::PathBuf::from(std::env::var_os("AI_GUARD_RUNTIME_PROGRAM").unwrap()),
    )
    .unwrap()
}
fn write_fixture_metadata(binary: &std::path::Path, entries: &[(&str, &str)]) {
    let directory = binary.with_extension("fixture");
    std::fs::create_dir_all(&directory).unwrap();
    for (key, value) in entries {
        std::fs::write(directory.join(key), value).unwrap();
    }
}

#[test]
fn approved_invocation_clears_ambient_environment_and_uses_control_cwd() {
    const CHILD: &str = "AI_GUARD_INVOCATION_ENV_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let runtime = fixture_runtime(RuntimeKind::Docker);
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let output = executor
            .block_on(runtime.cli(&[], std::time::Duration::from_secs(5)))
            .unwrap();
        let output = String::from_utf8(output.bytes).unwrap();
        assert!(
            output
                .lines()
                .any(|line| line == format!("CWD={}", runtime.invocation().cwd.display()))
        );
        assert!(output.lines().any(|line| line == "LANG=C.UTF-8"));
        assert!(
            output
                .lines()
                .any(|line| line == format!("HOME={}", runtime.invocation().environment["HOME"]))
        );
        assert!(
            !output.contains("AMBIENT_ONLY"),
            "ambient selectors, loaders and arbitrary environment must not be copied"
        );
        for key in [
            "PATH",
            "HOME",
            "LD_PRELOAD",
            "DYLD_LIBRARY_PATH",
            "DOCKER_HOST",
            "CONTAINERS_CONF",
        ] {
            assert!(
                executor
                    .block_on(runtime.cli_env(
                        &[],
                        std::time::Duration::from_secs(5),
                        &[(key, "/AMBIENT_ONLY")]
                    ))
                    .is_err()
            );
        }
        let accepted = executor
            .block_on(runtime.cli_env(
                &[],
                std::time::Duration::from_secs(5),
                &[("AGENTSANDBOX_CONFIG_DIR", "/explicit-control-directory")],
            ))
            .unwrap();
        assert!(
            String::from_utf8(accepted.bytes)
                .unwrap()
                .contains("AGENTSANDBOX_CONFIG_DIR=/explicit-control-directory")
        );
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let program = compile_native_fixture(
        temporary.path(),
        "environment",
        r#"fn main(){println!("CWD={}", std::env::current_dir().unwrap().display());for (key,value) in std::env::vars(){println!("{key}={value}")}}"#,
    );
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "approved_invocation_clears_ambient_environment_and_uses_control_cwd",
            "--nocapture",
        ])
        .current_dir(temporary.path())
        .env(CHILD, "1")
        .env("AI_GUARD_RUNTIME_PROGRAM", program)
        .envs([
            ("HOME", "/AMBIENT_ONLY"),
            ("PATH", ".:/AMBIENT_ONLY"),
            ("DOCKER_CONFIG", "/AMBIENT_ONLY"),
            ("DOCKER_HOST", "unix:///AMBIENT_ONLY"),
            ("CONTAINERS_CONF", "/AMBIENT_ONLY"),
            ("CONTAINER_CONNECTION", "AMBIENT_ONLY"),
            ("LD_PRELOAD", "/AMBIENT_ONLY"),
            ("DYLD_LIBRARY_PATH", "/AMBIENT_ONLY"),
            ("CUSTOM_SECRET", "AMBIENT_ONLY"),
        ])
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{} {}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
}

#[test]
fn approved_program_changes_and_unapproved_permissions_fail_before_execution() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temporary = tempfile::tempdir().unwrap();
    let first = compile_native_fixture(temporary.path(), "first", "fn main(){println!(\"first\")}");
    let second = temporary.path().join("second");
    std::fs::copy(&first, &second).unwrap();
    let entry = temporary.path().join("approved");
    symlink(&first, &entry).unwrap();
    let runtime = Runtime::with_program(RuntimeKind::Docker, &entry).unwrap();
    let persisted = runtime.invocation().clone();
    assert!(Runtime::from_invocation(RuntimeKind::Docker, persisted.clone()).is_ok());
    std::fs::remove_file(&entry).unwrap();
    symlink(&second, &entry).unwrap();
    assert!(
        runtime.command().is_ok(),
        "a retained descriptor keeps the original approved target despite an entry retarget"
    );
    assert!(Runtime::from_invocation(RuntimeKind::Docker, persisted).is_err());
    let runtime = Runtime::with_program(RuntimeKind::Docker, &second).unwrap();
    let replacement = temporary.path().join("replacement");
    std::fs::copy(&second, &replacement).unwrap();
    std::fs::rename(replacement, &second).unwrap();
    assert!(
        runtime.command().is_err(),
        "replacing the inode with identical content must fail before spawn"
    );
    let runtime = Runtime::with_program(RuntimeKind::Docker, &second).unwrap();
    let mut changed = std::fs::read(&second).unwrap();
    let midpoint = changed.len() / 2;
    changed[midpoint] ^= 1;
    std::fs::write(&second, changed).unwrap();
    assert!(
        runtime.command().is_err(),
        "same inode with different content must fail before spawn"
    );
    std::fs::set_permissions(&first, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(Runtime::with_program(RuntimeKind::Docker, &first).is_err());
    assert!(Runtime::with_program(RuntimeKind::Docker, std::path::Path::new("first")).is_err());
}

#[test]
fn non_regular_runtime_program_is_rejected_without_blocking_on_open() {
    use std::os::unix::ffi::OsStrExt;
    const CHILD: &str = "AI_GUARD_INVOCATION_FIFO_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let path = std::path::PathBuf::from(std::env::var_os("AI_GUARD_RUNTIME_PROGRAM").unwrap());
        assert!(Runtime::with_program(RuntimeKind::Docker, &path).is_err());
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let fifo = temporary.path().join("not-a-program");
    let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: path is NUL terminated and valid for the duration of mkfifo.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o700) }, 0);
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "non_regular_runtime_program_is_rejected_without_blocking_on_open",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("AI_GUARD_RUNTIME_PROGRAM", fifo)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("validating a non-regular program blocked before the CLI timeout");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[tokio::test]
async fn old_authority_requires_read_only_adoption_and_keeps_handle_fingerprint() {
    use agentsandbox::runtime::{ManagedRuntime, ManagedRuntimeError};
    let temporary = tempfile::tempdir().unwrap();
    let program = compile_native_fixture(
        temporary.path(),
        "managed",
        include_str!("runtime_fixtures/managed.rs"),
    );
    write_fixture_metadata(
        &program,
        &[
            ("AI_GUARD_MANAGED_ROOT", temporary.path().to_str().unwrap()),
            ("AI_GUARD_MANAGED_CHILD", "start_ok"),
        ],
    );
    let runtime = Runtime::with_program(RuntimeKind::Docker, &program).unwrap();
    let managed = ManagedRuntime::connect_docker_with_runtime(
        Uuid::new_v4(),
        "unix:///fixture/docker.sock",
        runtime.clone(),
    )
    .await
    .unwrap();
    let mut old = managed.binding().clone();
    let engine_fingerprint = old.fingerprint();
    old.invocation = None;
    assert_eq!(old.fingerprint(), engine_fingerprint);
    let error = ManagedRuntime::reopen(old.clone()).await.unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ManagedRuntimeError>(),
        Some(ManagedRuntimeError::InvocationMigrationRequired)
    ));
    std::fs::write(temporary.path().join("log"), "").unwrap();
    let adopted = ManagedRuntime::adopt_invocation(old, runtime.clone())
        .await
        .unwrap();
    assert_eq!(adopted.binding().fingerprint(), engine_fingerprint);
    assert_eq!(
        adopted.binding().invocation_fingerprint(),
        Some(runtime.invocation().fingerprint())
    );
    let log = std::fs::read_to_string(temporary.path().join("log")).unwrap();
    assert_eq!(log.lines().count(), 1);
    assert!(
        log.lines().all(|line| line.starts_with("info ")),
        "adoption can only verify authority, never bootstrap/create/start/kill/remove"
    );
    let mut policy_one = serde_json::to_value(runtime.invocation()).unwrap();
    policy_one["policy_version"] = json!(1);
    policy_one["cwd"] = json!("/");
    policy_one
        .as_object_mut()
        .unwrap()
        .remove("executable_directory");
    let policy_one =
        serde_json::from_value::<agentsandbox::runtime::InvocationBindingV1>(policy_one).unwrap();
    let migration = Runtime::from_invocation(RuntimeKind::Docker, policy_one.clone()).unwrap_err();
    assert!(format!("{migration:#}").contains("policy migration required"));
    let mut historical = adopted.binding().clone();
    historical.invocation = Some(policy_one);
    let migrated = ManagedRuntime::adopt_invocation(historical, runtime.clone())
        .await
        .unwrap();
    assert_eq!(migrated.binding().fingerprint(), engine_fingerprint);
    assert_eq!(
        migrated
            .binding()
            .invocation
            .as_ref()
            .unwrap()
            .policy_version,
        2
    );
    let different_program = temporary.path().join("different");
    std::fs::copy(&program, &different_program).unwrap();
    let different = Runtime::with_program(RuntimeKind::Docker, &different_program).unwrap();
    assert!(
        ManagedRuntime::reopen_with_runtime(adopted.binding().clone(), different)
            .await
            .is_err()
    );
    let mut changed = adopted.binding().invocation.clone().unwrap();
    changed.environment.insert("PATH".into(), "/project".into());
    assert!(Runtime::from_invocation(RuntimeKind::Docker, changed).is_err());
    let mut changed = runtime.invocation().clone();
    changed.cwd = "/project".into();
    assert!(Runtime::from_invocation(RuntimeKind::Docker, changed).is_err());
    let mut changed = runtime.invocation().clone();
    changed.policy_version = 1;
    assert!(Runtime::from_invocation(RuntimeKind::Docker, changed).is_err());
    let mut unknown = serde_json::to_value(runtime.invocation()).unwrap();
    unknown["unknown_field"] = json!(true);
    assert!(serde_json::from_value::<agentsandbox::runtime::InvocationBindingV1>(unknown).is_err());
}

fn compile_native_fixture(
    directory: &std::path::Path,
    name: &str,
    source: &str,
) -> std::path::PathBuf {
    let source_path = directory.join(format!("{name}.rs"));
    let binary = directory.join(name);
    std::fs::write(&source_path, source).unwrap();
    let compiled =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", "--crate-name", name])
            .arg(source_path)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    binary
}

#[tokio::test]
async fn trusted_anchor_executes_approved_leaf_after_parent_is_replaced() {
    let temporary = tempfile::tempdir().unwrap();
    let leaf = temporary.path().join("installed");
    let parked = temporary.path().join("original");
    std::fs::create_dir(&leaf).unwrap();
    let program = compile_native_fixture(
        &leaf,
        "engine",
        r#"fn main(){println!("APPROVED cwd={}",std::env::current_dir().unwrap().display())}"#,
    );
    let substitute = compile_native_fixture(
        temporary.path(),
        "substitute",
        "fn main(){println!(\"SUBSTITUTED\")}",
    );
    let runtime = Runtime::with_program(RuntimeKind::Docker, &program).unwrap();
    let mut command = runtime.command().unwrap();
    std::fs::rename(&leaf, &parked).unwrap();
    std::fs::create_dir(&leaf).unwrap();
    std::fs::copy(substitute, leaf.join("engine")).unwrap();
    drop(runtime);
    let output = command.output().await.unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("APPROVED cwd="),
        "a replaced parent selected an unapproved executable: {stdout}"
    );
    assert!(stdout.contains(parked.canonicalize().unwrap().to_str().unwrap()));
}

#[cfg(target_os = "macos")]
#[test]
fn trusted_anchor_refuses_extended_acl_on_program_or_leaf_directory() {
    let temporary = tempfile::tempdir().unwrap();
    let leaf = temporary.path().join("installed");
    std::fs::create_dir(&leaf).unwrap();
    let program = compile_native_fixture(&leaf, "engine", "fn main(){}");
    assert!(Runtime::with_program(RuntimeKind::Docker, &program).is_ok());
    let status = std::process::Command::new("/bin/chmod")
        .args(["+a", "everyone allow read"])
        .arg(&program)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(
        Runtime::with_program(RuntimeKind::Docker, &program).is_err(),
        "mode bits alone must not approve an extended ACL"
    );
    assert!(
        std::process::Command::new("/bin/chmod")
            .arg("-N")
            .arg(&program)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("/bin/chmod")
            .args(["+a", "everyone allow list"])
            .arg(&leaf)
            .status()
            .unwrap()
            .success()
    );
    assert!(Runtime::with_program(RuntimeKind::Docker, &program).is_err());
}

#[test]
fn trusted_anchor_refuses_oversized_program_before_reading_its_content() {
    use std::os::unix::fs::PermissionsExt;
    const CHILD: &str = "AI_GUARD_INVOCATION_SIZE_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let program =
            std::path::PathBuf::from(std::env::var_os("AI_GUARD_RUNTIME_PROGRAM").unwrap());
        assert!(Runtime::with_program(RuntimeKind::Docker, &program).is_err());
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let program = temporary.path().join("oversized");
    let file = std::fs::File::create(&program).unwrap();
    file.set_len(512 * 1024 * 1024 + 1).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "trusted_anchor_refuses_oversized_program_before_reading_its_content",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("AI_GUARD_RUNTIME_PROGRAM", program)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("oversized executable was read instead of being rejected by its size");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
