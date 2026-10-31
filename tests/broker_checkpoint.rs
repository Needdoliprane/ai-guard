use agentsandbox::policy;
use agentsandbox::{
    audit::Store,
    control::{
        AdmissionV1, ArmResultV1, ArmSubjectV1, ArmV1, ArmedStartClaimV1, DecisionPhaseV1,
        DecisionReceiptV1, ExportRequestV1, HelperCleanupV1, HelperExecutionPolicyV1,
        HelperObservationV1, HelperPermissionV1, HelperPurposeV1, HelperStartClaimV1,
        HostClockMethodV1, HostClockSampleV1, ImportVerificationReportV1, LaunchSpecV1,
        MutationReceiptV1, NetworkV1, QuotaPolicyV1, RestartPolicyV1, RuntimeInspectionV1,
        RuntimeKindV1, RuntimeObservationV1, RuntimeStatusV1, SecurityPresetV1,
        SupervisedAdmissionV1, SupervisionProtocolV1, WorkerMutationV1, WorkerStateV1,
        WorkerTransitionV1, WorkerV1, helper_execution_policy_v1,
    },
    policy::Policy,
    redaction::Redactor,
};
use agentsandbox::{
    broker::{
        checkpoint::{
            BrokerCheckpointV2, DecisionAnswerV1, HelperPreparationV1, SupervisionCheckpointV1,
            SupervisionOperationV1,
        },
        intent::ExecutionIntentV1,
        journal::{
            BindingInputs, CheckpointReceiptV1, ControlSecrets, FsExclusions, Journal,
            JournalLimitsV1, JournalScopeV1, ReserveResult, RevisionRefV1,
            recovery::StopRecoveryReaderV1,
        },
        lifecycle::{BrokerCheckpointV3, LocalCommandRequestV1, LocalCommandV1},
    },
    control_fs::Directory,
    model::DecisionRequest,
    runner::CheckpointPayloadV1,
    runtime::RuntimeAuthority,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};
use uuid::Uuid;

fn legacy_plan_value() -> Value {
    let mut plan: Value = serde_json::from_str(include_str!("fixtures/broker_intent-plan-v3.json"))
        .expect("typed private plan fixture");
    plan["version"] = json!(1);
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
    plan
}

fn legacy_intent() -> (ExecutionIntentV1, DecisionRequest) {
    let plan = legacy_plan_value();
    let request = serde_json::from_value(plan["request"].clone()).unwrap();
    let intent = ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap())
        .expect("valid legacy request with explicit no-credential binding");
    (intent, request)
}

fn open_journal(root: &Path, scope: JournalScopeV1) -> Journal {
    let root = root.canonicalize().expect("canonical private journal root");
    Journal::open(
        &Directory::open_existing(&root).expect("private journal root exists"),
        scope,
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).expect("empty control-secret set is valid"),
    )
    .expect("open real private journal")
}

fn approved_runner_checkpoint(intent: &ExecutionIntentV1) -> CheckpointPayloadV1 {
    let mut plan = legacy_plan_value();
    let unapproved: DecisionRequest = serde_json::from_value(plan["request"].clone()).unwrap();
    plan["request"]["human_approved"] = json!(true);
    plan["request"]["approval_hash"] = json!(policy::request_hash(&unapproved));
    CheckpointPayloadV1::from_plan_bytes(intent, &serde_json::to_vec(&plan).unwrap()).unwrap()
}

fn supervised_admission(
    intent: &ExecutionIntentV1,
    request: &DecisionRequest,
    receipt: DecisionReceiptV1,
) -> SupervisedAdmissionV1 {
    let workspace_manifest_digest = agentsandbox::control::content_hash(
        "ai-guard:workspace-manifest:v1",
        &serde_json::Map::<String, Value>::new(),
    )
    .unwrap();
    SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: intent.intent_hash().unwrap(),
        admission: AdmissionV1 {
            version: 1,
            operation_id: intent.operation_id().to_string(),
            decision_request: request.clone(),
            receipt,
            launch: LaunchSpecV1 {
                version: 1,
                session_id: request.session_id.clone().unwrap(),
                command: request.command.clone(),
                profile: request.profile.clone(),
                runtime_kind: RuntimeKindV1::Docker,
                authority_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
                image_ref: "agentsandbox-worker:local".into(),
                image_id: format!("sha256:{}", "a".repeat(64)),
                network: NetworkV1::None,
                cpu_millis: 2000,
                memory_bytes: 1_073_741_824,
                pids_limit: 256,
                timeout_seconds: 600,
                security_preset: SecurityPresetV1::FixedSandboxV2,
                restart_policy: RestartPolicyV1::No,
                workspace_manifest_digest,
                workspace_import_policy_version: 1,
                provider: "generic".into(),
                capture_output: false,
                credential_binding_id: None,
                credential_names: vec![],
                provider_resolution: None,
            },
        },
    }
}

fn append_v2(
    journal: &Journal,
    operation_id: Uuid,
    expected: &RevisionRefV1,
    runner: &CheckpointPayloadV1,
    namespace: Uuid,
    helpers: Vec<HelperPreparationV1>,
    operations: Vec<SupervisionOperationV1>,
) -> CheckpointReceiptV1 {
    let checkpoint = BrokerCheckpointV2::new(
        runner.clone(),
        SupervisionCheckpointV1::new(namespace, helpers, operations).unwrap(),
    )
    .unwrap();
    journal
        .advance_supervised(operation_id, expected.clone(), &checkpoint)
        .unwrap()
}

#[allow(
    clippy::too_many_arguments,
    reason = "the V3 append fixture carries one real journal graph and its durable command prefix"
)]
fn append_v3(
    journal: &Journal,
    operation_id: Uuid,
    head: &RevisionRefV1,
    runner: &CheckpointPayloadV1,
    namespace: Uuid,
    helpers: Vec<HelperPreparationV1>,
    operations: Vec<SupervisionOperationV1>,
    commands: &[LocalCommandV1],
) -> CheckpointReceiptV1 {
    try_append_v3(
        journal,
        operation_id,
        head,
        runner,
        namespace,
        helpers,
        operations,
        commands,
    )
    .unwrap()
}

#[allow(
    clippy::too_many_arguments,
    reason = "the fallible V3 fixture append reports the first causal lifecycle rejection"
)]
fn try_append_v3(
    journal: &Journal,
    operation_id: Uuid,
    head: &RevisionRefV1,
    runner: &CheckpointPayloadV1,
    namespace: Uuid,
    helpers: Vec<HelperPreparationV1>,
    operations: Vec<SupervisionOperationV1>,
    commands: &[LocalCommandV1],
) -> Result<CheckpointReceiptV1, agentsandbox::broker::journal::JournalError> {
    let broker = BrokerCheckpointV2::new(
        runner.clone(),
        SupervisionCheckpointV1::new(namespace, helpers, operations).unwrap(),
    )
    .unwrap();
    journal.advance_lifecycle(
        operation_id,
        head.clone(),
        &BrokerCheckpointV3::new(broker, commands.to_vec()).unwrap(),
    )
}

fn synthetic_runtime_authority() -> RuntimeAuthority {
    serde_json::from_value(json!({
        "authority_id":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "kind":"docker",
        "endpoint":"unix:///private/fixture.sock",
        "engine_id":null,
        "podman":null,
        "invocation":{
            "policy_version":2,
            "kind":"docker",
            "approved_entry":"/usr/bin/docker",
            "canonical_program":"/usr/bin/docker",
            "file_identity":{"device":1,"inode":2,"owner":1000,"group":1000,"mode":493,"length":10,"sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},
            "environment":{},
            "cwd":"/private/fixture"
        }
    }))
    .expect("explicit synthetic Docker authority binding")
}

fn managed_runner_checkpoint(
    intent: &ExecutionIntentV1,
    request: &DecisionRequest,
    admission: &AdmissionV1,
    worker: &WorkerV1,
    authority: &RuntimeAuthority,
    owned: Option<&agentsandbox::runtime::OwnedContainerSnapshot>,
    decision: &agentsandbox::model::Decision,
) -> CheckpointPayloadV1 {
    let mut plan = legacy_plan_value();
    plan["managed"] = json!({
        "tenant_id":"tenant-checkpoint-test",
        "gateway_origin":"http://127.0.0.1:8787",
        "config_root":"/private/control",
        "binding":authority,
        "authority_file":"runtime-authority.json",
        "admission":admission,
        "worker":worker,
        "owned":owned,
        "pending":null,
        "uncertain":[],
        "lock_identity":null,
        "before":{},
        "import":null,
        "decision":decision,
        "output_event":null,
        "output_acknowledged":false,
        "result":null,
        "archive_name":"worker.tar",
        "archive_digest":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "credential_digest":null,
        "workspace_control_exclusion_version":1,
        "invocation_event":null
    });
    plan["request"] = serde_json::to_value(request).unwrap();
    CheckpointPayloadV1::from_plan_bytes(intent, &serde_json::to_vec(&plan).unwrap())
        .expect("valid V1 managed runner projection from Store snapshots")
}

fn managed_pending_runner_checkpoint(
    intent: &ExecutionIntentV1,
    request: &DecisionRequest,
    admission: &AdmissionV1,
    authority: &RuntimeAuthority,
    decision: &agentsandbox::model::Decision,
) -> CheckpointPayloadV1 {
    let mut plan = legacy_plan_value();
    plan["managed"] = json!({
        "tenant_id":"tenant-checkpoint-test",
        "gateway_origin":"http://127.0.0.1:8787",
        "config_root":"/private/control",
        "binding":authority,
        "authority_file":"runtime-authority.json",
        "admission":admission,
        "worker":null,
        "owned":null,
        "pending":null,
        "uncertain":[],
        "lock_identity":null,
        "before":{},
        "import":null,
        "decision":decision,
        "output_event":null,
        "output_acknowledged":false,
        "result":null,
        "archive_name":"worker.tar",
        "archive_digest":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "credential_digest":null,
        "workspace_control_exclusion_version":1,
        "invocation_event":null
    });
    plan["request"] = serde_json::to_value(request).unwrap();
    CheckpointPayloadV1::from_plan_bytes(intent, &serde_json::to_vec(&plan).unwrap())
        .expect("valid prepared runner checkpoint before worker admission ACK")
}

fn synthetic_owned_snapshot(
    authority: &RuntimeAuthority,
    session: Uuid,
    operation: Uuid,
    inspection: &RuntimeInspectionV1,
    configuration_fingerprint: &str,
) -> agentsandbox::runtime::OwnedContainerSnapshot {
    serde_json::from_value(json!({
        "handle": {
            "authority_fingerprint":authority.fingerprint(),
            "session":session,
            "operation":operation,
            "id":inspection.container_id,
            "name":inspection.stable_name,
            "configuration_fingerprint_version":inspection.security_preset.configuration_fingerprint_version()
        },
        "observation": {
            "id":inspection.container_id,
            "name":inspection.stable_name,
            "image_id":inspection.image_id,
            "restart_count":0,
            "process":{"status":"running","running":true,"started_at":"2026-10-30T12:00:00Z","finished_at":null,"exit_code":null,"oom_killed":false},
            "allocation":{"cpu":{"quota":u64::from(inspection.cpu_millis)*100,"period":100000},"memory_bytes":inspection.memory_bytes,"pids_limit":inspection.pids_limit},
            "configuration_fingerprint":configuration_fingerprint,
            "configuration_fingerprint_version":inspection.security_preset.configuration_fingerprint_version(),
            "observed_at":"2026-10-30T12:00:00Z",
            "diagnostics":[]
        }
    }))
    .expect("explicit synthetic owned runtime snapshot")
}

fn helper_arm_request(
    parent: &WorkerV1,
    intent_hash: &str,
    helper_id: Uuid,
    purpose: HelperPurposeV1,
    permission: HelperPermissionV1,
    authority: &RuntimeAuthority,
) -> (
    HelperExecutionPolicyV1,
    ArmV1,
    agentsandbox::runtime::OwnedContainerSnapshot,
) {
    let policy = helper_execution_policy_v1(parent, purpose, helper_id).unwrap();
    let inspection = RuntimeInspectionV1 {
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
    };
    let armed_at = 100_000_000_000_u128;
    let fingerprint = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let arm = ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: parent.worker_id.clone(),
        attempt_id: parent.attempt.attempt_id.clone(),
        expected_revision: parent.revision,
        intent_hash: intent_hash.into(),
        subject: ArmSubjectV1::Helper {
            helper_id: helper_id.to_string(),
            purpose,
            policy_version: 1,
            permission: Box::new(permission),
        },
        inspection: inspection.clone(),
        authority_binding_hash: authority.fingerprint(),
        configuration_fingerprint_version: policy
            .security_preset
            .configuration_fingerprint_version(),
        configuration_fingerprint: fingerprint.into(),
        clock: HostClockSampleV1 {
            version: 1,
            method: HostClockMethodV1::DarwinMachContinuousV1,
            boot_id: "11111111-1111-4111-8111-111111111111".into(),
            continuous_ns_decimal: armed_at.to_string(),
            observed_utc: Some("2026-10-30T12:00:00Z".into()),
        },
        timeout_seconds: 120,
        deadline_ns_decimal: (armed_at + 120_000_000_000).to_string(),
    };
    let owned = synthetic_owned_snapshot(
        authority,
        Uuid::parse_str(&parent.launch.session_id).unwrap(),
        helper_id,
        &inspection,
        fingerprint,
    );
    (policy, arm, owned)
}

fn helper_claim_request(
    arm: &ArmV1,
    receipt: &agentsandbox::control::ArmReceiptV1,
    helper: Uuid,
) -> HelperStartClaimV1 {
    HelperStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: helper.to_string(),
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: receipt.clone(),
        clock_check: HostClockSampleV1 {
            version: 1,
            method: arm.clock.method,
            boot_id: arm.clock.boot_id.clone(),
            continuous_ns_decimal: "100000000001".into(),
            observed_utc: Some("2026-10-30T12:00:00Z".into()),
        },
    }
}

fn helper_observation_request(
    arm: &ArmV1,
    receipt: &agentsandbox::control::ArmReceiptV1,
    helper: Uuid,
    verification: Option<ImportVerificationReportV1>,
) -> HelperObservationV1 {
    HelperObservationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: helper.to_string(),
        arm_receipt: receipt.clone(),
        observation: RuntimeObservationV1 {
            inspection: arm.inspection.clone(),
            observed_at: "2026-10-30T12:00:02Z".into(),
            status: RuntimeStatusV1::Exited,
            running: Some(false),
            started_at: Some("2026-10-30T12:00:00Z".into()),
            finished_at: Some("2026-10-30T12:00:01Z".into()),
            exit_code: Some(0),
            oom_killed: Some(false),
            restart_count: Some(0),
        },
        verification,
    }
}

fn helper_cleanup_request(
    receipt: &agentsandbox::control::ArmReceiptV1,
    helper: Uuid,
) -> HelperCleanupV1 {
    HelperCleanupV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: helper.to_string(),
        arm_receipt: receipt.clone(),
        removed: true,
        error: None,
    }
}

fn synthetic_worker_inspection(worker: &WorkerV1) -> RuntimeInspectionV1 {
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

fn worker_arm_request(worker: &WorkerV1, intent_hash: &str, authority: &RuntimeAuthority) -> ArmV1 {
    let inspection = worker.attempt.created_inspection.clone().unwrap();
    let armed_at = 100_000_000_000_u128;
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
        authority_binding_hash: authority.fingerprint(),
        configuration_fingerprint_version: 2,
        configuration_fingerprint:
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        clock: HostClockSampleV1 {
            version: 1,
            method: HostClockMethodV1::DarwinMachContinuousV1,
            boot_id: "11111111-1111-4111-8111-111111111111".into(),
            continuous_ns_decimal: armed_at.to_string(),
            observed_utc: Some("2026-10-30T12:00:00Z".into()),
        },
        timeout_seconds: timeout,
        deadline_ns_decimal: (armed_at + u128::from(timeout) * 1_000_000_000).to_string(),
    }
}

fn worker_mutation(worker: &WorkerV1, transition: WorkerTransitionV1) -> WorkerMutationV1 {
    WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition,
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

#[allow(
    clippy::too_many_arguments,
    reason = "the helper lifecycle threads one shared journal/store graph and its causal checkpoint state"
)]
fn run_helper_lifecycle(
    journal: &Journal,
    store: &Store,
    operation_id: Uuid,
    head: &mut RevisionRefV1,
    runner: &CheckpointPayloadV1,
    namespace: Uuid,
    helpers: &mut Vec<HelperPreparationV1>,
    operations: &mut Vec<SupervisionOperationV1>,
    intent: &ExecutionIntentV1,
    parent: &WorkerV1,
    authority: &RuntimeAuthority,
    purpose: HelperPurposeV1,
    permission: HelperPermissionV1,
    verification: Option<ImportVerificationReportV1>,
) -> (Uuid, ArmResultV1, MutationReceiptV1, MutationReceiptV1) {
    let helper_id = Uuid::new_v4();
    let (policy, arm, owned) = helper_arm_request(
        parent,
        &intent.intent_hash().unwrap(),
        helper_id,
        purpose,
        permission.clone(),
        authority,
    );
    helpers.push(HelperPreparationV1::new(policy.clone(), permission.clone(), None).unwrap());
    *head = append_v2(
        journal,
        operation_id,
        head,
        runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();

    let helper_index = helpers.len() - 1;
    helpers[helper_index] =
        HelperPreparationV1::new(policy.clone(), permission.clone(), Some(owned.clone())).unwrap();
    let arm_index = operations.len();
    operations.push(SupervisionOperationV1::Arm {
        parent: parent.clone(),
        owned,
        request: arm.clone(),
        response: None,
    });
    *head = append_v2(
        journal,
        operation_id,
        head,
        runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();
    let armed = store.arm_execution(&arm).unwrap();
    operations[arm_index] = SupervisionOperationV1::Arm {
        parent: parent.clone(),
        owned: helpers[helper_index].owned().unwrap().clone(),
        request: arm.clone(),
        response: Some(armed.clone()),
    };
    *head = append_v2(
        journal,
        operation_id,
        head,
        runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();

    let claim = helper_claim_request(&arm, &armed.receipt, helper_id);
    let claim_index = operations.len();
    operations.push(SupervisionOperationV1::HelperClaim {
        request: claim.clone(),
        response: None,
    });
    *head = append_v2(
        journal,
        operation_id,
        head,
        runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();
    let claimed = store.claim_helper_start(&claim).unwrap();
    operations[claim_index] = SupervisionOperationV1::HelperClaim {
        request: claim,
        response: Some(claimed),
    };
    *head = append_v2(
        journal,
        operation_id,
        head,
        runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();

    let observation = helper_observation_request(&arm, &armed.receipt, helper_id, verification);
    let observation_index = operations.len();
    operations.push(SupervisionOperationV1::HelperObservation {
        request: observation.clone(),
        response: None,
    });
    *head = append_v2(
        journal,
        operation_id,
        head,
        runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();
    let terminal = store.observe_helper(&observation).unwrap();
    operations[observation_index] = SupervisionOperationV1::HelperObservation {
        request: observation,
        response: Some(terminal.clone()),
    };
    *head = append_v2(
        journal,
        operation_id,
        head,
        runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();

    let cleanup = helper_cleanup_request(&armed.receipt, helper_id);
    let cleanup_index = operations.len();
    operations.push(SupervisionOperationV1::HelperCleanup {
        request: cleanup.clone(),
        response: None,
    });
    *head = append_v2(
        journal,
        operation_id,
        head,
        runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();
    let cleanup_receipt = store.record_helper_cleanup(&cleanup).unwrap();
    operations[cleanup_index] = SupervisionOperationV1::HelperCleanup {
        request: cleanup,
        response: Some(cleanup_receipt.clone()),
    };
    *head = append_v2(
        journal,
        operation_id,
        head,
        runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();
    (helper_id, armed, terminal, cleanup_receipt)
}

#[allow(
    clippy::too_many_arguments,
    reason = "the fixture threads one real Store and Journal through the Export claim transition"
)]
fn append_export_helper_claim(
    journal: &Journal,
    store: &Store,
    operation_id: Uuid,
    head: &mut RevisionRefV1,
    runner: &CheckpointPayloadV1,
    namespace: Uuid,
    helpers: &mut Vec<HelperPreparationV1>,
    operations: &mut Vec<SupervisionOperationV1>,
    commands: &[LocalCommandV1],
    intent: &ExecutionIntentV1,
    parent: &WorkerV1,
    authority: &RuntimeAuthority,
    permission: HelperPermissionV1,
) -> Result<(), (&'static str, agentsandbox::broker::journal::JournalError)> {
    let helper_id = Uuid::new_v4();
    let (policy, arm, owned) = helper_arm_request(
        parent,
        &intent.intent_hash().unwrap(),
        helper_id,
        HelperPurposeV1::Export,
        permission.clone(),
        authority,
    );
    helpers.push(HelperPreparationV1::new(policy.clone(), permission.clone(), None).unwrap());
    *head = match try_append_v3(
        journal,
        operation_id,
        head,
        runner,
        namespace,
        helpers.clone(),
        operations.clone(),
        commands,
    ) {
        Ok(receipt) => receipt.head().clone(),
        Err(error) => {
            helpers.pop();
            return Err(("helper-preparation", error));
        }
    };

    let helper_index = helpers.len() - 1;
    helpers[helper_index] =
        HelperPreparationV1::new(policy.clone(), permission.clone(), Some(owned.clone())).unwrap();
    let arm_index = operations.len();
    operations.push(SupervisionOperationV1::Arm {
        parent: parent.clone(),
        owned: owned.clone(),
        request: arm.clone(),
        response: None,
    });
    *head = match try_append_v3(
        journal,
        operation_id,
        head,
        runner,
        namespace,
        helpers.clone(),
        operations.clone(),
        commands,
    ) {
        Ok(receipt) => receipt.head().clone(),
        Err(error) => {
            operations.pop();
            helpers[helper_index] =
                HelperPreparationV1::new(policy.clone(), permission.clone(), None).unwrap();
            return Err(("arm-pending", error));
        }
    };
    let armed = store.arm_execution(&arm).unwrap();
    operations[arm_index] = SupervisionOperationV1::Arm {
        parent: parent.clone(),
        owned: owned.clone(),
        request: arm.clone(),
        response: Some(armed.clone()),
    };
    *head = match try_append_v3(
        journal,
        operation_id,
        head,
        runner,
        namespace,
        helpers.clone(),
        operations.clone(),
        commands,
    ) {
        Ok(receipt) => receipt.head().clone(),
        Err(error) => {
            operations[arm_index] = SupervisionOperationV1::Arm {
                parent: parent.clone(),
                owned: owned.clone(),
                request: arm.clone(),
                response: None,
            };
            return Err(("arm-ack", error));
        }
    };

    let claim = helper_claim_request(&arm, &armed.receipt, helper_id);
    operations.push(SupervisionOperationV1::HelperClaim {
        request: claim.clone(),
        response: None,
    });
    let checkpoint = BrokerCheckpointV3::new(
        BrokerCheckpointV2::new(
            runner.clone(),
            SupervisionCheckpointV1::new(namespace, helpers.clone(), operations.clone()).unwrap(),
        )
        .unwrap(),
        commands.to_vec(),
    )
    .unwrap();
    let claim_receipt = match journal.advance_lifecycle(operation_id, head.clone(), &checkpoint) {
        Ok(receipt) => receipt,
        Err(error) => {
            operations.pop();
            return Err(("helper-claim", error));
        }
    };
    *head = claim_receipt.head().clone();
    let claim_result = store.claim_helper_start(&claim).unwrap();
    *operations.last_mut().unwrap() = SupervisionOperationV1::HelperClaim {
        request: claim.clone(),
        response: Some(claim_result),
    };
    *head = match try_append_v3(
        journal,
        operation_id,
        head,
        runner,
        namespace,
        helpers.clone(),
        operations.clone(),
        commands,
    ) {
        Ok(receipt) => receipt.head().clone(),
        Err(error) => {
            *operations.last_mut().unwrap() = SupervisionOperationV1::HelperClaim {
                request: claim,
                response: None,
            };
            return Err(("helper-claim-ack", error));
        }
    };
    Ok(())
}

fn revision_zero_path(root: &Path, operation_id: Uuid) -> std::path::PathBuf {
    let mut scopes = fs::read_dir(root.join("broker-journal-v1")).unwrap();
    scopes
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("jobs")
        .join(operation_id.to_string())
        .join("rev-00000000000000000000.json")
}

fn revision_path(root: &Path, operation_id: Uuid, revision: u64) -> std::path::PathBuf {
    revision_zero_path(root, operation_id).with_file_name(format!("rev-{revision:020}.json"))
}

fn current_pointer_path(root: &Path, operation_id: Uuid) -> std::path::PathBuf {
    revision_zero_path(root, operation_id)
        .parent()
        .unwrap()
        .join("current.json")
}

#[test]
fn real_journal_promotes_v1_rev0_to_pending_decision_v2_without_rewriting_rev0() {
    let (intent, request) = legacy_intent();
    let operation_id = intent.operation_id();
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let initial_bytes = initial.canonical_bytes().unwrap();
    let namespace = Uuid::from_u128(0x22222222222242228222222222222222);
    let scope = JournalScopeV1::new(
        Uuid::from_u128(0x11111111111141118111111111111111),
        "tenant-checkpoint-test",
        "http://127.0.0.1:8787",
        namespace,
        "broker-test",
    )
    .unwrap();
    let root = support::private_tempdir();
    let journal = open_journal(root.path(), scope.clone());
    let ticket = match journal
        .reserve(
            operation_id,
            br#"{"action":"submit","request":"checkpoint-v2"}"#,
        )
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("fresh operation must return one live ticket"),
    };
    let accepted = journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .expect("accept V1 with its initial V1 rev0");

    let rev0_path = revision_zero_path(root.path(), operation_id);
    let rev0_bytes = fs::read(&rev0_path).unwrap();
    let current_path = current_pointer_path(root.path(), operation_id);
    let valid_rev0_pointer = fs::read(&current_path).unwrap();
    let rev0_head = accepted.head().clone();
    assert_eq!(
        hex::encode(Sha256::digest(&rev0_bytes)),
        "9d92b7117f72e976f61abacce7bc71c0fdace0cb217ec9796259e4b8f73fa046",
        "V1 rev0 wire bytes match the frozen native baseline"
    );
    assert_eq!(
        rev0_head.hash(),
        "b49fc25a9999c1e194fe4d47cec4475f5534b4d7d8c75f5eb4705127907103ad",
        "V1 rev0 retains its historical hash domain and bytes"
    );
    let rev0: Value = serde_json::from_slice(&rev0_bytes).unwrap();
    assert_eq!(rev0["version"], 1);
    assert_eq!(rev0["revision"], 0);
    assert!(rev0["previous"].is_null());
    assert_eq!(rev0["payload"]["managed"], Value::Null);

    let next = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(
            namespace,
            Vec::new(),
            vec![SupervisionOperationV1::Decision {
                request: request.clone(),
                response: None,
            }],
        )
        .expect("first operation is pending until its real ACK"),
    )
    .expect("V2 wrapper retains the exact V1 runner checkpoint");
    let next_bytes = next.canonical_bytes().unwrap();
    let next_wire: Value = serde_json::from_slice(&next_bytes).unwrap();
    assert_eq!(next_wire["version"], 2);
    assert_eq!(next_wire["runner"]["managed"], Value::Null);
    assert!(next_wire["supervision"]["operations"][0]["response"].is_null());
    assert_eq!(
        BrokerCheckpointV2::from_bytes(&next_bytes)
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        next_bytes
    );
    let mut unknown_top_level = next_wire.clone();
    unknown_top_level["unexpected"] = json!(true);
    assert!(
        BrokerCheckpointV2::from_bytes(&serde_json::to_vec(&unknown_top_level).unwrap()).is_err()
    );
    let mut unknown_nested = next_wire.clone();
    unknown_nested["supervision"]["operations"][0]["request"]["unexpected"] = json!(true);
    assert!(BrokerCheckpointV2::from_bytes(&serde_json::to_vec(&unknown_nested).unwrap()).is_err());
    let mut unknown_tag = next_wire.clone();
    unknown_tag["supervision"]["operations"][0]["kind"] = json!("unrecognized");
    assert!(BrokerCheckpointV2::from_bytes(&serde_json::to_vec(&unknown_tag).unwrap()).is_err());
    let mut unknown_version = next_wire.clone();
    unknown_version["version"] = json!(3);
    assert!(
        BrokerCheckpointV2::from_bytes(&serde_json::to_vec(&unknown_version).unwrap()).is_err()
    );

    let receipt = journal
        .advance_supervised(operation_id, rev0_head.clone(), &next)
        .expect("advance the real V1 journal head to V2");
    assert_eq!(receipt.head().revision(), 1);

    let store_root = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let decision_store = Store::open(
        &store_root.path().join("audit.sqlite"),
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let policy = Policy::default();
    let stored_decision = decision_store
        .decision(
            &request,
            Some((policy::evaluate(&policy, &request), json!(null))),
        )
        .unwrap()
        .unwrap();
    let decision = stored_decision.event.decision.clone().unwrap();
    let decision_answer = DecisionAnswerV1::new(
        decision,
        DecisionReceiptV1 {
            version: 1,
            request_id: request.request_id.clone(),
            request_hash: policy::request_hash(&request),
            phase: DecisionPhaseV1::Initial,
            audit_seq: stored_decision.seq,
            audit_hash: stored_decision.hash,
        },
    )
    .unwrap();
    let acknowledged = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(
            namespace,
            Vec::new(),
            vec![SupervisionOperationV1::Decision {
                request: request.clone(),
                response: Some(decision_answer),
            }],
        )
        .unwrap(),
    )
    .unwrap();
    let rev2_receipt = journal
        .advance_supervised(operation_id, receipt.head().clone(), &acknowledged)
        .expect("a real Store decision ACK advances the V2 head");
    assert_eq!(rev2_receipt.head().revision(), 2);
    let valid_v2_pointer = fs::read(&current_path).unwrap();

    // Two cloned handles share the live anti-rollback floor. Restoring a
    // previously valid V1 pointer is rejected by both; put the real current
    // V2 pointer back before reopening.
    let sibling = journal.clone();
    fs::write(&current_path, &valid_rev0_pointer).unwrap();
    assert_eq!(fs::read(&current_path).unwrap(), valid_rev0_pointer);
    for live in [&journal, &sibling] {
        assert!(live.load(operation_id).is_err());
        assert!(
            live.advance_supervised(operation_id, rev2_receipt.head().clone(), &acknowledged)
                .is_err()
        );
    }
    fs::write(&current_path, &valid_v2_pointer).unwrap();
    assert_eq!(fs::read(&current_path).unwrap(), valid_v2_pointer);
    drop(sibling);
    let wrong_namespace = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(
            Uuid::from_u128(0x33333333333343338333333333333333),
            Vec::new(),
            vec![SupervisionOperationV1::Decision {
                request: request.clone(),
                response: None,
            }],
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        journal
            .advance_supervised(operation_id, rev2_receipt.head().clone(), &wrong_namespace)
            .is_err()
    );
    assert_eq!(
        journal.load(operation_id).unwrap().unwrap().head(),
        rev2_receipt.head()
    );
    assert_eq!(fs::read(&rev0_path).unwrap(), rev0_bytes);
    assert_eq!(accepted.head().hash(), rev0_head.hash());
    assert_eq!(
        journal
            .load(operation_id)
            .unwrap()
            .unwrap()
            .checkpoint()
            .canonical_bytes()
            .unwrap(),
        initial_bytes
    );

    drop(journal);
    let reopened = open_journal(root.path(), scope.clone());
    let recovered = reopened.load(operation_id).unwrap().unwrap();
    assert_eq!(recovered.head(), rev2_receipt.head());
    assert_eq!(
        recovered.checkpoint().canonical_bytes().unwrap(),
        initial_bytes
    );
    assert_eq!(
        recovered
            .broker_checkpoint_v2()
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        acknowledged.canonical_bytes().unwrap()
    );
    let original_replay = reopened
        .advance_supervised(operation_id, accepted.head().clone(), &next)
        .expect("exact historical V2 CAS replay returns its original receipt");
    assert_eq!(original_replay.head(), receipt.head());
    let mut divergent_wire = next_wire;
    divergent_wire["supervision"]["operations"][0]["request"]["request_id"] =
        json!("44444444-4444-4444-8444-444444444444");
    let divergent = BrokerCheckpointV2::from_bytes(&serde_json::to_vec(&divergent_wire).unwrap())
        .expect("valid but divergent request payload");
    assert!(
        reopened
            .advance_supervised(operation_id, accepted.head().clone(), &divergent)
            .is_err()
    );
    assert_eq!(
        reopened.load(operation_id).unwrap().unwrap().head(),
        rev2_receipt.head()
    );

    // A second live handle sees the same V2 floor: exact historical replay
    // returns the original receipt; divergent replay from the old V1 head is
    // refused without changing the shared head.
    let shared = reopened.clone();
    let shared_replay = shared
        .advance_supervised(operation_id, accepted.head().clone(), &next)
        .expect("shared handle returns exact historical V2 replay");
    assert_eq!(shared_replay.head(), receipt.head());
    assert_eq!(
        shared.load(operation_id).unwrap().unwrap().head(),
        rev2_receipt.head()
    );

    let other_scope = JournalScopeV1::new(
        Uuid::from_u128(0x11111111111141118111111111111111),
        "tenant-other-checkpoint-test",
        "http://127.0.0.1:8787",
        namespace,
        "broker-test",
    )
    .unwrap();
    let other_tenant_journal = open_journal(root.path(), other_scope);
    assert!(other_tenant_journal.load(operation_id).unwrap().is_none());
}

#[test]
fn approved_v1_runner_cannot_be_downgraded_by_first_v2_revision() {
    let (intent, request) = legacy_intent();
    let operation_id = intent.operation_id();
    let namespace = Uuid::from_u128(0x22222222222242228222222222222222);
    let scope = JournalScopeV1::new(
        Uuid::from_u128(0x11111111111141118111111111111111),
        "tenant-checkpoint-test",
        "http://127.0.0.1:8787",
        namespace,
        "broker-test",
    )
    .unwrap();
    let root = support::private_tempdir();
    let journal = open_journal(root.path(), scope.clone());
    let ticket = match journal
        .reserve(
            operation_id,
            br#"{"action":"submit","request":"checkpoint-v2-approved"}"#,
        )
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("fresh operation must return one live ticket"),
    };

    // Keep the same legacy intent/request fields as `legacy_intent`, then add
    // only a valid human approval to the V1 runner checkpoint.
    let mut normalized: Value =
        serde_json::from_str(include_str!("fixtures/broker_intent-plan-v3.json")).unwrap();
    normalized["version"] = json!(1);
    normalized["options"]["profile"] = json!("strict");
    normalized["options"]["network"] = json!("none");
    normalized["options"]["provider"] = json!("generic");
    normalized["options"]["argv"] = json!(["/usr/bin/id"]);
    for field in [
        "client",
        "auth_mode",
        "client_profile",
        "providers_file",
        "prompt",
        "model",
    ] {
        normalized["options"].as_object_mut().unwrap().remove(field);
    }
    normalized["request"]["profile"] = json!("strict");
    normalized["request"]["agent_id"] = json!("generic-22222222-2222-4222-8222-222222222222");
    normalized["request"]["command"]["argv"] = json!(["/usr/bin/id"]);
    normalized["request"]
        .as_object_mut()
        .unwrap()
        .remove("provider_resolution");
    normalized["provider_input"] = Value::Null;
    normalized["provider_credential"] = Value::Null;
    let approved_request: DecisionRequest =
        serde_json::from_value(normalized["request"].clone()).unwrap();
    normalized["request"]["human_approved"] = json!(true);
    normalized["request"]["approval_hash"] = json!(policy::request_hash(&approved_request));
    let approved =
        CheckpointPayloadV1::from_plan_bytes(&intent, &serde_json::to_vec(&normalized).unwrap())
            .unwrap();
    let approved_bytes = approved.canonical_bytes().unwrap();

    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let accepted = journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .expect("V1 rev0 remains the exact initial checkpoint");
    let rev0_bytes = fs::read(revision_zero_path(root.path(), operation_id)).unwrap();
    let approved_receipt = journal
        .advance(operation_id, accepted.head().clone(), &approved)
        .expect("historical V1 advance records a valid human approval");
    assert_eq!(approved_receipt.head().revision(), 1);
    let rev1_bytes = fs::read(revision_path(root.path(), operation_id, 1)).unwrap();

    let v2_runner = CheckpointPayloadV1::initial(&intent).unwrap();
    let next = BrokerCheckpointV2::new(
        v2_runner,
        SupervisionCheckpointV1::new(
            namespace,
            Vec::new(),
            vec![SupervisionOperationV1::Decision {
                request,
                response: None,
            }],
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        journal
            .advance_supervised(operation_id, approved_receipt.head().clone(), &next)
            .is_err(),
        "V2 must not erase a previously confirmed V1 human approval"
    );

    let still_v1 = journal.load(operation_id).unwrap().unwrap();
    assert!(still_v1.broker_checkpoint_v2().is_none());
    assert_eq!(
        still_v1.checkpoint().canonical_bytes().unwrap(),
        approved_bytes
    );
    assert_eq!(
        fs::read(revision_zero_path(root.path(), operation_id)).unwrap(),
        rev0_bytes
    );
    assert_eq!(
        fs::read(revision_path(root.path(), operation_id, 1)).unwrap(),
        rev1_bytes
    );
}

#[test]
fn sqlite_decision_and_admission_ack_follow_pending_journal_revisions() {
    let (intent, request) = legacy_intent();
    let operation_id = intent.operation_id();
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let store_root = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Store::open(
        &store_root.path().join("audit.sqlite"),
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    store
        .migrate_control_v3(&store_root.path().join("migration-backup.sqlite"))
        .unwrap();
    let namespace = Uuid::parse_str(
        store
            .supervision_state()
            .unwrap()
            .control_namespace_uuid
            .as_deref()
            .expect("migrated store has a control namespace"),
    )
    .unwrap();
    let scope = JournalScopeV1::new(
        Uuid::from_u128(0x11111111111141118111111111111111),
        "tenant-checkpoint-test",
        "http://127.0.0.1:8787",
        namespace,
        "broker-test",
    )
    .unwrap();
    let journal_root = support::private_tempdir();
    let journal = open_journal(journal_root.path(), scope.clone());
    let ticket = match journal
        .reserve(
            operation_id,
            br#"{"action":"submit","request":"checkpoint-v2-sqlite"}"#,
        )
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("fresh operation must return one live ticket"),
    };
    let accepted = journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .unwrap();

    let policy = Policy::default();
    let stored_decision = store
        .decision(
            &request,
            Some((policy::evaluate(&policy, &request), json!(null))),
        )
        .unwrap()
        .unwrap();
    let decision = stored_decision.event.decision.clone().unwrap();
    let decision_receipt = DecisionReceiptV1 {
        version: 1,
        request_id: request.request_id.clone(),
        request_hash: policy::request_hash(&request),
        phase: DecisionPhaseV1::Initial,
        audit_seq: stored_decision.seq,
        audit_hash: stored_decision.hash.clone(),
    };
    let decision_answer =
        DecisionAnswerV1::new(decision.clone(), decision_receipt.clone()).unwrap();

    let pending_decision = SupervisionOperationV1::Decision {
        request: request.clone(),
        response: None,
    };
    let rev1 = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(namespace, Vec::new(), vec![pending_decision]).unwrap(),
    )
    .unwrap();
    let rev1_receipt = journal
        .advance_supervised(operation_id, accepted.head().clone(), &rev1)
        .unwrap();

    let decision_ack = SupervisionOperationV1::Decision {
        request: request.clone(),
        response: Some(decision_answer),
    };
    let rev2 = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(namespace, Vec::new(), vec![decision_ack.clone()]).unwrap(),
    )
    .unwrap();
    let rev2_receipt = journal
        .advance_supervised(operation_id, rev1_receipt.head().clone(), &rev2)
        .unwrap();

    let workspace_manifest_digest = agentsandbox::control::content_hash(
        "ai-guard:workspace-manifest:v1",
        &serde_json::Map::<String, Value>::new(),
    )
    .unwrap();
    let admission = AdmissionV1 {
        version: 1,
        operation_id: operation_id.to_string(),
        decision_request: request.clone(),
        receipt: decision_receipt,
        launch: LaunchSpecV1 {
            version: 1,
            session_id: request.session_id.clone().unwrap(),
            command: request.command.clone(),
            profile: request.profile.clone(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
            image_ref: "agentsandbox-worker:local".into(),
            image_id: format!("sha256:{}", "a".repeat(64)),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            timeout_seconds: 600,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest,
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
        },
    };
    let supervised = SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: intent.intent_hash().unwrap(),
        admission,
    };
    let pending_admission = SupervisionOperationV1::Admission {
        request: supervised.clone(),
        response: None,
    };
    let rev3 = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(
            namespace,
            Vec::new(),
            vec![decision_ack.clone(), pending_admission],
        )
        .unwrap(),
    )
    .unwrap();
    let rev3_receipt = journal
        .advance_supervised(operation_id, rev2_receipt.head().clone(), &rev3)
        .unwrap();

    let admission_result = store
        .admit_supervised(
            &supervised,
            &policy,
            decision,
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .unwrap();
    assert!(admission_result.accepted);
    let admitted_worker = admission_result.worker.clone().unwrap();

    let rev4 = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(
            namespace,
            Vec::new(),
            vec![
                decision_ack,
                SupervisionOperationV1::Admission {
                    request: supervised,
                    response: Some(admission_result),
                },
            ],
        )
        .unwrap(),
    )
    .unwrap();
    let rev4_receipt = journal
        .advance_supervised(operation_id, rev3_receipt.head().clone(), &rev4)
        .unwrap();

    let prepare = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: admitted_worker.worker_id.clone(),
        attempt_id: admitted_worker.attempt.attempt_id.clone(),
        expected_revision: admitted_worker.revision,
        transition: WorkerTransitionV1::Prepare,
    };
    let mut prepare_ops = rev4.supervision().operations().to_vec();
    prepare_ops.push(SupervisionOperationV1::WorkerMutation {
        request: prepare.clone(),
        response: None,
    });
    let rev5 = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(namespace, Vec::new(), prepare_ops).unwrap(),
    )
    .unwrap();
    let rev5_receipt = journal
        .advance_supervised(operation_id, rev4_receipt.head().clone(), &rev5)
        .unwrap();
    let prepare_result = store.mutate_worker(&prepare).unwrap();
    assert_eq!(
        prepare_result.worker.state,
        agentsandbox::control::WorkerStateV1::Preparing
    );
    assert!(prepare_result.worker.quota_held);
    let prepared_worker = prepare_result.worker.clone();
    let mut prepare_ack_ops = rev4.supervision().operations().to_vec();
    prepare_ack_ops.push(SupervisionOperationV1::WorkerMutation {
        request: prepare.clone(),
        response: Some(prepare_result.clone()),
    });
    let rev6 = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(namespace, Vec::new(), prepare_ack_ops.clone()).unwrap(),
    )
    .unwrap();
    let rev6_receipt = journal
        .advance_supervised(operation_id, rev5_receipt.head().clone(), &rev6)
        .expect("true Store Prepare response is accepted");

    // Recreate the identical pending prefix in a second journal so the true
    // Store response can be accepted first, then a candidate with only its
    // returned state altered can be checked from the same pending revision.
    let alternate_root = support::private_tempdir();
    let alternate = open_journal(alternate_root.path(), scope);
    let alternate_ticket = match alternate
        .reserve(
            operation_id,
            br#"{"action":"submit","request":"checkpoint-v2-sqlite"}"#,
        )
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("fresh alternate journal needs a reservation"),
    };
    let alternate_accepted = alternate
        .accept(alternate_ticket, &intent, BindingInputs::None, &initial)
        .unwrap();
    let alternate_rev1 = alternate
        .advance_supervised(operation_id, alternate_accepted.head().clone(), &rev1)
        .unwrap();
    let alternate_rev2 = alternate
        .advance_supervised(operation_id, alternate_rev1.head().clone(), &rev2)
        .unwrap();
    let alternate_rev3 = alternate
        .advance_supervised(operation_id, alternate_rev2.head().clone(), &rev3)
        .unwrap();
    let alternate_rev4 = alternate
        .advance_supervised(operation_id, alternate_rev3.head().clone(), &rev4)
        .unwrap();
    let alternate_rev5 = alternate
        .advance_supervised(operation_id, alternate_rev4.head().clone(), &rev5)
        .unwrap();
    let mut altered_prepare_result = prepare_result.clone();
    altered_prepare_result.worker.state = agentsandbox::control::WorkerStateV1::Terminal;
    altered_prepare_result.worker.quota_held = false;
    let mut altered_ops = rev4.supervision().operations().to_vec();
    altered_ops.push(SupervisionOperationV1::WorkerMutation {
        request: prepare.clone(),
        response: Some(altered_prepare_result),
    });
    let altered = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(namespace, Vec::new(), altered_ops).unwrap(),
    )
    .unwrap();
    assert!(
        alternate
            .advance_supervised(operation_id, alternate_rev5.head().clone(), &altered)
            .is_err()
    );
    assert_eq!(
        alternate.load(operation_id).unwrap().unwrap().head(),
        alternate_rev5.head()
    );

    // Two operations may be pending against the same Store parent. A forged
    // pair of ACKs for the same worker/attempt/revision must not be accepted;
    // the second response below is deliberately synthesized from the genuine
    // Store result and is not represented as a Store-produced ACK.
    let duplicate_prepare = worker_mutation(&admitted_worker, WorkerTransitionV1::Prepare);
    let mut both_pending = rev4.supervision().operations().to_vec();
    both_pending.push(SupervisionOperationV1::WorkerMutation {
        request: prepare.clone(),
        response: None,
    });
    both_pending.push(SupervisionOperationV1::WorkerMutation {
        request: duplicate_prepare.clone(),
        response: None,
    });
    let both_pending_checkpoint = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(namespace, Vec::new(), both_pending.clone()).unwrap(),
    )
    .unwrap();
    let both_pending_head = alternate
        .advance_supervised(
            operation_id,
            alternate_rev5.head().clone(),
            &both_pending_checkpoint,
        )
        .unwrap();
    let mut unique_result = prepare_result.clone();
    unique_result.receipt.operation_id = duplicate_prepare.operation_id.clone();
    unique_result.receipt.input_hash =
        agentsandbox::control::content_hash("ai-guard:worker-mutation:v1", &duplicate_prepare)
            .unwrap();
    both_pending[2] = SupervisionOperationV1::WorkerMutation {
        request: prepare,
        response: Some(prepare_result.clone()),
    };
    both_pending[3] = SupervisionOperationV1::WorkerMutation {
        request: duplicate_prepare,
        response: Some(unique_result),
    };
    let duplicate_ack_checkpoint = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(namespace, Vec::new(), both_pending).unwrap(),
    )
    .unwrap();
    assert!(
        alternate
            .advance_supervised(
                operation_id,
                both_pending_head.head().clone(),
                &duplicate_ack_checkpoint
            )
            .is_err(),
        "distinct operations cannot acknowledge one worker revision twice"
    );
    assert_eq!(
        alternate.load(operation_id).unwrap().unwrap().head(),
        both_pending_head.head()
    );

    // A shape-valid pending Arm probes strict wire decoding only. The worker
    // came from Store; inspection and owned snapshot values are synthetic.
    let synthetic_inspection = RuntimeInspectionV1 {
        authority_id: prepared_worker.launch.authority_id.clone(),
        runtime_kind: prepared_worker.launch.runtime_kind,
        container_id: "d".repeat(64),
        stable_name: prepared_worker.attempt.stable_name.clone(),
        volume_name: prepared_worker.attempt.volume_name.clone(),
        image_id: prepared_worker.launch.image_id.clone(),
        spec_hash: prepared_worker.spec_hash.clone(),
        cpu_millis: prepared_worker.launch.cpu_millis,
        memory_bytes: prepared_worker.launch.memory_bytes,
        pids_limit: prepared_worker.launch.pids_limit,
        network: prepared_worker.launch.network,
        security_preset: prepared_worker.launch.security_preset,
        restart_policy: prepared_worker.launch.restart_policy,
    };
    let armed_at = 1000_u128;
    let arm = ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: prepared_worker.worker_id.clone(),
        attempt_id: prepared_worker.attempt.attempt_id.clone(),
        expected_revision: prepared_worker.revision,
        intent_hash: intent.intent_hash().unwrap(),
        subject: ArmSubjectV1::Worker,
        inspection: synthetic_inspection,
        authority_binding_hash: "b".repeat(64),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: "c".repeat(64),
        clock: HostClockSampleV1 {
            version: 1,
            method: HostClockMethodV1::DarwinMachContinuousV1,
            boot_id: "11111111-1111-4111-8111-111111111111".into(),
            continuous_ns_decimal: armed_at.to_string(),
            observed_utc: Some("2026-10-30T12:00:00Z".into()),
        },
        timeout_seconds: 600,
        deadline_ns_decimal: (armed_at + 600_000_000_000).to_string(),
    };
    let synthetic_owned: agentsandbox::runtime::OwnedContainerSnapshot = serde_json::from_value(json!({
        "handle": {
            "authority_fingerprint": "f".repeat(64),
            "session": request.session_id.as_deref().unwrap(),
            "operation": operation_id,
            "id": "d".repeat(64),
            "name": prepared_worker.attempt.stable_name,
            "configuration_fingerprint_version": 2
        },
        "observation": {
            "id": "d".repeat(64),
            "name": prepared_worker.attempt.stable_name,
            "image_id": prepared_worker.launch.image_id,
            "restart_count": 0,
            "process": {"status":"created","running":false,"started_at":null,"finished_at":null,"exit_code":null,"oom_killed":false},
            "allocation": {"cpu":null,"memory_bytes":null,"pids_limit":null},
            "configuration_fingerprint": "c".repeat(64),
            "configuration_fingerprint_version": 2,
            "observed_at": "2026-10-30T12:00:00Z",
            "diagnostics": []
        }
    }))
    .unwrap();
    let arm_envelope = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(
            namespace,
            Vec::new(),
            vec![SupervisionOperationV1::Arm {
                parent: prepared_worker.clone(),
                owned: synthetic_owned,
                request: arm,
                response: None,
            }],
        )
        .unwrap(),
    )
    .unwrap();
    let arm_wire: Value = serde_json::from_slice(&arm_envelope.canonical_bytes().unwrap()).unwrap();
    let ordinary_arm_bytes = serde_json::to_vec(&arm_wire).unwrap();
    assert!(BrokerCheckpointV2::from_bytes(&ordinary_arm_bytes).is_ok());
    assert!(serde_json::from_slice::<BrokerCheckpointV2>(&ordinary_arm_bytes).is_ok());
    let mut unknown_owned = arm_wire.clone();
    unknown_owned["supervision"]["operations"][0]["owned"]["unexpected"] = Value::Null;
    let unknown_owned_bytes = serde_json::to_vec(&unknown_owned).unwrap();
    let owned_from_bytes_rejected = BrokerCheckpointV2::from_bytes(&unknown_owned_bytes).is_err();
    let owned_deserialize_rejected =
        serde_json::from_slice::<BrokerCheckpointV2>(&unknown_owned_bytes).is_err();
    let mut unknown_observation = arm_wire;
    unknown_observation["supervision"]["operations"][0]["owned"]["observation"]["unexpected"] =
        Value::Null;
    let unknown_observation_bytes = serde_json::to_vec(&unknown_observation).unwrap();
    let observation_from_bytes_rejected =
        BrokerCheckpointV2::from_bytes(&unknown_observation_bytes).is_err();
    let observation_deserialize_rejected =
        serde_json::from_slice::<BrokerCheckpointV2>(&unknown_observation_bytes).is_err();
    assert!(
        owned_from_bytes_rejected
            && owned_deserialize_rejected
            && observation_from_bytes_rejected
            && observation_deserialize_rejected,
        "unknown Arm nested fields must be rejected: owned(from_bytes={owned_from_bytes_rejected}, Deserialize={owned_deserialize_rejected}), observation(from_bytes={observation_from_bytes_rejected}, Deserialize={observation_deserialize_rejected})"
    );

    // The Store rejects this malformed transition payload. The journal must
    // independently reject them while they are pending, before any ACK exists.
    let valid_prefix = rev6.supervision().operations().to_vec();
    let mutation = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: prepared_worker.worker_id.clone(),
        attempt_id: prepared_worker.attempt.attempt_id.clone(),
        expected_revision: prepared_worker.revision,
        transition: WorkerTransitionV1::Unknown {
            reason: String::new(),
        },
    };
    let before_store = store.head().unwrap();
    assert!(
        store.mutate_worker(&mutation).is_err(),
        "Store accepted empty Unknown reason"
    );
    assert_eq!(store.head().unwrap(), before_store);

    let mut operations = valid_prefix;
    operations.push(SupervisionOperationV1::WorkerMutation {
        request: mutation,
        response: None,
    });
    let malformed = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(namespace, Vec::new(), operations).unwrap(),
    )
    .unwrap();
    assert!(
        journal
            .advance_supervised(operation_id, rev6_receipt.head().clone(), &malformed)
            .is_err(),
        "journal accepted pending Unknown with an empty reason"
    );
    assert_eq!(
        journal.load(operation_id).unwrap().unwrap().head(),
        rev6_receipt.head()
    );

    let downgrade = journal.advance(operation_id, rev6_receipt.head().clone(), &initial);
    assert!(downgrade.is_err(), "V2 head cannot be downgraded to V1");
    let saved = journal.load(operation_id).unwrap().unwrap();
    assert_eq!(saved.head(), rev6_receipt.head());
    assert_eq!(
        saved.checkpoint().canonical_bytes().unwrap(),
        initial.canonical_bytes().unwrap()
    );
    assert_eq!(
        saved
            .broker_checkpoint_v2()
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        rev6.canonical_bytes().unwrap()
    );
}

#[test]
fn admission_pending_must_equal_approved_current_runner_request() {
    let (intent, request) = legacy_intent();
    let operation_id = intent.operation_id();
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let namespace = Uuid::from_u128(0x22222222222242228222222222222222);
    let scope = JournalScopeV1::new(
        Uuid::from_u128(0x11111111111141118111111111111111),
        "tenant-checkpoint-phase-test",
        "http://127.0.0.1:8787",
        namespace,
        "broker-test",
    )
    .unwrap();
    let root = support::private_tempdir();
    let journal = open_journal(root.path(), scope);
    let ticket = match journal
        .reserve(
            operation_id,
            br#"{"action":"submit","request":"phase-binding"}"#,
        )
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("fresh operation must return one live ticket"),
    };
    let accepted = journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .unwrap();

    let rev1 = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(
            namespace,
            Vec::new(),
            vec![SupervisionOperationV1::Decision {
                request: request.clone(),
                response: None,
            }],
        )
        .unwrap(),
    )
    .unwrap();
    let rev1_receipt = journal
        .advance_supervised(operation_id, accepted.head().clone(), &rev1)
        .unwrap();

    let store_root = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Store::open(
        &store_root.path().join("audit.sqlite"),
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let policy = Policy::default();
    let stored = store
        .decision(
            &request,
            Some((policy::evaluate(&policy, &request), json!(null))),
        )
        .unwrap()
        .unwrap();
    let decision = stored.event.decision.clone().unwrap();
    assert_eq!(decision.outcome, agentsandbox::model::Outcome::Allow);
    let receipt = DecisionReceiptV1 {
        version: 1,
        request_id: request.request_id.clone(),
        request_hash: policy::request_hash(&request),
        phase: DecisionPhaseV1::Initial,
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
    };
    let decision_ack = SupervisionOperationV1::Decision {
        request: request.clone(),
        response: Some(DecisionAnswerV1::new(decision, receipt.clone()).unwrap()),
    };
    let rev2 = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(namespace, Vec::new(), vec![decision_ack.clone()]).unwrap(),
    )
    .unwrap();
    let rev2_receipt = journal
        .advance_supervised(operation_id, rev1_receipt.head().clone(), &rev2)
        .unwrap();

    let admission = supervised_admission(&intent, &request, receipt);
    let mismatched = BrokerCheckpointV2::new(
        approved_runner_checkpoint(&intent),
        SupervisionCheckpointV1::new(
            namespace,
            Vec::new(),
            vec![
                decision_ack,
                SupervisionOperationV1::Admission {
                    request: admission,
                    response: None,
                },
            ],
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        journal
            .advance_supervised(operation_id, rev2_receipt.head().clone(), &mismatched)
            .is_err(),
        "approved runner cannot accept an unapproved Admission bound to an old Initial decision"
    );
    assert_eq!(
        journal.load(operation_id).unwrap().unwrap().head(),
        rev2_receipt.head()
    );
}

#[test]
fn prepared_managed_runner_can_match_a_pending_admission_without_worker() {
    let (intent, request) = legacy_intent();
    let operation_id = intent.operation_id();
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let store_root = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Store::open(
        &store_root.path().join("audit.sqlite"),
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    store
        .migrate_control_v3(&store_root.path().join("migration-backup.sqlite"))
        .unwrap();
    let namespace = Uuid::parse_str(
        store
            .supervision_state()
            .unwrap()
            .control_namespace_uuid
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    let scope = JournalScopeV1::new(
        Uuid::from_u128(0x11111111111141118111111111111111),
        "tenant-checkpoint-test",
        "http://127.0.0.1:8787",
        namespace,
        "broker-test",
    )
    .unwrap();
    let journal_root = support::private_tempdir();
    let journal = open_journal(journal_root.path(), scope);
    let ticket = match journal
        .reserve(
            operation_id,
            br#"{"action":"submit","request":"pending-preparation"}"#,
        )
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("fresh operation must return one live ticket"),
    };
    let accepted = journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .unwrap();

    let policy = Policy::default();
    let stored = store
        .decision(
            &request,
            Some((policy::evaluate(&policy, &request), json!(null))),
        )
        .unwrap()
        .unwrap();
    let decision = stored.event.decision.clone().unwrap();
    assert_eq!(decision.outcome, agentsandbox::model::Outcome::Allow);
    let decision_receipt = DecisionReceiptV1 {
        version: 1,
        request_id: request.request_id.clone(),
        request_hash: policy::request_hash(&request),
        phase: DecisionPhaseV1::Initial,
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
    };
    let admission = supervised_admission(&intent, &request, decision_receipt);
    let mut operations = vec![SupervisionOperationV1::Decision {
        request: request.clone(),
        response: None,
    }];
    let rev1 = append_v2(
        &journal,
        operation_id,
        accepted.head(),
        &initial,
        namespace,
        Vec::new(),
        operations.clone(),
    );
    operations[0] = SupervisionOperationV1::Decision {
        request: request.clone(),
        response: Some(
            DecisionAnswerV1::new(
                decision.clone(),
                DecisionReceiptV1 {
                    version: 1,
                    request_id: request.request_id.clone(),
                    request_hash: policy::request_hash(&request),
                    phase: DecisionPhaseV1::Initial,
                    audit_seq: stored.seq,
                    audit_hash: stored.hash.clone(),
                },
            )
            .unwrap(),
        ),
    };
    let rev2 = append_v2(
        &journal,
        operation_id,
        rev1.head(),
        &initial,
        namespace,
        Vec::new(),
        operations.clone(),
    );
    operations.push(SupervisionOperationV1::Admission {
        request: admission.clone(),
        response: None,
    });
    let rev3 = append_v2(
        &journal,
        operation_id,
        rev2.head(),
        &initial,
        namespace,
        Vec::new(),
        operations.clone(),
    );

    let authority = synthetic_runtime_authority();
    let prepared = managed_pending_runner_checkpoint(
        &intent,
        &request,
        &admission.admission,
        &authority,
        &decision,
    );
    let result = append_v2(
        &journal,
        operation_id,
        rev3.head(),
        &prepared,
        namespace,
        Vec::new(),
        operations.clone(),
    );
    assert_eq!(result.head().revision(), rev3.head().revision() + 1);
    let saved = journal.load(operation_id).unwrap().unwrap();
    assert_eq!(saved.head(), result.head());
    assert_eq!(
        saved
            .broker_checkpoint_v2()
            .unwrap()
            .runner()
            .canonical_bytes()
            .unwrap(),
        prepared.canonical_bytes().unwrap()
    );

    let no_capacity = QuotaPolicyV1 {
        max_active_workers: 0,
        ..QuotaPolicyV1::default()
    };
    let refused = store
        .admit_supervised(&admission, &policy, decision, json!(null), &no_capacity)
        .unwrap();
    assert!(!refused.accepted);
    assert_eq!(
        refused.refusal,
        Some(agentsandbox::control::AdmissionRefusalV1::Quota)
    );
    assert!(refused.worker.is_none());
    operations[1] = SupervisionOperationV1::Admission {
        request: admission,
        response: Some(refused),
    };
    let refused_checkpoint = BrokerCheckpointV2::new(
        prepared.clone(),
        SupervisionCheckpointV1::new(namespace, Vec::new(), operations).unwrap(),
    )
    .unwrap();
    let refused_receipt = journal
        .advance_supervised(operation_id, result.head().clone(), &refused_checkpoint)
        .expect("real quota refusal remains linked to its prepared managed runner");
    assert_eq!(
        refused_receipt.head().revision(),
        result.head().revision() + 1
    );
    let refused_saved = journal.load(operation_id).unwrap().unwrap();
    assert_eq!(refused_saved.head(), refused_receipt.head());
    assert_eq!(
        refused_saved
            .broker_checkpoint_v2()
            .unwrap()
            .runner()
            .canonical_bytes()
            .unwrap(),
        prepared.canonical_bytes().unwrap()
    );
}

#[test]
fn sqlite_store_receipts_cover_all_supervision_operation_variants() {
    let (intent, request) = legacy_intent();
    let operation_id = intent.operation_id();
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let store_root = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Store::open(
        &store_root.path().join("audit.sqlite"),
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    store
        .migrate_control_v3(&store_root.path().join("migration-backup.sqlite"))
        .unwrap();
    let namespace = Uuid::parse_str(
        store
            .supervision_state()
            .unwrap()
            .control_namespace_uuid
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    let scope = JournalScopeV1::new(
        Uuid::new_v4(),
        "tenant-checkpoint-test",
        "http://127.0.0.1:8787",
        namespace,
        "broker-test",
    )
    .unwrap();
    let journal_root = support::private_tempdir();
    let journal = open_journal(journal_root.path(), scope.clone());
    let ticket = match journal
        .reserve(operation_id, br#"{"action":"submit","request":"v2-graph"}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("fresh operation must return a reservation"),
    };
    let accepted = journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .unwrap();
    let policy = Policy::default();
    let stored = store
        .decision(
            &request,
            Some((policy::evaluate(&policy, &request), json!(null))),
        )
        .unwrap()
        .unwrap();
    let decision = stored.event.decision.clone().unwrap();
    assert_eq!(decision.outcome, agentsandbox::model::Outcome::Allow);
    let decision_receipt = DecisionReceiptV1 {
        version: 1,
        request_id: request.request_id.clone(),
        request_hash: policy::request_hash(&request),
        phase: DecisionPhaseV1::Initial,
        audit_seq: stored.seq,
        audit_hash: stored.hash,
    };
    let decision_answer =
        DecisionAnswerV1::new(decision.clone(), decision_receipt.clone()).unwrap();
    let mut operations = vec![SupervisionOperationV1::Decision {
        request: request.clone(),
        response: None,
    }];
    let mut head = append_v2(
        &journal,
        operation_id,
        accepted.head(),
        &initial,
        namespace,
        Vec::new(),
        operations.clone(),
    )
    .head()
    .clone();
    operations[0] = SupervisionOperationV1::Decision {
        request: request.clone(),
        response: Some(decision_answer),
    };
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &initial,
        namespace,
        Vec::new(),
        operations.clone(),
    )
    .head()
    .clone();

    let admission = supervised_admission(&intent, &request, decision_receipt);
    operations.push(SupervisionOperationV1::Admission {
        request: admission.clone(),
        response: None,
    });
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &initial,
        namespace,
        Vec::new(),
        operations.clone(),
    )
    .head()
    .clone();
    let admitted = store
        .admit_supervised(
            &admission,
            &policy,
            decision.clone(),
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .unwrap();
    assert!(admitted.accepted);
    let mut worker = admitted.worker.clone().unwrap();

    // An Admission that is only pending does not authorize worker state,
    // helper preparation, or an owned runtime snapshot. These candidates use
    // the real Store reservation and explicitly synthetic runtime facts.
    let pending_authority = synthetic_runtime_authority();
    let future_managed = managed_runner_checkpoint(
        &intent,
        &request,
        &admission.admission,
        &worker,
        &pending_authority,
        None,
        &decision,
    );
    let future_worker = BrokerCheckpointV2::new(
        future_managed,
        SupervisionCheckpointV1::new(namespace, Vec::new(), operations.clone()).unwrap(),
    )
    .unwrap();
    assert!(
        journal
            .advance_supervised(operation_id, head.clone(), &future_worker)
            .is_err()
    );
    let helper_id_before_admission = Uuid::new_v4();
    let (pending_helper_policy, _, pending_helper_owned) = helper_arm_request(
        &worker,
        &intent.intent_hash().unwrap(),
        helper_id_before_admission,
        HelperPurposeV1::Import,
        HelperPermissionV1::Admission {
            receipt: admitted.receipt.clone(),
        },
        &pending_authority,
    );
    let prepared_without_worker = managed_pending_runner_checkpoint(
        &intent,
        &request,
        &admission.admission,
        &pending_authority,
        &decision,
    );
    let pending_helper_permission = HelperPermissionV1::Admission {
        receipt: admitted.receipt.clone(),
    };
    let new_helper_without_owned = HelperPreparationV1::new(
        pending_helper_policy.clone(),
        pending_helper_permission.clone(),
        None,
    )
    .unwrap();
    let future_new_helper = BrokerCheckpointV2::new(
        prepared_without_worker,
        SupervisionCheckpointV1::new(
            namespace,
            vec![new_helper_without_owned],
            operations.clone(),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        journal
            .advance_supervised(operation_id, head.clone(), &future_new_helper)
            .is_err(),
        "a new helper needs an accepted Admission even with managed binding and owned=None"
    );

    let helper_with_owned_before_admission = HelperPreparationV1::new(
        pending_helper_policy,
        pending_helper_permission,
        Some(pending_helper_owned),
    )
    .unwrap();
    let future_owned_helper = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(
            namespace,
            vec![helper_with_owned_before_admission],
            operations.clone(),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        journal
            .advance_supervised(operation_id, head.clone(), &future_owned_helper)
            .is_err()
    );

    operations[1] = SupervisionOperationV1::Admission {
        request: admission.clone(),
        response: Some(admitted.clone()),
    };
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &initial,
        namespace,
        Vec::new(),
        operations.clone(),
    )
    .head()
    .clone();

    let prepare = worker_mutation(&worker, WorkerTransitionV1::Prepare);
    operations.push(SupervisionOperationV1::WorkerMutation {
        request: prepare.clone(),
        response: None,
    });
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &initial,
        namespace,
        Vec::new(),
        operations.clone(),
    )
    .head()
    .clone();
    let prepare_result = store.mutate_worker(&prepare).unwrap();
    worker = prepare_result.worker.clone();
    operations[2] = SupervisionOperationV1::WorkerMutation {
        request: prepare,
        response: Some(prepare_result),
    };
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &initial,
        namespace,
        Vec::new(),
        operations.clone(),
    )
    .head()
    .clone();
    let authority = synthetic_runtime_authority();
    let mut runner = managed_runner_checkpoint(
        &intent,
        &request,
        &admission.admission,
        &worker,
        &authority,
        None,
        &decision,
    );

    // D01: real V1 history once contained a managed checkpoint, then returned
    // to None. The promotion check must inspect the full V1 prefix.
    let downgrade_root = support::private_tempdir();
    let downgrade = open_journal(downgrade_root.path(), scope.clone());
    let downgrade_ticket = match downgrade
        .reserve(
            operation_id,
            br#"{"action":"submit","request":"v1-history"}"#,
        )
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("fresh D01 journal must return a reservation"),
    };
    let v1_accepted = downgrade
        .accept(downgrade_ticket, &intent, BindingInputs::None, &initial)
        .unwrap();
    let v1_rev0_bytes = fs::read(revision_zero_path(downgrade_root.path(), operation_id)).unwrap();
    let managed_v1 = downgrade
        .advance(operation_id, v1_accepted.head().clone(), &runner)
        .expect("a valid managed V1 checkpoint follows its exact intent");
    let managed_bytes = fs::read(revision_path(downgrade_root.path(), operation_id, 1)).unwrap();
    let none_v1 = downgrade
        .advance(
            operation_id,
            managed_v1.head().clone(),
            &CheckpointPayloadV1::initial(&intent).unwrap(),
        )
        .expect("historical V1 may return managed state to None");
    let none_bytes = fs::read(revision_path(downgrade_root.path(), operation_id, 2)).unwrap();
    let denied_promotion = BrokerCheckpointV2::new(
        initial.clone(),
        SupervisionCheckpointV1::new(
            namespace,
            Vec::new(),
            vec![SupervisionOperationV1::Decision {
                request: request.clone(),
                response: None,
            }],
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        downgrade
            .advance_supervised(operation_id, none_v1.head().clone(), &denied_promotion)
            .is_err(),
        "V1 managed history cannot be erased by returning to None before V2 promotion"
    );
    assert_eq!(
        downgrade.load(operation_id).unwrap().unwrap().head(),
        none_v1.head()
    );
    assert_eq!(
        fs::read(revision_path(downgrade_root.path(), operation_id, 1)).unwrap(),
        managed_bytes
    );
    assert_eq!(
        fs::read(revision_path(downgrade_root.path(), operation_id, 2)).unwrap(),
        none_bytes
    );
    assert_eq!(
        fs::read(revision_zero_path(downgrade_root.path(), operation_id)).unwrap(),
        v1_rev0_bytes
    );
    drop(downgrade);
    let downgrade_path = downgrade_root.path().canonicalize().unwrap();
    let recovery = StopRecoveryReaderV1::open_existing(
        &Directory::open_existing(&downgrade_path).unwrap(),
        scope.clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap()
    .expect("the historical V1 job already has a journal namespace")
    .scan();
    assert!(
        recovery.incomplete(),
        "managed V1 history that returned to None is incomplete for STOP recovery"
    );
    let historical_job = recovery
        .jobs()
        .iter()
        .find(|job| job.operation_id() == operation_id)
        .expect("the validated historical head remains visible");
    assert_eq!(historical_job.head(), none_v1.head());
    assert!(
        historical_job.stops().is_empty(),
        "a historical managed checkpoint without an acknowledged Arm yields no stop target"
    );
    drop(recovery);

    // GEL3 permits the runner's prepared managed projection while Admission is
    // pending; all worker/helper/owned authority still requires acknowledged
    // operations. Keep its exact admission and Store-prepared worker facts.
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        Vec::new(),
        operations.clone(),
    )
    .head()
    .clone();

    let manifest = worker.launch.workspace_manifest_digest.clone();
    let import_permission = HelperPermissionV1::Admission {
        receipt: admitted.receipt.clone(),
    };
    let mut helpers = Vec::new();
    let (import_id, import_arm, import_terminal, _import_cleanup) = run_helper_lifecycle(
        &journal,
        &store,
        operation_id,
        &mut head,
        &runner,
        namespace,
        &mut helpers,
        &mut operations,
        &intent,
        &worker,
        &authority,
        HelperPurposeV1::Import,
        import_permission,
        None,
    );
    let verification = ImportVerificationReportV1 {
        import_helper_id: import_id.to_string(),
        expected_manifest_digest: manifest.clone(),
        observed_manifest_digest: manifest.clone(),
    };
    let verify_permission = HelperPermissionV1::ImportVerification {
        import_helper_id: import_id.to_string(),
        import_arm: import_arm.receipt.clone(),
        import_terminal: import_terminal.clone(),
        manifest_digest: manifest.clone(),
    };
    let (_verify_id, _verify_arm, _verify_terminal, _verify_cleanup) = run_helper_lifecycle(
        &journal,
        &store,
        operation_id,
        &mut head,
        &runner,
        namespace,
        &mut helpers,
        &mut operations,
        &intent,
        &worker,
        &authority,
        HelperPurposeV1::ImportVerification,
        verify_permission,
        Some(verification),
    );

    let create = worker_mutation(&worker, WorkerTransitionV1::CreateIssued);
    let create_index = operations.len();
    operations.push(SupervisionOperationV1::WorkerMutation {
        request: create.clone(),
        response: None,
    });
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();
    let create_result = store.mutate_worker(&create).unwrap();
    worker = create_result.worker.clone();
    operations[create_index] = SupervisionOperationV1::WorkerMutation {
        request: create,
        response: Some(create_result),
    };
    runner = managed_runner_checkpoint(
        &intent,
        &request,
        &admission.admission,
        &worker,
        &authority,
        None,
        &decision,
    );
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();

    let created_inspection = synthetic_worker_inspection(&worker);
    let created = worker_mutation(
        &worker,
        WorkerTransitionV1::Created {
            inspection: created_inspection.clone(),
        },
    );
    let created_index = operations.len();
    operations.push(SupervisionOperationV1::WorkerMutation {
        request: created.clone(),
        response: None,
    });
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();
    let created_result = store.mutate_worker(&created).unwrap();
    worker = created_result.worker.clone();
    operations[created_index] = SupervisionOperationV1::WorkerMutation {
        request: created,
        response: Some(created_result),
    };
    let worker_fingerprint = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let worker_owned = synthetic_owned_snapshot(
        &authority,
        Uuid::parse_str(&request.session_id.clone().unwrap()).unwrap(),
        operation_id,
        &created_inspection,
        worker_fingerprint,
    );
    runner = managed_runner_checkpoint(
        &intent,
        &request,
        &admission.admission,
        &worker,
        &authority,
        Some(&worker_owned),
        &decision,
    );
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();

    let worker_arm = worker_arm_request(&worker, &intent.intent_hash().unwrap(), &authority);
    let worker_arm_index = operations.len();
    operations.push(SupervisionOperationV1::Arm {
        parent: worker.clone(),
        owned: worker_owned.clone(),
        request: worker_arm.clone(),
        response: None,
    });
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();
    let worker_arm_result = store.arm_execution(&worker_arm).unwrap();
    operations[worker_arm_index] = SupervisionOperationV1::Arm {
        parent: worker.clone(),
        owned: worker_owned.clone(),
        request: worker_arm.clone(),
        response: Some(worker_arm_result.clone()),
    };
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();

    let worker_claim = ArmedStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: worker_arm_result.receipt.clone(),
        clock_check: HostClockSampleV1 {
            version: 1,
            method: worker_arm.clock.method,
            boot_id: worker_arm.clock.boot_id.clone(),
            continuous_ns_decimal: "100000000001".into(),
            observed_utc: Some("2026-10-30T12:00:00Z".into()),
        },
    };
    let worker_claim_index = operations.len();
    operations.push(SupervisionOperationV1::WorkerClaim {
        request: worker_claim.clone(),
        response: None,
    });
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();
    let worker_claim_result = store.claim_armed_start(&worker_claim).unwrap();
    worker = worker_claim_result.worker.clone();
    operations[worker_claim_index] = SupervisionOperationV1::WorkerClaim {
        request: worker_claim,
        response: Some(worker_claim_result),
    };
    runner = managed_runner_checkpoint(
        &intent,
        &request,
        &admission.admission,
        &worker,
        &authority,
        Some(&worker_owned),
        &decision,
    );
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();

    let observe_running = worker_mutation(
        &worker,
        WorkerTransitionV1::Observe {
            observation: parent_observation(&worker, RuntimeStatusV1::Running, true, None, None),
        },
    );
    let observe_running_index = operations.len();
    operations.push(SupervisionOperationV1::WorkerMutation {
        request: observe_running.clone(),
        response: None,
    });
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();
    let running_result = store.mutate_worker(&observe_running).unwrap();
    worker = running_result.worker.clone();
    operations[observe_running_index] = SupervisionOperationV1::WorkerMutation {
        request: observe_running,
        response: Some(running_result),
    };
    runner = managed_runner_checkpoint(
        &intent,
        &request,
        &admission.admission,
        &worker,
        &authority,
        Some(&worker_owned),
        &decision,
    );
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();

    let stop = worker_mutation(&worker, WorkerTransitionV1::Stop);
    let stop_index = operations.len();
    operations.push(SupervisionOperationV1::WorkerMutation {
        request: stop.clone(),
        response: None,
    });
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();
    let stop_result = store.mutate_worker(&stop).unwrap();
    worker = stop_result.worker.clone();
    operations[stop_index] = SupervisionOperationV1::WorkerMutation {
        request: stop,
        response: Some(stop_result),
    };
    runner = managed_runner_checkpoint(
        &intent,
        &request,
        &admission.admission,
        &worker,
        &authority,
        Some(&worker_owned),
        &decision,
    );
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();

    let observe_exit = worker_mutation(
        &worker,
        WorkerTransitionV1::Observe {
            observation: parent_observation(
                &worker,
                RuntimeStatusV1::Exited,
                false,
                Some("2026-10-30T12:00:03Z"),
                Some(0),
            ),
        },
    );
    let observe_exit_index = operations.len();
    operations.push(SupervisionOperationV1::WorkerMutation {
        request: observe_exit.clone(),
        response: None,
    });
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();
    let exit_result = store.mutate_worker(&observe_exit).unwrap();
    worker = exit_result.worker.clone();
    assert_eq!(worker.state, WorkerStateV1::Terminal);
    operations[observe_exit_index] = SupervisionOperationV1::WorkerMutation {
        request: observe_exit,
        response: Some(exit_result),
    };
    runner = managed_runner_checkpoint(
        &intent,
        &request,
        &admission.admission,
        &worker,
        &authority,
        Some(&worker_owned),
        &decision,
    );
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();

    let final_scan_permission = HelperPermissionV1::Admission {
        receipt: admitted.receipt.clone(),
    };
    let _ = run_helper_lifecycle(
        &journal,
        &store,
        operation_id,
        &mut head,
        &runner,
        namespace,
        &mut helpers,
        &mut operations,
        &intent,
        &worker,
        &authority,
        HelperPurposeV1::FinalScan,
        final_scan_permission,
        None,
    );

    let export = ExportRequestV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        destination_ref: Uuid::new_v4().to_string(),
        destination_intent_hash: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            .into(),
    };
    let export_index = operations.len();
    operations.push(SupervisionOperationV1::Export {
        request: export.clone(),
        response: None,
    });
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();
    let export_receipt = store.request_export(&export).unwrap();
    operations[export_index] = SupervisionOperationV1::Export {
        request: export.clone(),
        response: Some(export_receipt.clone()),
    };
    head = append_v2(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
    )
    .head()
    .clone();
    let export_permission = HelperPermissionV1::Export {
        receipt: export_receipt,
        destination_ref: export.destination_ref.clone(),
        destination_intent_hash: export.destination_intent_hash.clone(),
    };
    let _ = run_helper_lifecycle(
        &journal,
        &store,
        operation_id,
        &mut head,
        &runner,
        namespace,
        &mut helpers,
        &mut operations,
        &intent,
        &worker,
        &authority,
        HelperPurposeV1::Export,
        export_permission,
        None,
    );

    let saved = journal.load(operation_id).unwrap().unwrap();
    let final_checkpoint = saved.broker_checkpoint_v2().unwrap();
    assert_eq!(saved.head(), &head);
    let kinds: std::collections::BTreeSet<String> = serde_json::from_slice::<Value>(
        &final_checkpoint.canonical_bytes().unwrap(),
    )
    .unwrap()["supervision"]["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|operation| operation["kind"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        kinds,
        [
            "admission",
            "arm",
            "decision",
            "export",
            "helper_claim",
            "helper_cleanup",
            "helper_observation",
            "worker_claim",
            "worker_mutation"
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    assert_eq!(final_checkpoint.supervision().helpers().len(), 4);
    assert!(store.integrity().unwrap().valid);

    // A durable cancel closes the previously acknowledged export grant. An
    // otherwise valid new helper claim must use an export request born after
    // that cancellation, even though the earlier grant remains in the journal.
    let cancel1_boundary = operations.len();
    let cancel = LocalCommandV1::new(
        operation_id,
        &intent.intent_hash().unwrap(),
        Uuid::new_v4(),
        LocalCommandRequestV1::CancelRequested,
    )
    .unwrap();
    let commands = vec![cancel.clone()];
    head = append_v3(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
        &commands,
    )
    .head()
    .clone();
    let stale_receipt = operations
        .iter()
        .find_map(|operation| match operation {
            SupervisionOperationV1::Export {
                response: Some(receipt),
                ..
            } => Some(receipt.clone()),
            _ => None,
        })
        .expect("the previous export request has a real Store receipt");
    let stale_permission = HelperPermissionV1::Export {
        receipt: stale_receipt,
        destination_ref: export.destination_ref.clone(),
        destination_intent_hash: export.destination_intent_hash.clone(),
    };
    let stale_claim = append_export_helper_claim(
        &journal,
        &store,
        operation_id,
        &mut head,
        &runner,
        namespace,
        &mut helpers,
        &mut operations,
        &commands,
        &intent,
        &worker,
        &authority,
        stale_permission,
    );

    // This positive control uses a distinct, real Store ExportRequest receipt
    // recorded strictly after the cancel command.
    let fresh_export = ExportRequestV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        destination_ref: Uuid::new_v4().to_string(),
        destination_intent_hash: "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .into(),
    };
    let fresh_export_index = operations.len();
    operations.push(SupervisionOperationV1::Export {
        request: fresh_export.clone(),
        response: None,
    });
    head = append_v3(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
        &commands,
    )
    .head()
    .clone();
    let fresh_export_receipt = store.request_export(&fresh_export).unwrap();
    operations[fresh_export_index] = SupervisionOperationV1::Export {
        request: fresh_export.clone(),
        response: Some(fresh_export_receipt.clone()),
    };
    head = append_v3(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
        &commands,
    )
    .head()
    .clone();
    let fresh_permission = HelperPermissionV1::Export {
        receipt: fresh_export_receipt.clone(),
        destination_ref: fresh_export.destination_ref.clone(),
        destination_intent_hash: fresh_export.destination_intent_hash.clone(),
    };
    append_export_helper_claim(
        &journal,
        &store,
        operation_id,
        &mut head,
        &runner,
        namespace,
        &mut helpers,
        &mut operations,
        &commands,
        &intent,
        &worker,
        &authority,
        fresh_permission,
    )
    .expect("a new Store export receipt after cancellation permits a helper claim");
    assert!(
        matches!(
            stale_claim,
            Err((_, agentsandbox::broker::journal::JournalError::Conflict))
        ),
        "a helper claim under an export grant from before cancellation must conflict"
    );

    // A second durable cancellation invalidates even a grant born after the
    // first one. An ExportRequest co-born with that second boundary is not a
    // post-cancel grant and cannot authorize a helper.
    let cancel2_boundary = operations.len();
    let cancel2 = LocalCommandV1::new(
        operation_id,
        &intent.intent_hash().unwrap(),
        Uuid::new_v4(),
        LocalCommandRequestV1::CancelRequested,
    )
    .unwrap();
    let commands2 = vec![cancel.clone(), cancel2.clone()];
    let co_born = ExportRequestV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        destination_ref: Uuid::new_v4().to_string(),
        destination_intent_hash: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            .into(),
    };
    let co_born_index = operations.len();
    operations.push(SupervisionOperationV1::Export {
        request: co_born,
        response: None,
    });
    assert!(matches!(
        try_append_v3(
            &journal,
            operation_id,
            &head,
            &runner,
            namespace,
            helpers.clone(),
            operations.clone(),
            &commands2,
        ),
        Err(agentsandbox::broker::journal::JournalError::Conflict)
    ));
    operations.truncate(co_born_index);
    head = append_v3(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
        &commands2,
    )
    .head()
    .clone();
    let old_post_first_cancel_permission = HelperPermissionV1::Export {
        receipt: fresh_export_receipt,
        destination_ref: fresh_export.destination_ref,
        destination_intent_hash: fresh_export.destination_intent_hash,
    };
    let second_stale = append_export_helper_claim(
        &journal,
        &store,
        operation_id,
        &mut head,
        &runner,
        namespace,
        &mut helpers,
        &mut operations,
        &commands2,
        &intent,
        &worker,
        &authority,
        old_post_first_cancel_permission,
    );
    assert!(matches!(
        second_stale,
        Err((
            "helper-preparation",
            agentsandbox::broker::journal::JournalError::Conflict
        ))
    ));

    let post_last_cancel_export = ExportRequestV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        destination_ref: Uuid::new_v4().to_string(),
        destination_intent_hash: "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
            .into(),
    };
    let post_last_index = operations.len();
    operations.push(SupervisionOperationV1::Export {
        request: post_last_cancel_export.clone(),
        response: None,
    });
    head = append_v3(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
        &commands2,
    )
    .head()
    .clone();
    let post_last_receipt = store.request_export(&post_last_cancel_export).unwrap();
    operations[post_last_index] = SupervisionOperationV1::Export {
        request: post_last_cancel_export.clone(),
        response: Some(post_last_receipt.clone()),
    };
    head = append_v3(
        &journal,
        operation_id,
        &head,
        &runner,
        namespace,
        helpers.clone(),
        operations.clone(),
        &commands2,
    )
    .head()
    .clone();
    let post_last_permission = HelperPermissionV1::Export {
        receipt: post_last_receipt,
        destination_ref: post_last_cancel_export.destination_ref,
        destination_intent_hash: post_last_cancel_export.destination_intent_hash,
    };
    append_export_helper_claim(
        &journal,
        &store,
        operation_id,
        &mut head,
        &runner,
        namespace,
        &mut helpers,
        &mut operations,
        &commands2,
        &intent,
        &worker,
        &authority,
        post_last_permission,
    )
    .expect("only a grant born after the last Cancel authorizes Export");

    // STOP-only recovery projects the exact durable head and acknowledged
    // Arm receipts. The last Cancel applies only to Arms before its boundary.
    let expected_head = head.clone();
    let expected_stops = operations
        .iter()
        .enumerate()
        .filter_map(|(index, operation)| match operation {
            SupervisionOperationV1::Arm {
                owned,
                request,
                response: Some(response),
                ..
            } => Some((index, owned, request, response)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        expected_stops
            .iter()
            .any(|(index, _, _, _)| *index < cancel1_boundary)
    );
    assert!(
        expected_stops
            .iter()
            .any(|(index, _, _, _)| *index >= cancel2_boundary)
    );

    drop(journal);
    let root = journal_root.path().canonicalize().unwrap();
    let reader = StopRecoveryReaderV1::open_existing(
        &Directory::open_existing(&root).unwrap(),
        scope.clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap()
    .expect("existing job namespace is recovered without creating it");
    let recovered = reader.scan();
    assert!(!recovered.incomplete(), "the intact fixture is complete");
    assert!(recovered.issues().is_empty());
    let job = recovered
        .jobs()
        .iter()
        .find(|job| job.operation_id() == operation_id)
        .expect("the durable operation is retained");
    assert_eq!(job.head(), &expected_head);
    assert_eq!(
        job.cancel_command_ids(),
        &[cancel.command_id(), cancel2.command_id()]
    );
    assert_eq!(job.stops().len(), expected_stops.len());
    for (stop, (index, owned, arm, response)) in job.stops().iter().zip(expected_stops) {
        assert_eq!(
            serde_json::to_value(stop.arm()).unwrap(),
            serde_json::to_value(arm).unwrap()
        );
        assert_eq!(
            serde_json::to_value(stop.receipt()).unwrap(),
            serde_json::to_value(&response.receipt).unwrap()
        );
        assert_eq!(
            serde_json::to_value(stop.owned()).unwrap(),
            serde_json::to_value(owned).unwrap()
        );
        assert_eq!(
            serde_json::to_value(stop.authority()).unwrap(),
            serde_json::to_value(&authority).unwrap()
        );
        assert_eq!(
            stop.cancel_after_arm(),
            if index < cancel2_boundary {
                Some(cancel2.command_id())
            } else {
                None
            }
        );
    }
    drop(recovered);

    // A malformed sibling is isolated from the proven job, but it closes
    // admissions globally and the ordinary strict opener rejects the tree.
    let jobs_dir = revision_path(&root, operation_id, 0)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let bad_sibling = jobs_dir.join(Uuid::new_v4().to_string());
    symlink(root.join("missing-sibling-target"), &bad_sibling).unwrap();
    let reader = StopRecoveryReaderV1::open_existing(
        &Directory::open_existing(&root).unwrap(),
        scope.clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap()
    .expect("reader retains access to the existing namespace");
    let partial = reader.scan();
    assert!(partial.incomplete());
    assert_eq!(partial.jobs().len(), 1, "the bad sibling is not projected");
    assert_eq!(partial.jobs()[0].operation_id(), operation_id);
    assert_eq!(partial.jobs()[0].head(), &expected_head);
    drop(partial);
    let normal_open = Journal::open(
        &Directory::open_existing(&root).unwrap(),
        scope,
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    );
    assert!(
        normal_open.is_err(),
        "strict journal open rejects the sibling"
    );
}

mod support;
