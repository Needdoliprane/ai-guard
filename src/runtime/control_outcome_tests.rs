//! Software custody tests: real libtest Children/pipes, no container engine.
use super::*;
use std::{future::Future, io::Write, path::Path, task::Poll};

fn runtime() -> Runtime {
    Runtime::with_program(RuntimeKind::Docker, &std::env::current_exe().unwrap()).unwrap()
}
async fn ready(root: &Path) -> Option<u32> {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(bytes) = std::fs::read_to_string(root.join("ready"))
            && let Ok(pid) = bytes.parse()
        {
            return Some(pid);
        }
        if Instant::now() >= until {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
async fn parked(scope: &NativeControlScope, root: &Path) -> Io {
    scope
        .spawn(
            scope.0.runtime.command().unwrap(),
            &[
                "--exact".into(),
                "runtime::control_outcome::tests::native_control_child".into(),
                "--ignored".into(),
                "--nocapture".into(),
            ],
            &[("AI_GUARD_NATIVE_CONTROL_CHILD_DIR", root.to_str().unwrap())],
            None,
        )
        .await
        .unwrap()
        .0
}
#[derive(Debug)]
struct OriginalCause(Arc<()>);
impl fmt::Display for OriginalCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("original fixture cause")
    }
}
impl std::error::Error for OriginalCause {}

#[derive(Debug)]
struct FollowupCause(Arc<()>);
impl fmt::Display for FollowupCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("followup fixture cause")
    }
}
impl std::error::Error for FollowupCause {}

#[test]
fn native_stop_failure_keeps_primary_and_followup_causes_and_exposes_primary_source() {
    let primary = Arc::new(());
    let secondary = Arc::new(());
    let error = anyhow::Error::new(NativeStopFailure {
        kill_failure: Some(anyhow::Error::new(OriginalCause(Arc::clone(&primary)))),
        followup_failure: anyhow::Error::new(FollowupCause(Arc::clone(&secondary))),
    });
    let retained = error.downcast_ref::<NativeStopFailure>().unwrap();
    let kill = retained
        .kill_failure
        .as_ref()
        .unwrap()
        .downcast_ref::<OriginalCause>()
        .unwrap();
    let followup = retained
        .followup_failure
        .downcast_ref::<FollowupCause>()
        .unwrap();
    let source = std::error::Error::source(retained).unwrap();
    let source_kill = source.downcast_ref::<OriginalCause>().unwrap();
    assert!(Arc::ptr_eq(&kill.0, &primary));
    assert!(Arc::ptr_eq(&followup.0, &secondary));
    assert!(std::ptr::eq(kill, source_kill));
    assert!(source.downcast_ref::<FollowupCause>().is_none());
    // This exercises error ownership only; it constructs no collection proof.
}

#[tokio::test]
async fn refused_environment_has_positive_no_child_outcome() {
    let scope = NativeControlScope::new(runtime());
    let scoped = scope.runtime();
    let result = scoped
        .capture_cli(
            scoped.command().unwrap(),
            &["kill".into(), "a".repeat(64)],
            Duration::from_secs(3),
            &[("UNAPPROVED_OVERRIDE", "fixture")],
        )
        .await;
    assert!(matches!(
        scope.finish(result),
        NativeRuntimeOutcome::NoChild { result: Err(_), .. }
    ));
}

#[tokio::test]
async fn actual_nonzero_native_cli_retains_original_error_capture_and_observation_order() {
    let (sink, mut collector) = observation::ObservationCollector::bounded(32).unwrap();
    let scope = NativeControlScope::new(
        runtime()
            .with_observation(sink)
            .observed_operation(Uuid::new_v4()),
    );
    let scoped = scope.runtime();
    let mut command = scoped.command().unwrap();
    command.arg("--invalid-native-outcome-fixture-option");
    let result = scoped
        .capture_cli(
            command,
            &["kill".into(), "a".repeat(64)],
            Duration::from_secs(3),
            &[],
        )
        .await;
    let NativeRuntimeOutcome::Collected {
        result: Err(error), ..
    } = scope.finish(result)
    else {
        panic!("actual nonzero CLI was not collected");
    };
    assert!(error.downcast_ref::<RuntimeCommandFailure>().is_some());
    let evidence = error.downcast_ref::<CliEvidence>().unwrap();
    assert!(evidence.0.status.is_some_and(|status| !status.success()));
    assert!(evidence.0.stdout_eof && evidence.0.stderr_eof);
    let mut phases = Vec::new();
    while let Some(event) = collector.try_recv() {
        phases.push(event.phase);
    }
    assert_eq!(
        phases,
        [
            observation::Phase::StopInvocationRequested,
            observation::Phase::StopSpawnRequested,
            observation::Phase::StopSpawnReturned,
            observation::Phase::StopCommandExitedError,
            observation::Phase::StopInvocationReturnedError
        ]
    );
    assert!(!collector.lost());
}

#[tokio::test]
async fn cancelled_collection_keeps_same_io_then_moves_original_cause_once() {
    let root = tempfile::tempdir().unwrap();
    let scope = NativeControlScope::new(runtime());
    let io = parked(&scope, root.path()).await;
    let ready_pid = ready(root.path()).await;
    let marker = Arc::new(());
    let NativeRuntimeOutcome::Uncollected(mut call) =
        scope.finish::<()>(Err(anyhow::Error::new(OriginalCause(Arc::clone(&marker)))))
    else {
        panic!("parked native call must remain owned");
    };
    let was_pending = {
        let collecting = call.collect();
        tokio::pin!(collecting);
        std::future::poll_fn(|cx| Poll::Ready(collecting.as_mut().poll(cx).is_pending())).await
    };
    let same = scope
        .state()
        .io
        .as_ref()
        .is_some_and(|retained| Arc::ptr_eq(retained, &io));
    std::fs::write(root.path().join("release"), b"release").unwrap();
    let collected = call.collect().await;
    let NativeCollectionOutcome::Collected {
        result: Err(error), ..
    } = collected
    else {
        panic!("same child was not actually collected");
    };
    assert!(ready_pid.is_some());
    assert!(was_pending && same);
    assert!(Arc::ptr_eq(
        &error.downcast_ref::<OriginalCause>().unwrap().0,
        &marker
    ));
    let evidence = error.downcast_ref::<CliEvidence>().unwrap();
    assert!(evidence.0.status.is_some_and(|status| status.success()));
    assert!(evidence.0.stdout_eof && evidence.0.stderr_eof);
    assert!(matches!(
        call.collect().await,
        NativeCollectionOutcome::PendingOrUnknown
    ));
    // The same context may emit again only after that actual collection. The
    // Unknown test separately checks that an unresolved context cannot do so.
    let scoped = scope.runtime();
    let mut command = scoped.command().unwrap();
    command.arg("--invalid-reused-native-outcome-fixture-option");
    let second = scoped
        .capture_cli(command, &[], Duration::from_secs(3), &[])
        .await;
    let NativeRuntimeOutcome::Collected {
        result: Err(second),
        ..
    } = scope.finish(second)
    else {
        panic!("same native context did not collect its second real CLI");
    };
    assert!(second.downcast_ref::<RuntimeCommandFailure>().is_some());
    let evidence = second.downcast_ref::<CliEvidence>().unwrap();
    assert!(evidence.0.status.is_some_and(|status| !status.success()));
    assert!(evidence.0.stdout_eof && evidence.0.stderr_eof);
}

#[tokio::test]
async fn lost_io_task_keeps_actual_child_and_pipes_and_refuses_reuse() {
    let root = tempfile::tempdir().unwrap();
    let scope = NativeControlScope::new(runtime());
    let io = parked(&scope, root.path()).await;
    let ready_pid = ready(root.path()).await;
    let NativeRuntimeOutcome::Uncollected(mut call) =
        scope.finish::<()>(Err(anyhow::Error::new(OriginalCause(Arc::new(())))))
    else {
        panic!("native owner missing");
    };
    io.lock().await.abort_retained_control_for_test();
    let pending = matches!(
        call.collect().await,
        NativeCollectionOutcome::PendingOrUnknown
    );
    let identity = io.lock().await.retained_control_identity_for_test().await;
    let phase = call.phase();
    let reuse_refused = scope
        .spawn(
            scope.0.runtime.command().unwrap(),
            &["--help".into()],
            &[],
            None,
        )
        .await
        .is_err();
    // Another independent native context still performs and collects its own
    // ordinary error; this Unknown cannot consume that context's capacity.
    let other = NativeControlScope::new(runtime());
    let scoped = other.runtime();
    let mut command = scoped.command().unwrap();
    command.arg("--invalid-independent-native-outcome-fixture-option");
    let other_result = scoped
        .capture_cli(command, &[], Duration::from_secs(3), &[])
        .await;
    let independent_collected = matches!(
        other.finish(other_result),
        NativeRuntimeOutcome::Collected { result: Err(_), .. }
    );
    // Exact test Child cleanup does not produce a native application receipt,
    // reset Unknown or authorize reuse of the original scope.
    io.lock().await.collect_aborted_control_fixture().await;
    assert!(ready_pid.is_some());
    assert_eq!(identity, (ready_pid, true, true));
    assert!(pending && reuse_refused && independent_collected);
    assert_eq!(phase, NativeIoPhase::Unknown);
    assert!(matches!(
        call.collect().await,
        NativeCollectionOutcome::PendingOrUnknown
    ));
}

#[test]
#[ignore = "owned native IO fixture child; selected by its parent only"]
fn native_control_child() {
    let Some(root) = std::env::var_os("AI_GUARD_NATIVE_CONTROL_CHILD_DIR") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    std::io::stdout()
        .write_all(b"native-outcome-private-stdout\n")
        .unwrap();
    std::io::stdout().flush().unwrap();
    std::io::stderr()
        .write_all(b"native-outcome-private-stderr\n")
        .unwrap();
    std::io::stderr().flush().unwrap();
    std::fs::write(root.join("ready"), std::process::id().to_string()).unwrap();
    let until = std::time::Instant::now() + Duration::from_secs(30);
    while !root.join("release").exists() && std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
}
