mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    Router,
    body::{Body, Bytes},
    http::{HeaderValue, Method, Request, StatusCode, header},
};
use futures_util::stream;
use http_body_util::BodyExt;
use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::Poll,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    process::{Child, ChildStderr, Command},
};
use tower::ServiceExt;

const WRITE_PATHS: [&str; 9] = [
    "/api/v1/events",
    "/api/v1/events/batch",
    "/api/v1/decide",
    "/api/v1/workers/reservations",
    "/api/v1/workers/00000000-0000-4000-8000-000000000001/transitions",
    "/api/events",
    "/api/decide",
    "/api/workers/reservations",
    "/api/workers/00000000-0000-4000-8000-000000000001/transitions",
];

struct Fixture {
    _directory: tempfile::TempDir,
    router: Router,
    stores: Vec<Arc<Store>>,
    runner_a: String,
    viewer_a: String,
    runner_b: String,
    viewer_b: String,
}

impl Fixture {
    fn new() -> Self {
        let directory = support::private_tempdir();
        let redactor = Redactor::new(vec![]);
        let runner_a = "a".repeat(48);
        let viewer_a = "b".repeat(48);
        let runner_b = "c".repeat(48);
        let viewer_b = "d".repeat(48);
        let store_a = Arc::new(
            Store::open_for_tenant(
                &directory.path().join("a.sqlite"),
                "tenant-a",
                redactor.clone(),
                100,
            )
            .unwrap(),
        );
        let store_b = Arc::new(
            Store::open_for_tenant(
                &directory.path().join("b.sqlite"),
                "tenant-b",
                redactor.clone(),
                100,
            )
            .unwrap(),
        );
        let state = GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: Arc::clone(&store_a),
                    runner_token: runner_a.clone(),
                    viewer_token: viewer_a.clone(),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: Arc::clone(&store_b),
                    runner_token: runner_b.clone(),
                    viewer_token: viewer_b.clone(),
                },
            ],
            Policy::default(),
            None,
            redactor,
        )
        .unwrap();
        Self {
            _directory: directory,
            router: gateway_router(state),
            stores: vec![store_a, store_b],
            runner_a,
            viewer_a,
            runner_b,
            viewer_b,
        }
    }

    fn audit_counts(&self) -> Vec<u64> {
        self.stores
            .iter()
            .map(|store| store.integrity().unwrap().event_count)
            .collect()
    }
}

fn counted_invalid_json_body(polls: Arc<AtomicUsize>) -> Body {
    let mut sent = false;
    Body::from_stream(stream::poll_fn(move |_context| {
        polls.fetch_add(1, Ordering::SeqCst);
        if sent {
            Poll::Ready(None)
        } else {
            sent = true;
            Poll::Ready(Some(Ok::<Bytes, Infallible>(Bytes::from_static(b"{"))))
        }
    }))
}

async fn counted_post(
    router: &Router,
    path: &str,
    authorization: Option<HeaderValue>,
) -> (StatusCode, usize) {
    let polls = Arc::new(AtomicUsize::new(0));
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    let request = builder
        .body(counted_invalid_json_body(Arc::clone(&polls)))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    (response.status(), polls.load(Ordering::SeqCst))
}

fn bearer(value: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("Bearer {value}")).unwrap()
}

#[tokio::test]
async fn missing_invalid_and_viewer_write_auth_reject_before_polling_any_write_body() {
    let fixture = Fixture::new();
    let before = fixture.audit_counts();
    let mut failures = Vec::new();

    for path in WRITE_PATHS {
        let cases = [
            ("missing", None, StatusCode::UNAUTHORIZED),
            (
                "invalid",
                Some(bearer("not-a-configured-credential")),
                StatusCode::UNAUTHORIZED,
            ),
            (
                "viewer-a",
                Some(bearer(&fixture.viewer_a)),
                StatusCode::FORBIDDEN,
            ),
            (
                "viewer-b",
                Some(bearer(&fixture.viewer_b)),
                StatusCode::FORBIDDEN,
            ),
        ];
        for (label, authorization, expected) in cases {
            let (status, body_polls) = counted_post(&fixture.router, path, authorization).await;
            if status != expected {
                failures.push(format!("{path} {label}: expected {expected}, got {status}"));
            }
            if body_polls != 0 {
                failures.push(format!(
                    "{path} {label}: request body polled {body_polls} time(s)"
                ));
            }
        }
    }

    let after = fixture.audit_counts();
    if after != before {
        failures.push(format!(
            "unauthorized writes changed audit counts: {before:?} -> {after:?}"
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn runner_malformed_write_bodies_keep_client_errors_without_audit_mutation() {
    let fixture = Fixture::new();
    let before = fixture.audit_counts();
    let mut failures = Vec::new();

    for path in WRITE_PATHS {
        let request = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", fixture.runner_a),
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from_stream(stream::iter([Ok::<Bytes, Infallible>(
                Bytes::from_static(b"{"),
            )])))
            .unwrap();
        let status = fixture
            .router
            .clone()
            .oneshot(request)
            .await
            .unwrap()
            .status();
        if status != StatusCode::BAD_REQUEST {
            failures.push(format!(
                "{path}: expected runner parse status 400, got {status}"
            ));
        }
    }

    let after = fixture.audit_counts();
    if after != before {
        failures.push(format!(
            "malformed runner writes changed audit counts: {before:?} -> {after:?}"
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn malformed_event_queries_authenticate_first_on_both_api_aliases() {
    let fixture = Fixture::new();
    let mut failures = Vec::new();

    for alias in ["/api/v1", "/api"] {
        for (label, authorization, expected) in [
            ("missing", None, StatusCode::UNAUTHORIZED),
            (
                "invalid",
                Some(bearer("not-a-configured-credential")),
                StatusCode::UNAUTHORIZED,
            ),
            (
                "runner",
                Some(bearer(&fixture.runner_a)),
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let mut builder = Request::builder()
                .uri(format!("{alias}/events?limit=invalid"))
                .method(Method::GET);
            if let Some(value) = authorization {
                builder = builder.header(header::AUTHORIZATION, value);
            }
            let status = fixture
                .router
                .clone()
                .oneshot(builder.body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status();
            if status != expected {
                failures.push(format!(
                    "{alias}/events {label}: expected {expected}, got {status}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn duplicate_authorization_and_oversized_bearer_are_rejected_before_body_polling() {
    let fixture = Fixture::new();
    let mut failures = Vec::new();

    let mut duplicate = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/identity")
        .body(Body::empty())
        .unwrap();
    duplicate
        .headers_mut()
        .append(header::AUTHORIZATION, bearer(&fixture.runner_a));
    duplicate
        .headers_mut()
        .append(header::AUTHORIZATION, bearer(&fixture.runner_b));
    let duplicate_status = fixture
        .router
        .clone()
        .oneshot(duplicate)
        .await
        .unwrap()
        .status();
    if duplicate_status != StatusCode::UNAUTHORIZED {
        failures.push(format!(
            "duplicate Authorization: expected 401, got {duplicate_status}"
        ));
    }

    let oversized = format!("Bearer {}", "x".repeat(4097));
    let (status, polls) = counted_post(
        &fixture.router,
        "/api/v1/events",
        Some(HeaderValue::from_str(&oversized).unwrap()),
    )
    .await;
    if status != StatusCode::UNAUTHORIZED {
        failures.push(format!("oversized bearer: expected 401, got {status}"));
    }
    if polls != 0 {
        failures.push(format!("oversized bearer polled body {polls} time(s)"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn health_stays_public_and_authenticated_identity_selects_tenant_and_role_on_both_aliases() {
    let fixture = Fixture::new();
    let health = fixture
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);

    let cases = [
        (
            "/api/v1/identity?tenant_id=tenant-b",
            &fixture.viewer_a,
            "tenant-a",
            "viewer",
        ),
        (
            "/api/identity?tenant_id=tenant-a",
            &fixture.runner_b,
            "tenant-b",
            "runner",
        ),
        (
            "/api/v1/identity?tenant_id=tenant-a",
            &fixture.viewer_b,
            "tenant-b",
            "viewer",
        ),
    ];
    let mut failures = Vec::new();
    for (path, token, expected_tenant, expected_role) in cases {
        let request = Request::builder()
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header("x-tenant-id", "tenant-b")
            .body(Body::empty())
            .unwrap();
        let response = fixture.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let value = response.into_body().collect().await.unwrap().to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&value).unwrap();
        if status != StatusCode::OK
            || value["tenant_id"] != expected_tenant
            || value["role"] != expected_role
        {
            failures.push(format!("{path}: status={status}, identity={value}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

struct GatewayChild {
    child: Child,
    _stderr: BufReader<ChildStderr>,
    address: SocketAddr,
}

async fn start_gateway_child(directory: &tempfile::TempDir) -> GatewayChild {
    let runner = "r".repeat(48);
    let viewer = "v".repeat(48);
    let runner_file = directory.path().join("runner-token");
    let viewer_file = directory.path().join("viewer-token");
    std::fs::write(&runner_file, runner).unwrap();
    std::fs::write(&viewer_file, viewer).unwrap();
    let database = directory.path().join("gateway.sqlite");
    let policy = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("policies/default.json");
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["gateway", "--listen", "127.0.0.1:0", "--data"])
        .arg(database)
        .arg("--runner-token-file")
        .arg(runner_file)
        .arg("--viewer-token-file")
        .arg(viewer_file)
        .arg("--policy")
        .arg(policy)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    let read = tokio::time::timeout(Duration::from_secs(5), stderr.read_line(&mut line))
        .await
        .expect("gateway child startup timed out")
        .unwrap();
    assert!(read > 0, "gateway child exited before readiness");
    let address = line
        .strip_prefix("Gateway ")
        .and_then(|rest| rest.split_whitespace().next())
        .expect("gateway child did not report its listener address")
        .parse::<SocketAddr>()
        .unwrap();
    assert!(address.ip().is_loopback());
    GatewayChild {
        child,
        _stderr: stderr,
        address,
    }
}

#[tokio::test]
async fn real_tcp_missing_bearer_gets_401_before_client_sends_declared_body() {
    let directory = support::private_tempdir();
    let mut gateway = start_gateway_child(&directory).await;
    let mut socket =
        tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(gateway.address))
            .await
            .expect("TCP connection timed out")
            .unwrap();
    socket
        .write_all(
            b"POST /api/v1/events HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();

    let mut reader = BufReader::new(socket);
    let mut status_line = String::new();
    let read = tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut status_line))
        .await
        .expect("gateway waited for the request body before returning its authorization status")
        .unwrap();
    assert!(
        read > 0,
        "gateway closed the connection without an HTTP status"
    );
    assert!(
        status_line.starts_with("HTTP/1.1 401 ") || status_line.starts_with("HTTP/1.0 401 "),
        "expected 401 before sending any body bytes, got {status_line:?}"
    );
    gateway.child.kill().await.unwrap();
}
