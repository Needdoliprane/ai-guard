//! Exact reserved inspection endpoint. Its private proof precedes body parsing.
use super::*;
use crate::{
    api::auth::owned_control::OwnedPolicyInspection,
    control::egress_policy::{EgressPolicyInspectionRequestV1, EgressPolicyInspectionV1},
};

pub(super) fn router() -> Router<GatewayState> {
    Router::new().route("/egress/policies/inspect", post(inspect))
}

struct Inspection(Arc<OwnedPolicyInspection>);
impl FromRequestParts<GatewayState> for Inspection {
    type Rejection = (StatusCode, Json<Value>);
    async fn from_request_parts(
        parts: &mut Parts,
        state: &GatewayState,
    ) -> Result<Self, Self::Rejection> {
        if state.owned_human.is_none()
            || parts.method != axum::http::Method::POST
            || parts.uri.query().is_some()
            || !matches!(
                parts.uri.path(),
                "/api/v1/egress/policies/inspect" | "/egress/policies/inspect"
            )
            || parts
                .extensions
                .get::<super::super::control_transport::ReservedPolicyInspection>()
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
            .get::<Arc<OwnedPolicyInspection>>()
            .cloned()
            .map(Self)
            .ok_or_else(|| failure(E::Unavailable))
    }
}

fn response(observation: EgressPolicyInspectionV1) -> ApiResult<Response> {
    let bytes = observation
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

async fn inspect(
    Inspection(inspection): Inspection,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let body = body.map_err(|_| failure(E::InvalidRequest))?;
    let request = EgressPolicyInspectionRequestV1::from_json(&body).map_err(failure)?;
    response(inspection.inspect(request).await?)
}
