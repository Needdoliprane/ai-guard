//! Headers-first, fixed-origin control transport. No credential interpretation,
//! no public admission permits, no retry, and no fallback to ingestion.
use super::{
    BodyFailure, MAX_BODY, MAX_URI, Timeouts, end_to_end_headers, failure, upstream_origin,
};
use crate::control_transport::{
    ControlRouteIdentity, ControlTransportFailureReasonV1, ControlTransportFailureV1,
    control_query, control_route,
};
use axum::{
    Router,
    body::{Body, HttpBody},
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use futures_util::StreamExt;
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

fn transport_failure(reason: ControlTransportFailureReasonV1) -> Response {
    let status = match reason {
        ControlTransportFailureReasonV1::UpstreamTimeout => StatusCode::GATEWAY_TIMEOUT,
        _ => StatusCode::BAD_GATEWAY,
    };
    let mut response = (
        status,
        axum::Json(serde_json::json!({"error":ControlTransportFailureV1::new(reason)})),
    )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

fn protocol_failure(reason: &'static str) -> Response {
    let mut response = (
        StatusCode::BAD_GATEWAY,
        axum::Json(serde_json::json!({"error": {
            "version":1,"code":"control_transport_protocol_failure","phase":"exchange",
            "outcome":"unknown","retry_action":"contact_operator","reason":reason
        }})),
    )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

fn classified_transport(error: &reqwest::Error) -> Option<ControlTransportFailureReasonV1> {
    use ControlTransportFailureReasonV1::*;
    if error.is_timeout() {
        return Some(UpstreamTimeout);
    }
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = cause {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            use std::io::ErrorKind::*;
            match io.kind() {
                ConnectionRefused | AddrNotAvailable | NotConnected | HostUnreachable
                | NetworkUnreachable => return Some(UpstreamConnectionUnavailable),
                ConnectionReset | ConnectionAborted | BrokenPipe => {
                    return Some(UpstreamConnectionInterrupted);
                }
                TimedOut => return Some(UpstreamTimeout),
                _ => {}
            }
        }
        cause = current.source();
    }
    // TLS verification, malformed framing and unknown errors are not transient
    // transport evidence merely because reqwest exposes them as connect/body.
    None
}

const MAX_HEADERS: usize = 16 * 1024;
const MAX_RESPONSE: usize = 512 * 1024;
const MAX_WORKER_INBOX_RESPONSE: usize = 2 * 1024 * 1024;
const MAX_WORKER_RECEIPT_RESPONSE: usize = 16 * 1024;
const ACTIVE: u8 = 0;
const COMPLETE: u8 = 1;
const TIMEOUT: u8 = 2;
const INCOMPLETE: u8 = 3;
const TOO_LARGE: u8 = 4;
const ABORTED: u8 = 5;

#[derive(Clone)]
struct ControlRelay {
    origin: reqwest::Url,
    client: reqwest::Client,
    timeouts: Timeouts,
}

pub(super) fn router(upstream: &str) -> anyhow::Result<Router> {
    router_with_timeouts(upstream, Timeouts::default())
}

fn router_with_timeouts(upstream: &str, timeouts: Timeouts) -> anyhow::Result<Router> {
    let state = ControlRelay {
        origin: upstream_origin(upstream)?,
        client: reqwest::Client::builder()
            .connect_timeout(timeouts.connect)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .http1_only()
            .build()?,
        timeouts,
    };
    Ok(Router::new().fallback(any(forward)).with_state(state))
}

fn bounded_headers(headers: &HeaderMap) -> bool {
    headers
        .iter()
        .try_fold(0usize, |bytes, (name, value)| {
            bytes.checked_add(name.as_str().len() + value.as_bytes().len() + 4)
        })
        .is_some_and(|bytes| bytes <= MAX_HEADERS)
}

struct UploadOwner(Arc<AtomicU8>);
impl Drop for UploadOwner {
    fn drop(&mut self) {
        // An HTTP client may drop its request body after an upstream refusal.
        // Preserve an explicit prior failure; never manufacture successful EOF.
        let _ = self
            .0
            .compare_exchange(ACTIVE, ABORTED, Ordering::AcqRel, Ordering::Acquire);
    }
}

fn upload_failure(state: &AtomicU8) -> Option<BodyFailure> {
    match state.load(Ordering::Acquire) {
        TIMEOUT => Some(BodyFailure::Timeout),
        INCOMPLETE => Some(BodyFailure::Incomplete),
        TOO_LARGE => Some(BodyFailure::TooLarge),
        _ => None,
    }
}

fn upload_body(
    body: Body,
    expected: Option<usize>,
    state: Arc<AtomicU8>,
    timeouts: Timeouts,
) -> reqwest::Body {
    // The absolute deadline begins at admission, not on an arbitrarily delayed
    // first body poll. No producer task survives cancellation of this request.
    let deadline = tokio::time::Instant::now() + timeouts.upload_total;
    let owner = UploadOwner(state.clone());
    let output = async_stream::stream! {
        let _owner = owner;
        let mut input = body.into_data_stream();
        let mut bytes = 0usize;
        loop {
            let next = tokio::time::timeout_at(
                deadline,
                tokio::time::timeout(timeouts.upload_idle, input.next()),
            ).await;
            let chunk = match next {
                Err(_) | Ok(Err(_)) => {
                    state.store(TIMEOUT, Ordering::Release);
                    yield Err(std::io::Error::other("control upload deadline"));
                    break;
                }
                Ok(Ok(Some(Err(_)))) => {
                    state.store(INCOMPLETE, Ordering::Release);
                    yield Err(std::io::Error::other("control upload incomplete"));
                    break;
                }
                Ok(Ok(None)) => {
                    if expected.is_some_and(|length| length != bytes) {
                        state.store(INCOMPLETE, Ordering::Release);
                        yield Err(std::io::Error::other("control upload incomplete"));
                    } else {
                        state.store(COMPLETE, Ordering::Release);
                    }
                    break;
                }
                Ok(Ok(Some(Ok(chunk)))) => chunk,
            };
            if chunk.len() > MAX_BODY - bytes
                || expected.is_some_and(|length| chunk.len() > length.saturating_sub(bytes))
            {
                state.store(TOO_LARGE, Ordering::Release);
                yield Err(std::io::Error::other("control upload exceeds bound"));
                break;
            }
            bytes += chunk.len();
            yield Ok(chunk);
        }
    };
    reqwest::Body::wrap_stream(output)
}

async fn forward(State(state): State<ControlRelay>, request: Request) -> Response {
    if request.uri().to_string().len() > MAX_URI {
        return failure(StatusCode::URI_TOO_LONG, "URI trop longue.");
    }
    if request.uri().scheme().is_some() || request.uri().authority().is_some() {
        return failure(StatusCode::BAD_REQUEST, "Cible de contrôle invalide.");
    }
    let query = match control_query(
        request.method(),
        request.uri().path(),
        request.uri().query(),
    ) {
        Ok(query) => query,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "Cible de contrôle invalide."),
    };
    let Some(identity) = control_route(request.method(), request.uri().path()) else {
        return failure(StatusCode::NOT_FOUND, "Route de contrôle inconnue.");
    };
    if !bounded_headers(request.headers()) {
        return failure(
            StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
            "En-têtes de requête trop volumineux.",
        );
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
                        .any(|part| part.trim().eq_ignore_ascii_case("upgrade"))
                })
            })
    {
        return failure(
            StatusCode::BAD_REQUEST,
            "Changement de protocole non autorisé.",
        );
    }
    let mut lengths = request.headers().get_all(header::CONTENT_LENGTH).iter();
    let expected = match lengths.next() {
        None => None,
        Some(value) => match value
            .to_str()
            .ok()
            .and_then(|text| text.parse::<u64>().ok())
        {
            Some(length) if length <= MAX_BODY as u64 && lengths.next().is_none() => {
                Some(length as usize)
            }
            Some(length) if length > MAX_BODY as u64 => {
                return BodyFailure::TooLarge.into_response();
            }
            _ => return failure(StatusCode::BAD_REQUEST, "Longueur de requête invalide."),
        },
    };
    let mut headers = match end_to_end_headers(request.headers(), true) {
        Ok(headers) => headers,
        Err(()) => return failure(StatusCode::BAD_REQUEST, "En-têtes de requête invalides."),
    };
    // Reframe the bounded stream as chunked: successful EOF must follow the
    // relay's length/deadline checks, even when the input declared a length.
    headers.remove(header::CONTENT_LENGTH);
    if let Some(value) = headers.get_mut(header::AUTHORIZATION) {
        value.set_sensitive(true);
    }
    let mut destination = state.origin.clone();
    destination.set_path(request.uri().path());
    destination.set_query(query.encoded().as_deref());
    let method = request.method().clone();
    let worker_inbox =
        identity == ControlRouteIdentity::WorkerRelay && method == axum::http::Method::GET;
    let body = request.into_body();
    let progress = Arc::new(AtomicU8::new(ACTIVE));
    let body = if body.is_end_stream() {
        if expected.is_some_and(|length| length != 0) {
            return BodyFailure::Incomplete.into_response();
        }
        progress.store(COMPLETE, Ordering::Release);
        reqwest::Body::from(Vec::new())
    } else {
        upload_body(body, expected, progress.clone(), state.timeouts)
    };
    // Unlike public forwarding this send starts before a request body exists.
    // The gateway, not the relay or a path label, authenticates its headers.
    let call = state
        .client
        .request(method, destination)
        .headers(headers)
        .body(body)
        .send();
    let upstream = match tokio::time::timeout(state.timeouts.headers, call).await {
        Ok(Ok(upstream)) => upstream,
        result => {
            if let Some(failure) = upload_failure(&progress) {
                return failure.into_response();
            }
            return match result {
                Err(_) => transport_failure(ControlTransportFailureReasonV1::UpstreamTimeout),
                Ok(Err(error)) => match classified_transport(&error) {
                    Some(reason) => transport_failure(reason),
                    None => protocol_failure("upstream_exchange_invalid"),
                },
                _ => protocol_failure("upstream_exchange_invalid"),
            };
        }
    };
    let status = upstream.status();
    let response_limit = if identity != ControlRouteIdentity::WorkerRelay {
        MAX_RESPONSE
    } else if status.is_success() && worker_inbox {
        MAX_WORKER_INBOX_RESPONSE
    } else {
        MAX_WORKER_RECEIPT_RESPONSE
    };
    // An identity refusal is allowed before body EOF. Other responses must not
    // turn a failed bounded upload into an apparent successful operation.
    if !matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
        && let Some(failure) = upload_failure(&progress)
    {
        return failure.into_response();
    }
    if status == StatusCode::SWITCHING_PROTOCOLS
        || status.is_success() && progress.load(Ordering::Acquire) != COMPLETE
        || !bounded_headers(upstream.headers())
    {
        return protocol_failure("upstream_response_inconsistent");
    }
    let headers = match end_to_end_headers(upstream.headers(), false) {
        Ok(headers) => headers,
        Err(()) => return protocol_failure("upstream_headers_invalid"),
    };
    // Control responses are bounded receipts, never SSE. Buffer through real EOF
    // so a truncated upstream response cannot appear as a complete receipt.
    let read = async {
        let mut bytes = Vec::new();
        let mut stream = upstream.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| match classified_transport(&error) {
                Some(reason) => transport_failure(reason),
                None => protocol_failure("upstream_response_incomplete"),
            })?;
            if chunk.len() > response_limit - bytes.len() {
                return Err(protocol_failure("upstream_response_too_large"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    };
    let bytes = match tokio::time::timeout(state.timeouts.chunk, read).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(response)) => return response,
        Err(_) => return transport_failure(ControlTransportFailureReasonV1::UpstreamTimeout),
    };
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}
