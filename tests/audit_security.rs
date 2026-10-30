mod support;

use agentsandbox::{audit::Store, model::Event, redaction::Redactor};

#[test]
fn durable_idempotent_redacted_audit_survives_restart() {
    let dir = support::private_tempdir();
    let path = dir.path().join("audit.sqlite");
    let mut event = Event::new("command.requested");
    event.data = serde_json::json!({"api_key":"never-write-secret"});
    {
        let store = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
        let a = store.append(event.clone()).unwrap();
        let b = store.append(event.clone()).unwrap();
        assert_eq!(a.seq, b.seq);
        assert!(
            !serde_json::to_string(&a)
                .unwrap()
                .contains("never-write-secret")
        );
        assert!(store.integrity().unwrap().valid);
    }
    let store = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
    assert_eq!(store.integrity().unwrap().event_count, 2);
    let mut conflict = event;
    conflict.kind = "command.completed".into();
    assert!(store.append(conflict).is_err());
}

#[test]
fn audit_capacity_fail_closed_and_tampering_detected() {
    let dir = support::private_tempdir();
    let path = dir.path().join("audit.sqlite");
    let store = Store::open(&path, Redactor::new(vec![]), 2).unwrap();
    store.append(Event::new("command.requested")).unwrap();
    assert!(store.append(Event::new("command.completed")).is_err());
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "UPDATE events SET payload=replace(payload,'command.requested','command.completed')",
        [],
    )
    .unwrap();
    drop(db);
    let store = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
    assert!(!store.integrity().unwrap().valid);
}

#[test]
fn source_time_is_normalized_to_utc_before_hashing() {
    let dir = support::private_tempdir();
    let store = Store::open(&dir.path().join("audit.sqlite"), Redactor::new(vec![]), 100).unwrap();
    let mut event = Event::new("test.time");
    event.timestamp = "2026-10-29T10:30:00+02:00".into();
    let stored = store.append(event).unwrap();
    assert_eq!(stored.event.timestamp, "2026-10-29T08:30:00.000Z");
    assert!(store.integrity().unwrap().valid);
}
