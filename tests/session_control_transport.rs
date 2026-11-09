//! The one session recovery transport path, with real gateway authentication.
//! Worker runtime facts come from the existing software fixture; no engine,
//! cluster, provider, real credential or external destination is used.
#[allow(dead_code)]
#[path = "support/relay_admission_fixture.rs"]
mod relay_admission_fixture;
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_control_router, gateway_router},
    audit::Store,
    control::*,
    control_transport::{ControlQuery, ControlRouteIdentity, control_query, control_route},
    ingress::ingress_router_with_control,
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    redaction::Redactor,
    relay::RelayEndpointV1,
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
    routing::any,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::Poll,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;
use uuid::Uuid;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const RUNNER: &str = "public-session-transport-runner-fixture-20261108";
const VIEWER: &str = "public-session-transport-viewer-fixture-20261108";
const WORKER: &str = "public-session-transport-worker-fixture-20261108";
const ID: &str = "11111111-1111-4111-8111-111111111111";
const NIL: &str = "00000000-0000-0000-0000-000000000000";
const PATH: &str =
    "/api/v1/kubernetes/session-reservations/11111111-1111-4111-8111-111111111111/control-recovery";
const GUARD: Duration = Duration::from_secs(30);

#[test]
fn only_the_exact_v1_post_session_recovery_path_is_runner_transport() {
    assert_eq!(
        control_route(&Method::POST, PATH),
        Some(ControlRouteIdentity::Runner)
    );
    // The existing transport helper accepts canonical nil. The session handler
    // separately refuses nil after real authentication; routing grants nothing.
    assert_eq!(
        control_route(&Method::POST, &PATH.replace(ID, NIL)),
        Some(ControlRouteIdentity::Runner)
    );
    for method in [
        Method::GET,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::HEAD,
        Method::OPTIONS,
        Method::CONNECT,
        Method::TRACE,
    ] {
        assert_eq!(control_route(&method, PATH), None, "{method}");
    }
    for path in [
        PATH.replacen("/api/v1/", "/api/", 1),
        format!("/control{PATH}"),
        format!("{PATH}/"),
        format!("{PATH}/extra"),
        PATH.replace("control-recovery", "reserve"),
        PATH.replace(ID, "not-a-uuid"),
        PATH.replace(ID, "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA"),
        PATH.replace(ID, "11111111111141118111111111111111"),
        PATH.replace(ID, "%31111111-1111-4111-8111-111111111111"),
        PATH.replace(ID, "a%2Fb"),
        PATH.replace(ID, ".."),
    ] {
        assert_eq!(control_route(&Method::POST, &path), None, "{path}");
    }
    assert_eq!(
        control_query(&Method::POST, PATH, None),
        Ok(ControlQuery::None)
    );
    for query in ["", "tenant=tenant-a", "action=observe_reserved"] {
        assert!(control_query(&Method::POST, PATH, Some(query)).is_err());
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    store: Arc<Store>,
    state: GatewayState,
}

fn fixture() -> Fixture {
    let directory = support::private_tempdir();
    let store = relay_admission_fixture::fresh(directory.path(), "tenant-a", 10_000);
    let worker = relay_admission_fixture::running_now(&store);
    relay_admission_fixture::bind(&store, &worker, WORKER);
    let store = Arc::new(store);
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "tenant-a".into(),
            store: Arc::clone(&store),
            runner_token: RUNNER.into(),
            viewer_token: VIEWER.into(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    Fixture {
        _directory: directory,
        store,
        state,
    }
}

fn pending_body(polls: Arc<AtomicUsize>) -> Body {
    Body::from_stream(futures_util::stream::poll_fn(move |_| {
        polls.fetch_add(1, Ordering::SeqCst);
        Poll::<Option<Result<axum::body::Bytes, std::io::Error>>>::Pending
    }))
}

#[tokio::test]
async fn real_control_routers_refuse_anonymous_viewer_and_worker_before_body() {
    let fixture = fixture();
    let head = fixture.store.head().unwrap();
    let mut observed = Vec::new();
    for (router, prefix) in [
        (gateway_control_router(fixture.state.clone()), ""),
        (gateway_router(fixture.state.clone()), "/control"),
    ] {
        for (token, expected) in [
            (None, StatusCode::UNAUTHORIZED),
            (Some(VIEWER), StatusCode::FORBIDDEN),
            (Some(WORKER), StatusCode::UNAUTHORIZED),
        ] {
            let polls = Arc::new(AtomicUsize::new(0));
            let mut request = Request::builder()
                .method(Method::POST)
                .uri(format!("{prefix}{PATH}"))
                .header(header::CONTENT_TYPE, "application/json");
            if let Some(token) = token {
                request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
            }
            let response = tokio::time::timeout(
                GUARD,
                router
                    .clone()
                    .oneshot(request.body(pending_body(Arc::clone(&polls))).unwrap()),
            )
            .await
            .expect("HARNESS: control identity refusal awaited a peer body")
            .unwrap();
            observed.push((response.status(), expected, polls.load(Ordering::SeqCst)));
        }
    }
    assert_eq!(fixture.store.head().unwrap(), head);
    for (actual, expected, polls) in observed {
        assert_eq!(
            actual, expected,
            "the real identity guard must run, not an unknown-route 404"
        );
        assert_eq!(polls, 0, "refused identities cannot consume the body");
    }
}

struct Server {
    origin: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Server {
    async fn start(router: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            origin,
            stop: Some(stop),
            task: Some(task),
        }
    }

    async fn shutdown(mut self) {
        let _ = self.stop.take().unwrap().send(());
        let mut task = self.task.take().unwrap();
        match tokio::time::timeout(GUARD, &mut task).await {
            Ok(result) => result.unwrap(),
            Err(_) => {
                task.abort();
                let _ = task.await;
                panic!("HARNESS: owned loopback server did not stop");
            }
        }
    }
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

fn counting_public(calls: Arc<AtomicUsize>) -> Router {
    Router::new().fallback(any(move || {
        let calls = Arc::clone(&calls);
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            StatusCode::IM_A_TEAPOT
        }
    }))
}

async fn response_headers(stream: &mut tokio::net::TcpStream) -> String {
    tokio::time::timeout(GUARD, async {
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            assert!(bytes.len() < 16 * 1024, "bounded loopback response headers");
            bytes.push(stream.read_u8().await.unwrap());
        }
        String::from_utf8(bytes).unwrap()
    })
    .await
    .expect("HARNESS: refusal headers unavailable before the peer sends a body")
}

#[tokio::test]
async fn real_relay_and_gateway_reject_identity_before_peer_sends_session_body() {
    let fixture = fixture();
    let head = fixture.store.head().unwrap();
    let public_calls = Arc::new(AtomicUsize::new(0));
    let public = Server::start(counting_public(Arc::clone(&public_calls))).await;
    let control = Server::start(gateway_control_router(fixture.state.clone())).await;
    let relay =
        Server::start(ingress_router_with_control(&public.origin, &control.origin).unwrap()).await;
    let mut observed = Vec::new();
    for (token, status) in [(None, 401), (Some(VIEWER), 403), (Some(WORKER), 401)] {
        let mut stream = tokio::net::TcpStream::connect(relay.origin.trim_start_matches("http://"))
            .await
            .unwrap();
        let authorization = token
            .map(|token| format!("Authorization: Bearer {token}\r\n"))
            .unwrap_or_default();
        let headers = format!(
            "POST /control{PATH} HTTP/1.1\r\nHost: fixture\r\n{authorization}Content-Type: application/json\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        observed.push((response_headers(&mut stream).await, status));
        // The peer deliberately never supplies the declared request body.
    }
    relay.shutdown().await;
    control.shutdown().await;
    public.shutdown().await;
    assert_eq!(
        public_calls.load(Ordering::SeqCst),
        0,
        "control cannot fall back to public ingress"
    );
    assert_eq!(fixture.store.head().unwrap(), head);
    for (headers, status) in observed {
        assert!(
            headers.starts_with(&format!("HTTP/1.1 {status} ")),
            "must reach real gateway authentication: {headers}"
        );
    }
}

// This final oracle targets the real S2 handler after Root composes S2. It uses
// no new S2 symbol, stub or fixture constructor, so tests-only still compiles
// against the older base. The base without S2 cannot satisfy this handler test.
#[tokio::test]
async fn real_session_handler_is_reached_through_control_transport_after_s2_composition() {
    let fixture = fixture();
    let head = fixture.store.head().unwrap();
    let public_calls = Arc::new(AtomicUsize::new(0));
    let public = Server::start(counting_public(Arc::clone(&public_calls))).await;
    let control = Server::start(gateway_control_router(fixture.state.clone())).await;
    let relay =
        Server::start(ingress_router_with_control(&public.origin, &control.origin).unwrap()).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(GUARD)
        .build()
        .unwrap();
    let inputs = [
        json!({"version":2,"action":"observe_reserved","reservation_input_hash":HASH_A}),
        json!({"version":2,"action":"close_before_effect","close":{
            "version":2,"operation_id":"22222222-2222-4222-8222-222222222222",
            "reservation_id":ID,"reservation_input_hash":HASH_A,"expected_revision":1,
            "reason":"operator_cancelled"
        }}),
        json!({"version":2,"action":"lookup_close_receipt","reservation_input_hash":HASH_A,
            "close_input_hash":HASH_B,"operation_id":"22222222-2222-4222-8222-222222222222"}),
    ];
    let mut observed = Vec::new();
    for input in inputs {
        let response = client
            .post(format!("{}/control{PATH}", relay.origin))
            .bearer_auth(RUNNER)
            .header(header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&input).unwrap())
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
        observed.push((status, body));
    }
    let response = client
        .post(format!("{}/control{}", relay.origin, PATH.replace(ID, NIL)))
        .bearer_auth(RUNNER)
        .header(header::CONTENT_TYPE, "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    let nil_status = response.status();
    let nil_body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    drop(client);
    relay.shutdown().await;
    control.shutdown().await;
    public.shutdown().await;
    assert_eq!(public_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.store.head().unwrap(), head);
    for (status, body) in observed {
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            body["error"]["code"], "session_control_recovery_forbidden",
            "the real closed DTO and disabled S2 policy must be reached"
        );
    }
    assert_eq!(nil_status, StatusCode::BAD_REQUEST);
    assert_eq!(
        nil_body["error"]["code"], "invalid_session_control_recovery_request",
        "nil stays invalid in the authenticated session handler"
    );
}
