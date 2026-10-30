#![cfg(unix)]

use agentsandbox::{audit::Store, model::Event, redaction::Redactor};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

fn private_directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

fn open(path: &Path) -> anyhow::Result<Store> {
    Store::open(path, Redactor::new(vec![]), 100)
}

#[test]
fn existing_permissive_database_is_refused_without_mutation_or_repair() {
    let directory = private_directory();
    let path = directory.path().join("audit.sqlite");
    drop(open(&path).unwrap());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(
        open(&path).is_err(),
        "a preexisting 0644 database must be refused"
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o644
    );
}

#[test]
fn existing_nonprivate_parent_is_refused_without_creation_or_repair() {
    let directory = private_directory();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let path = directory.path().join("audit.sqlite");
    assert!(open(&path).is_err(), "the database parent must be private");
    assert!(!path.exists());
    assert_eq!(
        fs::metadata(directory.path()).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn unsafe_sidecars_are_refused_before_sqlite_changes_any_bytes() {
    for suffix in ["-wal", "-shm", "-journal"] {
        let directory = private_directory();
        let path = directory.path().join("audit.sqlite");
        drop(open(&path).unwrap());
        let sidecar = directory.path().join(format!("audit.sqlite{suffix}"));
        fs::write(&sidecar, b"preserve invalid preexisting sidecar").unwrap();
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o644)).unwrap();
        let before = fs::read(&path).unwrap();
        assert!(
            open(&path).is_err(),
            "unsafe {suffix} must be refused before SQL"
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(
            fs::read(&sidecar).unwrap(),
            b"preserve invalid preexisting sidecar"
        );
    }
}

#[test]
fn permission_change_after_open_latches_before_new_event() {
    let directory = private_directory();
    let path = directory.path().join("audit.sqlite");
    let store = open(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(store.append(Event::new("must.not.persist")).is_err());
    assert!(!store.healthy());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(store.append(Event::new("still.refused")).is_err());
}

#[test]
fn reserved_sqlite_and_lock_names_are_refused_without_creation() {
    let directory = private_directory();
    for name in [
        "audit-wal",
        "audit-SHM",
        "audit-journal",
        ".ai-guard-sqlite-open.lock",
        ".AI-GUARD-SQLITE-open.lock",
    ] {
        let path = directory.path().join(name);
        assert!(
            open(&path).is_err(),
            "reserved SQLite name must be refused: {name}"
        );
        assert!(!path.exists());
    }
}

#[test]
fn dropping_or_refusing_another_store_preserves_sqlite_connection_locks() {
    const CHILD: &str = "AI_GUARD_SQLITE_LOCK_PROBE";
    if let Some(path) = std::env::var_os(CHILD) {
        use std::os::fd::AsRawFd;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        // SAFETY: flock is plain C data and initialized before fcntl; F_GETLK
        // inspects other-process POSIX locks without acquiring/releasing one.
        let mut lock: libc::flock = unsafe { std::mem::zeroed() };
        lock.l_type = libc::F_WRLCK as _;
        lock.l_whence = libc::SEEK_SET as _;
        lock.l_start = 0;
        lock.l_len = 0;
        // SAFETY: file descriptor and output pointer remain valid for fcntl.
        assert_eq!(
            unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut lock) },
            0
        );
        #[allow(clippy::unnecessary_cast)]
        let unlocked = libc::F_UNLCK as libc::c_short;
        assert_ne!(
            lock.l_type, unlocked,
            "the other SQLite connection must retain its actual POSIX main-file lock"
        );
        drop(file);
        let db = rusqlite::Connection::open(path).unwrap();
        db.busy_timeout(std::time::Duration::from_millis(100))
            .unwrap();
        assert!(
            db.execute_batch("PRAGMA journal_mode=DELETE").is_err(),
            "another process must not change the active WAL database to DELETE"
        );
        return;
    }
    let directory = private_directory();
    let path = directory.path().join("audit.sqlite");
    let first = open(&path).unwrap();
    let second = open(&path).unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    let _: i64 = connection
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    let probe = || {
        assert!(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "dropping_or_refusing_another_store_preserves_sqlite_connection_locks",
                    "--nocapture"
                ])
                .env(CHILD, &path)
                .status()
                .unwrap()
                .success()
        );
    };
    probe();
    drop(second);
    probe();
    assert!(Store::open_for_tenant(&path, "wrong-tenant", Redactor::new(vec![]), 100).is_err());
    probe();
    first.append(Event::new("locks.preserved")).unwrap();
    assert!(first.integrity().unwrap().valid);
}

#[test]
fn main_and_sidecar_special_entries_are_refused_without_opening_them() {
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, fs::symlink},
    };
    for suffix in ["", "-wal", "-shm", "-journal"] {
        for kind in ["fifo", "symlink", "hardlink", "directory"] {
            let directory = private_directory();
            let path = directory.path().join("audit.sqlite");
            if !suffix.is_empty() {
                drop(open(&path).unwrap());
            }
            let target = directory.path().join(format!("audit.sqlite{suffix}"));
            let original = directory.path().join("preserved");
            fs::write(&original, b"preserve special entry target").unwrap();
            fs::set_permissions(&original, fs::Permissions::from_mode(0o600)).unwrap();
            match kind {
                "fifo" => {
                    let encoded = CString::new(target.as_os_str().as_bytes()).unwrap();
                    // SAFETY: a new name within this test-owned private dir.
                    assert_eq!(unsafe { libc::mkfifo(encoded.as_ptr(), 0o600) }, 0);
                }
                "symlink" => symlink(&original, &target).unwrap(),
                "hardlink" => fs::hard_link(&original, &target).unwrap(),
                "directory" => fs::create_dir(&target).unwrap(),
                _ => unreachable!(),
            }
            let before = fs::symlink_metadata(&target).unwrap();
            assert!(open(&path).is_err(), "must refuse {suffix}/{kind}");
            use std::os::unix::fs::MetadataExt;
            let after = fs::symlink_metadata(&target).unwrap();
            assert_eq!(
                (after.dev(), after.ino(), after.mode(), after.nlink()),
                (before.dev(), before.ino(), before.mode(), before.nlink())
            );
            assert_eq!(
                fs::read(&original).unwrap(),
                b"preserve special entry target"
            );
        }
    }
}

#[test]
fn mutable_ancestor_is_refused_before_creating_database_or_lock() {
    use std::os::unix::fs::DirBuilderExt;
    let directory = private_directory();
    let parent = directory.path().join("mutable");
    fs::DirBuilder::new().mode(0o777).create(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o777)).unwrap();
    let private = parent.join("private");
    fs::DirBuilder::new().mode(0o700).create(&private).unwrap();
    assert!(open(&private.join("audit.sqlite")).is_err());
    assert_eq!(fs::read_dir(&private).unwrap().count(), 0);
    assert_eq!(
        fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
        0o777
    );
}

#[test]
fn changed_sidecar_and_parent_permissions_latch_before_append() {
    for target in ["-wal", "-shm", "parent"] {
        let directory = private_directory();
        let path = directory.path().join("audit.sqlite");
        let store = open(&path).unwrap();
        let changed = if target == "parent" {
            directory.path().to_path_buf()
        } else {
            directory.path().join(format!("audit.sqlite{target}"))
        };
        assert!(changed.exists());
        fs::set_permissions(
            &changed,
            fs::Permissions::from_mode(if target == "parent" { 0o755 } else { 0o644 }),
        )
        .unwrap();
        assert!(store.append(Event::new("permissions.refused")).is_err());
        assert!(!store.healthy());
        fs::set_permissions(
            &changed,
            fs::Permissions::from_mode(if target == "parent" { 0o700 } else { 0o600 }),
        )
        .unwrap();
        assert!(store.append(Event::new("latch.persists")).is_err());
    }
}

#[cfg(target_os = "macos")]
#[test]
fn extended_acl_on_main_or_sidecar_is_refused_and_preserved() {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let directory = private_directory();
        let path = directory.path().join("audit.sqlite");
        drop(open(&path).unwrap());
        let target = directory.path().join(format!("audit.sqlite{suffix}"));
        if !suffix.is_empty() {
            fs::write(&target, b"preserve ACL sidecar").unwrap();
        }
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            std::process::Command::new("/bin/chmod")
                .args(["+a", "everyone allow read"])
                .arg(&target)
                .status()
                .unwrap()
                .success()
        );
        let before = fs::read(&target).unwrap();
        let acl_before = std::process::Command::new("/bin/ls")
            .arg("-le")
            .arg(&target)
            .output()
            .unwrap()
            .stdout;
        assert!(open(&path).is_err());
        assert_eq!(fs::read(&target).unwrap(), before);
        assert_eq!(
            std::process::Command::new("/bin/ls")
                .arg("-le")
                .arg(&target)
                .output()
                .unwrap()
                .stdout,
            acl_before
        );
    }
}

#[test]
fn parallel_first_open_creates_one_private_database_and_preserves_all_events() {
    let directory = private_directory();
    let path = directory.path().join("audit.sqlite");
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || {
                let store = open(&path).unwrap();
                store.append(Event::new("parallel.open")).unwrap();
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let store = open(&path).unwrap();
    assert_eq!(store.head().unwrap(), 5);
    assert!(store.integrity().unwrap().valid);
    for name in [
        "audit.sqlite",
        "audit.sqlite-wal",
        "audit.sqlite-shm",
        ".ai-guard-sqlite-open.lock",
    ] {
        assert_eq!(
            fs::metadata(directory.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn initialization_lock_timeout_preserves_existing_database() {
    use std::os::fd::AsRawFd;
    let directory = private_directory();
    let path = directory.path().join("audit.sqlite");
    drop(open(&path).unwrap());
    let before = fs::read(&path).unwrap();
    let lock = fs::File::open(directory.path().join(".ai-guard-sqlite-open.lock")).unwrap();
    // SAFETY: the separately reserved fixture lock is live, with no SQLite FD.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let started = std::time::Instant::now();
    let error = open(&path)
        .err()
        .expect("a held initialization lock must time out");
    assert!(error.to_string().contains("initialization is busy"));
    assert!(started.elapsed() < std::time::Duration::from_secs(15));
    assert_eq!(fs::read(&path).unwrap(), before);
    drop(lock);
    assert!(open(&path).unwrap().integrity().unwrap().valid);
}

#[test]
fn legitimate_processes_can_open_and_append_to_one_private_database() {
    const CHILD: &str = "AI_GUARD_SQLITE_APPEND_CHILD";
    if let Some(path) = std::env::var_os(CHILD) {
        let store = open(Path::new(&path)).unwrap();
        store.append(Event::new("process.append")).unwrap();
        assert!(store.integrity().unwrap().valid);
        return;
    }
    let directory = private_directory();
    let path = directory.path().join("audit.sqlite");
    let parent_store = open(&path).unwrap();
    let children: Vec<_> = (0..3)
        .map(|_| {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "legitimate_processes_can_open_and_append_to_one_private_database",
                ])
                .env(CHILD, &path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    parent_store.append(Event::new("parent.append")).unwrap();
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(parent_store.head().unwrap(), 5);
    assert!(parent_store.integrity().unwrap().valid);
}

#[test]
fn orphan_sidecars_are_preserved_without_creating_a_new_database() {
    for suffix in ["-wal", "-shm", "-journal"] {
        let directory = private_directory();
        let path = directory.path().join("missing.sqlite");
        let sidecar = directory.path().join(format!("missing.sqlite{suffix}"));
        fs::write(&sidecar, b"orphaned SQLite state must survive").unwrap();
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(open(&path).is_err());
        assert!(!path.exists());
        assert_eq!(
            fs::read(&sidecar).unwrap(),
            b"orphaned SQLite state must survive"
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}

#[test]
fn a_lock_hardlink_to_another_live_sqlite_inode_is_refused_without_closing_it() {
    let directory = private_directory();
    let protected = directory.path().join("protected.sqlite");
    let protected_store = open(&protected).unwrap();
    let candidate = directory.path().join("candidate.sqlite");
    drop(open(&candidate).unwrap());
    let lock_path = directory.path().join(".ai-guard-sqlite-open.lock");
    // Deliberately corrupt only this fixture's separately created lock entry.
    fs::remove_file(&lock_path).unwrap();
    fs::hard_link(&protected, &lock_path).unwrap();
    let probe = || {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "dropping_or_refusing_another_store_preserves_sqlite_connection_locks",
            ])
            .env("AI_GUARD_SQLITE_LOCK_PROBE", &protected)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    probe();
    assert!(open(&candidate).is_err());
    probe();
    use std::os::unix::fs::MetadataExt;
    assert_eq!(
        fs::metadata(&lock_path).unwrap().ino(),
        fs::metadata(&protected).unwrap().ino()
    );
    assert!(protected_store.healthy());
}

#[test]
fn unicode_casefold_aliases_of_reserved_names_are_refused_before_creation() {
    for name in [
        "audit-ſhm",
        ".ai-guard-ſqlite-open.locK",
        "événements.sqlite",
    ] {
        let directory = private_directory();
        let path = directory.path().join(name);
        assert!(
            open(&path).is_err(),
            "Unicode SQLite basename must be refused before resolving aliases: {name}"
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[test]
fn unicode_parent_names_support_an_ascii_sqlite_filename() {
    use std::os::unix::fs::DirBuilderExt;
    let directory = private_directory();
    let parent = directory.path().join("contrôle-é");
    fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
    let store = open(&parent.join("audit.sqlite")).unwrap();
    store.append(Event::new("unicode.parent")).unwrap();
    assert!(store.integrity().unwrap().valid);
}
