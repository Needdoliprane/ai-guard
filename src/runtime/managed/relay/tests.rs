use super::*;
use crate::runtime::{ObservedAllocation, ProcessObservation};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn relay_requires_retained_profile_and_pinned_invocation_without_a_start_permit_or_stop_dependency()
{
    use crate::control_fs::Directory;
    use std::{fs, os::unix::fs::PermissionsExt};

    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = Directory::open_existing(&temp.path().canonicalize().unwrap()).unwrap();
    // Approve and pin this Rust executable for construction only. It is never
    // invoked as Docker; this cannot qualify an engine or a transport dialect.
    let runtime =
        Runtime::with_program(RuntimeKind::Docker, &std::env::current_exe().unwrap()).unwrap();
    let binding = DockerLaunchProfileBindingV1::new(
        &"1".repeat(64),
        Uuid::from_u128(1),
        &"2".repeat(64),
        "unix:///not-opened/relay-docker.sock",
        &runtime.invocation().fingerprint(),
    )
    .unwrap();
    let retained = RetainedDockerLaunchProfile::create(&root, &binding).unwrap();
    let mut managed = ManagedRuntime {
        binding: RuntimeAuthority {
            authority_id: Uuid::from_u128(2),
            kind: RuntimeKind::Docker,
            endpoint: binding.runtime_endpoint.clone(),
            engine_id: Some("software-only-engine".into()),
            invocation: Some(runtime.invocation().clone()),
            docker_launch_profile: Some(binding.clone()),
            podman: None,
        },
        runtime,
        launch_profile: Some(retained.clone()),
    };
    let (mut command, held) = managed.relay_command().unwrap();
    assert!(held.is_some());
    command.args(relay_exec_args(RuntimeKind::Docker, &"a".repeat(64)).unwrap());
    let args: Vec<_> = command
        .as_std()
        .get_args()
        .map(|arg| arg.to_os_string())
        .collect();
    assert_eq!(
        &args[..4],
        [
            std::ffi::OsString::from("--host"),
            binding.runtime_endpoint.clone().into(),
            "--config".into(),
            retained.directory_path().as_os_str().to_owned(),
        ]
    );
    assert_eq!(args[4], "exec");
    assert!(!args.iter().any(|arg| arg == "start"));
    assert!(command.as_std().get_envs().all(|(name, value)| {
        ![
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "ANTHROPIC_API_KEY",
            "AI_GUARD_PROVIDER_BOOTSTRAP",
        ]
        .iter()
        .any(|key| name == *key)
            || value.is_none()
    }));

    managed.binding.invocation.as_mut().unwrap().policy_version = 1;
    assert!(
        managed.relay_command().is_err(),
        "changed invocation must fail before spawn"
    );
    managed.binding.invocation = Some(managed.runtime.invocation().clone());
    fs::remove_file(
        retained
            .directory_path()
            .join(crate::runtime::launch_profile::FILE),
    )
    .unwrap();
    assert!(
        managed.relay_command().is_err(),
        "the exact retained profile must remain present"
    );
    let mut stop = managed.command().unwrap();
    stop.args(["stop", &"a".repeat(64)]);
    assert!(!stop.as_std().get_args().any(|arg| arg == "--config"));
    managed.launch_profile = None;
    assert!(
        managed.relay_command().is_err(),
        "no implicit profile reconstruction"
    );
}

fn owned() -> OwnedContainerSnapshot {
    let started = chrono::DateTime::parse_from_rfc3339("2026-11-07T10:00:00Z").unwrap();
    OwnedContainerSnapshot {
        handle: ContainerHandle {
            authority_fingerprint: "c".repeat(64),
            session: Uuid::from_u128(1),
            operation: Uuid::from_u128(2),
            id: "a".repeat(64),
            name: "aig-relay-fixture".into(),
            configuration_fingerprint_version: Some(2),
        },
        observation: ContainerObservation {
            id: Some("a".repeat(64)),
            name: "aig-relay-fixture".into(),
            image_id: Some(format!("sha256:{}", "b".repeat(64))),
            restart_count: Some(0),
            process: ProcessObservation {
                status: Some("running".into()),
                running: Some(true),
                started_at: Some(started),
                finished_at: None,
                exit_code: None,
                oom_killed: Some(false),
            },
            allocation: serde_json::from_value::<ObservedAllocation>(json!({
                "cpu": {"quota":200000, "period":100000},
                "memory_bytes": 1073741824_u64,
                "pids_limit":256
            }))
            .unwrap(),
            configuration_fingerprint: Some("d".repeat(64)),
            configuration_fingerprint_version: Some(2),
            observed_at: chrono::Utc::now(),
            diagnostics: vec![],
        },
    }
}

#[test]
fn docker_and_podman_exec_arguments_are_closed_and_use_the_exact_cid() {
    let cid = "a".repeat(64);
    let docker = relay_exec_args(RuntimeKind::Docker, &cid).unwrap();
    let mut expected = vec!["exec", "--interactive", "--tty=false", "--privileged=false"];
    expected.extend([
        "--env=AI_GUARD_PROVIDER_BOOTSTRAP=",
        "--env=OPENAI_API_KEY=",
        "--env=CODEX_API_KEY=",
        "--env=ANTHROPIC_API_KEY=",
        "--env=CLAUDE_CODE_OAUTH_TOKEN=",
        "--env=CODEX_HOME=",
        "--env=CLAUDE_CONFIG_DIR=",
    ]);
    expected.extend([
        "--user",
        "1000:1000",
        &cid,
        "/usr/local/bin/ai-guard-relay",
        "serve-v1",
    ]);
    assert_eq!(docker, expected);
    let podman = relay_exec_args(RuntimeKind::Podman, &cid).unwrap();
    let mut podman_expected = expected.clone();
    podman_expected.insert(4, "--detach-keys=");
    assert_eq!(podman, podman_expected);
    for invalid in [
        "",
        "worker",
        "1234",
        "--privileged",
        &"a".repeat(63),
        &"a".repeat(65),
    ] {
        assert_eq!(
            relay_exec_args(RuntimeKind::Docker, invalid),
            Err(RelayExecError::Identity)
        );
    }
    for args in [docker, podman] {
        assert!(!args.iter().any(|arg| {
            [
                "--env",
                "--env-file",
                "--privileged",
                "--detach",
                "--cidfile",
                "--latest",
                "--preserve-fds",
                "sh",
                "-c",
                "start",
            ]
            .contains(&arg.as_str())
        }));
    }
}

#[test]
fn relay_revalidation_requires_same_running_incarnation_and_complete_configuration() {
    let baseline = owned();
    validate_running_snapshot(&baseline, &baseline.observation).unwrap();
    let original = serde_json::to_value(&baseline.observation).unwrap();
    for (field, value) in [
        ("id", json!("e".repeat(64))),
        ("image_id", json!(format!("sha256:{}", "e".repeat(64)))),
        ("restart_count", json!(1)),
        ("configuration_fingerprint", json!(null)),
        ("configuration_fingerprint_version", json!(1)),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        let observed: ContainerObservation = serde_json::from_value(changed).unwrap();
        assert_eq!(
            validate_running_snapshot(&baseline, &observed),
            Err(RelayExecError::Identity)
        );
    }
    for (field, value) in [
        ("status", json!("paused")),
        ("running", json!(false)),
        ("started_at", json!("2026-11-07T10:00:01Z")),
        ("started_at", json!(null)),
        ("finished_at", json!("2026-11-07T10:00:01Z")),
    ] {
        let mut changed = original.clone();
        changed["process"][field] = value;
        let observed: ContainerObservation = serde_json::from_value(changed).unwrap();
        assert_eq!(
            validate_running_snapshot(&baseline, &observed),
            Err(RelayExecError::Identity)
        );
    }
    let mut invalid = baseline.clone();
    invalid.handle.id = "b".repeat(64);
    assert_eq!(
        validate_running_snapshot(&invalid, &baseline.observation),
        Err(RelayExecError::Identity)
    );
    invalid = baseline.clone();
    invalid.observation.configuration_fingerprint = None;
    assert_eq!(
        validate_running_snapshot(&invalid, &invalid.observation),
        Err(RelayExecError::Identity)
    );
}

#[test]
fn inherited_environment_is_closed_instead_of_assumed_clean() {
    let safe = [
        "HOME=/home/agent",
        "RUSTUP_HOME=/usr/local/rustup",
        "CARGO_HOME=/home/agent/.cargo",
        "PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        "LANG=C.UTF-8",
    ];
    let input =
        |env: serde_json::Value| serde_json::to_vec(&json!({"Config":{"Env":env}})).unwrap();
    validate_helper_environment(&input(json!(safe))).unwrap();
    validate_helper_environment(&input(json!([
        "NODE_VERSION=24.0.0",
        "YARN_VERSION=1.22.22",
        "AI_GUARD_PROVIDER_BOOTSTRAP=secret-canary",
        "CODEX_API_KEY=secret-canary"
    ])))
    .unwrap();
    // Reject the byte length before collecting dot-delimited components. This
    // synthetic value exercises the closed parser without runtime or secrets.
    let oversized = format!("24.{}1", ".".repeat(128 * 1024));
    assert!(!short_version(&oversized, Some("24")));
    assert_eq!(
        validate_helper_environment(&input(json!([format!("NODE_VERSION={oversized}")]))),
        Err(RelayExecError::Profile)
    );
    for env in [
        json!(null),
        json!(["HOME=/tmp"]),
        json!(["PRIVATE=secret-canary"]),
        json!(["HOME=/home/agent", "HOME=/home/agent"]),
        json!(["NODE_VERSION=25.0.0"]),
        json!(["NODE_VERSION=24.secret.canary"]),
        json!(["NODE_VERSION=24.00.1"]),
        json!(["YARN_VERSION=1.22.22-secret"]),
        json!(["OPENAI_API_KEY=first", "OPENAI_API_KEY=second"]),
    ] {
        let error = validate_helper_environment(&input(env)).unwrap_err();
        assert_eq!(error, RelayExecError::Profile);
        assert!(!format!("{error:?} {error}").contains("secret-canary"));
    }
}

fn qa_command(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "runtime::managed::relay::tests::stdio_child",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env("AI_GUARD_RELAY_STDIO_QA_CASE", mode)
        .kill_on_drop(true);
    command
}

async fn ready_fixture(transport: &mut RelayExecTransport) {
    // libtest emits its own bounded prefix before the internal child function.
    // Wait for the child's marker before sending its private test handshake;
    // no sleep or assumed scheduler ordering stands in for child readiness.
    let (mut input, mut output) = transport.io_pair().unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut seen = Vec::new();
        while !seen.ends_with(b"QA_READY\n") {
            assert!(seen.len() < 4096);
            let mut byte = [0u8; 1];
            input.read_exact(&mut byte).await.unwrap();
            seen.push(byte[0]);
        }
        output.write_all(b"!").await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn borrowed_stdio_is_real_and_join_does_not_manufacture_a_helper_or_worker_terminal() {
    let mut transport = RelayExecTransport::spawn(qa_command("echo"), owned(), None).unwrap();
    ready_fixture(&mut transport).await;
    let (mut input, mut output) = transport.io_pair().unwrap();
    output.write_all(b"relay-raw\x10\x11\n").await.unwrap();
    output.shutdown().await.unwrap();
    output.shutdown().await.unwrap(); // Closing the same owned pipe is idempotent.
    assert_eq!(
        output.write_all(b"not-forwarded").await.unwrap_err().kind(),
        std::io::ErrorKind::BrokenPipe
    );
    assert_eq!(
        output.flush().await.unwrap_err().kind(),
        std::io::ErrorKind::BrokenPipe
    );
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), input.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(
        bytes
            .windows(b"relay-raw\x10\x11\n".len())
            .any(|part| part == b"relay-raw\x10\x11\n")
    );
    assert!(
        !bytes
            .windows(b"not-forwarded".len())
            .any(|part| part == b"not-forwarded")
    );
    drop((input, output));
    assert!(
        transport.stdin.is_none(),
        "writer shutdown must close the owned pipe to send EOF"
    );
    let report = transport.close_and_join().await;
    assert!(report.local_joined);
    assert_eq!(report.cli_exit_code, Some(0));
    assert_eq!(transport.state(), RelayExecState::OutcomeUnknown);
    assert!(transport.io_pair().is_err());
    assert_eq!(transport.snapshot.observation.process.running, Some(true));
}

#[tokio::test]
async fn stderr_is_bounded_opaque_and_poisoned_without_becoming_stdout() {
    let mut transport = RelayExecTransport::spawn(qa_command("stderr"), owned(), None).unwrap();
    ready_fixture(&mut transport).await;
    let report = transport.close_and_join().await;
    assert!(report.local_joined);
    let stderr = transport.stderr_summary.as_ref().unwrap();
    assert!(stderr.exceeded);
    assert_eq!(stderr.observed_bytes, STDERR_LIMIT);
    assert!(!format!("{transport:?} {report:?} {stderr:?}").contains("secret-stderr-canary"));
    assert_eq!(transport.state(), RelayExecState::OutcomeUnknown);
}

#[tokio::test]
async fn closing_an_unresponsive_cli_kills_and_joins_only_that_cli() {
    let mut transport = RelayExecTransport::spawn(qa_command("wait"), owned(), None).unwrap();
    ready_fixture(&mut transport).await;
    {
        let (mut input, _output) = transport.io_pair().unwrap();
        let mut byte = [0u8; 1];
        assert!(
            tokio::time::timeout(Duration::from_millis(10), input.read(&mut byte))
                .await
                .is_err()
        );
        // Cancelling this serving read drops only its borrows, not the child or
        // stderr owner. Explicit bounded close still owns and joins that CLI.
    }
    let began = Instant::now();
    let report = transport.close_and_join().await;
    assert!(report.local_joined);
    assert!(report.cli_kill_requested);
    assert!(began.elapsed() < CLOSE_TIME);
    assert_eq!(transport.state(), RelayExecState::OutcomeUnknown);
    assert_eq!(transport.snapshot.observation.process.running, Some(true));
}

/// Internal child of these tests; this is not a runtime/provider qualification.
#[test]
#[ignore]
fn stdio_child() {
    use std::io::{Read, Write};
    std::io::stdout().write_all(b"QA_READY\n").unwrap();
    std::io::stdout().flush().unwrap();
    let mut handshake = [0u8; 1];
    std::io::stdin().read_exact(&mut handshake).unwrap();
    assert_eq!(handshake, [b'!']);
    match std::env::var("AI_GUARD_RELAY_STDIO_QA_CASE")
        .unwrap()
        .as_str()
    {
        "echo" => {
            let mut bytes = Vec::new();
            std::io::stdin().read_to_end(&mut bytes).unwrap();
            std::io::stdout().write_all(&bytes).unwrap();
        }
        "stderr" => {
            let chunk = [b'x'; 1024];
            let _ = std::io::stderr().write_all(b"secret-stderr-canary");
            for _ in 0..20 {
                if std::io::stderr().write_all(&chunk).is_err() {
                    break;
                }
            }
        }
        "wait" => std::thread::sleep(Duration::from_secs(30)),
        _ => panic!("unknown software QA child mode"),
    }
}
