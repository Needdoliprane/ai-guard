//! Real gateway/SQLite fixtures for the closed worker lane. Runtime records
//! are synthetic supervision evidence; no container or provider is executed.
use super::*;
use agentsandbox::{api::gateway_control_router, ingress::ingress_router_with_control};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    task::Poll,
    time::Duration,
};

const MESSAGES: &str = "/api/v1/relay/messages";

fn control_app(a: Arc<Store>, b: Arc<Store>) -> Router {
    gateway_control_router(
        GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: a,
                    runner_token: "a".repeat(48),
                    viewer_token: "v".repeat(48),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: b,
                    runner_token: "b".repeat(48),
                    viewer_token: "w".repeat(48),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    )
}

fn counted_body(polls: Arc<AtomicUsize>) -> Body {
    Body::from_stream(futures_util::stream::once(async move {
        polls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"{}"))
    }))
}
async fn raw(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Body,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

fn broker_proof(store: &Store, token: &str) {
    let broker_id = Uuid::new_v4().to_string();
    let credential_id = Uuid::new_v4().to_string();
    let incarnation_id = Uuid::new_v4().to_string();
    store
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential_id.clone(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: broker_id.clone(),
                incarnation_id: incarnation_id.clone(),
            },
            sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
            expires_at: (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
        })
        .unwrap();
    store
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id,
            expected_revision: None,
            credential_id,
            incarnation_id,
            authorities: vec![BrokerAuthorityV1 {
                authority_id: "fixture-local-authority".into(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 120,
            not_after: (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
        })
        .unwrap();
}

#[tokio::test]
async fn worker_lane_authenticates_before_body_and_its_eight_tenant_slots() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 10_000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 10_000));
    let a1 = running_worker(&a);
    let b1 = running_worker(&b);
    let token_a = "1".repeat(48);
    let token_b = "2".repeat(48);
    bind(&a, &a1, &token_a);
    bind(&b, &b1, &token_b);
    let human = "h".repeat(48);
    bind_local_human_metadata(&a, &human);
    let broker = "fixture-broker-proof-for-worker-lane".repeat(2);
    broker_proof(&a, &broker);
    let app = control_app(a.clone(), b.clone());
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let (polled, admission) = tokio::sync::oneshot::channel();
        let mut polled = Some(polled);
        let body = Body::from_stream(futures_util::stream::poll_fn(move |_| {
            if let Some(polled) = polled.take() {
                let _ = polled.send(());
            }
            Poll::<Option<Result<axum::body::Bytes, std::io::Error>>>::Pending
        }));
        let app = app.clone();
        let token = token_a.clone();
        tasks.spawn(async move { raw(&app, "POST", MESSAGES, Some(&token), body).await });
        tokio::time::timeout(Duration::from_secs(30), admission)
            .await
            .unwrap()
            .unwrap();
    }
    let heads = (a.head().unwrap(), b.head().unwrap());
    let runner = "a".repeat(48);
    let viewer = "v".repeat(48);
    let unknown = "z".repeat(48);
    for (token, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(unknown.as_str()), StatusCode::UNAUTHORIZED),
        (Some(runner.as_str()), StatusCode::FORBIDDEN),
        (Some(viewer.as_str()), StatusCode::FORBIDDEN),
        (Some(human.as_str()), StatusCode::FORBIDDEN),
        (Some(broker.as_str()), StatusCode::FORBIDDEN),
        (Some(token_a.as_str()), StatusCode::TOO_MANY_REQUESTS),
    ] {
        let polls = Arc::new(AtomicUsize::new(0));
        let response = raw(&app, "POST", MESSAGES, token, counted_body(polls.clone())).await;
        assert_eq!(response.status(), expected);
        assert_eq!(polls.load(Ordering::SeqCst), 0);
    }
    assert_eq!((a.head().unwrap(), b.head().unwrap()), heads);
    // The other tenant, runner lane and broker presence retain their capacity.
    let polls = Arc::new(AtomicUsize::new(0));
    assert_eq!(
        raw(
            &app,
            "POST",
            MESSAGES,
            Some(&token_b),
            counted_body(polls.clone())
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    assert_eq!(
        raw(
            &app,
            "GET",
            "/api/v1/control/protocol",
            Some(&runner),
            Body::empty()
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        raw(
            &app,
            "GET",
            "/api/v1/brokers/identity",
            Some(&broker),
            Body::empty()
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        raw(
            &app,
            "GET",
            "/api/v1/control/protocol",
            Some(&token_a),
            Body::empty()
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    let polls = Arc::new(AtomicUsize::new(0));
    assert_eq!(
        raw(
            &app,
            "POST",
            MESSAGES,
            Some(&token_a),
            counted_body(polls.clone())
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    for (method, path, expected) in [
        (
            "GET",
            "/api/v1/relay/messages?limit=0",
            StatusCode::BAD_REQUEST,
        ),
        (
            "GET",
            "/api/v1/relay/messages?after=01",
            StatusCode::BAD_REQUEST,
        ),
        (
            "POST",
            "/api/v1/relay/messages?limit=1",
            StatusCode::BAD_REQUEST,
        ),
        (
            "POST",
            "/api/v1/relay/messages/not-a-uuid/ack",
            StatusCode::NOT_FOUND,
        ),
    ] {
        let polls = Arc::new(AtomicUsize::new(0));
        assert_eq!(
            raw(
                &app,
                method,
                path,
                Some(&token_a),
                counted_body(polls.clone())
            )
            .await
            .status(),
            expected
        );
        assert_eq!(polls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn worker_lane_keeps_handler_binding_revalidation_after_upload() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 1000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 1000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let token = "1".repeat(48);
    let (credential_id, revision) = bind(&a, &a1, &token);
    bind(&a, &a2, &"2".repeat(48));
    let grant = directed(&a1, &a2, 300);
    a.relay_grant(&grant).unwrap();
    let payload = send(&grant, "must not dispatch after binding revocation");
    let app = control_app(a.clone(), b);
    let (polled, admission) = tokio::sync::oneshot::channel();
    let (finish, finished) = tokio::sync::oneshot::channel::<Vec<u8>>();
    let body = Body::from_stream(async_stream::stream! {
        let _ = polled.send(());
        if let Ok(bytes) = finished.await { yield Ok::<_, std::io::Error>(bytes); }
    });
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move { raw(&app, "POST", MESSAGES, Some(&token), body).await });
    tokio::time::timeout(Duration::from_secs(30), admission)
        .await
        .unwrap()
        .unwrap();
    a.revoke_workload_binding(&WorkloadBindingRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id,
        expected_revision: revision,
    })
    .unwrap();
    let head = a.head().unwrap();
    finish.send(serde_json::to_vec(&payload).unwrap()).unwrap();
    let response = tasks.join_next().await.unwrap().unwrap();
    assert!(matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    assert_eq!(a.head().unwrap(), head);
    assert_eq!(
        rows(
            &directory.path().join("tenant-a.sqlite"),
            "SELECT COUNT(*) FROM relay_messages"
        ),
        0
    );
}

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(app: Router) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server { url, task }
}

#[tokio::test]
async fn published_worker_lane_delivers_large_inbox_and_preserves_tenant_audit_contract() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 256));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 256));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let b1 = running_worker(&b);
    let token1 = "1".repeat(48);
    let token2 = "2".repeat(48);
    let token_b = "3".repeat(48);
    bind(&a, &a1, &token1);
    bind(&a, &a2, &token2);
    bind(&b, &b1, &token_b);
    let grant = directed(&a1, &a2, 300);
    a.relay_grant(&grant).unwrap();
    let gateway = serve(control_app(a.clone(), b.clone())).await;
    let public_calls = Arc::new(AtomicUsize::new(0));
    let count = public_calls.clone();
    let public = serve(Router::new().fallback(axum::routing::any(move || {
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            StatusCode::IM_A_TEAPOT
        }
    })))
    .await;
    let ingress = serve(ingress_router_with_control(&public.url, &gateway.url).unwrap()).await;
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let url = format!("{}/control{MESSAGES}", ingress.url);
    let mut operations = Vec::new();
    for _ in 0..10 {
        let payload = send(&grant, &"q".repeat(65_536));
        let response = http
            .post(&url)
            .bearer_auth(&token1)
            .json(&payload)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let receipt: RelayReceiptV1 = response.json().await.unwrap();
        assert_eq!(receipt.operation_id, payload.operation_id);
        operations.push(payload);
    }
    let foreign = http
        .post(&url)
        .bearer_auth(&token_b)
        .json(&send(&grant, "foreign attempt"))
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    let response = http
        .get(format!("{url}?limit=32&after=0"))
        .bearer_auth(&token2)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.bytes().await.unwrap();
    assert!(bytes.len() > 512 * 1024 && bytes.len() <= 2 * 1024 * 1024);
    let inbox: RelayInboxV1 = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(inbox.items.len(), 10);
    assert!(
        inbox
            .items
            .iter()
            .all(|message| message.body.len() == 65_536)
    );
    let response = http.get(&url).bearer_auth(&token_b).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let foreign: RelayInboxV1 = response.json().await.unwrap();
    assert!(foreign.items.is_empty());
    let ack = RelayAckV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: operations[0].operation_id.clone(),
    };
    let ack_url = format!("{url}/{}/ack", ack.message_id);
    for replayed in [false, true] {
        let response = http
            .post(&ack_url)
            .bearer_auth(&token2)
            .json(&ack)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let receipt: RelayReceiptV1 = response.json().await.unwrap();
        assert_eq!(receipt.operation_id, ack.operation_id);
        assert_eq!(receipt.replayed, replayed);
    }
    let head = a.head().unwrap();
    let invalid = http
        .post(&url)
        .bearer_auth(&token1)
        .json(&json!({"version":1,"tenant":"tenant-b"}))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(a.head().unwrap(), head);
    let mut exhausted = false;
    for _ in 0..512 {
        if a.append(agentsandbox::model::Event::new(
            "ordinary.worker-lane-capacity.fixture",
        ))
        .is_err()
        {
            exhausted = true;
            break;
        }
    }
    assert!(exhausted);
    let messages = rows(
        &directory.path().join("tenant-a.sqlite"),
        "SELECT COUNT(*) FROM relay_messages",
    );
    let response = http
        .post(&url)
        .bearer_auth(&token1)
        .json(&send(&grant, "must not persist without mandatory audit"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let response = http.get(&url).bearer_auth(&token2).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        rows(
            &directory.path().join("tenant-a.sqlite"),
            "SELECT COUNT(*) FROM relay_messages"
        ),
        messages
    );
    assert_eq!(public_calls.load(Ordering::SeqCst), 0);
    for store in [&a, &b] {
        assert!(store.integrity().unwrap().valid);
    }
}
