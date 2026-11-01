//! Product decision provenance through real API/audit and a local HTTP
//! protocol fixture. No provider/model or semantic qualification is implied.
mod support;

use agentsandbox::{
    api::{GatewayState, gateway_router},
    audit::Store,
    jev::JevClient,
    model::DecisionRequest,
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
use tokio::{sync::oneshot, task::JoinHandle, time::Duration};
use tower::ServiceExt;

struct FixtureServer {
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}
impl Drop for FixtureServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
impl FixtureServer {
    async fn finish(mut self) -> Result<(), &'static str> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let task = self.task.as_mut().ok_or("fixture_task_missing")?;
        match tokio::time::timeout(Duration::from_secs(3), task).await {
            Ok(Ok(Ok(()))) => {
                drop(self.task.take());
                Ok(())
            }
            Ok(_) => Err("fixture_server_failed"),
            Err(_) => Err("fixture_server_shutdown_budget_exhausted"),
        }
    }
}

#[tokio::test]
async fn typesafe_allow_keeps_selected_backend_provenance_in_response_and_audit() {
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = Arc::clone(&captured);
    let fixture = Router::new().route(
        "/v1/systemone",
        post(move |Json(body): Json<Value>| {
            let sink = Arc::clone(&sink);
            async move {
                let mut calls = sink.lock().unwrap_or_else(|poison| poison.into_inner());
                if calls.len() < 2 {
                    calls.push(body);
                }
                Json(json!({
                    "model": "fixture-returned-typesafe",
                    "answers": {"execution_policy": {
                        "type": "choice", "choice": "allow", "confidence": 0.99
                    }},
                    "usage": {"input_tokens": 11, "output_tokens": 2}
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("native loopback is required for the protocol fixture");
    let address = listener
        .local_addr()
        .expect("inspect owned fixture listener");
    let (shutdown, stop) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, fixture)
            .with_graceful_shutdown(async {
                let _ = stop.await;
            })
            .await
    });
    let server = FixtureServer {
        shutdown: Some(shutdown),
        task: Some(task),
    };

    let directory = support::private_tempdir();
    let redactor = Redactor::new(vec![]);
    let store = Arc::new(
        Store::open(
            &directory.path().join("audit.sqlite"),
            redactor.clone(),
            100,
        )
        .expect("open the real private audit fixture"),
    );
    let request: DecisionRequest = serde_json::from_value(json!({
        "request_id": uuid::Uuid::new_v4(),
        "profile": "strict",
        "command": {"argv": ["id"], "cwd": "/workspace"}
    }))
    .expect("construct the current product decision request");
    let jev = JevClient::new(
        &format!("http://{address}/v1/systemone"),
        "fixture-distinct-jev-key".repeat(3),
        "fixture-requested-typesafe".into(),
        0.85,
        true,
    )
    .expect("construct the existing explicit TypeSafe client");
    let app = gateway_router(GatewayState::new(
        Arc::clone(&store),
        "r".repeat(48),
        "v".repeat(48),
        Policy::default(),
        Some(jev),
        redactor,
    ));
    let result = async {
        let body = serde_json::to_vec(&request).map_err(|_| "request_serialization_failed")?;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decide")
                    .header("Authorization", format!("Bearer {}", "r".repeat(48)))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .map_err(|_| "fixture_request_invalid")?,
            )
            .await
            .map_err(|_| "decision_route_failed")?;
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .map_err(|_| "decision_body_failed")?
            .to_bytes();
        let result: Value = serde_json::from_slice(&bytes).map_err(|_| "decision_json_invalid")?;
        let stored = store
            .decision(&request, None)
            .map_err(|_| "confirmed_audit_read_failed")?
            .ok_or("decision_not_durable")?;
        Ok::<_, &'static str>((status, result, stored))
    }
    .await;
    let cleanup = server.finish().await;
    assert!(
        cleanup.is_ok(),
        "the owned HTTP fixture must stop before the oracle"
    );
    let (status, result, stored) = result.expect("collect the real decision and durable audit");
    let calls = captured.lock().unwrap_or_else(|poison| poison.into_inner());
    assert!(
        status == StatusCode::OK,
        "the real decision route must succeed"
    );
    assert!(
        calls.len() == 1,
        "the selected TypeSafe transport must be exercised exactly once"
    );
    assert!(
        calls[0]["questions"]["execution_policy"]["type"] == "choice"
            && calls[0]["model"] == "fixture-requested-typesafe",
        "the fixture must receive the real TypeSafe request contract"
    );
    assert!(
        result["decision"]["outcome"] == "allow",
        "this provenance oracle needs a real local and TypeSafe Allow"
    );
    assert!(
        result["jev"]["model"] == "fixture-returned-typesafe"
            && result["jev"]["confidence"] == 0.99,
        "the provided model/score must retain their actual source"
    );
    assert!(
        result["audit_seq"] == stored.seq
            && result["audit_hash"] == stored.hash
            && store.integrity().is_ok_and(|integrity| integrity.valid),
        "the response must name the actual durable decision"
    );

    // Current APIs compile this test, but current metadata omits backend.
    // This is selection provenance, not proof of a calibrated safety score.
    assert!(
        result["jev"]["backend"] == "typesafe",
        "the real decision response must identify the selected TypeSafe backend"
    );
    assert!(
        stored.event.data["jev"]["backend"] == "typesafe",
        "the confirmed audit must preserve the selected backend provenance"
    );
}
