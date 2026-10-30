#![cfg(unix)]

use axum::{
    Router,
    response::sse::{Event, Sse},
    routing::get,
};
use std::{
    convert::Infallible,
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};

struct ActiveStream(Arc<AtomicUsize>);
impl Drop for ActiveStream {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn ingress_binary_serves_both_listeners_and_stops_with_an_active_sse_client() {
    let active = Arc::new(AtomicUsize::new(0));
    let observed = active.clone();
    let app = Router::new()
        .route(
            "/healthz",
            get(|| async { axum::Json(serde_json::json!({"status":"ok"})) }),
        )
        .route(
            "/stream",
            get(move || {
                let observed = observed.clone();
                async move {
                    Sse::new(async_stream::stream! {
                        observed.fetch_add(1, Ordering::SeqCst);
                        let _active = ActiveStream(observed);
                        loop {
                            yield Ok::<_, Infallible>(Event::default().data("alive"));
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    })
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let upstream = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut child = Command::new(Path::new(env!("CARGO_BIN_EXE_agentsandbox")))
        .args([
            "ingress",
            "--gateway-listen",
            "127.0.0.1:0",
            "--dashboard-listen",
            "127.0.0.1:0",
            "--gateway-upstream",
            &origin,
            "--dashboard-upstream",
            &origin,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), stderr.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let parts: Vec<_> = line.split_whitespace().collect();
    assert_eq!(parts.first(), Some(&"Ingress"));
    let gateway: std::net::SocketAddr = parts[2].parse().unwrap();
    let dashboard: std::net::SocketAddr = parts[5].parse().unwrap();
    assert!(gateway.ip().is_loopback() && dashboard.ip().is_loopback());
    assert_ne!(gateway.port(), dashboard.port());
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    for address in [gateway, dashboard] {
        assert_eq!(
            http.get(format!("http://{address}/healthz"))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
    let mut stream = http
        .get(format!("http://{gateway}/stream"))
        .send()
        .await
        .unwrap();
    assert!(
        stream
            .chunk()
            .await
            .unwrap()
            .unwrap()
            .windows(5)
            .any(|bytes| bytes == b"alive")
    );
    assert_eq!(active.load(Ordering::SeqCst), 1);
    let pid = i32::try_from(child.id().unwrap()).unwrap();
    // SAFETY: pid identifies this test's unreaped child, so it cannot have
    // been recycled as an unrelated user process before child.wait().
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let status = tokio::time::timeout(Duration::from_secs(9), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    drop(stream);
    tokio::time::timeout(Duration::from_secs(2), async {
        while active.load(Ordering::SeqCst) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    upstream.abort();
}

#[tokio::test]
async fn healthcheck_fails_if_either_destination_is_down_or_oversized() {
    let app = Router::new()
        .route(
            "/healthy",
            get(|| async { axum::Json(serde_json::json!({"status":"ok"})) }),
        )
        .route(
            "/unhealthy",
            get(|| async { (axum::http::StatusCode::SERVICE_UNAVAILABLE, "unavailable") }),
        )
        .route(
            "/oversized",
            get(|| async {
                axum::Json(serde_json::json!({"status":"ok","padding":"x".repeat(5000)}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    for (second, success) in [
        ("healthy", true),
        ("unhealthy", false),
        ("oversized", false),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
            .args([
                "healthcheck",
                "--url",
                &format!("{origin}/healthy"),
                "--url",
                &format!("{origin}/{second}"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert_eq!(output.status.success(), success);
        if second == "oversized" {
            assert!(String::from_utf8_lossy(&output.stderr).contains("trop volumineuse"));
        }
    }
    server.abort();
}
