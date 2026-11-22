//! Handler control-flow regression using a real Journal/Writer. Delivery is a
//! bounded test seam; this module makes no claim about IPC or runtime stopping.
use super::*;
use crate::{
    audit::Store,
    broker::{
        checkpoint::{
            BrokerCheckpointV2, DecisionAnswerV1, HelperPreparationV1, SupervisionCheckpointV1,
            SupervisionOperationV1,
        },
        intent::ExecutionIntentV1,
        journal::{BindingInputs, RevisionRefV1},
    },
    control::{
        AdmissionV1, ArmSubjectV1, ArmV1, DecisionPhaseV1, DecisionReceiptV1, HelperObservationV1,
        HelperPermissionV1, HelperPurposeV1, HelperStartClaimV1, HostClockMethodV1,
        HostClockSampleV1, LaunchSpecV1, NetworkV1, QuotaPolicyV1, RestartPolicyV1,
        RuntimeInspectionV1, RuntimeKindV1, RuntimeObservationV1, RuntimeStatusV1,
        SecurityPresetV1, SupervisedAdmissionV1, SupervisionProtocolV1, WorkerMutationV1,
        WorkerTransitionV1, helper_execution_policy_v1,
    },
    model::{DecisionRequest, Outcome},
    policy::{self, Policy},
    redaction::Redactor,
    runner::CheckpointPayloadV1,
    runtime::{OwnedContainerSnapshot, RuntimeAuthority},
};
use std::{
    cell::RefCell,
    fs,
    ops::Deref,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
};

#[cfg(test)]
#[path = "checkpoint_source_tests.rs"]
mod checkpoint_source_tests;
#[path = "cold_audit_tests.rs"]
pub(super) mod cold_audit_tests;
#[path = "stop_auth_tests.rs"]
mod stop_auth_tests;

/// Production aliases ExecutionEnforcerV1 directly. A probe-only Service cannot
/// access a native client: leaving the explicitly scoped seam is a test bug.
pub(super) enum TestEnforcer {
    Native(ExecutionEnforcerV1),
    CancelDeliveryProbe,
}
impl Deref for TestEnforcer {
    type Target = ExecutionEnforcerV1;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Native(client) => client,
            Self::CancelDeliveryProbe => {
                panic!("cancel control-flow fixture escaped its scoped delivery seam")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Attempt {
    operation: Uuid,
    generation: u64,
}
#[derive(Default)]
struct Probe {
    attempts: Vec<Attempt>,
    overflow: bool,
}
thread_local! {
    static DELIVERY: RefCell<Option<Probe>> = const { RefCell::new(None) };
}

pub(super) fn delivery(operation: Uuid, generation: u64) -> Option<Result<(), BrokerErrorCodeV1>> {
    DELIVERY.with(|cell| {
        let mut state = cell.borrow_mut();
        let probe = state.as_mut()?;
        if probe.attempts.len() >= 2 {
            probe.overflow = true;
            return Some(Err(BrokerErrorCodeV1::Busy));
        }
        probe.attempts.push(Attempt {
            operation,
            generation,
        });
        Some(if probe.attempts.len() == 1 {
            Err(BrokerErrorCodeV1::NeedsReconciliation)
        } else {
            Ok(())
        })
    })
}
struct DeliveryScope;
impl DeliveryScope {
    fn begin() -> Self {
        DELIVERY.with(|cell| {
            assert!(
                cell.borrow().is_none(),
                "one delivery probe per test thread"
            );
            *cell.borrow_mut() = Some(Probe::default());
        });
        Self
    }
    fn attempts(&self) -> Vec<Attempt> {
        DELIVERY.with(|cell| cell.borrow().as_ref().unwrap().attempts.clone())
    }
    fn overflowed(&self) -> bool {
        DELIVERY.with(|cell| cell.borrow().as_ref().unwrap().overflow)
    }
}
impl Drop for DeliveryScope {
    fn drop(&mut self) {
        DELIVERY.with(|cell| *cell.borrow_mut() = None);
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    service: Service,
    jobs: Arc<ProfileJobs>,
    journal: Journal,
    writer: WriterClient,
    operation: Uuid,
    manual_blob: Option<PathBuf>,
    manual_source: Option<PathBuf>,
}
impl Fixture {
    fn new() -> Self {
        Self::with_manual_arm(false)
    }
    fn with_manual_arm(manual: bool) -> Self {
        Self::with_manual_arm_and_identity(manual, None)
    }
    fn with_manual_arm_and_identity(manual: bool, identity: Option<(&str, Uuid)>) -> Self {
        Self::with_manual_arm_count_and_identity(usize::from(manual), identity)
    }
    fn with_manual_arm_count(arm_count: usize) -> Self {
        Self::with_manual_arm_count_and_identity(arm_count, None)
    }
    fn with_manual_arm_count_and_identity(
        arm_count: usize,
        identity: Option<(&str, Uuid)>,
    ) -> Self {
        Self::with_manual_binding_and_arm_count_and_identity(arm_count > 0, arm_count, identity)
    }
    fn with_manual_binding_and_identity(identity: (&str, Uuid)) -> Self {
        Self::with_manual_binding_and_arm_count_and_identity(true, 0, Some(identity))
    }
    fn with_manual_binding_and_arm_count_and_identity(
        manual: bool,
        arm_count: usize,
        identity: Option<(&str, Uuid)>,
    ) -> Self {
        assert!(
            arm_count <= 2,
            "this fixture qualifies only zero, one or two Arms"
        );
        assert!(
            manual || arm_count == 0,
            "Arm fixtures require their manual binding"
        );
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let base = temp.path().canonicalize().unwrap();
        for name in [
            "root",
            "configuration",
            "sources",
            "exports",
            "root/journals",
        ] {
            fs::create_dir(base.join(name)).unwrap();
            fs::set_permissions(base.join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let installation = Uuid::from_u128(0x11111111111141118111111111111111);
        let store = (arm_count > 0).then(|| {
            let store =
                Store::open(&base.join("audit.sqlite"), Redactor::new(vec![]), 1000).unwrap();
            store
                .migrate_control_v3(&base.join("migration-backup.sqlite"))
                .unwrap();
            store
        });
        let namespace = store.as_ref().map_or_else(
            || Uuid::from_u128(0x22222222222242228222222222222222),
            |store| {
                Uuid::parse_str(
                    store
                        .supervision_state()
                        .unwrap()
                        .control_namespace_uuid
                        .as_deref()
                        .unwrap(),
                )
                .unwrap()
            },
        );
        let (namespace, gateway) = identity
            .map(|(gateway, namespace)| (namespace, gateway))
            .unwrap_or((namespace, "http://127.0.0.1:8787"));
        let scope = JournalScopeV1::new(
            installation,
            "tenant-cancel-retry-test",
            gateway,
            namespace,
            "cancel_retry_test",
        )
        .unwrap();
        // Real private configuration parsing, without a gateway call or a
        // credential read. Authentication is outside this private-handler test.
        let config = base.join("registry.json");
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "version": 1,
                "installation_id": installation,
                "root": base.join("root"),
                "max_enforcer_slots": 1,
                "profiles": [{
                    "id": scope.profile_id(),
                    "gateway": scope.gateway_origin(),
                    "tenant_id": scope.tenant_id(),
                    "control_namespace_uuid": namespace,
                    "config_dir": base.join("configuration"),
                    "runner_token_file": base.join("configuration/never-read.token"),
                    "runtime": "docker",
                    "runtime_program": "/usr/bin/false",
                    "runtime_endpoint": "unix:///never-used",
                    "image": "never-used:test",
                    "policy_profile": if manual { "observe" } else { "strict" },
                    "network": if manual { "bridge" } else { "none" },
                    "allow_rootful_runtime": false,
                    "max_timeout_seconds": 30,
                    "allow_capture_output": false,
                    "managed_clients": false,
                    "source_roots": [base.join("sources")],
                    "export_roots": [base.join("exports")]
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        let registry = Arc::new(Registry::load(&config).unwrap());
        let journal_root = base.join("root/journals").join(scope.profile_id());
        fs::create_dir(&journal_root).unwrap();
        fs::set_permissions(&journal_root, fs::Permissions::from_mode(0o700)).unwrap();
        let journal = Journal::open(
            &Directory::open_existing(&journal_root).unwrap(),
            scope.clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new()).unwrap(),
        )
        .unwrap();
        let operation = Uuid::from_u128(0x33333333333343338333333333333333);
        let manual_source = manual.then(|| {
            let path = base.join("manual-fixture.env");
            fs::write(&path, b"FIXTURE_KEY=synthetic-manual-value-123\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            path
        });
        let source = manual_source
            .as_deref()
            .map(|path| PrivateFile::open(path, 64 * 1024).unwrap());
        let mut plan = plan(operation);
        plan["options"]["gateway"] = json!(gateway);
        if let Some(path) = &manual_source {
            plan["options"]["secret_env_file"] = json!(path);
            plan["options"]["profile"] = json!("observe");
            plan["options"]["network"] = json!("bridge");
            plan["request"]["profile"] = json!("observe");
        }
        let intent =
            ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap();
        let ticket = match journal
            .reserve(operation, br#"{"cancel_retry_test":true}"#)
            .unwrap()
        {
            ReserveResult::New(ticket) => ticket,
            other => panic!("fresh real journal reservation required: {other:?}"),
        };
        let accepted = journal
            .accept(
                ticket,
                &intent,
                source
                    .as_ref()
                    .map_or(BindingInputs::None, |source| BindingInputs::Manual {
                        source,
                    }),
                &CheckpointPayloadV1::initial(&intent).unwrap(),
            )
            .unwrap();
        if let Some(store) = &store {
            prepare_manual_arm(
                &journal,
                store,
                &intent,
                plan,
                accepted.head(),
                accepted.binding_manifest(),
                arm_count,
            );
        }
        let manual_blob = accepted
            .binding_source()
            .map(|source| source.path().to_owned());
        drop(accepted);
        let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(10)).unwrap();
        let jobs = Arc::new(ProfileJobs::new());
        jobs.writer
            .set(ProfileWriterCapabilities::from_full(writer.clone()))
            .unwrap();
        let service = Service {
            full_owners: FullOwners::new(registry.data.max_enforcer_slots as usize).unwrap(),
            cold_owners: retained_audit::ColdOwners::new(registry.data.max_enforcer_slots as usize)
                .unwrap(),
            registry,
            root: Directory::open_existing(&base.join("root")).unwrap(),
            material: Arc::new(Material {
                literals: Vec::new(),
                files: Vec::new(),
                parents: Vec::new(),
                exclusions: FsExclusions::new(),
                complete: true,
            }),
            profiles: BTreeMap::from([(scope.profile_id().into(), Arc::clone(&jobs))]),
            enforcer: Some(TestEnforcer::CancelDeliveryProbe),
            stop_enforcer: None,
            blocking: Arc::new(Semaphore::new(1)),
            preparation: Arc::new(Semaphore::new(1)),
            execution: Arc::new(Semaphore::new(1)),
            stopping: Arc::new(AtomicBool::new(false)),
            fenced: Arc::new(AtomicBool::new(false)),
        };
        Self {
            _temp: temp,
            service,
            jobs,
            journal,
            writer,
            operation,
            manual_blob,
            manual_source,
        }
    }
}

fn plan(operation: Uuid) -> Value {
    let mut plan: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/broker_intent-plan-v3.json"
    )))
    .unwrap();
    plan["version"] = json!(1);
    plan["operation_id"] = json!(operation);
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

fn prepare_manual_arm(
    journal: &Journal,
    store: &Store,
    intent: &ExecutionIntentV1,
    mut plan: Value,
    initial_head: &RevisionRefV1,
    binding: &crate::broker::journal::BindingManifestV1,
    arm_count: usize,
) {
    let request: DecisionRequest = serde_json::from_value(plan["request"].clone()).unwrap();
    let namespace = journal.scope().control_namespace_uuid();
    let initial = CheckpointPayloadV1::initial(intent).unwrap();
    let mut head = initial_head.clone();
    let append = |head: &mut RevisionRefV1,
                  runner: &CheckpointPayloadV1,
                  helpers: &[HelperPreparationV1],
                  operations: &[SupervisionOperationV1]| {
        let next = BrokerCheckpointV2::new(
            runner.clone(),
            SupervisionCheckpointV1::new(namespace, helpers.to_vec(), operations.to_vec()).unwrap(),
        )
        .unwrap();
        *head = journal
            .advance_supervised(intent.operation_id(), head.clone(), &next)
            .unwrap()
            .head()
            .clone();
    };
    let policy = Policy::default();
    let stored = store
        .decision(
            &request,
            Some((policy::evaluate(&policy, &request), json!(null))),
        )
        .unwrap()
        .unwrap();
    let decision = stored.event.decision.unwrap();
    assert_eq!(decision.outcome, Outcome::Allow);
    let receipt = DecisionReceiptV1 {
        version: 1,
        request_id: request.request_id.clone(),
        request_hash: policy::request_hash(&request),
        phase: DecisionPhaseV1::Initial,
        audit_seq: stored.seq,
        audit_hash: stored.hash,
    };
    let mut operations = vec![SupervisionOperationV1::Decision {
        request: request.clone(),
        response: None,
    }];
    append(&mut head, &initial, &[], &operations);
    operations[0] = SupervisionOperationV1::Decision {
        request: request.clone(),
        response: Some(DecisionAnswerV1::new(decision.clone(), receipt.clone()).unwrap()),
    };
    append(&mut head, &initial, &[], &operations);
    let admission = SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: intent.intent_hash().unwrap(),
        admission: AdmissionV1 {
            version: 1,
            operation_id: intent.operation_id().to_string(),
            receipt,
            decision_request: request.clone(),
            launch: LaunchSpecV1 {
                version: 1,
                session_id: intent.session().to_string(),
                command: request.command.clone(),
                profile: request.profile.clone(),
                runtime_kind: RuntimeKindV1::Docker,
                authority_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
                image_ref: "agentsandbox-worker:local".into(),
                image_id: format!("sha256:{}", "a".repeat(64)),
                network: NetworkV1::Bridge,
                cpu_millis: 2000,
                memory_bytes: 1_073_741_824,
                pids_limit: 256,
                timeout_seconds: 600,
                security_preset: SecurityPresetV1::FixedSandboxV2,
                restart_policy: RestartPolicyV1::No,
                workspace_manifest_digest: crate::control::content_hash(
                    "ai-guard:workspace-manifest:v1",
                    &serde_json::Map::<String, Value>::new(),
                )
                .unwrap(),
                workspace_import_policy_version: 1,
                provider: "generic".into(),
                capture_output: false,
                credential_binding_id: binding.binding_id().map(|id| id.to_string()),
                credential_names: binding.names().to_vec(),
                provider_resolution: None,
                egress: None,
            },
        },
    };
    operations.push(SupervisionOperationV1::Admission {
        request: admission.clone(),
        response: None,
    });
    append(&mut head, &initial, &[], &operations);
    let admitted = store
        .admit_supervised(
            &admission,
            &policy,
            decision.clone(),
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .unwrap();
    let worker = admitted.worker.clone().unwrap();
    operations[1] = SupervisionOperationV1::Admission {
        request: admission.clone(),
        response: Some(admitted.clone()),
    };
    append(&mut head, &initial, &[], &operations);
    let prepare = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition: WorkerTransitionV1::Prepare,
    };
    operations.push(SupervisionOperationV1::WorkerMutation {
        request: prepare.clone(),
        response: None,
    });
    append(&mut head, &initial, &[], &operations);
    let prepared = store.mutate_worker(&prepare).unwrap();
    let worker = prepared.worker.clone();
    operations[2] = SupervisionOperationV1::WorkerMutation {
        request: prepare,
        response: Some(prepared),
    };
    append(&mut head, &initial, &[], &operations);
    let authority: RuntimeAuthority = serde_json::from_value(json!({
        "authority_id":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", "kind":"docker", "endpoint":"unix:///private/fixture.sock", "engine_id":null, "podman":null,
        "invocation":{"policy_version":2,"kind":"docker","approved_entry":"/usr/bin/docker","canonical_program":"/usr/bin/docker",
        "file_identity":{"device":1,"inode":2,"owner":1000,"group":1000,"mode":493,"length":10,"sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"environment":{},"cwd":"/private/fixture"}
    })).unwrap();
    plan["managed"] = json!({
        "tenant_id":journal.scope().tenant_id(), "gateway_origin":journal.scope().gateway_origin(), "config_root":"/private/control",
        "binding":authority, "authority_file":"runtime-authority.json", "admission":admission.admission,
        "worker":worker, "owned":null, "pending":null, "uncertain":[], "lock_identity":null,
        "before":{}, "import":null, "decision":decision, "output_event":null,"output_acknowledged":false,"result":null,
        "archive_name":"worker.tar","archive_digest":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "credential_digest":binding.handoff_sha(),"workspace_control_exclusion_version":1,"invocation_event":null
    });
    let runner =
        CheckpointPayloadV1::from_plan_bytes(intent, &serde_json::to_vec(&plan).unwrap()).unwrap();
    let helper_id = Uuid::new_v4();
    let helper = helper_execution_policy_v1(&worker, HelperPurposeV1::Import, helper_id).unwrap();
    let permission = HelperPermissionV1::Admission {
        receipt: admitted.receipt,
    };
    let mut helpers =
        vec![HelperPreparationV1::new(helper.clone(), permission.clone(), None).unwrap()];
    append(&mut head, &runner, &helpers, &operations);
    let inspection = RuntimeInspectionV1 {
        authority_id: helper.authority_id.clone(),
        runtime_kind: helper.runtime_kind,
        container_id: format!("{:064x}", helper_id.as_u128()),
        stable_name: helper.stable_name.clone(),
        volume_name: helper.volume_name.clone(),
        image_id: helper.image_id.clone(),
        spec_hash: helper.spec_hash().unwrap(),
        cpu_millis: helper.cpu_millis,
        memory_bytes: helper.memory_bytes,
        pids_limit: helper.pids_limit,
        network: helper.network,
        security_preset: helper.security_preset,
        restart_policy: helper.restart_policy,
    };
    let fingerprint = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let arm = ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        intent_hash: intent.intent_hash().unwrap(),
        subject: ArmSubjectV1::Helper {
            helper_id: helper_id.to_string(),
            purpose: HelperPurposeV1::Import,
            policy_version: 1,
            permission: Box::new(permission.clone()),
        },
        inspection: inspection.clone(),
        authority_binding_hash: authority.fingerprint(),
        configuration_fingerprint_version: helper
            .security_preset
            .configuration_fingerprint_version(),
        configuration_fingerprint: fingerprint.into(),
        clock: HostClockSampleV1 {
            version: 1,
            method: HostClockMethodV1::DarwinMachContinuousV1,
            boot_id: "11111111-1111-4111-8111-111111111111".into(),
            continuous_ns_decimal: "100000000000".into(),
            observed_utc: Some("2026-10-30T12:00:00Z".into()),
        },
        timeout_seconds: 120,
        deadline_ns_decimal: "220000000000".into(),
    };
    let owned: OwnedContainerSnapshot = serde_json::from_value(json!({
        "handle":{"authority_fingerprint":authority.fingerprint(),"session":intent.session(),"operation":helper_id,"id":inspection.container_id,"name":inspection.stable_name,"configuration_fingerprint_version":arm.configuration_fingerprint_version},
        "observation":{"id":inspection.container_id,"name":inspection.stable_name,"image_id":inspection.image_id,"restart_count":0,
        "process":{"status":"running","running":true,"started_at":"2026-10-30T12:00:00Z","finished_at":null,"exit_code":null,"oom_killed":false},
        "allocation":{"cpu":{"quota":u64::from(inspection.cpu_millis)*100,"period":100000},"memory_bytes":inspection.memory_bytes,"pids_limit":inspection.pids_limit},
        "configuration_fingerprint":fingerprint,"configuration_fingerprint_version":arm.configuration_fingerprint_version,"observed_at":"2026-10-30T12:00:00Z","diagnostics":[]}
    })).unwrap();
    helpers[0] = HelperPreparationV1::new(helper, permission, Some(owned.clone())).unwrap();
    let index = operations.len();
    operations.push(SupervisionOperationV1::Arm {
        parent: worker.clone(),
        owned: owned.clone(),
        request: arm.clone(),
        response: None,
    });
    append(&mut head, &runner, &helpers, &operations);
    let armed = store.arm_execution(&arm).unwrap();
    operations[index] = SupervisionOperationV1::Arm {
        parent: worker.clone(),
        owned,
        request: arm.clone(),
        response: Some(armed.clone()),
    };
    append(&mut head, &runner, &helpers, &operations);
    if arm_count == 2 {
        // Import is unique per parent attempt. The second proven target is an
        // ImportVerification helper, authorized by the first helper's actual
        // Store claim/terminal receipts. Runtime observations remain explicit
        // fixture data; no executable runtime or engine is made available.
        let claim = HelperStartClaimV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            helper_id: helper_id.to_string(),
            emitter_id: Uuid::new_v4().to_string(),
            arm_receipt: armed.receipt.clone(),
            clock_check: HostClockSampleV1 {
                continuous_ns_decimal: "100000000001".into(),
                ..arm.clock.clone()
            },
        };
        let claim_index = operations.len();
        operations.push(SupervisionOperationV1::HelperClaim {
            request: claim.clone(),
            response: None,
        });
        append(&mut head, &runner, &helpers, &operations);
        let claimed = store.claim_helper_start(&claim).unwrap();
        operations[claim_index] = SupervisionOperationV1::HelperClaim {
            request: claim,
            response: Some(claimed),
        };
        append(&mut head, &runner, &helpers, &operations);

        let observation = HelperObservationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            helper_id: helper_id.to_string(),
            arm_receipt: armed.receipt.clone(),
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
            verification: None,
        };
        let observation_index = operations.len();
        operations.push(SupervisionOperationV1::HelperObservation {
            request: observation.clone(),
            response: None,
        });
        append(&mut head, &runner, &helpers, &operations);
        let terminal = store.observe_helper(&observation).unwrap();
        operations[observation_index] = SupervisionOperationV1::HelperObservation {
            request: observation,
            response: Some(terminal.clone()),
        };
        append(&mut head, &runner, &helpers, &operations);

        let verify_id = Uuid::new_v4();
        let verify =
            helper_execution_policy_v1(&worker, HelperPurposeV1::ImportVerification, verify_id)
                .unwrap();
        let verify_permission = HelperPermissionV1::ImportVerification {
            import_helper_id: helper_id.to_string(),
            import_arm: armed.receipt.clone(),
            import_terminal: terminal,
            manifest_digest: worker.launch.workspace_manifest_digest.clone(),
        };
        let helper_index = helpers.len();
        helpers.push(
            HelperPreparationV1::new(verify.clone(), verify_permission.clone(), None).unwrap(),
        );
        append(&mut head, &runner, &helpers, &operations);
        let inspection = RuntimeInspectionV1 {
            authority_id: verify.authority_id.clone(),
            runtime_kind: verify.runtime_kind,
            container_id: format!("{:064x}", verify_id.as_u128()),
            stable_name: verify.stable_name.clone(),
            volume_name: verify.volume_name.clone(),
            image_id: verify.image_id.clone(),
            spec_hash: verify.spec_hash().unwrap(),
            cpu_millis: verify.cpu_millis,
            memory_bytes: verify.memory_bytes,
            pids_limit: verify.pids_limit,
            network: verify.network,
            security_preset: verify.security_preset,
            restart_policy: verify.restart_policy,
        };
        let verify_arm = ArmV1 {
            operation_id: Uuid::new_v4().to_string(),
            subject: ArmSubjectV1::Helper {
                helper_id: verify_id.to_string(),
                purpose: HelperPurposeV1::ImportVerification,
                policy_version: 1,
                permission: Box::new(verify_permission.clone()),
            },
            inspection: inspection.clone(),
            ..arm
        };
        let owned: OwnedContainerSnapshot = serde_json::from_value(json!({
            "handle":{"authority_fingerprint":authority.fingerprint(),"session":intent.session(),"operation":verify_id,"id":inspection.container_id,"name":inspection.stable_name,"configuration_fingerprint_version":verify_arm.configuration_fingerprint_version},
            "observation":{"id":inspection.container_id,"name":inspection.stable_name,"image_id":inspection.image_id,"restart_count":0,
            "process":{"status":"running","running":true,"started_at":"2026-10-30T12:00:00Z","finished_at":null,"exit_code":null,"oom_killed":false},
            "allocation":{"cpu":{"quota":u64::from(inspection.cpu_millis)*100,"period":100000},"memory_bytes":inspection.memory_bytes,"pids_limit":inspection.pids_limit},
            "configuration_fingerprint":fingerprint,"configuration_fingerprint_version":verify_arm.configuration_fingerprint_version,"observed_at":"2026-10-30T12:00:00Z","diagnostics":[]}
        })).unwrap();
        helpers[helper_index] =
            HelperPreparationV1::new(verify, verify_permission, Some(owned.clone())).unwrap();
        let index = operations.len();
        operations.push(SupervisionOperationV1::Arm {
            parent: worker.clone(),
            owned: owned.clone(),
            request: verify_arm.clone(),
            response: None,
        });
        append(&mut head, &runner, &helpers, &operations);
        let armed = store.arm_execution(&verify_arm).unwrap();
        operations[index] = SupervisionOperationV1::Arm {
            parent: worker,
            owned,
            request: verify_arm,
            response: Some(armed),
        };
        append(&mut head, &runner, &helpers, &operations);
    }
    assert_eq!(
        journal.load(intent.operation_id()).unwrap().unwrap().head(),
        &head
    );
}

// This child owns only the explicitly named native recovery fixture. Its
// unavailable invocation data cannot execute a host engine or qualify Docker.
const RECOVERY_CHILD_ROOT: &str = "AI_GUARD_TEST_RECOVERY_ROOT";
#[test]
#[ignore = "internal native child, invoked only by the recovery transport parent"]
fn native_recovery_server_child() {
    let path = std::env::var_os(RECOVERY_CHILD_ROOT).expect("private recovery fixture root");
    let root = Directory::open_existing(Path::new(&path)).unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(crate::broker::enforcer::run_internal_recovery_enforcer(
            &root, 8,
        ))
        .unwrap();
}

struct RecoveryChild(std::process::Child);
impl Drop for RecoveryChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn recovery_child(root: &Directory, previous: Option<Uuid>) -> RecoveryChild {
    recovery_child_with_faults(root, previous, &RecoveryFaults::default())
}
#[derive(Default)]
struct RecoveryFaults<'a> {
    stage: Option<&'a str>,
    arm_id: Option<&'a str>,
    barrier: Option<(&'a str, &'a Path)>,
    started_marker: Option<&'a Path>,
    fault_marker: Option<&'a Path>,
    repeat_barrier: bool,
}
fn recovery_child_with_faults(
    root: &Directory,
    previous: Option<Uuid>,
    faults: &RecoveryFaults<'_>,
) -> RecoveryChild {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "broker::service::server::cancel_retry_tests::native_recovery_server_child",
            "--ignored",
        ])
        .env_clear()
        .env(RECOVERY_CHILD_ROOT, root.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(stage) = faults.stage {
        command.env("AI_GUARD_TEST_RECOVERY_FAULT", stage);
    }
    if let Some(arm) = faults.arm_id {
        command.env("AI_GUARD_TEST_RECOVERY_FAULT_ARM", arm);
    }
    if let Some((arm, root)) = faults.barrier {
        command
            .env("AI_GUARD_TEST_RECOVERY_BARRIER_ARM", arm)
            .env("AI_GUARD_TEST_RECOVERY_BARRIER_ROOT", root);
    }
    if let Some(marker) = faults.started_marker {
        command.env("AI_GUARD_TEST_RECOVERY_STARTED_MARKER", marker);
    }
    if let Some(marker) = faults.fault_marker {
        command.env("AI_GUARD_TEST_RECOVERY_FAULT_MARKER", marker);
    }
    if faults.repeat_barrier {
        command.env("AI_GUARD_TEST_RECOVERY_REPEAT_BARRIER", "1");
    }
    let child = command.spawn().unwrap();
    let mut child = RecoveryChild(child);
    let expires = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "owned recovery child exited"
        );
        let endpoint = crate::broker::discovery::load_endpoint(root).unwrap();
        if endpoint
            .as_ref()
            .is_some_and(|endpoint| previous != Some(endpoint.epoch()))
        {
            return child;
        }
        assert!(
            std::time::Instant::now() < expires,
            "owned recovery child did not publish its endpoint"
        );
        // Deliberately wait synchronously before yielding the current-thread
        // parent: only our replacement child can publish the next epoch.
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn recovery_role_fixture() -> (tempfile::TempDir, Directory) {
    // The harness owns its writable namespace (TMPDIR in the OS sandbox).
    // Canonicalize that existing parent rather than reaching a global /tmp.
    let temporary_root = std::env::temp_dir().canonicalize().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("srf-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(temporary_root)
        .unwrap();
    let root = Directory::open_existing(&temp.path().canonicalize().unwrap()).unwrap();
    // Same portable UDS pathname cap as local_ipc; fail as a fixture prerequisite
    // before starting a child, without weakening the product's path checks.
    let socket_path = root.path().join("endpoints/i-0000000000000000/s");
    assert!(
        socket_path.as_os_str().as_encoded_bytes().len() <= 103,
        "recovery fixture requires TMPDIR to be a shorter private writable directory"
    );
    root.persistent_create("auth", &[0x5a; 32]).unwrap();
    root.persistent_create(".owner.lock", &[]).unwrap();
    root.persistent_child("endpoints", true).unwrap().unwrap();
    root.persistent_child("announcements", true)
        .unwrap()
        .unwrap();
    (temp, root)
}
async fn warmed_recovery_client(root: &Directory) -> EnforcerStopClient {
    let client = EnforcerStopClient::connect_or_spawn_for_recovery(
        root,
        &std::env::current_exe().unwrap(),
        8,
    )
    .await
    .unwrap();
    let epoch = crate::broker::discovery::load_endpoint(root)
        .unwrap()
        .unwrap()
        .epoch();
    let expires = std::time::Instant::now() + Duration::from_secs(3);
    while client.observed_watch_epoch() != Some(epoch) {
        assert!(
            std::time::Instant::now() < expires,
            "real Watch did not acknowledge the initial epoch"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    client
}
async fn wait_for_cancel_without_business_retry(
    client: &EnforcerStopClient,
    arm: &str,
    hash: &str,
    command: Uuid,
) -> bool {
    let expires = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < expires {
        if client
            .query_stop(arm, hash)
            .await
            .is_ok_and(|slot| slot.cancel_command_id() == Some(command))
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}
fn recovery_registration(
    scope: &JournalScopeV1,
    target: &crate::broker::journal::recovery::RecoveredStopV1,
) -> (RegistrationV1, String) {
    let registration = RegistrationV1::new(
        hex::encode(Sha256::digest(scope.gateway_origin().as_bytes())),
        scope.tenant_id().into(),
        target.arm().clone(),
        target.receipt().clone(),
        target.authority().clone(),
        target.owned().clone(),
    )
    .unwrap();
    let mut hash = Sha256::new();
    hash.update(b"ai-guard:enforcer-registration:v1\0");
    hash.update(serde_json::to_vec(&registration).unwrap());
    (registration, hex::encode(hash.finalize()))
}
fn recovery_restore_value(projection: &CancelStopProjectionV1, generation: u64) -> Value {
    let target = &projection.stops()[0];
    let (registration, registration_hash) = recovery_registration(projection.scope(), target);
    let prefix = projection.cancel_prefix_for_stop(0).unwrap();
    let cancel_prefix = prefix.receipts().iter().map(|receipt| json!({
        "command_id":receipt.command_id(), "input_hash":receipt.input_hash(), "confirmed_head":receipt.head(),
    })).collect::<Vec<_>>();
    json!({
        "version": 1, "request_id": Uuid::new_v4(), "registration": registration,
        "registration_hash": registration_hash,
        "source": { "scope_hash": hex::encode(Sha256::digest(canonical(projection.scope()).unwrap())),
            "job_id": projection.operation_id(), "intent_hash": target.arm().intent_hash,
            "confirmed_head": prefix.head(), "arm_operation_index": target.birth_index() },
        "cancel_generation": generation,
        "durable_cancel": cancel_prefix.last(), "cancel_prefix": cancel_prefix,
    })
}
async fn raw_recovery_request(root: &Directory, request: Value) -> Value {
    use hmac::{Hmac, Mac};
    #[derive(serde::Serialize)]
    struct Ledger<'a> {
        state: &'a str,
        reason: &'a str,
    }
    let endpoint = crate::broker::discovery::load_endpoint(root)
        .unwrap()
        .unwrap();
    let endpoints = root.persistent_child("endpoints", false).unwrap().unwrap();
    let mut stream = endpoint.connect_checked(&endpoints).await.unwrap();
    let challenge: Value = read_frame(&mut stream, 4096, Duration::from_secs(3))
        .await
        .unwrap();
    let canonical = serde_json::to_vec(&(
        2_u32,
        "enforcer",
        endpoint.epoch(),
        Uuid::parse_str(challenge["nonce"].as_str().unwrap()).unwrap(),
        challenge["root_binding"].as_str().unwrap(),
        ["stops_only"],
        Ledger {
            state: "unverified",
            reason: "recovery_transport_only",
        },
        "stops_only",
    ))
    .unwrap();
    let mut mac = Hmac::<Sha256>::new_from_slice(&[0x5a; 32]).unwrap();
    mac.update(b"ai-guard:enforcer-client-auth:v2\0");
    mac.update(&canonical);
    let proof = hex::encode(mac.finalize().into_bytes());
    write_frame(
        &mut stream,
        &json!({"requested_role":"stops_only","proof":proof}),
        4096,
        Duration::from_secs(3),
    )
    .await
    .unwrap();
    let grant: Value = read_frame(&mut stream, 4096, Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(grant["granted_role"], "stops_only");
    write_frame(&mut stream, &request, FRAME_BYTES, Duration::from_secs(3))
        .await
        .unwrap();
    read_frame(&mut stream, FRAME_BYTES, Duration::from_secs(3))
        .await
        .unwrap()
}

async fn same_epoch_failed_stop_is_retried(stage: &str) {
    let fixture = Fixture::with_manual_arm(true);
    let command = Uuid::new_v4();
    let projection = Arc::new(
        fixture
            .writer
            .stops()
            .append_cancel(CancelInputV1::new(fixture.operation, command).unwrap())
            .await
            .unwrap(),
    );
    let target = &projection.stops()[0];
    let arm = target.arm().operation_id.clone();
    let (_, hash) = recovery_registration(projection.scope(), target);
    let (_temp, root) = recovery_role_fixture();
    let fault_marker = root.path().join("fixture-fault-consumed");
    let child = recovery_child_with_faults(
        &root,
        None,
        &RecoveryFaults {
            stage: Some(stage),
            arm_id: Some(&arm),
            fault_marker: Some(&fault_marker),
            ..RecoveryFaults::default()
        },
    );
    let client = warmed_recovery_client(&root).await;
    let epoch = client.observed_watch_epoch().unwrap();
    let _initial_reply = client.deliver_projection(projection, 1).await;
    // Causal positive control: the real pre-dispatch close was consumed. A
    // correct background retry may already have completed before Query runs,
    // or may even satisfy this same caller; absence-after-error is not valid.
    assert_eq!(fs::read(&fault_marker).unwrap(), b"fault-consumed\n");
    let recovered = wait_for_cancel_without_business_retry(&client, &arm, &hash, command).await;
    let final_epoch = crate::broker::discovery::load_endpoint(&root)
        .unwrap()
        .unwrap()
        .epoch();
    let count = client.status().await.unwrap().slot_count();
    drop(client);
    drop(child);
    assert_eq!(
        final_epoch, epoch,
        "the same authenticated guardian must survive the fault"
    );
    assert!(
        recovered,
        "pending durable STOP must retry automatically in the same epoch ({stage})"
    );
    assert_eq!(count, 1, "retry must reserve the same target exactly once");
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_same_epoch_retries_connection_closed_before_reserve() {
    Box::pin(same_epoch_failed_stop_is_retried("before_reserve")).await;
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_same_epoch_retries_connection_closed_before_deliver() {
    Box::pin(same_epoch_failed_stop_is_retried("before_deliver")).await;
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_replay_snapshot_cannot_ack_a_later_pending_target() {
    let first = Fixture::with_manual_arm(true);
    let second = Fixture::with_manual_arm(true);
    let command_a = Uuid::new_v4();
    let command_b = Uuid::new_v4();
    let projection_a = Arc::new(
        first
            .writer
            .stops()
            .append_cancel(CancelInputV1::new(first.operation, command_a).unwrap())
            .await
            .unwrap(),
    );
    let projection_b = Arc::new(
        second
            .writer
            .stops()
            .append_cancel(CancelInputV1::new(second.operation, command_b).unwrap())
            .await
            .unwrap(),
    );
    let arm_a = projection_a.stops()[0].arm().operation_id.clone();
    let arm_b = projection_b.stops()[0].arm().operation_id.clone();
    let (_, hash_a) = recovery_registration(projection_a.scope(), &projection_a.stops()[0]);
    let (_, hash_b) = recovery_registration(projection_b.scope(), &projection_b.stops()[0]);
    let (_temp, root) = recovery_role_fixture();
    let mut child = recovery_child(&root, None);
    let client = warmed_recovery_client(&root).await;
    client.deliver_projection(projection_a, 1).await.unwrap();
    let old_epoch = client.observed_watch_epoch().unwrap();
    let barrier = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let barrier_root = barrier.path().canonicalize().unwrap();
    let fault_marker = root.path().join("fixture-fault-consumed");
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    child = recovery_child_with_faults(
        &root,
        Some(old_epoch),
        &RecoveryFaults {
            stage: Some("before_reserve"),
            arm_id: Some(&arm_b),
            barrier: Some((&arm_a, &barrier_root)),
            fault_marker: Some(&fault_marker),
            ..RecoveryFaults::default()
        },
    );
    let epoch = crate::broker::discovery::load_endpoint(&root)
        .unwrap()
        .unwrap()
        .epoch();
    let expires = std::time::Instant::now() + Duration::from_secs(3);
    while !barrier_root.join("reserved").exists() {
        assert!(
            std::time::Instant::now() < expires,
            "snapshot A did not reach the post-Reserve barrier"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // B is retained only after the worker captured A's replay snapshot. Its
    // genuine connection is closed before dispatch, while the same E lives.
    let _initial_reply = client.deliver_projection(projection_b, 2).await;
    assert_eq!(fs::read(&fault_marker).unwrap(), b"fault-consumed\n");
    let mut release = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(barrier_root.join("release"))
        .unwrap();
    use std::io::Write;
    release.write_all(b"fixture-release\n").unwrap();
    drop(release);
    let a_recovered =
        wait_for_cancel_without_business_retry(&client, &arm_a, &hash_a, command_a).await;
    let b_recovered =
        wait_for_cancel_without_business_retry(&client, &arm_b, &hash_b, command_b).await;
    let final_epoch = crate::broker::discovery::load_endpoint(&root)
        .unwrap()
        .unwrap()
        .epoch();
    drop(client);
    drop(child);
    assert_eq!(final_epoch, epoch);
    assert!(a_recovered, "original snapshot A must complete");
    assert!(
        b_recovered,
        "A's old snapshot ACK must not clear B's newer pending state"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_replay_retries_a_failed_actor_spawn_without_duplicate_threads() {
    let fixture = Fixture::with_manual_arm(true);
    let command = Uuid::new_v4();
    let projection = Arc::new(
        fixture
            .writer
            .stops()
            .append_cancel(CancelInputV1::new(fixture.operation, command).unwrap())
            .await
            .unwrap(),
    );
    let arm = projection.stops()[0].arm().operation_id.clone();
    let (_, hash) = recovery_registration(projection.scope(), &projection.stops()[0]);
    let (_temp, root) = recovery_role_fixture();
    let marker = root.path().join("fixture-worker-starts");
    let fault_marker = root.path().join("fixture-fault-consumed");
    let spawned_marker = marker.with_extension("spawned");
    let child = recovery_child_with_faults(
        &root,
        None,
        &RecoveryFaults {
            stage: Some("spawn_once"),
            arm_id: Some(&arm),
            started_marker: Some(&marker),
            fault_marker: Some(&fault_marker),
            ..RecoveryFaults::default()
        },
    );
    let client = warmed_recovery_client(&root).await;
    client
        .deliver_projection(Arc::clone(&projection), 1)
        .await
        .unwrap();
    assert_eq!(client.status().await.unwrap().slot_count(), 1);
    assert_eq!(
        fs::read(&fault_marker).unwrap(),
        b"fault-consumed\n",
        "positive control: the Err injected before OS spawn was consumed"
    );
    let (first, second) = tokio::join!(
        client.deliver_projection(Arc::clone(&projection), 1),
        client.deliver_projection(projection, 1),
    );
    first.unwrap();
    second.unwrap();
    let expires = std::time::Instant::now() + Duration::from_secs(3);
    while (!marker.exists() || !spawned_marker.exists()) && std::time::Instant::now() < expires {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let starts = fs::read(&marker).unwrap_or_default().len();
    // Success is recorded synchronously after spawn returns, before a Reserve
    // ACK. Entry stays at a test barrier, so a not-yet-scheduled second thread
    // cannot be hidden by reading the first entry then killing the child.
    let slot = client.query_stop(&arm, &hash).await.unwrap();
    let spawned = fs::read(&spawned_marker).unwrap_or_default().len();
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(marker.with_extension("release"))
        .and_then(|mut file| std::io::Write::write_all(&mut file, b"fixture-release\n"))
        .unwrap();
    let count = client.status().await.unwrap().slot_count();
    drop(client);
    drop(child);
    assert_eq!(
        starts, 1,
        "exact concurrent replays must start one previously absent worker thread"
    );
    assert_eq!(spawned, 1, "only one OS spawn may have succeeded");
    assert_eq!(count, 1);
    assert_eq!(slot.cancel_command_id(), Some(command));
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_old_cancel_replay_preserves_the_newer_proven_cancel() {
    let mut fixture = Fixture::with_manual_arm(true);
    let command_one = Uuid::new_v4();
    let command_two = Uuid::new_v4();
    let first = fixture
        .writer
        .stops()
        .append_cancel(CancelInputV1::new(fixture.operation, command_one).unwrap())
        .await
        .unwrap();
    let first_zero = Arc::new(
        fixture
            .writer
            .stops()
            .cancel_stop_projection(fixture.operation, command_one)
            .await
            .unwrap()
            .unwrap(),
    );
    let second = fixture
        .writer
        .stops()
        .append_cancel(CancelInputV1::new(fixture.operation, command_two).unwrap())
        .await
        .unwrap();
    let second_zero = Arc::new(
        fixture
            .writer
            .stops()
            .cancel_stop_projection(fixture.operation, command_two)
            .await
            .unwrap()
            .unwrap(),
    );
    let receipt_one = first.receipt().clone();
    assert!(first.receipt().head().revision() < second.receipt().head().revision());
    let arm = first.stops()[0].arm().operation_id.clone();
    assert_eq!(second.stops()[0].arm().operation_id, arm);
    let (_, hash) = recovery_registration(first.scope(), &first.stops()[0]);
    let (_temp, root) = recovery_role_fixture();
    let mut child = recovery_child(&root, None);
    let client = warmed_recovery_client(&root).await;
    fixture.service.enforcer = None;
    fixture.service.stop_enforcer = Some(client.clone());
    fixture.service.fenced.store(true, Ordering::SeqCst);
    let scope = fixture.writer.scope().clone();
    let generation = CancelGeneration::new();
    generation.invalidate();
    fixture
        .service
        .deliver_cancel(&scope, first, &generation)
        .await
        .unwrap();
    generation.invalidate();
    fixture
        .service
        .deliver_cancel(&scope, second, &generation)
        .await
        .unwrap();
    let first = fixture
        .writer
        .stops()
        .cancel_stop_projection(fixture.operation, command_one)
        .await
        .unwrap()
        .unwrap();
    let replay = fixture
        .service
        .deliver_cancel(&scope, first, &generation)
        .await;
    let retained = client.query_stop(&arm, &hash).await.unwrap();
    let count = client.status().await.unwrap().slot_count();
    let current_generation = generation.current();
    assert!(
        replay.is_ok(),
        "an exact old C1 replay must remain ACKable after C2: {replay:?}"
    );
    let replay = replay.unwrap();
    assert_eq!(replay.command_id(), receipt_one.command_id());
    assert_eq!(replay.input_hash(), receipt_one.input_hash());
    assert_eq!(replay.head(), receipt_one.head());
    assert_eq!(
        current_generation, 2,
        "replaying C1 must not invalidate another generation"
    );
    assert_eq!(retained.cancel_command_id(), Some(command_two));
    assert_eq!(retained.generation(), 2);
    assert_eq!(count, 1);
    // A replacement guardian and a new local cancellation generation both
    // start at zero. Two proven commands may share it; the verified prefix,
    // not numeric generation, decides which durable Cancel is retained.
    fixture.service.stop_enforcer = None;
    drop(client);
    let old_epoch = crate::broker::discovery::load_endpoint(&root)
        .unwrap()
        .unwrap()
        .epoch();
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    child = recovery_child(&root, Some(old_epoch));
    let client = warmed_recovery_client(&root).await;
    client.deliver_projection(first_zero, 0).await.unwrap();
    client.deliver_projection(second_zero, 0).await.unwrap();
    fixture.service.stop_enforcer = Some(client.clone());
    let zero = CancelGeneration::new();
    let old = fixture
        .writer
        .stops()
        .cancel_stop_projection(fixture.operation, command_one)
        .await
        .unwrap()
        .unwrap();
    let ack = fixture
        .service
        .deliver_cancel(&scope, old, &zero)
        .await
        .unwrap();
    let retained = client.query_stop(&arm, &hash).await.unwrap();
    let count = client.status().await.unwrap().slot_count();
    assert_eq!(ack.command_id(), receipt_one.command_id());
    assert_eq!(ack.input_hash(), receipt_one.input_hash());
    assert_eq!(ack.head(), receipt_one.head());
    assert_eq!(zero.current(), 0);
    assert_eq!(retained.generation(), 0);
    assert_eq!(retained.cancel_command_id(), Some(command_two));
    assert_eq!(count, 1);
    drop((client, fixture));
    drop(child);
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_two_durable_cancels_survive_epoch_replacement_without_full_admission() {
    // Real Journal/Writer/Store receipts; runtime snapshots and the unavailable
    // invocation remain explicit fixture data, never engine observations.
    let first = Fixture::with_manual_arm(true);
    let second = Fixture::with_manual_arm(true);
    let command_a = Uuid::new_v4();
    let command_b = Uuid::new_v4();
    let projection_a = Arc::new(
        first
            .writer
            .stops()
            .append_cancel(CancelInputV1::new(first.operation, command_a).unwrap())
            .await
            .unwrap(),
    );
    let projection_b = Arc::new(
        second
            .writer
            .stops()
            .append_cancel(CancelInputV1::new(second.operation, command_b).unwrap())
            .await
            .unwrap(),
    );
    assert_eq!(projection_a.stops().len(), 1);
    assert_eq!(projection_b.stops().len(), 1);
    #[cfg(target_os = "macos")]
    let temporary_root = "/private/tmp";
    #[cfg(not(target_os = "macos"))]
    let temporary_root = "/tmp";
    let temp = tempfile::Builder::new()
        .prefix("sr-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(temporary_root)
        .unwrap();
    let path = temp.path().canonicalize().unwrap();
    let root = Directory::open_existing(&path).unwrap();
    root.persistent_create("auth", &[0x5a; 32]).unwrap();
    root.persistent_create(".owner.lock", &[]).unwrap();
    root.persistent_child("endpoints", true).unwrap().unwrap();
    root.persistent_child("announcements", true)
        .unwrap()
        .unwrap();
    let mut child = recovery_child(&root, None);
    let client = EnforcerStopClient::connect_or_spawn_for_recovery(
        &root,
        &std::env::current_exe().unwrap(),
        8,
    )
    .await
    .unwrap();
    client
        .deliver_projection(Arc::clone(&projection_a), 3)
        .await
        .unwrap();
    client
        .deliver_projection(Arc::clone(&projection_b), 4)
        .await
        .unwrap();
    // Lost ACK/retry is the exact same reservation, not another actor.
    client
        .deliver_projection(Arc::clone(&projection_a), 3)
        .await
        .unwrap();
    assert_eq!(client.status().await.unwrap().slot_count(), 2);
    assert!(!client.status().await.unwrap().admission_available());
    let (_, hash_a) = recovery_registration(projection_a.scope(), &projection_a.stops()[0]);
    let (_, hash_b) = recovery_registration(projection_b.scope(), &projection_b.stops()[0]);
    let arm_a = projection_a.stops()[0].arm().operation_id.clone();
    let arm_b = projection_b.stops()[0].arm().operation_id.clone();
    assert_ne!(arm_a, arm_b);
    assert_eq!(
        client
            .query_stop(&arm_a, &hash_a)
            .await
            .unwrap()
            .cancel_command_id(),
        Some(command_a)
    );
    assert_eq!(
        client
            .query_stop(&arm_b, &hash_b)
            .await
            .unwrap()
            .cancel_command_id(),
        Some(command_b)
    );

    // A new genuine StopWriter Cancel is valid even at the same generation.
    // The negative must instead fork an already-seen identity/receipt without
    // a verified prefix extension. Only this authenticated raw fixture does so.
    let mut wire = recovery_restore_value(&projection_a, 3);
    let divergent_hash = if wire["durable_cancel"]["input_hash"] == "0".repeat(64) {
        "1".repeat(64)
    } else {
        "0".repeat(64)
    };
    wire["durable_cancel"]["input_hash"] = json!(divergent_hash);
    wire["cancel_prefix"][0]["input_hash"] = wire["durable_cancel"]["input_hash"].clone();
    let refused =
        raw_recovery_request(&root, json!({"kind":"deliver_durable_stop","value":wire})).await;
    assert_eq!(refused, json!({"kind":"error","code":"conflict"}));
    assert_eq!(
        client
            .query_stop(&arm_a, &hash_a)
            .await
            .unwrap()
            .cancel_command_id(),
        Some(command_a),
        "divergence must preserve the first durable Cancel"
    );
    client
        .deliver_projection(Arc::clone(&projection_a), 3)
        .await
        .unwrap();

    let old = crate::broker::discovery::load_endpoint(&root)
        .unwrap()
        .unwrap()
        .epoch();
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    child = recovery_child(&root, Some(old));
    // Reconnection is triggered only by Status/Watch, not by resending A/B.
    let expires = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(status) = client.status().await {
            assert!(!status.admission_available());
            if status.slot_count() == 2
                && client
                    .query_stop(&arm_a, &hash_a)
                    .await
                    .is_ok_and(|slot| slot.cancel_command_id() == Some(command_a))
                && client
                    .query_stop(&arm_b, &hash_b)
                    .await
                    .is_ok_and(|slot| slot.cancel_command_id() == Some(command_b))
            {
                break;
            }
        }
        assert!(
            std::time::Instant::now() < expires,
            "watch did not restore both known STOP targets"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    for (arm, hash, command, generation) in [
        (&arm_a, &hash_a, command_a, 3),
        (&arm_b, &hash_b, command_b, 4),
    ] {
        let slot = client.query_stop(arm, hash).await.unwrap();
        assert_eq!(slot.cancel_command_id(), Some(command));
        assert_eq!(slot.generation(), generation);
        assert!(
            !matches!(
                slot.phase(),
                crate::broker::enforcer::StopPhaseV1::StopReady
            ),
            "durable Cancel stays latched"
        );
    }
    // Query accepted the retained registration hashes, binding the original
    // CID/authority/Arm/deadline; no new registration or permission was sent.
    drop(client);
    drop(child);
}

#[tokio::test(flavor = "current_thread")]
async fn first_cancel_without_manual_blob_persists_before_delivery_and_replays() {
    let fixture = Fixture::with_manual_arm(true);
    let before = fixture
        .writer
        .load(fixture.operation)
        .await
        .unwrap()
        .unwrap();
    let expected_broker = before
        .broker_checkpoint_v2()
        .unwrap()
        .canonical_bytes()
        .unwrap();
    let before_head = before.head().clone();
    assert!(
        before.broker_checkpoint_v3().is_none(),
        "no Cancel or lifecycle command exists yet"
    );
    assert_eq!(
        before
            .broker_checkpoint_v2()
            .unwrap()
            .supervision()
            .operations()
            .iter()
            .filter(|op| matches!(
                op,
                SupervisionOperationV1::Arm {
                    response: Some(_),
                    ..
                }
            ))
            .count(),
        1
    );
    drop(before);
    let blob = fixture.manual_blob.as_ref().unwrap();
    let job_path = blob.parent().unwrap().parent().unwrap();
    fs::remove_file(blob).unwrap();
    fs::remove_file(fixture.manual_source.as_ref().unwrap()).unwrap();
    assert!(
        fixture.writer.load(fixture.operation).await.is_err(),
        "positive control: business loading refuses the missing durable blob"
    );
    let scope = fixture.writer.scope().clone();
    let profile = fixture
        .service
        .registry
        .profile(scope.profile_id())
        .unwrap();
    let command = Uuid::from_u128(0x55555555555545558555555555555555);
    assert!(
        fixture
            .writer
            .cancel_stop_projection(fixture.operation, command)
            .await
            .unwrap()
            .is_none()
    );
    let probe = DeliveryScope::begin();
    let first = fixture
        .service
        .cancel_for_profile(
            profile,
            &scope,
            Arc::clone(&fixture.jobs),
            fixture.operation,
            command,
        )
        .await;
    let projection = fixture
        .writer
        .cancel_stop_projection(fixture.operation, command)
        .await
        .unwrap()
        .expect("first Cancel must become durable even when its provider blob is missing");
    assert_eq!(
        projection.receipt().head().revision(),
        before_head.revision() + 1
    );
    assert_eq!(projection.stops().len(), 1);
    assert!(projection.incomplete());
    let receipt_id = projection.receipt().command_id();
    let receipt_hash = projection.receipt().input_hash().to_owned();
    let receipt_head = projection.receipt().head().clone();
    drop(projection);
    assert!(matches!(first, Err(BrokerErrorCodeV1::NeedsReconciliation)));
    let generation = fixture
        .jobs
        .job(fixture.operation)
        .unwrap()
        .cancel
        .current();
    assert_eq!(
        probe.attempts(),
        vec![Attempt {
            operation: fixture.operation,
            generation
        }]
    );
    let published: Value = serde_json::from_slice(
        &fs::read(job_path.join(format!("rev-{:020}.json", receipt_head.revision()))).unwrap(),
    )
    .unwrap();
    assert_eq!(
        canonical(&published["payload"]["broker"]).unwrap(),
        expected_broker,
        "only Cancel is appended; runner/supervision stay byte stable"
    );
    let current = fs::read(job_path.join("current.json")).unwrap();
    let replay = fixture
        .service
        .cancel_for_profile(
            profile,
            &scope,
            Arc::clone(&fixture.jobs),
            fixture.operation,
            command,
        )
        .await;
    assert!(
        matches!(replay, Err(BrokerErrorCodeV1::NeedsReconciliation)),
        "coverage remains explicitly incomplete after known delivery"
    );
    let replayed = fixture
        .writer
        .cancel_stop_projection(fixture.operation, command)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replayed.receipt().command_id(), receipt_id);
    assert_eq!(replayed.receipt().input_hash(), receipt_hash);
    assert_eq!(replayed.receipt().head(), &receipt_head);
    assert_eq!(fs::read(job_path.join("current.json")).unwrap(), current);
    assert_eq!(
        fixture
            .jobs
            .job(fixture.operation)
            .unwrap()
            .cancel
            .current(),
        generation
    );
    assert_eq!(
        probe.attempts(),
        vec![
            Attempt {
                operation: fixture.operation,
                generation
            },
            Attempt {
                operation: fixture.operation,
                generation
            }
        ]
    );
    assert!(!probe.overflowed());
    assert!(fixture.writer.load(fixture.operation).await.is_err());
    let diagnostic = fixture
        .service
        .recovery_status_for_profile(profile, &scope, fixture.operation)
        .await
        .unwrap();
    let BrokerReplyV1::RecoveryStatus { status } = diagnostic.reply else {
        panic!("recovery has its own explicit reply");
    };
    assert_eq!(status.source, RecoveryDataSourceV1::ConfirmedJournal);
    assert_eq!(
        status.runtime_evidence,
        RecoveryRuntimeEvidenceV1::NotQueried
    );
    assert!(status.incomplete && status.reconciliation_required);
    assert_eq!(status.known_arms.len(), 1);
    assert_eq!(status.cancel_commands.len(), 1);
    assert_eq!(status.cancel_commands[0].command_id, command);
    assert_eq!(status.cancel_commands[0].revision, receipt_head.revision());
    assert_eq!(status.known_arms[0].cancel_command_id, Some(command));
}

#[tokio::test(flavor = "current_thread")]
async fn cold_cancel_route_survives_invalid_sibling_without_opening_full_writer() {
    let fixture = Fixture::with_manual_arm(true);
    let before = fixture
        .writer
        .load(fixture.operation)
        .await
        .unwrap()
        .unwrap();
    let before_head = before.head().clone();
    let expected_broker = before
        .broker_checkpoint_v2()
        .unwrap()
        .canonical_bytes()
        .unwrap();
    let scope = fixture.writer.scope().clone();
    let job_path = fixture
        .manual_blob
        .as_ref()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    drop(before);
    let Fixture {
        _temp,
        mut service,
        jobs,
        journal,
        writer,
        operation,
        ..
    } = fixture;
    let cold_jobs = Arc::new(ProfileJobs::new());
    service
        .profiles
        .insert(scope.profile_id().into(), Arc::clone(&cold_jobs));
    drop(jobs);
    drop(writer);
    drop(journal);
    let sibling = job_path
        .parent()
        .unwrap()
        .join("unexpected-private-sibling");
    fs::write(&sibling, b"synthetic private sibling").unwrap();
    fs::set_permissions(&sibling, fs::Permissions::from_mode(0o600)).unwrap();
    let profile = service.registry.profile(scope.profile_id()).unwrap();
    let command = Uuid::from_u128(0x66666666666646668666666666666666);
    let probe = DeliveryScope::begin();
    let expires = tokio::time::Instant::now() + Duration::from_secs(5);
    // Closing the detached writer drains its queue before releasing the lock.
    // A transient Busy here must not be mistaken for the invalid-sibling RED.
    let result = loop {
        let result = service
            .cancel_for_profile(profile, &scope, Arc::clone(&cold_jobs), operation, command)
            .await;
        if matches!(result, Err(BrokerErrorCodeV1::Busy)) && tokio::time::Instant::now() < expires {
            tokio::time::sleep(Duration::from_millis(25)).await;
            continue;
        }
        break result;
    };
    assert!(
        !matches!(result, Err(BrokerErrorCodeV1::Busy)),
        "fixture writer lease must drain before classifying the cold-route result"
    );
    assert!(matches!(
        result,
        Err(BrokerErrorCodeV1::NeedsReconciliation)
    ));
    assert_eq!(
        probe.attempts().len(),
        1,
        "the cold Cancel route must persist and attempt known delivery despite an invalid sibling"
    );
    let current: Value =
        serde_json::from_slice(&fs::read(job_path.join("current.json")).unwrap()).unwrap();
    assert_eq!(
        current["head"]["revision"].as_u64(),
        Some(before_head.revision() + 1)
    );
    let checkpoint: Value = serde_json::from_slice(
        &fs::read(job_path.join(format!("rev-{:020}.json", before_head.revision() + 1))).unwrap(),
    )
    .unwrap();
    assert_eq!(
        canonical(&checkpoint["payload"]["broker"]).unwrap(),
        expected_broker
    );
    assert_eq!(
        checkpoint["payload"]["commands"][0]["command_id"],
        json!(command)
    );
    assert!(
        service.writer(scope.profile_id(), &scope).await.is_err(),
        "a STOP route must not turn the invalid namespace into a full business writer"
    );
    assert_eq!(fs::read(sibling).unwrap(), b"synthetic private sibling");
    // A safely countable regular sibling allowed the first publication.
    // An unknown nested directory cannot be assigned zero cost for a new one.
    let unknown = job_path.parent().unwrap().join("unknown-directory");
    fs::create_dir(&unknown).unwrap();
    fs::set_permissions(&unknown, fs::Permissions::from_mode(0o700)).unwrap();
    let nested = unknown.join("unmeasured-nested-content");
    fs::create_dir(&nested).unwrap();
    fs::set_permissions(&nested, fs::Permissions::from_mode(0o700)).unwrap();
    let stops = cold_jobs.writer.get().unwrap().stops();
    let pointer = fs::read(job_path.join("current.json")).unwrap();
    let failed = stops
        .append_cancel(CancelInputV1::new(operation, Uuid::new_v4()).unwrap())
        .await
        .unwrap_err();
    assert_eq!(
        failed.error(),
        crate::broker::journal::JournalError::UnsafeFilesystem
    );
    assert_eq!(fs::read(job_path.join("current.json")).unwrap(), pointer);
    let replayed = stops
        .append_cancel(CancelInputV1::new(operation, command).unwrap())
        .await
        .unwrap();
    assert_eq!(
        replayed.receipt().head().revision(),
        before_head.revision() + 1
    );
    assert_eq!(replayed.stops().len(), 1);
    assert!(replayed.incomplete());
    assert_eq!(fs::read(job_path.join("current.json")).unwrap(), pointer);
    let diagnostic = service
        .recovery_status_for_profile(profile, &scope, operation)
        .await
        .unwrap();
    let BrokerReplyV1::RecoveryStatus { status } = diagnostic.reply else {
        panic!("recovery has its own explicit reply");
    };
    assert_eq!(status.journal_mode, JournalModeV1::StopOnly);
    assert_eq!(
        status.runtime_evidence,
        RecoveryRuntimeEvidenceV1::NotQueried
    );
    assert!(status.incomplete && status.reconciliation_required);
    assert_eq!(status.tenant_id, scope.tenant_id());
    assert_eq!(status.known_arms.len(), 1);
    assert_eq!(status.cancel_commands.len(), 1);
    assert_eq!(status.head_revision, before_head.revision() + 1);
    assert!(!probe.overflowed());
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_diagnostic_does_not_create_a_missing_profile_namespace() {
    let mut fixture = Fixture::new();
    let scope = fixture.writer.scope().clone();
    let empty_root = fixture
        ._temp
        .path()
        .canonicalize()
        .unwrap()
        .join("empty-recovery-root");
    fs::create_dir(&empty_root).unwrap();
    fs::set_permissions(&empty_root, fs::Permissions::from_mode(0o700)).unwrap();
    fixture.service.root = Directory::open_existing(&empty_root).unwrap();
    fixture
        .service
        .profiles
        .insert(scope.profile_id().into(), Arc::new(ProfileJobs::new()));
    let profile = fixture
        .service
        .registry
        .profile(scope.profile_id())
        .unwrap();
    let refusal = fixture
        .service
        .recovery_status_for_profile(profile, &scope, fixture.operation)
        .await
        .err();
    assert_eq!(refusal, Some(BrokerErrorCodeV1::NeedsReconciliation));
    assert_eq!(fs::read_dir(&empty_root).unwrap().count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn acknowledged_cancel_retry_redelivers_without_revoking_later_execution() {
    let fixture = Fixture::new();
    let command = Uuid::from_u128(0x44444444444444448444444444444444);
    let probe = DeliveryScope::begin();
    let first = fixture
        .service
        .cancel(
            fixture.writer.stops(),
            Arc::clone(&fixture.jobs),
            fixture.operation,
            command,
        )
        .await;
    assert!(matches!(first, Err(BrokerErrorCodeV1::NeedsReconciliation)));
    let job = fixture.jobs.job(fixture.operation).unwrap();
    assert_eq!(
        probe.attempts(),
        vec![Attempt {
            operation: fixture.operation,
            generation: job.cancel.current(),
        }],
        "positive control: the first durable request attempted delivery"
    );
    let receipt = fixture
        .journal
        .command_receipt(fixture.operation, command)
        .unwrap()
        .unwrap();
    let head = fixture
        .writer
        .load(fixture.operation)
        .await
        .unwrap()
        .unwrap()
        .head()
        .clone();
    assert_eq!(receipt.head(), &head);

    // This is the real execution gate held by a later Export, not a simulated
    // runtime export or a claim that any helper has been launched.
    let later_export = job
        .enter()
        .expect("execution may enter after acknowledged Cancel");
    let generation = later_export.generation();
    assert!(matches!(job.enter(), Err(BrokerErrorCodeV1::Busy)));
    let replay = tokio::time::timeout(
        Duration::from_secs(5),
        fixture.service.cancel(
            fixture.writer.stops(),
            Arc::clone(&fixture.jobs),
            fixture.operation,
            command,
        ),
    )
    .await
    .expect("Cancel replay must not wait behind the later execution gate")
    .unwrap_or_else(|code| panic!("replayed handler returned {code:?}"));
    assert_eq!(
        serde_json::to_value(replay.reply).unwrap(),
        serde_json::to_value(command_ack(fixture.operation, &receipt)).unwrap()
    );
    assert!(replay.run.is_none());
    assert_eq!(
        job.cancel.current(),
        generation,
        "old Cancel must not invalidate later execution"
    );
    assert!(matches!(job.enter(), Err(BrokerErrorCodeV1::Busy)));
    assert_eq!(
        fixture
            .writer
            .load(fixture.operation)
            .await
            .unwrap()
            .unwrap()
            .head(),
        &head
    );
    assert_eq!(
        fixture
            .journal
            .command_receipt(fixture.operation, command)
            .unwrap()
            .unwrap()
            .head(),
        receipt.head()
    );
    assert!(!probe.overflowed());
    assert_eq!(
        probe.attempts(),
        vec![
            Attempt {
                operation: fixture.operation,
                generation
            },
            Attempt {
                operation: fixture.operation,
                generation
            },
        ],
        "a durable Cancel ACK is not a delivered stop: replay must retry delivery after the first failure"
    );
}

#[path = "recovery_epoch_tests.rs"]
mod recovery_epoch_tests;
#[path = "recovery_multi_arm_tests.rs"]
mod recovery_multi_arm_tests;
