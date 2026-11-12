//! Candidate regression source only. No native test was executed by this lot.
use super::*;
use std::os::unix::fs::PermissionsExt;

fn source13() -> (tempfile::TempDir, Store) {
    let root = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = root.path().canonicalize().unwrap();
    let store = Store::open_for_tenant(
        &path.join("audit.sqlite"),
        "A",
        Redactor::new(vec![]),
        10_000,
    )
    .unwrap();
    for target in 3..=13 {
        store
            .migrate_control_version(
                &path.join(format!("backup-{target}.sqlite")),
                target - 1,
                target,
            )
            .unwrap();
    }
    (root, store)
}

fn anchor(prefix: (i64, String)) -> SourceAnchorV1 {
    SourceAnchorV1 {
        version: 1,
        protocol: SOURCE_PROTOCOL.into(),
        from_format: 13,
        format: 14,
        migration_id: uuid::Uuid::from_u128(10),
        installation_id: uuid::Uuid::from_u128(1),
        source_id: uuid::Uuid::from_u128(2),
        kind: HumanSourceKindV1::TenantStore,
        tenant_id: "A".into(),
        durable_namespace_id: uuid::Uuid::from_u128(3),
        previous_seq: prefix.0,
        previous_hash: prefix.1,
    }
}

fn incarnation(anchor: AuthorityReceiptV1, prefix: (i64, String)) -> SourceIncarnationV1 {
    SourceIncarnationV1 {
        version: 1,
        protocol: SOURCE_PROTOCOL.into(),
        anchor,
        closure_id: uuid::Uuid::from_u128(4),
        closure_revision: 1,
        closure_digest: "ab".repeat(32),
        control_config_public_digest: "cd".repeat(32),
        owner_incarnation: uuid::Uuid::from_u128(5),
        revision: 1,
        previous_incarnation: None,
        previous_seq: prefix.0,
        previous_hash: prefix.1,
    }
}

// Break: accepting an ordinary format or any missing marker as modern authority.
#[test]
fn ordinary_thirteen_never_validates_as_review_authority() {
    let (_root, store) = source13();
    let db = store.db.lock().unwrap();
    assert!(replay(&db).is_err());
    assert!(validate(&db).is_ok()); // Historical scanner: no modern records is ordinary.
    assert!(
        db.query_row(
            "SELECT COUNT(*) FROM events WHERE kind GLOB 'control.review_authority.*'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap()
            == 0
    );
}

// Break: trusting a projection supplied separately from the durable event replay.
#[test]
fn exact_replay_rejects_projection_divergence_and_duplicate_anchor() {
    let (_root, store) = source13();
    let mut db = store.db.lock().unwrap();
    let tx = store.begin_checked(&mut db).unwrap();
    for (_, _, sql) in SCHEMA {
        tx.execute_batch(sql).unwrap();
    }
    tx.pragma_update(None, "user_version", 14).unwrap();
    let prefix = audit_head(&tx).unwrap();
    let mut value = anchor(prefix);
    value.durable_namespace_id = namespace(&tx).unwrap();
    let first = append_record(
        &store,
        &tx,
        uuid::Uuid::from_u128(10),
        AuthorityRecordV1::SourceAnchor(value.clone()),
        None,
    )
    .unwrap();
    assert!(replay(&tx).unwrap().incarnation.is_none()); // Dormant is not a live proof.
    let prefix = audit_head(&tx).unwrap();
    let second = incarnation(first, prefix);
    append_record(
        &store,
        &tx,
        uuid::Uuid::from_u128(11),
        AuthorityRecordV1::Incarnation(second),
        None,
    )
    .unwrap();
    let before = replay(&tx).unwrap();
    tx.execute(
        "UPDATE review_authority_meta SET incarnation_revision=8",
        [],
    )
    .unwrap();
    assert!(replay(&tx).is_err());
    write_projection(&tx, &before).unwrap();
    replay(&tx).unwrap();
    value.previous_seq = audit_head(&tx).unwrap().0;
    value.previous_hash = audit_head(&tx).unwrap().1;
    value.migration_id = uuid::Uuid::from_u128(12);
    assert!(
        append_record(
            &store,
            &tx,
            uuid::Uuid::from_u128(12),
            AuthorityRecordV1::SourceAnchor(value),
            None
        )
        .is_err()
    );
    drop(tx); // Real SQLite rollback, including the schema14 projection.
    assert!(Store::check_integrity(&db).unwrap().valid);
}

// Break: allowing an incarnation to name another anchor or skip its revision CAS.
#[test]
fn incarnation_requires_exact_anchor_and_previous_incarnation_receipt() {
    let mut projection = AuthorityProjection::default();
    let first = anchor((20, "11".repeat(32)));
    let first_receipt = AuthorityReceiptV1 {
        seq: 21,
        hash: "22".repeat(32),
    };
    projection
        .apply(
            AuthorityRecordV1::SourceAnchor(first),
            first_receipt.clone(),
        )
        .unwrap();
    let mut next = incarnation(first_receipt.clone(), (21, first_receipt.hash.clone()));
    next.anchor.hash = "33".repeat(32);
    assert!(
        projection
            .apply(
                AuthorityRecordV1::Incarnation(next.clone()),
                AuthorityReceiptV1 {
                    seq: 22,
                    hash: "44".repeat(32)
                }
            )
            .is_err()
    );
    next.anchor = first_receipt;
    next.revision = 2;
    assert!(
        projection
            .apply(
                AuthorityRecordV1::Incarnation(next),
                AuthorityReceiptV1 {
                    seq: 22,
                    hash: "44".repeat(32)
                }
            )
            .is_err()
    );
    assert!(projection.snapshot.unwrap().incarnation.is_none());
}

// Break: decoding unknown/malformed source metadata or putting closure_digest in
// the anchor itself, creating a marker/manifest digest circularity.
#[test]
fn anchor_is_closed_bounded_and_independent_of_closure_digest() {
    let value = anchor((20, "11".repeat(32)));
    value.validate().unwrap();
    let mut json = serde_json::to_value(&value).unwrap();
    json["closure_digest"] = serde_json::json!("ab".repeat(32));
    assert!(serde_json::from_value::<SourceAnchorV1>(json).is_err());
    let mut wrong = value;
    wrong.source_id = uuid::Uuid::nil();
    assert!(wrong.validate().is_err());
}

fn identical_private_copy(store: &Store, path: &std::path::Path) -> (StoreStorage, Connection) {
    let (backing, _initialization) = StoreStorage::create_new(path).unwrap();
    let mut destination = Connection::open_with_flags(
        backing.path(),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let source = store.db.lock().unwrap();
    {
        let backup = rusqlite::backup::Backup::new(&source, &mut destination).unwrap();
        assert_eq!(backup.step(-1).unwrap(), rusqlite::backup::StepResult::Done);
    }
    assert_eq!(
        audit_head(&source).unwrap(),
        audit_head(&destination).unwrap()
    );
    assert_ne!(store.backing.identity_key().1, backing.identity_key().1);
    (backing, destination)
}

// HRSC-01 break: HasMoved alone accepts an unrelated writer whose cloned
// namespace/history match. Refusal must not use the durable-mismatch latch.
#[test]
fn identical_copy_transaction_refuses_without_mutating_or_poisoning_the_source() {
    let (root, store) = source13();
    let (_copy, mut destination) = identical_private_copy(&store, &root.path().join("copy.sqlite"));
    let before = audit_head(&store.db.lock().unwrap()).unwrap();
    {
        let tx = destination
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        // Establish the precise old false assumption on the real VFS copy.
        store.backing.verify(&tx).unwrap();
        assert_eq!(
            namespace(&tx).unwrap(),
            namespace(&store.db.lock().unwrap()).unwrap()
        );
        let error = verify_connection_main(&store.backing, &tx).unwrap_err();
        assert_eq!(
            error.downcast_ref::<ReviewAuthorityFailure>(),
            Some(&ReviewAuthorityFailure::ForeignTransaction)
        );
        assert_eq!(audit_head(&tx).unwrap(), before);
        assert_eq!(audit_head(&store.db.lock().unwrap()).unwrap(), before);
        assert!(store.healthy());
    }
    verify_connection_main(&store.backing, &store.db.lock().unwrap()).unwrap();
    store
        .append(Event::new("fixture.source_after_foreign_transaction"))
        .unwrap();
    assert!(store.healthy());
    assert_eq!(audit_head(&destination).unwrap(), before);
}

// HRSC-01 break: classifying a real retained-main replacement as a harmless
// foreign argument would suppress the existing permanent backing failure.
#[test]
fn actual_retained_backing_replacement_keeps_the_permanent_failure_path() {
    let (root, store) = source13();
    let original_head = audit_head(&store.db.lock().unwrap()).unwrap();
    let copy_path = root.path().join("replacement.sqlite");
    let (_copy, destination) = identical_private_copy(&store, &copy_path);
    destination.close().unwrap();
    let original = store.backing.path().to_path_buf();
    let preserved = root.path().join("preserved-original.sqlite");
    std::fs::rename(&original, &preserved).unwrap();
    std::fs::rename(&copy_path, &original).unwrap();
    let error = verify_connection_main(&store.backing, &store.db.lock().unwrap()).unwrap_err();
    assert_ne!(
        error.downcast_ref::<ReviewAuthorityFailure>(),
        Some(&ReviewAuthorityFailure::ForeignTransaction)
    );
    assert!(
        store
            .append(Event::new("fixture.after_retained_replacement"))
            .is_err()
    );
    assert!(!store.healthy());
    assert_eq!(
        audit_head(&store.db.lock().unwrap()).unwrap(),
        original_head
    );
    assert!(preserved.exists()); // The actual original was not deleted/repaired.
}

// HRSC-01 break: matching main alone must not admit an additional schema that
// changes the namespace used by unqualified application queries.
#[test]
fn an_additional_schema_refuses_until_detached_without_poisoning_the_source() {
    let (_root, store) = source13();
    let db = store.db.lock().unwrap();
    let prefix = audit_head(&db).unwrap();
    db.execute_batch("ATTACH DATABASE ':memory:' AS auxiliary;")
        .unwrap();
    let error = verify_connection_main(&store.backing, &db).unwrap_err();
    assert_eq!(
        error.downcast_ref::<ReviewAuthorityFailure>(),
        Some(&ReviewAuthorityFailure::ForeignTransaction)
    );
    assert_eq!(audit_head(&db).unwrap(), prefix);
    assert!(store.healthy());
    db.execute_batch("DETACH DATABASE auxiliary;").unwrap();
    verify_connection_main(&store.backing, &db).unwrap();
}

// Break: SQLite retains an initialized TEMP namespace after its last object is
// dropped; rejecting that empty namespace rejects our otherwise exact main.
#[test]
fn an_initialized_empty_temporary_schema_keeps_the_exact_main_connection_valid() {
    let (_root, store) = source13();
    let db = store.db.lock().unwrap();
    let prefix = audit_head(&db).unwrap();
    db.execute_batch(
        "PRAGMA temp_store=MEMORY;
         CREATE TEMP TABLE connection_probe(value INTEGER);
         DROP TABLE temp.connection_probe;",
    )
    .unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM temp.sqlite_schema", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    verify_connection_main(&store.backing, &db).unwrap();
    assert_eq!(audit_head(&db).unwrap(), prefix);
    drop(db);
    assert!(store.healthy());
}

// Break: admitting TEMP by filename alone permits objects, including names that
// shadow the main schema. Even an unrelated in-memory table must refuse.
#[test]
fn temporary_objects_refuse_until_dropped_without_poisoning_the_source() {
    let (_root, store) = source13();
    let db = store.db.lock().unwrap();
    let prefix = audit_head(&db).unwrap();
    db.execute_batch("PRAGMA temp_store=MEMORY;").unwrap();
    for (create, remove) in [
        (
            "CREATE TEMP TABLE connection_probe(value INTEGER);",
            "DROP TABLE temp.connection_probe;",
        ),
        (
            "CREATE TEMP TABLE events(seq INTEGER);",
            "DROP TABLE temp.events;",
        ),
    ] {
        db.execute_batch(create).unwrap();
        let error = verify_connection_main(&store.backing, &db).unwrap_err();
        assert_eq!(
            error.downcast_ref::<ReviewAuthorityFailure>(),
            Some(&ReviewAuthorityFailure::ForeignTransaction)
        );
        db.execute_batch(remove).unwrap();
        verify_connection_main(&store.backing, &db).unwrap();
        assert_eq!(audit_head(&db).unwrap(), prefix);
    }
    drop(db);
    assert!(store.healthy());
}

// Break: checking only the first two rows hides an attached schema after main
// and initialized empty TEMP. Detaching it must restore the legitimate source.
#[test]
fn an_attached_schema_after_empty_temp_refuses_until_detached() {
    let (_root, store) = source13();
    let db = store.db.lock().unwrap();
    let prefix = audit_head(&db).unwrap();
    db.execute_batch(
        "PRAGMA temp_store=MEMORY;
         CREATE TEMP TABLE connection_probe(value INTEGER);
         DROP TABLE temp.connection_probe;
         ATTACH DATABASE ':memory:' AS auxiliary;",
    )
    .unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM pragma_database_list", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        3
    );
    let error = verify_connection_main(&store.backing, &db).unwrap_err();
    assert_eq!(
        error.downcast_ref::<ReviewAuthorityFailure>(),
        Some(&ReviewAuthorityFailure::ForeignTransaction)
    );
    assert_eq!(audit_head(&db).unwrap(), prefix);
    db.execute_batch("DETACH DATABASE auxiliary;").unwrap();
    verify_connection_main(&store.backing, &db).unwrap();
    assert_eq!(audit_head(&db).unwrap(), prefix);
    drop(db);
    assert!(store.healthy());
}
