//! HTTP bindings for the versioned, tenant-scoped supervision Store.
use super::*;
use crate::control::{
    AdmissionResultV1, ArmResultV1, ArmV1, ArmedStartClaimV1, ControlProtocolV1, ExportRequestV1,
    HelperClaimResultV1, HelperCleanupV1, HelperObservationV1, HelperStartClaimV1,
    MutationReceiptV1, MutationResultV1, SupervisedAdmissionV1,
};

// Deliberately merged only into /api/v1, preserving historical /api aliases.
pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/control/protocol", get(protocol))
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
        .execution_arm(arm_id)
        .map_err(control_error)?
        .filter(|result| result.arm.worker_id == id)
        .ok_or_else(|| control_error(ControlFailure::NotFound.into()))
}

async fn protocol(ReadIdentity(tenant, role): ReadIdentity) -> ApiResult<Json<ControlProtocolV1>> {
    tenant
        .blocking(true, move |tenant| {
            let state = tenant.store.supervision_state().map_err(control_error)?;
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
    let replay = tenant
        .blocking(false, move |tenant| {
            Ok(tenant
                .store
                .worker_operation(&operation_id)
                .map_err(control_error)?
                .is_some())
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
                tenant.store.supervision_state().map_err(control_error)
            })
            .await?;
        if ![3, 4, 5, 6, 7, 8, 9, 10, 11, 12].contains(&state.schema_version) {
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
                    .request_export(&request)
                    .map_err(control_error)?,
            ))
        })
        .await
}
