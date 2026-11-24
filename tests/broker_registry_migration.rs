mod support;

use agentsandbox::{
    audit::{AuditCapacityFailureV1, Store},
    model::Event,
    redaction::Redactor,
};

fn v5(path: &std::path::Path, capacity: u64) -> Store {
    let store = Store::open_for_tenant(path, "tenant-a", Redactor::new(vec![]), capacity).unwrap();
    store
        .migrate_control_v3(&path.with_extension("v2.bak"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("v3.bak"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("v4.bak"))
        .unwrap();
    store
}

#[test]
fn workload_migration_preserves_exact_backup_and_does_not_reactivate_reopened_origin() {
    let directory = support::private_tempdir();
    let path = directory.path().join("tenant.sqlite");
    let store = v5(&path, 1000);
    store.append(Event::new("test.preserved")).unwrap();
    let before = store.integrity().unwrap();
    assert!(store.supervision_state().unwrap().active);
    let backup = directory.path().join("workload-v5.bak");
    let report = store.migrate_workload_v6(&backup).unwrap();
    assert_eq!((report.from_version, report.to_version), (5, 6));
    assert_eq!(store.supervision_state().unwrap().schema_version, 6);
    assert!(store.supervision_state().unwrap().active);
    let backed =
        Store::open_existing_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    let backed_integrity = backed.integrity().unwrap();
    assert_eq!(backed_integrity.head_hash, before.head_hash);
    assert_eq!(backed_integrity.event_count, before.event_count);
    assert_eq!(backed.supervision_state().unwrap().schema_version, 5);
    assert!(!backed.supervision_state().unwrap().active);
    assert!(backed.brokers_page(None, 1).is_err());
    drop(store);
    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert!(reopened.integrity().unwrap().valid);
    assert!(!reopened.supervision_state().unwrap().active);
    assert!(reopened.brokers_page(None, 100).unwrap().is_empty());
    assert!(
        reopened
            .migrate_workload_v6(&directory.path().join("repeat.bak"))
            .is_err()
    );
    assert!(!directory.path().join("repeat.bak").exists());
}

#[test]
fn late_migration_audit_failure_rolls_back_ddl_version_and_head_but_retains_backup() {
    let directory = support::private_tempdir();
    let path = directory.path().join("tenant.sqlite");
    let store = v5(&path, 16);
    let mut capacity = store.capacity_policy().clone();
    capacity.control_reserve.events = 1;
    capacity.identity_reserve.events = 6;
    let store = store.with_capacity_policy(capacity).unwrap();
    while store.head().unwrap() < 9 {
        store.append(Event::new("test.fill")).unwrap();
    }
    let before = store.integrity().unwrap();
    // With six slots reserved for identity, one control write fits at head 9.
    // The second genesis must fail and roll back both the first genesis and DDL.
    let backup = directory.path().join("workload-v5.bak");
    let error = store.migrate_workload_v6(&backup).unwrap_err();
    assert_eq!(
        error.downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted)
    );
    assert!(backup.exists());
    assert!(store.healthy());
    assert_eq!(store.integrity().unwrap().head_hash, before.head_hash);
    assert_eq!(store.supervision_state().unwrap().schema_version, 5);
    let db = rusqlite::Connection::open(&path).unwrap();
    let tables:u32 = db.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE name GLOB 'workload_*' OR name GLOB 'broker_registry_*' OR name GLOB 'relay_*'",[],|row|row.get(0)).unwrap();
    assert_eq!(tables, 0);
    drop(db);
    let backed =
        Store::open_existing_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 16).unwrap();
    assert_eq!(backed.integrity().unwrap().head_hash, before.head_hash);
}

#[test]
fn schema6_rejects_missing_index_and_unknown_objects_instead_of_repairing() {
    for mutation in [
        "DROP INDEX workload_binding_history_prefix",
        "CREATE TABLE unrecognized(extra TEXT)",
    ] {
        let directory = support::private_tempdir();
        let path = directory.path().join("tenant.sqlite");
        let store = v5(&path, 1000);
        store
            .migrate_workload_v6(&directory.path().join("workload-v5.bak"))
            .unwrap();
        drop(store);
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(mutation).unwrap();
        drop(db);
        assert!(
            Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000)
                .is_err()
        );
    }
}
