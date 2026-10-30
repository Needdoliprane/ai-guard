mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
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
use std::sync::Arc;
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
