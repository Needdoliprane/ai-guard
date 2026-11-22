//! Human-only registry route; no technical credentials or caller-selected tenant.
use super::*;

#[cfg(test)]
#[path = "workers/federation_tests.rs"]
mod federation_tests;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkerPageQuery {
    limit: Option<usize>,
    cursor: Option<String>,
}
#[derive(Serialize)]
struct WorkerQueryKey {
    limit: usize,
}
pub(super) async fn workers(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    query: Result<Query<WorkerPageQuery>, QueryRejection>,
) -> ApiResult<Json<HumanWorkerRegistryPageV1>> {
    let Query(query) = query.map_err(|_| invalid_request())?;
    let limit = query.limit.unwrap_or(25);
    if !(1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit) {
        return Err(invalid_request());
    }
    let selected = headers.clone();
    run_human(&state, &proof, Some(&selected), move |state, proof| {
        let (context, tenant, scope) = selected_context(&state, &proof, &headers)?;
        let key = WorkerQueryKey { limit };
        let mut cursor = context
            .start_workers(&key, query.cursor.as_deref())
            .map_err(state_failure)?;
        let page = tenant
            .store
            .sqlite_api()?
            .human_workers(
                &scope,
                &crate::audit::HumanWorkerFrameV1::new(&context.id, &context.authorization_version)
                    .map_err(store_failure)?,
                cursor.after_worker_id.as_deref(),
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
        cursor.after_worker_id = page.after_worker_id;
        let mut body = page.page;
        let value = serde_json::to_value(&body).map_err(|_| invalid_request())?;
        if proof.redactor.value(&value) != value {
            return Err(store_failure(anyhow::anyhow!(
                "human worker envelope requires redaction"
            )));
        }
        let bytes = tenant
            .store
            .sqlite_api()?
            .check_human_worker_output(&proof.redactor, &body)
            .map_err(store_failure)?;
        cursor = context.advance_workers(cursor, bytes).map_err(|error| {
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
            body.next_cursor = Some(context.seal_workers(&key, &cursor).map_err(state_failure)?);
        }
        tenant
            .store
            .sqlite_api()?
            .check_human_worker_output(&proof.redactor, &body)
            .map_err(store_failure)?;
        Ok(Json(body))
    })
    .await
}

/// Both existing page limits remain independently enforced; only the fixed
/// composition envelope receives an additional 4 KiB allowance.
const MAX_HUMAN_FEDERATION_PAGE_BYTES: usize = 2 * HUMAN_MAX_PAGE_BYTES + 4096;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FederationWorkerPageQuery {
    worker_limit: Option<usize>,
    worker_cursor: Option<String>,
    broker_limit: Option<usize>,
    broker_cursor: Option<String>,
}

#[derive(Serialize)]
struct FederationQueryKey {
    route: &'static str,
    leg: &'static str,
    limit: usize,
}

/// Composition of two separately checked and audited registry snapshots. It
/// deliberately carries no tenant total or complete runtime inventory claim.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HumanFederationWorkerPageV1 {
    version: u32,
    source: String,
    consistency: String,
    coverage: String,
    runtime_inventory: String,
    workers: HumanWorkerRegistryPageV1,
    brokers: HumanBrokerRegistryPageV1,
}

fn require_federation_scope(scope: &AuthorizedScopeV1) -> ApiResult<()> {
    if scope.mode() != ContextModeV1::Administration
        || !scope.has_capability(HumanCapabilityV1::ReadWorkerRegistry)
        || !scope.has_capability(HumanCapabilityV1::ReadBrokerRegistry)
    {
        return Err(store_failure(IdentityFailureV1::PermissionDenied.into()));
    }
    Ok(())
}

fn federation_budget_failure(error: StateFailure) -> (StatusCode, Json<Value>) {
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
}

pub(super) async fn federation_workers(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    query: Result<Query<FederationWorkerPageQuery>, QueryRejection>,
) -> ApiResult<Json<HumanFederationWorkerPageV1>> {
    let Query(query) = query.map_err(|_| invalid_request())?;
    let worker_limit = query.worker_limit.unwrap_or(25);
    let broker_limit = query.broker_limit.unwrap_or(25);
    for limit in [worker_limit, broker_limit] {
        if !(1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit) {
            return Err(invalid_request());
        }
    }
    for cursor in [
        query.worker_cursor.as_deref(),
        query.broker_cursor.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if cursor.len() != HUMAN_CURSOR_HEX_BYTES
            || !cursor
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid_cursor());
        }
    }
    let selected = headers.clone();
    run_human(&state, &proof, Some(&selected), move |state, proof| {
        let (context, tenant, scope) = selected_context(&state, &proof, &headers)?;
        // Both current capabilities are necessary before either inventory read,
        // even though the existing worker-only endpoint keeps its own contract.
        require_federation_scope(&scope)?;
        let worker_key = FederationQueryKey {
            route: "human_federation_workers_v1",
            leg: "workers",
            limit: worker_limit,
        };
        let broker_key = FederationQueryKey {
            route: "human_federation_workers_v1",
            leg: "brokers",
            limit: broker_limit,
        };
        // Validate both sealed positions before either leg can append a receipt.
        let mut worker_cursor = context
            .start_workers(&worker_key, query.worker_cursor.as_deref())
            .map_err(state_failure)?;
        let mut broker_cursor = context
            .start_brokers(&broker_key, query.broker_cursor.as_deref())
            .map_err(state_failure)?;
        let workers = tenant
            .store
            .sqlite_api()?
            .human_workers(
                &scope,
                &crate::audit::HumanWorkerFrameV1::new(&context.id, &context.authorization_version)
                    .map_err(store_failure)?,
                worker_cursor.after_worker_id.as_deref(),
                worker_limit,
                &proof.redactor,
            )
            .map_err(store_failure)?;
        // A failure here does not undo the worker receipt and publishes neither
        // leg. Each receipt describes its own checked snapshot, never delivery.
        let brokers = tenant
            .store
            .sqlite_api()?
            .human_brokers(
                &scope,
                &crate::audit::HumanBrokerFrameV1::new(&context.id, &context.authorization_version)
                    .map_err(store_failure)?,
                broker_cursor.after_broker_id.as_deref(),
                broker_limit,
                &proof.redactor,
            )
            .map_err(store_failure)?;
        #[cfg(test)]
        state.human.pause_broker_publication()?;
        worker_cursor.after_worker_id = workers.after_worker_id;
        broker_cursor.after_broker_id = brokers.after_broker_id;
        let mut body = HumanFederationWorkerPageV1 {
            version: 1,
            source: "tenant_federation_registry".into(),
            consistency: "independent_snapshots".into(),
            coverage: "partial".into(),
            runtime_inventory: "unknown".into(),
            workers: workers.page,
            brokers: brokers.page,
        };
        let worker_bytes = tenant
            .store
            .sqlite_api()?
            .check_human_worker_output(&proof.redactor, &body.workers)
            .map_err(store_failure)?;
        let broker_bytes = tenant
            .store
            .sqlite_api()?
            .check_human_broker_output(&proof.redactor, &body.brokers)
            .map_err(store_failure)?;
        worker_cursor = context
            .advance_workers(worker_cursor, worker_bytes)
            .map_err(federation_budget_failure)?;
        broker_cursor = context
            .advance_brokers(broker_cursor, broker_bytes)
            .map_err(federation_budget_failure)?;
        if body.workers.has_more {
            body.workers.next_cursor = Some(
                context
                    .seal_workers(&worker_key, &worker_cursor)
                    .map_err(state_failure)?,
            );
        }
        if body.brokers.has_more {
            body.brokers.next_cursor = Some(
                context
                    .seal_brokers(&broker_key, &broker_cursor)
                    .map_err(state_failure)?,
            );
        }
        tenant
            .store
            .sqlite_api()?
            .check_human_worker_output(&proof.redactor, &body.workers)
            .map_err(store_failure)?;
        tenant
            .store
            .sqlite_api()?
            .check_human_broker_output(&proof.redactor, &body.brokers)
            .map_err(store_failure)?;
        let value = serde_json::to_value(&body).map_err(|_| invalid_request())?;
        if proof.redactor.value(&value) != value {
            return Err(store_failure(anyhow::anyhow!(
                "human federation envelope requires redaction"
            )));
        }
        if serde_json::to_vec(&body)
            .map_err(|_| invalid_request())?
            .len()
            > MAX_HUMAN_FEDERATION_PAGE_BYTES
        {
            return Err(store_failure(anyhow::anyhow!(
                "human federation envelope exceeds page byte limit"
            )));
        }
        // Revalidate the whole composition after output checks and sealing, not
        // merely each leg at its own earlier transaction boundary.
        reauthenticate(&state, &proof)?;
        let (current, _, current_scope) = selected_context(&state, &proof, &headers)?;
        require_federation_scope(&current_scope)?;
        if current.id != context.id
            || current.authorization_version != context.authorization_version
        {
            return Err(context_failure("context_revoked"));
        }
        context.ensure_valid().map_err(state_failure)?;
        Ok(Json(body))
    })
    .await
}
