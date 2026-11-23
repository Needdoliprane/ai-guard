//! Administration reads of the 360° views: quota, usage, memberships and the
//! effective configuration kinds. Each route selects one offered context,
//! lets the Store revalidate it and record the read before any result, then
//! re-reads credential, membership and authorization before publication. On
//! the owned source profile, these reads run in the proof's own retained source.
use super::*;
use crate::audit::{HumanMembershipFrameV1, HumanUsageFrameV1};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct KeysetQuery {
    limit: Option<usize>,
    cursor: Option<String>,
}
/// Authenticated cursor selection: the route and the page size. The route
/// name keeps a cursor of one traversal from opening another.
#[derive(Serialize)]
struct KeysetKey {
    route: &'static str,
    limit: usize,
}

fn publish_check(
    state: &GatewayState,
    proof: &VerifiedHuman,
    headers: &HeaderMap,
    id: &str,
    version: &str,
) -> ApiResult<()> {
    reauthenticate(state, proof)?;
    let (current, _, _) = selected_context(state, proof, headers)?;
    if current.id != id || current.authorization_version != version {
        return Err(context_failure("context_revoked"));
    }
    Ok(())
}
fn unredacted<T: Serialize>(proof: &VerifiedHuman, body: &T) -> ApiResult<()> {
    let value = serde_json::to_value(body).map_err(|_| invalid_request())?;
    if proof.redactor.value(&value) != value {
        return Err(store_failure(anyhow::anyhow!(
            "administration read requires redaction"
        )));
    }
    Ok(())
}
fn page_limit(limit: Option<usize>) -> ApiResult<usize> {
    let limit = limit.unwrap_or(50);
    if !(1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit) {
        return Err(invalid_request());
    }
    Ok(limit)
}

pub(super) async fn quota(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> ApiResult<Json<HumanQuotaV1>> {
    let selected = headers.clone();
    run_human(&state, &proof, Some(&selected), move |state, proof| {
        let (context, tenant, scope) = selected_context(&state, &proof, &headers)?;
        let frame = HumanUsageFrameV1::new(&context.id, &context.authorization_version)
            .map_err(store_failure)?;
        let store = tenant.store.sqlite_api()?;
        let body = match (&state.owned_human, &proof.source) {
            (Some(owned), HumanProofSource::Local(source)) => owned.quota(
                *source,
                store,
                &scope,
                &frame,
                &tenant.worker_quota,
                &proof.redactor,
            )?,
            (Some(_), _) => return Err(super::super::human_sources::unavailable()),
            (None, _) => store
                .human_quota(&scope, &frame, &tenant.worker_quota, &proof.redactor)
                .map_err(store_failure)?,
        };
        publish_check(
            &state,
            &proof,
            &headers,
            &context.id,
            &context.authorization_version,
        )?;
        unredacted(&proof, &body)?;
        Ok(Json(body))
    })
    .await
}

pub(super) async fn usage(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    query: Result<Query<KeysetQuery>, QueryRejection>,
) -> ApiResult<Json<HumanUsagePageV1>> {
    let Query(query) = query.map_err(|_| invalid_request())?;
    let limit = page_limit(query.limit)?;
    let selected = headers.clone();
    run_human(&state, &proof, Some(&selected), move |state, proof| {
        let (context, tenant, scope) = selected_context(&state, &proof, &headers)?;
        let key = KeysetKey {
            route: "human_usage_v1",
            limit,
        };
        let mut cursor = context
            .start_workers(&key, query.cursor.as_deref())
            .map_err(state_failure)?;
        let frame = HumanUsageFrameV1::new(&context.id, &context.authorization_version)
            .map_err(store_failure)?;
        let after = cursor.after_worker_id.as_deref();
        let store = tenant.store.sqlite_api()?;
        let page = match (&state.owned_human, &proof.source) {
            (Some(owned), HumanProofSource::Local(source)) => owned.usage(
                *source,
                store,
                &scope,
                &frame,
                after,
                limit,
                &proof.redactor,
            )?,
            (Some(_), _) => return Err(super::super::human_sources::unavailable()),
            (None, _) => store
                .human_usage(&scope, &frame, after, limit, &proof.redactor)
                .map_err(store_failure)?,
        };
        publish_check(
            &state,
            &proof,
            &headers,
            &context.id,
            &context.authorization_version,
        )?;
        cursor.after_worker_id = page.after_attempt_id;
        let mut body = page.page;
        unredacted(&proof, &body)?;
        let bytes = serde_json::to_vec(&body)
            .map_err(|_| invalid_request())?
            .len();
        cursor = context
            .advance_workers(cursor, bytes)
            .map_err(state_failure)?;
        body.next_cursor = if body.has_more {
            Some(context.seal_workers(&key, &cursor).map_err(state_failure)?)
        } else {
            None
        };
        Ok(Json(body))
    })
    .await
}

pub(super) async fn memberships(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    query: Result<Query<KeysetQuery>, QueryRejection>,
) -> ApiResult<Json<HumanMembershipPageV1>> {
    let Query(query) = query.map_err(|_| invalid_request())?;
    let limit = page_limit(query.limit)?;
    let selected = headers.clone();
    run_human(&state, &proof, Some(&selected), move |state, proof| {
        let (context, tenant, scope) = selected_context(&state, &proof, &headers)?;
        let key = KeysetKey {
            route: "human_memberships_v1",
            limit,
        };
        let mut cursor = context
            .start_workers(&key, query.cursor.as_deref())
            .map_err(state_failure)?;
        let frame = HumanMembershipFrameV1::new(&context.id, &context.authorization_version)
            .map_err(store_failure)?;
        let after = cursor.after_worker_id.clone();
        let store = tenant.store.sqlite_api()?;
        let page = match (&state.owned_human, &proof.source) {
            (Some(owned), HumanProofSource::Local(source)) => owned.memberships(
                *source,
                store,
                &scope,
                &frame,
                after.as_deref(),
                limit,
                &proof.redactor,
            )?,
            (Some(_), _) => return Err(super::super::human_sources::unavailable()),
            (None, _) => store
                .human_memberships(&scope, &frame, after.as_deref(), limit, &proof.redactor)
                .map_err(store_failure)?,
        };
        publish_check(
            &state,
            &proof,
            &headers,
            &context.id,
            &context.authorization_version,
        )?;
        cursor.after_worker_id = page.after_membership_id;
        let mut body = page.page;
        body.require_unredacted_metadata(&proof.redactor)
            .map_err(store_failure)?;
        let bytes = serde_json::to_vec(&body)
            .map_err(|_| invalid_request())?
            .len();
        cursor = context
            .advance_workers(cursor, bytes)
            .map_err(state_failure)?;
        body.next_cursor = if body.has_more {
            Some(context.seal_workers(&key, &cursor).map_err(state_failure)?)
        } else {
            None
        };
        Ok(Json(body))
    })
    .await
}

pub(super) async fn configuration(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> ApiResult<Json<HumanConfigurationV1>> {
    let selected = headers.clone();
    run_human(&state, &proof, Some(&selected), move |state, proof| {
        let (context, tenant, scope) = selected_context(&state, &proof, &headers)?;
        let store = tenant.store.sqlite_api()?;
        let facts = match (&state.owned_human, &proof.source) {
            (Some(owned), HumanProofSource::Local(source)) => {
                owned.configuration_facts(*source, store, &scope, &proof.redactor)?
            }
            (Some(_), _) => return Err(super::super::human_sources::unavailable()),
            (None, _) => store
                .human_configuration_facts(&scope, &proof.redactor)
                .map_err(store_failure)?,
        };
        publish_check(
            &state,
            &proof,
            &headers,
            &context.id,
            &context.authorization_version,
        )?;
        use ConfigurationAxisV1 as A;
        use ConfigurationProvenanceV1 as P;
        let entry = |axis, key: &str, value: &str, provenance| ConfigurationEntryV1 {
            axis,
            key: key.into(),
            value: value.into(),
            provenance,
        };
        let (engine, selection) = match &state.jev {
            Some(backend) => {
                let (engine, source) = backend.describe();
                (engine, source)
            }
            None => ("rust_policy_only", "default"),
        };
        let oidc = state.oidc.as_ref();
        let entries = vec![
            entry(
                A::Build,
                "product_version",
                env!("CARGO_PKG_VERSION"),
                P::Configured,
            ),
            entry(
                A::Topology,
                "source_profile",
                if state.owned_human.is_some() {
                    "owned_sources"
                } else {
                    "legacy"
                },
                P::Configured,
            ),
            entry(A::Identity, "local_credentials", "enabled", P::Default),
            entry(
                A::Identity,
                "oidc",
                if oidc.is_some() {
                    "enabled"
                } else {
                    "disabled"
                },
                P::Configured,
            ),
            entry(
                A::Identity,
                "oidc_transport",
                oidc.map(|gateway| gateway.transport_kind())
                    .unwrap_or("none"),
                if oidc.is_some() {
                    P::Configured
                } else {
                    P::Default
                },
            ),
            entry(A::Storage, "backend", "sqlite", P::Configured),
            entry(A::Storage, "tls_profile", "not_applicable", P::Derived),
            entry(
                A::Storage,
                "store_format",
                &facts.store_format.to_string(),
                P::Derived,
            ),
            entry(
                A::Storage,
                "human_rights_marker",
                if facts.human_rights_marker {
                    "installed"
                } else {
                    "absent"
                },
                P::Derived,
            ),
            entry(A::Decision, "engine", engine, P::Configured),
            entry(A::Decision, "selection", selection, P::Configured),
            entry(
                A::Runtime,
                "declared_runtime_kinds",
                &if facts.declared_runtime_kinds.is_empty() {
                    "none".to_owned()
                } else {
                    facts.declared_runtime_kinds.join(",")
                },
                P::Declared,
            ),
            entry(
                A::Runtime,
                "kubernetes_held_create",
                if tenant.held_create_policy.is_some() {
                    "configured"
                } else {
                    "not_configured"
                },
                P::Configured,
            ),
            entry(A::Egress, "mode", "unknown", P::Unknown),
            entry(A::Providers, "availability", "unknown", P::Unknown),
        ];
        for item in &entries {
            if !CONFIGURATION_KEYS.contains(&(item.axis, item.key.as_str())) {
                return Err(store_failure(anyhow::anyhow!(
                    "configuration key outside the closed set"
                )));
            }
        }
        let body = HumanConfigurationV1 {
            version: 1,
            context_id: context.id.clone(),
            authorization_version: context.authorization_version.clone(),
            evaluated_at: crate::broker_registry::format_time(chrono::Utc::now()),
            entries,
            read_receipt: facts.read_receipt,
        };
        unredacted(&proof, &body)?;
        Ok(Json(body))
    })
    .await
}

/// The dashboard relays to this owned-only route. Reject the legacy profile
/// before authenticating or reading any part of the submission body.
pub(super) async fn change_owned_membership(
    State(state): State<GatewayState>,
    request: Request,
) -> ApiResult<Json<HumanMembershipChangeReceiptV1>> {
    if state.owned_human.is_none() {
        return Err(super::super::human_sources::unavailable());
    }
    let (mut parts, body) = request.into_parts();
    let proof = VerifiedHuman::from_request_parts(&mut parts, &state).await?;
    let path = Path::<String>::from_request_parts(&mut parts, &state)
        .await
        .map_err(|_| invalid_request())?;
    let headers = parts.headers.clone();
    change_membership(
        proof,
        State(state),
        headers,
        path,
        Request::from_parts(parts, body),
    )
    .await
}

/// `POST /api/v1/human/admin/memberships/{membership_id}`: a delegated change
/// of one existing membership of the reader's organization. The authenticated
/// administrator is the actor; the identity mutation and its audit are durable
/// before the receipt is returned. A replay of the same operation returns the
/// same receipt; a different change under the same operation is a conflict.
/// On the owned profile the change runs under the source's write epoch.
pub(super) async fn change_membership(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    axum::extract::Path(membership_id): axum::extract::Path<String>,
    request: Request,
) -> ApiResult<Json<HumanMembershipChangeReceiptV1>> {
    let (_, change) = membership_change_body(request).await?;
    let expected_revision: u64 = change
        .expected_revision
        .parse()
        .map_err(|_| invalid_request())?;
    let selected = headers.clone();
    run_human(&state, &proof, Some(&selected), move |state, proof| {
        let (_, tenant, scope) = selected_context(&state, &proof, &headers)?;
        let store = tenant.store.sqlite_api()?;
        let receipt = match (&state.owned_human, &proof.source) {
            (Some(owned), HumanProofSource::Local(source)) => {
                owned.change_membership(*source, store, &scope, &membership_id, &change)?
            }
            (Some(_), _) => return Err(super::super::human_sources::unavailable()),
            (None, _) => store
                .human_change_membership(&scope, &membership_id, &change)
                .map_err(identity_change_failure)?,
        };
        // The change bumped the authorization revision, so the selecting
        // context is no longer current: publication checks the credential.
        reauthenticate(&state, &proof)?;
        Ok(Json(HumanMembershipChangeReceiptV1 {
            version: 1,
            operation_id: receipt.operation_id,
            membership_id,
            membership_revision: (expected_revision + 1).to_string(),
            authorization_revision: receipt.authorization_revision.to_string(),
            audit_seq: receipt.audit_seq.to_string(),
        }))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GrantQuery {
    resource_kind: ResourceKindV1,
    resource_id: String,
}

/// `GET /api/v1/human/admin/grants?resource_kind=&resource_id=`: the grant of
/// one resource of the context's organization, with its share list, for the
/// administrator who may change it. Recorded as the `grants` read.
pub(super) async fn grant(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    query: Result<Query<GrantQuery>, axum::extract::rejection::QueryRejection>,
) -> ApiResult<Json<HumanGrantV1>> {
    let Query(query) = query.map_err(|_| invalid_request())?;
    let resource = ResourceRefV1 {
        kind: query.resource_kind,
        resource_id: query.resource_id,
    };
    let selected = headers.clone();
    run_human(&state, &proof, Some(&selected), move |state, proof| {
        let (context, tenant, scope) = selected_context(&state, &proof, &headers)?;
        let frame =
            crate::audit::HumanGrantFrameV1::new(&context.id, &context.authorization_version)
                .map_err(store_failure)?;
        let store = tenant.store.sqlite_api()?;
        let body = match (&state.owned_human, &proof.source) {
            (Some(owned), HumanProofSource::Local(source)) => {
                owned.grant(*source, store, &scope, &frame, &resource, &proof.redactor)?
            }
            (Some(_), _) => return Err(super::super::human_sources::unavailable()),
            (None, _) => store
                .human_grant(&scope, &frame, &resource, &proof.redactor)
                .map_err(store_failure)?,
        };
        unredacted(&proof, &body)?;
        publish_check(
            &state,
            &proof,
            &headers,
            &context.id,
            &context.authorization_version,
        )?;
        Ok(Json(body))
    })
    .await
}

/// `POST /api/v1/human/admin/grants`: a delegated replacement of the shares
/// of one existing grant of the context's organization. Durable with its
/// audit before the receipt; exact replay returns the same receipt.
pub(super) async fn change_grant(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Json<HumanGrantChangeV1>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<Json<HumanGrantChangeReceiptV1>> {
    let Json(change) = body.map_err(|_| invalid_request())?;
    let expected_revision: u64 = change
        .expected_revision
        .parse()
        .map_err(|_| invalid_request())?;
    let selected = headers.clone();
    run_human(&state, &proof, Some(&selected), move |state, proof| {
        let (_, tenant, scope) = selected_context(&state, &proof, &headers)?;
        let store = tenant.store.sqlite_api()?;
        let receipt = match (&state.owned_human, &proof.source) {
            (Some(owned), HumanProofSource::Local(source)) => {
                owned.change_grant(*source, store, &scope, &change)?
            }
            (Some(_), _) => return Err(super::super::human_sources::unavailable()),
            (None, _) => store
                .human_change_grant(&scope, &change)
                .map_err(identity_change_failure)?,
        };
        reauthenticate(&state, &proof)?;
        Ok(Json(HumanGrantChangeReceiptV1 {
            version: 1,
            operation_id: receipt.operation_id,
            resource: change.resource,
            grant_revision: (expected_revision + 1).to_string(),
            authorization_revision: receipt.authorization_revision.to_string(),
            audit_seq: receipt.audit_seq.to_string(),
        }))
    })
    .await
}
