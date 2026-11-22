//! Broker registry routes. Heartbeat assertions never act as runtime observations.
use super::workload::{VerifiedBroker, match_path, parse_json};
use super::*;
use crate::broker_registry::*;

pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/brokers", get(list))
        .route("/federation/workers", get(federation_workers))
        .route("/brokers/grants", post(grant))
        .route("/brokers/{id}/revoke", post(revoke))
        .route("/brokers/heartbeat", post(heartbeat))
        .route(
            "/brokers/checkpoints",
            post(checkpoint).layer(DefaultBodyLimit::max(MAX_BROKER_CHECKPOINT_BYTES)),
        )
        .route("/brokers/checkpoints/self", get(own_checkpoint))
        .route("/brokers/self", get(own))
        .route("/brokers/identity", get(identity))
        .layer(middleware::from_fn(workload::request_guard))
}
async fn grant(
    RunnerIdentity(tenant): RunnerIdentity,
    body: Result<Json<BrokerGrantMutationV1>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<BrokerMutationResultV1>)> {
    let input = parse_json(body)?;
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            let result = tenant
                .store
                .sqlite_api()?
                .mutate_broker_grant(&input)
                .map_err(|error| workload::store_failure(&tenant.store, error))?;
            Ok((
                if result.replayed {
                    StatusCode::OK
                } else {
                    StatusCode::CREATED
                },
                Json(result),
            ))
        })
        .await
}
async fn revoke(
    RunnerIdentity(tenant): RunnerIdentity,
    Path(id): Path<String>,
    body: Result<Json<BrokerRevocationV1>, JsonRejection>,
) -> ApiResult<Json<BrokerMutationResultV1>> {
    let input = parse_json(body)?;
    match_path(&id, &input.broker_id)?;
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            Ok(Json(
                tenant
                    .store
                    .sqlite_api()?
                    .revoke_broker(&input)
                    .map_err(|error| workload::store_failure(&tenant.store, error))?,
            ))
        })
        .await
}
async fn heartbeat(
    broker: VerifiedBroker,
    body: Result<Json<BrokerHeartbeatV1>, JsonRejection>,
) -> ApiResult<Json<BrokerMutationResultV1>> {
    let input = parse_json(body)?;
    broker
        .blocking(move |tenant, identity, _redactor| {
            // Heartbeats use the untrusted Read gate and workload audit budget,
            // never the runner's mutation-frequency allowance.
            Ok(Json(
                tenant
                    .store
                    .sqlite_api()?
                    .broker_heartbeat(&identity, &input)
                    .map_err(|error| workload::store_failure(&tenant.store, error))?,
            ))
        })
        .await
}
async fn own(broker: VerifiedBroker) -> ApiResult<Json<BrokerViewV1>> {
    broker
        .blocking(move |tenant, identity, _redactor| {
            Ok(Json(
                tenant
                    .store
                    .sqlite_api()?
                    .broker_for_identity(&identity)
                    .map_err(|error| workload::store_failure(&tenant.store, error))?,
            ))
        })
        .await
}

async fn checkpoint(
    broker: VerifiedBroker,
    body: Result<Json<BrokerCheckpointV1>, JsonRejection>,
) -> ApiResult<Json<BrokerCheckpointResultV1>> {
    let input = parse_json(body)?;
    broker
        .blocking(move |tenant, identity, redactor| {
            // The authenticated broker, not a body field, chooses the Store.
            // The mutation retains its own workload budget and idempotence.
            if !checkpoint_is_public(&input, &redactor)? {
                return Err(workload::invalid());
            }
            let scope = tenant
                .store
                .sqlite_api()?
                .broker_authenticated_identity(&identity)
                .map_err(|error| workload::store_failure(&tenant.store, error))?;
            if !checkpoint_is_public(&scope, &redactor)? {
                return Err(checkpoint_unavailable());
            }
            let commitment = Store::broker_checkpoint_input_hash(&scope, &input)
                .map_err(|error| workload::store_failure(&tenant.store, error))?;
            if !checkpoint_is_public(&commitment, &redactor)? {
                return Err(workload::invalid());
            }
            let result = tenant
                .store
                .sqlite_api()?
                .broker_checkpoint(&identity, &input)
                .map_err(|error| workload::store_failure(&tenant.store, error))?;
            // Never mask identifiers or commitments into a different receipt.
            // A withheld reply does not imply rollback of its durable mutation.
            if !checkpoint_is_public(&result, &redactor)? {
                return Err(checkpoint_unavailable());
            }
            Ok(Json(result))
        })
        .await
}

async fn own_checkpoint(broker: VerifiedBroker) -> ApiResult<Json<BrokerCheckpointViewV1>> {
    broker
        .blocking(move |tenant, identity, redactor| {
            let view = tenant
                .store
                .sqlite_api()?
                .broker_checkpoint_for_identity(&identity)
                .map_err(|error| workload::store_failure(&tenant.store, error))?;
            if !checkpoint_is_public(&view, &redactor)? {
                return Err(checkpoint_unavailable());
            }
            Ok(Json(view))
        })
        .await
}

fn checkpoint_is_public(value: &impl serde::Serialize, redactor: &Redactor) -> ApiResult<bool> {
    let value = serde_json::to_value(value).map_err(|_| checkpoint_unavailable())?;
    Ok(redactor.value_unclipped(&value) == value)
}

fn checkpoint_unavailable() -> (StatusCode, Json<Value>) {
    workload::response(
        StatusCode::SERVICE_UNAVAILABLE,
        "broker_checkpoint_unavailable",
        "Broker checkpoint metadata is unavailable.",
    )
}

async fn identity(broker: VerifiedBroker) -> ApiResult<Json<BrokerIdentityViewV1>> {
    broker
        .blocking(move |tenant, identity, redactor| {
            let view = tenant
                .store
                .sqlite_api()?
                .broker_authenticated_identity(&identity)
                .map_err(|error| workload::store_failure(&tenant.store, error))?;
            let value = serde_json::to_value(&view).map_err(|_| identity_unavailable())?;
            if redactor.value_unclipped(&value) != value {
                // A scope field can itself equal the current proof. Never
                // substitute masked IDs or return a partial identity envelope.
                return Err(identity_unavailable());
            }
            Ok(Json(view))
        })
        .await
}

fn identity_unavailable() -> (StatusCode, Json<Value>) {
    workload::response(
        StatusCode::SERVICE_UNAVAILABLE,
        "identity_unavailable",
        "Authenticated broker identity is unavailable.",
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    cursor: Option<String>,
    limit: Option<u32>,
}
async fn federation_workers(
    ReadIdentity(tenant, _): ReadIdentity,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> ApiResult<Json<FederationWorkerPageV1>> {
    let Query(query) = query.map_err(|_| workload::invalid())?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(workload::invalid());
    }
    if let Some(cursor) = &query.cursor {
        registry_uuid(cursor).map_err(workload::failure)?;
    }
    tenant
        .blocking(true, move |tenant| {
            Ok(Json(
                tenant
                    .store
                    .sqlite_api()?
                    .federation_workers_page(query.cursor.as_deref(), limit)
                    .map_err(|error| workload::store_failure(&tenant.store, error))?,
            ))
        })
        .await
}
async fn list(
    ReadIdentity(tenant, _): ReadIdentity,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> ApiResult<Json<Value>> {
    let Query(query) = query.map_err(|_| workload::invalid())?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(workload::invalid());
    }
    if let Some(cursor) = &query.cursor {
        registry_uuid(cursor).map_err(workload::failure)?;
    }
    tenant
        .blocking(true, move |tenant| {
            let items = tenant
                .store
                .sqlite_api()?
                .brokers_page(query.cursor.as_deref(), limit)
                .map_err(|error| workload::store_failure(&tenant.store, error))?;
            let next_cursor = if items.len() == limit as usize {
                let last = items.last().map(|item| item.broker.broker_id.clone());
                if let Some(last) = last {
                    if tenant
                        .store
                        .sqlite_api()?
                        .brokers_page(Some(&last), 1)
                        .map_err(|error| workload::store_failure(&tenant.store, error))?
                        .is_empty()
                    {
                        None
                    } else {
                        Some(last)
                    }
                } else {
                    None
                }
            } else {
                None
            };
            Ok(Json(
                json!({"version":1,"items":items,"next_cursor":next_cursor}),
            ))
        })
        .await
}
