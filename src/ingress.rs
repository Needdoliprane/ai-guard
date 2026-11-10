//! A fixed-destination HTTP ingress. Authentication remains in the upstream
//! gateway/dashboard; this process has no credentials or business authority.
use axum::{
    Json, Router,
    body::{Body, HttpBody},
    extract::{Request, State},
    http::{HeaderMap, HeaderName, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use futures_util::StreamExt;
use serde_json::json;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::Duration,
};

const MAX_URI: usize = 4096;
const MAX_BODY: usize = 262_144;
const MAX_CONCURRENT: usize = 64;
const MAX_UPLOADS: usize = 16;
// Retained SSE responses share the total exchange cap, but cannot consume it
// completely. The remaining sixteen slots can serve ordinary exchanges.
const MAX_SSE_EXCHANGES: usize = 48;

mod control;

/// Gateway-root control relay; callers mount it under the explicit `/control`
/// transport prefix. Authentication remains exclusively in the fixed gateway.
pub fn control_relay_router(upstream: &str) -> anyhow::Result<Router> {
    control::router(upstream)
}

/// Public ingestion and control share the published origin, never their upload
/// or exchange budgets. An unknown control target cannot fall back to public.
pub fn ingress_router_with_control(
    upstream: &str,
    control_upstream: &str,
) -> anyhow::Result<Router> {
    anyhow::ensure!(
        upstream_origin(upstream)? != upstream_origin(control_upstream)?,
        "control upstream must be a distinct dedicated origin"
    );
    Ok(ingress_router(upstream)?.nest("/control", control_relay_router(control_upstream)?))
}

struct ExchangeLease {
    permit: Mutex<Option<tokio::sync::OwnedSemaphorePermit>>,
    stream_permit: Mutex<Option<tokio::sync::OwnedSemaphorePermit>>,
    body_done: AtomicBool,
    finished: tokio::sync::Notify,
}
impl ExchangeLease {
    fn release(&self) {
        // The critical section cannot await. Recover a poisoned mutex during
        // cancellation so an already failed exchange cannot retain its slot.
        for permit in [&self.permit, &self.stream_permit] {
            match permit.lock() {
                Ok(mut permit) => {
                    permit.take();
                }
                Err(poisoned) => {
                    poisoned.into_inner().take();
                }
            }
        }
    }
}
struct BodyLease(Arc<ExchangeLease>);
impl Drop for BodyLease {
    fn drop(&mut self) {
        self.0.body_done.store(true, Ordering::Release);
        self.0.release();
        self.0.finished.notify_one();
    }
}
struct PumpLease {
    lease: Arc<ExchangeLease>,
    terminal: Arc<AtomicU8>,
}
impl Drop for PumpLease {
    fn drop(&mut self) {
        if !self.lease.body_done.load(Ordering::Acquire) {
            // Includes timeout, cancellation and task abort before its first
            // poll. An unconsumed body must then expose an error, never EOF.
            self.terminal.store(2, Ordering::Release);
            self.lease.release();
        }
    }
}

#[derive(Clone, Copy)]
struct Timeouts {
    connect: Duration,
    headers: Duration,
    upload_total: Duration,
    upload_idle: Duration,
    chunk: Duration,
}
impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(3),
            headers: Duration::from_secs(15),
            upload_total: Duration::from_secs(5),
            upload_idle: Duration::from_secs(2),
            chunk: Duration::from_secs(30),
        }
    }
}
#[derive(Clone)]
struct Ingress {
    origin: reqwest::Url,
    client: reqwest::Client,
    uploads: Arc<tokio::sync::Semaphore>,
    concurrency: Arc<tokio::sync::Semaphore>,
    streams: Arc<tokio::sync::Semaphore>,
    timeouts: Timeouts,
}

pub fn ingress_router(upstream: &str) -> anyhow::Result<Router> {
    router(upstream, Timeouts::default())
}

fn upstream_origin(upstream: &str) -> anyhow::Result<reqwest::Url> {
    let origin =
        reqwest::Url::parse(upstream).map_err(|_| anyhow::anyhow!("invalid upstream origin"))?;
    let authority_and_target = upstream
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or_default();
    let user_info = authority_and_target
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .contains('@');
    // URL parsing normalizes dot segments and backslashes. Refuse an explicit
    // target before normalization can disguise it as an origin-only config.
    let raw_target = authority_and_target
        .find(['/', '?', '#'])
        .map(|start| &authority_and_target[start..])
        .unwrap_or_default();
    anyhow::ensure!(
        ["http", "https"].contains(&origin.scheme())
            && !authority_and_target.is_empty()
            && origin.host_str().is_some()
            && origin.username().is_empty()
            && origin.password().is_none()
            && !user_info
            && upstream.trim() == upstream
            && !upstream.contains('\\')
            && matches!(raw_target, "" | "/")
            && origin.path() == "/"
            && origin.query().is_none()
            && origin.fragment().is_none(),
        "upstream must be an http(s) origin without credentials, path, query or fragment"
    );
    Ok(origin)
}

fn router(upstream: &str, timeouts: Timeouts) -> anyhow::Result<Router> {
    let state = Ingress {
        origin: upstream_origin(upstream)?,
        client: reqwest::Client::builder()
            .connect_timeout(timeouts.connect)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .build()?,
        uploads: Arc::new(tokio::sync::Semaphore::new(MAX_UPLOADS)),
        concurrency: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT)),
        streams: Arc::new(tokio::sync::Semaphore::new(MAX_SSE_EXCHANGES)),
        timeouts,
    };
    // /healthz goes to the real backend, like every other path. It must not
    // manufacture a healthy response when that service is unavailable.
    Ok(Router::new().fallback(any(forward)).with_state(state))
}

fn failure(status: StatusCode, message: &'static str) -> Response {
    let mut response = (status, Json(json!({"error":message}))).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

fn is_event_stream(headers: &HeaderMap) -> bool {
    headers.get_all(header::CONTENT_TYPE).iter().any(|value| {
        value
            .as_bytes()
            .split(|byte| *byte == b';')
            .next()
            .is_some_and(|media| {
                media
                    .trim_ascii()
                    .eq_ignore_ascii_case(b"text/event-stream")
            })
    })
}

/// Parse all Connection values before deleting the header itself. A caller may
/// nominate any field as hop-by-hop, including fields unknown to this program.
pub(crate) fn end_to_end_headers(headers: &HeaderMap, request: bool) -> Result<HeaderMap, ()> {
    let mut nominated = Vec::new();
    for connection in headers.get_all(header::CONNECTION) {
        for token in connection.to_str().map_err(|_| ())?.split(',') {
            let token = token.trim();
            if !token.is_empty() {
                nominated.push(HeaderName::from_bytes(token.as_bytes()).map_err(|_| ())?);
            }
        }
    }
    let mut filtered = headers.clone();
    for name in nominated {
        filtered.remove(name);
    }
    for name in [
        "connection",
        "keep-alive",
        "proxy-connection",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "http2-settings",
    ] {
        filtered.remove(name);
    }
    if request {
        filtered.remove(header::HOST);
        filtered.remove("forwarded");
        filtered.remove("x-real-ip");
        let forwarded = filtered
            .keys()
            .filter(|name| name.as_str().starts_with("x-forwarded-"))
            .cloned()
            .collect::<Vec<_>>();
        for name in forwarded {
            filtered.remove(name);
        }
    }
    Ok(filtered)
}

enum BodyFailure {
    Timeout,
    Incomplete,
    TooLarge,
}
impl IntoResponse for BodyFailure {
    fn into_response(self) -> Response {
        match self {
            Self::Timeout => failure(
                StatusCode::REQUEST_TIMEOUT,
                "Délai de lecture de la requête dépassé.",
            ),
            Self::Incomplete => failure(StatusCode::BAD_REQUEST, "Corps de requête incomplet."),
            Self::TooLarge => failure(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Corps de requête trop volumineux.",
            ),
        }
    }
}
async fn read_request(body: Body, timeout: Duration) -> Result<Vec<u8>, BodyFailure> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    loop {
        let chunk = match tokio::time::timeout(timeout, stream.next()).await {
            Err(_) => {
                return Err(BodyFailure::Timeout);
            }
            Ok(None) => return Ok(bytes),
            Ok(Some(Err(_))) => {
                return Err(BodyFailure::Incomplete);
            }
            Ok(Some(Ok(chunk))) => chunk,
        };
        if chunk.len() > MAX_BODY - bytes.len() {
            return Err(BodyFailure::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
}

async fn forward(State(state): State<Ingress>, request: Request) -> Response {
    if request.uri().to_string().len() > MAX_URI {
        return failure(StatusCode::URI_TOO_LONG, "URI trop longue.");
    }
    if matches!(*request.method(), Method::CONNECT | Method::TRACE) {
        return failure(StatusCode::METHOD_NOT_ALLOWED, "Méthode non autorisée.");
    }
    if request.uri().scheme().is_some()
        || request.uri().authority().is_some()
        || !request.uri().path().starts_with('/')
    {
        return failure(StatusCode::BAD_REQUEST, "Cible de requête invalide.");
    }
    if request.headers().contains_key(header::UPGRADE)
        || request
            .headers()
            .get_all(header::CONNECTION)
            .iter()
            .any(|value| {
                value.to_str().is_ok_and(|value| {
                    value
                        .split(',')
                        .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
                })
            })
    {
        return failure(
            StatusCode::BAD_REQUEST,
            "Changement de protocole non autorisé.",
        );
    }
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > MAX_BODY as u64)
    {
        return failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Corps de requête trop volumineux.",
        );
    }
    let headers = match end_to_end_headers(request.headers(), true) {
        Ok(headers) => headers,
        Err(()) => return failure(StatusCode::BAD_REQUEST, "En-têtes de requête invalides."),
    };
    // set_path/set_query preserve the configured authority; Host, forwarded
    // headers, absolute request targets and redirects cannot choose another one.
    let mut destination = state.origin.clone();
    destination.set_path(request.uri().path());
    destination.set_query(request.uri().query());
    let method = request.method().clone();
    // Bound the complete request before contacting the backend. In particular,
    // a chunked over-limit upload must not be partly applied upstream.
    // A request with an incomplete body must not reserve an exchange needed by
    // a bodyless control read. The body's state, not its method or declared
    // Content-Length, determines whether it needs upload admission.
    let body = request.into_body();
    let body = if body.is_end_stream() {
        Vec::new()
    } else {
        let upload = match state.uploads.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                return failure(
                    StatusCode::TOO_MANY_REQUESTS,
                    "Trop de corps de requête simultanés.",
                );
            }
        };
        let result = tokio::time::timeout(
            state.timeouts.upload_total,
            read_request(body, state.timeouts.upload_idle),
        )
        .await;
        // Both phases are bounded independently; an upload never holds its
        // permit while waiting for an exchange or streaming a response.
        drop(upload);
        match result {
            Ok(Ok(body)) => body,
            Ok(Err(failure)) => return failure.into_response(),
            Err(_) => return BodyFailure::Timeout.into_response(),
        }
    };
    let permit = match state.concurrency.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return failure(
                StatusCode::TOO_MANY_REQUESTS,
                "Trop de requêtes simultanées.",
            );
        }
    };
    let call = state
        .client
        .request(method, destination)
        .headers(headers)
        .body(body)
        .send();
    let upstream = match tokio::time::timeout(state.timeouts.headers, call).await {
        Err(_) => {
            return failure(
                StatusCode::GATEWAY_TIMEOUT,
                "Délai de réponse du service dépassé.",
            );
        }
        Ok(Err(error)) if error.is_timeout() => {
            return failure(
                StatusCode::GATEWAY_TIMEOUT,
                "Délai de réponse du service dépassé.",
            );
        }
        Ok(Err(_)) => return failure(StatusCode::BAD_GATEWAY, "Service inaccessible."),
        Ok(Ok(upstream)) => upstream,
    };
    if upstream.status() == StatusCode::SWITCHING_PROTOCOLS {
        return failure(StatusCode::BAD_GATEWAY, "Réponse du service invalide.");
    }
    // Classify the fixed upstream response, never the request path, Accept
    // header or caller's claimed tenant. Inspect before hop-by-hop filtering,
    // so a Connection nomination cannot erase its SSE classification.
    let stream_permit = if is_event_stream(upstream.headers()) {
        match state.streams.clone().try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => {
                return failure(StatusCode::TOO_MANY_REQUESTS, "Trop de flux simultanés.");
            }
        }
    } else {
        None
    };
    let headers = match end_to_end_headers(upstream.headers(), false) {
        Ok(headers) => headers,
        Err(()) => return failure(StatusCode::BAD_GATEWAY, "Réponse du service invalide."),
    };
    let status = upstream.status();
    let mut stream = upstream.bytes_stream();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    // Only an explicit upstream EOF is a successful terminal state. A pump
    // failure/abort must remain an error after any already queued chunk, even
    // when a full channel could not carry a separate error message.
    let terminal = Arc::new(AtomicU8::new(0));
    let pump_terminal = terminal.clone();
    let lease = Arc::new(ExchangeLease {
        permit: Mutex::new(Some(permit)),
        stream_permit: Mutex::new(stream_permit),
        body_done: AtomicBool::new(false),
        finished: tokio::sync::Notify::new(),
    });
    let body_lease = BodyLease(lease.clone());
    let pump_lease = PumpLease {
        lease,
        terminal: pump_terminal.clone(),
    };
    tokio::spawn(async move {
        // Normal ownership lasts until Body completion/drop. This independent
        // guard can force release on errors even when that Body is not polled.
        let pump_lease = pump_lease;
        loop {
            let next = tokio::select! {
                biased;
                _ = sender.closed() => return,
                next = tokio::time::timeout(state.timeouts.chunk,stream.next()) => next,
            };
            let chunk = match next {
                Ok(Some(Ok(chunk))) => chunk,
                Ok(None) => {
                    pump_terminal.store(1, Ordering::Release);
                    break;
                }
                Ok(Some(Err(_))) | Err(_) => {
                    pump_terminal.store(2, Ordering::Release);
                    break;
                }
            };
            if !matches!(
                tokio::time::timeout(state.timeouts.chunk, sender.send(chunk)).await,
                Ok(Ok(()))
            ) {
                pump_terminal.store(2, Ordering::Release);
                break;
            }
        }
        drop(stream);
        drop(sender);
        if pump_terminal.load(Ordering::Acquire) == 1 {
            // EOF upstream alone does not consume the downstream body. Bound
            // final delivery too, while allowing its normal EOF/drop to release
            // the slot immediately. Notify remembers a completion before wait.
            let _ =
                tokio::time::timeout(state.timeouts.chunk, pump_lease.lease.finished.notified())
                    .await;
        }
    });
    let output = async_stream::stream! {
        let _lease = body_lease;
        while let Some(chunk) = receiver.recv().await { yield Ok::<_,std::io::Error>(chunk); }
        if terminal.load(Ordering::Acquire) != 1 {
            yield Err(std::io::Error::other("Flux du service interrompu ou délai dépassé."));
        }
    };
    let mut response = Response::new(Body::from_stream(output));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

#[cfg(test)]
#[path = "../tests/ingress_admission/slow_uploads.rs"]
mod admission_tests;

#[cfg(test)]
mod sse_recovery_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
    fn fast_timeouts() -> Timeouts {
        Timeouts {
            connect: Duration::from_millis(100),
            headers: Duration::from_millis(50),
            upload_total: Duration::from_millis(50),
            upload_idle: Duration::from_millis(50),
            chunk: Duration::from_millis(50),
        }
    }
    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    #[tokio::test]
    async fn sse_budget_uses_upstream_media_type_and_releases_on_body_drop() {
        use tower::ServiceExt;
        let upstream = serve(Router::new().fallback(any(|request: Request| async move {
            if request.uri().path() == "/plain" {
                return "available".into_response();
            }
            let stream = async_stream::stream! {
                yield Ok::<_, std::io::Error>("data: ready\n\n");
                std::future::pending::<()>().await;
            };
            let mut response = Response::new(Body::from_stream(stream));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("TeXt/Event-Stream ; charset=utf-8"),
            );
            response.headers_mut().insert(
                header::CONNECTION,
                axum::http::HeaderValue::from_static("content-type"),
            );
            response
        })))
        .await;
        let state = Ingress {
            origin: reqwest::Url::parse(&upstream.url).unwrap(),
            client: client(),
            uploads: Arc::new(tokio::sync::Semaphore::new(MAX_UPLOADS)),
            // Small component budgets exercise the same real HTTP admission;
            // the separate multi-tenant regression uses the production caps.
            concurrency: Arc::new(tokio::sync::Semaphore::new(2)),
            streams: Arc::new(tokio::sync::Semaphore::new(1)),
            timeouts: Timeouts {
                headers: Duration::from_secs(5),
                chunk: Duration::from_secs(10),
                ..Timeouts::default()
            },
        };
        let router = Router::new().fallback(any(forward)).with_state(state);
        let request = |path| Request::builder().uri(path).body(Body::empty()).unwrap();
        let retained = router.clone().oneshot(request("/arbitrary")).await.unwrap();
        assert_eq!(retained.status(), StatusCode::OK);
        assert!(!retained.headers().contains_key(header::CONTENT_TYPE));
        let declined = router.clone().oneshot(request("/another")).await.unwrap();
        assert_eq!(declined.status(), StatusCode::TOO_MANY_REQUESTS);
        let body = axum::body::to_bytes(declined.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            json!({"error":"Trop de flux simultanés."})
        );
        let ordinary = router.clone().oneshot(request("/plain")).await.unwrap();
        assert_eq!(ordinary.status(), StatusCode::OK);
        assert_eq!(
            axum::body::to_bytes(ordinary.into_body(), 100)
                .await
                .unwrap(),
            "available"
        );
        drop(retained);
        let next = router.oneshot(request("/arbitrary")).await.unwrap();
        assert_eq!(next.status(), StatusCode::OK);
        drop(next);
    }

    #[tokio::test]
    async fn headers_deadline_returns_generic_504_without_waiting_fifteen_seconds() {
        let upstream = serve(
            Router::new().fallback(any(|| async { std::future::pending::<Response>().await })),
        )
        .await;
        let ingress = serve(router(&upstream.url, fast_timeouts()).unwrap()).await;
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            client()
                .get(format!("{}/private?fixture-value=hidden", ingress.url))
                .bearer_auth("fixture-not-real")
                .send(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        let body = response.text().await.unwrap();
        for value in ["hidden", "fixture-not-real", &upstream.url] {
            assert!(!body.contains(value));
        }
    }

    #[tokio::test]
    async fn stalled_response_chunk_fails_the_stream_instead_of_a_complete_export() {
        let upstream = serve(Router::new().fallback(any(|| async {
            let stream = async_stream::stream! {
                yield Ok::<_,std::io::Error>("first");
                std::future::pending::<()>().await;
            };
            Response::new(Body::from_stream(stream))
        })))
        .await;
        let ingress = serve(router(&upstream.url, fast_timeouts()).unwrap()).await;
        let mut response = client().get(&ingress.url).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.chunk().await.unwrap().unwrap(), "first");
        assert!(
            tokio::time::timeout(Duration::from_secs(2), response.chunk())
                .await
                .unwrap()
                .is_err()
        );
    }

    #[tokio::test]
    async fn stalled_request_body_times_out_before_contacting_backend() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let upstream = serve(Router::new().fallback(any(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                "unexpected"
            }
        })))
        .await;
        let ingress = serve(router(&upstream.url, fast_timeouts()).unwrap()).await;
        let stream = async_stream::stream! {
            yield Ok::<_,std::io::Error>("partial");
            std::future::pending::<()>().await;
        };
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            client()
                .post(&ingress.url)
                .body(reqwest::Body::wrap_stream(stream))
                .send(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn continuously_progressing_upload_has_a_total_deadline_and_releases_its_slot() {
        use tower::ServiceExt;
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let upstream = serve(Router::new().fallback(any(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                "available"
            }
        })))
        .await;
        let uploads = Arc::new(tokio::sync::Semaphore::new(1));
        let exchanges = Arc::new(tokio::sync::Semaphore::new(1));
        let held_exchange = exchanges.clone().try_acquire_owned().unwrap();
        let state = Ingress {
            origin: reqwest::Url::parse(&upstream.url).unwrap(),
            client: client(),
            uploads: uploads.clone(),
            concurrency: exchanges.clone(),
            streams: Arc::new(tokio::sync::Semaphore::new(MAX_SSE_EXCHANGES)),
            timeouts: Timeouts {
                upload_total: Duration::from_secs(1),
                upload_idle: Duration::from_secs(120),
                ..Timeouts::default()
            },
        };
        let relay = Router::new().fallback(any(forward)).with_state(state);
        let chunks = Arc::new(AtomicUsize::new(0));
        let produced = chunks.clone();
        let stream = async_stream::stream! {
            loop {
                produced.fetch_add(1, Ordering::SeqCst);
                yield Ok::<_,std::io::Error>("x");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            relay.clone().oneshot(
                Request::builder()
                    .method(Method::POST)
                    .body(Body::from_stream(stream))
                    .unwrap(),
            ),
        )
        .await;
        let response = result.expect(
            "HARNESS INCONCLUSIVE: upload deadline did not settle; this is not a latency qualification",
        );
        assert!(
            chunks.load(Ordering::SeqCst) >= 2,
            "HARNESS INCONCLUSIVE: upload did not make progress before its deadline"
        );
        assert_eq!(response.unwrap().status(), StatusCode::REQUEST_TIMEOUT);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(uploads.available_permits(), 1);
        assert_eq!(exchanges.available_permits(), 0);
        drop(held_exchange);
        let response = relay
            .oneshot(Request::builder().body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 100)
                .await
                .unwrap(),
            "available"
        );
        assert_eq!(exchanges.available_permits(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn unpolled_export_times_out_releases_slot_and_later_reports_incomplete_body() {
        use tower::ServiceExt;
        let upstream = serve(Router::new().fallback(any(|| async {
            let stream = async_stream::stream! {
                for _ in 0..1024 { yield Ok::<_,std::io::Error>(vec![b'x';65_536]); }
            };
            Response::new(Body::from_stream(stream))
        })))
        .await;
        let permits = Arc::new(tokio::sync::Semaphore::new(1));
        let state = Ingress {
            origin: reqwest::Url::parse(&upstream.url).unwrap(),
            client: client(),
            uploads: Arc::new(tokio::sync::Semaphore::new(MAX_UPLOADS)),
            concurrency: permits.clone(),
            streams: Arc::new(tokio::sync::Semaphore::new(MAX_SSE_EXCHANGES)),
            timeouts: fast_timeouts(),
        };
        let response = Router::new()
            .fallback(any(forward))
            .with_state(state)
            .oneshot(
                Request::builder()
                    .uri("/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            permits.available_permits(),
            1,
            "unpolled downstream must not suspend the independent send deadline"
        );
        assert!(
            axum::body::to_bytes(response.into_body(), 70 * 1024 * 1024)
                .await
                .is_err(),
            "a queued chunk followed by send timeout must not become a clean EOF"
        );
    }

    #[tokio::test]
    async fn upstream_eof_keeps_slot_until_downstream_body_is_consumed_or_dropped() {
        use tower::ServiceExt;
        struct ActiveBody(Arc<AtomicUsize>);
        impl Drop for ActiveBody {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let active = Arc::new(AtomicUsize::new(0));
        let counter = active.clone();
        let upstream = serve(
            Router::new()
                .route(
                    "/live",
                    any(move || {
                        let counter = counter.clone();
                        async move {
                            counter.fetch_add(1, Ordering::SeqCst);
                            let guard = ActiveBody(counter);
                            let stream = async_stream::stream! {
                                let _guard = guard;
                                yield Ok::<_,std::io::Error>("pending");
                                std::future::pending::<()>().await;
                            };
                            Response::new(Body::from_stream(stream))
                        }
                    }),
                )
                .fallback(any(|| async { "complete" })),
        )
        .await;
        let permits = Arc::new(tokio::sync::Semaphore::new(1));
        let mut timeouts = fast_timeouts();
        timeouts.chunk = Duration::from_secs(1);
        let state = Ingress {
            origin: reqwest::Url::parse(&upstream.url).unwrap(),
            client: client(),
            uploads: Arc::new(tokio::sync::Semaphore::new(MAX_UPLOADS)),
            concurrency: permits.clone(),
            streams: Arc::new(tokio::sync::Semaphore::new(MAX_SSE_EXCHANGES)),
            timeouts,
        };
        let router = Router::new().fallback(any(forward)).with_state(state);
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            permits.available_permits(),
            0,
            "upstream EOF alone must not release a normal response body slot"
        );
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 100)
                .await
                .unwrap(),
            "complete"
        );
        assert_eq!(permits.available_permits(), 1);
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(permits.available_permits(), 0);
        drop(response);
        assert_eq!(
            permits.available_permits(),
            1,
            "drop before first poll must release its slot"
        );
        let response = router
            .oneshot(Request::builder().uri("/live").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(active.load(Ordering::SeqCst), 1);
        drop(response);
        assert_eq!(permits.available_permits(), 1);
        tokio::time::timeout(Duration::from_millis(500), async {
            while active.load(Ordering::SeqCst) != 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("body drop must cancel upstream before its one-second read deadline");
    }
}
