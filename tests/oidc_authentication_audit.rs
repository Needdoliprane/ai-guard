use agentsandbox::{
    audit::{Store, authentication::*},
    identity::PrincipalRefV1,
    redaction::Redactor,
};
use std::{
    fs,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};
use uuid::Uuid;

const INSTALLATION: Uuid = Uuid::from_u128(0x11111111111141118111111111111111);
const OPERATION: Uuid = Uuid::from_u128(0x22222222222242228222222222222222);
const TRANSACTION: Uuid = Uuid::from_u128(0x33333333333343338333333333333333);
const ISSUER: &str = "https://identity.example.test/realm";

fn private_directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
fn policy() -> AuthenticationAuditPolicyV1 {
    AuthenticationAuditPolicyV1 {
        max_records: 100,
        max_serialized_bytes: 100_000,
        approved_clients: vec![
            ApprovedAuthenticationClientV1 {
                issuer: ISSUER.into(),
                client_id: "public-web".into(),
            },
            ApprovedAuthenticationClientV1 {
                issuer: ISSUER.into(),
                client_id: "public-cli".into(),
            },
        ],
    }
}
fn open(path: &Path) -> anyhow::Result<AuthenticationAudit> {
    AuthenticationAudit::open_existing(path, INSTALLATION, policy(), Redactor::new(vec![]))
}
fn fresh(path: &Path) -> AuthenticationAudit {
    AuthenticationAudit::init_new(path, INSTALLATION, policy(), Redactor::new(vec![])).unwrap()
}
fn intention() -> AuthenticationRecordV1 {
    AuthenticationRecordV1 {
        operation_id: OPERATION,
        action: AuthenticationActionV1::Login,
        phase: AuthenticationPhaseV1::Intention,
        issuer: ISSUER.into(),
        client_id: "public-web".into(),
        principal: None,
        transaction_id: Some(TRANSACTION),
        session_id: None,
        causal: None,
    }
}
fn result(causal: AuthenticationReceiptV1) -> AuthenticationRecordV1 {
    AuthenticationRecordV1 {
        phase: AuthenticationPhaseV1::Result {
            outcome: AuthenticationOutcomeV1::Rejected,
            reason: AuthenticationReasonV1::NoMembership,
        },
        principal: Some(PrincipalRefV1::HumanOidc {
            issuer: ISSUER.into(),
            subject: "verified-subject".into(),
        }),
        causal: Some(causal),
        ..intention()
    }
}
fn rejected_as(error: anyhow::Error, expected: AuthenticationAuditFailureV1) {
    assert_eq!(
        error.downcast_ref::<AuthenticationAuditFailureV1>(),
        Some(&expected)
    );
}
fn bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap()
}

// Catches an audit that reports success without a durable separate origin.
#[test]
fn fresh_private_origin_survives_reopen_and_refuses_business_adoption() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let genesis = audit.verify().unwrap();
    assert_eq!(genesis.records, 1);
    assert_eq!(genesis.head.sequence, 1);
    let receipt = audit.append_typed(intention()).unwrap();
    assert_eq!(receipt.sequence, 2);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    drop(audit);
    let before = bytes(&path);
    assert!(Store::open(&path, Redactor::new(vec![]), 100).is_err());
    assert_eq!(bytes(&path), before);
    assert!(!path.with_file_name("authentication.sqlite-wal").exists());
    let reopened = open(&path).unwrap();
    assert_eq!(reopened.verify().unwrap().head, receipt);
    assert_eq!(reopened.append_typed(intention()).unwrap(), receipt);
}

// Catches duplicate writes, lost receipt identity and divergent replay acceptance.
#[test]
fn exact_phase_replay_returns_same_receipt_and_divergence_conflicts() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let first = audit.append_typed(intention()).unwrap();
    assert_eq!(audit.append_typed(intention()).unwrap(), first);
    let mut divergent = intention();
    divergent.transaction_id = Some(Uuid::new_v4());
    rejected_as(
        audit.append_typed(divergent).unwrap_err(),
        AuthenticationAuditFailureV1::Conflict,
    );
    let completed = result(first);
    let receipt = audit.append_typed(completed.clone()).unwrap();
    assert_eq!(audit.append_typed(completed).unwrap(), receipt);
    assert_eq!(audit.verify().unwrap().records, 3);
    drop(audit);
    assert_eq!(open(&path).unwrap().verify().unwrap().head, receipt);
}

// Catches result-before-intention, cross-operation and forged causal receipts.
#[test]
fn result_requires_exact_prior_intention_and_same_operation_context() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let genesis = audit.verify().unwrap().head;
    rejected_as(
        audit.append_typed(result(genesis)).unwrap_err(),
        AuthenticationAuditFailureV1::PhaseOrder,
    );
    let receipt = audit.append_typed(intention()).unwrap();
    let mut forged = result(receipt.clone());
    forged.causal.as_mut().unwrap().hash = "0".repeat(64);
    rejected_as(
        audit.append_typed(forged).unwrap_err(),
        AuthenticationAuditFailureV1::PhaseOrder,
    );
    let mut crossed = result(receipt.clone());
    crossed.client_id = "public-cli".into();
    rejected_as(
        audit.append_typed(crossed).unwrap_err(),
        AuthenticationAuditFailureV1::PhaseOrder,
    );
    audit.append_typed(result(receipt)).unwrap();
    assert_eq!(audit.verify().unwrap().records, 3);
}

// Catches pre-verification attribution and unapproved issuer/client persistence.
#[test]
fn unverified_principal_unapproved_client_and_sensitive_fields_are_refused() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let mut premature = intention();
    premature.principal = result(audit.verify().unwrap().head).principal;
    rejected_as(
        audit.append_typed(premature).unwrap_err(),
        AuthenticationAuditFailureV1::InvalidInput,
    );
    let mut external = intention();
    external.issuer = "https://other.example.test".into();
    assert!(audit.append_typed(external).is_err());
    let receipt = audit.append_typed(intention()).unwrap();
    let mut invalid_response = result(receipt);
    invalid_response.phase = AuthenticationPhaseV1::Result {
        outcome: AuthenticationOutcomeV1::Rejected,
        reason: AuthenticationReasonV1::InvalidResponse,
    };
    assert!(audit.append_typed(invalid_response).is_err());
    assert_eq!(audit.verify().unwrap().records, 2);
}

// Catches known secrets being persisted as a seemingly valid signed subject.
#[test]
fn known_secret_collision_is_refused_without_changing_the_principal() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = AuthenticationAudit::init_new(
        &path,
        INSTALLATION,
        policy(),
        Redactor::new(vec!["verified-subject".into()]),
    )
    .unwrap();
    let receipt = audit.append_typed(intention()).unwrap();
    let sensitive = result(receipt);
    let mut sensitive_policy = policy();
    sensitive_policy.approved_clients[0].client_id = "verified-subject".into();
    assert!(
        !format!(
            "{sensitive:?} {sensitive_policy:?} {:?}",
            sensitive_policy.approved_clients[0]
        )
        .contains("verified-subject")
    );
    rejected_as(
        audit.append_typed(sensitive).unwrap_err(),
        AuthenticationAuditFailureV1::InvalidInput,
    );
    assert_eq!(audit.verify().unwrap().records, 2);
    drop(audit);
    assert!(
        !bytes(&path)
            .windows(b"verified-subject".len())
            .any(|v| v == b"verified-subject")
    );
}

// Catches silently dropping records, granting capacity after reopen and disabling byte bounds.
#[test]
fn record_and_serialized_byte_bounds_fail_closed_without_truncation() {
    let root = private_directory();
    let path = root.path().join("count.sqlite");
    let mut count_policy = policy();
    count_policy.max_records = 2;
    let audit = AuthenticationAudit::init_new(
        &path,
        INSTALLATION,
        count_policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap();
    let receipt = audit.append_typed(intention()).unwrap();
    rejected_as(
        audit.append_typed(result(receipt.clone())).unwrap_err(),
        AuthenticationAuditFailureV1::Capacity,
    );
    assert_eq!(audit.append_typed(intention()).unwrap(), receipt);
    drop(audit);
    assert_eq!(
        AuthenticationAudit::open_existing(
            &path,
            INSTALLATION,
            count_policy,
            Redactor::new(vec![])
        )
        .unwrap()
        .verify()
        .unwrap()
        .records,
        2
    );
    let path = root.path().join("bytes.sqlite");
    let baseline = fresh(&path).verify().unwrap().serialized_bytes;
    let path = root.path().join("bounded.sqlite");
    let mut byte_policy = policy();
    byte_policy.max_serialized_bytes = baseline + 100;
    let audit =
        AuthenticationAudit::init_new(&path, INSTALLATION, byte_policy, Redactor::new(vec![]))
            .unwrap();
    rejected_as(
        audit.append_typed(intention()).unwrap_err(),
        AuthenticationAuditFailureV1::Capacity,
    );
    assert_eq!(audit.verify().unwrap().records, 1);
}

// Catches existing-file adoption, implicit repair, missing-file creation and WAL adoption.
#[test]
fn existing_empty_partial_and_business_files_are_never_adopted_or_repaired() {
    let root = private_directory();
    assert!(open(&root.path().join("missing.sqlite")).is_err());
    assert!(!root.path().join("missing.sqlite").exists());
    for kind in ["empty", "partial", "business"] {
        let path = root.path().join(format!("{kind}.sqlite"));
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        if kind == "partial" {
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute_batch("PRAGMA user_version=1000001; PRAGMA application_id=1095321409; CREATE TABLE authentication_meta(singleton INTEGER);").unwrap();
        } else if kind == "business" {
            drop(Store::open(&path, Redactor::new(vec![]), 100).unwrap());
        }
        let before = bytes(&path);
        assert!(
            AuthenticationAudit::init_new(&path, INSTALLATION, policy(), Redactor::new(vec![]))
                .is_err()
        );
        assert!(open(&path).is_err());
        assert_eq!(bytes(&path), before);
        assert!(!path.with_file_name(format!("{kind}.sqlite-wal")).exists());
    }
}

// Catches trusting schema version/metadata alone instead of replaying the full chain.
#[test]
fn corruption_schema_purpose_namespace_and_installation_changes_are_refused() {
    for mutation in [
        "UPDATE authentication_entries SET hash='tampered' WHERE seq=2",
        "DROP TABLE authentication_quota",
        "CREATE TRIGGER discard AFTER INSERT ON authentication_entries BEGIN DELETE FROM authentication_entries WHERE seq=NEW.seq; END;",
        "UPDATE authentication_meta SET namespace_uuid='44444444-4444-4444-8444-444444444444'",
        "UPDATE authentication_meta SET installation_id='44444444-4444-4444-8444-444444444444'",
        "PRAGMA ignore_check_constraints=ON; UPDATE authentication_meta SET purpose='business'",
        "PRAGMA application_id=0",
    ] {
        let root = private_directory();
        let path = root.path().join("authentication.sqlite");
        let audit = fresh(&path);
        audit.append_typed(intention()).unwrap();
        drop(audit);
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(mutation).unwrap();
        drop(db);
        let before = bytes(&path);
        assert!(open(&path).is_err(), "{mutation}");
        assert_eq!(bytes(&path), before, "{mutation}");
        assert!(!path.with_file_name("authentication.sqlite-wal").exists());
    }
}

// Catches acknowledgement after live durable quota-counter corruption.
#[test]
fn live_corruption_blocks_append_and_never_emits_a_receipt() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let receipt = audit.append_typed(intention()).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE authentication_quota SET bytes=0", [])
        .unwrap();
    assert!(audit.append_typed(result(receipt)).is_err());
    assert!(audit.verify().is_err());
}

// Catches backup overwrite, incorrect private modes and non-restorable snapshots.
#[test]
fn backup_is_new_private_verified_and_restores_same_origin_and_receipt() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let receipt = audit.append_typed(intention()).unwrap();
    let backup = root.path().join("backup.sqlite");
    let verified = audit.backup_new(&backup).unwrap();
    assert_eq!(verified.head, receipt);
    assert_eq!(
        fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let before = bytes(&backup);
    assert!(audit.backup_new(&backup).is_err());
    assert_eq!(bytes(&backup), before);
    let restored = open(&backup).unwrap();
    assert_eq!(restored.verify().unwrap(), verified);
    restored.append_typed(result(receipt)).unwrap();
    assert_eq!(audit.verify().unwrap().records, 2);
    let business = Store::open(
        &root.path().join("tenant.sqlite"),
        Redactor::new(vec![]),
        100,
    )
    .unwrap();
    audit.ensure_distinct_from_business(&business).unwrap();
}

// Catches loss of a committed prefix observed before a capacity refusal. This
// coherent same-inode SQLite restore is not a power-loss or attacker proof.
#[test]
fn capacity_refusal_retains_the_committed_prefix_observed_from_another_handle() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let backup_path = root.path().join("genesis-backup.sqlite");
    let mut capacity_policy = policy();
    capacity_policy.max_records = 2;
    let audit_a = AuthenticationAudit::init_new(
        &path,
        INSTALLATION,
        capacity_policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap();
    let genesis = audit_a.backup_new(&backup_path).unwrap();
    assert_eq!(genesis.records, 1);

    let audit_b = AuthenticationAudit::open_existing(
        &path,
        INSTALLATION,
        capacity_policy,
        Redactor::new(vec![]),
    )
    .unwrap();
    let observed = audit_b.append_typed(intention()).unwrap();
    assert_eq!(observed.sequence, 2);
    drop(audit_b);
    let new_operation = AuthenticationRecordV1 {
        operation_id: Uuid::from_u128(0x44444444444444448444444444444444),
        ..intention()
    };
    rejected_as(
        audit_a.append_typed(new_operation).unwrap_err(),
        AuthenticationAuditFailureV1::Capacity,
    );

    let identity_before = fs::metadata(&path).unwrap();
    let source = rusqlite::Connection::open_with_flags(
        &backup_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let mut destination = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    {
        // SQLite exclusively owns these descriptors and their lock lifecycle.
        // Restore pages in place; never unlink, replace or raw-open the inode.
        let restore = rusqlite::backup::Backup::new(&source, &mut destination).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while restore.step(64).unwrap() != rusqlite::backup::StepResult::Done {
            assert!(
                std::time::Instant::now() < deadline,
                "SQLite restore timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    let restored_records: i64 = destination
        .query_row(
            "SELECT records FROM authentication_quota WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(restored_records, 1);
    drop(destination);
    drop(source);
    let identity_after = fs::metadata(&path).unwrap();
    assert_eq!(
        (identity_after.dev(), identity_after.ino()),
        (identity_before.dev(), identity_before.ino())
    );
    assert!(
        audit_a.verify().is_err(),
        "a capacity refusal must retain the committed prefix it already validated"
    );
}
