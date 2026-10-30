mod support;

use agentsandbox::{
    api::{GatewayState, gateway_router},
    audit::Store,
    model::Event,
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;

fn app() -> (tempfile::TempDir, axum::Router) {
    let d = support::private_tempdir();
    let red = Redactor::new(vec![]);
    let store = Arc::new(Store::open(&d.path().join("events.sqlite"), red.clone(), 100).unwrap());
    let state = GatewayState::new(
        store,
        "r".repeat(48),
        "v".repeat(48),
        Policy::default(),
        None,
        red,
    );
    (d, gateway_router(state))
}
fn req(method: &str, path: &str, token: Option<&str>, body: String) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("Content-Type", "application/json");
    if let Some(t) = token {
        b = b.header("Authorization", format!("Bearer {t}"));
    }
    b.body(Body::from(body)).unwrap()
}
#[tokio::test]
async fn viewer_cannot_ingest_and_unknown_tokens_cannot_read() {
    let (_d, app) = app();
    assert_eq!(
        app.clone()
            .oneshot(req("GET", "/api/events", None, String::new()))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.clone()
            .oneshot(req(
                "POST",
                "/api/events",
                Some(&"v".repeat(48)),
                serde_json::to_string(&Event::new("test.event")).unwrap()
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.oneshot(req(
            "GET",
            "/api/events",
            Some(&"v".repeat(48)),
            String::new()
        ))
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
}
#[tokio::test]
async fn denied_command_is_audited_but_never_executed_by_api() {
    let (_d, app) = app();
    let id = uuid::Uuid::new_v4();
    let body = serde_json::json!({"request_id":id,"profile":"strict","command":{"argv":["sh","-c","touch impossible-marker"],"cwd":"/workspace"},"human_approved":false});
    let res = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/decide",
            Some(&"r".repeat(48)),
            body.to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["decision"]["outcome"], "deny");
    let res = app
        .oneshot(req(
            "GET",
            "/api/events",
            Some(&"v".repeat(48)),
            String::new(),
        ))
        .await
        .unwrap();
    let events: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(events["items"][0]["kind"], "security.decision");
    assert_eq!(events["items"].as_array().unwrap().len(), 2);
    assert_eq!(events["items"][1]["kind"], "control.schema.genesis");
}
#[tokio::test]
async fn json_limits_and_cross_origin_requests_are_rejected() {
    let (_d, app) = app();
    let mut r = req("GET", "/api/events", Some(&"v".repeat(48)), String::new());
    r.headers_mut()
        .insert("Origin", "https://attacker.invalid".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(r).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let large = format!("{{\"x\":\"{}\"}}", "x".repeat(300_000));
    assert_eq!(
        app.oneshot(req("POST", "/api/events", Some(&"r".repeat(48)), large))
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn export_preserves_requested_sequence_window() {
    let (_d, app) = app();
    for _ in 0..5 {
        let response = app
            .clone()
            .oneshot(req(
                "POST",
                "/api/events",
                Some(&"r".repeat(48)),
                serde_json::to_string(&Event::new("test.event")).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let response = app
        .oneshot(req(
            "GET",
            "/api/export?format=json&after=2&before=4",
            Some(&"v".repeat(48)),
            String::new(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 1);
    assert_eq!(value[0]["seq"], 3);
}
