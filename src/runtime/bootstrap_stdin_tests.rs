use super::{
    CancelGeneration, ContainerPurpose, ContainerSpec, EGRESS_PROXY_MARKER, HELD_CHALLENGE_ENV,
    NetworkMode, OwnedStartInput, RuntimeKind, SpawnedPrearmedInput,
    batch_io::OwnedBatchIo,
    bootstrap_stdin::{
        BootstrapStdinSubmitError, MAX_BOOTSTRAP_STDIN_BYTES, PrearmedBootstrapStdin,
        PreparedBootstrapStdin,
    },
    managed::{validate_bootstrap_environment, validate_bootstrap_stdin_flags},
    spawn_prearmed_cli,
};
use crate::{
    broker::journal::retirement_live_fixture::live_fixture,
    control::{EgressLaunchV1, LaunchSpecV1, NetworkV1, SecurityPresetV1, WorkerV1},
    provider::{BOOTSTRAP_ENV, BOOTSTRAP_PROGRAM},
};
use anyhow::Result;
use serde_json::json;
use std::{future::Future, pin::Pin, process::Stdio, time::Duration};
use tokio::{
    process::{Child, Command},
    sync::watch,
    time::{Instant, timeout},
};
use uuid::Uuid;

const TARGET: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ATTEMPT: &str = "550e8400-e29b-41d4-a716-446655440000";

fn held_worker() -> WorkerV1 {
    let launch: LaunchSpecV1 = serde_json::from_value(json!({
        "version": 1,
        "session_id": Uuid::from_u128(1),
        "command": {
            "argv": [BOOTSTRAP_PROGRAM, "exec", "--client", "codex", "--auth-mode", "api",
                     "--transport", "batch", "--preset-version", "1", "--prompt", "fixture"],
            "cwd": "/workspace"
        },
        "profile": "observe",
        "runtime_kind": "docker",
        "authority_id": Uuid::from_u128(7),
        "image_ref": "worker:fixture",
        "image_id": format!("sha256:{TARGET}"),
        "network": "none",
        "cpu_millis": 2000,
        "memory_bytes": 1_073_741_824_u64,
        "pids_limit": 256,
        "timeout_seconds": 120,
        "security_preset": "fixed_sandbox_v2",
        "restart_policy": "no",
        "workspace_manifest_digest": TARGET,
        "workspace_import_policy_version": 1,
        "provider": "codex",
        "capture_output": true,
        "credential_binding_id": Uuid::from_u128(2),
        "credential_names": [BOOTSTRAP_ENV],
        "provider_resolution": {
            "version": 1, "client": "codex", "auth_mode": "api", "transport": "batch",
            "profile_id": "codex-api", "profile_revision": 1,
            "credential_binding_id": Uuid::from_u128(2), "credential_names": [BOOTSTRAP_ENV],
            "invocation_preset_version": 1,
            "provenance": {"client": "cli", "auth_mode": "cli", "profile": "cli", "model": "cli"}
        },
        "egress": EgressLaunchV1::broker_proxy(3128).unwrap(),
    }))
    .unwrap();
    serde_json::from_value(json!({
        "worker_id": Uuid::from_u128(3),
        "admission_operation_id": Uuid::from_u128(4),
        "request_id": Uuid::from_u128(5),
        "receipt": {
            "version": 1, "request_id": Uuid::from_u128(5), "request_hash": TARGET,
            "phase": "initial", "audit_seq": 1, "audit_hash": TARGET
        },
        "spec_hash": launch.spec_hash().unwrap(),
        "launch": launch,
        "policy_hash": TARGET,
        "state": "reserved",
        "quota_held": true,
        "revision": 1,
        "attempt": {
            "attempt_id": ATTEMPT, "stable_name": "aig-held-fixture",
            "volume_name": "aig-held-fixture-work"
        },
        "allocation_inconsistent": false,
    }))
    .unwrap()
}

fn inert_cli(expected_args: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.env_clear().stdin(Stdio::null()).args([
        "-c",
        r#"expected="$1"; shift
           [ "$*" = "$expected" ] || exit 94
           exec /bin/cat"#,
        "fixture",
        expected_args,
    ]);
    command
}

fn unused_handoff<'a>(
    _: PreparedBootstrapStdin,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async { anyhow::bail!("spawn fixture must not call the handoff") })
}

fn bootstrap_child() -> (Child, PrearmedBootstrapStdin) {
    let (_stop_tx, stop) = watch::channel(false);
    let spec = ContainerSpec::for_held_worker(&held_worker()).unwrap();
    let binding = spec.held_bootstrap_for(true).unwrap().unwrap().clone();
    let fixture = live_fixture();
    let target = fixture.registration.prospective().unwrap();
    let cid = target.owned().handle.id();
    let (child, input) = spawn_prearmed_cli(
        inert_cli(&format!("start --attach --interactive {cid}")),
        target.authority(),
        target.owned(),
        OwnedStartInput::Bootstrap {
            ready: Box::new(unused_handoff),
            binding,
        },
        |authority, created| {
            anyhow::ensure!(
                fixture
                    .registration
                    .matches_spawn_target(authority, created)
            );
            Ok(())
        },
        &stop,
        None,
    )
    .unwrap();
    assert!(child.stdin.is_none(), "only the bootstrap owner can write");
    let SpawnedPrearmedInput::Bootstrap { stdin, .. } = input else {
        panic!("bootstrap spawn returned the historical input");
    };
    (child, stdin.unwrap())
}

#[tokio::test]
async fn exclusive_bootstrap_stdin_releases_or_closes_the_exact_pipe_once() {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Case {
        Submit,
        Stop,
        Veto,
        Oversize,
        Abandon,
    }

    let frame = b"local inert fixture\n";
    for case in [
        Case::Submit,
        Case::Stop,
        Case::Veto,
        Case::Oversize,
        Case::Abandon,
    ] {
        let (child, stdin) = bootstrap_child();
        let mut io = OwnedBatchIo::own(child, None, 256);
        stdin
            .wait_writable(Instant::now() + Duration::from_secs(5))
            .await
            .unwrap();
        let generation = CancelGeneration::new();
        let expected = generation.current();
        if case == Case::Stop {
            generation.invalidate();
        }
        if case == Case::Abandon {
            drop(stdin);
        } else {
            let oversize = [0; MAX_BOOTSTRAP_STDIN_BYTES + 1];
            let bytes = if case == Case::Oversize {
                oversize.as_slice()
            } else {
                frame.as_slice()
            };
            let result = stdin.submit_once(bytes, &generation, expected, || {
                assert!(matches!(case, Case::Submit | Case::Veto));
                anyhow::ensure!(case != Case::Veto, "fixture release veto");
                Ok(())
            });
            assert_eq!(
                result,
                if case == Case::Submit {
                    Ok(())
                } else {
                    Err(BootstrapStdinSubmitError::Refused)
                },
                "{case:?}"
            );
        }
        let result = timeout(Duration::from_secs(5), io.collect())
            .await
            .expect("dropping the only stdin owner must deliver EOF")
            .unwrap();
        assert!(result.status.unwrap().success(), "{result:?}");
        assert!(result.errors.is_empty(), "{result:?}");
        assert!(result.stdout_eof);
        assert_eq!(
            result.stdout.bytes.as_slice(),
            if case == Case::Submit {
                frame.as_slice()
            } else {
                &[]
            },
            "{case:?}"
        );
    }

    let (mut child, stdin) = bootstrap_child();
    stdin
        .wait_writable(Instant::now() + Duration::from_secs(5))
        .await
        .unwrap();
    timeout(Duration::from_secs(5), child.kill())
        .await
        .unwrap()
        .unwrap();
    let generation = CancelGeneration::new();
    assert_eq!(
        stdin.submit_once(frame, &generation, generation.current(), || Ok(())),
        Err(BootstrapStdinSubmitError::DeliveryUnknown)
    );
}

#[tokio::test]
async fn prearmed_spawn_preserves_historical_stdin_and_rejects_wrong_engine_flags() {
    let (_stop_tx, stop) = watch::channel(false);
    let fixture = live_fixture();
    let target = fixture.registration.prospective().unwrap();
    let cid = target.owned().handle.id();
    for archive in [None, Some(tempfile::tempfile().unwrap())] {
        let expected_args = if archive.is_some() {
            format!("start --attach --interactive {cid}")
        } else {
            format!("start --attach {cid}")
        };
        let has_archive = archive.is_some();
        let (child, input) = spawn_prearmed_cli(
            inert_cli(&expected_args),
            target.authority(),
            target.owned(),
            OwnedStartInput::Historical(archive),
            |authority, created| {
                anyhow::ensure!(
                    fixture
                        .registration
                        .matches_spawn_target(authority, created)
                );
                Ok(())
            },
            &stop,
            None,
        )
        .unwrap();
        let SpawnedPrearmedInput::Historical(archive) = input else {
            panic!("historical spawn returned a bootstrap pipe");
        };
        assert_eq!(child.stdin.is_some(), has_archive);
        let mut io = OwnedBatchIo::own(child, archive, 256);
        let result = timeout(Duration::from_secs(5), io.collect())
            .await
            .unwrap()
            .unwrap();
        assert!(result.status.unwrap().success(), "{result:?}");
        assert!(result.errors.is_empty(), "{result:?}");
    }

    let spec = ContainerSpec::new(
        uuid::Uuid::from_u128(1),
        ContainerPurpose::Run,
        "worker:fixture",
        vec!["/bin/cat".into()],
        NetworkMode::None,
    )
    .unwrap();
    assert!(spec.create_args_for(RuntimeKind::Docker, true).is_err());
    let spec = spec
        .with_broker_egress(&EgressLaunchV1::broker_proxy(3128).unwrap())
        .unwrap();

    for kind in [RuntimeKind::Docker, RuntimeKind::Podman] {
        let historical = spec.create_args(kind).unwrap();
        assert!(!historical.iter().any(|arg| arg == "--interactive"));
        assert!(
            !historical
                .iter()
                .any(|arg| arg.starts_with(HELD_CHALLENGE_ENV))
        );
        assert!(spec.create_args_for(kind, true).is_err());
        let docker = kind == RuntimeKind::Docker;
        let valid = json!({"Config": {
            "Tty": false, "OpenStdin": true, "AttachStdin": docker, "StdinOnce": docker
        }});
        validate_bootstrap_stdin_flags(&serde_json::to_vec(&valid).unwrap(), kind).unwrap();
        for (flag, wrong_value) in [
            ("Tty", true),
            ("OpenStdin", false),
            ("AttachStdin", !docker),
            ("StdinOnce", !docker),
        ] {
            let mut wrong = valid.clone();
            wrong["Config"][flag] = json!(wrong_value);
            assert!(
                validate_bootstrap_stdin_flags(&serde_json::to_vec(&wrong).unwrap(), kind).is_err()
            );
            wrong["Config"].as_object_mut().unwrap().remove(flag);
            assert!(
                validate_bootstrap_stdin_flags(&serde_json::to_vec(&wrong).unwrap(), kind).is_err()
            );
        }
    }
}

#[test]
fn held_spec_freezes_the_admitted_worker_and_emits_only_its_markers() {
    let worker = held_worker();
    let spec = ContainerSpec::for_held_worker(&worker).unwrap();
    assert_eq!(
        spec.operation_id().to_string(),
        worker.admission_operation_id
    );
    assert!(spec.create_args(RuntimeKind::Docker).is_err());
    assert!(spec.create_args_for(RuntimeKind::Podman, true).is_err());
    for kind in [RuntimeKind::Docker, RuntimeKind::Podman] {
        let mut matching = worker.clone();
        if kind == RuntimeKind::Podman {
            matching.launch.runtime_kind = crate::control::RuntimeKindV1::Podman;
            matching.spec_hash = matching.launch.spec_hash().unwrap();
        }
        let spec = ContainerSpec::for_held_worker(&matching).unwrap();
        let args = spec.create_args_for(kind, true).unwrap();
        assert!(args.iter().any(|arg| arg == "--interactive"));
        assert!(!args.iter().any(|arg| arg == "--tty"));
        for expected in [
            format!("AI_GUARD_EGRESS_HELD_V1={ATTEMPT}"),
            "AI_GUARD_EGRESS_PROXY=http://127.0.0.1:3128".to_owned(),
        ] {
            assert_eq!(
                args.windows(2)
                    .filter(|pair| pair[0] == "--env" && pair[1] == expected)
                    .count(),
                1
            );
        }
        let separator = args.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(&args[separator + 2..], &worker.launch.command.argv[1..]);
    }

    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root =
        crate::control_fs::Directory::open_existing(&directory.path().canonicalize().unwrap())
            .unwrap();
    let payload = format!("{BOOTSTRAP_ENV}=synthetic-credential\n");
    let collision = format!("{payload}{HELD_CHALLENGE_ENV}={ATTEMPT}\n");
    let file = root
        .persistent_create("colliding.env", collision.as_bytes())
        .unwrap();
    assert!(spec.clone().with_secret_env_handle(file).is_err());
    let file = root
        .persistent_create("provider.env", payload.as_bytes())
        .unwrap();
    let credential_args = spec
        .clone()
        .with_secret_env_handle(file)
        .unwrap()
        .create_args_for(RuntimeKind::Docker, true)
        .unwrap();
    assert!(
        !credential_args
            .iter()
            .any(|arg| arg.contains("synthetic-credential"))
    );

    let mut changed = spec.clone();
    *changed.argv.last_mut().unwrap() = "substituted prompt".into();
    assert!(changed.create_args_for(RuntimeKind::Docker, true).is_err());

    let mut bad = worker.clone();
    bad.launch.command.argv[0] = "/bin/cat".into();
    bad.spec_hash = bad.launch.spec_hash().unwrap();
    assert!(ContainerSpec::for_held_worker(&bad).is_err());

    let mut bad = worker.clone();
    bad.attempt.attempt_id = ATTEMPT.to_ascii_uppercase();
    assert!(ContainerSpec::for_held_worker(&bad).is_err());

    let mut bad = worker.clone();
    bad.launch.egress = None;
    bad.launch.network = NetworkV1::Bridge;
    bad.spec_hash = bad.launch.spec_hash().unwrap();
    assert!(ContainerSpec::for_held_worker(&bad).is_err());

    let mut bad = worker;
    bad.launch.security_preset = SecurityPresetV1::FixedSandboxInteractiveV3;
    bad.spec_hash = bad.launch.spec_hash().unwrap();
    assert!(ContainerSpec::for_held_worker(&bad).is_err());
}

#[test]
fn held_inspect_accepts_only_the_exact_marker_pair_and_hides_raw_values() {
    let spec = ContainerSpec::for_held_worker(&held_worker()).unwrap();
    let binding = spec.held_bootstrap_for(true).unwrap().unwrap();
    let challenge = format!("{HELD_CHALLENGE_ENV}={ATTEMPT}");
    let marker = format!("{EGRESS_PROXY_MARKER}=http://127.0.0.1:3128");
    let private = format!(r#"{BOOTSTRAP_ENV}={{"secret":"private-inspection-canary"}}"#);
    let inspect = |env: &[&str]| serde_json::to_vec(&json!({"Config": {"Env": env}})).unwrap();
    validate_bootstrap_environment(&inspect(&[&private, &marker, &challenge]), binding).unwrap();

    for env in [
        vec![private.as_str(), &challenge],
        vec![&private, &marker],
        vec![&private, &marker, &challenge, &challenge],
        vec![&private, &marker, &challenge, &marker],
        vec![&private, &marker, HELD_CHALLENGE_ENV],
        vec![
            &private,
            &marker,
            "AI_GUARD_EGRESS_HELD_V1=private-inspection-canary",
        ],
        vec![
            &private,
            &marker,
            "AI_GUARD_EGRESS_HELD_V1=550e8400-e29b-41d4-a716-446655440001",
        ],
        vec![
            &private,
            "AI_GUARD_EGRESS_PROXY=http://127.0.0.1:3129",
            &challenge,
        ],
        vec![
            &private,
            "AI_GUARD_EGRESS_PROXY=http://127.0.0.1:03128",
            &challenge,
        ],
    ] {
        let error = validate_bootstrap_environment(&inspect(&env), binding).unwrap_err();
        assert!(!format!("{error:?}").contains("private-inspection-canary"));
    }
}
