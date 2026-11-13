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
