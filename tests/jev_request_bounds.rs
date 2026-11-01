//! Request budgeting on a real HTTP protocol fixture, without a model call.
use agentsandbox::{
    jev::JevClient,
    model::{DecisionRequest, Outcome},
    redaction::Redactor,
};
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::{sync::oneshot, task::JoinHandle, time::Duration};

struct Server {
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
impl Server {
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

fn request(project: Option<String>, extra_arg: Option<String>) -> DecisionRequest {
    let mut argv = vec!["id".to_owned()];
    if let Some(argument) = extra_arg {
        argv.push(argument);
    }
    serde_json::from_value(json!({
        "request_id": uuid::Uuid::new_v4(), "profile": "strict",
        "command": {"argv": argv, "cwd": "/workspace"}, "project_id": project
    }))
    .expect("current decision request fixture")
}

#[tokio::test]
async fn typesafe_request_limit_is_checked_after_masking_before_any_oversize_send() {
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = Arc::clone(&captured);
    let app = Router::new().route(
        "/v1/systemone",
        post(move |Json(body): Json<Value>| {
            let sink = Arc::clone(&sink);
            async move {
                let mut calls = sink.lock().unwrap_or_else(|poison| poison.into_inner());
                if calls.len() < 4 {
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
        .expect("native loopback required for request budgeting fixture");
    let address = listener.local_addr().expect("inspect fixture listener");
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let server = Server {
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
    .expect("existing explicit TypeSafe constructor");
    let ordinary = client
        .decide(&request(None, None), &Redactor::new(vec![]))
        .await;
    // Two raw fields total more than 65536 bytes with JSON framing. Each is
    // below Redactor::value's historical per-string clip boundary. Masking
    // reduces them to an admissible body; a pre-mask raw-size gate is wrong.
    let marker = "0123456789012345678901234567890123456789";
    let maskable = marker.repeat(819);
    let masked = client
        .decide(
            &request(Some(maskable.clone()), Some(maskable)),
            &Redactor::new(vec![marker.into()]),
        )
        .await;
    // Neither field exceeds that old clip boundary, but their aggregate plus
    // actual protocol framing necessarily exceeds the new transport budget.
    let oversized = client
        .decide(
            &request(Some("p".repeat(32_768)), Some("a".repeat(32_768))),
            &Redactor::new(vec![]),
        )
        .await;
    drop(client);
    let clean = server.finish().await;
    assert!(
        clean,
        "the owned HTTP fixture must stop before the budget oracle"
    );
    assert!(
        ordinary.is_ok_and(|answer| answer.outcome == Outcome::Allow),
        "the ordinary positive must exercise the real TypeSafe transport"
    );
    assert!(
        masked.is_ok_and(|answer| answer.outcome == Outcome::Allow),
        "the budget applies after masking rather than to oversized raw fields"
    );
    let calls = captured.lock().unwrap_or_else(|poison| poison.into_inner());
    assert!(
        calls.len() >= 2,
        "both admissible positive requests must reach the fixture"
    );
    let masked_bytes = serde_json::to_vec(&calls[1]).expect("serialize observed masked request");
    assert!(
        masked_bytes.len() <= 65_536 && !String::from_utf8_lossy(&masked_bytes).contains(marker),
        "the actual sent positive must be bounded and masked"
    );

    // Current decide() has no total request-body bound: the third request is
    // sent and accepted. This is a behavioral RED on existing library APIs.
    assert!(
        oversized.is_err(),
        "an oversized masked JSON request must be refused before transport"
    );
    assert!(
        calls.len() == 2,
        "no oversized request may reach the HTTP fixture"
    );
}
