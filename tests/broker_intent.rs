//! Immutable execution-intent serialization and projection contracts.

use agentsandbox::{
    broker::intent::{
        ExecutionIntentV1, InitialOptionsV1, InitialRequestV1, IntentError, ProviderCredentialRefV1,
    },
    control::{AdmissionV1, WorkerMutationV1, WorkerTransitionV1},
    policy,
    runner::{CleanupErrorAudit, CleanupState, CleanupSummary, RunOptions, RunSummary},
    runtime::OwnedContainerSnapshot,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const GOLDEN_HASH: &str = "f6f8fa6cc4991a3d03a00655b5d57a5bb186e3e02bb2a1b4ac07adede936c616";
const PRIVATE_CANARY: &str = "intent-private-canary-is-not-a-real-secret";

fn plan() -> Value {
    serde_json::from_str(include_str!("fixtures/broker_intent-plan-v3.json")).unwrap()
}

fn legacy_plan_v1() -> Value {
    let mut value = plan();
    value["version"] = json!(1);
    value["options"]["profile"] = json!("strict");
    value["options"]["network"] = json!("none");
    value["options"]["provider"] = json!("generic");
    value["options"]["argv"] = json!(["/usr/bin/id"]);
    for field in [
        "client",
        "auth_mode",
        "client_profile",
        "providers_file",
        "prompt",
        "model",
    ] {
        value["options"].as_object_mut().unwrap().remove(field);
    }
    value["request"]["profile"] = json!("strict");
    value["request"]["agent_id"] = json!("generic-22222222-2222-4222-8222-222222222222");
    value["request"]["command"]["argv"] = json!(["/usr/bin/id"]);
    value["request"]
        .as_object_mut()
        .unwrap()
        .remove("provider_resolution");
    value["provider_input"] = Value::Null;
    value["provider_credential"] = Value::Null;
    value
}

fn intent_for(plan: &Value) -> ExecutionIntentV1 {
    ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(plan).unwrap())
        .expect("valid typed RunPlan projects to an immutable intent")
}

fn digest(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"ai-guard.execution-intent.v1\0");
    hash.update(bytes);
    hex::encode(hash.finalize())
}

#[test]
fn canonical_v1_matches_independent_golden_and_roundtrips_strictly() {
    let golden = include_bytes!("fixtures/broker_intent-v1.canonical.json");
    let golden = golden.strip_suffix(b"\n").unwrap_or(golden);
    let run_plan = plan();
    assert!(!std::path::Path::new("/definitely-absent/broker-intent/providers.json").exists());
    let intent = intent_for(&run_plan);
    intent.validate().unwrap();
    assert_eq!(intent.canonical_bytes().unwrap(), golden);
    assert_eq!(intent.intent_hash().unwrap(), GOLDEN_HASH);
    assert_eq!(digest(golden), GOLDEN_HASH);

    let decoded = ExecutionIntentV1::from_bytes(golden).unwrap();
    assert_eq!(decoded.canonical_bytes().unwrap(), golden);
    let wire: Value = serde_json::from_slice(&decoded.canonical_bytes().unwrap()).unwrap();
    let keys: Vec<_> = wire
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "initial_options",
            "initial_request",
            "operation_id",
            "plan_version",
            "provider_credential",
            "provider_input",
            "session",
            "version"
        ]
    );
    assert!(wire.get("managed").is_none());
    assert!(wire.get("credential_source").is_none());
    assert!(String::from_utf8_lossy(golden).contains("Résumé β\\nline two\\r\\n"));
}

#[test]
fn managed_checkpoint_mutations_do_not_change_the_intent_hash() {
    let original = plan();
    let baseline_hash = intent_for(&original).intent_hash().unwrap();
    let mut with_checkpoint = original.clone();
    with_checkpoint["managed"] = serde_json::from_str(include_str!(
        "fixtures/broker_intent-managed-checkpoint.json"
    ))
    .unwrap();
    // This must deserialize as the real strict RunPlan/ManagedCheckpoint,
    // including typed pending mutation, before it can serve as a checkpoint.
    let checkpoint_hash = intent_for(&with_checkpoint).intent_hash().unwrap();
    assert_eq!(checkpoint_hash, baseline_hash);

    let pending = WorkerMutationV1 {
        version: 1,
        operation_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
        worker_id: "88888888-8888-4888-8888-888888888888".into(),
        attempt_id: "99999999-9999-4999-8999-999999999999".into(),
        expected_revision: 2,
        transition: WorkerTransitionV1::Stop,
    };
    let mut pending_changed = with_checkpoint.clone();
    pending_changed["managed"]["pending"] = serde_json::to_value(pending).unwrap();
    assert_eq!(
        intent_for(&pending_changed).intent_hash().unwrap(),
        baseline_hash
    );

    let mut observation_changed = with_checkpoint.clone();
    let mut snapshot: OwnedContainerSnapshot = serde_json::from_value(json!({
        "handle": {
            "authority_fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "session": "22222222-2222-4222-8222-222222222222",
            "operation": "77777777-7777-4777-8777-777777777777",
            "id": "container-fixture-id",
            "name": "worker-fixture"
        },
        "observation": {
            "id": "container-fixture-id",
            "name": "worker-fixture",
            "image_id": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "restart_count": 0,
            "process": {
                "status": "created",
                "running": false,
                "started_at": null,
                "finished_at": null,
                "exit_code": null,
                "oom_killed": null
            },
            "allocation": {
                "cpu": {"quota": 2, "period": 1},
                "memory_bytes": 1073741824,
                "pids_limit": 256
            },
            "configuration_fingerprint": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "configuration_fingerprint_version": 2,
            "observed_at": "2026-10-30T12:00:00Z",
            "diagnostics": []
        }
    }))
    .unwrap();
    snapshot.observation.process.status = Some("running".into());
    snapshot.observation.process.running = Some(true);
    snapshot.observation.process.started_at = Some("2026-10-30T12:00:01Z".parse().unwrap());
    observation_changed["managed"]["owned"] = serde_json::to_value(snapshot).unwrap();
    assert_eq!(
        intent_for(&observation_changed).intent_hash().unwrap(),
        baseline_hash
    );

    let mut result_changed = with_checkpoint;
    let result = RunSummary {
        session_id: Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
        volume: "worker-volume-fixture".into(),
        exit_code: 17,
        timed_out: false,
        changed_files: Some(2),
        output_captured: false,
        cleanup: CleanupSummary {
            container: "container-fixture-id".into(),
            state: CleanupState::Unconfirmed,
            error: Some("fixture cleanup observation".into()),
            error_audit: CleanupErrorAudit::Unacknowledged,
        },
        postprocessing_error: Some("fixture postprocessing observation".into()),
        transport_stop_reason: None,
        control_deadline: None,
    };
    let mut encoded = serde_json::to_value(result).unwrap();
    assert!(encoded.get("control_deadline").is_none());
    let legacy: RunSummary = serde_json::from_value(encoded.clone()).unwrap();
    assert!(legacy.control_deadline.is_none());
    encoded["control_deadline"] = serde_json::json!({
        "deadline": "original_execution_deadline",
        "stop": "engine_acknowledged",
    });
    let read: RunSummary = serde_json::from_value(encoded).unwrap();
    assert_eq!(read.exit_code, 17);
    assert!(!read.timed_out);
    assert!(read.transport_stop_reason.is_none());
    assert_eq!(
        read.control_deadline,
        Some(agentsandbox::runner::ControlDeadlineObservation {
            deadline: agentsandbox::runner::ControlDeadlineKind::OriginalExecutionDeadline,
            stop: agentsandbox::runner::ControlStopObservation::EngineAcknowledged,
        })
    );
    result_changed["managed"]["result"] = serde_json::to_value(read).unwrap();
    assert_eq!(
        intent_for(&result_changed).intent_hash().unwrap(),
        baseline_hash
    );
}

#[test]
fn stable_execution_and_binding_fields_change_the_hash() {
    let original = plan();
    let original_hash = intent_for(&original).intent_hash().unwrap();

    let mut operation = original.clone();
    operation["operation_id"] = json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    assert_ne!(intent_for(&operation).intent_hash().unwrap(), original_hash);

    let mut session = original.clone();
    let new_session = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    session["session"] = json!(new_session);
    session["request"]["session_id"] = json!(new_session);
    session["request"]["sandbox_id"] = json!(new_session);
    session["request"]["agent_id"] = json!(format!("codex-{new_session}"));
    assert_ne!(intent_for(&session).intent_hash().unwrap(), original_hash);

    let mut timeout = original.clone();
    timeout["options"]["timeout"] = json!(601);
    timeout["provider_input"]["timeout"] = json!(601);
    assert_ne!(intent_for(&timeout).intent_hash().unwrap(), original_hash);

    let mut binding_digest = original.clone();
    binding_digest["provider_credential"]["digest"] =
        json!("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd");
    assert_ne!(
        intent_for(&binding_digest).intent_hash().unwrap(),
        original_hash
    );

    let mut binding_id = original.clone();
    binding_id["provider_credential"]["binding_id"] = json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    binding_id["provider_credential"]["file_name"] =
        json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.json");
    binding_id["request"]["provider_resolution"]["credential_binding_id"] =
        json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    assert_ne!(
        intent_for(&binding_id).intent_hash().unwrap(),
        original_hash
    );

    let mut control_path = original.clone();
    control_path["options"]["config_dir"] = json!("/private/other-control-root");
    control_path["provider_input"]["config_dir"] = json!("/private/other-control-root");
    assert_ne!(
        intent_for(&control_path).intent_hash().unwrap(),
        original_hash
    );
}

#[test]
fn malformed_versions_approvals_and_provider_relationships_are_refused() {
    let original = plan();
    for mutate in 0..28 {
        let mut invalid = original.clone();
        match mutate {
            0 => invalid["version"] = json!(4),
            1 => invalid["operation_id"] = json!("00000000-0000-0000-0000-000000000000"),
            2 => invalid["session"] = json!("00000000-0000-0000-0000-000000000000"),
            3 => invalid["options"]["approve_hash"] = json!("a".repeat(64)),
            4 => invalid["request"]["human_approved"] = json!(true),
            5 => invalid["request"]["approval_hash"] = json!("a".repeat(64)),
            6 => invalid["provider_input"]["argv"] = json!(["/bin/false"]),
            7 => invalid["provider_input"]["provider"] = json!("claude"),
            8 => invalid["provider_input"]["secret_env_file"] = json!("/private/key.env"),
            9 => invalid["provider_input"]["client"] = json!("claude-code"),
            10 => invalid["provider_input"]["auth_mode"] = json!("subscription"),
            11 => invalid["provider_input"]["client_profile"] = json!("different-profile"),
            12 => invalid["provider_input"]["prompt"] = json!("mismatched prompt"),
            13 => invalid["options"]["profile"] = json!("strict"),
            14 => invalid["request"]["profile"] = json!("strict"),
            15 => invalid["request"]["session_id"] = json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
            16 => invalid["request"]["command"]["cwd"] = json!("/other-workspace"),
            17 => invalid["request"]["provider_resolution"]["transport"] = json!("interactive_tty"),
            18 => invalid["provider_credential"]["digest"] = json!("A".repeat(64)),
            19 => invalid["provider_credential"]["file_name"] = json!("wrong.json"),
            20 => invalid["provider_input"]["model"] = json!("gpt-5"),
            21 => invalid["request"]["request_id"] = json!("00000000-0000-0000-0000-000000000000"),
            22 => invalid["options"]["provider"] = json!("claude"),
            23 => invalid["request"]["command"]["argv"][0] = json!("/bin/false"),
            24 => invalid["provider_input"]["image"] = json!("worker:other"),
            25 => invalid["provider_input"]["runtime"] = json!("podman"),
            26 => invalid["provider_input"]["project"] = json!("/other-workspace"),
            27 => invalid["provider_input"]["config_dir"] = json!("/other-control-root"),
            _ => unreachable!(),
        }
        assert!(
            ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&invalid).unwrap()).is_err(),
            "mutation {mutate} must be rejected"
        );
    }

    let mut prompt_changed = original.clone();
    let changed_prompt = "Résumé β\nline two\r\nchanged";
    prompt_changed["options"]["prompt"] = json!(changed_prompt);
    prompt_changed["provider_input"]["prompt"] = json!(changed_prompt);
    *prompt_changed["options"]["argv"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap() = json!(changed_prompt);
    *prompt_changed["request"]["command"]["argv"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap() = json!(changed_prompt);
    let changed_intent = intent_for(&prompt_changed);
    let changed_wire: Value =
        serde_json::from_slice(&changed_intent.canonical_bytes().unwrap()).unwrap();
    assert_eq!(changed_wire["initial_options"]["prompt"], changed_prompt);
    assert_eq!(changed_wire["provider_input"]["prompt"], changed_prompt);

    let mut argv_prompt_mismatch = original.clone();
    let prompt_without_argv_update = "Résumé β\nline two\r\nchanged";
    argv_prompt_mismatch["options"]["prompt"] = json!(prompt_without_argv_update);
    argv_prompt_mismatch["provider_input"]["prompt"] = json!(prompt_without_argv_update);
    assert!(
        ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&argv_prompt_mismatch).unwrap())
            .is_err()
    );

    let mut automatic = original.clone();
    automatic["options"]["client"] = json!("auto");
    automatic["provider_input"]["client"] = json!("auto");
    automatic["request"]["provider_resolution"]["provenance"]["client"] = json!("profile");
    assert!(ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&automatic).unwrap()).is_ok());

    let mut resolved_from_profile = original.clone();
    for options in ["options", "provider_input"] {
        resolved_from_profile[options]
            .as_object_mut()
            .unwrap()
            .remove("client");
        resolved_from_profile[options]
            .as_object_mut()
            .unwrap()
            .remove("auth_mode");
    }
    resolved_from_profile["request"]["provider_resolution"]["provenance"]["client"] =
        json!("profile");
    resolved_from_profile["request"]["provider_resolution"]["provenance"]["auth_mode"] =
        json!("profile");
    assert!(
        ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&resolved_from_profile).unwrap())
            .is_ok()
    );
}

#[test]
fn strict_wire_bounds_opaque_diagnostics_and_legacy_hashes_are_preserved() {
    let golden = include_bytes!("fixtures/broker_intent-v1.canonical.json");
    let mut unknown: Value = serde_json::from_slice(golden).unwrap();
    unknown["untrusted_extra"] = json!(PRIVATE_CANARY);
    let error = match ExecutionIntentV1::from_bytes(&serde_json::to_vec(&unknown).unwrap()) {
        Err(error) => error,
        Ok(_) => panic!("unknown intent field must be rejected"),
    };
    assert!(!error.to_string().contains(PRIVATE_CANARY));
    assert!(!format!("{error:?}").contains(PRIVATE_CANARY));

    let mut unknown_option: Value = serde_json::from_slice(golden).unwrap();
    unknown_option["initial_options"]["future_option"] = json!(PRIVATE_CANARY);
    assert!(ExecutionIntentV1::from_bytes(&serde_json::to_vec(&unknown_option).unwrap()).is_err());

    let mut unknown_request: Value = serde_json::from_slice(golden).unwrap();
    unknown_request["initial_request"]["future_request_field"] = json!(PRIVATE_CANARY);
    assert!(ExecutionIntentV1::from_bytes(&serde_json::to_vec(&unknown_request).unwrap()).is_err());

    let mut unknown_resolution: Value = serde_json::from_slice(golden).unwrap();
    unknown_resolution["initial_request"]["provider_resolution"]["future_resolution_field"] =
        json!(PRIVATE_CANARY);
    assert!(
        ExecutionIntentV1::from_bytes(&serde_json::to_vec(&unknown_resolution).unwrap()).is_err()
    );

    let duplicate_version = [b"{\"version\":1,".as_slice(), &golden[1..]].concat();
    assert!(ExecutionIntentV1::from_bytes(&duplicate_version).is_err());

    let mut unknown_version: Value = serde_json::from_slice(golden).unwrap();
    unknown_version["version"] = json!(2);
    assert!(ExecutionIntentV1::from_bytes(&serde_json::to_vec(&unknown_version).unwrap()).is_err());

    let mut v1_nil = legacy_plan_v1();
    v1_nil["operation_id"] = json!("00000000-0000-0000-0000-000000000000");
    assert!(ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&v1_nil).unwrap()).is_err());
    let mut v1_missing = legacy_plan_v1();
    v1_missing.as_object_mut().unwrap().remove("operation_id");
    assert!(ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&v1_missing).unwrap()).is_err());
    let v1_valid = legacy_plan_v1();
    assert!(ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&v1_valid).unwrap()).is_ok());
    let mut v2_valid = v1_valid.clone();
    v2_valid["version"] = json!(2);
    assert!(ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&v2_valid).unwrap()).is_ok());

    // Plans 1/2 retain manual provider labels, but may not add a managed
    // selection without the v3 frozen provider resolution and binding.
    let mut legacy_manual_provider = v1_valid.clone();
    legacy_manual_provider["options"]["provider"] = json!("claude");
    legacy_manual_provider["request"]["agent_id"] =
        json!("claude-22222222-2222-4222-8222-222222222222");
    assert!(
        ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&legacy_manual_provider).unwrap())
            .is_ok()
    );
    for version in [1, 2] {
        for (field, selected) in [
            ("client", json!("auto")),
            ("auth_mode", json!("api")),
            ("client_profile", json!("codex-api")),
            ("providers_file", json!("/definitely-absent/providers.json")),
            ("prompt", json!("frozen provider prompt")),
            ("model", json!("gpt-5")),
        ] {
            let mut invalid = v1_valid.clone();
            invalid["version"] = json!(version);
            invalid["options"][field] = selected.clone();
            assert!(
                ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&invalid).unwrap()).is_err(),
                "legacy plan {version} acquired managed selector {field}"
            );
        }
    }

    let mut legacy_options = plan()["options"].clone();
    legacy_options["unknown_historical_field"] = json!(true);
    assert!(serde_json::from_value::<RunOptions>(legacy_options).is_ok());

    let mut oversized_valid_plan = serde_json::to_vec(&plan()).unwrap();
    oversized_valid_plan.resize(32 * 1024 * 1024 + 1, b' ');
    assert!(matches!(
        ExecutionIntentV1::from_plan_bytes(&oversized_valid_plan),
        Err(IntentError::SourceTooLarge)
    ));

    let mut oversized_valid_intent = golden.to_vec();
    oversized_valid_intent.resize(256 * 1024 + 1, b' ');
    assert!(matches!(
        ExecutionIntentV1::from_bytes(&oversized_valid_intent),
        Err(IntentError::IntentTooLarge)
    ));

    let mut oversized_canonical_plan = plan();
    let large_prompt = "\u{0001}".repeat(15 * 1024);
    oversized_canonical_plan["options"]["prompt"] = json!(large_prompt);
    oversized_canonical_plan["provider_input"]["prompt"] = json!(large_prompt);
    *oversized_canonical_plan["options"]["argv"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap() = json!(large_prompt);
    *oversized_canonical_plan["request"]["command"]["argv"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap() = json!(large_prompt);
    let argv_json_len = serde_json::to_vec(&oversized_canonical_plan["options"]["argv"])
        .unwrap()
        .len();
    assert!(
        argv_json_len <= 96 * 1024,
        "argv JSON size is {argv_json_len}"
    );
    let oversized_plan_bytes = serde_json::to_vec(&oversized_canonical_plan).unwrap();
    assert!(oversized_plan_bytes.len() <= 32 * 1024 * 1024);
    assert!(matches!(
        ExecutionIntentV1::from_plan_bytes(&oversized_plan_bytes),
        Err(IntentError::IntentTooLarge)
    ));

    let old: AdmissionV1 = serde_json::from_str(include_str!(
        "fixtures/control/admission-fixed-sandbox-v1.json"
    ))
    .unwrap();
    assert_eq!(
        policy::request_hash(&old.decision_request),
        "f2b49355b13327290c05b7856040057dff6b15a7a73844c55d2028cab86793aa"
    );
    assert_eq!(
        serde_json::to_string(&old).unwrap(),
        include_str!("fixtures/control/admission-fixed-sandbox-v1.json").trim()
    );
}

#[test]
fn public_snapshot_deserialization_errors_do_not_echo_canary() {
    let canary = PRIVATE_CANARY;
    let mut leaks = Vec::new();
    macro_rules! require_opaque_error {
        ($label:literal, $result:expr) => {{
            let error = $result.unwrap_err();
            if error.to_string().contains(canary) {
                leaks.push(concat!($label, ":Display"));
            }
            if format!("{error:?}").contains(canary) {
                leaks.push(concat!($label, ":Debug"));
            }
        }};
    }
    let golden: Value =
        serde_json::from_slice(include_bytes!("fixtures/broker_intent-v1.canonical.json")).unwrap();

    let mut unknown_option = golden["initial_options"].clone();
    unknown_option[canary] = json!(true);
    require_opaque_error!(
        "options_unknown_key",
        serde_json::from_value::<InitialOptionsV1>(unknown_option)
    );

    let mut invalid_runtime = golden["initial_options"].clone();
    invalid_runtime["runtime"] = json!(canary);
    require_opaque_error!(
        "options_invalid_enum",
        serde_json::from_value::<InitialOptionsV1>(invalid_runtime)
    );

    let mut invalid_binding = golden["provider_credential"].clone();
    invalid_binding["binding_id"] = json!(canary);
    require_opaque_error!(
        "credential_invalid_uuid",
        serde_json::from_value::<ProviderCredentialRefV1>(invalid_binding)
    );

    let mut unknown_binding = golden["provider_credential"].clone();
    unknown_binding[canary] = json!(true);
    require_opaque_error!(
        "credential_unknown_key",
        serde_json::from_value::<ProviderCredentialRefV1>(unknown_binding)
    );

    let mut unknown_request = golden["initial_request"].clone();
    unknown_request[canary] = json!(true);
    require_opaque_error!(
        "initial_request_control",
        serde_json::from_value::<InitialRequestV1>(unknown_request)
    );

    let mut unknown_intent = golden;
    unknown_intent[canary] = json!(true);
    require_opaque_error!(
        "execution_intent_control",
        ExecutionIntentV1::from_bytes(&serde_json::to_vec(&unknown_intent).unwrap())
    );

    assert!(
        leaks.is_empty(),
        "snapshot diagnostics leaked in: {leaks:?}"
    );
}
