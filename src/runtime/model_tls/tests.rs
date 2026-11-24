use super::*;
use std::{cell::RefCell, sync::atomic::AtomicU32};
type SpawnHook = (Arc<AtomicU32>, Option<Cancellation>);
thread_local! {static ON_SPAWN:RefCell<Option<SpawnHook>>=const{RefCell::new(None)};}
pub(super) fn spawned(pid: Option<u32>) {
    let hook = ON_SPAWN.with(|slot| slot.borrow_mut().take());
    if let Some((record, cancel)) = hook {
        record.store(pid.unwrap_or(0), Ordering::SeqCst);
        if let Some(cancel) = cancel {
            cancel.cancel();
        }
    }
}
fn command(mode: &str) -> (Command, Vec<String>) {
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .env_clear()
        .env("AI_GUARD_TLS_NATIVE_PIPE_PROBE", mode);
    (
        child,
        [
            "--exact",
            "runtime::model_tls::tests::owned_native_pipe_probe",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ]
        .iter()
        .map(|value| (*value).into())
        .collect(),
    )
}

#[test]
#[ignore = "internal native pipe fixture invoked only by the owning transport tests"]
fn owned_native_pipe_probe() {
    use std::io::{Read, Write};
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    assert_eq!(input, b"fictional-private-input\n");
    std::io::stdout()
        .write_all(b"public-native-probe-ok\n")
        .unwrap();
    std::io::stderr()
        .write_all(b"fixture-not-a-real-secret\n")
        .unwrap();
    if std::env::var("AI_GUARD_TLS_NATIVE_PIPE_PROBE").unwrap() == "hold" {
        loop {
            std::thread::park();
        }
    }
}

#[tokio::test]
async fn transport_owns_stdin_and_returns_only_bounded_stdout_from_a_real_child() {
    let (command, args) = command("exit");
    let output = run(
        command,
        &args,
        Some(b"fictional-private-input\n"),
        4096,
        Instant::now() + Duration::from_secs(5),
        &Cancellation::default(),
    )
    .await
    .unwrap();
    assert!(
        output
            .windows(b"public-native-probe-ok".len())
            .any(|bytes| bytes == b"public-native-probe-ok")
    );
    assert!(
        !output
            .windows(b"fixture-not-a-real-secret".len())
            .any(|bytes| bytes == b"fixture-not-a-real-secret")
    );
}

#[tokio::test]
async fn timeout_and_cancellation_reap_only_the_owned_native_child() {
    for cancel_at_spawn in [false, true] {
        let cancel = Cancellation::default();
        let pid = Arc::new(AtomicU32::new(0));
        ON_SPAWN.with(|slot| {
            *slot.borrow_mut() = Some((pid.clone(), cancel_at_spawn.then(|| cancel.clone())))
        });
        let (command, args) = command("hold");
        let result = run(
            command,
            &args,
            Some(b"fictional-private-input\n"),
            4096,
            Instant::now() + Duration::from_millis(200),
            &cancel,
        )
        .await;
        assert_eq!(
            result.err(),
            Some(if cancel_at_spawn {
                Failure::Interrupted
            } else {
                Failure::TimedOut
            })
        );
        let pid = i32::try_from(pid.load(Ordering::SeqCst)).unwrap();
        assert!(pid > 0);
        // SAFETY: signal0 only queries this test's just-reaped child; it
        // never sends a signal to an old guard, worker or other process.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}

#[test]
fn preparer_has_only_two_named_volumes_and_no_secret_arguments_or_engine_mount() {
    let generation = uuid::Uuid::new_v4().to_string();
    let attempt = Attempt {
        generation: generation.clone(),
        preparer: format!("ai-guard-tls-preparer-{generation}"),
        cid: None,
        volumes: VolumeSelection::new(&generation),
        stop_observation: "not_created",
        cleanup_local: CleanupLocalStatus::NotAttempted,
        runtime: None,
    };
    let image = format!("sha256:{}", "a".repeat(64));
    assert!(image_id(&image));
    assert!(!image_id("example:latest"));
    for kind in [RuntimeKind::Docker, RuntimeKind::Podman] {
        let args = preparer_args(kind, &image, &attempt);
        assert_eq!(
            args.iter()
                .filter(|value| value.as_str() == "--mount")
                .count(),
            2
        );
        assert!(args.iter().any(|value| value == "--pull=never"));
        assert!(args.windows(2).any(|pair| pair == ["--network", "none"]));
        assert!(args.windows(2).any(|pair| pair == ["--user", "1000:1000"]));
        assert!(args.windows(2).any(|pair| pair == ["--pids-limit", "64"]));
        assert!(!args.iter().any(|value| value.contains("docker.sock")
            || value.contains("type=bind")
            || value == "--privileged"
            || value == "--env-file"));
        assert_eq!(args.last().map(String::as_str), Some("__model-tls-import"));
    }
}

#[test]
fn cleanup_error_projection_preserves_primary_and_local_unconfirmed_separately() {
    // Exercise the production error projection, not a fabricated kernel hang.
    let mut attempt = Attempt {
        generation: uuid::Uuid::new_v4().to_string(),
        preparer: "fictional-preparer".into(),
        cid: None,
        volumes: VolumeSelection::new("fictional-generation"),
        stop_observation: "creation_unknown",
        cleanup_local: CleanupLocalStatus::NotAttempted,
        runtime: None,
    };
    let primary = attempt.finish_failure(Failure::Interrupted, Err(Failure::LocalProcessUnknown));
    assert_eq!(primary, Failure::Interrupted);
    assert_eq!(attempt.stop_observation, "unknown");
    assert_eq!(
        attempt.cleanup_local,
        CleanupLocalStatus::TransportTerminationUnconfirmed
    );
    let diagnostic = serde_json::to_value(&attempt).unwrap();
    assert_eq!(diagnostic["stop_observation"], "unknown");
    assert_eq!(
        diagnostic["cleanup_local"],
        "transport_termination_unconfirmed"
    );
    assert_eq!(
        attempt.finish_failure(Failure::TimedOut, Err(Failure::Runtime)),
        Failure::TimedOut
    );
    assert_eq!(attempt.cleanup_local, CleanupLocalStatus::TransportsJoined);
}

#[test]
fn provenance_records_only_bounded_local_authority_and_distinguishes_endpoints() {
    let info = br#"{"ID":"fictional-engine","arbitrary":"discarded-output"}"#;
    let first =
        RuntimeAuthority::from_info(RuntimeKind::Docker, Some("unix:///fixture/one.sock"), info)
            .unwrap();
    let second =
        RuntimeAuthority::from_info(RuntimeKind::Docker, Some("unix:///fixture/two.sock"), info)
            .unwrap();
    assert_ne!(first, second);
    let receipt = RuntimeProvenance {
        invocation_sha256: "d".repeat(64),
        authority: first,
    };
    let rendered = serde_json::to_value(&receipt).unwrap();
    assert_eq!(
        rendered["authority"]["unix_endpoint"],
        "unix:///fixture/one.sock"
    );
    assert_eq!(rendered["authority"]["engine_id"], "fictional-engine");
    assert!(!rendered.to_string().contains("discarded-output"));
    for endpoint in [
        "tcp://localhost:2375",
        "unix://relative",
        "unix:///fixture/\nengine.sock",
    ] {
        assert!(RuntimeAuthority::from_info(RuntimeKind::Docker, Some(endpoint), info).is_err());
    }
    assert!(
        RuntimeAuthority::from_info(
            RuntimeKind::Docker,
            Some(&format!("unix:///{}", "x".repeat(4096))),
            info
        )
        .is_err()
    );
    let oversized = serde_json::to_vec(&serde_json::json!({"ID":"x".repeat(257)})).unwrap();
    assert!(
        RuntimeAuthority::from_info(
            RuntimeKind::Docker,
            Some("unix:///fixture/one.sock"),
            &oversized
        )
        .is_err()
    );
    let podman = br#"{"store":{"graphRoot":"/fixture/graph","runRoot":"/fixture/run"}}"#;
    assert!(RuntimeAuthority::from_info(RuntimeKind::Podman, None, podman).is_ok());
    assert!(
        RuntimeAuthority::from_info(
            RuntimeKind::Podman,
            Some("unix:///fixture/one.sock"),
            podman
        )
        .is_err()
    );
    assert!(
        RuntimeAuthority::from_info(
            RuntimeKind::Podman,
            None,
            br#"{"store":{"graphRoot":"relative","runRoot":"/fixture/run"}}"#
        )
        .is_err()
    );
}

#[test]
fn podman_bare_image_id_still_requires_the_exact_authorized_digest() {
    let digest = "a".repeat(64);
    let expected = format!("sha256:{digest}");
    let prefixed = Value::String(expected.clone());
    let bare = Value::String(digest.clone());

    assert!(matches_runtime_image(
        RuntimeKind::Docker,
        &prefixed,
        &expected
    ));
    assert!(!matches_runtime_image(
        RuntimeKind::Docker,
        &bare,
        &expected
    ));
    assert!(matches_runtime_image(
        RuntimeKind::Podman,
        &prefixed,
        &expected
    ));
    assert!(matches_runtime_image(RuntimeKind::Podman, &bare, &expected));

    for observed in [
        Value::Null,
        Value::String("b".repeat(64)),
        Value::String(format!("sha256:{}", "b".repeat(64))),
        Value::String(digest[..63].to_owned()),
    ] {
        assert!(!matches_runtime_image(
            RuntimeKind::Podman,
            &observed,
            &expected
        ));
    }
    assert!(!matches_runtime_image(RuntimeKind::Podman, &bare, &digest));
}
