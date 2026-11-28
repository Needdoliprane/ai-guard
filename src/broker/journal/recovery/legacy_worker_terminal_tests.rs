use super::super::*;
use super::{RetainedLegacyWorkerTerminalObservationV1, terminal_parent_audit_seq};
use crate::{
    broker::journal::retirement_live_fixture::live_fixture,
    control::{
        ClaimDispositionV1, DecisionPhaseV1, DecisionReceiptV1, LaunchSpecV1, MutationReceiptV1,
        MutationResultV1, NetworkV1, RestartPolicyV1, RuntimeInspectionV1, RuntimeKindV1,
        RuntimeObservationV1, RuntimeStatusV1, SecurityPresetV1, WorkerAttemptV1, WorkerStateV1,
        content_hash,
    },
    model::CommandSpec,
    runner::broker::{ColdRecoverySeed, terminal_pending_seed_for_cold_audit_test},
};

#[tokio::test]
async fn reopens_the_exact_legacy_worker_claim_at_a_confirmed_stop() {
    let seed = terminal_pending_seed_for_cold_audit_test().await;
    tokio::task::spawn_blocking(move || check_reopened_projection(&seed))
        .await
        .unwrap();
}

fn check_reopened_projection(seed: &ColdRecoverySeed) {
    let directory = Directory::open_existing(seed.journal_root()).unwrap();
    let report = StopRecoveryReaderV1::open_existing(
        &directory,
        seed.scope().clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap()
    .unwrap()
    .scan();
    let ExistingProfileV1::Stops(stops) = admit_report(&report, true).unwrap() else {
        panic!("expected the retained Stops journal");
    };
    let operation = seed.operation_id();
    let before = stops.recovery_status(operation).unwrap().unwrap();
    let runtime_calls = seed.runtime_calls();
    let proof = stops
        .legacy_worker_terminal_evidence(operation, seed.cancel_id())
        .unwrap();
    assert_eq!(proof.operation_id(), operation);
    assert_eq!(proof.head(), before.head());
    assert_eq!(
        canonical(proof.scope()).unwrap(),
        canonical(seed.scope()).unwrap()
    );
    let stop = before
        .cancel_receipts()
        .iter()
        .find(|receipt| receipt.command_id() == seed.cancel_id())
        .unwrap();
    assert_eq!(proof.cancel_receipt().head(), stop.head());
    assert_eq!(proof.cancel_receipt().input_hash(), stop.input_hash());
    assert_eq!(proof.cancel_receipt().command_id(), seed.cancel_id());
    assert!(proof.head().revision() > stop.head().revision());

    let history = seed.arm_claim_snapshot().as_array().unwrap();
    let expected_arm = history
        .iter()
        .find(|op| op["kind"] == "arm" && op["request"]["subject"]["kind"] == "worker")
        .unwrap();
    let expected_claim = history
        .iter()
        .find(|op| op["kind"] == "worker_claim")
        .unwrap();
    let (arm, arm_ack) = proof.arm();
    let (claim, claim_ack) = proof.claim();
    assert_eq!(serde_json::to_value(arm).unwrap(), expected_arm["request"]);
    assert_eq!(
        serde_json::to_value(arm_ack).unwrap(),
        expected_arm["response"]["receipt"]
    );
    assert_eq!(
        serde_json::to_value(claim).unwrap(),
        expected_claim["request"]
    );
    assert_eq!(
        serde_json::to_value(claim_ack).unwrap(),
        expected_claim["response"]["receipt"]
    );
    let (authority, owned) = proof.target();
    assert_eq!(
        authority.authority_id.to_string(),
        arm.inspection.authority_id
    );
    assert_eq!(owned.handle.id(), seed.container_id());
    assert_eq!(serde_json::to_value(owned).unwrap(), expected_arm["owned"]);
    assert_eq!(proof.worker().worker_id, seed.worker_id());
    assert_eq!(proof.worker().attempt.attempt_id, seed.attempt_id());
    assert!(proof.worker().observed_started_at.is_some());

    let unrecorded_command = Uuid::new_v4();
    assert!(matches!(
        stops.legacy_worker_terminal_evidence(operation, unrecorded_command),
        Err(JournalError::Interrupted)
    ));
    let after = stops.recovery_status(operation).unwrap().unwrap();
    assert_eq!(after.head(), before.head());
    assert_eq!(
        after.cancel_receipts().len(),
        before.cancel_receipts().len()
    );
    assert_eq!(seed.runtime_calls(), runtime_calls);
}

#[test]
fn confirmed_stop_for_an_import_arm_supplies_no_worker_claim_evidence() {
    let fixture = live_fixture();
    let operation = fixture.intent.operation_id();
    let command = Uuid::new_v4();
    let stops = StopJournal::from_full(&fixture.journal);
    let confirmed = stops.append_cancel(operation, command).unwrap();
    assert_eq!(confirmed.receipt().command_id(), command);
    assert!(matches!(
        stops.legacy_worker_terminal_evidence(operation, command),
        Err(JournalError::Interrupted)
    ));
    assert_eq!(
        stops.recovery_status(operation).unwrap().unwrap().head(),
        confirmed.receipt().head()
    );
}

#[test]
fn validates_only_the_exact_core_receipt_of_the_retained_terminal_observation() -> anyhow::Result<()>
{
    const CLAIM_SEQ: i64 = 17;
    const PARENT_SEQ: i64 = 23;
    let parent = running_worker()?;
    let running = parent
        .last_observation
        .clone()
        .expect("fixture Running observation");
    let running_request = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::from_u128(7).to_string(),
        worker_id: parent.worker_id.clone(),
        attempt_id: parent.attempt.attempt_id.clone(),
        expected_revision: 5,
        transition: WorkerTransitionV1::Observe {
            observation: running.clone(),
        },
    };
    let parent_ack = SupervisionOperationV1::WorkerMutation {
        response: Some(MutationResultV1 {
            worker: parent.clone(),
            receipt: MutationReceiptV1 {
                operation_id: running_request.operation_id.clone(),
                worker_id: Some(parent.worker_id.clone()),
                revision: Some(6),
                input_hash: content_hash("ai-guard:worker-mutation:v1", &running_request)?,
                audit_seq: PARENT_SEQ,
                audit_hash: "a".repeat(64),
            },
            disposition: None,
        }),
        request: running_request,
    };
    let parent_prefix = [parent_ack.clone()];
    let parent_audit_seq = terminal_parent_audit_seq(&parent_prefix, &parent, CLAIM_SEQ)?;
    assert_eq!(parent_audit_seq, PARENT_SEQ);
    let mut other_snapshot = parent.clone();
    other_snapshot.policy_hash = "b".repeat(64);
    assert!(matches!(
        terminal_parent_audit_seq(&parent_prefix, &other_snapshot, CLAIM_SEQ),
        Err(JournalError::Conflict)
    ));
    assert!(matches!(
        terminal_parent_audit_seq(&[parent_ack.clone(), parent_ack], &parent, CLAIM_SEQ),
        Err(JournalError::Conflict)
    ));

    let terminal = RuntimeObservationV1 {
        observed_at: "2026-11-28T01:00:04Z".into(),
        status: RuntimeStatusV1::Exited,
        running: Some(false),
        finished_at: Some("2026-11-28T01:00:03Z".into()),
        exit_code: Some(17),
        ..running
    };
    let request = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::from_u128(8).to_string(),
        worker_id: parent.worker_id.clone(),
        attempt_id: parent.attempt.attempt_id.clone(),
        expected_revision: 6,
        transition: WorkerTransitionV1::Observe {
            observation: terminal.clone(),
        },
    };
    let expected_worker = crate::audit::expected_worker_transition(&parent, &request)?;
    let mut worker = parent;
    worker.state = WorkerStateV1::Terminal;
    worker.revision = 7;
    worker.quota_held = false;
    worker.last_observation = Some(terminal.clone());
    let exact = MutationResultV1 {
        worker,
        receipt: MutationReceiptV1 {
            operation_id: request.operation_id.clone(),
            worker_id: Some(request.worker_id.clone()),
            revision: Some(7),
            input_hash: content_hash("ai-guard:worker-mutation:v1", &request)?,
            audit_seq: PARENT_SEQ + 6,
            audit_hash: "c".repeat(64),
        },
        disposition: None,
    };
    let mut different_request = request.clone();
    different_request.transition = WorkerTransitionV1::Observe {
        observation: RuntimeObservationV1 {
            exit_code: Some(18),
            ..terminal
        },
    };
    let different_hash = content_hash("ai-guard:worker-mutation:v1", &different_request)?;
    let head = RevisionRefV1 {
        revision: 9,
        hash: "d".repeat(64),
    };
    let pending = RetainedLegacyWorkerTerminalObservationV1 {
        head: head.clone(),
        stop: LocalCommandReceiptV1 {
            command_id: Uuid::from_u128(9),
            input_hash: "e".repeat(64),
            head: RevisionRefV1 {
                revision: 8,
                hash: "f".repeat(64),
            },
        },
        request,
        expected_worker,
        parent_audit_seq,
    };
    let before = canonical(pending.request())?;
    pending.validate_response(&exact)?;
    pending.validate_response(&exact)?;

    let rejects = |mutate: fn(&mut MutationResultV1)| {
        let mut other = exact.clone();
        mutate(&mut other);
        assert!(matches!(
            pending.validate_response(&other),
            Err(JournalError::Conflict)
        ));
    };
    rejects(|other| other.receipt.operation_id = Uuid::from_u128(11).to_string());
    rejects(|other| other.receipt.worker_id = Some(Uuid::from_u128(12).to_string()));
    rejects(|other| other.receipt.revision = Some(8));
    rejects(|other| other.worker.revision = 8);
    rejects(|other| other.worker.attempt.attempt_id = Uuid::from_u128(13).to_string());
    rejects(|other| other.worker.attempt.container_id = Some("0".repeat(64)));
    rejects(|other| other.worker.quota_held = true);
    rejects(|other| other.worker.last_observation = None);
    rejects(|other| other.receipt.audit_seq = CLAIM_SEQ + 1);
    rejects(|other| other.receipt.audit_seq = PARENT_SEQ);
    rejects(|other| other.receipt.audit_hash = "C".repeat(64));
    rejects(|other| other.disposition = Some(ClaimDispositionV1::AlreadyConsumed));
    let mut other_body = exact.clone();
    other_body.receipt.input_hash = different_hash;
    assert!(matches!(
        pending.validate_response(&other_body),
        Err(JournalError::Conflict)
    ));
    assert_eq!(canonical(pending.request())?, before);
    assert_eq!(pending.head(), &head);
    Ok(())
}

fn running_worker() -> anyhow::Result<WorkerV1> {
    let start = "2026-11-28T01:00:01Z";
    let launch = LaunchSpecV1 {
        version: 1,
        session_id: Uuid::from_u128(1).to_string(),
        command: CommandSpec {
            argv: vec!["/usr/bin/id".into()],
            cwd: "/workspace".into(),
        },
        profile: "strict".into(),
        runtime_kind: RuntimeKindV1::Docker,
        authority_id: Uuid::from_u128(2).to_string(),
        image_ref: "test-image".into(),
        image_id: format!("sha256:{}", "1".repeat(64)),
        network: NetworkV1::None,
        cpu_millis: 2000,
        memory_bytes: 1 << 30,
        pids_limit: 256,
        timeout_seconds: 60,
        security_preset: SecurityPresetV1::FixedSandboxV1,
        restart_policy: RestartPolicyV1::No,
        workspace_manifest_digest: "2".repeat(64),
        workspace_import_policy_version: 1,
        provider: "generic".into(),
        capture_output: false,
        credential_binding_id: None,
        credential_names: Vec::new(),
        provider_resolution: None,
        egress: None,
    };
    let inspection = RuntimeInspectionV1 {
        authority_id: launch.authority_id.clone(),
        runtime_kind: launch.runtime_kind,
        container_id: "3".repeat(64),
        stable_name: "test-worker".into(),
        volume_name: "test-worker-work".into(),
        image_id: launch.image_id.clone(),
        spec_hash: launch.spec_hash()?,
        cpu_millis: launch.cpu_millis,
        memory_bytes: launch.memory_bytes,
        pids_limit: launch.pids_limit,
        network: launch.network,
        security_preset: launch.security_preset,
        restart_policy: launch.restart_policy,
    };
    let request_id = Uuid::from_u128(3).to_string();
    Ok(WorkerV1 {
        worker_id: Uuid::from_u128(4).to_string(),
        admission_operation_id: request_id.clone(),
        request_id: request_id.clone(),
        receipt: DecisionReceiptV1 {
            version: 1,
            request_id,
            request_hash: "4".repeat(64),
            phase: DecisionPhaseV1::Initial,
            audit_seq: 1,
            audit_hash: "5".repeat(64),
        },
        launch,
        spec_hash: inspection.spec_hash.clone(),
        policy_hash: "6".repeat(64),
        state: WorkerStateV1::Running,
        quota_held: true,
        revision: 6,
        attempt: WorkerAttemptV1 {
            attempt_id: Uuid::from_u128(5).to_string(),
            stable_name: inspection.stable_name.clone(),
            volume_name: inspection.volume_name.clone(),
            container_id: Some(inspection.container_id.clone()),
            created_inspection: Some(inspection.clone()),
            create_operation_id: Some(Uuid::from_u128(6).to_string()),
            start_operation_id: Some(Uuid::from_u128(10).to_string()),
            start_emitter_id: Some(Uuid::from_u128(14).to_string()),
        },
        observed_started_at: Some(start.into()),
        allocation_inconsistent: false,
        last_observation: Some(RuntimeObservationV1 {
            inspection,
            observed_at: start.into(),
            status: RuntimeStatusV1::Running,
            running: Some(true),
            started_at: Some(start.into()),
            finished_at: None,
            exit_code: None,
            oom_killed: Some(false),
            restart_count: Some(0),
        }),
        unknown_reason: None,
        cleanup_removed: None,
        cleanup_error: None,
    })
}
