use super::load;
use serde_json::{Value, json};
use std::{
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

fn write_private_file(path: &Path, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .expect("create private gateway configuration fixture");
    file.write_all(bytes)
        .expect("write private gateway configuration fixture");
}

fn fixture() -> (tempfile::TempDir, PathBuf, Value) {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("create private gateway config test directory");
    write_private_file(&directory.path().join("runner"), "r".repeat(48).as_bytes());
    write_private_file(&directory.path().join("viewer"), "v".repeat(48).as_bytes());
    let registry_path = directory.path().join("tenants.json");
    let registry = json!({
        "version": 1,
        "tenants": [{
            "tenant_id": "tenant-a",
            "data": "tenant-a.sqlite",
            "runner_token_file": "runner",
            "viewer_token_file": "viewer"
        }]
    });
    write_private_file(&registry_path, &serde_json::to_vec(&registry).unwrap());
    (directory, registry_path, registry)
}

fn policy(approvals: Value) -> Value {
    json!({
        "version": 1,
        "decision_ttl_seconds": 30,
        "approvals": approvals
    })
}

#[test]
fn loads_legacy_and_held_policy_registry_without_mutating_the_file_or_opening_a_store() {
    let (_directory, registry_path, mut registry) = fixture();
    let database = registry_path.with_file_name("tenant-a.sqlite");

    let configured = load(&registry_path).expect("legacy registry without a policy remains valid");
    assert_eq!(configured.len(), 1);
    assert!(configured[0].held_create_policy.is_none());
    assert!(!database.exists());

    registry["tenants"][0]["held_create_policy"] = Value::Null;
    std::fs::write(&registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
    let configured = load(&registry_path).expect("explicit null policy means no policy");
    assert_eq!(configured.len(), 1);
    assert!(configured[0].held_create_policy.is_none());
    assert!(!database.exists());

    let raw_policy = policy(json!([
        {
            "authority_fingerprint": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "image": "worker-z@sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            "profile": "kubernetes_held_pod_v1"
        },
        {
            "authority_fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "image": "worker-a@sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            "profile": "kubernetes_held_pod_v1"
        }
    ]));
    registry["tenants"][0]["held_create_policy"] = raw_policy;
    let original_bytes = serde_json::to_vec(&registry).unwrap();
    std::fs::write(&registry_path, &original_bytes).unwrap();

    let configured = load(&registry_path).expect("valid tenant policy loads");
    assert_eq!(configured.len(), 1);
    let loaded_policy = configured[0]
        .held_create_policy
        .as_ref()
        .expect("configured policy is present");
    assert_eq!(
        serde_json::to_value(loaded_policy).unwrap(),
        policy(json!([
            {
                "authority_fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "image": "worker-a@sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
                "profile": "kubernetes_held_pod_v1"
            },
            {
                "authority_fingerprint": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "image": "worker-z@sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                "profile": "kubernetes_held_pod_v1"
            }
        ]))
    );
    assert_eq!(std::fs::read(&registry_path).unwrap(), original_bytes);
    assert!(!database.exists());
}

#[test]
fn rejects_invalid_held_policy_documents_before_any_tenant_database_exists() {
    let (_directory, registry_path, mut registry) = fixture();
    let database = registry_path.with_file_name("tenant-a.sqlite");
    let cases = [
        policy(json!([])),
        json!({
            "version": 2,
            "decision_ttl_seconds": 30,
            "approvals": [{
                "authority_fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "image": "worker@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "profile": "kubernetes_held_pod_v1"
            }]
        }),
        json!({
            "version": 1,
            "decision_ttl_seconds": 30,
            "approvals": [{
                "authority_fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "image": "worker@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "profile": "kubernetes_held_pod_v1",
                "unexpected": true
            }]
        }),
    ];

    for invalid in cases {
        registry["tenants"][0]["held_create_policy"] = invalid;
        std::fs::write(&registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
        assert!(load(&registry_path).is_err());
        assert!(!database.exists());
    }
}
