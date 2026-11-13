//! Execution remains NOT_RUN until the frozen source is admitted to QA.
use super::*;
use crate::human_authority::migration::MigratedHumanSources;
use crate::human_authority::migration::tests::Fixture;

fn read<T>(
    bundle: &MigratedHumanSources,
    tenant: &str,
    work: impl FnOnce(&Store, &HumanSourceAccess<'_>) -> T,
) -> T {
    let descriptor = bundle
        .owner()
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some(tenant))
        .unwrap();
    let token = bundle.owner().source_token(descriptor.source_id).unwrap();
    let epoch = bundle.owner().try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &token).unwrap();
    work(bundle.store(descriptor.source_id).unwrap(), &access)
}

#[test]
fn mutex_a_stays_locked_until_b_authentication_and_profile_receipt_commit() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    let a = bundle.store(uuid::Uuid::from_u128(1)).unwrap();
    let held = a.db.lock().unwrap();
    read(&bundle, "B", |store, access| {
        let before = audit_head(&store.db.lock().unwrap()).unwrap().0;
        let credential = store
            .modern_authenticate_local_digest(access, &[0xad; 32])
            .unwrap()
            .unwrap();
        let profile = store
            .modern_human_profile(access, &credential.principal, &Redactor::new(vec![]))
            .unwrap();
        assert!(profile.account.is_some());
        assert!(audit_head(&store.db.lock().unwrap()).unwrap().0 > before);
    });
    assert!(a.db.try_lock().is_err());
    drop(held);
}

#[test]
fn sqlite_a_immediate_lock_remains_held_until_b_authentication_and_receipt() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    let a = bundle.store(uuid::Uuid::from_u128(1)).unwrap();
    let connection = rusqlite::Connection::open(a.backing.path()).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    read(&bundle, "A", |store, access| {
        let error = store
            .modern_human_profile(access, fixture.principal(), &Redactor::new(vec![]))
            .err()
            .unwrap();
        assert_eq!(
            error.downcast_ref::<review_access::ReviewAccessFailure>(),
            Some(&review_access::ReviewAccessFailure::Busy)
        );
        assert!(store.healthy());
    });
    read(&bundle, "B", |store, access| {
        let credential = store
            .modern_authenticate_local_digest(access, &[0xad; 32])
            .unwrap()
            .unwrap();
        let profile = store
            .modern_human_profile(access, &credential.principal, &Redactor::new(vec![]))
            .unwrap();
        assert!(profile.account.is_some());
    });
    assert!(!connection.is_autocommit());
    connection.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn own_sqlite_busy_refuses_without_latching_and_restores_timeout() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    read(&bundle, "B", |store, access| {
        store
            .db
            .lock()
            .unwrap()
            .busy_timeout(std::time::Duration::from_millis(713))
            .unwrap();
        let connection = rusqlite::Connection::open(store.backing.path()).unwrap();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert!(
            store
                .modern_authenticate_local_digest(access, &[0xad; 32])
                .is_err()
        );
        let timeout: u64 = store
            .db
            .lock()
            .unwrap()
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        assert_eq!(timeout, 713);
        assert!(store.healthy());
        connection.execute_batch("ROLLBACK").unwrap();
        assert!(
            store
                .modern_authenticate_local_digest(access, &[0xad; 32])
                .unwrap()
                .is_some()
        );
    });
}

#[test]
fn own_mutex_busy_refuses_without_waiting_or_changing_timeout() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    read(&bundle, "B", |store, access| {
        let held = store.db.lock().unwrap();
        let error = store
            .modern_authenticate_local_digest(access, &[0xad; 32])
            .err()
            .unwrap();
        assert_eq!(
            error.downcast_ref::<review_access::ReviewAccessFailure>(),
            Some(&review_access::ReviewAccessFailure::Busy)
        );
        drop(held);
    });
}
