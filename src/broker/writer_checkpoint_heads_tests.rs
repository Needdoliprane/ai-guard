//! Diagnostic metadata only: real writer seeds and ACK barriers, no inventory
//! fallback, engine, invented accepted job, or wall-clock success threshold.
use super::tests::private_journal;
use super::*;
use crate::{
    broker::{intent::ExecutionIntentV1, journal::BindingInputs},
    runner::CheckpointPayloadV1,
};
use futures_util::FutureExt;
use serde_json::{Value, json};

fn accepted(journal: &Journal, operation: Uuid) -> AcceptedJobV1 {
    let mut plan: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/broker_intent-plan-v3.json"
    )))
    .unwrap();
    plan["version"] = json!(1);
    plan["operation_id"] = json!(operation.to_string());
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
    let intent = ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap();
    let ReserveResult::New(ticket) = journal
        .reserve(operation, br#"{"checkpoint_heads":true}"#)
        .unwrap()
    else {
        panic!("fresh heads fixture");
    };
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_heads_are_unknown_without_reading_existing_jobs_or_the_fifo() {
    use crate::{
        broker::journal::cost_measurement::{Measurement as JournalMeasurement, WriterKind},
        control_fs::cost_probe::Measurement as FsMeasurement,
    };
    let (root, journal) = private_journal();
    let job = accepted(&journal, Uuid::new_v4());
    let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(10)).unwrap();
    let pause = writer.pause_for_test().await.unwrap();
    assert!(pause.is_paused());
    let _exclusive = crate::control_fs::cost_probe::EXCLUSIVE.lock().await;
    let filesystem = FsMeasurement::start(&root.path().canonicalize().unwrap()).unwrap();
    let journal_cost = JournalMeasurement::start(
        &journal,
        writer.cost_context_for_test(),
        job.operation_id(),
        WriterKind::Status,
    )
    .unwrap();
    assert!(matches!(
        writer.last_confirmed_heads(100),
        Err(JournalError::Interrupted)
    ));
    assert_no_reads(filesystem, journal_cost);
    drop(_exclusive);
    drop(pause);
    assert!(writer.status(job.operation_id()).await.unwrap().is_some());
    assert_eq!(writer.last_confirmed_heads(100).unwrap().entries().len(), 1);
}

fn assert_no_reads(
    filesystem: crate::control_fs::cost_probe::Measurement,
    journal_cost: crate::broker::journal::cost_measurement::Measurement,
) {
    let fs = serde_json::to_value(filesystem.finish().unwrap()).unwrap();
    let cost = serde_json::to_value(journal_cost.finish().unwrap()).unwrap();
    assert!(
        fs["samples"]
            .as_array()
            .unwrap()
            .iter()
            .all(|sample| sample["calls"] == 0),
        "a metadata poll performed filesystem work"
    );
    for counter in [
        "usage_calls",
        "loads_started",
        "accepted_loads_completed",
        "verified_revisions",
    ] {
        assert_eq!(cost[counter], 0, "a metadata poll performed Full work");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warm_heads_only_report_the_confirmed_window_while_the_actor_is_paused() {
    use crate::{
        broker::journal::cost_measurement::{Measurement as JournalMeasurement, WriterKind},
        control_fs::cost_probe::Measurement as FsMeasurement,
    };
    let (root, journal) = private_journal();
    let job = accepted(&journal, Uuid::new_v4());
    let _unseeded = accepted(&journal, Uuid::new_v4());
    let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(10)).unwrap();
    writer.status(job.operation_id()).await.unwrap().unwrap();
    let pause = writer.pause_for_test().await.unwrap();
    assert!(pause.is_paused());
    let _exclusive = crate::control_fs::cost_probe::EXCLUSIVE.lock().await;
    let filesystem = FsMeasurement::start(&root.path().canonicalize().unwrap()).unwrap();
    let journal_cost = JournalMeasurement::start(
        &journal,
        writer.cost_context_for_test(),
        job.operation_id(),
        WriterKind::Status,
    )
    .unwrap();
    for _ in 0..100 {
        let page = writer.last_confirmed_heads(100).unwrap();
        assert_eq!(page.entries().len(), 1);
        assert_eq!(page.entries()[0].0, job.operation_id());
        assert_eq!(&page.entries()[0].1, job.head());
        assert!(!page.has_more(), "this describes the cache window only");
    }
    assert_no_reads(filesystem, journal_cost);
    drop(_exclusive);
    drop(pause);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heads_keep_the_old_revision_until_the_real_actor_acknowledges() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal, Uuid::new_v4());
    let next = BrokerCheckpointV3::new(
        BrokerCheckpointV2::new(
            job.checkpoint().clone(),
            SupervisionCheckpointV1::new(journal.scope().control_namespace_uuid(), vec![], vec![])
                .unwrap(),
        )
        .unwrap(),
        vec![],
    )
    .unwrap();
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    writer.status(job.operation_id()).await.unwrap().unwrap();
    let mut barrier = writer.advance_reply_barrier_for_test(false).await.unwrap();
    let mutation = writer.advance_lifecycle(job.operation_id(), job.head().clone(), next);
    tokio::pin!(mutation);
    assert!(mutation.as_mut().now_or_never().is_none());
    barrier.entered().await;
    assert_eq!(
        &writer.last_confirmed_heads(100).unwrap().entries()[0].1,
        job.head(),
        "the port exposed an unacknowledged revision"
    );
    drop(barrier);
    let receipt = mutation.await.unwrap();
    assert_eq!(
        &writer.last_confirmed_heads(100).unwrap().entries()[0].1,
        receipt.head()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heads_close_after_taint_or_loss_of_a_service_owner() {
    for trigger in 0..4 {
        let (_root, journal) = private_journal();
        let job = accepted(&journal, Uuid::new_v4());
        let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
        let fenced = Arc::new(AtomicBool::new(false));
        let stopping = Arc::new(AtomicBool::new(false));
        writer.bind_status_service(&fenced, &stopping);
        writer.status(job.operation_id()).await.unwrap().unwrap();
        assert_eq!(writer.last_confirmed_heads(100).unwrap().entries().len(), 1);
        match trigger {
            0 => writer.invalidate_status_projection(),
            1 => fenced.store(true, Ordering::SeqCst),
            2 => stopping.store(true, Ordering::SeqCst),
            3 => drop(fenced),
            _ => unreachable!(),
        }
        assert!(writer.last_confirmed_heads(100).is_err());
        stopping.store(false, Ordering::SeqCst);
        assert!(writer.last_confirmed_heads(100).is_err());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_heads_port_closes_when_its_actual_writer_actor_exits() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal, Uuid::new_v4());
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    writer.status(job.operation_id()).await.unwrap().unwrap();
    let port = Arc::clone(&writer.0.status_cache);
    assert_eq!(port.read_heads(100).unwrap().entries().len(), 1);
    let join = writer.0.retirements.take_writer_join_for_test().unwrap();
    drop(writer);
    tokio::task::spawn_blocking(move || join.join().unwrap())
        .await
        .unwrap();
    assert!(matches!(port.read_heads(100), Err(JournalError::Io)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heads_are_sorted_and_bounded_to_one_hundred_with_explicit_window_overflow() {
    let (_root, journal) = private_journal();
    let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(10)).unwrap();
    let mut jobs = Vec::new();
    for index in (1..=101).rev() {
        let operation = Uuid::from_u128(0x33333333333343338333000000000000 + index);
        let job = accepted(&journal, operation);
        writer.status(operation).await.unwrap().unwrap();
        jobs.push(job);
    }
    jobs.reverse();
    let page = writer.last_confirmed_heads(100).unwrap();
    assert_eq!(page.entries().len(), 100);
    assert!(page.has_more());
    for ((operation, head), job) in page.entries().iter().zip(jobs.iter().take(100)) {
        assert_eq!(*operation, job.operation_id());
        assert_eq!(head, job.head());
    }
    let first = writer.last_confirmed_heads(1).unwrap();
    assert_eq!(first.entries().len(), 1);
    assert_eq!(first.entries()[0].0, jobs[0].operation_id());
    assert!(first.has_more());
    for invalid in [0, 101, usize::MAX] {
        assert!(matches!(
            writer.last_confirmed_heads(invalid),
            Err(JournalError::InvalidInput)
        ));
    }
}
