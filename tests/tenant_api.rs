mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    identity::{
        HumanAccountV1, HumanCapabilityV1, IdentityChangeV1, IdentityMutationV1,
        LegacyControlRoleV1, LocalCredentialV1, LocalHumanProvisionV1, MembershipV1,
        PrincipalRefV1, generate_local_credential,
    },
    model::Event,
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};
use tower::ServiceExt;

fn tenant(path: &std::path::Path, id: &str, runner: &str, viewer: &str) -> TenantGateway {
    TenantGateway {
        tenant_id: id.into(),
        store: Arc::new(Store::open_for_tenant(path, id, Redactor::new(vec![]), 100).unwrap()),
        runner_token: runner.repeat(48),
        viewer_token: viewer.repeat(48),
    }
}
fn request(method: &str, path: &str, token: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {}", token.repeat(48)))
        .header("content-type", "application/json")
        .header("x-tenant-id", "tenant-b")
        .body(Body::from(body.to_string()))
        .unwrap()
}
async fn value(app: &Router, path: &str, token: &str) -> Value {
    let response = app
        .clone()
        .oneshot(request("GET", path, token, Value::Null))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn authenticated_identity_scopes_ingestion_history_export_and_derived_views() {
    let directory = support::private_tempdir();
    let state = GatewayState::for_tenants(
        vec![
            tenant(&directory.path().join("a.sqlite"), "tenant-a", "a", "v"),
            tenant(&directory.path().join("b.sqlite"), "tenant-b", "b", "w"),
        ],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let app = gateway_router(state);
    let mut event = Event::new("tenant.a");
    event.agent_id = Some("agent-a".into());
    event.sandbox_id = Some("sandbox-a".into());
    event.data = json!({"tenant_id":"tenant-b"});
    let response = app
        .clone()
        .oneshot(request("POST", "/api/events", "a", json!(event)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    for path in ["/api/events?tenant_id=tenant-b", "/api/v1/events"] {
        assert_eq!(
            value(&app, path, "v").await["items"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let tenant_b = value(&app, path, "w").await;
        assert_eq!(tenant_b["items"].as_array().unwrap().len(), 1);
        assert_eq!(tenant_b["items"][0]["kind"], "control.schema.genesis");
    }
    assert_eq!(
        value(&app, "/api/v1/identity?tenant_id=tenant-b", "v").await,
        json!({"tenant_id":"tenant-a","role":"viewer"})
    );
    assert_eq!(
        value(&app, "/api/identity", "b").await,
        json!({"tenant_id":"tenant-b","role":"runner"})
    );
    let tenant_b_export = value(&app, "/api/export?format=json", "w").await;
    assert_eq!(tenant_b_export.as_array().unwrap().len(), 1);
    assert_eq!(tenant_b_export[0]["kind"], "control.schema.genesis");
    assert_eq!(value(&app, "/api/integrity", "v").await["event_count"], 2);
    assert_eq!(value(&app, "/api/integrity", "w").await["event_count"], 1);
    for path in ["/api/agents", "/api/sandboxes"] {
        assert_eq!(
            value(&app, path, "v").await["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(value(&app, path, "w").await["items"], json!([]));
    }
    let response = app
        .oneshot(request(
            "POST",
            "/api/events",
            "v",
            json!(Event::new("test.event")),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn decision_and_live_stream_are_isolated_and_audit_failure_denies_only_affected_tenant() {
    let directory = support::private_tempdir();
    let a = tenant(&directory.path().join("a.sqlite"), "tenant-a", "a", "v");
    let b = tenant(&directory.path().join("b.sqlite"), "tenant-b", "b", "w");
    let a_store = a.store.clone();
    let app = gateway_router(
        GatewayState::for_tenants(vec![a, b], Policy::default(), None, Redactor::new(vec![]))
            .unwrap(),
    );
    let body = json!({"request_id":uuid::Uuid::new_v4(),"profile":"strict","command":{"argv":["id"],"cwd":"/workspace"}});
    let response = app
        .clone()
        .oneshot(request("POST", "/api/v1/decide", "a", body.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let b_events = value(&app, "/api/events", "w").await;
    assert_eq!(b_events["items"].as_array().unwrap().len(), 1);
    assert_eq!(b_events["items"][0]["kind"], "control.schema.genesis");
    let response = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/stream?after=1&tenant_id=tenant-b",
            "v",
            Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    use futures_util::StreamExt;
    let chunk = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8(chunk.to_vec()).unwrap();
    assert!(text.contains("security.decision"));
    let response = app
        .clone()
        .oneshot(request("GET", "/api/stream?after=1", "w", Value::Null))
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), stream.next())
            .await
            .is_err()
    );
    let mut spoof = body.clone();
    spoof["tenant_id"] = json!("tenant-b");
    assert_eq!(
        app.clone()
            .oneshot(request("POST", "/api/decide", "a", spoof))
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let db = rusqlite::Connection::open(directory.path().join("a.sqlite")).unwrap();
    db.execute("UPDATE events SET hash='tampered'", []).unwrap();
    assert!(!a_store.integrity().unwrap().valid);
    assert_eq!(
        app.clone()
            .oneshot(request("POST", "/api/decide", "a", body.clone()))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.oneshot(request("POST", "/api/decide", "b", body))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn local_human_authentication_survives_neighbor_lock_but_denies_own_store_lock() {
    let directory = support::private_tempdir();
    let a_path = directory.path().join("a.sqlite");
    let b_path = directory.path().join("b.sqlite");
    let a = tenant(&a_path, "tenant-a", "a", "v");
    let b = tenant(&b_path, "tenant-b", "b", "w");
    for (path, store) in [(&a_path, &a.store), (&b_path, &b.store)] {
        store
            .migrate_control_v3(&path.with_extension("before-v3"))
            .unwrap();
        store
            .migrate_identity_v4(&path.with_extension("before-v4"))
            .unwrap();
        store
            .migrate_human_access_v5(&path.with_extension("before-v5"))
            .unwrap();
    }
    let human_token = generate_local_credential().unwrap();
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: "290a1a3b-8538-49ef-a830-25df6fa19030".into(),
    };
    let mutation = |change| IdentityMutationV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "fixture-control-operator".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    };
    b.store
        .provision_local_human(&LocalHumanProvisionV1 {
            version: 1,
            mutations: [
                mutation(IdentityChangeV1::Account {
                    expected_revision: 0,
                    account: HumanAccountV1 {
                        version: 1,
                        principal: principal.clone(),
                        display_name: "Tenant B reader".into(),
                        revision: 1,
                        active: true,
                    },
                }),
                mutation(IdentityChangeV1::Credential {
                    expected_revision: 0,
                    binding: LocalCredentialV1 {
                        version: 1,
                        credential_id: uuid::Uuid::new_v4().to_string(),
                        principal: principal.clone(),
                        sha256_digest: hex::encode(Sha256::digest(human_token.as_bytes())),
                        revision: 1,
                        active: true,
                        expires_at: None,
                    },
                }),
                mutation(IdentityChangeV1::Membership {
                    expected_revision: 0,
                    membership: MembershipV1 {
                        version: 1,
                        membership_id: uuid::Uuid::new_v4().to_string(),
                        principal: principal.clone(),
                        tenant_id: "tenant-b".into(),
                        organization_scope: "organization-b".into(),
                        revision: 1,
                        active: true,
                        capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                    },
                }),
            ],
        })
        .unwrap();
    let app = gateway_router(
        GatewayState::for_tenants(vec![a, b], Policy::default(), None, Redactor::new(vec![]))
            .unwrap(),
    );
    let human_request = || {
        Request::builder()
            .uri("/api/v1/human/identity?tenant_id=tenant-a")
            .header("authorization", format!("Bearer {human_token}"))
            .header("x-tenant-id", "tenant-a")
            .body(Body::empty())
            .unwrap()
    };
    let response = app.clone().oneshot(human_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let identity: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(identity["contexts_complete"], true);

    // A real SQLite write lock affects only A. No tenant hint may replace the
    // tenant selected from B's authenticated local credential and memberships.
    let mut locked_a = rusqlite::Connection::open(&a_path).unwrap();
    let a_lock = locked_a
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let response = app.clone().oneshot(human_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let identity: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(identity["principal"], json!(principal));
    assert_eq!(identity["authentication"]["method"], "local_credential");
    assert_eq!(identity["contexts_complete"], false);
    assert_eq!(identity["contexts"].as_array().unwrap().len(), 1);
    assert_eq!(identity["contexts"][0]["tenant_id"], "tenant-b");
    a_lock.rollback().unwrap();

    // The missing contexts were unknown during the neighboring fault, not
    // revoked. Once A can be checked again, the complete projection recovers.
    let response = app.clone().oneshot(human_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let identity: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(identity["contexts_complete"], true);
    assert_eq!(identity["contexts"].as_array().unwrap().len(), 1);
    assert_eq!(identity["contexts"][0]["tenant_id"], "tenant-b");

    // B's mandatory identity/read audit must remain fail-closed when B itself
    // is locked; an unavailable store is never an empty authenticated profile.
    let mut locked_b = rusqlite::Connection::open(&b_path).unwrap();
    let b_lock = locked_b
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let response = app.oneshot(human_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let error: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(error.get("contexts").is_none());
    assert!(error.get("principal").is_none());
    b_lock.rollback().unwrap();
}

#[test]
fn ambiguous_credentials_and_wrong_database_binding_are_rejected() {
    let directory = support::private_tempdir();
    let a = tenant(&directory.path().join("a.sqlite"), "tenant-a", "a", "v");
    let mut b = tenant(&directory.path().join("b.sqlite"), "tenant-b", "b", "w");
    b.viewer_token = a.runner_token.clone();
    assert!(
        GatewayState::for_tenants(vec![a, b], Policy::default(), None, Redactor::new(vec![]))
            .is_err()
    );
    let mut wrong = tenant(&directory.path().join("c.sqlite"), "tenant-c", "c", "x");
    wrong.tenant_id = "tenant-d".into();
    assert!(
        GatewayState::for_tenants(vec![wrong], Policy::default(), None, Redactor::new(vec![]))
            .is_err()
    );
    let a = tenant(&directory.path().join("a.sqlite"), "tenant-a", "a", "v");
    let mut duplicate = tenant(&directory.path().join("e.sqlite"), "tenant-e", "e", "y");
    duplicate.tenant_id = "tenant-a".into();
    assert!(
        GatewayState::for_tenants(
            vec![a, duplicate],
            Policy::default(),
            None,
            Redactor::new(vec![])
        )
        .is_err()
    );
    let mut empty = tenant(&directory.path().join("f.sqlite"), "tenant-f", "f", "z");
    empty.runner_token.clear();
    assert!(
        GatewayState::for_tenants(vec![empty], Policy::default(), None, Redactor::new(vec![]))
            .is_err()
    );
    assert!(
        GatewayState::for_tenants(vec![], Policy::default(), None, Redactor::new(vec![])).is_err()
    );
}
