//! Shared software-only relay fixtures. Runtime facts are synthetic; every
//! admission/helper/arm/start transition still uses the real typed Store.
use super::*;

pub(super) fn decision_admission(store: &Store) -> AdmissionV1 {
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
            egress: None,
        },
        decision_request: request,
    }
}

pub(super) fn supervised_request(admission: AdmissionV1) -> SupervisedAdmissionV1 {
    let compact = Uuid::new_v4().simple().to_string();
    SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: format!("{compact}{compact}"),
        admission,
    }
}

pub(super) fn admit_supervised(
    store: &Store,
    request: &SupervisedAdmissionV1,
) -> AdmissionResultV1 {
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

pub(super) fn mutation(worker: &WorkerV1, transition: WorkerTransitionV1) -> WorkerMutationV1 {
    WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition,
    }
}

pub(super) fn inspection(worker: &WorkerV1) -> RuntimeInspectionV1 {
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

pub(super) fn host_clock(ns: u128) -> HostClockSampleV1 {
    HostClockSampleV1 {
        version: 1,
        method: HostClockMethodV1::DarwinMachContinuousV1,
        boot_id: "11111111-1111-4111-8111-111111111111".into(),
        continuous_ns_decimal: ns.to_string(),
        observed_utc: Some("2026-10-30T12:00:00Z".into()),
    }
}

pub(super) fn worker_arm(worker: &WorkerV1, intent_hash: &str) -> ArmV1 {
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

pub(super) fn helper_arm(
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

pub(super) fn helper_claim(
    _arm: &ArmV1,
    receipt: &ArmReceiptV1,
    helper_id: Uuid,
) -> HelperStartClaimV1 {
    HelperStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: helper_id.to_string(),
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: receipt.clone(),
        clock_check: host_clock(100_000_000_001),
    }
}

pub(super) fn helper_observation(
    arm: &ArmV1,
    receipt: &ArmReceiptV1,
    helper_id: Uuid,
    running: Option<bool>,
    exit_code: i64,
    verification: Option<ImportVerificationReportV1>,
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

pub(super) fn parent_observation(
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

pub(super) fn fresh(directory: &Path, tenant: &str, max_events: u64) -> Store {
    let path = directory.join(format!("{tenant}.sqlite"));
    let store = Store::create_supervised_for_tenant(
        &path,
        tenant,
        Redactor::new(vec!["fixture-private-provider-secret".into()]),
        max_events,
        &directory.join(format!("{tenant}-before-v3.sqlite")),
    )
    .unwrap();
    store
        .migrate_identity_v4(&directory.join(format!("{tenant}-before-v4.sqlite")))
        .unwrap();
    store
        .migrate_human_access_v5(&directory.join(format!("{tenant}-before-v5.sqlite")))
        .unwrap();
    store
        .migrate_workload_v6(&directory.join(format!("{tenant}-before-v6.sqlite")))
        .unwrap();
    store
}

pub(super) fn endpoint(worker: &WorkerV1) -> RelayEndpointV1 {
    RelayEndpointV1 {
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        generation: 1,
    }
}
pub(super) fn bind(store: &Store, worker: &WorkerV1, token: &str) -> (String, u64) {
    let credential_id = Uuid::new_v4().to_string();
    let receipt = store
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential_id.clone(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Worker {
                worker_id: worker.worker_id.clone(),
                attempt_id: worker.attempt.attempt_id.clone(),
                generation: 1,
            },
            sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
            expires_at: (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
        })
        .unwrap();
    (credential_id, receipt.revision)
}
pub(super) fn running_now(store: &Store) -> WorkerV1 {
    running_from(store, decision_admission(store))
}

/// Drive a caller-built admission through the real typed transitions to Running.
pub(super) fn running_from(store: &Store, admission: AdmissionV1) -> WorkerV1 {
    let (_, worker, now) = claimed_from(store, admission);
    let mut observation = parent_observation(&worker, RuntimeStatusV1::Running, true, None, None);
    observation.observed_at = now.to_rfc3339();
    observation.started_at = Some((now - chrono::Duration::seconds(1)).to_rfc3339());
    store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::Observe { observation },
        ))
        .unwrap()
        .worker
}

pub(super) fn claimed_from(
    store: &Store,
    admission: AdmissionV1,
) -> (ArmedStartClaimV1, WorkerV1, chrono::DateTime<chrono::Utc>) {
    let request = supervised_request(admission);
    let admitted = admit_supervised(store, &request);
    let mut worker = admitted.worker.unwrap();
    worker = store
        .mutate_worker(&mutation(&worker, WorkerTransitionV1::Prepare))
        .unwrap()
        .worker;
    let import_id = Uuid::new_v4();
    let import = helper_arm(
        &worker,
        &request.intent_hash,
        import_id,
        HelperPurposeV1::Import,
        HelperPermissionV1::Admission {
            receipt: admitted.receipt,
        },
    );
    let imported = store.arm_execution(&import).unwrap();
    store
        .claim_helper_start(&helper_claim(&import, &imported.receipt, import_id))
        .unwrap();
    let terminal = store
        .observe_helper(&helper_observation(
            &import,
            &imported.receipt,
            import_id,
            Some(false),
            0,
            None,
        ))
        .unwrap();
    let verify_id = Uuid::new_v4();
    let verify = helper_arm(
        &worker,
        &request.intent_hash,
        verify_id,
        HelperPurposeV1::ImportVerification,
        HelperPermissionV1::ImportVerification {
            import_helper_id: import_id.to_string(),
            import_arm: imported.receipt,
            import_terminal: terminal,
            manifest_digest: worker.launch.workspace_manifest_digest.clone(),
        },
    );
    let verified = store.arm_execution(&verify).unwrap();
    store
        .claim_helper_start(&helper_claim(&verify, &verified.receipt, verify_id))
        .unwrap();
    store
        .observe_helper(&helper_observation(
            &verify,
            &verified.receipt,
            verify_id,
            Some(false),
            0,
            Some(ImportVerificationReportV1 {
                import_helper_id: import_id.to_string(),
                expected_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
                observed_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
            }),
        ))
        .unwrap();
    worker = store
        .mutate_worker(&mutation(&worker, WorkerTransitionV1::CreateIssued))
        .unwrap()
        .worker;
    let mut observed = inspection(&worker);
    observed.container_id = format!("{:064x}", Uuid::new_v4().as_u128());
    worker = store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::Created {
                inspection: observed,
            },
        ))
        .unwrap()
        .worker;
    let mut arm = worker_arm(&worker, &request.intent_hash);
    let now = chrono::Utc::now();
    arm.clock.observed_utc = Some((now - chrono::Duration::seconds(2)).to_rfc3339());
    let armed = store.arm_execution(&arm).unwrap();
    let claim = ArmedStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: armed.receipt,
        clock_check: host_clock(100_000_000_001),
    };
    let result = store.claim_armed_start(&claim).unwrap();
    (claim, result.worker, now)
}

pub(super) fn request(
    store: &Store,
    worker: &WorkerV1,
    credential: &str,
    revision: u64,
    token: &str,
) -> Value {
    json!({"version":1,"operation_id":Uuid::new_v4().to_string(),
        "channel_id":Uuid::new_v4().to_string(),"epoch":1,
        "control_namespace_uuid":store.supervision_state().unwrap().control_namespace_uuid.unwrap(),
        "endpoint":endpoint(worker),"credential_id":credential,"binding_revision":revision,
        "credential_sha256_digest":hex::encode(Sha256::digest(token.as_bytes())),
        "session_id":worker.launch.session_id,"expected_worker_revision":worker.revision,
        "inspection":worker.attempt.created_inspection,"started_at":worker.observed_started_at,
        "runtime_source":"broker_reported_runtime","authority_binding_hash":HASH_A,
        "configuration_fingerprint_version":2,"configuration_fingerprint":HASH_B,
        "expires_at":(chrono::Utc::now()+chrono::Duration::seconds(30)).to_rfc3339()})
}
