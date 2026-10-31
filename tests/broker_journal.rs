//! Private journal behavior against real files and the frozen intent contract.

use agentsandbox::{
    audit::Store,
    broker::{
        intent::ExecutionIntentV1,
        journal::{
            BindingInputs, BindingKindV1, ControlSecrets, FsExclusions, Journal, JournalLimitsV1,
            JournalScopeV1, ReserveResult, recovery::StopRecoveryReaderV1,
        },
    },
    control::{
        AdmissionV1, DecisionPhaseV1, DecisionReceiptV1, LaunchSpecV1, NetworkV1, QuotaPolicyV1,
        RestartPolicyV1, RuntimeInspectionV1, RuntimeKindV1, SecurityPresetV1, WorkerMutationV1,
        WorkerTransitionV1,
    },
    control_fs::{Directory, PrivateFile},
    model::DecisionRequest,
    model::{Decision, Outcome},
    policy::{self, Policy},
    redaction::Redactor,
    runner::CheckpointPayloadV1,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use uuid::Uuid;

fn legacy_plan() -> Value {
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

fn intent() -> ExecutionIntentV1 {
    ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&legacy_plan()).unwrap())
        .expect("valid legacy request with explicit no-credential binding")
}

fn approved_checkpoint(intent: &ExecutionIntentV1) -> CheckpointPayloadV1 {
    let mut plan = legacy_plan();
    let request: DecisionRequest = serde_json::from_value(plan["request"].clone()).unwrap();
    plan["request"]["human_approved"] = json!(true);
    plan["request"]["approval_hash"] = json!(agentsandbox::policy::request_hash(&request));
    CheckpointPayloadV1::from_plan_bytes(intent, &serde_json::to_vec(&plan).unwrap()).unwrap()
}

fn private_root(path: &Path) -> std::path::PathBuf {
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path.canonicalize().unwrap()
}

fn scope() -> JournalScopeV1 {
    scope_with("tenant-journal-test", "http://127.0.0.1:8787")
}

fn scope_with(tenant: &str, gateway_origin: &str) -> JournalScopeV1 {
    JournalScopeV1::new(
        Uuid::from_u128(0x11111111111141118111111111111111),
        tenant,
        gateway_origin,
        Uuid::from_u128(0x22222222222242228222222222222222),
        "broker-test",
    )
    .expect("valid private journal scope")
}

fn open_journal(root: &Path) -> Journal {
    open_journal_with_control(
        root,
        ControlSecrets::new(Vec::new()).expect("empty control-secret set is valid"),
    )
}

fn open_journal_with_control(root: &Path, control: ControlSecrets) -> Journal {
    open_journal_with_limits_and_control(root, JournalLimitsV1::default(), control)
}

fn open_journal_with_limits(root: &Path, limits: JournalLimitsV1) -> Journal {
    open_journal_with_limits_and_control(
        root,
        limits,
        ControlSecrets::new(Vec::new()).expect("empty control-secret set is valid"),
    )
}

fn open_journal_with_limits_and_control(
    root: &Path,
    limits: JournalLimitsV1,
    control: ControlSecrets,
) -> Journal {
    open_journal_at_scope(root, scope(), limits, control)
}

fn open_journal_at_scope(
    root: &Path,
    scope: JournalScopeV1,
    limits: JournalLimitsV1,
    control: ControlSecrets,
) -> Journal {
    let root = private_root(root);
    let directory = Directory::open_existing(&root).expect("private root exists");
    Journal::open(&directory, scope, &FsExclusions::new(), limits, control)
        .expect("open private journal")
}

fn operation() -> Uuid {
    Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap()
}

fn second_operation() -> Uuid {
    Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap()
}

fn durable_binding_path(root: &Path, operation_id: Uuid, file_name: &str) -> std::path::PathBuf {
    let namespace = root.join("broker-journal-v1");
    let scope_dir = std::fs::read_dir(namespace)
        .unwrap()
        .map(Result::unwrap)
        .find(|entry| entry.file_type().unwrap().is_dir())
        .expect("fixture has one durable journal scope")
        .path();
    scope_dir
        .join("jobs")
        .join(operation_id.to_string())
        .join("bindings")
        .join(file_name)
}

fn scan_recovery(root: &Path) -> agentsandbox::broker::journal::recovery::StopRecoveryReportV1 {
    let canonical_root = private_root(root);
    let directory = Directory::open_existing(&canonical_root).unwrap();
    StopRecoveryReaderV1::open_existing(
        &directory,
        scope(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap()
    .expect("fixture journal exists")
    .scan()
}

fn versioned_managed_plan() -> (ExecutionIntentV1, Value) {
    let mut plan = legacy_plan();
    let operation = operation().to_string();
    let session = "22222222-2222-4222-8222-222222222222";
    plan["operation_id"] = json!(operation);
    plan["session"] = json!(session);
    plan["request"]["session_id"] = json!(session);
    let encoded = serde_json::to_vec(&plan).unwrap();
    let intent = ExecutionIntentV1::from_plan_bytes(&encoded).unwrap();

    let control = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Store::open(
        &control.path().join("audit.sqlite"),
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let request: DecisionRequest = serde_json::from_value(plan["request"].clone()).unwrap();
    let policy = Policy::default();
    let stored = store
        .decision(
            &request,
            Some((policy::evaluate(&policy, &request), json!(null))),
        )
        .unwrap()
        .unwrap();
    let authority_id = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
    let before = serde_json::Map::<String, Value>::new();
    let before_hash =
        agentsandbox::control::content_hash("ai-guard:workspace-manifest:v1", &before).unwrap();
    let admission = AdmissionV1 {
        version: 1,
        operation_id: operation.clone(),
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
            session_id: session.into(),
            command: request.command.clone(),
            profile: request.profile.clone(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: authority_id.to_string(),
            image_ref: plan["options"]["image"].as_str().unwrap().into(),
            image_id: format!("sha256:{}", "a".repeat(64)),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            timeout_seconds: 600,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: before_hash,
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
        },
        decision_request: request,
    };
    let worker = store
        .admit_worker(
            &admission,
            &policy,
            Decision::new(Outcome::Allow, "test", "test"),
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .unwrap()
        .worker
        .unwrap();
    let worker = store
        .mutate_worker(&WorkerMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            expected_revision: worker.revision,
            transition: WorkerTransitionV1::Prepare,
        })
        .unwrap()
        .worker;
    let worker = store
        .mutate_worker(&WorkerMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            expected_revision: worker.revision,
            transition: WorkerTransitionV1::CreateIssued,
        })
        .unwrap()
        .worker;
    let inspection = RuntimeInspectionV1 {
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
    };
    let worker = store
        .mutate_worker(&WorkerMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            expected_revision: worker.revision,
            transition: WorkerTransitionV1::Created { inspection },
        })
        .unwrap()
        .worker;
    let authority: Value = serde_json::from_value(json!({
        "authority_id": authority_id, "kind": "docker", "endpoint": "unix:///private/fixture.sock",
        "engine_id": null, "podman": null,
        "invocation": {
            "policy_version": 2, "kind": "docker", "approved_entry": "/usr/bin/docker",
            "canonical_program": "/usr/bin/docker",
            "file_identity": {"device":1,"inode":2,"owner":1000,"group":1000,"mode":493,"length":10,"sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},
            "environment": {}, "cwd": "/private/fixture"
        }
    })).unwrap();
    let authority: agentsandbox::runtime::RuntimeAuthority =
        serde_json::from_value(authority.clone()).unwrap();
    let mut plan_request = plan["request"].clone();
    let observation = json!({
        "id":"d".repeat(64), "name":worker.attempt.stable_name, "image_id":worker.launch.image_id,
        "restart_count":0, "process":{"status":"running","running":true,"started_at":null,"finished_at":null,"exit_code":null,"oom_killed":null},
        "allocation":{"cpu":null,"memory_bytes":null,"pids_limit":null},
        "configuration_fingerprint":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "configuration_fingerprint_version":2, "observed_at":"2026-10-30T12:00:00Z", "diagnostics":[]
    });
    assert_ne!(
        operation, worker.attempt.attempt_id,
        "plan operation and worker attempt are distinct IDs"
    );
    let owned = json!({"handle":{"authority_fingerprint":authority.fingerprint(),"session":session,"operation":operation,"id":"d".repeat(64),"name":worker.attempt.stable_name,"configuration_fingerprint_version":2},"observation":observation});
    let managed = json!({
        "tenant_id":"tenant-journal-test", "gateway_origin":"http://127.0.0.1:8787", "config_root":"/private/control",
        "binding":authority, "authority_file":"runtime-authority.json", "admission":admission,
        "worker":worker, "owned":owned, "pending":null, "uncertain":[], "lock_identity":null,
        "before":{}, "import":null, "decision":{"outcome":"allow"}, "output_event":null,
        "output_acknowledged":false, "result":null, "archive_name":"worker.tar", "archive_digest":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "credential_digest":null
    });
    plan["managed"] = managed;
    // Keep the complete typed request as the durable current request.
    plan_request["human_approved"] = json!(false);
    plan["request"] = plan_request;
    (intent, plan)
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn managed_checkpoint_rejects_unknown_runtime_schema_versions() {
    let (intent, baseline) = versioned_managed_plan();
    let base_bytes = serde_json::to_vec(&baseline).unwrap();
    CheckpointPayloadV1::from_plan_bytes(&intent, &base_bytes)
        .expect("baseline managed checkpoint with real Store admission/worker/owned snapshot");
    let mut accepted = Vec::new();

    let mut pending = baseline.clone();
    let worker = &pending["managed"]["worker"];
    pending["managed"]["pending"] = json!({
        "version":2, "operation_id":Uuid::new_v4().to_string(),
        "worker_id":worker["worker_id"], "attempt_id":worker["attempt"]["attempt_id"],
        "expected_revision":worker["revision"], "transition":{"action":"stop"}
    });
    accepted.push((
        "pending mutation version 2",
        CheckpointPayloadV1::from_plan_bytes(&intent, &serde_json::to_vec(&pending).unwrap())
            .is_ok(),
    ));

    let mut uncertain = baseline.clone();
    let worker = &uncertain["managed"]["worker"];
    uncertain["managed"]["uncertain"] = json!([{
        "version":2, "operation_id":Uuid::new_v4().to_string(),
        "worker_id":worker["worker_id"], "attempt_id":worker["attempt"]["attempt_id"],
        "expected_revision":worker["revision"], "transition":{"action":"stop"}
    }]);
    accepted.push((
        "uncertain mutation version 2",
        CheckpointPayloadV1::from_plan_bytes(&intent, &serde_json::to_vec(&uncertain).unwrap())
            .is_ok(),
    ));

    let mut invocation = baseline.clone();
    invocation["managed"]["binding"]["invocation"]["policy_version"] = json!(999);
    accepted.push((
        "invocation policy version 999",
        CheckpointPayloadV1::from_plan_bytes(&intent, &serde_json::to_vec(&invocation).unwrap())
            .is_ok(),
    ));

    let mut owned_handle = baseline.clone();
    owned_handle["managed"]["owned"]["handle"]["configuration_fingerprint_version"] = json!(999);
    accepted.push((
        "owned handle fingerprint version 999",
        CheckpointPayloadV1::from_plan_bytes(&intent, &serde_json::to_vec(&owned_handle).unwrap())
            .is_ok(),
    ));

    let mut owned_observation = baseline.clone();
    owned_observation["managed"]["owned"]["observation"]["configuration_fingerprint_version"] =
        json!(999);
    accepted.push((
        "owned observation fingerprint version 999",
        CheckpointPayloadV1::from_plan_bytes(
            &intent,
            &serde_json::to_vec(&owned_observation).unwrap(),
        )
        .is_ok(),
    ));

    let mut import_handle = baseline.clone();
    let import_operation = import_handle["managed"]["worker"]["attempt"]["attempt_id"].clone();
    let mut import_owned = import_handle["managed"]["owned"].clone();
    import_owned["handle"]["operation"] = import_operation.clone();
    import_handle["managed"]["import"] = json!({"operation_id":import_operation,"owned":import_owned,"start_issued":true,"completed":false});
    import_handle["managed"]["import"]["owned"]["handle"]["configuration_fingerprint_version"] =
        json!(999);
    accepted.push((
        "import owned handle fingerprint version 999",
        CheckpointPayloadV1::from_plan_bytes(&intent, &serde_json::to_vec(&import_handle).unwrap())
            .is_ok(),
    ));

    let mut import_observation = baseline.clone();
    let import_operation = import_observation["managed"]["worker"]["attempt"]["attempt_id"].clone();
    let mut import_owned = import_observation["managed"]["owned"].clone();
    import_owned["handle"]["operation"] = import_operation.clone();
    import_observation["managed"]["import"] = json!({"operation_id":import_operation,"owned":import_owned,"start_issued":true,"completed":false});
    import_observation["managed"]["import"]["owned"]["observation"]["configuration_fingerprint_version"] =
        json!(999);
    accepted.push((
        "import owned observation fingerprint version 999",
        CheckpointPayloadV1::from_plan_bytes(
            &intent,
            &serde_json::to_vec(&import_observation).unwrap(),
        )
        .is_ok(),
    ));

    let mut workspace_exclusion = baseline.clone();
    workspace_exclusion["managed"]["workspace_control_exclusion_version"] = json!(999);
    accepted.push((
        "workspace control exclusion version 999",
        CheckpointPayloadV1::from_plan_bytes(
            &intent,
            &serde_json::to_vec(&workspace_exclusion).unwrap(),
        )
        .is_ok(),
    ));

    // All inputs are the same valid, Store-derived worker checkpoint; only a
    // version discriminator changes in each case.
    assert!(!base_bytes.is_empty());
    assert!(
        accepted.iter().all(|(_, was_accepted)| !was_accepted),
        "unsupported version cases accepted: {:?}",
        accepted
            .iter()
            .filter(|(_, accepted)| *accepted)
            .map(|(name, _)| name)
            .collect::<Vec<_>>()
    );

    let mut supported = Vec::new();
    for version in [1, 2] {
        let mut candidate = baseline.clone();
        candidate["managed"]["binding"]["invocation"]["policy_version"] = json!(version);
        supported.push((
            format!("invocation policy version {version}"),
            CheckpointPayloadV1::from_plan_bytes(&intent, &serde_json::to_vec(&candidate).unwrap())
                .is_ok(),
        ));
    }
    for version in [1, 2, 3] {
        let mut candidate = baseline.clone();
        candidate["managed"]["owned"]["handle"]["configuration_fingerprint_version"] =
            json!(version);
        candidate["managed"]["owned"]["observation"]["configuration_fingerprint_version"] =
            json!(version);
        supported.push((
            format!("owned fingerprint version {version}"),
            CheckpointPayloadV1::from_plan_bytes(&intent, &serde_json::to_vec(&candidate).unwrap())
                .is_ok(),
        ));
    }
    assert!(
        supported.iter().all(|(_, was_accepted)| *was_accepted),
        "supported historical/current version cases rejected: {:?}",
        supported
            .iter()
            .filter(|(_, accepted)| !accepted)
            .map(|(name, _)| name)
            .collect::<Vec<_>>()
    );
}

#[test]
fn journal_context_managed_checkpoint_roundtrips_and_rejects_incoherent_context() {
    let (intent, baseline) = versioned_managed_plan();
    let managed =
        CheckpointPayloadV1::from_plan_bytes(&intent, &serde_json::to_vec(&baseline).unwrap())
            .expect("valid Store-derived managed checkpoint");
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let submit = br#"{"action":"managed-context"}"#;

    // accept() must create revision zero from the exact initial(intent)
    // payload; a pre-populated managed checkpoint cannot mark the job accepted.
    let rejected_root = tempfile::tempdir().unwrap();
    let rejected_journal = open_journal(rejected_root.path());
    let ticket = match rejected_journal.reserve(operation(), submit).unwrap() {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("new managed operation requires a reservation"),
    };
    assert!(matches!(
        rejected_journal.accept(ticket, &intent, BindingInputs::None, &managed),
        Err(agentsandbox::broker::journal::JournalError::Conflict)
    ));
    let accepted_path = current_pointer_path(rejected_root.path(), operation())
        .parent()
        .unwrap()
        .join("accepted.json");
    assert!(
        !accepted_path.exists(),
        "invalid rev0 must not publish accepted marker"
    );
    assert!(matches!(
        rejected_journal.load(operation()),
        Err(agentsandbox::broker::journal::JournalError::Interrupted)
    ));

    let temporary = tempfile::tempdir().unwrap();
    let journal = open_journal(temporary.path());
    let ticket = match journal.reserve(operation(), submit).unwrap() {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("new managed operation requires a reservation"),
    };
    let accepted = journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .expect("valid initial checkpoint must accept");
    assert_eq!(accepted.head().revision(), 0);
    let loaded_initial = journal.load(operation()).unwrap().unwrap();
    assert_eq!(loaded_initial.head().revision(), 0);
    assert!(
        loaded_initial.checkpoint().canonical_bytes().unwrap()
            == initial.canonical_bytes().unwrap()
    );

    let advanced = journal
        .advance(operation(), accepted.head().clone(), &managed)
        .expect("valid managed checkpoint must advance");
    assert_eq!(advanced.head().revision(), 1);
    let loaded = journal.load(operation()).unwrap().unwrap();
    assert_eq!(loaded.head().revision(), 1);
    assert_eq!(
        loaded.checkpoint().canonical_bytes().unwrap(),
        managed.canonical_bytes().unwrap()
    );
    drop(journal);

    let reopened = open_journal(temporary.path());
    let recovered = reopened.load(operation()).unwrap().unwrap();
    assert_eq!(recovered.head().revision(), 1);
    assert_eq!(
        recovered.checkpoint().canonical_bytes().unwrap(),
        managed.canonical_bytes().unwrap()
    );

    // Each malformed context starts from a fresh accepted initial checkpoint;
    // only its managed payload changes, so the failure is a context refusal.
    type ContextMutation = (&'static str, Box<dyn Fn(&mut Value)>);
    let mutations: [ContextMutation; 6] = [
        (
            "tenant",
            Box::new(|plan| plan["managed"]["tenant_id"] = json!("tenant-other")),
        ),
        (
            "origin",
            Box::new(|plan| plan["managed"]["gateway_origin"] = json!("http://127.0.0.1:8788")),
        ),
        (
            "spec",
            Box::new(|plan| plan["managed"]["worker"]["spec_hash"] = json!("f".repeat(64))),
        ),
        (
            "owned operation",
            Box::new(|plan| {
                let attempt_id = plan["managed"]["worker"]["attempt"]["attempt_id"].clone();
                plan["managed"]["owned"]["handle"]["operation"] = attempt_id;
            }),
        ),
        (
            "handoff",
            Box::new(|plan| plan["managed"]["credential_digest"] = json!("e".repeat(64))),
        ),
        (
            "worker",
            Box::new(|plan| {
                let attempt_id = plan["managed"]["worker"]["attempt"]["attempt_id"].clone();
                let revision = plan["managed"]["worker"]["revision"].clone();
                plan["managed"]["pending"] = json!({
                    "version":1, "operation_id":Uuid::new_v4().to_string(),
                    "worker_id":"ffffffff-ffff-4fff-8fff-ffffffffffff",
                    "attempt_id":attempt_id,
                    "expected_revision":revision, "transition":{"action":"stop"}
                });
            }),
        ),
    ];
    for (label, mutate) in mutations {
        let mut changed = baseline.clone();
        mutate(&mut changed);
        let changed_checkpoint =
            CheckpointPayloadV1::from_plan_bytes(&intent, &serde_json::to_vec(&changed).unwrap())
                .unwrap_or_else(|error| {
                    panic!("{label} mutation must remain a parseable checkpoint: {error}")
                });
        let root = tempfile::tempdir().unwrap();
        let journal = open_journal(root.path());
        let ticket = match journal.reserve(operation(), submit).unwrap() {
            ReserveResult::New(ticket) => ticket,
            _ => panic!("new operation requires a reservation"),
        };
        let accepted = journal
            .accept(ticket, &intent, BindingInputs::None, &initial)
            .unwrap();
        assert!(
            matches!(
                journal.advance(operation(), accepted.head().clone(), &changed_checkpoint),
                Err(agentsandbox::broker::journal::JournalError::Conflict)
            ),
            "managed {label} mismatch must be refused"
        );
        assert_eq!(
            journal
                .load(operation())
                .unwrap()
                .unwrap()
                .head()
                .revision(),
            0
        );
    }

    // A scope with a different origin cannot accept even the initial checkpoint.
    let root = tempfile::tempdir().unwrap();
    let scope_mismatch = open_journal_at_scope(
        root.path(),
        scope_with("tenant-journal-test", "http://127.0.0.1:8788"),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    );
    let ticket = match scope_mismatch.reserve(operation(), submit).unwrap() {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("new scope mismatch case requires a reservation"),
    };
    assert!(matches!(
        scope_mismatch.accept(ticket, &intent, BindingInputs::None, &initial),
        Err(agentsandbox::broker::journal::JournalError::Conflict)
    ));
}

#[test]
fn journal_context_managed_checkpoint_live_floor_refuses_pointer_rollback() {
    let (intent, baseline) = versioned_managed_plan();
    let managed =
        CheckpointPayloadV1::from_plan_bytes(&intent, &serde_json::to_vec(&baseline).unwrap())
            .expect("valid Store-derived managed checkpoint");
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let submit = br#"{"action":"managed-floor"}"#;
    let temporary = tempfile::tempdir().unwrap();
    let journal = open_journal(temporary.path());
    let sibling = journal.clone();
    let ticket = match journal.reserve(operation(), submit).unwrap() {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("new operation requires a reservation"),
    };
    let accepted = journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .unwrap();
    let path = current_pointer_path(temporary.path(), operation());
    let valid_rev0_pointer = std::fs::read(&path).unwrap();
    let rev1 = journal
        .advance(operation(), accepted.head().clone(), &managed)
        .unwrap();
    let rev2 = journal
        .advance(operation(), rev1.head().clone(), &managed)
        .unwrap();
    assert_eq!(rev2.head().revision(), 2);

    // Restore the exact old, valid pointer in place. The graph remains valid;
    // only the live writer's acknowledged floor can identify the rollback.
    std::fs::write(&path, &valid_rev0_pointer).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), valid_rev0_pointer);
    for live in [&journal, &sibling] {
        assert!(matches!(
            live.load(operation()),
            Err(agentsandbox::broker::journal::JournalError::Conflict)
        ));
        assert!(matches!(
            live.reserve(operation(), submit),
            Err(agentsandbox::broker::journal::JournalError::Conflict)
        ));
        assert!(matches!(
            live.advance(operation(), rev2.head().clone(), &managed),
            Err(agentsandbox::broker::journal::JournalError::Conflict)
        ));
    }
    drop(sibling);
    drop(journal);

    // Process-reopen intentionally has no promised anti-restore floor. Since
    // revision zero is a valid prefix, recovery may read it conservatively.
    let reopened = open_journal(temporary.path());
    let old = reopened.load(operation()).unwrap().unwrap();
    assert_eq!(old.head().revision(), 0);
    assert_eq!(
        old.checkpoint().canonical_bytes().unwrap(),
        initial.canonical_bytes().unwrap()
    );
    assert!(matches!(
        reopened.reserve(operation(), submit).unwrap(),
        ReserveResult::Accepted(_)
    ));
}

fn current_pointer_path(root: &Path, operation: Uuid) -> std::path::PathBuf {
    std::fs::read_dir(root.join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("jobs")
        .join(operation.to_string())
        .join("current.json")
}

#[test]
fn plan_checkpoint_builder_accepts_known_nulls_but_rejects_unknown_nulls() {
    let legacy = legacy_plan();
    let encoded = serde_json::to_vec(&legacy).unwrap();
    let intent = ExecutionIntentV1::from_plan_bytes(&encoded).unwrap();
    let built = CheckpointPayloadV1::from_plan_bytes(&intent, &encoded).unwrap();
    assert_eq!(
        built.canonical_bytes().unwrap(),
        CheckpointPayloadV1::initial(&intent)
            .unwrap()
            .canonical_bytes()
            .unwrap()
    );

    let request: DecisionRequest = serde_json::from_value(legacy["request"].clone()).unwrap();
    let mut typed_plan = legacy.clone();
    typed_plan["request"] = serde_json::to_value(request.clone()).unwrap();
    let typed =
        CheckpointPayloadV1::from_plan_bytes(&intent, &serde_json::to_vec(&typed_plan).unwrap())
            .unwrap();
    assert_eq!(
        typed.canonical_bytes().unwrap(),
        CheckpointPayloadV1::initial(&intent)
            .unwrap()
            .canonical_bytes()
            .unwrap()
    );

    let mut approved_plan = legacy.clone();
    approved_plan["request"]["human_approved"] = json!(true);
    approved_plan["request"]["approval_hash"] = json!(agentsandbox::policy::request_hash(&request));
    let approved =
        CheckpointPayloadV1::from_plan_bytes(&intent, &serde_json::to_vec(&approved_plan).unwrap())
            .unwrap();
    assert!(approved.canonical_bytes().is_ok());

    let mut unknown_null = legacy;
    unknown_null["request"]["private_canary_82a1"] = Value::Null;
    let unknown_error =
        CheckpointPayloadV1::from_plan_bytes(&intent, &serde_json::to_vec(&unknown_null).unwrap())
            .unwrap_err();
    let unknown_diagnostic = format!("{unknown_error:?} {unknown_error}");
    assert!(!unknown_diagnostic.contains("private_canary_82a1"));

    let mut trailing = encoded;
    trailing.extend_from_slice(b" {\"trailing_private_canary_82a1\":true}");
    let trailing_error = CheckpointPayloadV1::from_plan_bytes(&intent, &trailing).unwrap_err();
    let trailing_diagnostic = format!("{trailing_error:?} {trailing_error}");
    assert!(!trailing_diagnostic.contains("trailing_private_canary_82a1"));
}

#[test]
fn none_binding_acceptance_reopens_with_same_intent_submit_and_checkpoint() {
    let temporary = tempfile::tempdir().expect("private temporary root");
    let operation_id = operation();
    let submit = br#"{"action":"submit","request":"synthetic"}"#;
    let intent = intent();
    let initial = CheckpointPayloadV1::initial(&intent).expect("valid initial checkpoint");
    let journal = open_journal(temporary.path());
    let ticket = match journal.reserve(operation_id, submit).expect("reserve") {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("new operation must return one live reservation ticket"),
    };

    let accepted = journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .expect("durable acceptance with explicit no-credential binding");
    assert_eq!(accepted.operation_id(), operation_id);
    assert_eq!(accepted.intent_hash(), intent.intent_hash().unwrap());
    assert_eq!(accepted.submit_hash().len(), 64);
    assert_eq!(accepted.binding_manifest().kind(), BindingKindV1::None);
    assert!(accepted.binding_manifest().binding_id().is_none());
    assert!(accepted.binding_manifest().file_name().is_none());
    assert!(accepted.binding_manifest().source_sha().is_none());
    assert!(accepted.binding_manifest().stored_sha().is_none());
    assert!(accepted.binding_manifest().handoff_sha().is_none());
    assert!(accepted.binding_manifest().names().is_empty());
    assert_eq!(accepted.binding_manifest().stored_bytes(), 0);
    assert!(accepted.binding_source().is_none());
    assert_eq!(accepted.head().revision(), 0);
    assert_eq!(
        accepted.checkpoint().canonical_bytes().unwrap(),
        initial.canonical_bytes().unwrap()
    );

    drop(journal);
    let reopened = open_journal(temporary.path());
    let loaded = reopened
        .load(operation_id)
        .expect("valid accepted job can be reopened")
        .expect("accepted job exists");
    assert_eq!(loaded.operation_id(), operation_id);
    assert_eq!(loaded.intent_hash(), accepted.intent_hash());
    assert_eq!(loaded.submit_hash(), accepted.submit_hash());
    assert_eq!(loaded.binding_manifest().kind(), BindingKindV1::None);
    assert_eq!(loaded.head().revision(), 0);
    assert_eq!(
        loaded.checkpoint().canonical_bytes().unwrap(),
        accepted.checkpoint().canonical_bytes().unwrap()
    );
    assert!(matches!(
        reopened.reserve(operation_id, submit).unwrap(),
        ReserveResult::Accepted(_)
    ));
    assert!(matches!(
        reopened.reserve(operation_id, br#"{"action":"different"}"#),
        Err(agentsandbox::broker::journal::JournalError::Conflict)
    ));
    let namespace = temporary.path().join("broker-journal-v1");
    let scope_path = std::fs::read_dir(namespace)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let current = scope_path
        .join("jobs")
        .join(operation_id.to_string())
        .join("current.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(current).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o600);
        assert_eq!(metadata.nlink(), 1);
    }
}

#[test]
fn reservation_ticket_is_live_once_then_interrupted_after_drop_or_reopen() {
    let temporary = tempfile::tempdir().expect("private temporary root");
    let submit = br#"{"action":"submit","request":"synthetic"}"#;
    let journal = open_journal(temporary.path());
    let ticket = match journal.reserve(operation(), submit).unwrap() {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("first reserve returns the live ticket"),
    };

    assert!(matches!(
        journal.reserve(operation(), submit).unwrap(),
        ReserveResult::InProgress
    ));
    assert!(matches!(
        journal.reserve(operation(), br#"{"action":"different"}"#),
        Err(agentsandbox::broker::journal::JournalError::Conflict)
    ));
    assert!(matches!(
        Journal::open(
            &Directory::open_existing(&private_root(temporary.path())).unwrap(),
            scope(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new()).unwrap(),
        ),
        Err(agentsandbox::broker::journal::JournalError::Busy)
    ));

    drop(ticket);
    assert!(matches!(
        journal.reserve(operation(), submit).unwrap(),
        ReserveResult::Interrupted
    ));
    drop(journal);
    let reopened = open_journal(temporary.path());
    assert!(matches!(
        reopened.reserve(operation(), submit).unwrap(),
        ReserveResult::Interrupted
    ));
}

#[test]
fn checkpoint_cas_accepts_only_the_confirmed_chain_and_replays_original_receipt() {
    let temporary = tempfile::tempdir().expect("private temporary root");
    let submit = br#"{"action":"submit","request":"synthetic"}"#;
    let intent = intent();
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let approved = approved_checkpoint(&intent);
    let journal = open_journal(temporary.path());
    let ticket = match journal.reserve(operation(), submit).unwrap() {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("new operation needs its reservation ticket"),
    };
    let accepted = journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .unwrap();
    let revision0 = accepted.head().clone();

    let receipt1 = journal
        .advance(operation(), revision0.clone(), &approved)
        .expect("a valid payload can be checkpointed");
    let receipt2 = journal
        .advance(operation(), receipt1.head().clone(), &approved)
        .expect("the current head advances linearly");
    assert_eq!(receipt1.head().revision(), 1);
    assert_eq!(receipt2.head().revision(), 2);

    let replay = journal
        .advance(operation(), revision0.clone(), &approved)
        .expect("an exact historical CAS returns its original receipt");
    assert_eq!(replay.head(), receipt1.head());
    assert!(matches!(
        journal.advance(operation(), revision0, &initial),
        Err(agentsandbox::broker::journal::JournalError::Conflict)
    ));
    assert_eq!(
        journal.load(operation()).unwrap().unwrap().head(),
        receipt2.head()
    );
    assert_eq!(
        journal
            .load(operation())
            .unwrap()
            .unwrap()
            .checkpoint()
            .canonical_bytes()
            .unwrap(),
        approved.canonical_bytes().unwrap()
    );
}

#[test]
fn submit_literals_and_caps_are_rejected_before_job_creation() {
    let temporary = tempfile::tempdir().expect("private temporary root");
    let secret = "synthetic-control-canary-7f0d".to_owned();
    let directory = Directory::open_existing(&private_root(temporary.path())).unwrap();
    let journal = Journal::open(
        &directory,
        scope(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(vec![secret.clone()]).unwrap(),
    )
    .unwrap();
    let result = journal.reserve(operation(), secret.as_bytes());
    assert!(matches!(
        result,
        Err(agentsandbox::broker::journal::JournalError::ControlSecret)
    ));

    let jobs = std::fs::read_dir(temporary.path().join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("jobs");
    assert_eq!(std::fs::read_dir(jobs).unwrap().count(), 0);
    assert!(matches!(
        JournalLimitsV1::new(0, 1, 1, 1, 3, 1),
        Err(agentsandbox::broker::journal::JournalError::InvalidInput)
    ));
    let oversized = vec![b'x'; 256 * 1024 + 1];
    assert!(matches!(
        journal.reserve(operation(), &oversized),
        Err(agentsandbox::broker::journal::JournalError::TooLarge)
    ));
    let namespace = temporary.path().join("broker-journal-v1");
    let scope_path = std::fs::read_dir(namespace)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        std::fs::read_dir(scope_path.join("jobs")).unwrap().count(),
        0
    );
}

#[test]
fn configured_job_entry_revision_and_byte_limits_prevent_publication() {
    use agentsandbox::broker::journal::JournalError;

    let jobs_root = tempfile::tempdir().unwrap();
    let jobs_limits = JournalLimitsV1::new(64 * 1024, 12, 4, 128 * 1024, 24, 1).unwrap();
    let jobs_journal = open_journal_with_limits(jobs_root.path(), jobs_limits);
    let first_ticket = match jobs_journal
        .reserve(operation(), br#"{"action":"partial-reservation"}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("first operation must reserve a ticket"),
    };
    assert!(matches!(
        jobs_journal.reserve(second_operation(), br#"{"action":"over-job-cap"}"#),
        Err(JournalError::LimitExceeded)
    ));
    let jobs_path = std::fs::read_dir(jobs_root.path().join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("jobs");
    assert_eq!(std::fs::read_dir(jobs_path).unwrap().count(), 1);
    drop(first_ticket);

    let entries_root = tempfile::tempdir().unwrap();
    let entries_limits = JournalLimitsV1::new(64 * 1024, 2, 1, 128 * 1024, 16, 2).unwrap();
    let entries_journal = open_journal_with_limits(entries_root.path(), entries_limits);
    let entries_intent = intent();
    let entries_initial = CheckpointPayloadV1::initial(&entries_intent).unwrap();
    let entries_ticket = match entries_journal
        .reserve(operation(), br#"{"action":"entry-cap"}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("operation must reserve within the partial-entry cap"),
    };
    assert!(matches!(
        entries_journal.accept(
            entries_ticket,
            &entries_intent,
            BindingInputs::None,
            &entries_initial,
        ),
        Err(JournalError::LimitExceeded)
    ));
    let entries_jobs = std::fs::read_dir(entries_root.path().join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("jobs");
    let entry_job = std::fs::read_dir(entries_jobs)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(std::fs::read_dir(entry_job).unwrap().count(), 1);

    let revisions_root = tempfile::tempdir().unwrap();
    let revisions_limits = JournalLimitsV1::new(64 * 1024, 12, 1, 128 * 1024, 24, 2).unwrap();
    let revisions_journal = open_journal_with_limits(revisions_root.path(), revisions_limits);
    let revisions_intent = intent();
    let revisions_initial = CheckpointPayloadV1::initial(&revisions_intent).unwrap();
    let revisions_ticket = match revisions_journal
        .reserve(operation(), br#"{"action":"revision-cap"}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("operation must reserve within the revision cap"),
    };
    let revisions_accepted = revisions_journal
        .accept(
            revisions_ticket,
            &revisions_intent,
            BindingInputs::None,
            &revisions_initial,
        )
        .unwrap();
    assert!(matches!(
        revisions_journal.advance(
            operation(),
            revisions_accepted.head().clone(),
            &revisions_initial,
        ),
        Err(JournalError::LimitExceeded)
    ));
    let revisions_job = std::fs::read_dir(revisions_root.path().join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("jobs")
        .join(operation().to_string());
    assert!(!revisions_job.join("rev-00000000000000000001.json").exists());
    assert_eq!(
        revisions_journal
            .load(operation())
            .unwrap()
            .unwrap()
            .head()
            .revision(),
        0
    );

    let bytes_root = tempfile::tempdir().unwrap();
    let mut large_plan = legacy_plan();
    let large_argument = "x".repeat(30 * 1024);
    let argv = json!(["/usr/bin/echo", large_argument]);
    large_plan["options"]["argv"] = argv.clone();
    large_plan["request"]["command"]["argv"] = argv;
    let large_intent =
        ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&large_plan).unwrap()).unwrap();
    let large_initial = CheckpointPayloadV1::initial(&large_intent).unwrap();
    let cap = (large_intent.canonical_bytes().unwrap().len()
        + large_initial.canonical_bytes().unwrap().len()
        + 16 * 1024) as u64;
    let byte_limits = JournalLimitsV1::new(cap, 12, 4, cap + 128 * 1024, 24, 2).unwrap();
    let bytes_journal = open_journal_with_limits(bytes_root.path(), byte_limits);
    let bytes_ticket = match bytes_journal
        .reserve(operation(), br#"{"action":"byte-cap"}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("operation must reserve within the byte cap"),
    };
    let bytes_accepted = bytes_journal
        .accept(
            bytes_ticket,
            &large_intent,
            BindingInputs::None,
            &large_initial,
        )
        .expect("measured cap admits the initial graph");
    assert!(matches!(
        bytes_journal.advance(operation(), bytes_accepted.head().clone(), &large_initial,),
        Err(JournalError::LimitExceeded)
    ));
    assert_eq!(
        bytes_journal
            .load(operation())
            .unwrap()
            .unwrap()
            .head()
            .revision(),
        0
    );
    let bytes_job = std::fs::read_dir(bytes_root.path().join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("jobs")
        .join(operation().to_string());
    assert!(!bytes_job.join("rev-00000000000000000001.json").exists());
}

#[test]
fn manual_binding_canonicalizes_source_and_reloads_without_the_source_path() {
    let temporary = tempfile::tempdir().expect("private temporary root");
    let source = tempfile::NamedTempFile::new().expect("private source file");
    let raw = b"# synthetic comment\nFIRST_KEY=first-value-123\n\n SECOND_KEY=second-value-456 \n";
    std::fs::write(source.path(), raw).unwrap();
    let source_path = source.path().canonicalize().unwrap();
    let private_source = PrivateFile::open(&source_path, 64 * 1024).unwrap();

    let mut plan = legacy_plan();
    plan["options"]["secret_env_file"] = json!(source_path.to_string_lossy());
    plan["options"]["profile"] = json!("observe");
    plan["options"]["network"] = json!("bridge");
    plan["request"]["profile"] = json!("observe");
    let intent = ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap();
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let journal = open_journal(temporary.path());
    let ticket = match journal
        .reserve(operation(), br#"{"action":"manual-binding"}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("new operation requires a reservation ticket"),
    };
    let accepted = journal
        .accept(
            ticket,
            &intent,
            BindingInputs::Manual {
                source: &private_source,
            },
            &initial,
        )
        .expect("manual binding is copied and canonicalized");

    assert_eq!(accepted.binding_manifest().kind(), BindingKindV1::Manual);
    assert_eq!(
        accepted
            .binding_manifest()
            .names()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["FIRST_KEY", "SECOND_KEY"]
    );
    assert_eq!(accepted.binding_manifest().source_sha().unwrap().len(), 64);
    assert_eq!(
        accepted.binding_manifest().stored_sha(),
        accepted.binding_manifest().handoff_sha()
    );
    let stored_bytes = accepted.binding_source().unwrap().bytes().to_vec();
    assert!(stored_bytes == b"FIRST_KEY=first-value-123\nSECOND_KEY=second-value-456\n");
    assert_ne!(
        accepted.binding_manifest().source_sha(),
        accepted.binding_manifest().stored_sha()
    );

    drop(private_source);
    std::fs::remove_file(&source_path).unwrap();
    drop(source);
    let accepted_head = accepted.head().clone();
    drop(accepted);
    let durable_blob = durable_binding_path(temporary.path(), operation(), "manual-env-v1.bin");
    std::fs::remove_file(&durable_blob).unwrap();
    drop(journal);

    let recovered = scan_recovery(temporary.path());
    assert!(
        recovered.incomplete(),
        "a missing opaque blob is incomplete"
    );
    let recovered_job = recovered
        .jobs()
        .iter()
        .find(|job| job.operation_id() == operation())
        .expect("metadata-valid job remains available for STOP projection");
    assert_eq!(recovered_job.head(), &accepted_head);
    assert!(
        recovered_job.stops().is_empty(),
        "no Arm receipt was recorded"
    );
    drop(recovered);

    let reopened = open_journal(temporary.path());
    assert!(
        reopened.load(operation()).is_err(),
        "business replay still requires the durable credential blob"
    );
}

#[test]
fn provider_binding_preserves_exact_record_and_reports_actual_handoff_digest() {
    let temporary = tempfile::tempdir().expect("private temporary root");
    let binding_id = Uuid::parse_str("66666666-6666-4666-8666-666666666666").unwrap();
    let env_bytes = b"AI_GUARD_PROVIDER_BOOTSTRAP={\"version\":1,\"client\":\"codex\",\"auth_mode\":\"api\",\"secret\":\"synthetic-provider-key-987654321\"}\n";
    let secrets = vec!["synthetic-provider-key-987654321"];
    let record = json!({
        "version": 1,
        "binding_id": binding_id,
        "env_bytes": env_bytes.to_vec(),
        "secrets": secrets,
    });
    let raw_record = serde_json::to_vec(&record).unwrap();
    let source = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(source.path(), &raw_record).unwrap();
    let source_path = source.path().canonicalize().unwrap();
    let private_source = PrivateFile::open(&source_path, 512 * 1024).unwrap();

    let mut plan: Value =
        serde_json::from_str(include_str!("fixtures/broker_intent-plan-v3.json")).unwrap();
    plan["provider_credential"]["digest"] = json!(sha256(&raw_record));
    let intent = ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap();
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let journal = open_journal(temporary.path());
    let ticket = match journal
        .reserve(operation(), br#"{"action":"provider-binding"}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("new operation requires a reservation ticket"),
    };
    let accepted = journal
        .accept(
            ticket,
            &intent,
            BindingInputs::Provider {
                source: &private_source,
            },
            &initial,
        )
        .expect("exact provider binding record is durably copied");
    let manifest = accepted.binding_manifest();
    assert_eq!(manifest.kind(), BindingKindV1::Provider);
    assert_eq!(manifest.binding_id(), Some(binding_id));
    assert_eq!(
        manifest.file_name(),
        Some("66666666-6666-4666-8666-666666666666.json")
    );
    assert_eq!(
        manifest
            .names()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["AI_GUARD_PROVIDER_BOOTSTRAP"]
    );
    let record_sha = sha256(&raw_record);
    let handoff_sha = sha256(env_bytes);
    assert_eq!(manifest.source_sha(), Some(record_sha.as_str()));
    assert_eq!(manifest.stored_sha(), manifest.source_sha());
    assert_eq!(manifest.handoff_sha(), Some(handoff_sha.as_str()));
    assert_eq!(manifest.stored_bytes(), raw_record.len() as u64);
    assert!(accepted.binding_source().unwrap().bytes() == raw_record);

    // The reader inventories binding names without opening the provider blob.
    // An unexpected private regular file marks this job incomplete while the
    // manifest-validated STOP projection remains available.
    let accepted_head = accepted.head().clone();
    let unexpected = durable_binding_path(temporary.path(), operation(), "unexpected-test-entry");
    std::fs::write(&unexpected, b"synthetic opaque fixture").unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&unexpected, std::fs::Permissions::from_mode(0o600)).unwrap();
    drop(accepted);
    drop(private_source);
    std::fs::remove_file(source_path).unwrap();
    drop(source);
    drop(journal);
    let recovered = scan_recovery(temporary.path());
    assert!(recovered.incomplete());
    let recovered_job = recovered
        .jobs()
        .iter()
        .find(|job| job.operation_id() == operation())
        .expect("extra binding entry does not discard the proven job head");
    assert_eq!(recovered_job.head(), &accepted_head);
    assert!(recovered_job.stops().is_empty());
    drop(recovered);
    let journal_root = private_root(temporary.path());
    let directory = Directory::open_existing(&journal_root).unwrap();
    let normal_open = Journal::open(
        &directory,
        scope(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    );
    assert!(matches!(
        normal_open,
        Err(agentsandbox::broker::journal::JournalError::Corrupt)
    ));

    let blocked_root = tempfile::tempdir().unwrap();
    let hidden_control_value = "synthetic-control-canary-escape";
    let escaped = r#"{"version":1,"client":"codex","auth_mode":"api","secret":"synthetic-control-canary-escape"}"#
        .replace("-", r#"\u002d"#);
    let blocked_env = format!("AI_GUARD_PROVIDER_BOOTSTRAP={escaped}\n");
    let blocked_record = json!({
        "version": 1,
        "binding_id": binding_id,
        "env_bytes": blocked_env.as_bytes(),
        "secrets": ["unrelated-synthetic-value-123"],
    });
    let blocked_bytes = serde_json::to_vec(&blocked_record).unwrap();
    assert!(!String::from_utf8_lossy(&blocked_bytes).contains(hidden_control_value));
    let blocked_source_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(blocked_source_file.path(), &blocked_bytes).unwrap();
    let blocked_source_path = blocked_source_file.path().canonicalize().unwrap();
    let blocked_source = PrivateFile::open(&blocked_source_path, 512 * 1024).unwrap();
    let mut blocked_plan: Value =
        serde_json::from_str(include_str!("fixtures/broker_intent-plan-v3.json")).unwrap();
    blocked_plan["provider_credential"]["digest"] = json!(sha256(&blocked_bytes));
    let blocked_intent =
        ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&blocked_plan).unwrap()).unwrap();
    let blocked_initial = CheckpointPayloadV1::initial(&blocked_intent).unwrap();
    let blocked_control = ControlSecrets::new(vec![hidden_control_value.to_owned()]).unwrap();
    let blocked_journal = open_journal_with_control(blocked_root.path(), blocked_control);
    let blocked_ticket = match blocked_journal
        .reserve(operation(), br#"{"action":"escaped-control-value"}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("new operation requires a reservation ticket"),
    };
    assert!(matches!(
        blocked_journal.accept(
            blocked_ticket,
            &blocked_intent,
            BindingInputs::Provider {
                source: &blocked_source,
            },
            &blocked_initial,
        ),
        Err(agentsandbox::broker::journal::JournalError::ControlSecret)
    ));
    let namespace = blocked_root.path().join("broker-journal-v1");
    let scope_path = std::fs::read_dir(namespace)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let blocked_job = scope_path.join("jobs").join(operation().to_string());
    assert_eq!(std::fs::read_dir(&blocked_job).unwrap().count(), 1);
    assert!(!blocked_job.join("bindings").exists());
}

#[test]
fn journal_refuses_symlink_and_hardlink_replacements_without_repair() {
    use std::os::unix::fs::{MetadataExt, symlink};

    for hardlink in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let external = temporary.path().join("external-target");
        std::fs::write(&external, b"preserve this unrelated file").unwrap();
        let intent = intent();
        let initial = CheckpointPayloadV1::initial(&intent).unwrap();
        let journal = open_journal(temporary.path());
        let ticket = match journal
            .reserve(operation(), br#"{"action":"fs-substitution"}"#)
            .unwrap()
        {
            ReserveResult::New(ticket) => ticket,
            _ => panic!("new operation requires a reservation ticket"),
        };
        journal
            .accept(ticket, &intent, BindingInputs::None, &initial)
            .unwrap();
        let namespace = temporary.path().join("broker-journal-v1");
        let scope_path = std::fs::read_dir(namespace)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let current = scope_path
            .join("jobs")
            .join(operation().to_string())
            .join("current.json");
        std::fs::remove_file(&current).unwrap();
        if hardlink {
            std::fs::hard_link(&external, &current).unwrap();
        } else {
            symlink(&external, &current).unwrap();
        }

        assert!(journal.load(operation()).is_err());
        assert_eq!(
            std::fs::read(&external).unwrap(),
            b"preserve this unrelated file"
        );
        let metadata = std::fs::symlink_metadata(&current).unwrap();
        if hardlink {
            assert_eq!(metadata.nlink(), 2);
        } else {
            assert!(metadata.file_type().is_symlink());
        }
    }
}
