use super::{EventQuery, Redactor, Store};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

const KUBERNETES_GENESIS_KIND: &str = "control.kubernetes.create.genesis";

fn migrated_schema8() -> (tempfile::TempDir, std::path::PathBuf, Store) {
    let directory = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("create private schema8 fixture");
    let path = directory.path().join("tenant.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000)
        .expect("create tenant audit");
    store
        .migrate_control_v3(&directory.path().join("before-3.sqlite"))
        .expect("migrate schema2 to schema3");
    store
        .migrate_identity_v4(&directory.path().join("before-4.sqlite"))
        .expect("migrate schema3 to schema4");
    store
        .migrate_human_access_v5(&directory.path().join("before-5.sqlite"))
        .expect("migrate schema4 to schema5");
    store
        .migrate_workload_v6(&directory.path().join("before-6.sqlite"))
        .expect("migrate schema5 to schema6");
    store
        .migrate_human_brokers_v7(&directory.path().join("before-7.sqlite"))
        .expect("migrate schema6 to schema7");
    store
        .migrate_kubernetes_create_v8(&directory.path().join("before-8.sqlite"))
        .expect("migrate schema7 to schema8");
    assert!(store.healthy(), "freshly migrated schema8 is healthy");
    assert!(store.integrity().unwrap().valid);
    (directory, path, store)
}

fn rewrite_kubernetes_genesis(path: &Path, mutate: impl FnOnce(&mut Value)) {
    let db = Connection::open(path).expect("open closed fixture for deliberate corruption");
    let (seq, received_at, previous_hash, payload): (i64, String, String, String) = db
        .query_row(
            "SELECT seq,received_at,prev_hash,payload FROM events WHERE kind=?1",
            [KUBERNETES_GENESIS_KIND],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("read real migration genesis");
    let mut event: Value = serde_json::from_str(&payload).expect("decode genesis payload");
    mutate(&mut event);
    let kind = event["kind"]
        .as_str()
        .expect("genesis kind is text")
        .to_owned();
    let payload = serde_json::to_string(&event).expect("encode deliberate genesis mutation");
    let hash = super::digest(seq, &received_at, &previous_hash, &payload);
    let byte_delta = i64::try_from(payload.len()).unwrap()
        - i64::try_from(
            db.query_row::<String, _, _>("SELECT payload FROM events WHERE seq=?1", [seq], |row| {
                row.get(0)
            })
            .unwrap()
            .len(),
        )
        .unwrap();
    db.execute(
        "UPDATE events SET kind=?1,payload=?2,hash=?3 WHERE seq=?4",
        params![kind, payload, hash, seq],
    )
    .expect("persist deliberate genesis mutation");
    db.execute("UPDATE quota SET bytes=bytes+?1 WHERE id=1", [byte_delta])
        .expect("keep audit quota consistent with rehashed fixture");
}

fn assert_corrupt_schema8_is_diagnostic_only(path: &Path, case: &str) {
    let reopened = Store::open_existing_for_tenant(path, "tenant-a", Redactor::new(vec![]), 10_000)
        .expect("maintenance open must return a diagnostic handle");
    assert!(!reopened.healthy(), "marked {case} healthy on reopen");
    let result = reopened.integrity();
    assert!(
        matches!(result, Err(_) | Ok(super::Integrity { valid: false, .. })),
        "schema8 integrity validation accepted {case}"
    );
    assert!(
        reopened.supervision_state().is_err(),
        "enabled supervision for {case}"
    );
    let before = store_event_count(&reopened);
    assert!(
        reopened
            .append(super::Event::new("fixture.integrity_probe"))
            .is_err(),
        "accepted an event append for {case}"
    );
    assert_eq!(store_event_count(&reopened), before, "appended for {case}");
    assert!(!reopened.healthy(), "restored health for {case}");
}

fn store_event_count(store: &Store) -> i64 {
    store
        .db
        .lock()
        .expect("lock audit for no-append assertion")
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .expect("count audit events")
}

#[test]
fn explicit_schema8_migration_preserves_schema7_backup_and_reopens_inactive() {
    let directory = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("create private migration fixture");
    let path = directory.path().join("tenant.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000)
        .expect("create tenant audit");
    store
        .migrate_control_v3(&directory.path().join("before-3.sqlite"))
        .expect("migrate schema2 to schema3");
    store
        .migrate_identity_v4(&directory.path().join("before-4.sqlite"))
        .expect("migrate schema3 to schema4");
    store
        .migrate_human_access_v5(&directory.path().join("before-5.sqlite"))
        .expect("migrate schema4 to schema5");
    store
        .migrate_workload_v6(&directory.path().join("before-6.sqlite"))
        .expect("migrate schema5 to schema6");
    store
        .migrate_human_brokers_v7(&directory.path().join("before-7.sqlite"))
        .expect("migrate schema6 to schema7");

    let before = store.integrity().expect("validate schema7 audit");
    assert!(before.valid);
    assert_eq!(store.supervision_state().unwrap().schema_version, 7);
    let backup = directory.path().join("before-8.sqlite");

    let report = store
        .migrate_control_version(&backup, 7, 8)
        .expect("explicitly migrate schema7 to schema8");
    assert_eq!(report.from_version, 7);
    assert_eq!(report.to_version, 8);
    assert!(backup.is_file());

    let migrated_integrity = store.integrity().expect("validate schema8 audit");
    assert!(migrated_integrity.valid, "{:?}", migrated_integrity.error);
    assert_eq!(migrated_integrity.event_count, before.event_count + 1);
    assert_eq!(report.genesis_seq as u64, migrated_integrity.event_count);
    assert_eq!(report.genesis_hash, migrated_integrity.head_hash);
    assert_eq!(store.supervision_state().unwrap().schema_version, 8);
    let genesis = store
        .events(&EventQuery {
            kind: Some("control.kubernetes.create.genesis".into()),
            ..Default::default()
        })
        .expect("read Kubernetes CREATE genesis");
    assert_eq!(genesis.len(), 1);
    assert_eq!(genesis[0].seq as u64, report.genesis_seq as u64);

    {
        let db = store.db.lock().expect("lock migrated audit");
        let creates: i64 = db
            .query_row("SELECT count(*) FROM kubernetes_creates", [], |row| {
                row.get(0)
            })
            .expect("schema8 CREATE projection exists");
        assert_eq!(creates, 0, "migration starts with an empty projection");
    }
    drop(store);

    let saved = Store::open_existing_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 10_000)
        .expect("reopen preserved schema7 backup");
    let saved_integrity = saved.integrity().expect("validate schema7 backup");
    assert!(saved_integrity.valid);
    assert_eq!(saved.supervision_state().unwrap().schema_version, 7);
    assert_eq!(saved_integrity.event_count, before.event_count);
    assert_eq!(saved_integrity.head_hash, before.head_hash);
    drop(saved);

    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000)
            .expect("reopen migrated schema8 without activation");
    assert_eq!(reopened.supervision_state().unwrap().schema_version, 8);
    assert!(!reopened.supervision_state().unwrap().active);
    let reopened_integrity = reopened
        .integrity()
        .expect("validate reopened schema8 audit");
    assert!(reopened_integrity.valid);
    assert_eq!(reopened_integrity.event_count, before.event_count + 1);
    assert_eq!(reopened_integrity.head_hash, migrated_integrity.head_hash);
}

#[test]
fn schema8_rejects_a_rehashed_genesis_for_a_different_tenant() {
    let (_directory, path, store) = migrated_schema8();
    drop(store);
    rewrite_kubernetes_genesis(&path, |event| {
        event["data"]["tenant_id"] = json!("tenant-b");
    });

    assert_corrupt_schema8_is_diagnostic_only(&path, "genesis bound to a different tenant");
}

#[test]
fn schema8_rejects_a_missing_kubernetes_genesis() {
    let (_directory, path, store) = migrated_schema8();
    drop(store);
    rewrite_kubernetes_genesis(&path, |event| {
        event["kind"] = json!("fixture.kubernetes.create.genesis");
    });

    assert_corrupt_schema8_is_diagnostic_only(&path, "missing Kubernetes genesis");
}

#[test]
fn schema8_rejects_a_duplicate_kubernetes_genesis() {
    let (_directory, path, store) = migrated_schema8();
    drop(store);
    let db = Connection::open(&path).expect("open closed fixture for duplicate genesis");
    let (sequence, previous_hash, payload): (i64, String, String) = db
        .query_row(
            "SELECT seq,hash,(SELECT payload FROM events WHERE kind=?1) FROM events ORDER BY seq DESC LIMIT 1",
            [KUBERNETES_GENESIS_KIND],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read real schema8 head and genesis");
    let mut event: Value = serde_json::from_str(&payload).expect("decode genesis payload");
    let id = uuid::Uuid::new_v4().to_string();
    event["id"] = json!(id);
    let received_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let payload = serde_json::to_string(&event).expect("encode duplicate genesis event");
    let next_sequence = sequence + 1;
    let hash = super::digest(next_sequence, &received_at, &previous_hash, &payload);
    db.execute(
        "INSERT INTO events(seq,id,received_at,prev_hash,hash,payload,kind) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![next_sequence, id, received_at, previous_hash, hash, payload, KUBERNETES_GENESIS_KIND],
    )
    .expect("append a second correctly chained genesis row");
    db.execute(
        "UPDATE quota SET bytes=bytes+(SELECT length(payload) FROM events WHERE seq=?1) WHERE id=1",
        [next_sequence],
    )
    .expect("account for duplicate event bytes");
    drop(db);

    assert_corrupt_schema8_is_diagnostic_only(&path, "duplicate Kubernetes genesis");
}

#[test]
fn schema8_rejects_an_orphaned_create_projection_row() {
    let (_directory, path, store) = migrated_schema8();
    drop(store);
    let db = Connection::open(&path).expect("open closed fixture for orphan projection");
    let genesis_seq: i64 = db
        .query_row(
            "SELECT seq FROM events WHERE kind=?1",
            [KUBERNETES_GENESIS_KIND],
            |row| row.get(0),
        )
        .expect("read genesis sequence");
    db.execute(
        "INSERT INTO kubernetes_creates(create_id,session_id,intent_hash,state,create_epoch,revision,quota_held,cpu_millis,memory_bytes,snapshot_json,snapshot_hash,last_event_seq) VALUES(?1,?2,?3,'reserved',1,1,1,1000,1048576,'{}',?4,?5)",
        params![
            "11111111-1111-4111-8111-111111111111",
            "22222222-2222-4222-8222-222222222222",
            "a".repeat(64),
            "b".repeat(64),
            genesis_seq,
        ],
    )
    .expect("insert structurally valid but unaudited projection row");
    drop(db);

    assert_corrupt_schema8_is_diagnostic_only(&path, "orphaned CREATE projection row");
}
