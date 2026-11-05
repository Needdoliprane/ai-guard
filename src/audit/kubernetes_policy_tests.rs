//! Schema 9 policy marker migration; Kubernetes identity/claims here are test fixtures only.
use super::{
    AuditCapacityFailureV1, AuditCapacityPolicyV1, AuditCapacityV1, Event, EventQuery, Redactor,
    Store, kubernetes_admission_state_tests::Fixture,
};
use crate::control::{QuotaPolicyV1, QuotaUsageV1, kubernetes_admission::*};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use uuid::Uuid;

const POLICY_GENESIS_KIND: &str = "control.kubernetes.create.policy.genesis";
const CREATE_GENESIS_KIND: &str = "control.kubernetes.create.genesis";
const POLICY_PROTOCOL: &str = "kubernetes_held_create_policy_v1";
const MAX_EVENTS: u64 = 10_000;
const FUTURE: &str = "2100-01-01T00:00:00.000Z";

#[derive(Debug, Clone, PartialEq, Eq)]
struct CreateProjection {
    create_id: String,
    session_id: String,
    intent_hash: String,
    state: String,
    create_epoch: i64,
    revision: i64,
    quota_held: i64,
    cpu_millis: i64,
    memory_bytes: i64,
    snapshot_json: String,
    snapshot_hash: String,
    last_event_seq: i64,
}

type EventPrefix = Vec<(i64, String, String, String, String, String, String)>;

fn seeded_schema8_fixture() -> Fixture {
    let fixture = Fixture::new();
    let quota = QuotaPolicyV1::default();
    let authorization = fixture
        .store
        .authorize_held_create_for_test(&fixture.identity, &fixture.intent, &quota, FUTURE)
        .expect("authorize the fixture's real schema8 tenant binding");
    let (_, snapshot, replayed) = fixture
        .store
        .reserve_kubernetes_create(&fixture.identity, &authorization, &fixture.intent)
        .expect("reserve one real schema8 CREATE projection row");
    assert!(!replayed);
    assert_eq!(snapshot.state, KubernetesCreateStateV1::Reserved);
    assert_eq!(snapshot.revision, 1);
    assert_eq!(snapshot.create_epoch, 1);
    assert!(snapshot.quota_held);
    fixture
}

fn projection(store: &Store) -> CreateProjection {
    let db = store.db.lock().expect("lock real CREATE projection");
    db.query_row(
        "SELECT create_id,session_id,intent_hash,state,create_epoch,revision,quota_held,cpu_millis,memory_bytes,snapshot_json,snapshot_hash,last_event_seq FROM kubernetes_creates ORDER BY create_id",
        [],
        |row| {
            Ok(CreateProjection {
                create_id: row.get(0)?,
                session_id: row.get(1)?,
                intent_hash: row.get(2)?,
                state: row.get(3)?,
                create_epoch: row.get(4)?,
                revision: row.get(5)?,
                quota_held: row.get(6)?,
                cpu_millis: row.get(7)?,
                memory_bytes: row.get(8)?,
                snapshot_json: row.get(9)?,
                snapshot_hash: row.get(10)?,
                last_event_seq: row.get(11)?,
            })
        },
    )
    .expect("read one real schema8 CREATE projection row")
}

fn event_prefix(store: &Store, through_seq: i64) -> EventPrefix {
    let db = store.db.lock().expect("lock audit prefix");
    let mut statement = db
        .prepare("SELECT seq,id,received_at,prev_hash,hash,payload,kind FROM events WHERE seq<=?1 ORDER BY seq")
        .expect("prepare audit-prefix read");
    statement
        .query_map([through_seq], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })
        .expect("query audit prefix")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect audit prefix")
}

fn scalar_i64(store: &Store, sql: &str) -> i64 {
    store
        .db
        .lock()
        .expect("lock scalar audit read")
        .query_row(sql, [], |row| row.get(0))
        .expect("read scalar audit value")
}

fn audit_quota_bytes(store: &Store) -> i64 {
    scalar_i64(store, "SELECT bytes FROM quota WHERE id=1")
}

fn last_time_floor(store: &Store) -> String {
    store
        .db
        .lock()
        .expect("lock audit time floor read")
        .query_row(
            "SELECT max_received_at FROM audit_time_floors ORDER BY event_seq DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("read the last durable audit time floor")
}

fn assert_worker_quota_eq(actual: &QuotaUsageV1, expected: &QuotaUsageV1) {
    assert_eq!(actual.policy, expected.policy);
    assert_eq!(actual.held_workers, expected.held_workers);
    assert_eq!(actual.held_cpu_millis, expected.held_cpu_millis);
    assert_eq!(actual.held_memory_bytes, expected.held_memory_bytes);
}

fn marker_events(store: &Store) -> Vec<super::StoredEvent> {
    store
        .events(&EventQuery {
            kind: Some(POLICY_GENESIS_KIND.into()),
            ..Default::default()
        })
        .expect("read policy genesis markers")
}

fn quota_policy_with_ordinary_event_limit(allowed_total_events: u64) -> AuditCapacityPolicyV1 {
    assert!(allowed_total_events >= 4 && allowed_total_events < MAX_EVENTS);
    let bytes = super::capacity::PHYSICAL_BYTES;
    AuditCapacityPolicyV1 {
        version: 1,
        human_tenant: AuditCapacityV1 {
            events: 2,
            bytes: 2_048,
        },
        human_principal: AuditCapacityV1 {
            events: 1,
            bytes: 1_024,
        },
        control_reserve: AuditCapacityV1 {
            events: 1,
            bytes: 1_024,
        },
        identity_reserve: AuditCapacityV1 {
            events: MAX_EVENTS - allowed_total_events,
            bytes: bytes - 1_048_576,
        },
    }
}

fn v8_genesis(store: &Store) -> super::StoredEvent {
    let events = store
        .events(&EventQuery {
            kind: Some(CREATE_GENESIS_KIND.into()),
            ..Default::default()
        })
        .expect("read original schema8 genesis");
    assert_eq!(events.len(), 1);
    events.into_iter().next().unwrap()
}

#[test]
fn schema9_migration_preserves_schema8_backup_projection_and_adds_one_ordinary_marker() {
    let mut fixture = seeded_schema8_fixture();
    let before = fixture.store.integrity().expect("validate schema8 source");
    assert!(before.valid);
    let state8 = fixture
        .store
        .supervision_state()
        .expect("read schema8 supervision state");
    assert_eq!(state8.schema_version, 8);
    assert!(
        state8.active,
        "the real reservation requires an already active lease"
    );
    let namespace = fixture.intent.control_namespace_uuid.clone();
    let genesis8 = v8_genesis(&fixture.store);
    let row_before = projection(&fixture.store);
    let quota_before = fixture
        .store
        .worker_quota(&QuotaPolicyV1::default())
        .unwrap();
    assert_eq!(quota_before.held_workers, 1);
    let legacy_authorization = fixture
        .store
        .authorize_held_create_for_test(
            &fixture.identity,
            &fixture.intent,
            &QuotaPolicyV1::default(),
            FUTURE,
        )
        .expect("retain a real schema8 V1 authorization across migration");
    let prefix_before = event_prefix(&fixture.store, before.event_count as i64);
    let floor_count_before = scalar_i64(&fixture.store, "SELECT count(*) FROM audit_time_floors");
    let last_floor_before = last_time_floor(&fixture.store);
    let bytes_before = audit_quota_bytes(&fixture.store);

    // Leave exactly one event slot in the ordinary-write budget for the marker.
    fixture.store = fixture
        .store
        .with_capacity_policy(quota_policy_with_ordinary_event_limit(
            before.event_count + 1,
        ))
        .expect("select capacity with one ordinary marker slot remaining");

    let backup_path = fixture._directory.path().join("before-policy-v9.sqlite");
    let report = fixture
        .store
        .migrate_kubernetes_policy_v9(&backup_path)
        .expect("explicitly migrate schema8 to schema9");
    assert_eq!(report.from_version, 8);
    assert_eq!(report.to_version, 9);
    assert_eq!(report.genesis_seq, before.event_count as i64 + 1);
    assert_eq!(
        report.backup_path,
        backup_path.to_string_lossy().into_owned()
    );

    let markers = marker_events(&fixture.store);
    assert_eq!(markers.len(), 1);
    let marker = &markers[0];
    assert_eq!(marker.seq as i64, report.genesis_seq);
    assert_eq!(marker.hash, report.genesis_hash);
    assert_eq!(marker.event.kind, POLICY_GENESIS_KIND);
    assert_eq!(marker.event.source, "gateway");
    assert_eq!(marker.event.provenance, "observed");
    assert_eq!(marker.event.coverage, "observed");
    assert_eq!(
        marker.event.data,
        json!({
            "version": 1,
            "protocol": POLICY_PROTOCOL,
            "tenant_id": "tenant-a",
            "control_namespace_uuid": namespace.clone(),
            "create_genesis_seq": genesis8.seq as i64,
            "create_genesis_hash": genesis8.hash.clone(),
            "previous_seq": before.event_count as i64,
            "previous_hash": before.head_hash.clone(),
        })
    );
    assert!(
        super::kubernetes_admission::workload_audit_identity(&marker.event)
            .unwrap()
            .is_none(),
        "policy genesis consumes ordinary, not broker-workload, budget"
    );

    let marker_payload_len = fixture
        .store
        .db
        .lock()
        .unwrap()
        .query_row::<String, _, _>(
            "SELECT payload FROM events WHERE seq=?1",
            [report.genesis_seq],
            |row| row.get(0),
        )
        .unwrap()
        .len() as i64;
    assert_eq!(
        audit_quota_bytes(&fixture.store),
        bytes_before + marker_payload_len
    );
    assert_eq!(
        scalar_i64(&fixture.store, "SELECT count(*) FROM audit_time_floors"),
        floor_count_before + 1,
        "the ordinary marker has exactly one durable time floor"
    );
    assert_eq!(
        scalar_i64(
            &fixture.store,
            "SELECT count(*) FROM audit_time_floors WHERE event_seq=(SELECT max(seq) FROM events)"
        ),
        1
    );
    let marker_floor: String = fixture
        .store
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT max_received_at FROM audit_time_floors WHERE event_seq=?1",
            [report.genesis_seq],
            |row| row.get(0),
        )
        .expect("read the policy marker's exact time floor");
    let expected_floor = crate::broker_registry::parse_time(&last_floor_before)
        .unwrap()
        .max(crate::broker_registry::parse_time(&marker.received_at).unwrap())
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    assert_eq!(marker_floor, expected_floor);

    let after = fixture.store.integrity().expect("validate schema9 source");
    assert!(after.valid);
    assert_eq!(after.event_count, before.event_count + 1);
    assert_eq!(
        event_prefix(&fixture.store, before.event_count as i64),
        prefix_before
    );
    assert_eq!(projection(&fixture.store), row_before);
    assert_worker_quota_eq(
        &fixture
            .store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap(),
        &quota_before,
    );
    let state9 = fixture
        .store
        .supervision_state()
        .expect("read schema9 supervision state");
    assert_eq!(state9.schema_version, 9);
    assert_eq!(
        state9.control_namespace_uuid.as_deref(),
        Some(namespace.as_str())
    );
    assert_eq!(
        state9.active, state8.active,
        "migration preserves the live lease state"
    );

    let head_after_migration = fixture.store.head().unwrap();
    let prefix_after_migration = event_prefix(&fixture.store, head_after_migration);
    let projection_after_migration = projection(&fixture.store);
    let quota_after_migration = fixture
        .store
        .worker_quota(&QuotaPolicyV1::default())
        .unwrap();
    let old_writer_v1_error = fixture
        .store
        .reserve_kubernetes_create(&fixture.identity, &legacy_authorization, &fixture.intent)
        .err()
        .expect("schema9 must reject a schema8 V1 writer capability");
    assert_eq!(
        old_writer_v1_error.downcast_ref::<KubernetesCreateFailureV1>(),
        Some(&KubernetesCreateFailureV1::MigrationRequired)
    );
    assert!(fixture.store.healthy());
    assert_eq!(fixture.store.head().unwrap(), head_after_migration);
    assert_eq!(
        event_prefix(&fixture.store, head_after_migration),
        prefix_after_migration
    );
    assert_eq!(projection(&fixture.store), projection_after_migration);
    assert_worker_quota_eq(
        &fixture
            .store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap(),
        &quota_after_migration,
    );
    assert_eq!(marker_events(&fixture.store).len(), 1);

    let backup = Store::open_existing_for_tenant(
        &backup_path,
        "tenant-a",
        Redactor::new(vec![]),
        MAX_EVENTS,
    )
    .expect("open exact schema8 migration backup");
    let backup_integrity = backup.integrity().expect("validate schema8 backup");
    assert!(backup_integrity.valid);
    assert_eq!(backup_integrity.event_count, before.event_count);
    assert_eq!(backup_integrity.head_hash, before.head_hash);
    assert_eq!(backup.supervision_state().unwrap().schema_version, 8);
    assert_eq!(
        event_prefix(&backup, before.event_count as i64),
        prefix_before
    );
    assert_eq!(projection(&backup), row_before);
    assert_worker_quota_eq(
        &backup.worker_quota(&QuotaPolicyV1::default()).unwrap(),
        &quota_before,
    );
    assert_eq!(audit_quota_bytes(&backup), bytes_before);
    drop(backup);

    let database_path = fixture._directory.path().join("tenant.sqlite");
    drop(fixture.store);
    let reopened = Store::open_existing_for_tenant(
        &database_path,
        "tenant-a",
        Redactor::new(vec![]),
        MAX_EVENTS,
    )
    .expect("reopen schema9 without restoring runtime activation");
    assert!(reopened.healthy());
    let reopened_state = reopened.supervision_state().unwrap();
    assert_eq!(reopened_state.schema_version, 9);
    assert!(!reopened_state.active);
    assert_eq!(
        reopened_state.control_namespace_uuid.as_deref(),
        Some(namespace.as_str())
    );
    assert_eq!(reopened.integrity().unwrap().head_hash, after.head_hash);
    assert_eq!(projection(&reopened), row_before);
    assert_worker_quota_eq(
        &reopened.worker_quota(&QuotaPolicyV1::default()).unwrap(),
        &quota_before,
    );
    assert_eq!(marker_events(&reopened).len(), 1);
}

#[test]
fn schema9_migration_refuses_existing_backup_and_ordinary_budget_exhaustion_without_source_mutation()
 {
    let valid = Fixture::new();
    let valid_backup = valid._directory.path().join("valid-v9-backup.sqlite");
    let valid_report = valid
        .store
        .migrate_kubernetes_policy_v9(&valid_backup)
        .expect("positive schema8-to-9 control proves the migration path is live");
    assert_eq!(valid_report.to_version, 9);
    assert_eq!(marker_events(&valid.store).len(), 1);

    let fixture = seeded_schema8_fixture();
    let before = fixture.store.integrity().unwrap();
    let row_before = projection(&fixture.store);
    let quota_before = fixture
        .store
        .worker_quota(&QuotaPolicyV1::default())
        .unwrap();
    let prefix_before = event_prefix(&fixture.store, before.event_count as i64);
    let backup_path = fixture._directory.path().join("already-exists.sqlite");
    let sentinel = b"existing operator backup bytes";
    fs::write(&backup_path, sentinel).unwrap();

    assert!(
        fixture
            .store
            .migrate_kubernetes_policy_v9(&backup_path)
            .is_err()
    );
    assert_eq!(fs::read(&backup_path).unwrap(), sentinel);
    assert!(fixture.store.healthy());
    assert!(fixture.store.integrity().unwrap().valid);
    assert_eq!(fixture.store.head().unwrap() as u64, before.event_count);
    assert_eq!(
        event_prefix(&fixture.store, before.event_count as i64),
        prefix_before
    );
    assert_eq!(projection(&fixture.store), row_before);
    assert_worker_quota_eq(
        &fixture
            .store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap(),
        &quota_before,
    );
    assert_eq!(marker_events(&fixture.store).len(), 0);
    assert_eq!(fixture.store.supervision_state().unwrap().schema_version, 8);

    let mut capacity_fixture = seeded_schema8_fixture();
    let capacity_before = capacity_fixture.store.integrity().unwrap();
    let capacity_row = projection(&capacity_fixture.store);
    let capacity_usage = capacity_fixture
        .store
        .worker_quota(&QuotaPolicyV1::default())
        .unwrap();
    let capacity_prefix = event_prefix(&capacity_fixture.store, capacity_before.event_count as i64);
    capacity_fixture.store = capacity_fixture
        .store
        .with_capacity_policy(quota_policy_with_ordinary_event_limit(
            capacity_before.event_count,
        ))
        .expect("select a valid policy with no ordinary marker event left");
    let capacity_backup = capacity_fixture
        ._directory
        .path()
        .join("capacity-backup.sqlite");

    let failure = capacity_fixture
        .store
        .migrate_kubernetes_policy_v9(&capacity_backup)
        .err()
        .expect("ordinary audit capacity must refuse the schema9 marker");
    assert_eq!(
        failure.downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted),
        "the marker is charged to the ordinary-write budget"
    );
    assert!(capacity_fixture.store.healthy());
    assert!(capacity_fixture.store.integrity().unwrap().valid);
    assert_eq!(
        capacity_fixture.store.head().unwrap() as u64,
        capacity_before.event_count
    );
    assert_eq!(
        event_prefix(&capacity_fixture.store, capacity_before.event_count as i64),
        capacity_prefix
    );
    assert_eq!(projection(&capacity_fixture.store), capacity_row);
    assert_worker_quota_eq(
        &capacity_fixture
            .store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap(),
        &capacity_usage,
    );
    assert_eq!(marker_events(&capacity_fixture.store).len(), 0);
    assert_eq!(
        capacity_fixture
            .store
            .supervision_state()
            .unwrap()
            .schema_version,
        8
    );
}

fn migrate_test_database_to_schema9() -> (tempfile::TempDir, std::path::PathBuf) {
    let fixture = Fixture::new();
    let database_path = fixture._directory.path().join("tenant.sqlite");
    let backup_path = fixture._directory.path().join("before-policy-v9.sqlite");
    fixture
        .store
        .migrate_kubernetes_policy_v9(&backup_path)
        .expect("create a real schema9 marker before corruption fixture mutation");
    drop(fixture.store);
    (fixture._directory, database_path)
}

fn rewrite_policy_marker(path: &Path, mutate: impl FnOnce(&mut Value)) {
    let db = Connection::open(path).expect("open closed schema9 fixture for deliberate corruption");
    let (sequence, received_at, previous_hash, old_payload): (i64, String, String, String) = db
        .query_row(
            "SELECT seq,received_at,prev_hash,payload FROM events WHERE kind=?1",
            [POLICY_GENESIS_KIND],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("read real schema9 policy marker");
    let mut event: Value = serde_json::from_str(&old_payload).expect("decode typed marker event");
    mutate(&mut event);
    let kind = event["kind"]
        .as_str()
        .expect("marker event kind remains a string")
        .to_owned();
    let payload = serde_json::to_string(&event).expect("encode deliberately changed marker");
    let hash = super::digest(sequence, &received_at, &previous_hash, &payload);
    let byte_delta =
        i64::try_from(payload.len()).unwrap() - i64::try_from(old_payload.len()).unwrap();
    db.execute(
        "UPDATE events SET kind=?1,payload=?2,hash=?3 WHERE seq=?4",
        params![kind, payload, hash, sequence],
    )
    .expect("keep the deliberately changed event on the valid audit hash chain");
    db.execute("UPDATE quota SET bytes=bytes+?1 WHERE id=1", [byte_delta])
        .expect("keep audit byte quota exact after rehash");
}

fn policy_marker_data(path: &Path) -> Value {
    let db = Connection::open(path).expect("open schema9 fixture to read marker references");
    let payload: String = db
        .query_row(
            "SELECT payload FROM events WHERE kind=?1",
            [POLICY_GENESIS_KIND],
            |row| row.get(0),
        )
        .expect("read schema9 policy marker");
    serde_json::from_str::<Value>(&payload).expect("decode schema9 policy marker")["data"].clone()
}

fn run_schema8_writer(data_path: &Path, backup_path: &Path) -> Output {
    let writer = env::var_os("AI_GUARD_SCHEMA8_WRITER")
        .expect("the explicitly configured schema8 writer is required for this ignored test");
    let home = env::var_os("HOME").expect("QA HOME is explicitly configured");
    let temp = env::var_os("TMPDIR").expect("QA TMPDIR is explicitly configured");
    Command::new(writer)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("TMPDIR", temp)
        .arg("migrate-control")
        .arg("--data")
        .arg(data_path)
        .arg("--tenant-id")
        .arg("tenant-a")
        .arg("--backup")
        .arg(backup_path)
        .arg("--target-version")
        .arg("8")
        .arg("--confirm-control-stopped")
        .arg("--max-events")
        .arg(MAX_EVENTS.to_string())
        .output()
        .expect("invoke the prebuilt schema8 CLI in the isolated QA container")
}

fn private_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("create private schema compatibility fixture")
}

fn schema7_writer_source() -> (tempfile::TempDir, PathBuf, Vec<u8>) {
    let fixture = Fixture::new();
    let path = fixture._directory.path().join("tenant.before8");
    assert!(
        path.is_file(),
        "the real 7-to-8 migration retained its schema7 backup"
    );
    let schema7 =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), MAX_EVENTS)
            .expect("open the genuine schema7 backup produced by the fixture");
    assert!(schema7.integrity().unwrap().valid);
    assert_eq!(schema7.supervision_state().unwrap().schema_version, 7);
    drop(schema7);
    let bytes = fs::read(&path).expect("read the closed schema7 backup bytes");
    (fixture._directory, path, bytes)
}

#[test]
#[ignore = "requires a schema8 writer executable in AI_GUARD_SCHEMA8_WRITER"]
fn schema8_writer_rejects_schema9_before_mutating_it() {
    let (_schema7_dir, schema7_source, schema7_bytes) = schema7_writer_source();
    let positive_dir = private_tempdir();
    let positive_data = positive_dir.path().join("schema7-copy.sqlite");
    let positive_backup = positive_dir.path().join("schema7-copy-before8.sqlite");
    fs::copy(&schema7_source, &positive_data).expect("copy schema7 to a disposable path");

    let positive = run_schema8_writer(&positive_data, &positive_backup);
    assert!(
        positive.status.success(),
        "the same CLI accepts a genuine schema7 migration to8"
    );
    assert_eq!(
        fs::read(&schema7_source).unwrap(),
        schema7_bytes,
        "the source fixture's original schema7 backup was not passed as writable data"
    );
    let migrated = Store::open_existing_for_tenant(
        &positive_data,
        "tenant-a",
        Redactor::new(vec![]),
        MAX_EVENTS,
    )
    .expect("open the CLI's schema8 result");
    assert!(migrated.healthy());
    assert!(migrated.integrity().unwrap().valid);
    assert_eq!(migrated.supervision_state().unwrap().schema_version, 8);
    drop(migrated);
    let preserved_schema7 = Store::open_existing_for_tenant(
        &positive_backup,
        "tenant-a",
        Redactor::new(vec![]),
        MAX_EVENTS,
    )
    .expect("open the CLI's schema7 backup");
    assert!(preserved_schema7.integrity().unwrap().valid);
    assert_eq!(
        preserved_schema7
            .supervision_state()
            .unwrap()
            .schema_version,
        7
    );
    drop(preserved_schema7);

    let (_schema9_dir, schema9_path) = migrate_test_database_to_schema9();
    let schema9 = Store::open_existing_for_tenant(
        &schema9_path,
        "tenant-a",
        Redactor::new(vec![]),
        MAX_EVENTS,
    )
    .expect("open schema9 before exercising the old writer");
    assert!(schema9.healthy());
    assert!(schema9.integrity().unwrap().valid);
    assert_eq!(schema9.supervision_state().unwrap().schema_version, 9);
    let head_before = schema9.head().unwrap();
    assert_eq!(marker_events(&schema9).len(), 1);
    drop(schema9);
    let bytes_before = fs::read(&schema9_path).expect("read the closed schema9 database");

    let negative_dir = private_tempdir();
    let negative_backup = negative_dir.path().join("must-not-be-created.sqlite");
    let negative = run_schema8_writer(&schema9_path, &negative_backup);
    assert!(
        !negative.status.success(),
        "schema8 writer accepted schema9"
    );
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&negative.stdout),
        String::from_utf8_lossy(&negative.stderr)
    );
    let diagnostic: String = diagnostic.chars().take(240).collect();
    assert!(
        diagnostic.contains("unsupported audit schema version"),
        "old writer refused schema9 for an unexpected reason: {diagnostic}"
    );
    assert!(
        !negative_backup.exists(),
        "refusal must not create a backup"
    );
    assert_eq!(
        fs::read(&schema9_path).unwrap(),
        bytes_before,
        "the old schema8 writer must not change the schema9 database"
    );
    let reopened = Store::open_existing_for_tenant(
        &schema9_path,
        "tenant-a",
        Redactor::new(vec![]),
        MAX_EVENTS,
    )
    .expect("reopen schema9 after the old writer refused it");
    assert!(reopened.healthy());
    assert!(reopened.integrity().unwrap().valid);
    assert_eq!(reopened.head().unwrap(), head_before);
    assert_eq!(reopened.supervision_state().unwrap().schema_version, 9);
    assert_eq!(marker_events(&reopened).len(), 1);
}

fn append_duplicate_policy_marker(path: &Path) {
    let db = Connection::open(path).expect("open closed schema9 fixture for duplicate marker");
    let (last_sequence, last_hash): (i64, String) = db
        .query_row(
            "SELECT seq,hash FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read schema9 audit head");
    let (marker_payload, previous_floor): (String, String) = db
        .query_row(
            "SELECT e.payload,f.max_received_at FROM events e JOIN audit_time_floors f ON f.event_seq=e.seq WHERE e.kind=?1",
            [POLICY_GENESIS_KIND],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read original marker and durable floor");
    let mut duplicate: Value = serde_json::from_str(&marker_payload).unwrap();
    let sequence = last_sequence + 1;
    let received_at = crate::broker_registry::parse_time(&previous_floor)
        .unwrap()
        .max(chrono::Utc::now())
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    duplicate["id"] = json!(Uuid::new_v4().to_string());
    duplicate["timestamp"] = json!(received_at.clone());
    duplicate["data"]["previous_seq"] = json!(last_sequence);
    duplicate["data"]["previous_hash"] = json!(last_hash);
    let payload = serde_json::to_string(&duplicate).unwrap();
    let hash = super::digest(sequence, &received_at, &last_hash, &payload);
    db.execute(
        "INSERT INTO events(seq,id,received_at,prev_hash,hash,payload,kind) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            sequence,
            duplicate["id"].as_str().unwrap(),
            received_at.clone(),
            last_hash,
            hash,
            payload,
            POLICY_GENESIS_KIND,
        ],
    )
    .expect("append a second individually well-linked policy marker");
    db.execute(
        "UPDATE quota SET bytes=bytes+(SELECT length(payload) FROM events WHERE seq=?1) WHERE id=1",
        [sequence],
    )
    .expect("account for the duplicate marker bytes");
    super::workload::advance_time_floor(&db, sequence, &received_at)
        .expect("record the duplicate event's exact time floor");
}

fn assert_corrupt_policy_schema_is_diagnostic_only(path: &Path, case: &str) {
    let reopened =
        Store::open_existing_for_tenant(path, "tenant-a", Redactor::new(vec![]), MAX_EVENTS)
            .expect("corrupt schema must return a diagnostic handle");
    assert!(!reopened.healthy(), "marked {case} healthy on reopen");
    let integrity = reopened.integrity();
    assert!(
        matches!(
            integrity,
            Err(_) | Ok(super::Integrity { valid: false, .. })
        ),
        "schema9 accepted {case}"
    );
    assert!(reopened.supervision_state().is_err(), "enabled {case}");
    let before = scalar_i64(&reopened, "SELECT count(*) FROM events");
    assert!(
        reopened
            .append(Event::new("fixture.policy_integrity_probe"))
            .is_err(),
        "accepted an event append for {case}"
    );
    assert_eq!(
        scalar_i64(&reopened, "SELECT count(*) FROM events"),
        before,
        "appended a probe event for {case}"
    );
    assert!(!reopened.healthy(), "restored health for {case}");
}

#[test]
fn schema9_requires_one_well_linked_policy_marker_and_rejects_it_on_schema8() {
    // Each corruption begins with a successful real schema8->9 migration and
    // then changes only the closed marker while preserving the event hash chain.
    let (_missing_dir, missing_path) = migrate_test_database_to_schema9();
    rewrite_policy_marker(&missing_path, |event| {
        event["kind"] = json!("fixture.removed.policy.genesis");
    });
    assert_corrupt_policy_schema_is_diagnostic_only(&missing_path, "missing schema9 policy marker");

    let (_malformed_dir, malformed_path) = migrate_test_database_to_schema9();
    rewrite_policy_marker(&malformed_path, |event| {
        event["data"]["create_genesis_hash"] = json!("0".repeat(64));
    });
    assert_corrupt_policy_schema_is_diagnostic_only(&malformed_path, "malformed schema8 reference");

    let (_misbound_genesis_dir, misbound_genesis_path) = migrate_test_database_to_schema9();
    let misbound_genesis_data = policy_marker_data(&misbound_genesis_path);
    let previous_seq = misbound_genesis_data["previous_seq"].as_i64().unwrap();
    let previous_hash = misbound_genesis_data["previous_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    let genesis_seq = misbound_genesis_data["create_genesis_seq"]
        .as_i64()
        .unwrap();
    let genesis_hash = misbound_genesis_data["create_genesis_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(
        (genesis_seq, genesis_hash.as_str()),
        (previous_seq, previous_hash.as_str())
    );
    rewrite_policy_marker(&misbound_genesis_path, |event| {
        event["data"]["create_genesis_seq"] = json!(previous_seq);
        event["data"]["create_genesis_hash"] = json!(previous_hash);
    });
    assert_corrupt_policy_schema_is_diagnostic_only(
        &misbound_genesis_path,
        "schema8 genesis reference replaced by the valid previous head pair",
    );

    let (_old_previous_dir, old_previous_path) = migrate_test_database_to_schema9();
    let old_previous_data = policy_marker_data(&old_previous_path);
    let previous_seq = old_previous_data["previous_seq"].as_i64().unwrap();
    let previous_hash = old_previous_data["previous_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    let genesis_seq = old_previous_data["create_genesis_seq"].as_i64().unwrap();
    let genesis_hash = old_previous_data["create_genesis_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(
        (genesis_seq, genesis_hash.as_str()),
        (previous_seq, previous_hash.as_str())
    );
    rewrite_policy_marker(&old_previous_path, |event| {
        event["data"]["previous_seq"] = json!(genesis_seq);
        event["data"]["previous_hash"] = json!(genesis_hash);
    });
    assert_corrupt_policy_schema_is_diagnostic_only(
        &old_previous_path,
        "previous pair replaced by the valid schema8 genesis pair",
    );

    let (_future_timestamp_dir, future_timestamp_path) = migrate_test_database_to_schema9();
    rewrite_policy_marker(&future_timestamp_path, |event| {
        event["timestamp"] = json!("2100-01-01T00:00:00.000Z");
    });
    assert_corrupt_policy_schema_is_diagnostic_only(
        &future_timestamp_path,
        "future observed policy marker timestamp",
    );

    let (_duplicate_dir, duplicate_path) = migrate_test_database_to_schema9();
    append_duplicate_policy_marker(&duplicate_path);
    assert_corrupt_policy_schema_is_diagnostic_only(&duplicate_path, "duplicate policy marker");

    let (_schema8_dir, schema8_path) = migrate_test_database_to_schema9();
    let db = Connection::open(&schema8_path).expect("open closed schema9 fixture for old version");
    db.pragma_update(None, "user_version", 8)
        .expect("restore the old declared schema version without changing the marker");
    drop(db);
    assert_corrupt_policy_schema_is_diagnostic_only(&schema8_path, "policy marker in schema8");
}
