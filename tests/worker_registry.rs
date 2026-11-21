mod support;

use agentsandbox::{audit::Store, redaction::Redactor};
use agentsandbox::{
    control::*,
    model::*,
    policy::{self, Policy},
};
use serde_json::json;

#[test]
fn fixed_sandbox_v2_is_a_distinct_closed_contract() {
    let legacy = SecurityPresetV1::FixedSandboxV1;
    assert_eq!(
        serde_json::to_string(&legacy).unwrap(),
        "\"fixed_sandbox_v1\""
    );
    let current: SecurityPresetV1 = serde_json::from_str("\"fixed_sandbox_v2\"").unwrap();
    assert_ne!(current, legacy);
    assert_eq!(
        serde_json::to_string(&current).unwrap(),
        "\"fixed_sandbox_v2\""
    );
    assert!(!legacy.requires_no_engine_logs());
    assert!(current.requires_no_engine_logs());
    assert_eq!(legacy.configuration_fingerprint_version(), 1);
    assert_eq!(current.configuration_fingerprint_version(), 2);
    for unknown in [
        "fixed_sandbox_v0",
        "fixed_sandbox_v3",
        "fixed_sandbox",
        "none",
    ] {
        assert!(serde_json::from_value::<SecurityPresetV1>(json!(unknown)).is_err());
    }
}

#[test]
fn fixed_sandbox_v1_admission_bytes_and_hashes_are_historical_goldens() {
    let golden = include_str!("fixtures/control/admission-fixed-sandbox-v1.json").trim();
    let admission: AdmissionV1 = serde_json::from_str(golden).unwrap();
    admission.validate().unwrap();
    assert_eq!(serde_json::to_string(&admission).unwrap(), golden);
    assert_eq!(
        admission.receipt.request_hash,
        "f2b49355b13327290c05b7856040057dff6b15a7a73844c55d2028cab86793aa"
    );
    assert_eq!(
        admission.launch.spec_hash().unwrap(),
        "fc12ab2b6757338a1c8a1c01a8cc4abaa67d8f90d5f50a1679d81088a18995fa"
    );
    assert_eq!(
        admission.input_hash().unwrap(),
        "5104ce0ad4918f3271e04e8a85ea564dc3a99fcf79fe82ea823c7fd02e4ba68c"
    );
}

#[test]
fn fixed_sandbox_v2_changes_only_the_preset_in_the_version_one_admission() {
    let legacy = include_str!("fixtures/control/admission-fixed-sandbox-v1.json").trim();
    let current = legacy.replace("fixed_sandbox_v1", "fixed_sandbox_v2");
    let admission: AdmissionV1 = serde_json::from_str(&current).unwrap();
    admission.validate().unwrap();
    assert_eq!(serde_json::to_string(&admission).unwrap(), current);
    assert_eq!(admission.version, 1);
    assert_eq!(admission.launch.version, 1);
    assert_eq!(
        admission.launch.spec_hash().unwrap(),
        "10677993c133ad52c34a97e18e09c8f740db63702687063f819c00e70597fc06"
    );
    assert_eq!(
        admission.input_hash().unwrap(),
        "9f7afae6f8dec5e98915b8f007488b554f436c24eb8bebcfa0f363c428a750c0"
    );
    for unknown in ["fixed_sandbox_v0", "fixed_sandbox_v3"] {
        assert!(
            serde_json::from_str::<AdmissionV1>(&legacy.replace("fixed_sandbox_v1", unknown))
                .is_err()
        );
    }
    let mut unsupported = admission;
    unsupported.launch.version = 2;
    assert!(unsupported.validate().is_err());
    unsupported.launch.version = 1;
    unsupported.version = 2;
    assert!(unsupported.validate().is_err());
}

#[test]
fn interactive_admission_binds_mode_and_refuses_batch_receipts_capture_and_term_override() {
    use agentsandbox::model::ExecutionModeV1;
    let legacy = include_str!("fixtures/control/admission-fixed-sandbox-v1.json").trim();
    let mut admission: AdmissionV1 = serde_json::from_str(legacy).unwrap();
    let batch_hash = admission.receipt.request_hash.clone();
    admission.launch.security_preset = SecurityPresetV1::FixedSandboxInteractiveV3;
    assert!(
        admission.validate().is_err(),
        "batch decision cannot authorize a TTY"
    );
    admission.decision_request.execution_mode = Some(ExecutionModeV1::InteractiveTtyV1);
    assert!(
        admission.validate().is_err(),
        "batch receipt cannot authorize a TTY"
    );
    admission.receipt.request_hash =
        agentsandbox::policy::request_hash(&admission.decision_request);
    assert_ne!(admission.receipt.request_hash, batch_hash);
    admission.launch.capture_output = false;
    admission.validate().unwrap();
    assert_eq!(
        admission
            .launch
            .security_preset
            .configuration_fingerprint_version(),
        3
    );
    assert!(
        serde_json::to_string(&admission)
            .unwrap()
            .contains("fixed_sandbox_interactive_v3")
    );
    let mut invalid = admission.clone();
    invalid.launch.capture_output = true;
    assert!(invalid.validate().is_err());
    let mut invalid = admission.clone();
    invalid.launch.security_preset = SecurityPresetV1::FixedSandboxV2;
    assert!(
        invalid.validate().is_err(),
        "TTY decision cannot authorize a batch profile"
    );
    let mut invalid = admission;
    invalid.launch.credential_binding_id = Some(uuid::Uuid::new_v4().to_string());
    invalid.launch.credential_names = vec!["TERM".into()];
    assert!(invalid.validate().is_err(), "TERM is fixed by the preset");
    let mut unknown: serde_json::Value = serde_json::from_str(legacy).unwrap();
    unknown["decision_request"]["execution_mode"] = json!("interactive_tty_v2");
    assert!(serde_json::from_value::<AdmissionV1>(unknown).is_err());
}

fn admission(store: &Store) -> AdmissionV1 {
    let session = uuid::Uuid::new_v4().to_string();
    let request = DecisionRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        profile: "strict".into(),
        command: CommandSpec {
            argv: vec!["/usr/bin/id".into()],
            cwd: "/workspace".into(),
        },
        project_id: None,
        sandbox_id: None,
        agent_id: None,
        session_id: Some(session.clone()),
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    };
    let stored = store
        .decision(
            &request,
            Some((policy::evaluate(&Policy::default(), &request), json!(null))),
        )
        .unwrap()
        .unwrap();
    AdmissionV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
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
            session_id: session,
            command: request.command.clone(),
            profile: "strict".into(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: "local-daemon".into(),
            image_ref: "worker:test".into(),
            image_id: format!("sha256:{}", "a".repeat(64)),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1073741824,
            pids_limit: 256,
            timeout_seconds: 60,
            security_preset: SecurityPresetV1::FixedSandboxV1,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: "b".repeat(64),
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
            egress: None,
        },
        decision_request: request,
    }
}
fn admit(store: &Store, request: &AdmissionV1, quota: &QuotaPolicyV1) -> AdmissionResultV1 {
    store
        .admit_worker(
            request,
            &Policy::default(),
            policy::evaluate(&Policy::default(), &request.decision_request),
            json!(null),
            quota,
        )
        .unwrap()
}

#[test]
fn reserved_sqlite_trigger_names_cannot_bypass_schema_validation_or_quota() {
    let files = support::private_tempdir();
    let path = files.path().join("audit.sqlite");
    let store = Store::open_for_tenant(&path, "a", Redactor::new(vec![]), 1000).unwrap();
    let quota = QuotaPolicyV1 {
        max_active_workers: 1,
        ..Default::default()
    };
    let first = admission(&store);
    let worker = admit(&store, &first, &quota).worker.unwrap();
    let attack = rusqlite::Connection::open(&path).unwrap();
    attack.execute_batch("PRAGMA writable_schema=ON; CREATE TRIGGER sqlite_quota AFTER UPDATE ON workers BEGIN UPDATE workers SET quota_held=0 WHERE worker_id=NEW.worker_id; END;").unwrap();
    drop(attack);
    let mutation = mutation(&worker, WorkerTransitionV1::Prepare);
    let result = store.mutate_worker(&mutation);
    assert!(
        result.is_err(),
        "an unexpected reserved-name trigger was accepted before a control transition"
    );
}

#[test]
fn reserved_sqlite_triggers_are_refused_at_startup() {
    let files = support::private_tempdir();
    let path = files.path().join("audit.sqlite");
    drop(Store::open_for_tenant(&path, "a", Redactor::new(vec![]), 1000).unwrap());
    let attack = rusqlite::Connection::open(&path).unwrap();
    attack.execute_batch("PRAGMA writable_schema=ON; CREATE TRIGGER sqlite_quota AFTER UPDATE ON workers BEGIN UPDATE workers SET quota_held=0 WHERE worker_id=NEW.worker_id; END;").unwrap();
    drop(attack);
    assert!(
        Store::open_for_tenant(&path, "a", Redactor::new(vec![]), 1000).is_err(),
        "reserved-name triggers must not be treated as SQLite autoindexes"
    );
}

#[test]
fn reserved_sqlite_trigger_cannot_reissue_start_permission() {
    let directory = support::private_tempdir();
    let path = directory.path().join("db");
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let admission = admission(&store);
    let worker = created(
        &store,
        admit(&store, &admission, &QuotaPolicyV1::default())
            .worker
            .unwrap(),
    );
    let snapshot = serde_json::to_string(&worker).unwrap().replace('\'', "''");
    let attack = rusqlite::Connection::open(&path).unwrap();
    attack.execute_batch(&format!("PRAGMA writable_schema=ON; CREATE TRIGGER sqlite_rearm AFTER INSERT ON worker_mutations WHEN NEW.action='start_claim' BEGIN UPDATE workers SET state='created',revision={},snapshot_json='{}' WHERE worker_id='{}'; END;",worker.revision,snapshot,worker.worker_id)).unwrap();
    drop(attack);
    let first = store.mutate_worker(&mutation(
        &worker,
        WorkerTransitionV1::StartClaim {
            emitter_id: uuid::Uuid::new_v4().to_string(),
        },
    ));
    match first {
        Err(_) => assert!(
            !store.healthy(),
            "unexpected schema must latch before a claim"
        ),
        Ok(first) => {
            assert_eq!(first.disposition, Some(ClaimDispositionV1::EmitOnce));
            let restored = store.worker(&worker.worker_id).unwrap().unwrap();
            let second = store.mutate_worker(&mutation(
                &restored,
                WorkerTransitionV1::StartClaim {
                    emitter_id: uuid::Uuid::new_v4().to_string(),
                },
            ));
            assert!(
                !second
                    .is_ok_and(|second| second.disposition == Some(ClaimDispositionV1::EmitOnce)),
                "a trigger reset allowed a second fresh EmitOnce for the same attempt"
            );
        }
    }
}

#[test]
fn reserved_sqlite_trigger_cannot_admit_over_quota() {
    let directory = support::private_tempdir();
    let path = directory.path().join("db");
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let first = admission(&store);
    let second = admission(&store);
    let quota = QuotaPolicyV1 {
        max_active_workers: 1,
        ..Default::default()
    };
    let worker = admit(&store, &first, &quota).worker.unwrap();
    let attack = rusqlite::Connection::open(&path).unwrap();
    attack.execute_batch("PRAGMA writable_schema=ON; CREATE TRIGGER sqlite_quota AFTER UPDATE ON workers BEGIN UPDATE workers SET quota_held=0 WHERE worker_id=NEW.worker_id; END;").unwrap();
    drop(attack);
    let _ = store.mutate_worker(&mutation(&worker, WorkerTransitionV1::Prepare));
    let over = store.admit_worker(
        &second,
        &Policy::default(),
        policy::evaluate(&Policy::default(), &second.decision_request),
        json!(null),
        &quota,
    );
    assert!(
        !over.is_ok_and(|result| result.accepted),
        "a reserved-name trigger released the first slot and admitted a second worker under quota one"
    );
}
fn mutation(worker: &WorkerV1, transition: WorkerTransitionV1) -> WorkerMutationV1 {
    WorkerMutationV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition,
    }
}

#[test]
fn reserved_sqlite_tables_views_and_explicit_indexes_are_also_refused() {
    for ddl in [
        "CREATE TABLE sqlite_extra(value TEXT)",
        "CREATE VIEW sqlite_extra AS SELECT 1 AS value",
        "CREATE INDEX sqlite_extra ON workers(state)",
        "CREATE TRIGGER sqlite_autoindex_events_1 AFTER UPDATE ON workers BEGIN SELECT 1; END",
    ] {
        let directory = support::private_tempdir();
        let path = directory.path().join("db");
        drop(Store::open(&path, Redactor::new(vec![]), 1000).unwrap());
        let attack = rusqlite::Connection::open(&path).unwrap();
        attack
            .execute_batch(&format!("PRAGMA writable_schema=ON; {ddl}"))
            .unwrap();
        drop(attack);
        assert!(
            Store::open(&path, Redactor::new(vec![]), 1000).is_err(),
            "unexpected object accepted: {ddl}"
        );
    }
}
fn inspection(worker: &WorkerV1) -> RuntimeInspectionV1 {
    RuntimeInspectionV1 {
        authority_id: worker.launch.authority_id.clone(),
        runtime_kind: worker.launch.runtime_kind,
        container_id: "c".repeat(64),
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
fn created(store: &Store, mut worker: WorkerV1) -> WorkerV1 {
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

#[test]
fn mixed_security_presets_replay_without_rewriting_identity_or_granting_new_claims() {
    let dir = support::private_tempdir();
    let path = dir.path().join("db");
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let quota = QuotaPolicyV1::default();
    let legacy = admission(&store);
    let mut current = admission(&store);
    current.launch.security_preset = SecurityPresetV1::FixedSandboxV2;
    let first = admit(&store, &legacy, &quota);
    let second = admit(&store, &current, &quota);
    let legacy_worker = created(&store, first.worker.clone().unwrap());
    let claim = mutation(
        &legacy_worker,
        WorkerTransitionV1::StartClaim {
            emitter_id: uuid::Uuid::new_v4().to_string(),
        },
    );
    let claimed = store.mutate_worker(&claim).unwrap();
    assert_eq!(claimed.disposition, Some(ClaimDispositionV1::EmitOnce));
    let current_worker = created(&store, second.worker.clone().unwrap());
    assert_eq!(
        current_worker
            .attempt
            .created_inspection
            .as_ref()
            .unwrap()
            .security_preset,
        current.launch.security_preset
    );
    let before = serde_json::to_value(store.workers().unwrap()).unwrap();
    let head = store.head().unwrap();
    drop(store);

    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    assert!(store.healthy());
    assert_eq!(
        serde_json::to_value(store.workers().unwrap()).unwrap(),
        before
    );
    assert_eq!(admit(&store, &legacy, &quota).receipt, first.receipt);
    assert_eq!(admit(&store, &current, &quota).receipt, second.receipt);
    assert_eq!(
        store.mutate_worker(&claim).unwrap().disposition,
        Some(ClaimDispositionV1::AlreadyConsumed)
    );
    let mut replacement = legacy.clone();
    replacement.launch.security_preset = current.launch.security_preset;
    for operation in [
        legacy.operation_id.clone(),
        uuid::Uuid::new_v4().to_string(),
    ] {
        replacement.operation_id = operation;
        assert_eq!(
            store
                .admit_worker(
                    &replacement,
                    &Policy::default(),
                    policy::evaluate(&Policy::default(), &replacement.decision_request),
                    json!(null),
                    &quota,
                )
                .unwrap_err()
                .downcast_ref::<ControlFailure>(),
            Some(&ControlFailure::Conflict)
        );
    }
    assert_eq!(store.head().unwrap(), head);
    assert_eq!(
        serde_json::to_value(store.workers().unwrap()).unwrap(),
        before
    );

    let worker = store
        .mutate_worker(&mutation(&claimed.worker, WorkerTransitionV1::Stop))
        .unwrap()
        .worker;
    let terminal = mutation(
        &worker,
        WorkerTransitionV1::Observe {
            observation: RuntimeObservationV1 {
                inspection: inspection(&worker),
                observed_at: "2026-10-29T10:00:02Z".into(),
                status: RuntimeStatusV1::Exited,
                running: Some(false),
                started_at: Some("2026-10-29T10:00:00Z".into()),
                finished_at: Some("2026-10-29T10:00:01Z".into()),
                exit_code: Some(0),
                oom_killed: Some(false),
                restart_count: Some(0),
            },
        },
    );
    let closed = store.mutate_worker(&terminal).unwrap();
    assert_eq!(closed.worker.state, WorkerStateV1::Terminal);
    assert_eq!(
        closed.worker.launch.security_preset,
        SecurityPresetV1::FixedSandboxV1
    );
    assert_eq!(store.worker_quota(&quota).unwrap().held_workers, 1);
    assert_eq!(store.worker_usage().unwrap().len(), 1);
    assert!(!store.worker_usage().unwrap()[0].billable);
    drop(store);
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    assert!(store.healthy());
    assert_eq!(
        store.mutate_worker(&terminal).unwrap().receipt,
        closed.receipt
    );
    assert!(store.integrity().unwrap().valid);
}

#[test]
fn indoubt_before_create_resumes_preparation_without_a_new_attempt_or_reservation() {
    for preparing in [false, true] {
        let dir = support::private_tempdir();
        let path = dir.path().join("db");
        let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
        let request = admission(&store);
        let mut worker = admit(&store, &request, &QuotaPolicyV1::default())
            .worker
            .unwrap();
        if preparing {
            worker = store
                .mutate_worker(&mutation(&worker, WorkerTransitionV1::Prepare))
                .unwrap()
                .worker;
        }
        let stale = mutation(&worker, WorkerTransitionV1::Prepare);
        let unknown = store
            .mutate_worker(&mutation(
                &worker,
                WorkerTransitionV1::Unknown {
                    reason: "control interrupted before create claim".into(),
                },
            ))
            .unwrap()
            .worker;
        drop(store);
        let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
        assert_eq!(
            store.mutate_worker(&stale).unwrap_err().downcast_ref(),
            Some(&ControlFailure::Conflict)
        );
        let prepare = mutation(&unknown, WorkerTransitionV1::Prepare);
        let result = store.mutate_worker(&prepare).unwrap();
        assert_eq!(result.worker.state, WorkerStateV1::Preparing);
        assert!(result.worker.quota_held);
        assert!(result.worker.unknown_reason.is_none());
        assert_eq!(result.worker.attempt, worker.attempt);
        assert_eq!(result.worker.spec_hash, worker.spec_hash);
        assert_eq!(result.worker.launch.spec_hash().unwrap(), worker.spec_hash);
        assert_eq!(result.worker.revision, unknown.revision + 1);
        assert_eq!(
            store.mutate_worker(&prepare).unwrap().receipt,
            result.receipt
        );
        assert_eq!(
            store
                .worker_quota(&QuotaPolicyV1::default())
                .unwrap()
                .held_workers,
            1
        );
        assert!(store.worker_usage().unwrap().is_empty());
        drop(store);
        let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
        assert!(store.integrity().unwrap().valid);
        assert_eq!(
            store.worker(&worker.worker_id).unwrap().unwrap().state,
            WorkerStateV1::Preparing
        );
    }
}

#[test]
fn indoubt_after_create_claim_records_the_found_container_and_preserves_claims() {
    let dir = support::private_tempdir();
    let path = dir.path().join("db");
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let request = admission(&store);
    let worker = admit(&store, &request, &QuotaPolicyV1::default())
        .worker
        .unwrap();
    let worker = store
        .mutate_worker(&mutation(&worker, WorkerTransitionV1::Prepare))
        .unwrap()
        .worker;
    let issued = mutation(&worker, WorkerTransitionV1::CreateIssued);
    let worker = store.mutate_worker(&issued).unwrap().worker;
    let stale = mutation(
        &worker,
        WorkerTransitionV1::Created {
            inspection: inspection(&worker),
        },
    );
    let worker = store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::Unknown {
                reason: "create acknowledgement lost".into(),
            },
        ))
        .unwrap()
        .worker;
    drop(store);
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(
        store.mutate_worker(&stale).unwrap_err().downcast_ref(),
        Some(&ControlFailure::Conflict)
    );
    assert_eq!(
        store
            .mutate_worker(&mutation(&worker, WorkerTransitionV1::Prepare))
            .unwrap_err()
            .downcast_ref(),
        Some(&ControlFailure::Conflict)
    );
    assert_eq!(
        store
            .mutate_worker(&mutation(&worker, WorkerTransitionV1::CreateIssued))
            .unwrap_err()
            .downcast_ref(),
        Some(&ControlFailure::Conflict)
    );
    for defect in [
        "authority",
        "runtime",
        "name",
        "volume",
        "image",
        "spec",
        "cpu",
        "memory",
        "pids",
        "network",
        "container",
    ] {
        let mut invalid = inspection(&worker);
        match defect {
            "authority" => invalid.authority_id = "other-daemon".into(),
            "runtime" => invalid.runtime_kind = RuntimeKindV1::Podman,
            "name" => invalid.stable_name.push_str("-other"),
            "volume" => invalid.volume_name.push_str("-other"),
            "image" => invalid.image_id = format!("sha256:{}", "d".repeat(64)),
            "spec" => invalid.spec_hash = "d".repeat(64),
            "cpu" => invalid.cpu_millis /= 2,
            "memory" => invalid.memory_bytes /= 2,
            "pids" => invalid.pids_limit /= 2,
            "network" => invalid.network = NetworkV1::Bridge,
            _ => invalid.container_id.clear(),
        }
        assert!(
            store
                .mutate_worker(&mutation(
                    &worker,
                    WorkerTransitionV1::Created {
                        inspection: invalid
                    }
                ))
                .is_err(),
            "{defect}"
        );
    }
    let recovered = mutation(
        &worker,
        WorkerTransitionV1::Created {
            inspection: inspection(&worker),
        },
    );
    let result = store.mutate_worker(&recovered).unwrap();
    assert_eq!(result.worker.state, WorkerStateV1::Created);
    assert!(result.worker.quota_held);
    assert!(result.worker.unknown_reason.is_none());
    assert_eq!(result.worker.attempt.attempt_id, worker.attempt.attempt_id);
    assert_eq!(
        result.worker.attempt.create_operation_id.as_deref(),
        Some(issued.operation_id.as_str())
    );
    assert_eq!(
        result.worker.attempt.created_inspection,
        Some(inspection(&worker))
    );
    assert!(result.worker.attempt.start_operation_id.is_none());
    assert_eq!(
        store.mutate_worker(&recovered).unwrap().receipt,
        result.receipt
    );
    let claim = mutation(
        &result.worker,
        WorkerTransitionV1::StartClaim {
            emitter_id: uuid::Uuid::new_v4().to_string(),
        },
    );
    assert_eq!(
        store.mutate_worker(&claim).unwrap().disposition,
        Some(ClaimDispositionV1::EmitOnce)
    );
    drop(store);
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(
        store.mutate_worker(&claim).unwrap().disposition,
        Some(ClaimDispositionV1::AlreadyConsumed)
    );
    assert!(store.integrity().unwrap().valid);
    assert!(store.worker_usage().unwrap().is_empty());
}

#[test]
fn indoubt_recovery_cannot_rewind_an_existing_container_or_start_claim() {
    for start_claimed in [false, true] {
        let dir = support::private_tempdir();
        let path = dir.path().join("db");
        let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
        let request = admission(&store);
        let mut worker = created(
            &store,
            admit(&store, &request, &QuotaPolicyV1::default())
                .worker
                .unwrap(),
        );
        if start_claimed {
            worker = store
                .mutate_worker(&mutation(
                    &worker,
                    WorkerTransitionV1::StartClaim {
                        emitter_id: uuid::Uuid::new_v4().to_string(),
                    },
                ))
                .unwrap()
                .worker;
        }
        let worker = store
            .mutate_worker(&mutation(
                &worker,
                WorkerTransitionV1::Unknown {
                    reason: "runtime unavailable".into(),
                },
            ))
            .unwrap()
            .worker;
        let head = store.head().unwrap();
        for transition in [
            WorkerTransitionV1::Prepare,
            WorkerTransitionV1::CreateIssued,
            WorkerTransitionV1::Created {
                inspection: inspection(&worker),
            },
            WorkerTransitionV1::StartClaim {
                emitter_id: uuid::Uuid::new_v4().to_string(),
            },
        ] {
            assert_eq!(
                store
                    .mutate_worker(&mutation(&worker, transition))
                    .unwrap_err()
                    .downcast_ref(),
                Some(&ControlFailure::Conflict)
            );
        }
        assert_eq!(store.head().unwrap(), head);
        assert_eq!(
            store.worker(&worker.worker_id).unwrap().unwrap().attempt,
            worker.attempt
        );
        assert_eq!(
            store
                .worker_quota(&QuotaPolicyV1::default())
                .unwrap()
                .held_workers,
            1
        );
        assert!(store.worker_usage().unwrap().is_empty());
    }
}

#[test]
fn projection_replay_rejects_prepare_and_created_records_without_their_exact_transition() {
    for action in ["prepare", "created"] {
        let dir = support::private_tempdir();
        let path = dir.path().join("db");
        let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
        let request = admission(&store);
        let worker = admit(&store, &request, &QuotaPolicyV1::default())
            .worker
            .unwrap();
        store
            .mutate_worker(&mutation(
                &worker,
                WorkerTransitionV1::Unknown {
                    reason: "interrupted".into(),
                },
            ))
            .unwrap();
        drop(store);
        rewrite_last_control_record(&path, |record| record["action"] = json!(action));
        let reopened = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
        assert!(
            !reopened.healthy(),
            "replay accepted a {action} record whose worker remains InDoubt"
        );
        assert!(reopened.workers().is_err());
    }
}

// Simulate an attacker with direct database write access who can recompute all
// unkeyed hashes and projections. This is not an API or worker capability.
fn rewrite_last_control_record(
    path: &std::path::Path,
    rewrite: impl FnOnce(&mut serde_json::Value),
) {
    use sha2::{Digest, Sha256};
    let db = rusqlite::Connection::open(path).unwrap();
    let (seq, received, previous, payload): (i64, String, String, String) = db
        .query_row(
            "SELECT seq,received_at,prev_hash,payload FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let mut event: serde_json::Value = serde_json::from_str(&payload).unwrap();
    rewrite(&mut event["data"]);
    let record = &mut event["data"];
    if !record["worker"].is_null() {
        let worker: WorkerV1 = serde_json::from_value(record["worker"].clone()).unwrap();
        let snapshot_hash = content_hash("ai-guard:worker-snapshot:v1", &worker).unwrap();
        record["snapshot_hash"] = json!(snapshot_hash);
        db.execute("UPDATE workers SET state=?1,quota_held=?2,revision=?3,snapshot_json=?4,snapshot_hash=?5 WHERE worker_id=?6", rusqlite::params![serde_json::to_value(worker.state).unwrap().as_str().unwrap(), worker.quota_held, i64::try_from(worker.revision).unwrap(), serde_json::to_string(&worker).unwrap(), snapshot_hash, worker.worker_id]).unwrap();
        db.execute("UPDATE worker_attempts SET container_id=?1,create_operation_id=?2,start_operation_id=?3,start_emitter_id=?4,snapshot_json=?5 WHERE attempt_id=?6", rusqlite::params![worker.attempt.container_id, worker.attempt.create_operation_id, worker.attempt.start_operation_id, worker.attempt.start_emitter_id, serde_json::to_string(&worker.attempt).unwrap(), worker.attempt.attempt_id]).unwrap();
        db.execute("DELETE FROM worker_allocations WHERE event_seq=?1", [seq])
            .unwrap();
        db.execute("DELETE FROM worker_observations WHERE event_seq=?1", [seq])
            .unwrap();
        if !record["observation"].is_null() {
            let observation: RuntimeObservationV1 =
                serde_json::from_value(record["observation"].clone()).unwrap();
            db.execute("INSERT INTO worker_observations(observation_id,worker_id,attempt_id,input_hash,payload_json,event_seq) VALUES(?1,?2,?3,?4,?5,?6)", rusqlite::params![record["operation_id"].as_str().unwrap(),worker.worker_id,worker.attempt.attempt_id,record["input_hash"].as_str().unwrap(),serde_json::to_string(&observation).unwrap(),seq]).unwrap();
        }
        if !record["allocation"].is_null() {
            let allocation: AllocationV1 =
                serde_json::from_value(record["allocation"].clone()).unwrap();
            db.execute("INSERT INTO worker_allocations(attempt_id,worker_id,terminal_observation_id,payload_json,event_seq) VALUES(?1,?2,?3,?4,?5)", rusqlite::params![allocation.attempt_id,worker.worker_id,allocation.terminal_observation_id,serde_json::to_string(&allocation).unwrap(),seq]).unwrap();
        }
    }
    db.execute(
        "UPDATE worker_mutations SET action=?1,input_hash=?2 WHERE event_seq=?3",
        rusqlite::params![
            record["action"].as_str().unwrap(),
            record["input_hash"].as_str().unwrap(),
            seq
        ],
    )
    .unwrap();
    let forged = serde_json::to_string(&event).unwrap();
    let mut digest = Sha256::new();
    digest.update(b"agentsandbox:audit:v1\0");
    digest.update(seq.to_be_bytes());
    for value in [&received, &previous, &forged] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    db.execute(
        "UPDATE events SET payload=?1,hash=?2 WHERE seq=?3",
        rusqlite::params![forged, hex::encode(digest.finalize()), seq],
    )
    .unwrap();
    db.execute(
        "UPDATE quota SET bytes=bytes+?1 WHERE id=1",
        [i64::try_from(forged.len()).unwrap() - i64::try_from(payload.len()).unwrap()],
    )
    .unwrap();
}

#[test]
fn rehashed_database_unknown_cannot_release_quota_without_terminal_evidence() {
    let dir = support::private_tempdir();
    let path = dir.path().join("db");
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let request = admission(&store);
    let worker = admit(&store, &request, &QuotaPolicyV1::default())
        .worker
        .unwrap();
    store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::Unknown {
                reason: "interrupted".into(),
            },
        ))
        .unwrap();
    drop(store);
    rewrite_last_control_record(&path, |record| {
        record["worker"]["state"] = json!("terminal");
        record["worker"]["quota_held"] = json!(false);
        assert!(record["observation"].is_null());
        assert!(record["allocation"].is_null());
    });
    let reopened = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    assert!(
        !reopened.healthy(),
        "rehashed unknown event released quota with no terminal evidence"
    );
    assert!(reopened.worker_quota(&QuotaPolicyV1::default()).is_err());
    assert!(reopened.worker_usage().is_err());
}

#[test]
fn rehashed_database_actions_cannot_invent_or_discard_claims() {
    for action in [
        "create_issued",
        "start_claim",
        "stop",
        "cancel_reserved",
        "observe",
        "cleanup_result",
        "unsupported",
    ] {
        let dir = support::private_tempdir();
        let path = dir.path().join("db");
        let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
        let request = admission(&store);
        let mut worker = admit(&store, &request, &QuotaPolicyV1::default())
            .worker
            .unwrap();
        if action == "create_issued" {
            worker = store
                .mutate_worker(&mutation(&worker, WorkerTransitionV1::Prepare))
                .unwrap()
                .worker;
        } else if action != "unsupported" {
            worker = created(&store, worker);
        }
        let unknown = mutation(
            &worker,
            WorkerTransitionV1::Unknown {
                reason: "interrupted".into(),
            },
        );
        store.mutate_worker(&unknown).unwrap();
        drop(store);
        rewrite_last_control_record(&path, |record| {
            record["action"] = json!(action);
            let mut forged = unknown.clone();
            match action {
                "create_issued" => {
                    forged.transition = WorkerTransitionV1::CreateIssued;
                    record["worker"]["state"] = json!("preparing");
                    record["worker"]["unknown_reason"] = json!(null);
                }
                "start_claim" => {
                    let emitter_id = uuid::Uuid::new_v4().to_string();
                    forged.transition = WorkerTransitionV1::StartClaim {
                        emitter_id: emitter_id.clone(),
                    };
                    record["worker"]["attempt"]["start_emitter_id"] = json!(emitter_id);
                    record["worker"]["state"] = json!("start_issued");
                }
                "stop" => forged.transition = WorkerTransitionV1::Stop,
                "cancel_reserved" => {
                    forged.transition = WorkerTransitionV1::CancelReserved;
                    record["worker"]["state"] = json!("cancelled");
                    record["worker"]["quota_held"] = json!(false);
                }
                "cleanup_result" => {
                    forged.transition = WorkerTransitionV1::CleanupResult {
                        removed: true,
                        error: None,
                    };
                    record["worker"]["cleanup_removed"] = json!(true);
                }
                _ => (),
            }
            if !["observe", "unsupported"].contains(&action) {
                record["input_hash"] =
                    json!(content_hash("ai-guard:worker-mutation:v1", &forged).unwrap());
            }
        });
        let reopened = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
        assert!(
            !reopened.healthy(),
            "rehashed {action} record bypassed its transition"
        );
        assert!(reopened.workers().is_err());
    }
}

#[test]
fn rehashed_database_admission_cannot_begin_with_execution_or_cleanup_evidence() {
    for defect in [
        "terminal",
        "create",
        "start",
        "observation",
        "cleanup",
        "inconsistent",
    ] {
        let dir = support::private_tempdir();
        let path = dir.path().join("db");
        let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
        let request = admission(&store);
        admit(&store, &request, &QuotaPolicyV1::default());
        drop(store);
        rewrite_last_control_record(&path, |record| match defect {
            "terminal" => {
                record["worker"]["state"] = json!("terminal");
                record["worker"]["quota_held"] = json!(false);
            }
            "create" => {
                record["worker"]["attempt"]["create_operation_id"] =
                    json!(uuid::Uuid::new_v4().to_string())
            }
            "start" => {
                record["worker"]["attempt"]["start_operation_id"] =
                    json!(uuid::Uuid::new_v4().to_string())
            }
            "observation" => {
                record["worker"]["observed_started_at"] = json!("2026-10-29T10:00:00Z")
            }
            "cleanup" => record["worker"]["cleanup_removed"] = json!(true),
            _ => record["worker"]["allocation_inconsistent"] = json!(true),
        });
        let reopened = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
        assert!(
            !reopened.healthy(),
            "admission introduced {defect} evidence"
        );
        assert!(reopened.workers().is_err());
    }
}

#[test]
fn rehashed_database_admission_requires_its_exact_prior_allow_and_input() {
    for defect in [
        "receipt_hash",
        "receipt_sequence",
        "receipt_phase",
        "admission_input",
    ] {
        let dir = support::private_tempdir();
        let path = dir.path().join("db");
        let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
        let request = admission(&store);
        admit(&store, &request, &QuotaPolicyV1::default());
        drop(store);
        rewrite_last_control_record(&path, |record| match defect {
            "receipt_hash" => record["worker"]["receipt"]["audit_hash"] = json!("0".repeat(64)),
            "receipt_sequence" => record["worker"]["receipt"]["audit_seq"] = json!(1),
            "receipt_phase" => record["worker"]["receipt"]["phase"] = json!("approved"),
            _ => record["input_hash"] = json!("0".repeat(64)),
        });
        let reopened = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
        assert!(!reopened.healthy(), "admission accepted {defect}");
        assert!(reopened.workers().is_err());
    }
}

#[test]
fn approved_admission_reconstructs_its_bound_request_metadata_after_reopen() {
    let dir = support::private_tempdir();
    let path = dir.path().join("db");
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let mut request = admission(&store);
    let decision = &mut request.decision_request;
    decision.request_id = uuid::Uuid::new_v4().to_string();
    decision.profile = "review".into();
    decision.command.argv = vec!["/usr/bin/printf".into(), "approved-work".into()];
    decision.project_id = Some("project-a".into());
    decision.sandbox_id = Some("sandbox-a".into());
    decision.agent_id = Some("agent-a".into());
    decision.correlation_id = Some("correlation-a".into());
    decision.trace_id = Some("trace-a".into());
    let initial = policy::evaluate(&Policy::default(), decision);
    assert_eq!(initial.outcome, Outcome::Review);
    store
        .decision(decision, Some((initial, json!(null))))
        .unwrap();
    decision.human_approved = true;
    decision.approval_hash = Some(policy::request_hash(decision));
    let approved = store
        .decision(
            decision,
            Some((policy::evaluate(&Policy::default(), decision), json!(null))),
        )
        .unwrap()
        .unwrap();
    request.launch.profile = decision.profile.clone();
    request.launch.command = decision.command.clone();
    request.receipt = DecisionReceiptV1 {
        version: 1,
        request_id: decision.request_id.clone(),
        request_hash: policy::request_hash(decision),
        phase: DecisionPhaseV1::Approved,
        audit_seq: approved.seq,
        audit_hash: approved.hash,
    };
    let accepted = admit(&store, &request, &QuotaPolicyV1::default());
    assert!(accepted.accepted);
    drop(store);
    let reopened = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    assert!(reopened.healthy());
    assert!(reopened.integrity().unwrap().valid);
    assert_eq!(
        admit(&reopened, &request, &QuotaPolicyV1::default()).receipt,
        accepted.receipt
    );
    assert_eq!(
        reopened
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
}

#[test]
fn rehashed_database_terminal_requires_its_exact_allocation_and_mutation_input() {
    for defect in ["missing_allocation", "missing_observation", "changed_input"] {
        let dir = support::private_tempdir();
        let path = dir.path().join("db");
        let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
        let request = admission(&store);
        let worker = created(
            &store,
            admit(&store, &request, &QuotaPolicyV1::default())
                .worker
                .unwrap(),
        );
        let worker = store
            .mutate_worker(&mutation(
                &worker,
                WorkerTransitionV1::StartClaim {
                    emitter_id: uuid::Uuid::new_v4().to_string(),
                },
            ))
            .unwrap()
            .worker;
        store
            .mutate_worker(&mutation(
                &worker,
                WorkerTransitionV1::Observe {
                    observation: RuntimeObservationV1 {
                        inspection: inspection(&worker),
                        observed_at: "2026-10-29T10:00:02Z".into(),
                        status: RuntimeStatusV1::Exited,
                        running: Some(false),
                        started_at: Some("2026-10-29T10:00:00Z".into()),
                        finished_at: Some("2026-10-29T10:00:01Z".into()),
                        exit_code: Some(0),
                        oom_killed: Some(false),
                        restart_count: Some(0),
                    },
                },
            ))
            .unwrap();
        drop(store);
        rewrite_last_control_record(&path, |record| match defect {
            "missing_allocation" => record["allocation"] = json!(null),
            "missing_observation" => {
                record["observation"] = json!(null);
                record["allocation"] = json!(null);
            }
            _ => record["input_hash"] = json!("0".repeat(64)),
        });
        let reopened = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
        assert!(!reopened.healthy(), "terminal accepted {defect}");
        assert!(reopened.worker_usage().is_err());
    }
}

#[test]
fn live_transition_history_reopens_with_the_same_claims_allocations_and_cleanup() {
    let dir = support::private_tempdir();
    let path = dir.path().join("db");
    let mut store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let request = admission(&store);
    let mut worker = admit(&store, &request, &QuotaPolicyV1::default())
        .worker
        .unwrap();
    let running = RuntimeObservationV1 {
        inspection: inspection(&worker),
        observed_at: "2026-10-29T10:00:01Z".into(),
        status: RuntimeStatusV1::Running,
        running: Some(true),
        started_at: Some("2026-10-29T10:00:00Z".into()),
        finished_at: None,
        exit_code: None,
        oom_killed: Some(false),
        restart_count: Some(0),
    };
    let mut terminal = running.clone();
    terminal.observed_at = "2026-10-29T10:00:03Z".into();
    terminal.status = RuntimeStatusV1::Exited;
    terminal.running = Some(false);
    terminal.finished_at = Some("2026-10-29T10:00:02Z".into());
    terminal.exit_code = Some(0);
    let transitions = [
        WorkerTransitionV1::Prepare,
        WorkerTransitionV1::CreateIssued,
        WorkerTransitionV1::Created {
            inspection: inspection(&worker),
        },
        WorkerTransitionV1::StartClaim {
            emitter_id: uuid::Uuid::new_v4().to_string(),
        },
        WorkerTransitionV1::Observe {
            observation: running,
        },
        WorkerTransitionV1::Stop,
        WorkerTransitionV1::Observe {
            observation: terminal,
        },
        WorkerTransitionV1::CleanupResult {
            removed: false,
            error: Some("daemon unavailable".into()),
        },
        WorkerTransitionV1::CleanupResult {
            removed: true,
            error: None,
        },
    ];
    for transition in transitions {
        let change = mutation(&worker, transition);
        let result = store.mutate_worker(&change).unwrap();
        worker = result.worker;
        drop(store);
        store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
        assert!(store.healthy());
        assert!(store.integrity().unwrap().valid);
        let replay = store.mutate_worker(&change).unwrap();
        assert_eq!(replay.receipt, result.receipt);
        if matches!(change.transition, WorkerTransitionV1::StartClaim { .. }) {
            assert_eq!(
                replay.disposition,
                Some(ClaimDispositionV1::AlreadyConsumed)
            );
        }
    }
    assert_eq!(worker.state, WorkerStateV1::Terminal);
    assert_eq!(worker.cleanup_removed, Some(true));
    let ledger = store.worker_usage().unwrap();
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger[0].duration_ns, "2000000000");
    assert!(!ledger[0].billable);
    let request = admission(&store);
    let reserved = admit(&store, &request, &QuotaPolicyV1::default())
        .worker
        .unwrap();
    store
        .mutate_worker(&mutation(&reserved, WorkerTransitionV1::CancelReserved))
        .unwrap();
    drop(store);
    let reopened = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    assert!(reopened.healthy());
    assert_eq!(
        reopened.worker(&reserved.worker_id).unwrap().unwrap().state,
        WorkerStateV1::Cancelled
    );
    assert_eq!(
        reopened
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
    assert_eq!(reopened.worker_usage().unwrap(), ledger);
}

#[test]
fn new_database_has_versioned_control_genesis() {
    let dir = support::private_tempdir();
    let path = dir.path().join("tenant.sqlite");
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let db = rusqlite::Connection::open(path).unwrap();
    let version: i64 = db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(
        version, 2,
        "new databases must initialize the exact v2 control schema"
    );
    assert!(store.integrity().unwrap().valid);
}

#[test]
fn quota_replay_conflict_and_exact_receipt_survive_restart() {
    let dir = support::private_tempdir();
    let path = dir.path().join("db");
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let request = admission(&store);
    let quota = QuotaPolicyV1 {
        max_active_workers: 1,
        ..Default::default()
    };
    let first = admit(&store, &request, &quota);
    assert!(first.accepted);
    let mut changed = request.clone();
    changed.launch.timeout_seconds += 1;
    assert_eq!(
        store
            .admit_worker(
                &changed,
                &Policy::default(),
                policy::evaluate(&Policy::default(), &changed.decision_request),
                json!(null),
                &quota
            )
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::Conflict)
    );
    let second = admission(&store);
    let denied = admit(&store, &second, &quota);
    assert_eq!(denied.refusal, Some(AdmissionRefusalV1::Quota));
    assert_eq!(admit(&store, &second, &quota).receipt, denied.receipt);
    drop(store);
    let reopened = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(admit(&reopened, &request, &quota).receipt, first.receipt);
    assert_eq!(reopened.workers().unwrap().len(), 1);
    assert_eq!(reopened.worker_quota(&quota).unwrap().held_workers, 1);
    assert!(reopened.integrity().unwrap().valid);
}

#[test]
fn historical_allow_is_only_a_link_and_does_not_bypass_current_policy() {
    let dir = support::private_tempdir();
    let store = Store::open(&dir.path().join("db"), Redactor::new(vec![]), 100).unwrap();
    let mut request = admission(&store);
    request.decision_request.request_id = uuid::Uuid::new_v4().to_string();
    request.decision_request.command.argv = vec!["rm".into(), "-rf".into(), "/workspace".into()];
    request.launch.command = request.decision_request.command.clone();
    let stored = store
        .decision(
            &request.decision_request,
            Some((
                Decision::new(Outcome::Allow, "forged", "historic"),
                json!(null),
            )),
        )
        .unwrap()
        .unwrap();
    request.receipt.request_id = request.decision_request.request_id.clone();
    request.receipt.request_hash = policy::request_hash(&request.decision_request);
    request.receipt.audit_seq = stored.seq;
    request.receipt.audit_hash = stored.hash;
    let result = store
        .admit_worker(
            &request,
            &Policy::default(),
            Decision::new(Outcome::Allow, "forged", "fresh proposal is not sufficient"),
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .unwrap();
    assert_eq!(result.refusal, Some(AdmissionRefusalV1::Policy));
    assert!(store.workers().unwrap().is_empty());
}

#[test]
fn receipt_from_another_tenant_and_secret_spec_are_rejected_without_worker() {
    let dir = support::private_tempdir();
    let a = Store::open_for_tenant(&dir.path().join("a"), "a", Redactor::new(vec![]), 100).unwrap();
    let b = Store::open_for_tenant(&dir.path().join("b"), "b", Redactor::new(vec![]), 100).unwrap();
    let request = admission(&a);
    assert!(
        b.admit_worker(
            &request,
            &Policy::default(),
            policy::evaluate(&Policy::default(), &request.decision_request),
            json!(null),
            &QuotaPolicyV1::default()
        )
        .is_err()
    );
    let mut secret = request.clone();
    secret.launch.image_ref = "worker:password=do-not-store".into();
    assert!(
        a.admit_worker(
            &secret,
            &Policy::default(),
            policy::evaluate(&Policy::default(), &secret.decision_request),
            json!(null),
            &QuotaPolicyV1::default()
        )
        .is_err()
    );
    assert!(a.workers().unwrap().is_empty());
    assert!(b.workers().unwrap().is_empty());
}

#[test]
fn claim_replay_never_returns_permission_and_unknown_retains_quota() {
    let dir = support::private_tempdir();
    let path = dir.path().join("db");
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let request = admission(&store);
    let worker = created(
        &store,
        admit(&store, &request, &QuotaPolicyV1::default())
            .worker
            .unwrap(),
    );
    assert_eq!(
        serde_json::to_value(&worker.attempt).unwrap()["created_inspection"],
        serde_json::to_value(inspection(&worker)).unwrap(),
        "the actual validated create inspection must remain durable before claim"
    );
    let claim = mutation(
        &worker,
        WorkerTransitionV1::StartClaim {
            emitter_id: uuid::Uuid::new_v4().to_string(),
        },
    );
    let first = store.mutate_worker(&claim).unwrap();
    assert_eq!(first.disposition, Some(ClaimDispositionV1::EmitOnce));
    let replay = store.mutate_worker(&claim).unwrap();
    assert_eq!(
        replay.disposition,
        Some(ClaimDispositionV1::AlreadyConsumed)
    );
    assert_eq!(first.receipt, replay.receipt);
    let unknown = store
        .mutate_worker(&mutation(
            &first.worker,
            WorkerTransitionV1::Unknown {
                reason: "daemon unavailable".into(),
            },
        ))
        .unwrap();
    assert!(unknown.worker.quota_held);
    assert!(store.worker_usage().unwrap().is_empty());
    drop(store);
    let reopened = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(
        reopened.mutate_worker(&claim).unwrap().disposition,
        Some(ClaimDispositionV1::AlreadyConsumed)
    );
    assert_eq!(
        reopened
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    let stale = mutation(&worker, WorkerTransitionV1::CancelReserved);
    assert!(reopened.mutate_worker(&stale).is_err());
}

#[test]
fn terminal_is_atomic_idempotent_and_uses_exact_allocation_products() {
    let dir = support::private_tempdir();
    let path = dir.path().join("db");
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let request = admission(&store);
    let worker = created(
        &store,
        admit(&store, &request, &QuotaPolicyV1::default())
            .worker
            .unwrap(),
    );
    let worker = store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::StartClaim {
                emitter_id: uuid::Uuid::new_v4().to_string(),
            },
        ))
        .unwrap()
        .worker;
    let observation = RuntimeObservationV1 {
        inspection: inspection(&worker),
        observed_at: "2026-10-29T10:00:02Z".into(),
        status: RuntimeStatusV1::Exited,
        running: Some(false),
        started_at: Some("2026-10-29T10:00:00.000000001Z".into()),
        finished_at: Some("2026-10-29T10:00:01.000000002Z".into()),
        exit_code: Some(0),
        oom_killed: Some(false),
        restart_count: Some(0),
    };
    let terminal = mutation(&worker, WorkerTransitionV1::Observe { observation });
    let result = store.mutate_worker(&terminal).unwrap();
    assert_eq!(result.worker.state, WorkerStateV1::Terminal);
    assert!(!result.worker.quota_held);
    assert_eq!(
        store.mutate_worker(&terminal).unwrap().receipt,
        result.receipt
    );
    let ledger = store.worker_usage().unwrap();
    assert_eq!(ledger.len(), 1);
    assert!(!ledger[0].billable);
    assert_eq!(ledger[0].duration_ns, "1000000001");
    assert_eq!(ledger[0].cpu_millicore_nanoseconds, "2000000002000");
    assert_eq!(ledger[0].memory_byte_nanoseconds, "1073741825073741824");
    assert_eq!(
        store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
    assert!(store.integrity().unwrap().valid);
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute("UPDATE worker_allocations SET payload_json='{}'", [])
        .unwrap();
    assert!(store.worker_usage().is_err());
    assert!(!store.healthy());
    assert!(store.mutate_worker(&terminal).is_err());
}

#[test]
fn two_connections_serialize_quota_one_and_projection_tampering_latches() {
    let dir = support::private_tempdir();
    let path = dir.path().join("db");
    let first = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let second = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let a = admission(&first);
    let b = admission(&second);
    let quota = QuotaPolicyV1 {
        max_active_workers: 1,
        ..Default::default()
    };
    let barrier = std::sync::Barrier::new(2);
    let (a, b) = std::thread::scope(|scope| {
        let thread = scope.spawn(|| {
            barrier.wait();
            admit(&first, &a, &quota)
        });
        barrier.wait();
        let b = admit(&second, &b, &quota);
        (thread.join().unwrap(), b)
    });
    assert_ne!(a.accepted, b.accepted);
    assert_eq!(first.workers().unwrap().len(), 1);
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute("UPDATE workers SET quota_held=0", []).unwrap();
    assert!(first.workers().is_err());
    assert!(!first.healthy());
}

#[test]
fn zero_and_hundred_slot_boundaries_and_pages_are_real_reservations() {
    let dir = support::private_tempdir();
    let store = Store::open(&dir.path().join("db"), Redactor::new(vec![]), 10000).unwrap();
    let request = admission(&store);
    let zero = QuotaPolicyV1 {
        max_active_workers: 0,
        ..Default::default()
    };
    assert_eq!(
        admit(&store, &request, &zero).refusal,
        Some(AdmissionRefusalV1::Quota)
    );
    let full = QuotaPolicyV1 {
        version: 1,
        max_active_workers: 100,
        max_cpu_millis: 200000,
        max_memory_bytes: 100 * 1073741824,
        max_timeout_seconds: 3600,
    };
    for _ in 0..100 {
        let request = admission(&store);
        assert!(admit(&store, &request, &full).accepted);
    }
    let extra = admission(&store);
    assert_eq!(
        admit(&store, &extra, &full).refusal,
        Some(AdmissionRefusalV1::Quota)
    );
    let a = store.workers_page(None, 60).unwrap();
    let b = store.workers_page(Some(&a[59].worker_id), 60).unwrap();
    assert_eq!(a.len(), 60);
    assert_eq!(b.len(), 40);
    assert!(
        a.iter()
            .all(|x| b.iter().all(|y| x.worker_id != y.worker_id))
    );
    assert!(store.workers_page(None, 101).is_err());
    assert_eq!(store.worker_quota(&full).unwrap().held_workers, 100);
    assert_eq!(
        store.worker_quota(&zero).unwrap().held_workers,
        100,
        "policy reduction cannot release existing allocations"
    );
}

#[test]
fn terminal_audit_full_retains_capacity_and_retry_commits_once() {
    let dir = support::private_tempdir();
    let path = dir.path().join("db");
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let request = admission(&store);
    let worker = created(
        &store,
        admit(&store, &request, &QuotaPolicyV1::default())
            .worker
            .unwrap(),
    );
    let worker = store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::StartClaim {
                emitter_id: uuid::Uuid::new_v4().to_string(),
            },
        ))
        .unwrap()
        .worker;
    let terminal = mutation(
        &worker,
        WorkerTransitionV1::Observe {
            observation: RuntimeObservationV1 {
                inspection: inspection(&worker),
                observed_at: "2026-10-29T10:00:02Z".into(),
                status: RuntimeStatusV1::Exited,
                running: Some(false),
                started_at: Some("2026-10-29T10:00:00Z".into()),
                finished_at: Some("2026-10-29T10:00:01Z".into()),
                exit_code: Some(0),
                oom_killed: Some(false),
                restart_count: Some(0),
            },
        },
    );
    let capacity = store.head().unwrap() as u64;
    drop(store);
    let full = Store::open(&path, Redactor::new(vec![]), capacity).unwrap();
    assert!(full.mutate_worker(&terminal).is_err());
    assert!(full.worker_usage().unwrap().is_empty());
    assert_eq!(
        full.worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    assert_eq!(
        full.worker(&worker.worker_id).unwrap().unwrap().revision,
        worker.revision
    );
    drop(full);
    let recovered = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let result = recovered.mutate_worker(&terminal).unwrap();
    assert!(!result.worker.quota_held);
    assert_eq!(recovered.worker_usage().unwrap().len(), 1);
    assert!(recovered.integrity().unwrap().valid);
}

#[test]
fn incomplete_terminal_restart_and_identity_mismatch_retain_quota() {
    for defect in ["exit", "restart", "limits", "authority", "dates"] {
        let dir = support::private_tempdir();
        let store = Store::open(&dir.path().join("db"), Redactor::new(vec![]), 1000).unwrap();
        let request = admission(&store);
        let worker = created(
            &store,
            admit(&store, &request, &QuotaPolicyV1::default())
                .worker
                .unwrap(),
        );
        let worker = store
            .mutate_worker(&mutation(
                &worker,
                WorkerTransitionV1::StartClaim {
                    emitter_id: uuid::Uuid::new_v4().to_string(),
                },
            ))
            .unwrap()
            .worker;
        let mut observation = RuntimeObservationV1 {
            inspection: inspection(&worker),
            observed_at: "2026-10-29T10:00:02Z".into(),
            status: RuntimeStatusV1::Exited,
            running: Some(false),
            started_at: Some("2026-10-29T10:00:00Z".into()),
            finished_at: Some("2026-10-29T10:00:01Z".into()),
            exit_code: Some(0),
            oom_killed: Some(false),
            restart_count: Some(0),
        };
        match defect {
            "exit" => observation.exit_code = None,
            "restart" => observation.restart_count = Some(1),
            "limits" => observation.inspection.cpu_millis = 1000,
            "authority" => observation.inspection.authority_id = "other-daemon".into(),
            _ => observation.finished_at = None,
        };
        let result = store
            .mutate_worker(&mutation(
                &worker,
                WorkerTransitionV1::Observe { observation },
            ))
            .unwrap();
        assert_eq!(result.worker.state, WorkerStateV1::InDoubt, "{defect}");
        assert!(result.worker.quota_held);
        assert!(store.worker_usage().unwrap().is_empty());
    }
}

#[test]
fn concurrent_claims_allow_one_fresh_emitter_and_cancel_closes_future_claims() {
    let dir = support::private_tempdir();
    let path = dir.path().join("db");
    let first = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let request = admission(&first);
    let worker = created(
        &first,
        admit(&first, &request, &QuotaPolicyV1::default())
            .worker
            .unwrap(),
    );
    let second = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let a = mutation(
        &worker,
        WorkerTransitionV1::StartClaim {
            emitter_id: uuid::Uuid::new_v4().to_string(),
        },
    );
    let b = mutation(
        &worker,
        WorkerTransitionV1::StartClaim {
            emitter_id: uuid::Uuid::new_v4().to_string(),
        },
    );
    let barrier = std::sync::Barrier::new(2);
    let (a, b) = std::thread::scope(|scope| {
        let thread = scope.spawn(|| {
            barrier.wait();
            first.mutate_worker(&a)
        });
        barrier.wait();
        let result = second.mutate_worker(&b);
        (thread.join().unwrap(), result)
    });
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(
        a.or(b).unwrap().disposition,
        Some(ClaimDispositionV1::EmitOnce)
    );
    let request = admission(&first);
    let reserved = admit(&first, &request, &QuotaPolicyV1::default())
        .worker
        .unwrap();
    let cancelled = first
        .mutate_worker(&mutation(&reserved, WorkerTransitionV1::CancelReserved))
        .unwrap()
        .worker;
    assert!(!cancelled.quota_held);
    assert!(
        first
            .mutate_worker(&mutation(&cancelled, WorkerTransitionV1::Prepare))
            .is_err()
    );
}

#[test]
fn quota_process_helper() {
    let Ok(path) = std::env::var("AI_GUARD_QUOTA_CHILD_DB") else {
        return;
    };
    let request: AdmissionV1 =
        serde_json::from_str(&std::env::var("AI_GUARD_QUOTA_CHILD_REQUEST").unwrap()).unwrap();
    let store = Store::open(std::path::Path::new(&path), Redactor::new(vec![]), 1000).unwrap();
    let gate = std::env::var("AI_GUARD_QUOTA_CHILD_GATE").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !std::path::Path::new(&gate).exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let quota = QuotaPolicyV1 {
        max_active_workers: 1,
        ..Default::default()
    };
    let result = admit(&store, &request, &quota);
    std::fs::write(
        std::env::var("AI_GUARD_QUOTA_CHILD_RESULT").unwrap(),
        result.accepted.to_string(),
    )
    .unwrap();
}

#[test]
fn separate_processes_cannot_oversubscribe_one_slot() {
    let dir = support::private_tempdir();
    let path = dir.path().join("db");
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let a = admission(&store);
    let b = admission(&store);
    let gate = dir.path().join("gate");
    let ra = dir.path().join("a-result");
    let rb = dir.path().join("b-result");
    let spawn = |request: &AdmissionV1, result: &std::path::Path| {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "quota_process_helper", "--nocapture"])
            .env("AI_GUARD_QUOTA_CHILD_DB", &path)
            .env(
                "AI_GUARD_QUOTA_CHILD_REQUEST",
                serde_json::to_string(request).unwrap(),
            )
            .env("AI_GUARD_QUOTA_CHILD_GATE", &gate)
            .env("AI_GUARD_QUOTA_CHILD_RESULT", result)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    };
    let ca = spawn(&a, &ra);
    let cb = spawn(&b, &rb);
    std::fs::write(&gate, b"go").unwrap();
    assert!(ca.wait_with_output().unwrap().status.success());
    assert!(cb.wait_with_output().unwrap().status.success());
    assert_ne!(
        std::fs::read_to_string(ra).unwrap(),
        std::fs::read_to_string(rb).unwrap()
    );
    assert_eq!(store.workers().unwrap().len(), 1);
}

#[test]
fn a_changed_runtime_start_cannot_be_laundered_through_unknown_observations() {
    let dir = support::private_tempdir();
    let store = Store::open(&dir.path().join("db"), Redactor::new(vec![]), 1000).unwrap();
    let request = admission(&store);
    let worker = created(
        &store,
        admit(&store, &request, &QuotaPolicyV1::default())
            .worker
            .unwrap(),
    );
    let worker = store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::StartClaim {
                emitter_id: uuid::Uuid::new_v4().to_string(),
            },
        ))
        .unwrap()
        .worker;
    let running = RuntimeObservationV1 {
        inspection: inspection(&worker),
        observed_at: "2026-10-29T10:00:02Z".into(),
        status: RuntimeStatusV1::Running,
        running: Some(true),
        started_at: Some("2026-10-29T10:00:00Z".into()),
        finished_at: None,
        exit_code: None,
        oom_killed: Some(false),
        restart_count: Some(0),
    };
    let worker = store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::Observe {
                observation: running.clone(),
            },
        ))
        .unwrap()
        .worker;
    let mut changed = running;
    changed.started_at = Some("2026-10-29T10:00:01Z".into());
    changed.status = RuntimeStatusV1::Exited;
    changed.running = Some(false);
    changed.finished_at = Some("2026-10-29T10:00:02Z".into());
    changed.exit_code = Some(0);
    let worker = store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::Observe {
                observation: changed.clone(),
            },
        ))
        .unwrap()
        .worker;
    assert_eq!(worker.state, WorkerStateV1::InDoubt);
    let worker = store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::Observe {
                observation: changed,
            },
        ))
        .unwrap()
        .worker;
    assert_eq!(
        worker.state,
        WorkerStateV1::InDoubt,
        "repeating changed StartedAt cannot make it authoritative"
    );
    assert!(worker.quota_held);
    assert!(store.worker_usage().unwrap().is_empty());
}

#[test]
fn a_restart_or_changed_limit_cannot_be_erased_by_a_later_zero_snapshot() {
    for defect in ["restart", "limits"] {
        let dir = support::private_tempdir();
        let store = Store::open(&dir.path().join("db"), Redactor::new(vec![]), 1000).unwrap();
        let request = admission(&store);
        let worker = created(
            &store,
            admit(&store, &request, &QuotaPolicyV1::default())
                .worker
                .unwrap(),
        );
        let worker = store
            .mutate_worker(&mutation(
                &worker,
                WorkerTransitionV1::StartClaim {
                    emitter_id: uuid::Uuid::new_v4().to_string(),
                },
            ))
            .unwrap()
            .worker;
        let complete = RuntimeObservationV1 {
            inspection: inspection(&worker),
            observed_at: "2026-10-29T10:00:02Z".into(),
            status: RuntimeStatusV1::Exited,
            running: Some(false),
            started_at: Some("2026-10-29T10:00:00Z".into()),
            finished_at: Some("2026-10-29T10:00:01Z".into()),
            exit_code: Some(0),
            oom_killed: Some(false),
            restart_count: Some(0),
        };
        let mut bad = complete.clone();
        if defect == "restart" {
            bad.restart_count = Some(1);
        } else {
            bad.inspection.memory_bytes /= 2;
        }
        let worker = store
            .mutate_worker(&mutation(
                &worker,
                WorkerTransitionV1::Observe { observation: bad },
            ))
            .unwrap()
            .worker;
        let worker = store
            .mutate_worker(&mutation(
                &worker,
                WorkerTransitionV1::Observe {
                    observation: complete,
                },
            ))
            .unwrap()
            .worker;
        assert_eq!(
            worker.state,
            WorkerStateV1::InDoubt,
            "{defect} cannot disappear from allocation history"
        );
        assert!(worker.quota_held);
        assert!(store.worker_usage().unwrap().is_empty());
    }
}
