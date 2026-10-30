//! SQLite lock preservation when the private journal encounters an alias.
//!
//! The journal namespace is expected to contain only journal files. It does
//! not scan for arbitrary SQLite databases: this test injects a hardlink to
//! exercise the known-alias refusal path.
#![cfg(unix)]

use agentsandbox::{
    audit::Store,
    broker::{
        intent::ExecutionIntentV1,
        journal::{
            BindingInputs, ControlSecrets, FsExclusions, Journal, JournalError, JournalLimitsV1,
            JournalScopeV1, ReserveResult,
        },
    },
    control_fs::Directory,
    model::Event,
    redaction::Redactor,
    runner::CheckpointPayloadV1,
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{FileExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use uuid::Uuid;

const SUBMIT: &[u8] = br#"{"action":"submit","fixture":"journal-sqlite-alias"}"#;
const OPERATION_ID: Uuid = Uuid::from_u128(0x33333333333343338333333333333333);
const LOCK_PROBE_ENV: &str = "AI_GUARD_JOURNAL_SQLITE_LOCK_PROBE";

fn private_root(path: &Path) -> PathBuf {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    let path = path.canonicalize().unwrap();
    #[cfg(target_os = "macos")]
    let path = {
        // Directory::open_existing requires the canonical /private/var path on
        // macOS, while TempDir can retain the /var alias.
        if path.starts_with("/var/") {
            PathBuf::from(path.to_string_lossy().replacen("/var/", "/private/var/", 1))
        } else {
            path
        }
    };
    path
}

fn scope() -> JournalScopeV1 {
    JournalScopeV1::new(
        Uuid::from_u128(0x11111111111141118111111111111111),
        "tenant-journal-sqlite-test",
        "http://127.0.0.1:8787",
        Uuid::from_u128(0x22222222222242228222222222222222),
        "broker-test",
    )
    .unwrap()
}

fn open_journal(root: &Path) -> Result<Journal, JournalError> {
    let root = private_root(root);
    let directory = Directory::open_existing(&root).unwrap();
    Journal::open(
        &directory,
        scope(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
}

fn legacy_plan() -> Value {
    let mut plan: Value =
        serde_json::from_str(include_str!("fixtures/broker_intent-plan-v3.json")).unwrap();
    plan["version"] = json!(1);
    plan["options"]["profile"] = json!("strict");
    plan["options"]["network"] = json!("none");
    plan["options"]["provider"] = json!("generic");
    plan["options"]["argv"] = json!(["/usr/bin/id"]);
    for field in [
        "client",
        "auth_mode",
        "client_profile",
        "providers_file",
        "prompt",
        "model",
    ] {
        plan["options"].as_object_mut().unwrap().remove(field);
    }
    plan["request"]["profile"] = json!("strict");
    plan["request"]["agent_id"] = json!("generic-22222222-2222-4222-8222-222222222222");
    plan["request"]["command"]["argv"] = json!(["/usr/bin/id"]);
    plan["request"]
        .as_object_mut()
        .unwrap()
        .remove("provider_resolution");
    plan["provider_input"] = Value::Null;
    plan["provider_credential"] = Value::Null;
    plan
}

fn accepted_none_journal(root: &Path) {
    let plan = serde_json::to_vec(&legacy_plan()).unwrap();
    let intent = ExecutionIntentV1::from_plan_bytes(&plan).unwrap();
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let journal = open_journal(root).unwrap();
    let ticket = match journal.reserve(OPERATION_ID, SUBMIT).unwrap() {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("fixture operation must be new"),
    };
    journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .unwrap();
}

fn journal_scope_dir(root: &Path) -> PathBuf {
    let namespace = root.join("broker-journal-v1");
    fs::read_dir(namespace)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
}

fn bytes_from_retained_file(file: &fs::File) -> Vec<u8> {
    let length = usize::try_from(file.metadata().unwrap().len()).unwrap();
    let mut bytes = vec![0; length];
    let mut offset = 0;
    while offset < bytes.len() {
        let read = file.read_at(&mut bytes[offset..], offset as u64).unwrap();
        assert!(
            read > 0,
            "retained SQLite main file ended during fixture read"
        );
        offset += read;
    }
    bytes
}

// This subprocess reproduces the F_GETLK + journal_mode=DELETE probe used by
// storage_security.rs::dropping_or_refusing_another_store_preserves_sqlite_connection_locks.
fn probe_sqlite_posix_locks(path: &Path) {
    use std::os::fd::AsRawFd;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    // SAFETY: flock is initialized before F_GETLK; the call only inspects
    // conflicting POSIX locks held by other processes.
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as _;
    lock.l_whence = libc::SEEK_SET as _;
    lock.l_start = 0;
    lock.l_len = 0;
    assert_eq!(
        unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut lock) },
        0
    );
    #[allow(clippy::unnecessary_cast)]
    let unlocked = libc::F_UNLCK as libc::c_short;
    assert_ne!(
        lock.l_type, unlocked,
        "the independent SQLite connection must retain its main-file POSIX lock"
    );
    drop(file);
    let db = rusqlite::Connection::open(path).unwrap();
    db.busy_timeout(Duration::from_millis(100)).unwrap();
    assert!(
        db.execute_batch("PRAGMA journal_mode=DELETE").is_err(),
        "a second process must not switch the active WAL database to DELETE"
    );
}

fn assert_live_sqlite_posix_locks(path: &Path, phase: &str) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "hardlink_alias_is_refused_without_releasing_sqlite_locks",
        ])
        .env(LOCK_PROBE_ENV, path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "SQLite lock probe failed during {phase}; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn hardlink_alias_is_refused_without_releasing_sqlite_locks() {
    if let Some(path) = std::env::var_os(LOCK_PROBE_ENV) {
        // Match the existing storage_security probe shape: the harness child
        // does only the F_GETLK + journal_mode=DELETE checks, then exits.
        probe_sqlite_posix_locks(Path::new(&path));
        return;
    }

    let temporary = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let fixture_root = temporary.path().join("journal-root");
    fs::create_dir(&fixture_root).unwrap();
    fs::set_permissions(&fixture_root, fs::Permissions::from_mode(0o700)).unwrap();
    let root = private_root(&fixture_root);

    // Store and a separate SQLite connection keep the actual main/WAL/SHM
    // files open throughout both probes and the journal rejection.
    let store_path = temporary.path().join("private-store");
    fs::create_dir(&store_path).unwrap();
    fs::set_permissions(&store_path, fs::Permissions::from_mode(0o700)).unwrap();
    let db_path = store_path.join("audit.sqlite");
    let store = Store::open(&db_path, Redactor::new(Vec::new()), 100).unwrap();
    store.append(Event::new("journal.sqlite.fixture")).unwrap();
    let sqlite = rusqlite::Connection::open(&db_path).unwrap();
    let _: i64 = sqlite
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .unwrap();
    // Retain this descriptor across refusal probes. Reopening or closing any
    // descriptor for the same inode in this process releases POSIX locks.
    let db_file = fs::File::open(&db_path).unwrap();
    for sidecar in ["audit.sqlite-wal", "audit.sqlite-shm"] {
        assert!(
            store_path.join(sidecar).exists(),
            "SQLite {sidecar} must be live"
        );
    }

    accepted_none_journal(&root);
    let scope_dir = journal_scope_dir(&root);
    let alias_name = "current.json";
    let alias_path = scope_dir
        .join("jobs")
        .join(OPERATION_ID.to_string())
        .join(alias_name);
    let backup_path = temporary
        .path()
        .join(format!("journal-alias-{alias_name}.fixture-backup"));
    fs::rename(&alias_path, &backup_path).unwrap();
    fs::hard_link(&db_path, &alias_path).unwrap();
    assert_eq!(
        fs::metadata(&alias_path).unwrap().ino(),
        fs::metadata(&db_path).unwrap().ino()
    );
    assert_eq!(fs::metadata(&alias_path).unwrap().nlink(), 2);

    assert_live_sqlite_posix_locks(&db_path, "before Journal alias refusal");
    let db_bytes_before_refusal = bytes_from_retained_file(&db_file);
    // Journal::open inventories existing job entries using metadata. The
    // hardlinked current pointer must fail there, in this same process as
    // Store, before any file open/close can release its POSIX locks.
    assert!(matches!(
        open_journal(&root),
        Err(JournalError::UnsafeFilesystem)
    ));
    assert_live_sqlite_posix_locks(&db_path, "after Journal alias refusal");
    assert_eq!(bytes_from_retained_file(&db_file), db_bytes_before_refusal);

    // The injected hardlink is test-owned. Remove that link after proving the
    // refusal so the real Store's single-link invariant is restored; the
    // displaced journal entry remains preserved in the backup above.
    fs::remove_file(&alias_path).unwrap();
    store
        .append(Event::new("journal.sqlite.after_refusal"))
        .unwrap();
    assert!(store.integrity().unwrap().valid);
    let _: i64 = sqlite
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .unwrap();
    assert!(
        backup_path.exists(),
        "preserve the displaced journal fixture"
    );
    assert_eq!(fs::metadata(&db_path).unwrap().nlink(), 1);
}
