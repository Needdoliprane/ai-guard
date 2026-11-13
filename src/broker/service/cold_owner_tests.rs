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

    owner
        .start(service.cold_owners.try_reserve().expect("cold owner slot"))
        .expect("cold owner registered before HTTP");

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

// This contract-first fixture needs the existing cfg(test) BeforeAck selector
// to be visible within crate::broker. It supplies no replacement publisher.
#[tokio::test(flavor = "current_thread")]
async fn retained_reconcile_confirms_quarantine_before_any_fresh_owner_or_post() {
    let seed = terminal_pending_seed_for_cold_audit_test().await;
    // An earlier accepted sibling makes a Busy/mismatched recovery precede
    // the actual target. Stopping discovery at None would strand quarantine.
    let earlier_operation = Uuid::from_u128(1);
    assert!(earlier_operation < seed.operation_id());
    let journal_root = Directory::open_existing(seed.journal_root()).unwrap();
    let journal = Journal::open(
        &journal_root,
        seed.scope().clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap();
    let original = journal.load(seed.operation_id()).unwrap().unwrap();
    let mut earlier_intent = serde_json::to_value(original.intent()).unwrap();
    earlier_intent["operation_id"] = serde_json::json!(earlier_operation);
    let earlier_intent = crate::broker::intent::ExecutionIntentV1::from_bytes(
        &serde_json::to_vec(&earlier_intent).unwrap(),
    )
    .unwrap();
    assert!(earlier_intent.provider_credential().is_none());
    let initial = crate::runner::CheckpointPayloadV1::initial(&earlier_intent).unwrap();
    let crate::broker::journal::ReserveResult::New(ticket) = journal
        .reserve(earlier_operation, br#"{"fixture":"earlier-cold-job"}"#)
        .unwrap()
    else {
        panic!("the earlier fixture operation must be new");
    };
    journal
        .accept(
            ticket,
            &earlier_intent,
            crate::broker::journal::BindingInputs::None,
            &initial,
        )
        .unwrap();
    drop(original);
    drop(journal);
    let store = Store::open_for_tenant(
        seed.database(),
        seed.scope().tenant_id(),
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let initial_head = store.head().unwrap();
    let (service, jobs, enforcer_owner) =
        super::super::cancel_retry_tests::cold_audit_tests::stop_only_service(&seed).await;
    let capabilities = jobs.writer.get().unwrap();
    let cold = capabilities.cold_audit().unwrap();
    let stops = capabilities.stops();
    assert!(capabilities.full().is_none());
    assert!(jobs.known(seed.operation_id()).is_none());
    let profile = service.registry.profile(seed.scope().profile_id()).unwrap();
    let credential = service.material.reconciler(profile).unwrap();
    let verified =
        VerifiedProfile::authenticate(&service.registry, &profile.id, &credential, false)
            .await
            .unwrap();
    let reserved = cold
        .prepare(seed.operation_id())
        .await
        .unwrap()
        .with_test_before_ack_fault();
    let before = reserved.prepared().head().clone();
    let pending = reserved.prepared().pending_events();
    assert_eq!(pending.len(), 2);
    let first_id = pending[0].event_id();
    let second_id = pending[1].event_id();
    let budget = ColdAuditBudget::start();
    let mut receipts = Vec::new();
    for entry in pending {
        receipts.push(
            verified
                .client
                .post_cold_audit_event(entry.event(), &budget)
                .await
                .unwrap(),
        );
    }
    receipts.truncate(1);
    assert!(matches!(
        reserved.submit(receipts).unwrap().confirmed().await,
        Err(crate::broker::writer::CompletionFailure::Unknown {
            error: JournalError::Io
        })
    ));
    assert_eq!(
        cold.prepare(seed.operation_id()).await.err(),
        Some(JournalError::Busy)
    );
    assert_eq!(store.head().unwrap(), initial_head + 2);
    let page = stops.recovery_inventory(None, 100).await.unwrap();
    assert_eq!(page.entries()[0].operation_id(), earlier_operation);
    assert!(page.entries().iter().any(|entry| {
        entry.operation_id() == seed.operation_id()
            && !entry.incomplete()
            && entry.head().revision() == before.revision() + 1
    }));

    service.reconcile_retained_audit().await;

    assert!(
        jobs.known(seed.operation_id()).is_none(),
        "confirmation must not construct a Job or consume its autonomous retry budget"
    );
    let remaining = cold
        .prepare(seed.operation_id())
        .await
        .expect("the real service call confirmed and released its sealed quarantine");
    assert_eq!(
        remaining.prepared().head().revision(),
        before.revision() + 1
    );
    assert_eq!(remaining.prepared().pending_events().len(), 1);
    assert_eq!(
        remaining.prepared().pending_events()[0].event_id(),
        second_id
    );
    assert_ne!(first_id, second_id);
    drop(remaining);
    assert_eq!(store.head().unwrap(), initial_head + 2);
    assert!(capabilities.full().is_none());
    assert_eq!(seed.runtime_calls(), seed.runtime_call_snapshot());
    assert_eq!(
        seed.current_decision_request_count(),
        seed.decision_request_count_snapshot()
    );
    assert!(matches!(
        service.require_starts().await,
        Err(BrokerErrorCodeV1::NeedsReconciliation)
    ));

    drop(verified);
    drop(stops);
    drop(cold);
    drop(service);
    drop(jobs);
    drop(enforcer_owner);
}

fn profile_job_path_for_test(
    profile_root: &std::path::Path,
    operation: Uuid,
) -> std::path::PathBuf {
    let scopes = std::fs::read_dir(profile_root.join("broker-journal-v1"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(scopes.len(), 1);
    scopes[0].join("jobs").join(operation.to_string())
}

async fn reopen_profile_for_test(
    profile_root: &std::path::Path,
    scope: &crate::broker::journal::JournalScopeV1,
) -> Journal {
    let directory = Directory::open_existing(profile_root).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match Journal::open(
            &directory,
            scope.clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new()).unwrap(),
        ) {
            Ok(journal) => return journal,
            Err(JournalError::Busy) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("retained test actor did not release its lease: {error}"),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn unconfirmable_quarantine_in_first_profile_does_not_starve_second_profile_audit() {
    use crate::broker::{
        journal::{BindingInputs, ReserveResult, recovery::StopRecoveryReaderV1},
        writer::ProfileWriterCapabilities,
    };
    use crate::runner::broker::ColdRecoverySeed;
    use std::{
        collections::BTreeMap,
        fs,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };

    let installation = Uuid::new_v4();
    let seed_a = ColdRecoverySeed::with_scope_for_cold_audit_test(installation, "a-blocked").await;
    let seed_b = ColdRecoverySeed::with_scope_for_cold_audit_test(installation, "b-pending").await;
    assert_eq!(seed_a.scope().installation_id(), installation);
    assert_eq!(seed_b.scope().installation_id(), installation);
    assert_ne!(
        seed_a.scope().gateway_origin(),
        seed_b.scope().gateway_origin()
    );

    // Retain an accepted sibling before preparing A. Its later disappearance
    // is a per-job issue, so A remains inventoried while exact scope checks
    // refuse confirmation. No selected revision or floor is rolled back.
    let sibling = Uuid::new_v4();
    let journal_a = reopen_profile_for_test(seed_a.journal_root(), seed_a.scope()).await;
    let original = journal_a.load(seed_a.operation_id()).unwrap().unwrap();
    let mut intent = serde_json::to_value(original.intent()).unwrap();
    intent["operation_id"] = serde_json::json!(sibling);
    let intent =
        crate::broker::intent::ExecutionIntentV1::from_bytes(&serde_json::to_vec(&intent).unwrap())
            .unwrap();
    assert!(intent.provider_credential().is_none());
    let ReserveResult::New(ticket) = journal_a
        .reserve(sibling, br#"{"fixture":"preserved-quarantine-sibling"}"#)
        .unwrap()
    else {
        panic!("the sibling operation is new");
    };
    journal_a
        .accept(
            ticket,
            &intent,
            BindingInputs::None,
            &crate::runner::CheckpointPayloadV1::initial(&intent).unwrap(),
        )
        .unwrap();
    drop(original);
    drop(journal_a);

    let (single_service, jobs_a, enforcer_owner) =
        super::super::cancel_retry_tests::cold_audit_tests::stop_only_service(&seed_a).await;
    let mut registry: serde_json::Value = serde_json::from_slice(
        &fs::read(seed_a.config().join("cold-recovery-registry.json")).unwrap(),
    )
    .unwrap();
    let mut profile_b = registry["profiles"][0].clone();
    profile_b["id"] = serde_json::json!(seed_b.scope().profile_id());
    profile_b["gateway"] = serde_json::json!(seed_b.scope().gateway_origin());
    profile_b["tenant_id"] = serde_json::json!(seed_b.scope().tenant_id());
    profile_b["control_namespace_uuid"] =
        serde_json::json!(seed_b.scope().control_namespace_uuid());
    profile_b["config_dir"] = serde_json::json!(seed_b.config());
    profile_b["runner_token_file"] = serde_json::json!(seed_b.config().join("reconciler.token"));
    profile_b["runtime_program"] = serde_json::json!(seed_b.root().join("control/engine"));
    for (name, field) in [("sources", "source_roots"), ("exports", "export_roots")] {
        let path = seed_b.root().join(name);
        fs::create_dir_all(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        profile_b[field] = serde_json::json!([path]);
    }
    registry["profiles"].as_array_mut().unwrap().push(profile_b);
    registry["max_enforcer_slots"] = serde_json::json!(2);
    let registry_path = seed_a.config().join("two-profile-recovery-registry.json");
    let mut registry_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&registry_path)
        .unwrap();
    std::io::Write::write_all(&mut registry_file, &serde_json::to_vec(&registry).unwrap()).unwrap();
    registry_file.sync_all().unwrap();
    drop(registry_file);
    let registry = Arc::new(Registry::load(&registry_path).unwrap());
    let material = Arc::new(Material::load(&registry).unwrap());
    assert_eq!(registry.data.profiles[0].id, "a-blocked");
    assert_eq!(registry.data.profiles[1].id, "b-pending");

    // Move the closed, intact B profile into this service's real journal
    // layout before opening its actor. Its scope and bytes are unchanged.
    let profile_b_root = seed_a
        .root()
        .join("journals")
        .join(seed_b.scope().profile_id());
    fs::rename(seed_b.journal_root(), &profile_b_root).unwrap();
    let directory_b = Directory::open_existing(&profile_b_root).unwrap();
    let reader_b = StopRecoveryReaderV1::open_existing(
        &directory_b,
        seed_b.scope().clone(),
        &material.exclusions,
        JournalLimitsV1::default(),
        ControlSecrets::new(material.literals.clone()).unwrap(),
    )
    .unwrap()
    .unwrap();
    let report_b = reader_b.scan();
    let jobs_b = Arc::new(ProfileJobs::new());
    jobs_b
        .writer
        .set(
            ProfileWriterCapabilities::from_recovery_report(
                &report_b,
                true,
                64,
                Duration::from_secs(60),
            )
            .unwrap(),
        )
        .unwrap();
    drop(report_b);
    drop(single_service);
    drop(enforcer_owner);
    let service = Arc::new(Service {
        full_owners: FullOwners::new(2).unwrap(),
        cold_owners: ColdOwners::new(2).unwrap(),
        registry,
        root: Directory::open_existing(seed_a.root()).unwrap(),
        material,
        profiles: BTreeMap::from([
            ("a-blocked".to_owned(), Arc::clone(&jobs_a)),
            ("b-pending".to_owned(), Arc::clone(&jobs_b)),
        ]),
        enforcer: None,
        stop_enforcer: None,
        blocking: Arc::new(tokio::sync::Semaphore::new(1)),
        preparation: Arc::new(tokio::sync::Semaphore::new(1)),
        execution: Arc::new(tokio::sync::Semaphore::new(2)),
        stopping: Arc::new(AtomicBool::new(false)),
        fenced: Arc::new(AtomicBool::new(true)),
    });
    let cold_a = jobs_a.writer.get().unwrap().cold_audit().unwrap();
    let cold_b = jobs_b.writer.get().unwrap().cold_audit().unwrap();
    let stops_a = jobs_a.writer.get().unwrap().stops();
    let store_a = Store::open_for_tenant(
        seed_a.database(),
        seed_a.scope().tenant_id(),
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let store_b = Store::open_for_tenant(
        seed_b.database(),
        seed_b.scope().tenant_id(),
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let b_initial_head = store_b.head().unwrap();
    let admissible_b = cold_b.prepare(seed_b.operation_id()).await.unwrap();
    let b_before = admissible_b.prepared().head().clone();
    let b_expected = admissible_b
        .prepared()
        .pending_events()
        .iter()
        .map(|entry| entry.event().clone())
        .collect::<Vec<_>>();
    assert_eq!(b_expected.len(), 2);
    drop(admissible_b);
    let profile_a = service.registry.profile("a-blocked").unwrap();
    let verified_a = VerifiedProfile::authenticate(
        &service.registry,
        "a-blocked",
        &service.material.reconciler(profile_a).unwrap(),
        false,
    )
    .await
    .unwrap();
    let reserved_a = cold_a
        .prepare(seed_a.operation_id())
        .await
        .unwrap()
        .with_test_before_ack_fault();
    let a_before = reserved_a.prepared().head().clone();
    let budget = ColdAuditBudget::start();
    let mut receipts = Vec::new();
    for entry in reserved_a.prepared().pending_events() {
        receipts.push(
            verified_a
                .client
                .post_cold_audit_event(entry.event(), &budget)
                .await
                .unwrap(),
        );
    }
    assert!(matches!(
        reserved_a.submit(receipts).unwrap().confirmed().await,
        Err(crate::broker::writer::CompletionFailure::Unknown {
            error: JournalError::Io
        })
    ));
    let a_store_before = store_a
        .events(&EventQuery {
            limit: Some(1000),
            ..Default::default()
        })
        .unwrap();
    let a_head_before = store_a.head().unwrap();
    let a_pointer = profile_job_path_for_test(seed_a.journal_root(), seed_a.operation_id())
        .join("current.json");
    let a_pointer_before = fs::read(&a_pointer).unwrap();
    let sibling_path = profile_job_path_for_test(seed_a.journal_root(), sibling);
    let sibling_backup = seed_a.root().join(format!("preserved-sibling-{sibling}"));
    fs::rename(&sibling_path, &sibling_backup).unwrap();
    let inventory_a = stops_a.recovery_inventory(None, 100).await.unwrap();
    let a_is_reachable = inventory_a
        .entries()
        .iter()
        .any(|entry| entry.operation_id() == seed_a.operation_id() && !entry.incomplete());
    let (mut a_post_seen, release_a_post) = seed_a.pause_next_audit_response_for_test();
    // Pre-release the real HTTP barrier: it observes an unexpected successful
    // A POST without blocking cleanup or replacing its gateway response.
    release_a_post.send(()).unwrap();

    // Record this precondition before the actual call, but assert it after
    // cleanup: a global control refusal must not mimic profile starvation.
    let control_ready = service.validate_control().await.is_ok();
    service.reconcile_retained_audit().await;
    service.cold_owners.close().unwrap();
    let drain = tokio::time::timeout(Duration::from_secs(60), service.cold_owners.drain())
        .await
        .unwrap();
    let a_confirmation_error = cold_a
        .recover_quarantined(seed_a.operation_id())
        .await
        .err();
    let a_reservation_error = cold_a.prepare(seed_a.operation_id()).await.err();
    let no_a_post = matches!(
        a_post_seen.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    );
    let a_store_after = store_a
        .events(&EventQuery {
            limit: Some(1000),
            ..Default::default()
        })
        .unwrap();
    let b_store_after = store_b
        .events(&EventQuery {
            limit: Some(1000),
            ..Default::default()
        })
        .unwrap();
    let b_head_after = store_b.head().unwrap();
    let a_pointer_after = fs::read(&a_pointer).unwrap();
    let a_job_unused = jobs_a.known(seed_a.operation_id()).is_none();
    let both_stops = jobs_a.writer.get().unwrap().full().is_none()
        && jobs_b.writer.get().unwrap().full().is_none();
    fs::rename(&sibling_backup, &sibling_path).unwrap();
    drop(inventory_a);
    drop(verified_a);
    drop(stops_a);
    drop(cold_a);
    drop(cold_b);
    drop(service);
    drop(jobs_a);
    drop(jobs_b);

    // Reopen both real journals and restore the relocated B profile before
    // the final RED assertion, so the expected failure leaves no live owner
    // or displaced fixture directory behind.
    let journal_a = reopen_profile_for_test(seed_a.journal_root(), seed_a.scope()).await;
    let a_durable = journal_a.load(seed_a.operation_id()).unwrap().unwrap();
    let a_durable_revision = a_durable.head().revision();
    drop(a_durable);
    drop(journal_a);
    let journal_b = reopen_profile_for_test(&profile_b_root, seed_b.scope()).await;
    let b_durable = journal_b.load(seed_b.operation_id()).unwrap().unwrap();
    let b_durable_revision = b_durable.head().revision();
    let b_acknowledged = b_expected.iter().all(|expected| {
        let central = b_store_after
            .iter()
            .find(|stored| stored.event.id == expected.id);
        let local = b_durable
            .broker_checkpoint_v4()
            .unwrap()
            .events()
            .iter()
            .find(|event| event.event().id == expected.id);
        central.zip(local).is_some_and(|(stored, local)| {
            serde_json::to_value(&stored.event).unwrap() == serde_json::to_value(expected).unwrap()
                && local.audit_seq() == Some(stored.seq)
        })
    });
    drop(b_durable);
    drop(journal_b);
    drop(directory_b);
    fs::rename(&profile_b_root, seed_b.journal_root()).unwrap();

    eprintln!(
        "quarantine_two_profiles control_ready={control_ready} a_reachable={a_is_reachable} a_confirmation={a_confirmation_error:?} a_reservation={a_reservation_error:?} no_a_post={no_a_post} b_head={b_head_after} b_expected_head={} b_acknowledged={b_acknowledged}",
        b_initial_head + 2
    );
    assert!(drain.is_ok());
    assert!(
        control_ready,
        "both profile controls must be valid before reconciliation"
    );
    assert!(
        a_is_reachable,
        "the failing A quarantine must actually reach recovery"
    );
    assert_eq!(a_confirmation_error, Some(JournalError::Conflict));
    assert_eq!(a_reservation_error, Some(JournalError::Busy));
    assert!(a_job_unused);
    assert!(both_stops);
    assert!(no_a_post, "confirmation cannot resend A events");
    assert_eq!(
        serde_json::to_value(&a_store_after).unwrap(),
        serde_json::to_value(&a_store_before).unwrap()
    );
    assert_eq!(store_a.head().unwrap(), a_head_before);
    assert_eq!(a_pointer_after, a_pointer_before);
    assert_eq!(a_durable_revision, a_before.revision() + 1);
    for seed in [&seed_a, &seed_b] {
        assert_eq!(seed.runtime_calls(), seed.runtime_call_snapshot());
        assert_eq!(
            seed.current_decision_request_count(),
            seed.decision_request_count_snapshot()
        );
    }
    assert_eq!(
        b_head_after,
        b_initial_head + 2,
        "B must POST its two real retained events despite A's unconfirmable quarantine"
    );
    assert!(
        b_acknowledged,
        "B must durably ACK its exact real HTTP receipts"
    );
    assert_eq!(b_durable_revision, b_before.revision() + 1);
}
