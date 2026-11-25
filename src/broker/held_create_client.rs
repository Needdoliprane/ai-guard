//! Broker-only held-CREATE control client. HTTP ACKs are not Store proofs.
use crate::{
    broker::clock::{HostClock, NativeHostClock},
    broker_registry::BrokerIdentityViewV1,
    control::{HostClockSampleV1, kubernetes_admission::*},
    control_fs::PrivateFile,
    redaction::Redactor,
    runtime::kubernetes::admission::journal::AdmissionJournal,
};
use std::{
    fmt,
    sync::{Arc, Mutex},
};
use tokio::sync::Semaphore;

#[path = "held_create_client_http.rs"]
mod http;
#[path = "kubernetes_session_client.rs"]
pub(crate) mod sessions;
#[path = "held_create_client_validation.rs"]
mod validation;
#[cfg(test)]
use http::MaterialTestGate;

pub(crate) type ClientResult<T> = Result<T, HeldCreateClientError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeldCreateClientError {
    InvalidConfiguration,
    UnsafeFilesystem,
    InvalidContext,
    InvalidRequest,
    /// HTTP400 received from the control after validating its response framing.
    ControlRejected,
    Authentication,
    Forbidden,
    NotFound,
    Conflict,
    StaleEpoch,
    StaleRevision,
    Capacity,
    Clock,
    Protocol,
    Transport,
    Unavailable,
}
impl fmt::Display for HeldCreateClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "held-create broker control configuration is invalid",
            Self::UnsafeFilesystem => {
                "held-create broker control material changed or is unavailable"
            }
            Self::InvalidContext => {
                "held-create broker control scope differs from the retained request"
            }
            Self::InvalidRequest => "held-create broker control request is invalid",
            Self::ControlRejected => "held-create broker control rejected the request",
            Self::Authentication => "held-create broker authentication was refused",
            Self::Forbidden => "held-create broker scope or policy was refused",
            Self::NotFound => "held-create broker operation was not found",
            Self::Conflict => "held-create broker operation conflicts with durable state",
            Self::StaleEpoch => "held-create result precedes the current epoch",
            Self::StaleRevision => "held-create result precedes the current revision",
            Self::Capacity => "held-create control capacity is unavailable",
            Self::Clock => "held-create control clock or emission window is invalid",
            Self::Protocol => "held-create broker control response failed validation",
            Self::Transport => "held-create broker control transport is unavailable",
            Self::Unavailable => "held-create broker control is unavailable",
        })
    }
}
impl std::error::Error for HeldCreateClientError {}

/// Explicit expectations only; this descriptive scope grants no permission.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct HeldCreateClientScope {
    pub(crate) gateway: String,
    pub(crate) tenant_id: String,
    pub(crate) control_namespace_uuid: String,
    pub(crate) broker_id: String,
    pub(crate) credential_id: String,
    pub(crate) incarnation_id: String,
}

pub(crate) struct HeldCreateControlClient {
    scope: HeldCreateClientScope,
    token: PrivateFile,
    ca: Option<PrivateFile>,
    redactor: Redactor,
    http: reqwest::Client,
    origin: reqwest::Url,
    authorization: reqwest::header::HeaderValue,
    material_capacity: Arc<Semaphore>,
    revisions: Mutex<(u64, u64)>,
    #[cfg(test)]
    material_gate: Mutex<Option<MaterialTestGate>>,
}
impl HeldCreateControlClient {
    pub(crate) fn new(
        scope: HeldCreateClientScope,
        token: PrivateFile,
        ca: Option<PrivateFile>,
        known_secrets: Vec<String>,
    ) -> ClientResult<Self> {
        http::prepare(scope, token, ca, known_secrets)
    }

    pub(crate) fn expected_scope(&self) -> &HeldCreateClientScope {
        &self.scope
    }

    // Closed held-CREATE client surface; production wiring pending.
    #[allow(dead_code)]
    pub(crate) async fn protocol(&self) -> ClientResult<KubernetesHeldCreateProtocolV1> {
        self.handshake().await.map(|(_, protocol)| protocol)
    }

    // Closed held-CREATE client surface; production wiring pending.
    #[allow(dead_code)]
    pub(crate) async fn reserve(
        &self,
        intent: &KubernetesCreateIntentV1,
    ) -> ClientResult<KubernetesCreateMutationResponseV1> {
        validation::intent(&self.scope, intent)?;
        let bytes = self.request_bytes(intent)?;
        self.handshake().await?;
        let (status, response) = self
            .exchange("/api/v1/brokers/kubernetes/held-creates", Some(bytes))
            .await?;
        validation::reservation(&self.scope, intent, &response, status)?;
        Ok(response)
    }

    // Closed held-CREATE client surface; production wiring pending.
    #[allow(dead_code)]
    pub(crate) async fn claim_once(
        &self,
        attempt: HeldCreateClaimAttempt,
    ) -> ClientResult<HeldCreateClaimExchange> {
        validation::intent(&self.scope, &attempt.intent)?;
        if attempt.tenant_id != self.scope.tenant_id {
            return Err(HeldCreateClientError::InvalidContext);
        }
        let bytes = self.request_bytes(&attempt.claim)?;
        if !validation::origin_live(&attempt.clock, &attempt.before_claim) {
            return Err(HeldCreateClientError::Clock);
        }
        let before = self.handshake().await?;
        let (status, response): (_, KubernetesCreateClaimResponseV1) = self
            .exchange(
                &format!(
                    "/api/v1/brokers/kubernetes/held-creates/{}/claims",
                    attempt.claim.create_id
                ),
                Some(bytes),
            )
            .await?;
        validation::claim(
            &self.scope,
            &response,
            &attempt.claim.create_id,
            &attempt.claim.operation_id,
            Some((&attempt.intent, &attempt.claim, &attempt.target)),
        )?;
        if (status == reqwest::StatusCode::CREATED)
            != (response.disposition == KubernetesCreateDispositionV1::EmitOnce)
        {
            return Err(HeldCreateClientError::Protocol);
        }
        let after = self.handshake().await;
        let eligible = status == reqwest::StatusCode::CREATED
            && after.as_ref().is_ok_and(|after| after == &before)
            && validation::fresh(&attempt.clock, &attempt.before_claim, &response.receipt);
        let live_ack = eligible.then(|| LiveHeldCreateClaimAck {
            tenant_id: self.scope.tenant_id.clone(),
            claim: attempt.claim,
            response: response.clone(),
            identity: before.0,
            clock: attempt.clock,
            before_claim: attempt.before_claim,
            protected_files: self.protected_files(),
        });
        Ok(HeldCreateClaimExchange { response, live_ack })
    }

    pub(crate) async fn observe_claim(
        &self,
        create_id: &str,
        operation_id: &str,
    ) -> ClientResult<KubernetesCreateClaimResponseV1> {
        validation::id(create_id)?;
        validation::id(operation_id)?;
        self.handshake().await?;
        let (status, response) = self
            .exchange(
                &format!(
                    "/api/v1/brokers/kubernetes/held-creates/{create_id}/claims/{operation_id}"
                ),
                None,
            )
            .await?;
        validation::claim(&self.scope, &response, create_id, operation_id, None)?;
        if status != reqwest::StatusCode::OK
            || response.disposition != KubernetesCreateDispositionV1::ObserveOnly
        {
            return Err(HeldCreateClientError::Protocol);
        }
        Ok(response)
    }

    pub(crate) async fn result(
        &self,
        result: &KubernetesCreateResultV1,
    ) -> ClientResult<ConfirmedHeldCreateResultAck> {
        result
            .validate()
            .map_err(|_| HeldCreateClientError::InvalidRequest)?;
        let bytes = self.request_bytes(result)?;
        self.handshake().await?;
        let (status, response) = self
            .exchange(
                &format!(
                    "/api/v1/brokers/kubernetes/held-creates/{}/results",
                    result.create_id
                ),
                Some(bytes),
            )
            .await?;
        validation::result(&self.scope, result, &response)?;
        if status != reqwest::StatusCode::OK {
            return Err(HeldCreateClientError::Protocol);
        }
        Ok(ConfirmedHeldCreateResultAck {
            tenant_id: self.scope.tenant_id.clone(),
            result: result.clone(),
            receipt: response.receipt,
            snapshot: response.snapshot,
        })
    }

    // Closed held-CREATE client surface; production wiring pending.
    #[allow(dead_code)]
    pub(crate) async fn cancel(
        &self,
        cancel: &KubernetesCreateCancelV1,
    ) -> ClientResult<KubernetesCreateMutationResponseV1> {
        cancel
            .validate()
            .map_err(|_| HeldCreateClientError::InvalidRequest)?;
        let bytes = self.request_bytes(cancel)?;
        self.handshake().await?;
        let (status, response) = self
            .exchange(
                &format!(
                    "/api/v1/brokers/kubernetes/held-creates/{}/cancellations",
                    cancel.create_id
                ),
                Some(bytes),
            )
            .await?;
        validation::cancel(&self.scope, cancel, &response)?;
        if status != reqwest::StatusCode::OK {
            return Err(HeldCreateClientError::Protocol);
        }
        Ok(response)
    }

    /// Close one retained, never-claimed reservation. The caller retains both
    /// values and retries this exact operation ID after an uncertain exchange.
    /// This method never creates an operation ID or a runtime authorization.
    // Closed held-CREATE client surface; production wiring pending.
    #[allow(dead_code)]
    pub(crate) async fn recover(
        &self,
        intent: &KubernetesCreateIntentV1,
        recovery: &KubernetesCreateRecoveryV3,
    ) -> ClientResult<KubernetesCreateRecoveryResultV3> {
        validation::recovery_request(&self.scope, intent, recovery)?;
        let bytes = self.request_bytes(recovery)?;
        let (_, protocol) = self.handshake().await?;
        if protocol.schema_version != 13 {
            return Err(HeldCreateClientError::Protocol);
        }
        let (status, response) = self
            .exchange(
                &format!(
                    "/api/v1/brokers/kubernetes/held-creates/{}/recoveries",
                    recovery.create_id
                ),
                Some(bytes),
            )
            .await?;
        validation::recovery(&self.scope, intent, recovery, &response, status, false)?;
        Ok(response)
    }

    /// Read only the receipt for this retained exact request. Missing data is
    /// NotFound; no POST, issuer, replacement UUID or launch fallback is used.
    // Closed held-CREATE client surface; production wiring pending.
    #[allow(dead_code)]
    pub(crate) async fn observe_recovery(
        &self,
        intent: &KubernetesCreateIntentV1,
        recovery: &KubernetesCreateRecoveryV3,
    ) -> ClientResult<KubernetesCreateRecoveryResultV3> {
        validation::recovery_request(&self.scope, intent, recovery)?;
        let (_, protocol) = self.handshake().await?;
        if protocol.schema_version != 13 {
            return Err(HeldCreateClientError::Protocol);
        }
        let (status, response) = self
            .exchange(
                &format!(
                    "/api/v1/brokers/kubernetes/held-creates/{}/recoveries/{}",
                    recovery.create_id, recovery.operation_id
                ),
                None,
            )
            .await?;
        validation::recovery(&self.scope, intent, recovery, &response, status, true)?;
        Ok(response)
    }
}

/// One owned invocation, sampled before every preclaim control read.
/// No wire value or recovered journal can recreate this invocation.
// Closed held-CREATE claim target; production wiring pending.
#[allow(dead_code)]
pub(crate) struct HeldCreateClaimAttempt {
    tenant_id: String,
    intent: KubernetesCreateIntentV1,
    claim: KubernetesCreateClaimV1,
    target: KubernetesCreateTargetV1,
    clock: Arc<dyn HostClock>,
    before_claim: HostClockSampleV1,
}
impl HeldCreateClaimAttempt {
    // Closed held-CREATE claim target; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn begin(journal: &AdmissionJournal) -> ClientResult<Self> {
        let clock: Arc<dyn HostClock> =
            Arc::new(NativeHostClock::new().map_err(|_| HeldCreateClientError::Clock)?);
        let before = clock.sample().map_err(|_| HeldCreateClientError::Clock)?;
        Self::from_clock_origin(journal, clock, before)
    }

    /// Trusted runtime glue passes its original producer and preclaim sample.
    /// This request context is descriptive and grants no HTTP or runtime effect.
    // Closed held-CREATE claim target; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn from_clock_origin(
        journal: &AdmissionJournal,
        clock: Arc<dyn HostClock>,
        before_claim: HostClockSampleV1,
    ) -> ClientResult<Self> {
        // Synchronous preparation belongs in the caller's admitted JournalIo worker.
        journal
            .revalidate_retained()
            .map_err(|_| HeldCreateClientError::UnsafeFilesystem)?;
        if journal.observation_only() || journal.candidate().is_some() {
            return Err(HeldCreateClientError::InvalidContext);
        }
        before_claim
            .validate()
            .map_err(|_| HeldCreateClientError::Clock)?;
        journal
            .claim_request()
            .validate()
            .map_err(|_| HeldCreateClientError::InvalidRequest)?;
        let prepared = crate::runtime::kubernetes::admission::prepare(journal.intent())
            .map_err(|_| HeldCreateClientError::InvalidContext)?;
        if prepared.target() != journal.target() {
            return Err(HeldCreateClientError::InvalidContext);
        }
        Ok(Self {
            tenant_id: journal.tenant_id().into(),
            intent: journal.intent().clone(),
            claim: journal.claim_request().clone(),
            target: journal.target().clone(),
            clock,
            before_claim,
        })
    }

    #[cfg(test)]
    pub(crate) fn begin_for_test(
        journal: &AdmissionJournal,
        clock: Arc<dyn HostClock>,
    ) -> ClientResult<Self> {
        let before = clock.sample().map_err(|_| HeldCreateClientError::Clock)?;
        Self::from_clock_origin(journal, clock, before)
    }
}

// Closed held-CREATE claim target; production wiring pending.
#[allow(dead_code)]
pub(crate) struct HeldCreateClaimExchange {
    response: KubernetesCreateClaimResponseV1,
    live_ack: Option<LiveHeldCreateClaimAck>,
}
// Closed held-CREATE claim target; production wiring pending.
#[allow(dead_code)]
impl HeldCreateClaimExchange {
    pub(crate) fn response(&self) -> &KubernetesCreateClaimResponseV1 {
        &self.response
    }
    pub(crate) fn into_live_ack(self) -> Option<LiveHeldCreateClaimAck> {
        self.live_ack
    }
}

/// Private live HTTP provenance, never audit::FreshKubernetesCreateClaimCommit.
// Closed held-CREATE claim target; production wiring pending.
#[allow(dead_code)]
pub(crate) struct LiveHeldCreateClaimAck {
    tenant_id: String,
    claim: KubernetesCreateClaimV1,
    response: KubernetesCreateClaimResponseV1,
    identity: BrokerIdentityViewV1,
    clock: Arc<dyn HostClock>,
    before_claim: HostClockSampleV1,
    protected_files: Vec<PrivateFile>,
}
// Closed held-CREATE claim target; production wiring pending.
#[allow(dead_code)]
impl LiveHeldCreateClaimAck {
    pub(crate) fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    pub(crate) fn claim(&self) -> &KubernetesCreateClaimV1 {
        &self.claim
    }
    pub(crate) fn receipt(&self) -> &KubernetesCreateReceiptV1 {
        &self.response.receipt
    }
    pub(crate) fn snapshot(&self) -> &KubernetesCreateSnapshotV1 {
        &self.response.snapshot
    }
    pub(crate) fn clock(&self) -> Arc<dyn HostClock> {
        Arc::clone(&self.clock)
    }
    pub(crate) fn before_claim(&self) -> &HostClockSampleV1 {
        &self.before_claim
    }
    pub(crate) fn revalidate_material(&self) -> ClientResult<()> {
        // Caller executes this synchronous final check inside owned JournalIo.
        for file in &self.protected_files {
            file.revalidate()
                .map_err(|_| HeldCreateClientError::UnsafeFilesystem)?;
        }
        Ok(())
    }
}

/// HTTP provenance and request links, including historical replay; no emission.
/// The client has partial context: before ack.json or a handle, the journal must
/// compare the whole snapshot against its retained emission and exact candidate.
pub(crate) struct ConfirmedHeldCreateResultAck {
    tenant_id: String,
    result: KubernetesCreateResultV1,
    receipt: KubernetesCreateReceiptV1,
    snapshot: KubernetesCreateSnapshotV1,
}
impl ConfirmedHeldCreateResultAck {
    pub(crate) fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    pub(crate) fn result(&self) -> &KubernetesCreateResultV1 {
        &self.result
    }
    pub(crate) fn receipt(&self) -> &KubernetesCreateReceiptV1 {
        &self.receipt
    }
    pub(crate) fn snapshot(&self) -> &KubernetesCreateSnapshotV1 {
        &self.snapshot
    }
}

macro_rules! opaque_debug {
    ($($kind:ty),+ $(,)?) => {$(
        impl fmt::Debug for $kind {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($kind), "(<private>)"))
            }
        }
    )+};
}
opaque_debug!(
    HeldCreateClientScope,
    HeldCreateControlClient,
    HeldCreateClaimAttempt,
    HeldCreateClaimExchange,
    LiveHeldCreateClaimAck,
    ConfirmedHeldCreateResultAck,
);

#[cfg(test)]
#[path = "held_create_client_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "held_create_recovery_tests.rs"]
mod recovery_tests;
