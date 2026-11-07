//! Closed worker transport targets and response bounds, using loopback HTTP
//! only. The upstream is a byte-budget fixture, not an authenticated workload.
use agentsandbox::{
    control_transport::{ControlQuery, ControlRouteIdentity, control_query, control_route},
    ingress::ingress_router_with_control,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Request,
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use std::sync::{Arc, Mutex};

const MESSAGES: &str = "/api/v1/relay/messages";
const ACK: &str = "/api/v1/relay/messages/11111111-1111-4111-8111-111111111111/ack";

#[test]
fn worker_control_query_is_unique_canonical_bounded_and_inbox_only() {
    for (query, after, limit, encoded) in [
        (None, None, None, None),
        (Some("after=0"), Some(0), None, Some("after=0")),
        (Some("limit=32"), None, Some(32), Some("limit=32")),
        (
            Some("limit=1&after=9223372036854775807"),
            Some(i64::MAX),
            Some(1),
            Some("after=9223372036854775807&limit=1"),
        ),
    ] {
        let parsed = control_query(&Method::GET, MESSAGES, query).unwrap();
        assert_eq!(parsed, ControlQuery::Inbox { after, limit });
        assert_eq!(parsed.encoded().as_deref(), encoded);
    }
    for query in [
        "",
        "after",
        "after=",
        "after=-1",
        "after=+1",
        "after=01",
        "after=%31",
        "after=9223372036854775808",
        "after=18446744073709551616",
        "after=0&after=0",
        "limit=0",
        "limit=33",
        "limit=01",
        "limit=1&limit=2",
        "limit=1&",
        "&limit=1",
        "limit=1=2",
        "limit=1;after=0",
        "tenant=a",
        "after=0&unknown=1",
        "Limit=1",
    ] {
        assert!(
            control_query(&Method::GET, MESSAGES, Some(query)).is_err(),
            "{query}"
        );
    }
    for (method, path) in [
        (Method::POST, MESSAGES),
        (Method::POST, ACK),
        (Method::GET, "/api/v1/control/protocol"),
    ] {
        assert_eq!(
            control_query(&method, path, None).unwrap(),
            ControlQuery::None
        );
        assert!(control_query(&method, path, Some("limit=1")).is_err());
    }
    assert_eq!(
        control_route(&Method::POST, ACK),
        Some(ControlRouteIdentity::WorkerRelay)
    );
    assert_eq!(
        control_route(
            &Method::POST,
            "/api/v1/relay/messages/00000000-0000-0000-0000-000000000000/ack",
        ),
        Some(ControlRouteIdentity::WorkerRelay)
    );
    for method in [Method::GET, Method::PUT, Method::DELETE] {
        assert_eq!(control_route(&method, ACK), None);
    }
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
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap()
}

#[tokio::test]
async fn worker_control_query_is_reconstructed_and_rejections_do_not_reach_upstream() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let saved = observed.clone();
    let upstream = serve(Router::new().fallback(any(move |request: Request| {
        let saved = saved.clone();
        async move {
            saved.lock().unwrap().push(request.uri().to_string());
            assert_eq!(
                request.headers()[header::AUTHORIZATION],
                "Bearer synthetic-opaque-proof"
            );
            to_bytes(request.into_body(), 262_144).await.unwrap();
            (StatusCode::OK, "{}").into_response()
        }
    })))
    .await;
    let public = serve(Router::new().fallback(any(|| async { StatusCode::IM_A_TEAPOT }))).await;
    let relay = serve(ingress_router_with_control(&public.url, &upstream.url).unwrap()).await;
    let http = client();
    let response = http
        .get(format!("{}/control{MESSAGES}?limit=2&after=0", relay.url))
        .bearer_auth("synthetic-opaque-proof")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        observed.lock().unwrap().as_slice(),
        ["/api/v1/relay/messages?after=0&limit=2"]
    );
    for (method, target) in [
        (Method::GET, format!("{MESSAGES}?after=00")),
        (Method::GET, format!("{MESSAGES}?limit=1&limit=1")),
        (Method::GET, format!("{MESSAGES}?tenant=b")),
        (Method::POST, format!("{MESSAGES}?limit=1")),
        (Method::POST, format!("{ACK}?after=0")),
        (Method::GET, "/api/v1/control/protocol?limit=1".into()),
    ] {
        let response = http
            .request(method, format!("{}/control{target}", relay.url))
            .bearer_auth("synthetic-opaque-proof")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{target}");
    }
    assert_eq!(observed.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn worker_response_budgets_are_per_route_and_errors_remain_small() {
    // Fixture selection is server-owned mutable state, not an ingress query.
    let answer = Arc::new(Mutex::new((StatusCode::OK, 0usize)));
    let current = answer.clone();
    let upstream = serve(Router::new().fallback(any(move |request: Request| {
        let current = current.clone();
        async move {
            to_bytes(request.into_body(), 262_144).await.unwrap();
            let (status, bytes) = *current.lock().unwrap();
            let mut response = Response::new(Body::from(vec![b'x'; bytes]));
            *response.status_mut() = status;
            response
        }
    })))
    .await;
    let public = serve(Router::new().fallback(any(|| async { StatusCode::IM_A_TEAPOT }))).await;
    let relay = serve(ingress_router_with_control(&public.url, &upstream.url).unwrap()).await;
    let http = client();
    for (method, path, status, cap) in [
        (Method::GET, MESSAGES, StatusCode::OK, 2 * 1024 * 1024),
        (Method::POST, MESSAGES, StatusCode::CREATED, 16 * 1024),
        (Method::POST, ACK, StatusCode::OK, 16 * 1024),
        (Method::GET, MESSAGES, StatusCode::FORBIDDEN, 16 * 1024),
        (
            Method::GET,
            "/api/v1/control/protocol",
            StatusCode::OK,
            512 * 1024,
        ),
    ] {
        for extra in [0, 1] {
            *answer.lock().unwrap() = (status, cap + extra);
            let response = http
                .request(method.clone(), format!("{}/control{path}", relay.url))
                .send()
                .await
                .unwrap();
            if extra == 0 {
                assert_eq!(response.status(), status, "{path}");
                assert_eq!(response.bytes().await.unwrap().len(), cap);
            } else {
                assert_eq!(response.status(), StatusCode::BAD_GATEWAY, "{path}");
                let body: serde_json::Value = response.json().await.unwrap();
                assert_eq!(body["error"]["code"], "control_transport_protocol_failure");
                assert_eq!(body["error"]["outcome"], "unknown");
                assert_eq!(body["error"]["reason"], "upstream_response_too_large");
            }
        }
    }
}
