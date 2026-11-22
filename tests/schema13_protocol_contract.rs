//! Actual CLI migration and loopback gateway reads of both schema versions,
//! using private synthetic tenant data and an independently restored copy.
use agentsandbox::{
    audit::{Integrity, Store},
    control::{ControlProtocolV1, MigrationReportV1, SupervisionProtocolV1},
    redaction::Redactor,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
use tokio::{io::AsyncBufReadExt, time::timeout};

const TENANT: &str = "schema13-contract-tenant";
const SCHEMA: &str = include_str!("../schemas/supervision-protocol.schema.json");
const RUNNER: &str = "schema13-contract-synthetic-runner-fixture";
const VIEWER: &str = "schema13-contract-synthetic-viewer-fixture";

fn prepare_schema12(path: &Path) {
    let store = Store::open_for_tenant(path, TENANT, Redactor::new(vec![]), 10_000).unwrap();
    store
        .migrate_control_v3(&path.with_extension("before3"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("before4"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("before5"))
        .unwrap();
    store
        .migrate_workload_v6(&path.with_extension("before6"))
        .unwrap();
    store
        .migrate_human_brokers_v7(&path.with_extension("before7"))
        .unwrap();
    store
        .migrate_kubernetes_create_v8(&path.with_extension("before8"))
        .unwrap();
    store
        .migrate_kubernetes_policy_v9(&path.with_extension("before9"))
        .unwrap();
    store
        .migrate_human_workers_v10(&path.with_extension("before10"))
        .unwrap();
    store
        .migrate_kubernetes_session_reservations_v11(&path.with_extension("before11"))
        .unwrap();
    store
        .migrate_broker_checkpoints_v12(&path.with_extension("before12"))
        .unwrap();
    assert!(store.integrity().unwrap().valid);
}

fn migration_cli(root: &Path, database: &Path, backup: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root)
        .env("TMPDIR", root)
        .args(["migrate-control", "--data"])
        .arg(database)
        .args(["--tenant-id", TENANT, "--backup"])
        .arg(backup)
        .args([
            "--target-version",
            "13",
            "--confirm-control-stopped",
            "--max-events",
            "10000",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the delivered CLI only on owned private fixture data");
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().unwrap(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "owned migration CLI did not terminate within its harness bound: {result:?}"
                );
            }
        }
    }
}

fn create_private_file(path: &Path, bytes: &[u8]) {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

async fn read_json<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> T {
    request
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn read_gateway(
    root: &Path,
    database: &Path,
    expected_version: u32,
    expected_namespace: Option<&str>,
    expected_integrity: &Integrity,
) -> ControlProtocolV1 {
    let config = root.join(format!("gateway-schema{expected_version}.json"));
    create_private_file(
        &config,
        &serde_json::to_vec(&json!({
            "version":1,
            "tenants":[{
                "tenant_id":TENANT,"data":database,
                "runner_token_file":"runner","viewer_token_file":"viewer"
            }]
        }))
        .unwrap(),
    );
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root)
        .env("TMPDIR", root)
        .current_dir(root)
        .args([
            "gateway",
            "--listen",
            "127.0.0.1:0",
            "--max-events",
            "10000",
        ])
        .arg("--tenants-file")
        .arg(config)
        .arg("--policy")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("policies/default.json"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start the delivered gateway only on the owned fixture database");
    let mut stderr = tokio::io::BufReader::new(child.stderr.take().unwrap());
    let mut ready = String::new();
    timeout(Duration::from_secs(15), stderr.read_line(&mut ready))
        .await
        .expect("gateway readiness deadline")
        .unwrap();
    let address: SocketAddr = ready
        .strip_prefix("Gateway ")
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or_else(|| panic!("schema{expected_version} gateway did not report readiness"))
        .parse()
        .unwrap();
    assert_eq!(address.ip(), Ipv4Addr::LOCALHOST);
    assert_ne!(address.port(), 0);
    let origin = format!("http://{address}/api/v1");
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let get = |route, token| http.get(format!("{origin}/{route}")).bearer_auth(token);
    let runner: ControlProtocolV1 = read_json(get("control/protocol", RUNNER)).await;
    runner.validate().unwrap();
    assert_eq!(runner.schema_version, expected_version);
    assert_eq!(runner.tenant_id, TENANT);
    assert_eq!(runner.role, "runner");
    assert_eq!(runner.control_namespace_uuid.as_deref(), expected_namespace);
    assert_eq!(
        runner.protocols,
        [SupervisionProtocolV1::BrokerSupervisionV1]
    );
    assert!(!runner.active);

    let viewer: ControlProtocolV1 = read_json(get("control/protocol", VIEWER)).await;
    viewer.validate().unwrap();
    assert_eq!(viewer.schema_version, expected_version);
    assert_eq!(viewer.tenant_id, TENANT);
    assert_eq!(viewer.role, "viewer");
    assert_eq!(viewer.control_namespace_uuid.as_deref(), expected_namespace);
    assert!(!viewer.active);
    let identity: Value = read_json(get("identity", VIEWER)).await;
    assert_eq!(identity, json!({"tenant_id":TENANT,"role":"viewer"}));
    let forbidden = http
        .post(format!("{origin}/workers/supervised-reservations"))
        .bearer_auth(VIEWER)
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), reqwest::StatusCode::FORBIDDEN);
    let integrity: Value = read_json(get("integrity", VIEWER)).await;
    let mut expected = serde_json::to_value(expected_integrity).unwrap();
    expected["stored_events"] = json!(expected_integrity.event_count);
    expected["retention"] = json!({
        "capability": false,
        "segments": 0,
        "archived_through_seq": 0,
        "reclaimed_events": 0,
        "reclaimed_bytes": 0,
        "last": null,
        "segment_list": [],
    });
    assert_eq!(integrity, expected);
    timeout(Duration::from_secs(5), child.kill())
        .await
        .expect("fixture gateway exit deadline")
        .unwrap();
    runner
}

#[tokio::test]
async fn cli_schema13_and_the_served_protocol_keep_the_same_inactive_namespace() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    create_private_file(&root.path().join("runner"), RUNNER.as_bytes());
    create_private_file(&root.path().join("viewer"), VIEWER.as_bytes());
    let database = root.path().join("tenant.sqlite");
    prepare_schema12(&database);
    let source12 =
        Store::open_existing_for_tenant(&database, TENANT, Redactor::new(vec![]), 10_000).unwrap();
    let before = source12.integrity().unwrap();
    let state12 = source12.supervision_state().unwrap();
    assert_eq!(state12.schema_version, 12);
    assert!(!state12.active);
    drop(source12);
    let backup = root.path().join("before-recovery13.sqlite");
    let output = migration_cli(root.path(), &database, &backup);
    assert!(
        output.status.success(),
        "schema13 CLI migration failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: MigrationReportV1 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!((report.from_version, report.to_version), (12, 13));
    let backup_bytes = fs::read(&backup).unwrap();
    let backup_identity = fs::metadata(&backup).unwrap();
    assert_eq!(backup_identity.mode() & 0o777, 0o600);
    let repeated = migration_cli(root.path(), &database, &backup);
    assert!(
        !repeated.status.success(),
        "explicit migration must not silently reapply"
    );
    assert_eq!(fs::read(&backup).unwrap(), backup_bytes);
    let retained = fs::metadata(&backup).unwrap();
    assert_eq!(
        (backup_identity.dev(), backup_identity.ino()),
        (retained.dev(), retained.ino())
    );

    let store =
        Store::open_existing_for_tenant(&database, TENANT, Redactor::new(vec![]), 10_000).unwrap();
    let integrity = store.integrity().unwrap();
    assert!(integrity.valid);
    assert_eq!(integrity.event_count, before.event_count + 1);
    assert_eq!(integrity.head_hash, report.genesis_hash);
    let state = store.supervision_state().unwrap();
    assert_eq!(state.schema_version, 13);
    assert_eq!(state.control_namespace_uuid, state12.control_namespace_uuid);
    assert!(!state.active);
    drop(store);

    let upgraded = read_gateway(
        root.path(),
        &database,
        13,
        state12.control_namespace_uuid.as_deref(),
        &integrity,
    )
    .await;
    let rollback = root.path().join("rollback");
    fs::DirBuilder::new().mode(0o700).create(&rollback).unwrap();
    let restored_database = rollback.join("tenant.sqlite");
    create_private_file(&restored_database, &backup_bytes);
    let restored = read_gateway(
        root.path(),
        &restored_database,
        12,
        state12.control_namespace_uuid.as_deref(),
        &before,
    )
    .await;

    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    assert!(
        schema["properties"]["schema_version"]["enum"]
            .as_array()
            .unwrap()
            .contains(&Value::from(13))
    );
    assert!(schema["allOf"].as_array().unwrap().iter().any(|rule| {
        rule["if"]["properties"]["active"]["const"] == true
            && rule["then"]["properties"]["schema_version"]["enum"]
                .as_array()
                .is_some_and(|values| values.contains(&Value::from(13)))
    }));
    assert_eq!(fs::read(&backup).unwrap(), backup_bytes);
    let retained = fs::metadata(&backup).unwrap();
    assert_eq!(
        (retained.dev(), retained.ino()),
        (backup_identity.dev(), backup_identity.ino())
    );
    let store =
        Store::open_existing_for_tenant(&database, TENANT, Redactor::new(vec![]), 10_000).unwrap();
    let after_reads = store.integrity().unwrap();
    assert!(after_reads.valid);
    assert_eq!(after_reads.event_count, integrity.event_count);
    assert_eq!(after_reads.head_hash, integrity.head_hash);
    println!(
        "{}",
        json!({
            "tenant":TENANT,
            "control_namespace_uuid":upgraded.control_namespace_uuid,
            "upgraded":{"schema_version":upgraded.schema_version,"active":upgraded.active,
                "event_count":integrity.event_count,"head_hash":integrity.head_hash},
            "restored_copy":{"schema_version":restored.schema_version,"active":restored.active,
                "event_count":before.event_count,"head_hash":before.head_hash},
            "backup_unchanged":true,
        })
    );
}
