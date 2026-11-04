//! Typed relay routes. Worker endpoints do not accept a tenant or source selector.
use super::workload::{VerifiedWorker, match_path, parse_json};
use super::*;
use crate::relay::*;

pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/relay/grants", post(grant))
        .route("/relay/grants/{id}/revoke", post(revoke))
        .route("/relay/messages", post(send).get(inbox))
        .route("/relay/messages/{id}/ack", post(ack))
        .route("/relay/messages/{id}/prune", post(prune))
        .layer(middleware::from_fn(workload::request_guard))
}
fn failure(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    if let Some(response) = audit_capacity_failure(&error) {
        return response;
    }
    let (status, code, message) = match error.downcast_ref::<RelayFailure>() {
        Some(RelayFailure::InvalidRequest) => (
            StatusCode::BAD_REQUEST,
            "invalid_relay_request",
            "The request does not match the relay contract.",
        ),
        Some(RelayFailure::Forbidden) => (
            StatusCode::FORBIDDEN,
            "relay_forbidden",
            "The directed relay exchange is not currently authorized.",
        ),
        Some(RelayFailure::NotFound) => (
            StatusCode::NOT_FOUND,
            "relay_not_found",
            "The relay resource was not found in the authorized scope.",
        ),
        Some(RelayFailure::Conflict) => (
            StatusCode::CONFLICT,
            "relay_conflict",
            "The operation identity belongs to different durable content.",
        ),
        Some(RelayFailure::RateLimited) => (
            StatusCode::TOO_MANY_REQUESTS,
            "relay_rate_limited",
            "The durable relay grant rate limit was reached.",
        ),
        Some(RelayFailure::Capacity) => (
            StatusCode::TOO_MANY_REQUESTS,
            "relay_capacity_exhausted",
            "Retained relay content reached its limit; contact the operator.",
        ),
        Some(RelayFailure::Expired) => (
            StatusCode::FORBIDDEN,
            "relay_expired",
            "The relay message is no longer available for a new acknowledgement.",
        ),
        None => return workload::failure(error),
    };
    workload::response(status, code, message)
}
fn store_failure(store: &Store, error: anyhow::Error) -> (StatusCode, Json<Value>) {
    if store.healthy() {
        failure(error)
    } else {
        unavailable(error)
    }
}
async fn grant(
    RunnerIdentity(tenant): RunnerIdentity,
    body: Result<Json<RelayGrantV1>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<RelayReceiptV1>)> {
    let input = parse_json(body)?;
    chrono::DateTime::parse_from_rfc3339(&input.expires_at).map_err(|_| workload::invalid())?;
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            let receipt = tenant
                .store
                .relay_grant(&input)
                .map_err(|error| store_failure(&tenant.store, error))?;
            Ok((
                if receipt.replayed {
                    StatusCode::OK
                } else {
                    StatusCode::CREATED
                },
                Json(receipt),
            ))
        })
        .await
}
async fn revoke(
    RunnerIdentity(tenant): RunnerIdentity,
    Path(id): Path<String>,
    body: Result<Json<RelayRevokeV1>, JsonRejection>,
) -> ApiResult<Json<RelayReceiptV1>> {
    let input = parse_json(body)?;
    match_path(&id, &input.grant_id)?;
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            Ok(Json(
                tenant
                    .store
                    .relay_revoke(&input)
                    .map_err(|error| store_failure(&tenant.store, error))?,
            ))
        })
        .await
}
async fn send(
    worker: VerifiedWorker,
    body: Result<Json<RelaySendV1>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<RelayReceiptV1>)> {
    let input = parse_json(body)?;
    worker
        .blocking(move |tenant, identity, redactor| {
            // The untrusted Read gate and durable relay/workload budgets apply.
            // Do not spend the runner's shared mutation-frequency allowance.
            // The current bearer is known only at the HTTP boundary. Reject it
            // before entering the Store; never create a new sibling DB/redactor.
            if redactor.text(&input.body) != input.body {
                return Err(failure(RelayFailure::InvalidRequest.into()));
            }
            if let Ok(value) = serde_json::from_str::<Value>(&input.body)
                && redactor.value_unclipped(&value) != value
            {
                return Err(failure(RelayFailure::InvalidRequest.into()));
            }
            let receipt = tenant
                .store
                .relay_send(&identity, &input)
                .map_err(|error| store_failure(&tenant.store, error))?;
            Ok((
                if receipt.replayed {
                    StatusCode::OK
                } else {
                    StatusCode::CREATED
                },
                Json(receipt),
            ))
        })
        .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InboxQuery {
    after: Option<i64>,
    limit: Option<u32>,
}
async fn inbox(
    worker: VerifiedWorker,
    query: Result<Query<InboxQuery>, QueryRejection>,
) -> ApiResult<Json<RelayInboxV1>> {
    let Query(query) = query.map_err(|_| workload::invalid())?;
    worker
        .blocking(move |tenant, identity, _| {
            Ok(Json(
                tenant
                    .store
                    .relay_inbox(&identity, query.after, query.limit.unwrap_or(32))
                    .map_err(|error| store_failure(&tenant.store, error))?,
            ))
        })
        .await
}
async fn ack(
    worker: VerifiedWorker,
    Path(id): Path<String>,
    body: Result<Json<RelayAckV1>, JsonRejection>,
) -> ApiResult<Json<RelayReceiptV1>> {
    let input = parse_json(body)?;
    match_path(&id, &input.message_id)?;
    worker
        .blocking(move |tenant, identity, _| {
            Ok(Json(
                tenant
                    .store
                    .relay_ack(&identity, &input)
                    .map_err(|error| store_failure(&tenant.store, error))?,
            ))
        })
        .await
}
async fn prune(
    RunnerIdentity(tenant): RunnerIdentity,
    Path(id): Path<String>,
    body: Result<Json<RelayPruneV1>, JsonRejection>,
) -> ApiResult<Json<RelayReceiptV1>> {
    let input = parse_json(body)?;
    match_path(&id, &input.message_id)?;
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            Ok(Json(
                tenant
                    .store
                    .relay_prune(&input)
                    .map_err(|error| store_failure(&tenant.store, error))?,
            ))
        })
        .await
}
