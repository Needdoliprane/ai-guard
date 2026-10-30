mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    model::{CommandSpec, Decision, DecisionRequest, Event, Outcome},
    policy::{Policy, request_hash},
    redaction::Redactor,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server(path: &Path, capacity: u64) -> Server {
    server_with_redactor(path, capacity, Redactor::new(vec![])).await
}
async fn server_with_redactor(path: &Path, capacity: u64, redactor: Redactor) -> Server {
    let store =
        Arc::new(Store::open_for_tenant(path, "tenant-a", redactor.clone(), capacity).unwrap());
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "tenant-a".into(),
            store,
            runner_token: "a".repeat(48),
            viewer_token: "v".repeat(48),
        }],
        Policy::default(),
        None,
        redactor,
    )
    .unwrap();
    serve(state).await
}
async fn serve(state: GatewayState) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, gateway_router(state)).await.unwrap();
    });
    Server { url, task }
}
fn request(profile: &str, argv: &[&str]) -> DecisionRequest {
    DecisionRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        profile: profile.into(),
        command: CommandSpec {
            argv: argv.iter().map(|s| s.to_string()).collect(),
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
    }
}
async fn decide(server: &Server, request: &DecisionRequest) -> (StatusCode, Value) {
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/decide", server.url))
        .bearer_auth("a".repeat(48))
        .json(request)
        .send()
        .await
        .unwrap();
    (response.status(), response.json().await.unwrap())
}
async fn count(server: &Server) -> u64 {
    reqwest::Client::new()
        .get(format!("{}/api/integrity", server.url))
        .bearer_auth("v".repeat(48))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["event_count"]
        .as_u64()
        .unwrap()
}

#[tokio::test]
async fn repeated_http_decision_is_identical_across_restart_and_full_capacity() {
    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let first = server(&path, 2).await;
    let request = request("strict", &["id"]);
    let (status, original) = decide(&first, &request).await;
    assert_eq!(status, StatusCode::OK);
    let (status, repeated) = decide(&first, &request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(repeated, original);
    assert_eq!(count(&first).await, 2);
    drop(first);
    let restarted = server(&path, 2).await;
    assert_eq!(
        decide(&restarted, &request).await,
        (StatusCode::OK, original)
    );
    assert_eq!(count(&restarted).await, 2);
}

#[tokio::test]
async fn same_request_identity_with_different_content_is_a_conflict() {
    let directory = support::private_tempdir();
    let server = server(&directory.path().join("audit.sqlite"), 100).await;
    let mut request = request("strict", &["id"]);
    assert_eq!(decide(&server, &request).await.0, StatusCode::OK);
    request.command.argv = vec!["pwd".into()];
    assert_eq!(decide(&server, &request).await.0, StatusCode::CONFLICT);
    assert_eq!(count(&server).await, 2);
}

#[tokio::test]
async fn approved_phase_requires_durable_review_and_exact_hash_even_after_cache() {
    let directory = support::private_tempdir();
    let server = server(&directory.path().join("audit.sqlite"), 100).await;
    let initial = request("review", &["cargo", "test"]);
    let mut approved = initial.clone();
    approved.human_approved = true;
    approved.approval_hash = Some(request_hash(&initial));
    assert_eq!(decide(&server, &approved).await.0, StatusCode::FORBIDDEN);
    let (_, review) = decide(&server, &initial).await;
    assert_eq!(review["decision"]["outcome"], "review");
    let (status, allowed) = decide(&server, &approved).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(allowed["decision"]["outcome"], "allow");
    assert_eq!(decide(&server, &approved).await, (StatusCode::OK, allowed));
    assert_eq!(decide(&server, &initial).await, (StatusCode::OK, review));
    approved.approval_hash = Some("0".repeat(64));
    assert_eq!(decide(&server, &approved).await.0, StatusCode::FORBIDDEN);
    assert_eq!(count(&server).await, 3);
    let denied = request("review", &["rm", "-rf", "/workspace"]);
    assert_eq!(
        decide(&server, &denied).await.1["decision"]["outcome"],
        "deny"
    );
    let mut approved_deny = denied;
    approved_deny.human_approved = true;
    approved_deny.approval_hash = Some(request_hash(&approved_deny));
    assert_eq!(
        decide(&server, &approved_deny).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(count(&server).await, 4);
}

#[tokio::test]
async fn concurrent_connections_commit_one_decision_and_replays_recheck_integrity() {
    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let first = server(&path, 100).await;
    let second = server(&path, 100).await;
    let request = request("strict", &["id"]);
    let (a, b) = tokio::join!(decide(&first, &request), decide(&second, &request));
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(b, a);
    assert_eq!(count(&first).await, 2);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE events SET hash='tampered'", []).unwrap();
    assert_eq!(
        decide(&first, &request).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        decide(&second, &request).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn identical_request_ids_remain_independent_between_authenticated_tenants() {
    let directory = support::private_tempdir();
    let tenants = [("tenant-a", "a", "v"), ("tenant-b", "b", "w")].map(|(id, runner, viewer)| {
        TenantGateway {
            tenant_id: id.into(),
            store: Arc::new(
                Store::open_for_tenant(
                    &directory.path().join(format!("{id}.sqlite")),
                    id,
                    Redactor::new(vec![]),
                    100,
                )
                .unwrap(),
            ),
            runner_token: runner.repeat(48),
            viewer_token: viewer.repeat(48),
        }
    });
    let server = serve(
        GatewayState::for_tenants(
            tenants.into(),
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    )
    .await;
    let mut request = request("strict", &["id"]);
    let (_, original_a) = decide(&server, &request).await;
    request.command.argv = vec!["pwd".into()];
    assert_eq!(decide(&server, &request).await.0, StatusCode::CONFLICT);
    let response = reqwest::Client::new()
        .post(format!("{}/api/decide", server.url))
        .bearer_auth("b".repeat(48))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let b: Value = response.json().await.unwrap();
    assert_eq!(b["audit_seq"], 2);
    assert_ne!(b["request_hash"], original_a["request_hash"]);
    assert_eq!(count(&server).await, 2);
}

#[tokio::test]
async fn ambiguous_legacy_history_is_a_conflict_without_silent_merge() {
    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let server = server(&path, 100).await;
    let first = request("strict", &["id"]);
    assert_eq!(decide(&server, &first).await.0, StatusCode::OK);
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 100).unwrap();
    let mut duplicate = store
        .events(&EventQuery::default())
        .unwrap()
        .remove(0)
        .event;
    duplicate.id = uuid::Uuid::new_v4().to_string();
    store.append(duplicate).unwrap();
    assert_eq!(decide(&server, &first).await.0, StatusCode::CONFLICT);
    let missing = request("strict", &["pwd"]);
    let mut incomplete = Event::new("security.decision");
    incomplete.data = json!({"request_id":missing.request_id});
    store.append(incomplete).unwrap();
    assert_eq!(decide(&server, &missing).await.0, StatusCode::CONFLICT);
    assert_eq!(count(&server).await, 4);
}

#[tokio::test]
async fn replay_refuses_a_new_trigger_and_new_decisions_fail_when_audit_is_full() {
    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let server = server(&path, 2).await;
    let request = request("strict", &["id"]);
    assert_eq!(decide(&server, &request).await.0, StatusCode::OK);
    let mut other = request.clone();
    other.request_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        decide(&server, &other).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(decide(&server, &request).await.0, StatusCode::OK);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER discard_new_audit AFTER INSERT ON events BEGIN DELETE FROM events WHERE seq=NEW.seq; END;").unwrap();
    assert_eq!(
        decide(&server, &request).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn legacy_approval_preceding_its_review_is_rejected_as_ambiguous() {
    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let initial = request("review", &["cargo", "test"]);
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 100).unwrap();
    for (phase, outcome) in [(true, Outcome::Allow), (false, Outcome::Review)] {
        let mut event = Event::new("security.decision");
        event.command = Some(initial.command.clone());
        event.decision = Some(Decision::new(outcome, "legacy", "legacy"));
        event.data = json!({"request_id":initial.request_id,"request_hash":request_hash(&initial),"profile":"review","human_approved":phase,"jev":null});
        store.append(event).unwrap();
    }
    drop(store);
    let server = server(&path, 100).await;
    let mut approved = initial;
    approved.human_approved = true;
    approved.approval_hash = Some(request_hash(&approved));
    assert_eq!(decide(&server, &approved).await.0, StatusCode::CONFLICT);
    assert_eq!(count(&server).await, 3);
}

#[tokio::test]
async fn secret_request_identity_is_refused_before_audit_without_disabling_redaction() {
    let directory = support::private_tempdir();
    let request = request("strict", &["id"]);
    let server = server_with_redactor(
        &directory.path().join("audit.sqlite"),
        100,
        Redactor::new(vec![request.request_id.clone()]),
    )
    .await;
    assert_eq!(decide(&server, &request).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(decide(&server, &request).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(count(&server).await, 1); // the refused request adds nothing after genesis
}
