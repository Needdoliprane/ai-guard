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
use axum::extract::{FromRequest, FromRequestParts};
use axum::http::request::Parts;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
mod admin;
mod brokers;
/// Proposed owned (formats 14/15) human broker registry projection, served on
/// its own path because its page shape is not the legacy page's.
mod brokers_owned;
#[cfg(test)]
pub(super) mod event_read_test_hook;
mod recent;
mod summary;
mod workers;

const CONTEXT_HEADER: &str = "x-aiguard-context";
pub(super) struct HumanAccessState {
    contexts: ContextRegistry,
    #[cfg(test)]
    broker_publication_pause: std::sync::Mutex<Option<BrokerPublicationPause>>,
    #[cfg(test)]
    event_publication_pause: std::sync::Mutex<Option<event_read_test_hook::Pause>>,
}
#[cfg(test)]
struct BrokerPublicationPause {
    reached: std::sync::mpsc::SyncSender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}
impl HumanAccessState {
    pub(super) fn close_contexts(&self) {
        self.contexts.close();
    }
    pub(super) fn for_tenants<'a>(
        tenants: impl IntoIterator<Item = &'a str>,
    ) -> anyhow::Result<Self> {
        Ok(Self::from_contexts(ContextRegistry::for_tenants(tenants)?))
    }
    pub(super) fn disabled() -> Self {
        Self::from_contexts(ContextRegistry::disabled())
    }
    fn from_contexts(contexts: ContextRegistry) -> Self {
        Self {
            contexts,
            #[cfg(test)]
            broker_publication_pause: std::sync::Mutex::new(None),
            #[cfg(test)]
            event_publication_pause: std::sync::Mutex::new(None),
        }
    }
    #[cfg(test)]
    fn pause_broker_publication(&self) -> ApiResult<()> {
        let pause = self
            .broker_publication_pause
            .lock()
            .map_err(|_| cursor_unavailable())?
            .take();
        if let Some(pause) = pause {
            pause.reached.send(()).map_err(|_| cursor_unavailable())?;
            pause
                .resume
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| cursor_unavailable())?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod brokers_tests;
#[cfg(test)]
pub(super) use brokers_tests::install_registry_publication_pause;
#[cfg(test)]
mod context_fairness_controls;
#[cfg(test)]
mod context_fairness_tests;
#[cfg(test)]
mod epoch_error_tests;

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
pub(super) fn store_failure(error: anyhow::Error) -> (StatusCode, Json<Value>) {
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
        _ if matches!(
            error.downcast_ref::<crate::human_authority::HumanAuthorityFailure>(),
            Some(crate::human_authority::HumanAuthorityFailure::EpochExpired)
        ) =>
        {
            failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "identity_unavailable",
                "The human access check expired. Refresh identity, then retry the refused read explicitly.",
                "retry",
            )
        }
        _ => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "audit_unavailable",
            "The required identity or audit store is unavailable.",
            "contact_operator",
        ),
    }
}
/// Failures of a delegated identity change. A conflict (stale revision, or
/// an operation identifier reused for another change) asks for a re-read; a
/// refusal names the change, not a read. Everything else maps like a read.
fn identity_change_failure(
    error: anyhow::Error,
    conflict: &str,
    invalid: fn() -> (StatusCode, Json<Value>),
) -> (StatusCode, Json<Value>) {
    match error.downcast_ref::<IdentityFailureV1>() {
        Some(IdentityFailureV1::Conflict) => {
            failure(StatusCode::CONFLICT, "conflict", conflict, "reread")
        }
        Some(IdentityFailureV1::PermissionDenied) => failure(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "This context does not permit this change.",
            "review_permissions",
        ),
        Some(IdentityFailureV1::InvalidRequest) => invalid(),
        _ => store_failure(error),
    }
}
pub(super) fn membership_change_failure(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    identity_change_failure(
        error,
        "The membership changed since it was read, or this operation identifier was used for another change. Re-read it before deciding again.",
        invalid_membership_request,
    )
}

pub(super) fn grant_change_failure(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    identity_change_failure(
        error,
        "The shares changed since they were read, or this operation identifier was used for another change. Re-read the shares before deciding again.",
        invalid_grant_request,
    )
}

/// Failures of a nominative launch preparation. A conflict (a divergent intent
/// reused under one operation, or a stale target revision) asks for a re-read;
/// a missing capability asks for the explicit migration. Everything else maps
/// like a read. Prepare confers no grant, reservation or start claim.
pub(super) fn session_prepare_failure(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    match error.downcast_ref::<IdentityFailureV1>() {
        Some(IdentityFailureV1::Conflict) => failure(
            StatusCode::CONFLICT,
            "conflict",
            "The preparation intent differs from the one already retained under this operation, or the target revision changed. Re-read it before deciding again.",
            "reread",
        ),
        Some(IdentityFailureV1::PermissionDenied) => failure(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "This context does not permit preparing this launch.",
            "review_permissions",
        ),
        Some(IdentityFailureV1::InvalidRequest) => invalid_session_prepare_request(),
        _ => store_failure(error),
    }
}

pub(super) fn invalid_session_prepare_request() -> (StatusCode, Json<Value>) {
    failure(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "Check the launch preparation fields, then try again.",
        "retry",
    )
}

/// Failures of a nominative session launch. A conflict (a divergent intent reused
/// under one operation, a consumed preparation, or a changed target revision)
/// asks for a re-read; a missing capability asks for the explicit migration.
/// Launch creates the real grant; it never elevates from a read or admin context.
pub(super) fn session_launch_failure(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    match error.downcast_ref::<IdentityFailureV1>() {
        Some(IdentityFailureV1::Conflict) => failure(
            StatusCode::CONFLICT,
            "conflict",
            "The launch intent differs from the one already retained under this operation, the preparation was already consumed, or the target revision changed. Re-read it before deciding again.",
            "reread",
        ),
        Some(IdentityFailureV1::PermissionDenied) => failure(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "This context does not permit launching this session.",
            "review_permissions",
        ),
        Some(IdentityFailureV1::InvalidRequest) => invalid_session_launch_request(),
        _ => store_failure(error),
    }
}

pub(super) fn invalid_session_launch_request() -> (StatusCode, Json<Value>) {
    failure(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "Check the session launch fields, then try again.",
        "retry",
    )
}

/// Failures of one SendSessionMessage enqueue. The closed control-plane states
/// are named distinctly (unsupported channel, STOP, changed binding, expiry,
/// already-issued delivery, masking refusal, bounded queue); a divergent intent
/// under one operation is a conflict. The body is never echoed in an error.
pub(super) fn session_message_failure(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    use crate::audit::session_message::SessionMessageFailureV1 as M;
    if let Some(reason) = error.downcast_ref::<M>() {
        let (status, code, message, retry) = match reason {
            M::ChannelNotAdmitted => (
                StatusCode::CONFLICT,
                "channel_not_admitted",
                "The session has no resolved conversation channel; input is unsupported until the trusted control binds one.",
                "retry",
            ),
            M::SessionClosed => (
                StatusCode::CONFLICT,
                "session_closed",
                "The session conversation is closed; a message never reopens it.",
                "reread",
            ),
            M::BindingChanged => (
                StatusCode::CONFLICT,
                "binding_changed",
                "The conversation binding changed since it was read. Re-read it before sending again.",
                "reread",
            ),
            M::BindingExpired => (
                StatusCode::CONFLICT,
                "binding_expired",
                "The session's original deadline passed; it keeps priority over new input.",
                "reread",
            ),
            M::EmissionAlreadyIssued => (
                StatusCode::CONFLICT,
                "delivery_unknown",
                "An emission was already issued for this operation; delivery uncertainty is retained, never resent.",
                "reread",
            ),
            M::ContentRequiresRedaction => (
                StatusCode::BAD_REQUEST,
                "content_requires_redaction",
                "The instruction cannot be masked safely without changing its meaning; it is refused before enqueue.",
                "retry",
            ),
            M::CapacityExhausted => (
                StatusCode::TOO_MANY_REQUESTS,
                "capacity_exhausted",
                "The bounded unresolved message queue for this session or principal is full.",
                "retry",
            ),
        };
        return failure(status, code, message, retry);
    }
    match error.downcast_ref::<IdentityFailureV1>() {
        Some(IdentityFailureV1::Conflict) => failure(
            StatusCode::CONFLICT,
            "conflict",
            "The instruction differs from the one already retained under this operation. Re-read it before deciding again.",
            "reread",
        ),
        Some(IdentityFailureV1::PermissionDenied) => failure(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "This context does not permit sending a message to this session.",
            "review_permissions",
        ),
        Some(IdentityFailureV1::InvalidRequest) => invalid_session_message_request(),
        _ => store_failure(error),
    }
}

pub(super) fn invalid_session_message_request() -> (StatusCode, Json<Value>) {
    failure(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "Check the session message fields, then try again.",
        "retry",
    )
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

pub(super) fn invalid_membership_request() -> (StatusCode, Json<Value>) {
    failure(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "Check the membership change fields, then try again.",
        "retry",
    )
}

pub(super) fn invalid_grant_request() -> (StatusCode, Json<Value>) {
    failure(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "Check the share change fields, then try again.",
        "retry",
    )
}

/// Read the delegated membership request on either HTTP entry point. Keep the
/// original bytes for the dashboard relay and let the gateway use the typed body.
pub(super) async fn membership_change_body(
    request: Request,
) -> ApiResult<(axum::body::Bytes, HumanMembershipChangeV1)> {
    let (bytes, change) = delegated_change_body::<HumanMembershipChangeV1>(
        request,
        MEMBERSHIP_CHANGE_BODY_MAX_BYTES,
        invalid_membership_request,
    )
    .await?;
    crate::control::uuid(&change.operation_id).map_err(|_| invalid_membership_request())?;
    Ok((bytes, change))
}

pub(super) async fn grant_change_body(
    request: Request,
) -> ApiResult<(axum::body::Bytes, HumanGrantChangeV1)> {
    let (bytes, change) = delegated_change_body::<HumanGrantChangeV1>(
        request,
        GRANT_CHANGE_BODY_MAX_BYTES,
        invalid_grant_request,
    )
    .await?;
    crate::control::uuid(&change.operation_id).map_err(|_| invalid_grant_request())?;
    Ok((bytes, change))
}

pub(super) async fn delegated_change_body<T: DeserializeOwned>(
    request: Request,
    max_bytes: usize,
    invalid: impl Fn() -> (StatusCode, Json<Value>),
) -> ApiResult<(axum::body::Bytes, T)> {
    let (parts, body) = request.into_parts();
    if parts.headers.get_all(header::CONTENT_TYPE).iter().count() != 1 {
        return Err(invalid());
    }
    let bytes = tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(body, max_bytes),
    )
    .await
    .map_err(|_| invalid())?
    .map_err(|_| invalid())?;
    // Serde's struct visitor also accepts sequences. The HTTP contract is an
    // object, with the typed parse still checking duplicate and unknown keys.
    if bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        != Some(b'{')
    {
        return Err(invalid());
    }
    let buffered = Request::from_parts(parts, Body::from(bytes.clone()));
    let Json(change) = Json::<T>::from_request(buffered, &())
        .await
        .map_err(|_| invalid())?;
    Ok((bytes, change))
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
pub(super) struct VerifiedHuman {
    credential: HumanAuthenticationProof,
    source: HumanProofSource,
    digest: [u8; 32],
    redactor: Redactor,
    _owned_usage: Option<Arc<super::human_sources::HumanOwnedUsage>>,
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
    if let Some(owned) = &state.owned_human {
        if !crate::identity::local_credential_format(token) {
            return Err(super::human_sources::unavailable());
        }
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let source = owned.source(&digest)?.ok_or_else(|| {
            failure(
                StatusCode::UNAUTHORIZED,
                "credential_invalid",
                "The credential is not a local human credential.",
                "authenticate",
            )
        })?;
        let usage = owned.admit()?;
        let credential = owned
            .authenticate(source, state.tenants[source].store.sqlite_api()?, &digest)?
            .ok_or_else(super::human_sources::unavailable)?;
        return Ok(VerifiedHuman {
            credential: HumanAuthenticationProof {
                principal: credential.principal,
                credential_id: credential.credential_id,
                revision: credential.revision,
                expires_at: credential.expires_at,
            },
            source: HumanProofSource::Local(source),
            digest,
            redactor: state.redactor.union(&Redactor::new(vec![token.to_owned()])),
            _owned_usage: Some(usage),
        });
    }
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
        let proof = VerifiedHuman {
            credential: HumanAuthenticationProof {
                principal: proof.principal.clone(),
                credential_id: proof.authentication_id.clone(),
                revision: u64::from(proof.generation),
                expires_at: Some(expiry),
            },
            digest: proof.digest,
            source: HumanProofSource::Oidc(proof),
            redactor: state.redactor.union(&Redactor::new(vec![token.to_owned()])),
            _owned_usage: None,
        };
        check_legacy_human_collision(state, &proof)?;
        return Ok(proof);
    }
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let mut matches = Vec::new();
    let mut unavailable = None;
    for (index, tenant) in state.tenants.iter().enumerate() {
        match credential_binding(state, tenant, &digest, None) {
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
    let proof = VerifiedHuman {
        credential: HumanAuthenticationProof {
            principal: credential.principal,
            credential_id: credential.credential_id,
            revision: credential.revision,
            expires_at: credential.expires_at,
        },
        source: HumanProofSource::Local(source),
        digest,
        redactor: state.redactor.union(&Redactor::new(vec![token.to_owned()])),
        _owned_usage: None,
    };
    check_legacy_human_collision(state, &proof)?;
    Ok(proof)
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
    own_lane: Option<store_io::Lane>,
) -> Result<Option<LocalCredentialV1>, MetadataUnavailable> {
    let Some(lane) = own_lane else {
        return tenant
            .store
            .sqlite()
            .map_err(|refusal| MetadataUnavailable::Storage(refusal.into()))?
            .try_local_credential_binding(digest)
            .into_result()
            .map_err(MetadataUnavailable::Storage);
    };
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
    if state.owned_human.is_some() {
        return Err(super::human_sources::unavailable());
    }
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    for (index, tenant) in state.tenants.iter().enumerate() {
        // MySQL tenants are checked once, with the workload family, from a
        // single verified snapshot (see workload::reject_control_collision).
        #[cfg(feature = "mysql-storage")]
        if tenant.store.mysql().is_some() {
            continue;
        }
        let lane = own
            .filter(|(source, _)| *source == index)
            .map(|(_, lane)| lane);
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
    if let Some(owned) = &state.owned_human {
        let HumanProofSource::Local(source) = proof.source else {
            return Err(super::human_sources::unavailable());
        };
        let verified = owned
            .authenticate(
                source,
                state.tenants[source].store.sqlite_api()?,
                &proof.digest,
            )?
            .ok_or_else(super::human_sources::unavailable)?;
        if verified.principal != proof.credential.principal
            || verified.credential_id != proof.credential.credential_id
            || verified.revision != proof.credential.revision
        {
            owned.require_rebuild();
            return Err(super::human_sources::unavailable());
        }
        return Ok(());
    }
    if let HumanProofSource::Local(source) = proof.source {
        let mut count = 0usize;
        for (index, tenant) in state.tenants.iter().enumerate() {
            let lane = (index == source).then_some(store_io::Lane::Read);
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
    }
    validate_legacy_human_proof(state, proof)?;
    check_legacy_human_collision(state, proof)
}

fn check_legacy_human_collision(state: &GatewayState, proof: &VerifiedHuman) -> ApiResult<()> {
    let source = match &proof.source {
        HumanProofSource::Local(source) => Some(*source),
        HumanProofSource::Oidc(_) => None,
    };
    super::workload::reject_human_collision(state, &proof.digest, source)?;
    // A verified peer lookup can consume its network budget. Check the same
    // source and proof again before proceeding, without another peer lookup.
    #[cfg(feature = "mysql-storage")]
    if state
        .tenants
        .iter()
        .any(|tenant| tenant.store.mysql().is_some())
    {
        validate_legacy_human_proof(state, proof)?;
    }
    Ok(())
}

fn validate_legacy_human_proof(state: &GatewayState, proof: &VerifiedHuman) -> ApiResult<()> {
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
    let mut scope = if let Some(owned) = &state.owned_human {
        let HumanProofSource::Local(source) = proof.source else {
            return Err(super::human_sources::unavailable());
        };
        if tenant.tenant_id != state.tenants[source].tenant_id {
            return Err(super::human_sources::unavailable());
        }
        owned.authorize(
            source,
            tenant.store.sqlite_api()?,
            &proof.credential.principal,
            &binding.membership_id,
            binding.membership_revision,
            binding.mode,
        )?
    } else {
        tenant
            .store
            .sqlite_api()?
            .authorize_human_context(
                &proof.credential.principal,
                &binding.membership_id,
                binding.membership_revision,
                binding.mode,
            )
            .map_err(store_failure)?
    };
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
    let usage = state
        .owned_human
        .as_ref()
        .map(|owned| owned.admit())
        .transpose()?;
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
            let _usage = usage;
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
    fn has_event_detail_filters(&self) -> bool {
        self.cursor.is_some() || self.resource_id.is_some() || self.resource_kind.is_some()
    }

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
        archived_through_seq: None,
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

/// Upper bound of one retained launch-preparation intent. It names an opaque
/// target, its revision, an operation id and a deadline; it carries no argv,
/// host path, credential or free provider choice.
pub(super) const SESSION_PREPARE_BODY_MAX_BYTES: usize = 4_096;

async fn session_prepare_body(
    request: Request,
) -> ApiResult<crate::control::session_control::SessionLaunchPrepareRequestV1> {
    let (_, request) =
        delegated_change_body::<crate::control::session_control::SessionLaunchPrepareRequestV1>(
            request,
            SESSION_PREPARE_BODY_MAX_BYTES,
            invalid_session_prepare_request,
        )
        .await?;
    request
        .validate()
        .map_err(|_| invalid_session_prepare_request())?;
    Ok(request)
}

/// `POST /api/v1/human/owned/sessions/prepare`: retain one nominative launch
/// preparation on the caller's own owned source. The personal launch context is
/// revalidated inside the transaction; the mandatory audit receipt and the
/// preparation row commit together before the receipt is returned. Prepare
/// creates no grant, reservation or start claim. An exact replay returns the
/// original receipt to a still-authorized caller; a divergent intent under the
/// same operation is a conflict; revocation closes it.
pub(super) async fn session_launch_prepare(
    State(state): State<GatewayState>,
    request: Request,
) -> ApiResult<Json<crate::control::session_control::SessionPrepareReceiptV1>> {
    if state.owned_human.is_none() {
        return Err(super::human_sources::unavailable());
    }
    let (mut parts, body) = request.into_parts();
    let proof = VerifiedHuman::from_request_parts(&mut parts, &state).await?;
    let headers = parts.headers.clone();
    let prepare = session_prepare_body(Request::from_parts(parts, body)).await?;
    let selected = headers.clone();
    run_human(&state, &proof, Some(&headers), move |state, proof| {
        let (_, tenant, scope) = selected_context(&state, &proof, &selected)?;
        let store = tenant.store.sqlite_api()?;
        let owned = state
            .owned_human
            .as_ref()
            .ok_or_else(super::human_sources::unavailable)?;
        let HumanProofSource::Local(source) = proof.source else {
            return Err(super::human_sources::unavailable());
        };
        let receipt = owned.session_launch_prepare(source, store, &scope, &prepare)?;
        Ok(Json(receipt))
    })
    .await
}

/// Upper bound of one session-launch commit. It names the launch operation, the
/// opaque target and revision, and the consumed preparation by reference and
/// exact fingerprint; it carries no argv, host path, image, credential or free
/// provider choice.
pub(super) const SESSION_LAUNCH_BODY_MAX_BYTES: usize = 4_096;

async fn session_launch_body(
    request: Request,
) -> ApiResult<crate::control::session_control::SessionLaunchRequestV1> {
    let (_, request) =
        delegated_change_body::<crate::control::session_control::SessionLaunchRequestV1>(
            request,
            SESSION_LAUNCH_BODY_MAX_BYTES,
            invalid_session_launch_request,
        )
        .await?;
    request
        .validate()
        .map_err(|_| invalid_session_launch_request())?;
    Ok(request)
}

/// `POST /api/v1/human/owned/sessions/launch`: commit one retained preparation as
/// a real nominative session launch on the caller's own owned source. The personal
/// launch context is revalidated inside the identity-authority transaction; the
/// mandatory launch audit, the nominative session grant and the retained
/// conversation binding commit together before the receipt is returned. An exact
/// replay returns the original receipt to a still-authorized caller; a divergent
/// intent under the same operation, a consumed preparation or a changed target is
/// a conflict; revocation closes it. The supervised worker admission and the
/// runtime-observed start remain the broker's separate effect.
pub(super) async fn session_launch(
    State(state): State<GatewayState>,
    request: Request,
) -> ApiResult<Json<crate::control::session_control::SessionLaunchReceiptV1>> {
    if state.owned_human.is_none() {
        return Err(super::human_sources::unavailable());
    }
    let (mut parts, body) = request.into_parts();
    let proof = VerifiedHuman::from_request_parts(&mut parts, &state).await?;
    let headers = parts.headers.clone();
    let launch = session_launch_body(Request::from_parts(parts, body)).await?;
    let selected = headers.clone();
    run_human(&state, &proof, Some(&headers), move |state, proof| {
        let (_, tenant, scope) = selected_context(&state, &proof, &selected)?;
        let store = tenant.store.sqlite_api()?;
        let owned = state
            .owned_human
            .as_ref()
            .ok_or_else(super::human_sources::unavailable)?;
        let HumanProofSource::Local(source) = proof.source else {
            return Err(super::human_sources::unavailable());
        };
        let receipt = owned.session_launch(source, store, &scope, &launch)?;
        Ok(Json(receipt))
    })
    .await
}

/// `GET /api/v1/human/owned/sessions/preparations/{operation_id}`: read one
/// retained preparation receipt for a still-authorized caller. Revocation of
/// the identity, membership or context closes the read.
pub(super) async fn session_preparation(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    path: Result<Path<String>, axum::extract::rejection::PathRejection>,
) -> ApiResult<Json<crate::control::session_control::SessionPrepareReceiptV1>> {
    if state.owned_human.is_none() {
        return Err(super::human_sources::unavailable());
    }
    let Path(operation_id) = path.map_err(|_| invalid_session_prepare_request())?;
    let selected = headers.clone();
    run_human(&state, &proof, Some(&headers), move |state, proof| {
        let (_, tenant, scope) = selected_context(&state, &proof, &selected)?;
        let store = tenant.store.sqlite_api()?;
        let owned = state
            .owned_human
            .as_ref()
            .ok_or_else(super::human_sources::unavailable)?;
        let HumanProofSource::Local(source) = proof.source else {
            return Err(super::human_sources::unavailable());
        };
        let receipt = owned.session_preparation(source, store, &scope, &operation_id)?;
        Ok(Json(receipt))
    })
    .await
}

/// Upper bound of one short-instruction body. It names the session, the pinned
/// input epoch, the operation id and the instruction; it carries no argv, host
/// path, credential or provider choice. The body is tenant business data.
pub(super) const SESSION_MESSAGE_BODY_MAX_BYTES: usize = 16_384;

async fn session_message_body(
    request: Request,
) -> ApiResult<crate::control::session_control::SessionMessageRequestV1> {
    let (_, request) =
        delegated_change_body::<crate::control::session_control::SessionMessageRequestV1>(
            request,
            SESSION_MESSAGE_BODY_MAX_BYTES,
            invalid_session_message_request,
        )
        .await?;
    request
        .validate()
        .map_err(|_| invalid_session_message_request())?;
    Ok(request)
}

/// `POST /api/v1/human/owned/sessions/messages`: durably enqueue one short
/// instruction to an existing session on the caller's own owned source. The
/// personal SendSessionMessage context and the session's owner ACL are
/// revalidated inside the transaction; the mandatory audit receipt and the
/// message row commit together before the receipt is returned. The body is
/// never duplicated into the audit. An exact replay returns the retained result
/// to a still-authorized caller; divergent content is a conflict; STOP, expiry,
/// a changed binding or revocation closes it. Emission on the resolved channel
/// remains the trusted control's separate effect.
pub(super) async fn session_message_send(
    State(state): State<GatewayState>,
    request: Request,
) -> ApiResult<Json<crate::control::session_control::SessionMessageReceiptV1>> {
    if state.owned_human.is_none() {
        return Err(super::human_sources::unavailable());
    }
    let (mut parts, body) = request.into_parts();
    let proof = VerifiedHuman::from_request_parts(&mut parts, &state).await?;
    let headers = parts.headers.clone();
    let message = session_message_body(Request::from_parts(parts, body)).await?;
    let selected = headers.clone();
    run_human(&state, &proof, Some(&headers), move |state, proof| {
        let (_, tenant, scope) = selected_context(&state, &proof, &selected)?;
        let store = tenant.store.sqlite_api()?;
        let owned = state
            .owned_human
            .as_ref()
            .ok_or_else(super::human_sources::unavailable)?;
        let HumanProofSource::Local(source) = proof.source else {
            return Err(super::human_sources::unavailable());
        };
        let receipt = owned.session_message_send(source, store, &scope, &message)?;
        Ok(Json(receipt))
    })
    .await
}

/// `GET /api/v1/human/owned/sessions/messages/{operation_id}`: read one retained
/// message receipt (metadata and delivery status only, never the body) for a
/// still-authorized owner. Revocation closes the read.
pub(super) async fn session_message_receipt(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    path: Result<Path<String>, axum::extract::rejection::PathRejection>,
) -> ApiResult<Json<crate::control::session_control::SessionMessageReceiptV1>> {
    if state.owned_human.is_none() {
        return Err(super::human_sources::unavailable());
    }
    let Path(operation_id) = path.map_err(|_| invalid_session_message_request())?;
    let selected = headers.clone();
    run_human(&state, &proof, Some(&headers), move |state, proof| {
        let (_, tenant, scope) = selected_context(&state, &proof, &selected)?;
        let store = tenant.store.sqlite_api()?;
        let owned = state
            .owned_human
            .as_ref()
            .ok_or_else(super::human_sources::unavailable)?;
        let HumanProofSource::Local(source) = proof.source else {
            return Err(super::human_sources::unavailable());
        };
        let receipt = owned.session_message_receipt(source, store, &scope, &operation_id)?;
        Ok(Json(receipt))
    })
    .await
}

pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/human/identity", get(identity))
        .route(
            "/human/owned/sessions/prepare",
            post(session_launch_prepare).layer(axum::extract::DefaultBodyLimit::max(
                SESSION_PREPARE_BODY_MAX_BYTES,
            )),
        )
        .route(
            "/human/owned/sessions/launch",
            post(session_launch).layer(axum::extract::DefaultBodyLimit::max(
                SESSION_LAUNCH_BODY_MAX_BYTES,
            )),
        )
        .route(
            "/human/owned/sessions/preparations/{operation_id}",
            get(session_preparation),
        )
        .route(
            "/human/owned/sessions/messages",
            post(session_message_send).layer(axum::extract::DefaultBodyLimit::max(
                SESSION_MESSAGE_BODY_MAX_BYTES,
            )),
        )
        .route(
            "/human/owned/sessions/messages/{operation_id}",
            get(session_message_receipt),
        )
        .route("/human/brokers", get(brokers::brokers))
        .route("/human/owned/brokers", get(brokers_owned::brokers))
        .route("/human/workers", get(workers::workers))
        .route(
            "/human/federation/workers",
            get(workers::federation_workers),
        )
        .route("/human/summary", get(summary::summary))
        .route("/human/admin/quota", get(admin::quota))
        .route("/human/admin/usage", get(admin::usage))
        .route("/human/admin/memberships", get(admin::memberships))
        .route(
            "/human/admin/grants",
            get(admin::grant).post(admin::change_grant).layer(
                axum::extract::DefaultBodyLimit::max(GRANT_CHANGE_BODY_MAX_BYTES),
            ),
        )
        .route(
            "/human/admin/memberships/{membership_id}",
            post(admin::change_membership),
        )
        .route(
            "/human/owned/admin/memberships/{membership_id}",
            post(admin::change_owned_membership),
        )
        .route("/human/owned/admin/grants", post(admin::change_owned_grant))
        .route("/human/admin/configuration", get(admin::configuration))
        .route("/human/resources", get(resources))
        .route("/human/events", get(events))
        .route("/human/events/recent", get(recent::recent))
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
    let mut capacity_exhausted = false;
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
        if state.owned_human.is_some()
            && !matches!(proof.source,HumanProofSource::Local(source) if source==index)
        {
            contexts_complete = false;
            continue;
        }
        let snapshot = if let Some(owned) = &state.owned_human {
            // Preserve typed API status/code/retry action from the real own
            // source, especially cumulative audit-budget refusals.
            state
                .io_scope
                .with_store(tenant, store_io::Lane::Read, true, |store| {
                    owned.profile(index, store, &proof.credential.principal, &proof.redactor)
                })??
        } else {
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
            match snapshot {
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
        let facts = OperationFactsV1 {
            owned_profile: state.owned_human.is_some(),
            human_rights_marker: snapshot.human_rights_marker,
        };
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
                let context =
                    match state
                        .human
                        .contexts
                        .obtain(binding, remaining, expires.to_rfc3339())
                    {
                        Ok(context) => context,
                        Err(StateFailure::Capacity) => {
                            contexts_complete = false;
                            capacity_exhausted = true;
                            continue;
                        }
                        Err(error) => return Err(state_failure(error)),
                    };
                let capabilities: Vec<HumanCapabilityV1> = member
                    .capabilities
                    .iter()
                    .copied()
                    .filter(|cap| mode == ContextModeV1::Administration || !cap.is_administration())
                    .collect();
                let operations = context_operations(
                    mode,
                    &capabilities.iter().copied().collect(),
                    &member.capabilities,
                    facts,
                );
                contexts.push(HumanContextV1 {
                    id: context.id.clone(),
                    tenant_id: tenant.tenant_id.clone(),
                    organization_scope: member.organization_scope.clone(),
                    mode,
                    capabilities,
                    membership_revision: member.revision.to_string(),
                    authorization_version: context.authorization_version.clone(),
                    expires_at: context.expires_at.clone(),
                    operations,
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
        read_profiles: state.owned_human.as_ref().map(|_| HumanReadProfilesV1 {
            broker_registry: HumanBrokerRegistryReadProfileV1::OwnedV1,
            worker_registry: HumanWorkerRegistryReadProfileV1::OwnedV1,
            federation_registry: HumanUnavailableReadProfileV1::Unavailable,
        }),
    };
    if capacity_exhausted && identity.contexts.is_empty() {
        return Err(state_failure(StateFailure::Capacity));
    }
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
    let page = if let Some(owned) = &state.owned_human {
        let HumanProofSource::Local(source) = proof.source else {
            return Err(super::human_sources::unavailable());
        };
        owned.resources(
            source,
            tenant.store.sqlite_api()?,
            &scope,
            &cursor.after_resource,
            key.limit,
            &proof.redactor,
        )?
    } else {
        tenant
            .store
            .sqlite_api()?
            .human_resources(&scope, &cursor.after_resource, key.limit, &proof.redactor)
            .map_err(store_failure)?
    };
    reauthenticate(&state, &proof)?;
    cursor.after_resource = page.after_resource.clone();
    if state.owned_human.is_some() {
        selected_context(&state, &proof, &headers)?;
    }
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
pub(super) fn owned_event_detail_query(uri: &axum::http::Uri) -> bool {
    Query::<PageQuery>::try_from_uri(uri).is_ok_and(|Query(query)| {
        !query.has_event_detail_filters()
            && matches!(query.view, None | Some(HumanReadViewV1::Metadata))
    })
}

async fn event(
    proof: VerifiedHuman,
    State(state): State<GatewayState>,
    headers: HeaderMap,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Path(id): Path<String>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> ApiResult<Json<HumanPageV1<HumanEventItemV1>>> {
    let valid_id = uuid::Uuid::parse_str(&id).is_ok_and(|uuid| {
        state.owned_human.is_none()
            || (uuid.to_string() == id && uri.path().rsplit('/').next() == Some(id.as_str()))
    });
    if !valid_id {
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
        if query.has_event_detail_filters() {
            return Err(invalid_request());
        }
        query.limit = Some(1);
    }
    let key = query.key(if is_export {
        Traversal::Export
    } else {
        Traversal::Events
    })?;
    if state.owned_human.is_some() && (is_export || key.view != HumanReadViewV1::Metadata) {
        return Err(super::human_sources::unavailable());
    }
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
    let poll: HumanEventPollV1 = if let Some(owned) = &state.owned_human {
        let HumanProofSource::Local(source) = proof.source else {
            return Err(super::human_sources::unavailable());
        };
        owned.events(
            source,
            tenant.store.sqlite_api()?,
            &scope,
            &filter,
            cursor.after_event,
            cursor.upper_event,
            key.limit,
            &proof.redactor,
        )?
    } else {
        tenant
            .store
            .sqlite_api()?
            .human_events(
                &scope,
                &filter,
                cursor.after_event,
                cursor.upper_event,
                key.limit,
                &proof.redactor,
            )
            .map_err(store_failure)?
    };
    let page = poll.page.ok_or_else(invalid_request)?;
    #[cfg(test)]
    event_read_test_hook::pause(&state.human)?;
    if id.is_some() && page.items.is_empty() {
        return Err(failure(
            StatusCode::NOT_FOUND,
            "not_found",
            "The event is not available in this context.",
            "review_permissions",
        ));
    }
    reauthenticate(&state, &proof)?;
    if state.owned_human.is_some() {
        selected_context(&state, &proof, &headers)?;
    }
    cursor.after_event = page.after_event;
    cursor.upper_event = Some(page.upper_event);
    let mut page = envelope(&context, page, Some(key.view), is_export);
    page.archived_through_seq = poll.archived_through_seq;
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
        .sqlite_api()?
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
#[path = "human_workload_tests.rs"]
mod workload_collision_tests;

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
        let observations = observe(state.tenants[1].store.sqlite_api().unwrap());
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
