use agentsandbox::{
    audit::{EventQuery, Store},
    control::ControlFailure,
    model::Event,
    redaction::Redactor,
};
use std::path::Path;

fn private_tempdir() -> tempfile::TempDir {
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().unwrap()
}

fn v1(path: &Path) -> Store {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    drop(options.open(path).unwrap());
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch("CREATE TABLE events(seq INTEGER PRIMARY KEY,id TEXT UNIQUE NOT NULL,received_at TEXT NOT NULL,prev_hash TEXT NOT NULL,hash TEXT NOT NULL,payload TEXT NOT NULL,kind TEXT NOT NULL,agent_id TEXT,sandbox_id TEXT,project_id TEXT,decision TEXT);
CREATE TABLE quota(id INTEGER PRIMARY KEY CHECK(id=1),bytes INTEGER NOT NULL);
CREATE TABLE tenant_binding(singleton INTEGER PRIMARY KEY CHECK(singleton=1),tenant_id TEXT NOT NULL UNIQUE,schema_version INTEGER NOT NULL CHECK(schema_version=1));
CREATE INDEX events_agent ON events(agent_id,seq); CREATE INDEX events_sandbox ON events(sandbox_id,seq);CREATE INDEX events_project ON events(project_id,seq);CREATE INDEX events_kind ON events(kind,seq);
INSERT INTO quota VALUES(1,0);INSERT INTO tenant_binding VALUES(1,'local',1);PRAGMA user_version=1;").unwrap();
    drop(db);
    Store::open(path, Redactor::new(vec![]), 1000).unwrap()
}

fn assert_legacy_event(store: &Store, seq: i64, hash: &str) {
    assert_eq!(store.head().unwrap(), seq);
    assert!(store.integrity().unwrap().valid);
    let events = store
        .events(&EventQuery {
            order: Some("asc".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].hash, hash);
    assert_eq!(
        store
            .workers()
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::MigrationRequired)
    );
}

#[cfg(unix)]
fn private_directory(path: &Path) {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
}

#[cfg(unix)]
fn existing_entry(path: &Path, kind: &str) {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let write = |target: &Path, mode| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(target)
            .unwrap();
        file.write_all(b"preexisting fixture, preserve exactly")
            .unwrap();
    };
    match kind {
        "private" => write(path, 0o600),
        "public" => write(path, 0o644),
        "directory" => private_directory(path),
        "symlink" => {
            let target = path.with_extension("symlink-target");
            write(&target, 0o600);
            std::os::unix::fs::symlink(&target, path).unwrap();
        }
        "hardlink" => {
            let target = path.with_extension("hardlink-target");
            write(&target, 0o600);
            std::fs::hard_link(&target, path).unwrap();
        }
        "fifo" => {
            use std::os::unix::ffi::OsStrExt;
            let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            // SAFETY: the path is an owned NUL-terminated fixture name.
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        }
        _ => panic!("unknown fixture kind"),
    }
}

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
struct EntryState {
    device: u64,
    inode: u64,
    uid: u32,
    mode: u32,
    links: u64,
    bytes: Option<Vec<u8>>,
    link_target: Option<std::path::PathBuf>,
}

#[cfg(unix)]
fn entry_state(path: &Path) -> EntryState {
    use std::os::unix::fs::MetadataExt;
    let facts = std::fs::symlink_metadata(path).unwrap();
    EntryState {
        device: facts.dev(),
        inode: facts.ino(),
        uid: facts.uid(),
        mode: facts.mode(),
        links: facts.nlink(),
        bytes: facts.is_file().then(|| std::fs::read(path).unwrap()),
        link_target: facts
            .file_type()
            .is_symlink()
            .then(|| std::fs::read_link(path).unwrap()),
    }
}

#[cfg(unix)]
#[test]
fn backup_parent_must_be_private_before_any_destination_or_ddl_is_created() {
    use std::os::unix::fs::PermissionsExt;
    for mode in [0o755, 0o770, 0o777] {
        let dir = private_tempdir();
        let source = dir.path().join("v1");
        let store = v1(&source);
        let original = store.append(Event::new("historic.event")).unwrap();
        let parent = dir.path().join("public-backups");
        private_directory(&parent);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(mode)).unwrap();
        let backup = parent.join("backup");
        assert!(
            store.migrate_control_v2(&backup).is_err(),
            "backup parent mode {mode:o} must be refused"
        );
        assert!(!backup.exists());
        assert_eq!(
            std::fs::metadata(&parent).unwrap().permissions().mode() & 0o7777,
            mode
        );
        assert_legacy_event(&store, original.seq, &original.hash);
        drop(store);
        let reopened = Store::open(&source, Redactor::new(vec![]), 1000).unwrap();
        assert_legacy_event(&reopened, original.seq, &original.hash);
    }
}

#[cfg(unix)]
#[test]
fn backup_private_directory_under_mutable_ancestor_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = private_tempdir();
    let store = v1(&dir.path().join("v1"));
    let original = store.append(Event::new("historic.event")).unwrap();
    let mutable = dir.path().join("shared");
    private_directory(&mutable);
    std::fs::set_permissions(&mutable, std::fs::Permissions::from_mode(0o777)).unwrap();
    let private = mutable.join("private-backups");
    private_directory(&private);
    let backup = private.join("backup");
    assert!(store.migrate_control_v2(&backup).is_err());
    assert!(!backup.exists());
    assert_legacy_event(&store, original.seq, &original.hash);
}

#[cfg(unix)]
#[test]
fn backup_directory_alias_is_refused_without_writing_through_it() {
    let dir = private_tempdir();
    let source = dir.path().join("v1");
    let store = v1(&source);
    let original = store.append(Event::new("historic.event")).unwrap();
    let real = dir.path().join("real-backups");
    private_directory(&real);
    let alias = dir.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    assert!(store.migrate_control_v2(&alias.join("backup")).is_err());
    assert!(!real.join("backup").exists());
    assert_eq!(std::fs::read_link(&alias).unwrap(), real);
    assert_legacy_event(&store, original.seq, &original.hash);
}

#[cfg(unix)]
#[test]
fn existing_backup_entries_are_refused_without_repair_or_replacement() {
    for kind in [
        "private",
        "public",
        "directory",
        "symlink",
        "hardlink",
        "fifo",
    ] {
        let dir = private_tempdir();
        let source = dir.path().join("v1");
        let store = v1(&source);
        let original = store.append(Event::new("historic.event")).unwrap();
        let backup = dir.path().join("backup");
        existing_entry(&backup, kind);
        let before = entry_state(&backup);
        assert!(store.migrate_control_v2(&backup).is_err(), "{kind}");
        assert_eq!(entry_state(&backup), before, "{kind}");
        assert_legacy_event(&store, original.seq, &original.hash);
        drop(store);
        let reopened = Store::open(&source, Redactor::new(vec![]), 1000).unwrap();
        assert_legacy_event(&reopened, original.seq, &original.hash);
    }
}

#[cfg(unix)]
#[test]
fn every_preexisting_backup_sidecar_is_preserved_before_main_creation() {
    for suffix in ["-wal", "-shm", "-journal"] {
        for kind in [
            "private",
            "public",
            "directory",
            "symlink",
            "hardlink",
            "fifo",
        ] {
            let dir = private_tempdir();
            let store = v1(&dir.path().join("v1"));
            let original = store.append(Event::new("historic.event")).unwrap();
            let backup = dir.path().join("backup");
            let sidecar = dir.path().join(format!("backup{suffix}"));
            existing_entry(&sidecar, kind);
            let before = entry_state(&sidecar);
            assert!(
                store.migrate_control_v2(&backup).is_err(),
                "{suffix}/{kind}"
            );
            assert!(!backup.exists(), "{suffix}/{kind}");
            assert_eq!(entry_state(&sidecar), before, "{suffix}/{kind}");
            assert_legacy_event(&store, original.seq, &original.hash);
        }
    }
}

#[test]
fn source_cannot_be_selected_as_its_own_backup() {
    let dir = private_tempdir();
    let source = dir.path().join("v1");
    let store = v1(&source);
    let original = store.append(Event::new("historic.event")).unwrap();
    assert!(store.migrate_control_v2(&source).is_err());
    assert_legacy_event(&store, original.seq, &original.hash);
}

#[cfg(target_os = "macos")]
#[test]
fn backup_parent_with_extended_acl_is_refused_without_removing_acl() {
    use std::process::Command;
    let dir = private_tempdir();
    let store = v1(&dir.path().join("v1"));
    let original = store.append(Event::new("historic.event")).unwrap();
    let parent = dir.path().join("backups");
    private_directory(&parent);
    assert!(
        Command::new("/bin/chmod")
            .args(["+a", "everyone allow list,search"])
            .arg(&parent)
            .status()
            .unwrap()
            .success()
    );
    let acl_before = Command::new("/bin/ls")
        .arg("-lde")
        .arg(&parent)
        .output()
        .unwrap();
    assert!(acl_before.status.success());
    assert!(store.migrate_control_v2(&parent.join("backup")).is_err());
    assert!(!parent.join("backup").exists());
    let acl_after = Command::new("/bin/ls")
        .arg("-lde")
        .arg(&parent)
        .output()
        .unwrap();
    assert!(acl_after.status.success());
    assert_eq!(acl_before.stdout, acl_after.stdout);
    assert_legacy_event(&store, original.seq, &original.hash);
}

#[test]
fn explicit_migration_preserves_prior_events_and_verified_backup() {
    let dir = private_tempdir();
    let path = dir.path().join("v1");
    let backup = dir.path().join("backup");
    let store = v1(&path);
    let original = store.append(Event::new("historic.event")).unwrap();
    assert_eq!(
        store
            .workers()
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::MigrationRequired)
    );
    let report = store.migrate_control_v2(&backup).unwrap();
    assert_eq!(report.from_version, 1);
    assert_eq!(report.to_version, 2);
    assert_eq!(report.genesis_seq, original.seq + 1);
    assert!(store.workers().unwrap().is_empty());
    assert!(store.integrity().unwrap().valid);
    let events = store
        .events(&EventQuery {
            order: Some("asc".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(events[0].hash, original.hash);
    let migrated_head = store.head().unwrap();
    assert!(store.migrate_control_v2(&backup).is_err());
    let unnecessary = dir.path().join("second-backup");
    assert!(store.migrate_control_v2(&unnecessary).is_err());
    assert!(!unnecessary.exists());
    assert_eq!(store.head().unwrap(), migrated_head);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let facts = std::fs::metadata(&backup).unwrap();
        assert!(facts.is_file());
        // SAFETY: geteuid has no pointer arguments or preconditions.
        assert_eq!(facts.uid(), unsafe { libc::geteuid() });
        assert_eq!(facts.mode() & 0o7777, 0o600);
        assert_eq!(facts.nlink(), 1);
    }
    drop(store);
    let source = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    assert!(source.workers().unwrap().is_empty());
    assert_eq!(source.head().unwrap(), migrated_head);
    assert!(source.integrity().unwrap().valid);
    let backed = Store::open(&backup, Redactor::new(vec![]), 1000).unwrap();
    assert_legacy_event(&backed, original.seq, &original.hash);
}

#[test]
fn migration_never_overwrites_backup_and_audit_full_rolls_back_ddl() {
    let dir = private_tempdir();
    let path = dir.path().join("v1");
    let store = v1(&path);
    let original = store.append(Event::new("historic.event")).unwrap();
    drop(store);
    let store = Store::open(&path, Redactor::new(vec![]), 1).unwrap();
    let backup = dir.path().join("backup");
    assert!(store.migrate_control_v2(&backup).is_err());
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='control_meta'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    drop(db);
    assert_legacy_event(&store, original.seq, &original.hash);
    let before = std::fs::read(&backup).unwrap();
    assert!(store.migrate_control_v2(&backup).is_err());
    assert_eq!(std::fs::read(&backup).unwrap(), before);
    drop(store);
    let source = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    assert_legacy_event(&source, original.seq, &original.hash);
    let backed = Store::open(&backup, Redactor::new(vec![]), 1000).unwrap();
    assert_legacy_event(&backed, original.seq, &original.hash);
}

#[test]
fn future_and_partial_schemas_are_refused_without_repair() {
    let dir = private_tempdir();
    let path = dir.path().join("v1");
    drop(v1(&path));
    let db = rusqlite::Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 3).unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    assert!(Store::open(&path, Redactor::new(vec![]), 100).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 1).unwrap();
    db.execute("DROP INDEX events_kind", []).unwrap();
    drop(db);
    assert!(Store::open(&path, Redactor::new(vec![]), 100).is_err());
}

#[test]
fn missing_v2_projection_is_not_repaired_and_backup_sidecars_are_preserved() {
    let dir = private_tempdir();
    let path = dir.path().join("v2");
    drop(Store::open(&path, Redactor::new(vec![]), 100).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("DROP TABLE worker_mutations", []).unwrap();
    drop(db);
    assert!(Store::open(&path, Redactor::new(vec![]), 100).is_err());
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='worker_mutations'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    let store = v1(&dir.path().join("v1"));
    let backup = dir.path().join("backup");
    let sidecar = dir.path().join("backup-wal");
    std::fs::write(&sidecar, b"preserved").unwrap();
    assert!(store.migrate_control_v2(&backup).is_err());
    assert!(!backup.exists());
    assert_eq!(std::fs::read(sidecar).unwrap(), b"preserved");
}
