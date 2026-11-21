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

fn session_reservation_policy() -> Value {
    json!({
        "version": 2,
        "decision_ttl_seconds": 30,
        "approvals": [{
            "authority_id": "11111111-1111-4111-8111-111111111111",
            "authority_fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "image": "registry.example.invalid/worker@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "client": "codex",
            "auth_mode": "api",
            "transport": "batch",
            "profile_id": "codex_api",
            "profile_revision": 1
        }]
    })
}

fn session_recovery_policy() -> Value {
    json!({
        "version": 2,
        "decision_ttl_seconds": 30,
        "allowed_broker_ids": [
            "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
            "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        ]
    })
}

const SESSION_POLICY_FIELDS: [&str; 3] = [
    "session_reservation_policy",
    "session_recovery_policy",
    "session_control_recovery_policy",
];

#[test]
fn loads_session_v2_registry_and_explicit_null_without_opening_a_database() {
    let (_directory, registry_path, mut registry) = fixture();
    let database = registry_path.with_file_name("tenant-a.sqlite");
    for configured in [false, true] {
        for field in SESSION_POLICY_FIELDS {
            registry["tenants"][0][field] = if configured {
                if field == "session_reservation_policy" {
                    session_reservation_policy()
                } else {
                    session_recovery_policy()
                }
            } else {
                Value::Null
            };
        }
        let original = serde_json::to_vec(&registry).unwrap();
        std::fs::write(&registry_path, &original).unwrap();
        let tenants = load(&registry_path).expect("closed V2 session policies load");
        super::validate_policies(&tenants, 10_000).unwrap();
        assert_eq!(tenants.len(), 1);
        assert_eq!(std::fs::read(&registry_path).unwrap(), original);
        assert!(!database.exists());
    }
}

#[test]
fn rejects_invalid_session_v2_policies_without_mutation_or_database_creation() {
    let (_directory, registry_path, registry) = fixture();
    let database = registry_path.with_file_name("tenant-a.sqlite");
    for field in SESSION_POLICY_FIELDS {
        let valid = if field == "session_reservation_policy" {
            session_reservation_policy()
        } else {
            session_recovery_policy()
        };
        let mut invalid_policies = vec![json!({})];
        for (key, value) in [
            ("version", json!(1)),
            ("decision_ttl_seconds", json!(0)),
            ("decision_ttl_seconds", json!(61)),
            ("unexpected", json!(true)),
        ] {
            let mut invalid = valid.clone();
            invalid[key] = value;
            invalid_policies.push(invalid);
        }
        let entries = if field == "session_reservation_policy" {
            "approvals"
        } else {
            "allowed_broker_ids"
        };
        let mut duplicate = valid.clone();
        duplicate[entries] = json!([valid[entries][0].clone(), valid[entries][0].clone()]);
        invalid_policies.push(duplicate);
        let mut too_many = valid.clone();
        too_many[entries] = if field == "session_reservation_policy" {
            Value::Array(vec![valid[entries][0].clone(); 65])
        } else {
            Value::Array(
                (1..=65)
                    .map(|index| json!(format!("{index:08x}-1111-4111-8111-111111111111")))
                    .collect(),
            )
        };
        invalid_policies.push(too_many);
        if field == "session_reservation_policy" {
            for (key, value) in [
                ("authority_id", json!("not-a-uuid")),
                (
                    "authority_id",
                    json!("00000000-0000-0000-0000-000000000000"),
                ),
                ("profile_revision", json!(0)),
                ("profile_id", json!("invalid/profile")),
                ("client", json!("unknown")),
                ("unexpected", json!(true)),
            ] {
                let mut invalid = valid.clone();
                invalid["approvals"][0][key] = value;
                invalid_policies.push(invalid);
            }
            let mut oversized = valid.clone();
            oversized["approvals"] = Value::Array(
                (1..=64)
                    .map(|index| {
                        let mut approval = valid["approvals"][0].clone();
                        approval["profile_id"] =
                            json!(format!("profile_{index:02}_{}", "x".repeat(69)));
                        approval
                    })
                    .collect(),
            );
            assert!(serde_json::to_vec(&oversized).unwrap().len() > 16_384);
            invalid_policies.push(oversized);
        } else {
            for invalid_id in [
                "not-a-uuid",
                "00000000-0000-0000-0000-000000000000",
                "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
            ] {
                let mut invalid = valid.clone();
                invalid["allowed_broker_ids"] = json!([invalid_id]);
                invalid_policies.push(invalid);
            }
        }
        for invalid in invalid_policies {
            let mut candidate = registry.clone();
            candidate["tenants"][0][field] = invalid;
            let original = serde_json::to_vec(&candidate).unwrap();
            std::fs::write(&registry_path, &original).unwrap();
            assert!(load(&registry_path).is_err(), "must reject {field}");
            assert_eq!(std::fs::read(&registry_path).unwrap(), original);
            assert!(!database.exists());
        }
    }
}

#[test]
fn rejects_duplicate_session_json_fields_and_tenant_ids_before_database_creation() {
    let (_directory, registry_path, mut registry) = fixture();
    for field in SESSION_POLICY_FIELDS {
        registry["tenants"][0][field] = if field == "session_reservation_policy" {
            session_reservation_policy()
        } else {
            session_recovery_policy()
        };
    }
    let encoded = serde_json::to_string(&registry).unwrap();
    let mut candidates = vec![
        encoded.replacen("\"version\":2", "\"version\":2,\"version\":2", 1),
        encoded.replacen(
            "\"profile_revision\":1",
            "\"profile_revision\":1,\"profile_revision\":1",
            1,
        ),
        encoded.replacen(
            "\"session_recovery_policy\":",
            "\"session_recovery_policy\":null,\"session_recovery_policy\":",
            1,
        ),
    ];
    let tenant = registry["tenants"][0].clone();
    registry["tenants"].as_array_mut().unwrap().push(tenant);
    candidates.push(serde_json::to_string(&registry).unwrap());
    for original in candidates {
        std::fs::write(&registry_path, original.as_bytes()).unwrap();
        assert!(load(&registry_path).is_err());
        assert_eq!(std::fs::read(&registry_path).unwrap(), original.as_bytes());
        assert!(!registry_path.with_file_name("tenant-a.sqlite").exists());
    }
}

#[test]
fn consumes_canonical_session_policies_and_keeps_legacy_defaults_disabled() {
    let (_directory, registry_path, mut registry) = fixture();
    let tenants = load(&registry_path).unwrap();
    assert!(tenants[0].session_reservation_policy.is_none());
    assert!(tenants[0].session_recovery_policy.is_none());
    assert!(tenants[0].session_control_recovery_policy.is_none());

    for field in SESSION_POLICY_FIELDS {
        registry["tenants"][0][field] = Value::Null;
    }
    std::fs::write(&registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
    let tenants = load(&registry_path).unwrap();
    assert!(tenants[0].session_reservation_policy.is_none());
    assert!(tenants[0].session_recovery_policy.is_none());
    assert!(tenants[0].session_control_recovery_policy.is_none());

    let mut reservation = session_reservation_policy();
    let mut first = reservation["approvals"][0].clone();
    first["authority_id"] = json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    reservation["approvals"] = json!([first, reservation["approvals"][0].clone()]);
    registry["tenants"][0]["session_reservation_policy"] = reservation.clone();
    for field in ["session_recovery_policy", "session_control_recovery_policy"] {
        registry["tenants"][0][field] = session_recovery_policy();
    }
    let original = serde_json::to_vec(&registry).unwrap();
    std::fs::write(&registry_path, &original).unwrap();
    let tenants = load(&registry_path).unwrap();
    super::validate_policies(&tenants, 10_000).unwrap();
    let configured = &tenants[0];
    let loaded = configured.session_reservation_policy.as_ref().unwrap();
    reservation["approvals"].as_array_mut().unwrap().reverse();
    assert_eq!(serde_json::to_value(loaded).unwrap(), reservation);
    let sorted = json!([
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
    ]);
    assert_eq!(
        serde_json::to_value(configured.session_recovery_policy.as_ref().unwrap()).unwrap()["allowed_broker_ids"],
        sorted
    );
    assert_eq!(
        serde_json::to_value(configured.session_control_recovery_policy.as_ref().unwrap()).unwrap()
            ["allowed_broker_ids"],
        sorted
    );
    assert_eq!(std::fs::read(&registry_path).unwrap(), original);
    assert!(!configured.storage.sqlite_path().unwrap().exists());
}

#[test]
fn validates_in_memory_session_policy_limits_before_database_creation() {
    let (_directory, registry_path, _) = fixture();
    let mut tenants = load(&registry_path).unwrap();
    tenants[0].session_reservation_policy =
        Some(serde_json::from_value(session_reservation_policy()).unwrap());
    tenants[0].session_recovery_policy =
        Some(serde_json::from_value(session_recovery_policy()).unwrap());
    tenants[0].session_control_recovery_policy =
        Some(serde_json::from_value(session_recovery_policy()).unwrap());
    super::validate_policies(&tenants, 10_000).unwrap();
    tenants[0]
        .session_reservation_policy
        .as_mut()
        .unwrap()
        .decision_ttl_seconds = 0;
    assert!(super::validate_policies(&tenants, 10_000).is_err());
    tenants[0]
        .session_reservation_policy
        .as_mut()
        .unwrap()
        .decision_ttl_seconds = 30;
    tenants[0]
        .session_recovery_policy
        .as_mut()
        .unwrap()
        .allowed_broker_ids
        .push("invalid".into());
    assert!(super::validate_policies(&tenants, 10_000).is_err());
    tenants[0]
        .session_recovery_policy
        .as_mut()
        .unwrap()
        .allowed_broker_ids
        .pop();
    tenants[0]
        .session_control_recovery_policy
        .as_mut()
        .unwrap()
        .decision_ttl_seconds = 61;
    assert!(super::validate_policies(&tenants, 10_000).is_err());
    tenants[0]
        .session_control_recovery_policy
        .as_mut()
        .unwrap()
        .decision_ttl_seconds = 30;
    let policy = tenants[0].session_reservation_policy.as_mut().unwrap();
    let approval = policy.approvals[0].clone();
    policy.approvals = (1..=64)
        .map(|index| {
            let mut value = approval.clone();
            value.profile_id = format!("profile_{index:02}_{}", "x".repeat(69));
            value
        })
        .collect();
    assert!(super::validate_policies(&tenants, 10_000).is_err());
    assert!(!tenants[0].storage.sqlite_path().unwrap().exists());
}

#[test]
fn accepts_empty_session_allowlists_without_inventing_default_approvals() {
    let (_directory, registry_path, mut registry) = fixture();
    let mut reservation = session_reservation_policy();
    reservation["approvals"] = json!([]);
    let mut recovery = session_recovery_policy();
    recovery["allowed_broker_ids"] = json!([]);
    registry["tenants"][0]["session_reservation_policy"] = reservation;
    registry["tenants"][0]["session_recovery_policy"] = recovery.clone();
    registry["tenants"][0]["session_control_recovery_policy"] = recovery;
    let original = serde_json::to_vec(&registry).unwrap();
    std::fs::write(&registry_path, &original).unwrap();
    let tenants = load(&registry_path).unwrap();
    super::validate_policies(&tenants, 10_000).unwrap();
    assert!(
        tenants[0]
            .session_reservation_policy
            .as_ref()
            .unwrap()
            .approvals
            .is_empty()
    );
    assert!(
        tenants[0]
            .session_recovery_policy
            .as_ref()
            .unwrap()
            .allowed_broker_ids
            .is_empty()
    );
    assert!(
        tenants[0]
            .session_control_recovery_policy
            .as_ref()
            .unwrap()
            .allowed_broker_ids
            .is_empty()
    );
    assert_eq!(std::fs::read(&registry_path).unwrap(), original);
    assert!(!tenants[0].storage.sqlite_path().unwrap().exists());
}

#[test]
fn accepts_sixty_four_distinct_brokers_for_each_session_recovery_policy() {
    let (_directory, registry_path, mut registry) = fixture();
    let mut policy = session_recovery_policy();
    policy["allowed_broker_ids"] = Value::Array(
        (1..=64)
            .rev()
            .map(|index| json!(format!("{index:08x}-1111-4111-8111-111111111111")))
            .collect(),
    );
    for field in ["session_recovery_policy", "session_control_recovery_policy"] {
        registry["tenants"][0][field] = policy.clone();
    }
    let original = serde_json::to_vec(&registry).unwrap();
    std::fs::write(&registry_path, &original).unwrap();
    let tenants = load(&registry_path).unwrap();
    super::validate_policies(&tenants, 10_000).unwrap();
    assert_eq!(
        tenants[0]
            .session_recovery_policy
            .as_ref()
            .unwrap()
            .allowed_broker_ids
            .len(),
        64
    );
    assert_eq!(
        tenants[0]
            .session_control_recovery_policy
            .as_ref()
            .unwrap()
            .allowed_broker_ids
            .len(),
        64
    );
    assert_eq!(std::fs::read(&registry_path).unwrap(), original);
    assert!(!tenants[0].storage.sqlite_path().unwrap().exists());
}

#[test]
fn enforces_exact_canonical_session_reservation_policy_byte_boundary() {
    let (_directory, registry_path, mut registry) = fixture();
    let mut policy = session_reservation_policy();
    let template = policy["approvals"][0].clone();
    policy["approvals"] = json!([]);
    for index in 1..=64 {
        let mut approval = template.clone();
        approval["authority_id"] = json!(format!("{index:08x}-1111-4111-8111-111111111111"));
        approval["profile_id"] = json!("p");
        policy["approvals"].as_array_mut().unwrap().push(approval);
        if serde_json::to_vec(&policy).unwrap().len() > 16_384 {
            policy["approvals"].as_array_mut().unwrap().pop();
            break;
        }
    }
    let mut next_approval = 0;
    while serde_json::to_vec(&policy).unwrap().len() < 16_385 {
        let approvals = policy["approvals"].as_array_mut().unwrap();
        let length = approvals.len();
        let profile = approvals[next_approval % length]["profile_id"]
            .as_str()
            .unwrap();
        assert!(profile.len() < 80, "fixture must stay within field bounds");
        let extended = format!("{profile}x");
        approvals[next_approval % length]["profile_id"] = json!(extended);
        next_approval += 1;
        if serde_json::to_vec(&policy).unwrap().len() == 16_384 {
            registry["tenants"][0]["session_reservation_policy"] = policy.clone();
            let original = serde_json::to_vec(&registry).unwrap();
            std::fs::write(&registry_path, &original).unwrap();
            let tenants =
                load(&registry_path).expect("exactly 16 KiB canonical policy is accepted");
            super::validate_policies(&tenants, 10_000).unwrap();
            assert_eq!(std::fs::read(&registry_path).unwrap(), original);
            assert!(!tenants[0].storage.sqlite_path().unwrap().exists());
        }
    }
    assert_eq!(serde_json::to_vec(&policy).unwrap().len(), 16_385);
    registry["tenants"][0]["session_reservation_policy"] = policy;
    let original = serde_json::to_vec(&registry).unwrap();
    std::fs::write(&registry_path, &original).unwrap();
    assert!(load(&registry_path).is_err());
    assert_eq!(std::fs::read(&registry_path).unwrap(), original);
    assert!(!registry_path.with_file_name("tenant-a.sqlite").exists());
}
