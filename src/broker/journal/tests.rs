//! Test-only fault points. Every point is reached after the real filesystem operation.

use super::{
    BindingInputs, ControlSecrets, FaultStage, FsExclusions, Journal, JournalError,
    JournalLimitsV1, JournalResult, JournalScopeV1, RejectionCodeV1, RejectionPhaseV1,
    ReserveResult,
};
use crate::{
    broker::intent::ExecutionIntentV1, control_fs::Directory, runner::CheckpointPayloadV1,
};
use serde_json::{Value, json};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

thread_local! {
    static FAIL_ONCE: Cell<Option<FaultStage>> = const { Cell::new(None) };
    static CRASH_ADVANCE_ARMED: Cell<bool> = const { Cell::new(false) };
    static MOVE_BINDING_ONCE: RefCell<Option<(FaultStage, PathBuf, PathBuf)>> = const { RefCell::new(None) };
}

pub(super) fn checkpoint(stage: FaultStage) -> JournalResult<()> {
    let move_binding = MOVE_BINDING_ONCE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot
            .as_ref()
            .is_some_and(|(expected, _, _)| *expected == stage)
        {
            slot.take()
        } else {
            None
        }
    });
    if let Some((_, source, backup)) = move_binding {
        std::fs::rename(source, backup).map_err(|_| JournalError::Io)?;
    }
    if stage == FaultStage::WriterLockAbsent
        && let (Ok(barrier), Ok(release)) = (
            std::env::var("AI_GUARD_JOURNAL_LOCK_BARRIER"),
            std::env::var("AI_GUARD_JOURNAL_LOCK_RELEASE"),
        )
    {
        std::fs::write(barrier, b"observed-absent-before-create").map_err(|_| JournalError::Io)?;
        let release = Path::new(&release);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !release.exists() {
            if Instant::now() >= deadline {
                return Err(JournalError::Io);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    if let (Ok(expected), Ok(barrier)) = (
        std::env::var("AI_GUARD_JOURNAL_CRASH_STAGE"),
        std::env::var("AI_GUARD_JOURNAL_CRASH_BARRIER"),
    ) {
        let phase = std::env::var("AI_GUARD_JOURNAL_CRASH_PHASE").unwrap_or_default();
        let armed = CRASH_ADVANCE_ARMED.with(Cell::get);
        let stage_match = expected == format!("{stage:?}")
            || expected == "BeforeAckAdvance" && stage == FaultStage::BeforeAck;
        if stage_match && (phase != "advance" || armed) {
            std::fs::write(barrier, b"reached-after-operation").map_err(|_| JournalError::Io)?;
            loop {
                thread::park_timeout(Duration::from_secs(60));
            }
        }
    }
    let fail = FAIL_ONCE.with(|slot| {
        if slot.get() == Some(stage) {
            slot.set(None);
            true
        } else {
            false
        }
    });
    if fail { Err(JournalError::Io) } else { Ok(()) }
}

fn fail_once_at(stage: FaultStage) {
    FAIL_ONCE.with(|slot| {
        assert!(
            slot.replace(Some(stage)).is_none(),
            "only one pending fault is supported"
        );
    });
}

#[test]
fn fault_hook_dispatch_is_one_shot_and_thread_local() {
    fail_once_at(FaultStage::BeforeAck);
    thread::spawn(|| assert_eq!(checkpoint(FaultStage::BeforeAck), Ok(())))
        .join()
        .unwrap();
    assert_eq!(checkpoint(FaultStage::BeforeAck), Err(JournalError::Io));
}

#[test]
fn injected_journal_io_error_never_returns_ack_and_recovery_is_conservative() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = open(temporary.path());

    fail_once_at(FaultStage::ReservationDurable);
    assert!(matches!(
        journal.reserve(operation(), submit()),
        Err(JournalError::Io)
    ));
    assert!(matches!(
        journal.reserve(operation(), submit()).unwrap(),
        ReserveResult::Interrupted
    ));

    let ticket = match journal
        .reserve(second_operation(), br#"{"action":"accept-failure"}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("new second operation requires its reservation ticket"),
    };
    let intent = intent_for(second_operation());
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    fail_once_at(FaultStage::BeforeAck);
    assert!(matches!(
        journal.accept(ticket, &intent, BindingInputs::None, &initial),
        Err(JournalError::Io)
    ));
    let replay = match journal
        .reserve(second_operation(), br#"{"action":"accept-failure"}"#)
        .unwrap()
    {
        ReserveResult::Accepted(job) => job,
        _ => panic!("durable accepted graph must be replayed without a false ACK"),
    };
    assert_eq!(replay.head().revision(), 0);

    fail_once_at(FaultStage::CurrentPublished);
    assert!(matches!(
        journal.advance(second_operation(), replay.head().clone(), &initial),
        Err(JournalError::Io)
    ));
    assert_eq!(
        journal
            .load(second_operation())
            .unwrap()
            .unwrap()
            .head()
            .revision(),
        1
    );
}

#[test]
fn injected_rejection_io_errors_never_return_ack_and_exact_replay_recovers() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = open(temporary.path());
    let operations = [
        (operation(), FaultStage::RejectedWritten),
        (second_operation(), FaultStage::RejectedDurable),
        (
            Uuid::parse_str("55555555-5555-4555-8555-555555555555").unwrap(),
            FaultStage::BeforeAck,
        ),
    ];

    for (operation, stage) in operations {
        let ticket = match journal.reserve(operation, submit()).unwrap() {
            ReserveResult::New(ticket) => ticket,
            _ => panic!("reject test requires a fresh ticket"),
        };
        fail_once_at(stage);
        assert!(matches!(
            journal.reject(
                ticket,
                RejectionPhaseV1::SubmissionPreparation,
                RejectionCodeV1::PreparationFailed,
            ),
            Err(JournalError::Io)
        ));

        let replay = match journal.reserve(operation, submit()).unwrap() {
            ReserveResult::Rejected(receipt) => receipt,
            other => panic!("replay after durable rejection returned {other:?}"),
        };
        assert_eq!(replay.operation_id(), operation);
        assert!(matches!(
            journal.status(operation).unwrap(),
            Some(super::JournalStatusV1::Rejected(receipt))
                if receipt.record_hash() == replay.record_hash()
        ));
    }
}

#[test]
fn lifecycle_adapter_retains_observation_of_an_inherited_partial_job() {
    use crate::broker::{
        checkpoint::{BrokerCheckpointV2, SupervisionCheckpointV1},
        lifecycle::BrokerCheckpointV3,
    };

    let temporary = tempfile::tempdir().unwrap();
    let journal = open(temporary.path());
    let ReserveResult::New(ticket) = journal.reserve(operation(), submit()).unwrap() else {
        panic!("a fresh fixture requires its reservation ticket");
    };
    let job_path = journal.0.jobs.path().join(operation().to_string());
    drop(ticket);
    drop(journal);

    // Reopening must not first observe this inherited reservation via status.
    let journal = open(temporary.path());
    let intent = intent();
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let lifecycle = BrokerCheckpointV3::new(
        BrokerCheckpointV2::new(
            initial,
            SupervisionCheckpointV1::new(journal.scope().control_namespace_uuid(), vec![], vec![])
                .unwrap(),
        )
        .unwrap(),
        vec![],
    )
    .unwrap();
    let expected = super::RevisionRefV1::new(0, &"a".repeat(64)).unwrap();
    assert!(matches!(
        journal.advance_lifecycle_preserving_outbox(operation(), expected, &lifecycle),
        Err(JournalError::Interrupted)
    ));

    let preserved = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("preserved-partial-job");
    std::fs::rename(&job_path, &preserved).unwrap();
    assert!(preserved.join("reservation.json").is_file());
    assert!(matches!(
        journal.status(operation()),
        Err(JournalError::Conflict)
    ));
    assert!(matches!(
        journal.reserve(operation(), submit()),
        Err(JournalError::Conflict)
    ));
    assert!(
        !job_path.exists(),
        "no replacement reservation may be created"
    );
}

#[test]
fn full_writer_rechecks_binding_after_real_publication_before_ack() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = open(temporary.path());
    let base = temporary.path().canonicalize().unwrap();
    let source_path = base.join("manual-fixture.env");
    std::fs::write(&source_path, b"FIXTURE_KEY=synthetic-canary-123\n").unwrap();
    std::fs::set_permissions(&source_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let source = crate::control_fs::PrivateFile::open(&source_path, 64 * 1024).unwrap();
    let mut plan = plan_for(operation());
    plan["options"]["profile"] = json!("observe");
    plan["options"]["network"] = json!("bridge");
    plan["options"]["secret_env_file"] = json!(source_path);
    plan["request"]["profile"] = json!("observe");
    let intent = ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap();
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let ReserveResult::New(ticket) = journal.reserve(operation(), submit()).unwrap() else {
        panic!("fresh canary");
    };
    let accepted = journal
        .accept(
            ticket,
            &intent,
            BindingInputs::Manual { source: &source },
            &initial,
        )
        .unwrap();
    let blob = accepted.binding_source().unwrap().path().to_path_buf();
    let job_path = blob.parent().unwrap().parent().unwrap().to_path_buf();
    let first = journal
        .advance(operation(), accepted.head().clone(), &initial)
        .unwrap();
    assert_eq!(first.head().revision(), 1);
    drop(accepted);
    let backup = base.join("preserved-binding.bin");
    MOVE_BINDING_ONCE.with(|slot| {
        assert!(
            slot.borrow_mut()
                .replace((FaultStage::CurrentDurable, blob, backup.clone()))
                .is_none()
        );
    });
    assert!(
        journal
            .advance(operation(), first.head().clone(), &initial)
            .is_err(),
        "a published revision cannot be acknowledged without its strict post-load binding"
    );
    assert!(MOVE_BINDING_ONCE.with(|slot| slot.borrow().is_none()));
    assert!(
        backup.is_file(),
        "fixture bytes are preserved outside the binding namespace"
    );
    let current: Value =
        serde_json::from_slice(&std::fs::read(job_path.join("current.json")).unwrap()).unwrap();
    assert_eq!(
        current["head"]["revision"],
        json!(2),
        "positive publication control"
    );
    assert!(journal.load(operation()).is_err());
    let snapshot = super::StopJournal::from_full(&journal)
        .recovery_status(operation())
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.head().revision(), 2);
    assert!(snapshot.incomplete());
}

#[test]
fn recovered_full_admission_retains_lease_and_fails_before_ack_on_durability_error() {
    use crate::broker::writer::ProfileWriterCapabilities;
    let temporary = tempfile::tempdir().unwrap();
    let journal = open(temporary.path());
    let scope = journal.scope().clone();
    drop(journal);
    let root = Directory::open_existing(&temporary.path().canonicalize().unwrap()).unwrap();
    let report = super::recovery::StopRecoveryReaderV1::open_existing(
        &root,
        scope.clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(vec![]).unwrap(),
    )
    .unwrap()
    .unwrap()
    .scan();
    assert!(!report.incomplete());
    fail_once_at(FaultStage::NamespaceDurable);
    assert!(matches!(
        ProfileWriterCapabilities::from_recovery_report(&report, false, 4, Duration::from_secs(2)),
        Err(JournalError::Io)
    ));
    assert!(
        FAIL_ONCE.with(|slot| slot.get().is_none()),
        "the post-sync admission barrier must have been reached"
    );
    assert!(
        matches!(
            Journal::open(
                &root,
                scope,
                &FsExclusions::new(),
                JournalLimitsV1::default(),
                ControlSecrets::new(vec![]).unwrap()
            ),
            Err(JournalError::Busy)
        ),
        "failed admission must not release the report's lease"
    );
    let capabilities =
        ProfileWriterCapabilities::from_recovery_report(&report, false, 4, Duration::from_secs(2))
            .unwrap();
    assert!(
        !capabilities.is_stop_only(),
        "no facade escaped the failed attempt, so exact retry may admit once"
    );
}

fn intent_for(operation: Uuid) -> ExecutionIntentV1 {
    ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan_for(operation)).unwrap()).unwrap()
}

fn plan_for(operation: Uuid) -> Value {
    let mut plan: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/broker_intent-plan-v3.json"
    ))
    .unwrap();
    plan["operation_id"] = json!(operation);
    plan["version"] = json!(1);
    plan["options"]["profile"] = json!("strict");
    plan["options"]["network"] = json!("none");
    plan["options"]["provider"] = json!("generic");
    plan["options"]["argv"] = json!(["/usr/bin/id"]);
    for field in [
        "client",
        "auth_mode",
        "client_profile",
        "providers_file",
        "prompt",
        "model",
    ] {
        plan["options"].as_object_mut().unwrap().remove(field);
    }
    plan["request"]["profile"] = json!("strict");
    plan["request"]["agent_id"] = json!("generic-22222222-2222-4222-8222-222222222222");
    plan["request"]["command"]["argv"] = json!(["/usr/bin/id"]);
    plan["request"]
        .as_object_mut()
        .unwrap()
        .remove("provider_resolution");
    plan["provider_input"] = Value::Null;
    plan["provider_credential"] = Value::Null;
    plan
}

fn intent() -> ExecutionIntentV1 {
    intent_for(operation())
}

fn open(root: &Path) -> Journal {
    #[cfg(unix)]
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let canonical_root = root.canonicalize().unwrap();
    let root = Directory::open_existing(&canonical_root).unwrap();
    Journal::open(
        &root,
        JournalScopeV1::new(
            Uuid::from_u128(0x11111111111141118111111111111111),
            "tenant-journal-crash",
            "http://127.0.0.1:8787",
            Uuid::from_u128(0x22222222222242228222222222222222),
            "broker-test",
        )
        .unwrap(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap()
}

fn operation() -> Uuid {
    Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap()
}
fn second_operation() -> Uuid {
    Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap()
}
fn submit() -> &'static [u8] {
    br#"{"action":"crash-test"}"#
}

/// A manual diagnostic, not a latency threshold or a runtime qualification.
/// Measures real FS calls without changing the normal Journal/Writer path.
/// The 600-second timeout is cooperative: synchronous filesystem calls may
/// delay observation, and caller cancellation never cancels accepted work.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "opt-in bounded Journal/Writer cost measurement; no runtime or provider"]
async fn measure_durable_journal_cost_without_runtime() {
    use crate::{
        broker::{
            checkpoint::{BrokerCheckpointV2, SupervisionCheckpointV1},
            journal::{JournalStatusV1, RevisionRefV1},
            lifecycle::BrokerCheckpointV3,
            writer::WriterClient,
        },
        control_fs::cost_probe::Operation,
    };

    tokio::time::timeout(Duration::from_secs(600), async {
        let temporary = tempfile::Builder::new()
            .prefix("ai-guard-journal-cost-")
            .tempdir()
            .unwrap_or_else(|_| panic!("cost fixture private directory unavailable"));
        // This entirely new private fixture survives a failed/unknown sample.
        // Its path remains in memory and is never part of a diagnostic.
        let root = temporary
            .path()
            .canonicalize()
            .unwrap_or_else(|_| panic!("cost fixture root resolution failed"));
        let _retained = temporary.keep();
        let journal = open(&root);
        let operation_id = operation();
        let intent = intent_for(operation_id);
        let initial = CheckpointPayloadV1::initial(&intent).unwrap();
        let ReserveResult::New(ticket) = journal.reserve(operation_id, submit()).unwrap() else {
            panic!("cost fixture requires a fresh reservation");
        };
        let accepted = journal
            .accept(ticket, &intent, BindingInputs::None, &initial)
            .unwrap();
        assert_eq!(accepted.head().revision(), 0);
        let mut head = accepted.head().clone();
        let lifecycle = BrokerCheckpointV3::new(
            BrokerCheckpointV2::new(
                initial,
                SupervisionCheckpointV1::new(
                    journal.scope().control_namespace_uuid(),
                    vec![],
                    vec![],
                )
                .unwrap(),
            )
            .unwrap(),
            vec![],
        )
        .unwrap();
        // Only this diagnostic fixture uses a 60-second caller wait. This does
        // not cancel an accepted actor command; product defaults are intact.
        let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(60)).unwrap();

        for (target, load_syncs, advance_syncs) in [(0, 6, 16), (8, 14, 32)] {
            // Setup uses the normal writer path and actual acknowledged chain.
            // It is outside every sample.
            while head.revision() < target {
                let receipt = writer
                    .advance_lifecycle(operation_id, head.clone(), lifecycle.clone())
                    .await
                    .unwrap();
                assert!(
                    receipt.operation_id() == operation_id,
                    "receipt operation changed"
                );
                assert_eq!(receipt.head().revision(), head.revision() + 1);
                head = receipt.head().clone();
            }
            assert_eq!(head.revision(), target);

            let (loaded, first) = measure_writer_cost(
                &root,
                "load_first",
                target,
                writer.load(operation_id),
                |job| job.as_ref().map(|job| job.head().revision()),
            )
            .await;
            let loaded = loaded.expect("accepted cost fixture must remain present");
            assert!(loaded.head() == &head, "load must confirm the current head");
            // The original RED observed zero calls here after a real load.
            // The head-zero oracle remains six real file synchronizations.
            assert_eq!(
                first.metric(Operation::FileSync).calls,
                load_syncs,
                "real load must observe every file synchronization"
            );
            verify_cost_snapshot(&first, load_syncs, 3);

            let (loaded, second) = measure_writer_cost(
                &root,
                "load_second",
                target,
                writer.load(operation_id),
                |job| job.as_ref().map(|job| job.head().revision()),
            )
            .await;
            assert!(
                loaded.unwrap().head() == &head,
                "repeated load changed head"
            );
            verify_cost_snapshot(&second, load_syncs, 3);

            let (status, status_cost) = measure_writer_cost(
                &root,
                "status",
                target,
                writer.status(operation_id),
                |status| match status {
                    Some(JournalStatusV1::Accepted(job)) => Some(job.head().revision()),
                    _ => None,
                },
            )
            .await;
            assert!(
                matches!(status, Some(JournalStatusV1::Accepted(job)) if job.head() == &head),
                "status must confirm the same accepted head"
            );
            verify_cost_snapshot(&status_cost, load_syncs, 3);

            let before = head.clone();
            let (receipt, advanced) = measure_writer_cost(
                &root,
                "advance_lifecycle",
                target,
                writer.advance_lifecycle(operation_id, before.clone(), lifecycle.clone()),
                |receipt| Some(receipt.head().revision()),
            )
            .await;
            assert!(
                receipt.operation_id() == operation_id,
                "receipt operation changed"
            );
            assert_eq!(receipt.head().revision(), target + 1);
            verify_cost_snapshot(&advanced, advance_syncs, 9);
            assert_eq!(advanced.metric(Operation::Create).calls, 2);
            assert_eq!(advanced.metric(Operation::Publish).calls, 1);
            head = receipt.head().clone();

            // These functional checks are deliberately outside timed samples.
            let confirmed = journal.load(operation_id).unwrap().unwrap();
            assert!(
                confirmed.head() == &head,
                "ACK must identify the strict confirmed head"
            );
            assert!(confirmed.broker_checkpoint_v3().is_some());
            let mut wrong_hash = "f".repeat(64);
            if wrong_hash == head.hash() {
                wrong_hash = "e".repeat(64);
            }
            let wrong_cas = RevisionRefV1::new(head.revision(), &wrong_hash).unwrap();
            assert!(
                wrong_cas.hash() != head.hash(),
                "CAS fixture requires a different hash"
            );
            assert!(
                matches!(
                    writer
                        .advance_lifecycle(operation_id, wrong_cas, lifecycle.clone(),)
                        .await,
                    Err(JournalError::Conflict)
                ),
                "wrong CAS must be refused"
            );
            assert!(
                journal.load(operation_id).unwrap().unwrap().head() == &head,
                "refused CAS must preserve the acknowledged chain"
            );
        }
    })
    .await
    .expect("durable cost measurement exceeded its cooperative 600-second observation bound");
}

async fn measure_writer_cost<T>(
    root: &Path,
    stage: &'static str,
    head_before: u64,
    call: impl std::future::Future<Output = JournalResult<T>>,
    head_after: impl FnOnce(&T) -> Option<u64>,
) -> (T, crate::control_fs::cost_probe::Snapshot) {
    use crate::control_fs::cost_probe::Measurement;

    let measurement = Measurement::start(root).unwrap();
    let started = Instant::now();
    let result = call.await;
    let caller_ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    let snapshot = measurement.finish().unwrap();
    let observed_head = result.as_ref().ok().and_then(head_after);
    let outcome = match &result {
        Ok(_) => "ok",
        Err(JournalError::InvalidInput) => "invalid_input",
        Err(JournalError::UnsupportedVersion) => "unsupported_version",
        Err(JournalError::TooLarge) => "too_large",
        Err(JournalError::LimitExceeded) => "limit_exceeded",
        Err(JournalError::Busy) => "busy",
        Err(JournalError::Conflict) => "conflict",
        Err(JournalError::Interrupted) => "interrupted",
        Err(JournalError::Corrupt) => "corrupt",
        Err(JournalError::UnsafeFilesystem) => "unsafe_filesystem",
        Err(JournalError::ControlSecret) => "control_secret",
        Err(JournalError::Io) => "io_or_unknown_completion",
    };
    eprintln!(
        "durable_journal_cost {}",
        json!({
            "stage": stage,
            "head_before": head_before,
            "head_after": observed_head,
            "outcome": outcome,
            "caller_ns": caller_ns,
            "span_durations": "inclusive_and_overlapping",
            "sync_all_spans": "exact_std_call",
            "debug_assertions": cfg!(debug_assertions),
            "snapshot": snapshot,
        })
    );
    assert_eq!(
        snapshot.outstanding, 0,
        "completed measurement has unfinished spans"
    );
    let value = result.unwrap_or_else(|error| panic!("cost measurement failed: {error:?}"));
    (value, snapshot)
}

fn verify_cost_snapshot(
    snapshot: &crate::control_fs::cost_probe::Snapshot,
    file_syncs: u64,
    directory_syncs: u64,
) {
    use crate::control_fs::cost_probe::Operation;

    for (operation, expected) in [
        (Operation::FileSync, file_syncs),
        (Operation::FileSyncAll, file_syncs),
        (Operation::DirectorySync, directory_syncs),
        (Operation::DirectorySyncAll, directory_syncs),
    ] {
        let metric = snapshot.metric(operation);
        assert_eq!(
            metric.calls, expected,
            "synchronization call count mismatch"
        );
        assert_eq!(
            metric.succeeded, expected,
            "synchronization did not succeed"
        );
        assert_eq!(metric.failed, 0);
        assert_eq!(metric.abandoned, 0);
    }
    for operation in [
        Operation::Read,
        Operation::ReadToEnd,
        Operation::Revalidate,
        Operation::ReadAt,
        Operation::DirectoryRevalidate,
    ] {
        let metric = snapshot.metric(operation);
        assert!(
            metric.calls > 0,
            "real validation/read calls must be observed"
        );
        assert_eq!(metric.calls, metric.succeeded);
        assert_eq!(metric.failed, 0);
        assert_eq!(metric.abandoned, 0);
    }
    assert!(snapshot.metric(Operation::ReadToEnd).bytes > 0);
    assert!(snapshot.metric(Operation::ReadAt).bytes > 0);
}

#[test]
#[ignore = "spawned only by the parent SIGKILL test"]
fn crash_child_at_stage() {
    let (Ok(stage), Ok(root)) = (
        std::env::var("AI_GUARD_JOURNAL_CRASH_STAGE"),
        std::env::var("AI_GUARD_JOURNAL_CRASH_ROOT"),
    ) else {
        return;
    };
    let journal = open(Path::new(&root));
    let ticket = match journal.reserve(operation(), submit()).unwrap() {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("child requires a fresh operation"),
    };
    let phase = std::env::var("AI_GUARD_JOURNAL_CRASH_PHASE").unwrap_or_else(|_| "accept".into());
    if stage != "ReservationDurable" {
        let intent = intent();
        let initial = CheckpointPayloadV1::initial(&intent).unwrap();
        let accepted = journal
            .accept(ticket, &intent, BindingInputs::None, &initial)
            .unwrap();
        if phase == "advance" {
            CRASH_ADVANCE_ARMED.with(|armed| armed.set(true));
            let _ = journal.advance(operation(), accepted.head().clone(), &initial);
        }
    }
    panic!("configured crash stage was not reached: {stage}");
}

#[test]
fn sigkill_at_durable_boundaries_never_returns_a_fresh_ticket() {
    enum Recovery {
        Interrupted,
        Accepted(u64),
        OrphanBeforePublish,
    }
    for (stage, phase, recovery) in [
        ("ReservationDurable", "accept", Recovery::Interrupted),
        ("IntentDurable", "accept", Recovery::Interrupted),
        ("AcceptedWritten", "accept", Recovery::Accepted(0)),
        ("AcceptedDurable", "accept", Recovery::Accepted(0)),
        ("BeforeAck", "accept", Recovery::Accepted(0)),
        ("RevisionDurable", "advance", Recovery::OrphanBeforePublish),
        ("CurrentPublished", "advance", Recovery::Accepted(1)),
        ("CurrentDurable", "advance", Recovery::Accepted(1)),
        ("BeforeAckAdvance", "advance", Recovery::Accepted(1)),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let barrier = temporary.path().join("reached");
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "broker::journal::tests::crash_child_at_stage",
                "--nocapture",
            ])
            .env("AI_GUARD_JOURNAL_CRASH_STAGE", stage)
            .env("AI_GUARD_JOURNAL_CRASH_PHASE", phase)
            .env("AI_GUARD_JOURNAL_CRASH_ROOT", temporary.path())
            .env("AI_GUARD_JOURNAL_CRASH_BARRIER", &barrier)
            .arg("--ignored")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut child = ChildGuard(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !barrier.exists() && Instant::now() < deadline {
            if let Some(status) = child.0.try_wait().unwrap() {
                panic!("child exited before crash barrier: {status}");
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(barrier.exists(), "child did not reach {stage}");
        child.0.kill().unwrap();
        let _ = child.0.wait().unwrap();

        let journal = open(temporary.path());
        match recovery {
            Recovery::Interrupted => assert!(matches!(
                journal.reserve(operation(), submit()).unwrap(),
                ReserveResult::Interrupted
            )),
            Recovery::Accepted(revision) => match journal.reserve(operation(), submit()).unwrap() {
                ReserveResult::Accepted(job) => {
                    assert_eq!(job.head().revision(), revision, "stage {stage}")
                }
                _ => panic!("stage {stage} did not recover accepted state"),
            },
            Recovery::OrphanBeforePublish => {
                let job_dir = temporary.path().join("broker-journal-v1");
                let scope_dir = std::fs::read_dir(job_dir)
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                let job_dir = scope_dir.join("jobs").join(operation().to_string());
                assert!(job_dir.join("rev-00000000000000000001.json").exists());
                let current: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(job_dir.join("current.json")).unwrap())
                        .unwrap();
                assert_eq!(current["head"]["revision"], 0);
                match journal.reserve(operation(), submit()) {
                    Ok(ReserveResult::Accepted(job)) => assert_eq!(job.head().revision(), 0),
                    Err(JournalError::Interrupted | JournalError::Corrupt) => {}
                    _ => panic!("orphan revision must never be promoted"),
                }
            }
        }
    }
}

#[test]
#[cfg(unix)]
fn simultaneous_first_writer_creation_returns_busy() {
    let temporary = tempfile::tempdir().unwrap();
    std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let barrier = temporary.path().join("writer-lock-absent");
    let release = temporary.path().join("writer-lock-release");
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "broker::journal::tests::writer_lock_race_child",
                "--ignored",
                "--nocapture",
            ])
            .env("AI_GUARD_JOURNAL_LOCK_ROOT", temporary.path())
            .env("AI_GUARD_JOURNAL_LOCK_BARRIER", &barrier)
            .env("AI_GUARD_JOURNAL_LOCK_RELEASE", &release)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    while !barrier.exists() {
        assert!(
            Instant::now() < deadline,
            "child did not reach the actual absent-lock/create boundary"
        );
        thread::sleep(Duration::from_millis(10));
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "lock race child exited before barrier"
        );
    }
    // The child has observed no lock but has not attempted O_EXCL creation.
    // This real opener now creates and exclusively holds the writer lock.
    let writer = open(temporary.path());
    std::fs::write(&release, b"continue").unwrap();
    let status = child.0.wait().unwrap();
    assert!(
        status.success(),
        "second concurrent first opener did not return Busy"
    );
    drop(writer);
}

#[test]
#[cfg(unix)]
#[ignore = "spawned by simultaneous_first_writer_creation_returns_busy"]
fn writer_lock_race_child() {
    let Ok(root) = std::env::var("AI_GUARD_JOURNAL_LOCK_ROOT") else {
        return;
    };
    let canonical_root = Path::new(&root).canonicalize().unwrap();
    let root = Directory::open_existing(&canonical_root).unwrap();
    let result = Journal::open(
        &root,
        JournalScopeV1::new(
            Uuid::from_u128(0x11111111111141118111111111111111),
            "tenant-journal-crash",
            "http://127.0.0.1:8787",
            Uuid::from_u128(0x22222222222242228222222222222222),
            "broker-test",
        )
        .unwrap(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    );
    assert!(
        matches!(&result, Err(JournalError::Busy)),
        "second writer should observe Busy, got {:?}",
        result.as_ref().err()
    );
}
