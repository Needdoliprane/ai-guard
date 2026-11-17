//! Real SQLite decision/admission/preparation/Arm + Journal fixture.
//! The runtime snapshot is explicit Created metadata; no engine/CLI is run.
use super::*;
use anyhow::Context;

/// Real OS executable used only as pinned invocation data. SyntheticReady
/// returns before run_inner/reopen; this fixture never executes this program.
/// This does not represent or qualify a Docker/Podman runtime.
pub(crate) const INVOCATION_DATA_PROGRAM: &str = "/usr/bin/true";
use crate::runtime::{Runtime, RuntimeKind};
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};
pub(crate) struct LiveFixture {
    pub _root: tempfile::TempDir,
    pub _store_root: tempfile::TempDir,
    pub store: crate::audit::Store,
    pub journal: Journal,
    pub intent: ExecutionIntentV1,
    pub head: RevisionRefV1,
    pub registration: crate::broker::enforcer::RegistrationV1,
}
fn live_namespace(
    namespace: Uuid,
    origin: &str,
    manual_credential: bool,
    runtime_program: Option<&std::path::Path>,
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
        origin,
        namespace,
        "launch-test",
    )
    .unwrap();
    let journal_directory = if manual_credential {
        for name in ["journals", "journals/launch-test", "config"] {
            fs::create_dir(directory.path().join(name)).unwrap();
            fs::set_permissions(
                directory.path().join(name),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        Directory::open_existing(&directory.path().join("journals/launch-test")).unwrap()
    } else {
        directory.clone()
    };
    let journal = Journal::open(
        &journal_directory,
        scope,
        &FsExclusions::new(),
        JournalLimitsV1::new(256 * 1024 * 1024, 256, 256, 512 * 1024 * 1024, 256, 128).unwrap(),
        ControlSecrets::new(vec![]).unwrap(),
    )
    .unwrap();
    let mut plan: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/broker_intent-plan-v3.json"
    ))
    .unwrap();
    plan["version"] = json!(1);
    plan["options"]["gateway"] = json!(origin);
    plan["options"]["profile"] = json!("strict");
    plan["options"]["network"] = json!("none");
    plan["options"]["runtime_endpoint"] = json!("unix:///not-opened/docker.sock");
    plan["options"]["provider"] = json!("generic");
    plan["options"]["argv"] = json!(["/usr/bin/id"]);
    if let Some(runtime_program) = runtime_program {
        plan["options"]["runtime_program"] = json!(runtime_program);
    }
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
    if manual_credential {
        let source = directory.path().join("manual-business.env");
        fs::write(
            &source,
            b"FIXTURE_KEY=public-synthetic-retirement-credential\n",
        )
        .unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        plan["options"]["secret_env_file"] = json!(source);
        plan["options"]["config_dir"] = json!(directory.path().join("config"));
        plan["options"]["profile"] = json!("observe");
        plan["options"]["network"] = json!("bridge");
        plan["request"]["profile"] = json!("observe");
    }
    let intent = ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap();
    // Full production approve/pin/hash/verify on a small immutable OS file;
    // no command is built or executed in this explicit SyntheticReady fixture.
    let invocation_started = std::time::Instant::now();
    eprintln!("d5-retirement-fixture phase=invocation.selection.start");
    let runtime = Runtime::with_program(
        RuntimeKind::Docker,
        std::path::Path::new(INVOCATION_DATA_PROGRAM),
    )
    .context("d5-retirement-fixture phase=invocation.selection.approve_pin")
    .unwrap();
    eprintln!(
        "d5-retirement-fixture phase=invocation.selection.done elapsed_ms={} invocation_bytes={}",
        invocation_started.elapsed().as_millis(),
        runtime.invocation().file_identity.length
    );
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
    let source = intent
        .initial_options()
        .run_options()
        .secret_env_file
        .as_ref()
        .map(|path| PrivateFile::open(path, 64 * 1024).unwrap());
    journal
        .accept_with_launch_profile(
            ticket,
            intent,
            source
                .as_ref()
                .map_or(BindingInputs::None, |source| BindingInputs::Manual {
                    source,
                }),
            &CheckpointPayloadV1::initial(intent).unwrap(),
            selection,
        )
        .unwrap()
}
pub(crate) fn live_fixture() -> LiveFixture {
    live_fixture_with_gateway("http://127.0.0.1:8787", false)
}

/// Request data only. The caller must enqueue this event, receive the real
/// Store HTTP ACK and persist that exact ACK before attempting Begin.
pub(crate) fn abandonment_event_data(
    intent: &ExecutionIntentV1,
    retirement: Uuid,
    arm: Uuid,
    epoch: Uuid,
    generation: u64,
) -> crate::model::Event {
    crate::broker::journal::retirement::event(
        intent,
        retirement,
        "broker.abandon-before-claim.requested",
        json!({
            "version":1, "operation_id":intent.operation_id(), "arm_id":arm,
            "expected_epoch":epoch, "generation":generation, "command_id":retirement,
        }),
    )
}

/// Optional service-test setup; the existing default fixture remains unchanged.
/// The manual binding is a real private file/manifest/blob, never a live proof.
pub(crate) fn live_fixture_with_gateway(origin: &str, manual_credential: bool) -> LiveFixture {
    live_fixture_with_gateway_inner(origin, manual_credential, None, false, false)
}

/// Synthetic Created checkpoint with no pending Arm in the journal. The
/// independent Store still contains this fixture's baseline Arm; callers use
/// this state only to prove refusal before any new Arm HTTP request.
pub(crate) fn live_fixture_with_created_helper_for_prospective(origin: &str) -> LiveFixture {
    live_fixture_with_gateway_inner(origin, true, None, false, true)
}

pub(crate) fn live_fixture_with_pending_arm_for_prospective(origin: &str) -> LiveFixture {
    live_fixture_with_gateway_inner(origin, true, None, true, false)
}

/// Keep the real Arm request durable while its actual Store reply is not yet
/// locally acknowledged. Registration remains data and is not used to start.
pub(crate) fn live_fixture_with_pending_arm() -> LiveFixture {
    live_fixture_with_gateway_inner("http://127.0.0.1:8787", false, None, true, false)
}

/// Service fixtures must freeze the same explicit program that their routing
/// profile requires. This changes the intention before hashing or acceptance;
/// the historical wrapper above deliberately retains its original projection.
pub(crate) fn live_fixture_with_gateway_and_program(
    origin: &str,
    manual_credential: bool,
    runtime_program: &std::path::Path,
) -> LiveFixture {
    live_fixture_with_gateway_inner(
        origin,
        manual_credential,
        Some(runtime_program),
        false,
        false,
    )
}

fn live_fixture_with_gateway_inner(
    origin: &str,
    manual_credential: bool,
    runtime_program: Option<&std::path::Path>,
    pending_arm: bool,
    created_only: bool,
) -> LiveFixture {
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
    let (_temp, journal, intent, selection) =
        live_namespace(namespace, origin, manual_credential, runtime_program);
    let accepted = accept(&journal, &intent, Some(&selection));
    let expected = accepted.launch_profile_binding().unwrap();
    let invocation_started = std::time::Instant::now();
    eprintln!("d5-retirement-fixture phase=invocation.authority.start");
    let runtime = Runtime::with_program(
        RuntimeKind::Docker,
        std::path::Path::new(INVOCATION_DATA_PROGRAM),
    )
    .context("d5-retirement-fixture phase=invocation.authority.approve_pin")
    .unwrap();
    eprintln!(
        "d5-retirement-fixture phase=invocation.authority.done elapsed_ms={} invocation_bytes={}",
        invocation_started.elapsed().as_millis(),
        runtime.invocation().file_identity.length
    );
    let options = intent.initial_options().run_options();
    let request = intent.request_data();
    let mut managed: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/broker_intent-managed-checkpoint.json"
    ))
    .unwrap();
    managed["tenant_id"] = json!(journal.scope().tenant_id());
    managed["gateway_origin"] = json!(journal.scope().gateway_origin());
    managed["credential_digest"] = json!(accepted.binding_manifest().handoff_sha());
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
    launch["network"] = json!(options.network);
    launch["credential_binding_id"] = json!(accepted.binding_manifest().binding_id());
    launch["credential_names"] = json!(accepted.binding_manifest().names());
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
    let clock = crate::broker::clock::HostClock::sample(
        &crate::broker::clock::NativeHostClock::new().unwrap(),
    )
    .unwrap();
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
        clock: clock.clone(),
        timeout_seconds: 120,
        deadline_ns_decimal: crate::broker::clock::checked_deadline(
            clock.continuous_ns().unwrap(),
            120,
        )
        .unwrap()
        .to_string(),
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
    if !created_only {
        history.push(pending(operations.last().unwrap()));
    }
    append(&mut head, runner, helpers.clone(), history.clone());
    let registration = if pending_arm || created_only {
        let SupervisionOperationV1::Arm {
            request,
            response: Some(response),
            owned,
            ..
        } = operations.last().unwrap()
        else {
            panic!("real central Arm result");
        };
        crate::broker::enforcer::RegistrationV1::new(
            sha(journal.scope().gateway_origin().as_bytes()),
            journal.scope().tenant_id().into(),
            request.clone(),
            response.receipt.clone(),
            authority.clone(),
            owned.clone(),
        )
        .unwrap()
    } else {
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
        let stop = &snapshot.stops()[0];
        crate::broker::enforcer::RegistrationV1::new(
            sha(journal.scope().gateway_origin().as_bytes()),
            journal.scope().tenant_id().into(),
            stop.arm().clone(),
            stop.receipt().clone(),
            stop.authority().clone(),
            stop.owned().clone(),
        )
        .unwrap()
    };
    LiveFixture {
        _root: _temp,
        _store_root: store_temp,
        store,
        journal,
        intent,
        head,
        registration,
    }
}
