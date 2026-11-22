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
        .route("/relay/retention", get(retention))
        .layer(middleware::from_fn(workload::request_guard))
}
/// A typed 429 keeps the historical `{code,message}` shape and adds closed
/// diagnostic fields. No body and no figure of another sender is ever included.
fn refusal(code: &str, message: &str, detail: Value) -> (StatusCode, Json<Value>) {
    let mut error = json!({"code":code,"message":message});
    if let (Some(error), Value::Object(detail)) = (error.as_object_mut(), detail) {
        error.extend(detail);
    }
    (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error":error})))
}
fn capacity_refusal(own: &RelayCapacityRefusalV1) -> (StatusCode, Json<Value>) {
    let (code, message) = match own.scope {
        RelayCapacityScopeV1::Sender => (
            "relay_sender_capacity_exhausted",
            "This sender reached its retained relay share.",
        ),
        RelayCapacityScopeV1::Grant => (
            "relay_grant_capacity_exhausted",
            "This directed grant reached its retained relay share.",
        ),
    };
    // Serialization of this closed struct cannot fail; a failure would only
    // drop the optional figures, never widen the answer.
    refusal(
        code,
        message,
        serde_json::to_value(own).unwrap_or(Value::Null),
    )
}
fn failure(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    if let Some(response) = audit_capacity_failure(&error) {
        return response;
    }
    if let Some(own) = error.downcast_ref::<RelayCapacityRefusalV1>() {
        return capacity_refusal(own);
    }
    // The shared ceiling describes other senders: no occupancy, no deadline.
    // The action tells whether a later retry of the same operation can pass.
    if let Some(tenant) = error.downcast_ref::<RelayTenantCapacityRefusalV1>() {
        return refusal(
            "relay_capacity_exhausted",
            "Retained relay content reached the tenant limit.",
            json!({"scope":RelayRefusalScopeV1::Tenant,"retry_action":tenant.retry_action}),
        );
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
        Some(RelayFailure::RateLimited) => {
            return refusal(
                "relay_rate_limited",
                "The durable relay grant rate limit was reached.",
                json!({
                    "scope":RelayRefusalScopeV1::Grant,
                    "retry_action":RelayRetryActionV1::RetryAfterWindow
                }),
            );
        }
        // Historical untyped form of the tenant ceiling. The Store now reports
        // it as `RelayTenantCapacityRefusalV1`; without a computed action the
        // only safe advice is the operator.
        Some(RelayFailure::Capacity) => {
            return refusal(
                "relay_capacity_exhausted",
                "Retained relay content reached the tenant limit; contact the operator.",
                json!({
                    "scope":RelayRefusalScopeV1::Tenant,
                    "retry_action":RelayRetryActionV1::ContactOperator
                }),
            );
        }
        Some(RelayFailure::Expired) => (
            StatusCode::FORBIDDEN,
            "relay_expired",
            "The relay message is no longer available for a new acknowledgement.",
        ),
        None => return workload::failure(error),
    };
    workload::response(status, code, message)
}
fn store_failure(
    store: &impl crate::tenant_store::StoreHealth,
    error: anyhow::Error,
) -> (StatusCode, Json<Value>) {
    if store.store_healthy() {
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
                .sqlite_api()?
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
                    .sqlite_api()?
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
                .sqlite_api()?
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
                    .sqlite_api()?
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
                    .sqlite_api()?
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
                    .sqlite_api()?
                    .relay_prune(&input)
                    .map_err(|error| store_failure(&tenant.store, error))?,
            ))
        })
        .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionQuery {
    limit: Option<u32>,
}
/// Runner or viewer of the authenticated tenant. A worker proof is not a
/// control credential and is refused before any Store work.
async fn retention(
    ReadIdentity(tenant, _): ReadIdentity,
    query: Result<Query<RetentionQuery>, QueryRejection>,
) -> ApiResult<Json<RelayRetentionStatusV1>> {
    let Query(query) = query.map_err(|_| failure(RelayFailure::InvalidRequest.into()))?;
    let limit = query.limit.unwrap_or(32);
    tenant
        .blocking(true, move |tenant| {
            Ok(Json(
                tenant
                    .store
                    .sqlite_api()?
                    .relay_retention_status(limit)
                    .map_err(|error| store_failure(&tenant.store, error))?,
            ))
        })
        .await
}
