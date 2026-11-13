//! E1 policy administration only. No public/legacy writer or runtime effect.
use super::auth::owned_control::{OwnedPolicyWriter, PolicyWriteAction};
use super::*;
use crate::control::egress_policy::{
    EgressPolicyAdminErrorV1 as E, EgressPolicyReceiptV1, EgressReplacePolicyRequestV1,
    EgressRevokePolicyRequestV1,
};
use axum::{
    body::Bytes,
    extract::{FromRequestParts, rejection::BytesRejection},
    http::request::Parts,
};

pub(super) fn router() -> Router<GatewayState> {
    Router::new()
        .route("/egress/policies/replace", post(replace))
        .route("/egress/policies/revoke", post(revoke))
}

pub(in crate::api) fn failure(error: E) -> (StatusCode, Json<Value>) {
    let status = error
        .http_status()
        .and_then(|value| StatusCode::from_u16(value).ok())
        .unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
    (
        status,
        Json(
            json!({"error":{"code":error.code(),"outcome":"unresolved","retry_action":"resume_same_operation"}}),
        ),
    )
}
struct Writer(Arc<OwnedPolicyWriter>);
impl FromRequestParts<GatewayState> for Writer {
    type Rejection = (StatusCode, Json<Value>);
    async fn from_request_parts(
        parts: &mut Parts,
        state: &GatewayState,
    ) -> Result<Self, Self::Rejection> {
        if state.owned_human.is_none()
            || parts
                .extensions
                .get::<super::control_transport::ReservedPolicy>()
                .is_none()
        {
            return Err(failure(E::Unavailable));
        }
        let mut content_types = parts.headers.get_all(header::CONTENT_TYPE).iter();
        if content_types.next() != Some(&HeaderValue::from_static("application/json"))
            || content_types.next().is_some()
        {
            return Err(failure(E::InvalidRequest));
        }
        parts
            .extensions
            .get::<Arc<OwnedPolicyWriter>>()
            .cloned()
            .map(Self)
            .ok_or_else(|| failure(E::Unavailable))
    }
}
fn response(receipt: EgressPolicyReceiptV1) -> ApiResult<Response> {
    let bytes = receipt
        .canonical_bytes()
        .map_err(|_| failure(E::Unavailable))?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response())
}
async fn replace(
    Writer(writer): Writer,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    if writer.action() != PolicyWriteAction::Replace {
        return Err(failure(E::Unavailable));
    }
    let body = body.map_err(|_| failure(E::InvalidRequest))?;
    let request = EgressReplacePolicyRequestV1::from_json(&body).map_err(failure)?;
    response(writer.replace(request).await?)
}
async fn revoke(
    Writer(writer): Writer,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    if writer.action() != PolicyWriteAction::Revoke {
        return Err(failure(E::Unavailable));
    }
    let body = body.map_err(|_| failure(E::InvalidRequest))?;
    let request = EgressRevokePolicyRequestV1::from_json(&body).map_err(failure)?;
    response(writer.revoke(request).await?)
}
