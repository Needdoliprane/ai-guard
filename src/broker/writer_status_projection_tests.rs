//! Real journal/writer actor. No engine, provider, fabricated receipt or timing
//! threshold: a retained observation must complete while the actor is paused.
use super::tests::private_journal;
use super::*;
use crate::{
    broker::{intent::ExecutionIntentV1, journal::BindingInputs},
    runner::CheckpointPayloadV1,
};
use futures_util::FutureExt;
use serde_json::{Value, json};

fn accepted(journal: &Journal) -> AcceptedJobV1 {
    accepted_with_profile(journal, "strict")
}
fn accepted_with_profile(journal: &Journal, profile: &str) -> AcceptedJobV1 {
    let operation = Uuid::new_v4();
    let mut plan: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/broker_intent-plan-v3.json"
    )))
    .unwrap();
    plan["version"] = json!(1);
    plan["operation_id"] = json!(operation.to_string());
    plan["options"]["profile"] = json!(profile);
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
    plan["request"]["profile"] = json!(profile);
    plan["request"]["agent_id"] = json!("generic-22222222-2222-4222-8222-222222222222");
    plan["request"]["command"]["argv"] = json!(["/usr/bin/id"]);
    plan["request"]
        .as_object_mut()
        .unwrap()
        .remove("provider_resolution");
    plan["provider_input"] = Value::Null;
    plan["provider_credential"] = Value::Null;
    let intent = ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap();
    let ReserveResult::New(ticket) = journal
        .reserve(operation, br#"{"status_projection":true}"#)
        .unwrap()
    else {
        panic!("fresh status fixture");
    };
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .unwrap()
}

fn lifecycle(journal: &Journal, job: &AcceptedJobV1) -> BrokerCheckpointV3 {
    use crate::broker::checkpoint::{BrokerCheckpointV2, SupervisionCheckpointV1};
    BrokerCheckpointV3::new(
        BrokerCheckpointV2::new(
            job.checkpoint().clone(),
            SupervisionCheckpointV1::new(journal.scope().control_namespace_uuid(), vec![], vec![])
                .unwrap(),
        )
        .unwrap(),
        vec![],
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_confirmed_status_does_not_queue_behind_an_owned_writer_pause() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    let seeded = writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(seeded.entry().head(), Some(job.head()));
    let pause = writer.pause_for_test().await.unwrap();
    assert!(pause.is_paused());
    let observed = writer.last_confirmed_status(operation).now_or_never();
    drop(pause);
    assert!(
        observed.is_some(),
        "last-confirmed status repeated a strict FIFO read instead of using its acknowledged projection"
    );
    assert_eq!(
        observed.unwrap().unwrap().unwrap().entry().head(),
        Some(job.head())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retirement_audit_on_a_v3_job_keeps_its_last_confirmed_status() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let next = lifecycle(&journal, &job);
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    let receipt = writer
        .advance_lifecycle(operation, job.head().clone(), next)
        .await
        .unwrap();
    let before = writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.entry().head(), Some(receipt.head()));

    let pending = writer
        .stops()
        .pending_retirement_events(operation)
        .await
        .unwrap();
    assert!(pending.is_empty());
    let after = writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.entry().head(), Some(receipt.head()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retirement_audit_conflict_for_a_missing_observed_job_closes_the_status_projection() {
    let (root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    let scope = std::fs::read_dir(root.path().join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::rename(
        scope.join("jobs").join(operation.to_string()),
        root.path().join("retained-job-fixture"),
    )
    .unwrap();

    assert!(matches!(
        writer.stops().pending_retirement_events(operation).await,
        Err(JournalError::Conflict)
    ));
    assert!(matches!(
        writer.last_confirmed_status(operation).await,
        Err(JournalError::Corrupt)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn result_read_timeout_keeps_acknowledged_status_readable() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let mut writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    let pause = writer.pause_for_test().await.unwrap();
    assert!(pause.is_paused());
    // The actor is already paused. Give the read a zero reply budget without
    // introducing a disk-speed threshold or altering the production timeout.
    Arc::get_mut(&mut writer.0).unwrap().timeout = Duration::ZERO;
    assert!(matches!(
        writer.result_status(operation).await,
        Err(JournalError::Io)
    ));
    let retained = writer
        .last_confirmed_status(operation)
        .now_or_never()
        .expect("a lost read cannot invalidate the retained head")
        .unwrap()
        .unwrap();
    assert_eq!(retained.entry().head(), Some(job.head()));
    drop(pause);
    Arc::get_mut(&mut writer.0).unwrap().timeout = Duration::from_secs(10);
    let Some(JournalStatusV1::Accepted(read)) = writer.result_status(operation).await.unwrap()
    else {
        panic!("the intact journal must still supply the operation");
    };
    assert_eq!(read.head(), job.head());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn result_read_checks_the_service_fence_after_a_queued_read() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    let fence = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    writer.bind_status_service(&fence, &stop);
    writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    let pause = writer.pause_for_test().await.unwrap();
    let read = writer.result_status(operation);
    tokio::pin!(read);
    assert!(read.as_mut().now_or_never().is_none());
    fence.store(true, Ordering::SeqCst);
    drop(pause);
    assert!(matches!(read.await, Err(JournalError::Io)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_confirmed_status_publishes_after_ack_and_polls_do_no_full_or_sync() {
    use crate::{
        broker::journal::cost_measurement::{Measurement as JournalMeasurement, WriterKind},
        control_fs::cost_probe::Measurement as FsMeasurement,
    };
    let (root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let next = lifecycle(&journal, &job);
    let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(10)).unwrap();
    writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    let mut barrier = writer.advance_reply_barrier_for_test(false).await.unwrap();
    let mutation = writer.advance_lifecycle(operation, job.head().clone(), next);
    tokio::pin!(mutation);
    assert!(
        mutation.as_mut().now_or_never().is_none(),
        "the mutation was actually queued"
    );
    barrier.entered().await;
    let _exclusive = crate::control_fs::cost_probe::EXCLUSIVE.lock().await;
    let filesystem = FsMeasurement::start(&root.path().canonicalize().unwrap()).unwrap();
    let journal_cost = JournalMeasurement::start(
        &journal,
        writer.cost_context_for_test(),
        operation,
        WriterKind::Status,
    )
    .unwrap();
    for _ in 0..100 {
        let projected = writer
            .last_confirmed_status(operation)
            .now_or_never()
            .expect("cached poll must not await the owned actor")
            .unwrap()
            .unwrap();
        assert_eq!(
            projected.entry().head(),
            Some(job.head()),
            "unacknowledged new head escaped its reply barrier"
        );
    }
    let fs = serde_json::to_value(filesystem.finish().unwrap()).unwrap();
    let cost = serde_json::to_value(journal_cost.finish().unwrap()).unwrap();
    assert!(
        fs["samples"]
            .as_array()
            .unwrap()
            .iter()
            .all(|sample| sample["calls"] == 0)
    );
    for counter in [
        "usage_calls",
        "loads_started",
        "accepted_loads_completed",
        "verified_revisions",
    ] {
        assert_eq!(
            cost[counter], 0,
            "cached polls performed strict journal work"
        );
    }
    drop(_exclusive);
    drop(barrier);
    let receipt = mutation.await.unwrap();
    let projected = writer
        .last_confirmed_status(operation)
        .now_or_never()
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(projected.entry().head(), Some(receipt.head()));
    let strict = writer.status(operation).await.unwrap().unwrap();
    let JournalStatusV1::Accepted(strict) = strict else {
        panic!("accepted strict fixture");
    };
    assert_eq!(strict.head(), receipt.head());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_publication_invalidates_projection_without_inventing_a_rollback() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let next = lifecycle(&journal, &job);
    let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(10)).unwrap();
    writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    let mut barrier = writer.advance_reply_barrier_for_test(true).await.unwrap();
    let mutation = writer.advance_lifecycle(operation, job.head().clone(), next);
    tokio::pin!(mutation);
    assert!(mutation.as_mut().now_or_never().is_none());
    barrier.entered().await;
    assert_eq!(
        writer
            .last_confirmed_status(operation)
            .await
            .unwrap()
            .unwrap()
            .entry()
            .head(),
        Some(job.head())
    );
    drop(barrier);
    assert!(matches!(mutation.await, Err(JournalError::Io)));
    assert!(matches!(
        writer.last_confirmed_status(operation).await,
        Err(JournalError::Corrupt)
    ));
    assert_eq!(
        journal.load(operation).unwrap().unwrap().head().revision(),
        1,
        "the actual durable new revision must survive the lost acknowledgement"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandoned_mutation_waiter_invalidates_projection_while_actor_keeps_its_work() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let next = lifecycle(&journal, &job);
    let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(10)).unwrap();
    writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    let mut barrier = writer.advance_reply_barrier_for_test(false).await.unwrap();
    {
        let mutation = writer.advance_lifecycle(operation, job.head().clone(), next);
        tokio::pin!(mutation);
        assert!(mutation.as_mut().now_or_never().is_none());
        barrier.entered().await;
        assert_eq!(
            writer
                .last_confirmed_status(operation)
                .await
                .unwrap()
                .unwrap()
                .entry()
                .head(),
            Some(job.head())
        );
        // Drop the actual queued call, not a fabricated reply or actor task.
    }
    assert!(matches!(
        writer.last_confirmed_status(operation).await,
        Err(JournalError::Corrupt)
    ));
    assert!(matches!(
        writer.result_status(operation).now_or_never(),
        Some(Err(JournalError::Corrupt))
    ));
    drop(barrier);
    let Some(JournalStatusV1::Accepted(strict)) = writer.status(operation).await.unwrap() else {
        panic!("the owned actor must drain its actual mutation");
    };
    assert_eq!(strict.head().revision(), 1);
    assert!(matches!(
        writer.last_confirmed_status(operation).await,
        Err(JournalError::Corrupt)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_scope_refuses_next_real_publication_and_invalidates_projection() {
    use std::io::Write;
    let (root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let next = lifecycle(&journal, &job);
    let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(10)).unwrap();
    writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    let namespace = root.path().join("broker-journal-v1");
    let scope = std::fs::read_dir(namespace)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    // Change only this owned synthetic fixture after the genuine confirmation.
    std::fs::OpenOptions::new()
        .append(true)
        .open(scope.join("scope.json"))
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    assert!(
        writer
            .advance_lifecycle(operation, job.head().clone(), next)
            .await
            .is_err(),
        "cached facts must never authorize a journal mutation"
    );
    assert!(matches!(
        writer.last_confirmed_status(operation).await,
        Err(JournalError::Corrupt)
    ));
    assert!(
        !scope
            .join("jobs")
            .join(operation.to_string())
            .join("rev-00000000000000000001.json")
            .exists(),
        "refused effect published a revision"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn strict_list_conflict_for_a_missing_observed_job_closes_the_status_projection() {
    let (root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    let scope = std::fs::read_dir(root.path().join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::rename(
        scope.join("jobs").join(operation.to_string()),
        root.path().join("retained-job-fixture"),
    )
    .unwrap();
    assert!(
        matches!(writer.list(None, 100).await, Err(JournalError::Conflict)),
        "the real strict inventory must detect its missing floor/job"
    );
    assert!(
        matches!(
            writer.last_confirmed_status(operation).await,
            Err(JournalError::Corrupt)
        ),
        "a known failed inventory left an Accepted cache readable"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn useful_partial_stop_inventory_closes_contradicted_status_without_losing_its_good_entry() {
    let (root, journal) = private_journal();
    let missing = accepted(&journal);
    let retained = accepted(&journal);
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    writer
        .last_confirmed_status(missing.operation_id())
        .await
        .unwrap()
        .unwrap();
    writer
        .last_confirmed_status(retained.operation_id())
        .await
        .unwrap()
        .unwrap();
    let scope = std::fs::read_dir(root.path().join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::rename(
        scope.join("jobs").join(missing.operation_id().to_string()),
        root.path().join("retained-job-fixture"),
    )
    .unwrap();
    let page = writer.stops().recovery_inventory(None, 100).await.unwrap();
    assert!(page.incomplete());
    assert!(
        page.issues()
            .iter()
            .any(|issue| issue.error() == JournalError::Conflict)
    );
    assert_eq!(page.entries().len(), 1);
    assert_eq!(page.entries()[0].operation_id(), retained.operation_id());
    assert!(matches!(
        writer.last_confirmed_status(missing.operation_id()).await,
        Err(JournalError::Corrupt)
    ));
    assert!(matches!(
        writer.last_confirmed_status(retained.operation_id()).await,
        Err(JournalError::Corrupt)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn strict_stop_reader_scope_conflict_closes_the_full_status_projection() {
    use std::io::Write;
    let (root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    let scope = std::fs::read_dir(root.path().join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::OpenOptions::new()
        .append(true)
        .open(scope.join("scope.json"))
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    assert!(
        matches!(
            writer.stops().recovery_status(operation).await,
            Err(JournalError::Conflict)
        ),
        "the actual STOP reader must reject changed scope bytes"
    );
    assert!(
        matches!(
            writer.last_confirmed_status(operation).await,
            Err(JournalError::Corrupt)
        ),
        "a known failed strict STOP read left Accepted cache data readable"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn useful_partial_stop_status_preserves_its_head_and_closes_full_diagnostics() {
    use std::os::unix::fs::OpenOptionsExt;
    let (root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    let scope = std::fs::read_dir(root.path().join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let _extra = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(scope.join("jobs/unrecognized-synthetic-fixture"))
        .unwrap();
    let snapshot = writer
        .stops()
        .recovery_status(operation)
        .await
        .unwrap()
        .unwrap();
    assert!(snapshot.incomplete());
    assert_eq!(snapshot.head(), job.head());
    assert!(matches!(
        writer.last_confirmed_status(operation).await,
        Err(JournalError::Corrupt)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coverage_only_pending_arm_does_not_taint_a_valid_last_confirmed_status() {
    let fixture = crate::broker::journal::retirement_live_fixture::live_fixture_with_pending_arm();
    let operation = fixture.intent.operation_id();
    let writer = WriterClient::start(fixture.journal.clone(), 8, Duration::from_secs(10)).unwrap();
    let before = writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.entry().pending_rpc_count(), Some(1));
    let stop = writer
        .stops()
        .recovery_status(operation)
        .await
        .unwrap()
        .unwrap();
    assert!(stop.incomplete() && stop.pending_arms() == 1);
    let page = writer.stops().recovery_inventory(None, 100).await.unwrap();
    assert!(page.incomplete());
    assert!(
        page.issues()
            .iter()
            .all(|issue| issue.error() == JournalError::Interrupted)
    );
    assert!(
        matches!(
            writer
                .stops()
                .recovery_inventory(Some(operation), 100)
                .await,
            Err(JournalError::Interrupted)
        ),
        "a cursor excluding a coverage-only pending job is not a corruption proof"
    );
    let after = writer
        .last_confirmed_status(operation)
        .now_or_never()
        .expect("a valid retained pending view remains immediate")
        .unwrap()
        .unwrap();
    assert_eq!(after.entry().head(), before.entry().head());
    assert_eq!(after.entry().pending_rpc_count(), Some(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_shared_scope_inventory_interrupted_still_closes_the_cached_view() {
    let (root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    let scope = std::fs::read_dir(root.path().join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::rename(
        scope.join("scope.json"),
        root.path().join("retained-scope-fixture.json"),
    )
    .unwrap();
    assert!(matches!(
        writer.stops().recovery_inventory(None, 100).await,
        Err(JournalError::Interrupted)
    ));
    assert!(
        matches!(
            writer.last_confirmed_status(operation).await,
            Err(JournalError::Corrupt)
        ),
        "a known SharedScope failure is not coverage-only Interrupted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_mutation_preload_conflict_closes_cache_without_misclassifying_cas() {
    use std::io::Write;
    let (root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    let scope = std::fs::read_dir(root.path().join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::OpenOptions::new()
        .append(true)
        .open(scope.join("scope.json"))
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    let failure = writer
        .stops()
        .append_cancel(CancelInputV1::new(operation, Uuid::new_v4()).unwrap())
        .await
        .unwrap_err();
    assert_eq!(
        failure.error(),
        JournalError::Conflict,
        "the real STOP mutation must fail at its shared-core strict preload"
    );
    assert!(matches!(
        writer.last_confirmed_status(operation).await,
        Err(JournalError::Corrupt)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projection_port_closes_after_its_real_actor_owner_exits() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal);
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    writer
        .last_confirmed_status(job.operation_id())
        .await
        .unwrap()
        .unwrap();
    let port = Arc::clone(&writer.0.status_cache);
    let join = writer.0.retirements.take_writer_join_for_test().unwrap();
    drop(writer);
    tokio::task::spawn_blocking(move || join.join().unwrap())
        .await
        .unwrap();
    assert!(
        matches!(port.read(job.operation_id()), Err(JournalError::Io)),
        "a retained reader outlived its actual actor owner"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_status_is_revoked_by_fencing_stopping_or_service_owner_loss() {
    for trigger in 0..3 {
        let (_root, journal) = private_journal();
        let job = accepted(&journal);
        let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
        let fence = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        writer.bind_status_service(&fence, &stop);
        writer
            .last_confirmed_status(job.operation_id())
            .await
            .unwrap()
            .unwrap();
        if trigger == 0 {
            fence.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        if trigger == 1 {
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        if trigger == 2 {
            drop(fence);
        }
        assert!(
            writer
                .last_confirmed_status(job.operation_id())
                .await
                .is_err(),
            "an unavailable service owner exposed retained facts"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acknowledged_local_failure_projection_replay_does_not_increment_again() {
    use crate::broker::lifecycle::{LocalFailureCodeV1, LocalFailurePhaseV1};
    let (_root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let next = lifecycle(&journal, &job);
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    writer
        .advance_lifecycle(operation, job.head().clone(), next)
        .await
        .unwrap();
    let command = Uuid::new_v4();
    let request = LocalCommandRequestV1::FailureObserved {
        phase: LocalFailurePhaseV1::Import,
        code: LocalFailureCodeV1::Unavailable,
    };
    let receipt = writer
        .append_command(operation, command, request.clone())
        .await
        .unwrap();
    let replay = writer
        .append_command(operation, command, request)
        .await
        .unwrap();
    assert_eq!(receipt.head(), replay.head());
    let projected = writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(projected.entry().head(), Some(receipt.head()));
    assert_eq!(projected.entry().failure_count(), 1);
    let Some(JournalStatusV1::Accepted(strict)) = writer.status(operation).await.unwrap() else {
        panic!("strict accepted job");
    };
    let strict = super::super::journal::status_projection::LastConfirmedStatus::from_status(
        operation,
        &JournalStatusV1::Accepted(strict),
    );
    assert_eq!(
        projected.entry().failure_count(),
        strict.entry().failure_count()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_legacy_local_command_requires_a_strict_v2_projection_seed() {
    use crate::broker::lifecycle::{LocalFailureCodeV1, LocalFailurePhaseV1};
    let (_root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    assert_eq!(
        writer
            .last_confirmed_status(operation)
            .await
            .unwrap()
            .unwrap()
            .entry()
            .pending_rpc_count(),
        None
    );
    writer
        .append_command(
            operation,
            Uuid::new_v4(),
            LocalCommandRequestV1::FailureObserved {
                phase: LocalFailurePhaseV1::Decision,
                code: LocalFailureCodeV1::Unavailable,
            },
        )
        .await
        .unwrap();
    assert!(
        writer.0.status_cache.read(operation).unwrap().is_none(),
        "a head-only legacy update invented V2 projection fields"
    );
    let seeded = writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(seeded.entry().pending_rpc_count(), Some(0));
    assert_eq!(seeded.entry().failure_count(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_legacy_outbox_event_requires_a_strict_v2_projection_seed() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    assert_eq!(
        writer
            .last_confirmed_status(operation)
            .await
            .unwrap()
            .unwrap()
            .entry()
            .pending_rpc_count(),
        None
    );
    let mut event = Event::new("broker.status-projection.fixture");
    event.source = "ai-guard-broker".into();
    event.coverage = "native".into();
    let data = job.intent().request_data();
    event.session_id = data.session_id.clone();
    event.project_id = data.project_id.clone();
    event.sandbox_id = data.sandbox_id.clone();
    event.agent_id = data.agent_id.clone();
    event.correlation_id = data.correlation_id.clone();
    event.trace_id = data.trace_id.clone();
    let receipt = writer.enqueue_event(operation, event).await.unwrap();
    assert!(
        writer.0.status_cache.read(operation).unwrap().is_none(),
        "a head-only outbox update invented V2 projection fields"
    );
    let seeded = writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(seeded.entry().head(), Some(receipt.head()));
    assert_eq!(seeded.entry().pending_rpc_count(), Some(0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_real_review_keeps_the_legacy_hash_hidden_in_local_lifecycle_and_seed() {
    use crate::{
        audit::Store,
        broker::{
            checkpoint::{
                BrokerCheckpointV2, DecisionAnswerV1, SupervisionCheckpointV1,
                SupervisionOperationV1,
            },
            lifecycle::LocalCommandRequestV1,
        },
        control::{DecisionPhaseV1, DecisionReceiptV1},
        model::{DecisionRequest, Outcome},
        policy::{self, Policy},
        redaction::Redactor,
    };
    let (root, journal) = private_journal();
    let job = accepted_with_profile(&journal, "review");
    let operation = job.operation_id();
    let request: DecisionRequest = job.intent().request_data().clone();
    let store = Store::create_supervised_for_tenant(
        &root.path().join("review.sqlite"),
        journal.scope().tenant_id(),
        Redactor::new(Vec::new()),
        1000,
        &root.path().join("review-before-supervision.sqlite"),
    )
    .unwrap();
    let policy = Policy {
        allowed_argv: Vec::new(),
        ..Policy::default()
    };
    let stored = store
        .decision(
            &request,
            Some((policy::evaluate(&policy, &request), json!(null))),
        )
        .unwrap()
        .unwrap();
    let decision = stored.event.decision.unwrap();
    assert_eq!(decision.outcome, Outcome::Review);
    let receipt = DecisionReceiptV1 {
        version: 1,
        request_id: request.request_id.clone(),
        request_hash: policy::request_hash(&request),
        phase: DecisionPhaseV1::Initial,
        audit_seq: stored.seq,
        audit_hash: stored.hash,
    };
    let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(10)).unwrap();
    writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    let checkpoint = |response| {
        BrokerCheckpointV3::new(
            BrokerCheckpointV2::new(
                job.checkpoint().clone(),
                SupervisionCheckpointV1::new(
                    journal.scope().control_namespace_uuid(),
                    vec![],
                    vec![SupervisionOperationV1::Decision {
                        request: request.clone(),
                        response,
                    }],
                )
                .unwrap(),
            )
            .unwrap(),
            vec![],
        )
        .unwrap()
    };
    let pending = writer
        .advance_lifecycle(operation, job.head().clone(), checkpoint(None))
        .await
        .unwrap();
    let acknowledged = writer
        .advance_lifecycle(
            operation,
            pending.head().clone(),
            checkpoint(Some(DecisionAnswerV1::new(decision, receipt).unwrap())),
        )
        .await
        .unwrap();
    let before = writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.entry().head(), Some(acknowledged.head()));
    assert_eq!(
        before.review_request_hash(),
        Some(policy::request_hash(&request).as_str())
    );
    let cancel_id = Uuid::new_v4();
    writer
        .append_command(operation, cancel_id, LocalCommandRequestV1::CancelRequested)
        .await
        .unwrap();
    let after_local = writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    assert!(after_local.entry().cancel_requested() && after_local.review_request_hash().is_none());
    let loaded = writer.load(operation).await.unwrap().unwrap();
    writer
        .advance_lifecycle(
            operation,
            loaded.head().clone(),
            loaded.broker_checkpoint_v3().unwrap().clone(),
        )
        .await
        .unwrap();
    let after_lifecycle = writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    assert!(
        after_lifecycle.entry().cancel_requested()
            && after_lifecycle.review_request_hash().is_none()
    );
    // The real STOP replay clears the diagnostic cache; the next port call is
    // seeded from a strict full read of the actual cancelled Review checkpoint.
    writer
        .stops()
        .append_cancel(CancelInputV1::new(operation, cancel_id).unwrap())
        .await
        .unwrap();
    let after_seed = writer
        .last_confirmed_status(operation)
        .await
        .unwrap()
        .unwrap();
    assert!(after_seed.entry().cancel_requested() && after_seed.review_request_hash().is_none());
}

#[tokio::test]
async fn status_projection_capacity_keeps_a_miss_distinct_from_an_absent_job() {
    let (_root, journal) = private_journal();
    let first = accepted(&journal);
    let second = accepted(&journal);
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    let a = writer
        .last_confirmed_status(first.operation_id())
        .await
        .unwrap()
        .unwrap();
    let b = writer
        .last_confirmed_status(second.operation_id())
        .await
        .unwrap()
        .unwrap();
    let cache = status_cache::StatusCache::new(1, 1024 * 1024);
    for value in [a.clone(), b] {
        let (send, receive) = oneshot::channel();
        assert!(
            cache
                .reply(
                    send,
                    Ok(()),
                    status_cache::Update::Replace(value),
                    false,
                    false
                )
                .is_ok()
        );
        receive.await.unwrap().unwrap();
    }
    assert!(cache.read(first.operation_id()).unwrap().is_some());
    assert!(
        cache.read(second.operation_id()).unwrap().is_none(),
        "capacity cannot invent an absent or accepted second job"
    );
    let byte_bounded = status_cache::StatusCache::new(1, 1);
    let (send, receive) = oneshot::channel();
    assert!(
        byte_bounded
            .reply(send, Ok(()), status_cache::Update::Replace(a), false, false)
            .is_ok()
    );
    receive.await.unwrap().unwrap();
    assert!(byte_bounded.read(first.operation_id()).unwrap().is_none());
    assert!(
        writer
            .last_confirmed_status(second.operation_id())
            .await
            .unwrap()
            .is_some(),
        "a cache refusal must not hide the real strict job"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outstanding_counts_each_caller_awaiting_the_actor_until_its_reply_or_cancellation() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal);
    let operation = job.operation_id();
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    assert_eq!(writer.outstanding(), 0);
    let pause = writer.pause_for_test().await.unwrap();
    assert!(pause.is_paused());
    assert_eq!(writer.outstanding(), 0, "the pause itself was acknowledged");
    let waiting = tokio::spawn({
        let writer = writer.clone();
        async move { writer.load(operation).await }
    });
    let cancelled = tokio::spawn({
        let writer = writer.clone();
        async move { writer.load(operation).await }
    });
    // Bounded observation of the two queued callers; no product timing.
    tokio::time::timeout(Duration::from_secs(10), async {
        while writer.outstanding() != 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("both callers awaiting the paused actor are counted");
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    assert_eq!(
        writer.outstanding(),
        1,
        "a cancelled waiter is no longer counted"
    );
    drop(pause);
    let loaded = waiting.await.unwrap().unwrap().unwrap();
    assert_eq!(loaded.head(), job.head());
    assert_eq!(writer.outstanding(), 0, "a replied caller is released");
}
