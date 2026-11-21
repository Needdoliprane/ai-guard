//! Owned (formats 14/15) human broker registry route,
//! `GET /api/v1/human/owned/brokers`. Proposal submitted to Root, not an
//! admitted interface.
//!
//! Its page (`profile = human_broker_registry_owned_v1`) has another shape
//! than the legacy registry page, so it never answers the legacy path: on an
//! owned gateway `/api/v1/human/brokers` stays closed by `route_gate`
//! (`503 identity_unavailable`), as before, and the existing dashboard parser
//! never receives this shape. A legacy gateway refuses this route before any
//! extractor, Store or audit work.
//!
//! The local human proof, its selected Administration context and the
//! `ReadBrokerRegistry` capability are revalidated by the Store in the read
//! transaction of the proof's own retained source, through the exact owned
//! `read` generation protocol. The mandatory read audit is appended before any
//! page, including an empty one. Credential, context and authorization are
//! re-read before publication. A page is never a tenant total
//! (`count_scope=page`); heartbeat and checkpoint freshness stay independent;
//! an unknown checkpoint state is never shown as absent. The sealed cursor
//! belongs to this owned profile only and never opens a legacy page.
use super::*;
use crate::audit::{HumanBrokerFrameV1, OwnedHumanBrokerPageV1};

/// Cursor domain of the owned projection, distinct from the legacy page; the
/// page names the same profile.
const PROFILE: &str = "human_broker_registry_owned_v1";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OwnedBrokerPageQuery {
    limit: Option<usize>,
    cursor: Option<String>,
}

/// Authenticated cursor selection: profile and page size.
#[derive(Serialize)]
struct OwnedBrokerQueryKey {
    profile: &'static str,
    limit: usize,
}

/// A legacy gateway (no owned human-source bundle) refuses before the human
/// extractor, the query parser, the Store or any audit; an owned gateway
/// then extracts the human proof and the query exactly as the legacy page
/// does.
pub(super) async fn brokers(State(state): State<GatewayState>, request: Request) -> Response {
    if state.owned_human.is_none() {
        return super::super::human_sources::unavailable().into_response();
    }
    let (mut parts, _) = request.into_parts();
    let proof = match VerifiedHuman::from_request_parts(&mut parts, &state).await {
        Ok(proof) => proof,
        Err(rejection) => return rejection.into_response(),
    };
    let query = Query::<OwnedBrokerPageQuery>::try_from_uri(&parts.uri);
    page(proof, state, parts.headers, query)
        .await
        .into_response()
}

async fn page(
    proof: VerifiedHuman,
    state: GatewayState,
    headers: HeaderMap,
    query: Result<Query<OwnedBrokerPageQuery>, QueryRejection>,
) -> ApiResult<Json<OwnedHumanBrokerPageV1>> {
    let Query(query) = query.map_err(|_| invalid_request())?;
    let limit = query.limit.unwrap_or(25);
    if !(1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit) {
        return Err(invalid_request());
    }
    let selected = headers.clone();
    run_human(&state, &proof, Some(&selected), move |state, proof| {
        let owned = state
            .owned_human
            .clone()
            .ok_or_else(super::super::human_sources::unavailable)?;
        let HumanProofSource::Local(source) = proof.source else {
            return Err(super::super::human_sources::unavailable());
        };
        let (context, tenant, scope) = selected_context(&state, &proof, &headers)?;
        let key = OwnedBrokerQueryKey {
            profile: PROFILE,
            limit,
        };
        let mut cursor = context
            .start_brokers(&key, query.cursor.as_deref())
            .map_err(state_failure)?;
        let frame = HumanBrokerFrameV1::new(&context.id, &context.authorization_version)
            .map_err(store_failure)?;
        let page = owned.human_brokers(
            source,
            tenant.store.sqlite_api()?,
            &scope,
            &frame,
            cursor.after_broker_id.as_deref(),
            limit,
            &proof.redactor,
        )?;
        // Re-read the credential, membership and authorization revision after
        // the checked transaction, before publishing its result.
        reauthenticate(&state, &proof)?;
        let (current, _, _) = selected_context(&state, &proof, &headers)?;
        if current.id != context.id
            || current.authorization_version != context.authorization_version
        {
            return Err(context_failure("context_revoked"));
        }
        cursor.after_broker_id = page.after_broker_id;
        let mut body = page.page;
        let value = serde_json::to_value(&body).map_err(|_| invalid_request())?;
        if proof.redactor.value(&value) != value {
            return Err(store_failure(anyhow::anyhow!(
                "owned human broker envelope requires redaction"
            )));
        }
        let bytes = tenant
            .store
            .sqlite_api()?
            .check_owned_human_broker_output(&proof.redactor, &body)
            .map_err(store_failure)?;
        cursor = context.advance_brokers(cursor, bytes).map_err(|error| {
            if error == StateFailure::Capacity {
                failure(
                    StatusCode::TOO_MANY_REQUESTS,
                    "capacity_exhausted",
                    "The traversal page, byte or lifetime limit has been reached.",
                    "select_context",
                )
            } else {
                state_failure(error)
            }
        })?;
        context.ensure_valid().map_err(state_failure)?;
        if body.has_more {
            body.next_cursor = Some(context.seal_brokers(&key, &cursor).map_err(state_failure)?);
        }
        tenant
            .store
            .sqlite_api()?
            .check_owned_human_broker_output(&proof.redactor, &body)
            .map_err(store_failure)?;
        Ok(Json(body))
    })
    .await
}
