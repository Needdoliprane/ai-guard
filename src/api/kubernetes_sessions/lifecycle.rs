//! Broker-authenticated pre-effect R2 bindings. Runtime effects stay closed.
use super::*;
use crate::{
    api::workload::VerifiedBroker,
    audit::kubernetes_session::KubernetesSessionRecoveryAuthorizationV2,
    control::kubernetes_session::{lifecycle::*, *},
};
use axum::{extract::FromRequestParts, http::request::Parts};
use serde::de::DeserializeOwned;

fn invalid() -> (StatusCode, Json<Value>) {
    workload::response(
        StatusCode::BAD_REQUEST,
        "invalid_session_lifecycle_request",
        "The request does not match the closed pre-effect session lifecycle contract.",
    )
}
fn forbidden() -> (StatusCode, Json<Value>) {
    workload::response(
        StatusCode::FORBIDDEN,
        "session_lifecycle_forbidden",
        "The current broker scope or tenant policy does not authorize this session lifecycle operation.",
    )
}
fn failure(store: &Store, error: anyhow::Error) -> (StatusCode, Json<Value>) {
    use KubernetesSessionReservationFailureV2 as F;
    if !store.healthy() {
        return unavailable(error);
    }
    match error.downcast_ref::<F>() {
        Some(F::InvalidRequest) => invalid(),
        Some(F::PermissionDenied) => forbidden(),
        Some(F::Conflict) => workload::response(
            StatusCode::CONFLICT,
            "session_lifecycle_conflict",
            "The request conflicts with the exact durable reservation or retained operation.",
        ),
        Some(F::NotFound) => workload::response(
            StatusCode::NOT_FOUND,
            "session_lifecycle_not_found",
            "The exact session reservation or operation was not found in the authenticated broker scope.",
        ),
        Some(F::QuotaExceeded) => workload::response(
            StatusCode::TOO_MANY_REQUESTS,
            "session_lifecycle_capacity",
            "The configured tenant quota cannot admit this operation.",
        ),
        Some(F::MigrationRequired | F::SupervisionInactive | F::Unavailable) => workload::response(
            StatusCode::SERVICE_UNAVAILABLE,
            "session_lifecycle_unavailable",
            "The pre-effect session lifecycle is unavailable or its format is unsupported.",
        ),
        // Preserve the existing typed audit-capacity and clock responses.
        None => workload::store_failure(store, error),
    }
}

struct SessionBroker(VerifiedBroker);
impl FromRequestParts<GatewayState> for SessionBroker {
    type Rejection = (StatusCode, Json<Value>);
    async fn from_request_parts(parts: &mut Parts, state: &GatewayState) -> ApiResult<Self> {
        // Native17 requires its own AuthenticatedExecutionBroker/source/grant
        // and same-transaction Store port. Generic workload or Runner proofs
        // cannot substitute for it. Refuse before body/path extraction.
        if state.owned_human.is_some() {
            return Err(workload::response(
                StatusCode::SERVICE_UNAVAILABLE,
                "session_lifecycle_native17_unavailable",
                "The native Source17 broker session lifecycle port is not implemented; no session effect is authorized.",
            ));
        }
        Ok(Self(
            VerifiedBroker::from_request_parts(parts, state).await?,
        ))
    }
}

pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route(
            "/brokers/kubernetes/session-reservation-protocol",
            get(protocol),
        )
        .route("/brokers/kubernetes/session-reservations", post(reserve))
        .route(
            "/brokers/kubernetes/session-reservations/{id}",
            get(observe),
        )
        .route(
            "/brokers/kubernetes/session-reservations/{id}/confirmations",
            post(confirm),
        )
        .route(
            "/brokers/kubernetes/session-reservations/{id}/abandonments",
            post(abandon),
        )
        .route(
            "/brokers/kubernetes/session-reservations/{id}/effects",
            post(claim_effect).get(effect_state),
        )
        .route(
            "/brokers/kubernetes/session-reservations/{id}/effect-results",
            post(effect_result),
        )
        .route(
            "/brokers/kubernetes/session-reservations/{id}/terminals",
            post(effect_terminal),
        )
        .route(
            "/brokers/kubernetes/session-reservations/{id}/settlements",
            post(effect_settlement),
        )
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .layer(middleware::from_fn(workload::request_guard))
}

async fn input<T: DeserializeOwned>(request: Request) -> ApiResult<T> {
    if request.uri().query().is_some() {
        return Err(invalid());
    }
    let types = request.headers().get_all(header::CONTENT_TYPE);
    if types.iter().count() != 1
        || !types
            .iter()
            .next()
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
    {
        return Err(invalid());
    }
    let bytes = tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(request.into_body(), MAX_REQUEST_BYTES),
    )
    .await
    .map_err(|_| invalid())?
    .map_err(|_| invalid())?;
    let value = crate::json::decode_unique(&bytes, 64).map_err(|_| invalid())?;
    serde_json::from_value(value).map_err(|_| invalid())
}
fn screen<T: Serialize>(value: &T, redactor: &Redactor) -> ApiResult<()> {
    let value = serde_json::to_value(value).map_err(|_| invalid())?;
    if redactor.value_unclipped(&value) != value {
        return Err(invalid());
    }
    Ok(())
}
fn output<T: Serialize>(status: StatusCode, value: &T, redactor: &Redactor) -> ApiResult<Response> {
    let value = serde_json::to_value(value)
        .map_err(|_| unavailable(anyhow::anyhow!("session response unavailable")))?;
    if redactor.value_unclipped(&value) != value {
        return Err(unavailable(anyhow::anyhow!(
            "session response contains protected material"
        )));
    }
    let bytes = serde_json::to_vec(&value)
        .map_err(|_| unavailable(anyhow::anyhow!("session response unavailable")))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(unavailable(anyhow::anyhow!(
            "session response exceeds bound"
        )));
    }
    Ok((
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response())
}
fn path_id(path: Result<Path<String>, PathRejection>) -> ApiResult<String> {
    let Path(id) = path.map_err(|_| invalid())?;
    crate::control::kubernetes_session::validation::uuid(&id).map_err(|_| invalid())?;
    Ok(id)
}

async fn protocol(SessionBroker(broker): SessionBroker, request: Request) -> ApiResult<Response> {
    if request.uri().query().is_some() {
        return Err(invalid());
    }
    broker
        .blocking(|tenant, identity, redactor| {
            let identity = tenant
                .store
                .broker_authenticated_identity(&identity)
                .map_err(|e| failure(&tenant.store, e))?;
            if ![11, 12, 13].contains(&identity.schema_version) {
                return Err(failure(
                    &tenant.store,
                    KubernetesSessionReservationFailureV2::MigrationRequired.into(),
                ));
            }
            output(
                StatusCode::OK,
                &SessionProtocolV2 {
                    version: 2,
                    identity,
                    reservation_policy_configured: tenant.session_reservation_policy.is_some(),
                    recovery_policy_configured: tenant.session_recovery_policy.is_some(),
                    effects_available: tenant.session_effect_policy.is_some(),
                },
                &redactor,
            )
        })
        .await
}

async fn reserve(SessionBroker(broker): SessionBroker, request: Request) -> ApiResult<Response> {
    let intent: KubernetesSessionReservationIntentV2 = input(request).await?;
    broker
        .blocking(move |tenant, identity, redactor| {
            screen(&intent, &redactor)?;
            let policy = tenant
                .session_reservation_policy
                .as_deref()
                .ok_or_else(forbidden)?;
            let store = tenant
                .store
                .with_additional_redaction(&redactor)
                .map_err(|e| failure(&tenant.store, e))?;
            let authorization = store
                .authorize_kubernetes_session_reservation(
                    &identity,
                    &intent,
                    policy,
                    &tenant.worker_quota,
                )
                .map_err(|e| failure(&store, e))?;
            let response = store
                .reserve_kubernetes_session(
                    &identity,
                    &authorization,
                    &intent,
                    policy,
                    &tenant.worker_quota,
                )
                .map_err(|e| failure(&store, e))?;
            output(
                if response.replayed {
                    StatusCode::OK
                } else {
                    StatusCode::CREATED
                },
                &response,
                &redactor,
            )
        })
        .await
}

async fn observe(
    SessionBroker(broker): SessionBroker,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> ApiResult<Response> {
    if request.uri().query().is_some() {
        return Err(invalid());
    }
    let id = path_id(path)?;
    broker
        .blocking(move |tenant, identity, redactor| {
            let authorization = tenant
                .store
                .authorize_kubernetes_session_reconciliation(&identity, &id, None)
                .map_err(|e| failure(&tenant.store, e))?;
            let current = tenant
                .store
                .observe_kubernetes_session(&identity, &authorization, &id)
                .map_err(|e| failure(&tenant.store, e))?;
            output(StatusCode::OK, &current, &redactor)
        })
        .await
}

async fn confirm(
    SessionBroker(broker): SessionBroker,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> ApiResult<Response> {
    let id = path_id(path)?;
    let request: SessionConfirmationRequestV2 = input(request).await?;
    request.validate().map_err(|_| invalid())?;
    broker
        .blocking(move |tenant, identity, redactor| {
            screen(&request, &redactor)?;
            let authorization = tenant
                .store
                .authorize_kubernetes_session_reconciliation(&identity, &id, None)
                .map_err(|e| failure(&tenant.store, e))?;
            let confirmed = tenant
                .store
                .confirm_kubernetes_session_before_effect(&identity, &authorization, &id)
                .map_err(|e| failure(&tenant.store, e))?;
            if confirmed.snapshot().input_hash != request.reservation_input_hash {
                return Err(forbidden());
            }
            let identity = tenant
                .store
                .broker_authenticated_identity(&identity)
                .map_err(|e| failure(&tenant.store, e))?;
            let response = SessionConfirmationResponseV2 {
                version: 2,
                request_id: request.request_id,
                identity,
                current: KubernetesSessionReservationCurrentV2 {
                    version: 2,
                    snapshot: confirmed.snapshot().clone(),
                    last_event_seq: confirmed.audit_seq(),
                    last_event_hash: confirmed.audit_hash().into(),
                },
            };
            output(StatusCode::OK, &response, &redactor)
        })
        .await
}

async fn abandon(
    SessionBroker(broker): SessionBroker,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> ApiResult<Response> {
    let id = path_id(path)?;
    let close: KubernetesSessionReservationCloseV2 = input(request).await?;
    close.validate().map_err(|_| invalid())?;
    if close.reservation_id != id
        || close.reason != KubernetesSessionReservationCloseReasonV2::PreparationAbandoned
    {
        return Err(invalid());
    }
    broker
        .blocking(move |tenant, identity, redactor| {
            screen(&close, &redactor)?;
            let store = tenant
                .store
                .with_additional_redaction(&redactor)
                .map_err(|e| failure(&tenant.store, e))?;
            let intent = store
                .kubernetes_session_intent_for_reconciliation(&identity, &id)
                .map_err(|e| failure(&store, e))?;
            let broker_view = store
                .broker_authenticated_identity(&identity)
                .map_err(|e| failure(&store, e))?;
            let response = if intent
                .ids()
                .map_err(|e| failure(&store, e))?
                .incarnation_id
                .to_string()
                == broker_view.incarnation_id
            {
                let authorization = store
                    .authorize_kubernetes_session_reconciliation(&identity, &id, Some(&close))
                    .map_err(|e| failure(&store, e))?;
                store
                    .close_kubernetes_session_before_effect(&identity, &authorization, &close)
                    .map_err(|e| failure(&store, e))?
            } else {
                let policy = tenant
                    .session_recovery_policy
                    .as_deref()
                    .ok_or_else(forbidden)?;
                match store
                    .authorize_kubernetes_session_recovery(&identity, &id, Some(&close), policy)
                    .map_err(|e| failure(&store, e))?
                {
                    KubernetesSessionRecoveryAuthorizationV2::BeforeEffect(authorization) => store
                        .recover_kubernetes_session_before_effect(
                            &identity,
                            &authorization,
                            &close,
                            policy,
                        )
                        .map_err(|e| failure(&store, e))?,
                    KubernetesSessionRecoveryAuthorizationV2::ReceiptLookup(authorization) => store
                        .lookup_kubernetes_session_recovery_receipt(
                            &identity,
                            &authorization,
                            &close,
                            policy,
                        )
                        .map_err(|e| failure(&store, e))?,
                }
            };
            output(StatusCode::OK, &response, &redactor)
        })
        .await
}

/// Central claim of one exact effect (G11, R1). 201 with `EmitOnce` for a
/// fresh claim, 200 with `ObserveOnly` for any replay. The tenant comes
/// from the authenticated broker before the body; the effect policy only
/// from the tenant configuration.
async fn claim_effect(
    SessionBroker(broker): SessionBroker,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> ApiResult<Response> {
    use crate::control::kubernetes_session::effects::KubernetesSessionEffectRequestV2;
    let id = path_id(path)?;
    let effect: KubernetesSessionEffectRequestV2 = input(request).await?;
    effect.validate().map_err(|_| invalid())?;
    if effect.reservation_id != id {
        return Err(invalid());
    }
    broker
        .blocking(move |tenant, identity, redactor| {
            screen(&effect, &redactor)?;
            let store = tenant
                .store
                .with_additional_redaction(&redactor)
                .map_err(|e| failure(&tenant.store, e))?;
            let ack = store
                .claim_kubernetes_session_effect(
                    &identity,
                    &effect,
                    tenant.session_effect_policy.as_deref(),
                )
                .map_err(|e| failure(&store, e))?;
            output(
                if ack.replayed {
                    StatusCode::OK
                } else {
                    StatusCode::CREATED
                },
                &ack,
                &redactor,
            )
        })
        .await
}
async fn effect_result(
    SessionBroker(broker): SessionBroker,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> ApiResult<Response> {
    use crate::control::kubernetes_session::effects::KubernetesSessionEffectResultV2;
    let id = path_id(path)?;
    let result: KubernetesSessionEffectResultV2 = input(request).await?;
    result.validate().map_err(|_| invalid())?;
    if result.reservation_id != id {
        return Err(invalid());
    }
    broker
        .blocking(move |tenant, identity, redactor| {
            screen(&result, &redactor)?;
            let state = tenant
                .store
                .record_kubernetes_session_effect_result(&identity, &result)
                .map_err(|e| failure(&tenant.store, e))?;
            output(StatusCode::OK, &state, &redactor)
        })
        .await
}
async fn effect_terminal(
    SessionBroker(broker): SessionBroker,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> ApiResult<Response> {
    use crate::control::kubernetes_session::effects::KubernetesSessionTerminalV2;
    let id = path_id(path)?;
    let terminal: KubernetesSessionTerminalV2 = input(request).await?;
    terminal.validate().map_err(|_| invalid())?;
    if terminal.reservation_id != id {
        return Err(invalid());
    }
    broker
        .blocking(move |tenant, identity, redactor| {
            screen(&terminal, &redactor)?;
            let state = tenant
                .store
                .record_kubernetes_session_terminal(&identity, &terminal)
                .map_err(|e| failure(&tenant.store, e))?;
            output(StatusCode::OK, &state, &redactor)
        })
        .await
}
async fn effect_settlement(
    SessionBroker(broker): SessionBroker,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> ApiResult<Response> {
    use crate::control::kubernetes_session::effects::KubernetesSessionSettlementV2;
    let id = path_id(path)?;
    let settlement: KubernetesSessionSettlementV2 = input(request).await?;
    settlement.validate().map_err(|_| invalid())?;
    if settlement.reservation_id != id {
        return Err(invalid());
    }
    broker
        .blocking(move |tenant, identity, redactor| {
            screen(&settlement, &redactor)?;
            let state = tenant
                .store
                .settle_kubernetes_session(&identity, &settlement)
                .map_err(|e| failure(&tenant.store, e))?;
            output(StatusCode::OK, &state, &redactor)
        })
        .await
}
async fn effect_state(
    SessionBroker(broker): SessionBroker,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> ApiResult<Response> {
    if request.uri().query().is_some() {
        return Err(invalid());
    }
    let id = path_id(path)?;
    broker
        .blocking(move |tenant, identity, redactor| {
            let state = tenant
                .store
                .kubernetes_session_effect_state(&identity, &id)
                .map_err(|e| failure(&tenant.store, e))?;
            output(StatusCode::OK, &state, &redactor)
        })
        .await
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
