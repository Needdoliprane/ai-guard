//! Checks that the served control-protocol versions are represented by the
//! delivered schema's version enum and active-version constraint.
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    control::{ControlProtocolV1, SupervisionProtocolV1},
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

        for version in 3..=7 {
            let backup = directory
                .path()
                .join(format!("tenant-before-v{version}.sqlite"));
            migrate_to(&store, &backup, version);
            let protocol = get_protocol(&router, &runner_token).await;
            check_protocol_and_schema(&protocol, &schema, version, true, &mut failures);
        }
    }

    // The exact backup taken before7 remains a historical schema6 reader. It
    // must reopen inactive instead of inheriting the source's live lease.
    let historical6 = Arc::new(
        Store::open_existing_for_tenant(
            &directory.path().join("tenant-before-v7.sqlite"),
            "schema-contract-tenant",
            Redactor::new(vec![]),
            10_000,
        )
        .unwrap(),
    );
    let historical6_router = gateway_router(
        GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "schema-contract-tenant".into(),
                store: Arc::clone(&historical6),
                runner_token: runner_token.clone(),
                viewer_token: "historical-six-viewer".repeat(3),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let protocol = get_protocol(&historical6_router, &runner_token).await;
    check_protocol_and_schema(&protocol, &schema, 6, false, &mut failures);

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
    assert_eq!(reopened.supervision_state().unwrap().schema_version, 7);
    assert!(!reopened.supervision_state().unwrap().active);
    let protocol = get_protocol(&reopened_router, &runner_token).await;
    check_protocol_and_schema(&protocol, &schema, 7, false, &mut failures);

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
