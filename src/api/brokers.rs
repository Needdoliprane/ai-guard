//! Broker registry routes. Heartbeat assertions never act as runtime observations.
use super::workload::{VerifiedBroker, match_path, parse_json};
use super::*;
use crate::broker_registry::*;

pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/brokers", get(list))
        .route("/brokers/grants", post(grant))
        .route("/brokers/{id}/revoke", post(revoke))
        .route("/brokers/heartbeat", post(heartbeat))
        .route("/brokers/self", get(own))
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
            Ok(Json(tenant.store.revoke_broker(&input).map_err(
                |error| workload::store_failure(&tenant.store, error),
            )?))
        })
        .await
}
async fn heartbeat(
    broker: VerifiedBroker,
    body: Result<Json<BrokerHeartbeatV1>, JsonRejection>,
) -> ApiResult<Json<BrokerMutationResultV1>> {
    let input = parse_json(body)?;
    broker
        .blocking(move |tenant, identity| {
            // Heartbeats use the untrusted Read gate and workload audit budget,
            // never the runner's mutation-frequency allowance.
            Ok(Json(
                tenant
                    .store
                    .broker_heartbeat(&identity, &input)
                    .map_err(|error| workload::store_failure(&tenant.store, error))?,
            ))
        })
        .await
}
async fn own(broker: VerifiedBroker) -> ApiResult<Json<BrokerViewV1>> {
    broker
        .blocking(move |tenant, identity| {
            Ok(Json(tenant.store.broker_for_identity(&identity).map_err(
                |error| workload::store_failure(&tenant.store, error),
            )?))
        })
        .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    cursor: Option<String>,
    limit: Option<u32>,
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
                .brokers_page(query.cursor.as_deref(), limit)
                .map_err(|error| workload::store_failure(&tenant.store, error))?;
            let next_cursor = if items.len() == limit as usize {
                let last = items.last().map(|item| item.broker.broker_id.clone());
                if let Some(last) = last {
                    if tenant
                        .store
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
