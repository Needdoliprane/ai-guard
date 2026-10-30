//! RED contract for the real broker submission preparation seam.

use agentsandbox::{
    broker::journal::{
        BindingInputs, BindingKindV1, ControlSecrets, FsExclusions, Journal, JournalScopeV1,
        ReserveResult,
    },
    control_fs::Directory,
    model::DecisionRequest,
    policy,
    provider::{AuthMode, ClientKind, CredentialSourceV1, ProviderConfigV1, ProviderProfileV1},
    runner::CheckpointPayloadV1,
    runner::{
        RunOptions,
        broker::{BrokerRunState, credentials_from_job, prepare_broker_submission},
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::PermissionsExt};
use tempfile::tempdir;
use uuid::Uuid;

fn legacy_plan() -> Value {
    let mut plan: Value = serde_json::from_str(include_str!("fixtures/broker_intent-plan-v3.json"))
        .expect("valid existing plan fixture");
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

#[test]
fn preparation_uses_imposed_operation_and_real_none_binding_journal() {
    let root = tempdir().expect("private test root");
    let project = root.path().join("project");
    let config = root.path().join("config");
    let journal_path = root.path().join("journal");
    for path in [&project, &config, &journal_path] {
        fs::create_dir(path).expect("create private directory");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("private mode");
    }
    fs::write(project.join("README.md"), b"synthetic project\n").unwrap();
    let project = project.canonicalize().unwrap();
    let config = config.canonicalize().unwrap();
    let journal_path = journal_path.canonicalize().unwrap();

    let mut plan = legacy_plan();
    plan["options"]["project"] = project.to_string_lossy().into_owned().into();
    plan["options"]["config_dir"] = config.to_string_lossy().into_owned().into();
    let options: RunOptions = serde_json::from_value(plan["options"].clone()).unwrap();
    let expected_options = options.clone();
    let operation_id = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
    let prepared = prepare_broker_submission(options, operation_id, &[])
        .expect("valid generic/no-credential plan prepares");
    assert_eq!(prepared.intent().operation_id(), operation_id);
    let checkpoint = prepared.initial_checkpoint().unwrap();
    let intent = prepared.intent();
    let canonical = intent.canonical_bytes().unwrap();

    let directory = Directory::open_existing(&journal_path).unwrap();
    let scope = JournalScopeV1::new(
        Uuid::from_u128(0x11111111111141118111111111111111),
        "tenant-broker-submission-test",
        "http://127.0.0.1:8787",
        operation_id,
        "broker-submission-test",
    )
    .unwrap();
    let journal = Journal::open(
        &directory,
        scope,
        &FsExclusions::new(),
        Default::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap();
    let ticket = match journal.reserve(operation_id, &canonical).unwrap() {
        ReserveResult::New(ticket) => ticket,
        other => panic!("expected a fresh operation, got {other:?}"),
    };
    assert!(matches!(prepared.binding_inputs(), BindingInputs::None));
    let job = journal
        .accept(ticket, intent, prepared.binding_inputs(), &checkpoint)
        .unwrap();
    let initial_head = job.head().clone();
    let mut approved_plan = plan.clone();
    approved_plan["version"] = json!(intent.plan_version());
    approved_plan["operation_id"] = json!(operation_id);
    approved_plan["session"] = json!(intent.session());
    approved_plan["request"] = serde_json::to_value(intent.initial_request()).unwrap();
    let request: DecisionRequest =
        serde_json::from_value(approved_plan["request"].clone()).unwrap();
    let approval_hash = policy::request_hash(&request);
    approved_plan["request"]["human_approved"] = json!(true);
    approved_plan["request"]["approval_hash"] = json!(approval_hash);
    let approved =
        CheckpointPayloadV1::from_plan_bytes(intent, &serde_json::to_vec(&approved_plan).unwrap())
            .unwrap();
    let receipt = journal
        .advance(operation_id, initial_head, &approved)
        .unwrap();
    let job = journal.load(operation_id).unwrap().unwrap();
    assert_eq!(receipt.head(), job.head());
    let state = BrokerRunState::from_job(&job).unwrap();
    assert_eq!(state.operation_id(), operation_id);
    assert_eq!(state.session(), intent.session());
    assert_eq!(state.options(), &expected_options);
    assert_eq!(
        serde_json::to_value(state.request()).unwrap(),
        approved_plan["request"]
    );
    assert!(state.request().human_approved);
    assert_eq!(
        state.request().approval_hash.as_deref(),
        Some(approval_hash.as_str())
    );
    assert!(
        credentials_from_job(&job, prepared.config())
            .unwrap()
            .is_none()
    );
}

fn options_at(paths: &std::path::Path) -> RunOptions {
    let mut plan = legacy_plan();
    plan["options"]["project"] = paths
        .join("absent-project")
        .to_string_lossy()
        .into_owned()
        .into();
    plan["options"]["config_dir"] = paths
        .join("absent-config")
        .to_string_lossy()
        .into_owned()
        .into();
    serde_json::from_value(plan["options"].clone()).unwrap()
}

#[test]
fn nil_operation_and_approval_hash_fail_before_filesystem_access() {
    let root = tempdir().expect("private test root");
    let paths = root.path().join("not-created");
    let mut options = options_at(&paths);
    assert!(
        prepare_broker_submission(options.clone(), Uuid::nil(), &[]).is_err(),
        "nil identity must be rejected before projection or filesystem access"
    );
    assert!(!paths.exists());

    options.approve_hash = Some("approval-hash-canary-not-a-secret".into());
    let error = match prepare_broker_submission(
        options,
        Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap(),
        &[],
    ) {
        Err(error) => error,
        Ok(_) => panic!("submission preparation must not inherit an interactive approval"),
    };
    let diagnostic = format!("{error:#}");
    assert!(!diagnostic.contains("approval-hash-canary-not-a-secret"));
    assert!(
        !paths.exists(),
        "invalid approval must be rejected before IO"
    );

    let mut interactive = options_at(&paths);
    interactive.interactive = true;
    assert!(
        prepare_broker_submission(
            interactive,
            Uuid::parse_str("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee").unwrap(),
            &[],
        )
        .is_err()
    );
    assert!(
        !paths.exists(),
        "interactive batch rejection must precede IO"
    );
}

#[test]
fn excessive_control_literals_fail_before_filesystem_access() {
    let root = tempdir().expect("private test root");
    let paths = root.path().join("not-created");
    let options = options_at(&paths);
    let oversized = vec!["x".repeat(8 * 1024 * 1024)];
    assert!(
        prepare_broker_submission(
            options,
            Uuid::parse_str("cccccccc-cccc-4ccc-8ccc-cccccccccccc").unwrap(),
            &oversized,
        )
        .is_err(),
        "control redaction literals must have a bounded input size"
    );
    assert!(!paths.exists());
}

#[test]
fn manual_binding_credentials_come_from_accepted_bytes_after_source_rotation() {
    let root = tempdir().expect("private test root");
    let project = root.path().join("project");
    let config = root.path().join("config");
    let journal_path = root.path().join("journal");
    for path in [&project, &config, &journal_path] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let journal_path = journal_path.canonicalize().unwrap();
    let source = root.path().join("manual.env");
    let original_secret = "manual-source-key-canary-47";
    fs::write(&source, format!("SYNTHETIC_API_KEY={original_secret}\n")).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();

    let mut plan: Value =
        serde_json::from_str(include_str!("fixtures/broker_intent-plan-v3.json")).unwrap();
    plan["options"]["project"] = project.to_string_lossy().into_owned().into();
    plan["options"]["config_dir"] = config.to_string_lossy().into_owned().into();
    plan["options"]["profile"] = "observe".into();
    plan["options"]["network"] = "bridge".into();
    plan["options"]["provider"] = "generic".into();
    plan["options"]["argv"] = json!(["/usr/bin/id"]);
    plan["options"]["secret_env_file"] = source.to_string_lossy().into_owned().into();
    for field in [
        "client",
        "auth_mode",
        "client_profile",
        "providers_file",
        "prompt",
        "model",
    ] {
        plan["options"][field] = Value::Null;
    }
    let options: RunOptions = serde_json::from_value(plan["options"].clone()).unwrap();
    let operation_id = Uuid::parse_str("dddddddd-dddd-4ddd-8ddd-dddddddddddd").unwrap();
    let prepared = prepare_broker_submission(options, operation_id, &[]).unwrap();
    assert!(matches!(
        prepared.binding_inputs(),
        BindingInputs::Manual { .. }
    ));
    let intent = prepared.intent();
    let checkpoint = prepared.initial_checkpoint().unwrap();

    let directory = Directory::open_existing(&journal_path).unwrap();
    let scope = JournalScopeV1::new(
        Uuid::from_u128(0x33333333333343338333333333333333),
        "tenant-manual-binding-test",
        "http://127.0.0.1:8787",
        Uuid::from_u128(0x44444444444444448444444444444444),
        "broker-manual-binding-test",
    )
    .unwrap();
    let journal = Journal::open(
        &directory,
        scope,
        &FsExclusions::new(),
        Default::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap();
    let submit = intent.canonical_bytes().unwrap();
    let ticket = match journal.reserve(operation_id, &submit).unwrap() {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("expected a new operation reservation"),
    };
    let job = journal
        .accept(ticket, intent, prepared.binding_inputs(), &checkpoint)
        .unwrap();
    assert_eq!(job.binding_manifest().kind(), BindingKindV1::Manual);

    let rotated = root.path().join("manual.env.rotated");
    fs::rename(&source, &rotated).unwrap();
    fs::write(&source, "SYNTHETIC_API_KEY=replacement-canary-48\n").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    let credentials = credentials_from_job(&job, prepared.config())
        .unwrap()
        .expect("accepted manual credential binding is restored from the job");
    let credential_path = credentials.path().to_path_buf();
    let config_path = prepared.config().path().canonicalize().unwrap();
    let expected_handoff = format!("SYNTHETIC_API_KEY={original_secret}\n").into_bytes();
    let handoff_sha = hex::encode(Sha256::digest(&expected_handoff));
    assert_eq!(fs::read(&credential_path).unwrap(), expected_handoff);
    assert_eq!(
        job.binding_manifest().handoff_sha(),
        Some(handoff_sha.as_str())
    );
    credentials.validate().unwrap();
    assert_eq!(credentials.names(), ["SYNTHETIC_API_KEY"]);
    assert!(
        credentials
            .secrets()
            .iter()
            .any(|value| value == original_secret)
    );
    assert!(
        !credentials
            .secrets()
            .iter()
            .any(|value| value.contains("replacement-canary-48"))
    );
    let debug = format!("{credentials:?}");
    assert!(!debug.contains(original_secret));
    assert!(!debug.contains("replacement-canary-48"));
    drop(job);
    drop(prepared);
    credentials.validate().unwrap();
    assert_eq!(fs::read(&credential_path).unwrap(), expected_handoff);
    fs::write(&credential_path, b"SYNTHETIC_API_KEY=tampered-canary\n").unwrap();
    assert!(credentials.validate().is_err());
    assert!(config_path.join("credential-snapshots").exists());
}

#[test]
fn provider_binding_is_frozen_and_reconstructed_without_source_reread() {
    let root = tempdir().expect("private test root");
    let project = root.path().join("project");
    let config = root.path().join("config");
    let journal_path = root.path().join("journal");
    for path in [&project, &config, &journal_path] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let journal_path = journal_path.canonicalize().unwrap();
    let source = root.path().join("provider.env");
    let original_secret = "provider-source-key-canary-49";
    fs::write(&source, format!("OPENAI_API_KEY={original_secret}\n")).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    let providers_path = config.join("providers.json");
    let provider_config = ProviderConfigV1 {
        version: 1,
        default_profile: None,
        auto_priority: Vec::new(),
        profiles: vec![ProviderProfileV1 {
            id: "codex-api".into(),
            revision: 1,
            client: ClientKind::Codex,
            auth_mode: AuthMode::Api,
            model: None,
            credential: CredentialSourceV1::ApiEnvFile {
                path: source.clone(),
                key: None,
            },
        }],
    };
    fs::write(
        &providers_path,
        serde_json::to_vec(&provider_config).unwrap(),
    )
    .unwrap();
    fs::set_permissions(&providers_path, fs::Permissions::from_mode(0o600)).unwrap();

    let mut plan: Value =
        serde_json::from_str(include_str!("fixtures/broker_intent-plan-v3.json")).unwrap();
    plan["options"]["project"] = project.to_string_lossy().into_owned().into();
    plan["options"]["config_dir"] = config.to_string_lossy().into_owned().into();
    plan["options"]["profile"] = "observe".into();
    plan["options"]["network"] = "bridge".into();
    plan["options"]["provider"] = "generic".into();
    plan["options"]["argv"] = json!([]);
    plan["options"]["client"] = "codex".into();
    plan["options"]["auth_mode"] = "api".into();
    plan["options"]["client_profile"] = "codex-api".into();
    plan["options"]["providers_file"] = providers_path.to_string_lossy().into_owned().into();
    plan["options"]["prompt"] = "synthetic provider prompt".into();
    plan["options"]["model"] = Value::Null;
    plan["options"]["secret_env_file"] = Value::Null;
    let options: RunOptions = serde_json::from_value(plan["options"].clone()).unwrap();
    let operation_id = Uuid::parse_str("ffffffff-ffff-4fff-8fff-ffffffffffff").unwrap();
    let prepared = prepare_broker_submission(options, operation_id, &[]).unwrap();
    assert!(matches!(
        prepared.binding_inputs(),
        BindingInputs::Provider { .. }
    ));
    let intent = prepared.intent();
    let checkpoint = prepared.initial_checkpoint().unwrap();

    let directory = Directory::open_existing(&journal_path).unwrap();
    let scope = JournalScopeV1::new(
        Uuid::from_u128(0x55555555555545558555555555555555),
        "tenant-provider-binding-test",
        "http://127.0.0.1:8787",
        Uuid::from_u128(0x66666666666646668666666666666666),
        "broker-provider-binding-test",
    )
    .unwrap();
    let journal = Journal::open(
        &directory,
        scope,
        &FsExclusions::new(),
        Default::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap();
    let submit = intent.canonical_bytes().unwrap();
    let ticket = match journal.reserve(operation_id, &submit).unwrap() {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("expected a new operation reservation"),
    };
    let job = journal
        .accept(ticket, intent, prepared.binding_inputs(), &checkpoint)
        .unwrap();
    assert_eq!(job.binding_manifest().kind(), BindingKindV1::Provider);
    let frozen_file_name = job
        .intent()
        .provider_credential()
        .expect("frozen provider credential reference")
        .file_name()
        .to_owned();

    let rotated = root.path().join("provider.env.rotated");
    fs::rename(&source, &rotated).unwrap();
    fs::write(&source, "OPENAI_API_KEY=replacement-provider-canary-50\n").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    let credentials = credentials_from_job(&job, prepared.config())
        .unwrap()
        .expect("accepted provider record is sufficient for handoff");
    let credential_path = credentials.path().to_path_buf();
    let config_path = prepared.config().path().canonicalize().unwrap();
    let expected_handoff = format!(
        "AI_GUARD_PROVIDER_BOOTSTRAP={{\"version\":1,\"client\":\"codex\",\"auth_mode\":\"api\",\"secret\":\"{original_secret}\"}}\n"
    )
    .into_bytes();
    let handoff_sha = hex::encode(Sha256::digest(&expected_handoff));
    assert_eq!(fs::read(&credential_path).unwrap(), expected_handoff);
    assert_eq!(
        job.binding_manifest().handoff_sha(),
        Some(handoff_sha.as_str())
    );
    credentials.validate().unwrap();
    assert_eq!(credentials.names(), ["AI_GUARD_PROVIDER_BOOTSTRAP"]);
    assert!(
        credentials
            .secrets()
            .iter()
            .any(|value| value == original_secret)
    );
    assert!(
        !credentials
            .secrets()
            .iter()
            .any(|value| value.contains("replacement-provider-canary-50"))
    );
    let debug = format!("{credentials:?}");
    assert!(!debug.contains(original_secret));
    assert!(!debug.contains("replacement-provider-canary-50"));

    fs::remove_file(&providers_path).unwrap();
    let legacy_binding = config
        .join("credential-bindings")
        .join(format!("{frozen_file_name}.json"));
    if legacy_binding.exists() {
        fs::remove_file(legacy_binding).unwrap();
    }
    let legacy_directory = config.join("credential-bindings");
    if legacy_directory.exists() {
        for entry in fs::read_dir(&legacy_directory).unwrap() {
            let entry = entry.unwrap();
            assert!(entry.file_type().unwrap().is_file());
            fs::remove_file(entry.path()).unwrap();
        }
        fs::remove_dir(legacy_directory).unwrap();
    }
    assert!(!config.join("credential-bindings").exists());
    drop(job);
    drop(prepared);
    let replay = journal.load(operation_id).unwrap().unwrap();
    let replay_state = BrokerRunState::from_job(&replay).unwrap();
    assert_eq!(replay_state.operation_id(), operation_id);
    let replay_credentials =
        credentials_from_job(&replay, &Directory::open_existing(&config_path).unwrap())
            .unwrap()
            .expect("replay uses the durable provider record only");
    replay_credentials.validate().unwrap();
    assert_eq!(
        fs::read(replay_credentials.path()).unwrap(),
        expected_handoff
    );
    assert_eq!(
        replay.binding_manifest().handoff_sha(),
        Some(handoff_sha.as_str())
    );
    drop(replay_state);
    drop(replay);
    replay_credentials.validate().unwrap();
    fs::write(replay_credentials.path(), b"tampered provider handoff\n").unwrap();
    assert!(replay_credentials.validate().is_err());
    assert!(config_path.join("credential-snapshots").exists());
}
