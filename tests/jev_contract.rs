mod support;

use agentsandbox::{
    api::{GatewayState, gateway_router},
    audit::Store,
    jev::JevClient,
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode},
    routing::post,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[tokio::test]
async fn official_http_contract_and_jev_veto_are_enforced() {
    let captured = Arc::new(Mutex::new(None));
    let sink = captured.clone();
    let mock=Router::new().route("/v1/systemone",post(move|Json(body):Json<Value>|{let sink=sink.clone();async move{*sink.lock().unwrap()=Some(body);Json(json!({"model":"fixture-model","answers":{"execution_policy":{"type":"choice","choice":"deny","confidence":0.99,"probabilities":{"allow":0.0,"deny":0.99,"review":0.01}}},"usage":{"input_tokens":20,"output_tokens":2}}))}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback networking required for this test");
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });
    let jev = JevClient::new(
        &format!("http://{address}/v1/systemone"),
        "fixture-key".repeat(4),
        "fixture-model".into(),
        0.85,
        true,
    )
    .unwrap();
    let directory = support::private_tempdir();
    let red = Redactor::new(vec![]);
    let store =
        Arc::new(Store::open(&directory.path().join("audit.sqlite"), red.clone(), 100).unwrap());
    let app = gateway_router(GatewayState::new(
        store.clone(),
        "r".repeat(48),
        "v".repeat(48),
        Policy::default(),
        Some(jev),
        red,
    ));
    let body = json!({"request_id":uuid::Uuid::new_v4(),"profile":"strict","command":{"argv":["id"],"cwd":"/workspace"}});
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/decide")
                .header("Authorization", format!("Bearer {}", "r".repeat(48)))
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(result["decision"]["outcome"], "deny");
    assert_eq!(result["decision"]["engine"], "jev");
    assert!(result["audit_seq"].as_u64().unwrap() > 0);
    let request = captured.lock().unwrap().take().unwrap();
    assert_eq!(request["model"], "fixture-model");
    assert_eq!(request["questions"]["execution_policy"]["type"], "choice");
    assert_eq!(request["state"]["command"]["argv"][0], "id");
    assert!(store.integrity().unwrap().valid);
    server.abort();
}

#[tokio::test]
async fn unreachable_jev_cannot_turn_local_allow_into_execution_permission() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback networking required for this test");
    let address = listener.local_addr().unwrap();
    drop(listener);
    let jev = JevClient::new(
        &format!("http://{address}/v1/systemone"),
        "fixture-key".repeat(4),
        "fixture-model".into(),
        0.85,
        true,
    )
    .unwrap();
    let d = support::private_tempdir();
    let red = Redactor::new(vec![]);
    let store = Arc::new(Store::open(&d.path().join("audit.sqlite"), red.clone(), 100).unwrap());
    let app = gateway_router(GatewayState::new(
        store,
        "r".repeat(48),
        "v".repeat(48),
        Policy::default(),
        Some(jev),
        red,
    ));
    let body = json!({"request_id":uuid::Uuid::new_v4(),"profile":"strict","command":{"argv":["id"],"cwd":"/workspace"}});
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/decide")
                .header("Authorization", format!("Bearer {}", "r".repeat(48)))
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let result: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(result["decision"]["outcome"], "deny");
    assert_eq!(result["decision"]["rule"], "jev_unavailable_or_invalid");
}
