use super::races::{connection, context, snapshot, state};
use super::*;
use std::os::unix::fs::PermissionsExt;

pub(super) struct LoopbackGateway {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
}
impl LoopbackGateway {
    pub(super) async fn start(router: Router) -> (Self, std::net::SocketAddr) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        (
            Self {
                stop: Some(stop),
                task: Some(task),
            },
            address,
        )
    }

    pub(super) async fn close(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let task = self.task.as_mut().unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let _ = self.task.take();
    }
}
impl Drop for LoopbackGateway {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_dashboard_events_metadata_forwards_exact_get_without_identity_probe() {
    // The real dashboard must forward this exact owned GET once. A hidden
    // identity preflight would append two receipts instead of one.
    let fixture = Fixture::new_owned();
    let event = seed_event(&fixture, "B", "owned-dashboard-b", fixture.principal());
    let state = state(&fixture);
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let router = gateway_router(state.clone()).layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            observed.fetch_add(1, Ordering::SeqCst);
            async move { next.run(request).await }
        },
    ));
    let (server, address) = LoopbackGateway::start(router).await;
    let front = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    std::fs::write(
        front.path().join("index.html"),
        "synthetic journal dashboard",
    )
    .unwrap();
    let dashboard = dashboard_router_with_human_mode(
        &format!("http://{address}"),
        None,
        front.path().to_owned(),
        None,
        true,
    )
    .unwrap();
    let context = context(&dashboard).await;
    let database = connection(&state);
    let before = snapshot(&database);
    let (status, page) = get(&dashboard, "/api/v1/human/events", Some(&context)).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["metadata"]["id"], event);
    assert!(page["items"][0]["content"].is_null());
    assert!(page.get("live_cursor").is_none());
    assert_eq!(snapshot(&database).0, before.0 + 1);
    let action: String = database
        .query_row(
            "SELECT json_extract(payload,'$.data.action') FROM events WHERE id=?1",
            [page["read_receipt"]["audit_event_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(action, "events");

    let detail_path = format!("/api/v1/human/events/{event}");
    let before = snapshot(&database);
    let calls_before = calls.load(Ordering::SeqCst);
    let (status, detail) = get(
        &dashboard,
        &format!("{detail_path}?view=metadata"),
        Some(&context),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["items"], page["items"]);
    assert_eq!(detail["context_id"], page["context_id"]);
    assert_eq!(
        detail["authorization_version"],
        page["authorization_version"]
    );
    assert_eq!(detail["view"], "metadata");
    assert_eq!(snapshot(&database).0, before.0 + 1);
    let action: String = database
        .query_row(
            "SELECT json_extract(payload,'$.data.action') FROM events WHERE id=?1",
            [detail["read_receipt"]["audit_event_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(action, "event");
    assert_eq!(calls.load(Ordering::SeqCst), calls_before + 1);

    let noncanonical = format!("/api/v1/human/events/{}", event.replace('-', ""));
    let before = snapshot(&database);
    let (status, refused) = get(
        &gateway_router(state.clone()),
        &noncanonical,
        Some(&context),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert_eq!(refused["error"]["code"], "not_found");
    assert!(refused.get("items").is_none());
    assert_eq!(snapshot(&database), before);

    let calls_before = calls.load(Ordering::SeqCst);
    for (method, path) in [
        ("HEAD", "/api/v1/human/events".to_owned()),
        ("HEAD", detail_path.clone()),
        ("GET", format!("{detail_path}?view=content")),
        (
            "GET",
            format!("{detail_path}?view=metadata&cursor=synthetic"),
        ),
        ("GET", noncanonical),
        ("GET", format!("/api/human/events/{event}")),
        ("GET", "/api/v1/human/export".to_owned()),
        ("GET", "/api/v1/human/stream".to_owned()),
    ] {
        let before = snapshot(&database);
        let response = dashboard
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(&path)
                    .header("authorization", format!("Bearer {}", Fixture::bearer("B")))
                    .header("x-aiguard-context", &context)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{method} {path}"
        );
        assert_eq!(snapshot(&database), before);
    }
    assert_eq!(calls.load(Ordering::SeqCst), calls_before);
    server.close().await;
}

const OWNED_PROXY_BEARER: &str = "synthetic-owned-proxy-human-proof";
const OWNED_PROXY_CONTEXT: &str = "synthetic-owned-proxy-context";

/// Transport canary only. The upstream deliberately refuses the read; it is
/// not a synthetic successful owned projection or a substitute for Store ACLs.
async fn owned_read_proxy(
    path: &'static str,
    query: Option<&'static str>,
) -> (
    LoopbackGateway,
    Router,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    tempfile::TempDir,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let unexpected = Arc::new(AtomicUsize::new(0));
    let received = calls.clone();
    let other = unexpected.clone();
    let router = Router::new()
        .route(
            path,
            axum::routing::get(move |request: axum::extract::Request| {
                let received = received.clone();
                async move {
                    received.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(request.method(), "GET");
                    assert_eq!(request.uri().query(), query);
                    assert_eq!(
                        request
                            .headers()
                            .get("authorization")
                            .unwrap()
                            .to_str()
                            .unwrap(),
                        format!("Bearer {OWNED_PROXY_BEARER}")
                    );
                    assert_eq!(
                        request
                            .headers()
                            .get("x-aiguard-context")
                            .unwrap()
                            .to_str()
                            .unwrap(),
                        OWNED_PROXY_CONTEXT
                    );
                    (
                        StatusCode::FORBIDDEN,
                        axum::Json(
                            json!({"error":{"code":"permission_denied","next":"select_context"}}),
                        ),
                    )
                }
            }),
        )
        .fallback(move || {
            let other = other.clone();
            async move {
                other.fetch_add(1, Ordering::SeqCst);
                StatusCode::SERVICE_UNAVAILABLE
            }
        });
    let (server, address) = LoopbackGateway::start(router).await;
    let front = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    std::fs::write(
        front.path().join("index.html"),
        "synthetic owned proxy canary",
    )
    .unwrap();
    let dashboard = dashboard_router_with_human_mode(
        &format!("http://{address}"),
        None,
        front.path().to_owned(),
        None,
        true,
    )
    .unwrap();
    (server, dashboard, calls, unexpected, front)
}

async fn require_owned_get_is_relayed(path: &'static str, query: Option<&'static str>) {
    let (server, dashboard, calls, unexpected, _front) = owned_read_proxy(path, query).await;
    let uri = match query {
        Some(query) => format!("{path}?{query}"),
        None => path.to_owned(),
    };
    let response = dashboard
        .oneshot(
            axum::http::Request::get(uri)
                .header("authorization", format!("Bearer {OWNED_PROXY_BEARER}"))
                .header("x-aiguard-context", OWNED_PROXY_CONTEXT)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["error"]["code"], "permission_denied");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        unexpected.load(Ordering::SeqCst),
        0,
        "no viewer/identity preflight or legacy fallback"
    );
    server.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_dashboard_brokers_forwards_exact_get_context_and_gateway_refusal_without_identity_probe()
 {
    require_owned_get_is_relayed("/api/v1/human/owned/brokers", Some("limit=25")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_dashboard_workers_forwards_exact_get_context_and_gateway_refusal_without_identity_probe()
 {
    require_owned_get_is_relayed("/api/v1/human/workers", Some("limit=25")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_dashboard_quota_forwards_exact_get_context_and_gateway_refusal_without_identity_probe()
 {
    require_owned_get_is_relayed("/api/v1/human/admin/quota", None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_dashboard_usage_forwards_exact_get_context_and_gateway_refusal_without_identity_probe()
 {
    require_owned_get_is_relayed("/api/v1/human/admin/usage", Some("limit=25")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_dashboard_relays_the_delegated_membership_change_and_gateway_receipt() {
    const MEMBERSHIP: &str = "a20b685f-76bb-4d65-90de-a1d2e4cbbbea";
    const OPERATION: &str = "915e4e07-26a2-4d71-a45f-3b602a8dcf93";
    let path = format!("/api/v1/human/admin/memberships/{MEMBERSHIP}");
    let upstream_path = format!("/api/v1/human/owned/admin/memberships/{MEMBERSHIP}");
    let change = json!({
        "operation_id": OPERATION,
        "expected_revision": "1",
        "capabilities": ["read_metadata", "read_metadata"],
        "active": false,
    });
    let receipt = json!({
        "version": 1,
        "operation_id": OPERATION,
        "membership_id": MEMBERSHIP,
        "membership_revision": "2",
        "authorization_revision": "4",
        "audit_seq": "12",
    });
    let body = format!(" \n{}", serde_json::to_string(&change).unwrap()).into_bytes();
    let expected_body = body.clone();
    let mut changed = change.clone();
    changed["active"] = json!(true);
    let conflicting = serde_json::to_vec(&changed).unwrap();
    let conflicting_body = conflicting.clone();
    let upstream_receipt = receipt.clone();
    let reached = Arc::new(AtomicUsize::new(0));
    let seen = reached.clone();
    let unexpected = Arc::new(AtomicUsize::new(0));
    let other = unexpected.clone();
    let upstream = Router::new()
        .route(
            &upstream_path,
            axum::routing::post(move |request: axum::extract::Request| {
                let body = expected_body.clone();
                let conflicting = conflicting_body.clone();
                let receipt = upstream_receipt.clone();
                let seen = seen.clone();
                async move {
                    assert!(request.uri().query().is_none());
                    let headers = request.headers();
                    assert_eq!(headers.get_all("authorization").iter().count(), 1);
                    assert_eq!(
                        headers["authorization"].to_str().unwrap(),
                        format!("Bearer {OWNED_PROXY_BEARER}")
                    );
                    assert_eq!(headers.get_all("x-aiguard-context").iter().count(), 1);
                    assert_eq!(headers["x-aiguard-context"], OWNED_PROXY_CONTEXT);
                    assert_eq!(headers["content-type"], "application/json");
                    assert!(!headers.contains_key("cookie"));
                    assert!(!headers.contains_key("x-tenant-id"));
                    let received = axum::body::to_bytes(request.into_body(), 4096)
                        .await
                        .unwrap();
                    seen.fetch_add(1, Ordering::SeqCst);
                    if received.as_ref() == body.as_slice() {
                        (StatusCode::OK, axum::Json(receipt))
                    } else {
                        assert_eq!(received.as_ref(), conflicting.as_slice());
                        (
                            StatusCode::CONFLICT,
                            axum::Json(json!({"error":{
                                "code":"conflict",
                                "message":"The membership revision changed.",
                                "retry_action":"reread"
                            }})),
                        )
                    }
                }
            }),
        )
        .fallback(move || {
            let other = other.clone();
            async move {
                other.fetch_add(1, Ordering::SeqCst);
                StatusCode::SERVICE_UNAVAILABLE
            }
        });
    let (server, address) = LoopbackGateway::start(upstream).await;
    let front = tempfile::tempdir().unwrap();
    let dashboard = dashboard_router_with_human_mode(
        &format!("http://{address}"),
        None,
        front.path().to_owned(),
        None,
        true,
    )
    .unwrap();
    let request = |target: &str| {
        axum::http::Request::post(target)
            .header("authorization", format!("Bearer {OWNED_PROXY_BEARER}"))
            .header("content-type", "application/json; charset=utf-8")
    };
    let with_context =
        |target: &str| request(target).header("x-aiguard-context", OWNED_PROXY_CONTEXT);
    for (expected_status, submitted) in [
        (StatusCode::OK, &body),
        (StatusCode::CONFLICT, &conflicting),
    ] {
        let response = dashboard
            .clone()
            .oneshot(
                with_context(&path)
                    .header("cookie", "unrelated=synthetic")
                    .header("x-tenant-id", "unrelated")
                    .body(Body::from(submitted.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected_status);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let encoded = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&encoded).unwrap();
        if expected_status == StatusCode::OK {
            assert_eq!(value, receipt);
        } else {
            assert_eq!(value["error"]["code"], "conflict");
            assert_eq!(value["error"]["retry_action"], "reread");
        }
    }

    let mut oversized = body.clone();
    oversized.resize(4097, b' ');
    let extra_field = serde_json::to_vec(&json!({
        "operation_id": OPERATION, "expected_revision": "1", "capabilities": ["read_metadata"],
        "active": false, "reason": "test annotation"
    }))
    .unwrap();
    for (request, content, status) in [
        (request(&path), body.clone(), StatusCode::FORBIDDEN),
        (
            with_context(&path).header("x-aiguard-context", OWNED_PROXY_CONTEXT),
            body.clone(),
            StatusCode::FORBIDDEN,
        ),
        (
            with_context(&path).header("authorization", format!("Bearer {OWNED_PROXY_BEARER}")),
            body.clone(),
            StatusCode::UNAUTHORIZED,
        ),
        (with_context(&path), oversized, StatusCode::BAD_REQUEST),
        (with_context(&path), extra_field, StatusCode::BAD_REQUEST),
        (
            with_context(&format!("{path}?limit=1")),
            body.clone(),
            StatusCode::BAD_REQUEST,
        ),
        (
            with_context(&format!(
                "/api/v1/human/admin/memberships/{}",
                MEMBERSHIP.to_uppercase()
            )),
            body.clone(),
            StatusCode::BAD_REQUEST,
        ),
        (
            with_context("/api/v1/human/admin/grants"),
            body.clone(),
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            with_context(&upstream_path),
            body.clone(),
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            with_context(&path.replace("/api/v1/", "/api/")),
            body.clone(),
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            axum::http::Request::get(&path),
            Vec::new(),
            StatusCode::METHOD_NOT_ALLOWED,
        ),
    ] {
        let response = dashboard
            .clone()
            .oneshot(request.body(Body::from(content)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        if status == StatusCode::BAD_REQUEST {
            require_invalid_membership_request(response).await;
        }
    }
    for (content_type, content) in invalid_membership_uploads(&change) {
        let mut request = with_context(&path);
        request
            .headers_mut()
            .unwrap()
            .insert("content-type", HeaderValue::from_static(content_type));
        let response = dashboard
            .clone()
            .oneshot(request.body(Body::from(content)).unwrap())
            .await
            .unwrap();
        require_invalid_membership_request(response).await;
    }
    let legacy = dashboard_router_with_human_mode(
        &format!("http://{address}"),
        None,
        front.path().to_owned(),
        None,
        false,
    )
    .unwrap();
    let response = legacy
        .oneshot(with_context(&path).body(Body::from(body)).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(reached.load(Ordering::SeqCst), 2);
    assert_eq!(unexpected.load(Ordering::SeqCst), 0);
    server.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_dashboard_brokers_keeps_head_mutations_legacy_and_neighbor_routes_closed() {
    let (server, dashboard, calls, unexpected, _front) =
        owned_read_proxy("/api/v1/human/owned/brokers", Some("limit=25")).await;
    let unauthenticated = dashboard
        .clone()
        .oneshot(
            axum::http::Request::get("/api/v1/human/owned/brokers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    let bytes = axum::body::to_bytes(unauthenticated.into_body(), 4096)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["error"]["code"], "authentication_required");
    for (method, path, status) in [
        (
            "HEAD",
            "/api/v1/human/owned/brokers",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "POST",
            "/api/v1/human/owned/brokers",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            "DELETE",
            "/api/v1/human/owned/brokers",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            "GET",
            "/api/v1/human/brokers",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "HEAD",
            "/api/v1/human/workers",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "HEAD",
            "/api/v1/human/admin/quota",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "HEAD",
            "/api/v1/human/admin/usage",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "GET",
            "/api/v1/human/owned/workers",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "GET",
            "/api/v1/human/federation/workers",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "GET",
            "/api/v1/federation/workers",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "GET",
            "/api/v1/human/owned/brokers/foreign",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "GET",
            "/api/human/owned/brokers",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let response = dashboard
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(path)
                    .header("authorization", format!("Bearer {OWNED_PROXY_BEARER}"))
                    .header("x-aiguard-context", OWNED_PROXY_CONTEXT)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{method} {path}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(unexpected.load(Ordering::SeqCst), 0);
    server.close().await;
}
