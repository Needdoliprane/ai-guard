use super::*;
use crate::{
    audit::{EventQuery, Store},
    broker::journal::{ControlSecrets, FsExclusions, Journal, JournalError, JournalLimitsV1},
    control_fs::Directory,
    model::Event,
    redaction::Redactor,
    runner::broker::terminal_pending_seed_for_cold_audit_test,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

async fn owner_for(
    service: &Arc<Service>,
    jobs: &Arc<ProfileJobs>,
    seed: &crate::runner::broker::ColdRecoverySeed,
    credential: String,
    stop_after_first_receipt: bool,
) -> RetainedAuditOwner {
    let profile = service
        .registry
        .profile(seed.scope().profile_id())
        .unwrap()
        .clone();
    let capabilities = jobs.writer.get().expect("retained stop actor");
    let reserved = capabilities
        .cold_audit()
        .expect("recovery retained cold audit capability")
        .prepare(seed.operation_id())
        .await
        .expect("exact pending events are reserved before HTTP");
    let job = jobs
        .job(seed.operation_id())
        .expect("retained operation job");
    let running = job.enter().expect("single retained owner");
    let capacity = Arc::clone(&service.execution)
        .try_acquire_owned()
        .expect("owner holds execution capacity");
    RetainedAuditOwner {
        registry: Arc::clone(&service.registry),
        profile_id: profile.id,
        credential,
        reserved,
        job,
        _running: running,
        _capacity: capacity,
        stopping: Arc::clone(&service.stopping),
        stop_after_first_receipt,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn retained_owner_submits_first_real_receipt_when_stopping_before_second_post() {
    let seed = terminal_pending_seed_for_cold_audit_test().await;
    assert!(seed.is_terminal());
    assert!(seed.result_pending());
    assert!(!seed.result_acknowledged());
    assert!(seed.retained_gateway_is_live());

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
    let profile = service
        .registry
        .profile(seed.scope().profile_id())
        .unwrap()
        .clone();
    let capabilities = jobs.writer.get().expect("retained stop actor");
    assert!(capabilities.is_stop_only());
    assert!(capabilities.full().is_none());

    let invalid_credential = format!("cold-owner-invalid-runner-{}", Uuid::new_v4());
    let invalid_owner = owner_for(&service, &jobs, &seed, invalid_credential, false).await;
    assert!(matches!(
        invalid_owner.publish().await,
        Err(BrokerErrorCodeV1::Unauthorized)
    ));
    assert!(!service.stopping.load(Ordering::SeqCst));
    assert_eq!(store.head().unwrap(), initial_head);
    assert!(
        store
            .events(&EventQuery {
                limit: Some(1000),
                ..Default::default()
            })
            .unwrap()
            .iter()
            .all(|stored| {
                stored.event.id != seed.coverage_event_id().to_string()
                    && stored.event.id != seed.event_id().to_string()
            })
    );

    // A new preparation proves the failed identity owner released the exact
    // completion reservation while leaving both retained events pending.
    let credential = service
        .material
        .reconciler(&profile)
        .expect("retained runner credential");
    let owner = owner_for(&service, &jobs, &seed, credential, true).await;
    assert!(owner.job.autonomous_attempt());
    let reserved = &owner.reserved;
    let pending = reserved.prepared().pending_events();
    assert_eq!(
        pending.len(),
        2,
        "fixture has the retained coverage/result pair"
    );
    let expected_first: Event = pending[0].event().clone();
    let first_id = pending[0].event_id();
    let second_id = pending[1].event_id();
    assert_ne!(first_id, second_id);
    assert!(initial_events.iter().all(|stored| {
        stored.event.id != first_id.to_string() && stored.event.id != second_id.to_string()
    }));

    owner.start();

    let deadline = Instant::now() + Duration::from_secs(60);
    let stored_first = loop {
        let events = store
            .events(&EventQuery {
                limit: Some(1000),
                ..Default::default()
            })
            .unwrap();
        let first = events
            .into_iter()
            .find(|stored| stored.event.id == first_id.to_string());
        if service.stopping.load(Ordering::SeqCst) {
            if let Some(event) = first {
                break event;
            }
        }
        assert!(
            Instant::now() < deadline,
            "first real HTTP receipt was not stored"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(service.stopping.load(Ordering::SeqCst));
    assert_eq!(stored_first.event.id, first_id.to_string());
    assert_eq!(
        serde_json::to_value(&stored_first.event).unwrap(),
        serde_json::to_value(&expected_first).unwrap(),
        "the first persisted Event is the exact retained input"
    );
    assert_eq!(store.head().unwrap(), initial_head + 1);
    let final_events = store
        .events(&EventQuery {
            limit: Some(1000),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(final_events.len(), initial_events.len() + 1);
    assert!(
        final_events
            .iter()
            .all(|stored| stored.event.id != second_id.to_string())
    );
    for before in &initial_events {
        let after = final_events
            .iter()
            .find(|stored| stored.event.id == before.event.id)
            .expect("initial Store event remains present");
        assert_eq!(
            serde_json::to_value(after).unwrap(),
            serde_json::to_value(before).unwrap(),
            "first partial publication preserves existing Store rows"
        );
    }

    // Dropping the observer-side capabilities does not interrupt the detached
    // owner. Its retained actor lease remains until the partial ACK is durable.
    drop(service);
    drop(jobs);
    drop(enforcer_owner);
    let journal_root = Directory::open_existing(seed.journal_root()).unwrap();
    let open_deadline = Instant::now() + Duration::from_secs(60);
    let journal = loop {
        match Journal::open(
            &journal_root,
            seed.scope().clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new()).unwrap(),
        ) {
            Ok(journal) => break journal,
            Err(JournalError::Busy) => {
                assert!(
                    Instant::now() < open_deadline,
                    "partial ACK owner did not finish"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("cold journal reopen failed: {error}"),
        }
    };
    let durable = journal
        .load(seed.operation_id())
        .unwrap()
        .expect("retained operation remains durable");
    let checkpoint = durable
        .broker_checkpoint_v4()
        .expect("retained event outbox remains V4");
    let first = checkpoint
        .events()
        .iter()
        .find(|event| event.event_id() == first_id)
        .expect("first event remains in the outbox");
    let second = checkpoint
        .events()
        .iter()
        .find(|event| event.event_id() == second_id)
        .expect("second event remains in the outbox");
    let first_seq = first.audit_seq().expect("first exact receipt was ACKed");
    assert_eq!(first_seq, stored_first.seq);
    assert_eq!(second.audit_seq(), None, "second event remains pending");
    let managed: serde_json::Value = serde_json::to_value(durable.checkpoint()).unwrap();
    assert_eq!(
        managed["managed"]["output_acknowledged"].as_bool(),
        Some(seed.result_acknowledged()),
        "partial publication does not alter managed output acknowledgement"
    );
    assert_eq!(seed.runtime_calls(), seed.runtime_call_snapshot());
    assert_eq!(
        seed.current_decision_request_count(),
        seed.decision_request_count_snapshot(),
        "the owner uses no runtime or decision service"
    );
}
