//! Source-only / NOT_RUN. Real software Child, never a container engine.
use super::*;
use crate::runtime::{
    batch_io::OwnedBatchIo,
    deadline_supervision::{Cause, CauseLatch, NativeStop, Supervisor},
};
use std::{cell::Cell, process::Stdio, time::Duration};

#[tokio::test(flavor = "current_thread")]
async fn saturated_diagnostics_preserve_actual_deadline_stop_and_child_collection() {
    let (sink, collector) = ObservationCollector::bounded(1).unwrap();
    let trace = sink.operation(Uuid::new_v4()).instance(&"a".repeat(64));
    trace.event(Phase::Submitted); // Deliberately retain a full queue, no reader.
    let child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .env_clear()
        .env("AI_GUARD_DEADLINE_DRIVER_FIXTURE17", "park-after-stdin")
        .args([
            "--exact",
            "runtime::deadline_driver_tests::driver_child_fixture17",
            "--ignored",
            "--nocapture",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let io = OwnedBatchIo::own_observed(child, None, 4096, Some(trace.clone()));
    let cause = CauseLatch::default();
    let calls = Cell::new(0);
    let stop = async {
        calls.set(calls.get() + 1);
        NativeStop {
            observed_stopped: false,
            evidence: None,
            errors: vec![anyhow::anyhow!("UNIT_NOT_NATIVE_STOP")],
            moved_outer_fallback: false,
        }
    };
    let mut supervisor = Supervisor::new(
        io,
        tokio::time::Instant::now() - Duration::from_secs(1),
        cause,
        stop,
    )
    .with_observation(Some(trace));
    supervisor.wait_attach().await;
    supervisor.finish_stop().await;
    let collected = supervisor.io.collect().await.unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(supervisor.cause.get(), Some(Cause::Deadline));
    assert!(collected.status.is_some() && collected.stdout_eof && collected.stderr_eof);
    assert!(collector.lost(), "saturation must invalidate evidence");
}

#[test]
fn disconnected_or_uncontrolled_identifier_marks_trace_lost() {
    let (sink, collector) = ObservationCollector::bounded(1).unwrap();
    drop(collector);
    sink.operation(Uuid::new_v4()).event(Phase::Submitted);
    assert!(sink.lost.load(Ordering::Acquire));
    let (sink, collector) = ObservationCollector::bounded(1).unwrap();
    sink.operation(Uuid::new_v4())
        .instance("untrusted-name")
        .event(Phase::Created);
    assert!(collector.lost());
}

#[tokio::test(flavor = "current_thread")]
async fn stop_invocation_refusal_does_not_claim_spawn_or_cli_exit() {
    let (sink, mut collector) = ObservationCollector::bounded(32).unwrap();
    let runtime = crate::runtime::Runtime::with_program(
        crate::runtime::RuntimeKind::Docker,
        &std::env::current_exe().unwrap(),
    )
    .unwrap()
    .with_observation(sink)
    .observed_operation(Uuid::new_v4());
    let args = vec!["kill".into(), "a".repeat(64)];
    let error = runtime
        .capture_cli(
            runtime.command().unwrap(),
            &args,
            Duration::from_secs(1),
            &[("UNAPPROVED_UNIT_KEY", "refused")],
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("environment override"));
    let mut phases = Vec::new();
    while let Some(event) = collector.try_recv() {
        phases.push(event.phase);
    }
    assert_eq!(
        phases,
        [
            Phase::StopInvocationRequested,
            Phase::StopInvocationReturnedError
        ]
    );
    assert!(!collector.lost());
}

#[tokio::test(flavor = "current_thread")]
async fn actual_nonzero_stop_cli_exit_is_distinct_from_unknown_status() {
    let (sink, mut collector) = ObservationCollector::bounded(32).unwrap();
    let runtime = crate::runtime::Runtime::with_program(
        crate::runtime::RuntimeKind::Docker,
        &std::env::current_exe().unwrap(),
    )
    .unwrap()
    .with_observation(sink)
    .observed_operation(Uuid::new_v4());
    let target = "a".repeat(64);
    // The real libtest CLI refuses this option. No engine is invoked.
    let mut command = runtime.command().unwrap();
    command.arg("--rme-invalid-stop-command-option");
    let error = runtime
        .capture_cli(
            command,
            &["kill".into(), target.clone()],
            Duration::from_secs(5),
            &[],
        )
        .await
        .unwrap_err();
    let retained = error.downcast_ref::<crate::runtime::CliEvidence>().unwrap();
    assert!(retained.0.status.is_some_and(|status| !status.success()));
    assert!(retained.0.stdout_eof && retained.0.stderr_eof);
    assert!(
        error
            .downcast_ref::<crate::runtime::RuntimeCommandFailure>()
            .is_some()
    );
    let mut phases = Vec::new();
    while let Some(event) = collector.try_recv() {
        phases.push(event.phase);
    }
    assert_eq!(
        phases,
        [
            Phase::StopInvocationRequested,
            Phase::StopSpawnRequested,
            Phase::StopSpawnReturned,
            Phase::StopCommandExitedError,
            Phase::StopInvocationReturnedError,
        ]
    );
    // A missing wait result has no exit event, including after collection loss.
    crate::runtime::record_cli_exit(&runtime.trace_target(&target), None);
    assert!(collector.try_recv().is_none());
    assert!(!collector.lost());
}

#[tokio::test(flavor = "current_thread")]
async fn control_probe_timeout_collects_the_same_real_child_and_both_pipes() {
    let runtime = crate::runtime::Runtime::with_program(
        crate::runtime::RuntimeKind::Docker,
        &std::env::current_exe().unwrap(),
    )
    .unwrap();
    let error = runtime
        .cli_env(
            &[
                "--exact".into(),
                "runtime::observation::tests::probe_child_fixture".into(),
                "--ignored".into(),
                "--nocapture".into(),
            ],
            Duration::from_secs(1),
            &[(
                "AGENTSANDBOX_CONFIG_DIR",
                "/ai-guard-software-probe-timeout-fixture",
            )],
        )
        .await
        .unwrap_err();
    // The public qualification probe calls this same retained CLI pipeline.
    // A kill request alone would leave no actual status or EOF in this outcome.
    let retained = error
        .downcast_ref::<crate::runtime::deadline_supervision::Failure>()
        .unwrap();
    let io = retained.io.as_ref().unwrap();
    assert!(io.status.is_some_and(|status| !status.success()));
    assert!(io.stdout_eof && io.stderr_eof);
    assert!(
        io.stdout
            .bytes
            .windows(b"RME_PROBE_STDOUT".len())
            .any(|w| w == b"RME_PROBE_STDOUT"),
        "fixture readiness was not observed; stderr delivery is not established"
    );
    // Nonzero capture_cli status moves stderr into this exact typed cause. The
    // Failure still owns the wait/EOF evidence; its empty io.stderr is a moved
    // capture, not an observation that the child emitted no stderr.
    let mut command_failures = retained
        .causes
        .iter()
        .filter_map(|cause| cause.downcast_ref::<crate::runtime::RuntimeCommandFailure>());
    let command_failure = command_failures
        .next()
        .expect("actual nonzero CLI cause retained");
    assert!(
        command_failures.next().is_none(),
        "one CLI attempt must have one command failure"
    );
    assert_eq!(Some(command_failure.status), io.status);
    assert_eq!(command_failure.runtime, runtime.kind);
    assert!(io.stderr.bytes.is_empty() && io.stderr.total_bytes == 0);
    let stderr = command_failure.stderr_capture();
    assert!(
        stderr
            .bytes
            .windows(b"RME_PROBE_STDERR".len())
            .any(|w| w == b"RME_PROBE_STDERR"),
        "stderr flushed before fixture readiness is missing from its actual owner"
    );
    assert!(
        retained
            .causes
            .iter()
            .any(|cause| cause.to_string().contains("timed out"))
    );
    let retained_runtime = retained
        .runtime
        .as_ref()
        .expect("actual runtime owner retained");
    assert!(std::sync::Arc::ptr_eq(
        &retained_runtime.anchor,
        &runtime.anchor
    ));
    assert!(retained_runtime.invocation() == runtime.invocation());
}

#[test]
#[ignore = "software Child fixture; selected explicitly by the owning probe test"]
fn probe_child_fixture() {
    if std::env::var("AGENTSANDBOX_CONFIG_DIR").ok().as_deref()
        != Some("/ai-guard-software-probe-timeout-fixture")
    {
        return;
    }
    use std::io::Write;
    // This order makes STDOUT the readiness signal: observing it proves
    // that this same child already completed its STDERR write and flush. A
    // delayed startup may still fail readiness; it is never called pipe loss.
    std::io::stderr().write_all(b"RME_PROBE_STDERR\n").unwrap();
    std::io::stderr().flush().unwrap();
    std::io::stdout().write_all(b"RME_PROBE_STDOUT\n").unwrap();
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}
