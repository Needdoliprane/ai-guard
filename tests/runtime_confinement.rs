//! Real runtime checks, deliberately separate from gateway/audit qualification.
//! Images, session volume, and the source/export evidence directory are retained.
use agentsandbox::{
    runtime::{
        Capture, ContainerPurpose, ContainerSpec, ExecutionResult, NetworkMode, Runtime,
        RuntimeKind,
    },
    workspace::{Limits, extract_workspace, prepare_workspace},
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{fs, path::Path, time::Duration};
use uuid::Uuid;
#[cfg(unix)]
#[path = "runtime_confinement/observation_harness.rs"]
mod observation_harness;
#[cfg(unix)]
use observation_harness::{Qualification, receipt};

#[cfg(unix)]
async fn execute(
    runtime: &Qualification,
    session: Uuid,
    image: &str,
    argv: &[&str],
    duration: Duration,
) -> Result<ExecutionResult> {
    let spec = ContainerSpec::new(
        session,
        ContainerPurpose::Run,
        image,
        argv.iter().map(|arg| (*arg).to_string()).collect(),
        NetworkMode::None,
    )?;
    runtime
        .execute(&spec, None, duration, 64 * 1024, |_| async { Ok(()) })
        .await
}

#[cfg(unix)]
async fn inspect(runtime: &Qualification, name: &str, field: &str) -> Result<Value> {
    let output = runtime
        .cli(
            &[
                "inspect".into(),
                "--format".into(),
                format!("{{{{json .{field}}}}}"),
                name.into(),
            ],
            Duration::from_secs(10),
        )
        .await?;
    ensure!(!output.truncated, "inspect response exceeded its bound");
    Ok(serde_json::from_slice(&output.bytes)?)
}

fn validate_configuration(host: &Value, mounts: &Value, user: &Value, volume: &str) -> Result<()> {
    ensure!(user == "1000:1000", "unexpected worker user");
    ensure!(
        host["ReadonlyRootfs"] == true,
        "worker root must be readonly"
    );
    ensure!(host["Privileged"] == false, "privileged worker refused");
    ensure!(host["NetworkMode"] == "none", "worker network must be none");
    ensure!(
        host["Memory"] == 1024_u64 * 1024 * 1024,
        "unexpected memory limit"
    );
    ensure!(
        host["NanoCpus"] == 2_000_000_000_u64,
        "unexpected CPU limit"
    );
    ensure!(host["PidsLimit"] == 256, "unexpected PID limit");
    let dropped = host["CapDrop"]
        .as_array()
        .context("missing capability configuration")?;
    ensure!(
        dropped.iter().any(|v| v == "ALL"),
        "worker capabilities must be dropped"
    );
    let security = host["SecurityOpt"]
        .as_array()
        .context("missing security options")?;
    ensure!(
        security.iter().any(|v| {
            v.as_str()
                .is_some_and(|s| s.starts_with("no-new-privileges"))
        }),
        "worker requires no-new-privileges"
    );
    let mounts = mounts.as_array().context("missing worker mounts")?;
    ensure!(
        mounts.iter().any(|mount| {
            mount["Type"] == "volume"
                && mount["Name"] == volume
                && mount["Destination"] == "/workspace"
                && mount["RW"] == true
        }),
        "expected workspace volume missing"
    );
    for mount in mounts {
        ensure!(
            mount["Type"] != "bind",
            "worker must not have a host bind mount"
        );
        ensure!(
            matches!(
                mount["Destination"].as_str(),
                Some("/workspace" | "/tmp" | "/home/agent")
            ),
            "unexpected worker mount destination"
        );
    }
    Ok(())
}

fn require_missing_control_path(exit_code: i64, stderr: &Capture) -> Result<()> {
    ensure!(
        exit_code == 2
            && !stderr.truncated
            && stderr.total_bytes == stderr.bytes.len() as u64
            && stderr.bytes
                == b"/bin/ls: cannot access '/run/secrets': No such file or directory\n",
        "absence of /run/secrets not proven by an untruncated ENOENT diagnostic"
    );
    Ok(())
}

#[test]
fn permission_and_io_errors_do_not_prove_control_path_absence() {
    for message in [
        "/bin/ls: cannot access '/run/secrets': Permission denied\n",
        "/bin/ls: cannot access '/run/secrets': Input/output error\n",
    ] {
        let stderr = Capture {
            bytes: message.as_bytes().to_vec(),
            total_bytes: message.len() as u64,
            truncated: false,
        };
        assert!(require_missing_control_path(2, &stderr).is_err());
    }
}

#[test]
fn enoent_diagnostic_must_be_complete() {
    let message = b"/bin/ls: cannot access '/run/secrets': No such file or directory\n";
    let mut stderr = Capture {
        bytes: message.to_vec(),
        total_bytes: message.len() as u64,
        truncated: false,
    };
    assert!(require_missing_control_path(2, &stderr).is_ok());
    stderr.truncated = true;
    assert!(require_missing_control_path(2, &stderr).is_err());
}

#[test]
fn configuration_rejection_does_not_unwind() {
    let rejected = std::panic::catch_unwind(|| {
        validate_configuration(
            &Value::Null,
            &Value::Null,
            &Value::from("0:0"),
            "test-volume",
        )
    });
    assert!(
        rejected.is_ok(),
        "configuration rejection must preserve runtime cleanup"
    );
    assert!(rejected.unwrap().is_err());
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires an explicitly selected real engine and built worker image; retains session volume and evidence"]
async fn real_worker_confinement_copy_export_and_timeout() -> Result<()> {
    let kind = match std::env::var("AGENTSANDBOX_TEST_RUNTIME").as_deref() {
        Ok("docker") => RuntimeKind::Docker,
        Ok("podman") => RuntimeKind::Podman,
        _ => anyhow::bail!("explicitly set AGENTSANDBOX_TEST_RUNTIME=docker or podman"),
    };
    let image = std::env::var("AGENTSANDBOX_TEST_IMAGE")
        .context("set AGENTSANDBOX_TEST_IMAGE to a deliberately selected built image")?;
    let program = std::env::var_os("AGENTSANDBOX_TEST_RUNTIME_PROGRAM")
        .context("set AGENTSANDBOX_TEST_RUNTIME_PROGRAM to the explicitly approved CLI")?;
    let endpoint = std::env::var("AGENTSANDBOX_TEST_RUNTIME_ENDPOINT")
        .context("set AGENTSANDBOX_TEST_RUNTIME_ENDPOINT explicitly")?;
    let evidence = tempfile::Builder::new()
        .prefix("ai-guard-runtime-confinement-")
        .tempdir()?
        .keep();
    eprintln!("retained evidence={}", evidence.display());
    receipt(
        &evidence.join("INCONCLUSIVE.json"),
        &serde_json::json!({"status":"INCONCLUSIVE","scope":"runtime confinement only; no gateway audit or provider"}),
    )?;
    let (sink, collector) =
        agentsandbox::runtime::observation::ObservationCollector::bounded(2048)?;
    let raw = Runtime::with_program(kind, Path::new(&program))?.with_observation(sink);
    let managed = match kind {
        RuntimeKind::Docker => {
            agentsandbox::runtime::ManagedRuntime::connect_docker_with_runtime(
                Uuid::new_v4(),
                &endpoint,
                raw.clone(),
            )
            .await?
        }
        RuntimeKind::Podman => {
            // Reuse a separately admitted authority marker. This test never
            // bootstraps/replaces a Podman marker or removes its volume.
            let path = std::env::var_os("AGENTSANDBOX_TEST_PODMAN_AUTHORITY")
                .context("existing approved Podman authority file required")?;
            let binding: agentsandbox::runtime::RuntimeAuthority =
                serde_json::from_slice(&fs::read(path)?)?;
            ensure!(
                binding.endpoint == endpoint && binding.kind == kind,
                "Podman authority differs from explicit selection"
            );
            agentsandbox::runtime::ManagedRuntime::reopen_with_runtime(binding, raw.clone()).await?
        }
    };
    let runtime = Qualification {
        managed,
        raw,
        evidence: evidence.clone(),
    };
    let info = runtime
        .managed
        .doctor(std::env::var("AGENTSANDBOX_TEST_ALLOW_ROOTFUL").as_deref() == Ok("1"))
        .await?;
    eprintln!(
        "runtime={} rootless={} server={:?}",
        info.runtime, info.rootless, info.server_version
    );
    let (finish, finished) = tokio::sync::oneshot::channel();
    let writer = tokio::spawn(observation_harness::persist(
        collector,
        evidence.join("monotonic.jsonl"),
        finished,
    ));
    let timeout_operation = std::cell::Cell::new(None);
    let observed_session = std::cell::Cell::new(None);
    let outcome = confinement_case(
        &runtime,
        &evidence,
        &image,
        &timeout_operation,
        &observed_session,
    )
    .await;
    let inventory_result: Result<()> = async {
        if let Some(session) = observed_session.get() {
            let inventory = runtime
                .cli(
                    &[
                        "ps".into(),
                        "--all".into(),
                        "--no-trunc".into(),
                        "--filter".into(),
                        format!("label=agentsandbox.session={session}"),
                        "--format".into(),
                        "{{.ID}}".into(),
                    ],
                    Duration::from_secs(10),
                )
                .await;
            match inventory {
                Ok(inventory) => {
                    // These are technical IDs only; preserve unknown/nonterminal
                    // resources without any cleanup, retry or broad stop.
                    let ids = String::from_utf8(inventory.bytes)?;
                    ensure!(
                        ids.lines().all(|id| id.len() == 64
                            && id
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))),
                        "inventory IDs unavailable; INCONCLUSIVE"
                    );
                    receipt(
                        &evidence.join("final-session-inventory.json"),
                        &serde_json::json!({"session":session,"truncated":inventory.truncated,"ids":ids.lines().collect::<Vec<_>>()}),
                    )?;
                    ensure!(
                        !inventory.truncated && ids.trim().is_empty(),
                        "exact session still has resources; retained for diagnosis"
                    );
                }
                Err(error) => return Err(error.context("final inventory unknown; INCONCLUSIVE")),
            }
        }
        Ok(())
    }.await;
    let finish_result = finish
        .send(())
        .map_err(|_| anyhow::anyhow!("diagnostic finish receiver unavailable; INCONCLUSIVE"));
    // Always await the actual consumer before retaining all concurrent outcomes.
    let (consumer_joined, diagnostics) = match writer.await {
        Ok(result) => (true, result),
        Err(error) => (
            false,
            Err(anyhow::Error::new(error).context("diagnostic consumer failed; INCONCLUSIVE")),
        ),
    };
    let timeline = match &diagnostics {
        Ok(events) => match timeout_operation.get() {
            Some(operation) => observation_harness::require_timeout_timeline(events, operation),
            None => Err(anyhow::anyhow!("timeout phase not reached; INCONCLUSIVE")),
        },
        // The actual consumer error is retained below; do not invent events.
        Err(_) => Ok(()),
    };
    let case_receipt = receipt(
        &evidence.join("case-outcome.json"),
        &serde_json::json!({"probe_outcome":if outcome.is_ok() { "COMPLETED" } else { "FAIL" },"inventory_known_empty":inventory_result.is_ok(),"diagnostic_consumer_joined":consumer_joined,"diagnostics_complete":diagnostics.is_ok()}),
    );
    let _events = observation_harness::finish_case(
        outcome,
        inventory_result,
        diagnostics,
        finish_result,
        case_receipt,
        timeline,
    )?;
    receipt(
        &evidence.join("complete.json"),
        &serde_json::json!({"status":"PASS","scope":"runtime confinement only","audit_e2e":"NOT_RUN"}),
    )?;
    Ok(())
}

#[cfg(unix)]
async fn confinement_case(
    runtime: &Qualification,
    evidence: &Path,
    image: &str,
    timeout_operation: &std::cell::Cell<Option<Uuid>>,
    observed_session: &std::cell::Cell<Option<Uuid>>,
) -> Result<()> {
    let source = evidence.join("original");
    fs::create_dir(&source)?;
    fs::write(source.join("original.txt"), "original unchanged")?;
    fs::write(source.join(".env"), "PRIVATE_TEST_VALUE=excluded-fixture")?;
    let prepared = prepare_workspace(&source, Limits::default())?;
    let session = Uuid::new_v4();
    observed_session.set(Some(session));
    let volume = agentsandbox::runtime::volume_name(session);
    eprintln!(
        "retained evidence={} session={} volume={}",
        evidence.display(),
        session,
        volume
    );
    receipt(
        &evidence.join("session.json"),
        &serde_json::json!({"session":session,"volume":volume,"retained":true}),
    )?;
    runtime.create_volume(session).await?;
    let import = ContainerSpec::new(
        session,
        ContainerPurpose::Import,
        &image,
        [
            "/bin/tar",
            "--extract",
            "--file=-",
            "--directory=/workspace",
            "--no-same-owner",
            "--no-same-permissions",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
        NetworkMode::None,
    )?;
    let imported = runtime
        .execute(
            &import,
            Some(prepared.archive.reopen()?),
            Duration::from_secs(120),
            16 * 1024,
            |_| async { Ok(()) },
        )
        .await?;
    assert_eq!(imported.state.exit_code, 0, "workspace import failed");
    let probe = ContainerSpec::new(
        session,
        ContainerPurpose::Run,
        &image,
        vec!["/bin/cat".into(), "/proc/self/status".into()],
        NetworkMode::None,
    )?;
    let expected_volume = volume.clone();
    let observed = runtime
        .execute(
            &probe,
            None,
            Duration::from_secs(20),
            32 * 1024,
            |observed| {
                let id = observed.id.clone();
                let volume = expected_volume.clone();
                let runtime = &runtime;
                async move {
                    let id = id.context("native instance ID missing")?;
                    let host = inspect(runtime, &id, "HostConfig").await?;
                    let mounts = inspect(runtime, &id, "Mounts").await?;
                    let user = inspect(runtime, &id, "Config.User").await?;
                    validate_configuration(&host, &mounts, &user, &volume)
                }
            },
        )
        .await?;
    assert_eq!(observed.state.exit_code, 0);
    let status = String::from_utf8(observed.stdout.bytes)?;
    for (field, expected) in [
        ("Uid:", "1000\t1000\t1000\t1000"),
        ("Gid:", "1000\t1000\t1000\t1000"),
        ("CapEff:", "0000000000000000"),
        ("CapPrm:", "0000000000000000"),
        ("CapBnd:", "0000000000000000"),
        ("NoNewPrivs:", "1"),
        ("Seccomp:", "2"),
    ] {
        assert_eq!(
            status
                .lines()
                .find_map(|line| line.strip_prefix(field))
                .map(str::trim),
            Some(expected),
            "unexpected {field}"
        );
    }
    eprintln!("PASS inspected restrictions and process UID/GID/capabilities/NNP/seccomp");
    // Exercise a rejected post-start check through the Result path. Reuse this
    // test's volume; only this operation's container may be stopped or removed.
    let rejected_probe = ContainerSpec::new(
        session,
        ContainerPurpose::Run,
        &image,
        vec!["/bin/sleep".into(), "60".into()],
        NetworkMode::None,
    )?;
    let rejected_name = rejected_probe.name();
    let rejected = runtime
        .execute(
            &rejected_probe,
            None,
            Duration::from_secs(20),
            1024,
            |observed| {
                let id = observed.id.clone();
                let wrong_volume = format!("{volume}-intentional-mismatch");
                let runtime = &runtime;
                async move {
                    let id = id.context("native instance ID missing")?;
                    let host = inspect(runtime, &id, "HostConfig").await?;
                    let mounts = inspect(runtime, &id, "Mounts").await?;
                    let user = inspect(runtime, &id, "Config.User").await?;
                    validate_configuration(&host, &mounts, &user, &wrong_volume)
                }
            },
        )
        .await;
    assert!(
        rejected
            .expect_err("deliberately mismatched volume must reject the probe")
            .to_string()
            .contains("expected workspace volume missing")
    );
    let remaining = runtime
        .cli(
            &[
                "ps".into(),
                "--all".into(),
                "--filter".into(),
                format!("name={rejected_name}"),
                "--format".into(),
                "{{.Names}}".into(),
            ],
            Duration::from_secs(10),
        )
        .await?;
    assert!(!remaining.truncated);
    assert!(
        remaining.bytes.is_empty(),
        "rejected probe container was not removed"
    );
    eprintln!("PASS rejected on_started check returns an error and removes only its container");
    let limits = execute(
        &runtime,
        session,
        &image,
        &[
            "/bin/cat",
            "/sys/fs/cgroup/memory.max",
            "/sys/fs/cgroup/pids.max",
            "/sys/fs/cgroup/cpu.max",
        ],
        Duration::from_secs(20),
    )
    .await?;
    assert_eq!(
        limits.state.exit_code, 0,
        "this qualification requires cgroup v2 evidence"
    );
    assert_eq!(
        String::from_utf8(limits.stdout.bytes)?,
        "1073741824\n256\n200000 100000\n"
    );
    eprintln!("PASS cgroup v2 memory/pid/cpu allocation limits");
    let readonly = execute(
        &runtime,
        session,
        &image,
        &["/usr/bin/touch", "/readonly-probe"],
        Duration::from_secs(20),
    )
    .await?;
    assert_ne!(readonly.state.exit_code, 0);
    assert!(String::from_utf8(readonly.stderr.bytes)?.contains("Read-only file system"));
    let secrets = execute(
        &runtime,
        session,
        &image,
        &["/bin/ls", "-ld", "/run/secrets"],
        Duration::from_secs(20),
    )
    .await?;
    require_missing_control_path(secrets.state.exit_code, &secrets.stderr)?;
    // A network-less namespace may still contain unconfigured kernel tunnel
    // devices. Routes and failed connection attempts are the relevant effects.
    let routes = execute(
        &runtime,
        session,
        &image,
        &["/bin/cat", "/proc/net/route"],
        Duration::from_secs(20),
    )
    .await?;
    assert_eq!(routes.state.exit_code, 0);
    assert_eq!(String::from_utf8(routes.stdout.bytes)?.lines().count(), 1);
    let ipv6_routes = execute(
        &runtime,
        session,
        &image,
        &["/bin/cat", "/proc/net/ipv6_route"],
        Duration::from_secs(20),
    )
    .await?;
    assert_eq!(ipv6_routes.state.exit_code, 0);
    assert!(
        String::from_utf8(ipv6_routes.stdout.bytes)?
            .lines()
            .all(|line| { line.split_whitespace().last() == Some("lo") })
    );
    let network = execute(
        &runtime,
        session,
        &image,
        &[
            "/usr/bin/curl",
            "--noproxy",
            "*",
            "--connect-timeout",
            "1",
            "--max-time",
            "2",
            "http://192.0.2.1",
        ],
        Duration::from_secs(10),
    )
    .await?;
    assert_eq!(network.state.exit_code, 7, "curl must fail to connect");
    eprintln!(
        "PASS readonly root, /run/secrets ENOENT, absent egress routes and failed connection"
    );
    let changed = execute(
        &runtime,
        session,
        &image,
        &["/usr/bin/touch", "/workspace/generated.txt"],
        Duration::from_secs(20),
    )
    .await?;
    assert_eq!(changed.state.exit_code, 0);
    let started = std::time::Instant::now();
    let timeout_spec = ContainerSpec::new(
        session,
        ContainerPurpose::Run,
        &image,
        vec!["/bin/sleep".into(), "60".into()],
        NetworkMode::None,
    )?;
    timeout_operation.set(Some(timeout_spec.operation_id()));
    let timeout = runtime
        .execute(
            &timeout_spec,
            None,
            Duration::from_secs(1),
            64 * 1024,
            |_| async { Ok(()) },
        )
        .await?;
    assert!(timeout.timed_out);
    assert_ne!(timeout.state.exit_code, 0);
    ensure!(
        !timeout.stdout.truncated && !timeout.stderr.truncated,
        "timeout captures truncated; INCONCLUSIVE"
    );
    let whole_call_elapsed = started.elapsed();
    receipt(
        &evidence.join("timeout-whole-call.json"),
        &serde_json::json!({"operation":timeout_spec.operation_id(),"elapsed_ns":whole_call_elapsed.as_nanos(),"worker_timeout_seconds":1,"whole_call_guard_seconds":30}),
    )?;
    ensure!(
        whole_call_elapsed < Duration::from_secs(30),
        "worker did not stop promptly at the broker timeout"
    );
    eprintln!("PASS broker timeout stops its worker; volume retained");
    let export = ContainerSpec::new(
        session,
        ContainerPurpose::Export,
        &image,
        [
            "/bin/tar",
            "--create",
            "--file=-",
            "--directory=/workspace",
            ".",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
        NetworkMode::None,
    )?;
    let exported = runtime
        .execute(
            &export,
            None,
            Duration::from_secs(120),
            Limits::default().max_archive_bytes(),
            |_| async { Ok(()) },
        )
        .await?;
    assert_eq!(exported.state.exit_code, 0);
    assert!(!exported.stdout.truncated);
    let destination = evidence.join("export");
    let manifest = extract_workspace(
        exported.stdout.bytes.as_slice(),
        &destination,
        &source,
        Limits::default(),
    )?;
    assert!(manifest.contains_key("generated.txt"));
    assert!(!manifest.contains_key(".env"));
    assert!(destination.join("generated.txt").is_file());
    assert!(!source.join("generated.txt").exists());
    assert_eq!(
        fs::read_to_string(source.join("original.txt"))?,
        "original unchanged"
    );
    assert_eq!(
        fs::read_to_string(destination.join("original.txt"))?,
        "original unchanged"
    );
    assert!(!Path::new(&destination).join(".env").exists());
    eprintln!("PASS filtered copy, isolated mutation and fresh export; audit E2E NOT_RUN");
    Ok(())
}
