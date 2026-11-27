//! HTTP bindings for the versioned, tenant-scoped supervision Store.
use super::*;
use crate::audit::legacy_session_prepare::{
    LegacyNamedCancelReservationV1, LegacyNamedInitialDecision,
};
use crate::control::{
    AdmissionResultV1, ArmResultV1, ArmV1, ArmedStartClaimV1, ControlCapabilitiesV1,
    ControlProtocolV1, ExportRequestV1, HelperClaimResultV1, HelperCleanupV1, HelperObservationV1,
    HelperStartClaimV1, MutationReceiptV1, MutationResultV1, SupervisedAdmissionV1,
    session_control::{
        LEGACY_CONFIRMED_LAUNCH_SPEC_RESPONSE_BYTES, LEGACY_NAMED_ADMISSION_RECEIPT_NOT_FOUND,
        LEGACY_NAMED_ADMISSION_REQUEST_BYTES, LEGACY_NAMED_IMPORT_ARM_RECEIPT_NOT_FOUND,
        LEGACY_NAMED_IMPORT_ARM_RESPONSE_BYTES, LegacyConfirmedPlanV1,
        LegacyNamedAdmissionReceiptV1, LegacyNamedImportArmRequestV1,
        LegacyNamedSupervisedAdmissionV1, canonical_uuid,
    },
};
use crate::tenant_store::StoreHealth;
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, FromRequestParts},
    http::request::Parts,
};

// Deliberately merged only into /api/v1, preserving historical /api aliases.
pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/control/protocol", get(protocol))
        .route("/control/capabilities", get(capabilities))
        .route(
            "/control/session-confirmations/{binding_ref}",
            get(confirmed_plan),
        )
        .route(
            "/control/session-confirmations/{binding_ref}/launch-spec",
            get(confirmed_launch_spec),
        )
        .route(
            "/control/session-confirmations/{binding_ref}/initial-decision",
            post(initial_named_decision),
        )
        .route(
            "/control/session-confirmations/{binding_ref}/admission",
            post(reserve_named),
        )
        .route(
            "/control/session-confirmations/{binding_ref}/admission/reconcile",
            post(reconcile_named_admission),
        )
        .route(
            "/control/session-confirmations/{binding_ref}/admission/cancel-reserved",
            post(cancel_named_reservation)
                .layer(DefaultBodyLimit::max(LEGACY_NAMED_ADMISSION_REQUEST_BYTES)),
        )
        .route(
            "/control/session-confirmations/{binding_ref}/admission/import-arm",
            post(arm_named_import)
                .layer(DefaultBodyLimit::max(LEGACY_NAMED_ADMISSION_REQUEST_BYTES)),
        )
        .route(
            "/control/session-confirmations/{binding_ref}/admission/import-arm/reconcile",
            post(reconcile_named_import_arm)
                .layer(DefaultBodyLimit::max(LEGACY_NAMED_ADMISSION_REQUEST_BYTES)),
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

fn confirmed_plan_error(store: &TenantStore, error: anyhow::Error) -> (StatusCode, Json<Value>) {
    if error.downcast_ref::<ControlFailure>() == Some(&ControlFailure::NotFound) {
        workload::response(
            StatusCode::NOT_FOUND,
            "session_confirmation_not_found",
            "Confirmation de session introuvable.",
        )
    } else {
        workload::store_failure(store, error)
    }
}

async fn confirmed_plan(
    _private: PrivateControlTransport,
    broker: workload::VerifiedBroker,
    Path(binding_ref): Path<String>,
) -> ApiResult<Json<LegacyConfirmedPlanV1>> {
    let binding_ref = canonical_uuid(&binding_ref).map_err(control_error)?;
    broker
        .blocking_on(store_io::Lane::Control, move |tenant, broker, redactor| {
            let store = tenant.store.sqlite_api()?;
            let plan = store
                .legacy_confirmed_plan(&broker, binding_ref)
                .map_err(|error| confirmed_plan_error(&tenant.store, error))?;
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

async fn confirmed_launch_spec(
    _private: PrivateControlTransport,
    broker: workload::VerifiedBroker,
    Path(binding_ref): Path<String>,
) -> ApiResult<Response> {
    let binding_ref = canonical_uuid(&binding_ref).map_err(control_error)?;
    broker
        .blocking_on(store_io::Lane::Control, move |tenant, broker, redactor| {
            let store = tenant.store.sqlite_api()?;
            store
                .prepare_legacy_confirmed_launch_spec(&broker, binding_ref, |material| {
                    let value = serde_json::to_value(material)?;
                    anyhow::ensure!(
                        store.redactor_union(&redactor).value_unclipped(&value) == value,
                        "confirmed launch spec requires redaction"
                    );
                    let bytes = serde_json::to_vec(material)?;
                    anyhow::ensure!(
                        bytes.len() <= LEGACY_CONFIRMED_LAUNCH_SPEC_RESPONSE_BYTES,
                        "confirmed launch spec exceeds its response bound"
                    );
                    Ok(([(header::CONTENT_TYPE, "application/json")], bytes).into_response())
                })
                .map_err(|error| confirmed_plan_error(&tenant.store, error))
        })
        .await
}

fn named_admission_error(store: &TenantStore, error: anyhow::Error) -> (StatusCode, Json<Value>) {
    if store.store_healthy() && error.is::<ControlFailure>() {
        control_error(error)
    } else {
        workload::store_failure(store, error)
    }
}

fn named_admission_store(tenant: &TenantState, redactor: &Redactor) -> ApiResult<Arc<Store>> {
    tenant
        .store
        .sqlite_api()?
        .with_additional_redaction(redactor)
        .map_err(|error| named_admission_error(&tenant.store, error))
}

enum InitialDecisionResponse {
    Evaluate(DecisionRequest),
    Recorded(Response),
}

fn prepare_initial_decision_response(
    state: LegacyNamedInitialDecision,
    redactor: &Redactor,
) -> anyhow::Result<InitialDecisionResponse> {
    match state {
        LegacyNamedInitialDecision::Evaluate(request) => {
            Ok(InitialDecisionResponse::Evaluate(request))
        }
        LegacyNamedInitialDecision::Recorded(decision) => {
            let value = serde_json::to_value(&decision)?;
            anyhow::ensure!(
                redactor.value_unclipped(&value) == value,
                "initial named decision requires redaction"
            );
            let bytes = serde_json::to_vec(&decision)?;
            anyhow::ensure!(
                bytes.len() <= LEGACY_CONFIRMED_LAUNCH_SPEC_RESPONSE_BYTES,
                "initial named decision exceeds its response bound"
            );
            let status = if decision.replayed {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            };
            Ok(InitialDecisionResponse::Recorded(
                (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response(),
            ))
        }
    }
}

fn initial_decision_error(store: &TenantStore, error: anyhow::Error) -> (StatusCode, Json<Value>) {
    if error.downcast_ref::<ControlFailure>() == Some(&ControlFailure::NotFound) {
        confirmed_plan_error(store, error)
    } else if store.store_healthy() && error.is::<DecisionFailure>() {
        decision_error(error)
    } else {
        named_admission_error(store, error)
    }
}

async fn initial_named_decision(
    _private: PrivateControlTransport,
    broker: workload::VerifiedBroker,
    State(state): State<GatewayState>,
    Path(binding_ref): Path<String>,
    body: Bytes,
) -> ApiResult<Response> {
    let binding_ref = canonical_uuid(&binding_ref).map_err(control_error)?;
    if !body.is_empty() {
        return Err(control_error(ControlFailure::InvalidRequest.into()));
    }
    let pending = broker
        .clone()
        .blocking_on(
            store_io::Lane::Control,
            move |tenant, identity, redactor| {
                let store = named_admission_store(&tenant, &redactor)?;
                let prepared = store
                    .prepare_legacy_named_initial_decision(&identity, binding_ref, None, |result| {
                        prepare_initial_decision_response(result, &redactor)
                    })
                    .map_err(|error| initial_decision_error(&tenant.store, error))?;
                if matches!(&prepared, InitialDecisionResponse::Evaluate(_)) {
                    tenant.write_quota()?;
                }
                Ok(prepared)
            },
        )
        .await?;
    let request = match pending {
        InitialDecisionResponse::Recorded(response) => return Ok(response),
        InitialDecisionResponse::Evaluate(request) => request,
    };
    let hash = policy::request_hash(&request);
    let (decision, jev) = fresh_policy(&state, &request).await;
    broker
        .blocking_on(
            store_io::Lane::Control,
            move |tenant, identity, redactor| {
                let store = named_admission_store(&tenant, &redactor)?;
                let response = store
                    .prepare_legacy_named_initial_decision(
                        &identity,
                        binding_ref,
                        Some((hash, decision, jev)),
                        |result| prepare_initial_decision_response(result, &redactor),
                    )
                    .map_err(|error| initial_decision_error(&tenant.store, error))?;
                match response {
                    InitialDecisionResponse::Recorded(response) => Ok(response),
                    InitialDecisionResponse::Evaluate(_) => Err(control_error(anyhow::anyhow!(
                        "initial named decision has no retained receipt"
                    ))),
                }
            },
        )
        .await
}

fn named_admission_response(
    receipt: LegacyNamedAdmissionReceiptV1,
    redactor: &Redactor,
) -> ApiResult<(StatusCode, Json<LegacyNamedAdmissionReceiptV1>)> {
    let value = serde_json::to_value(&receipt).map_err(|error| control_error(error.into()))?;
    if redactor.value_unclipped(&value) != value {
        return Err(control_error(anyhow::anyhow!(
            "named admission receipt requires redaction"
        )));
    }
    Ok((admission_status(&receipt.admission), Json(receipt)))
}

async fn reserve_named(
    _private: PrivateControlTransport,
    broker: workload::VerifiedBroker,
    State(state): State<GatewayState>,
    Path(binding_ref): Path<String>,
    body: Result<Json<LegacyNamedSupervisedAdmissionV1>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<LegacyNamedAdmissionReceiptV1>)> {
    let request = Arc::new(named_admission_request(&binding_ref, body)?);
    let preflight = Arc::clone(&request);
    let replay = broker
        .clone()
        .blocking_on(
            store_io::Lane::Control,
            move |tenant, identity, redactor| {
                let store = named_admission_store(&tenant, &redactor)?;
                if let Some(receipt) = store
                    .read_legacy_named_admission(&identity, &preflight, Some(&tenant.worker_quota))
                    .map_err(|error| named_admission_error(&tenant.store, error))?
                {
                    return Ok(Some(named_admission_response(receipt, &redactor)?));
                }
                tenant.write_quota()?;
                Ok(None)
            },
        )
        .await?;
    if let Some(response) = replay {
        return Ok(response);
    }
    let (decision, jev) = fresh_policy(&state, &request.admission.admission.decision_request).await;
    broker
        .blocking_on(
            store_io::Lane::Control,
            move |tenant, identity, redactor| {
                let store = named_admission_store(&tenant, &redactor)?;
                let receipt = store
                    .admit_legacy_named(
                        &identity,
                        &request,
                        &state.policy,
                        decision,
                        jev,
                        &tenant.worker_quota,
                    )
                    .map_err(|error| named_admission_error(&tenant.store, error))?;
                named_admission_response(receipt, &redactor)
            },
        )
        .await
}

fn named_admission_request(
    binding_ref: &str,
    body: Result<Json<LegacyNamedSupervisedAdmissionV1>, JsonRejection>,
) -> ApiResult<LegacyNamedSupervisedAdmissionV1> {
    let binding_ref = canonical_uuid(binding_ref).map_err(control_error)?;
    let request = parse_json(body)?;
    if request.binding_ref != binding_ref {
        return Err(control_error(ControlFailure::InvalidRequest.into()));
    }
    Ok(request)
}

async fn reconcile_named_admission(
    _private: PrivateControlTransport,
    broker: workload::VerifiedBroker,
    Path(binding_ref): Path<String>,
    body: Result<Json<LegacyNamedSupervisedAdmissionV1>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<LegacyNamedAdmissionReceiptV1>)> {
    let request = named_admission_request(&binding_ref, body)?;
    broker
        .blocking_on(
            store_io::Lane::Control,
            move |tenant, identity, redactor| {
                let store = named_admission_store(&tenant, &redactor)?;
                // A pending result may be absent even when a concurrent
                // submission will commit. This path cannot create it.
                let receipt = store
                    .read_legacy_named_admission(&identity, &request, None)
                    .map_err(|error| named_admission_error(&tenant.store, error))?
                    .ok_or_else(|| {
                        workload::response(
                            StatusCode::NOT_FOUND,
                            LEGACY_NAMED_ADMISSION_RECEIPT_NOT_FOUND,
                            "Reçu d'admission introuvable.",
                        )
                    })?;
                named_admission_response(receipt, &redactor)
            },
        )
        .await
}

async fn cancel_named_reservation(
    _private: PrivateControlTransport,
    broker: workload::VerifiedBroker,
    Path(binding_ref): Path<String>,
    body: Result<Json<LegacyNamedCancelReservationV1>, JsonRejection>,
) -> ApiResult<Json<MutationResultV1>> {
    let binding_ref = canonical_uuid(&binding_ref).map_err(control_error)?;
    let request = parse_json(body)?;
    if request.binding_ref != binding_ref {
        return Err(control_error(ControlFailure::InvalidRequest.into()));
    }
    broker
        .blocking_on(
            store_io::Lane::Control,
            move |tenant, identity, redactor| {
                let store = named_admission_store(&tenant, &redactor)?;
                let result = store
                    .cancel_legacy_named_reservation(&identity, &request)
                    .map_err(|error| named_admission_error(&tenant.store, error))?;
                let value =
                    serde_json::to_value(&result).map_err(|error| control_error(error.into()))?;
                if redactor.value_unclipped(&value) != value {
                    return Err(control_error(anyhow::anyhow!(
                        "named reservation cancellation receipt requires redaction"
                    )));
                }
                Ok(Json(result))
            },
        )
        .await
}

fn named_import_arm_request(
    binding_ref: &str,
    body: Result<Json<LegacyNamedImportArmRequestV1>, JsonRejection>,
) -> ApiResult<LegacyNamedImportArmRequestV1> {
    let binding_ref = canonical_uuid(binding_ref).map_err(control_error)?;
    let request = parse_json(body)?;
    if request.binding_ref != binding_ref {
        return Err(control_error(ControlFailure::InvalidRequest.into()));
    }
    Ok(request)
}

fn named_import_arm_absent() -> (StatusCode, Json<Value>) {
    workload::response(
        StatusCode::NOT_FOUND,
        LEGACY_NAMED_IMPORT_ARM_RECEIPT_NOT_FOUND,
        "Reçu d’armement d’import introuvable.",
    )
}

fn named_import_arm_response(result: ArmResultV1, redactor: &Redactor) -> ApiResult<Response> {
    let status = if result.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    let value = serde_json::to_value(result).map_err(|error| control_error(error.into()))?;
    if redactor.value_unclipped(&value) != value {
        return Err(control_error(anyhow::anyhow!(
            "named import arm receipt requires redaction"
        )));
    }
    let bytes = serde_json::to_vec(&value).map_err(|error| control_error(error.into()))?;
    if bytes.len() > LEGACY_NAMED_IMPORT_ARM_RESPONSE_BYTES {
        return Err(control_error(anyhow::anyhow!(
            "named import arm receipt exceeds its response bound"
        )));
    }
    Ok((status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response())
}

async fn arm_named_import(
    _private: PrivateControlTransport,
    broker: workload::VerifiedBroker,
    Path(binding_ref): Path<String>,
    body: Result<Json<LegacyNamedImportArmRequestV1>, JsonRejection>,
) -> ApiResult<Response> {
    let request = named_import_arm_request(&binding_ref, body)?;
    broker
        .blocking_on(
            store_io::Lane::Control,
            move |tenant, identity, redactor| {
                let store = named_admission_store(&tenant, &redactor)?;
                let result = store
                    .arm_legacy_named_import(&identity, &request)
                    .map_err(|error| named_admission_error(&tenant.store, error))?;
                named_import_arm_response(result, &store.redactor_union(&redactor))
            },
        )
        .await
}

async fn reconcile_named_import_arm(
    _private: PrivateControlTransport,
    broker: workload::VerifiedBroker,
    Path(binding_ref): Path<String>,
    body: Result<Json<LegacyNamedImportArmRequestV1>, JsonRejection>,
) -> ApiResult<Response> {
    let request = named_import_arm_request(&binding_ref, body)?;
    broker
        .blocking_on(
            store_io::Lane::Control,
            move |tenant, identity, redactor| {
                let store = named_admission_store(&tenant, &redactor)?;
                let result = store
                    .read_legacy_named_import_arm(&identity, &request)
                    .map_err(|error| named_admission_error(&tenant.store, error))?
                    .ok_or_else(named_import_arm_absent)?;
                named_import_arm_response(result, &store.redactor_union(&redactor))
            },
        )
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
    Ok((admission_status(&result), Json(result)))
}

fn admission_status(result: &AdmissionResultV1) -> StatusCode {
    match result.refusal {
        Some(AdmissionRefusalV1::Policy) => StatusCode::FORBIDDEN,
        Some(AdmissionRefusalV1::Quota) => StatusCode::TOO_MANY_REQUESTS,
        None if result.replayed => StatusCode::OK,
        None => StatusCode::CREATED,
    }
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
