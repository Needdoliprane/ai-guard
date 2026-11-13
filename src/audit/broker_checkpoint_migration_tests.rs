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

#[test]
fn schema12_semantic_marker_gate_rejects_rehashed_missing_and_misbound_history() {
    // Removing the semantic gate, while retaining chain validation, must fail
    // this regression: every mutation below still has a valid audit hash chain.
    let (directory, _source, store) = retained_schema11();
    store
        .migrate_broker_checkpoints_v12(&directory.path().join("before12.sqlite"))
        .unwrap();
    assert!(store.integrity().unwrap().valid);
    let before = durable_snapshot(&store.db.lock().unwrap());
    let mut db = store.db.lock().unwrap();
    let (seq, received_at, previous_hash, original): (i64, String, String, String) = db
        .query_row(
            "SELECT seq,received_at,prev_hash,payload FROM events WHERE kind=?1",
            [broker_checkpoint_migration::GENESIS_KIND],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        audit_head(&db).unwrap().0,
        seq,
        "rewrite only the real tail"
    );
    broker_checkpoint_migration::validate(&db).unwrap();
    for case in [
        "missing",
        "tenant",
        "namespace",
        "from_schema",
        "predecessor",
        "hash",
        "future_timestamp",
    ] {
        let tx = db.transaction().unwrap();
        let mut event: Event = serde_json::from_str(&original).unwrap();
        let expected_error = match case {
            "missing" => {
                event.kind = "fixture.removed.broker.checkpoints.genesis".into();
                "invalid schema12 genesis count"
            }
            "from_schema" => {
                event.data["from_schema"] = serde_json::json!(10);
                "invalid broker checkpoint migration marker"
            }
            _ => {
                match case {
                    "tenant" => event.data["tenant_id"] = serde_json::json!("tenant-b"),
                    "namespace" => {
                        let other = "33333333-3333-4333-8333-333333333333";
                        assert_ne!(
                            event.data["control_namespace_uuid"],
                            serde_json::json!(other)
                        );
                        event.data["control_namespace_uuid"] = serde_json::json!(other);
                    }
                    "predecessor" => {
                        let old_seq = seq - 2;
                        assert!(old_seq > 0);
                        let old_hash: String = tx
                            .query_row("SELECT hash FROM events WHERE seq=?1", [old_seq], |row| {
                                row.get(0)
                            })
                            .unwrap();
                        event.data["previous_seq"] = serde_json::json!(old_seq);
                        event.data["previous_hash"] = serde_json::json!(old_hash);
                    }
                    "hash" => {
                        let wrong_hash = "0".repeat(64);
                        assert_ne!(previous_hash, wrong_hash);
                        event.data["previous_hash"] = serde_json::json!(wrong_hash);
                    }
                    "future_timestamp" => {
                        event.timestamp = crate::broker_registry::format_time(
                            crate::broker_registry::parse_time(&received_at).unwrap()
                                + chrono::Duration::milliseconds(1),
                        );
                    }
                    _ => unreachable!(),
                }
                // These are individually well-formed markers, rejected only
                // when compared with the retained tenant/namespace/head/time.
                broker_checkpoint_migration::marker(&event).unwrap();
                "broker checkpoint migration marker differs from retained control history"
            }
        };
        let payload = serde_json::to_string(&event).unwrap();
        let hash = super::digest(seq, &received_at, &previous_hash, &payload);
        assert_eq!(
            tx.execute(
                "UPDATE events SET kind=?1,payload=?2,hash=?3 WHERE seq=?4",
                params![event.kind, payload, hash, seq],
            )
            .unwrap(),
            1
        );
        tx.execute(
            "UPDATE quota SET bytes=bytes+?1 WHERE id=1",
            [i64::try_from(payload.len()).unwrap() - i64::try_from(original.len()).unwrap()],
        )
        .unwrap();
        Store::validate_metadata(&tx, "tenant-a").unwrap();
        assert!(
            Store::check_integrity(&tx).unwrap().valid,
            "{case}: chain precondition"
        );
        let error = broker_checkpoint_migration::validate(&tx).unwrap_err();
        assert!(
            error.to_string().contains(expected_error),
            "{case}: {error:#}"
        );
        let control_error = control::validate(&tx).unwrap_err();
        assert!(
            control_error.to_string().contains(expected_error),
            "{case}: the real control gate must reach the same semantic refusal: {control_error:#}"
        );
        tx.rollback().unwrap();
        assert_eq!(durable_snapshot(&db), before, "{case}: fixture rollback");
    }
    drop(db);
    assert!(store.integrity().unwrap().valid);
    assert_eq!(held(&store), (1, 500, 134_217_728));
}

#[test]
fn schema12_audit_cap_failure_keeps_source11_and_a_verified_reopenable_backup() {
    // Bypassing the mandatory genesis append or committing user_version before
    // it succeeds must fail this regression. No trigger intercepts preflight.
    let (directory, source, store) = retained_schema11();
    let before = durable_snapshot(&store.db.lock().unwrap());
    let reached_cap = u64::try_from(store.head().unwrap()).unwrap();
    assert_eq!(reached_cap, u64::try_from(before.events.len()).unwrap());
    drop(store);
    let store =
        Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), reached_cap)
            .unwrap();
    assert!(store.integrity().unwrap().valid);
    assert_eq!(durable_snapshot(&store.db.lock().unwrap()), before);
    let backup_path = directory.path().join("failed-before12.sqlite");
    let error = store
        .migrate_broker_checkpoints_v12(&backup_path)
        .unwrap_err();
    assert!(
        error.to_string().contains("audit capacity exhausted"),
        "must reach the mandatory append after verified backup: {error:#}"
    );
    assert_eq!(durable_snapshot(&store.db.lock().unwrap()), before);
    assert_eq!(version(&store), 11);
    assert!(store.integrity().unwrap().valid);
    assert_eq!(held(&store), (1, 500, 134_217_728));
    assert!(!store.supervision_state().unwrap().active);
    let backup =
        Store::open_existing_for_tenant(&backup_path, "tenant-a", Redactor::new(vec![]), 10_000)
            .unwrap();
    assert!(backup.integrity().unwrap().valid);
    assert_eq!(durable_snapshot(&backup.db.lock().unwrap()), before);
    assert_eq!(held(&backup), (1, 500, 134_217_728));
    assert!(!backup.supervision_state().unwrap().active);
    drop(backup);
    drop(store);
    let reopened =
        Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
            .unwrap();
    assert!(reopened.integrity().unwrap().valid);
    assert_eq!(durable_snapshot(&reopened.db.lock().unwrap()), before);
    assert_eq!(held(&reopened), (1, 500, 134_217_728));
}

#[test]
fn schema12_existing_backup_repeat_and_schema14_refuse_without_replacement() {
    use std::os::unix::fs::MetadataExt;

    // Overwriting an existing backup, duplicating migration12, or adopting a
    // future schema must fail at its boundary without replacing retained data.
    let (directory, source, store) = retained_schema11();
    let before11 = durable_snapshot(&store.db.lock().unwrap());
    let existing_path = source.with_extension("before11");
    let existing =
        Store::open_existing_for_tenant(&existing_path, "tenant-a", Redactor::new(vec![]), 10_000)
            .unwrap();
    assert_eq!(version(&existing), 10);
    assert!(existing.integrity().unwrap().valid);
    let existing_snapshot = durable_snapshot(&existing.db.lock().unwrap());
    drop(existing);
    let existing_bytes = std::fs::read(&existing_path).unwrap();
    let identity = std::fs::metadata(&existing_path).unwrap();
    let error = store
        .migrate_broker_checkpoints_v12(&existing_path)
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("create new migration backup destination"),
        "must refuse the existing destination, not an earlier preflight: {error:#}"
    );
    assert_eq!(std::fs::read(&existing_path).unwrap(), existing_bytes);
    let after_identity = std::fs::metadata(&existing_path).unwrap();
    assert_eq!(
        (after_identity.dev(), after_identity.ino()),
        (identity.dev(), identity.ino())
    );
    assert_eq!(durable_snapshot(&store.db.lock().unwrap()), before11);
    assert_eq!(held(&store), (1, 500, 134_217_728));
    let existing =
        Store::open_existing_for_tenant(&existing_path, "tenant-a", Redactor::new(vec![]), 10_000)
            .unwrap();
    assert!(existing.integrity().unwrap().valid);
    assert_eq!(
        durable_snapshot(&existing.db.lock().unwrap()),
        existing_snapshot
    );
    drop(existing);

    store
        .migrate_broker_checkpoints_v12(&directory.path().join("before12.sqlite"))
        .unwrap();
    let after12 = durable_snapshot(&store.db.lock().unwrap());
    let repeated_backup = directory.path().join("repeat-before12.sqlite");
    let error = store
        .migrate_broker_checkpoints_v12(&repeated_backup)
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::InvalidRequest)
    );
    assert!(
        !repeated_backup.exists(),
        "wrong source version must fail before creating backup"
    );
    assert!(store.integrity().unwrap().valid);
    assert_eq!(durable_snapshot(&store.db.lock().unwrap()), after12);
    assert_eq!(held(&store), (1, 500, 134_217_728));
    assert!(!store.supervision_state().unwrap().active);
    drop(store);

    // Change only the declared version on this closed synthetic database.
    // A failed Store open must neither downgrade nor initialize/replace it.
    let db = Connection::open_with_flags(
        &source,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    db.pragma_update(None, "user_version", 14).unwrap();
    let mut expected14 = after12;
    expected14.schema = 14;
    assert_eq!(durable_snapshot(&db), expected14);
    drop(db);
    let bytes14 = std::fs::read(&source).unwrap();
    let identity14 = std::fs::metadata(&source).unwrap();
    let error = Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
        .err()
        .expect("unknown schema14 must be refused");
    assert!(
        error
            .to_string()
            .contains("unsupported audit schema version"),
        "{error:#}"
    );
    assert_eq!(std::fs::read(&source).unwrap(), bytes14);
    let after_identity14 = std::fs::metadata(&source).unwrap();
    assert_eq!(
        (after_identity14.dev(), after_identity14.ino()),
        (identity14.dev(), identity14.ino())
    );
    let db = Connection::open_with_flags(
        &source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    assert_eq!(durable_snapshot(&db), expected14);
}
