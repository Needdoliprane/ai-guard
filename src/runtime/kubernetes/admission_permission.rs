//! One-shot local claim and emission capability. No production activation factory.
use super::{
    PreparedHeldCreate,
    journal::{AdmissionJournal, EmissionCheckpoint},
};
use crate::{
    audit::{FreshKubernetesCreateClaimCommit, KubernetesCreateAuthorization, Store},
    broker::clock::{HostClock, NativeHostClock},
    broker_registry::BrokerIdentityV1,
    control::{HostClockSampleV1, kubernetes_admission::*},
    runtime::kubernetes::{
        KubernetesAuthorityV1, KubernetesCredentials, KubernetesError, KubernetesResult,
    },
};
use std::sync::{Arc, atomic::AtomicBool};

pub(crate) struct HeldCreateActivation {
    intent_hash: String,
    authority_fingerprint: String,
    #[cfg(test)]
    owned_listener: std::net::TcpListener,
}
impl HeldCreateActivation {
    #[cfg(test)]
    pub(crate) fn for_test(
        intent: &KubernetesCreateIntentV1,
        listener: &std::net::TcpListener,
    ) -> KubernetesResult<Self> {
        intent
            .validate()
            .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?;
        let activation = Self {
            intent_hash: intent
                .intent_hash()
                .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?,
            authority_fingerprint: intent.authority.fingerprint(),
            owned_listener: listener
                .try_clone()
                .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?,
        };
        activation.revalidate(intent)?;
        Ok(activation)
    }
    fn revalidate(&self, intent: &KubernetesCreateIntentV1) -> KubernetesResult<()> {
        if self.intent_hash
            != intent
                .intent_hash()
                .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?
            || self.authority_fingerprint != intent.authority.fingerprint()
        {
            return Err(KubernetesError::HeldCreateAuthorizationDenied);
        }
        #[cfg(test)]
        {
            let endpoint = reqwest::Url::parse(&intent.authority.endpoint)
                .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?;
            let ip = endpoint
                .host_str()
                .and_then(|host| {
                    host.trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .ok()
                })
                .filter(std::net::IpAddr::is_loopback)
                .ok_or(KubernetesError::HeldCreateAuthorizationDenied)?;
            let port = endpoint
                .port_or_known_default()
                .ok_or(KubernetesError::HeldCreateAuthorizationDenied)?;
            if self
                .owned_listener
                .local_addr()
                .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?
                != std::net::SocketAddr::new(ip, port)
            {
                return Err(KubernetesError::HeldCreateAuthorizationDenied);
            }
        }
        Ok(())
    }
}

pub(crate) struct PendingHeldCreateClaim {
    activation: HeldCreateActivation,
    clock: Arc<dyn HostClock>,
    before_claim: HostClockSampleV1,
    intent: KubernetesCreateIntentV1,
    prepared: PreparedHeldCreate,
}
pub(crate) enum HeldCreateClaimOutcome {
    Ready {
        response: KubernetesCreateClaimResponseV1,
        permission: FreshKubernetesCreatePermission,
    },
    ObserveOnly {
        response: KubernetesCreateClaimResponseV1,
    },
}
pub(crate) struct FreshKubernetesCreatePermission {
    activation: HeldCreateActivation,
    clock: Arc<dyn HostClock>,
    before_claim: HostClockSampleV1,
    fresh: FreshKubernetesCreateClaimCommit,
    checkpoint: EmissionCheckpoint,
    intent: KubernetesCreateIntentV1,
    prepared: PreparedHeldCreate,
}
impl PendingHeldCreateClaim {
    pub(crate) fn intent(&self) -> &KubernetesCreateIntentV1 {
        &self.intent
    }
    pub(crate) fn begin(
        activation: HeldCreateActivation,
        intent: &KubernetesCreateIntentV1,
    ) -> KubernetesResult<Self> {
        let clock = Arc::new(
            NativeHostClock::new().map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?,
        );
        Self::begin_with_clock(activation, clock, intent)
    }
    #[cfg(test)]
    pub(crate) fn begin_for_test(
        activation: HeldCreateActivation,
        clock: Arc<dyn HostClock>,
        intent: &KubernetesCreateIntentV1,
    ) -> KubernetesResult<Self> {
        Self::begin_with_clock(activation, clock, intent)
    }
    fn begin_with_clock(
        activation: HeldCreateActivation,
        clock: Arc<dyn HostClock>,
        intent: &KubernetesCreateIntentV1,
    ) -> KubernetesResult<Self> {
        let prepared = super::prepare(intent)?;
        activation.revalidate(intent)?;
        let before_claim = clock
            .sample()
            .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?;
        before_claim
            .validate()
            .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?;
        Ok(Self {
            activation,
            clock,
            before_claim,
            intent: intent.clone(),
            prepared,
        })
    }
    pub(crate) fn claim(
        self,
        store: &Store,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesCreateAuthorization,
        claim: &KubernetesCreateClaimV1,
        journal: &mut AdmissionJournal,
    ) -> KubernetesResult<HeldCreateClaimOutcome> {
        self.activation.revalidate(&self.intent)?;
        claim
            .validate()
            .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?;
        if journal.intent() != &self.intent
            || journal.target() != self.prepared.target()
            || journal.claim_request() != claim
            || journal.tenant_id() != store.tenant_id()
            || identity.tenant_id() != store.tenant_id()
            || claim.create_id != self.intent.create_id
            || claim.intent_hash != self.activation.intent_hash
        {
            return Err(KubernetesError::HeldCreateAuthorizationDenied);
        }
        // Never consume a central claim after losing the exact prepared replay
        // identity on disk. The retained preclaim sample is not renewed here.
        journal
            .revalidate_retained()
            .map_err(|_| KubernetesError::HeldCreateJournalUnavailable)?;
        let (response, fresh) = if journal.observation_only() {
            (
                store
                    .observe_kubernetes_create_claim(identity, authorization, claim)
                    .map_err(control_error)?,
                None,
            )
        } else {
            store
                .claim_kubernetes_create(identity, authorization, claim)
                .map_err(control_error)?
        };
        response
            .receipt
            .validate()
            .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?;
        if response.version != 1
            || response.snapshot.intent != self.intent
            || response.snapshot.target != *self.prepared.target()
            || response.snapshot.claim.as_ref() != Some(claim)
        {
            return Err(KubernetesError::HeldCreateAuthorizationDenied);
        }
        let Some(fresh) = fresh else {
            if response.disposition != KubernetesCreateDispositionV1::ObserveOnly {
                return Err(KubernetesError::HeldCreateAuthorizationDenied);
            }
            journal
                .retain_observed_claim(store.tenant_id(), &response)
                .map_err(|_| KubernetesError::HeldCreateJournalUnavailable)?;
            return Ok(HeldCreateClaimOutcome::ObserveOnly { response });
        };
        if response.disposition != KubernetesCreateDispositionV1::EmitOnce
            || fresh.tenant_id() != store.tenant_id()
            || fresh.claim() != claim
            || fresh.receipt() != &response.receipt
            || fresh.snapshot() != &response.snapshot
            || response.snapshot.state != KubernetesCreateStateV1::EmissionPossible
        {
            return Err(KubernetesError::HeldCreateAuthorizationDenied);
        }
        let checkpoint = journal
            .commit_emission_possible(&fresh)
            .map_err(|_| KubernetesError::HeldCreateJournalUnavailable)?;
        Ok(HeldCreateClaimOutcome::Ready {
            response,
            permission: FreshKubernetesCreatePermission {
                activation: self.activation,
                clock: self.clock,
                before_claim: self.before_claim,
                fresh,
                checkpoint,
                intent: self.intent,
                prepared: self.prepared,
            },
        })
    }
}
impl FreshKubernetesCreatePermission {
    pub(crate) fn intent(&self) -> &KubernetesCreateIntentV1 {
        &self.intent
    }
    pub(crate) fn target(&self) -> &KubernetesCreateTargetV1 {
        self.prepared.target()
    }
    pub(crate) fn pod_bytes(&self) -> &[u8] {
        self.prepared.pod_bytes()
    }
    pub(crate) fn check_before_create(
        self,
        binding: &KubernetesAuthorityV1,
        body: &[u8],
        journal: &AdmissionJournal,
        cancelled: &AtomicBool,
        credentials: &KubernetesCredentials,
    ) -> KubernetesResult<()> {
        journal
            .revalidate_emission(&self.checkpoint)
            .map_err(|_| KubernetesError::HeldCreateJournalUnavailable)?;
        credentials.revalidate()?;
        self.activation.revalidate(&self.intent)?;
        if binding != &self.intent.authority
            || body != self.prepared.pod_bytes()
            || journal.intent() != &self.intent
            || journal.target() != self.prepared.target()
            || self.fresh.tenant_id() != journal.tenant_id()
            || journal.claim_request() != self.fresh.claim()
        {
            return Err(KubernetesError::HeldCreateAuthorizationDenied);
        }
        let receipt = self.fresh.receipt();
        receipt
            .validate()
            .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?;
        let window = receipt
            .emission_window
            .as_ref()
            .ok_or(KubernetesError::HeldCreateAuthorizationDenied)?;
        let issued = crate::broker_registry::parse_time(&window.issued_at)
            .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?;
        let end = crate::broker_registry::parse_time(&window.not_after)
            .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?;
        let budget = u128::try_from(
            (end - issued)
                .num_nanoseconds()
                .ok_or(KubernetesError::HeldCreateAuthorizationDenied)?,
        )
        .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?;
        if budget == 0 || budget > 8_000_000_000 {
            return Err(KubernetesError::HeldCreateAuthorizationDenied);
        }
        // All filesystem and preparation work precedes this last producer sample.
        // elapsed starts before the actual control call, never after its ACK.
        let current = self
            .clock
            .sample()
            .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?;
        current
            .validate()
            .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?;
        let elapsed = current
            .continuous_ns()
            .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?
            .checked_sub(
                self.before_claim
                    .continuous_ns()
                    .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?,
            )
            .ok_or(KubernetesError::HeldCreateAuthorizationDenied)?;
        let utc = chrono::Utc::now();
        if current.boot_id != self.before_claim.boot_id
            || current.method != self.before_claim.method
            || elapsed >= budget
            || utc < issued
            || utc >= end
            || cancelled.load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(KubernetesError::HeldCreateAuthorizationDenied);
        }
        Ok(())
    }
}

fn control_error(error: anyhow::Error) -> KubernetesError {
    use KubernetesCreateFailureV1::*;
    match error.downcast_ref::<KubernetesCreateFailureV1>() {
        Some(
            InvalidRequest | Conflict | PermissionDenied | SupervisionInactive | MigrationRequired
            | NotFound | QuotaExceeded,
        ) => KubernetesError::HeldCreateAuthorizationDenied,
        Some(Unavailable) | None => KubernetesError::HeldCreateControlUnavailable,
    }
}

#[cfg(test)]
#[path = "admission_permission_tests.rs"]
mod tests;
