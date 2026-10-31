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
        AdmissionV1, ArmSubjectV1, ArmV1, DecisionPhaseV1, DecisionReceiptV1, HelperPermissionV1,
        HelperPurposeV1, HostClockMethodV1, HostClockSampleV1, LaunchSpecV1, NetworkV1,
        QuotaPolicyV1, RestartPolicyV1, RuntimeInspectionV1, RuntimeKindV1, SecurityPresetV1,
        SupervisedAdmissionV1, SupervisionProtocolV1, WorkerMutationV1, WorkerTransitionV1,
        helper_execution_policy_v1,
    },
    model::{DecisionRequest, Outcome},
    policy::{self, Policy},
    redaction::Redactor,
    runner::CheckpointPayloadV1,
    runtime::{OwnedContainerSnapshot, RuntimeAuthority},
};
use std::{cell::RefCell, fs, ops::Deref, os::unix::fs::PermissionsExt, path::PathBuf};

#[path = "stop_auth_tests.rs"]
mod stop_auth_tests;

/// Production aliases EnforcerClient directly. A probe-only Service cannot
/// access a native client: leaving the explicitly scoped seam is a test bug.
pub(super) enum TestEnforcer {
    Native(EnforcerClient),
    CancelDeliveryProbe,
}
impl Deref for TestEnforcer {
    type Target = EnforcerClient;
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
        let store = manual.then(|| {
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
        let manual_blob = store.as_ref().map(|store| {
            prepare_manual_arm(
                &journal,
                store,
                &intent,
                plan,
                accepted.head(),
                accepted.binding_manifest(),
            );
            let scope_root = fs::read_dir(journal_root.join("broker-journal-v1"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            scope_root
                .join("jobs")
                .join(operation.to_string())
                .join("bindings/manual-env-v1.bin")
        });
        drop(accepted);
        let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(10)).unwrap();
        let jobs = Arc::new(ProfileJobs::new());
        jobs.writer
            .set(ProfileWriterCapabilities::from_full(writer.clone()))
            .unwrap();
        let service = Service {
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
            enforcer: TestEnforcer::CancelDeliveryProbe,
            blocking: Arc::new(Semaphore::new(1)),
            preparation: Arc::new(Semaphore::new(1)),
            execution: Arc::new(Semaphore::new(1)),
            stopping: Arc::new(AtomicBool::new(false)),
            fenced: AtomicBool::new(false),
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
        parent: worker,
        owned,
        request: arm,
        response: Some(armed),
    };
    append(&mut head, &runner, &helpers, &operations);
    assert_eq!(
        journal.load(intent.operation_id()).unwrap().unwrap().head(),
        &head
    );
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
    assert!(matches!(
        fixture
            .service
            .recovery_status_for_profile(profile, &scope, fixture.operation)
            .await,
        Err(BrokerErrorCodeV1::InvalidInput)
    ));
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
