use super::*;
use crate::build_identity::{BuildStatusV1, Component, MAX_STATUS_BYTES};
use std::sync::mpsc;
use tower::ServiceExt;

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), MAX_STATUS_BYTES).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn owned_build_status_is_public_without_store_work_and_is_not_health() {
    let fixture = crate::human_authority::migration::tests::Fixture::new_owned();
    let (bundle, credentials, tokens) = fixture.owned_startup();
    let mut state = GatewayState::for_owned_human_sources(
        bundle, credentials, tokens, Policy::default(), None, Redactor::new(vec![]),
    ).unwrap();
    let observations: Vec<_> = state.tenants.iter()
        .map(|tenant| crate::audit::read_test_observer::observe(&tenant.store))
        .collect();
    state.valid_credentials = false;
    let router = gateway_router(state);
    let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = Arc::clone(&polls);
    let body = Body::from_stream(futures_util::stream::poll_fn(move |_| {
        observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::task::Poll::Pending::<Option<Result<axum::body::Bytes, std::io::Error>>>
    }));
    let response = router.clone().oneshot(Request::builder()
        .uri("/api/v1/status")
        .header("authorization", "Bearer synthetic-unbound-status-credential")
        .body(body).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(polls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(body_json(response).await,
        serde_json::to_value(BuildStatusV1::current(Component::Gateway)).unwrap());
    for observation in &observations {
        assert!(observation.try_recv().is_err());
    }
    let head = router.clone().oneshot(Request::builder().method("HEAD")
        .uri("/api/v1/status").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert!(axum::body::to_bytes(head.into_body(), MAX_STATUS_BYTES).await.unwrap().is_empty());
    let post = router.clone().oneshot(Request::builder().method("POST")
        .uri("/api/v1/status").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
    for observation in &observations {
        assert!(observation.try_recv().is_err());
    }
    let health = router.oneshot(Request::builder().uri("/healthz")
        .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(health.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body_json(health).await, json!({"status":"unavailable","service":"gateway",
        "reason":"configuration_or_audit_unavailable","version":env!("CARGO_PKG_VERSION")}));
}

struct Upstream {
    url: String,
    requests: mpsc::Receiver<(String, HeaderMap)>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Upstream {
    fn drop(&mut self) { self.task.abort(); }
}
async fn upstream(status: StatusCode, bytes: Vec<u8>) -> Upstream {
    let (send, requests) = mpsc::channel();
    let app = Router::new().fallback(move |request: Request| {
        let send = send.clone();
        let bytes = bytes.clone();
        async move {
            send.send((request.uri().to_string(), request.headers().clone())).unwrap();
            let body = if bytes.len() == MAX_STATUS_BYTES + 1 {
                // No Content-Length: exercise the streaming cap as well.
                Body::from_stream(futures_util::stream::iter([
                    Ok::<_, std::convert::Infallible>(axum::body::Bytes::copy_from_slice(&bytes[..MAX_STATUS_BYTES])),
                    Ok(axum::body::Bytes::copy_from_slice(&bytes[MAX_STATUS_BYTES..])),
                ]))
            } else if bytes.is_empty() && status == StatusCode::OK {
                Body::from_stream(futures_util::stream::pending::<Result<axum::body::Bytes, std::io::Error>>())
            } else {
                Body::from(bytes)
            };
            Response::builder().status(status).header(header::CONTENT_TYPE, "application/json")
                .header(header::LOCATION, "/must-not-follow")
                .body(body).unwrap()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    Upstream { url, requests, task }
}

#[tokio::test]
async fn dashboard_local_status_does_not_contact_the_gateway() {
    let server = upstream(StatusCode::INTERNAL_SERVER_ERROR, vec![]).await;
    let router = dashboard_shared_router(&server.url, PathBuf::from("unused-status-front")).unwrap();
    let response = router.clone().oneshot(Request::builder().uri("/api/v1/status")
        .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await,
        serde_json::to_value(BuildStatusV1::current(Component::Dashboard)).unwrap());
    assert!(server.requests.try_recv().is_err());
    let health = router.oneshot(Request::builder().uri("/healthz")
        .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(body_json(health).await, json!({"status":"ok","service":"dashboard"}));
    assert!(server.requests.try_recv().is_err());
}

#[tokio::test]
async fn gateway_status_proxy_is_fixed_and_credential_free_and_preserves_a_different_version() {
    let mut status = serde_json::to_value(BuildStatusV1::current(Component::Gateway)).unwrap();
    status["version"] = json!("0.0.2");
    let server = upstream(StatusCode::OK, serde_json::to_vec(&status).unwrap()).await;
    let router = dashboard_router(&server.url, "synthetic-configured-viewer".into(),
        PathBuf::from("unused-status-front")).unwrap();
    let response = router.oneshot(Request::builder()
        .uri("/api/v1/gateway/status?upstream=http://untrusted.invalid/")
        .header(header::AUTHORIZATION, "Bearer synthetic-user-credential")
        .header(header::COOKIE, "synthetic-user-cookie")
        .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await, status);
    let (path, headers) = server.requests.try_recv().unwrap();
    assert_eq!(path, "/api/v1/status");
    assert!(!headers.contains_key(header::AUTHORIZATION));
    assert!(!headers.contains_key(header::COOKIE));
    assert!(server.requests.try_recv().is_err());
}

#[tokio::test]
async fn gateway_status_proxy_refuses_redirect_invalid_shape_and_oversized_response() {
    let current = serde_json::to_vec(&BuildStatusV1::current(Component::Gateway)).unwrap();
    let foreign = serde_json::to_vec(&BuildStatusV1::current(Component::Dashboard)).unwrap();
    for (code, bytes) in [
        (StatusCode::FOUND, current),
        (StatusCode::OK, foreign),
        (StatusCode::OK, b"{\"schema_version\":1}".to_vec()),
        (StatusCode::OK, vec![b' '; MAX_STATUS_BYTES + 1]),
        (StatusCode::OK, vec![b' '; MAX_STATUS_BYTES + 2]),
    ] {
        let server = upstream(code, bytes).await;
        let router = dashboard_shared_router(&server.url, PathBuf::from("unused-status-front")).unwrap();
        let response = router.oneshot(Request::builder().uri("/api/v1/gateway/status")
            .body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = body_json(response).await;
        assert!(body.get("version").is_none());
        assert!(body.get("error").is_some());
        assert_eq!(server.requests.try_recv().unwrap().0, "/api/v1/status");
        assert!(server.requests.try_recv().is_err());
    }
}

#[tokio::test]
async fn gateway_status_proxy_bounds_a_stalled_body_without_fabricating_a_version() {
    let server = upstream(StatusCode::OK, vec![]).await;
    let router = dashboard_shared_router(&server.url, PathBuf::from("unused-status-front")).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(30), router.oneshot(Request::builder()
        .uri("/api/v1/gateway/status").body(Body::empty()).unwrap()))
        .await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert!(body_json(response).await.get("version").is_none());
}
