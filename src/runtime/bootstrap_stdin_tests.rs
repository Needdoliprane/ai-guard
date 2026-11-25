use super::{
    BeforeSpawnGuard, CancelGeneration, ContainerPurpose, ContainerSpec, NetworkMode,
    OwnedStartInput, RuntimeKind, SpawnedPrearmedInput,
    batch_io::OwnedBatchIo,
    bootstrap_stdin::{
        BootstrapStdinSubmitError, MAX_BOOTSTRAP_STDIN_BYTES, PrearmedBootstrapStdin,
        PreparedBootstrapStdin,
    },
    managed::validate_bootstrap_stdin_flags,
    spawn_prearmed_cli,
};
use crate::control::EgressLaunchV1;
use anyhow::Result;
use std::{future::Future, pin::Pin, process::Stdio, time::Duration};
use tokio::{
    process::{Child, Command},
    sync::watch,
    time::{Instant, timeout},
};

const TARGET: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct FixtureGuard;
impl BeforeSpawnGuard for FixtureGuard {
    fn check_before_spawn(self: Box<Self>, target: &str) -> Result<()> {
        anyhow::ensure!(target == TARGET);
        Ok(())
    }
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
    _: PreparedBootstrapStdin<'a>,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async { anyhow::bail!("spawn fixture must not call the handoff") })
}

fn bootstrap_child() -> (Child, PrearmedBootstrapStdin) {
    let (_stop_tx, stop) = watch::channel(false);
    let (child, input) = spawn_prearmed_cli(
        inert_cli(&format!("start --attach --interactive {TARGET}")),
        TARGET,
        OwnedStartInput::Bootstrap(Box::new(unused_handoff)),
        Box::new(FixtureGuard),
        &stop,
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
    for archive in [None, Some(tempfile::tempfile().unwrap())] {
        let expected_args = if archive.is_some() {
            format!("start --attach --interactive {TARGET}")
        } else {
            format!("start --attach {TARGET}")
        };
        let has_archive = archive.is_some();
        let (child, input) = spawn_prearmed_cli(
            inert_cli(&expected_args),
            TARGET,
            OwnedStartInput::Historical(archive),
            Box::new(FixtureGuard),
            &stop,
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

    use serde_json::json;
    for kind in [RuntimeKind::Docker, RuntimeKind::Podman] {
        let historical = spec.create_args(kind).unwrap();
        assert!(!historical.iter().any(|arg| arg == "--interactive"));
        let bootstrap = spec.create_args_for(kind, true).unwrap();
        assert!(bootstrap.iter().any(|arg| arg == "--interactive"));
        assert!(!bootstrap.iter().any(|arg| arg == "--tty"));
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
