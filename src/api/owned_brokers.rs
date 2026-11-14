//! Owned broker-supervision routes of the proposed
//! `broker_supervision_owned_v2` profile (proposal submitted to Root, not an
//! admitted interface).
//!
//! Only the seven closed operations exist. The operation is chosen by the
//! server route, never by a request field, and a handler serves a request only
//! with the private route marker and the retained work that the control
//! transport gate attached after authenticating the owned broker proof for
//! this exact operation. Without them it refuses before any body, Store or
//! audit work; the legacy presence routes and every existing owned refusal are
//! unchanged. Bodies are read from bounded bytes and parsed into the closed
//! request types. Refusals are the closed owned error envelope only. A
//! positive DTO is published only after its Store commit and redaction screen,
//! never altered; a mutation whose outcome cannot be published is withheld for
//! reconciliation of the same operation. Nothing here releases a quota, fences
//! a worker, meters usage, proves a runtime inventory or sends a command.
use super::*;
use super::{
    auth::owned_broker_supervision::OwnedBrokerSupervisionWork, control_transport::OwnedBrokerRoute,
};
use crate::broker_registry::owned_supervision::{
    OwnedBrokerCheckpointRequestV2, OwnedBrokerHeartbeatRequestV2,
    OwnedReconciliationRequestV2,
    OwnedBrokerOperationV2 as Operation, OwnedChecked, OwnedSupervisionErrorCodeV2 as Code,
};

pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route(Operation::Identity.router_path(), get(identity))
        .route(Operation::SelfView.router_path(), get(self_view))
        .route(Operation::Heartbeat.router_path(), post(heartbeat))
        .route(Operation::Checkpoint.router_path(), post(checkpoint))
        .route(Operation::CheckpointSelf.router_path(), get(checkpoint_self))
        .route(Operation::HeartbeatReconcile.router_path(), post(heartbeat_reconcile))
        .route(Operation::CheckpointReconcile.router_path(), post(checkpoint_reconcile))
}

/// Closed owned refusal: the code fixes its HTTP status, stable wire code,
/// fixed prose and retry class together. No request value or secret is used.
pub(super) fn refusal(code: Code) -> (StatusCode, Json<Value>) {
    match StatusCode::from_u16(code.status()) {
        Ok(status) => (status, Json(code.body())),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(Code::SupervisionUnavailable.body()),
        ),
    }
}

/// After a possibly durable mutation, any later refusal withholds the
/// response: the publisher reconciles the same operation by replay and exact
/// self-read, never by allocating another one. Reads keep their own code. A
/// deterministic publication refusal stays `ResponseUnpublishable` (wait for
/// operator): reconciliation would only repeat it.
pub(super) fn withheld(mutating: bool, code: Code) -> Code {
    if mutating && code != Code::ResponseUnpublishable {
        Code::ResponseWithheld
    } else {
        code
    }
}

async fn identity(request: Request) -> Response {
    serve(Operation::Identity, request).await
}

async fn self_view(request: Request) -> Response {
    serve(Operation::SelfView, request).await
}

async fn heartbeat(request: Request) -> Response {
    serve(Operation::Heartbeat, request).await
}

async fn checkpoint(request: Request) -> Response {
    serve(Operation::Checkpoint, request).await
}

async fn checkpoint_self(request: Request) -> Response {
    serve(Operation::CheckpointSelf, request).await
}

async fn heartbeat_reconcile(request: Request) -> Response {
    serve(Operation::HeartbeatReconcile, request).await
}

async fn checkpoint_reconcile(request: Request) -> Response {
    serve(Operation::CheckpointReconcile, request).await
}

async fn serve(route: Operation, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    // A request without the gate's marker and work for this exact operation
    // (public router, legacy gateway, other method or query) is refused like
    // any other unadopted owned target, before body, Store or audit work.
    let Some(work) = admitted(route, &parts) else {
        return super::human_sources::unavailable().into_response();
    };
    match operate(route, work, &parts.headers, body).await {
        Ok(response) => response,
        Err(code) => refusal(code).into_response(),
    }
}

/// The private route marker and the retained work attached by the transport
/// gate, both for this handler's operation, with its exact method and no
/// query. Neither can be produced by a client header or wire field.
fn admitted(
    route: Operation,
    parts: &axum::http::request::Parts,
) -> Option<Arc<OwnedBrokerSupervisionWork>> {
    let marker = parts.extensions.get::<OwnedBrokerRoute>().copied()?;
    let work = parts
        .extensions
        .get::<Arc<OwnedBrokerSupervisionWork>>()
        .cloned()?;
    (marker.operation() == route
        && work.operation() == route
        && parts.method.as_str() == route.method()
        && parts.uri.query().is_none())
    .then_some(work)
}

async fn operate(
    route: Operation,
    work: Arc<OwnedBrokerSupervisionWork>,
    headers: &HeaderMap,
    body: Body,
) -> OwnedChecked<Response> {
    // Ambiguous browser-origin headers are refused, as by the legacy
    // workload guard; the outer gateway policy still applies to the first.
    if headers.get_all(header::ORIGIN).iter().count() > 1
        || headers.get_all("sec-fetch-site").iter().count() > 1
    {
        return Err(Code::Forbidden);
    }
    // The gate already buffered the bounded body; a GET admits zero bytes.
    let bytes = axum::body::to_bytes(body, route.request_limit())
        .await
        .map_err(|_| Code::InvalidRequest)?;
    match route {
        Operation::Identity => published(route, &work.identity().await?),
        Operation::SelfView => published(route, &work.self_view().await?),
        Operation::Heartbeat => {
            let request: OwnedBrokerHeartbeatRequestV2 =
                serde_json::from_slice(&bytes).map_err(|_| Code::InvalidRequest)?;
            published(route, &work.heartbeat(request).await?)
        }
        Operation::Checkpoint => {
            let request: OwnedBrokerCheckpointRequestV2 =
                serde_json::from_slice(&bytes).map_err(|_| Code::InvalidRequest)?;
            published(route, &work.checkpoint(request).await?)
        }
        Operation::CheckpointSelf => published(route, &work.checkpoint_self().await?),
        Operation::HeartbeatReconcile | Operation::CheckpointReconcile => {
            let request: OwnedReconciliationRequestV2 =
                serde_json::from_slice(&bytes).map_err(|_| Code::InvalidRequest)?;
            if request.operation() != route {
                return Err(Code::ProtocolMismatch);
            }
            published(route, &work.reconcile(request).await?)
        }
    }
}

/// Bounded JSON publication of a committed, screened positive DTO, exactly as
/// produced; an unpublishable value is refused (`ResponseUnpublishable`, wait
/// for operator), never truncated or masked.
fn published<T: serde::Serialize>(route: Operation, value: &T) -> OwnedChecked<Response> {
    let unpublishable = Code::ResponseUnpublishable;
    let bytes = serde_json::to_vec(value).map_err(|_| unpublishable)?;
    if bytes.len() > route.response_limit() {
        return Err(unpublishable);
    }
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}
