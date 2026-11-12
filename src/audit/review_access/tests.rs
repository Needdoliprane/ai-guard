use super::*;
use std::os::unix::fs::PermissionsExt;

fn fixture() -> (tempfile::TempDir, Store) {
    let temp = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let parent = std::fs::canonicalize(temp.path()).unwrap();
    let store = Store::open_for_tenant(
        &parent.join("audit.sqlite"),
        "tenant-A",
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    (temp, store)
}

// This is an unsupported-header refusal fixture, not a migrated modern Store:
// no source anchor, owner, incarnation, marker receipt or Ready is manufactured.
// Removing the pre-recovery fence would replay or invalidate this legacy origin.
#[test]
fn legacy_header_refusal_precedes_suspended_recovery_and_cache_publication() {
    let (_temp, store) = fixture();
    store.append(Event::new("fixture.before")).unwrap();
    let mut db = store.db.lock().unwrap();
    let floor = store.supervision_lease.state.lock().unwrap().floor.clone();
    let version = store.validated_data_version.load(Ordering::Acquire);
    let changes = store.validated_total_changes.load(Ordering::Acquire);
    let actual_format: i64 = db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    db.pragma_update(None, "user_version", 14).unwrap();
    store
        .supervision_lease
        .suspended
        .store(true, Ordering::Release);
    let error = match store.begin_checked(&mut db) {
        Ok(_) => panic!("a header alone must never authorize a modern transaction"),
        Err(error) => error,
    };
    assert_eq!(
        error.downcast_ref::<review_authority::ReviewAuthorityFailure>(),
        Some(&review_authority::ReviewAuthorityFailure::Required)
    );
    assert!(store.permanently_healthy());
    assert!(store.supervision_lease.suspended.load(Ordering::Acquire));
    assert_eq!(store.supervision_lease.state.lock().unwrap().floor, floor);
    assert_eq!(
        store.validated_data_version.load(Ordering::Acquire),
        version
    );
    assert_eq!(
        store.validated_total_changes.load(Ordering::Acquire),
        changes
    );
    db.pragma_update(None, "user_version", actual_format)
        .unwrap();
    let tx = store.begin_checked(&mut db).unwrap();
    store.commit_checked(tx).unwrap();
    drop(db);
    assert!(store.healthy());
    store.append(Event::new("fixture.after")).unwrap();
}

#[test]
fn unscoped_storage_and_diagnostic_checks_refuse_header_without_latching() {
    let (_temp, store) = fixture();
    let db = store.db.lock().unwrap();
    let actual_format: i64 = db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    db.pragma_update(None, "user_version", 14).unwrap();
    for result in [
        store.verify_storage(&db),
        store.verify_diagnostic_backing(&db),
    ] {
        assert_eq!(
            result
                .unwrap_err()
                .downcast_ref::<review_authority::ReviewAuthorityFailure>(),
            Some(&review_authority::ReviewAuthorityFailure::Required)
        );
        assert!(store.permanently_healthy());
    }
    drop(db);
    assert_eq!(
        store
            .integrity()
            .unwrap_err()
            .downcast_ref::<review_authority::ReviewAuthorityFailure>(),
        Some(&review_authority::ReviewAuthorityFailure::Required)
    );
    assert!(store.permanently_healthy());
    let db = store.db.lock().unwrap();
    db.pragma_update(None, "user_version", actual_format)
        .unwrap();
    store.verify_storage(&db).unwrap();
}

// A transaction received through a legacy signature cannot upgrade itself by
// changing its header and returning through the shared commit helper.
#[test]
fn legacy_commit_rechecks_format_and_rolls_back_without_advancing_floor() {
    let (_temp, store) = fixture();
    let mut db = store.db.lock().unwrap();
    let head = audit_head(&db).unwrap();
    let actual_format: i64 = db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let tx = store.begin_checked(&mut db).unwrap();
    tx.pragma_update(None, "user_version", 14).unwrap();
    let error = store.commit_checked(tx).unwrap_err();
    assert_eq!(
        error.downcast_ref::<review_authority::ReviewAuthorityFailure>(),
        Some(&review_authority::ReviewAuthorityFailure::Required)
    );
    assert!(store.healthy());
    assert_eq!(audit_head(&db).unwrap(), head);
    assert_eq!(store.supervision_lease.state.lock().unwrap().floor, head);
    assert_eq!(
        db.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        actual_format
    );
}

// A real transaction of another Store is a caller error, not evidence that
// either retained origin has diverged. Both real legacy origins remain usable.
#[test]
fn foreign_checked_transaction_refuses_without_invalidating_either_origin() {
    let (_first_temp, first) = fixture();
    let (_second_temp, second) = fixture();
    let mut db = first.db.lock().unwrap();
    let head = audit_head(&db).unwrap();
    let tx = first.begin_checked(&mut db).unwrap();
    assert_eq!(
        second
            .commit_checked(tx)
            .unwrap_err()
            .downcast_ref::<review_authority::ReviewAuthorityFailure>(),
        Some(&review_authority::ReviewAuthorityFailure::ForeignTransaction)
    );
    assert_eq!(audit_head(&db).unwrap(), head);
    drop(db);
    assert!(first.healthy());
    assert!(second.healthy());
    first.append(Event::new("fixture.first")).unwrap();
    second.append(Event::new("fixture.second")).unwrap();
}

#[test]
fn legacy_authority_helper_rechecks_current_header_before_identity_replay() {
    let (_temp, store) = fixture();
    let mut db = store.db.lock().unwrap();
    let tx = store.begin_checked(&mut db).unwrap();
    tx.require_identity_authority().unwrap();
    tx.require_business_access().unwrap();
    tx.pragma_update(None, "user_version", 14).unwrap();
    assert_eq!(
        tx.require_identity_authority()
            .unwrap_err()
            .downcast_ref::<review_authority::ReviewAuthorityFailure>(),
        Some(&review_authority::ReviewAuthorityFailure::Required)
    );
    assert_eq!(
        tx.require_business_access()
            .unwrap_err()
            .downcast_ref::<review_authority::ReviewAuthorityFailure>(),
        Some(&review_authority::ReviewAuthorityFailure::Required)
    );
    drop(tx);
    assert!(store.healthy());
}
