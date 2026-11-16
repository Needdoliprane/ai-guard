//! Shared Runner recovery is restricted to centrally proven no-effect state.
use super::{auth::VerifiedControlRecoveryRunnerV2, *};
use crate::{
    audit::kubernetes_session::KubernetesSessionControlRecoveryAuthorizationV2,
    control::kubernetes_session::{
        KubernetesSessionControlRecoveryTargetV2, KubernetesSessionReservationCloseV2,
        KubernetesSessionReservationFailureV2,
    },
};
use axum::{body::to_bytes, extract::rejection::PathRejection};
use serde::Serialize;

const MAX_INPUT: usize = 80 * 1024;
mod lifecycle;

pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .merge(lifecycle::routes())
        .route(
            "/kubernetes/session-reservations/{id}/control-recovery",
            post(recover),
        )
        .layer(DefaultBodyLimit::max(MAX_INPUT))
}

// This is an API DTO, distinct from descriptive Core targets and capabilities.
#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum ControlRecoveryRequestV2 {
    ObserveReserved {
        version: u32,
        reservation_input_hash: String,
    },
    CloseBeforeEffect {
        version: u32,
        close: KubernetesSessionReservationCloseV2,
    },
    LookupCloseReceipt {
        version: u32,
        reservation_input_hash: String,
        close_input_hash: String,
        operation_id: String,
    },
}

fn invalid() -> (StatusCode, Json<Value>) {
    workload::response(
        StatusCode::BAD_REQUEST,
        "invalid_session_control_recovery_request",
        "The request does not match the closed session recovery contract.",
    )
}

fn forbidden() -> (StatusCode, Json<Value>) {
    workload::response(
        StatusCode::FORBIDDEN,
        "session_control_recovery_forbidden",
        "The current tenant policy does not authorize this recovery operation.",
    )
}

fn failure(store: &Store, error: anyhow::Error) -> (StatusCode, Json<Value>) {
    if !store.healthy() {
        return unavailable(error);
    }
    use KubernetesSessionReservationFailureV2 as F;
    let (status, code, message) = match error.downcast_ref::<F>() {
        Some(F::InvalidRequest) => return invalid(),
        Some(F::PermissionDenied) => return forbidden(),
        Some(F::NotFound) => (
            StatusCode::NOT_FOUND,
            "session_control_recovery_not_found",
            "The exact reservation or receipt was not found in the authenticated scope.",
        ),
        Some(F::Conflict) => (
            StatusCode::CONFLICT,
            "session_control_recovery_conflict",
            "The operation conflicts with retained content or state.",
        ),
        Some(F::QuotaExceeded) => (
            StatusCode::TOO_MANY_REQUESTS,
            "session_control_recovery_quota_exceeded",
            "The configured audit or tenant budget cannot admit this operation.",
        ),
        Some(F::MigrationRequired | F::SupervisionInactive | F::Unavailable) | None => {
            return unavailable(error);
        }
    };
    workload::response(status, code, message)
}

impl ControlRecoveryRequestV2 {
    fn target(&self, id: &str) -> ApiResult<KubernetesSessionControlRecoveryTargetV2> {
        use crate::control::kubernetes_session::validation::{hash, uuid};
        let target = match self {
            Self::ObserveReserved {
                version,
                reservation_input_hash,
            } => {
                if *version != 2 {
                    return Err(invalid());
                }
                hash(reservation_input_hash).map_err(|_| invalid())?;
                KubernetesSessionControlRecoveryTargetV2::ObserveReserved {
                    reservation_id: id.into(),
                    reservation_input_hash: reservation_input_hash.clone(),
                }
            }
            Self::CloseBeforeEffect { version, close } => {
                if *version != 2 || close.reservation_id != id {
                    return Err(invalid());
                }
                close.validate().map_err(|_| invalid())?;
                KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect {
                    close: close.clone(),
                }
            }
            Self::LookupCloseReceipt {
                version,
                reservation_input_hash,
                close_input_hash,
                operation_id,
            } => {
                if *version != 2 || operation_id == id {
                    return Err(invalid());
                }
                hash(reservation_input_hash).map_err(|_| invalid())?;
                hash(close_input_hash).map_err(|_| invalid())?;
                uuid(operation_id).map_err(|_| invalid())?;
                KubernetesSessionControlRecoveryTargetV2::LookupCloseReceipt {
                    reservation_id: id.into(),
                    reservation_input_hash: reservation_input_hash.clone(),
                    close_input_hash: close_input_hash.clone(),
                    operation_id: operation_id.clone(),
                }
            }
        };
        Ok(target)
    }
}

async fn input(request: Request) -> ApiResult<ControlRecoveryRequestV2> {
    if request.uri().query().is_some() {
        return Err(invalid());
    }
    let content_types = request.headers().get_all(header::CONTENT_TYPE);
    if content_types.iter().count() != 1
        || !content_types
            .iter()
            .next()
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
            })
    {
        return Err(invalid());
    }
    // Authentication has completed; no Store-I/O lease spans a peer upload.
    let bytes = tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(request.into_body(), MAX_INPUT),
    )
    .await
    .map_err(|_| invalid())?
    .map_err(|_| invalid())?;
    let value = crate::json::decode_unique(&bytes, 64).map_err(|_| invalid())?;
    serde_json::from_value(value).map_err(|_| invalid())
}

async fn recover(
    runner: VerifiedControlRecoveryRunnerV2,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> ApiResult<Response> {
    let Path(id) = path.map_err(|_| invalid())?;
    crate::control::kubernetes_session::validation::uuid(&id).map_err(|_| invalid())?;
    let request = input(request).await?;
    let target = request.target(&id)?;
    runner
        .blocking(move |auth, tenant| {
            let policy = tenant
                .session_control_recovery_policy
                .as_deref()
                .ok_or_else(forbidden)?;
            // Keep the exact authenticated Store Arc; a redaction sibling is not
            // interchangeable with this request's Store authority.
            let store = &tenant.store;
            let redactor = store.redactor_union(&Redactor::new(vec![]));
            let value = serde_json::to_value(&request).map_err(|_| invalid())?;
            if redactor.value_unclipped(&value) != value {
                return Err(invalid());
            }
            let authorization = store
                .authorize_kubernetes_session_control_recovery(auth, &target, policy)
                .map_err(|error| failure(store, error))?;
            match (request, authorization) {
                (
                    ControlRecoveryRequestV2::ObserveReserved { .. },
                    KubernetesSessionControlRecoveryAuthorizationV2::BeforeEffect(cap),
                ) => {
                    let body = store
                        .observe_kubernetes_session_control_recovery(auth, &cap, policy)
                        .map_err(|error| failure(store, error))?;
                    kubernetes::encode_response(StatusCode::OK, &body, &redactor)
                }
                (
                    ControlRecoveryRequestV2::CloseBeforeEffect { close, .. },
                    KubernetesSessionControlRecoveryAuthorizationV2::BeforeEffect(cap),
                ) => {
                    let body = store
                        .close_kubernetes_session_control_before_effect(auth, &cap, &close, policy)
                        .map_err(|error| failure(store, error))?;
                    kubernetes::encode_response(StatusCode::OK, &body, &redactor)
                }
                (
                    ControlRecoveryRequestV2::LookupCloseReceipt { .. }
                    | ControlRecoveryRequestV2::CloseBeforeEffect { .. },
                    KubernetesSessionControlRecoveryAuthorizationV2::ReceiptLookup(cap),
                ) => {
                    let body = store
                        .lookup_kubernetes_session_control_recovery_receipt(auth, &cap, policy)
                        .map_err(|error| failure(store, error))?;
                    kubernetes::encode_response(StatusCode::OK, &body, &redactor)
                }
                _ => Err(forbidden()),
            }
        })
        .await
}
