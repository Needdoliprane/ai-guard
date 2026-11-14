use super::races::{connection, context, snapshot, state};
use super::*;
use std::os::unix::fs::PermissionsExt;

struct LoopbackGateway {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
}
impl LoopbackGateway {
    async fn close(mut self) {
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let router = gateway_router(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let server = LoopbackGateway {
        stop: Some(stop),
        task: Some(task),
    };
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
    for (method, path) in [
        ("HEAD", "/api/v1/human/events"),
        ("GET", "/api/v1/human/export"),
    ] {
        let before = snapshot(&database);
        let response = dashboard
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(path)
                    .header("authorization", format!("Bearer {}", Fixture::bearer("B")))
                    .header("x-aiguard-context", &context)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(snapshot(&database), before);
    }
    server.close().await;
}

const OWNED_PROXY_BEARER: &str = "synthetic-owned-proxy-human-proof";
const OWNED_PROXY_CONTEXT: &str = "synthetic-owned-proxy-context";

/// Transport canary only. The upstream deliberately refuses the read; it is
/// not a synthetic successful owned projection or a substitute for Store ACLs.
async fn owned_brokers_proxy() -> (
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
            "/api/v1/human/owned/brokers",
            axum::routing::get(move |request: axum::extract::Request| {
                let received = received.clone();
                async move {
                    received.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(request.method(), "GET");
                    assert_eq!(request.uri().query(), Some("limit=25"));
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
    let server = LoopbackGateway {
        stop: Some(stop),
        task: Some(task),
    };
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_dashboard_brokers_forwards_exact_get_context_and_gateway_refusal_without_identity_probe()
 {
    let (server, dashboard, calls, unexpected, _front) = owned_brokers_proxy().await;
    let response = dashboard
        .oneshot(
            axum::http::Request::get("/api/v1/human/owned/brokers?limit=25")
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
async fn owned_dashboard_brokers_keeps_head_mutations_legacy_and_neighbor_routes_closed() {
    let (server, dashboard, calls, unexpected, _front) = owned_brokers_proxy().await;
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
            "GET",
            "/api/v1/human/workers",
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
