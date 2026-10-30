mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    control::{
        AdmissionResultV1, AdmissionV1, ArmReceiptV1, ArmSubjectV1, ArmV1, ArmedStartClaimV1,
        BindLegacyIntentV1, ClaimDispositionV1, ControlFailure, DecisionPhaseV1, DecisionReceiptV1,
        ExportRequestV1, HelperCleanupV1, HelperObservationV1, HelperPermissionV1, HelperPurposeV1,
        HelperStartClaimV1, HostClockMethodV1, HostClockSampleV1, LaunchSpecV1, NetworkV1,
        QuotaPolicyV1, RestartPolicyV1, RuntimeInspectionV1, RuntimeKindV1, RuntimeObservationV1,
        RuntimeStatusV1, SecurityPresetV1, SupervisedAdmissionV1, SupervisionProtocolV1,
        WorkerMutationV1, WorkerStateV1, WorkerTransitionV1, WorkerV1, helper_execution_policy_v1,
    },
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    redaction::Redactor,
};
use rusqlite::{Connection, backup::Backup, params};
use serde_json::json;
use std::{path::Path, sync::Arc, sync::Barrier, thread, time::Duration};
use uuid::Uuid;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn open(path: &Path, max_events: u64) -> Store {
    Store::open(path, Redactor::new(vec![]), max_events).unwrap()
}

fn decision_admission(store: &Store) -> AdmissionV1 {
    let session_id = Uuid::new_v4().to_string();
    let request = DecisionRequest {
        request_id: Uuid::new_v4().to_string(),
        profile: "strict".into(),
        command: CommandSpec {
            argv: vec!["/usr/bin/id".into()],
            cwd: "/workspace".into(),
        },
        project_id: None,
        sandbox_id: None,
        agent_id: None,
        session_id: Some(session_id.clone()),
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    };
    let policy = Policy::default();
    let stored = store
        .decision(
            &request,
            Some((policy::evaluate(&policy, &request), json!(null))),
        )
        .unwrap()
        .unwrap();
    AdmissionV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        receipt: DecisionReceiptV1 {
            version: 1,
            request_id: request.request_id.clone(),
            request_hash: policy::request_hash(&request),
            phase: DecisionPhaseV1::Initial,
            audit_seq: stored.seq,
            audit_hash: stored.hash,
        },
        launch: LaunchSpecV1 {
            version: 1,
            session_id,
            command: request.command.clone(),
            profile: request.profile.clone(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: Uuid::new_v4().to_string(),
            image_ref: "worker:fixture".into(),
            image_id: format!("sha256:{HASH_A}"),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            timeout_seconds: 600,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: HASH_B.into(),
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
        },
        decision_request: request,
    }
}

fn supervised_request(admission: AdmissionV1) -> SupervisedAdmissionV1 {
    let compact = Uuid::new_v4().simple().to_string();
    SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: format!("{compact}{compact}"),
        admission,
    }
}

fn admit_legacy(store: &Store, admission: &AdmissionV1) -> AdmissionResultV1 {
    let policy = Policy::default();
    store
        .admit_worker(
            admission,
            &policy,
            policy::evaluate(&policy, &admission.decision_request),
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .unwrap()
}

fn admit_supervised(store: &Store, request: &SupervisedAdmissionV1) -> AdmissionResultV1 {
    let policy = Policy::default();
    store
        .admit_supervised(
            request,
            &policy,
            policy::evaluate(&policy, &request.admission.decision_request),
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .unwrap()
}

fn mutation(worker: &WorkerV1, transition: WorkerTransitionV1) -> WorkerMutationV1 {
    WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition,
    }
}

fn inspection(worker: &WorkerV1) -> RuntimeInspectionV1 {
    RuntimeInspectionV1 {
        authority_id: worker.launch.authority_id.clone(),
        runtime_kind: worker.launch.runtime_kind,
        container_id: "d".repeat(64),
        stable_name: worker.attempt.stable_name.clone(),
        volume_name: worker.attempt.volume_name.clone(),
        image_id: worker.launch.image_id.clone(),
        spec_hash: worker.spec_hash.clone(),
        cpu_millis: worker.launch.cpu_millis,
        memory_bytes: worker.launch.memory_bytes,
        pids_limit: worker.launch.pids_limit,
        network: worker.launch.network,
        security_preset: worker.launch.security_preset,
        restart_policy: worker.launch.restart_policy,
    }
}

fn create_legacy_worker(store: &Store, admission: &AdmissionV1) -> WorkerV1 {
    let mut worker = admit_legacy(store, admission).worker.unwrap();
    worker = store
        .mutate_worker(&mutation(&worker, WorkerTransitionV1::Prepare))
        .unwrap()
        .worker;
    worker = store
        .mutate_worker(&mutation(&worker, WorkerTransitionV1::CreateIssued))
        .unwrap()
        .worker;
    store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::Created {
                inspection: inspection(&worker),
            },
        ))
        .unwrap()
        .worker
}

fn host_clock(ns: u128) -> HostClockSampleV1 {
    HostClockSampleV1 {
        version: 1,
        method: HostClockMethodV1::DarwinMachContinuousV1,
        boot_id: "11111111-1111-4111-8111-111111111111".into(),
        continuous_ns_decimal: ns.to_string(),
        observed_utc: Some("2026-10-30T12:00:00Z".into()),
    }
}

fn worker_arm(worker: &WorkerV1, intent_hash: &str) -> ArmV1 {
    let inspection = worker.attempt.created_inspection.clone().unwrap();
    let start = 100_000_000_000_u128;
    let timeout = worker.launch.timeout_seconds;
    ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        intent_hash: intent_hash.into(),
        subject: ArmSubjectV1::Worker,
        inspection,
        authority_binding_hash: HASH_A.into(),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: HASH_B.into(),
        clock: host_clock(start),
        timeout_seconds: timeout,
        deadline_ns_decimal: (start + u128::from(timeout) * 1_000_000_000).to_string(),
    }
}

fn helper_arm(
    worker: &WorkerV1,
    intent_hash: &str,
    helper_id: Uuid,
    purpose: HelperPurposeV1,
    permission: HelperPermissionV1,
) -> ArmV1 {
    worker
        .launch
        .validate()
        .expect("parent launch must be valid");
    assert_eq!(worker.spec_hash, worker.launch.spec_hash().unwrap());
    let policy = helper_execution_policy_v1(worker, purpose, helper_id).unwrap();
    let start = 100_000_000_000_u128;
    let timeout = 120_u32;
    ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        intent_hash: intent_hash.into(),
        subject: ArmSubjectV1::Helper {
            helper_id: helper_id.to_string(),
            purpose,
            policy_version: 1,
            permission: Box::new(permission),
        },
        inspection: RuntimeInspectionV1 {
            authority_id: policy.authority_id.clone(),
            runtime_kind: policy.runtime_kind,
            container_id: format!("{:064x}", helper_id.as_u128()),
            stable_name: policy.stable_name.clone(),
            volume_name: policy.volume_name.clone(),
            image_id: policy.image_id.clone(),
            spec_hash: policy.spec_hash().unwrap(),
            cpu_millis: policy.cpu_millis,
            memory_bytes: policy.memory_bytes,
            pids_limit: policy.pids_limit,
            network: policy.network,
            security_preset: policy.security_preset,
            restart_policy: policy.restart_policy,
        },
        authority_binding_hash: HASH_A.into(),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: HASH_C.into(),
        clock: host_clock(start),
        timeout_seconds: timeout,
        deadline_ns_decimal: (start + u128::from(timeout) * 1_000_000_000).to_string(),
    }
}

fn helper_claim(_arm: &ArmV1, receipt: &ArmReceiptV1, helper_id: Uuid) -> HelperStartClaimV1 {
    HelperStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: helper_id.to_string(),
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: receipt.clone(),
        clock_check: host_clock(100_000_000_001),
    }
}

fn helper_observation(
    arm: &ArmV1,
    receipt: &ArmReceiptV1,
    helper_id: Uuid,
    running: Option<bool>,
    exit_code: i64,
    verification: Option<agentsandbox::control::ImportVerificationReportV1>,
) -> HelperObservationV1 {
    HelperObservationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: helper_id.to_string(),
        arm_receipt: receipt.clone(),
        observation: RuntimeObservationV1 {
            inspection: arm.inspection.clone(),
            observed_at: "2026-10-30T12:00:02Z".into(),
            status: if running == Some(true) {
                RuntimeStatusV1::Running
            } else {
                RuntimeStatusV1::Exited
            },
            running,
            started_at: Some("2026-10-30T12:00:00Z".into()),
            finished_at: Some("2026-10-30T12:00:01Z".into()),
            exit_code: Some(exit_code),
            oom_killed: Some(false),
            restart_count: Some(0),
        },
        verification,
    }
}

fn parent_observation(
    worker: &WorkerV1,
    status: RuntimeStatusV1,
    running: bool,
    finished_at: Option<&str>,
    exit_code: Option<i64>,
) -> RuntimeObservationV1 {
    RuntimeObservationV1 {
        inspection: worker.attempt.created_inspection.clone().unwrap(),
        observed_at: "2026-10-30T12:00:02Z".into(),
        status,
        running: Some(running),
        started_at: Some("2026-10-30T12:00:00Z".into()),
        finished_at: finished_at.map(str::to_owned),
        exit_code,
        oom_killed: Some(false),
        restart_count: Some(0),
    }
}

fn migrate(store: &Store, directory: &Path) -> agentsandbox::control::MigrationReportV1 {
    store
        .migrate_control_v3(&directory.join("migration-backup.sqlite"))
        .unwrap()
}

#[test]
fn schema2_default_and_explicit_migration_preserve_history_and_legacy_replay() {
    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let store = open(&path, 10_000);
    let before_state = store.supervision_state().unwrap();
    assert_eq!(before_state.schema_version, 2);
    assert!(!before_state.active);
    assert!(before_state.protocols.is_empty());

    let first_admission = decision_admission(&store);
    let second_admission = decision_admission(&store);
    let pending_admission = decision_admission(&store);
    let old_worker = create_legacy_worker(&store, &first_admission);
    let created_worker = create_legacy_worker(&store, &second_admission);
    let consumed = mutation(
        &old_worker,
        WorkerTransitionV1::StartClaim {
            emitter_id: Uuid::new_v4().to_string(),
        },
    );
    let first_claim = store.mutate_worker(&consumed).unwrap();
    assert_eq!(first_claim.disposition, Some(ClaimDispositionV1::EmitOnce));

    let before_integrity = store.integrity().unwrap();
    assert!(before_integrity.valid);
    let before_workers = store.workers().unwrap();
    let before_quota = store.worker_quota(&QuotaPolicyV1::default()).unwrap();
    let namespace = before_state.control_namespace_uuid.clone().unwrap();
    let migration = migrate(&store, directory.path());
    assert_eq!((migration.from_version, migration.to_version), (2, 3));
    assert!(Path::new(&migration.backup_path).exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&migration.backup_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0,
            "migration backup must remain private"
        );
    }

    let after_state = store.supervision_state().unwrap();
    assert!(
        !after_state.active,
        "historical migration must require maintenance"
    );
    assert_eq!(after_state.schema_version, 3);
    assert_eq!(
        after_state.control_namespace_uuid.as_deref(),
        Some(namespace.as_str())
    );
    assert_eq!(
        serde_json::to_value(store.workers().unwrap()).unwrap(),
        serde_json::to_value(before_workers).unwrap()
    );
    assert_eq!(
        store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        before_quota.held_workers
    );
    let migrated_integrity = store.integrity().unwrap();
    assert!(migrated_integrity.valid);
    assert_eq!(
        migrated_integrity.event_count,
        before_integrity.event_count + 1
    );

    let backup = open(Path::new(&migration.backup_path), 10_000);
    assert_eq!(backup.supervision_state().unwrap().schema_version, 2);
    let backup_integrity = backup.integrity().unwrap();
    assert!(backup_integrity.valid);
    assert_eq!(backup_integrity.event_count, before_integrity.event_count);
    assert_eq!(backup_integrity.head_hash, before_integrity.head_hash);
    drop(backup);
    drop(store);

    let store = open(&path, 10_000);
    assert!(store.healthy());
    assert_eq!(
        store
            .supervision_state()
            .unwrap()
            .control_namespace_uuid
            .as_deref(),
        Some(namespace.as_str())
    );
    assert!(store.integrity().unwrap().valid);
    let replay = store.mutate_worker(&consumed).unwrap();
    assert_eq!(replay.receipt, first_claim.receipt);
    assert_eq!(
        replay.disposition,
        Some(ClaimDispositionV1::AlreadyConsumed)
    );

    let before_refusal = store.head().unwrap();
    let policy = Policy::default();
    assert!(
        store
            .admit_worker(
                &pending_admission,
                &policy,
                policy::evaluate(&policy, &pending_admission.decision_request),
                json!(null),
                &QuotaPolicyV1::default(),
            )
            .is_err(),
        "fresh legacy admission must be closed after schema3 boundary"
    );
    assert_eq!(store.head().unwrap(), before_refusal);

    // Historical schema2 data remains maintenance-only after migration;
    // binding a legacy Created worker must not append a fresh permission.
    let intent = supervised_request(second_admission).intent_hash;
    let before_bind = store.head().unwrap();
    let bind_error = store
        .bind_legacy_intent(&BindLegacyIntentV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            worker_id: created_worker.worker_id.clone(),
            attempt_id: created_worker.attempt.attempt_id.clone(),
            expected_revision: created_worker.revision,
            intent_hash: intent.clone(),
        })
        .unwrap_err();
    assert_eq!(
        *bind_error.downcast_ref::<ControlFailure>().unwrap(),
        ControlFailure::SupervisionInactive
    );
    assert_eq!(store.head().unwrap(), before_bind);
    assert!(store.healthy());
}

#[test]
fn schema2_keeps_legacy_api_and_schema3_supervised_admission_is_atomic_and_idempotent() {
    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let store = open(&path, 10_000);
    let admission = decision_admission(&store);
    let request = supervised_request(admission.clone());
    let before_v2 = store.head().unwrap();
    assert!(
        store
            .admit_supervised(
                &request,
                &Policy::default(),
                policy::evaluate(&Policy::default(), &admission.decision_request),
                json!(null),
                &QuotaPolicyV1::default(),
            )
            .is_err(),
        "supervised admission must require explicit schema3 migration"
    );
    assert_eq!(store.head().unwrap(), before_v2);
    let legacy = admit_legacy(&store, &admission);
    assert!(legacy.accepted);
    assert_eq!(store.supervision_state().unwrap().schema_version, 2);

    let second_dir = support::private_tempdir();
    let second_path = second_dir.path().join("audit.sqlite");
    let second_store = open(&second_path, 10_000);
    migrate(&second_store, second_dir.path());
    let concurrent_admission = decision_admission(&second_store);
    let concurrent_request = supervised_request(concurrent_admission);
    let mut divergent = concurrent_request.clone();
    let compact = Uuid::new_v4().simple().to_string();
    divergent.intent_hash = format!("{compact}{compact}");
    let first = second_store.sibling().unwrap();
    let second = second_store.sibling().unwrap();
    let barrier = std::sync::Arc::new(Barrier::new(2));
    let first_barrier = barrier.clone();
    let first_request = concurrent_request.clone();
    let first_thread = thread::spawn(move || {
        first_barrier.wait();
        admit_supervised(&first, &first_request)
    });
    let second_barrier = barrier.clone();
    let second_thread = thread::spawn(move || {
        second_barrier.wait();
        admit_supervised(&second, &concurrent_request)
    });
    let first_result = first_thread.join().unwrap();
    let second_result = second_thread.join().unwrap();
    assert!(first_result.accepted && second_result.accepted);
    assert_eq!(
        first_result.worker.as_ref().unwrap().worker_id,
        second_result.worker.as_ref().unwrap().worker_id
    );
    assert_ne!(first_result.replayed, second_result.replayed);
    let committed_head = second_store.head().unwrap();
    // The durable operation belongs to the original admission. A different
    // decision/admission body with that ID must conflict without append.
    let stored = second_store.sibling().unwrap();
    assert!(
        stored
            .admit_supervised(
                &divergent,
                &Policy::default(),
                policy::evaluate(&Policy::default(), &divergent.admission.decision_request),
                json!(null),
                &QuotaPolicyV1::default(),
            )
            .is_err()
    );
    assert_eq!(stored.head().unwrap(), committed_head);
    let stored = second_store.sibling().unwrap();
    assert!(stored.integrity().unwrap().valid);
    assert_eq!(stored.workers().unwrap().len(), 1);
    assert_eq!(
        stored
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
}

#[test]
fn supervised_admission_requires_uuid_authority_while_legacy_v2_keeps_opaque_ids() {
    let legacy_directory = support::private_tempdir();
    let legacy = open(&legacy_directory.path().join("legacy.sqlite"), 10_000);
    let mut legacy_admission = decision_admission(&legacy);
    legacy_admission.launch.authority_id = "local-daemon".into();
    legacy_admission.validate().unwrap();
    assert!(admit_legacy(&legacy, &legacy_admission).accepted);

    let supervised_directory = support::private_tempdir();
    let supervised = open(
        &supervised_directory.path().join("supervised.sqlite"),
        10_000,
    );
    migrate(&supervised, supervised_directory.path());
    let mut supervised_admission = decision_admission(&supervised);
    supervised_admission.launch.authority_id = "local-daemon".into();
    supervised_admission.validate().unwrap();
    let request = supervised_request(supervised_admission.clone());
    let before = supervised.head().unwrap();
    let policy = Policy::default();
    let result = supervised.admit_supervised(
        &request,
        &policy,
        policy::evaluate(&policy, &supervised_admission.decision_request),
        json!(null),
        &QuotaPolicyV1::default(),
    );
    assert!(
        result.is_err(),
        "new supervised admission must reject non-UUID runtime authority before quota"
    );
    assert_eq!(supervised.head().unwrap(), before);
    assert!(supervised.workers().unwrap().is_empty());
    assert_eq!(
        supervised
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
}

fn run_import_gate(
    mismatch_manifest: bool,
    unknown_running: bool,
    second_verification_mismatch: Option<bool>,
    clone_before_claim: bool,
    pending_verification: bool,
) {
    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let store = open(&path, 10_000);
    migrate(&store, directory.path());
    let request = supervised_request(decision_admission(&store));
    let admitted = admit_supervised(&store, &request);
    assert!(admitted.accepted);
    let mut worker = admitted.worker.unwrap();
    assert_eq!(worker.state, WorkerStateV1::Reserved);
    assert_eq!(
        store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    worker = store
        .mutate_worker(&mutation(&worker, WorkerTransitionV1::Prepare))
        .unwrap()
        .worker;
    assert_eq!(worker.revision, 2);
    let before_create = store.head().unwrap();
    assert!(
        store
            .mutate_worker(&mutation(&worker, WorkerTransitionV1::CreateIssued))
            .is_err(),
        "worker create must wait for both import helpers"
    );
    assert_eq!(store.head().unwrap(), before_create);

    let import_id = Uuid::new_v4();
    let import_arm = helper_arm(
        &worker,
        &request.intent_hash,
        import_id,
        HelperPurposeV1::Import,
        HelperPermissionV1::Admission {
            receipt: admitted.receipt.clone(),
        },
    );
    let import_armed = store.arm_execution(&import_arm).unwrap();
    assert!(!import_armed.replayed);
    let import_claim = helper_claim(&import_arm, &import_armed.receipt, import_id);

    // A syntactically valid but foreign arm receipt must not create a helper
    // claim or return EmitOnce.
    let mut forged_claim = import_claim.clone();
    forged_claim.operation_id = Uuid::new_v4().to_string();
    forged_claim.arm_receipt.control_namespace_uuid = Uuid::new_v4().to_string();
    let forged_operation = forged_claim.operation_id.clone();
    assert!(store.claim_helper_start(&forged_claim).is_err());
    assert!(store.worker_operation(&forged_operation).unwrap().is_none());

    let imported = store.claim_helper_start(&import_claim).unwrap();
    assert_eq!(imported.disposition, ClaimDispositionV1::EmitOnce);
    assert_eq!(
        store.claim_helper_start(&import_claim).unwrap().disposition,
        ClaimDispositionV1::AlreadyConsumed
    );
    let before_cleanup = HelperCleanupV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: import_id.to_string(),
        arm_receipt: import_armed.receipt.clone(),
        removed: true,
        error: None,
    };
    assert!(store.record_helper_cleanup(&before_cleanup).is_err());

    let import_terminal = store
        .observe_helper(&helper_observation(
            &import_arm,
            &import_armed.receipt,
            import_id,
            if unknown_running { None } else { Some(false) },
            0,
            None,
        ))
        .unwrap();
    assert!(store.worker_usage().unwrap().is_empty());
    assert_eq!(
        store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );

    if !unknown_running {
        let mut conflicting_terminal = helper_observation(
            &import_arm,
            &import_armed.receipt,
            import_id,
            Some(false),
            1,
            None,
        );
        conflicting_terminal.observation.observed_at = "2026-10-30T12:00:03Z".into();
        let head = store.head().unwrap();
        assert!(store.observe_helper(&conflicting_terminal).is_err());
        assert_eq!(
            store.head().unwrap(),
            head,
            "terminal evidence must be sticky"
        );
    }

    let verify_id = Uuid::new_v4();
    let verify_arm = helper_arm(
        &worker,
        &request.intent_hash,
        verify_id,
        HelperPurposeV1::ImportVerification,
        HelperPermissionV1::ImportVerification {
            import_helper_id: import_id.to_string(),
            import_arm: import_armed.receipt.clone(),
            import_terminal: import_terminal.clone(),
            manifest_digest: worker.launch.workspace_manifest_digest.clone(),
        },
    );
    if unknown_running {
        assert!(store.arm_execution(&verify_arm).is_err());
        let head = store.head().unwrap();
        assert!(
            store
                .mutate_worker(&mutation(&worker, WorkerTransitionV1::CreateIssued))
                .is_err()
        );
        assert_eq!(store.head().unwrap(), head);
        assert_eq!(
            store.worker(&worker.worker_id).unwrap().unwrap().state,
            WorkerStateV1::Preparing
        );
        return;
    }

    let verify_armed = store.arm_execution(&verify_arm).unwrap();
    let verify_claim = helper_claim(&verify_arm, &verify_armed.receipt, verify_id);
    assert_eq!(
        store.claim_helper_start(&verify_claim).unwrap().disposition,
        ClaimDispositionV1::EmitOnce
    );
    if pending_verification {
        let pending_head = store.head().unwrap();
        assert!(
            store
                .mutate_worker(&mutation(&worker, WorkerTransitionV1::CreateIssued))
                .is_err()
        );
        assert_eq!(store.head().unwrap(), pending_head);
    }
    let observed_manifest = if mismatch_manifest { HASH_C } else { HASH_B };
    let verify_terminal = store
        .observe_helper(&helper_observation(
            &verify_arm,
            &verify_armed.receipt,
            verify_id,
            Some(false),
            0,
            Some(agentsandbox::control::ImportVerificationReportV1 {
                import_helper_id: import_id.to_string(),
                expected_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
                observed_manifest_digest: observed_manifest.into(),
            }),
        ))
        .unwrap();
    assert!(verify_terminal.audit_seq > import_terminal.audit_seq);

    if let Some(second_mismatch) = second_verification_mismatch {
        let verify_id_v2 = Uuid::new_v4();
        let verify_arm_v2 = helper_arm(
            &worker,
            &request.intent_hash,
            verify_id_v2,
            HelperPurposeV1::ImportVerification,
            HelperPermissionV1::ImportVerification {
                import_helper_id: import_id.to_string(),
                import_arm: import_armed.receipt.clone(),
                import_terminal: import_terminal.clone(),
                manifest_digest: worker.launch.workspace_manifest_digest.clone(),
            },
        );
        let verify_armed_v2 = store.arm_execution(&verify_arm_v2).unwrap();
        let verify_claim_v2 = helper_claim(&verify_arm_v2, &verify_armed_v2.receipt, verify_id_v2);
        assert_eq!(
            store
                .claim_helper_start(&verify_claim_v2)
                .unwrap()
                .disposition,
            ClaimDispositionV1::EmitOnce
        );
        store
            .observe_helper(&helper_observation(
                &verify_arm_v2,
                &verify_armed_v2.receipt,
                verify_id_v2,
                Some(false),
                0,
                Some(agentsandbox::control::ImportVerificationReportV1 {
                    import_helper_id: import_id.to_string(),
                    expected_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
                    observed_manifest_digest: if second_mismatch { HASH_C } else { HASH_B }.into(),
                }),
            ))
            .unwrap();
    }
    let helper_cleanup = HelperCleanupV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: import_id.to_string(),
        arm_receipt: import_armed.receipt.clone(),
        removed: true,
        error: None,
    };
    store.record_helper_cleanup(&helper_cleanup).unwrap();

    let before_gate = store.head().unwrap();
    let create = store.mutate_worker(&mutation(&worker, WorkerTransitionV1::CreateIssued));
    if mismatch_manifest || second_verification_mismatch == Some(true) {
        assert!(
            create.is_err(),
            "a failed import verification must block worker create even after an earlier success"
        );
        assert_eq!(store.head().unwrap(), before_gate);
        assert_eq!(
            store.worker(&worker.worker_id).unwrap().unwrap().state,
            WorkerStateV1::Preparing
        );
        assert!(store.worker_usage().unwrap().is_empty());
        drop(store);
        let reopened = open(&path, 10_000);
        assert!(!reopened.supervision_state().unwrap().active);
        let reopened_worker = reopened.worker(&worker.worker_id).unwrap().unwrap();
        let reopened_head = reopened.head().unwrap();
        assert!(
            reopened
                .mutate_worker(&mutation(
                    &reopened_worker,
                    WorkerTransitionV1::CreateIssued
                ))
                .is_err()
        );
        assert_eq!(reopened.head().unwrap(), reopened_head);
        return;
    }

    worker = create.unwrap().worker;
    assert_eq!(worker.state, WorkerStateV1::Preparing);
    let create_inspection = inspection(&worker);
    worker = store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::Created {
                inspection: create_inspection,
            },
        ))
        .unwrap()
        .worker;
    assert_eq!(worker.state, WorkerStateV1::Created);
    let worker_arm = worker_arm(&worker, &request.intent_hash);
    let worker_armed = store.arm_execution(&worker_arm).unwrap();
    let start_claim = ArmedStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: worker_armed.receipt.clone(),
        clock_check: host_clock(100_000_000_001),
    };
    let mut clone_store = None;
    if clone_before_claim {
        let clone_path = directory.path().join("pre-claim-clone.sqlite");
        {
            let source = Connection::open(&path).unwrap();
            #[cfg(unix)]
            let destination_file = {
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&clone_path)
                    .expect("create private clone destination independent of umask")
            };
            #[cfg(not(unix))]
            let destination_file = std::fs::File::create(&clone_path).unwrap();
            drop(destination_file);
            let mut destination = Connection::open(&clone_path).unwrap();
            let backup = Backup::new(&source, &mut destination).unwrap();
            backup
                .run_to_completion(256, Duration::from_millis(1), None)
                .unwrap();
        }
        clone_store = Some(open(&clone_path, 10_000));
        assert!(
            !clone_store
                .as_ref()
                .unwrap()
                .supervision_state()
                .unwrap()
                .active
        );
    }
    let legacy_claim = WorkerMutationV1 {
        version: 1,
        operation_id: start_claim.operation_id.clone(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition: WorkerTransitionV1::StartClaim {
            emitter_id: start_claim.emitter_id.clone(),
        },
    };
    let before_legacy = store.head().unwrap();
    assert!(
        store.mutate_worker(&legacy_claim).is_err(),
        "legacy start claim must be blocked even with a durable arm"
    );
    assert_eq!(store.head().unwrap(), before_legacy);
    assert!(
        store
            .worker_operation(&start_claim.operation_id)
            .unwrap()
            .is_none()
    );
    let started = store.claim_armed_start(&start_claim).unwrap();
    assert_eq!(started.disposition, Some(ClaimDispositionV1::EmitOnce));
    if let Some(clone_store) = clone_store {
        let clone_head = clone_store.head().unwrap();
        let clone_error = clone_store.claim_armed_start(&start_claim).unwrap_err();
        assert_eq!(
            *clone_error.downcast_ref::<ControlFailure>().unwrap(),
            ControlFailure::SupervisionInactive
        );
        assert_eq!(clone_store.head().unwrap(), clone_head);
        assert!(
            clone_store
                .worker_operation(&start_claim.operation_id)
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(
        store.claim_armed_start(&start_claim).unwrap().disposition,
        Some(ClaimDispositionV1::AlreadyConsumed),
        "same-process replay of original claim remains idempotent"
    );
    if clone_before_claim {
        let reopened = open(&path, 10_000);
        assert!(!reopened.supervision_state().unwrap().active);
        let mut fresh_claim = start_claim.clone();
        fresh_claim.operation_id = Uuid::new_v4().to_string();
        let reopened_head = reopened.head().unwrap();
        let reopened_error = reopened.claim_armed_start(&fresh_claim).unwrap_err();
        assert_eq!(
            *reopened_error.downcast_ref::<ControlFailure>().unwrap(),
            ControlFailure::SupervisionInactive
        );
        assert_eq!(reopened.head().unwrap(), reopened_head);
        assert!(
            reopened
                .worker_operation(&fresh_claim.operation_id)
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(
        store.claim_armed_start(&start_claim).unwrap().disposition,
        Some(ClaimDispositionV1::AlreadyConsumed)
    );
    worker = store
        .mutate_worker(&mutation(
            &started.worker,
            WorkerTransitionV1::Observe {
                observation: parent_observation(
                    &started.worker,
                    RuntimeStatusV1::Running,
                    true,
                    None,
                    None,
                ),
            },
        ))
        .unwrap()
        .worker;
    worker = store
        .mutate_worker(&mutation(&worker, WorkerTransitionV1::Stop))
        .unwrap()
        .worker;
    worker = store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::Observe {
                observation: parent_observation(
                    &worker,
                    RuntimeStatusV1::Exited,
                    false,
                    Some("2026-10-30T12:00:01Z"),
                    Some(0),
                ),
            },
        ))
        .unwrap()
        .worker;
    assert_eq!(worker.state, WorkerStateV1::Terminal);
    assert_eq!(
        store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
    let usage = store.worker_usage().unwrap();
    assert_eq!(usage.len(), 1);
    assert!(!usage[0].billable);

    let export = ExportRequestV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        destination_ref: Uuid::new_v4().to_string(),
        destination_intent_hash: HASH_B.into(),
    };
    let export_receipt = store.request_export(&export).unwrap();
    let before_rotation = store.head().unwrap();
    let mut rotated_destination = export.clone();
    rotated_destination.operation_id = Uuid::new_v4().to_string();
    rotated_destination.destination_intent_hash = HASH_C.into();
    assert!(store.request_export(&rotated_destination).is_err());
    assert_eq!(store.head().unwrap(), before_rotation);

    let export_helper_id = Uuid::new_v4();
    let forged_export_arm = helper_arm(
        &worker,
        &request.intent_hash,
        export_helper_id,
        HelperPurposeV1::Export,
        HelperPermissionV1::Export {
            receipt: export_receipt.clone(),
            destination_ref: export.destination_ref.clone(),
            destination_intent_hash: HASH_C.into(),
        },
    );
    assert!(store.arm_execution(&forged_export_arm).is_err());
    let export_arm = helper_arm(
        &worker,
        &request.intent_hash,
        export_helper_id,
        HelperPurposeV1::Export,
        HelperPermissionV1::Export {
            receipt: export_receipt,
            destination_ref: export.destination_ref.clone(),
            destination_intent_hash: export.destination_intent_hash.clone(),
        },
    );
    let export_armed = store.arm_execution(&export_arm).unwrap();
    let export_claim = helper_claim(&export_arm, &export_armed.receipt, export_helper_id);
    assert_eq!(
        store.claim_helper_start(&export_claim).unwrap().disposition,
        ClaimDispositionV1::EmitOnce
    );
    assert_eq!(
        store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
    assert_eq!(
        store.worker_usage().unwrap().len(),
        1,
        "helpers do not add parent usage"
    );
    assert!(store.integrity().unwrap().valid);
}

#[test]
fn import_verification_success_mismatch_and_unknown_terminal_gate_create() {
    run_import_gate(false, false, None, false, false);
    run_import_gate(true, false, None, false, false);
    run_import_gate(false, true, None, false, false);
    run_import_gate(false, false, Some(true), false, false);
    run_import_gate(true, false, Some(false), false, false);
    run_import_gate(false, false, None, false, true);
}

#[test]
fn pre_claim_sqlite_clone_cannot_reauthorize_armed_start() {
    run_import_gate(false, false, None, true, false);
}

#[tokio::test]
async fn reopened_historical_schema3_http_mutation_returns_typed_maintenance_503() {
    use tower::ServiceExt;

    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
    let legacy_admission = decision_admission(&store);
    let worker = create_legacy_worker(&store, &legacy_admission);
    migrate(&store, directory.path());
    drop(store);

    let reopened =
        Arc::new(Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap());
    assert!(!reopened.supervision_state().unwrap().active);
    let before = reopened.head().unwrap();
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "tenant-a".into(),
            store: reopened.clone(),
            runner_token: "r".repeat(48),
            viewer_token: "v".repeat(48),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let mutation = mutation(
        &worker,
        WorkerTransitionV1::StartClaim {
            emitter_id: Uuid::new_v4().to_string(),
        },
    );
    let operation_id = mutation.operation_id.clone();
    let response = gateway_router(state)
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("/api/v1/workers/{}/transitions", worker.worker_id))
                .header("authorization", format!("Bearer {}", "r".repeat(48)))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_vec(&mutation).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["code"], "supervision_inactive");
    assert_eq!(reopened.head().unwrap(), before);
    assert!(reopened.worker_operation(&operation_id).unwrap().is_none());
    assert_eq!(
        reopened.worker(&worker.worker_id).unwrap().unwrap().state,
        WorkerStateV1::Created
    );
}

#[test]
fn live_watermark_rejects_same_process_rollback_for_all_siblings() {
    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let rollback_path = directory.path().join("before-decisions.sqlite");
    let store = open(&path, 10_000);
    migrate(&store, directory.path());
    let sibling = store.sibling().unwrap();
    let genesis_head = store.head().unwrap();
    let genesis_hash: String = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT hash FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    #[cfg(unix)]
    let original_identity = {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(&path).unwrap();
        (metadata.dev(), metadata.ino())
    };
    {
        let source = Connection::open(&path).unwrap();
        let mut destination = Connection::open(&rollback_path).unwrap();
        let backup = Backup::new(&source, &mut destination).unwrap();
        backup
            .run_to_completion(256, Duration::from_millis(1), None)
            .unwrap();
    }

    let barrier = Arc::new(Barrier::new(2));
    let first_barrier = barrier.clone();
    let first = store.sibling().unwrap();
    let first_thread = thread::spawn(move || {
        first_barrier.wait();
        decision_admission(&first);
    });
    let second_barrier = barrier.clone();
    let second = sibling.sibling().unwrap();
    let second_thread = thread::spawn(move || {
        second_barrier.wait();
        decision_admission(&second);
    });
    first_thread.join().unwrap();
    second_thread.join().unwrap();
    assert!(store.head().unwrap() > genesis_head);

    // Restore a valid earlier SQLite image into the same backing file while
    // the original Stores remain alive. The shared in-memory floor must detect
    // this rollback and invalidate every sibling before another operation.
    {
        let source = Connection::open(&rollback_path).unwrap();
        let mut destination = Connection::open(&path).unwrap();
        let backup = Backup::new(&source, &mut destination).unwrap();
        backup
            .run_to_completion(256, Duration::from_millis(1), None)
            .unwrap();
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!((metadata.dev(), metadata.ino()), original_identity);
    }
    let raw = Connection::open(&path).unwrap();
    let restored_head: (i64, String) = raw
        .query_row(
            "SELECT seq,hash FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(restored_head, (genesis_head, genesis_hash));
    drop(raw);

    let rollback_error = store.supervision_state().unwrap_err();
    assert!(
        rollback_error
            .to_string()
            .contains("audit rolled back below the live supervision witness")
    );
    assert!(sibling.supervision_state().is_err());
    assert!(!store.healthy());
    assert!(!sibling.healthy());
}

#[test]
fn audit_capacity_failure_does_not_persist_an_arm_or_permission() {
    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let store = open(&path, 5);
    migrate(&store, directory.path());
    let request = supervised_request(decision_admission(&store));
    let admitted = admit_supervised(&store, &request);
    let worker = store
        .mutate_worker(&mutation(
            &admitted.worker.unwrap(),
            WorkerTransitionV1::Prepare,
        ))
        .unwrap()
        .worker;
    let helper_id = Uuid::new_v4();
    let arm = helper_arm(
        &worker,
        &request.intent_hash,
        helper_id,
        HelperPurposeV1::Import,
        HelperPermissionV1::Admission {
            receipt: admitted.receipt,
        },
    );
    let head = store.head().unwrap();
    assert!(
        store.arm_execution(&arm).is_err(),
        "audit full must refuse the arm transaction"
    );
    assert_eq!(store.head().unwrap(), head);
    assert!(store.execution_arm(&arm.operation_id).unwrap().is_none());
    assert!(store.worker_operation(&arm.operation_id).unwrap().is_none());
    assert_eq!(
        store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    assert!(store.healthy());
    assert!(store.integrity().unwrap().valid);
}

#[test]
fn projection_corruption_latches_v3_store_unhealthy() {
    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let store = open(&path, 10_000);
    migrate(&store, directory.path());
    let request = supervised_request(decision_admission(&store));
    let admitted = admit_supervised(&store, &request);
    let worker_id = admitted.worker.unwrap().worker_id;
    let raw = Connection::open(&path).unwrap();
    raw.execute(
        "UPDATE worker_supervision SET intent_hash=?1 WHERE worker_id=?2",
        params![HASH_C, worker_id],
    )
    .unwrap();
    drop(raw);
    assert!(store.worker(&worker_id).is_err());
    assert!(
        !store.healthy(),
        "projection divergence must latch the Store"
    );
    assert!(
        store
            .admit_supervised(
                &request,
                &Policy::default(),
                policy::evaluate(&Policy::default(), &request.admission.decision_request),
                json!(null),
                &QuotaPolicyV1::default(),
            )
            .is_err(),
        "an unhealthy projection must not grant replayed admission"
    );
}
