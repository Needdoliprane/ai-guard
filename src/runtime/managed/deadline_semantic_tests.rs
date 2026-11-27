//! Semantic scheduler units with the existing real Rust managed CLI fixture.
//!
//! NOT_RUN here. No Docker/Podman engine, Native17, Store or audit qualification.
//! Created comes from the actual on_created callback. Terminal proof comes ONLY
//! from WorkObservation::observe's current managed native pipeline, never JSON.
#![cfg(unix)]
use crate::runtime::{
    ContainerPurpose, ContainerSpec, ManagedRetainedExecution, ManagedRuntime, NetworkMode,
    OwnedContainerSnapshot, Runtime, RuntimeKind, StartDirective,
    batch_io::{BatchIoCollected, OwnedBatchIo},
    deadline_supervision::{
        Cause, CauseLatch, NativeStop, StopEvidence, Supervisor, WorkObservation,
        outer_fallback_decision, resolve_outer_fallback,
    },
};
use std::{
    cell::{Cell, RefCell},
    fs,
    future::{Future, pending, poll_fn},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    pin::Pin,
    process::{ExitStatus, Stdio},
    sync::Arc,
    task::Poll,
    time::Duration,
};
use tokio::{sync::oneshot, time::Instant};
use uuid::Uuid;

struct Fixture {
    managed: ManagedRuntime,
    created: OwnedContainerSnapshot,
    _retained: ManagedRetainedExecution,
    // Drop native invocation owners/data before the private fixture directory.
    _temporary: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> anyhow::Result<Self> {
        let (temporary, program) = native_program()?;
        let runtime = Runtime::with_program(RuntimeKind::Docker, &program)?;
        let managed = ManagedRuntime::connect_docker_with_runtime(
            Uuid::new_v4(),
            "unix:///fixture/docker.sock",
            runtime,
        )
        .await?;
        let spec = ContainerSpec::with_operation(
            Uuid::new_v4(),
            ContainerPurpose::Run,
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            vec!["/usr/bin/id".into()],
            NetworkMode::None,
            Uuid::new_v4(),
        )?
        .with_managed_names("aig-fixture-worker", "aig-fixture-worker-work")?;
        let captured = RefCell::new(None);
        let retained = managed
            .execute_retained(
                &spec,
                None,
                Duration::from_secs(120),
                1024,
                |created| {
                    captured.replace(Some(created));
                    async { Ok(StartDirective::EmitOnce) }
                },
                |_| async { Ok(()) },
            )
            .await?;
        let created = captured
            .into_inner()
            .ok_or_else(|| anyhow::anyhow!("actual on_created callback was not called"))?;
        Ok(Self {
            managed,
            created,
            _retained: retained,
            _temporary: temporary,
        })
    }
    fn work(&self) -> anyhow::Result<Arc<WorkObservation>> {
        let scope = self
            .managed
            .batch_observation_scope(self.created.handle.id(), &self.created.observation)?;
        let work = WorkObservation::new(Some(scope));
        work.require_binding(
            &self.managed,
            self.created.handle.id(),
            &self.created.observation,
        )?;
        Ok(work)
    }
    async fn exited_cli(&self) -> anyhow::Result<(OwnedBatchIo, ExitStatus)> {
        self.managed.verify_authority().await?;
        exited_command(self.managed.command_async().await?).await
    }
}

fn native_program() -> anyhow::Result<(tempfile::TempDir, PathBuf)> {
    let temporary = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let source = temporary.path().join("managed_semantic.rs");
    let program = temporary.path().join("managed_semantic");
    // Same standalone source/compilation shape as runtime_managed.rs:1087.
    fs::write(
        &source,
        include_str!("../../../tests/runtime_fixtures/managed.rs"),
    )?;
    let compiled =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", "--crate-name", "managed_semantic"])
            .arg(&source)
            .arg("-o")
            .arg(&program)
            .output()?;
    anyhow::ensure!(
        compiled.status.success(),
        "managed software fixture compiler failed"
    );
    let metadata = program.with_extension("fixture");
    fs::create_dir(&metadata)?;
    fs::write(
        metadata.join("AI_GUARD_MANAGED_ROOT"),
        temporary.path().to_string_lossy().as_bytes(),
    )?;
    fs::write(metadata.join("AI_GUARD_MANAGED_CHILD"), "start_ok")?;
    Ok((temporary, program))
}
#[tokio::test]
async fn import_creation_orders_native_mutations_with_stop() -> anyhow::Result<()> {
    use crate::broker::enforcer::CancelGeneration;

    let (temporary, program) = native_program()?;
    let managed = ManagedRuntime::connect_docker_with_runtime(
        Uuid::new_v4(),
        "unix:///fixture/docker.sock",
        Runtime::with_program(RuntimeKind::Docker, &program)?,
    )
    .await?;
    let spec = ContainerSpec::with_operation(
        Uuid::new_v4(),
        ContainerPurpose::Import,
        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        vec!["/usr/bin/id".into()],
        NetworkMode::None,
        Uuid::new_v4(),
    )?
    .with_managed_names("aig-fixture-import", "aig-fixture-worker-work")?;
    assert!(managed.lookup_never_started_import(&spec).await?.is_none());
    let labels = [("agentsandbox.owner".into(), "expected".into())];
    let cancel = CancelGeneration::new();
    let rejected_generation = cancel.current();
    fs::write(temporary.path().join("pause_info_for_create"), "")?;
    let volume = {
        let managed = managed.clone();
        let cancel = cancel.clone();
        let labels = labels.clone();
        tokio::spawn(async move {
            managed
                .create_named_volume_for_generation(
                    "aig-fixture-worker-work",
                    &labels,
                    &cancel,
                    rejected_generation,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(4), async {
        while !temporary.path().join("info_waiting_for_create").is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    cancel.invalidate();
    fs::write(temporary.path().join("release_info_for_create"), "")?;
    assert!(volume.await?.is_err());
    fs::remove_file(temporary.path().join("pause_info_for_create"))?;
    assert!(
        managed
            .create_owned_import_for_generation(&spec, &cancel, rejected_generation)
            .await
            .is_err()
    );
    let log = temporary.path().join("log");
    assert!(
        fs::read_to_string(&log)?
            .lines()
            .all(|line| !line.starts_with("volume create ") && !line.starts_with("create "))
    );

    let fresh = CancelGeneration::new();
    managed
        .create_named_volume_for_generation(
            "aig-fixture-worker-work",
            &labels,
            &fresh,
            fresh.current(),
        )
        .await?;
    let created = managed
        .create_owned_import_for_generation(&spec, &fresh, fresh.current())
        .await?;
    assert_eq!(created.observation.process.running, Some(false));
    assert!(created.observation.process.started_at.is_none());
    assert_eq!(created.observation.id.as_deref(), Some(created.handle.id()));
    let recovered = managed
        .lookup_never_started_import(&spec)
        .await?
        .ok_or_else(|| anyhow::anyhow!("exact Created import was not recovered"))?;
    assert_eq!(recovered.handle.id(), created.handle.id());
    assert_eq!(
        recovered.observation.configuration_fingerprint,
        created.observation.configuration_fingerprint
    );
    fs::write(temporary.path().join("state"), "exited")?;
    assert!(managed.lookup_created(&spec).await?.is_some());
    assert!(managed.lookup_never_started_import(&spec).await.is_err());
    let calls = fs::read_to_string(log)?;
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("volume create "))
            .count(),
        1
    );
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("create "))
            .count(),
        1
    );
    assert!(
        calls
            .lines()
            .all(|line| !line.starts_with("start ") && !line.starts_with("rm "))
    );
    Ok(())
}

async fn exited_command(
    mut command: tokio::process::Command,
) -> anyhow::Result<(OwnedBatchIo, ExitStatus)> {
    let mut child = command
        .args(["info", "--format", "{{json .}}"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // The existing fixture's info response is only its short engine ID.
    // Child owns both pipes throughout this successful wait. Tokio1.54.1 wait
    // stores FusedChild::Done; handing THE SAME reaped Child to the IO owner
    // does not repeat an OS wait or infer exit from a marker/timeout.
    let exit = match child.wait().await {
        Ok(exit) => exit,
        Err(_) => pending::<ExitStatus>().await,
    };
    Ok((OwnedBatchIo::own_cli(child, 256 * 1024, 64 * 1024), exit))
}

async fn elapsed_deadline() -> Instant {
    let deadline = Instant::now();
    // Await this already-due timer, not an arbitrary race sleep. Thus a later
    // pending drive cannot be explained by the deadline not having elapsed.
    tokio::time::sleep_until(deadline).await;
    deadline
}
async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}
// Return Ready output, or Pending only after a causal predicate became true
// during a pending poll. Callers retain Ready output and never repoll it.
async fn poll_until<F: Future>(
    mut future: Pin<&mut F>,
    condition: impl Fn() -> bool,
) -> Poll<F::Output> {
    poll_fn(|cx| match future.as_mut().poll(cx) {
        Poll::Ready(result) => Poll::Ready(Poll::Ready(result)),
        Poll::Pending if condition() => Poll::Ready(Poll::Pending),
        Poll::Pending => Poll::Pending,
    })
    .await
}
fn negative_stop() -> NativeStop {
    // Scheduling unit only; no native stopped observation/evidence is supplied.
    NativeStop {
        observed_stopped: false,
        evidence: None,
        errors: vec![anyhow::anyhow!("UNIT_NOT_NATIVE_STOP")],
        moved_outer_fallback: false,
    }
}
async fn collect<S: Future<Output = NativeStop>>(
    supervisor: &mut Supervisor<S>,
) -> BatchIoCollected {
    supervisor.finish_stop().await;
    supervisor.wait_attach().await;
    supervisor.finish_stop().await;
    supervisor.io.collect().await.unwrap()
}
fn assert_cli_collected(collected: &BatchIoCollected, native_exit: ExitStatus) {
    assert_eq!(collected.status, Some(native_exit));
    assert!(collected.stdout_eof && collected.stderr_eof);
    assert!(collected.errors.is_empty(), "{collected:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn exact_native_terminal_before_pending_callback_disarms_elapsed_deadline() {
    let fixture = Fixture::new().await.unwrap();
    let work = fixture.work().unwrap();
    let observed = work.observe().await.unwrap();
    let terminal_before_callback = work.terminal_proven();
    let (io, cli_exit) = fixture.exited_cli().await.unwrap();
    let cause = CauseLatch::default();
    let stop_calls = Cell::new(0);
    let stop = async {
        stop_calls.set(stop_calls.get() + 1);
        negative_stop()
    };
    let mut supervisor =
        Supervisor::new(io, elapsed_deadline().await, cause.clone(), stop).with_work(work.clone());
    let calls = Cell::new(0);
    let done = Cell::new(false);
    let (release, released) = oneshot::channel::<()>();
    let mut callback = Box::pin(async {
        calls.set(calls.get() + 1);
        let _ = released.await;
        done.set(true);
        Ok::<(), anyhow::Error>(())
    });
    let mut drive = Box::pin(supervisor.drive(callback.as_mut()));
    let initial = poll_once(drive.as_mut()).await;
    let pending_while_due = matches!(&initial, Poll::Pending);
    let no_deadline_while_held =
        cause.get().is_none() && stop_calls.get() == 0 && calls.get() == 1 && !done.get();
    let released = release.send(()).is_ok();
    let result = match initial {
        Poll::Ready(result) => result,
        Poll::Pending => drive.as_mut().await,
    };
    drop(drive);
    drop(callback);
    let collected = collect(&mut supervisor).await;
    // All callback/native CLI/capture owners completed before assertions.
    assert!(result.is_ok() && released && done.get());
    assert!(terminal_before_callback && pending_while_due && no_deadline_while_held);
    assert_eq!(observed.id.as_deref(), Some(fixture.created.handle.id()));
    assert!(work.terminal_proven());
    assert_eq!(cause.get(), None);
    assert_eq!(calls.get(), 1);
    assert_eq!(stop_calls.get(), 0);
    assert_cli_collected(&collected, cli_exit);
}

#[tokio::test(flavor = "current_thread")]
async fn actual_cli_exit_with_pending_inspection_keeps_work_deadline_active() {
    let fixture = Fixture::new().await.unwrap();
    let work = fixture.work().unwrap();
    let (io, cli_exit) = fixture.exited_cli().await.unwrap();
    let cause = CauseLatch::default();
    let stop_calls = Cell::new(0);
    let stop = async {
        stop_calls.set(stop_calls.get() + 1);
        negative_stop()
    };
    let mut supervisor =
        Supervisor::new(io, elapsed_deadline().await, cause.clone(), stop).with_work(work.clone());
    let entered = Cell::new(false);
    let (release, released) = oneshot::channel::<()>();
    let mut inspection = Box::pin(async {
        entered.set(true);
        let _ = released.await;
        // Only now run the actual native producer; the gate minted no proof.
        work.observe().await.map(|_| ())
    });
    let mut drive = Box::pin(supervisor.drive(inspection.as_mut()));
    let selected = poll_until(drive.as_mut(), || {
        cause.get() == Some(Cause::Deadline) && stop_calls.get() == 1
    })
    .await;
    let deadline_selected_before_inspection = matches!(&selected, Poll::Pending)
        && entered.get()
        && !work.terminal_proven()
        && cause.get() == Some(Cause::Deadline);
    let released = release.send(()).is_ok();
    let result = match selected {
        Poll::Ready(result) => result,
        Poll::Pending => drive.as_mut().await,
    };
    drop(drive);
    drop(inspection);
    let collected = collect(&mut supervisor).await;
    assert!(result.is_ok() && released && deadline_selected_before_inspection);
    assert_eq!(cause.get(), Some(Cause::Deadline));
    assert_cli_collected(&collected, cli_exit);
}

#[tokio::test(flavor = "current_thread")]
async fn selected_deadline_survives_later_exact_terminal_proof_and_pending_callback() {
    let fixture = Fixture::new().await.unwrap();
    let work = fixture.work().unwrap();
    let (io, cli_exit) = fixture.exited_cli().await.unwrap();
    let cause = CauseLatch::default();
    let stop_calls = Cell::new(0);
    let stop = async {
        stop_calls.set(stop_calls.get() + 1);
        negative_stop()
    };
    let mut supervisor =
        Supervisor::new(io, elapsed_deadline().await, cause.clone(), stop).with_work(work.clone());
    let callback_calls = Cell::new(0);
    let callback_done = Cell::new(false);
    let (inspect_release, inspect_gate) = oneshot::channel::<()>();
    let (callback_release, callback_gate) = oneshot::channel::<()>();
    let mut operation = Box::pin(async {
        let _ = inspect_gate.await;
        work.observe().await?;
        callback_calls.set(callback_calls.get() + 1);
        let _ = callback_gate.await;
        callback_done.set(true);
        Ok::<(), anyhow::Error>(())
    });
    let mut drive = Box::pin(supervisor.drive(operation.as_mut()));
    let first = poll_until(drive.as_mut(), || {
        cause.get() == Some(Cause::Deadline) && stop_calls.get() == 1
    })
    .await;
    let selected_before_proof = matches!(&first, Poll::Pending) && !work.terminal_proven();
    let inspected = inspect_release.send(()).is_ok();
    let late = match first {
        Poll::Ready(result) => Poll::Ready(result),
        Poll::Pending => {
            poll_until(drive.as_mut(), || {
                work.terminal_proven() && callback_calls.get() == 1
            })
            .await
        }
    };
    let cause_while_late_callback_pending = cause.get();
    let proof_before_callback_release =
        work.terminal_proven() && callback_calls.get() == 1 && !callback_done.get();
    let released = callback_release.send(()).is_ok();
    let result = match late {
        Poll::Ready(result) => result,
        Poll::Pending => drive.as_mut().await,
    };
    drop(drive);
    drop(operation);
    let collected = collect(&mut supervisor).await;
    assert!(result.is_ok() && inspected && released && selected_before_proof);
    assert!(proof_before_callback_release && callback_done.get());
    assert_eq!(callback_calls.get(), 1);
    assert_eq!(cause_while_late_callback_pending, Some(Cause::Deadline));
    assert_eq!(
        cause.get(),
        Some(Cause::Deadline),
        "late proof must not clear timed_out's source cause"
    );
    assert_cli_collected(&collected, cli_exit);
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_proof_retains_late_callback_error_and_exempts_only_real_proof_from_fallback() {
    // Exercises the exact shared decision called by both outer wrappers. The
    // late error is this real local callback's error, not an injected OS capture
    // fault in an actual owner. That separate IO-F01 coverage remains open.
    let fixture = Fixture::new().await.unwrap();
    let work = fixture.work().unwrap();
    let _ = work.observe().await.unwrap();
    let (io, cli_exit) = fixture.exited_cli().await.unwrap();
    let cause = CauseLatch::default();
    let stop = async { negative_stop() };
    let mut supervisor =
        Supervisor::new(io, elapsed_deadline().await, cause.clone(), stop).with_work(work.clone());
    let (release, released) = oneshot::channel::<()>();
    let mut callback = Box::pin(async {
        let _ = released.await;
        Err::<(), _>(anyhow::anyhow!(
            "UNIT_REAL_CALLBACK_LATE_ERROR_NOT_IO_FAULT"
        ))
    });
    let mut drive = Box::pin(supervisor.drive(callback.as_mut()));
    let first = poll_once(drive.as_mut()).await;
    let pending_callback = matches!(&first, Poll::Pending);
    let released = release.send(()).is_ok();
    let callback_error = match first {
        Poll::Ready(result) => result,
        Poll::Pending => drive.as_mut().await,
    };
    drop(drive);
    drop(callback);
    // Retain the actual callback Result until after IO collection and fallback
    // decision. This is not a fabricated OS capture failure or NativeStop proof.
    let collected = collect(&mut supervisor).await;
    let fallback_calls = Cell::new(0);
    let decision = outer_fallback_decision(&work, None);
    let stopped = resolve_outer_fallback(decision, || async {
        fallback_calls.set(fallback_calls.get() + 1);
        false
    })
    .await;
    let no_proof = WorkObservation::new(None);
    let no_proof_calls = Cell::new(0);
    let no_proof_decision = outer_fallback_decision(&no_proof, None);
    let no_proof_stopped = resolve_outer_fallback(no_proof_decision, || async {
        no_proof_calls.set(no_proof_calls.get() + 1);
        false
    })
    .await;
    assert!(released && pending_callback && work.terminal_proven());
    assert!(callback_error.as_ref().is_err_and(|error| {
        error
            .to_string()
            .contains("UNIT_REAL_CALLBACK_LATE_ERROR_NOT_IO_FAULT")
    }));
    assert_eq!(decision, Some(true));
    assert!(stopped);
    assert_eq!(fallback_calls.get(), 0);
    assert_eq!(no_proof_decision, None);
    assert!(!no_proof_stopped);
    assert_eq!(no_proof_calls.get(), 1);
    assert_eq!(cause.get(), None);
    assert_cli_collected(&collected, cli_exit);
}

#[tokio::test(flavor = "current_thread")]
async fn direct_native_created_receipt_binds_its_weaker_proof_and_disarms_deadline() {
    let (_temporary, program) = native_program().unwrap();
    let runtime = Runtime::with_program(RuntimeKind::Docker, &program).unwrap();
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
    // The private instance comes from the actual generic create/start path.
    // No CreatedInstance, RuntimeState or terminal proof is constructed here.
    let execution = runtime
        .execute_retained(&spec, None, Duration::from_secs(120), 1024, |_| async {
            Ok(())
        })
        .await
        .unwrap();
    let work = WorkObservation::for_direct(&runtime, &execution.instance).unwrap();
    let matching = work
        .require_direct_binding(&runtime, &execution.instance)
        .is_ok();
    let other_owner = Runtime::with_program(RuntimeKind::Docker, &program).unwrap();
    let wrong_owner_rejected = work
        .require_direct_binding(&other_owner, &execution.instance)
        .is_err();
    let wrong_target_rejected = work
        .require_direct_target(&runtime, "not-the-created-target")
        .is_err();
    let _ = work.inspect().await.unwrap();
    let proved_before_callback = work.terminal_proven();
    let (io, cli_exit) = exited_command(runtime.command_async().await.unwrap())
        .await
        .unwrap();
    let cause = CauseLatch::default();
    let stop_calls = Cell::new(0);
    let stop = async {
        stop_calls.set(stop_calls.get() + 1);
        negative_stop()
    };
    let mut supervisor =
        Supervisor::new(io, elapsed_deadline().await, cause.clone(), stop).with_work(work.clone());
    let entered = Cell::new(false);
    let (release, released) = oneshot::channel::<()>();
    let mut callback = Box::pin(async {
        entered.set(true);
        let _ = released.await;
        Ok::<(), anyhow::Error>(())
    });
    let mut drive = Box::pin(supervisor.drive(callback.as_mut()));
    let first = poll_once(drive.as_mut()).await;
    let pending_while_due =
        matches!(&first, Poll::Pending) && entered.get() && cause.get().is_none();
    let released = release.send(()).is_ok();
    let result = match first {
        Poll::Ready(result) => result,
        Poll::Pending => drive.as_mut().await,
    };
    drop(drive);
    drop(callback);
    let collected = collect(&mut supervisor).await;
    assert!(result.is_ok() && released && pending_while_due && proved_before_callback);
    assert!(matching && wrong_owner_rejected && wrong_target_rejected);
    assert!(matches!(
        work.terminal_evidence(),
        Some(StopEvidence::Bare(_))
    ));
    assert_eq!(cause.get(), None);
    assert_eq!(stop_calls.get(), 0);
    assert_cli_collected(&collected, cli_exit);
}

#[tokio::test(flavor = "current_thread")]
async fn contradicted_terminal_shortcut_requires_the_first_real_native_reconciliation() {
    let fixture = Fixture::new().await.unwrap();
    let work = fixture.work().unwrap();
    let _ = work.observe().await.unwrap();
    let cause = CauseLatch::default();
    cause.latch(Cause::OperationFailure);
    // Call the actual private producer used by execute_created. No positive
    // NativeStop or terminal evidence is constructed by this test.
    let shortcut = fixture
        .managed
        .runtime
        .stop_batch_native(
            &fixture._retained.execution.instance,
            Some((&fixture.managed, &fixture.created.observation)),
            &work,
            &cause,
        )
        .await;
    let shortcut_did_not_claim_fallback = !shortcut.moved_outer_fallback;
    let coherent_decision = outer_fallback_decision(&work, Some(&shortcut));
    let coherent_calls = Cell::new(0);
    let coherent_stopped = resolve_outer_fallback(coherent_decision, || async {
        coherent_calls.set(coherent_calls.get() + 1);
        false
    })
    .await;

    // Change only this software fixture's native backend state, then consume
    // an actual new inspect from the same pinned program/scope. This is not a
    // JSON proof constructor, an actual worker restart or a lifecycle permit.
    fs::write(fixture._temporary.path().join("state"), "running").unwrap();
    let contradictory_observation = work.observe().await;
    let contradicted_decision = outer_fallback_decision(&work, Some(&shortcut));
    let fallback_calls = Cell::new(0);
    let actual_reconciliation = RefCell::new(None);
    let stopped = resolve_outer_fallback(contradicted_decision, || async {
        fallback_calls.set(fallback_calls.get() + 1);
        // Exactly the Managed wrapper's existing strict fallback, including
        // handle/source/configuration/authority checks and native IO joins.
        let result = fixture.managed.stop_snapshot_owned(&fixture.created).await;
        let stopped = result.is_ok();
        actual_reconciliation.replace(Some(result));
        stopped
    })
    .await;
    let reconciliation = actual_reconciliation.into_inner();
    let log = fs::read_to_string(fixture._temporary.path().join("log")).unwrap();
    // Both the real shortcut call and the real reconciliation have finished.
    assert!(shortcut.observed_stopped && shortcut.errors.is_empty());
    assert!(shortcut_did_not_claim_fallback);
    assert_eq!(coherent_decision, Some(true));
    assert!(coherent_stopped);
    assert_eq!(coherent_calls.get(), 0);
    assert!(contradictory_observation.is_err());
    assert!(
        !work.terminal_proven(),
        "contradiction must retain the refusal"
    );
    assert_eq!(contradicted_decision, None);
    assert!(stopped);
    assert_eq!(fallback_calls.get(), 1);
    assert!(
        reconciliation
            .as_ref()
            .is_some_and(|result| result.as_ref().is_ok_and(|observed| {
                observed.id.as_deref() == Some(fixture.created.handle.id())
                    && observed.process.running == Some(false)
            }))
    );
    assert_eq!(
        log.lines().filter(|line| line.starts_with("kill ")).count(),
        1
    );
    assert_eq!(cause.get(), Some(Cause::OperationFailure));
}

#[tokio::test(flavor = "current_thread")]
async fn performed_native_reconciliation_is_not_retried_after_a_later_contradiction() {
    let fixture = Fixture::new().await.unwrap();
    let work = fixture.work().unwrap(); // No terminal proof has been observed.
    let cause = CauseLatch::default();
    cause.latch(Cause::OperationFailure);
    fs::write(fixture._temporary.path().join("state"), "running").unwrap();
    // The exact producer must run the real native fallback this time. Its
    // result/evidence comes from actual guarded inspect/kill/inspect, not DATA.
    let performed = fixture
        .managed
        .runtime
        .stop_batch_native(
            &fixture._retained.execution.instance,
            Some((&fixture.managed, &fixture.created.observation)),
            &work,
            &cause,
        )
        .await;
    let _ = work.observe().await.unwrap();
    fs::write(fixture._temporary.path().join("state"), "running").unwrap();
    let contradictory_observation = work.observe().await;
    let decision = outer_fallback_decision(&work, Some(&performed));
    let duplicate_calls = Cell::new(0);
    let previously_observed_stopped = resolve_outer_fallback(decision, || async {
        duplicate_calls.set(duplicate_calls.get() + 1);
        false
    })
    .await;
    let log = fs::read_to_string(fixture._temporary.path().join("log")).unwrap();
    // No live engine/process is simulated by state=running: this fixture's
    // original Child is already joined. The contradiction remains an error.
    assert!(performed.moved_outer_fallback && performed.observed_stopped);
    assert!(performed.errors.is_empty());
    assert!(matches!(
        &performed.evidence,
        Some(StopEvidence::Managed(_))
    ));
    assert!(contradictory_observation.is_err() && !work.terminal_proven());
    assert_eq!(decision, Some(true));
    assert!(previously_observed_stopped);
    assert_eq!(duplicate_calls.get(), 0);
    assert_eq!(
        log.lines().filter(|line| line.starts_with("kill ")).count(),
        1
    );
    assert_eq!(cause.get(), Some(Cause::OperationFailure));
}
