//! Fixed-purpose, non-streaming model transport. This relay holds only its
//! leaf TLS identity; it does not receive or forward control credentials.
use axum::{
    Json, Router,
    body::Body,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use std::{sync::Arc, time::Duration};
const MAX_BODY: usize = 65_536;

#[derive(Clone)]
struct Relay {
    origin: reqwest::Url,
    client: reqwest::Client,
    inference: Arc<tokio::sync::Semaphore>,
    health: Arc<tokio::sync::Semaphore>,
}
pub fn model_ingress_router() -> anyhow::Result<Router> {
    router("http://model:8080")
}
fn router(origin: &str) -> anyhow::Result<Router> {
    let origin = reqwest::Url::parse(origin)?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()?;
    Ok(Router::new().fallback(any(forward)).with_state(Relay {
        origin,
        client,
        inference: Arc::new(tokio::sync::Semaphore::new(1)),
        health: Arc::new(tokio::sync::Semaphore::new(4)),
    }))
}
fn failure(status: StatusCode) -> Response {
    let mut response = (
        status,
        Json(json!({"error":"model_transport_unavailable_or_invalid"})),
    )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
#[derive(Deserialize)]
struct StreamMode {
    stream: bool,
}
async fn read_request(body: Body) -> Result<Vec<u8>, StatusCode> {
    tokio::time::timeout(Duration::from_secs(2), async move {
        let mut stream = body.into_data_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| StatusCode::BAD_REQUEST)?;
            if chunk.len() > MAX_BODY - bytes.len() {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    })
    .await
    .unwrap_or(Err(StatusCode::REQUEST_TIMEOUT))
}
async fn forward(State(state): State<Relay>, request: Request) -> Response {
    match tokio::time::timeout(Duration::from_secs(5), forward_inner(state, request)).await {
        Ok(response) => response,
        Err(_) => failure(StatusCode::GATEWAY_TIMEOUT),
    }
}
async fn forward_inner(state: Relay, request: Request) -> Response {
    let uri = request.uri();
    if uri.to_string().len() > 4096
        || uri.scheme().is_some()
        || uri.authority().is_some()
        || uri.query().is_some()
    {
        return failure(StatusCode::BAD_REQUEST);
    }
    if matches!(*request.method(), Method::CONNECT | Method::TRACE) {
        return failure(StatusCode::METHOD_NOT_ALLOWED);
    }
    if request.headers().contains_key(header::UPGRADE)
        || request
            .headers()
            .get_all(header::CONNECTION)
            .iter()
            .any(|value| {
                value.to_str().is_ok_and(|text| {
                    text.split(',')
                        .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
                })
            })
    {
        return failure(StatusCode::BAD_REQUEST);
    }
    if crate::ingress::end_to_end_headers(request.headers(), true).is_err() {
        return failure(StatusCode::BAD_REQUEST);
    }
    let inference = match (request.method(), uri.path()) {
        (&Method::POST, "/v1/chat/completions") => true,
        (&Method::GET, "/health") => false,
        (_, "/v1/chat/completions" | "/health") => return failure(StatusCode::METHOD_NOT_ALLOWED),
        _ => return failure(StatusCode::NOT_FOUND),
    };
    let limit = if inference {
        &state.inference
    } else {
        &state.health
    };
    let Ok(_permit) = limit.clone().try_acquire_owned() else {
        return failure(StatusCode::TOO_MANY_REQUESTS);
    };
    if inference
        && !request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
            })
    {
        return failure(StatusCode::BAD_REQUEST);
    }
    let path = if inference {
        "/v1/chat/completions"
    } else {
        "/health"
    };
    let bytes = match read_request(request.into_body()).await {
        Ok(bytes) => bytes,
        Err(status) => return failure(status),
    };
    if (inference && !serde_json::from_slice::<StreamMode>(&bytes).is_ok_and(|mode| !mode.stream))
        || (!inference && !bytes.is_empty())
    {
        return failure(StatusCode::BAD_REQUEST);
    }
    let Ok(url) = state.origin.join(path) else {
        return failure(StatusCode::BAD_GATEWAY);
    };
    let mut upstream = if inference {
        state
            .client
            .post(url)
            .header(header::CONTENT_TYPE, "application/json")
            .body(bytes)
    } else {
        state.client.get(url)
    };
    // Deliberately do not copy caller Authorization, Cookie, forwarding fields
    // or any other credential into the unauthenticated isolated model.
    upstream = upstream.header(header::ACCEPT, "application/json");
    let mut response = match upstream.send().await {
        Ok(response) => response,
        Err(error) => {
            return failure(if error.is_timeout() {
                StatusCode::GATEWAY_TIMEOUT
            } else {
                StatusCode::BAD_GATEWAY
            });
        }
    };
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > MAX_BODY as u64)
    {
        return failure(StatusCode::BAD_GATEWAY);
    }
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if chunk.len() <= MAX_BODY - bytes.len() => {
                bytes.extend_from_slice(&chunk)
            }
            Ok(Some(_)) => return failure(StatusCode::BAD_GATEWAY),
            Ok(None) => break,
            Err(error) => {
                return failure(if error.is_timeout() {
                    StatusCode::GATEWAY_TIMEOUT
                } else {
                    StatusCode::BAD_GATEWAY
                });
            }
        }
    }
    if serde_json::from_slice::<serde_json::Value>(&bytes).is_err() {
        return failure(StatusCode::BAD_GATEWAY);
    }
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests;
