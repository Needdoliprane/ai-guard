//! Operator command `agentsandbox audit-retention`, driven through the real
//! binary on a private format-13 tenant store. Reclaiming needs history older
//! than the policy floor (24 hours); the reclaim itself is exercised at the
//! Store seam in `audit_retention.rs` with an explicit evaluation time.
mod support;

use agentsandbox::{audit::Store, model::Event, redaction::Redactor};
use serde_json::Value;
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Output,
    time::Duration,
};

const TENANT: &str = "tenant-a";
const MAX_EVENTS: u64 = 1_000;

fn store13(dir: &Path) -> PathBuf {
    let data = dir.join("tenant-a.sqlite");
    let history = dir.join("history");
    std::fs::create_dir(&history).unwrap();
    std::fs::set_permissions(&history, std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .unwrap();
    let store = Store::open_for_tenant(&data, TENANT, Redactor::new(vec![]), MAX_EVENTS).unwrap();
    for (index, migrate) in [
        Store::migrate_control_v3,
        Store::migrate_identity_v4,
        Store::migrate_human_access_v5,
        Store::migrate_workload_v6,
        Store::migrate_human_brokers_v7,
        Store::migrate_kubernetes_create_v8,
        Store::migrate_kubernetes_policy_v9,
        Store::migrate_human_workers_v10,
        Store::migrate_kubernetes_session_reservations_v11,
        Store::migrate_broker_checkpoints_v12,
        Store::migrate_kubernetes_recovery_v13,
    ]
    .into_iter()
    .enumerate()
    {
        migrate(&store, &history.join(format!("before-{}.sqlite", index + 3))).unwrap();
    }
    for index in 0..5 {
        let mut event = Event::new("command.requested");
        event.session_id = Some(format!("session-{index}"));
        store.append(event).unwrap();
    }
    data
}

fn private_file(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

async fn run(args: &[&std::ffi::OsStr]) -> Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command.arg("audit-retention").args(args).kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(120), command.output())
        .await
        .expect("audit-retention finishes")
        .expect("audit-retention starts")
}

fn document(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "one JSON document expected ({error}); stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn os(value: &str) -> &std::ffi::OsStr {
    std::ffi::OsStr::new(value)
}

#[tokio::test]
async fn install_verify_and_export_through_the_operator_command() {
    let dir = support::private_tempdir();
    let data = store13(dir.path());
    let head = Store::open_existing_for_tenant(&data, TENANT, Redactor::new(vec![]), MAX_EVENTS)
        .unwrap()
        .integrity()
        .unwrap();
    assert!(head.valid);
    let backup = dir.path().join("before-retention.sqlite");

    let unattested = run(&[
        os("install"),
        os("--data"),
        data.as_os_str(),
        os("--tenant-id"),
        os(TENANT),
        os("--backup"),
        backup.as_os_str(),
    ])
    .await;
    assert!(!unattested.status.success());
    assert!(!backup.exists(), "a refused install writes no backup");

    let installed = run(&[
        os("install"),
        os("--data"),
        data.as_os_str(),
        os("--tenant-id"),
        os(TENANT),
        os("--backup"),
        backup.as_os_str(),
        os("--confirm-control-stopped"),
    ])
    .await;
    assert_eq!(installed.status.code(), Some(0), "{:?}", installed);
    let installed = document(&installed);
    assert_eq!(installed["format"], "ai-guard.audit.retention-command.v1");
    assert_eq!(installed["command"], "install");
    assert_eq!(installed["backup_head_seq"], head.event_count);
    assert!(backup.exists());

    let verified = run(&[
        os("verify"),
        os("--data"),
        data.as_os_str(),
        os("--tenant-id"),
        os(TENANT),
    ])
    .await;
    assert_eq!(verified.status.code(), Some(0), "{:?}", verified);
    let verified = document(&verified);
    assert_eq!(
        verified["format"],
        "ai-guard.audit.retention-verification.v1"
    );
    assert_eq!(verified["verdict"], "verified");
    assert_eq!(verified["segments"], 0);
    assert_eq!(verified["archived_through_seq"], 0);

    let export = dir.path().join("export-1");
    let exported = run(&[
        os("export"),
        os("--data"),
        data.as_os_str(),
        os("--tenant-id"),
        os(TENANT),
        os("--archive-dir"),
        export.as_os_str(),
    ])
    .await;
    assert_eq!(exported.status.code(), Some(0), "{:?}", exported);
    let exported = document(&exported);
    assert_eq!(exported["command"], "export");
    assert_eq!(exported["archive"]["purpose"], "export");
    assert_eq!(exported["archive"]["first_seq"], 1);
    let last = exported["archive"]["last_seq"].as_i64().unwrap();
    assert!(last > i64::try_from(head.event_count).unwrap());

    let again = run(&[
        os("export"),
        os("--data"),
        data.as_os_str(),
        os("--tenant-id"),
        os(TENANT),
        os("--archive-dir"),
        export.as_os_str(),
    ])
    .await;
    assert_eq!(again.status.code(), Some(2));
    assert_eq!(document(&again)["code"], "archive_destination_not_new");

    let checked = run(&[
        os("verify"),
        os("--data"),
        data.as_os_str(),
        os("--tenant-id"),
        os(TENANT),
        os("--archive-dir"),
        export.as_os_str(),
    ])
    .await;
    assert_eq!(checked.status.code(), Some(0), "{:?}", checked);
    let checked = document(&checked);
    assert_eq!(checked["verdict"], "verified");
    assert_eq!(checked["exports_verified"], 1);

    // One byte of one archived payload: the export no longer verifies.
    let events = export.join("events.jsonl");
    let original = std::fs::read(&events).unwrap();
    let mut tampered = original.clone();
    let index = tampered
        .windows(17)
        .position(|window| window == b"command.requested")
        .unwrap();
    tampered[index] = b'C';
    std::fs::write(&events, &tampered).unwrap();
    let refused = run(&[
        os("verify"),
        os("--data"),
        data.as_os_str(),
        os("--tenant-id"),
        os(TENANT),
        os("--archive-dir"),
        export.as_os_str(),
    ])
    .await;
    assert_eq!(refused.status.code(), Some(2));
    let refused = document(&refused);
    assert_eq!(refused["verdict"], "refused");
    assert!(
        refused["failure"]["code"] == "archive_chain_invalid"
            || refused["failure"]["code"] == "archive_digest_mismatch",
        "{refused}"
    );
}

#[tokio::test]
async fn plan_and_apply_refuse_history_younger_than_the_policy_floor() {
    let dir = support::private_tempdir();
    let data = store13(dir.path());
    let backup = dir.path().join("before-retention.sqlite");
    let installed = run(&[
        os("install"),
        os("--data"),
        data.as_os_str(),
        os("--tenant-id"),
        os(TENANT),
        os("--backup"),
        backup.as_os_str(),
        os("--confirm-control-stopped"),
    ])
    .await;
    assert_eq!(installed.status.code(), Some(0), "{:?}", installed);
    let policy = dir.path().join("retention-policy.json");
    private_file(
        &policy,
        br#"{"version":1,"min_age_seconds":86400,"keep_recent_events":1,"max_segment_events":100000,"classes":["ordinary","human_read"]}"#,
    );
    let below_floor = dir.path().join("too-short.json");
    private_file(
        &below_floor,
        br#"{"version":1,"min_age_seconds":60,"keep_recent_events":1,"max_segment_events":100000,"classes":["ordinary"]}"#,
    );
    let before = Store::open_existing_for_tenant(&data, TENANT, Redactor::new(vec![]), MAX_EVENTS)
        .unwrap()
        .integrity()
        .unwrap();

    let invalid = run(&[
        os("plan"),
        os("--data"),
        data.as_os_str(),
        os("--tenant-id"),
        os(TENANT),
        os("--policy"),
        below_floor.as_os_str(),
    ])
    .await;
    assert_eq!(invalid.status.code(), Some(2));
    assert_eq!(document(&invalid)["code"], "policy_invalid");

    let planned = run(&[
        os("plan"),
        os("--data"),
        data.as_os_str(),
        os("--tenant-id"),
        os(TENANT),
        os("--policy"),
        policy.as_os_str(),
    ])
    .await;
    assert_eq!(planned.status.code(), Some(2), "{:?}", planned);
    assert_eq!(document(&planned)["code"], "nothing_to_reclaim");

    let archive = dir.path().join("archive-1");
    let applied = run(&[
        os("apply"),
        os("--data"),
        data.as_os_str(),
        os("--tenant-id"),
        os(TENANT),
        os("--policy"),
        policy.as_os_str(),
        os("--through-seq"),
        os("3"),
        os("--expect-plan"),
        os(&"0".repeat(64)),
        os("--archive-dir"),
        archive.as_os_str(),
        os("--confirm-control-stopped"),
    ])
    .await;
    assert_eq!(applied.status.code(), Some(2), "{:?}", applied);
    assert_eq!(document(&applied)["code"], "nothing_to_reclaim");
    assert!(!archive.exists(), "a refused apply creates no archive");

    let after = Store::open_existing_for_tenant(&data, TENANT, Redactor::new(vec![]), MAX_EVENTS)
        .unwrap()
        .integrity()
        .unwrap();
    assert!(after.valid);
    assert_eq!(
        (after.event_count, after.head_hash),
        (before.event_count, before.head_hash)
    );
}
