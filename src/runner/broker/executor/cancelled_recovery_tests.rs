use crate::control::WorkerStateV1;
use fixture::{Case, RecoveryFixture};

#[path = "cancelled_recovery_fixture.rs"]
mod fixture;

pub(crate) use fixture::{
    ColdRecoverySeed, terminal_cleanup_jobs_for_test, terminal_pending_seed_for_cold_audit_test,
};

#[test]
fn resumed_terminal_worker_reconciles_cleanup_only_for_its_observed_container() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should initialize");
    let fixture = runtime.block_on(RecoveryFixture::new(Case::TerminalCleanupRecovery));
    assert_eq!(fixture.worker().cleanup_removed, None);
    runtime.block_on(fixture.record_fixture_cleanup(false));
    let before = fixture.worker();
    assert_eq!(before.state, WorkerStateV1::Terminal);
    assert!(!before.quota_held);
    assert!(!fixture.cancel_present());
    assert_eq!(before.cleanup_removed, Some(false));
    assert!(fixture.completed_cleanup_receipts().is_empty());
    let usage = fixture.usage();
    assert_eq!(usage.len(), 1);
    let arm_claim = fixture.arm_claim_snapshot();
    let publication = fixture.result_publication_snapshot();
    assert!(publication.output_acknowledged);
    assert_eq!(publication.stored_result_events.len(), 1);
    let cid = before.attempt.container_id.as_deref().unwrap();
    let inspect = format!("inspect {cid}");
    let cleanup = format!("rm {cid}");

    fixture.set_case(Case::UnknownContainer);
    let unavailable = fixture.run_sync(&runtime);
    assert_eq!(fixture.worker().revision, before.revision);
    assert!(fixture.completed_cleanup_receipts().is_empty());
    let unavailable_calls = fixture.runtime_calls();
    assert!(!unavailable_calls.contains(&cleanup));

    fixture.set_case(Case::TerminalCleanupRecovery);
    fixture
        .run_sync(&runtime)
        .expect("the same observed terminal container can be removed");
    let after = fixture.worker();
    assert_eq!(after.cleanup_removed, Some(true));
    assert_eq!(after.revision, before.revision + 1);
    assert!(unavailable.is_err(), "absence does not prove cleanup");
    assert_eq!(after.worker_id, before.worker_id);
    assert_eq!(after.attempt.attempt_id, before.attempt.attempt_id);
    assert_eq!(after.attempt.container_id, before.attempt.container_id);
    assert_eq!(after.state, WorkerStateV1::Terminal);
    assert!(!after.quota_held);
    let receipts = fixture.completed_cleanup_receipts();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].revision, Some(after.revision));
    assert!(receipts[0].audit_seq > 0);
    let calls = fixture.runtime_calls();
    assert_eq!(calls.iter().filter(|call| *call == &cleanup).count(), 1);
    assert!(calls.iter().all(|call| {
        call == "info" || call == "image inspect" || call == &inspect || call == &cleanup
    }));
    let cleanup_at = calls.iter().position(|call| call == &cleanup).unwrap();
    assert!(calls[unavailable_calls.len()..cleanup_at].contains(&inspect));

    fixture
        .run_sync(&runtime)
        .expect("the exact cleanup acknowledgement survives replay");
    assert_eq!(fixture.worker().revision, after.revision);
    assert_eq!(fixture.completed_cleanup_receipts(), receipts);
    assert_eq!(fixture.usage(), usage);
    assert_eq!(fixture.arm_claim_snapshot(), arm_claim);
    assert_eq!(fixture.result_publication_snapshot(), publication);
    assert!(
        fixture.runtime_calls()[calls.len()..]
            .iter()
            .all(|call| call == "info" || call == "image inspect")
    );
}

#[test]
fn resumed_terminal_worker_keeps_container_when_cancel_wins_after_inspection() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should initialize");
    let fixture = runtime.block_on(RecoveryFixture::new(Case::TerminalCleanupRecovery));
    let before = fixture.worker();
    assert_eq!(before.state, WorkerStateV1::Terminal);
    assert_eq!(before.cleanup_removed, None);
    let cleanup = format!("rm {}", before.attempt.container_id.as_deref().unwrap());

    let result = runtime.block_on(fixture.run_invalidated_during_cleanup_authority());
    assert!(
        !fixture.runtime_calls().contains(&cleanup),
        "rm spawned after the execution generation was invalidated"
    );
    assert!(result.is_err());
    assert_eq!(fixture.worker().revision, before.revision);
    assert_eq!(fixture.worker().cleanup_removed, None);
    assert!(fixture.completed_cleanup_receipts().is_empty());
}

#[test]
fn resumed_terminal_worker_keeps_container_when_final_scan_fails() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should initialize");
    let fixture = runtime.block_on(RecoveryFixture::new(Case::TerminalCleanupWithoutScan));
    let before = fixture.worker();
    assert_eq!(before.state, WorkerStateV1::Terminal);
    assert_eq!(before.cleanup_removed, None);
    let cleanup = format!("rm {}", before.attempt.container_id.as_deref().unwrap());

    // The synthetic engine rejects the final scan's helper inventory.
    assert!(fixture.run_sync(&runtime).is_err());
    let calls = fixture.runtime_calls();
    assert!(calls.iter().any(|call| call.starts_with("ps ")));
    assert!(!calls.contains(&cleanup), "rm must wait for the scan ACK");
    assert_eq!(fixture.worker().revision, before.revision);
    assert_eq!(fixture.worker().cleanup_removed, None);
    assert!(fixture.completed_cleanup_receipts().is_empty());
}

#[test]
fn cancelled_running_worker_reconciles_an_exact_terminal_without_a_new_start() {
    assert_exact_terminal_reconciliation(Case::TerminalAfterCancel);
}

#[test]
fn cancelled_running_worker_reconciles_a_created_owned_snapshot() {
    assert_exact_terminal_reconciliation(Case::TerminalCreatedOwned);
}

fn assert_exact_terminal_reconciliation(case: Case) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should initialize");
    eprintln!("cancelled recovery phase=setup_start");
    let fixture = runtime.block_on(RecoveryFixture::new(case));
    eprintln!("cancelled recovery phase=setup_complete");
    let before = fixture.worker();
    assert_eq!(before.state, WorkerStateV1::Running);
    assert!(before.quota_held);
    eprintln!("cancelled recovery phase=before_worker_verified");
    assert!(fixture.usage().is_empty());
    eprintln!("cancelled recovery phase=before_allocation_verified");
    assert!(fixture.cancel_present());
    let durable_cancel = fixture.cancel_id();
    eprintln!("cancelled recovery phase=before_cancel_verified");
    let worker_id = before.worker_id.clone();
    let attempt_id = before.attempt.attempt_id.clone();
    let cid = before
        .attempt
        .container_id
        .clone()
        .expect("running worker has its durable CID");
    let started_at = before
        .observed_started_at
        .clone()
        .expect("running worker has a durable StartedAt");
    let arm_claim_before = fixture.arm_claim_snapshot();
    eprintln!("cancelled recovery phase=before_arm_claim_snapshot_ready");
    let runtime_calls_before = fixture.runtime_calls();
    assert!(runtime_calls_before.is_empty());

    eprintln!("cancelled recovery phase=run_start");
    let run = fixture.run_sync(&runtime);
    if run.is_err() {
        eprintln!("cancelled recovery phase=run_returned_err");
        fixture.report_post_run_diagnostic();
        eprintln!("cancelled recovery phase=post_run_diagnostic_complete");
    }
    run.expect("exact terminal observation should reconcile the cancelled worker");
    eprintln!("cancelled recovery phase=run_complete");

    let after = fixture.worker();
    assert_eq!(after.state, WorkerStateV1::Terminal);
    assert!(!after.quota_held);
    assert_eq!(after.worker_id, worker_id);
    assert_eq!(after.attempt.attempt_id, attempt_id);
    assert_eq!(after.attempt.container_id.as_deref(), Some(cid.as_str()));
    assert!(fixture.cancel_present());
    assert_eq!(fixture.cancel_id(), durable_cancel);
    assert_eq!(fixture.arm_claim_snapshot(), arm_claim_before);
    let runtime_calls = fixture.runtime_calls();
    assert!(
        runtime_calls
            .iter()
            .all(|line| matches!(line.split_whitespace().next(), Some("info" | "inspect"))),
        "cancelled recovery must use read-only runtime calls only"
    );
    assert!(
        runtime_calls
            .iter()
            .any(|line| line == &format!("inspect {cid}")),
        "recovery did not inspect the exact retained CID"
    );

    let usage = fixture.usage();
    assert_eq!(usage.len(), 1);
    let allocation = &usage[0];
    let duration_ns = allocation.duration_ns.parse::<u128>().unwrap();
    assert_eq!(duration_ns, 2_000_000_000);
    let started = chrono::DateTime::parse_from_rfc3339(&allocation.started_at).unwrap();
    let finished = chrono::DateTime::parse_from_rfc3339(&allocation.finished_at).unwrap();
    assert_eq!(
        u128::try_from(
            finished
                .signed_duration_since(started)
                .num_nanoseconds()
                .unwrap()
        )
        .unwrap(),
        duration_ns
    );
    assert_eq!(allocation.worker_id, worker_id);
    assert_eq!(allocation.attempt_id, attempt_id);
    assert_eq!(allocation.source.container_id, cid);
    assert_eq!(allocation.started_at, "2026-10-30T12:00:00.000000000Z");
    assert_eq!(allocation.started_at, started_at);
    assert_eq!(allocation.finished_at, "2026-10-30T12:00:02.000000000Z");
    assert_eq!(
        allocation.cpu_millicore_nanoseconds,
        (duration_ns * u128::from(after.launch.cpu_millis)).to_string()
    );
    assert_eq!(
        allocation.memory_byte_nanoseconds,
        (duration_ns * u128::from(after.launch.memory_bytes)).to_string()
    );
    assert!(!allocation.billable);

    let terminal_revision = after.revision;
    let committed_allocation = usage;
    fixture
        .run_sync(&runtime)
        .expect("replaying a terminal cancelled worker should remain successful");
    let replayed = fixture.worker();
    assert_eq!(replayed.state, WorkerStateV1::Terminal);
    assert!(!replayed.quota_held);
    assert_eq!(replayed.revision, terminal_revision);
    assert_eq!(fixture.usage(), committed_allocation);
    assert_eq!(fixture.cancel_id(), durable_cancel);
    assert!(fixture.cancel_present());
    assert_eq!(fixture.arm_claim_snapshot(), arm_claim_before);
    assert!(
        fixture
            .runtime_calls()
            .iter()
            .all(|line| { line == "info" || line == &format!("inspect {cid}") })
    );
}

#[test]
fn cancelled_terminal_ack_loss_replays_result_publication_exactly_once() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should initialize");
    let fixture = runtime.block_on(RecoveryFixture::new(Case::TerminalAfterCancel));
    let before = fixture.worker();
    assert_eq!(before.state, WorkerStateV1::Running);
    assert!(before.quota_held);
    assert!(fixture.usage().is_empty());
    let cancel_id = fixture.cancel_id();
    let worker_id = before.worker_id.clone();
    let attempt_id = before.attempt.attempt_id.clone();
    let cid = before
        .attempt
        .container_id
        .clone()
        .expect("running worker has its durable CID");
    let arm_claim_before = fixture.arm_claim_snapshot();
    let publication_before = fixture.result_publication_snapshot();
    assert!(!publication_before.output_acknowledged);
    assert!(publication_before.result_outbox.is_empty());
    assert!(publication_before.stored_result_events.is_empty());

    fixture.arm_transition_response_cut();
    assert!(fixture.transition_response_cut_pending());
    assert!(
        fixture.run_sync(&runtime).is_err(),
        "the committed transition response should be lost to the caller"
    );
    assert!(!fixture.transition_response_cut_pending());

    let committed = fixture.worker();
    assert_eq!(committed.state, WorkerStateV1::Terminal);
    assert!(!committed.quota_held);
    assert_eq!(committed.worker_id, worker_id);
    assert_eq!(committed.attempt.attempt_id, attempt_id);
    assert_eq!(
        committed.attempt.container_id.as_deref(),
        Some(cid.as_str())
    );
    let committed_usage = fixture.usage();
    assert_eq!(committed_usage.len(), 1);
    assert!(!committed_usage[0].billable);
    assert!(fixture.cancel_present());
    assert_eq!(fixture.cancel_id(), cancel_id);
    assert_eq!(fixture.arm_claim_snapshot(), arm_claim_before);
    let pending_publication = fixture.result_publication_snapshot();
    assert!(!pending_publication.output_acknowledged);
    assert_eq!(pending_publication.result_outbox.len(), 1);
    let result_event_id = pending_publication.result_outbox[0].0;
    assert_eq!(pending_publication.result_outbox[0].1, None);
    assert!(pending_publication.stored_result_events.is_empty());
    let runtime_calls_after_first_run = fixture.runtime_calls();

    fixture
        .run_sync(&runtime)
        .expect("retry should finish pending acknowledgement and result publication");
    let after_retry = fixture.worker();
    assert_eq!(after_retry.state, WorkerStateV1::Terminal);
    assert!(!after_retry.quota_held);
    assert_eq!(fixture.usage(), committed_usage);
    assert!(fixture.cancel_present());
    assert_eq!(fixture.cancel_id(), cancel_id);
    assert_eq!(fixture.arm_claim_snapshot(), arm_claim_before);
    let published = fixture.result_publication_snapshot();
    assert!(published.output_acknowledged);
    assert_eq!(published.result_outbox.len(), 1);
    assert_eq!(published.result_outbox[0].0, result_event_id);
    let audit_seq = published.result_outbox[0]
        .1
        .expect("result event must have a durable audit acknowledgement");
    assert_eq!(published.stored_result_events.len(), 1);
    assert_eq!(
        published.stored_result_events[0].0,
        result_event_id.to_string()
    );
    assert_eq!(published.stored_result_events[0].1, audit_seq);
    assert!(
        fixture
            .runtime_calls()
            .iter()
            .all(|line| { line == "info" || line == &format!("inspect {cid}") })
    );
    assert_eq!(fixture.runtime_calls(), runtime_calls_after_first_run);

    let publication_after_success = fixture.result_publication_snapshot();
    let terminal_revision = after_retry.revision;
    fixture
        .run_sync(&runtime)
        .expect("replaying an acknowledged terminal result should remain successful");
    let after_terminal_replay = fixture.worker();
    assert_eq!(after_terminal_replay.state, WorkerStateV1::Terminal);
    assert!(!after_terminal_replay.quota_held);
    assert_eq!(after_terminal_replay.revision, terminal_revision);
    assert_eq!(fixture.usage(), committed_usage);
    assert!(fixture.cancel_present());
    assert_eq!(fixture.cancel_id(), cancel_id);
    assert_eq!(fixture.arm_claim_snapshot(), arm_claim_before);
    assert_eq!(
        fixture.result_publication_snapshot(),
        publication_after_success
    );
    assert_eq!(fixture.runtime_calls(), runtime_calls_after_first_run);
}

#[test]
fn cancelled_running_worker_rejects_divergent_runtime_observations_without_mutation() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should initialize");
    let fixture = runtime.block_on(RecoveryFixture::new(Case::TerminalAfterCancel));
    let before = fixture.worker();
    assert_eq!(before.state, WorkerStateV1::Running);
    assert!(before.quota_held);
    assert!(fixture.usage().is_empty());
    let worker_revision = before.revision;
    let worker_id = before.worker_id.clone();
    let attempt_id = before.attempt.attempt_id.clone();
    let cancel_id = fixture.cancel_id();
    let cid = before
        .attempt
        .container_id
        .clone()
        .expect("running worker has its durable CID");
    let arm_claim_before = fixture.arm_claim_snapshot();

    for (label, case) in [
        ("still-running", Case::StillRunning),
        ("unknown-container", Case::UnknownContainer),
        ("changed-cid", Case::ChangedCid),
        ("changed-image", Case::ChangedImage),
        ("changed-configuration", Case::ChangedConfiguration),
        ("restart-count", Case::Restarted),
        ("changed-started-at", Case::ChangedStartedAt),
        ("missing-started-at", Case::MissingStartedAt),
    ] {
        eprintln!("cancelled recovery invalid observation={label}");
        fixture.set_case(case);
        assert!(
            fixture.run_sync(&runtime).is_err(),
            "{label} observation must not reconcile the cancelled worker"
        );
        let after = fixture.worker();
        assert_eq!(after.state, WorkerStateV1::Running, "{label}");
        assert!(after.quota_held, "{label}");
        assert_eq!(after.revision, worker_revision, "{label}");
        assert_eq!(after.worker_id, worker_id, "{label}");
        assert_eq!(after.attempt.attempt_id, attempt_id, "{label}");
        assert_eq!(
            after.attempt.container_id.as_deref(),
            Some(cid.as_str()),
            "{label}"
        );
        assert!(fixture.usage().is_empty(), "{label}");
        assert_eq!(fixture.cancel_id(), cancel_id, "{label}");
        assert!(fixture.cancel_present(), "{label}");
        assert_eq!(fixture.arm_claim_snapshot(), arm_claim_before, "{label}");
        assert!(
            fixture
                .runtime_calls()
                .iter()
                .all(|line| { line == "info" || line == &format!("inspect {cid}") }),
            "{label} must not start or clean up a runtime instance"
        );
    }

    fixture.set_case(Case::TerminalAfterCancel);
    fixture
        .run_sync(&runtime)
        .expect("a later exact terminal observation should remain retryable");
    let terminal = fixture.worker();
    assert_eq!(terminal.state, WorkerStateV1::Terminal);
    assert!(!terminal.quota_held);
    assert_eq!(terminal.worker_id, worker_id);
    assert_eq!(terminal.attempt.attempt_id, attempt_id);
    assert_eq!(terminal.attempt.container_id.as_deref(), Some(cid.as_str()));
    assert!(fixture.cancel_present());
    assert_eq!(fixture.cancel_id(), cancel_id);
    assert_eq!(fixture.arm_claim_snapshot(), arm_claim_before);
    assert_eq!(fixture.usage().len(), 1);
    assert!(
        fixture
            .runtime_calls()
            .iter()
            .all(|line| { line == "info" || line == &format!("inspect {cid}") })
    );
}
