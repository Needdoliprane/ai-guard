//! Normal service shutdown drains an already accepted retained-audit owner.
//! The gateway commits real SQLite rows; one response is held after commit so
//! shutdown must retain the same owner until its receipt is durably ACKed.
use crate::{
    audit::{EventQuery, Store},
    broker::journal::{ControlSecrets, FsExclusions, Journal, JournalError, JournalLimitsV1},
    control_fs::Directory,
    redaction::Redactor,
    runner::broker::terminal_pending_seed_for_cold_audit_test,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::task::JoinSet;

#[tokio::test(flavor = "current_thread")]
async fn normal_shutdown_waits_for_a_retained_owner_to_ack_its_committed_receipt() {
    let seed = terminal_pending_seed_for_cold_audit_test().await;
    assert!(seed.is_terminal());
    assert!(seed.result_pending());
    assert!(!seed.result_acknowledged());
    assert!(seed.retained_gateway_is_live());

    let (response_entered, release_response) = seed.pause_next_audit_response_for_test();
    let store = Store::open_for_tenant(
        seed.database(),
        seed.scope().tenant_id(),
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let initial_head = store.head().unwrap();
    let initial_events = store
        .events(&EventQuery {
            limit: Some(1000),
            ..Default::default()
        })
        .unwrap();

    let (service, jobs, enforcer_owner) =
        super::super::cancel_retry_tests::cold_audit_tests::stop_only_service(&seed).await;
    assert!(service.recovery_only());
    assert!(service.require_starts().await.is_err());
    let capabilities = jobs.writer.get().expect("retained Stops capability");
    assert!(capabilities.is_stop_only());
    assert!(capabilities.full().is_none());
    let cold = capabilities
        .cold_audit()
        .expect("the existing Stops actor owns the cold-audit port");
    let prepared = cold
        .prepare(seed.operation_id())
        .await
        .expect("the real pending V4 pair is prepared before HTTP");
    let expected = prepared
        .prepared()
        .pending_events()
        .iter()
        .map(|entry| entry.event().clone())
        .collect::<Vec<_>>();
    assert_eq!(expected.len(), 2);
    assert_eq!(expected[0].kind, "observer.coverage");
    assert_eq!(expected[0].id, seed.coverage_event_id().to_string());
    assert_eq!(expected[1].kind, "broker.batch.result");
    assert_eq!(expected[1].id, seed.event_id().to_string());
    drop(prepared);

    // This is the real producer called by reconcile_loop. It returns only
    // after it has spawned the retained owner; the held POST proves the owner
    // remains live independently of this completed producer handle.
    let producer_service = Arc::clone(&service);
    let producer = tokio::spawn(async move {
        producer_service.reconcile_retained_audit().await;
    });
    tokio::time::timeout(Duration::from_secs(60), response_entered)
        .await
        .expect("the retained owner reaches the real gateway POST")
        .expect("the gateway owns the response-release barrier");
    let producer_deadline = Instant::now() + Duration::from_secs(5);
    while !producer.is_finished() {
        assert!(
            Instant::now() < producer_deadline,
            "reconcile producer must finish after registering its separate owner"
        );
        tokio::task::yield_now().await;
    }

    // The fixture barrier is after next.run: SQLite has committed the first
    // exact Event, while its response (and therefore any client-side receipt)
    // is still withheld.
    let stored_before_release = store
        .events(&EventQuery {
            limit: Some(1000),
            ..Default::default()
        })
        .unwrap();
    let first = stored_before_release
        .iter()
        .filter(|stored| stored.event.id == expected[0].id)
        .collect::<Vec<_>>();
    assert_eq!(first.len(), 1, "the real first POST committed exactly once");
    assert_eq!(first[0].seq, initial_head + 1);
    assert_eq!(
        serde_json::to_value(&first[0].event).unwrap(),
        serde_json::to_value(&expected[0]).unwrap(),
        "the committed row is the exact retained coverage Event"
    );
    assert!(
        stored_before_release
            .iter()
            .all(|stored| stored.event.id != expected[1].id)
    );
    assert_eq!(stored_before_release.len(), initial_events.len() + 1);
    assert_eq!(store.head().unwrap(), initial_head + 1);

    let (presence_shutdown, _) = tokio::sync::watch::channel(false);
    let mut presence_tasks = JoinSet::new();
    let mut requests = JoinSet::new();
    let mut shutdown = Box::pin(service.shutdown(
        producer,
        &presence_shutdown,
        &mut presence_tasks,
        &mut requests,
    ));
    let shutdown_before_release =
        tokio::time::timeout(Duration::from_millis(100), shutdown.as_mut()).await;
    let returned_before_release = shutdown_before_release.is_ok();

    // Always release the actual gateway response before asserting, including
    // on the expected RED, so the private fixture and owner can be reaped.
    release_response
        .send(())
        .expect("the real gateway response is still held for its owner");
    let shutdown_result = match shutdown_before_release {
        Ok(result) => result,
        Err(_) => tokio::time::timeout(Duration::from_secs(60), shutdown.as_mut())
            .await
            .expect("normal shutdown finishes after the retained receipt is released"),
    };

    // On the expected fixed path, inspect the live Stops actor before dropping
    // any service observers. This is a sequenced writer snapshot after shutdown
    // returned, not an atomic observation of the exact return instant.
    if !returned_before_release {
        assert!(shutdown_result.is_ok());
        let live_reservation = cold
            .prepare(seed.operation_id())
            .await
            .expect("the completed owner released its cold-audit reservation");
        let facts = live_reservation
            .prepared()
            .retained_terminal_facts()
            .expect("the live writer still exposes the retained V4 facts");
        let live_events = facts.checkpoint.events();
        let live_coverage = live_events
            .iter()
            .find(|entry| entry.event_id() == seed.coverage_event_id())
            .expect("the exact retained coverage entry remains present");
        let live_result = live_events
            .iter()
            .find(|entry| entry.event_id() == seed.event_id())
            .expect("the exact retained result entry remains present");
        let committed_coverage = store
            .events(&EventQuery {
                limit: Some(1000),
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .find(|stored| stored.event.id == seed.coverage_event_id().to_string())
            .expect("the gateway retains the first committed event");
        assert_eq!(live_coverage.audit_seq(), Some(committed_coverage.seq));
        assert_eq!(live_result.audit_seq(), None);
        drop(live_reservation);
    }

    // Shutdown must not return while an accepted owner still holds the exact
    // completion reservation. On the old implementation this bounded reopen
    // waits for the detached task after releasing the response, for cleanup.
    drop(shutdown);
    drop(presence_tasks);
    drop(requests);
    drop(service);
    drop(jobs);
    drop(cold);
    drop(enforcer_owner);

    let profile = Directory::open_existing(seed.journal_root()).unwrap();
    let reopen_deadline = Instant::now() + Duration::from_secs(60);
    let journal = loop {
        match Journal::open(
            &profile,
            seed.scope().clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new()).unwrap(),
        ) {
            Ok(journal) => break journal,
            Err(JournalError::Busy) => {
                assert!(
                    Instant::now() < reopen_deadline,
                    "the retained owner did not finish and release its writer"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("full Journal reopen failed after drain: {error}"),
        }
    };
    let job = journal
        .load(seed.operation_id())
        .unwrap()
        .expect("cold job remains present after shutdown");
    let outbox = job.broker_checkpoint_v4().unwrap();
    let coverage = outbox
        .events()
        .iter()
        .find(|entry| entry.event_id() == seed.coverage_event_id())
        .expect("the original coverage outbox entry remains present");
    let result = outbox
        .events()
        .iter()
        .find(|entry| entry.event_id() == seed.event_id())
        .expect("the original result outbox entry remains present");
    let stored_after_release = store
        .events(&EventQuery {
            limit: Some(1000),
            ..Default::default()
        })
        .unwrap();
    let stored_coverage = stored_after_release
        .iter()
        .find(|stored| stored.event.id == seed.coverage_event_id().to_string())
        .expect("the exact first event remains in the gateway Store");
    assert_eq!(
        coverage.audit_seq(),
        Some(stored_coverage.seq),
        "shutdown returns only after the first real receipt is durably ACKed"
    );
    assert_eq!(
        result.audit_seq(),
        None,
        "shutdown does not start the second POST"
    );
    assert_eq!(stored_after_release.len(), initial_events.len() + 1);
    assert!(
        stored_after_release
            .iter()
            .all(|stored| stored.event.id != seed.event_id().to_string())
    );
    assert!(shutdown_result.is_ok());
    assert!(
        !returned_before_release,
        "shutdown must wait for the admitted owner, not just its producer"
    );
    assert_eq!(seed.runtime_calls(), seed.runtime_call_snapshot());
    assert_eq!(
        seed.current_decision_request_count(),
        seed.decision_request_count_snapshot()
    );
    let worker = store.worker(seed.worker_id()).unwrap().unwrap();
    assert_eq!(worker.state, crate::control::WorkerStateV1::Terminal);
    assert!(!worker.quota_held);
}
