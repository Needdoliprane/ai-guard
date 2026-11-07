//! Loopback HTTP checks of the explicit control transport. The upstream checks
//! are fixtures; real Rust identities and durable mutation receipts must be
//! tested separately at the gateway boundary. No engine or provider is contacted.
use agentsandbox::{
    control_transport::{
        ControlRouteIdentity, ControlTransportFailureReasonV1, ControlTransportFailureV1,
        control_route,
    },
    ingress::ingress_router_with_control,
};
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::Request,
    http::{Method, StatusCode, header},
    routing::any,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const GUARD: Duration = Duration::from_secs(30);
const BODY_LIMIT: usize = 262_144;
const TOKEN: &str = "fixture-opaque-control-value-not-a-real-credential";
const PATH: &str = "/control/api/v1/control/events";

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(router: Router) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Server { url, task }
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(GUARD)
        .build()
        .unwrap()
}
async fn read_headers(stream: &mut tokio::net::TcpStream) -> String {
    tokio::time::timeout(GUARD, async {
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            assert!(bytes.len() < 16 * 1024, "bounded fixture response headers");
            bytes.push(stream.read_u8().await.unwrap());
        }
        String::from_utf8(bytes).unwrap()
    })
    .await
    .expect("HARNESS: loopback response headers unavailable")
}
fn counting_public(calls: Arc<AtomicUsize>) -> Router {
    Router::new().fallback(any(move || {
        let calls = calls.clone();
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            StatusCode::IM_A_TEAPOT
        }
    }))
}

#[test]
fn closed_route_table_rejects_other_authorities_and_path_aliases() {
    assert_eq!(
        control_route(&Method::POST, "/api/v1/workers/not-a-uuid/transitions"),
        Some(ControlRouteIdentity::Runner)
    ); // UUID validation remains after identity in the gateway handler.
    assert_eq!(
        control_route(&Method::POST, "/api/v1/brokers/heartbeat"),
        Some(ControlRouteIdentity::BrokerPresence)
    );
    for path in [
        "/api/v1/events",
        "/api/events",
        "/api/v1/stream",
        "/auth/oidc/login",
        "/api/v1/relay/messages",
        "/api/v1/brokers/kubernetes/held-creates",
        "/api/v1/human/context",
        "/api//v1/control/events",
        "/api/v1/control/events/",
        "/api/v1/workers/../transitions",
        "/api/v1/workers/%2e%2e/transitions",
        "/api/v1/workers/a%2Fb/transitions",
        "/api/v1/workers/a\\b/transitions",
    ] {
        assert_eq!(control_route(&Method::POST, path), None, "{path}");
    }
    assert_eq!(
        control_route(&Method::CONNECT, "/api/v1/control/events"),
        None
    );
    assert_eq!(control_route(&Method::GET, "/api/v1/control/events"), None);
}

#[tokio::test]
async fn headers_reach_gateway_and_refusal_returns_without_client_body() {
    let calls = Arc::new(AtomicUsize::new(0));
    let control_calls = calls.clone();
    let control = serve(Router::new().fallback(any(move |request: Request| {
        let calls = control_calls.clone();
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.uri().path(), "/api/v1/control/events");
            // Deliberately do not poll a body: this fixture models the ordering
            // required of the real gateway's identity extractor.
            StatusCode::UNAUTHORIZED
        }
    })))
    .await;
    let public_calls = Arc::new(AtomicUsize::new(0));
    let public = serve(counting_public(public_calls.clone())).await;
    let relay = serve(ingress_router_with_control(&public.url, &control.url).unwrap()).await;
    let address = relay.url.trim_start_matches("http://");
    for authorization in ["", "Authorization: Bearer fixture-denied\r\n"] {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let headers = format!(
            "POST {PATH} HTTP/1.1\r\nHost: fixture\r\n{authorization}Content-Type: application/json\r\nContent-Length: 1000\r\n\r\n"
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        assert!(read_headers(&mut stream).await.starts_with("HTTP/1.1 401"));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(public_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn control_transits_opaque_proof_to_fixed_origin_and_does_not_follow_redirect() {
    let redirect_calls = Arc::new(AtomicUsize::new(0));
    let redirect = serve(counting_public(redirect_calls.clone())).await;
    let redirect_url = redirect.url.clone();
    let control = serve(Router::new().fallback(any(move |request: Request| {
        let redirect_url = redirect_url.clone();
        async move {
            assert_eq!(
                request.headers()[header::AUTHORIZATION],
                format!("Bearer {TOKEN}")
            );
            assert_eq!(request.uri().path(), "/api/v1/control/events");
            assert!(!request.headers().contains_key("x-forwarded-host"));
            assert!(
                request.headers()[header::HOST]
                    .to_str()
                    .unwrap()
                    .starts_with("127.0.0.1:")
            );
            let bytes = to_bytes(request.into_body(), BODY_LIMIT).await.unwrap();
            assert_eq!(bytes, "{\"fixture\":true}");
            (
                StatusCode::TEMPORARY_REDIRECT,
                [(header::LOCATION, redirect_url)],
            )
        }
    })))
    .await;
    let public_calls = Arc::new(AtomicUsize::new(0));
    let public = serve(counting_public(public_calls.clone())).await;
    let relay = serve(ingress_router_with_control(&public.url, &control.url).unwrap()).await;
    let response = client()
        .post(format!("{}{PATH}", relay.url))
        .bearer_auth(TOKEN)
        .header(header::HOST, "untrusted.invalid")
        .header("x-forwarded-host", "untrusted.invalid")
        .body("{\"fixture\":true}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(redirect_calls.load(Ordering::SeqCst), 0);
    assert_eq!(public_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn body_limit_and_real_truncation_never_reach_fixture_application() {
    let applications = Arc::new(AtomicUsize::new(0));
    let applied = applications.clone();
    let control = serve(Router::new().fallback(any(move |request: Request| {
        let applied = applied.clone();
        async move {
            match to_bytes(request.into_body(), BODY_LIMIT).await {
                Ok(_) => {
                    applied.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
                Err(_) => StatusCode::BAD_REQUEST,
            }
        }
    })))
    .await;
    let public_calls = Arc::new(AtomicUsize::new(0));
    let public = serve(counting_public(public_calls.clone())).await;
    let relay = serve(ingress_router_with_control(&public.url, &control.url).unwrap()).await;
    // An unknown-size/chunked body defeats a Content-Length-only screen.
    let stream = futures_util::stream::iter([
        Ok::<_, std::io::Error>(Bytes::from(vec![b'a'; BODY_LIMIT])),
        Ok(Bytes::from_static(b"b")),
    ]);
    let response = client()
        .post(format!("{}{PATH}", relay.url))
        .bearer_auth(TOKEN)
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(applications.load(Ordering::SeqCst), 0);

    let mut raw = tokio::net::TcpStream::connect(relay.url.trim_start_matches("http://"))
        .await
        .unwrap();
    raw.write_all(
        format!("POST {PATH} HTTP/1.1\r\nHost: fixture\r\nConnection: close\r\nContent-Length: 20\r\n\r\n{{}}")
            .as_bytes(),
    )
    .await
    .unwrap();
    raw.shutdown().await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(GUARD, raw.take(16 * 1024).read_to_end(&mut response))
        .await
        .expect("HARNESS: truncated request did not settle")
        .unwrap();
    // HTTP may close the already truncated connection without a response.
    // Neither a normal reply nor a partial application is admissible.
    assert!(response.is_empty() || response.starts_with(b"HTTP/1.1 400"));
    assert_eq!(applications.load(Ordering::SeqCst), 0);

    let response = client()
        .post(format!("{}{PATH}", relay.url))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(applications.load(Ordering::SeqCst), 1);
    assert_eq!(public_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unavailable_control_never_falls_back_or_echoes_opaque_headers() {
    let public_calls = Arc::new(AtomicUsize::new(0));
    let public = serve(counting_public(public_calls.clone())).await;
    let unused = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unavailable = format!("http://{}", unused.local_addr().unwrap());
    drop(unused);
    let relay = serve(ingress_router_with_control(&public.url, &unavailable).unwrap()).await;
    let response = client()
        .post(format!("{}{PATH}", relay.url))
        .bearer_auth(TOKEN)
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = response.text().await.unwrap();
    let wire: serde_json::Value = serde_json::from_str(&body).unwrap();
    let typed: ControlTransportFailureV1 = serde_json::from_value(wire["error"].clone()).unwrap();
    assert!(typed.validate());
    assert_eq!(
        typed.reason,
        ControlTransportFailureReasonV1::UpstreamConnectionUnavailable
    );
    for opaque in [TOKEN, unavailable.as_str(), "Bearer"] {
        assert!(!body.contains(opaque));
    }
    for path in [
        "/control/api/v1/events",
        "/control/api/v1/stream",
        "/control",
    ] {
        let response = client()
            .post(format!("{}{path}", relay.url))
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(public_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancelling_control_request_releases_its_pending_body() {
    struct BodyOwner(Option<tokio::sync::oneshot::Sender<()>>);
    impl Drop for BodyOwner {
        fn drop(&mut self) {
            if let Some(dropped) = self.0.take() {
                let _ = dropped.send(());
            }
        }
    }
    let (entered, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let control = serve(Router::new().fallback(any(move |request: Request| {
        let entered = entered.clone();
        async move {
            let _ = entered.send(());
            match to_bytes(request.into_body(), BODY_LIMIT).await {
                Ok(_) => StatusCode::OK,
                Err(_) => StatusCode::BAD_REQUEST,
            }
        }
    })))
    .await;
    let public_calls = Arc::new(AtomicUsize::new(0));
    let public = serve(counting_public(public_calls.clone())).await;
    let relay = ingress_router_with_control(&public.url, &control.url).unwrap();
    let (dropped, drop_rx) = tokio::sync::oneshot::channel();
    let owner = BodyOwner(Some(dropped));
    let (polled, poll_rx) = tokio::sync::oneshot::channel();
    let mut polled = Some(polled);
    let stream = futures_util::stream::poll_fn(move |_| {
        let _retained = &owner;
        if let Some(polled) = polled.take() {
            let _ = polled.send(());
        }
        std::task::Poll::<Option<Result<Bytes, std::io::Error>>>::Pending
    });
    let mut task = tokio::task::JoinSet::new();
    let held = relay.clone();
    task.spawn(async move {
        use tower::ServiceExt;
        held.oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(PATH)
                .body(Body::from_stream(stream))
                .unwrap(),
        )
        .await
        .unwrap()
    });
    tokio::time::timeout(GUARD, async {
        poll_rx.await.unwrap();
        entered_rx.recv().await.unwrap();
    })
    .await
    .expect("HARNESS: owned request did not reach pending upload");
    assert!(
        task.try_join_next().is_none(),
        "HARNESS: upload expired before cancellation"
    );
    task.abort_all();
    while task.join_next().await.is_some() {}
    tokio::time::timeout(GUARD, drop_rx)
        .await
        .expect("cancelled request retained its body")
        .unwrap();
    let relay = serve(relay).await;
    assert_eq!(
        client()
            .post(format!("{}{PATH}", relay.url))
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(public_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn real_truncated_upstream_reply_cannot_be_a_complete_receipt() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_headers(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\n{}")
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    });
    let control = Server { url, task };
    let public_calls = Arc::new(AtomicUsize::new(0));
    let public = serve(counting_public(public_calls.clone())).await;
    let relay = serve(ingress_router_with_control(&public.url, &control.url).unwrap()).await;
    let response = client()
        .get(format!("{}/control/api/v1/control/protocol", relay.url))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = response.text().await.unwrap();
    assert!(!body.contains(TOKEN));
    assert!(!body.contains(&control.url));
    let wire: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(wire["error"]["code"], "control_transport_protocol_failure");
    assert!(serde_json::from_value::<ControlTransportFailureV1>(wire["error"].clone()).is_err());
    assert_eq!(public_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn transport_failure_wire_is_closed_and_requires_exact_version_and_metadata() {
    let good = ControlTransportFailureV1::new(ControlTransportFailureReasonV1::UpstreamTimeout);
    assert!(good.validate());
    let wire = serde_json::to_value(good).unwrap();
    for (field, value) in [
        ("version", serde_json::json!(2)),
        ("code", serde_json::json!("audit_unavailable")),
        ("outcome", serde_json::json!("rolled_back")),
        ("phase", serde_json::json!("audit")),
        ("retry_action", serde_json::json!("retry_start")),
    ] {
        let mut changed = wire.clone();
        changed[field] = value;
        assert!(
            !serde_json::from_value::<ControlTransportFailureV1>(changed)
                .unwrap()
                .validate()
        );
    }
    for (field, value) in [
        ("reason", serde_json::json!("certificate_invalid")),
        ("extra", serde_json::json!("fixture")),
    ] {
        let mut changed = wire.clone();
        changed[field] = value;
        assert!(serde_json::from_value::<ControlTransportFailureV1>(changed).is_err());
    }
}

#[tokio::test]
async fn real_http_framing_is_canonical_and_truncation_does_not_dispatch() {
    let applications = Arc::new(AtomicUsize::new(0));
    let applied = applications.clone();
    let control = serve(Router::new().fallback(any(move |request: Request| {
        let applied = applied.clone();
        async move {
            assert!(!request.headers().contains_key(header::CONTENT_LENGTH));
            match to_bytes(request.into_body(), BODY_LIMIT).await {
                Ok(bytes) => {
                    assert_eq!(bytes, "{}");
                    applied.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
                Err(_) => StatusCode::BAD_REQUEST,
            }
        }
    })))
    .await;
    let public_calls = Arc::new(AtomicUsize::new(0));
    let public = serve(counting_public(public_calls.clone())).await;
    let relay = serve(ingress_router_with_control(&public.url, &control.url).unwrap()).await;
    let cases = [
        (
            "Content-Length: 99\r\nTransfer-Encoding: chunked\r\n",
            "2\r\n{}\r\n0\r\n\r\n",
            true,
        ),
        ("Content-Length: 2\r\nContent-Length: 2\r\n", "{}", true),
        ("Content-Length: 2\r\nContent-Length: 3\r\n", "{}", false),
        ("Transfer-Encoding: chunked\r\n", "5\r\n{}", false),
    ];
    let mut expected_applications = 0;
    for (framing, body, complete) in cases {
        let mut raw = tokio::net::TcpStream::connect(relay.url.trim_start_matches("http://"))
            .await
            .unwrap();
        raw.write_all(
            format!(
                "POST {PATH} HTTP/1.1\r\nHost: fixture\r\nConnection: close\r\n{framing}\r\n{body}"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        // Only the last case is deliberately cut mid-chunk.
        if !complete {
            raw.shutdown().await.unwrap();
        }
        let mut response = Vec::new();
        tokio::time::timeout(GUARD, raw.take(16 * 1024).read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        if complete {
            assert!(response.starts_with(b"HTTP/1.1 200"));
            expected_applications += 1;
        } else {
            assert!(response.is_empty() || response.starts_with(b"HTTP/1.1 400"));
        }
        assert_eq!(applications.load(Ordering::SeqCst), expected_applications);
    }
    // Hyper removes CL when TE wins and collapses identical CL fields; we
    // verify unambiguous bounded reframing, not rejection of those normalized
    // original bytes which the application never receives.
    assert_eq!(public_calls.load(Ordering::SeqCst), 0);
}
