//! Runner-only typed relay admission. Authentication precedes every body/path.
use super::*;
use crate::relay::admission::*;

pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/relay/channels/admissions", post(reserve))
        .route("/relay/channels/{id}/claims", post(claim))
        .route("/relay/channels/{id}/transitions", post(transition))
        .route(
            "/relay/channels/{id}/reconcile",
            post(reconcile).layer(DefaultBodyLimit::max(16_384)),
        )
}
fn failure(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    if let Some(response) = audit_capacity_failure(&error) {
        return response;
    }
    if let Some(kind) = error.downcast_ref::<RelayAdmissionFailure>() {
        let (status, code) = match kind {
            RelayAdmissionFailure::InvalidRequest => {
                (StatusCode::BAD_REQUEST, "invalid_relay_channel_request")
            }
            RelayAdmissionFailure::Conflict => (StatusCode::CONFLICT, "relay_channel_conflict"),
            RelayAdmissionFailure::Forbidden => (StatusCode::FORBIDDEN, "relay_channel_forbidden"),
            RelayAdmissionFailure::NotFound => (StatusCode::NOT_FOUND, "relay_channel_not_found"),
            RelayAdmissionFailure::Capacity => {
                (StatusCode::TOO_MANY_REQUESTS, "relay_channel_capacity")
            }
        };
        return workload::response(status, code, &kind.to_string());
    }
    workload::failure(error)
}
fn parse<T>(body: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    body.map(|Json(input)| input)
        .map_err(|_| failure(RelayAdmissionFailure::InvalidRequest.into()))
}
fn path(id: &str, channel: &str) -> ApiResult<()> {
    if id == channel {
        Ok(())
    } else {
        Err(failure(RelayAdmissionFailure::InvalidRequest.into()))
    }
}
async fn reserve(
    RunnerIdentity(tenant): RunnerIdentity,
    body: Result<Json<RelayChannelAdmissionV1>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<RelayChannelResultV1>)> {
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            let input = parse(body)?;
            let result = tenant
                .store
                .reserve_relay_channel(&input)
                .map_err(failure)?;
            let status = if result.replayed {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            };
            Ok((status, Json(result)))
        })
        .await
}
async fn claim(
    RunnerIdentity(tenant): RunnerIdentity,
    Path(id): Path<String>,
    body: Result<Json<RelayChannelClaimV1>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<RelayChannelResultV1>)> {
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            let input = parse(body)?;
            path(&id, &input.channel_id)?;
            let result = tenant
                .store
                .claim_relay_channel_exec(&input)
                .map_err(failure)?;
            let status = if result.replayed {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            };
            Ok((status, Json(result)))
        })
        .await
}
async fn transition(
    RunnerIdentity(tenant): RunnerIdentity,
    Path(id): Path<String>,
    body: Result<Json<RelayChannelTransitionV1>, JsonRejection>,
) -> ApiResult<Json<RelayChannelResultV1>> {
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            let input = parse(body)?;
            path(&id, &input.channel_id)?;
            Ok(Json(
                tenant
                    .store
                    .transition_relay_channel(&input)
                    .map_err(failure)?,
            ))
        })
        .await
}

async fn reconcile(
    RunnerIdentity(tenant): RunnerIdentity,
    Path(id): Path<String>,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    body: Result<Json<RelayChannelReconcileV1>, JsonRejection>,
) -> ApiResult<Json<RelayChannelReconciliationV1>> {
    tenant
        .blocking(true, move |tenant| {
            let input = parse(body)?;
            input.validate().map_err(failure)?;
            path(&id, &input.admission.channel_id)?;
            let raw_path = uri.path().strip_prefix("/control").unwrap_or(uri.path());
            if uri.query().is_some() || raw_path != format!("/api/v1/relay/channels/{id}/reconcile")
            {
                return Err(failure(RelayAdmissionFailure::InvalidRequest.into()));
            }
            Ok(Json(
                tenant
                    .store
                    .reconcile_relay_channel(&input)
                    .map_err(failure)?,
            ))
        })
        .await
}
