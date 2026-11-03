//! Invert the audit's slow-upload oracle: an authenticated worker-read must
//! remain usable while 64 anonymous request bodies are incomplete. These tests
//! exercise the real ingress and HTTP client, with no engine or worker process.
use super::*;
use axum::body::Bytes;
use futures_util::Stream;
use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};
use tokio::{sync::oneshot, task::JoinSet};
use tower::ServiceExt;

const MONITOR_CREDENTIAL: &str = "fixture-ingress-monitor-not-a-real-credential";

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

#[derive(Clone, Copy, Debug)]
enum UploadAdmission {
    Reading,
    RejectedBeforeReading,
}

/// The first poll confirms that ingress has started reading the body. A body
/// dropped before that poll confirms an admission refusal instead. The upload
/// then remains pending until cancellation or the configured body deadline.
struct SlowUpload {
    admitted: Option<oneshot::Sender<UploadAdmission>>,
    active: Arc<AtomicUsize>,
    reading: bool,
}
impl Stream for SlowUpload {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(admitted) = self.admitted.take() {
            self.active.fetch_add(1, Ordering::SeqCst);
            self.reading = true;
            let _ = admitted.send(UploadAdmission::Reading);
            return Poll::Ready(Some(Ok(Bytes::from_static(b"{"))));
        }
        Poll::Pending
    }
}
impl Drop for SlowUpload {
    fn drop(&mut self) {
        if self.reading {
            // Each body increments once on its first poll and is dropped once.
            // Bodies rejected before their first poll never decrement, so this
            // body still owns one count even when others drop concurrently.
            self.active.fetch_sub(1, Ordering::SeqCst);
        }
        if let Some(admitted) = self.admitted.take() {
            let _ = admitted.send(UploadAdmission::RejectedBeforeReading);
        }
    }
}

struct HeldUploads {
    _requests: JoinSet<Response>,
    active: Arc<AtomicUsize>,
    reading: usize,
}
impl HeldUploads {
    fn assert_reading(&self, phase: &str) {
        let active = self.active.load(Ordering::SeqCst);
        assert!(
            active > 0 && active == self.reading,
            "HARNESS INCONCLUSIVE: {active} of {} admitted uploads remain {phase}; this is not a security RED or GREEN",
            self.reading
        );
    }
}

async fn hold_64_anonymous_uploads(router: &Router) -> HeldUploads {
    let mut requests = JoinSet::new();
    let mut admissions = Vec::new();
    let active = Arc::new(AtomicUsize::new(0));
    // This literal is the attack size from the audit, independent of the
    // implementation's current exchange or upload capacity.
    for _ in 0..64 {
        let (admitted, admission) = oneshot::channel();
        admissions.push(admission);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/events")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONTENT_LENGTH, "1000")
            .body(Body::from_stream(SlowUpload {
                admitted: Some(admitted),
                active: active.clone(),
                reading: false,
            }))
            .unwrap();
        let router = router.clone();
        requests.spawn(async move { router.oneshot(request).await.unwrap() });
    }
    let admissions = tokio::time::timeout(
        Duration::from_secs(30),
        futures_util::future::join_all(admissions),
    )
    .await
    .expect("HARNESS INCONCLUSIVE: upload admission did not settle; this is not a security RED");
    let mut reading = 0;
    let mut rejected = 0;
    for admission in admissions {
        match admission.expect("HARNESS INCONCLUSIVE: upload body lost its admission signal") {
            UploadAdmission::Reading => reading += 1,
            UploadAdmission::RejectedBeforeReading => rejected += 1,
        }
    }
    assert!(
        reading > 0,
        "HARNESS INCONCLUSIVE: no upload entered body reading; this is not a security RED or GREEN"
    );
    println!("64 anonymous uploads settled: reading={reading}, rejected_before_body={rejected}");
    // JoinSet aborts every still-pending upload on drop, including assertion
    // failures below. No detached task or externally owned runtime is used.
    let uploads = HeldUploads {
        _requests: requests,
        active,
        reading,
    };
    // All admission signals have settled: no new body can begin reading after
    // this point. Requiring the entire admitted set avoids a false GREEN if
    // even one upload expires and frees an exchange in the old implementation.
    uploads.assert_reading("after upload admission");
    uploads
}

#[tokio::test]
async fn anonymous_slow_uploads_do_not_block_worker_monitor_or_bodyless_exchanges() {
    let upstream = serve(Router::new().fallback(any(|request: Request| async move {
        if request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            != Some("Bearer fixture-ingress-monitor-not-a-real-credential")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        match (request.method(), request.uri().path()) {
            (&Method::GET, "/api/v1/workers/worker-fixture") => {
                Json(json!({"worker":"observed-fixture"})).into_response()
            }
            (&Method::POST, "/empty-control") => StatusCode::NO_CONTENT.into_response(),
            (&Method::GET, "/events") => {
                let stream = async_stream::stream! {
                    yield Ok::<_,std::io::Error>("id: 7\nevent: audit\ndata: fixture\n\n");
                    std::future::pending::<()>().await;
                };
                let mut response = Response::new(Body::from_stream(stream));
                response.headers_mut().insert(
                    header::CONTENT_TYPE,
                    axum::http::HeaderValue::from_static("text/event-stream"),
                );
                response
            }
            _ => StatusCode::NOT_FOUND.into_response(),
        }
    })))
    .await;
    // Long body deadlines keep the synchronized attack present throughout the
    // oracle. Timing out this watchdog is a harness failure, never a proof that
    // the worker monitor was denied. No test advances the product's clock.
    let timeouts = Timeouts {
        connect: Duration::from_secs(3),
        headers: Duration::from_secs(30),
        upload_total: Duration::from_secs(120),
        upload_idle: Duration::from_secs(120),
        chunk: Duration::from_secs(120),
    };
    let relay = router(&upstream.url, timeouts).unwrap();
    let ingress = serve(relay.clone()).await;
    let uploads = hold_64_anonymous_uploads(&relay).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    uploads.assert_reading("before the HTTP monitor request");
    let response = client
        .get(format!("{}/api/v1/workers/worker-fixture", ingress.url))
        .bearer_auth(MONITOR_CREDENTIAL)
        .send()
        .await;
    uploads.assert_reading("after the HTTP monitor response");
    let response = response.expect("HARNESS INCONCLUSIVE: no HTTP result for the worker monitor");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "incomplete anonymous uploads must not turn a worker monitor GET into an ingress refusal"
    );
    let observed = response.json::<serde_json::Value>().await;
    uploads.assert_reading("after the HTTP monitor body");
    assert_eq!(observed.unwrap(), json!({"worker":"observed-fixture"}));
    // Exercise the actual control client's get_json path used by refresh,
    // without claiming to launch a worker or qualify an engine.
    let control = crate::runner::AuditClient::new(&ingress.url, MONITOR_CREDENTIAL).unwrap();
    uploads.assert_reading("before the control-client monitor request");
    let observed = control.get_json("api/v1/workers/worker-fixture").await;
    uploads.assert_reading("after the control-client monitor response");
    assert_eq!(observed.unwrap(), json!({"worker":"observed-fixture"}));

    uploads.assert_reading("before the bodyless control request");
    let response = client
        .post(format!("{}/empty-control", ingress.url))
        .bearer_auth(MONITOR_CREDENTIAL)
        .send()
        .await;
    uploads.assert_reading("after the bodyless control response");
    assert_eq!(
        response.unwrap().status(),
        StatusCode::NO_CONTENT,
        "bodyless admission must depend on body state rather than the HTTP method"
    );
    uploads.assert_reading("before the SSE request");
    let sse = client
        .get(format!("{}/events", ingress.url))
        .bearer_auth(MONITOR_CREDENTIAL)
        .header("last-event-id", "6")
        .send()
        .await;
    uploads.assert_reading("after the SSE response headers");
    let mut sse = sse.unwrap();
    assert_eq!(sse.status(), StatusCode::OK);
    assert_eq!(sse.headers()[header::CONTENT_TYPE], "text/event-stream");
    uploads.assert_reading("before the first SSE event");
    let first_event = sse.chunk().await;
    uploads.assert_reading("after the first SSE event");
    assert_eq!(
        first_event.unwrap().unwrap(),
        "id: 7\nevent: audit\ndata: fixture\n\n"
    );
}

#[tokio::test]
async fn upload_budget_is_released_on_cancellation_failure_and_success() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let upstream = serve(Router::new().fallback(any(move || {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            "accepted"
        }
    })))
    .await;
    let uploads = Arc::new(tokio::sync::Semaphore::new(1));
    let exchanges = Arc::new(tokio::sync::Semaphore::new(1));
    let mut state = Ingress {
        origin: reqwest::Url::parse(&upstream.url).unwrap(),
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        uploads: uploads.clone(),
        concurrency: exchanges.clone(),
        timeouts: Timeouts {
            connect: Duration::from_secs(3),
            headers: Duration::from_secs(30),
            upload_total: Duration::from_secs(120),
            upload_idle: Duration::from_secs(120),
            chunk: Duration::from_secs(120),
        },
    };
    let relay = Router::new()
        .fallback(any(forward))
        .with_state(state.clone());
    let active = Arc::new(AtomicUsize::new(0));
    let (admitted, admission) = oneshot::channel();
    let slow = Request::builder()
        .method(Method::POST)
        .body(Body::from_stream(SlowUpload {
            admitted: Some(admitted),
            active: active.clone(),
            reading: false,
        }))
        .unwrap();
    let mut pending = JoinSet::new();
    let upload_relay = relay.clone();
    pending.spawn(async move { upload_relay.oneshot(slow).await.unwrap() });
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(30), admission)
            .await
            .expect("HARNESS INCONCLUSIVE: upload admission did not settle")
            .expect("HARNESS INCONCLUSIVE: upload admission signal was lost"),
        UploadAdmission::Reading
    ));
    assert_eq!(uploads.available_permits(), 0);
    assert_eq!(exchanges.available_permits(), 1);

    // Neither GET nor a declared Content-Length of zero makes a nonempty
    // in-memory body eligible to bypass the upload budget.
    let rejected = relay
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .header(header::CONTENT_LENGTH, "0")
                .body(Body::from("nonempty fixture"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        active.load(Ordering::SeqCst),
        1,
        "HARNESS INCONCLUSIVE: the admitted upload expired before the capacity oracle"
    );
    assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let bodyless = relay
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        active.load(Ordering::SeqCst),
        1,
        "HARNESS INCONCLUSIVE: the admitted upload expired before the bodyless oracle"
    );
    assert_eq!(bodyless.status(), StatusCode::OK);
    assert_eq!(uploads.available_permits(), 0);
    axum::body::to_bytes(bodyless.into_body(), 100)
        .await
        .unwrap();
    assert_eq!(exchanges.available_permits(), 1);

    pending.abort_all();
    tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(result) = pending.join_next().await {
            assert!(result.unwrap_err().is_cancelled());
        }
    })
    .await
    .expect("HARNESS INCONCLUSIVE: cancellation did not settle");
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(uploads.available_permits(), 1);
    assert_eq!(exchanges.available_permits(), 1);

    let failing_bodies = [
        (
            Body::from_stream(futures_util::stream::once(async {
                Err::<Bytes, _>(std::io::Error::other("inert incomplete fixture"))
            })),
            StatusCode::BAD_REQUEST,
        ),
        (
            Body::from(vec![b'x'; MAX_BODY + 1]),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ];
    for (body, status) in failing_bodies {
        let response = relay
            .clone()
            .oneshot(Request::builder().method(Method::POST).body(body).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(uploads.available_permits(), 1);
        assert_eq!(exchanges.available_permits(), 1);
    }

    state.timeouts.upload_total = Duration::from_millis(50);
    state.timeouts.upload_idle = Duration::from_millis(20);
    let timeout_relay = Router::new().fallback(any(forward)).with_state(state);
    let response = tokio::time::timeout(
        Duration::from_secs(30),
        timeout_relay.oneshot(
            Request::builder()
                .method(Method::POST)
                .body(Body::from_stream(futures_util::stream::pending::<
                    Result<Bytes, std::io::Error>,
                >()))
                .unwrap(),
        ),
    )
    .await
    .expect("HARNESS INCONCLUSIVE: upload timeout did not settle")
    .unwrap();
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert_eq!(uploads.available_permits(), 1);
    assert_eq!(exchanges.available_permits(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let response = relay
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .body(Body::from("complete fixture"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(uploads.available_permits(), 1);
    assert_eq!(exchanges.available_permits(), 0);
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 100)
            .await
            .unwrap(),
        "accepted"
    );
    assert_eq!(exchanges.available_permits(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
