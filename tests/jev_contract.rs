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
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tower::ServiceExt;

fn decision_body() -> Value {
    json!({
        "request_id": uuid::Uuid::new_v4(),
        "profile": "strict",
        "command": {"argv": ["id"], "cwd": "/workspace"}
    })
}

fn fixture_gateway(store: Arc<Store>, jev: JevClient, redactor: Redactor) -> Router {
    gateway_router(GatewayState::new(
        store,
        "r".repeat(48),
        "v".repeat(48),
        Policy::default(),
        Some(jev),
        redactor,
    ))
}

async fn decide(app: Router, body: &Value) -> Value {
    let request = Request::builder()
        .method("POST")
        .uri("/api/decide")
        .header("Authorization", format!("Bearer {}", "r".repeat(48)))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(10), app.oneshot(request))
        .await
        .expect("decision must finish within the fixture deadline")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

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
    let result = decide(fixture_gateway(store.clone(), jev, red), &decision_body()).await;
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
    let result = decide(fixture_gateway(store, jev, red), &decision_body()).await;
    assert_eq!(result["decision"]["outcome"], "deny");
    assert_eq!(result["decision"]["rule"], "jev_unavailable_or_invalid");
}

#[tokio::test]
async fn below_threshold_jev_allow_is_durably_denied_and_replayed_without_post() {
    let posts = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&posts);
    let mock = Router::new().route(
        "/v1/systemone",
        post(move || {
            let observed = Arc::clone(&observed);
            async move {
                observed.fetch_add(1, Ordering::Relaxed);
                Json(json!({
                    "model": "fixture-model",
                    "answers": {"execution_policy": {
                        "type": "choice", "choice": "allow", "confidence": 0.84
                    }},
                    "usage": {"input_tokens": 20, "output_tokens": 2}
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback networking required for this test");
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await });
    let jev = JevClient::new(
        &format!("http://{address}/v1/systemone"),
        "fixture-key".repeat(4),
        "fixture-model".into(),
        0.85,
        true,
    )
    .unwrap();

    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let open = || Arc::new(Store::open(&path, Redactor::new(vec![]), 100).unwrap());
    let body = decision_body();
    let store = open();
    let before = store.integrity().unwrap();
    let denied = decide(
        fixture_gateway(store.clone(), jev.clone(), Redactor::new(vec![])),
        &body,
    )
    .await;
    assert_eq!(denied["decision"]["outcome"], "deny");
    assert_eq!(denied["decision"]["engine"], "jev");
    assert_eq!(denied["decision"]["rule"], "jev_unavailable_or_invalid");
    assert_eq!(denied["jev"]["status"], "unavailable_or_invalid");
    assert_eq!(denied["jev"]["error"], "invalid_response");
    assert_eq!(denied["receipt"]["request_id"], body["request_id"]);
    assert_eq!(denied["receipt"]["request_hash"], denied["request_hash"]);
    assert_eq!(denied["receipt"]["audit_seq"], denied["audit_seq"]);
    assert_eq!(denied["receipt"]["audit_hash"], denied["audit_hash"]);
    let committed = store.integrity().unwrap();
    assert!(committed.valid);
    assert_eq!(committed.event_count, before.event_count + 1);
    assert_eq!(denied["audit_hash"], committed.head_hash);
    assert_eq!(posts.load(Ordering::Relaxed), 1);
    drop(store);

    let reopened = open();
    let replayed = decide(
        fixture_gateway(reopened.clone(), jev, Redactor::new(vec![])),
        &body,
    )
    .await;
    assert_eq!(replayed, denied);
    assert_eq!(posts.load(Ordering::Relaxed), 1);
    let after = reopened.integrity().unwrap();
    assert!(after.valid);
    assert_eq!(after.event_count, committed.event_count);
    assert_eq!(after.head_hash, committed.head_hash);
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
