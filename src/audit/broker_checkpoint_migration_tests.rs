//! Real v11 source and independently reopened backup before migration12.
//! No broker workload, runtime or network is created by this regression.
use super::*;
use crate::control::{ControlFailure, QuotaPolicyV1};

#[path = "schema13_read_baseline_tests.rs"]
mod schema13_read_baseline_tests;

#[path = "schema13_compat_tests.rs"]
mod schema13_compat_tests;

fn retained_schema11() -> (tempfile::TempDir, std::path::PathBuf, Store) {
    let fixture = kubernetes_admission_state_tests::Fixture::new();
    let source = fixture._directory.path().join("tenant.sqlite");
    let authorization = fixture
        .store
        .authorize_held_create_for_test(
            &fixture.identity,
            &fixture.intent,
            &QuotaPolicyV1::default(),
            "2100-01-01T00:00:00.000Z",
        )
        .unwrap();
    let (_, reserved, replayed) = fixture
        .store
        .reserve_kubernetes_create(&fixture.identity, &authorization, &fixture.intent)
        .unwrap();
    assert!(!replayed && reserved.quota_held && reserved.claim.is_none());
    fixture
        .store
        .migrate_kubernetes_policy_v9(&source.with_extension("before9"))
        .unwrap();
    fixture
        .store
        .migrate_human_workers_v10(&source.with_extension("before10"))
        .unwrap();
    fixture
        .store
        .migrate_kubernetes_session_reservations_v11(&source.with_extension("before11"))
        .unwrap();
    drop(fixture.store);
    let store = Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
        .unwrap();
    assert_eq!(version(&store), 11);
    assert!(store.integrity().unwrap().valid);
    assert_eq!(held(&store), (1, 500, 134_217_728));
    assert!(!store.supervision_state().unwrap().active);
    (fixture._directory, source, store)
}

#[derive(Debug, PartialEq, Eq)]
struct DurableSnapshot {
    schema: i64,
    namespace: String,
    audit_bytes: i64,
    events: Vec<(i64, String, String, String, String)>,
    reservations: Vec<(String, String)>,
}

fn durable_snapshot(db: &Connection) -> DurableSnapshot {
    let mut events = db
        .prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events ORDER BY seq")
        .unwrap();
    let mut reservations = db
        .prepare("SELECT snapshot_json,snapshot_hash FROM kubernetes_creates ORDER BY create_id")
        .unwrap();
    DurableSnapshot {
        schema: db
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap(),
        namespace: db
            .query_row(
                "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap(),
        audit_bytes: db
            .query_row("SELECT bytes FROM quota WHERE id=1", [], |row| row.get(0))
            .unwrap(),
        events: events
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap(),
        reservations: reservations
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap(),
    }
}

fn version(store: &Store) -> i64 {
    store
        .db
        .lock()
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

fn rows(store: &Store) -> Vec<(i64, String, String)> {
    let db = store.db.lock().unwrap();
    let mut statement = db
        .prepare("SELECT seq,hash,payload FROM events ORDER BY seq")
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn held(store: &Store) -> (u32, u64, u64) {
    let quota = store.worker_quota(&QuotaPolicyV1::default()).unwrap();
    (
        quota.held_workers,
        quota.held_cpu_millis,
        quota.held_memory_bytes,
    )
}

#[test]
fn migration_11_to_12_preserves_held_quota_and_exact_backup() {
    let fixture = kubernetes_admission_state_tests::Fixture::new();
    let source = fixture._directory.path().join("tenant.sqlite");
    let authorization = fixture
        .store
        .authorize_held_create_for_test(
            &fixture.identity,
            &fixture.intent,
            &QuotaPolicyV1::default(),
            "2100-01-01T00:00:00.000Z",
        )
        .unwrap();
    let (_, reserved, replayed) = fixture
        .store
        .reserve_kubernetes_create(&fixture.identity, &authorization, &fixture.intent)
        .unwrap();
    assert!(!replayed && reserved.quota_held && reserved.claim.is_none());
    fixture
        .store
        .migrate_kubernetes_policy_v9(&source.with_extension("before9"))
        .unwrap();
    fixture
        .store
        .migrate_human_workers_v10(&source.with_extension("before10"))
        .unwrap();
    fixture
        .store
        .migrate_kubernetes_session_reservations_v11(&source.with_extension("before11"))
        .unwrap();
    drop(fixture.store);
    let store = Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
        .unwrap();
    assert_eq!(version(&store), 11);
    assert!(store.integrity().unwrap().valid);
    assert!(!store.supervision_state().unwrap().active);
    let before = rows(&store);
    let namespace = store.supervision_state().unwrap().control_namespace_uuid;
    assert!(namespace.is_some());
    assert_eq!(held(&store), (1, 500, 134_217_728));
    let backup_path = fixture
        ._directory
        .path()
        .join("before-checkpoints12.sqlite");

    // Existing migration dispatch gives a behavioral baseline without a new
    // stub, handwritten schema12 or fabricated migration result.
    let result = store.migrate_control_version(&backup_path, 11, 12);
    let backup =
        Store::open_existing_for_tenant(&backup_path, "tenant-a", Redactor::new(vec![]), 10_000)
            .unwrap();
    assert_eq!(version(&backup), 11);
    assert!(backup.integrity().unwrap().valid);
    assert_eq!(rows(&backup), before);
    assert_eq!(held(&backup), held(&store));
    drop(backup);
    if let Err(error) = &result {
        assert_eq!(
            error.downcast_ref::<ControlFailure>(),
            Some(&ControlFailure::InvalidRequest)
        );
        assert_eq!(version(&store), 11);
        assert_eq!(rows(&store), before);
        assert!(store.integrity().unwrap().valid);
        eprintln!("schema12_red_verified source11 backup_exact quota_held source_unchanged");
    }
    let report = result.unwrap_or_else(|error| {
        panic!("schema12_missing_migration_after_verified_backup: {error}")
    });
    assert_eq!((report.from_version, report.to_version), (11, 12));
    assert_eq!(version(&store), 12);
    assert_eq!(held(&store), (1, 500, 134_217_728));
    assert!(!store.supervision_state().unwrap().active);
    assert_eq!(
        store.supervision_state().unwrap().control_namespace_uuid,
        namespace
    );
    let after = rows(&store);
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(&after[..before.len()], before.as_slice());
    let marker = after.last().unwrap();
    assert_eq!(
        (report.genesis_seq, &report.genesis_hash),
        (marker.0, &marker.1)
    );
    let event: Event = serde_json::from_str(&marker.2).unwrap();
    assert_eq!(event.kind, "control.broker_checkpoints.genesis");
    assert_eq!(event.data["from_schema"], 11);
    assert!(store.integrity().unwrap().valid);
    drop(store);
    let reopened =
        Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
            .unwrap();
    assert_eq!(version(&reopened), 12);
    assert_eq!(rows(&reopened), after);
    assert_eq!(held(&reopened), (1, 500, 134_217_728));
    assert!(!reopened.supervision_state().unwrap().active);
}
