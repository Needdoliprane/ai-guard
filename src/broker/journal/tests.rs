//! Test-only fault points. Every point is reached after the real filesystem operation.

use super::{
    BindingInputs, ControlSecrets, FaultStage, FsExclusions, Journal, JournalError,
    JournalLimitsV1, JournalResult, JournalScopeV1, JournalStatusV1, RejectionCodeV1,
    RejectionPhaseV1, ReserveResult, RevisionRefV1,
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
    sync::{Arc, OnceLock},
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
    static CURRENT_DURABLE_OBSERVER: RefCell<Option<Arc<OnceLock<Instant>>>> = const { RefCell::new(None) };
    static FAIL_ONCE: Cell<Option<FaultStage>> = const { Cell::new(None) };
    static CRASH_ADVANCE_ARMED: Cell<bool> = const { Cell::new(false) };
    static MOVE_BINDING_ONCE: RefCell<Option<(FaultStage, PathBuf, PathBuf)>> = const { RefCell::new(None) };
}

pub(in crate::broker) struct CurrentDurableObserver(Option<Arc<OnceLock<Instant>>>);
impl CurrentDurableObserver {
    pub(in crate::broker) fn install(observed: Arc<OnceLock<Instant>>) -> Self {
        Self(CURRENT_DURABLE_OBSERVER.replace(Some(observed)))
    }
}
impl Drop for CurrentDurableObserver {
    fn drop(&mut self) {
        CURRENT_DURABLE_OBSERVER.replace(self.0.take());
    }
}

pub(super) fn checkpoint(stage: FaultStage) -> JournalResult<()> {
    if stage == FaultStage::CurrentDurable {
        CURRENT_DURABLE_OBSERVER.with_borrow(|slot| {
            if let Some(observed) = slot {
                let _ = observed.set(Instant::now());
            }
        });
    }
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

pub(super) fn fail_once_at(stage: FaultStage) {
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
    use crate::broker::{
        checkpoint::{BrokerCheckpointV2, SupervisionCheckpointV1},
        journal::cost_measurement::WriterKind,
        lifecycle::BrokerCheckpointV3,
        writer::WriterClient,
    };

    tokio::time::timeout(Duration::from_secs(600), async {
        let mut output = StableCostOutput::default();
        let temporary = tempfile::Builder::new()
            .prefix("ai-guard-journal-cost-")
            .tempdir()
            .unwrap_or_else(|_| panic!("cost fixture private directory unavailable"));
        let root = temporary
            .path()
            .canonicalize()
            .unwrap_or_else(|_| panic!("cost fixture root resolution failed"));
        // A failed/unknown sample retains its new private fixture. Never print it.
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
        assert!(
            accepted.operation_id() == operation_id,
            "accepted operation changed"
        );
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
        // Only this diagnostic fixture uses this caller wait. Product defaults
        // and the cooperative 600-second overall observation bound are intact.
        let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(60)).unwrap();

        for target in [0, 8, 17] {
            // Prepare the actual acknowledged chain outside every measurement.
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
            for stage in ["load_first", "load_second"] {
                measure_stable_read_cost(
                    &root,
                    &journal,
                    &writer,
                    operation_id,
                    &head,
                    stage,
                    WriterKind::Load,
                    writer.load(operation_id),
                    |job| {
                        job.as_ref()
                            .map(|job| (job.operation_id(), job.head().clone()))
                    },
                    &mut output,
                )
                .await;
            }
            for stage in ["status_first", "status_second"] {
                measure_stable_read_cost(
                    &root,
                    &journal,
                    &writer,
                    operation_id,
                    &head,
                    stage,
                    WriterKind::Status,
                    writer.status(operation_id),
                    |status| match status {
                        Some(JournalStatusV1::Accepted(job)) => {
                            Some((job.operation_id(), job.head().clone()))
                        }
                        _ => None,
                    },
                    &mut output,
                )
                .await;
            }
            // Independent functional confirmation remains outside samples.
            let confirmed = journal.load(operation_id).unwrap().unwrap();
            assert!(
                confirmed.operation_id() == operation_id,
                "confirmed operation changed"
            );
            assert!(
                confirmed.head() == &head,
                "stable reads changed the confirmed head"
            );
            assert!(target == 0 || confirmed.broker_checkpoint_v3().is_some());
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
                        .advance_lifecycle(operation_id, wrong_cas, lifecycle.clone())
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

/// At most twelve complete sample lines, or a prefix plus an explicit marker.
/// The entire output, including prefix/newlines/marker, is capped at 16 KiB.
#[derive(Default)]
struct StableCostOutput {
    text: String,
    recorded: u64,
    omitted: u64,
}
impl StableCostOutput {
    const LIMIT: usize = 16 * 1024;
    const MARKER_RESERVE: usize = 256;
    fn record(&mut self, sample: Value) {
        let line = format!("durable_journal_cost {sample}\n");
        if self.omitted != 0
            || self.text.len().saturating_add(line.len()) > Self::LIMIT - Self::MARKER_RESERVE
        {
            self.omitted = self.omitted.saturating_add(1);
        } else {
            self.text.push_str(&line);
            self.recorded = self.recorded.saturating_add(1);
        }
    }
}
impl Drop for StableCostOutput {
    fn drop(&mut self) {
        if self.omitted != 0 {
            let marker = format!(
                "durable_journal_cost {}\n",
                json!({
                    "truncated": true, "samples_recorded": self.recorded,
                    "samples_omitted": self.omitted, "performance_qualified": false,
                })
            );
            // Three bounded counts (at most 12) and fixed labels fit the
            // reserved 256 bytes. No incomplete JSON line is ever emitted.
            self.text.push_str(&marker);
        }
        eprint!("{}", self.text);
    }
}

#[allow(clippy::too_many_arguments)]
async fn measure_stable_read_cost<T>(
    root: &Path,
    selected_journal: &Journal,
    writer: &crate::broker::writer::WriterClient,
    operation: Uuid,
    head: &RevisionRefV1,
    stage: &'static str,
    kind: super::cost_measurement::WriterKind,
    call: impl std::future::Future<Output = JournalResult<T>>,
    identity: impl FnOnce(&T) -> Option<(Uuid, RevisionRefV1)>,
    output: &mut StableCostOutput,
) {
    use crate::control_fs::cost_probe::{Measurement, Operation};

    let _exclusive = crate::control_fs::cost_probe::EXCLUSIVE.lock().await;
    let filesystem = Measurement::start(root).unwrap();
    let journal = super::cost_measurement::Measurement::start(
        selected_journal,
        writer.cost_context_for_test(),
        operation,
        kind,
    )
    .unwrap();
    let started = Instant::now();
    let result = call.await;
    let caller_ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    let journal = journal.finish().unwrap();
    let filesystem = filesystem.finish().unwrap();
    let coverage = journal.writer_read_coverage(kind);
    let observed = result.as_ref().ok().and_then(identity);
    // Retain complete fixed-schema lines only while the total budget allows.
    // Truncation is explicit; array order is declared in the first row.
    let fs_metrics: Vec<_> = [
        Operation::Read,
        Operation::ReadToEnd,
        Operation::Revalidate,
        Operation::ReadAt,
        Operation::DirectoryRevalidate,
        Operation::DirectorySync,
        Operation::DirectorySyncAll,
        Operation::FileSync,
        Operation::FileSyncAll,
        Operation::Create,
        Operation::Publish,
    ]
    .into_iter()
    .map(|operation| {
        let metric = filesystem.metric(operation);
        json!([
            operation,
            metric.calls,
            metric.succeeded,
            metric.failed,
            metric.abandoned,
            metric.bytes,
            metric.elapsed_ns
        ])
    })
    .collect();
    let mut sample = json!({
        "stage": stage, "head_before": head.revision(),
        "head_after": observed.as_ref().map(|value| value.1.revision()),
        "outcome": if result.is_ok() { "ok" } else { "error" },
        "caller_ns": caller_ns, "performance_qualified": false,
        "filesystem": fs_metrics, "filesystem_outstanding": filesystem.outstanding,
        "journal": journal, "matched_hooks": coverage,
    });
    if output.recorded == 0 && output.omitted == 0 {
        sample["schema"] = json!({
            "filesystem": ["operation", "calls", "succeeded", "failed", "abandoned", "bytes", "elapsed_ns"],
            "inventories": ["scope", "jobs", "job_files", "bindings"],
            "writer": ["attempted", "sent", "taken", "reply_ready", "actor_errors", "received",
                "caller_errors", "caller_abandoned", "actor_abandoned", "submit_to_take_samples",
                "submit_to_take_ns", "actor_ns", "caller_ns"],
            "reply_ready": "before_send_not_client_reception",
            "durations": "inclusive_and_overlapping",
        });
    }
    output.record(sample);
    assert!(
        coverage.is_complete(),
        "selected writer/journal hooks were not observed"
    );
    assert_eq!(
        filesystem.outstanding, 0,
        "filesystem measurement is incomplete"
    );
    assert_eq!(
        journal.caller_outstanding, 0,
        "caller measurement is incomplete"
    );
    assert_eq!(
        journal.actor_outstanding, 0,
        "actor measurement is incomplete"
    );
    assert_eq!(journal.context_conflicts, 0, "measurement context changed");
    assert_eq!(
        journal.missed_hooks, 0,
        "measurement hook coverage is incomplete"
    );
    assert_eq!(
        filesystem.metric(Operation::Create).calls,
        0,
        "read created a durable record"
    );
    assert_eq!(
        filesystem.metric(Operation::Publish).calls,
        0,
        "read published a durable record"
    );
    assert!(result.is_ok(), "stable read failed");
    let (observed_operation, observed_head) = observed.expect("accepted stable read missing");
    assert!(
        observed_operation == operation,
        "stable read operation changed"
    );
    assert!(
        &observed_head == head,
        "stable read changed the acknowledged head"
    );
}

async fn measure_writer_cost<T>(
    root: &Path,
    stage: &'static str,
    head_before: u64,
    call: impl std::future::Future<Output = JournalResult<T>>,
    head_after: impl FnOnce(&T) -> Option<u64>,
) -> (T, crate::control_fs::cost_probe::Snapshot) {
    use crate::control_fs::cost_probe::Measurement;

    let _exclusive = crate::control_fs::cost_probe::EXCLUSIVE.lock().await;
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

/// Copies only the explicitly selected private journal records. Imported scope
/// and historical receipts are fixture data, never authentication or permission.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "opt-in retained journal copy measurement; explicit private source required"]
async fn measure_retained_journal_copy_cost_without_runtime() {
    use crate::{broker::writer::WriterClient, control_fs::cost_probe::Operation};

    tokio::time::timeout(Duration::from_secs(600), async {
        let source = std::env::var_os("AI_GUARD_JOURNAL_COST_SOURCE_SCOPE")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .expect("explicit absolute source scope is required");
        let source = crate::control_fs::absolute_path(&source)
            .unwrap_or_else(|_| panic!("source scope path is unsupported"));
        let operation_id = std::env::var("AI_GUARD_JOURNAL_COST_SOURCE_OPERATION")
            .ok()
            .and_then(|value| Uuid::parse_str(&value).ok())
            .filter(|value| !value.is_nil())
            .expect("explicit source operation is required");
        // Caller wall time covers the entire copy preparation, including new
        // destination writes. Only filesystem spans below SOURCE are collected.
        let (copy, source_cost) = measure_writer_cost(
            &source,
            "copy_preparation_source_counters",
            30,
            async { copy_retained_cost_records(&source, operation_id) },
            |copy| Some(copy.head.revision()),
        )
        .await;
        for operation in [
            Operation::FileSync, Operation::FileSyncAll, Operation::DirectorySync,
            Operation::DirectorySyncAll, Operation::Create, Operation::Publish,
        ] {
            assert_eq!(source_cost.metric(operation).calls, 0,
                "source must receive no write or synchronization request");
        }
        for operation in [Operation::Read, Operation::ReadToEnd, Operation::Revalidate,
            Operation::ReadAt, Operation::DirectoryRevalidate] {
            let metric = source_cost.metric(operation);
            assert!(metric.calls > 0, "source reads must be observed");
            assert_eq!(metric.calls, metric.succeeded);
            assert_eq!(metric.failed, 0);
            assert_eq!(metric.abandoned, 0);
        }

        // Only the new copy is opened as a Journal. Its fresh local lock does
        // not authorize any historical claim, execution or external operation.
        let journal = Journal::open(&copy.root, copy.scope.clone(), &FsExclusions::new(),
            JournalLimitsV1::default(), ControlSecrets::new(vec![]).unwrap()).unwrap();
        let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(60)).unwrap();
        let mut confirmed = None;
        for stage in ["retained_load_first", "retained_load_second"] {
            let (loaded, cost) = measure_writer_cost(
                copy.root.path(), stage, 30, writer.load(operation_id),
                |job| job.as_ref().map(|job| job.head().revision()),
            ).await;
            let loaded = loaded.expect("copied journal must contain the selected operation");
            assert!(loaded.head() == &copy.head, "copied load must confirm the exact head");
            verify_cost_snapshot(&cost, 36, 3);
            assert_eq!(cost.metric(Operation::Create).calls, 0);
            assert_eq!(cost.metric(Operation::Publish).calls, 0);
            confirmed = Some(loaded);
        }
        let (status, cost) = measure_writer_cost(
            copy.root.path(), "retained_status", 30, writer.status(operation_id),
            |status| match status {
                Some(super::JournalStatusV1::Accepted(job)) => Some(job.head().revision()),
                _ => None,
            },
        ).await;
        assert!(matches!(status, Some(super::JournalStatusV1::Accepted(job)) if job.head() == &copy.head),
            "copied status must confirm the exact accepted head");
        verify_cost_snapshot(&cost, 36, 3);
        assert_eq!(cost.metric(Operation::Create).calls, 0);
        assert_eq!(cost.metric(Operation::Publish).calls, 0);

        let loaded = confirmed.unwrap();
        assert!(loaded.0.chain.first() == Some(&copy.initial), "initial checkpoint changed");
        // This is wall time of the real pure verifier over COPIED bytes. It is
        // neither CPU time nor an independently subtractible part of a load.
        let (verified, validation_cost) = measure_writer_cost(
            copy.root.path(), "chain_validation_in_memory", 30,
            async {
                let mut verifier = super::confirmed::ChainVerifierV1::new(
                    journal.scope(), &journal.0.control, loaded.intent(),
                    loaded.binding_manifest(), &copy.initial,
                )?;
                for (revision, bytes) in copy.revisions.iter().enumerate() {
                    verifier.push(revision as u64, bytes)?;
                }
                verifier.finish(loaded.head())
            },
            |verified| verified.chain.last().map(|head| head.revision()),
        ).await;
        for operation in [Operation::Read, Operation::ReadToEnd, Operation::Revalidate,
            Operation::ReadAt, Operation::DirectoryRevalidate, Operation::DirectorySync,
            Operation::DirectorySyncAll, Operation::FileSync, Operation::FileSyncAll,
            Operation::Create, Operation::Publish] {
            assert_eq!(validation_cost.metric(operation).calls, 0,
                "in-memory verification must perform no filesystem operation");
        }
        assert!(verified.chain == loaded.0.chain, "verified chain differs from strict load");
        assert_eq!(verified.chain.len(), 31);
        assert!(verified.chain.last() == Some(&copy.head), "verified head changed");
        assert_eq!(verified.legacy_managed_seen, loaded.0.legacy_managed_seen);
        assert_eq!(verified.last_cancel_operations, loaded.0.last_cancel_operations);
        assert_eq!(verified.command_receipts.len(), loaded.0.command_receipts.len());
        assert_eq!(verified.outbox_receipts.len(), loaded.0.outbox_receipts.len());
        let same_payload = match (&verified.payload, &loaded.0.checkpoint) {
            (super::LoadedCheckpoint::V1(left), super::LoadedCheckpoint::V1(right)) =>
                super::canonical(left).unwrap() == super::canonical(right).unwrap(),
            (super::LoadedCheckpoint::V2(left), super::LoadedCheckpoint::V2(right)) =>
                super::canonical(left).unwrap() == super::canonical(right).unwrap(),
            (super::LoadedCheckpoint::V3(left), super::LoadedCheckpoint::V3(right)) =>
                super::canonical(left).unwrap() == super::canonical(right).unwrap(),
            (super::LoadedCheckpoint::V4(left), super::LoadedCheckpoint::V4(right)) =>
                super::canonical(left).unwrap() == super::canonical(right).unwrap(),
            _ => false,
        };
        assert!(same_payload, "verified final payload differs from strict load");
        copy.revalidate_source().unwrap();
    }).await.expect("retained cost probe exceeded its cooperative 600-second observation bound");
}

struct RetainedCostCopy {
    root: Directory,
    scope: JournalScopeV1,
    initial: super::RevisionRefV1,
    head: super::RevisionRefV1,
    revisions: Vec<Vec<u8>>,
    source_scope: Directory,
    source_job: Directory,
    source_files: Vec<crate::control_fs::PrivateFile>,
    scope_entries: Vec<(String, bool, u64)>,
    job_entries: Vec<(String, bool, u64)>,
}

impl RetainedCostCopy {
    fn revalidate_source(&self) -> JournalResult<()> {
        for file in &self.source_files {
            file.persistent_revalidate()
                .map_err(|_| JournalError::UnsafeFilesystem)?;
        }
        if retained_cost_entries(&self.source_scope, 4)? != self.scope_entries
            || retained_cost_entries(&self.source_job, 37)? != self.job_entries
        {
            return Err(JournalError::Conflict);
        }
        Ok(())
    }
}

fn retained_cost_entries(
    directory: &Directory,
    limit: u32,
) -> JournalResult<Vec<(String, bool, u64)>> {
    let mut entries: Vec<_> = directory
        .persistent_entries(limit)
        .map_err(|_| JournalError::UnsafeFilesystem)?
        .into_iter()
        .map(|entry| (entry.name, entry.directory, entry.bytes))
        .collect();
    entries.sort();
    Ok(entries)
}

fn copy_retained_cost_records(
    source: &Path,
    operation_id: Uuid,
) -> JournalResult<RetainedCostCopy> {
    use crate::control_fs::PrivateFile;
    let source_scope =
        Directory::open_existing(source).map_err(|_| JournalError::UnsafeFilesystem)?;
    let scope_entries = retained_cost_entries(&source_scope, 4)?;
    if scope_entries.len() != 3
        || scope_entries
            .iter()
            .any(|(name, directory, bytes)| match name.as_str() {
                ".writer.lock" => *directory || *bytes != 0,
                "scope.json" => *directory || *bytes > super::SMALL,
                "jobs" => !*directory,
                _ => true,
            })
    {
        return Err(JournalError::InvalidInput);
    }
    let scope_file = source_scope
        .persistent_read("scope.json", super::SMALL, &[])
        .map_err(|_| JournalError::UnsafeFilesystem)?
        .ok_or(JournalError::Interrupted)?;
    let scope: JournalScopeV1 = super::parse(scope_file.bytes())?;
    let scope_bytes = super::canonical(&scope)?;
    let scope_hash = super::digest("ai-guard:broker-journal-scope:v1", &scope_bytes);
    if scope_bytes != scope_file.bytes()
        || source.file_name().and_then(|name| name.to_str()) != Some(scope_hash.as_str())
    {
        return Err(JournalError::Conflict);
    }
    let jobs = source_scope
        .persistent_child("jobs", false)
        .map_err(|_| JournalError::UnsafeFilesystem)?
        .ok_or(JournalError::Interrupted)?;
    let source_job = jobs
        .persistent_child(&operation_id.to_string(), false)
        .map_err(|_| JournalError::UnsafeFilesystem)?
        .ok_or(JournalError::Interrupted)?;
    let job_entries = retained_cost_entries(&source_job, 37)?;
    let fixed = [
        "reservation.json",
        "accepted.json",
        "intent.json",
        "bindings.json",
        "current.json",
    ];
    let mut expected: Vec<_> = fixed.iter().map(|name| (*name).to_owned()).collect();
    expected.extend((0..31).map(super::revision_name));
    expected.sort();
    let actual: Vec<_> = job_entries
        .iter()
        .map(|(name, _, _)| name.clone())
        .collect();
    let total_bytes = job_entries
        .iter()
        .try_fold(scope_file.bytes().len() as u64, |total, (_, _, bytes)| {
            total.checked_add(*bytes).ok_or(JournalError::TooLarge)
        })?;
    let revision_bytes = job_entries
        .iter()
        .filter(|(name, _, _)| name.starts_with("rev-"))
        .try_fold(0u64, |total, (_, _, bytes)| {
            total.checked_add(*bytes).ok_or(JournalError::TooLarge)
        })?;
    if actual != expected
        || job_entries.iter().any(|(_, directory, _)| *directory)
        || total_bytes > 2 * 1024 * 1024
        || revision_bytes != 866_540
    {
        return Err(JournalError::InvalidInput);
    }
    let mut files: Vec<(String, PrivateFile)> = Vec::new();
    for (name, _, bytes) in &job_entries {
        let bound = if name == "intent.json" {
            256 * 1024
        } else if name.starts_with("rev-") {
            super::CHECKPOINT
        } else {
            super::SMALL
        };
        if *bytes > bound {
            return Err(JournalError::TooLarge);
        }
        let file = source_job
            .persistent_read(name, *bytes, &[])
            .map_err(|_| JournalError::UnsafeFilesystem)?
            .ok_or(JournalError::Interrupted)?;
        files.push((name.clone(), file));
    }
    let find = |name: &str| -> JournalResult<&PrivateFile> {
        files
            .iter()
            .find(|(found, _)| found == name)
            .map(|(_, file)| file)
            .ok_or(JournalError::Interrupted)
    };
    let reservation: super::Reservation = super::parse(find("reservation.json")?.bytes())?;
    let accepted: super::Accepted = super::parse(find("accepted.json")?.bytes())?;
    let bindings: super::Bindings = super::parse(find("bindings.json")?.bytes())?;
    let current: super::Current = super::parse(find("current.json")?.bytes())?;
    let intent = ExecutionIntentV1::from_bytes(find("intent.json")?.bytes())
        .map_err(|_| JournalError::Corrupt)?;
    let intent_hash = intent.intent_hash().map_err(|_| JournalError::Corrupt)?;
    if reservation.version != 1
        || reservation.operation_id != operation_id
        || reservation.scope_hash != scope_hash
        || accepted.version != 1
        || accepted.operation_id != operation_id
        || accepted.scope_hash != scope_hash
        || accepted.submit_hash != reservation.submit_hash
        || accepted.intent_hash != intent_hash
        || bindings.version != 1
        || bindings.operation_id != operation_id
        || bindings.intent_hash != intent_hash
        || bindings.binding.kind() != super::BindingKindV1::None
        || intent.operation_id() != operation_id
        || intent.provider_credential().is_some()
        || intent.provider_input().is_some()
        || intent
            .initial_options()
            .run_options()
            .secret_env_file
            .is_some()
        || current.version != 1
        || current.operation_id != operation_id
        || current.intent_hash != intent_hash
        || current.head.revision() != 30
        || accepted.initial_checkpoint.revision() != 0
        || super::canonical(&reservation)? != find("reservation.json")?.bytes()
        || super::canonical(&accepted)? != find("accepted.json")?.bytes()
        || super::canonical(&bindings)? != find("bindings.json")?.bytes()
        || super::canonical(&current)? != find("current.json")?.bytes()
        || intent
            .canonical_bytes()
            .map_err(|_| JournalError::Corrupt)?
            != find("intent.json")?.bytes()
        || accepted.bindings_hash
            != super::digest(
                "ai-guard:broker-bindings:v1",
                find("bindings.json")?.bytes(),
            )
    {
        return Err(JournalError::InvalidInput);
    }
    super::valid_hash(&reservation.submit_hash)?;
    for revision in 0..31 {
        let file = find(&super::revision_name(revision))?;
        let raw: Value = super::parse(file.bytes())?;
        if !matches!(raw.get("version").and_then(Value::as_u64), Some(1..=4))
            || raw.get("operation_id").and_then(Value::as_str)
                != Some(operation_id.to_string().as_str())
            || raw.get("intent_hash").and_then(Value::as_str) != Some(intent_hash.as_str())
            || raw.get("revision").and_then(Value::as_u64) != Some(revision)
            || !raw.get("payload").is_some_and(Value::is_object)
            || super::canonical(&raw)? != file.bytes()
        {
            return Err(JournalError::Corrupt);
        }
    }
    for (_, file) in &files {
        file.persistent_revalidate()
            .map_err(|_| JournalError::UnsafeFilesystem)?;
    }
    scope_file
        .persistent_revalidate()
        .map_err(|_| JournalError::UnsafeFilesystem)?;
    if retained_cost_entries(&source_scope, 4)? != scope_entries
        || retained_cost_entries(&source_job, 37)? != job_entries
    {
        return Err(JournalError::Conflict);
    }

    let parent = std::env::temp_dir()
        .canonicalize()
        .map_err(|_| JournalError::UnsafeFilesystem)?;
    if parent.starts_with(source) {
        return Err(JournalError::UnsafeFilesystem);
    }
    let temporary = tempfile::Builder::new()
        .prefix("ai-guard-retained-cost-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(&parent)
        .map_err(|_| JournalError::UnsafeFilesystem)?;
    let destination = temporary.keep();
    if destination.starts_with(source) || source.starts_with(&destination) {
        return Err(JournalError::UnsafeFilesystem);
    }
    let root =
        Directory::open_existing(&destination).map_err(|_| JournalError::UnsafeFilesystem)?;
    let child = |directory: &Directory, name: &str| -> JournalResult<Directory> {
        directory
            .persistent_child(name, true)
            .map_err(|_| JournalError::UnsafeFilesystem)?
            .ok_or(JournalError::Io)
    };
    let namespace = child(&root, "broker-journal-v1")?;
    let target_scope = child(&namespace, &scope_hash)?;
    let target_jobs = child(&target_scope, "jobs")?;
    let target_job = child(&target_jobs, &operation_id.to_string())?;
    let copy_file =
        |directory: &Directory, name: &str, original: &PrivateFile| -> JournalResult<PrivateFile> {
            original
                .persistent_revalidate()
                .map_err(|_| JournalError::UnsafeFilesystem)?;
            let copied = directory
                .persistent_create(name, original.bytes())
                .map_err(|_| JournalError::Io)?;
            if copied.bytes() != original.bytes() || copied.digest() != original.digest() {
                return Err(JournalError::Corrupt);
            }
            copied.persistent_sync().map_err(|_| JournalError::Io)?;
            Ok(copied)
        };
    copy_file(&target_scope, "scope.json", &scope_file)?;
    let mut revisions = Vec::with_capacity(31);
    for (name, file) in &files {
        let copied = copy_file(&target_job, name, file)?;
        if name.starts_with("rev-") {
            revisions.push(copied.bytes().to_vec());
        }
    }
    for directory in [&target_job, &target_jobs, &target_scope, &namespace, &root] {
        directory.sync().map_err(|_| JournalError::Io)?;
    }
    let mut source_files = vec![scope_file];
    source_files.extend(files.into_iter().map(|(_, file)| file));
    let copy = RetainedCostCopy {
        root,
        scope,
        initial: accepted.initial_checkpoint,
        head: current.head,
        revisions,
        source_scope,
        source_job,
        source_files,
        scope_entries,
        job_entries,
    };
    copy.revalidate_source()?;
    Ok(copy)
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
