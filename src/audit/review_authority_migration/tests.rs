//! Real SQLite/Store candidate tests. All NOT_RUN in the source-only lot.
use super::*;
use std::os::unix::fs::PermissionsExt;
fn source13() -> (tempfile::TempDir, Store) {
    let root = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = root.path().canonicalize().unwrap();
    let store = Store::open_for_tenant(
        &path.join("source.sqlite"),
        "A",
        Redactor::new(vec![]),
        10_000,
    )
    .unwrap();
    for version in 3..=13 {
        store
            .migrate_control_version(
                &path.join(format!("old-{version}.sqlite")),
                version - 1,
                version,
            )
            .unwrap();
    }
    (root, store)
}
fn backup_path(root: &Path) -> std::path::PathBuf {
    let directory =
        crate::control_fs::Directory::open_or_create(&root.join("fresh-backup")).unwrap();
    directory.durable_bootstrap().unwrap();
    directory.path().join("source.sqlite")
}
// Break: backup comes from a different prefix or an existing destination is overwritten.
#[test]
fn pinned_writer_backup_is_exact_new_and_original_prefix_unchanged() {
    let (root, store) = source13();
    let mut db = store.db.lock().unwrap();
    let tx = store.begin_checked(&mut db).unwrap();
    let prefix = audit_head(&tx).unwrap();
    let path = backup_path(&root.path().canonicalize().unwrap());
    let backup = copy_verified_backup(&store, &tx, &path).unwrap();
    backup.revalidate(&store, &tx).unwrap();
    assert_eq!(audit_head(&tx).unwrap(), prefix);
    assert!(copy_verified_backup(&store, &tx, &path).is_err());
    backup.revalidate(&store, &tx).unwrap();
}
// Break: using a Deferred/read transaction as the old-writer fence.
#[test]
fn a_read_transaction_cannot_pin_the_offline_migration() {
    let (root, store) = source13();
    let mut db = store.db.lock().unwrap();
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)
        .unwrap();
    audit_head(&tx).unwrap();
    let path = backup_path(&root.path().canonicalize().unwrap());
    assert!(copy_verified_backup(&store, &tx, &path).is_err());
    assert!(!path.exists());
}
// Break: applying DDL after a source prefix has changed since its retained backup.
#[test]
fn a_changed_writer_prefix_cannot_reuse_a_verified_backup() {
    let (root, store) = source13();
    let mut db = store.db.lock().unwrap();
    let tx = store.begin_checked(&mut db).unwrap();
    let backup = copy_verified_backup(
        &store,
        &tx,
        &backup_path(&root.path().canonicalize().unwrap()),
    )
    .unwrap();
    let (event, payload) = store
        .prepare_event(Event::new("fixture.after_backup"))
        .unwrap();
    store.append_in_transaction(&tx, event, payload).unwrap();
    assert!(backup.revalidate(&store, &tx).is_err());
}
// Break: accepting a corrupted/tampered SQLite backup merely because it exists.
#[test]
fn changed_backup_contents_refuse_without_repairing_the_original() {
    let (root, store) = source13();
    let mut db = store.db.lock().unwrap();
    let tx = store.begin_checked(&mut db).unwrap();
    let prefix = audit_head(&tx).unwrap();
    let path = backup_path(&root.path().canonicalize().unwrap());
    let backup = copy_verified_backup(&store, &tx, &path).unwrap();
    let changed = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    changed
        .execute(
            "UPDATE events SET hash=?1 WHERE seq=(SELECT MAX(seq) FROM events)",
            ["00".repeat(32)],
        )
        .unwrap();
    changed.close().unwrap();
    assert!(backup.revalidate(&store, &tx).is_err());
    assert_eq!(audit_head(&tx).unwrap(), prefix);
    assert!(Store::check_integrity(&tx).unwrap().valid);
}

// HRSC-01 break: a foreign cloned writer can pass metadata/HasMoved and create
// a backup/migration result attributed to the retained source's different inode.
#[test]
fn foreign_writer_refuses_before_backup_creation_and_both_prefixes_stay_unchanged() {
    let (root, store) = source13();
    let mut db = store.db.lock().unwrap();
    let tx = store.begin_checked(&mut db).unwrap();
    let prefix = audit_head(&tx).unwrap();
    let copy_path = backup_path(&root.path().canonicalize().unwrap());
    let copy = copy_verified_backup(&store, &tx, &copy_path).unwrap();
    let mut foreign = Connection::open_with_flags(
        &copy_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let target = copy_path
        .parent()
        .unwrap()
        .join("must-not-be-created.sqlite");
    {
        let foreign_tx = foreign
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        store.backing.verify(&foreign_tx).unwrap(); // Precise insufficient old guard.
        assert_eq!(audit_head(&foreign_tx).unwrap(), prefix);
        let failure = copy_verified_backup(&store, &foreign_tx, &target);
        assert!(matches!(
            failure
                .as_ref()
                .err()
                .and_then(|e| e.downcast_ref::<ReviewAuthorityFailure>()),
            Some(ReviewAuthorityFailure::ForeignTransaction)
        ));
        assert!(!target.exists());
        assert_eq!(audit_head(&foreign_tx).unwrap(), prefix);
        assert_eq!(audit_head(&tx).unwrap(), prefix);
        assert!(store.healthy());
    }
    copy.revalidate(&store, &tx).unwrap(); // Exact retained writer remains usable.
    assert_eq!(audit_head(&foreign).unwrap(), prefix);
}
