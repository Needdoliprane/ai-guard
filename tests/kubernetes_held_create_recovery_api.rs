//! A real worker credential must never enter the broker recovery boundary.
//! Existing Store supervision fixtures declare runtime facts; no engine runs.
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    control::*,
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    redaction::Redactor,
    relay::*,
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc, time::Duration};
use tower::ServiceExt;
use uuid::Uuid;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[allow(dead_code)]
#[path = "support/relay_admission_fixture.rs"]
mod fixture;

async fn call(
    router: &Router,
    method: &str,
    target: &str,
    token: &str,
    body: String,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(target)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(15), router.clone().oneshot(request))
        .await
        .unwrap()
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn authenticated_worker_cannot_mutate_or_read_broker_recovery() {
    let directory = support::private_tempdir();
    let store = Arc::new(fixture::fresh(
        directory.path(),
        "recovery-worker-role",
        10_000,
    ));
    let worker = fixture::running_now(&store);
    let token = "public-worker-recovery-role-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let (credential_id, binding_revision) = fixture::bind(&store, &worker, token);
    let gateway = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "recovery-worker-role".into(),
            store: Arc::clone(&store),
            runner_token: "public-recovery-runner-rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr".into(),
            viewer_token: "public-recovery-viewer-vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv".into(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let router = gateway_router(gateway);
    let unknown = "public-unknown-worker-recovery-xxxxxxxxxxxxxxxxxxxxxxxxxxxx";
    let before_canary = store.head().unwrap();
    let (status, error) = call(
        &router,
        "GET",
        "/api/v1/relay/messages?limit=1",
        unknown,
        String::new(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error["error"]["code"], "authentication_required");
    assert_eq!(store.head().unwrap(), before_canary);

    // This existing public route requires VerifiedWorker, then revalidates it
    // in the real Store. An invalid credential cannot produce a successful
    // inbox or its worker-attributed durable read event.
    let (status, body) = call(
        &router,
        "GET",
        "/api/v1/relay/messages?limit=1",
        token,
        String::new(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let inbox: RelayInboxV1 = serde_json::from_value(body).unwrap();
    assert_eq!(inbox.version, 1);
    assert!(inbox.items.is_empty());
    assert!(inbox.next_after.is_none());
    assert_eq!(store.head().unwrap(), before_canary + 1);
    let reads = store
        .events(&EventQuery {
            kind: Some("control.relay.read".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(reads.len(), 1);
    let actor = &reads[0].event.data["actor"];
    assert_eq!(actor["credential_id"], credential_id);
    assert_eq!(actor["binding_revision"], binding_revision);
    assert_eq!(actor["endpoint"]["worker_id"], worker.worker_id);
    assert_eq!(actor["endpoint"]["attempt_id"], worker.attempt.attempt_id);
    assert_eq!(actor["endpoint"]["generation"], 1);

    let create_id = Uuid::new_v4().to_string();
    let operation_id = Uuid::new_v4().to_string();
    let base = format!("/api/v1/brokers/kubernetes/held-creates/{create_id}/recoveries");
    let before = store.head().unwrap();
    let quota = store.worker_quota(&QuotaPolicyV1::default()).unwrap();
    for (method, target, body) in [
        ("POST", base.clone(), json!({"version":3,"create_id":create_id,"operation_id":operation_id,"intent_hash":HASH_A,"expected_revision":1,"expected_create_epoch":1}).to_string()),
        ("GET", format!("{base}/{operation_id}"), String::new()),
    ] {
        let (status, error) = call(&router, method, &target, unknown, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(error["error"]["code"], "authentication_required");
        assert_eq!(store.head().unwrap(), before);
        let (status, error) = call(&router, method, &target, token, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(error["error"]["code"], "workload_forbidden");
        assert_eq!(store.head().unwrap(), before);
        let after = store.worker_quota(&QuotaPolicyV1::default()).unwrap();
        assert_eq!(serde_json::to_value(after).unwrap(), serde_json::to_value(&quota).unwrap());
    }
}
