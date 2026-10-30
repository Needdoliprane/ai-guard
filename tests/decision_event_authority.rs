mod support;

use agentsandbox::{
    api::{GatewayState, gateway_router},
    audit::Store,
    model::{CommandSpec, Decision, DecisionRequest, Event, Outcome},
    policy::{Policy, request_hash},
    redaction::Redactor,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::sync::Arc;

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
    store: Arc<Store>,
    _directory: tempfile::TempDir,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server() -> Server {
    let directory = support::private_tempdir();
    let redactor = Redactor::new(vec![]);
    let store = Arc::new(
        Store::open(
            &directory.path().join("audit.sqlite"),
            redactor.clone(),
            100,
        )
        .unwrap(),
    );
    let app = gateway_router(GatewayState::new(
        store.clone(),
        "r".repeat(48),
        "v".repeat(48),
        Policy::default(),
        None,
        redactor,
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server {
        url,
        task,
        store,
        _directory: directory,
    }
}
async fn ingest(server: &Server, path: &str, event: &Event) -> StatusCode {
    reqwest::Client::new()
        .post(format!("{}{path}", server.url))
        .bearer_auth("r".repeat(48))
        .json(event)
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn arbitrary_ingestion_cannot_manufacture_a_cached_allow_for_a_denied_command() {
    let server = server().await;
    let request = DecisionRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        profile: "strict".into(),
        command: CommandSpec {
            argv: vec!["rm".into(), "-rf".into(), "/workspace".into()],
            cwd: "/workspace".into(),
        },
        project_id: None,
        sandbox_id: None,
        agent_id: None,
        session_id: None,
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    };
    let mut forged = Event::new("security.decision");
    forged.command = Some(request.command.clone());
    forged.decision = Some(Decision::new(
        Outcome::Allow,
        "fixture_forged_allow",
        "fixture",
    ));
    forged.data = json!({"request_id":request.request_id,"request_hash":request_hash(&request),"profile":"strict","human_approved":false,"jev":null});
    let status = ingest(&server, "/api/v1/events", &forged).await;
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/decide", server.url))
        .bearer_auth("r".repeat(48))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value = response.json().await.unwrap();
    assert_eq!(
        result["decision"]["outcome"], "deny",
        "ingestion must never become an authorization receipt"
    );
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(server.store.head().unwrap(), 2); // genesis + genuine deny
}

#[tokio::test]
async fn control_namespaces_are_reserved_on_both_ingestion_aliases() {
    let server = server().await;
    for path in ["/api/events", "/api/v1/events"] {
        for kind in [
            "security.decision",
            "security.decision.extra",
            "control",
            "control.worker.reserved",
            "control.schema.migrated",
            "worker",
            "worker.start.claimed",
            "usage.allocation",
            "usage.allocation.terminal",
        ] {
            let mut event = Event::new(kind);
            // A declared provider event cannot mint control state either.
            event.source = "provider".into();
            event.provenance = "declared".into();
            assert_eq!(
                ingest(&server, path, &event).await,
                StatusCode::FORBIDDEN,
                "{kind}"
            );
        }
    }
    assert_eq!(server.store.head().unwrap(), 1); // genesis only
}

#[tokio::test]
async fn existing_runner_and_declared_telemetry_remain_ingestible() {
    let server = server().await;
    for kind in [
        "command.requested",
        "command.started",
        "command.completed",
        "runtime.submitted",
        "runtime.execution.error",
        "runtime.cleanup.failed",
        "runtime.security_downgrade",
        "workspace.import.completed",
        "workspace.export.completed",
        "agent.declared",
        "tool.requested",
        "tool.result",
        "usage.reported",
        "security.decision_reported",
    ] {
        let mut event = Event::new(kind);
        event.source = "runner".into();
        if kind == "agent.declared"
            || kind == "tool.requested"
            || kind == "tool.result"
            || kind == "usage.reported"
        {
            event.provenance = "declared".into();
        }
        assert_eq!(
            ingest(&server, "/api/events", &event).await,
            StatusCode::OK,
            "{kind}"
        );
    }
    assert_eq!(server.store.head().unwrap(), 15); // genesis + 14 accepted declarations
}
