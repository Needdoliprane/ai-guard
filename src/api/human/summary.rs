//! Per-context 360° summary, `GET /api/v1/human/summary?window=24h|7d|30d|all`.
//!
//! One store snapshot transaction of the selected context's tenant, recorded
//! under the read action `summary` before publication. The Store revalidates
//! the context and decides every section from its own capabilities; the route
//! re-reads credential, membership and authorization before publishing. On
//! the owned source profile the read runs in the proof's own retained source.
//! The worker section uses durable registry facts on SQLite formats 14 and 15.
use super::*;
use crate::audit::HumanSummaryFrameV1;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SummaryQuery {
    window: Option<SummaryWindowV1>,
}

pub(super) async fn summary(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    query: Result<Query<SummaryQuery>, QueryRejection>,
) -> ApiResult<Json<HumanSummaryV1>> {
    let Query(query) = query.map_err(|_| invalid_request())?;
    let window = query.window.unwrap_or(SummaryWindowV1::Day);
    let selected = headers.clone();
    run_human(&state, &proof, Some(&selected), move |state, proof| {
        let (context, tenant, scope) = selected_context(&state, &proof, &headers)?;
        let frame = HumanSummaryFrameV1::new(&context.id, &context.authorization_version)
            .map_err(store_failure)?;
        let store = tenant.store.sqlite_api()?;
        let body = match (&state.owned_human, &proof.source) {
            (Some(owned), HumanProofSource::Local(source)) => owned.summary(
                *source,
                store,
                &scope,
                &frame,
                window,
                &tenant.worker_quota,
                &proof.redactor,
            )?,
            (Some(_), _) => return Err(super::super::human_sources::unavailable()),
            (None, _) => store
                .human_summary(
                    &scope,
                    &frame,
                    window,
                    &tenant.worker_quota,
                    &proof.redactor,
                )
                .map_err(store_failure)?,
        };
        // Re-read credential, OIDC admission/mapping and membership/revision
        // after the checked transaction, before publishing its result.
        reauthenticate(&state, &proof)?;
        let (current, _, _) = selected_context(&state, &proof, &headers)?;
        if current.id != context.id
            || current.authorization_version != context.authorization_version
        {
            return Err(context_failure("context_revoked"));
        }
        let value = serde_json::to_value(&body).map_err(|_| invalid_request())?;
        if proof.redactor.value(&value) != value {
            return Err(store_failure(anyhow::anyhow!(
                "human summary requires redaction"
            )));
        }
        Ok(Json(body))
    })
    .await
}
