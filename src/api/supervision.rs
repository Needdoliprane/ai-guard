//! HTTP bindings for the versioned, tenant-scoped supervision Store.
use super::*;
use crate::control::{
    AdmissionResultV1, ArmResultV1, ArmV1, ArmedStartClaimV1, ControlCapabilitiesV1,
    ControlProtocolV1, ExportRequestV1, HelperClaimResultV1, HelperCleanupV1, HelperObservationV1,
    HelperStartClaimV1, MutationReceiptV1, MutationResultV1, SupervisedAdmissionV1,
    session_control::{LegacyConfirmedPlanV1, canonical_uuid},
};
use axum::{extract::FromRequestParts, http::request::Parts};

// Deliberately merged only into /api/v1, preserving historical /api aliases.
pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/control/protocol", get(protocol))
        .route("/control/capabilities", get(capabilities))
        .route(
            "/control/session-confirmations/{binding_ref}",
            get(confirmed_plan),
        )
        .route("/workers/supervised-reservations", post(reserve))
        .route("/workers/{id}/arms", post(arm))
        .route("/workers/{id}/arms/{arm_id}", get(read_arm))
        .route("/workers/{id}/armed-start-claims", post(claim_worker))
        .route("/workers/{id}/helper-start-claims", post(claim_helper))
        .route("/workers/{id}/helper-observations", post(observe_helper))
        .route("/workers/{id}/helper-cleanup", post(cleanup_helper))
        .route("/workers/{id}/export-requests", post(request_export))
}

fn parse_json<T>(body: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    body.map(|Json(request)| request)
        .map_err(|_| control_error(ControlFailure::InvalidRequest.into()))
}

fn match_worker_path(id: &str, worker_id: &str) -> ApiResult<()> {
    if id != worker_id {
        return Err(control_error(ControlFailure::InvalidRequest.into()));
    }
    Ok(())
}

fn owned_arm(tenant: &TenantState, id: &str, arm_id: &str) -> ApiResult<ArmResultV1> {
    crate::control::uuid(id).map_err(control_error)?;
    tenant
        .store
        .sqlite_api()?
        .execution_arm(arm_id)
        .map_err(control_error)?
        .filter(|result| result.arm.worker_id == id)
        .ok_or_else(|| control_error(ControlFailure::NotFound.into()))
}

enum ProtocolIdentity {
    Legacy(TenantState, &'static str),
    Owned(Arc<auth::owned_control::OwnedProtocol>),
}

impl FromRequestParts<GatewayState> for ProtocolIdentity {
    type Rejection = (StatusCode, Json<Value>);

    async fn from_request_parts(parts: &mut Parts, state: &GatewayState) -> ApiResult<Self> {
        if state.owned_human.is_some() {
            // The private marker retains exact-target classification across
            // the API router's normal prefix stripping, never a client claim.
            if parts.method != axum::http::Method::GET
                || parts.uri.query().is_some()
                || parts
                    .extensions
                    .get::<control_transport::ReservedProtocol>()
                    .is_none()
            {
                return Err(human_sources::unavailable());
            }
            return parts
                .extensions
                .get::<Arc<auth::owned_control::OwnedProtocol>>()
                .cloned()
                .map(Self::Owned)
                .ok_or_else(human_sources::unavailable);
        }
        let ReadIdentity(tenant, role) = ReadIdentity::from_request_parts(parts, state).await?;
        Ok(Self::Legacy(tenant, role))
    }
}

/// Refuses before authentication unless the private control gate admitted it.
struct PrivateControlTransport;

impl FromRequestParts<GatewayState> for PrivateControlTransport {
    type Rejection = (StatusCode, Json<Value>);

    async fn from_request_parts(parts: &mut Parts, _: &GatewayState) -> ApiResult<Self> {
        parts
            .extensions
            .get::<super::control_transport::ControlTransportRequest>()
            .map(|_| Self)
            .ok_or_else(|| error(StatusCode::NOT_FOUND, "Route inconnue."))
    }
}

async fn confirmed_plan(
    _private: PrivateControlTransport,
    broker: workload::VerifiedBroker,
    Path(binding_ref): Path<String>,
) -> ApiResult<Json<LegacyConfirmedPlanV1>> {
    let binding_ref = canonical_uuid(&binding_ref).map_err(control_error)?;
    broker
        .blocking(move |tenant, broker, redactor| {
            let store = tenant.store.sqlite_api()?;
            let plan = store
                .legacy_confirmed_plan(&broker, binding_ref)
                .map_err(|error| {
                    if error.downcast_ref::<ControlFailure>() == Some(&ControlFailure::NotFound) {
                        workload::response(
                            StatusCode::NOT_FOUND,
                            "session_confirmation_not_found",
                            "Confirmation de session introuvable.",
                        )
                    } else {
                        workload::store_failure(&tenant.store, error)
                    }
                })?;
            let value = serde_json::to_value(&plan).map_err(|error| control_error(error.into()))?;
            if redactor.value_unclipped(&value) != value {
                return Err(control_error(anyhow::anyhow!(
                    "confirmed broker plan requires redaction"
                )));
            }
            Ok(Json(plan))
        })
        .await
}

async fn capabilities(
    _private: PrivateControlTransport,
    RunnerIdentity(tenant): RunnerIdentity,
) -> ApiResult<Json<ControlCapabilitiesV1>> {
    tenant
        .blocking(true, move |tenant| {
            let capabilities = tenant.store.control_capabilities().map_err(control_error)?;
            // The answer must match the identity that asked, not only the Store.
            if capabilities.tenant_id != tenant.tenant_id {
                return Err(control_error(anyhow::anyhow!("tenant binding mismatch")));
            }
            Ok(Json(capabilities))
        })
        .await
}

async fn protocol(identity: ProtocolIdentity) -> ApiResult<Json<ControlProtocolV1>> {
    let (tenant, role) = match identity {
        ProtocolIdentity::Owned(proof) => return proof.read().await,
        ProtocolIdentity::Legacy(tenant, role) => (tenant, role),
    };
    tenant
        .blocking(true, move |tenant| {
            let state = tenant
                .store
                .sqlite_api()?
                .supervision_state()
                .map_err(control_error)?;
            Ok(Json(ControlProtocolV1 {
                version: state.version,
                schema_version: state.schema_version,
                control_namespace_uuid: state.control_namespace_uuid,
                tenant_id: tenant.tenant_id,
                role: role.into(),
                protocols: state.protocols,
                active: state.active,
            }))
        })
        .await
}

fn validate_before_policy(s: &GatewayState, request: &SupervisedAdmissionV1) -> ApiResult<()> {
    request.validate().map_err(control_error)?;
    // Match the Store's bounded typed admission envelope before a paid policy call.
    // This is a preflight only; the Store rechecks the actual transaction input.
    let input = json!({"action":"admission","request":request});
    let bytes = serde_json::to_vec(&input)
        .map_err(|_| control_error(ControlFailure::InvalidRequest.into()))?;
    if bytes.len() > 65536 || s.redactor.value(&input) != input {
        return Err(control_error(ControlFailure::InvalidRequest.into()));
    }
    Ok(())
}

async fn reserve(
    RunnerIdentity(tenant): RunnerIdentity,
    State(s): State<GatewayState>,
    body: Result<Json<SupervisedAdmissionV1>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<AdmissionResultV1>)> {
    tenant.write_quota()?;
    let request = parse_json(body)?;
    validate_before_policy(&s, &request)?;
    let operation_id = request.admission.operation_id.clone();
    let session_id = request.admission.launch.session_id.clone();
    let replay = tenant
        .blocking(false, move |tenant| {
            tenant
                .store
                .sqlite_api()?
                .manual_admission_replay(&operation_id, &session_id)
                .map_err(control_error)
        })
        .await?;
    let (decision, jev) = if replay {
        (
            policy::evaluate(&s.policy, &request.admission.decision_request),
            Value::Null,
        )
    } else {
        let state = tenant
            .blocking(false, |tenant| {
                tenant
                    .store
                    .sqlite_api()?
                    .supervision_state()
                    .map_err(control_error)
            })
            .await?;
        if ![3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13].contains(&state.schema_version) {
            return Err(control_error(ControlFailure::MigrationRequired.into()));
        }
        if !state.active {
            return Err(control_error(ControlFailure::SupervisionInactive.into()));
        }
        fresh_policy(&s, &request.admission.decision_request).await
    };
    let result = tenant
        .blocking(false, move |tenant| {
            tenant
                .store
                .sqlite_api()?
                .admit_supervised(&request, &s.policy, decision, jev, &tenant.worker_quota)
                .map_err(control_error)
        })
        .await?;
    let status = match result.refusal {
        Some(AdmissionRefusalV1::Policy) => StatusCode::FORBIDDEN,
        Some(AdmissionRefusalV1::Quota) => StatusCode::TOO_MANY_REQUESTS,
        None if result.replayed => StatusCode::OK,
        None => StatusCode::CREATED,
    };
    Ok((status, Json(result)))
}

async fn arm(
    RunnerIdentity(tenant): RunnerIdentity,
    Path(id): Path<String>,
    body: Result<Json<ArmV1>, JsonRejection>,
) -> ApiResult<Json<ArmResultV1>> {
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            let request = parse_json(body)?;
            match_worker_path(&id, &request.worker_id)?;
            Ok(Json(
                tenant
                    .store
                    .sqlite_api()?
                    .arm_execution(&request)
                    .map_err(control_error)?,
            ))
        })
        .await
}

async fn read_arm(
    ReadIdentity(tenant, _): ReadIdentity,
    Path((id, arm_id)): Path<(String, String)>,
) -> ApiResult<Json<ArmResultV1>> {
    tenant
        .blocking(true, move |tenant| {
            Ok(Json(owned_arm(&tenant, &id, &arm_id)?))
        })
        .await
}

async fn claim_worker(
    RunnerIdentity(tenant): RunnerIdentity,
    Path(id): Path<String>,
    body: Result<Json<ArmedStartClaimV1>, JsonRejection>,
) -> ApiResult<Json<MutationResultV1>> {
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            let request = parse_json(body)?;
            match_worker_path(&id, &request.worker_id)?;
            owned_arm(&tenant, &id, &request.arm_receipt.arm_id)?;
            Ok(Json(
                tenant
                    .store
                    .sqlite_api()?
                    .claim_armed_start(&request)
                    .map_err(control_error)?,
            ))
        })
        .await
}

async fn claim_helper(
    RunnerIdentity(tenant): RunnerIdentity,
    Path(id): Path<String>,
    body: Result<Json<HelperStartClaimV1>, JsonRejection>,
) -> ApiResult<Json<HelperClaimResultV1>> {
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            let request = parse_json(body)?;
            owned_arm(&tenant, &id, &request.arm_receipt.arm_id)?;
            Ok(Json(
                tenant
                    .store
                    .sqlite_api()?
                    .claim_helper_start(&request)
                    .map_err(control_error)?,
            ))
        })
        .await
}

async fn observe_helper(
    RunnerIdentity(tenant): RunnerIdentity,
    Path(id): Path<String>,
    body: Result<Json<HelperObservationV1>, JsonRejection>,
) -> ApiResult<Json<MutationReceiptV1>> {
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            let request = parse_json(body)?;
            owned_arm(&tenant, &id, &request.arm_receipt.arm_id)?;
            Ok(Json(
                tenant
                    .store
                    .sqlite_api()?
                    .observe_helper(&request)
                    .map_err(control_error)?,
            ))
        })
        .await
}

async fn cleanup_helper(
    RunnerIdentity(tenant): RunnerIdentity,
    Path(id): Path<String>,
    body: Result<Json<HelperCleanupV1>, JsonRejection>,
) -> ApiResult<Json<MutationReceiptV1>> {
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            let request = parse_json(body)?;
            owned_arm(&tenant, &id, &request.arm_receipt.arm_id)?;
            Ok(Json(
                tenant
                    .store
                    .sqlite_api()?
                    .record_helper_cleanup(&request)
                    .map_err(control_error)?,
            ))
        })
        .await
}

async fn request_export(
    RunnerIdentity(tenant): RunnerIdentity,
    Path(id): Path<String>,
    body: Result<Json<ExportRequestV1>, JsonRejection>,
) -> ApiResult<Json<MutationReceiptV1>> {
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            let request = parse_json(body)?;
            match_worker_path(&id, &request.worker_id)?;
            Ok(Json(
                tenant
                    .store
                    .sqlite_api()?
                    .request_export(&request)
                    .map_err(control_error)?,
            ))
        })
        .await
}
