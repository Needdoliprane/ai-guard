//! Existing-client protocol regression: raw duplicate JSON must not authorize.
use agentsandbox::{jev::JevClient, model::DecisionRequest, redaction::Redactor};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    response::Response,
    routing::post,
};
use serde_json::json;
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
    async fn finish(mut self) -> Result<(), &'static str> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let task = self.task.as_mut().ok_or("fixture_task_missing")?;
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .map_err(|_| "fixture_shutdown_budget")?
            .map_err(|_| "fixture_join")?
            .map_err(|_| "fixture_server")?;
        self.task.take();
        Ok(())
    }
}
#[derive(Default)]
struct Capture {
    requests: Vec<()>,
}
async fn server(replies: Vec<(StatusCode, Vec<u8>)>) -> (String, Arc<Mutex<Capture>>, Server) {
    let captures = Arc::new(Mutex::new(Capture::default()));
    let captured = Arc::clone(&captures);
    let replies = Arc::new(replies);
    let app = Router::new().route(
        "/fixture",
        post(move |_: Request<Body>| {
            let captured = Arc::clone(&captured);
            let replies = Arc::clone(&replies);
            async move {
                let mut capture = captured.lock().unwrap_or_else(|poison| poison.into_inner());
                let index = capture.requests.len();
                capture.requests.push(());
                let (status, bytes) = replies
                    .get(index)
                    .cloned()
                    .unwrap_or((StatusCode::INTERNAL_SERVER_ERROR, Vec::new()));
                let mut response = Response::new(Body::from(bytes));
                *response.status_mut() = status;
                response
                    .headers_mut()
                    .insert("content-type", "application/json".parse().unwrap());
                response
                    .headers_mut()
                    .insert("location", "/fixture".parse().unwrap());
                response
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    (
        format!("http://{address}/fixture"),
        captures,
        Server {
            stop: Some(stop),
            task: Some(task),
        },
    )
}
fn request(argv: &[&str]) -> DecisionRequest {
    serde_json::from_value(json!({"request_id":uuid::Uuid::new_v4(),"profile":"strict","command":{"argv":argv,"cwd":"/workspace"}})).unwrap()
}

#[tokio::test]
async fn typesafe_rejects_duplicate_decisions_scores_oversize_and_redirects() {
    let positive = br#"{"model":"fixture-reported","answers":{"execution_policy":{"type":"choice","choice":"allow","confidence":0.99,"probabilities":{"allow":0.99,"deny":0.01,"review":0}}},"usage":{"input_tokens":null,"output_tokens":1,"unused_extension":true},"request_id":"fixture-provider-metadata"}"#.to_vec();
    let choice = br#"{"model":"fixture","answers":{"execution_policy":{"type":"choice","choice":"deny","choice":"allow","confidence":0.99}}}"#.to_vec();
    let score = br#"{"model":"fixture","answers":{"execution_policy":{"type":"choice","choice":"allow","confidence":0.1,"confidence":0.99}}}"#.to_vec();
    let usage = br#"{"model":"fixture","answers":{"execution_policy":{"type":"choice","choice":"allow","confidence":0.99}},"usage":{"input_tokens":null,"input_tokens":7,"output_tokens":1}}"#.to_vec();
    let (url, captures, server) = server(vec![
        (StatusCode::OK, positive),
        (StatusCode::OK, choice),
        (StatusCode::OK, score),
        (StatusCode::OK, usage),
        (StatusCode::OK, vec![b'x'; 65_537]),
        (StatusCode::TEMPORARY_REDIRECT, Vec::new()),
    ])
    .await;
    let observed = async {
        let client = JevClient::new(
            &url,
            "fixture-distinct-key".repeat(3),
            "fixture-requested".into(),
            0.85,
            true,
        )
        .map_err(|_| "client")?;
        let positive = client
            .decide(&request(&["/usr/bin/id"]), &Redactor::new(vec![]))
            .await
            .map_err(|_| "positive_contract")?;
        let mut errors = Vec::new();
        for _ in 0..5 {
            errors.push(
                client
                    .decide(&request(&["/usr/bin/id"]), &Redactor::new(vec![]))
                    .await
                    .is_err(),
            );
        }
        Ok::<_, &'static str>((positive, errors))
    }
    .await;
    let cleanup = server.finish().await;
    assert!(cleanup.is_ok(), "owned fixture cleanup");
    let (positive, errors) = observed.expect("collect TypeSafe protocol observations");
    assert_eq!(positive.outcome, agentsandbox::model::Outcome::Allow);
    assert_eq!(positive.model, "fixture-reported");
    assert!(positive.usage["input_tokens"].is_null());
    assert_eq!(positive.usage["output_tokens"], 1);
    assert_eq!(
        captures
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .requests
            .len(),
        6
    );
    eprintln!(
        "Jev protocol refusals: duplicate_choice={} duplicate_confidence={} duplicate_usage={} oversize={} redirect={}",
        errors[0], errors[1], errors[2], errors[3], errors[4]
    );
    assert!(errors[0], "duplicate TypeSafe choice must be refused");
    assert!(errors[1], "duplicate TypeSafe confidence must be refused");
    assert!(
        errors[2],
        "duplicate TypeSafe usage counter must be refused"
    );
    assert!(
        errors[3] && errors[4],
        "response byte limit and redirects must remain closed"
    );
}
