//! Human-only authentication, bounded capabilities and opaque pagination.
use super::human_state::{
    CONTEXT_TTL, CURSOR_HEX_BYTES, ContextBinding, ContextRegistry, CursorState,
    MAX_OWNER_CONTEXTS, RegisteredContext, StateFailure,
};
use super::*;
use crate::{
    audit::{HumanEventFilterV1, HumanEventPollV1, HumanStorePageV1},
    identity::*,
};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use serde::{Deserialize, Serialize};

const CONTEXT_HEADER: &str = "x-aiguard-context";
pub(super) struct HumanAccessState {
    contexts: ContextRegistry,
}
impl HumanAccessState {
    pub(super) fn new() -> Self {
        Self {
            contexts: ContextRegistry::new(),
        }
    }
}

pub(super) fn failure(
    status: StatusCode,
    code: &str,
    message: &str,
    retry: &str,
) -> (StatusCode, Json<Value>) {
    (
        status,
        Json(json!({"error":{"code":code,"message":message,"retry_action":retry}})),
    )
}
fn store_failure(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    if let Some(response) = super::audit_capacity_failure(&error) {
        return response;
    }
    match error.downcast_ref::<IdentityFailureV1>() {
        Some(IdentityFailureV1::MigrationRequired) => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "migration_required",
            "Explicit human access audit migration is required before reading.",
            "contact_operator",
        ),
        Some(IdentityFailureV1::CredentialExpired) => failure(
            StatusCode::UNAUTHORIZED,
            "credential_expired",
            "The local credential has expired.",
            "authenticate",
        ),
        Some(IdentityFailureV1::IdentityRevoked) => failure(
            StatusCode::UNAUTHORIZED,
            "identity_revoked",
            "The local identity or credential has been revoked.",
            "authenticate",
        ),
        Some(IdentityFailureV1::ContextRevoked) => context_failure("context_revoked"),
        Some(IdentityFailureV1::PermissionDenied) => failure(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "This context does not permit this read.",
            "review_permissions",
        ),
        Some(IdentityFailureV1::NotFound) => failure(
            StatusCode::NOT_FOUND,
            "not_found",
            "The resource is not available in this context.",
            "review_permissions",
        ),
        Some(IdentityFailureV1::InvalidRequest) => invalid_request(),
        _ => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "audit_unavailable",
            "The required identity or audit store is unavailable.",
            "contact_operator",
        ),
    }
}
fn context_failure(code: &str) -> (StatusCode, Json<Value>) {
    failure(
        StatusCode::FORBIDDEN,
        code,
        "Select a current authorized context.",
        "select_context",
    )
}
fn invalid_request() -> (StatusCode, Json<Value>) {
    failure(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "The human read request is invalid or exceeds its bounds.",
        "reduce_page",
    )
}
fn invalid_cursor() -> (StatusCode, Json<Value>) {
    failure(
        StatusCode::BAD_REQUEST,
        "invalid_cursor",
        "The cursor is invalid, expired or belongs to another selection.",
        "select_context",
    )
}
fn cursor_unavailable() -> (StatusCode, Json<Value>) {
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "identity_unavailable",
        "The cursor control is unavailable.",
        "retry",
    )
}

#[derive(Clone)]
struct VerifiedHuman {
    credential: HumanAuthenticationProof,
    source: HumanProofSource,
    digest: [u8; 32],
    redactor: Redactor,
}
#[derive(Clone)]
struct HumanAuthenticationProof {
    principal: PrincipalRefV1,
    credential_id: String,
    revision: u64,
    expires_at: Option<String>,
}
#[derive(Clone)]
enum HumanProofSource {
    Local(usize),
    Oidc(super::oidc::OidcHumanProof),
}
impl VerifiedHuman {
    fn method(&self) -> &'static str {
        match self.source {
            HumanProofSource::Local(_) => "local_credential",
            HumanProofSource::Oidc(_) => "oidc_access_token",
        }
    }
    fn ceiling(&self) -> Option<&std::collections::BTreeSet<HumanCapabilityV1>> {
        match &self.source {
            HumanProofSource::Local(_) => None,
            HumanProofSource::Oidc(proof) => Some(&proof.ceiling),
        }
    }
    fn mapping_revision(&self) -> Option<String> {
        match &self.source {
            HumanProofSource::Local(_) => None,
            HumanProofSource::Oidc(proof) => Some(proof.mapping_revision.clone()),
        }
    }
}
impl FromRequestParts<GatewayState> for VerifiedHuman {
    type Rejection = (StatusCode, Json<Value>);
    async fn from_request_parts(parts: &mut Parts, state: &GatewayState) -> ApiResult<Self> {
        let headers = parts.headers.clone();
        state
            .blocking_identity(move |state| authenticate(&state, &headers))
            .await
    }
}
fn authenticate(state: &GatewayState, headers: &HeaderMap) -> ApiResult<VerifiedHuman> {
    if !state.valid_credentials {
        return Err(failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "identity_unavailable",
            "Identity configuration is unavailable.",
            "contact_operator",
        ));
    }
    let token = super::oidc::human_bearer(headers).ok_or_else(|| {
        failure(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "A human bearer proof is required.",
            "authenticate",
        )
    })?;
    if !crate::identity::local_credential_format(token) {
        let oidc = state.oidc.as_ref().ok_or_else(|| {
            failure(
                StatusCode::UNAUTHORIZED,
                "credential_invalid",
                "No admitted human proof matches this bearer.",
                "authenticate",
            )
        })?;
        if state.credential_index.lookup(token).is_some() {
            return Err(failure(
                StatusCode::UNAUTHORIZED,
                "credential_invalid",
                "A control credential is not a human proof.",
                "authenticate",
            ));
        }
        reject_control_collision(state, token, None)?;
        let proof = oidc.authenticate(token)?;
        let expiry = chrono::DateTime::from_timestamp(proof.expires_at, 0)
            .ok_or_else(invalid_request)?
            .to_rfc3339();
        return Ok(VerifiedHuman {
            credential: HumanAuthenticationProof {
                principal: proof.principal.clone(),
                credential_id: proof.authentication_id.clone(),
                revision: u64::from(proof.generation),
                expires_at: Some(expiry),
            },
            digest: proof.digest,
            source: HumanProofSource::Oidc(proof),
            redactor: state.redactor.union(&Redactor::new(vec![token.to_owned()])),
        });
    }
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let mut matches = Vec::new();
    let mut unavailable = None;
    for (index, tenant) in state.tenants.iter().enumerate() {
        match credential_binding(state, tenant, &digest, store_io::Lane::Discovery) {
            Ok(Some(_)) => matches.push(index),
            Ok(None) => {}
            Err(error) => {
                // The source is not known until a readable binding matches.
                // Keep an error for an unresolved proof, without allowing an
                // unrelated unavailable store to deny a verified source.
                if unavailable.is_none() {
                    unavailable = Some(error);
                }
            }
        }
    }
    let control_credential = state.credential_index.lookup(token).is_some();
    if !control_credential
        && matches.is_empty()
        && let Some(error) = unavailable
    {
        return Err(error.into_api());
    }
    if control_credential || matches.is_empty() {
        return Err(failure(
            StatusCode::UNAUTHORIZED,
            "credential_invalid",
            "The credential is not a local human credential.",
            "authenticate",
        ));
    }
    if matches.len() != 1 {
        return Err(failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "identity_unavailable",
            "A credential binding is ambiguous.",
            "contact_operator",
        ));
    }
    let source = matches[0];
    let credential = state
        .io_scope
        .with_store(
            &state.tenants[source],
            store_io::Lane::Read,
            true,
            |store| store.authenticate_local_digest(&digest),
        )?
        .map_err(store_failure)?
        .ok_or_else(|| {
            failure(
                StatusCode::UNAUTHORIZED,
                "credential_invalid",
                "The credential is invalid.",
                "authenticate",
            )
        })?;
    Ok(VerifiedHuman {
        credential: HumanAuthenticationProof {
            principal: credential.principal,
            credential_id: credential.credential_id,
            revision: credential.revision,
            expires_at: credential.expires_at,
        },
        source: HumanProofSource::Local(source),
        digest,
        redactor: state.redactor.union(&Redactor::new(vec![token.to_owned()])),
    })
}
// Unavailable metadata is not an absent binding. A neighboring failure may be
// tolerated only with the separate, freshly verified source proof (policy B).
enum MetadataUnavailable {
    Admission((StatusCode, Json<Value>)),
    Storage(anyhow::Error),
}
impl MetadataUnavailable {
    fn into_api(self) -> (StatusCode, Json<Value>) {
        match self {
            Self::Admission(error) => error,
            Self::Storage(error) => store_failure(error),
        }
    }
}
fn credential_binding(
    state: &GatewayState,
    tenant: &TenantState,
    digest: &[u8; 32],
    lane: store_io::Lane,
) -> Result<Option<LocalCredentialV1>, MetadataUnavailable> {
    state
        .io_scope
        .with_store(tenant, lane, lane == store_io::Lane::Read, |store| {
            store.local_credential_binding(digest)
        })
        .map_err(MetadataUnavailable::Admission)?
        .map_err(MetadataUnavailable::Storage)
}
pub(super) fn reject_control_collision(
    state: &GatewayState,
    token: &str,
    own: Option<(usize, store_io::Lane)>,
) -> ApiResult<()> {
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    for (index, tenant) in state.tenants.iter().enumerate() {
        let lane = own
            .filter(|(source, _)| *source == index)
            .map_or(store_io::Lane::Discovery, |(_, lane)| lane);
        match credential_binding(state, tenant, &digest, lane) {
            Ok(Some(_)) => {
                return Err(error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Classes d'identité ambiguës : accès refusé.",
                ));
            }
            Ok(None) => {}
            Err(unavailable) if own.is_some_and(|(source, _)| source == index) => {
                return Err(match unavailable {
                    MetadataUnavailable::Admission(error) => error,
                    MetadataUnavailable::Storage(_) => super::error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Configuration d'identité indisponible.",
                    ),
                });
            }
            // An unavailable sibling cannot authenticate its local bindings.
            // Readable collisions are still checked on every request, including
            // bindings provisioned after the gateway was constructed.
            Err(_) => {}
        }
    }
    Ok(())
}
fn reauthenticate(state: &GatewayState, proof: &VerifiedHuman) -> ApiResult<()> {
    let source = match &proof.source {
        HumanProofSource::Oidc(oidc_proof) => {
            return state
                .oidc
                .as_ref()
                .ok_or_else(|| context_failure("context_revoked"))?
                .reauthenticate(oidc_proof);
        }
        HumanProofSource::Local(source) => *source,
    };
    let mut count = 0usize;
    for (index, tenant) in state.tenants.iter().enumerate() {
        let lane = if index == source {
            store_io::Lane::Read
        } else {
            store_io::Lane::Discovery
        };
        match credential_binding(state, tenant, &proof.digest, lane) {
            Ok(binding) => count += usize::from(binding.is_some()),
            Err(error) if index == source => return Err(error.into_api()),
            Err(_) => {}
        }
    }
    if count != 1 {
        return Err(failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "identity_unavailable",
            "A credential binding changed or is ambiguous.",
            "contact_operator",
        ));
    }
    let verified = state
        .io_scope
        .with_store(
            &state.tenants[source],
            store_io::Lane::Read,
            true,
            |store| store.authenticate_local_digest(&proof.digest),
        )?
        .map_err(store_failure)?
        .ok_or_else(|| {
            failure(
                StatusCode::UNAUTHORIZED,
                "credential_invalid",
                "The credential is no longer available.",
                "authenticate",
            )
        })?;
    if verified.principal != proof.credential.principal
        || verified.credential_id != proof.credential.credential_id
    {
        return Err(failure(
            StatusCode::UNAUTHORIZED,
            "credential_invalid",
            "The credential identity changed.",
            "authenticate",
        ));
    }
    if verified.revision != proof.credential.revision {
        return Err(context_failure("context_revoked"));
    }
    Ok(())
}

fn state_failure(error: StateFailure) -> (StatusCode, Json<Value>) {
    match error {
        StateFailure::InvalidCursor => invalid_cursor(),
        StateFailure::InvalidContext => context_failure("context_invalid"),
        StateFailure::Expired => context_failure("context_expired"),
        StateFailure::Revoked => context_failure("context_revoked"),
        StateFailure::Unavailable => cursor_unavailable(),
        StateFailure::Capacity => failure(
            StatusCode::TOO_MANY_REQUESTS,
            "capacity_exhausted",
            "Live context capacity is full; wait for expiry or reduce authorized contexts.",
            "retry",
        ),
    }
}
fn registered_context(
    state: &GatewayState,
    proof: &VerifiedHuman,
    headers: &HeaderMap,
) -> ApiResult<(Arc<RegisteredContext>, TenantState)> {
    let mut values = headers.get_all(CONTEXT_HEADER).iter();
    let value = values
        .next()
        .ok_or_else(|| context_failure("context_required"))?;
    if values.next().is_some() {
        return Err(context_failure("context_invalid"));
    }
    let id = value
        .to_str()
        .map_err(|_| context_failure("context_invalid"))?;
    let principal = proof.credential.principal.key().map_err(store_failure)?;
    let context = state
        .human
        .contexts
        .lookup(
            id,
            &proof.digest,
            &principal,
            &proof.credential.credential_id,
            proof.credential.revision,
        )
        .map_err(state_failure)?;
    let binding = &context.binding;
    let tenant = state
        .tenants
        .iter()
        .find(|tenant| tenant.tenant_id == binding.tenant_id)
        .ok_or_else(|| context_failure("context_revoked"))?
        .clone();
    context.ensure_valid().map_err(state_failure)?;
    Ok((context, tenant))
}
fn selected_context(
    state: &GatewayState,
    proof: &VerifiedHuman,
    headers: &HeaderMap,
) -> ApiResult<(Arc<RegisteredContext>, TenantState, AuthorizedScopeV1)> {
    let (context, tenant) = registered_context(state, proof, headers)?;
    let binding = &context.binding;
    let mut scope = tenant
        .store
        .authorize_human_context(
            &proof.credential.principal,
            &binding.membership_id,
            binding.membership_revision,
            binding.mode,
        )
        .map_err(store_failure)?;
    if let Some(ceiling) = proof.ceiling() {
        scope = scope
            .restrict_capabilities(ceiling.clone())
            .map_err(|error| store_failure(error.into()))?;
    }
    if binding.authentication_method != proof.method()
        || binding.mapping_revision != proof.mapping_revision()
    {
        return Err(context_failure("context_revoked"));
    }
    if scope.authorization_revision() != binding.authorization_revision {
        return Err(context_failure("context_revoked"));
    }
    if scope.organization_scope() != binding.organization_scope {
        return Err(context_failure("context_revoked"));
    }
    context.ensure_valid().map_err(state_failure)?;
    Ok((context, tenant, scope))
}

async fn run_human<T, F>(
    state: &GatewayState,
    proof: &VerifiedHuman,
    headers: Option<&HeaderMap>,
    operation: F,
) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(GatewayState, VerifiedHuman) -> ApiResult<T> + Send + 'static,
{
    // Header/context lookup is memory-only. It cannot select an arbitrary
    // tenant: the registry binds this selection to the authenticated proof.
    let mut scope = store_io::Scope::default();
    if let HumanProofSource::Local(source) = proof.source {
        scope.hold(&state.tenants[source], store_io::Lane::Read, true)?;
    }
    if let Some(headers) = headers {
        scope.hold(
            &registered_context(state, proof, headers)?.1,
            store_io::Lane::Read,
            true,
        )?;
    }
    let state = state.scoped(Arc::new(scope));
    let proof = proof.clone();
    store_io::run(
        Arc::clone(&state.io),
        None,
        store_io::Lane::Read,
        true,
        move || {
            reauthenticate(&state, &proof)?;
            operation(state, proof)
        },
    )
    .await
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PageQuery {
    cursor: Option<String>,
    limit: Option<usize>,
    view: Option<HumanReadViewV1>,
    resource_kind: Option<ResourceKindV1>,
    resource_id: Option<String>,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Traversal {
    Resources,
    Events,
    Export,
    Stream,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
struct QueryKey {
    traversal: Traversal,
    limit: usize,
    view: HumanReadViewV1,
    resource: Option<ResourceRefV1>,
}
impl PageQuery {
    fn key(&self, traversal: Traversal) -> ApiResult<QueryKey> {
        let limit = self.limit.unwrap_or(25);
        if !(1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit) {
            return Err(invalid_request());
        }
        let resource = match (&self.resource_kind, &self.resource_id) {
            (None, None) => None,
            (Some(kind), Some(id)) => {
                let resource = ResourceRefV1 {
                    kind: *kind,
                    resource_id: id.clone(),
                };
                resource.validate().map_err(|_| invalid_request())?;
                Some(resource)
            }
            _ => return Err(invalid_request()),
        };
        if traversal == Traversal::Resources && (resource.is_some() || self.view.is_some()) {
            return Err(invalid_request());
        }
        Ok(QueryKey {
            traversal,
            limit,
            view: self.view.unwrap_or(HumanReadViewV1::Metadata),
            resource,
        })
    }
}
fn advance_budget(
    context: &RegisteredContext,
    state: CursorState,
    bytes: usize,
) -> ApiResult<CursorState> {
    context.advance(state, bytes).map_err(|error| {
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
    })
}
fn envelope<T>(
    context: &RegisteredContext,
    page: HumanStorePageV1<T>,
    view: Option<HumanReadViewV1>,
    export: bool,
) -> HumanPageV1<T> {
    HumanPageV1 {
        version: 1,
        context_id: context.id.clone(),
        authorization_version: context.authorization_version.clone(),
        items: page.items,
        next_cursor: None,
        has_more: page.has_more,
        coverage: "authorized_selection".into(),
        read_receipt: page.receipt,
        view,
        selection_complete: export.then_some(!page.has_more),
    }
}
fn finish_page<T: Serialize>(
    context: &RegisteredContext,
    query: &QueryKey,
    cursor: CursorState,
    mut page: HumanPageV1<T>,
) -> ApiResult<Json<HumanPageV1<T>>> {
    // The placeholder has exactly the fixed hex-token size and is never
    // published. Count the complete envelope before authenticating its budget.
    if page.has_more {
        page.next_cursor = Some("0".repeat(CURSOR_HEX_BYTES));
    }
    let size = serde_json::to_vec(&page)
        .map_err(|_| invalid_request())?
        .len();
    if size > HUMAN_MAX_PAGE_BYTES {
        return Err(invalid_request());
    }
    let cursor = advance_budget(context, cursor, size)?;
    context.ensure_valid().map_err(state_failure)?;
    if page.has_more {
        page.next_cursor = Some(context.seal(query, &cursor).map_err(state_failure)?);
    }
    if serde_json::to_vec(&page)
        .map_err(|_| invalid_request())?
        .len()
        > HUMAN_MAX_PAGE_BYTES
    {
        return Err(invalid_request());
    }
    Ok(Json(page))
}

pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/human/identity", get(identity))
        .route("/human/resources", get(resources))
        .route("/human/events", get(events))
        .route("/human/events/{id}", get(event))
        .route("/human/export", get(export))
        .route("/human/stream", get(stream))
}
async fn identity(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
) -> ApiResult<Json<HumanIdentityV1>> {
    run_human(&state, &proof, None, |state, proof| {
        identity_sync(proof, State(state))
    })
    .await
}
fn identity_sync(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
) -> ApiResult<Json<HumanIdentityV1>> {
    let mut contexts = Vec::new();
    let mut contexts_complete = true;
    let mut display_name = None;
    let principal_key = proof.credential.principal.key().map_err(store_failure)?;
    let mut remaining = proof
        .credential
        .expires_at
        .as_ref()
        .map(|expiry| {
            chrono::DateTime::parse_from_rfc3339(expiry).map(|expiry| {
                expiry
                    .with_timezone(&chrono::Utc)
                    .signed_duration_since(chrono::Utc::now())
                    .to_std()
                    .unwrap_or(Duration::ZERO)
            })
        })
        .transpose()
        .map_err(|_| invalid_request())?
        .unwrap_or(CONTEXT_TTL)
        .min(CONTEXT_TTL);
    if let HumanProofSource::Oidc(oidc) = &proof.source {
        remaining = remaining.min(oidc.deadline.saturating_duration_since(Instant::now()));
    }
    for (index, tenant) in state.tenants.iter().enumerate() {
        let snapshot = match state.io_scope.with_store(
            tenant,
            store_io::Lane::Read,
            true,
            |store| store.human_profile(&proof.credential.principal, &proof.redactor),
        ) {
            Ok(snapshot) => snapshot,
            Err(error) if matches!(proof.source, HumanProofSource::Local(source) if source == index) =>
            {
                return Err(error);
            }
            Err(_) => {
                contexts_complete = false;
                continue;
            }
        };
        let snapshot = match snapshot {
            Ok(snapshot) => snapshot,
            Err(error) if matches!(&proof.source, HumanProofSource::Local(source) if index == *source) =>
            {
                return Err(store_failure(error));
            }
            Err(_) => {
                // Do not reconcile an unreadable profile as an empty one: its
                // existing contexts remain unknown and are revalidated against
                // their own store before any use.
                contexts_complete = false;
                continue;
            }
        };
        state
            .human
            .contexts
            .reconcile(
                &proof.digest,
                &principal_key,
                proof.credential.revision,
                &tenant.tenant_id,
                snapshot.authorization_revision,
                &snapshot.memberships,
            )
            .map_err(state_failure)?;
        let Some(account) = snapshot.account else {
            continue;
        };
        if matches!(proof.source, HumanProofSource::Local(source) if index == source)
            || matches!(proof.source, HumanProofSource::Oidc(_)) && display_name.is_none()
        {
            display_name = Some(account.display_name);
        }
        let revision = snapshot
            .authorization_revision
            .ok_or_else(|| context_failure("context_revoked"))?;
        for mut member in snapshot.memberships {
            if let Some(ceiling) = proof.ceiling() {
                member.capabilities = member.capabilities.intersection(ceiling).copied().collect();
            }
            if member.capabilities.is_empty() {
                continue;
            }
            let mut modes = vec![ContextModeV1::Personal];
            if member
                .capabilities
                .iter()
                .any(|cap| cap.is_administration())
            {
                modes.push(ContextModeV1::Administration);
            }
            for mode in modes {
                if contexts.len() >= MAX_OWNER_CONTEXTS {
                    return Err(failure(
                        StatusCode::TOO_MANY_REQUESTS,
                        "capacity_exhausted",
                        "The authorized context list exceeds its bound.",
                        "contact_operator",
                    ));
                }
                let binding = ContextBinding {
                    digest: proof.digest,
                    principal_key: principal_key.clone(),
                    credential_id: proof.credential.credential_id.clone(),
                    credential_revision: proof.credential.revision,
                    authentication_method: proof.method().into(),
                    mapping_revision: proof.mapping_revision(),
                    tenant_id: tenant.tenant_id.clone(),
                    organization_scope: member.organization_scope.clone(),
                    membership_id: member.membership_id.clone(),
                    mode,
                    membership_revision: member.revision,
                    authorization_revision: revision,
                };
                let expires = chrono::Utc::now()
                    + chrono::Duration::from_std(remaining).map_err(|_| invalid_request())?;
                let context = state
                    .human
                    .contexts
                    .obtain(binding, remaining, expires.to_rfc3339())
                    .map_err(state_failure)?;
                let capabilities = member
                    .capabilities
                    .iter()
                    .copied()
                    .filter(|cap| mode == ContextModeV1::Administration || !cap.is_administration())
                    .collect();
                contexts.push(HumanContextV1 {
                    id: context.id.clone(),
                    tenant_id: tenant.tenant_id.clone(),
                    organization_scope: member.organization_scope.clone(),
                    mode,
                    capabilities,
                    membership_revision: member.revision.to_string(),
                    authorization_version: context.authorization_version.clone(),
                    expires_at: context.expires_at.clone(),
                });
            }
        }
    }
    reauthenticate(&state, &proof)?;
    let identity = HumanIdentityV1 {
        version: 1,
        principal: proof.credential.principal.clone(),
        display_name: display_name.ok_or_else(|| {
            if !contexts_complete {
                return failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "audit_unavailable",
                    "The required identity or audit store is unavailable.",
                    "contact_operator",
                );
            }
            failure(
                StatusCode::UNAUTHORIZED,
                "identity_revoked",
                "The authenticated account is unavailable.",
                "authenticate",
            )
        })?,
        authentication: HumanAuthenticationV1 {
            method: proof.method().into(),
            credential_id: proof.credential.credential_id.clone(),
            expires_at: proof.credential.expires_at.clone(),
        },
        contexts_complete,
        contexts,
    };
    let value = proof
        .redactor
        .value(&serde_json::to_value(identity).map_err(|_| invalid_request())?);
    if serde_json::to_vec(&value)
        .map_err(|_| invalid_request())?
        .len()
        > HUMAN_MAX_PAGE_BYTES
    {
        return Err(failure(
            StatusCode::TOO_MANY_REQUESTS,
            "capacity_exhausted",
            "The authorized identity projection exceeds its byte bound.",
            "contact_operator",
        ));
    }
    Ok(Json(
        serde_json::from_value(value).map_err(|_| invalid_request())?,
    ))
}
async fn resources(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> ApiResult<Json<HumanPageV1<HumanResourceItemV1>>> {
    let Query(query) = query.map_err(|_| invalid_request())?;
    let selected = headers.clone();
    run_human(&state, &proof, Some(&selected), move |state, proof| {
        resources_sync(proof, State(state), headers, Ok(Query(query)))
    })
    .await
}
fn resources_sync(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> ApiResult<Json<HumanPageV1<HumanResourceItemV1>>> {
    let Query(query) = query.map_err(|_| invalid_request())?;
    let (context, tenant, scope) = selected_context(&state, &proof, &headers)?;
    let key = query.key(Traversal::Resources)?;
    let mut cursor = context
        .start(&key, query.cursor.as_deref())
        .map_err(state_failure)?;
    let page = tenant
        .store
        .human_resources(&scope, &cursor.after_resource, key.limit, &proof.redactor)
        .map_err(store_failure)?;
    reauthenticate(&state, &proof)?;
    cursor.after_resource = page.after_resource.clone();
    let page = envelope(&context, page, None, false);
    finish_page(&context, &key, cursor, page)
}
async fn events(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> ApiResult<Json<HumanPageV1<HumanEventItemV1>>> {
    event_page(proof, state, headers, query, None, false).await
}
async fn export(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> ApiResult<Json<HumanPageV1<HumanEventItemV1>>> {
    event_page(proof, state, headers, query, None, true).await
}
async fn event(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> ApiResult<Json<HumanPageV1<HumanEventItemV1>>> {
    if uuid::Uuid::parse_str(&id).is_err() {
        return Err(failure(
            StatusCode::NOT_FOUND,
            "not_found",
            "The event is not available in this context.",
            "review_permissions",
        ));
    }
    event_page(proof, state, headers, query, Some(id), false).await
}
async fn event_page(
    proof: VerifiedHuman,
    state: GatewayState,
    headers: HeaderMap,
    query: Result<Query<PageQuery>, QueryRejection>,
    id: Option<String>,
    is_export: bool,
) -> ApiResult<Json<HumanPageV1<HumanEventItemV1>>> {
    let Query(query) = query.map_err(|_| invalid_request())?;
    let selected = headers.clone();
    run_human(&state, &proof, Some(&selected), move |state, proof| {
        event_page_sync(proof, state, headers, Ok(Query(query)), id, is_export)
    })
    .await
}
fn event_page_sync(
    proof: VerifiedHuman,
    state: GatewayState,
    headers: HeaderMap,
    query: Result<Query<PageQuery>, QueryRejection>,
    id: Option<String>,
    is_export: bool,
) -> ApiResult<Json<HumanPageV1<HumanEventItemV1>>> {
    let Query(mut query) = query.map_err(|_| invalid_request())?;
    let (context, tenant, scope) = selected_context(&state, &proof, &headers)?;
    if id.is_some() {
        if query.cursor.is_some() || query.resource_id.is_some() || query.resource_kind.is_some() {
            return Err(invalid_request());
        }
        query.limit = Some(1);
    }
    let key = query.key(if is_export {
        Traversal::Export
    } else {
        Traversal::Events
    })?;
    let mut cursor = context
        .start(&key, query.cursor.as_deref())
        .map_err(state_failure)?;
    let filter = HumanEventFilterV1 {
        view: key.view,
        resource: key.resource.clone(),
        event_id: id.clone(),
        export: is_export,
        stream: false,
    };
    let poll: HumanEventPollV1 = tenant
        .store
        .human_events(
            &scope,
            &filter,
            cursor.after_event,
            cursor.upper_event,
            key.limit,
            &proof.redactor,
        )
        .map_err(store_failure)?;
    let page = poll.page.ok_or_else(invalid_request)?;
    if id.is_some() && page.items.is_empty() {
        return Err(failure(
            StatusCode::NOT_FOUND,
            "not_found",
            "The event is not available in this context.",
            "review_permissions",
        ));
    }
    reauthenticate(&state, &proof)?;
    cursor.after_event = page.after_event;
    cursor.upper_event = Some(page.upper_event);
    let page = envelope(&context, page, Some(key.view), is_export);
    finish_page(&context, &key, cursor, page)
}
async fn stream(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> ApiResult<Response> {
    if headers.contains_key("last-event-id") {
        return Err(invalid_cursor());
    }
    let Query(query) = query.map_err(|_| invalid_request())?;
    let selected = headers.clone();
    let (context, tenant, scope) =
        run_human(&state, &proof, Some(&headers), move |state, proof| {
            selected_context(&state, &proof, &selected)
        })
        .await?;
    let key = query.key(Traversal::Stream)?;
    let mut cursor = context
        .start(&key, query.cursor.as_deref())
        .map_err(state_failure)?;
    let lease = tenant
        .human_streams
        .try_acquire(Arc::clone(&tenant.streams), &proof.credential.principal)
        .map_err(|error| match error {
            human_streams::AdmissionFailure::Capacity => failure(
                StatusCode::TOO_MANY_REQUESTS,
                "capacity_exhausted",
                "Concurrent stream capacity is full. Close unused streams before reconnecting.",
                "close_unused_streams",
            ),
            human_streams::AdmissionFailure::Unavailable => failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "stream_admission_unavailable",
                "Stream admission is unavailable.",
                "contact_operator",
            ),
        })?;
    let events = async_stream::stream! {
        loop {
            let poll_context = Arc::clone(&context);
            let poll_scope = scope.clone();
            let poll_key = key.clone();
            let poll_cursor = cursor.clone();
            let poll_headers = headers.clone();
            let result = run_human(&state,&proof,Some(&headers),move |state,proof|
                stream_poll(&state,&proof,&poll_headers,&poll_context,&poll_scope,&poll_key,poll_cursor)).await;
            let mut dense = false;
            match result {
                Ok((Some((event,has_more)),next)) => {cursor = next;dense = has_more;yield Ok::<_,Infallible>(event);},
                Ok((None,next)) => {cursor = next;},
                Err((_,Json(error))) => {yield Ok(SseEvent::default().event("error").json_data(error).expect("bounded serializable human error"));break;},
            }
            if dense {tokio::task::yield_now().await;} else {tokio::time::sleep(Duration::from_secs(1)).await;}
        }
    };
    Ok(Sse::new(lease.bind(events))
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(10))
                .text("heartbeat"),
        )
        .into_response())
}

fn stream_poll(
    state: &GatewayState,
    proof: &VerifiedHuman,
    headers: &HeaderMap,
    context: &Arc<RegisteredContext>,
    scope: &AuthorizedScopeV1,
    key: &QueryKey,
    mut cursor: CursorState,
) -> ApiResult<(Option<(SseEvent, bool)>, CursorState)> {
    let (current_context, tenant, current) = selected_context(state, proof, headers)?;
    if !Arc::ptr_eq(context, &current_context)
        || current.authorization_revision() != scope.authorization_revision()
    {
        return Err(context_failure("context_revoked"));
    }
    let filter = HumanEventFilterV1 {
        view: key.view,
        resource: key.resource.clone(),
        event_id: None,
        export: false,
        stream: true,
    };
    let poll = tenant
        .store
        .human_events(
            scope,
            &filter,
            cursor.after_event,
            cursor.upper_event,
            key.limit,
            &proof.redactor,
        )
        .map_err(store_failure)?;
    reauthenticate(state, proof)?;
    let Some(page) = poll.page else {
        cursor.after_event = poll.after_event;
        cursor.upper_event = None;
        return Ok((None, cursor));
    };
    cursor.after_event = page.after_event;
    cursor.upper_event = if page.has_more {
        Some(page.upper_event)
    } else {
        None
    };
    let has_more = page.has_more;
    let page = envelope(context, page, Some(key.view), false);
    let frame_bytes = serde_json::to_vec(&page)
        .map_err(|_| invalid_request())?
        .len()
        .checked_add(CURSOR_HEX_BYTES + 64)
        .ok_or_else(invalid_request)?;
    if frame_bytes > HUMAN_MAX_PAGE_BYTES {
        return Err(invalid_request());
    }
    cursor = advance_budget(context, cursor, frame_bytes)?;
    let resume = context.seal(key, &cursor).map_err(state_failure)?;
    let event = SseEvent::default()
        .event("human_audit")
        .id(resume)
        .json_data(page)
        .map_err(|_| invalid_request())?;
    Ok((Some((event, has_more)), cursor))
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use crate::api::read_execution_tests::{human_get, local_identity_state};
    use crate::audit::read_test_observer::{Call, observe};
    use axum::http::Request;

    async fn resources_from_verified_proof(
        state: &GatewayState,
        proof: &VerifiedHuman,
        context: &str,
    ) -> ApiResult<Json<HumanPageV1<HumanResourceItemV1>>> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTEXT_HEADER, HeaderValue::from_str(context).unwrap());
        let selected = headers.clone();
        run_human(state, proof, Some(&selected), move |state, proof| {
            resources_sync(
                proof,
                State(state),
                headers,
                Ok(Query(PageQuery {
                    cursor: None,
                    limit: Some(1),
                    view: None,
                    resource_kind: None,
                    resource_id: None,
                })),
            )
        })
        .await
    }

    #[tokio::test]
    async fn context_a_revalidation_cannot_open_saturated_credential_source_b() {
        let (_directory, state, token) = local_identity_state();
        let router = gateway_router(state.clone());
        let (status, identity) = human_get(&router, &token, "/api/v1/human/identity", None).await;
        assert_eq!(status, StatusCode::OK);
        let contexts = identity["contexts"].as_array().unwrap();
        let context_id = |tenant| {
            contexts
                .iter()
                .find(|context| context["tenant_id"] == tenant)
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let context_a = context_id("tenant-a");
        let context_b = context_id("tenant-b");
        let (mut parts, _) = Request::builder()
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
            .into_parts();
        // Use the real extractor before saturation. The retained proof lets
        // this test isolate revalidation from the initial authentication I/O.
        let proof = VerifiedHuman::from_request_parts(&mut parts, &state)
            .await
            .unwrap();
        let observations = observe(&state.tenants[1].store);
        let _three_source_slots: Vec<_> = (0..3)
            .map(|_| {
                state.tenants[1]
                    .io
                    .admit(store_io::Lane::Read, false)
                    .unwrap()
            })
            .collect();
        // Source == context needs one remaining slot, not a double acquisition.
        let _ = resources_from_verified_proof(&state, &proof, &context_b)
            .await
            .unwrap();
        let baseline_transactions = observations
            .try_iter()
            .filter(|sample| sample.call == Call::BeginChecked)
            .count();
        assert!(baseline_transactions > 0);

        let _fourth_source_slot = state.tenants[1]
            .io
            .admit(store_io::Lane::Read, false)
            .unwrap();
        let result = resources_from_verified_proof(&state, &proof, &context_a).await;
        let status = result
            .as_ref()
            .err()
            .map_or(StatusCode::OK, |error| error.0);
        let transactions = observations
            .try_iter()
            .filter(|sample| sample.call == Call::BeginChecked)
            .count();
        assert_eq!(
            transactions, 0,
            "context A bypassed the source B admission during revalidation; status={status}"
        );
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    }
}
