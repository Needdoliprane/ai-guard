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

// Opening a WAL-mode database read-only may create an empty WAL and its SHM
// index. That is SQLite lock bookkeeping, not an adoption or committed write.
fn assert_no_committed_wal_frames(path: &Path) {
    let wal = path.with_file_name(format!(
        "{}-wal",
        path.file_name().unwrap().to_str().unwrap()
    ));
    if wal.exists() {
        assert_eq!(fs::metadata(wal).unwrap().len(), 0);
    }
    assert!(
        !path
            .with_file_name(format!(
                "{}-journal",
                path.file_name().unwrap().to_str().unwrap()
            ))
            .exists()
    );
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
        assert_no_committed_wal_frames(&path);
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
        assert_no_committed_wal_frames(&path);
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
    // macOS tempfile paths can traverse /var -> /private/var. SQLite's
    // NOFOLLOW rejects any symlink in a path; match StoreStorage's canonical
    // path preparation while retaining strict NOFOLLOW for these VFS opens.
    let source = rusqlite::Connection::open_with_flags(
        fs::canonicalize(&backup_path).unwrap(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let mut destination = rusqlite::Connection::open_with_flags(
        fs::canonicalize(&path).unwrap(),
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

fn binding() -> AuthenticationSessionBindingV1 {
    AuthenticationSessionBindingV1 {
        session_id: Uuid::from_u128(0x55555555555545558555555555555555),
        generation: 1,
        principal: PrincipalRefV1::HumanOidc {
            issuer: ISSUER.into(),
            subject: "verified-subject".into(),
        },
        issuer: ISSUER.into(),
        client_id: "public-web".into(),
        kind: OidcSessionKindV1::WebPkce,
        access_fingerprint: AuthenticationTokenFingerprintV1::access_token("first-access-fixture"),
        refresh_fingerprint: Some(AuthenticationTokenFingerprintV1::refresh_token(
            "first-refresh-fixture",
        )),
        expires_at_unix_seconds: 4_000_000_000,
    }
}

fn initial_for(
    binding: &AuthenticationSessionBindingV1,
    operation_id: Uuid,
) -> AuthenticationRecordV1 {
    AuthenticationRecordV1 {
        operation_id,
        session_id: Some(binding.session_id),
        issuer: binding.issuer.clone(),
        client_id: binding.client_id.clone(),
        ..intention()
    }
}

fn successful(
    initial: AuthenticationRecordV1,
    causal: AuthenticationReceiptV1,
    principal: PrincipalRefV1,
    reason: AuthenticationReasonV1,
) -> AuthenticationRecordV1 {
    AuthenticationRecordV1 {
        principal: Some(principal),
        causal: Some(causal),
        phase: AuthenticationPhaseV1::Result {
            outcome: AuthenticationOutcomeV1::Succeeded,
            reason,
        },
        ..initial
    }
}

fn admit(
    audit: &AuthenticationAudit,
    binding: &AuthenticationSessionBindingV1,
    operation: Uuid,
) -> AuthenticationReceiptV1 {
    let initial = initial_for(binding, operation);
    let receipt = audit.append_typed(initial.clone()).unwrap();
    audit
        .admit_session(
            successful(
                initial,
                receipt,
                binding.principal.clone(),
                AuthenticationReasonV1::Verified,
            ),
            binding.clone(),
        )
        .unwrap()
}

fn session_intention(
    binding: &AuthenticationSessionBindingV1,
    action: AuthenticationActionV1,
    operation: Uuid,
) -> AuthenticationRecordV1 {
    AuthenticationRecordV1 {
        action,
        principal: Some(binding.principal.clone()),
        transaction_id: None,
        ..initial_for(binding, operation)
    }
}

// A digest from the wrong token domain cannot satisfy an access-token CAS;
// neither serialization nor diagnostic formatting retains the raw fixture.
#[test]
fn ledger_fingerprints_are_domain_separated_and_do_not_retain_credentials() {
    let raw = "same-test-token-material";
    let access = AuthenticationTokenFingerprintV1::access_token(raw);
    let refresh = AuthenticationTokenFingerprintV1::refresh_token(raw);
    assert_ne!(access, refresh);
    assert_eq!(access, AuthenticationTokenFingerprintV1::access_token(raw));
    assert!(!serde_json::to_string(&access).unwrap().contains(raw));
    assert!(!format!("{access:?} {refresh:?}").contains(raw));
}

// Catches generic result-only admission, causal receipt substitution and
// explicit re-admission of a previously seen JWT after process memory is lost.
#[test]
fn ledger_admission_consumes_exact_intention_and_remembers_seen_access_on_restart() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let binding = binding();
    let initial = initial_for(&binding, OPERATION);
    let intention_receipt = audit.append_typed(initial.clone()).unwrap();
    let admission = successful(
        initial,
        intention_receipt,
        binding.principal.clone(),
        AuthenticationReasonV1::Verified,
    );
    let mut forged = admission.clone();
    forged.causal.as_mut().unwrap().hash = "0".repeat(64);
    assert!(audit.admit_session(forged, binding.clone()).is_err());
    let receipt = audit
        .admit_session(admission.clone(), binding.clone())
        .unwrap();
    assert_eq!(
        audit
            .admit_session(admission.clone(), binding.clone())
            .unwrap(),
        receipt
    );
    assert_eq!(
        audit.session_current(binding.session_id).unwrap(),
        Some(AuthenticationSessionCurrentV1 {
            binding: binding.clone(),
            state: AuthenticationSessionStateV1::Active
        })
    );
    assert!(audit.fingerprint_seen(&binding.access_fingerprint).unwrap());
    let mut crossed = binding.clone();
    crossed.session_id = Uuid::new_v4();
    assert!(audit.admit_session(admission, crossed.clone()).is_err());
    assert!(audit.session_current(crossed.session_id).unwrap().is_none());
    drop(audit);
    let reopened = open(&path).unwrap();
    assert!(
        reopened
            .fingerprint_seen(&binding.access_fingerprint)
            .unwrap()
    );
    let initial = initial_for(&crossed, Uuid::new_v4());
    let receipt = reopened.append_typed(initial.clone()).unwrap();
    let admission = successful(
        initial,
        receipt,
        crossed.principal.clone(),
        AuthenticationReasonV1::Verified,
    );
    assert!(
        reopened.admit_session(admission, crossed).is_err(),
        "a new PKCE operation cannot re-admit the same access JWT"
    );
}

// Catches a replay permitting another provider exchange and concurrent
// refresh operations being admitted for one session generation.
#[test]
fn ledger_refresh_pending_is_durable_and_has_one_exchange_winner() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let binding = binding();
    admit(&audit, &binding, OPERATION);
    let pending = session_intention(&binding, AuthenticationActionV1::Refresh, Uuid::new_v4());
    let inserted = audit
        .refresh_pending(pending.clone(), 1, binding.access_fingerprint)
        .unwrap();
    assert!(inserted.inserted);
    let replayed = audit
        .refresh_pending(pending.clone(), 1, binding.access_fingerprint)
        .unwrap();
    assert!(!replayed.inserted);
    assert_eq!(inserted.receipt, replayed.receipt);
    let competitor = AuthenticationRecordV1 {
        operation_id: Uuid::new_v4(),
        ..pending.clone()
    };
    assert!(
        audit
            .refresh_pending(competitor, 1, binding.access_fingerprint)
            .is_err()
    );
    assert!(
        audit
            .refresh_pending(pending.clone(), 2, binding.access_fingerprint)
            .is_err()
    );
    assert!(
        audit
            .refresh_pending(
                pending.clone(),
                1,
                AuthenticationTokenFingerprintV1::access_token("wrong-token")
            )
            .is_err()
    );
    drop(audit);
    let reopened = open(&path).unwrap();
    assert_eq!(
        reopened
            .session_current(binding.session_id)
            .unwrap()
            .unwrap()
            .state,
        AuthenticationSessionStateV1::RefreshPending {
            operation_id: pending.operation_id,
            intention: inserted.receipt
        }
    );
    assert!(
        !reopened
            .refresh_pending(pending, 1, binding.access_fingerprint)
            .unwrap()
            .inserted
    );
}

// Catches rotation granting a different subject/client/kind, stale generation,
// old access JWT, wrong intention or a nonconsecutive generation.
#[test]
fn ledger_rotation_requires_pending_exact_binding_and_new_unseen_generation() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let old = binding();
    admit(&audit, &old, OPERATION);
    let initial = session_intention(&old, AuthenticationActionV1::Refresh, Uuid::new_v4());
    let pending = audit
        .refresh_pending(initial.clone(), 1, old.access_fingerprint)
        .unwrap();
    let result = successful(
        initial,
        pending.receipt,
        old.principal.clone(),
        AuthenticationReasonV1::Verified,
    );
    let mut next = old.clone();
    next.generation = 2;
    next.access_fingerprint =
        AuthenticationTokenFingerprintV1::access_token("second-access-fixture");
    next.refresh_fingerprint = Some(AuthenticationTokenFingerprintV1::refresh_token(
        "second-refresh-fixture",
    ));
    for mutation in 0..6 {
        let mut invalid = next.clone();
        match mutation {
            0 => invalid.generation = 1,
            1 => invalid.generation = 3,
            2 => invalid.kind = OidcSessionKindV1::NativeCli,
            3 => invalid.client_id = "public-cli".into(),
            4 => {
                invalid.principal = PrincipalRefV1::HumanOidc {
                    issuer: ISSUER.into(),
                    subject: "other-subject".into(),
                }
            }
            5 => invalid.access_fingerprint = old.access_fingerprint,
            _ => unreachable!(),
        }
        assert!(
            audit
                .rotate_session(result.clone(), invalid, 1, old.access_fingerprint)
                .is_err()
        );
    }
    assert!(
        audit
            .rotate_session(result.clone(), next.clone(), 2, old.access_fingerprint)
            .is_err()
    );
    assert!(
        audit
            .rotate_session(
                result.clone(),
                next.clone(),
                1,
                AuthenticationTokenFingerprintV1::refresh_token("first-access-fixture")
            )
            .is_err()
    );
    let receipt = audit
        .rotate_session(result.clone(), next.clone(), 1, old.access_fingerprint)
        .unwrap();
    assert_eq!(
        audit
            .rotate_session(result, next.clone(), 1, old.access_fingerprint)
            .unwrap(),
        receipt
    );
    assert_eq!(
        audit.session_current(old.session_id).unwrap(),
        Some(AuthenticationSessionCurrentV1 {
            binding: next.clone(),
            state: AuthenticationSessionStateV1::Active
        })
    );
    assert!(audit.fingerprint_seen(&old.access_fingerprint).unwrap());
    assert!(audit.fingerprint_seen(&next.access_fingerprint).unwrap());
    drop(audit);
    assert_eq!(
        open(&path)
            .unwrap()
            .session_current(old.session_id)
            .unwrap()
            .unwrap()
            .binding,
        next
    );
}

// Catches logout which is merely process-local, or a late refresh result
// reconnecting a session that was durably closed while provider work ran.
#[test]
fn ledger_close_is_terminal_even_for_a_late_refresh_response() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let old = binding();
    admit(&audit, &old, OPERATION);
    let refreshing = session_intention(&old, AuthenticationActionV1::Refresh, Uuid::new_v4());
    let pending = audit
        .refresh_pending(refreshing.clone(), 1, old.access_fingerprint)
        .unwrap();
    let logout = session_intention(&old, AuthenticationActionV1::Logout, Uuid::new_v4());
    let intention = audit.append_typed(logout.clone()).unwrap();
    let completed = successful(
        logout,
        intention,
        old.principal.clone(),
        AuthenticationReasonV1::UserRequested,
    );
    let receipt = audit
        .close_session(completed.clone(), 1, old.access_fingerprint)
        .unwrap();
    assert_eq!(
        audit
            .close_session(completed, 1, old.access_fingerprint)
            .unwrap(),
        receipt
    );
    let late = successful(
        refreshing.clone(),
        pending.receipt,
        old.principal.clone(),
        AuthenticationReasonV1::Verified,
    );
    let mut next = old.clone();
    next.generation = 2;
    next.access_fingerprint = AuthenticationTokenFingerprintV1::access_token("late-access-fixture");
    assert!(
        audit
            .rotate_session(late, next.clone(), 1, old.access_fingerprint)
            .is_err()
    );
    assert!(!audit.fingerprint_seen(&next.access_fingerprint).unwrap());
    assert!(
        audit
            .refresh_pending(refreshing, 1, old.access_fingerprint)
            .is_err()
    );
    drop(audit);
    assert_eq!(
        open(&path)
            .unwrap()
            .session_current(old.session_id)
            .unwrap()
            .unwrap()
            .state,
        AuthenticationSessionStateV1::Closed {
            reason: AuthenticationReasonV1::UserRequested
        }
    );
}

// Catches recording refresh intention without its CAS state, or mutating
// projected session state before the audit can durably accept the operation.
#[test]
fn ledger_capacity_refuses_pending_atomically_and_queries_revalidate_integrity() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let mut limited = policy();
    limited.max_records = 3;
    let audit =
        AuthenticationAudit::init_new(&path, INSTALLATION, limited, Redactor::new(vec![])).unwrap();
    let old = binding();
    admit(&audit, &old, OPERATION);
    let before = audit.verify().unwrap();
    let pending = session_intention(&old, AuthenticationActionV1::Refresh, Uuid::new_v4());
    rejected_as(
        audit
            .refresh_pending(pending, 1, old.access_fingerprint)
            .unwrap_err(),
        AuthenticationAuditFailureV1::Capacity,
    );
    assert_eq!(audit.verify().unwrap(), before);
    assert_eq!(
        audit
            .session_current(old.session_id)
            .unwrap()
            .unwrap()
            .state,
        AuthenticationSessionStateV1::Active
    );
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE authentication_quota SET bytes=0", [])
        .unwrap();
    assert!(audit.session_current(old.session_id).is_err());
    assert!(audit.fingerprint_seen(&old.access_fingerprint).is_err());
}

// Catches accepting local/workload principals, zero generations, unbound
// issuer, or substituting a refresh-domain digest for an access fingerprint.
#[test]
fn ledger_admission_validates_human_positive_generation_expiry_and_domains() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let valid = binding();
    let initial = initial_for(&valid, OPERATION);
    let receipt = audit.append_typed(initial.clone()).unwrap();
    let result = successful(
        initial,
        receipt,
        valid.principal.clone(),
        AuthenticationReasonV1::Verified,
    );
    for mutation in 0..5 {
        let mut invalid = valid.clone();
        match mutation {
            0 => invalid.generation = 0,
            1 => invalid.expires_at_unix_seconds = 0,
            2 => invalid.issuer = "https://other.example.test".into(),
            3 => {
                invalid.access_fingerprint =
                    AuthenticationTokenFingerprintV1::refresh_token("wrong-domain")
            }
            4 => {
                invalid.refresh_fingerprint = Some(AuthenticationTokenFingerprintV1::access_token(
                    "wrong-domain",
                ))
            }
            _ => unreachable!(),
        }
        assert!(audit.admit_session(result.clone(), invalid).is_err());
    }
    audit.admit_session(result, valid).unwrap();
}

fn assert_generic_terminal_result_refused(
    action: AuthenticationActionV1,
    reason: AuthenticationReasonV1,
) {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let binding = binding();
    admit(&audit, &binding, OPERATION);
    let active = AuthenticationSessionCurrentV1 {
        binding: binding.clone(),
        state: AuthenticationSessionStateV1::Active,
    };
    assert_eq!(
        audit.session_current(binding.session_id).unwrap(),
        Some(active.clone())
    );
    let initial = session_intention(&binding, action, Uuid::new_v4());
    // Terminal intentions remain ordinary observations; their results must
    // consume that same receipt through the atomic session transition lane.
    let intention = audit.append_typed(initial.clone()).unwrap();
    let completed = successful(initial, intention, binding.principal.clone(), reason);
    let before = audit.verify().unwrap();
    assert!(
        audit.append_typed(completed.clone()).is_err(),
        "generic terminal result must not consume the typed close intention"
    );
    assert_eq!(audit.verify().unwrap(), before);
    assert_eq!(
        audit.session_current(binding.session_id).unwrap(),
        Some(active)
    );
    let receipt = audit
        .close_session(completed, 1, binding.access_fingerprint)
        .unwrap();
    assert_eq!(audit.verify().unwrap().head, receipt);
    assert_eq!(
        audit
            .session_current(binding.session_id)
            .unwrap()
            .unwrap()
            .state,
        AuthenticationSessionStateV1::Closed { reason }
    );
}

// Catches a successful audit observation consuming the logout intention while
// leaving the tracked binding Active and making typed close conflict.
#[test]
fn ledger_generic_logout_result_cannot_bypass_terminal_transition() {
    assert_generic_terminal_result_refused(
        AuthenticationActionV1::Logout,
        AuthenticationReasonV1::UserRequested,
    );
}

// Expiration uses the same durable terminal transition even when its
// observation was initiated outside a provider request.
#[test]
fn ledger_generic_expiration_result_cannot_bypass_terminal_transition() {
    assert_generic_terminal_result_refused(
        AuthenticationActionV1::Expiration,
        AuthenticationReasonV1::Expired,
    );
}

// Catches a generic refresh intention bypassing Active -> Pending. Ordinary
// observations of untracked sessions remain accepted by the same public API.
#[test]
fn ledger_generic_refresh_intention_refuses_tracked_sessions_and_keeps_untracked_compatibility() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let binding = binding();
    let mut untracked = binding.clone();
    untracked.session_id = Uuid::new_v4();
    let observation =
        session_intention(&untracked, AuthenticationActionV1::Refresh, Uuid::new_v4());
    let intention = audit.append_typed(observation.clone()).unwrap();
    audit
        .append_typed(successful(
            observation,
            intention,
            untracked.principal.clone(),
            AuthenticationReasonV1::Verified,
        ))
        .unwrap();
    assert!(
        audit
            .session_current(untracked.session_id)
            .unwrap()
            .is_none()
    );

    admit(&audit, &binding, OPERATION);
    let active = audit.session_current(binding.session_id).unwrap().unwrap();
    assert_eq!(active.state, AuthenticationSessionStateV1::Active);
    let before = audit.verify().unwrap();
    let refresh = session_intention(&binding, AuthenticationActionV1::Refresh, Uuid::new_v4());
    assert!(
        audit.append_typed(refresh.clone()).is_err(),
        "tracked refresh intention requires the Pending CAS lane"
    );
    assert_eq!(audit.verify().unwrap(), before);
    assert_eq!(
        audit.session_current(binding.session_id).unwrap(),
        Some(active)
    );
    let pending = audit
        .refresh_pending(refresh.clone(), 1, binding.access_fingerprint)
        .unwrap();
    assert!(pending.inserted);
    assert_eq!(
        audit
            .session_current(binding.session_id)
            .unwrap()
            .unwrap()
            .state,
        AuthenticationSessionStateV1::RefreshPending {
            operation_id: refresh.operation_id,
            intention: pending.receipt
        }
    );
}

// Catches a plain successful refresh result consuming the Pending intention
// without installing the new generation or remembered access fingerprint.
#[test]
fn ledger_generic_refresh_success_result_preserves_pending_for_typed_rotation() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let old = binding();
    admit(&audit, &old, OPERATION);
    let refresh = session_intention(&old, AuthenticationActionV1::Refresh, Uuid::new_v4());
    let pending = audit
        .refresh_pending(refresh.clone(), 1, old.access_fingerprint)
        .unwrap();
    let completed = successful(
        refresh,
        pending.receipt,
        old.principal.clone(),
        AuthenticationReasonV1::Verified,
    );
    let before = audit.verify().unwrap();
    let pending_state = audit.session_current(old.session_id).unwrap().unwrap();
    assert!(matches!(
        pending_state.state,
        AuthenticationSessionStateV1::RefreshPending { .. }
    ));
    assert!(
        audit.append_typed(completed.clone()).is_err(),
        "generic refresh result must not consume the pending rotation receipt"
    );
    assert_eq!(audit.verify().unwrap(), before);
    assert_eq!(
        audit.session_current(old.session_id).unwrap(),
        Some(pending_state)
    );
    let mut next = old.clone();
    next.generation = 2;
    next.access_fingerprint =
        AuthenticationTokenFingerprintV1::access_token("typed-rotation-after-refused-observation");
    audit
        .rotate_session(completed, next.clone(), 1, old.access_fingerprint)
        .unwrap();
    assert_eq!(
        audit.session_current(old.session_id).unwrap(),
        Some(AuthenticationSessionCurrentV1 {
            binding: next.clone(),
            state: AuthenticationSessionStateV1::Active
        })
    );
    assert!(audit.fingerprint_seen(&next.access_fingerprint).unwrap());
}

// A failed or ambiguous provider result must atomically close Pending through
// the typed lane; a generic observation cannot consume that recovery receipt.
#[test]
fn ledger_generic_refresh_failure_result_preserves_pending_for_typed_close() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let binding = binding();
    admit(&audit, &binding, OPERATION);
    let refresh = session_intention(&binding, AuthenticationActionV1::Refresh, Uuid::new_v4());
    let pending = audit
        .refresh_pending(refresh.clone(), 1, binding.access_fingerprint)
        .unwrap();
    let mut completed = successful(
        refresh,
        pending.receipt,
        binding.principal.clone(),
        AuthenticationReasonV1::Verified,
    );
    completed.phase = AuthenticationPhaseV1::Result {
        outcome: AuthenticationOutcomeV1::Unknown,
        reason: AuthenticationReasonV1::ProviderUnavailable,
    };
    let before = audit.verify().unwrap();
    let pending_state = audit.session_current(binding.session_id).unwrap().unwrap();
    assert!(matches!(
        pending_state.state,
        AuthenticationSessionStateV1::RefreshPending { .. }
    ));
    assert!(
        audit.append_typed(completed.clone()).is_err(),
        "generic failure result must not consume the pending close receipt"
    );
    assert_eq!(audit.verify().unwrap(), before);
    assert_eq!(
        audit.session_current(binding.session_id).unwrap(),
        Some(pending_state)
    );
    audit
        .close_session(completed, 1, binding.access_fingerprint)
        .unwrap();
    assert_eq!(
        audit
            .session_current(binding.session_id)
            .unwrap()
            .unwrap()
            .state,
        AuthenticationSessionStateV1::Closed {
            reason: AuthenticationReasonV1::ProviderUnavailable
        }
    );
}

// A previously valid observation stays idempotent after later session tracking;
// retrieving its old receipt neither appends an entry nor changes authority.
#[test]
fn ledger_historical_generic_receipt_remains_idempotent_after_admission() {
    let root = private_directory();
    let path = root.path().join("authentication.sqlite");
    let audit = fresh(&path);
    let binding = binding();
    let observation = session_intention(&binding, AuthenticationActionV1::Refresh, Uuid::new_v4());
    let receipt = audit.append_typed(observation.clone()).unwrap();
    assert!(audit.session_current(binding.session_id).unwrap().is_none());
    admit(&audit, &binding, OPERATION);
    let before = audit.verify().unwrap();
    assert_eq!(audit.append_typed(observation).unwrap(), receipt);
    assert_eq!(audit.verify().unwrap(), before);
    assert_eq!(
        audit.session_current(binding.session_id).unwrap(),
        Some(AuthenticationSessionCurrentV1 {
            binding,
            state: AuthenticationSessionStateV1::Active,
        })
    );
}
