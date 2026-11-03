//! Software contract checks only. These tests do not qualify a Kubernetes
//! cluster, NetworkPolicy enforcement, workload execution or tenant isolation.
use agentsandbox::runtime::kubernetes::{KubernetesAuthorityV1, KubernetesPodHandle};
use uuid::Uuid;

fn authority() -> KubernetesAuthorityV1 {
    KubernetesAuthorityV1::new(
        Uuid::from_u128(1),
        "https://127.0.0.1:6443",
        "11111111-1111-4111-8111-111111111111",
        "ai-guard-tenant-a",
        "22222222-2222-4222-8222-222222222222",
        &"a".repeat(64),
    )
    .unwrap()
}

#[test]
fn authority_rejects_implicit_or_ambiguous_cluster_selection() {
    for endpoint in [
        "http://127.0.0.1:6443",
        "https://runner:private@127.0.0.1:6443",
        "https://127.0.0.1:6443/?tenant=b",
        "https://127.0.0.1:6443/#context=b",
        "https://127.0.0.1:6443/proxy",
        "file:///private/config",
        "docker-desktop",
    ] {
        assert!(
            KubernetesAuthorityV1::new(
                Uuid::from_u128(1),
                endpoint,
                "11111111-1111-4111-8111-111111111111",
                "ai-guard-tenant-a",
                "22222222-2222-4222-8222-222222222222",
                &"a".repeat(64),
            )
            .is_err(),
            "ambiguous API endpoint accepted"
        );
    }
}

#[test]
fn authority_validation_applies_after_deserialization_and_binds_every_selection() {
    let valid = authority();
    let valid_bytes = serde_json::to_value(&valid).unwrap();
    let fingerprint = valid.fingerprint();
    for (field, value) in [
        ("authority_id", serde_json::json!(Uuid::nil())),
        ("namespace", serde_json::json!("../tenant-b")),
        ("namespace", serde_json::json!("default")),
        ("namespace_uid", serde_json::json!("")),
        ("cluster_namespace_uid", serde_json::json!("")),
        ("trust_sha256", serde_json::json!("not-a-digest")),
        ("version", serde_json::json!(2)),
    ] {
        let mut altered = valid_bytes.clone();
        altered[field] = value;
        let decoded: Result<KubernetesAuthorityV1, _> = serde_json::from_value(altered);
        assert!(
            decoded.is_err() || decoded.unwrap().validate().is_err(),
            "invalid authority survived checkpoint validation: {field}"
        );
    }
    for (field, value) in [
        ("authority_id", serde_json::json!(Uuid::from_u128(2))),
        ("endpoint", serde_json::json!("https://127.0.0.1:7443")),
        ("namespace", serde_json::json!("ai-guard-tenant-b")),
        (
            "namespace_uid",
            serde_json::json!("33333333-3333-4333-8333-333333333333"),
        ),
        (
            "cluster_namespace_uid",
            serde_json::json!("44444444-4444-4444-8444-444444444444"),
        ),
        ("trust_sha256", serde_json::json!("b".repeat(64))),
    ] {
        let mut altered = valid_bytes.clone();
        altered[field] = value;
        let decoded: KubernetesAuthorityV1 = serde_json::from_value(altered).unwrap();
        decoded.validate().unwrap();
        assert_ne!(fingerprint, decoded.fingerprint(), "unbound field: {field}");
    }
    let mut unknown = valid_bytes;
    unknown["context"] = serde_json::json!("ambient");
    assert!(serde_json::from_value::<KubernetesAuthorityV1>(unknown).is_err());
}

#[test]
fn an_authority_binding_does_not_admit_a_workload() {
    let binding = authority();
    let error = binding.require_workload_admission().unwrap_err();
    assert_eq!(
        error.to_string(),
        "Kubernetes workload admission is unavailable: confinement, workspace transfer and durable supervision remain unqualified"
    );
    let encoded = serde_json::to_string(&binding).unwrap();
    assert!(!encoded.contains("credential"));
    assert!(!encoded.contains("token"));
}

#[test]
fn pod_configuration_commitment_covers_nested_fields_and_ordered_arguments() {
    let first: serde_json::Value = serde_json::from_str(r#"{"containers":[{"name":"worker","args":["a","b"],"securityContext":{"readOnlyRootFilesystem":true,"runAsUser":1000}}]}"#).unwrap();
    let reordered: serde_json::Value = serde_json::from_str(r#"{"containers":[{"securityContext":{"runAsUser":1000,"readOnlyRootFilesystem":true},"args":["a","b"],"name":"worker"}]}"#).unwrap();
    let digest = KubernetesPodHandle::configuration_digest(&first).unwrap();
    assert_eq!(
        digest,
        KubernetesPodHandle::configuration_digest(&reordered).unwrap()
    );
    let mut changed = first;
    changed["containers"][0]["args"] = serde_json::json!(["b", "a"]);
    assert_ne!(
        digest,
        KubernetesPodHandle::configuration_digest(&changed).unwrap()
    );
    assert!(KubernetesPodHandle::configuration_digest(&serde_json::json!(["not-a-spec"])).is_err());
    assert!(
        KubernetesPodHandle::configuration_digest(
            &serde_json::json!({"oversized":"x".repeat(1024*1024+1)})
        )
        .is_err()
    );
}
