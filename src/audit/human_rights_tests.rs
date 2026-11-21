//! Forged or unknown identity-chain facts never open: they are refused when
//! written, or the store refuses to reopen. Real private SQLite stores.
use super::*;
use std::os::unix::fs::PermissionsExt;

fn private() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    directory
}
fn migrated(path: &Path, through_workers: bool) -> Store {
    let store = Store::open_for_tenant(path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    let steps: Vec<fn(&Store, &Path) -> anyhow::Result<crate::control::MigrationReportV1>> = vec![
        Store::migrate_control_v3,
        Store::migrate_identity_v4,
        Store::migrate_human_access_v5,
        Store::migrate_workload_v6,
        Store::migrate_human_brokers_v7,
        Store::migrate_kubernetes_create_v8,
        Store::migrate_kubernetes_policy_v9,
        Store::migrate_human_workers_v10,
        Store::migrate_kubernetes_session_reservations_v11,
        Store::migrate_broker_checkpoints_v12,
        Store::migrate_kubernetes_recovery_v13,
    ];
    let count = if through_workers { steps.len() } else { 7 };
    for (index, step) in steps.into_iter().take(count).enumerate() {
        step(&store, &path.with_extension(format!("before{}", index + 3))).unwrap();
    }
    store
}
/// Appends one identity-namespace event through the store's own chained
/// writer, as a future binary would. Returns the write refusal, if any.
fn append_identity(store: &Store, kind: &str, data: Value) -> anyhow::Result<()> {
    let mut db = store.db.lock().unwrap();
    let tx = store.begin_checked(&mut db)?;
    let mut event = Event::new(kind);
    event.data = data;
    let (event, payload) = store.prepare_event(event)?;
    store.append_identity_in_transaction(&tx, event, payload)?;
    store.commit_checked(tx)
}
/// The fact is never accepted: either its write is refused with the head
/// unchanged, or the store refuses to reopen. Returns the refusal text.
fn never_accepted(path: &Path, store: Store, kind: &str, data: Value) -> String {
    let head = store.head().unwrap();
    if let Err(error) = append_identity(&store, kind, data) {
        assert_eq!(
            store.head().unwrap(),
            head,
            "a refused write leaves the head"
        );
        return format!("{error:#}");
    }
    drop(store);
    // Opening records the failed validation instead of refusing the handle:
    // the reopened store is unhealthy and refuses its first checked write.
    match Store::open_existing_for_tenant(path, "tenant-a", Redactor::new(vec![]), 1000) {
        Ok(reopened) => {
            assert!(
                !reopened.healthy(),
                "a store carrying {kind} reopened healthy"
            );
            let refusal = reopened
                .append(Event::new("fixture.after_unknown_identity"))
                .expect_err("an unhealthy store refuses writes");
            format!("{error:#}", error = refusal)
        }
        Err(error) => format!("{error:#}"),
    }
}
fn marker_payload(store: &Store) -> Value {
    let db = store.db.lock().unwrap();
    let (payload,): (String,) = db
        .query_row(
            "SELECT payload FROM events WHERE kind=?1",
            [GENESIS_KIND],
            |r| Ok((r.get(0)?,)),
        )
        .unwrap();
    let event: Event = serde_json::from_str(&payload).unwrap();
    event.data
}

#[test]
fn an_unknown_identity_kind_fails_closed_like_an_older_binary_meets_the_marker() {
    // An older binary replays the identity chain with the same closed match:
    // the marker is, for it, exactly this kind of unknown identity event.
    let files = private();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    let refusal = never_accepted(
        &path,
        store,
        "control.identity.future_capability.genesis",
        json!({"version": 1}),
    );
    assert!(!refusal.is_empty());
}

#[test]
fn a_second_marker_is_refused() {
    let files = private();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    store
        .migrate_human_rights_v2(&path.with_extension("exact13"))
        .unwrap();
    let copy = marker_payload(&store);
    let refusal = never_accepted(&path, store, GENESIS_KIND, copy);
    assert!(
        refusal.contains("human rights marker"),
        "the marker checks refuse it: {refusal}"
    );
}

#[test]
fn a_marker_that_differs_from_its_durable_prefix_is_refused() {
    let files = private();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    // A well-formed payload copied from a real marker of another store, so
    // only its bindings (namespace, genesis, prefix) are wrong here.
    let other_files = private();
    let other_path = other_files.path().join("other.sqlite");
    let other = migrated(&other_path, true);
    other
        .migrate_human_rights_v2(&other_path.with_extension("exact13"))
        .unwrap();
    let forged = marker_payload(&other);
    let refusal = never_accepted(&path, store, GENESIS_KIND, forged);
    assert!(
        refusal.contains("invalid human rights marker"),
        "the marker validation refuses it: {refusal}"
    );
}

#[test]
fn a_marker_on_a_store_without_worker_access_is_refused() {
    let files = private();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, false);
    let refusal = never_accepted(&path, store, GENESIS_KIND, json!({"version": 2}));
    assert!(
        refusal.contains("worker access marker") || refusal.contains("human rights marker"),
        "the format check refuses it: {refusal}"
    );
}

#[test]
fn new_read_actions_wait_for_the_marker_with_the_head_unchanged() {
    let files = private();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: uuid::Uuid::new_v4().to_string(),
        account_id: uuid::Uuid::new_v4().to_string(),
    };
    let head = store.head().unwrap();
    for action in ["summary", "memberships", "quota", "usage", "configuration"] {
        let mut db = store.db.lock().unwrap();
        let tx = db.transaction().unwrap();
        let refusal = store
            .record_human_read(&tx, &principal, None, action, 0, &Redactor::new(vec![]))
            .unwrap_err();
        assert_eq!(
            refusal.downcast_ref::<IdentityFailureV1>(),
            Some(&IdentityFailureV1::MigrationRequired),
            "{action} is recorded only after the marker"
        );
        drop(tx);
        drop(db);
        assert_eq!(store.head().unwrap(), head);
    }
}
