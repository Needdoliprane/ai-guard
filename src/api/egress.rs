//! Broker-facing egress routes and the tenant read of egress facts.
//!
//! The tenant is the one of the authenticated runner (or reader) credential;
//! no body field selects it. Decisions are durable in the tenant journal before
//! the response, and an audit failure answers 503 so the broker refuses the
//! connection. Free-text causes never leave the gateway.

use super::*;
use crate::{
    audit::egress_decisions::EgressJournalError,
    control::egress_decision::{
        EgressDecideRequestV1, EgressDecisionV1, EgressObserveRequestV1, EgressReceiptV1,
        EgressRenewRequestV1, EgressRenewResponseV1,
    },
};

pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/egress/decisions", post(decide))
        .route("/egress/observations", post(observe))
        .route("/egress/renewals", post(renew))
        .route("/egress/events", get(events))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn journal_failure(failure: anyhow::Error) -> (StatusCode, Json<Value>) {
    match failure.downcast_ref::<EgressJournalError>() {
        Some(EgressJournalError::Conflict) => (
            StatusCode::CONFLICT,
            Json(json!({"error":{"code":"egress_conflict","retry_action":"none"}})),
        ),
        Some(EgressJournalError::Invalid) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"egress_invalid","retry_action":"none"}})),
        ),
        None => unavailable(failure),
    }
}

fn healthy(tenant: &TenantState) -> ApiResult<()> {
    if tenant.store.healthy() {
        Ok(())
    } else {
        Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Intégrité de l'audit défaillante : sortie réseau refusée.",
        ))
    }
}

async fn decide(
    RunnerIdentity(tenant): RunnerIdentity,
    Json(request): Json<EgressDecideRequestV1>,
) -> ApiResult<Json<EgressDecisionV1>> {
    tenant.write_quota()?;
    healthy(&tenant)?;
    tenant
        .blocking(false, move |tenant| {
            let policy = tenant.store.egress_policy();
            tenant
                .store
                .egress_decide(&request, &policy, now_ms())
                .map(Json)
                .map_err(journal_failure)
        })
        .await
}

async fn observe(
    RunnerIdentity(tenant): RunnerIdentity,
    Json(request): Json<EgressObserveRequestV1>,
) -> ApiResult<Json<EgressReceiptV1>> {
    tenant.write_quota()?;
    healthy(&tenant)?;
    tenant
        .blocking(false, move |tenant| {
            tenant
                .store
                .egress_observe(&request)
                .map(Json)
                .map_err(journal_failure)
        })
        .await
}

async fn renew(
    RunnerIdentity(tenant): RunnerIdentity,
    Json(request): Json<EgressRenewRequestV1>,
) -> ApiResult<Json<EgressRenewResponseV1>> {
    healthy(&tenant)?;
    tenant
        .blocking(false, move |tenant| {
            let policy = tenant.store.egress_policy();
            tenant
                .store
                .egress_renew(&request, &policy, now_ms())
                .map(Json)
                .map_err(journal_failure)
        })
        .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EgressEventsQuery {
    session_id: String,
    limit: Option<usize>,
}

async fn events(
    ReadIdentity(tenant, _): ReadIdentity,
    Query(query): Query<EgressEventsQuery>,
) -> ApiResult<Json<Value>> {
    uuid::Uuid::parse_str(&query.session_id)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "Session invalide."))?;
    tenant
        .blocking(true, move |tenant| {
            let limit = query.limit.unwrap_or(200).clamp(1, 1_000);
            let items = tenant
                .store
                .egress_session_events(&query.session_id, limit)
                .map_err(unavailable)?;
            let complete = items.len() < limit;
            Ok(Json(json!({
                "items": items,
                "complete": complete,
                "coverage": {
                    "proxy_requests": "observed_by_broker",
                    "direct_attempts": "direct_attempts_unobserved",
                    "tls": "opaque",
                    "remote_tools": "remote_unmediated"
                }
            })))
        })
        .await
}
