//! Pure V2 metadata tests. No Pod, credential read, provider or runtime is used.
use super::*;
use crate::{
    provider::{AuthMode, ChoiceSource, ClientKind, ClientTransport, ProviderChoiceProvenanceV1},
    runtime::kubernetes::{
        KubernetesError, KubernetesPreflightPlanV1, KubernetesPreflightResourcesV1,
    },
    workspace::Fingerprint,
};
use serde_json::{Value, json};
use uuid::Uuid;

const FILE_HASH: &str = "3a6eb0790f39ac87c94f3856b2dd2c5d110e6811602261a9a923d3bb23adc8b7";
const INVOCATION_HASH: &str = "51ca852deb460d152ef9d9ca8bd31eead82b15b6779973c17651f31aac68a9ed";
const WORKSPACE_HASH: &str = "becd5e288a2b47750f6151f94079c0ee4d874cd2efdd28ed9a601ff505a9bd91";
const PROFILE_HASH: &str = "5095f1cfa92d6ddce06856127fd99d5b8f89a2b0fbde24d1af61771c22a80c20";

fn authority() -> KubernetesAuthorityV1 {
    KubernetesAuthorityV1::new(
        Uuid::from_u128(1),
        "https://127.0.0.1:6443",
        "11111111-1111-4111-8111-111111111111",
        "tenant-a",
        "22222222-2222-4222-8222-222222222222",
        &"a".repeat(64),
    )
    .unwrap()
}

fn binding(authority: &KubernetesAuthorityV1) -> KubernetesSessionBindingV2 {
    KubernetesSessionBindingV2::new(
        authority,
        KubernetesSessionIdsV2 {
            installation_id: Uuid::from_u128(10),
            control_namespace_uuid: Uuid::from_u128(3),
            broker_id: Uuid::from_u128(4),
            incarnation_id: Uuid::from_u128(5),
            session_id: Uuid::from_u128(7),
            attempt_id: Uuid::from_u128(8),
        },
    )
    .unwrap()
}

fn provider() -> ProviderResolutionV1 {
    ProviderResolutionV1 {
        version: 1,
        client: ClientKind::Codex,
        auth_mode: AuthMode::Api,
        transport: ClientTransport::Batch,
        profile_id: "default".into(),
        profile_revision: 1,
        model: None,
        credential_binding_id: Uuid::from_u128(9),
        credential_names: vec![crate::provider::BOOTSTRAP_ENV.into()],
        invocation_preset_version: 1,
        provenance: ProviderChoiceProvenanceV1 {
            client: ChoiceSource::Cli,
            auth_mode: ChoiceSource::Cli,
            profile: ChoiceSource::Cli,
            model: ChoiceSource::Cli,
        },
    }
}

fn arguments() -> Vec<String> {
    [
        "exec",
        "--client",
        "codex",
        "--auth-mode",
        "api",
        "--transport",
        "batch",
        "--preset-version",
        "1",
        "--prompt",
        "private fixture prompt",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn resources() -> KubernetesSessionResourcesV2 {
    KubernetesSessionResourcesV2 {
        cpu_millis: 2000,
        memory_mib: 1024,
        temporary_mib: 64,
        home_mib: 64,
        deadline_seconds: 60,
    }
}

fn manifest() -> Manifest {
    Manifest::from([(
        "tool".into(),
        Fingerprint {
            sha256: FILE_HASH.into(),
            bytes: 4,
            mode: Some(0o755),
        },
    )])
}

fn profile() -> KubernetesSessionProfileV2 {
    let authority = authority();
    KubernetesSessionProfileV2::build(
        &authority,
        &binding(&authority),
        &format!("local/worker@sha256:{}", "b".repeat(64)),
        resources(),
        &provider(),
        KubernetesSessionInvocationV2::new(crate::provider::BOOTSTRAP_PROGRAM, &arguments())
            .unwrap(),
        &manifest(),
    )
    .unwrap()
}

fn decode(value: &Value) -> SessionProfileResult<KubernetesSessionProfileV2> {
    let authority = authority();
    KubernetesSessionProfileV2::from_json(
        &serde_json::to_vec(value).unwrap(),
        &authority,
        &binding(&authority),
    )
}

#[test]
fn profile_has_independent_commitments_and_contains_no_private_invocation() {
    // Literal SHA-256 oracles were computed with system shasum from the
    // domain+NUL and structured JSON, independently of the production helpers.
    let profile = profile();
    assert_eq!(profile.profile_hash().unwrap(), PROFILE_HASH);
    let value: Value = serde_json::from_slice(&profile.canonical_bytes().unwrap()).unwrap();
    assert_eq!(
        value,
        json!({
            "version":2, "profile":"kubernetes_session_v2",
            "binding":{
                "installation_id":"00000000-0000-0000-0000-00000000000a",
                "control_namespace_uuid":"00000000-0000-0000-0000-000000000003",
                "broker_id":"00000000-0000-0000-0000-000000000004",
                "incarnation_id":"00000000-0000-0000-0000-000000000005",
                "session_id":"00000000-0000-0000-0000-000000000007",
                "attempt_id":"00000000-0000-0000-0000-000000000008",
                "authority_fingerprint":"6587b1d5ac873356ad23c7c8c9ff9c2dc7f167b292c5abf95dca08b8b2d49a61"
            },
            "image":"local/worker@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "network":"denied",
            "resources":{"cpu_millis":2000,"memory_mib":1024,"temporary_mib":64,"home_mib":64,"deadline_seconds":60},
            "provider":{
                "version":1,"client":"codex","auth_mode":"api","transport":"batch",
                "profile_id":"default","profile_revision":1,
                "credential_binding_id":"00000000-0000-0000-0000-000000000009",
                "credential_names":["AI_GUARD_PROVIDER_BOOTSTRAP"],"invocation_preset_version":1,
                "provenance":{"client":"cli","auth_mode":"cli","profile":"cli","model":"cli"}
            },
            "invocation_sha256": INVOCATION_HASH,
            "workspace":{"manifest_sha256":WORKSPACE_HASH,"file_count":1,"total_bytes":4}
        })
    );
    let text = String::from_utf8(profile.canonical_bytes().unwrap()).unwrap();
    for private in [
        "private fixture prompt",
        "--prompt",
        "tool",
        FILE_HASH,
        "secret",
        "PodSpec",
    ] {
        assert!(
            !text.contains(private),
            "private artifact escaped into the profile"
        );
        assert!(!format!("{profile:?}").contains(private));
    }
    profile.verify_workspace(&manifest()).unwrap();
    profile
        .verify_invocation(
            KubernetesSessionInvocationV2::new(crate::provider::BOOTSTRAP_PROGRAM, &arguments())
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        decode(&value).unwrap().profile_hash().unwrap(),
        profile.profile_hash().unwrap()
    );
}

#[test]
fn every_session_identity_and_authority_must_match_the_retained_binding() {
    let original = serde_json::to_value(profile()).unwrap();
    for field in [
        "installation_id",
        "control_namespace_uuid",
        "broker_id",
        "incarnation_id",
        "session_id",
        "attempt_id",
    ] {
        for replacement in [
            Uuid::nil().to_string(),
            Uuid::from_u128(42).to_string(),
            "00000000000000000000000000000007".into(),
        ] {
            let mut changed = original.clone();
            changed["binding"][field] = json!(replacement);
            assert!(
                decode(&changed).is_err(),
                "accepted changed binding field {field}"
            );
        }
    }
    let mut changed = original.clone();
    changed["binding"]["authority_fingerprint"] = json!("f".repeat(64));
    assert!(decode(&changed).is_err());
    let foreign = KubernetesAuthorityV1::new(
        Uuid::from_u128(42),
        "https://127.0.0.1:6443",
        "11111111-1111-4111-8111-111111111111",
        "tenant-a",
        "22222222-2222-4222-8222-222222222222",
        &"a".repeat(64),
    )
    .unwrap();
    assert!(
        KubernetesSessionProfileV2::from_json(
            &serde_json::to_vec(&original).unwrap(),
            &foreign,
            &binding(&authority())
        )
        .is_err()
    );
}

#[test]
fn unknown_fields_duplicates_versions_and_unbounded_json_are_rejected_opaquely() {
    let original = serde_json::to_value(profile()).unwrap();
    let canary = "private-secret-canary-do-not-echo";
    for pointer in [
        "",
        "/binding",
        "/provider",
        "/provider/provenance",
        "/resources",
        "/workspace",
    ] {
        let mut changed = original.clone();
        changed
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), json!(canary));
        let error = decode(&changed).unwrap_err();
        assert!(!format!("{error:?} {error}").contains(canary));
    }
    let authority = authority();
    let binding = binding(&authority);
    let bytes = profile().canonical_bytes().unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    for repeated in [
        text.replacen("\"version\":2", "\"version\":2,\"version\":2", 1),
        text.replacen("\"file_count\":1", "\"file_count\":1,\"file_count\":1", 1),
        text.replacen(
            "\"profile_revision\":1",
            "\"profile_revision\":1,\"profile_revision\":1",
            1,
        ),
        format!("{text}{{}}"),
    ] {
        assert!(
            KubernetesSessionProfileV2::from_json(repeated.as_bytes(), &authority, &binding)
                .is_err()
        );
    }
    let mut version = original.clone();
    version["version"] = json!(1);
    assert!(decode(&version).is_err());
    let mut padded = bytes.clone();
    padded.resize(64 * 1024, b' ');
    KubernetesSessionProfileV2::from_json(&padded, &authority, &binding).unwrap();
    padded.push(b' ');
    assert_eq!(
        KubernetesSessionProfileV2::from_json(&padded, &authority, &binding).unwrap_err(),
        KubernetesSessionProfileError::InputTooLarge
    );
    let nested = format!("{}0{}", "[".repeat(65), "]".repeat(65));
    assert!(
        KubernetesSessionProfileV2::from_json(nested.as_bytes(), &authority, &binding).is_err()
    );
}

#[test]
fn all_metadata_changes_are_bound_and_only_denied_network_and_immutable_images_parse() {
    let original = serde_json::to_value(profile()).unwrap();
    let original_hash = profile().profile_hash().unwrap();
    for (pointer, replacement) in [
        (
            "/image",
            json!(format!("local/worker@sha256:{}", "c".repeat(64))),
        ),
        ("/resources/cpu_millis", json!(1000)),
        ("/resources/memory_mib", json!(2048)),
        ("/resources/home_mib", json!(32)),
        ("/resources/temporary_mib", json!(32)),
        ("/resources/deadline_seconds", json!(61)),
        ("/provider/profile_revision", json!(2)),
        ("/provider/profile_id", json!("other")),
        ("/provider/client", json!("claude_code")),
        ("/provider/auth_mode", json!("subscription")),
        ("/provider/transport", json!("interactive_tty")),
        ("/provider/provenance/model", json!("profile")),
        (
            "/provider/credential_binding_id",
            json!(Uuid::from_u128(42).to_string()),
        ),
        ("/workspace/manifest_sha256", json!("c".repeat(64))),
        ("/workspace/total_bytes", json!(5)),
        ("/invocation_sha256", json!("c".repeat(64))),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        assert_ne!(
            decode(&changed).unwrap().profile_hash().unwrap(),
            original_hash,
            "unbound field {pointer}"
        );
    }
    for (pointer, replacement) in [
        ("/network", json!("host")),
        ("/network", json!("none")),
        ("/image", json!("local/worker:latest")),
        (
            "/provider/credential_binding_id",
            json!("00000000000000000000000000000009"),
        ),
        (
            "/provider/credential_binding_id",
            json!("00000000-0000-0000-0000-00000000000A"),
        ),
        ("/provider/credential_names", json!(["TOKEN"])),
        ("/provider/auth_mode", json!("auto")),
        ("/workspace/file_count", json!(0)),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        assert!(decode(&changed).is_err(), "invalid field {pointer}");
    }
}

#[test]
fn workspace_commitment_checks_every_entry_without_the_profile_wire_size_cap() {
    let authority = authority();
    let binding = binding(&authority);
    let many: Manifest = (0..20_000)
        .map(|index| {
            (
                format!("file-{index:05}"),
                Fingerprint {
                    sha256: FILE_HASH.into(),
                    bytes: 4,
                    mode: Some(0o644),
                },
            )
        })
        .collect();
    assert!(serde_json::to_vec(&many).unwrap().len() > 64 * 1024);
    let large = KubernetesSessionProfileV2::build(
        &authority,
        &binding,
        &format!("local/worker@sha256:{}", "b".repeat(64)),
        resources(),
        &provider(),
        KubernetesSessionInvocationV2::new(crate::provider::BOOTSTRAP_PROGRAM, &arguments())
            .unwrap(),
        &many,
    )
    .unwrap();
    assert!(large.canonical_bytes().unwrap().len() < 64 * 1024);
    large.verify_workspace(&many).unwrap();
    let mut changed = many;
    changed.get_mut("file-19999").unwrap().sha256 = "c".repeat(64);
    assert_eq!(
        large.verify_workspace(&changed),
        Err(KubernetesSessionProfileError::WorkspaceMismatch)
    );
    changed.insert(
        "extra".into(),
        Fingerprint {
            sha256: FILE_HASH.into(),
            bytes: 0,
            mode: Some(0o644),
        },
    );
    assert_eq!(
        large.verify_workspace(&changed),
        Err(KubernetesSessionProfileError::InvalidProfile)
    );
}

#[test]
fn workspace_paths_modes_digests_and_volume_bounds_are_validated_before_hashing() {
    let expected = profile();
    expected.verify_workspace(&manifest()).unwrap();
    for path in [
        "",
        "/tool",
        "../tool",
        "a/../tool",
        "./tool",
        "a//tool",
        "a\\tool",
        "a:tool",
    ] {
        let invalid = Manifest::from([(path.into(), manifest()["tool"].clone())]);
        assert_eq!(
            expected.verify_workspace(&invalid),
            Err(KubernetesSessionProfileError::InvalidProfile),
            "invalid path must fail validation before commitment comparison"
        );
    }
    for fingerprint in [
        Fingerprint {
            mode: None,
            ..manifest()["tool"].clone()
        },
        Fingerprint {
            mode: Some(0o4755),
            ..manifest()["tool"].clone()
        },
        Fingerprint {
            sha256: "A".repeat(64),
            ..manifest()["tool"].clone()
        },
        Fingerprint {
            bytes: 32 * 1024 * 1024 + 1,
            ..manifest()["tool"].clone()
        },
    ] {
        assert_eq!(
            expected.verify_workspace(&Manifest::from([("tool".into(), fingerprint)])),
            Err(KubernetesSessionProfileError::InvalidProfile)
        );
    }
    let mut conflict = manifest();
    conflict.insert("tool/child".into(), manifest()["tool"].clone());
    assert_eq!(
        expected.verify_workspace(&conflict),
        Err(KubernetesSessionProfileError::InvalidProfile)
    );
    let oversized: Manifest = (0..5)
        .map(|index| {
            (
                format!("file-{index}"),
                Fingerprint {
                    sha256: FILE_HASH.into(),
                    bytes: 32 * 1024 * 1024,
                    mode: Some(0o644),
                },
            )
        })
        .collect();
    let boundary: Manifest = oversized
        .iter()
        .take(4)
        .map(|(name, fingerprint)| (name.clone(), fingerprint.clone()))
        .collect();
    let empty = Manifest::new();
    let authority = authority();
    KubernetesSessionProfileV2::build(
        &authority,
        &binding(&authority),
        &format!("local/worker@sha256:{}", "b".repeat(64)),
        resources(),
        &provider(),
        KubernetesSessionInvocationV2::new(crate::provider::BOOTSTRAP_PROGRAM, &arguments())
            .unwrap(),
        &boundary,
    )
    .unwrap();
    assert_eq!(
        expected.verify_workspace(&oversized),
        Err(KubernetesSessionProfileError::InvalidProfile)
    );
    KubernetesSessionProfileV2::build(
        &authority,
        &binding(&authority),
        &format!("local/worker@sha256:{}", "b".repeat(64)),
        resources(),
        &provider(),
        KubernetesSessionInvocationV2::new(crate::provider::BOOTSTRAP_PROGRAM, &arguments())
            .unwrap(),
        &empty,
    )
    .unwrap();
}

#[test]
fn resource_arithmetic_and_provider_invocation_must_be_valid() {
    let original = serde_json::to_value(profile()).unwrap();
    for (field, value) in [
        ("cpu_millis", 0),
        ("cpu_millis", 200_001),
        ("memory_mib", 102_401),
        ("temporary_mib", 0),
        ("home_mib", 0),
        ("home_mib", 960),
        ("deadline_seconds", 0),
        ("deadline_seconds", 86_401),
        ("temporary_mib", u32::MAX),
        ("home_mib", u32::MAX),
    ] {
        let mut changed = original.clone();
        changed["resources"][field] = json!(value);
        assert!(decode(&changed).is_err(), "invalid resource {field}");
    }
    let expected = profile();
    let mut changed = arguments();
    *changed.last_mut().unwrap() = "other private prompt".into();
    assert_eq!(
        expected.verify_invocation(
            KubernetesSessionInvocationV2::new(crate::provider::BOOTSTRAP_PROGRAM, &changed)
                .unwrap()
        ),
        Err(KubernetesSessionProfileError::InvocationMismatch)
    );
    changed = arguments();
    changed[4] = "subscription".into();
    assert!(
        expected
            .verify_invocation(
                KubernetesSessionInvocationV2::new(crate::provider::BOOTSTRAP_PROGRAM, &changed)
                    .unwrap()
            )
            .is_err()
    );
    assert!(KubernetesSessionInvocationV2::new("/bin/sh", &arguments()).is_err());
    assert!(
        KubernetesSessionInvocationV2::new(
            crate::provider::BOOTSTRAP_PROGRAM,
            &vec!["x".into(); 257]
        )
        .is_err()
    );
}

#[test]
fn existing_prompt_bound_and_interactive_transport_remain_explicit() {
    let authority = authority();
    let image = format!("local/worker@sha256:{}", "b".repeat(64));
    let mut args = arguments();
    *args.last_mut().unwrap() = "p".repeat(24 * 1024);
    KubernetesSessionProfileV2::build(
        &authority,
        &binding(&authority),
        &image,
        resources(),
        &provider(),
        KubernetesSessionInvocationV2::new(crate::provider::BOOTSTRAP_PROGRAM, &args).unwrap(),
        &manifest(),
    )
    .unwrap();
    args.last_mut().unwrap().push('p');
    assert!(KubernetesSessionInvocationV2::new(crate::provider::BOOTSTRAP_PROGRAM, &args).is_err());
    let mut interactive = provider();
    interactive.transport = ClientTransport::InteractiveTty;
    interactive.client = ClientKind::ClaudeCode;
    interactive.auth_mode = AuthMode::Subscription;
    args = arguments();
    args[2] = "claude-code".into();
    args[4] = "subscription".into();
    args[6] = "interactive-tty".into();
    args.truncate(9);
    let profile = KubernetesSessionProfileV2::build(
        &authority,
        &binding(&authority),
        &image,
        resources(),
        &interactive,
        KubernetesSessionInvocationV2::new(crate::provider::BOOTSTRAP_PROGRAM, &args).unwrap(),
        &manifest(),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(profile).unwrap()["provider"]["transport"],
        "interactive_tty"
    );
}

#[test]
fn v1_metadata_cannot_decode_as_v2_and_activation_remains_closed() {
    let authority = authority();
    let v1 = KubernetesPreflightPlanV1::new(
        &authority,
        Uuid::from_u128(7),
        Uuid::from_u128(8),
        &format!("local/worker@sha256:{}", "b".repeat(64)),
        KubernetesPreflightResourcesV1 {
            cpu_millis: 2000,
            memory_mib: 1024,
            workspace_mib: 128,
            temporary_mib: 64,
            deadline_seconds: 60,
        },
    )
    .unwrap();
    assert!(
        KubernetesSessionProfileV2::from_json(
            &serde_json::to_vec(&v1).unwrap(),
            &authority,
            &binding(&authority)
        )
        .is_err()
    );
    assert_eq!(
        authority.fingerprint(),
        "6587b1d5ac873356ad23c7c8c9ff9c2dc7f167b292c5abf95dca08b8b2d49a61"
    );
    profile();
    assert_eq!(
        authority.require_workload_admission(),
        Err(KubernetesError::WorkloadAdmissionUnavailable)
    );
}
