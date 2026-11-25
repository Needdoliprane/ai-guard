//! G06/R2 first slice: read-only selection of a closed supervised worker on
//! the maintenance path. The synthetic supervised cycle provides the fixture
//! oracle of the ticket (`.scratch/v1-resume/issues/12-g06-retention-r2.md`):
//! a terminal attempt whose worker and helpers are cleaned, correlated
//! receipts, and two controlled heads (verified Store head + broker Full
//! journal head of the same operation). No publication happens here.
use super::*;
use agentsandbox::audit::retention::{
    SEAL_SELECTION_FORMAT, SealBrokerHeadV1, SealProofRefV1,
    failure::{code as seal_code, refusal as seal_refusal},
};

/// Every receipt of one complete supervised cycle, in admission order.
struct Cycle {
    worker: WorkerV1,
    decision: DecisionReceiptV1,
    admission: MutationReceiptV1,
    prepare: MutationReceiptV1,
    import_arm: ArmResultV1,
    import_claim: HelperClaimResultV1,
    import_terminal: MutationReceiptV1,
    import_cleanup: MutationReceiptV1,
    verify_arm: ArmResultV1,
    verify_claim: HelperClaimResultV1,
    verify_terminal: MutationReceiptV1,
    verify_cleanup: MutationReceiptV1,
    create_issued: MutationReceiptV1,
    created: MutationReceiptV1,
    worker_arm: ArmResultV1,
    start_claim: MutationReceiptV1,
    running: MutationReceiptV1,
    stop: MutationReceiptV1,
    terminal: MutationReceiptV1,
    cleanup: MutationReceiptV1,
    inspection: RuntimeInspectionV1,
}

/// The happy path of the supervised cycle fixture, without its negative
/// detours: admission, import and verification helpers, worker arm and
/// consumed claim, running observation, stop, terminal observation of the
/// same container, confirmed cleanup.
async fn supervised_cycle(fixture: &Fixture) -> Cycle {
    let admission_a = supervised(admission(&fixture.a));
    let decision = admission_a.admission.receipt.clone();
    let response = json_request(
        &fixture.router,
        Method::POST,
        "/api/v1/workers/supervised-reservations",
        Some(&fixture.runner_a),
        &admission_a,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let reserved: AdmissionResultV1 = response_json(response).await;
    assert!(reserved.accepted && !reserved.replayed);
    let mut worker = reserved.worker.clone().unwrap();
    let admission_receipt = reserved.receipt.clone();

    let prepare = transition_with_receipt(
        &fixture.router,
        &fixture.runner_a,
        &worker,
        WorkerTransitionV1::Prepare,
    )
    .await;
    worker = prepare.worker.clone();

    let import_id = Uuid::new_v4();
    let import_arm = helper_arm(
        &worker,
        HelperPurposeV1::Import,
        import_id,
        HelperPermissionV1::Admission {
            receipt: admission_receipt.clone(),
        },
        "1".repeat(64).as_str(),
    );
    let import_path = format!("/api/v1/workers/{}/arms", worker.worker_id);
    let response = json_request(
        &fixture.router,
        Method::POST,
        &import_path,
        Some(&fixture.runner_a),
        &import_arm,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let import_arm_result: ArmResultV1 = response_json(response).await;
    let helper_claim_path = format!("/api/v1/workers/{}/helper-start-claims", worker.worker_id);
    let import_claim = HelperStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: import_id.to_string(),
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: import_arm_result.receipt.clone(),
        clock_check: host_clock(100_000_000_001),
    };
    let response = json_request(
        &fixture.router,
        Method::POST,
        &helper_claim_path,
        Some(&fixture.runner_a),
        &import_claim,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let import_claim_result: HelperClaimResultV1 = response_json(response).await;
    assert_eq!(
        import_claim_result.disposition,
        ClaimDispositionV1::EmitOnce
    );
    let response = json_request(
        &fixture.router,
        Method::POST,
        &format!("/api/v1/workers/{}/helper-observations", worker.worker_id),
        Some(&fixture.runner_a),
        &helper_observation(&import_arm_result, import_id, None),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let import_terminal: MutationReceiptV1 = response_json(response).await;
    let response = json_request(
        &fixture.router,
        Method::POST,
        &format!("/api/v1/workers/{}/helper-cleanup", worker.worker_id),
        Some(&fixture.runner_a),
        &HelperCleanupV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            helper_id: import_id.to_string(),
            arm_receipt: import_arm_result.receipt.clone(),
            removed: true,
            error: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let import_cleanup: MutationReceiptV1 = response_json(response).await;

    let verify_id = Uuid::new_v4();
    let verify_arm = helper_arm(
        &worker,
        HelperPurposeV1::ImportVerification,
        verify_id,
        HelperPermissionV1::ImportVerification {
            import_helper_id: import_id.to_string(),
            import_arm: import_arm_result.receipt.clone(),
            import_terminal: import_terminal.clone(),
            manifest_digest: worker.launch.workspace_manifest_digest.clone(),
        },
        "2".repeat(64).as_str(),
    );
    let response = json_request(
        &fixture.router,
        Method::POST,
        &import_path,
        Some(&fixture.runner_a),
        &verify_arm,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let verify_result: ArmResultV1 = response_json(response).await;
    let verify_claim = HelperStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: verify_id.to_string(),
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: verify_result.receipt.clone(),
        clock_check: host_clock(100_000_000_001),
    };
    let response = json_request(
        &fixture.router,
        Method::POST,
        &helper_claim_path,
        Some(&fixture.runner_a),
        &verify_claim,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let verify_claim_result: HelperClaimResultV1 = response_json(response).await;
    let verification = Some(ImportVerificationReportV1 {
        import_helper_id: import_id.to_string(),
        expected_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
        observed_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
    });
    let response = json_request(
        &fixture.router,
        Method::POST,
        &format!("/api/v1/workers/{}/helper-observations", worker.worker_id),
        Some(&fixture.runner_a),
        &helper_observation(&verify_result, verify_id, verification),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let verify_terminal: MutationReceiptV1 = response_json(response).await;
    let response = json_request(
        &fixture.router,
        Method::POST,
        &format!("/api/v1/workers/{}/helper-cleanup", worker.worker_id),
        Some(&fixture.runner_a),
        &HelperCleanupV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            helper_id: verify_id.to_string(),
            arm_receipt: verify_result.receipt.clone(),
            removed: true,
            error: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let verify_cleanup: MutationReceiptV1 = response_json(response).await;

    let create_issued = transition_with_receipt(
        &fixture.router,
        &fixture.runner_a,
        &worker,
        WorkerTransitionV1::CreateIssued,
    )
    .await;
    worker = create_issued.worker.clone();
    let inspection = worker_inspection(&worker);
    let created = transition_with_receipt(
        &fixture.router,
        &fixture.runner_a,
        &worker,
        WorkerTransitionV1::Created {
            inspection: inspection.clone(),
        },
    )
    .await;
    worker = created.worker.clone();

    let worker_arm = ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        intent_hash: HASH_C.into(),
        subject: ArmSubjectV1::Worker,
        inspection: inspection.clone(),
        authority_binding_hash: HASH_A.into(),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: HASH_B.into(),
        clock: host_clock(100_000_000_000),
        timeout_seconds: worker.launch.timeout_seconds,
        deadline_ns_decimal: (100_000_000_000_u128
            + u128::from(worker.launch.timeout_seconds) * 1_000_000_000)
            .to_string(),
    };
    let response = json_request(
        &fixture.router,
        Method::POST,
        &format!("/api/v1/workers/{}/arms", worker.worker_id),
        Some(&fixture.runner_a),
        &worker_arm,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let worker_arm_result: ArmResultV1 = response_json(response).await;
    let claim = ArmedStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: worker_arm_result.receipt.clone(),
        clock_check: host_clock(100_000_000_001),
    };
    let response = json_request(
        &fixture.router,
        Method::POST,
        &format!("/api/v1/workers/{}/armed-start-claims", worker.worker_id),
        Some(&fixture.runner_a),
        &claim,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: MutationResultV1 = response_json(response).await;
    assert_eq!(claimed.disposition, Some(ClaimDispositionV1::EmitOnce));
    worker = claimed.worker.clone();

    let running = transition_with_receipt(
        &fixture.router,
        &fixture.runner_a,
        &worker,
        WorkerTransitionV1::Observe {
            observation: worker_observation(
                &inspection,
                RuntimeStatusV1::Running,
                true,
                None,
                None,
            ),
        },
    )
    .await;
    worker = running.worker.clone();
    let stop = transition_with_receipt(
        &fixture.router,
        &fixture.runner_a,
        &worker,
        WorkerTransitionV1::Stop,
    )
    .await;
    worker = stop.worker.clone();
    let terminal = transition_with_receipt(
        &fixture.router,
        &fixture.runner_a,
        &worker,
        WorkerTransitionV1::Observe {
            observation: worker_observation(
                &inspection,
                RuntimeStatusV1::Exited,
                false,
                Some("2026-10-30T12:00:01Z"),
                Some(0),
            ),
        },
    )
    .await;
    assert_eq!(terminal.worker.state, WorkerStateV1::Terminal);
    let cleanup = transition_with_receipt(
        &fixture.router,
        &fixture.runner_a,
        &terminal.worker,
        WorkerTransitionV1::CleanupResult {
            removed: true,
            error: None,
        },
    )
    .await;
    worker = cleanup.worker.clone();
    assert_eq!(worker.cleanup_removed, Some(true));
    assert!(!worker.quota_held);

    Cycle {
        worker,
        decision,
        admission: admission_receipt,
        prepare: prepare.receipt,
        import_arm: import_arm_result,
        import_claim: import_claim_result,
        import_terminal,
        import_cleanup,
        verify_arm: verify_result,
        verify_claim: verify_claim_result,
        verify_terminal,
        verify_cleanup,
        create_issued: create_issued.receipt,
        created: created.receipt,
        worker_arm: worker_arm_result,
        start_claim: claimed.receipt,
        running: running.receipt,
        stop: stop.receipt,
        terminal: terminal.receipt,
        cleanup: cleanup.receipt,
        inspection,
    }
}

/// The maintenance path: an independent open of the same private SQLite file.
fn maintenance_open(fixture: &Fixture) -> Store {
    Store::open_existing_for_tenant(
        &fixture._directory.path().join("a.sqlite"),
        "tenant-a",
        Redactor::new(vec![]),
        10_000,
    )
    .unwrap()
}

/// The controlled broker Full journal head of the worker's start operation,
/// as the maintenance caller reads it from an accepted, settled journal entry.
fn settled_broker_head(cycle: &Cycle) -> SealBrokerHeadV1 {
    SealBrokerHeadV1 {
        operation_id: cycle
            .worker
            .attempt
            .start_operation_id
            .clone()
            .expect("the cycle consumed the armed start"),
        head_revision: Some(9),
        head_hash: Some("9".repeat(64)),
        worker_state: Some(WorkerStateV1::Terminal),
        quota_held: Some(false),
        runtime_status: Some(RuntimeStatusV1::Exited),
        pending_rpc_count: Some(0),
        cancel_requested: false,
    }
}

/// The exact ordered reference set of the cycle: the decision, then every
/// correlated receipt from admission to confirmed cleanup.
fn expected_references(cycle: &Cycle) -> Vec<SealProofRefV1> {
    let mut refs = vec![SealProofRefV1 {
        seq: cycle.decision.audit_seq,
        hash: cycle.decision.audit_hash.clone(),
        kind: "security.decision".into(),
        action: "decision".into(),
        operation_id: None,
    }];
    let mut push = |seq: i64, hash: &str, kind: &str, action: &str, operation: Option<String>| {
        refs.push(SealProofRefV1 {
            seq,
            hash: hash.into(),
            kind: kind.into(),
            action: action.into(),
            operation_id: operation,
        });
    };
    let receipt = |r: &MutationReceiptV1| Some(r.operation_id.clone());
    push(
        cycle.admission.audit_seq,
        &cycle.admission.audit_hash,
        TRANSITION,
        "supervised_admit",
        receipt(&cycle.admission),
    );
    push(
        cycle.prepare.audit_seq,
        &cycle.prepare.audit_hash,
        WORKER_TRANSITION,
        "prepare",
        receipt(&cycle.prepare),
    );
    push(
        cycle.import_arm.receipt.audit_seq,
        &cycle.import_arm.receipt.audit_hash,
        ARM,
        "arm",
        Some(cycle.import_arm.receipt.arm_id.clone()),
    );
    push(
        cycle.import_claim.receipt.audit_seq,
        &cycle.import_claim.receipt.audit_hash,
        TRANSITION,
        "helper_start_claim",
        receipt(&cycle.import_claim.receipt),
    );
    push(
        cycle.import_terminal.audit_seq,
        &cycle.import_terminal.audit_hash,
        TRANSITION,
        "helper_observe",
        receipt(&cycle.import_terminal),
    );
    push(
        cycle.import_cleanup.audit_seq,
        &cycle.import_cleanup.audit_hash,
        TRANSITION,
        "helper_cleanup",
        receipt(&cycle.import_cleanup),
    );
    push(
        cycle.verify_arm.receipt.audit_seq,
        &cycle.verify_arm.receipt.audit_hash,
        ARM,
        "arm",
        Some(cycle.verify_arm.receipt.arm_id.clone()),
    );
    push(
        cycle.verify_claim.receipt.audit_seq,
        &cycle.verify_claim.receipt.audit_hash,
        TRANSITION,
        "helper_start_claim",
        receipt(&cycle.verify_claim.receipt),
    );
    push(
        cycle.verify_terminal.audit_seq,
        &cycle.verify_terminal.audit_hash,
        TRANSITION,
        "helper_observe",
        receipt(&cycle.verify_terminal),
    );
    push(
        cycle.verify_cleanup.audit_seq,
        &cycle.verify_cleanup.audit_hash,
        TRANSITION,
        "helper_cleanup",
        receipt(&cycle.verify_cleanup),
    );
    push(
        cycle.create_issued.audit_seq,
        &cycle.create_issued.audit_hash,
        WORKER_TRANSITION,
        "create_issued",
        receipt(&cycle.create_issued),
    );
    push(
        cycle.created.audit_seq,
        &cycle.created.audit_hash,
        WORKER_TRANSITION,
        "created",
        receipt(&cycle.created),
    );
    push(
        cycle.worker_arm.receipt.audit_seq,
        &cycle.worker_arm.receipt.audit_hash,
        ARM,
        "arm",
        Some(cycle.worker_arm.receipt.arm_id.clone()),
    );
    push(
        cycle.start_claim.audit_seq,
        &cycle.start_claim.audit_hash,
        TRANSITION,
        "armed_start_claim",
        receipt(&cycle.start_claim),
    );
    push(
        cycle.running.audit_seq,
        &cycle.running.audit_hash,
        WORKER_TRANSITION,
        "observe",
        receipt(&cycle.running),
    );
    push(
        cycle.stop.audit_seq,
        &cycle.stop.audit_hash,
        WORKER_TRANSITION,
        "stop",
        receipt(&cycle.stop),
    );
    push(
        cycle.terminal.audit_seq,
        &cycle.terminal.audit_hash,
        WORKER_TRANSITION,
        "observe",
        receipt(&cycle.terminal),
    );
    push(
        cycle.cleanup.audit_seq,
        &cycle.cleanup.audit_hash,
        WORKER_TRANSITION,
        "cleanup_result",
        receipt(&cycle.cleanup),
    );
    refs.sort_by_key(|reference| reference.seq);
    refs
}

const TRANSITION: &str = "control.supervision.transition";
const WORKER_TRANSITION: &str = "control.worker.transition";
const ARM: &str = "control.execution.arm";

#[tokio::test]
async fn seal_selection_returns_the_exact_ordered_reference_set_from_two_controlled_heads() {
    let fixture = Fixture::new();
    let cycle = supervised_cycle(&fixture).await;
    let maintenance = maintenance_open(&fixture);
    assert!(!maintenance.supervision_state().unwrap().active);
    let head_before = maintenance.head().unwrap();
    let ledger_before = maintenance.audit_retention_ledger().unwrap();

    let selection = maintenance
        .select_sealed_worker(&cycle.worker.worker_id, &settled_broker_head(&cycle))
        .expect("the cleaned terminal cycle is selectable");

    assert_eq!(selection.format, SEAL_SELECTION_FORMAT);
    assert_eq!(selection.tenant_id, "tenant-a");
    assert_eq!(selection.worker_id, cycle.worker.worker_id);
    assert_eq!(selection.attempt_id, cycle.worker.attempt.attempt_id);
    // The central proof is anchored to the verified Store head: the cleanup
    // receipt is the last event of this quiescent fixture.
    assert_eq!(selection.store_head_seq, cycle.cleanup.audit_seq);
    assert_eq!(selection.store_head_hash, cycle.cleanup.audit_hash);
    // The broker head names the same operation and is carried verbatim.
    assert_eq!(
        selection.broker_operation_id,
        cycle.worker.attempt.start_operation_id.clone().unwrap()
    );
    assert_eq!(selection.broker_head_revision, 9);
    assert_eq!(selection.broker_head_hash, "9".repeat(64));
    // The unique non-billable allocation is linked to the terminal
    // observation of the same container.
    assert_eq!(
        selection.terminal_observation_id,
        cycle.terminal.operation_id
    );
    assert!(!selection.allocation.billable);
    assert_eq!(selection.allocation.worker_id, cycle.worker.worker_id);
    assert_eq!(
        selection.allocation.attempt_id,
        cycle.worker.attempt.attempt_id
    );
    assert_eq!(
        selection.allocation.terminal_observation_id,
        cycle.terminal.operation_id
    );
    assert_eq!(
        selection.allocation.source.container_id,
        cycle.inspection.container_id
    );
    // The exact ordered reference set, from the decision to the cleanup.
    assert_eq!(selection.references, expected_references(&cycle));
    assert!(
        selection
            .references
            .windows(2)
            .all(|pair| pair[0].seq < pair[1].seq)
    );
    assert_eq!(selection.references.first().unwrap().action, "decision");
    assert_eq!(
        selection.references.last().unwrap().action,
        "cleanup_result"
    );
    // Read-only: head, ledger, quota and projections are untouched.
    assert_eq!(maintenance.head().unwrap(), head_before);
    assert_eq!(maintenance.audit_retention_ledger().unwrap(), ledger_before);
    let quota = maintenance
        .worker_quota(&agentsandbox::control::QuotaPolicyV1::default())
        .unwrap();
    assert_eq!(quota.held_workers, 0);
    let retained = maintenance
        .worker(&cycle.worker.worker_id)
        .unwrap()
        .unwrap();
    assert_eq!(retained.state, WorkerStateV1::Terminal);
    assert_eq!(retained.cleanup_removed, Some(true));
    assert!(!retained.quota_held);
}

#[tokio::test]
async fn seal_selection_refuses_a_pending_export_and_preserves_head_receipts_quota_and_registry() {
    let fixture = Fixture::new();
    let cycle = supervised_cycle(&fixture).await;
    // A distinct case: an export grant stays pending after the terminal.
    let export = ExportRequestV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: cycle.worker.worker_id.clone(),
        attempt_id: cycle.worker.attempt.attempt_id.clone(),
        destination_ref: Uuid::new_v4().to_string(),
        destination_intent_hash: HASH_B.into(),
    };
    let response = json_request(
        &fixture.router,
        Method::POST,
        &format!("/api/v1/workers/{}/export-requests", cycle.worker.worker_id),
        Some(&fixture.runner_a),
        &export,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let export_receipt: MutationReceiptV1 = response_json(response).await;

    let maintenance = maintenance_open(&fixture);
    let head_before = maintenance.head().unwrap();
    assert_eq!(head_before, export_receipt.audit_seq);
    let integrity_before = maintenance.integrity().unwrap();
    assert!(integrity_before.valid);
    let ledger_before = maintenance.audit_retention_ledger().unwrap();
    let quota_policy = agentsandbox::control::QuotaPolicyV1::default();
    let quota_before = maintenance.worker_quota(&quota_policy).unwrap();
    let usage_before = maintenance.worker_usage().unwrap();
    let worker_before = maintenance
        .worker(&cycle.worker.worker_id)
        .unwrap()
        .unwrap();

    let refused = maintenance
        .select_sealed_worker(&cycle.worker.worker_id, &settled_broker_head(&cycle))
        .expect_err("a pending export leaves the closure incomplete");
    let failure = seal_refusal(&refused).expect("a closed retention refusal");
    assert_eq!(failure.code, seal_code::SEAL_CLOSURE_INCOMPLETE);
    assert_eq!(failure.detail["reason"], "pending_export");
    assert_eq!(failure.detail["operation_id"], export.operation_id);

    // Nothing changed: head, receipts, quota, projections, retention registry.
    assert_eq!(maintenance.head().unwrap(), head_before);
    let integrity_after = maintenance.integrity().unwrap();
    assert!(integrity_after.valid);
    assert_eq!(integrity_after.head_hash, integrity_before.head_hash);
    assert_eq!(integrity_after.event_count, integrity_before.event_count);
    assert_eq!(maintenance.audit_retention_ledger().unwrap(), ledger_before);
    assert_eq!(
        serde_json::to_value(maintenance.worker_quota(&quota_policy).unwrap()).unwrap(),
        serde_json::to_value(&quota_before).unwrap()
    );
    assert_eq!(
        serde_json::to_value(maintenance.worker_usage().unwrap()).unwrap(),
        serde_json::to_value(&usage_before).unwrap()
    );
    assert_eq!(
        serde_json::to_value(
            maintenance
                .worker(&cycle.worker.worker_id)
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&worker_before).unwrap()
    );
    // The export grant itself stays readable at the preserved head.
    let facts = maintenance
        .events(&EventQuery {
            after: Some(cycle.cleanup.audit_seq),
            order: Some("asc".into()),
            limit: Some(8),
            ..Default::default()
        })
        .unwrap();
    let [grant] = facts.as_slice() else {
        panic!("only the pending export grant follows the cleanup");
    };
    assert_eq!(grant.seq, export_receipt.audit_seq);
    assert_eq!(grant.hash, export_receipt.audit_hash);
}

#[tokio::test]
async fn seal_selection_refuses_an_incomplete_or_divergent_broker_head() {
    let fixture = Fixture::new();
    let cycle = supervised_cycle(&fixture).await;
    let maintenance = maintenance_open(&fixture);
    let head_before = maintenance.head().unwrap();
    let settled = settled_broker_head(&cycle);

    let mut missing_head = settled.clone();
    missing_head.head_revision = None;
    missing_head.head_hash = None;
    let mut divergent_state = settled.clone();
    divergent_state.worker_state = Some(WorkerStateV1::Running);
    let mut held_quota = settled.clone();
    held_quota.quota_held = Some(true);
    let mut uncertain_publication = settled.clone();
    uncertain_publication.pending_rpc_count = None;
    let mut cancelled = settled.clone();
    cancelled.cancel_requested = true;
    let mut other_operation = settled.clone();
    other_operation.operation_id = Uuid::new_v4().to_string();

    for (name, head) in [
        ("missing_head", missing_head),
        ("divergent_state", divergent_state),
        ("held_quota", held_quota),
        ("uncertain_publication", uncertain_publication),
        ("cancelled", cancelled),
        ("other_operation", other_operation),
    ] {
        let refused = maintenance
            .select_sealed_worker(&cycle.worker.worker_id, &head)
            .expect_err(name);
        let failure = seal_refusal(&refused).expect("a closed retention refusal");
        assert_eq!(
            failure.code,
            seal_code::SEAL_BROKER_HEAD_INCOMPLETE,
            "{name}: {:?}",
            failure.detail
        );
        assert_eq!(maintenance.head().unwrap(), head_before, "{name}");
    }
    // The settled head still selects after every refusal.
    let selection = maintenance
        .select_sealed_worker(&cycle.worker.worker_id, &settled)
        .unwrap();
    assert_eq!(selection.references, expected_references(&cycle));
}
