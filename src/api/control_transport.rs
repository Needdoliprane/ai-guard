//! Authentication precedes both reserved operation admission and body polling.
//! Existing handlers authenticate/revalidate again at their normal boundaries.
use super::*;
use crate::control_transport::{ControlRouteIdentity, control_query, control_route};
use axum::{body::HttpBody, extract::FromRequestParts};
use tokio::sync::Semaphore;

const RUNNER_OPERATIONS: usize = 16;
const BROKER_PRESENCE: usize = 4;
const WORKER_RELAY_OPERATIONS: usize = 8;
const MAX_BODY: usize = 262_144;
const UPLOAD_TOTAL: Duration = Duration::from_secs(5);
const UPLOAD_IDLE: Duration = Duration::from_secs(2);

pub(super) struct ControlCapacity {
    runner: Arc<Semaphore>,
    presence: Arc<Semaphore>,
    worker_relay: Arc<Semaphore>,
}
impl ControlCapacity {
    pub(super) fn new() -> Self {
        Self {
            runner: Arc::new(Semaphore::new(RUNNER_OPERATIONS)),
            presence: Arc::new(Semaphore::new(BROKER_PRESENCE)),
            worker_relay: Arc::new(Semaphore::new(WORKER_RELAY_OPERATIONS)),
        }
    }
}

pub(super) fn router(state: GatewayState) -> Router {
    // No recursion: this is the existing API builder without its /control nest.
    // The gate is outermost; non-control routes cannot reach the base router.
    super::gateway_base_router(state.clone())
        .layer(middleware::from_fn_with_state(state.clone(), gate))
        .layer(middleware::from_fn_with_state(
            state,
            gateway_security_headers,
        ))
}

fn refused(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    let mut response = (
        status,
        Json(json!({"error": {
            "code": code,
            "message": message,
            "phase": "control_admission",
            "outcome": "not_dispatched"
        }})),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn read_body(body: Body) -> Result<Vec<u8>, Response> {
    let deadline = tokio::time::Instant::now() + UPLOAD_TOTAL;
    let mut input = body.into_data_stream();
    let mut bytes = Vec::new();
    loop {
        let next =
            tokio::time::timeout_at(deadline, tokio::time::timeout(UPLOAD_IDLE, input.next()))
                .await;
        let chunk = match next {
            Err(_) | Ok(Err(_)) => {
                return Err(refused(
                    StatusCode::REQUEST_TIMEOUT,
                    "control_body_timeout",
                    "Control request body deadline exceeded.",
                ));
            }
            Ok(Ok(Some(Err(_)))) => {
                return Err(refused(
                    StatusCode::BAD_REQUEST,
                    "control_body_incomplete",
                    "Control request body is incomplete.",
                ));
            }
            Ok(Ok(None)) => return Ok(bytes),
            Ok(Ok(Some(Ok(chunk)))) => chunk,
        };
        if chunk.len() > MAX_BODY - bytes.len() {
            return Err(refused(
                StatusCode::PAYLOAD_TOO_LARGE,
                "control_body_too_large",
                "Control request body exceeds its bound.",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
}

async fn gate(State(state): State<GatewayState>, request: Request, next: Next) -> Response {
    if state.owned_human.is_some() {
        let method = if request.method() == "HEAD" {
            &axum::http::Method::GET
        } else {
            request.method()
        };
        if control_route(method, request.uri().path()).is_some() {
            return super::human_sources::unavailable().into_response();
        }
        if [axum::http::Method::GET, axum::http::Method::POST]
            .iter()
            .any(|method| control_route(method, request.uri().path()).is_some())
        {
            return refused(
                StatusCode::METHOD_NOT_ALLOWED,
                "control_method_not_allowed",
                "Method not registered for this control transport route.",
            );
        }
        // Unknown routes/methods still use the closed transport classifier;
        // no legacy extractor or body reader is reached in this branch.
        return refused(
            StatusCode::NOT_FOUND,
            "unknown_control_route",
            "Unknown control transport route.",
        );
    }
    if request.uri().to_string().len() > 4096
        || request.uri().scheme().is_some()
        || request.uri().authority().is_some()
        || control_query(
            request.method(),
            request.uri().path(),
            request.uri().query(),
        )
        .is_err()
    {
        return refused(
            StatusCode::BAD_REQUEST,
            "invalid_control_target",
            "Invalid control request target.",
        );
    }
    let Some(identity) = control_route(request.method(), request.uri().path()) else {
        return refused(
            StatusCode::NOT_FOUND,
            "unknown_control_route",
            "Unknown control transport route.",
        );
    };
    let (mut parts, body) = request.into_parts();
    let semaphore = match identity {
        ControlRouteIdentity::Runner => {
            match RunnerIdentity::from_request_parts(&mut parts, &state).await {
                Ok(RunnerIdentity(tenant)) => tenant.control_capacity.runner.clone(),
                Err(rejection) => return rejection.into_response(),
            }
        }
        ControlRouteIdentity::BrokerPresence => {
            let broker =
                match workload::VerifiedBroker::from_request_parts(&mut parts, &state).await {
                    Ok(broker) => broker,
                    Err(rejection) => return rejection.into_response(),
                };
            match broker.control_capacity() {
                Ok(capacity) => capacity.presence.clone(),
                Err(rejection) => return rejection.into_response(),
            }
        }
        ControlRouteIdentity::WorkerRelay => {
            let worker =
                match workload::VerifiedWorker::from_request_parts(&mut parts, &state).await {
                    Ok(worker) => worker,
                    Err(rejection) => return rejection.into_response(),
                };
            match worker.control_capacity() {
                Ok(capacity) => capacity.worker_relay.clone(),
                Err(rejection) => return rejection.into_response(),
            }
        }
    };
    let _operation = match semaphore.try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return refused(
                StatusCode::TOO_MANY_REQUESTS,
                "control_operation_capacity_exhausted",
                "This tenant's control operation capacity is occupied.",
            );
        }
    };
    // Header/framing errors are screened after the required identity, without
    // letting a malformed declaration or over-limit stream reach a handler.
    let mut lengths = parts.headers.get_all(header::CONTENT_LENGTH).iter();
    let expected = match lengths.next() {
        None => None,
        Some(length) => match length
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
        {
            Some(length) if length <= MAX_BODY as u64 && lengths.next().is_none() => {
                Some(length as usize)
            }
            Some(length) if length > MAX_BODY as u64 => {
                return refused(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "control_body_too_large",
                    "Control request body exceeds its bound.",
                );
            }
            _ => {
                return refused(
                    StatusCode::BAD_REQUEST,
                    "control_body_incomplete",
                    "Invalid control body framing.",
                );
            }
        },
    };
    if expected.is_some() && parts.headers.contains_key(header::TRANSFER_ENCODING) {
        return refused(
            StatusCode::BAD_REQUEST,
            "control_body_incomplete",
            "Ambiguous control body framing.",
        );
    }
    let bytes = if body.is_end_stream() {
        Vec::new()
    } else {
        match read_body(body).await {
            Ok(bytes) => bytes,
            Err(response) => return response,
        }
    };
    if expected.is_some_and(|length| length != bytes.len()) {
        return refused(
            StatusCode::BAD_REQUEST,
            "control_body_incomplete",
            "Control request body is incomplete.",
        );
    }
    parts.headers.remove(header::TRANSFER_ENCODING);
    parts.headers.remove(header::CONTENT_LENGTH);
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

#[cfg(test)]
mod tests;
