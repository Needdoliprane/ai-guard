//! A bounded judge request must retain a long nonsecret field exactly.
//! This exercises HTTP serialization, not model quality or an execution grant.
use agentsandbox::{jev::JevClient, model::DecisionRequest, redaction::Redactor};
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::{sync::oneshot, task::JoinHandle, time::Duration};

struct Fixture {
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
impl Fixture {
    async fn finish(mut self) -> bool {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let Some(task) = self.task.as_mut() else {
            return false;
        };
        match tokio::time::timeout(Duration::from_secs(3), task).await {
            Ok(Ok(Ok(()))) => {
                drop(self.task.take());
                true
            }
            _ => false,
        }
    }
}

#[tokio::test]
async fn typesafe_keeps_a_40000_byte_nonsecret_field_intact_under_the_request_budget() {
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = Arc::clone(&captured);
    let app = Router::new().route(
        "/v1/systemone",
        post(move |Json(body): Json<Value>| {
            let sink = Arc::clone(&sink);
            async move {
                let mut calls = sink.lock().unwrap_or_else(|poison| poison.into_inner());
                if calls.len() < 2 {
                    calls.push(body);
                }
                Json(json!({
                    "model": "fixture-typesafe",
                    "answers": {"execution_policy": {
                        "type": "choice", "choice": "allow", "confidence": 0.99
                    }}
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("native loopback required for the masking fixture");
    let address = listener.local_addr().expect("inspect owned listener");
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let fixture = Fixture {
        stop: Some(stop),
        task: Some(task),
    };
    let client = JevClient::new(
        &format!("http://{address}/v1/systemone"),
        "fixture-distinct-key".repeat(3),
        "fixture-requested".into(),
        0.85,
        true,
    )
    .expect("construct existing explicit TypeSafe client");
    let nonsecret = "x".repeat(40_000);
    let request: DecisionRequest = serde_json::from_value(json!({
        "request_id": uuid::Uuid::new_v4(), "profile": "strict",
        "command": {"argv": ["id"], "cwd": "/workspace"},
        "project_id": nonsecret
    }))
    .expect("current request fixture");
    let result = client.decide(&request, &Redactor::new(vec![])).await;
    drop(client);
    let clean = fixture.finish().await;
    assert!(
        clean,
        "the owned fixture must stop before the preservation oracle"
    );
    assert!(
        result.is_ok(),
        "a valid request under the total budget must remain admissible to the adapter"
    );
    let calls = captured.lock().unwrap_or_else(|poison| poison.into_inner());
    assert!(
        calls.len() == 1,
        "the real TypeSafe HTTP path must run exactly once"
    );
    let bytes = serde_json::to_vec(&calls[0]).expect("serialize actual sent body");
    assert!(
        bytes.len() <= 65_536,
        "the complete observed request must fit the total budget"
    );

    // Historical Redactor::value clips this field at32768. Jev must use its
    // additive nonclipping path and then bound the complete serialized body.
    assert!(
        calls[0]["state"]["project_id"].as_str() == Some(nonsecret.as_str()),
        "the actual HTTP request must preserve all40000nonsecret bytes without clipping"
    );
}
