//! Workload-only HTTP authentication. Metadata discovery is not an authority proof.
use super::*;
use crate::{
    broker_registry::BrokerIdentityV1,
    relay::{RelayFailure, WorkerRelayIdentity},
    workload::{
        AuthenticatedWorkload, WorkloadBindingIssueV1, WorkloadBindingReceiptV1,
        WorkloadBindingRevocationV1, WorkloadFailureV1,
    },
};
use axum::{extract::FromRequestParts, http::request::Parts};

// Single-process serialization of HTTP issue preflight and durable installation.
// This is not a claim of cross-process or multi-gateway uniqueness/availability.
static ISSUANCE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/workload-bindings", post(issue))
        .route("/workload-bindings/{id}/revoke", post(revoke))
        .layer(middleware::from_fn(request_guard))
}

pub(super) fn response(status: StatusCode, code: &str, message: &str) -> (StatusCode, Json<Value>) {
    (
        status,
        Json(json!({"error":{"code":code,"message":message}})),
    )
}
fn unauthorized() -> (StatusCode, Json<Value>) {
    response(
        StatusCode::UNAUTHORIZED,
        "authentication_required",
        "A current workload bearer proof is required.",
    )
}
pub(super) fn forbidden() -> (StatusCode, Json<Value>) {
    response(
        StatusCode::FORBIDDEN,
        "workload_forbidden",
        "This workload proof does not authorize this operation.",
    )
}
fn ambiguous() -> (StatusCode, Json<Value>) {
    response(
        StatusCode::SERVICE_UNAVAILABLE,
        "identity_unavailable",
        "Credential ownership is ambiguous; contact the operator.",
    )
}
pub(super) fn invalid() -> (StatusCode, Json<Value>) {
    response(
        StatusCode::BAD_REQUEST,
        "invalid_workload_request",
        "The request does not match the closed workload contract.",
    )
}
fn clock_failure(error: &anyhow::Error) -> Option<(StatusCode, Json<Value>)> {
    let uncertain = error.downcast_ref::<WorkloadFailureV1>()
        == Some(&WorkloadFailureV1::ClockRegression)
        || error.downcast_ref::<crate::broker_registry::BrokerRegistryFailureV1>()
            == Some(&crate::broker_registry::BrokerRegistryFailureV1::ClockRegression);
    uncertain.then(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":{
                "code":"control_clock_uncertain",
                "message":"The control clock precedes its durable authorization evidence. Preserve the state and contact the operator.",
                "retry_action":"contact_operator"
            }})),
        )
    })
}
pub(super) fn failure(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    if let Some(response) = clock_failure(&error) {
        return response;
    }
    if let Some(response) = audit_capacity_failure(&error) {
        return response;
    }
    if error.downcast_ref::<ControlFailure>().is_some() {
        return control_error(error);
    }
    if let Some(failure) = error.downcast_ref::<crate::broker_registry::BrokerRegistryFailureV1>() {
        use crate::broker_registry::BrokerRegistryFailureV1 as B;
        return match failure {
            B::InvalidRequest => invalid(),
            B::Conflict => response(
                StatusCode::CONFLICT,
                "broker_conflict",
                "The broker operation conflicts with durable state.",
            ),
            B::NotFound => response(
                StatusCode::NOT_FOUND,
                "broker_not_found",
                "The broker resource was not found in the authorized scope.",
            ),
            B::PermissionDenied => forbidden(),
            B::MigrationRequired | B::ClockRegression => unavailable(error),
        };
    }
    match error.downcast_ref::<WorkloadFailureV1>() {
        Some(WorkloadFailureV1::InvalidRequest) => invalid(),
        Some(WorkloadFailureV1::Conflict) => response(
            StatusCode::CONFLICT,
            "workload_conflict",
            "The credential or operation conflicts with durable state.",
        ),
        Some(WorkloadFailureV1::Unauthorized) => forbidden(),
        _ if error.downcast_ref::<RelayFailure>() == Some(&RelayFailure::Forbidden) => forbidden(),
        _ => unavailable(error),
    }
}
fn authentication_failure(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    if let Some(response) = clock_failure(&error) {
        return response;
    }
    if error.downcast_ref::<WorkloadFailureV1>() == Some(&WorkloadFailureV1::Unauthorized) {
        unauthorized()
    } else if error.downcast_ref::<RelayFailure>() == Some(&RelayFailure::Forbidden)
        || error.downcast_ref::<crate::broker_registry::BrokerRegistryFailureV1>()
            == Some(&crate::broker_registry::BrokerRegistryFailureV1::PermissionDenied)
    {
        forbidden()
    } else {
        unavailable(error)
    }
}
pub(super) fn store_failure(store: &Store, error: anyhow::Error) -> (StatusCode, Json<Value>) {
    if store.healthy() {
        failure(error)
    } else {
        unavailable(error)
    }
}
pub(super) fn parse_json<T>(body: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    body.map(|Json(value)| value).map_err(|_| invalid())
}
pub(super) fn match_path(path: &str, field: &str) -> ApiResult<()> {
    if path == field {
        Ok(())
    } else {
        Err(invalid())
    }
}

/// Header ambiguity is rejected without polling a body. The outer gateway also
/// applies the origin/fetch-site policy and security headers to these routes.
pub(super) async fn request_guard(request: Request, next: Next) -> Response {
    if bearer(request.headers()).is_none() {
        return unauthorized().into_response();
    }
    if request.headers().get_all(header::ORIGIN).iter().count() > 1
        || request.headers().get_all("sec-fetch-site").iter().count() > 1
    {
        return forbidden().into_response();
    }
    next.run(request).await
}

fn resolve_sync(state: &GatewayState, token: &str) -> ApiResult<(usize, AuthenticatedWorkload)> {
    if !state.valid_credentials {
        return Err(ambiguous());
    }
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let control = state.credential_index.lookup(token).is_some();
    let mut sources = Vec::new();
    let mut human = false;
    let mut unavailable_source = None;
    for (index, tenant) in state.tenants.iter().enumerate() {
        let lookup = state
            .io_scope
            .with_store(tenant, store_io::Lane::Read, true, |store| {
                Ok::<_, anyhow::Error>((
                    store.workload_digest_binding(&digest)?,
                    store.local_credential_binding(&digest)?.is_some(),
                ))
            });
        match lookup {
            Ok(Ok((owner, local))) => {
                human |= local;
                if owner.is_some() {
                    sources.push(index);
                }
            }
            Ok(Err(error)) => {
                if unavailable_source.is_none() {
                    unavailable_source = Some(unavailable(error));
                }
            }
            Err(error) => {
                if unavailable_source.is_none() {
                    unavailable_source = Some(error);
                }
            }
        }
    }
    if sources.len() > 1 || (!sources.is_empty() && (control || human)) || (control && human) {
        return Err(ambiguous());
    }
    let Some(source) = sources.first().copied() else {
        if control || human {
            return Err(forbidden());
        }
        return Err(unavailable_source.unwrap_or_else(unauthorized));
    };
    // Policy B: a readable, independently authenticated source wins over an
    // unavailable neighbor. No unavailable source is ever treated as absent.
    let identity = state
        .io_scope
        .with_store(
            &state.tenants[source],
            store_io::Lane::Read,
            true,
            |store| store.authenticate_workload_token(token),
        )?
        .map_err(authentication_failure)?
        .ok_or_else(unauthorized)?;
    Ok((source, identity))
}

struct Proof {
    state: GatewayState,
    source: usize,
    identity: AuthenticatedWorkload,
    token: String,
    redactor: Redactor,
}
impl Proof {
    async fn extract(parts: &Parts, state: &GatewayState) -> ApiResult<Self> {
        let token = bearer(&parts.headers).ok_or_else(unauthorized)?;
        if !(32..=512).contains(&token.len()) {
            return Err(unauthorized());
        }
        let token = token.to_owned();
        let state = state.scoped(Arc::default());
        store_io::run(
            Arc::clone(&state.io),
            None,
            store_io::Lane::Read,
            false,
            move || {
                let (source, identity) = resolve_sync(&state, &token)?;
                let redactor = state.redactor.union(&Redactor::new(vec![token.clone()]));
                Ok(Self {
                    state: state.scoped(Arc::default()),
                    source,
                    identity,
                    token,
                    redactor,
                })
            },
        )
        .await
    }
    async fn blocking<T: Send + 'static>(
        self,
        operation: impl FnOnce(TenantState, AuthenticatedWorkload, Redactor) -> ApiResult<T>
        + Send
        + 'static,
    ) -> ApiResult<T> {
        let tenant = self
            .state
            .tenants
            .get(self.source)
            .cloned()
            .ok_or_else(ambiguous)?;
        let shared = Arc::clone(&self.state.io);
        store_io::run(shared, None, store_io::Lane::Read, false, move || {
            let mut scope = store_io::Scope::default();
            scope.hold(&tenant, store_io::Lane::Read, true)?;
            let state = self.state.scoped(Arc::new(scope));
            let (source, current) = resolve_sync(&state, &self.token)?;
            if source != self.source || !same_identity(&self.identity, &current) {
                return Err(forbidden());
            }
            operation(tenant, current, self.redactor)
        })
        .await
    }
}
fn same_identity(first: &AuthenticatedWorkload, second: &AuthenticatedWorkload) -> bool {
    match (first, second) {
        (AuthenticatedWorkload::Worker(a), AuthenticatedWorkload::Worker(b)) => {
            a.tenant_id() == b.tenant_id()
                && a.worker_id() == b.worker_id()
                && a.attempt_id() == b.attempt_id()
                && a.generation() == b.generation()
                && a.credential_id() == b.credential_id()
                && a.binding_revision() == b.binding_revision()
        }
        (AuthenticatedWorkload::Broker(a), AuthenticatedWorkload::Broker(b)) => {
            a.tenant_id() == b.tenant_id()
                && a.broker_id() == b.broker_id()
                && a.credential_id() == b.credential_id()
                && a.binding_revision() == b.binding_revision()
                && a.grant_revision() == b.grant_revision()
        }
        _ => false,
    }
}
pub(super) struct VerifiedWorker(Proof);
pub(super) struct VerifiedBroker(Proof);
impl FromRequestParts<GatewayState> for VerifiedWorker {
    type Rejection = (StatusCode, Json<Value>);
    async fn from_request_parts(parts: &mut Parts, state: &GatewayState) -> ApiResult<Self> {
        let proof = Proof::extract(parts, state).await?;
        if !matches!(&proof.identity, AuthenticatedWorkload::Worker(_)) {
            return Err(forbidden());
        }
        Ok(Self(proof))
    }
}
impl FromRequestParts<GatewayState> for VerifiedBroker {
    type Rejection = (StatusCode, Json<Value>);
    async fn from_request_parts(parts: &mut Parts, state: &GatewayState) -> ApiResult<Self> {
        let proof = Proof::extract(parts, state).await?;
        if !matches!(&proof.identity, AuthenticatedWorkload::Broker(_)) {
            return Err(forbidden());
        }
        Ok(Self(proof))
    }
}
impl VerifiedWorker {
    pub(super) async fn blocking<T: Send + 'static>(
        self,
        operation: impl FnOnce(TenantState, WorkerRelayIdentity, Redactor) -> ApiResult<T>
        + Send
        + 'static,
    ) -> ApiResult<T> {
        self.0
            .blocking(move |tenant, identity, redactor| match identity {
                AuthenticatedWorkload::Worker(identity) => operation(tenant, identity, redactor),
                _ => Err(forbidden()),
            })
            .await
    }
}
impl VerifiedBroker {
    pub(super) async fn blocking<T: Send + 'static>(
        self,
        operation: impl FnOnce(TenantState, BrokerIdentityV1, Redactor) -> ApiResult<T> + Send + 'static,
    ) -> ApiResult<T> {
        self.0
            .blocking(move |tenant, identity, redactor| match identity {
                AuthenticatedWorkload::Broker(identity) => operation(tenant, identity, redactor),
                _ => Err(forbidden()),
            })
            .await
    }
}

/// Hook for legacy control auth. A readable workload binding cannot shadow a
/// runner/viewer; a source failure is fatal, an unrelated failure follows B.
pub(super) fn reject_control_collision(
    state: &GatewayState,
    token: &str,
    own: Option<(usize, store_io::Lane)>,
) -> ApiResult<()> {
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    for (index, tenant) in state.tenants.iter().enumerate() {
        let lane = own
            .filter(|(source, _)| *source == index)
            .map_or(store_io::Lane::Read, |(_, lane)| lane);
        let lookup =
            state
                .io_scope
                .with_store(tenant, lane, lane == store_io::Lane::Read, |store| {
                    store.workload_digest_binding(&digest)
                });
        match lookup {
            Ok(Ok(Some(_))) => return Err(ambiguous()),
            Ok(Ok(None)) => (),
            Ok(Err(error)) if own.is_some_and(|(source, _)| source == index) => {
                return Err(unavailable(error));
            }
            Err(error) if own.is_some_and(|(source, _)| source == index) => return Err(error),
            _ => (),
        }
    }
    Ok(())
}

/// Negative class guard for an independently verified human proof. The caller
/// must verify its local source/OIDC session before applying policy B here.
/// This reads historical ownership, including rotated and revoked digests;
/// it neither creates a proof nor converts an unavailable lookup into absence.
pub(super) fn reject_human_collision(
    state: &GatewayState,
    digest: &[u8; 32],
    own_source: Option<usize>,
) -> ApiResult<()> {
    let unavailable_identity = || {
        human::failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "identity_unavailable",
            "Human and workload credential ownership cannot be established unambiguously. Contact the operator.",
            "contact_operator",
        )
    };
    if own_source.is_some_and(|source| source >= state.tenants.len()) {
        return Err(unavailable_identity());
    }
    for (index, tenant) in state.tenants.iter().enumerate() {
        let lane = if own_source == Some(index) {
            store_io::Lane::Read
        } else {
            store_io::Lane::Discovery
        };
        let lookup =
            state
                .io_scope
                .with_store(tenant, lane, lane == store_io::Lane::Read, |store| {
                    store.workload_digest_binding(digest)
                });
        match lookup {
            Ok(Ok(Some(_))) => return Err(unavailable_identity()),
            Ok(Ok(None)) => (),
            Ok(Err(_)) | Err(_) if own_source == Some(index) => return Err(unavailable_identity()),
            // Policy B only: the caller has freshly verified its independent
            // human source. Unknown neighbors stay unavailable, not absent.
            Ok(Err(_)) | Err(_) => (),
        }
    }
    Ok(())
}

async fn issue(
    RunnerIdentity(tenant): RunnerIdentity,
    State(state): State<GatewayState>,
    body: Result<Json<WorkloadBindingIssueV1>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<WorkloadBindingReceiptV1>)> {
    let input = parse_json(body)?;
    input.validate().map_err(failure)?;
    let source = state
        .tenants
        .iter()
        .position(|candidate| Arc::ptr_eq(&candidate.store, &tenant.store))
        .ok_or_else(ambiguous)?;
    let digest_vec = hex::decode(&input.sha256_digest).map_err(|_| invalid())?;
    let digest: [u8; 32] = digest_vec.try_into().map_err(|_| invalid())?;
    let mut scope = store_io::Scope::default();
    scope.hold(&tenant, store_io::Lane::Control, false)?;
    let scope = Arc::new(scope);
    let state = state.scoped(Arc::clone(&scope));
    store_io::run(
        Arc::clone(&state.io),
        None,
        store_io::Lane::Control,
        false,
        move || {
            let _scope = scope;
            let _issuance = ISSUANCE.try_lock().map_err(|_| {
                response(
                    StatusCode::TOO_MANY_REQUESTS,
                    "issuance_busy",
                    "Credential issuance is busy; retry.",
                )
            })?;
            let proof = tenant.control_proof.as_ref().ok_or_else(ambiguous)?;
            let (current, role) = proof
                .state
                .scoped(Arc::clone(&_scope))
                .authorize_sync(&proof.headers, true)?;
            if role != "runner" || !Arc::ptr_eq(&current.store, &tenant.store) {
                return Err(forbidden());
            }
            tenant.write_quota()?;
            if state.credential_index.lookup_digest(&digest).is_some() {
                return Err(failure(WorkloadFailureV1::Conflict.into()));
            }
            for (index, candidate) in state.tenants.iter().enumerate() {
                let lane = if index == source {
                    store_io::Lane::Control
                } else {
                    store_io::Lane::Read
                };
                let (owner, human) = state
                    .io_scope
                    .with_store(candidate, lane, lane == store_io::Lane::Read, |store| {
                        Ok::<_, anyhow::Error>((
                            store.workload_digest_binding(&digest)?,
                            store.local_credential_binding(&digest)?.is_some(),
                        ))
                    })?
                    .map_err(unavailable)?;
                if human
                    || owner.is_some_and(|owner| {
                        index != source
                            || owner.credential_id != input.credential_id
                            || owner.subject != input.subject
                    })
                {
                    return Err(failure(WorkloadFailureV1::Conflict.into()));
                }
            }
            let receipt = tenant
                .store
                .issue_workload_binding(&input)
                .map_err(|error| store_failure(&tenant.store, error))?;
            Ok((
                if receipt.replayed {
                    StatusCode::OK
                } else {
                    StatusCode::CREATED
                },
                Json(receipt),
            ))
        },
    )
    .await
}
async fn revoke(
    RunnerIdentity(tenant): RunnerIdentity,
    Path(id): Path<String>,
    body: Result<Json<WorkloadBindingRevocationV1>, JsonRejection>,
) -> ApiResult<Json<WorkloadBindingReceiptV1>> {
    let input = parse_json(body)?;
    match_path(&id, &input.credential_id)?;
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            Ok(Json(
                tenant
                    .store
                    .revoke_workload_binding(&input)
                    .map_err(|error| store_failure(&tenant.store, error))?,
            ))
        })
        .await
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicBool, Ordering},
    };
    use tower::ServiceExt;

    fn body(polled: Arc<AtomicBool>, payload: Vec<u8>) -> Body {
        Body::from_stream(futures_util::stream::once(async move {
            polled.store(true, Ordering::SeqCst);
            Ok::<_, std::io::Error>(axum::body::Bytes::from(payload))
        }))
    }
    async fn rejected_before_body_and_control_admitted(app: &Router) {
        let polled = Arc::new(AtomicBool::new(false));
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/v1/relay/messages")
            .header("authorization", format!("Bearer {}", "x".repeat(48)))
            .header("content-type", "application/json")
            .body(body(polled.clone(), b"must not be read".to_vec()))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert!(!polled.load(Ordering::SeqCst));

        let polled = Arc::new(AtomicBool::new(false));
        let input = WorkloadBindingIssueV1 {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            credential_id: uuid::Uuid::new_v4().to_string(),
            expected_revision: 0,
            subject: crate::workload::WorkloadSubjectV1::Broker {
                broker_id: uuid::Uuid::new_v4().to_string(),
                incarnation_id: uuid::Uuid::new_v4().to_string(),
            },
            sha256_digest: "1".repeat(64),
            expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
        };
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/v1/workload-bindings")
            .header("authorization", format!("Bearer {}", "r".repeat(48)))
            .header("content-type", "application/json")
            .body(body(polled.clone(), serde_json::to_vec(&input).unwrap()))
            .unwrap();
        // This schema2 fixture deliberately lacks migration6. The control lane
        // reaches its typed Store method and refuses the migration prerequisite;
        // it is not rejected because untrusted/read admission is occupied.
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(polled.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn workload_auth_uses_bounded_read_slots_and_does_not_spend_runner_reserve() {
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let store = Arc::new(
            Store::open_for_tenant(
                &directory.path().join("tenant.sqlite"),
                "tenant-a",
                Redactor::new(vec![]),
                1000,
            )
            .unwrap(),
        );
        let state = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "tenant-a".into(),
                store,
                runner_token: "r".repeat(48),
                viewer_token: "v".repeat(48),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap();
        let app = gateway_router(state.clone());
        let source: Vec<_> = (0..4)
            .map(|_| {
                state.tenants[0]
                    .io
                    .admit(store_io::Lane::Read, false)
                    .unwrap()
            })
            .collect();
        rejected_before_body_and_control_admitted(&app).await;
        drop(source);
        let shared: Vec<_> = (0..32)
            .map(|_| state.io.admit(store_io::Lane::Read, false).unwrap())
            .collect();
        rejected_before_body_and_control_admitted(&app).await;
        drop(shared);
    }
}
