use agentsandbox::{audit::Store, model::Event, redaction::Redactor};

fn private_directory() -> tempfile::TempDir {
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().unwrap()
}

#[test]
fn startup_rejects_trigger_that_discards_audit_before_acknowledgement() {
    let directory = private_directory();
    let path = directory.path().join("audit.sqlite");
    drop(Store::open(&path, Redactor::new(vec![]), 100).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER discard_new_audit AFTER INSERT ON events BEGIN DELETE FROM events WHERE seq=NEW.seq; END;").unwrap();
    drop(db);
    assert!(Store::open(&path, Redactor::new(vec![]), 100).is_err());
}

#[test]
fn external_corruption_is_detected_before_append_without_integrity_endpoint() {
    let directory = private_directory();
    let path = directory.path().join("audit.sqlite");
    let store = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
    store.append(Event::new("command.requested")).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE events SET hash='tampered'", []).unwrap();
    assert!(store.append(Event::new("security.decision")).is_err());
    assert!(!store.healthy());
    assert_eq!(store.head().unwrap(), 2); // control genesis + requested event
}

#[test]
fn external_schema_change_is_detected_before_append() {
    let directory = private_directory();
    let path = directory.path().join("audit.sqlite");
    let store = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER discard_new_audit AFTER INSERT ON events BEGIN DELETE FROM events WHERE seq=NEW.seq; END;").unwrap();
    assert!(store.append(Event::new("security.decision")).is_err());
    assert!(!store.healthy());
    assert_eq!(store.head().unwrap(), 1); // genesis remains the only durable event
}

#[test]
fn corrupted_audit_opens_for_diagnosis_but_never_accepts_a_new_event() {
    let directory = private_directory();
    let path = directory.path().join("audit.sqlite");
    let store = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
    store.append(Event::new("command.requested")).unwrap();
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE events SET hash='tampered'", []).unwrap();
    drop(db);
    let store = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
    assert!(!store.healthy());
    assert!(store.append(Event::new("security.decision")).is_err());
    assert!(!store.integrity().unwrap().valid);
}

#[test]
fn unknown_schema_version_is_rejected_before_creating_application_tables() {
    let directory = private_directory();
    let path = directory.path().join("future.sqlite");
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
    }
    let db = rusqlite::Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 999).unwrap();
    assert!(Store::open(&path, Redactor::new(vec![]), 100).is_err());
    let tables: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0);
    let journal: String = db
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(journal, "delete");
}

#[test]
fn token_concatenation_cannot_disguise_missing_primary_key_constraint() {
    let directory = private_directory();
    let path = directory.path().join("audit.sqlite");
    drop(Store::open(&path, Redactor::new(vec![]), 100).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    let schema: String = db
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='events'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    // Simulate an external writer deliberately bypassing the new genesis FK.
    db.execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE events")
        .unwrap();
    db.execute_batch(&schema.replace("seq INTEGER PRIMARY KEY", "seq INTEGERPRIMARYKEY"))
        .unwrap();
    db.execute_batch("CREATE INDEX events_agent ON events(agent_id,seq); CREATE INDEX events_sandbox ON events(sandbox_id,seq); CREATE INDEX events_project ON events(project_id,seq); CREATE INDEX events_kind ON events(kind,seq);").unwrap();
    assert!(Store::open(&path, Redactor::new(vec![]), 100).is_err());
}

#[test]
fn missing_versioned_schema_objects_are_refused_without_repair() {
    for (statement, missing) in [
        ("DROP TABLE tenant_binding", "tenant_binding"),
        ("DROP TABLE quota", "quota"),
        ("DROP INDEX events_agent", "events_agent"),
    ] {
        let directory = private_directory();
        let path = directory.path().join("audit.sqlite");
        drop(Store::open(&path, Redactor::new(vec![]), 100).unwrap());
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(statement).unwrap();
        assert!(Store::open(&path, Redactor::new(vec![]), 100).is_err());
        let count: i64 = db
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name=?1",
                [missing],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}

#[test]
fn external_quota_corruption_blocks_append_instead_of_granting_capacity() {
    let directory = private_directory();
    let path = directory.path().join("audit.sqlite");
    let store = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
    store.append(Event::new("command.requested")).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE quota SET bytes=-1000000", []).unwrap();
    assert!(store.append(Event::new("security.decision")).is_err());
    assert!(!store.healthy());
    assert_eq!(store.head().unwrap(), 2);
}

#[test]
fn another_valid_store_can_append_without_false_corruption() {
    let directory = private_directory();
    let path = directory.path().join("audit.sqlite");
    let first = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
    let second = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
    first.append(Event::new("first.event")).unwrap();
    second.append(Event::new("second.event")).unwrap();
    first.append(Event::new("third.event")).unwrap();
    assert_eq!(first.head().unwrap(), 4);
    assert!(first.integrity().unwrap().valid);
}

#[cfg(unix)]
#[test]
fn hardlinked_database_is_rejected_in_distinct_processes() {
    const CHILD_PATH: &str = "AI_GUARD_AUDIT_HARDLINK_CHILD_PATH";
    if let Some(path) = std::env::var_os(CHILD_PATH) {
        let tenant = std::env::var("AI_GUARD_AUDIT_HARDLINK_CHILD_TENANT").unwrap();
        assert!(
            Store::open_for_tenant(
                std::path::Path::new(&path),
                &tenant,
                Redactor::new(vec![]),
                100
            )
            .is_err()
        );
        return;
    }
    let directory = private_directory();
    let a = directory.path().join("a.sqlite");
    let b = directory.path().join("b.sqlite");
    std::fs::write(&a, []).unwrap();
    std::fs::hard_link(&a, &b).unwrap();
    let executable = std::env::current_exe().unwrap();
    let children = [(&a, "tenant-a"), (&b, "tenant-b")].map(|(path, tenant)| {
        std::process::Command::new(&executable)
            .args([
                "--exact",
                "hardlinked_database_is_rejected_in_distinct_processes",
                "--nocapture",
            ])
            .env(CHILD_PATH, path)
            .env("AI_GUARD_AUDIT_HARDLINK_CHILD_TENANT", tenant)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    });
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "child must refuse a shared inode: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(std::fs::metadata(&a).unwrap().len(), 0);
    assert!(!directory.path().join("a.sqlite-wal").exists());
    assert!(!directory.path().join("b.sqlite-wal").exists());
}

#[cfg(unix)]
#[test]
fn final_symlink_and_parent_symlink_alias_are_refused() {
    use std::os::unix::fs::{DirBuilderExt, symlink};
    let directory = private_directory();
    let real = directory.path().join("real");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&real)
        .unwrap();
    symlink(&real, directory.path().join("alias")).unwrap();
    let path = real.join("audit.sqlite");
    let first = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
    assert!(
        Store::open(
            &directory.path().join("alias/audit.sqlite"),
            Redactor::new(vec![]),
            100,
        )
        .is_err()
    );
    first.append(Event::new("first.event")).unwrap();
    assert_eq!(first.head().unwrap(), 2);
    symlink(&path, directory.path().join("linked.sqlite")).unwrap();
    assert!(
        Store::open(
            &directory.path().join("linked.sqlite"),
            Redactor::new(vec![]),
            100
        )
        .is_err()
    );
}

#[cfg(unix)]
#[test]
fn database_path_substitution_or_new_hardlink_blocks_append() {
    let directory = private_directory();
    let path = directory.path().join("audit.sqlite");
    let store = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
    std::fs::hard_link(&path, directory.path().join("alias.sqlite")).unwrap();
    assert!(store.append(Event::new("security.decision")).is_err());
    assert!(!store.healthy());
    let path = directory.path().join("other.sqlite");
    let store = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
    std::fs::rename(&path, directory.path().join("preserved.sqlite")).unwrap();
    std::fs::write(&path, []).unwrap();
    assert!(store.append(Event::new("security.decision")).is_err());
    assert!(!store.healthy());
}
