//! Regressions over the exact production SQL and expiry predicate. These SQLite
//! projection fixtures do not authenticate a worker or qualify a runtime.
use super::*;
use rusqlite::{StatementStatus, types::Value as SqlValue};

const A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const ATTEMPT_A: &str = "aaaaaaaa-0000-4000-8000-aaaaaaaaaaaa";
const ATTEMPT_B: &str = "bbbbbbbb-0000-4000-8000-bbbbbbbbbbbb";
const GRANT: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const OLD: &str = "2026-11-03T10:00:00.000Z";
const RECENT: &str = "2026-11-03T11:00:00.000Z";
const NOW: &str = "2026-11-03T11:00:05.000Z";

fn endpoint(worker: &str, attempt: &str) -> RelayEndpointV1 {
    RelayEndpointV1 {
        worker_id: worker.into(),
        attempt_id: attempt.into(),
        generation: 1,
    }
}
fn schema() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(super::super::EVENTS_SCHEMA).unwrap();
    initialize(&db).unwrap();
    install_time_floor(&db);
    db.pragma_update(None, "user_version", 6).unwrap();
    db
}
pub(super) fn install_time_floor(db: &Connection) {
    let (_, _, sql) = super::super::workload::SCHEMA
        .iter()
        .find(|(_, name, _)| *name == "audit_time_floors")
        .expect("clock projection is part of schema6");
    db.execute_batch(sql).unwrap();
}
fn event(db: &Connection, seq: i64, received: &str) {
    let previous = audit_head(db).unwrap().1;
    let mut event = Event::new("fixture.query_projection");
    event.timestamp = received.into();
    let payload = serde_json::to_string(&event).unwrap();
    let hash = super::super::digest(seq, received, &previous, &payload);
    db.execute("INSERT INTO events(seq,id,received_at,prev_hash,hash,payload,kind) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![seq,event.id,received,previous,hash,payload,event.kind]).unwrap();
    super::super::workload::advance_time_floor(db, seq, received).unwrap();
}
fn insert_message(
    db: &Connection,
    seq: i64,
    received: &str,
    retained: bool,
    destination: &RelayEndpointV1,
) {
    event(db, seq, received);
    let metadata = RelayMessageMetadataV1 {
        version: 1,
        message_id: uuid::Uuid::new_v4().to_string(),
        grant_id: GRANT.into(),
        source: endpoint(B, ATTEMPT_B),
        destination: destination.clone(),
        channel: "tasks".into(),
        created_at: received.into(),
        expires_at: "2026-11-03T12:00:00.000Z".into(),
        body_bytes: 1,
        body_sha256: digest(b"x"),
        acknowledged: false,
        body_pruned: !retained,
    };
    let body = retained.then_some("x");
    // The RED prefix intentionally has the old projection. Select its actual
    // shape so the oracle reports VM/row work, not a parameter/schema error.
    let indexed: bool = db
        .prepare("PRAGMA table_info(relay_messages)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|r| r.unwrap())
        .any(|name| name == "destination_worker_id");
    if indexed {
        db.execute("INSERT INTO relay_messages(message_id,grant_id,metadata_json,body,accepted_event_seq,event_seq,destination_worker_id,destination_attempt_id,destination_generation,body_bytes,acknowledged,expires_at,received_at) VALUES(?1,?2,?3,?4,?5,?5,?6,?7,?8,?9,0,?10,?11)",params![metadata.message_id,GRANT,serde_json::to_string(&metadata).unwrap(),body,seq,destination.worker_id,destination.attempt_id,1_i64,1_i64,metadata.expires_at,received]).unwrap();
    } else {
        db.execute("INSERT INTO relay_messages(message_id,grant_id,metadata_json,body,accepted_event_seq,event_seq) VALUES(?1,?2,?3,?4,?5,?5)",params![metadata.message_id,GRANT,serde_json::to_string(&metadata).unwrap(),body,seq]).unwrap();
    }
}
fn populated(tombstones: i64, other_retained: i64, recent_retained: i64) -> Connection {
    let mut db = schema();
    let tx = db.transaction().unwrap();
    event(&tx, 1, OLD);
    tx.execute("INSERT INTO relay_grants VALUES(?1,'{}',1)", [GRANT])
        .unwrap();
    let mut seq = 1;
    for _ in 0..tombstones {
        seq += 1;
        insert_message(&tx, seq, OLD, false, &endpoint(A, ATTEMPT_A));
    }
    for _ in 0..other_retained {
        seq += 1;
        insert_message(&tx, seq, RECENT, true, &endpoint(B, ATTEMPT_B));
    }
    for _ in 0..recent_retained {
        seq += 1;
        insert_message(&tx, seq, RECENT, true, &endpoint(A, ATTEMPT_A));
    }
    tx.commit().unwrap();
    db
}
fn plan(db: &Connection, sql: &str, params: &[SqlValue]) -> String {
    db.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap()
        .query_map(rusqlite::params_from_iter(params), |r| {
            r.get::<_, String>(3)
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}
fn aggregate(db: &Connection) -> (i64, i64, i32, i32) {
    let mut stmt = db.prepare(RETAINED_USAGE_SQL).unwrap();
    let (count, bytes) = stmt.query_row([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    (
        count,
        bytes,
        stmt.get_status(StatementStatus::VmStep),
        stmt.get_status(StatementStatus::FullscanStep),
    )
}
fn rate(db: &Connection) -> (i64, i32) {
    let mut stmt = db.prepare(RECENT_GRANT_SQL).unwrap();
    let mut args = vec![
        SqlValue::Text(GRANT.into()),
        SqlValue::Text("2026-11-03T10:59:05.000Z".into()),
    ];
    if stmt.parameter_count() == 3 {
        args.push(SqlValue::Text(NOW.into()));
    }
    let count = stmt
        .query_row(rusqlite::params_from_iter(&args), |r| r.get(0))
        .unwrap();
    (count, stmt.get_status(StatementStatus::VmStep))
}
fn inbox_args(db: &Connection) -> Vec<SqlValue> {
    if db.prepare(INBOX_SQL).unwrap().parameter_count() == 1 {
        vec![SqlValue::Integer(0)]
    } else {
        vec![
            SqlValue::Text(A.into()),
            SqlValue::Text(ATTEMPT_A.into()),
            SqlValue::Integer(1),
            SqlValue::Integer(0),
            SqlValue::Text(NOW.into()),
            SqlValue::Integer(i64::try_from(MAX_RETAINED_MESSAGES).unwrap()),
        ]
    }
}
fn inbox(db: &Connection) -> (usize, i32, i32) {
    let mut stmt = db.prepare(INBOX_SQL).unwrap();
    let args = inbox_args(db);
    let mut rows = stmt.query(rusqlite::params_from_iter(&args)).unwrap();
    let mut count = 0;
    while rows.next().unwrap().is_some() {
        count += 1;
    }
    drop(rows);
    (
        count,
        stmt.get_status(StatementStatus::VmStep),
        stmt.get_status(StatementStatus::FullscanStep),
    )
}

#[test]
fn retained_quota_and_rate_work_do_not_grow_with_pruned_history() {
    let small = populated(1024, 0, 2);
    let large = populated(2048, 0, 2);
    let a = aggregate(&small);
    let b = aggregate(&large);
    eprintln!("relay retained SQL n=1024 {a:?}; n=2048 {b:?} (count,bytes,VM,fullscan)");
    assert_eq!((a.0, a.1), (2, 2));
    assert_eq!((b.0, b.1), (2, 2));
    assert!(
        b.2 <= a.2 + 80,
        "retained usage VM work grew with tombstones: {a:?} -> {b:?}"
    );
    assert!(
        a.3 <= 8 && b.3 <= 8,
        "retained usage visited pruned rows: {a:?} -> {b:?}"
    );
    assert!(plan(&large, RETAINED_USAGE_SQL, &[]).contains("relay_messages_retained"));
    let a = rate(&small);
    let b = rate(&large);
    eprintln!("relay rate SQL n=1024 {a:?}; n=2048 {b:?} (count,VM)");
    assert_eq!(a.0, 2);
    assert_eq!(b.0, 2);
    assert!(
        b.1 <= a.1 + 80,
        "grant rate VM work grew with old history: {a:?} -> {b:?}"
    );
    let mut args = vec![
        SqlValue::Text(GRANT.into()),
        SqlValue::Text("2026-11-03T10:59:05.000Z".into()),
    ];
    if large.prepare(RECENT_GRANT_SQL).unwrap().parameter_count() == 3 {
        args.push(SqlValue::Text(NOW.into()));
    }
    assert!(plan(&large, RECENT_GRANT_SQL, &args).contains("relay_messages_rate"));
}

fn snapshot(db: &Connection) -> (usize, i32, i32) {
    let mut stmt = db.prepare(retention::RETAINED_SNAPSHOT_SQL).unwrap();
    let mut rows = stmt.query([]).unwrap();
    let mut count = 0;
    while rows.next().unwrap().is_some() {
        count += 1;
    }
    drop(rows);
    (
        count,
        stmt.get_status(StatementStatus::VmStep),
        stmt.get_status(StatementStatus::FullscanStep),
    )
}

#[test]
fn retained_snapshot_work_does_not_grow_with_pruned_history() {
    let small = populated(1024, 0, 2);
    let large = populated(2048, 0, 2);
    let a = snapshot(&small);
    let b = snapshot(&large);
    eprintln!("relay retained snapshot SQL n=1024 {a:?}; n=2048 {b:?} (rows,VM,fullscan)");
    assert_eq!((a.0, b.0), (2, 2));
    assert!(
        b.1 <= a.1 + 80,
        "retained snapshot VM work grew with tombstones: {a:?} -> {b:?}"
    );
    // Scan steps follow the two retained rows, never the tombstones.
    assert!(
        a.2 <= 8 && b.2 == a.2,
        "retained snapshot visited pruned rows: {a:?} -> {b:?}"
    );
    assert!(
        plan(&large, retention::RETAINED_SNAPSHOT_SQL, &[]).contains("relay_messages_retained")
    );
    // The typed admission pass reads exactly those rows.
    let retained = retention::Retained::load(&large).unwrap();
    let now = Utc::now();
    assert_eq!(
        retained.usage(now, |_| true).retained,
        RelayCapacityV1 {
            messages: 2,
            bytes: 2
        }
    );
    assert_eq!(
        retained
            .usage(now, |message| message.source.worker_id == A)
            .retained,
        RelayCapacityV1::default(),
        "fixture bodies are sent by B"
    );
}

#[test]
fn inbox_sql_never_reads_another_destinations_body_or_pruned_history() {
    let small = populated(1024, 256, 0);
    let large = populated(2048, 512, 0);
    let a = inbox(&small);
    let b = inbox(&large);
    eprintln!(
        "relay empty inbox SQL n=1024/other256 {a:?}; n=2048/other512 {b:?} (rows,VM,fullscan)"
    );
    assert_eq!(a.0, 0, "SQL read content for a different destination");
    assert_eq!(b.0, 0, "SQL read content for a different destination");
    assert!(
        b.1 <= a.1 + 80,
        "empty inbox VM work grew with unrelated content: {a:?} -> {b:?}"
    );
    assert_eq!((a.2, b.2), (0, 0));
    assert!(plan(&large, INBOX_SQL, &inbox_args(&large)).contains("relay_messages_inbox"));
}

#[test]
fn durable_later_witness_cannot_be_masked_to_reactivate_an_expired_grant() {
    let db = schema();
    event(&db, 1, "2026-11-03T10:00:03.000Z");
    event(&db, 2, "2026-11-03T10:00:00.000Z");
    let grant = Grant {
        grant_id: GRANT.into(),
        source: endpoint(B, ATTEMPT_B),
        destination: endpoint(A, ATTEMPT_A),
        channel: "tasks".into(),
        expires_at: "2026-11-03T10:00:02.000Z".into(),
        revoked: false,
    };
    let before = audit_head(&db).unwrap();
    let result = active_grant_at(&db, &grant, timestamp("2026-11-03T10:00:01.000Z").unwrap());
    assert!(
        result.is_err(),
        "an older last event hid the durable post-expiry witness"
    );
    assert_eq!(
        result
            .unwrap_err()
            .downcast_ref::<crate::workload::WorkloadFailureV1>(),
        Some(&crate::workload::WorkloadFailureV1::ClockRegression)
    );
    assert_eq!(audit_head(&db).unwrap(), before);
}

#[test]
fn derived_indexes_are_checked_against_audited_fields_after_body_pruning() {
    for assignment in [
        "destination_worker_id='bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'",
        "destination_attempt_id='bbbbbbbb-0000-4000-8000-bbbbbbbbbbbb'",
        "body_bytes=2",
        "acknowledged=1",
        "expires_at='2026-11-03T13:00:00.000Z'",
        "received_at='2026-11-03T10:00:01.000Z'",
    ] {
        let db = populated(1, 0, 0);
        let original = db.query_row(MESSAGE_ROW_SQL, [], MessageRow::read).unwrap();
        let metadata: RelayMessageMetadataV1 = serde_json::from_str(&original.metadata).unwrap();
        assert!(original.body.is_none());
        assert_eq!(original.index.body_bytes, 1);
        assert_eq!((original.accepted, original.last), (2, 2));
        check_message_projection(&original, &metadata, 2, OLD, 2).unwrap();
        assert_eq!((aggregate(&db).0, aggregate(&db).1), (0, 0));
        db.execute(&format!("UPDATE relay_messages SET {assignment}"), [])
            .unwrap();
        let altered = db.query_row(MESSAGE_ROW_SQL, [], MessageRow::read).unwrap();
        assert!(
            check_message_projection(&altered, &metadata, 2, OLD, 2).is_err(),
            "derived column escaped validation: {assignment}"
        );
    }
}
