//! Checks that the served control-protocol versions are represented by the
//! delivered schema's version enum and active-version constraint.
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    control::{ControlProtocolV1, MigrationReportV1, SupervisionProtocolV1},
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::Value;
use std::{path::Path, sync::Arc};
use tower::ServiceExt;

const CONTROL_PROTOCOL_SCHEMA: &str = include_str!("../schemas/supervision-protocol.schema.json");

async fn get_protocol(router: &Router, runner_token: &str) -> ControlProtocolV1 {
    let request = Request::get("/api/v1/control/protocol")
        .header(header::AUTHORIZATION, format!("Bearer {runner_token}"))
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&body).expect("control protocol response is a DTO")
}

fn check_protocol_and_schema(
    protocol: &ControlProtocolV1,
    schema: &Value,
    expected_version: u32,
    expected_active: bool,
    failures: &mut Vec<String>,
) {
    let label = format!("schema {} active={}", expected_version, expected_active);
    if protocol.version != 1
        || protocol.schema_version != expected_version
        || protocol.tenant_id != "schema-contract-tenant"
        || protocol.role != "runner"
        || protocol.active != expected_active
    {
        failures.push(format!("{label}: unexpected served DTO {protocol:?}"));
    }
    if let Err(error) = protocol.validate() {
        failures.push(format!(
            "{label}: served DTO failed its Rust contract: {error}"
        ));
    }

    let expected_protocols = if expected_version >= 3 {
        vec![SupervisionProtocolV1::BrokerSupervisionV1]
    } else {
        vec![]
    };
    if protocol.protocols != expected_protocols {
        failures.push(format!("{label}: unexpected protocol capabilities"));
    }

    let enum_contains_version = schema
        .pointer("/properties/schema_version/enum")
        .and_then(Value::as_array)
        .is_some_and(|values| {
            values
                .iter()
                .any(|value| value.as_u64() == Some(u64::from(expected_version)))
        });
    if !enum_contains_version {
        failures.push(format!(
            "{label}: delivered schema enum does not list served schema_version {expected_version}"
        ));
    }

    if expected_active {
        let active_version_constraints = schema
            .get("allOf")
            .and_then(Value::as_array)
            .map(|rules| {
                rules
                    .iter()
                    .filter(|rule| {
                        rule.pointer("/if/properties/active/const")
                            .and_then(Value::as_bool)
                            == Some(true)
                    })
                    .filter_map(|rule| rule.pointer("/then/properties/schema_version"))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut recognized_constraint = false;
        let mut constraints_match_version = true;
        for constraint in &active_version_constraints {
            if let Some(value) = constraint.get("const") {
                recognized_constraint = true;
                constraints_match_version &= value.as_u64() == Some(u64::from(expected_version));
            }
            if let Some(values) = constraint.get("enum") {
                recognized_constraint = true;
                constraints_match_version &= values.as_array().is_some_and(|values| {
                    values
                        .iter()
                        .any(|value| value.as_u64() == Some(u64::from(expected_version)))
                });
            }
        }
        if !recognized_constraint || !constraints_match_version {
            failures.push(format!(
                "{label}: delivered active schema_version constraint does not admit {expected_version}"
            ));
        }
    }
}

fn migrate_to(store: &Store, backup: &Path, version: u32) {
    assert!(
        !backup.exists(),
        "migration backup must start fresh: {backup:?}"
    );
    match version {
        3 => store.migrate_control_v3(backup).unwrap(),
        4 => store.migrate_identity_v4(backup).unwrap(),
        5 => store.migrate_human_access_v5(backup).unwrap(),
        6 => store.migrate_workload_v6(backup).unwrap(),
        7 => store.migrate_human_brokers_v7(backup).unwrap(),
        8 => store.migrate_kubernetes_create_v8(backup).unwrap(),
        9 => store.migrate_kubernetes_policy_v9(backup).unwrap(),
        10 => store.migrate_human_workers_v10(backup).unwrap(),
        11 => store
            .migrate_kubernetes_session_reservations_v11(backup)
            .unwrap(),
        12 => store.migrate_broker_checkpoints_v12(backup).unwrap(),
        _ => panic!("unexpected migration target {version}"),
    };
    assert!(
        backup.is_file(),
        "migration backup was not created: {backup:?}"
    );
}

#[tokio::test]
async fn served_control_protocol_versions_match_schema_enum_and_active_constraint() {
    let directory = support::private_tempdir();
    let database = directory.path().join("tenant.sqlite");
    let runner_token = "schema-contract-runner".repeat(3);
    let schema: Value = serde_json::from_str(CONTROL_PROTOCOL_SCHEMA)
        .expect("delivered protocol schema is valid JSON");
    let mut failures = Vec::new();

    {
        let store = Arc::new(
            Store::open_for_tenant(
                &database,
                "schema-contract-tenant",
                Redactor::new(vec![]),
                10_000,
            )
            .unwrap(),
        );
        assert_eq!(store.supervision_state().unwrap().schema_version, 2);
        let state = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "schema-contract-tenant".into(),
                store: Arc::clone(&store),
                runner_token: runner_token.clone(),
                viewer_token: "schema-contract-viewer".repeat(3),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap();
        let router = gateway_router(state);

        let protocol = get_protocol(&router, &runner_token).await;
        check_protocol_and_schema(&protocol, &schema, 2, false, &mut failures);

        for version in 3..=9 {
            let backup = directory
                .path()
                .join(format!("tenant-before-v{version}.sqlite"));
            migrate_to(&store, &backup, version);
            let protocol = get_protocol(&router, &runner_token).await;
            check_protocol_and_schema(&protocol, &schema, version, true, &mut failures);
        }
    }

    // Exact pre-migration backups remain historical readers. Historical readers
    // never inherit the source's live lease on reopen.
    for version in [6, 7, 8] {
        let historical = Arc::new(
            Store::open_existing_for_tenant(
                &directory
                    .path()
                    .join(format!("tenant-before-v{}.sqlite", version + 1)),
                "schema-contract-tenant",
                Redactor::new(vec![]),
                10_000,
            )
            .unwrap(),
        );
        let historical_router = gateway_router(
            GatewayState::for_tenants(
                vec![TenantGateway {
                    tenant_id: "schema-contract-tenant".into(),
                    store: Arc::clone(&historical),
                    runner_token: runner_token.clone(),
                    viewer_token: format!("historical-v{version}-viewer").repeat(3),
                }],
                Policy::default(),
                None,
                Redactor::new(vec![]),
            )
            .unwrap(),
        );
        let protocol = get_protocol(&historical_router, &runner_token).await;
        check_protocol_and_schema(&protocol, &schema, version, false, &mut failures);
    }

    let reopened = Arc::new(
        Store::open_for_tenant(
            &database,
            "schema-contract-tenant",
            Redactor::new(vec![]),
            10_000,
        )
        .unwrap(),
    );
    let reopened_state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "schema-contract-tenant".into(),
            store: Arc::clone(&reopened),
            runner_token: runner_token.clone(),
            viewer_token: "schema-contract-viewer".repeat(3),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let reopened_router = gateway_router(reopened_state);
    assert_eq!(reopened.supervision_state().unwrap().schema_version, 9);
    assert!(!reopened.supervision_state().unwrap().active);
    let protocol = get_protocol(&reopened_router, &runner_token).await;
    check_protocol_and_schema(&protocol, &schema, 9, false, &mut failures);

    // Exercise the delivered CLI on a disposable COPY of the real schema8
    // backup; the original backup remains available for historical readers.
    let cli_data = directory.path().join("cli-schema8-copy.sqlite");
    let cli_backup = directory.path().join("cli-before9.sqlite");
    let source_backup = directory.path().join("tenant-before-v9.sqlite");
    let source_bytes = std::fs::read(&source_backup).unwrap();
    std::fs::copy(&source_backup, &cli_data).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", directory.path())
        .env("TMPDIR", directory.path())
        .args(["migrate-control", "--data"])
        .arg(&cli_data)
        .args(["--tenant-id", "schema-contract-tenant", "--backup"])
        .arg(&cli_backup)
        .args([
            "--target-version",
            "9",
            "--confirm-control-stopped",
            "--max-events",
            "10000",
        ])
        .output()
        .expect("invoke the delivered CLI on private fixture data");
    assert!(output.status.success(), "schema9 CLI migration failed");
    let report: MigrationReportV1 =
        serde_json::from_slice(&output.stdout).expect("CLI emits a typed migration report");
    assert_eq!((report.from_version, report.to_version), (8, 9));
    assert!(cli_backup.is_file());
    assert_eq!(std::fs::read(&source_backup).unwrap(), source_bytes);
    let cli_store = Store::open_existing_for_tenant(
        &cli_data,
        "schema-contract-tenant",
        Redactor::new(vec![]),
        10_000,
    )
    .unwrap();
    let cli_integrity = cli_store.integrity().unwrap();
    assert!(cli_integrity.valid);
    assert_eq!(cli_integrity.head_hash, report.genesis_hash);
    let cli_control = cli_store.supervision_state().unwrap();
    assert_eq!(cli_control.schema_version, 9);
    assert!(!cli_control.active);
    assert_eq!(
        cli_control.control_namespace_uuid,
        protocol.control_namespace_uuid
    );

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn human_worker_feature_is_advertised_only_after_the_explicit10_migration() {
    let files = support::private_tempdir();
    let store = Arc::new(
        Store::open_for_tenant(
            &files.path().join("tenant.sqlite"),
            "schema-contract-tenant",
            Redactor::new(vec![]),
            10_000,
        )
        .unwrap(),
    );
    for version in 3..=9 {
        migrate_to(
            &store,
            &files.path().join(format!("before{version}")),
            version,
        );
    }
    let token = "worker-schema-contract-runner".repeat(3);
    let router = gateway_router(
        GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "schema-contract-tenant".into(),
                store: store.clone(),
                runner_token: token.clone(),
                viewer_token: "worker-schema-contract-viewer".repeat(3),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let schema: Value = serde_json::from_str(CONTROL_PROTOCOL_SCHEMA).unwrap();
    let mut failures = vec![];
    check_protocol_and_schema(
        &get_protocol(&router, &token).await,
        &schema,
        9,
        true,
        &mut failures,
    );
    migrate_to(&store, &files.path().join("before10"), 10);
    check_protocol_and_schema(
        &get_protocol(&router, &token).await,
        &schema,
        10,
        true,
        &mut failures,
    );
    assert!(store.integrity().unwrap().valid);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn schema11_and12_protocols_match_active_and_inactive_schema_constraints_and_reject15() {
    // A delivered schema that rejects the real v11/v12 responses must fail
    // here even when those DTOs satisfy the Rust contract. No schema validator
    // dependency or fabricated Store/HTTP response replaces the real route.
    let files = support::private_tempdir();
    let database = files.path().join("tenant.sqlite");
    let runner = "schema12-contract-runner".repeat(3);
    let viewer = "schema12-contract-viewer".repeat(3);
    let redactor = || Redactor::new(vec![runner.clone(), viewer.clone()]);
    let router_for = |store: Arc<Store>| {
        gateway_router(
            GatewayState::for_tenants(
                vec![TenantGateway {
                    tenant_id: "schema-contract-tenant".into(),
                    store,
                    runner_token: runner.clone(),
                    viewer_token: viewer.clone(),
                }],
                Policy::default(),
                None,
                redactor(),
            )
            .unwrap(),
        )
    };
    let schema: Value = serde_json::from_str(CONTROL_PROTOCOL_SCHEMA).unwrap();
    let mut failures = Vec::new();
    let last_protocol = {
        let store = Arc::new(
            Store::open_for_tenant(&database, "schema-contract-tenant", redactor(), 10_000)
                .unwrap(),
        );
        let router = router_for(Arc::clone(&store));
        let mut last_protocol = None;
        for version in 3..=12 {
            migrate_to(
                &store,
                &files.path().join(format!("before{version}.sqlite")),
                version,
            );
            if version >= 11 {
                assert!(store.integrity().unwrap().valid);
                let protocol = get_protocol(&router, &runner).await;
                assert_eq!(protocol.schema_version, version);
                assert!(protocol.active, "same fresh-origin lease stays active");
                protocol.validate().unwrap();
                check_protocol_and_schema(&protocol, &schema, version, true, &mut failures);
                last_protocol = Some(protocol);
            }
        }
        last_protocol.unwrap()
        // Both the router and every live Store Arc drop before reopen below.
    };
    assert!(last_protocol.control_namespace_uuid.is_some());
    for (version, path) in [
        (11, files.path().join("before12.sqlite")),
        (12, database.clone()),
    ] {
        let historical = Arc::new(
            Store::open_existing_for_tenant(&path, "schema-contract-tenant", redactor(), 10_000)
                .unwrap(),
        );
        assert!(historical.integrity().unwrap().valid);
        let router = router_for(Arc::clone(&historical));
        let protocol = get_protocol(&router, &runner).await;
        assert_eq!(protocol.schema_version, version);
        assert!(!protocol.active, "reopen cannot inherit the active origin");
        assert_eq!(
            protocol.control_namespace_uuid,
            last_protocol.control_namespace_uuid
        );
        protocol.validate().unwrap();
        check_protocol_and_schema(&protocol, &schema, version, false, &mut failures);
    }

    // This negative changes only the version/active fields of a valid served
    // DTO. It tests public Rust validation, not a claim that Store serves v15.
    for active in [false, true] {
        let mut future = last_protocol.clone();
        future.schema_version = 15;
        future.active = active;
        assert!(
            future.validate().is_err(),
            "schema15 must remain unsupported"
        );
    }
    for pointer in [
        "/properties/schema_version/enum",
        "/allOf/0/then/properties/schema_version/enum",
    ] {
        let allowed = schema.pointer(pointer).and_then(Value::as_array).unwrap();
        assert!(
            !allowed.iter().any(|value| value.as_u64() == Some(15)),
            "delivered schema must reject15 at {pointer}"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn schema14_descriptive_dto_and_published_json_contract_agree() {
    let schema: Value = serde_json::from_str(CONTROL_PROTOCOL_SCHEMA).unwrap();
    let protocol = ControlProtocolV1 {
        version: 1,
        schema_version: 14,
        control_namespace_uuid: Some("00000000-0000-0000-0000-000000000001".into()),
        tenant_id: "schema-contract-tenant".into(),
        role: "runner".into(),
        protocols: vec![SupervisionProtocolV1::BrokerSupervisionV1],
        active: false,
    };
    protocol.validate().unwrap();
    assert!(!protocol.active);
    assert_eq!(
        schema["properties"]["schema_version"]["enum"],
        serde_json::json!((1..=14).collect::<Vec<_>>())
    );
    let rules = schema["allOf"].as_array().unwrap();
    let supervised = rules
        .iter()
        .find(|rule| {
            rule["if"]["properties"]["schema_version"]["enum"]
                == serde_json::json!((3..=14).collect::<Vec<_>>())
        })
        .expect("every supervised metadata generation has the same shape, even inactive");
    assert_eq!(
        supervised["then"]["properties"]["control_namespace_uuid"]["type"],
        "string"
    );
    assert_eq!(
        supervised["then"]["properties"]["protocols"]["const"],
        serde_json::json!(["broker_supervision_v1"])
    );
    let legacy = rules
        .iter()
        .find(|rule| {
            rule["if"]["properties"]["schema_version"]["enum"] == serde_json::json!([1, 2])
        })
        .unwrap();
    assert_eq!(legacy["then"]["properties"]["active"]["const"], false);
    assert_eq!(
        legacy["then"]["properties"]["protocols"]["const"],
        serde_json::json!([])
    );
    let mut missing_namespace = protocol.clone();
    missing_namespace.control_namespace_uuid = None;
    assert!(missing_namespace.validate().is_err());
    let mut missing_protocol = protocol.clone();
    missing_protocol.protocols.clear();
    assert!(missing_protocol.validate().is_err());
    let mut future = protocol;
    future.schema_version = 15;
    assert!(future.validate().is_err());
    assert!(
        !schema["properties"]["schema_version"]["enum"]
            .as_array()
            .unwrap()
            .contains(&Value::from(15))
    );
}
