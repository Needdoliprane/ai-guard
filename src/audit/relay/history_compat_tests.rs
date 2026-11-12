//! Historical ordinary event fixtures only. No event content is executed.
use super::validate;
use crate::audit::{Store, audit_head, capacity};
use crate::model::{Event, StoredEvent};
use crate::redaction::Redactor;
use rusqlite::{Connection, OpenFlags, types::Value};
use std::path::{Path, PathBuf};

#[path = "../../../tests/support/mod.rs"]
mod support;

fn fixture() -> (tempfile::TempDir, PathBuf, Store) {
    let directory = support::private_tempdir();
    let path = directory.path().join("tenant.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    store
        .migrate_control_v3(&path.with_extension("v3.backup"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("v4.backup"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("v5.backup"))
        .unwrap();
    store
        .migrate_workload_v6(&path.with_extension("v6.backup"))
        .unwrap();
    let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        db.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        6
    );
    assert!(store.healthy());
    assert!(store.integrity().unwrap().valid);
    (directory, path, store)
}

/// The old ordinary path, without its namespace admission guard. This is
/// confined to historical test setup; all durable writer checks remain real.
fn append_historical_ordinary(store: &Store, event: Event) -> anyhow::Result<StoredEvent> {
    assert!(
        !event.kind.starts_with("control."),
        "historical fixture kind was refused by the old control namespace guard"
    );
    let (event, payload) = store.prepare_event(event)?;
    let mut db = store
        .db
        .lock()
        .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
    let tx = store.begin_checked(&mut db)?;
    let stored =
        store.append_with_capacity_class(&tx, event, payload, capacity::Class::OrdinaryWrite)?;
    store.commit_checked(tx)?;
    Ok(stored)
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    head: (i64, String),
    rows: Vec<Vec<Value>>,
    relay_counts: (i64, i64, i64),
}

fn snapshot(path: &Path) -> Snapshot {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut query = db.prepare("SELECT seq,id,received_at,prev_hash,hash,payload,kind,agent_id,sandbox_id,project_id,decision FROM events ORDER BY seq").unwrap();
    let rows = query
        .query_map([], |row| {
            (0..11)
                .map(|column| row.get::<_, Value>(column))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    Snapshot {
        head: audit_head(&db).unwrap(),
        rows,
        relay_counts: (
            db.query_row("SELECT COUNT(*) FROM relay_grants", [], |row| row.get(0))
                .unwrap(),
            db.query_row("SELECT COUNT(*) FROM relay_messages", [], |row| row.get(0))
                .unwrap(),
            db.query_row("SELECT COUNT(*) FROM relay_operations", [], |row| {
                row.get(0)
            })
            .unwrap(),
        ),
    }
}

#[test]
fn canonical_unknown_relay_names_refuse_validation_and_rollback_without_history_change() {
    for kind in ["control.relay.x", "control.relay.X"] {
        let (_directory, path, store) = fixture();
        let before = snapshot(&path);
        {
            let (event, payload) = store.prepare_event(Event::new(kind)).unwrap();
            let mut db = store.db.lock().unwrap();
            let tx = store.begin_checked(&mut db).unwrap();
            let stored = store
                .append_with_capacity_class(&tx, event, payload, capacity::Class::OrdinaryWrite)
                .unwrap();
            assert_eq!(stored.event.kind, kind);
            assert_eq!(stored.seq, before.head.0 + 1);
            assert!(Store::check_integrity(&tx).unwrap().valid);
            let error = validate(&tx).unwrap_err();
            assert_eq!(error.to_string(), "unknown authoritative relay event");
            eprintln!("canonical relay validator refused {kind}: {error:#}");
            // No invalid canonical row is committed. The actual checked
            // transaction rolls back; no durable receipt is claimed here.
            drop(tx);
        }
        assert_eq!(snapshot(&path), before);
        assert_eq!(store.head().unwrap(), before.head.0);
        drop(store);
        let reopened =
            Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000)
                .unwrap();
        assert!(
            reopened.healthy(),
            "rollback left a persisted canonical row"
        );
        assert!(reopened.integrity().unwrap().valid);
        assert_eq!(snapshot(&path), before);
    }
}

#[test]
fn historical_mixed_case_ordinary_relay_names_remain_unpromoted_and_unchanged() {
    let (_directory, path, store) = fixture();
    let initial = snapshot(&path);
    for kind in [
        "Control.relay.x",
        "CONTROL.relay.x",
        "CoNtRoL.ReLaY.x",
        "Fixture.MixedCase",
    ] {
        let stored = append_historical_ordinary(&store, Event::new(kind)).unwrap();
        assert_eq!(stored.event.kind, kind, "historical fixture kind changed");
    }
    let before = snapshot(&path);
    assert_eq!(before.head.0, initial.head.0 + 4);
    assert_eq!(before.relay_counts, (0, 0, 0));
    let direct = {
        let db = store.db.lock().unwrap();
        assert!(Store::check_integrity(&db).unwrap().valid);
        validate(&db)
    };
    let direct_error = direct.as_ref().err().map(|error| format!("{error:#}"));
    assert_eq!(snapshot(&path), before);
    drop(store);

    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    let reopened_healthy = reopened.healthy();
    let integrity = reopened.integrity();
    let integrity_error = integrity.as_ref().err().map(|error| format!("{error:#}"));
    assert_eq!(snapshot(&path), before);
    // This assertion expects the correct future reader behaviour. On the
    // unchanged LIKE reader, preserve the actual validation/reopen errors.
    assert!(
        direct.is_ok() && reopened_healthy && integrity.as_ref().is_ok_and(|value| value.valid),
        "historical ordinary kinds entered relay authority: direct_error={direct_error:?}; reopened_healthy={reopened_healthy}; integrity_error={integrity_error:?}"
    );
}
