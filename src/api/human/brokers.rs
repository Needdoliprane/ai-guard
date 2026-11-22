//! Human-only registry route; no technical credentials or caller-selected tenant.
use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BrokerPageQuery {
    limit: Option<usize>,
    cursor: Option<String>,
}
#[derive(Serialize)]
struct BrokerQueryKey {
    limit: usize,
}
pub(super) async fn brokers(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    query: Result<Query<BrokerPageQuery>, QueryRejection>,
) -> ApiResult<Json<HumanBrokerRegistryPageV1>> {
    let Query(query) = query.map_err(|_| invalid_request())?;
    let limit = query.limit.unwrap_or(25);
    if !(1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit) {
        return Err(invalid_request());
    }
    let selected = headers.clone();
    run_human(&state, &proof, Some(&selected), move |state, proof| {
        let (context, tenant, scope) = selected_context(&state, &proof, &headers)?;
        let key = BrokerQueryKey { limit };
        let mut cursor = context
            .start_brokers(&key, query.cursor.as_deref())
            .map_err(state_failure)?;
        let page = tenant
            .store
            .sqlite_api()?
            .human_brokers(
                &scope,
                &crate::audit::HumanBrokerFrameV1::new(&context.id, &context.authorization_version)
                    .map_err(store_failure)?,
                cursor.after_broker_id.as_deref(),
                limit,
                &proof.redactor,
            )
            .map_err(store_failure)?;
        #[cfg(test)]
        state.human.pause_broker_publication()?;
        // Re-read credential, OIDC admission/mapping and membership/revision
        // after the checked transaction, before publishing its result.
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
                "human broker envelope requires redaction"
            )));
        }
        let bytes = tenant
            .store
            .sqlite_api()?
            .check_human_broker_output(&proof.redactor, &body)
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
            .check_human_broker_output(&proof.redactor, &body)
            .map_err(store_failure)?;
        Ok(Json(body))
    })
    .await
}
