mod support;

use agentsandbox::{audit::Store, model::Event, redaction::Redactor};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    time::timeout,
};

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

fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn probe_command(operation: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .arg(operation)
        .env_clear()
        .stdin(Stdio::null())
        .kill_on_drop(true);
    command
}

async fn inspect_gateway(data: &Path, runner: &Path, viewer: &Path) -> (Value, StatusCode, Value) {
    let mut gateway = probe_command("gateway")
        .args(["--listen", "127.0.0.1:0", "--data"])
        .arg(data)
        .arg("--runner-token-file")
        .arg(runner)
        .arg("--viewer-token-file")
        .arg(viewer)
        .arg("--policy")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("policies/default.json"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the loopback gateway");
    let mut stderr = BufReader::new(gateway.stderr.take().expect("gateway stderr"));
    let address = timeout(Duration::from_secs(15), async {
        let mut line = String::new();
        loop {
            line.clear();
            let read = stderr
                .read_line(&mut line)
                .await
                .expect("read gateway stderr");
            assert!(read > 0, "gateway exited before announcing its address");
            if let Some(address) = line
                .strip_prefix("Gateway ")
                .and_then(|rest| rest.split_whitespace().next())
            {
                break address
                    .parse::<SocketAddr>()
                    .expect("gateway socket address");
            }
        }
    })
    .await
    .expect("gateway did not announce its address in time");
    assert!(address.ip().is_loopback() && address.port() != 0);
    // Keep draining startup diagnostics without copying them into test output.
    let drain =
        tokio::spawn(async move { tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await });
    let url = format!("http://{address}");

    let result = timeout(
        Duration::from_secs(30),
        probe_command("verify-audit")
            .arg("--viewer-token-file")
            .arg(viewer)
            .arg("--gateway")
            .arg(&url)
            .output(),
    )
    .await
    .expect("verify-audit timed out")
    .expect("run verify-audit");
    assert!(
        result.status.success(),
        "verify-audit failed: {}",
        result.status
    );
    let audit = serde_json::from_slice(&result.stdout).expect("verify-audit JSON");

    let token = agentsandbox::runner::read_token(viewer).expect("read the fixture viewer token");
    let response = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
        .get(format!("{url}/api/workers"))
        .bearer_auth(token)
        .send()
        .await
        .expect("read the worker registry over loopback");
    let status = response.status();
    let workers = response.json().await.expect("worker registry JSON");

    let pid = i32::try_from(gateway.id().expect("owned gateway PID")).unwrap();
    // SAFETY: this test still owns the child and has not reaped it.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let stopped = timeout(Duration::from_secs(15), async {
        let status = gateway.wait().await.expect("wait for the gateway to stop");
        drain
            .await
            .expect("join stderr drain")
            .expect("drain gateway stderr");
        status
    })
    .await
    .expect("gateway shutdown timed out");
    assert!(
        stopped.success(),
        "gateway did not drain cleanly: {stopped}"
    );
    (audit, status, workers)
}

#[tokio::test]
async fn migration_cli_preserves_history_and_backup_and_requires_operator_attestation() {
    let dir = support::private_tempdir();
    let data = dir.path().join("audit.sqlite");
    let backup = dir.path().join("backup.sqlite");
    let runner = dir.path().join("runner-token");
    let viewer = dir.path().join("viewer-token");
    write_private(&runner, uuid::Uuid::new_v4().to_string().as_bytes());
    write_private(&viewer, uuid::Uuid::new_v4().to_string().as_bytes());
    legacy(&data);
    let original = Store::open(&data, Redactor::new(vec![]), 1000)
        .unwrap()
        .append(Event::new("historic.event"))
        .unwrap();
    let (before, legacy_status, legacy_workers) = inspect_gateway(&data, &runner, &viewer).await;
    assert_eq!(before["valid"], true);
    assert_eq!(before["event_count"], 1);
    assert_eq!(before["head_hash"], original.hash);
    assert_eq!(legacy_status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(legacy_workers["code"], "migration_required");

    assert!(!migrate(&data, &backup, false).await.status.success());
    assert!(!backup.exists());
    let result = migrate(&data, &backup, true).await;
    assert!(
        result.status.success(),
        "migration failed: {}",
        result.status
    );
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["from_version"], 1);
    assert_eq!(report["to_version"], 2);
    assert_eq!(report["genesis_seq"], original.seq + 1);
    let (after, migrated_status, migrated_workers) = inspect_gateway(&data, &runner, &viewer).await;
    assert_eq!(migrated_status, StatusCode::OK);
    assert_eq!(migrated_workers["items"], json!([]));
    assert_eq!(after["valid"], true);
    assert_eq!(after["event_count"], 2);
    assert_eq!(after["head_hash"], report["genesis_hash"]);
    let current = Store::open(&data, Redactor::new(vec![]), 1000).unwrap();
    assert!(current.workers().unwrap().is_empty());
    assert!(current.integrity().unwrap().valid);
    drop(current);

    // Reopen a fresh copy so the backup itself stays untouched by SQLite.
    let bytes = fs::read(&backup).unwrap();
    let restored_dir = dir.path().join("restored");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&restored_dir)
        .unwrap();
    let restored = restored_dir.join("audit.sqlite");
    write_private(&restored, &bytes);
    let archived = Store::open(&restored, Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(archived.head().unwrap(), original.seq);
    assert_eq!(archived.integrity().unwrap().head_hash, original.hash);
    drop(archived);
    let (recovered, restored_status, restored_workers) =
        inspect_gateway(&restored, &runner, &viewer).await;
    assert_eq!(recovered, before);
    assert_eq!(restored_status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(restored_workers["code"], "migration_required");

    assert!(!migrate(&data, &backup, true).await.status.success());
    assert_eq!(fs::read(&backup).unwrap(), bytes);
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
    assert!(
        !again.exists(),
        "no backup when the capability is already enabled"
    );
    let current =
        Store::open_existing_for_tenant(&data, "tenant-a", Redactor::new(vec![]), 100_000).unwrap();
    assert_eq!(current.head().unwrap(), head + 1);
    assert!(current.integrity().unwrap().valid);
}
