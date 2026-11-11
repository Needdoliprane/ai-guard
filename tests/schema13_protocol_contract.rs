//! Actual offline CLI migration and gateway protocol, with synthetic tenant
//! configuration only. No model, worker runtime or external network is used.
use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    control::{ControlProtocolV1, MigrationReportV1, SupervisionProtocolV1},
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    process::{Command, Output, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
use tower::ServiceExt;

const TENANT: &str = "schema13-contract-tenant";
const SCHEMA: &str = include_str!("../schemas/supervision-protocol.schema.json");

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

#[tokio::test]
async fn cli_schema13_and_the_served_protocol_keep_the_same_inactive_namespace() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
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
    let preserved =
        Store::open_existing_for_tenant(&backup, TENANT, Redactor::new(vec![]), 10_000).unwrap();
    let preserved_integrity = preserved.integrity().unwrap();
    assert!(preserved_integrity.valid);
    assert_eq!(preserved_integrity.event_count, before.event_count);
    assert_eq!(preserved_integrity.head_hash, before.head_hash);
    assert_eq!(preserved.supervision_state().unwrap().schema_version, 12);
    assert!(!preserved.supervision_state().unwrap().active);
    drop(preserved);
    let backup_bytes = fs::read(&backup).unwrap();
    let backup_identity = fs::metadata(&backup).unwrap();
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

    let store = Arc::new(
        Store::open_existing_for_tenant(&database, TENANT, Redactor::new(vec![]), 10_000).unwrap(),
    );
    let integrity = store.integrity().unwrap();
    assert!(integrity.valid);
    assert_eq!(integrity.event_count, before.event_count + 1);
    assert_eq!(integrity.head_hash, report.genesis_hash);
    let state = store.supervision_state().unwrap();
    assert_eq!(state.schema_version, 13);
    assert_eq!(state.control_namespace_uuid, state12.control_namespace_uuid);
    assert!(!state.active);
    let runner_token = "schema13-contract-runner-fixture".repeat(2);
    let viewer_token = "schema13-contract-viewer-fixture".repeat(2);
    let router = gateway_router(
        GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: TENANT.into(),
                store: Arc::clone(&store),
                runner_token: runner_token.clone(),
                viewer_token: viewer_token.clone(),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let request = Request::get("/api/v1/control/protocol")
        .header(header::AUTHORIZATION, format!("Bearer {runner_token}"))
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    let protocol: ControlProtocolV1 = serde_json::from_slice(&bytes).unwrap();
    protocol.validate().unwrap();
    assert_eq!(protocol.schema_version, 13);
    assert_eq!(protocol.tenant_id, TENANT);
    assert_eq!(protocol.role, "runner");
    assert_eq!(
        protocol.control_namespace_uuid,
        state12.control_namespace_uuid
    );
    assert_eq!(
        protocol.protocols,
        vec![SupervisionProtocolV1::BrokerSupervisionV1]
    );
    assert!(!protocol.active);
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
    let viewer_request = Request::get("/api/v1/control/protocol")
        .header(header::AUTHORIZATION, format!("Bearer {viewer_token}"))
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(viewer_request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    let viewer: ControlProtocolV1 = serde_json::from_slice(&bytes).unwrap();
    viewer.validate().unwrap();
    assert_eq!(viewer.schema_version, 13);
    assert_eq!(viewer.role, "viewer");
    assert_eq!(viewer.tenant_id, TENANT);
    let forbidden = Request::post("/api/v1/workers/supervised-reservations")
        .header(header::AUTHORIZATION, format!("Bearer {viewer_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let response = router.oneshot(forbidden).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let after_reads = store.integrity().unwrap();
    assert!(after_reads.valid);
    assert_eq!(after_reads.event_count, integrity.event_count);
    assert_eq!(after_reads.head_hash, integrity.head_hash);
}
