//! Offline operator migration of a real historical V1 authentication ledger.
mod support;

use agentsandbox::{audit::authentication::*, identity::PrincipalRefV1, redaction::Redactor};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
use uuid::Uuid;

const INSTALLATION: Uuid = Uuid::from_u128(0x11111111111141118111111111111111);
const NAMESPACE: Uuid = Uuid::from_u128(0x22222222222242228222222222222222);
const V1: i64 = 1_000_001;
const PRIVATE_MARKER: &str = "synthetic-private-marker-never-print";

fn configuration() -> Value {
    // The published example uses a capacity plan. These tests exercise the
    // historical configuration needed for migration targets 2 and 3.
    let mut config: Value = serde_json::from_str(include_str!(
        "../docs/setup/sso/examples/gateway.https.json"
    ))
    .unwrap();
    config["version"] = json!(1);
    config["authentication_audit"]
        .as_object_mut()
        .unwrap()
        .remove("capacity_plan");
    config
}

fn private_write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn policy() -> AuthenticationAuditPolicyV1 {
    serde_json::from_value(configuration()["authentication_audit"]["policy"].clone()).unwrap()
}

fn open(path: &Path) -> AuthenticationAudit {
    AuthenticationAudit::open_existing(path, INSTALLATION, policy(), Redactor::new(vec![])).unwrap()
}

fn legacy_hash(sequence: u64, received: &str, previous: &str, payload: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"ai-guard:authentication-audit:v1\0");
    hash.update(sequence.to_be_bytes());
    for value in [received, previous, payload] {
        hash.update(value.as_bytes());
        hash.update([0]);
    }
    hex::encode(hash.finalize())
}

// Historical V1 schema, deliberately independent from the new V2 writer.
// The denied-login history is synthetic: no account, credential, actual provider
// response or live session is created by the fixture.
fn legacy_v1(path: &Path) -> AuthenticationVerificationV1 {
    let payload = json!({
        "kind":"genesis", "format":1, "purpose":"authentication_audit_v1",
        "installation_id":INSTALLATION, "namespace_id":NAMESPACE, "policy":policy()
    })
    .to_string();
    let received = "2026-11-07T00:00:00.000000000Z";
    let previous = "0".repeat(64);
    let hash = legacy_hash(1, received, &previous, &payload);
    let operation = Uuid::from_u128(0x33333333333343338333333333333333);
    let intention = AuthenticationRecordV1 {
        operation_id: operation,
        action: AuthenticationActionV1::Login,
        phase: AuthenticationPhaseV1::Intention,
        issuer: policy().approved_clients[0].issuer.clone(),
        client_id: "ai-guard-web-public".into(),
        principal: None,
        transaction_id: Some(Uuid::from_u128(0x44444444444444448444444444444444)),
        session_id: None,
        causal: None,
    };
    let intention_payload = json!({"kind":"authentication", "record":intention}).to_string();
    let intention_hash = legacy_hash(2, received, &hash, &intention_payload);
    let denied = AuthenticationRecordV1 {
        phase: AuthenticationPhaseV1::Result {
            outcome: AuthenticationOutcomeV1::Rejected,
            reason: AuthenticationReasonV1::NoMembership,
        },
        principal: Some(PrincipalRefV1::HumanOidc {
            issuer: intention.issuer.clone(),
            subject: "historical-synthetic-subject".into(),
        }),
        causal: Some(AuthenticationReceiptV1 {
            namespace_id: NAMESPACE,
            sequence: 2,
            hash: intention_hash.clone(),
        }),
        ..intention
    };
    let denied_payload = json!({"kind":"authentication", "record":denied}).to_string();
    let denied_hash = legacy_hash(3, received, &intention_hash, &denied_payload);
    let serialized_bytes = (payload.len() + intention_payload.len() + denied_payload.len()) as u64;
    let db = Connection::open(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    db.execute_batch(concat!(
        "CREATE TABLE authentication_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),purpose TEXT NOT NULL CHECK(purpose='authentication_audit_v1'),installation_id TEXT NOT NULL,namespace_uuid TEXT NOT NULL,genesis_hash TEXT NOT NULL);",
        "CREATE TABLE authentication_entries(seq INTEGER PRIMARY KEY,received_at TEXT NOT NULL,prev_hash TEXT NOT NULL,hash TEXT NOT NULL UNIQUE,payload TEXT NOT NULL,operation_id TEXT,phase TEXT CHECK(phase IN('intention','result')),CHECK((operation_id IS NULL AND phase IS NULL) OR (operation_id IS NOT NULL AND phase IS NOT NULL)),UNIQUE(operation_id,phase));",
        "CREATE TABLE authentication_quota(singleton INTEGER PRIMARY KEY CHECK(singleton=1),records INTEGER NOT NULL CHECK(records>=1),bytes INTEGER NOT NULL CHECK(bytes>=0));"
    ))
    .unwrap();
    db.execute(
        "INSERT INTO authentication_meta VALUES(1,'authentication_audit_v1',?1,?2,?3)",
        params![INSTALLATION.to_string(), NAMESPACE.to_string(), hash],
    )
    .unwrap();
    db.execute("INSERT INTO authentication_entries(seq,received_at,prev_hash,hash,payload) VALUES(1,?1,?2,?3,?4)", params![received,previous,hash,payload]).unwrap();
    db.execute(
        "INSERT INTO authentication_entries VALUES(2,?1,?2,?3,?4,?5,'intention')",
        params![
            received,
            hash,
            intention_hash,
            intention_payload,
            operation.to_string()
        ],
    )
    .unwrap();
    db.execute(
        "INSERT INTO authentication_entries VALUES(3,?1,?2,?3,?4,?5,'result')",
        params![
            received,
            intention_hash,
            denied_hash,
            denied_payload,
            operation.to_string()
        ],
    )
    .unwrap();
    db.execute(
        "INSERT INTO authentication_quota VALUES(1,3,?1)",
        [serialized_bytes as i64],
    )
    .unwrap();
    db.pragma_update(None, "application_id", 0x4149_4741_i64)
        .unwrap();
    db.pragma_update(None, "user_version", V1).unwrap();
    drop(db);
    let expected = AuthenticationVerificationV1 {
        namespace_id: NAMESPACE,
        records: 3,
        serialized_bytes,
        head: AuthenticationReceiptV1 {
            namespace_id: NAMESPACE,
            sequence: 3,
            hash: denied_hash,
        },
    };
    assert_eq!(open(path).verify().unwrap(), expected);
    assert_eq!(schema(path), V1, "opening must not migrate legacy data");
    expected
}

fn schema(path: &Path) -> i64 {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

fn config_file(directory: &Path, config: &Value) -> PathBuf {
    let path = directory.join("oidc.json");
    private_write(&path, &serde_json::to_vec(config).unwrap());
    path
}

fn migrate(config: &Path, backup: &Path, confirmation: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .args(["migrate-authentication", "--oidc-config"])
        .arg(config)
        .arg("--backup")
        .arg(backup)
        .stdin(Stdio::null());
    if confirmation {
        command.arg("--confirm-control-stopped");
    }
    command.output().unwrap()
}

#[test]
fn migration_requires_operator_confirmation_before_reading_configuration() {
    let files = support::private_tempdir();
    let config = files.path().join("missing-oidc.json");
    let backup = files.path().join("new.backup");
    let output = migrate(&config, &backup, false);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--confirm-control-stopped"));
    assert!(!backup.exists());
    assert!(!config.exists());
}

#[test]
fn migration_config_is_closed_private_and_local_before_backup_or_database_open() {
    let files = support::private_tempdir();
    let backup = files.path().join("new.backup");
    let mut unknown = configuration();
    unknown[PRIVATE_MARKER] = json!(PRIVATE_MARKER);
    let mut wrong_type = configuration();
    wrong_type["authentication_audit"]["path"] = json!({PRIVATE_MARKER:PRIVATE_MARKER});
    let mut missing_ca = configuration();
    missing_ca["ca_bundle_file"] = json!("missing-ca.pem");
    let mut traversal = configuration();
    traversal["authentication_audit"]["path"] = json!("../outside.sqlite");
    for config in [unknown, wrong_type, missing_ca, traversal] {
        let path = config_file(files.path(), &config);
        let output = migrate(&path, &backup, true);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains(PRIVATE_MARKER));
        assert!(!backup.exists());
        assert!(!files.path().join("authentication.sqlite").exists());
    }
    let path = config_file(files.path(), &configuration());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!migrate(&path, &backup, true).status.success());
    assert!(!backup.exists());
}

#[test]
fn explicit_migration_preserves_verified_v1_history_and_exact_new_backup() {
    let files = support::private_tempdir();
    let data = files.path().join("authentication.sqlite");
    let expected = legacy_v1(&data);
    let path = config_file(files.path(), &configuration());
    let backup = files.path().join("before-v2.backup");
    let output = migrate(&path, &backup, true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["migration"], "verified_initial_v2");
    assert_eq!(report["namespace_id"], NAMESPACE.to_string());
    assert_eq!(report["records"], expected.records);
    assert_eq!(report["serialized_bytes"], expected.serialized_bytes);
    assert_eq!(
        report["head"],
        serde_json::to_value(&expected.head).unwrap()
    );
    assert_eq!(open(&data).verify().unwrap(), expected);
    assert_eq!(schema(&data), 1_000_002);
    assert_eq!(schema(&backup), V1);
    assert_eq!(open(&backup).verify().unwrap(), expected);
    assert_eq!(
        fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let second_backup = files.path().join("must-not-exist.backup");
    assert!(!migrate(&path, &second_backup, true).status.success());
    assert!(
        !second_backup.exists(),
        "already V2 refuses before creating a backup"
    );
    assert_eq!(open(&data).verify().unwrap(), expected);
}

#[test]
fn migration_parser_requires_configuration_and_backup_without_provider_options() {
    for arguments in [
        vec!["--backup", "new.backup", "--confirm-control-stopped"],
        vec!["--oidc-config", "oidc.json", "--confirm-control-stopped"],
        vec![
            "--oidc-config",
            "oidc.json",
            "--backup",
            "new.backup",
            "--confirm-control-stopped",
            "--provider-token",
            PRIVATE_MARKER,
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
            .arg("migrate-authentication")
            .args(arguments)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn existing_backup_is_preserved_and_legacy_source_remains_v1() {
    let files = support::private_tempdir();
    let data = files.path().join("authentication.sqlite");
    let expected = legacy_v1(&data);
    let path = config_file(files.path(), &configuration());
    let backup = files.path().join("existing.backup");
    private_write(&backup, PRIVATE_MARKER.as_bytes());
    let output = migrate(&path, &backup, true);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains(PRIVATE_MARKER));
    assert_eq!(fs::read(&backup).unwrap(), PRIVATE_MARKER.as_bytes());
    assert_eq!(schema(&data), V1);
    assert_eq!(open(&data).verify().unwrap(), expected);
}

// Missing dispatch or silently choosing V1→V2 must fail this real maintenance
// path; expected versions/history come from the independent historical fixture.
#[test]
fn explicit_native_logout_v3_migration_preserves_v2_history_and_new_backup() {
    let files = support::private_tempdir();
    let data = files.path().join("authentication.sqlite");
    let expected = legacy_v1(&data);
    let path = config_file(files.path(), &configuration());
    let before_v2 = files.path().join("before-v2.backup");
    assert!(migrate(&path, &before_v2, true).status.success());
    assert_eq!(schema(&data), 1_000_002);
    assert_eq!(schema(&before_v2), V1);

    let before_v3 = files.path().join("before-v3.backup");
    let output = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args([
            "migrate-authentication",
            "--target-version",
            "3",
            "--oidc-config",
        ])
        .arg(&path)
        .arg("--backup")
        .arg(&before_v3)
        .arg("--confirm-control-stopped")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "explicit V2→V3 maintenance failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["migration"], "native_logout_v3");
    assert_eq!(report["namespace_id"], NAMESPACE.to_string());
    assert_eq!(report["records"], expected.records);
    assert_eq!(report["serialized_bytes"], expected.serialized_bytes);
    assert_eq!(
        report["head"],
        serde_json::to_value(&expected.head).unwrap()
    );
    assert_eq!(schema(&data), 1_000_003);
    assert_eq!(schema(&before_v3), 1_000_002);
    assert_eq!(open(&data).verify().unwrap(), expected);
    assert_eq!(open(&before_v3).verify().unwrap(), expected);
    assert_eq!(
        fs::metadata(&before_v3).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(schema(&before_v2), V1, "previous backup must remain V1");
}
