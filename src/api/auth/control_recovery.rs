//! Request-local authority for the control's no-effect recovery path.
use super::super::{ApiResult, ControlProof, GatewayState, Store, TenantState, error, store_io};
use axum::{
    Json,
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

const RETIRED: usize = 1usize << (usize::BITS - 1);
const MAX_PROOF_LIFETIME: Duration = Duration::from_secs(60);

/// One irreversible gate, shared by every clone and scoped GatewayState.
pub(in crate::api) struct ControlRecoveryAuthEpochV2 {
    id: Uuid,
    // The top bit closes admission; remaining bits count live usage leases.
    leases: AtomicUsize,
}

impl ControlRecoveryAuthEpochV2 {
    pub(in crate::api) fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            leases: AtomicUsize::new(0),
        }
    }

    fn retired(&self) -> bool {
        self.leases.load(Ordering::Acquire) & RETIRED != 0
    }

    fn acquire(&self) -> anyhow::Result<()> {
        self.leases
            .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < RETIRED - 1).then(|| current + 1)
            })
            .map(|_| ())
            .map_err(|_| anyhow::anyhow!("control recovery authentication unavailable"))
    }

    fn retire(&self) {
        self.leases.fetch_or(RETIRED, Ordering::AcqRel);
    }

    fn drained(&self) -> bool {
        self.leases.load(Ordering::Acquire) == RETIRED
    }
}

/// Own this handle before binding a listener. Retirement is irreversible;
/// completion requires both serving owners to stop and `drained()` to be true.
pub struct GatewayControlRecoveryRetirementV2 {
    epoch: Arc<ControlRecoveryAuthEpochV2>,
}

impl GatewayControlRecoveryRetirementV2 {
    pub fn retire(&self) {
        self.epoch.retire();
    }
    pub fn drained(&self) -> bool {
        self.epoch.drained()
    }
}

impl Drop for GatewayControlRecoveryRetirementV2 {
    fn drop(&mut self) {
        self.retire();
    }
}

impl GatewayState {
    pub fn control_recovery_retirement(&self) -> GatewayControlRecoveryRetirementV2 {
        GatewayControlRecoveryRetirementV2 {
            epoch: Arc::clone(&self.control_recovery_epoch),
        }
    }
}

/// No Clone, serde, free tenant/Store constructor, or credential-bearing Debug.
pub(crate) struct AuthenticatedControlRecoveryV2 {
    store: Arc<Store>,
    tenant_id: String,
    proof: Arc<ControlProof>,
    // authorize_sync deliberately strips scopes from returned ControlProof.
    // Retain the exact admitted scope separately for every later real reauth.
    scope: Arc<store_io::Scope>,
    epoch: Arc<ControlRecoveryAuthEpochV2>,
    request_auth_id: Uuid,
    authenticated_at: String,
    authenticated_utc: DateTime<Utc>,
    expires_utc: DateTime<Utc>,
    created: Instant,
    deadline: Instant,
}

/// Borrowing the proof prevents a lease from escaping its blocking segment.
/// Core keeps this named guard alive through its authorized transaction commit.
pub(crate) struct ControlRecoveryUsageV2<'auth> {
    auth: &'auth AuthenticatedControlRecoveryV2,
}

impl Drop for ControlRecoveryUsageV2<'_> {
    fn drop(&mut self) {
        self.auth.epoch.leases.fetch_sub(1, Ordering::AcqRel);
    }
}

fn denied() -> (StatusCode, Json<Value>) {
    error(
        StatusCode::FORBIDDEN,
        "Control recovery authentication is unavailable.",
    )
}

fn same_runner(
    proof: &ControlProof,
    scope: &Arc<store_io::Scope>,
    expected_store: &Arc<Store>,
    expected_tenant: &str,
    epoch: &Arc<ControlRecoveryAuthEpochV2>,
) -> ApiResult<TenantState> {
    if !proof.write
        || proof.lane != store_io::Lane::Control
        || !Arc::ptr_eq(&proof.state.control_recovery_epoch, epoch)
        || epoch.retired()
    {
        return Err(denied());
    }
    let (tenant, role) = proof
        .state
        .scoped(Arc::clone(scope))
        .authorize_sync(&proof.headers, true)?;
    if role != "runner"
        || tenant.tenant_id != expected_tenant
        || !Arc::ptr_eq(tenant.store.sqlite_api()?, expected_store)
        || epoch.retired()
    {
        return Err(denied());
    }
    Ok(tenant)
}

impl AuthenticatedControlRecoveryV2 {
    fn lifetime_at(&self, utc: DateTime<Utc>, monotone: Instant) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.authenticated_utc <= utc
                && utc < self.expires_utc
                && self.created <= monotone
                && monotone < self.deadline
                && self.deadline.saturating_duration_since(self.created) <= MAX_PROOF_LIFETIME,
            "control recovery authentication unavailable"
        );
        Ok(())
    }

    pub(crate) fn acquire_usage<'auth>(
        &'auth self,
        store: &Store,
    ) -> anyhow::Result<ControlRecoveryUsageV2<'auth>> {
        anyhow::ensure!(
            std::ptr::eq(store, Arc::as_ref(&self.store)),
            "control recovery authentication unavailable"
        );
        // Collision scans perform real Store I/O. The caller has not locked
        // SQLite, and this proof retains the worker's exact admission scope.
        self.lifetime_at(Utc::now(), Instant::now())?;
        same_runner(
            &self.proof,
            &self.scope,
            &self.store,
            &self.tenant_id,
            &self.epoch,
        )
        .map_err(|_| anyhow::anyhow!("control recovery authentication unavailable"))?;
        self.lifetime_at(Utc::now(), Instant::now())?;
        self.epoch.acquire()?;
        Ok(ControlRecoveryUsageV2 { auth: self })
    }

    pub(crate) fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    pub(crate) fn auth_epoch_id(&self) -> Uuid {
        self.epoch.id
    }
    pub(crate) fn request_auth_id(&self) -> Uuid {
        self.request_auth_id
    }
    pub(crate) fn authenticated_at(&self) -> &str {
        &self.authenticated_at
    }
    pub(crate) fn monotone_deadline(&self) -> Instant {
        self.deadline
    }
}

/// Must precede Path and Request/body extraction in the protected handler.
pub(in crate::api) struct VerifiedControlRecoveryRunnerV2 {
    tenant: TenantState,
    proof: Arc<ControlProof>,
}

impl FromRequestParts<GatewayState> for VerifiedControlRecoveryRunnerV2 {
    type Rejection = (StatusCode, Json<Value>);

    async fn from_request_parts(parts: &mut Parts, state: &GatewayState) -> ApiResult<Self> {
        let (tenant, role) = state.authorize(&parts.headers, true).await?;
        if role != "runner" {
            return Err(denied());
        }
        let proof = tenant.control_proof.as_ref().cloned().ok_or_else(denied)?;
        Ok(Self { tenant, proof })
    }
}

impl VerifiedControlRecoveryRunnerV2 {
    fn into_authenticated(
        self,
        scope: Arc<store_io::Scope>,
    ) -> ApiResult<(AuthenticatedControlRecoveryV2, TenantState)> {
        let epoch = Arc::clone(&self.proof.state.control_recovery_epoch);
        let tenant = same_runner(
            &self.proof,
            &scope,
            self.tenant.store.sqlite_api()?,
            &self.tenant.tenant_id,
            &epoch,
        )?;
        let created = Instant::now();
        let authenticated_utc = Utc::now();
        let auth = AuthenticatedControlRecoveryV2 {
            store: Arc::clone(tenant.store.sqlite_api()?),
            tenant_id: tenant.tenant_id.clone(),
            proof: self.proof,
            scope,
            epoch,
            request_auth_id: Uuid::new_v4(),
            authenticated_at: authenticated_utc.to_rfc3339_opts(SecondsFormat::Millis, true),
            authenticated_utc,
            expires_utc: authenticated_utc + chrono::Duration::seconds(60),
            created,
            deadline: created + MAX_PROOF_LIFETIME,
        };
        if auth.epoch.retired() {
            return Err(denied());
        }
        Ok((auth, tenant))
    }

    pub(in crate::api) async fn blocking<T, F>(self, operation: F) -> ApiResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&AuthenticatedControlRecoveryV2, TenantState) -> ApiResult<T> + Send + 'static,
    {
        let mut scope = store_io::Scope::default();
        scope.hold(&self.tenant, store_io::Lane::Control, false)?;
        let scope = Arc::new(scope);
        let shared = Arc::clone(&self.tenant.shared_io);
        store_io::run(shared, None, store_io::Lane::Control, false, move || {
            let (auth, tenant) = self.into_authenticated(scope)?;
            // A retirement racing issuance is rejected even before a Store
            // issuer asks for its own lease. No capability escapes T:'static.
            operation(&auth, tenant)
        })
        .await
    }
}

/// Test seam uses the production extractor and blocking factory. The selected
/// Store is returned by real authentication, never accepted as caller input.
#[cfg(test)]
pub(crate) async fn with_authenticated_control_recovery_for_test<T, F>(
    state: &GatewayState,
    headers: axum::http::HeaderMap,
    operation: F,
) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&AuthenticatedControlRecoveryV2, Arc<Store>) -> anyhow::Result<T> + Send + 'static,
{
    let (mut parts, _) = axum::http::Request::new(axum::body::Body::empty()).into_parts();
    parts.headers = headers;
    let verified = VerifiedControlRecoveryRunnerV2::from_request_parts(&mut parts, state).await?;
    verified
        .blocking(move |auth, tenant| {
            operation(auth, tenant.store.sqlite_api()?.clone()).map_err(super::super::unavailable)
        })
        .await
}

/// Two independent HTTP authentications to one selected Store, for proving
/// that Core capabilities cannot be used by another real request context.
#[cfg(test)]
pub(crate) async fn with_control_recovery_auth_pair_for_test<T, F>(
    state: &GatewayState,
    headers: axum::http::HeaderMap,
    operation: F,
) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(
            &AuthenticatedControlRecoveryV2,
            &AuthenticatedControlRecoveryV2,
            Arc<Store>,
        ) -> anyhow::Result<T>
        + Send
        + 'static,
{
    let (mut parts, _) = axum::http::Request::new(axum::body::Body::empty()).into_parts();
    parts.headers = headers;
    let first = VerifiedControlRecoveryRunnerV2::from_request_parts(&mut parts, state).await?;
    let second = VerifiedControlRecoveryRunnerV2::from_request_parts(&mut parts, state).await?;
    first
        .blocking(move |auth, tenant| {
            let (other, selected) = second.into_authenticated(Arc::clone(&auth.scope))?;
            if !tenant.store.ptr_eq(&selected.store) {
                return Err(denied());
            }
            operation(auth, &other, tenant.store.sqlite_api()?.clone())
                .map_err(super::super::unavailable)
        })
        .await
}

#[cfg(test)]
#[path = "../control_recovery_auth_tests.rs"]
mod tests;
