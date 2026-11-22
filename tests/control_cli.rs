mod support;

use agentsandbox::{audit::Store, model::Event, redaction::Redactor};
use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt, path::Path, time::Duration};

fn legacy(path: &Path) {
    drop(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap(),
    );
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch("CREATE TABLE events(seq INTEGER PRIMARY KEY,id TEXT UNIQUE NOT NULL,received_at TEXT NOT NULL,prev_hash TEXT NOT NULL,hash TEXT NOT NULL,payload TEXT NOT NULL,kind TEXT NOT NULL,agent_id TEXT,sandbox_id TEXT,project_id TEXT,decision TEXT);
CREATE TABLE quota(id INTEGER PRIMARY KEY CHECK(id=1),bytes INTEGER NOT NULL);
CREATE TABLE tenant_binding(singleton INTEGER PRIMARY KEY CHECK(singleton=1),tenant_id TEXT NOT NULL UNIQUE,schema_version INTEGER NOT NULL CHECK(schema_version=1));
CREATE INDEX events_agent ON events(agent_id,seq);CREATE INDEX events_sandbox ON events(sandbox_id,seq);CREATE INDEX events_project ON events(project_id,seq);CREATE INDEX events_kind ON events(kind,seq);
INSERT INTO quota VALUES(1,0);INSERT INTO tenant_binding VALUES(1,'local',1);PRAGMA user_version=1;").unwrap();
}
async fn migrate(data: &Path, backup: &Path, confirmed: bool) -> std::process::Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .args(["migrate-control", "--tenant-id", "local", "--data"])
        .arg(data)
        .arg("--backup")
        .arg(backup)
        .kill_on_drop(true);
    if confirmed {
        command.arg("--confirm-control-stopped");
    }
    tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn migration_cli_preserves_history_and_backup_and_requires_operator_attestation() {
    let dir = support::private_tempdir();
    let data = dir.path().join("audit.sqlite");
    let backup = dir.path().join("backup.sqlite");
    legacy(&data);
    let original = Store::open(&data, Redactor::new(vec![]), 1000)
        .unwrap()
        .append(Event::new("historic.event"))
        .unwrap();
    assert!(!migrate(&data, &backup, false).await.status.success());
    assert!(!backup.exists());
    let result = migrate(&data, &backup, true).await;
    assert!(
        result.status.success(),
        "migration failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["from_version"], 1);
    assert_eq!(report["to_version"], 2);
    assert_eq!(report["genesis_seq"], original.seq + 1);
    let archived = Store::open(&backup, Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(archived.head().unwrap(), original.seq);
    assert_eq!(archived.integrity().unwrap().head_hash, original.hash);
    let current = Store::open(&data, Redactor::new(vec![]), 1000).unwrap();
    assert!(current.workers().unwrap().is_empty());
    assert!(current.integrity().unwrap().valid);
    let bytes = std::fs::read(&backup).unwrap();
    assert!(!migrate(&data, &backup, true).await.status.success());
    assert_eq!(std::fs::read(&backup).unwrap(), bytes);
}

#[tokio::test]
async fn migration_cli_does_not_create_a_missing_source_database() {
    let dir = support::private_tempdir();
    let data = dir.path().join("missing.sqlite");
    let backup = dir.path().join("backup.sqlite");
    assert!(!migrate(&data, &backup, true).await.status.success());
    assert!(
        !data.exists(),
        "migration must never initialize a mistyped database path"
    );
    assert!(!backup.exists());
}

async fn enable_effects(data: &Path, backup: &Path, confirmed: bool) -> std::process::Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .args([
            "enable-kubernetes-session-effects",
            "--tenant-id",
            "tenant-a",
            "--data",
        ])
        .arg(data)
        .arg("--backup")
        .arg(backup)
        .kill_on_drop(true);
    if confirmed {
        command.arg("--confirm-control-stopped");
    }
    tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .unwrap()
        .unwrap()
}

/// The offline effect capability maintenance through the real binary, on a
/// store migrated to format 13. Qualifies no gateway, broker or cluster.
#[tokio::test]
async fn effect_capability_cli_requires_attestation_keeps_the_format_and_runs_once() {
    use agentsandbox::audit::Store;
    let dir = support::private_tempdir();
    let data = dir.path().join("audit.sqlite");
    let store = Store::open_for_tenant(&data, "tenant-a", Redactor::new(vec![]), 100_000).unwrap();
    for (i, migrate) in [
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
        migrate(&store, &dir.path().join(format!("before-{}.sqlite", i + 3))).unwrap();
    }
    let head = store.head().unwrap();
    drop(store);
    let backup = dir.path().join("before-effects.sqlite");
    assert!(!enable_effects(&data, &backup, false).await.status.success());
    assert!(!backup.exists(), "no backup without the attestation");
    let result = enable_effects(&data, &backup, true).await;
    assert!(
        result.status.success(),
        "maintenance failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["from_version"], 13);
    assert_eq!(report["to_version"], 13, "no format change");
    assert_eq!(report["genesis_seq"], head + 1);
    let archived =
        Store::open_existing_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 100_000)
            .unwrap();
    assert_eq!(archived.head().unwrap(), head);
    // A second run is refused and appends nothing.
    let again = dir.path().join("effects-again.sqlite");
    assert!(!enable_effects(&data, &again, true).await.status.success());
    let current =
        Store::open_existing_for_tenant(&data, "tenant-a", Redactor::new(vec![]), 100_000).unwrap();
    assert_eq!(current.head().unwrap(), head + 1);
    assert!(current.integrity().unwrap().valid);
}
