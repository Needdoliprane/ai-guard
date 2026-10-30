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
fn database_cannot_be_reassigned_to_another_tenant_after_restart() {
    let directory = private_directory();
    let path = directory.path().join("tenant.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 100).unwrap();
    store.append(Event::new("tenant.event")).unwrap();
    drop(store);
    assert!(Store::open_for_tenant(&path, "tenant-b", Redactor::new(vec![]), 100).is_err());
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 100).unwrap();
    assert_eq!(store.head().unwrap(), 2);
    assert_eq!(store.tenant_id(), "tenant-a");
}

#[test]
fn legacy_nonempty_database_requires_explicit_local_identity() {
    let directory = private_directory();
    let path = directory.path().join("legacy.sqlite");
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
    }
    // Build the historical schema rather than relabelling a v2 control database.
    let legacy = rusqlite::Connection::open(&path).unwrap();
    legacy.execute_batch("CREATE TABLE events(seq INTEGER PRIMARY KEY,id TEXT UNIQUE NOT NULL,received_at TEXT NOT NULL,prev_hash TEXT NOT NULL,hash TEXT NOT NULL,payload TEXT NOT NULL,kind TEXT NOT NULL,agent_id TEXT,sandbox_id TEXT,project_id TEXT,decision TEXT);
CREATE TABLE quota(id INTEGER PRIMARY KEY CHECK(id=1),bytes INTEGER NOT NULL);
CREATE INDEX events_agent ON events(agent_id,seq);CREATE INDEX events_sandbox ON events(sandbox_id,seq);CREATE INDEX events_project ON events(project_id,seq);CREATE INDEX events_kind ON events(kind,seq);INSERT INTO quota VALUES(1,0);").unwrap();
    drop(legacy);
    let store = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
    store.append(Event::new("legacy.event")).unwrap();
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("DROP TABLE tenant_binding; PRAGMA user_version=0;")
        .unwrap();
    drop(db);
    assert!(Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 100).is_err());
    let store = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
    assert_eq!(store.head().unwrap(), 1);
}

#[test]
fn invalid_tenant_identity_does_not_create_database() {
    let directory = private_directory();
    let path = directory.path().join("invalid.sqlite");
    for tenant in ["", "../tenant", "tenant/a", "tenant\n", &"a".repeat(129)] {
        assert!(Store::open_for_tenant(&path, tenant, Redactor::new(vec![]), 100).is_err());
    }
    assert!(!path.exists());
}
