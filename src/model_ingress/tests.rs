use super::*;
use axum::{body::Body, http::Request, routing::any};
use http_body_util::BodyExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

#[tokio::test]
async fn model_routes_bounds_and_credentials_are_checked_before_upstream() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let fixture = Router::new().fallback(any(move |request: Request<Body>| {
        seen.fetch_add(1, Ordering::SeqCst);
        async move {
            assert!(!request.headers().contains_key(header::AUTHORIZATION));
            assert!(!request.headers().contains_key(header::COOKIE));
            (
                [(header::CONTENT_TYPE, "application/json")],
                "{\"status\":\"ok\"}",
            )
        }
    }));
    let task = tokio::spawn(async move {
        axum::serve(listener, fixture).await.unwrap();
    });
    let app = router(&origin).unwrap();
    for (method, path, body, expected) in [
        ("GET", "/slots", "", StatusCode::NOT_FOUND),
        (
            "POST",
            "/v1/chat/completions?x=1",
            "{}",
            StatusCode::BAD_REQUEST,
        ),
        ("TRACE", "/health", "", StatusCode::METHOD_NOT_ALLOWED),
        (
            "POST",
            "/v1/chat/completions",
            "{\"stream\":true}",
            StatusCode::BAD_REQUEST,
        ),
        (
            "POST",
            "/v1/chat/completions",
            "{\"stream\":true,\"stream\":false}",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(vec![b'x'; MAX_BODY + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "fixture-control-never-forward")
                .header(header::COOKIE, "fixture-cookie-never-forward")
                .body(Body::from("{\"stream\":false}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        b"{\"status\":\"ok\"}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn model_inference_has_no_queue_and_oversize_response_is_never_success() {
    let started = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let seen = started.clone();
    let released = finish.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let fixture = Router::new().fallback(any(move || {
        let seen = seen.clone();
        let released = released.clone();
        async move {
            seen.notify_one();
            released.notified().await;
            (
                [(header::CONTENT_TYPE, "application/json")],
                vec![b'x'; MAX_BODY + 1],
            )
        }
    }));
    let task = tokio::spawn(async move {
        axum::serve(listener, fixture).await.unwrap();
    });
    let app = router(&origin).unwrap();
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{\"stream\":false}"))
            .unwrap()
    };
    let pending = tokio::spawn(app.clone().oneshot(request()));
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    assert_eq!(
        app.oneshot(request()).await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    finish.notify_one();
    assert_eq!(
        pending.await.unwrap().unwrap().status(),
        StatusCode::BAD_GATEWAY
    );
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn slow_request_and_truncated_upstream_never_become_success() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let upstream = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 4096];
        assert!(socket.read(&mut buffer).await.unwrap() > 0);
        seen.fetch_add(1, Ordering::SeqCst);
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 64\r\nConnection: close\r\n\r\n{}").await.unwrap();
        socket.shutdown().await.unwrap();
    });
    let app = router(&origin).unwrap();
    let stream = async_stream::stream! {
        yield Ok::<_,std::io::Error>(axum::body::Bytes::from_static(b"{\"stream\":false"));
        std::future::pending::<()>().await;
    };
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from_stream(stream))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{\"stream\":false}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    tokio::time::timeout(Duration::from_secs(2), upstream)
        .await
        .unwrap()
        .unwrap();
}
