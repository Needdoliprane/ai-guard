//! Real schema11/12 migration and read path; no runtime or network.
use super::*;

#[test]
fn schema12_federation_read_keeps_the_registry_available_after_checkpoint_migration() {
    let (directory, source, store) = retained_schema11();
    let canary = store.federation_workers_page(None, 10).unwrap();
    assert_eq!(canary.registry_status, "available");
    assert_eq!(held(&store), (1, 500, 134_217_728));
    store
        .migrate_broker_checkpoints_v12(&directory.path().join("before12.sqlite"))
        .unwrap();
    assert_eq!(version(&store), 12);
    assert!(store.integrity().unwrap().valid);
    let before = rows(&store);
    drop(store);
    let store = Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
        .unwrap();
    assert_eq!(version(&store), 12);
    assert!(store.integrity().unwrap().valid);
    assert!(!store.supervision_state().unwrap().active);
    assert_eq!(held(&store), (1, 500, 134_217_728));
    eprintln!(
        "schema12_federation_ready: schema11 read passed, real schema12 migration and inactive reopen valid, held quota retained"
    );
    let page = store
        .federation_workers_page(None, 10)
        .unwrap_or_else(|error| {
            panic!("schema12 must retain its inherited federation registry read: {error}")
        });
    assert_eq!(page.registry_status, "available");
    assert_eq!(page.source, "tenant_worker_broker_registry");
    assert_eq!(page.consistency, "snapshot_per_page");
    assert_eq!(page.runtime_inventory, "unknown");
    // This fixture holds a legacy CREATE, not a classical Worker row. Its
    // allocation remains visible in quota, and is not invented as an item.
    assert!(page.items.is_empty());
    assert_eq!(
        rows(&store),
        before,
        "registry reads must not rewrite history"
    );
    assert_eq!(held(&store), (1, 500, 134_217_728));
    assert!(store.integrity().unwrap().valid);
}
