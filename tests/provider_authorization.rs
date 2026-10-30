mod support;

use agentsandbox::{
    audit::{DecisionFailure, EventQuery, Store},
    control::{AdmissionV1, NetworkV1, QuotaPolicyV1, SecurityPresetV1},
    model::{CommandSpec, DecisionRequest, Outcome},
    policy,
    provider::{
        self, AuthMode, AuthPreference, ClientKind, ClientTransport, CredentialSourceV1,
        ProviderConfigV1, ProviderProfileV1, ProviderSelection,
    },
    redaction::Redactor,
};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt};

fn legacy_admission() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/control/admission-fixed-sandbox-v1.json"
    ))
    .expect("historical public fixture")
}

fn resolution() -> Value {
    json!({
        "version": 1,
        "client": "codex",
        "auth_mode": "api",
        "transport": "batch",
        "profile_id": "codex-api",
        "profile_revision": 1,
        "credential_binding_id": "55555555-5555-4555-8555-555555555555",
        "credential_names": ["AI_GUARD_PROVIDER_BOOTSTRAP"],
        "invocation_preset_version": 1,
        "provenance": {"client": "cli", "auth_mode": "cli", "profile": "profile", "model": "profile"}
    })
}

#[test]
fn managed_provider_context_roundtrips_and_changes_approval_hash() {
    let legacy = legacy_admission()["decision_request"].clone();
    let old: DecisionRequest = serde_json::from_value(legacy.clone()).unwrap();
    let mut managed = legacy;
    managed["provider_resolution"] = resolution();
    let current: DecisionRequest = serde_json::from_value(managed)
        .expect("a managed request must retain its typed provider resolution");
    assert_eq!(
        serde_json::to_value(&current).unwrap()["provider_resolution"],
        resolution()
    );
    assert_ne!(policy::request_hash(&old), policy::request_hash(&current));
}

#[test]
fn every_provider_identity_choice_is_bound_to_the_request_hash() {
    let mut value = legacy_admission()["decision_request"].clone();
    value["provider_resolution"] = resolution();
    let request: DecisionRequest = serde_json::from_value(value.clone()).unwrap();
    let original_hash = policy::request_hash(&request);
    for (key, changed) in [
        ("client", json!("claude_code")),
        ("auth_mode", json!("subscription")),
        ("transport", json!("interactive_tty")),
        ("profile_revision", json!(2)),
        (
            "credential_binding_id",
            json!("66666666-6666-4666-8666-666666666666"),
        ),
    ] {
        let mut changed_value = value.clone();
        changed_value["provider_resolution"][key] = changed;
        let changed_request: DecisionRequest = serde_json::from_value(changed_value).unwrap();
        assert_ne!(
            policy::request_hash(&changed_request),
            original_hash,
            "unbound provider choice: {key}"
        );
    }
}

#[test]
fn historical_request_and_admission_keep_exact_serialization_and_hashes() {
    let raw = include_str!("fixtures/control/admission-fixed-sandbox-v1.json").trim();
    let old: AdmissionV1 = serde_json::from_str(raw).unwrap();
    assert_eq!(serde_json::to_string(&old).unwrap(), raw);
    assert_eq!(
        policy::request_hash(&old.decision_request),
        "f2b49355b13327290c05b7856040057dff6b15a7a73844c55d2028cab86793aa"
    );
    assert!(
        serde_json::to_value(&old.decision_request)
            .unwrap()
            .get("provider_resolution")
            .is_none()
    );
    assert!(
        serde_json::to_value(&old.launch)
            .unwrap()
            .get("provider_resolution")
            .is_none()
    );
}

fn managed_request(root: &std::path::Path) -> DecisionRequest {
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    let env = root.join("provider.env");
    fs::write(
        &env,
        "OPENAI_API_KEY=public-regression-canary-never-a-real-key\n",
    )
    .unwrap();
    fs::set_permissions(&env, fs::Permissions::from_mode(0o600)).unwrap();
    let config = ProviderConfigV1 {
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
                path: env,
                key: None,
            },
        }],
    };
    let selected = ProviderSelection {
        client: Some(ClientKind::Codex),
        auth_mode: AuthPreference::Api,
        profile_id: None,
        transport: ClientTransport::Batch,
        model: None,
    };
    let prepared = provider::prepare(
        &config,
        &selected,
        &project,
        Some("Read the project summary"),
        uuid::Uuid::new_v4(),
    )
    .unwrap();
    let mut request: DecisionRequest =
        serde_json::from_value(legacy_admission()["decision_request"].clone()).unwrap();
    request.profile = "observe".into();
    request.command = CommandSpec {
        argv: prepared.argv,
        cwd: "/workspace".into(),
    };
    request.provider_resolution = Some(prepared.resolution);
    request
}

#[test]
fn explicit_null_or_unknown_provider_metadata_never_becomes_a_legacy_request() {
    let mut value = legacy_admission()["decision_request"].clone();
    value["provider_resolution"] = Value::Null;
    assert!(serde_json::from_value::<DecisionRequest>(value.clone()).is_err());
    value["provider_resolution"] = resolution();
    value["provider_resolution"]["auth_mode"] = json!("other-account");
    assert!(serde_json::from_value::<DecisionRequest>(value).is_err());
    let mut admission = legacy_admission();
    admission["launch"]["provider_resolution"] = Value::Null;
    assert!(serde_json::from_value::<AdmissionV1>(admission).is_err());
}

#[test]
fn managed_decision_and_worker_reopen_with_exact_provider_identity() {
    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let request = managed_request(directory.path());
    let policy = policy::Policy::default();
    let decision = policy::evaluate(&policy, &request);
    assert_eq!(decision.outcome, Outcome::Allow);
    let store = Store::open(&path, Redactor::new(Vec::new()), 1000).unwrap();
    let stored = store
        .decision(&request, Some((decision.clone(), Value::Null)))
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.event.data["provider_resolution"],
        serde_json::to_value(&request.provider_resolution).unwrap()
    );
    let mut admission: AdmissionV1 = serde_json::from_value(legacy_admission()).unwrap();
    admission.decision_request = request.clone();
    admission.receipt.request_hash = policy::request_hash(&request);
    admission.receipt.audit_seq = stored.seq;
    admission.receipt.audit_hash = stored.hash.clone();
    admission.launch.command = request.command.clone();
    admission.launch.profile = request.profile.clone();
    admission.launch.security_preset = SecurityPresetV1::FixedSandboxV2;
    admission.launch.network = NetworkV1::Bridge;
    admission.launch.provider = "codex".into();
    let resolution = request.provider_resolution.as_ref().unwrap();
    admission.launch.provider_resolution = Some(resolution.clone());
    admission.launch.credential_binding_id = Some(resolution.credential_binding_id.to_string());
    admission.launch.credential_names = resolution.credential_names.clone();
    admission.validate().unwrap();
    for change in [
        "binding",
        "provider",
        "names",
        "transport",
        "context",
        "legacy_preset",
    ] {
        let mut altered = admission.clone();
        match change {
            "binding" => {
                altered.launch.credential_binding_id = Some(uuid::Uuid::new_v4().to_string())
            }
            "provider" => altered.launch.provider = "claude".into(),
            "names" => altered.launch.credential_names = vec!["ANTHROPIC_API_KEY".into()],
            "transport" => {
                altered.launch.security_preset = SecurityPresetV1::FixedSandboxInteractiveV3
            }
            "context" => altered.launch.provider_resolution = None,
            "legacy_preset" => altered.launch.security_preset = SecurityPresetV1::FixedSandboxV1,
            _ => unreachable!(),
        }
        assert!(
            altered.validate().is_err(),
            "unbound launch field: {change}"
        );
    }
    let result = store
        .admit_worker(
            &admission,
            &policy,
            decision,
            Value::Null,
            &QuotaPolicyV1::default(),
        )
        .unwrap();
    assert!(result.accepted);
    let worker_id = result.worker.unwrap().worker_id;
    drop(store);
    let reopened = Store::open(&path, Redactor::new(Vec::new()), 1000).unwrap();
    assert!(reopened.integrity().unwrap().valid);
    let replayed = reopened.decision(&request, None).unwrap().unwrap();
    assert_eq!((replayed.seq, replayed.hash), (stored.seq, stored.hash));
    assert_eq!(
        reopened
            .worker(&worker_id)
            .unwrap()
            .unwrap()
            .launch
            .provider_resolution,
        request.provider_resolution
    );
    let mut changed = request.clone();
    changed
        .provider_resolution
        .as_mut()
        .unwrap()
        .profile_revision += 1;
    assert!(matches!(
        reopened
            .decision(&changed, None)
            .unwrap_err()
            .downcast_ref::<DecisionFailure>(),
        Some(DecisionFailure::Conflict)
    ));
}

#[test]
fn contradictory_provider_invocation_cannot_create_an_allow_even_through_store() {
    let directory = support::private_tempdir();
    let mut request = managed_request(directory.path());
    request.provider_resolution.as_mut().unwrap().auth_mode = AuthMode::Subscription;
    let policy = policy::Policy::default();
    assert_eq!(policy::evaluate(&policy, &request).outcome, Outcome::Deny);
    let store = Store::open(
        &directory.path().join("audit.sqlite"),
        Redactor::new(Vec::new()),
        1000,
    )
    .unwrap();
    let before = store.events(&EventQuery::default()).unwrap().len();
    assert!(store.decision(&request, None).is_err());
    assert_eq!(store.events(&EventQuery::default()).unwrap().len(), before);
    assert!(store.workers().unwrap().is_empty());
}
