//! Adopted local human source bundle. Discovery proposes; Store proves.
use super::*;
use crate::{
    audit::{
        HumanDiscoveryGeneration, HumanDiscoveryStale, HumanSourceAccess, HumanUsageFrameV1,
        HumanUsageStorePageV1, HumanWorkerFrameV1, HumanWorkerStorePageV1,
    },
    control::QuotaPolicyV1,
    control_fs::PrivateFile,
    human_authority::{
        HumanSourcesWriteEpoch, OwnedHumanSources, OwnedSourceToken,
        startup::HumanOwnedStartupBundle,
    },
    identity::*,
    tenant_store::TenantStore,
};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

/// Owned broker-supervision candidates and owned broker reads.
mod broker_candidates;
mod recent;

#[cfg(test)]
mod owned_policy_cli_tests;

const MAX_CANDIDATES: usize = 65_536;
const MAX_CANDIDATE_BYTES: usize = 16 * 1024 * 1024;
const CLOSED: usize = 1usize << (usize::BITS - 1);
const COMPLETE: usize = 0;
const REBUILD_REQUIRED: usize = 1;
const CLOSING: usize = 2;
const INVALID: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Candidate {
    Local(usize),
    /// Historical workload digest of its only retained source by index.
    Workload(usize),
    Technical,
}
#[derive(Default)]
struct CandidateIndex {
    entries: HashMap<[u8; 32], Vec<Candidate>>,
    count: usize,
    bytes: usize,
}
impl CandidateIndex {
    fn insert(&mut self, digest: [u8; 32], candidate: Candidate) -> anyhow::Result<()> {
        // Fixed-size accounting is a bound on retained representation, not a
        // measurement of allocator/process memory. Charge before allocation.
        let charge =
            std::mem::size_of::<([u8; 32], Candidate)>() + std::mem::size_of::<Vec<Candidate>>();
        let bytes = self
            .bytes
            .checked_add(charge)
            .ok_or_else(|| anyhow::anyhow!("human candidate capacity exceeded"))?;
        anyhow::ensure!(
            self.count < MAX_CANDIDATES && bytes <= MAX_CANDIDATE_BYTES,
            "human candidate capacity exceeded"
        );
        self.entries.entry(digest).or_default().push(candidate);
        self.count += 1;
        self.bytes = bytes;
        Ok(())
    }
    fn local_source(&self, digest: &[u8; 32]) -> anyhow::Result<Option<usize>> {
        match self.entries.get(digest).map(Vec::as_slice) {
            None => Ok(None),
            Some([Candidate::Local(source)]) => Ok(Some(*source)),
            Some([_]) => Ok(None),
            Some(_) => anyhow::bail!("human credential candidate is ambiguous"),
        }
    }
}

pub(super) struct OwnedHumanGateway {
    pub(super) owner: OwnedHumanSources,
    pub(super) sources: Vec<OwnedSourceToken>,
    discovery_generations: Vec<arc_swap::ArcSwap<HumanDiscoveryGeneration>>,
    candidates: CandidateIndex,
    pub(super) registry: Arc<auth::ReviewCredentialRegistry>,
    _authentication: Option<crate::audit::authentication::AuthenticationAudit>,
    oidc_selected: bool,
    coverage: AtomicUsize,
    usages: AtomicUsize,
    drained: Notify,
    human: Arc<human::HumanAccessState>,
}

/// Retains the actual bundle from construction until accepted work drains.
pub struct GatewayHumanRetirement {
    inner: Arc<OwnedHumanGateway>,
}
impl GatewayHumanRetirement {
    pub fn retire(&self) {
        self.inner.begin_close();
    }
    pub async fn drain(&self) -> anyhow::Result<()> {
        self.inner.drain().await
    }
}
impl Drop for GatewayHumanRetirement {
    fn drop(&mut self) {
        self.retire();
    }
}

/// Memory-only obligation acquired before queueing, retained by the actual
/// blocking closure/proof. It carries no business authorization.
pub(super) struct HumanOwnedUsage {
    bundle: Arc<OwnedHumanGateway>,
}
impl Drop for HumanOwnedUsage {
    fn drop(&mut self) {
        self.bundle.usages.fetch_sub(1, Ordering::AcqRel);
        self.bundle.drained.notify_waiters();
    }
}

pub(super) fn unavailable() -> (StatusCode, Json<Value>) {
    human::failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "identity_unavailable",
        "The selected human source coverage is unavailable.",
        "contact_operator",
    )
}
impl OwnedHumanGateway {
    pub(super) fn admit(self: &Arc<Self>) -> ApiResult<Arc<HumanOwnedUsage>> {
        self.check()?;
        self.usages
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < CLOSED - 1).then(|| current + 1)
            })
            .map_err(|_| unavailable())?;
        Ok(Arc::new(HumanOwnedUsage {
            bundle: self.clone(),
        }))
    }
    fn check(&self) -> ApiResult<()> {
        if self.coverage.load(Ordering::Acquire) != COMPLETE {
            return Err(unavailable());
        }
        Ok(())
    }
    pub(super) fn require_rebuild(&self) {
        self.coverage.store(REBUILD_REQUIRED, Ordering::Release);
        self.human.close_contexts();
    }
    pub(super) fn healthy(&self) -> bool {
        self.coverage.load(Ordering::Acquire) == COMPLETE && self.owner.admission_available()
    }
    pub(super) fn oidc_selected(&self) -> bool {
        self.oidc_selected
    }
    fn begin_close(&self) {
        self.coverage.store(CLOSING, Ordering::Release);
        self.usages.fetch_or(CLOSED, Ordering::AcqRel);
        self.human.close_contexts();
        self.owner.begin_close();
        self.drained.notify_waiters();
    }
    async fn drain(&self) -> anyhow::Result<()> {
        self.begin_close();
        loop {
            let notification = self.drained.notified();
            tokio::pin!(notification);
            notification.as_mut().enable();
            if self.usages.load(Ordering::Acquire) == CLOSED {
                break;
            }
            notification.await;
        }
        self.owner.drain().await?;
        Ok(())
    }
    pub(super) fn source(&self, digest: &[u8; 32]) -> ApiResult<Option<usize>> {
        self.check()?;
        self.candidates
            .local_source(digest)
            .map_err(|_| unavailable())
    }
    /// Complete startup collision coverage, not a substitute for the live
    /// registry, source generation or transaction-bound runner binding.
    pub(super) fn technical_candidate(&self, digest: &[u8; 32]) -> ApiResult<bool> {
        self.check()?;
        Ok(matches!(
            self.candidates.entries.get(digest).map(Vec::as_slice),
            Some([Candidate::Technical])
        ))
    }

    /// Read the expected generation while holding the owner's read or write
    /// epoch. The checked Store still compares it to the durable projection.
    pub(super) fn discovery_generation(&self, index: usize) -> Option<HumanDiscoveryGeneration> {
        self.discovery_generations
            .get(index)
            .map(|generation| **generation.load())
    }

    fn read<T>(
        &self,
        index: usize,
        store: &Store,
        work: impl FnOnce(&crate::audit::HumanSourceAccess<'_>) -> anyhow::Result<T>,
    ) -> ApiResult<T> {
        self.check()?;
        let source = self.sources.get(index).ok_or_else(unavailable)?;
        let epoch = self.owner.try_read_epoch().map_err(|error| {
            if matches!(
                error,
                crate::human_authority::HumanAuthorityFailure::OwnerInvalid
                    | crate::human_authority::HumanAuthorityFailure::StorageBindingMismatch
                    | crate::human_authority::HumanAuthorityFailure::ConfigurationMismatch
            ) {
                self.coverage.store(INVALID, Ordering::Release);
            }
            unavailable()
        })?;
        let access = HumanSourceAccess::read(&epoch, source).map_err(|_| unavailable())?;
        let expected = self.discovery_generation(index).ok_or_else(unavailable)?;
        if store
            .modern_discovery_generation(&access)
            .map_err(human::store_failure)?
            != expected
        {
            self.require_rebuild();
            return Err(unavailable());
        }
        let result = work(&access).map_err(human::store_failure)?;
        if store
            .modern_discovery_generation(&access)
            .map_err(human::store_failure)?
            != expected
        {
            self.require_rebuild();
            return Err(unavailable());
        }
        epoch.check().map_err(|_| unavailable())?;
        self.check()?;
        let _ = store; // Store is retained by the caller's genuine TenantState.
        Ok(result)
    }
    pub(super) fn authenticate(
        &self,
        index: usize,
        store: &Store,
        digest: &[u8; 32],
    ) -> ApiResult<Option<VerifiedLocalCredentialV1>> {
        if self.source(digest)? != Some(index) {
            return Err(unavailable());
        }
        let verified = self.read(index, store, |access| {
            store.modern_authenticate_local_digest(access, digest)
        })?;
        // A complete candidate was actually present at startup. Its removal
        // without an adopted writer makes discovery stale, not an absent key.
        if verified.is_none() {
            self.coverage.store(REBUILD_REQUIRED, Ordering::Release);
            return Err(unavailable());
        }
        Ok(verified)
    }
    pub(super) fn profile(
        &self,
        index: usize,
        store: &Store,
        principal: &PrincipalRefV1,
        redactor: &Redactor,
    ) -> ApiResult<crate::audit::HumanProfileSnapshotV1> {
        self.read(index, store, |access| {
            store.modern_human_profile(access, principal, redactor)
        })
    }
    pub(super) fn authorize(
        &self,
        index: usize,
        store: &Store,
        principal: &PrincipalRefV1,
        membership: &str,
        revision: u64,
        mode: ContextModeV1,
    ) -> ApiResult<AuthorizedScopeV1> {
        self.read(index, store, |access| {
            store.modern_authorize_human_context(access, principal, membership, revision, mode)
        })
    }
    pub(super) fn resources(
        &self,
        index: usize,
        store: &Store,
        scope: &AuthorizedScopeV1,
        after: &str,
        limit: usize,
        redactor: &Redactor,
    ) -> ApiResult<crate::audit::HumanStorePageV1<HumanResourceItemV1>> {
        self.read(index, store, |access| {
            store.modern_human_resources(access, scope, after, limit, redactor)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn human_workers(
        &self,
        index: usize,
        store: &Store,
        scope: &AuthorizedScopeV1,
        frame: &HumanWorkerFrameV1,
        after: Option<&str>,
        limit: usize,
        redactor: &Redactor,
    ) -> ApiResult<HumanWorkerStorePageV1> {
        self.read(index, store, |access| {
            store.modern_human_workers(access, scope, frame, after, limit, redactor)
        })
    }

    pub(super) fn change_membership(
        &self,
        index: usize,
        store: &Store,
        scope: &AuthorizedScopeV1,
        membership_id: &str,
        change: &HumanMembershipChangeV1,
    ) -> ApiResult<IdentityReceiptV1> {
        self.change_delegated_identity(
            index,
            human::membership_change_failure,
            |epoch, source, expected| {
                store.modern_human_change_membership(
                    epoch,
                    source,
                    scope,
                    membership_id,
                    change,
                    expected,
                )
            },
        )
    }

    pub(super) fn change_grant(
        &self,
        index: usize,
        store: &Store,
        scope: &AuthorizedScopeV1,
        change: &HumanGrantChangeV1,
    ) -> ApiResult<IdentityReceiptV1> {
        self.change_delegated_identity(
            index,
            human::grant_change_failure,
            |epoch, source, expected| {
                store.modern_human_change_grant(epoch, source, scope, change, expected)
            },
        )
    }

    /// Retain one nominative launch preparation on the proof's own retained
    /// source under its write epoch. Prepare creates no grant, reservation or
    /// claim and changes no authorization revision, so the discovery generation
    /// is unchanged. An exact replay returns the original receipt; a divergent
    /// intent under the same operation is a conflict; revocation closes it.
    pub(super) fn session_launch_prepare(
        &self,
        index: usize,
        store: &Store,
        scope: &AuthorizedScopeV1,
        request: &crate::control::session_control::SessionLaunchPrepareRequestV1,
    ) -> ApiResult<crate::control::session_control::SessionPrepareReceiptV1> {
        self.change_delegated_identity(
            index,
            human::session_prepare_failure,
            |epoch, source, expected| {
                store.modern_session_launch_prepare(epoch, source, scope, request, expected)
            },
        )
    }

    /// Read one retained preparation receipt for a still-authorized caller.
    pub(super) fn session_preparation(
        &self,
        index: usize,
        store: &Store,
        scope: &AuthorizedScopeV1,
        operation_id: &str,
    ) -> ApiResult<crate::control::session_control::SessionPrepareReceiptV1> {
        self.read(index, store, |access| {
            store.modern_session_preparation(access, scope, operation_id)
        })
    }

    /// Publish a committed delegated identity change on the proof's own retained
    /// source before releasing its write epoch. Replays keep the current generation.
    fn change_delegated_identity<R>(
        &self,
        index: usize,
        map_failure: fn(anyhow::Error) -> (StatusCode, Json<Value>),
        change: impl FnOnce(
            &HumanSourcesWriteEpoch,
            &OwnedSourceToken,
            HumanDiscoveryGeneration,
        ) -> anyhow::Result<(R, Option<HumanDiscoveryGeneration>)>,
    ) -> ApiResult<R> {
        self.check()?;
        let source = self.sources.get(index).ok_or_else(unavailable)?;
        let generation = self
            .discovery_generations
            .get(index)
            .ok_or_else(unavailable)?;
        let receipt = {
            let epoch = self.owner.try_write_epoch().map_err(|_| unavailable())?;
            self.check()?;
            epoch.validate_source(source).map_err(|_| unavailable())?;
            let expected = **generation.load();
            let result = change(&epoch, source, expected);
            if result
                .as_ref()
                .is_err_and(|error| error.is::<HumanDiscoveryStale>())
            {
                self.require_rebuild();
                return Err(unavailable());
            }
            epoch.validate_source(source).map_err(|_| unavailable())?;
            let (receipt, next) = result.map_err(map_failure)?;
            self.check()?;
            if let Some(next) = next {
                generation.store(Arc::new(next));
            }
            receipt
        };
        self.check()?;
        Ok(receipt)
    }
    pub(super) fn grant(
        &self,
        index: usize,
        store: &Store,
        scope: &AuthorizedScopeV1,
        frame: &crate::audit::HumanGrantFrameV1,
        resource: &ResourceRefV1,
        redactor: &Redactor,
    ) -> ApiResult<HumanGrantV1> {
        self.read(index, store, |access| {
            store.modern_human_grant(access, scope, frame, resource, redactor)
        })
    }
    /// The 360° summary of an owned source's context, read in its scoped
    /// transaction with the same receipt as the legacy profile.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn summary(
        &self,
        index: usize,
        store: &Store,
        scope: &AuthorizedScopeV1,
        frame: &crate::audit::HumanSummaryFrameV1,
        window: SummaryWindowV1,
        quota: &crate::control::QuotaPolicyV1,
        redactor: &Redactor,
    ) -> ApiResult<HumanSummaryV1> {
        self.read(index, store, |access| {
            store.modern_human_summary(access, scope, frame, window, quota, redactor)
        })
    }

    pub(super) fn quota(
        &self,
        index: usize,
        store: &Store,
        scope: &AuthorizedScopeV1,
        frame: &HumanUsageFrameV1,
        policy: &QuotaPolicyV1,
        redactor: &Redactor,
    ) -> ApiResult<HumanQuotaV1> {
        self.read(index, store, |access| {
            store.modern_human_quota(access, scope, frame, policy, redactor)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn usage(
        &self,
        index: usize,
        store: &Store,
        scope: &AuthorizedScopeV1,
        frame: &HumanUsageFrameV1,
        after: Option<&str>,
        limit: usize,
        redactor: &Redactor,
    ) -> ApiResult<HumanUsageStorePageV1> {
        self.read(index, store, |access| {
            store.modern_human_usage(access, scope, frame, after, limit, redactor)
        })
    }

    pub(super) fn memberships(
        &self,
        index: usize,
        store: &Store,
        scope: &AuthorizedScopeV1,
        frame: &crate::audit::HumanMembershipFrameV1,
        after: Option<&str>,
        limit: usize,
        redactor: &Redactor,
    ) -> ApiResult<crate::audit::HumanMembershipStorePageV1> {
        self.read(index, store, |access| {
            store.modern_human_memberships(access, scope, frame, after, limit, redactor)
        })
    }
    pub(super) fn configuration_facts(
        &self,
        index: usize,
        store: &Store,
        scope: &AuthorizedScopeV1,
        redactor: &Redactor,
    ) -> ApiResult<crate::audit::HumanConfigurationStoreFactsV1> {
        self.read(index, store, |access| {
            store.modern_human_configuration_facts(access, scope, redactor)
        })
    }

    pub(super) fn events(
        &self,
        index: usize,
        store: &Store,
        scope: &AuthorizedScopeV1,
        filter: &crate::audit::HumanEventFilterV1,
        after: i64,
        upper: Option<i64>,
        limit: usize,
        redactor: &Redactor,
    ) -> ApiResult<crate::audit::HumanEventPollV1> {
        self.read(index, store, |access| {
            store.modern_human_events(access, scope, filter, after, upper, limit, redactor)
        })
    }
}

impl GatewayState {
    #[cfg(test)]
    pub(crate) fn human_admission_closed_for_test(&self) -> bool {
        self.owned_human
            .as_ref()
            .is_some_and(|owned| owned.admit().is_err())
    }
    pub fn human_retirement(&self) -> Option<GatewayHumanRetirement> {
        self.owned_human
            .as_ref()
            .map(|inner| GatewayHumanRetirement {
                inner: inner.clone(),
            })
    }

    /// The complete already opened bundle and exact configured technical
    /// credentials enter together. No legacy opener/integrity/metadata scan.
    pub fn for_owned_human_sources(
        mut bundle: HumanOwnedStartupBundle,
        credentials: PrivateFile,
        tokens: Vec<(String, String, String)>,
        policy: Policy,
        jev: Option<JevBackend>,
        redactor: Redactor,
    ) -> anyhow::Result<Self> {
        let mut gateways = Vec::new();
        let mut sources = Vec::new();
        let mut ids = HashSet::new();
        for (tenant_id, runner_token, viewer_token) in tokens {
            anyhow::ensure!(ids.insert(tenant_id.clone()), "duplicate human tenant");
            let descriptor = bundle
                .owner
                .closure()
                .sources
                .iter()
                .find(|source| source.tenant_id.as_deref() == Some(&tenant_id))
                .ok_or_else(|| anyhow::anyhow!("human source absent"))?;
            let store = bundle
                .stores
                .remove(&descriptor.source_id)
                .ok_or_else(|| anyhow::anyhow!("retained human store absent"))?;
            sources.push(bundle.owner.source_token(descriptor.source_id)?);
            gateways.push(TenantGateway {
                tenant_id,
                runner_token,
                viewer_token,
                store: TenantStore::from(Arc::new(store)),
            });
        }
        anyhow::ensure!(
            !gateways.is_empty() && bundle.stores.is_empty(),
            "incomplete human tenant union"
        );
        let index = CredentialIndex::for_tenants(&gateways)?;
        let redactor = gateways.iter().fold(redactor, |redactor, tenant| {
            redactor.union(&Redactor::new(vec![
                tenant.runner_token.clone(),
                tenant.viewer_token.clone(),
            ]))
        });
        anyhow::ensure!(
            gateways
                .iter()
                .all(|gateway| gateway.store.owned_redaction_covers(&redactor)),
            "owned source redaction configuration differs"
        );
        if let Some(jev) = &jev {
            jev.validate_control_credentials(
                gateways.iter().flat_map(|tenant| {
                    [tenant.runner_token.as_str(), tenant.viewer_token.as_str()]
                }),
            )?;
            jev.validate_redaction(&redactor)?;
        }
        let io = Arc::new(store_io::Gate::new(true));
        let tenants = gateways
            .into_iter()
            .map(|gateway| TenantState::from_gateway(gateway, io.clone()))
            .collect::<Vec<_>>();
        let human = Arc::new(human::HumanAccessState::for_tenants(
            tenants.iter().map(|tenant| tenant.tenant_id.as_str()),
        )?);
        let registry = Arc::new(auth::ReviewCredentialRegistry::from_private_file(
            credentials,
            bundle.owner.clone(),
            &index,
            &tenants,
        )?);
        let mut candidates = CandidateIndex::default();
        let mut discovery_generations = Vec::new();
        for (i, tenant) in tenants.iter().enumerate() {
            let epoch = bundle.owner.try_read_epoch()?;
            let access = HumanSourceAccess::read(&epoch, &sources[i])?;
            let store = tenant.store.sqlite()?;
            let revision = store.modern_visit_candidates(&access, |digest, workload| {
                candidates.insert(
                    digest,
                    if workload {
                        Candidate::Workload(i)
                    } else {
                        Candidate::Local(i)
                    },
                )
            })?;
            discovery_generations.push(arc_swap::ArcSwap::from_pointee(revision));
            epoch.check()?;
        }
        for digest in index.digests() {
            candidates.insert(*digest, Candidate::Technical)?;
        }
        let oidc_selected = bundle.owner.closure().configuration.oidc.is_some();
        anyhow::ensure!(
            oidc_selected == bundle.authentication.is_some(),
            "authentication source selection differs"
        );
        let owned = Arc::new(OwnedHumanGateway {
            owner: bundle.owner.clone(),
            sources,
            discovery_generations,
            candidates,
            registry,
            _authentication: bundle.authentication.take(),
            oidc_selected,
            coverage: AtomicUsize::new(COMPLETE),
            usages: AtomicUsize::new(0),
            drained: Notify::new(),
            human: human.clone(),
        });
        bundle.transferred = true;
        Ok(Self {
            tenants: Arc::new(tenants),
            credential_index: Arc::new(index),
            valid_credentials: true,
            policy: Arc::new(policy),
            jev: jev.map(Arc::new),
            redactor: Arc::new(redactor),
            human,
            oidc: None,
            io,
            io_scope: Arc::default(),
            control_recovery_epoch: Arc::new(auth::ControlRecoveryAuthEpochV2::new()),
            owned_human: Some(owned),
            #[cfg(test)]
            owned_control_hook: Arc::default(),
        })
    }
}

#[cfg(test)]
mod journal_tests;
#[cfg(all(test, unix))]
mod owned_control_tests;
#[cfg(all(test, unix))]
mod owned_policy_inspection_cli_tests;
#[cfg(all(test, unix))]
mod owned_policy_inspection_regression_tests;
#[cfg(all(test, unix))]
mod owned_policy_inspection_tests;
#[cfg(all(test, unix))]
mod owned_policy_tests;
#[cfg(test)]
pub(super) mod tests;

const HUMAN_EVENT_DETAIL_PATH: &str = "/api/v1/human/events/{id}";
pub(super) const HUMAN_GRANT_CHANGE_PATH: &str = "/api/v1/human/admin/grants";
pub(super) const HUMAN_OWNED_GRANT_CHANGE_PATH: &str = "/api/v1/human/owned/admin/grants";
pub(super) const HUMAN_MEMBERSHIP_CHANGE_PATH: &str =
    "/api/v1/human/admin/memberships/{membership_id}";
pub(super) const HUMAN_OWNED_MEMBERSHIP_CHANGE_PATH: &str =
    "/api/v1/human/owned/admin/memberships/{membership_id}";

pub(super) fn membership_change_id<'a>(
    uri: &'a axum::http::Uri,
    template: &str,
) -> Option<&'a str> {
    if uri.query().is_some() {
        return None;
    }
    let prefix = template.strip_suffix("{membership_id}")?;
    uri.path()
        .strip_prefix(prefix)
        .filter(|id| crate::control::uuid(id).is_ok())
}

const OWNED_HUMAN_GET_PATHS: &[&str] = &[
    "/api/v1/human/admin/configuration",
    HUMAN_GRANT_CHANGE_PATH,
    "/api/v1/human/admin/memberships",
    "/api/v1/human/admin/quota",
    "/api/v1/human/admin/usage",
    "/api/v1/human/events",
    "/api/v1/human/events/recent",
    "/api/v1/human/identity",
    "/api/v1/human/owned/brokers",
    "/api/v1/human/resources",
    "/api/v1/human/summary",
    "/api/v1/human/workers",
];

// Exact existing route templates, reviewed in CONTRACT_W0_R2.
const REGISTERED_ROUTES: &[(&str, &[&str])] = &[
    ("/api/v1/egress/channel-openings", &["POST"]),
    ("/api/v1/egress/connect-permits", &["POST"]),
    ("/api/v1/egress/decisions", &["POST"]),
    ("/api/v1/egress/events", &["GET", "HEAD"]),
    ("/api/v1/egress/observations", &["POST"]),
    ("/api/v1/egress/renewals", &["POST"]),
    ("/api/v1/egress/policies/replace", &["POST"]),
    ("/api/v1/egress/policies/revoke", &["POST"]),
    ("/api/v1/egress/policies/inspect", &["POST"]),
    ("/api/agents", &["GET", "HEAD"]),
    ("/api/decide", &["POST"]),
    ("/api/events", &["GET", "POST", "HEAD"]),
    ("/api/export", &["GET", "HEAD"]),
    ("/api/identity", &["GET", "HEAD"]),
    ("/api/integrity", &["GET", "HEAD"]),
    ("/api/overview", &["GET", "HEAD"]),
    ("/api/quota", &["GET", "HEAD"]),
    ("/api/sandboxes", &["GET", "HEAD"]),
    ("/api/stream", &["GET", "HEAD"]),
    ("/api/usage", &["GET", "HEAD"]),
    ("/api/v1/agents", &["GET", "HEAD"]),
    ("/api/v1/brokers", &["GET", "HEAD"]),
    ("/api/v1/brokers/checkpoints", &["POST"]),
    ("/api/v1/brokers/checkpoints/self", &["GET", "HEAD"]),
    ("/api/v1/brokers/grants", &["POST"]),
    ("/api/v1/brokers/heartbeat", &["POST"]),
    ("/api/v1/brokers/identity", &["GET", "HEAD"]),
    (
        "/api/v1/brokers/kubernetes/held-create-protocol",
        &["GET", "HEAD"],
    ),
    ("/api/v1/brokers/kubernetes/held-creates", &["POST"]),
    (
        "/api/v1/brokers/kubernetes/held-creates/{create_id}/cancellations",
        &["POST"],
    ),
    (
        "/api/v1/brokers/kubernetes/held-creates/{create_id}/claims",
        &["POST"],
    ),
    (
        "/api/v1/brokers/kubernetes/held-creates/{create_id}/claims/{operation_id}",
        &["GET", "HEAD"],
    ),
    (
        "/api/v1/brokers/kubernetes/held-creates/{create_id}/recoveries",
        &["POST"],
    ),
    (
        "/api/v1/brokers/kubernetes/held-creates/{create_id}/recoveries/{operation_id}",
        &["GET", "HEAD"],
    ),
    (
        "/api/v1/brokers/kubernetes/held-creates/{create_id}/results",
        &["POST"],
    ),
    ("/api/v1/brokers/self", &["GET", "HEAD"]),
    ("/api/v1/brokers/{id}/revoke", &["POST"]),
    ("/api/v1/control/events", &["POST"]),
    ("/api/v1/control/protocol", &["GET", "HEAD"]),
    ("/api/v1/decide", &["POST"]),
    ("/api/v1/events", &["GET", "POST", "HEAD"]),
    ("/api/v1/events/batch", &["POST"]),
    ("/api/v1/export", &["GET", "HEAD"]),
    ("/api/v1/federation/workers", &["GET", "HEAD"]),
    ("/api/v1/human/admin/configuration", &["GET", "HEAD"]),
    // The S6 POST on this path still uses the existing mutation handler.
    (HUMAN_GRANT_CHANGE_PATH, &["GET", "HEAD"]),
    ("/api/v1/human/admin/memberships", &["GET", "HEAD"]),
    (HUMAN_MEMBERSHIP_CHANGE_PATH, &["POST"]),
    ("/api/v1/human/admin/quota", &["GET", "HEAD"]),
    ("/api/v1/human/admin/usage", &["GET", "HEAD"]),
    ("/api/v1/human/brokers", &["GET", "HEAD"]),
    ("/api/v1/human/events", &["GET", "HEAD"]),
    ("/api/v1/human/events/recent", &["GET", "HEAD"]),
    (HUMAN_EVENT_DETAIL_PATH, &["GET", "HEAD"]),
    ("/api/v1/human/export", &["GET", "HEAD"]),
    ("/api/v1/human/federation/workers", &["GET", "HEAD"]),
    ("/api/v1/human/identity", &["GET", "HEAD"]),
    (HUMAN_OWNED_GRANT_CHANGE_PATH, &["POST"]),
    (HUMAN_OWNED_MEMBERSHIP_CHANGE_PATH, &["POST"]),
    ("/api/v1/human/owned/brokers", &["GET", "HEAD"]),
    ("/api/v1/human/resources", &["GET", "HEAD"]),
    ("/api/v1/human/stream", &["GET", "HEAD"]),
    ("/api/v1/human/summary", &["GET", "HEAD"]),
    ("/api/v1/human/workers", &["GET", "HEAD"]),
    ("/api/v1/identity", &["GET", "HEAD"]),
    ("/api/v1/integrity", &["GET", "HEAD"]),
    (
        "/api/v1/kubernetes/session-reservations/{id}/control-recovery",
        &["POST"],
    ),
    ("/api/v1/overview", &["GET", "HEAD"]),
    ("/api/v1/owned/brokers/checkpoints", &["POST"]),
    ("/api/v1/owned/brokers/checkpoints/reconcile", &["POST"]),
    ("/api/v1/owned/brokers/checkpoints/self", &["GET", "HEAD"]),
    ("/api/v1/owned/brokers/heartbeats", &["POST"]),
    ("/api/v1/owned/brokers/heartbeats/reconcile", &["POST"]),
    ("/api/v1/owned/brokers/identity", &["GET", "HEAD"]),
    ("/api/v1/owned/brokers/self", &["GET", "HEAD"]),
    ("/api/v1/quota", &["GET", "HEAD"]),
    ("/api/v1/relay/channels/admissions", &["POST"]),
    ("/api/v1/relay/channels/{id}/claims", &["POST"]),
    ("/api/v1/relay/channels/{id}/reconcile", &["POST"]),
    ("/api/v1/relay/channels/{id}/transitions", &["POST"]),
    ("/api/v1/relay/grants", &["POST"]),
    ("/api/v1/relay/grants/{id}/revoke", &["POST"]),
    ("/api/v1/relay/messages", &["GET", "POST", "HEAD"]),
    ("/api/v1/relay/messages/{id}/ack", &["POST"]),
    ("/api/v1/relay/messages/{id}/prune", &["POST"]),
    ("/api/v1/relay/retention", &["GET", "HEAD"]),
    ("/api/v1/sandboxes", &["GET", "HEAD"]),
    ("/api/v1/status", &["GET", "HEAD"]),
    ("/api/v1/stream", &["GET", "HEAD"]),
    ("/api/v1/usage", &["GET", "HEAD"]),
    ("/api/v1/workers", &["GET", "HEAD"]),
    ("/api/v1/workers/operations/{id}", &["GET", "HEAD"]),
    ("/api/v1/workers/reservations", &["POST"]),
    ("/api/v1/workers/supervised-reservations", &["POST"]),
    ("/api/v1/workers/{id}", &["GET", "HEAD"]),
    ("/api/v1/workers/{id}/armed-start-claims", &["POST"]),
    ("/api/v1/workers/{id}/arms", &["POST"]),
    ("/api/v1/workers/{id}/arms/{arm_id}", &["GET", "HEAD"]),
    ("/api/v1/workers/{id}/export-requests", &["POST"]),
    ("/api/v1/workers/{id}/helper-cleanup", &["POST"]),
    ("/api/v1/workers/{id}/helper-observations", &["POST"]),
    ("/api/v1/workers/{id}/helper-start-claims", &["POST"]),
    ("/api/v1/workers/{id}/transitions", &["POST"]),
    ("/api/v1/workload-bindings", &["POST"]),
    ("/api/v1/workload-bindings/{id}/revoke", &["POST"]),
    ("/api/workers", &["GET", "HEAD"]),
    ("/api/workers/operations/{id}", &["GET", "HEAD"]),
    ("/api/workers/reservations", &["POST"]),
    ("/api/workers/{id}", &["GET", "HEAD"]),
    ("/api/workers/{id}/transitions", &["POST"]),
    ("/auth/oidc/admit", &["POST"]),
    ("/auth/oidc/callback", &["GET", "HEAD"]),
    ("/auth/oidc/cli-profile", &["GET", "HEAD"]),
    ("/auth/oidc/logout", &["POST"]),
    ("/auth/oidc/native-intent", &["POST"]),
    ("/auth/oidc/refresh", &["POST"]),
    ("/auth/oidc/rotate", &["POST"]),
    ("/auth/oidc/start", &["POST"]),
    ("/auth/oidc/status", &["GET", "HEAD"]),
    ("/auth/oidc/tokens", &["POST"]),
    ("/healthz", &["GET", "HEAD", "HEAD"]),
];

/// Runs only on a matched existing route, before extractors or body polling.
pub(super) async fn route_gate(
    State(state): State<GatewayState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(owned) = &state.owned_human else {
        return next.run(request).await;
    };
    let template = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|path| path.as_str());
    let known =
        template.and_then(|template| REGISTERED_ROUTES.iter().find(|(path, _)| *path == template));
    if !known.is_some_and(|(_, methods)| methods.contains(&request.method().as_str())) {
        return next.run(request).await;
    }
    if let Some(membership_template) = template.filter(|path| {
        request.method() == "POST"
            && matches!(
                *path,
                HUMAN_MEMBERSHIP_CHANGE_PATH | HUMAN_OWNED_MEMBERSHIP_CHANGE_PATH
            )
    }) {
        if membership_change_id(request.uri(), membership_template).is_none() {
            return human::invalid_membership_request().into_response();
        }
        return next.run(request).await;
    }
    if template == Some(HUMAN_OWNED_GRANT_CHANGE_PATH) && request.method() == "POST" {
        if request.uri().query().is_some() {
            return human::invalid_grant_request().into_response();
        }
        return next.run(request).await;
    }
    let path = request.uri().path();
    let allowed = (["/healthz", "/api/v1/status"].contains(&path)
        && (request.method() == "GET" || request.method() == "HEAD"))
        || super::control_transport::owned_protocol_request(&request)
        || super::control_transport::owned_broker_request(&request)
        || super::control_transport::owned_policy_request(&request)
        || super::control_transport::owned_policy_inspection_request(&request)
        || request.method() == "GET"
            && (OWNED_HUMAN_GET_PATHS.contains(&path) || template == Some(HUMAN_EVENT_DETAIL_PATH))
        || request.method() == "GET"
            && !owned.oidc_selected()
            && ["/auth/oidc/status", "/auth/oidc/cli-profile"].contains(&path);
    if allowed {
        next.run(request).await
    } else {
        unavailable().into_response()
    }
}
