use agentsandbox::ingress::ingress_router;
use axum::{
    Json, Router,
    body::Body,
    extract::Request,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server { url, task }
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap()
}
async fn echo(request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, 300_000).await.unwrap();
    let mut response=(StatusCode::CREATED,Json(json!({"method":parts.method.as_str(),"uri":parts.uri.to_string(),"body":String::from_utf8(bytes.to_vec()).unwrap(),"authorization":parts.headers.get("authorization").and_then(|h|h.to_str().ok()),"content_type":parts.headers.get("content-type").and_then(|h|h.to_str().ok()),"last_event_id":parts.headers.get("last-event-id").and_then(|h|h.to_str().ok()),"origin":parts.headers.get("origin").and_then(|h|h.to_str().ok()),"sec_fetch_site":parts.headers.get("sec-fetch-site").and_then(|h|h.to_str().ok()),"host":parts.headers.get("host").and_then(|h|h.to_str().ok()),"hop":parts.headers.get("x-remove-me").and_then(|h|h.to_str().ok()),"forwarded":parts.headers.get("x-forwarded-host").and_then(|h|h.to_str().ok()),"proxy_auth":parts.headers.get("proxy-authorization").and_then(|h|h.to_str().ok())}))).into_response();
    response.headers_mut().insert(
        header::CONNECTION,
        HeaderValue::from_static("x-backend-only, keep-alive"),
    );
    response
        .headers_mut()
        .insert("x-backend-only", HeaderValue::from_static("remove"));
    response
        .headers_mut()
        .insert("x-end-to-end", HeaderValue::from_static("preserve"));
    response
}

#[tokio::test]
async fn forwards_method_body_status_and_end_to_end_headers_but_strips_hop_headers() {
    let upstream = serve(Router::new().fallback(any(echo))).await;
    let ingress = serve(ingress_router(&upstream.url).unwrap()).await;
    let response = client()
        .patch(format!("{}/api/events?after=a%2Fb", ingress.url))
        .header("authorization", "Bearer fixture-not-a-real-credential")
        .header("content-type", "application/custom+json")
        .header("last-event-id", "47")
        .header("origin", "https://fixture.invalid")
        .header("sec-fetch-site", "cross-site")
        .header("host", "attacker.invalid")
        .header("x-forwarded-host", "attacker.invalid")
        .header("connection", "x-remove-me, keep-alive")
        .header("x-remove-me", "remove")
        .header("proxy-authorization", "Basic fixture")
        .body("payload")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert!(!response.headers().contains_key("connection"));
    assert!(!response.headers().contains_key("x-backend-only"));
    assert_eq!(response.headers()["x-end-to-end"], "preserve");
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["method"], "PATCH");
    assert_eq!(value["uri"], "/api/events?after=a%2Fb");
    assert_eq!(value["body"], "payload");
    assert_eq!(
        value["authorization"],
        "Bearer fixture-not-a-real-credential"
    );
    assert_eq!(value["content_type"], "application/custom+json");
    assert_eq!(value["last_event_id"], "47");
    assert_eq!(value["origin"], "https://fixture.invalid");
    assert_eq!(value["sec_fetch_site"], "cross-site");
    assert_eq!(value["host"], upstream.url.trim_start_matches("http://"));
    for name in ["hop", "forwarded", "proxy_auth"] {
        assert!(value[name].is_null());
    }
    let value: Value = client()
        .get(format!("{}//attacker.invalid/resource", ingress.url))
        .header("host", "attacker.invalid")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(value["host"], upstream.url.trim_start_matches("http://"));
    assert_eq!(value["uri"], "//attacker.invalid/resource");
}

#[tokio::test]
async fn redirects_are_returned_without_sending_authorization_to_a_third_origin() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let third = serve(Router::new().fallback(any(move || {
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            "third"
        }
    })))
    .await;
    let destination = third.url.clone();
    let upstream = serve(Router::new().fallback(any(move || {
        let destination = destination.clone();
        async move { (StatusCode::FOUND, [(header::LOCATION, destination)]) }
    })))
    .await;
    let ingress = serve(ingress_router(&upstream.url).unwrap()).await;
    let response = client()
        .get(&ingress.url)
        .bearer_auth("fixture-not-real")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(response.headers()["location"], third.url);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rejects_oversized_and_forbidden_requests_without_contacting_upstream() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let upstream = serve(Router::new().fallback(any(move || {
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            "unexpected"
        }
    })))
    .await;
    let ingress = serve(ingress_router(&upstream.url).unwrap()).await;
    assert_eq!(
        client()
            .post(&ingress.url)
            .body(vec![b'a'; 262_145])
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let body = futures_util::stream::iter([
        Ok::<_, std::io::Error>(vec![b'a'; 131_072]),
        Ok(vec![b'b'; 131_073]),
    ]);
    assert_eq!(
        client()
            .post(&ingress.url)
            .body(reqwest::Body::wrap_stream(body))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        client()
            .get(format!("{}/{}", ingress.url, "x".repeat(4096)))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::URI_TOO_LONG
    );
    assert_eq!(
        client()
            .request(reqwest::Method::TRACE, &ingress.url)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        client()
            .get(&ingress.url)
            .header("connection", "upgrade")
            .header("upgrade", "websocket")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let address = ingress.url.trim_start_matches("http://");
    for request in [
        "GET http://attacker.invalid/path HTTP/1.1\r\nHost: attacker.invalid\r\nConnection: close\r\n\r\n",
        "CONNECT attacker.invalid:443 HTTP/1.1\r\nHost: attacker.invalid\r\nConnection: close\r\n\r\n",
    ] {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut output = String::new();
        tokio::time::timeout(Duration::from_secs(2), stream.read_to_string(&mut output))
            .await
            .unwrap()
            .unwrap();
        assert!(output.starts_with("HTTP/1.1 400") || output.starts_with("HTTP/1.1 405"));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
async fn sse(headers: HeaderMap, active: Arc<AtomicUsize>) -> Response {
    let cursor = headers
        .get("last-event-id")
        .and_then(|header| header.to_str().ok())
        .unwrap_or("none")
        .to_owned();
    active.fetch_add(1, Ordering::SeqCst);
    let guard = Active(active);
    let body = async_stream::stream! {
        let _guard=guard;
        yield Ok::<_,std::io::Error>(format!("id: {cursor}\nevent: audit\ndata: first\n\n"));
        std::future::pending::<()>().await;
    };
    let mut response = Response::new(Body::from_stream(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    response
}

#[tokio::test]
async fn streams_sse_immediately_and_disconnect_releases_a_slot_and_upstream_body() {
    let active = Arc::new(AtomicUsize::new(0));
    let count = active.clone();
    let upstream = serve(Router::new().route(
        "/stream",
        get(move |headers: HeaderMap| sse(headers, count.clone())),
    ))
    .await;
    let ingress = serve(ingress_router(&upstream.url).unwrap()).await;
    let client = client();
    let mut held = Vec::new();
    for _ in 0..64 {
        let mut response = tokio::time::timeout(
            Duration::from_secs(2),
            client
                .get(format!("{}/stream", ingress.url))
                .header("last-event-id", "27")
                .send(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let chunk = tokio::time::timeout(Duration::from_secs(1), response.chunk())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&chunk).contains("id: 27"));
        held.push(response);
    }
    assert_eq!(active.load(Ordering::SeqCst), 64);
    assert_eq!(
        client
            .get(format!("{}/stream", ingress.url))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(held.pop());
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if active.load(Ordering::SeqCst) == 63 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let mut replacement = client
        .get(format!("{}/stream", ingress.url))
        .send()
        .await
        .unwrap();
    assert_eq!(replacement.status(), StatusCode::OK);
    assert!(replacement.chunk().await.unwrap().is_some());
}

#[tokio::test]
async fn health_reflects_the_real_backend_and_backend_errors_are_generic() {
    let upstream = serve(Router::new().route(
        "/healthz",
        get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
    ))
    .await;
    let ingress = serve(ingress_router(&upstream.url).unwrap()).await;
    assert_eq!(
        client()
            .get(format!("{}/healthz", ingress.url))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let ingress = serve(ingress_router(&url).unwrap()).await;
    let response = client()
        .get(format!(
            "{}/healthz?fixture-sensitive-query=hidden",
            ingress.url
        ))
        .bearer_auth("fixture-not-real")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = response.text().await.unwrap();
    for secret in ["hidden", "fixture-not-real", &url] {
        assert!(!body.contains(secret));
    }
}

#[test]
fn upstream_must_be_a_fixed_http_origin_without_credentials_or_url_components() {
    for upstream in [
        "ftp://localhost",
        "http:localhost",
        "http://user:pass@localhost",
        "http://@localhost",
        "http://localhost/path",
        "http://localhost/path/..",
        "http://localhost/.",
        "http://localhost\\path\\..",
        "http://localhost/?query=value",
        "http://localhost/#fragment",
        "file:///tmp/backend",
    ] {
        assert!(ingress_router(upstream).is_err());
    }
    assert!(ingress_router("http://localhost:8787").is_ok());
    assert!(ingress_router("https://localhost:8787/").is_ok());
}
