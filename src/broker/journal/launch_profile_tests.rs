//! Synthetic private files only; no engine, provider or real control configuration.
use super::*;
use crate::runtime::{Runtime, RuntimeKind};
use serde_json::json;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

fn fixture() -> (
    tempfile::TempDir,
    Journal,
    ExecutionIntentV1,
    LaunchProfileSelection,
) {
    fixture_in_namespace(Uuid::from_u128(2))
}
fn fixture_in_namespace(
    namespace: Uuid,
) -> (
    tempfile::TempDir,
    Journal,
    ExecutionIntentV1,
    LaunchProfileSelection,
) {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let directory = Directory::open_existing(&temp.path().canonicalize().unwrap()).unwrap();
    let scope = JournalScopeV1::new(
        Uuid::from_u128(1),
        "tenant-launch-test",
        "http://127.0.0.1:8787",
        namespace,
        "launch-test",
    )
    .unwrap();
    let journal = Journal::open(
        &directory,
        scope,
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(vec![]).unwrap(),
    )
    .unwrap();
    let mut plan: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/broker_intent-plan-v3.json"
    ))
    .unwrap();
    plan["version"] = json!(1);
    plan["options"]["profile"] = json!("strict");
    plan["options"]["network"] = json!("none");
    plan["options"]["runtime_endpoint"] = json!("unix:///not-opened/docker.sock");
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
    let intent = ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap();
    // Approve this Rust test executable as data; it is never invoked as a CLI.
    let runtime =
        Runtime::with_program(RuntimeKind::Docker, &std::env::current_exe().unwrap()).unwrap();
    let selection =
        LaunchProfileSelection::from_runtime(&runtime, "unix:///not-opened/docker.sock").unwrap();
    (temp, journal, intent, selection)
}
fn accept(
    journal: &Journal,
    intent: &ExecutionIntentV1,
    selection: Option<&LaunchProfileSelection>,
) -> AcceptedJobV1 {
    let ReserveResult::New(ticket) = journal
        .reserve(intent.operation_id(), b"synthetic submit")
        .unwrap()
    else {
        panic!("fresh reservation required")
    };
    journal
        .accept_with_launch_profile(
            ticket,
            intent,
            BindingInputs::None,
            &CheckpointPayloadV1::initial(intent).unwrap(),
            selection,
        )
        .unwrap()
}

#[test]
fn accepted_v2_profile_survives_replay_and_legacy_bytes_remain_v1() {
    let (_temp, journal, intent, selection) = fixture();
    let accepted = accept(&journal, &intent, Some(&selection));
    let profile = accepted.launch_profile_binding().unwrap();
    assert_eq!(profile.parent_operation_id, intent.operation_id());
    assert_eq!(profile.journal_scope_hash, journal.0.scope_hash);
    let job = journal.job(intent.operation_id()).unwrap();
    let manifest: Bindings = journal.read(&job, "bindings.json", SMALL).unwrap();
    let record: Accepted = journal.read(&job, "accepted.json", SMALL).unwrap();
    assert_eq!(record.version, 2);
    assert_eq!(manifest.version, 2);
    let bytes = canonical(&manifest).unwrap();
    let mut independent = Sha256::new();
    independent.update(b"ai-guard:broker-bindings:v2\0");
    independent.update(&bytes);
    assert_eq!(record.bindings_hash, hex::encode(independent.finalize()));
    let mut double_nul = Sha256::new();
    double_nul.update(b"ai-guard:broker-bindings:v2\0\0");
    double_nul.update(&bytes);
    assert_ne!(record.bindings_hash, hex::encode(double_nul.finalize()));
    let ReserveResult::Accepted(replayed) = journal
        .reserve(intent.operation_id(), b"synthetic submit")
        .unwrap()
    else {
        panic!("exact replay must remain accepted")
    };
    assert_eq!(replayed.launch_profile_binding(), Some(profile));
    assert_eq!(replayed.head(), accepted.head());
    let saved_profile = profile.clone();
    let scope = journal.scope().clone();
    let next = journal
        .advance(
            intent.operation_id(),
            accepted.head().clone(),
            &CheckpointPayloadV1::initial(&intent).unwrap(),
        )
        .unwrap();
    assert_eq!(next.head().revision(), 1);
    assert_eq!(
        journal
            .load(intent.operation_id())
            .unwrap()
            .unwrap()
            .launch_profile_binding(),
        Some(&saved_profile)
    );
    drop(replayed);
    drop(accepted);
    drop(journal);
    let root = Directory::open_existing(&_temp.path().canonicalize().unwrap()).unwrap();
    let reopened = Journal::open(
        &root,
        scope,
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(vec![]).unwrap(),
    )
    .unwrap();
    let recovered = reopened.load(intent.operation_id()).unwrap().unwrap();
    assert_eq!(recovered.head(), next.head());
    assert_eq!(recovered.launch_profile_binding(), Some(&saved_profile));

    let (_legacy_temp, legacy, intent, _selection) = fixture();
    let accepted = accept(&legacy, &intent, None);
    assert!(accepted.launch_profile_binding().is_none());
    let job = legacy.job(intent.operation_id()).unwrap();
    let manifest: Bindings = legacy.read(&job, "bindings.json", SMALL).unwrap();
    let record: Accepted = legacy.read(&job, "accepted.json", SMALL).unwrap();
    let bytes = canonical(&manifest).unwrap();
    assert_eq!(record.version, 1);
    assert_eq!(
        record.bindings_hash,
        digest("ai-guard:broker-bindings:v1", &bytes)
    );
    assert!(
        !parse::<Value>(&bytes)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("docker_launch_profile")
    );
}

#[test]
fn blob_faults_preserve_stop_projection_but_unknown_budget_refuses_ack() {
    for variant in 0..10 {
        let (_temp, journal, intent, selection) = fixture();
        let accepted = accept(&journal, &intent, Some(&selection));
        let job = journal.job(intent.operation_id()).unwrap();
        let profile = job.path().join(crate::runtime::launch_profile::DIRECTORY);
        let file = profile.join(crate::runtime::launch_profile::FILE);
        match variant {
            0 => fs::remove_file(&file).unwrap(),
            1 => fs::write(&file, b"[]\n").unwrap(),
            2 => {
                let dir = job
                    .persistent_child(crate::runtime::launch_profile::DIRECTORY, false)
                    .unwrap()
                    .unwrap();
                dir.persistent_create("extra", b"not-opened").unwrap();
            }
            3 => fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap(),
            4 => {
                fs::rename(&profile, _temp.path().join("saved-profile")).unwrap();
            }
            5 => {
                fs::remove_file(&file).unwrap();
                symlink("not-followed", &file).unwrap();
            }
            6 => {
                fs::set_permissions(&profile, fs::Permissions::from_mode(0o755)).unwrap();
            }
            7 => {
                fs::rename(&profile, _temp.path().join("saved-profile")).unwrap();
                job.persistent_create(
                    crate::runtime::launch_profile::DIRECTORY,
                    b"not-a-directory",
                )
                .unwrap();
            }
            8 => {
                job.persistent_child(crate::runtime::launch_profile::DIRECTORY, false)
                    .unwrap()
                    .unwrap()
                    .persistent_child("unexpected", true)
                    .unwrap();
            }
            _ => {
                fs::hard_link(&file, _temp.path().join("linked-file")).unwrap();
            }
        }
        assert!(journal.load(intent.operation_id()).is_err());
        let stop = StopJournal::from_full(&journal);
        let snapshot = stop
            .recovery_status(intent.operation_id())
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.head(), accepted.head());
        // Same-length content changes cannot be detected by metadata-only STOP.
        if variant != 1 {
            assert!(snapshot.incomplete());
        }
        let result = stop.append_cancel(intent.operation_id(), Uuid::from_u128(0x42));
        if matches!(variant, 3 | 5 | 6 | 8 | 9) {
            assert!(
                matches!(result, Err(JournalError::UnsafeFilesystem)),
                "unknown budget must never produce ACK"
            );
            assert_eq!(
                stop.recovery_status(intent.operation_id())
                    .unwrap()
                    .unwrap()
                    .head(),
                accepted.head()
            );
        } else {
            let cancel = result.unwrap();
            assert_eq!(cancel.receipt().head().revision(), 1);
        }
    }
}

#[test]
fn engaged_profile_metadata_mismatch_refuses_full_and_stop() {
    let (_temp, journal, intent, selection) = fixture();
    let _accepted = accept(&journal, &intent, Some(&selection));
    let job = journal.job(intent.operation_id()).unwrap();
    let mut manifest: Bindings = journal.read(&job, "bindings.json", SMALL).unwrap();
    manifest
        .docker_launch_profile
        .as_mut()
        .unwrap()
        .journal_scope_hash = "f".repeat(64);
    fs::write(
        job.path().join("bindings.json"),
        canonical(&manifest).unwrap(),
    )
    .unwrap();
    assert!(journal.load(intent.operation_id()).is_err());
    assert!(
        StopJournal::from_full(&journal)
            .recovery_status(intent.operation_id())
            .is_err()
    );
}

#[test]
fn common_authority_profile_is_checked_before_worker_or_owned_exists() {
    strict_helper_graph_fixture();
}

fn strict_helper_graph_fixture() -> (tempfile::TempDir, Journal, ExecutionIntentV1, RevisionRefV1) {
    use crate::{
        audit::Store,
        broker::checkpoint::{
            DecisionAnswerV1, HelperPreparationV1, SupervisionCheckpointV1, SupervisionOperationV1,
        },
        control::*,
        redaction::Redactor,
    };
    let store_temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Store::create_supervised_for_tenant(
        &store_temp.path().join("audit.sqlite"),
        "tenant-launch-test",
        Redactor::new(vec![]),
        1000,
        &store_temp.path().join("before-v3.sqlite"),
    )
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
    let (_temp, journal, intent, selection) = fixture_in_namespace(namespace);
    let accepted = accept(&journal, &intent, Some(&selection));
    let expected = accepted.launch_profile_binding().unwrap();
    let runtime =
        Runtime::with_program(RuntimeKind::Docker, &std::env::current_exe().unwrap()).unwrap();
    let options = intent.initial_options().run_options();
    let request = intent.request_data();
    let mut managed: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/broker_intent-managed-checkpoint.json"
    ))
    .unwrap();
    managed["tenant_id"] = json!(journal.scope().tenant_id());
    managed["config_root"] = json!(options.config_dir);
    managed["pending"] = Value::Null;
    managed["admission"]["operation_id"] = json!(intent.operation_id());
    managed["admission"]["decision_request"] = json!(request);
    managed["admission"]["receipt"]["request_id"] = json!(request.request_id);
    managed["admission"]["receipt"]["request_hash"] = json!(crate::policy::request_hash(request));
    let authority = Uuid::from_u128(0x55);
    let launch = &mut managed["admission"]["launch"];
    launch["session_id"] = json!(intent.session());
    launch["command"] = json!(request.command);
    launch["authority_id"] = json!(authority);
    launch["image_ref"] = json!(options.image);
    launch["profile"] = json!(options.profile);
    launch["timeout_seconds"] = json!(options.timeout);
    launch["provider"] = json!(options.provider);
    launch["capture_output"] = json!(options.capture_output);
    launch["workspace_manifest_digest"] =
        json!(crate::control::content_hash("ai-guard:workspace-manifest:v1", &json!({})).unwrap());
    managed["binding"] = json!({"authority_id":authority,"kind":"docker","endpoint":expected.runtime_endpoint,
        "engine_id":"synthetic-engine","podman":null,"invocation":runtime.invocation(),"docker_launch_profile":expected});
    let payload = |managed: &Value| -> LoadedCheckpoint {
        let runner: CheckpointPayloadV1 = serde_json::from_value(
            json!({"version":1,"current_request":request,"managed":managed}),
        )
        .unwrap();
        assert!(runner.broker_context().worker.is_none());
        assert!(runner.broker_context().owned.is_none());
        LoadedCheckpoint::V1(runner)
    };
    let correct = payload(&managed);
    confirmed::context_checkpoint_with_launch(
        journal.scope(),
        &journal.0.control,
        &correct,
        &intent,
        accepted.binding_manifest(),
        Some(expected),
    )
    .unwrap();
    assert_eq!(
        confirmed::check_transition(
            &correct,
            &LoadedCheckpoint::V1(CheckpointPayloadV1::initial(&intent).unwrap()),
            false,
            None
        ),
        Err(JournalError::Conflict)
    );
    for variant in 0..5 {
        let mut changed = managed.clone();
        match variant {
            0 => {
                changed["binding"]
                    .as_object_mut()
                    .unwrap()
                    .remove("docker_launch_profile");
            }
            1 => {
                changed["binding"]["docker_launch_profile"]["journal_scope_hash"] =
                    json!("f".repeat(64))
            }
            2 => {
                changed["binding"]["docker_launch_profile"]["parent_operation_id"] =
                    json!(Uuid::from_u128(6))
            }
            3 => changed["binding"]["docker_launch_profile"]["directory_ref"] = json!("unexpected"),
            _ => {
                changed["binding"]["docker_launch_profile"]["invocation_fingerprint"] =
                    json!("e".repeat(64))
            }
        }
        assert_eq!(
            confirmed::context_checkpoint_with_launch(
                journal.scope(),
                &journal.0.control,
                &payload(&changed),
                &intent,
                accepted.binding_manifest(),
                Some(expected)
            ),
            Err(JournalError::Conflict)
        );
    }
    assert_eq!(
        confirmed::context_checkpoint_with_launch(
            journal.scope(),
            &journal.0.control,
            &correct,
            &intent,
            accepted.binding_manifest(),
            None
        ),
        Err(JournalError::Conflict),
        "Legacy means None exactly, not unverified context"
    );

    // Real Store admission and preparation snapshots. The helper's operation
    // differs from the Accepted parent; the worker has never reached Created.
    let policy = crate::policy::Policy::default();
    let stored = store
        .decision(
            request,
            Some((crate::policy::evaluate(&policy, request), Value::Null)),
        )
        .unwrap()
        .unwrap();
    let decision = stored.event.decision.unwrap();
    assert_eq!(decision.outcome, crate::model::Outcome::Allow);
    let receipt = DecisionReceiptV1 {
        version: 1,
        request_id: request.request_id.clone(),
        request_hash: crate::policy::request_hash(request),
        phase: DecisionPhaseV1::Initial,
        audit_seq: stored.seq,
        audit_hash: stored.hash,
    };
    managed["admission"]["receipt"] = json!(receipt);
    managed["admission"]["launch"]["security_preset"] = json!("fixed_sandbox_v2");
    managed["decision"] = json!(decision);
    let admission = SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: intent.intent_hash().unwrap(),
        admission: serde_json::from_value(managed["admission"].clone()).unwrap(),
    };
    let admitted = store
        .admit_supervised(
            &admission,
            &policy,
            decision.clone(),
            Value::Null,
            &QuotaPolicyV1::default(),
        )
        .unwrap();
    let parent = admitted.worker.as_ref().unwrap();
    let prepare = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: parent.worker_id.clone(),
        attempt_id: parent.attempt.attempt_id.clone(),
        expected_revision: parent.revision,
        transition: WorkerTransitionV1::Prepare,
    };
    let prepared = store.mutate_worker(&prepare).unwrap();
    let worker = prepared.worker.clone();
    assert_eq!(worker.state, WorkerStateV1::Preparing);
    managed["worker"] = json!(worker);
    let authority: crate::runtime::RuntimeAuthority =
        serde_json::from_value(managed["binding"].clone()).unwrap();
    let helper_id = Uuid::new_v4();
    assert_ne!(helper_id, intent.operation_id());
    let helper = helper_execution_policy_v1(&worker, HelperPurposeV1::Import, helper_id).unwrap();
    let permission = HelperPermissionV1::Admission {
        receipt: admitted.receipt.clone(),
    };
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
    let fingerprint = "c".repeat(64);
    let owned:crate::runtime::OwnedContainerSnapshot=serde_json::from_value(json!({"handle":{"authority_fingerprint":authority.fingerprint(),"session":intent.session(),"operation":helper_id,"id":inspection.container_id,"name":inspection.stable_name,"configuration_fingerprint_version":2},"observation":{"id":inspection.container_id,"name":inspection.stable_name,"image_id":inspection.image_id,"restart_count":0,"process":{"status":"created","running":false,"started_at":null,"finished_at":null,"exit_code":null,"oom_killed":false},"allocation":{"cpu":{"quota":u64::from(inspection.cpu_millis)*100,"period":100000},"memory_bytes":inspection.memory_bytes,"pids_limit":inspection.pids_limit},"configuration_fingerprint":fingerprint,"configuration_fingerprint_version":2,"observed_at":"2026-10-30T12:00:00Z","diagnostics":[]}})).unwrap();
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
        inspection,
        authority_binding_hash: authority.fingerprint(),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: fingerprint,
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
    let mut operations = vec![
        SupervisionOperationV1::Decision {
            request: request.clone(),
            response: Some(DecisionAnswerV1::new(decision, receipt).unwrap()),
        },
        SupervisionOperationV1::Admission {
            request: admission,
            response: Some(admitted),
        },
        SupervisionOperationV1::WorkerMutation {
            request: prepare,
            response: Some(prepared),
        },
    ];
    let make_helper_checkpoint =
        |managed: &Value,
         helpers: Vec<HelperPreparationV1>,
         operations: Vec<SupervisionOperationV1>| {
            let runner: CheckpointPayloadV1 = serde_json::from_value(
                json!({"version":1,"current_request":request,"managed":managed}),
            )
            .unwrap();
            assert!(runner.broker_context().owned.is_none());
            LoadedCheckpoint::V2(
                BrokerCheckpointV2::new(
                    runner,
                    SupervisionCheckpointV1::new(namespace, helpers, operations).unwrap(),
                )
                .unwrap(),
            )
        };
    let prepared_helper = make_helper_checkpoint(
        &managed,
        vec![HelperPreparationV1::new(helper.clone(), permission.clone(), None).unwrap()],
        operations.clone(),
    );
    confirmed::context_checkpoint_with_launch(
        journal.scope(),
        &journal.0.control,
        &prepared_helper,
        &intent,
        accepted.binding_manifest(),
        Some(expected),
    )
    .unwrap();
    let armed = store.arm_execution(&arm).unwrap();
    operations.push(SupervisionOperationV1::Arm {
        parent: worker,
        owned: owned.clone(),
        request: arm,
        response: Some(armed),
    });
    let helpers = vec![HelperPreparationV1::new(helper, permission, Some(owned)).unwrap()];
    let armed_helper = make_helper_checkpoint(&managed, helpers.clone(), operations.clone());
    confirmed::context_checkpoint_with_launch(
        journal.scope(),
        &journal.0.control,
        &armed_helper,
        &intent,
        accepted.binding_manifest(),
        Some(expected),
    )
    .unwrap();
    for variant in 0..5 {
        let mut changed = managed.clone();
        match variant {
            0 => {
                changed["binding"]
                    .as_object_mut()
                    .unwrap()
                    .remove("docker_launch_profile");
            }
            1 => {
                changed["binding"]["docker_launch_profile"]["journal_scope_hash"] =
                    json!("f".repeat(64))
            }
            2 => {
                changed["binding"]["docker_launch_profile"]["parent_operation_id"] =
                    json!(helper_id)
            }
            3 => changed["binding"]["docker_launch_profile"]["directory_ref"] = json!("unexpected"),
            _ => {
                changed["binding"]["docker_launch_profile"]["invocation_fingerprint"] =
                    json!("e".repeat(64))
            }
        }
        let changed = make_helper_checkpoint(&changed, helpers.clone(), operations.clone());
        assert_eq!(
            confirmed::context_checkpoint_with_launch(
                journal.scope(),
                &journal.0.control,
                &changed,
                &intent,
                accepted.binding_manifest(),
                Some(expected)
            ),
            Err(JournalError::Conflict)
        );
    }
    // Publish the same proven snapshots through the normal pending/ACK chain,
    // so blob-fault regressions can reuse a real durable Arm projection.
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let mut head = accepted.head().clone();
    let append = |head: &mut RevisionRefV1,
                  runner: &CheckpointPayloadV1,
                  helpers: Vec<HelperPreparationV1>,
                  operations: Vec<SupervisionOperationV1>| {
        let next = BrokerCheckpointV2::new(
            runner.clone(),
            SupervisionCheckpointV1::new(namespace, helpers, operations).unwrap(),
        )
        .unwrap();
        *head = journal
            .advance_supervised(intent.operation_id(), head.clone(), &next)
            .unwrap()
            .head()
            .clone();
    };
    let pending = |operation: &SupervisionOperationV1| {
        let mut value = serde_json::to_value(operation).unwrap();
        value["response"] = Value::Null;
        serde_json::from_value::<SupervisionOperationV1>(value).unwrap()
    };
    let mut history = Vec::new();
    for operation in &operations[..3] {
        history.push(pending(operation));
        append(&mut head, &initial, vec![], history.clone());
        *history.last_mut().unwrap() = operation.clone();
        append(&mut head, &initial, vec![], history.clone());
    }
    let runner = armed_helper.runner();
    let helper_prepared = HelperPreparationV1::new(
        helpers[0].policy().clone(),
        helpers[0].permission().clone(),
        None,
    )
    .unwrap();
    append(&mut head, runner, vec![helper_prepared], history.clone());
    history.push(pending(operations.last().unwrap()));
    append(&mut head, runner, helpers.clone(), history.clone());
    *history.last_mut().unwrap() = operations.last().unwrap().clone();
    append(&mut head, runner, helpers, history);
    let snapshot = StopJournal::from_full(&journal)
        .recovery_status(intent.operation_id())
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.head(), &head);
    assert_eq!(
        snapshot.stops().len(),
        1,
        "positive durable Arm projection before any blob fault"
    );
    (_temp, journal, intent, head)
}

#[test]
fn oversized_launch_blob_keeps_proven_arm_and_other_jobs_without_append_ack() {
    use std::os::unix::fs::MetadataExt;
    for scenario in 0..3 {
        let (_temp, journal, intent, head) = strict_helper_graph_fixture();
        let mut sibling = serde_json::to_value(&intent).unwrap();
        sibling["operation_id"] = json!(Uuid::from_u128(u128::MAX));
        let sibling = ExecutionIntentV1::from_bytes(&canonical(&sibling).unwrap()).unwrap();
        let sibling_job = accept(&journal, &sibling, None);
        let stop = StopJournal::from_full(&journal);
        let before = stop
            .recovery_status(intent.operation_id())
            .unwrap()
            .unwrap();
        assert_eq!(before.head(), &head);
        assert_eq!(before.stops().len(), 1);
        let job = journal.job(intent.operation_id()).unwrap();
        let path = job
            .path()
            .join(crate::runtime::launch_profile::DIRECTORY)
            .join(crate::runtime::launch_profile::FILE);
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        // Sparse length changes exercise metadata accounting, never allocate/read a
        // giant payload. The original bounded control records and Arm are untouched.
        let bytes = match scenario {
            0 => journal.0.limits.job_bytes + 1,
            1 => journal.0.limits.scope_bytes + 1,
            _ => 1_u64 << 40,
        };
        file.set_len(bytes).unwrap();
        let metadata = file.metadata().unwrap();
        assert_eq!(metadata.len(), bytes);
        assert_eq!(metadata.mode() & 0o7777, 0o600);
        assert_eq!(metadata.nlink(), 1);
        assert!(
            journal.load(intent.operation_id()).is_err(),
            "Full must refuse oversized blob"
        );
        let recovered = stop
            .recovery_status(intent.operation_id())
            .expect("blob size must not discard an independently proven STOP chain")
            .expect("accepted Arm projection remains available");
        assert_eq!(recovered.head(), &head);
        assert_eq!(recovered.stops().len(), 1);
        assert!(recovered.incomplete());
        assert!(
            matches!(
                stop.append_cancel(intent.operation_id(), Uuid::from_u128(0x1234)),
                Err(JournalError::LimitExceeded)
            ),
            "over-budget metadata can never produce append ACK"
        );
        assert_eq!(
            stop.recovery_status(intent.operation_id())
                .unwrap()
                .unwrap()
                .head(),
            &head,
            "refused append never advances the head"
        );
        let scope = journal.scope().clone();
        let limits = journal.0.limits.clone();
        let root = Directory::open_existing(&_temp.path().canonicalize().unwrap()).unwrap();
        drop(recovered);
        drop(before);
        drop(stop);
        drop(journal);
        let reader = super::recovery::StopRecoveryReaderV1::open_existing(
            &root,
            scope,
            &FsExclusions::new(),
            limits,
            ControlSecrets::new(vec![]).unwrap(),
        )
        .unwrap()
        .unwrap();
        let report = reader.scan();
        assert!(report.incomplete());
        assert_eq!(
            report.jobs().len(),
            2,
            "oversized blob must not stop discovery of the later bounded job"
        );
        let active = report
            .jobs()
            .iter()
            .find(|job| job.operation_id() == intent.operation_id())
            .unwrap();
        assert_eq!(active.head(), &head);
        assert_eq!(active.stops().len(), 1);
        let sibling = report
            .jobs()
            .iter()
            .find(|job| job.operation_id() == sibling.operation_id())
            .unwrap();
        assert_eq!(sibling.head(), sibling_job.head());
    }
}
