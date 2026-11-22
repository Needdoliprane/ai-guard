//! Actual R2 broker HTTP calls over the retained, bounded control transport.
//! A decoded DTO has no authority. Only a nonce-matched live exchange bracketed
//! by the same authenticated broker identity can mint the private local ACK.
use super::{HeldCreateClientError, HeldCreateClientScope, HeldCreateControlClient};
use crate::{
    audit::kubernetes_session::ConfirmedSessionReservationV2,
    broker_registry::BrokerIdentityViewV1,
    control::kubernetes_session::{lifecycle::*, *},
    control_fs::PrivateFile,
};
use reqwest::StatusCode;
use std::fmt;
use uuid::Uuid;

pub(crate) type Result<T> = std::result::Result<T, SessionControlErrorV2>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionControlErrorV2 {
    InvalidInput,
    Scope,
    Authentication,
    Forbidden,
    NotFound,
    Conflict,
    Capacity,
    Unavailable,
    Protocol,
    /// No validated ACK was obtained. The mutation may already be durable.
    ReceptionUnknown,
}
impl fmt::Display for SessionControlErrorV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "Kubernetes session control input or private material is invalid",
            Self::Scope => "Kubernetes session control identity differs from the expected broker scope",
            Self::Authentication => "Kubernetes session broker authentication was refused",
            Self::Forbidden => "Kubernetes session broker scope or tenant policy was refused",
            Self::NotFound => "Kubernetes session reservation or exact retained receipt was not found",
            Self::Conflict => "Kubernetes session request conflicts with durable state",
            Self::Capacity => "Kubernetes session control audit or quota capacity is unavailable",
            Self::Unavailable => "Kubernetes session control is unavailable or its format is unsupported",
            Self::Protocol => "Kubernetes session control response failed exact validation",
            Self::ReceptionUnknown => "Kubernetes session ACK is unavailable; reception is unknown, preserve the same request and operation ID",
        })
    }
}
impl std::error::Error for SessionControlErrorV2 {}
impl From<HeldCreateClientError> for SessionControlErrorV2 {
    fn from(error: HeldCreateClientError) -> Self {
        use HeldCreateClientError as E;
        match error {
            E::InvalidConfiguration
            | E::UnsafeFilesystem
            | E::InvalidRequest
            | E::ControlRejected => Self::InvalidInput,
            E::InvalidContext => Self::Scope,
            E::Authentication => Self::Authentication,
            E::Forbidden => Self::Forbidden,
            E::NotFound => Self::NotFound,
            E::Conflict | E::StaleEpoch | E::StaleRevision => Self::Conflict,
            E::Capacity => Self::Capacity,
            E::Transport => Self::ReceptionUnknown,
            E::Unavailable | E::Clock => Self::Unavailable,
            E::Protocol => Self::Protocol,
        }
    }
}

pub(crate) struct SessionControlClientV2 {
    transport: HeldCreateControlClient,
}
impl fmt::Debug for SessionControlClientV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionControlClientV2(<private>)")
    }
}
impl SessionControlClientV2 {
    pub(crate) fn new(
        scope: HeldCreateClientScope,
        token: PrivateFile,
        ca: Option<PrivateFile>,
        known_secrets: Vec<String>,
    ) -> Result<Self> {
        Ok(Self {
            transport: HeldCreateControlClient::new(scope, token, ca, known_secrets)?,
        })
    }
    pub(crate) fn scope(&self) -> &HeldCreateClientScope {
        self.transport.expected_scope()
    }
    fn validate_intent(
        &self,
        intent: &KubernetesSessionReservationIntentV2,
        successor: bool,
    ) -> Result<()> {
        intent
            .validate()
            .map_err(|_| SessionControlErrorV2::InvalidInput)?;
        let ids = intent
            .ids()
            .map_err(|_| SessionControlErrorV2::InvalidInput)?;
        let scope = self.scope();
        if ids.control_namespace_uuid.to_string() != scope.control_namespace_uuid
            || ids.broker_id.to_string() != scope.broker_id
            || !successor && ids.incarnation_id.to_string() != scope.incarnation_id
        {
            return Err(SessionControlErrorV2::Scope);
        }
        Ok(())
    }
    async fn identity(&self) -> Result<BrokerIdentityViewV1> {
        let (status, identity): (_, BrokerIdentityViewV1) = self
            .transport
            .exchange("/api/v1/brokers/identity", None)
            .await?;
        let scope = self.scope();
        if status != StatusCode::OK
            || identity.version != 1
            || ![11, 12, 13].contains(&identity.schema_version)
        {
            return Err(SessionControlErrorV2::Unavailable);
        }
        if identity.tenant_id != scope.tenant_id
            || identity.control_namespace_uuid != scope.control_namespace_uuid
            || identity.broker_id != scope.broker_id
            || identity.incarnation_id != scope.incarnation_id
            || identity.credential_id != scope.credential_id
            || identity.binding_revision == 0
            || identity.grant_revision == 0
            || identity.binding_revision > i64::MAX as u64
            || identity.grant_revision > i64::MAX as u64
        {
            return Err(SessionControlErrorV2::Scope);
        }
        Ok(identity)
    }
    pub(crate) async fn protocol(&self) -> Result<SessionProtocolV2> {
        let identity = self.identity().await?;
        let (status, protocol): (_, SessionProtocolV2) =
            self.transport.exchange(PROTOCOL_PATH, None).await?;
        // `effects_available` only reports whether the tenant's ledger and
        // effect policy are configured; pre-effect commands ignore it.
        if status != StatusCode::OK || protocol.version != 2 || protocol.identity != identity {
            return Err(SessionControlErrorV2::Protocol);
        }
        Ok(protocol)
    }
    fn validate_current(
        &self,
        intent: &KubernetesSessionReservationIntentV2,
        current: &KubernetesSessionReservationCurrentV2,
    ) -> Result<()> {
        current
            .snapshot
            .validate(&self.scope().tenant_id)
            .map_err(|_| SessionControlErrorV2::Protocol)?;
        if current.version != 2
            || current.last_event_seq <= 0
            || crate::control::kubernetes_session::validation::hash(&current.last_event_hash)
                .is_err()
            || !current
                .snapshot
                .intent
                .same(intent)
                .map_err(|_| SessionControlErrorV2::Protocol)?
        {
            return Err(SessionControlErrorV2::Protocol);
        }
        Ok(())
    }
    fn validate_mutation(
        &self,
        intent: &KubernetesSessionReservationIntentV2,
        response: &KubernetesSessionReservationMutationResponseV2,
        close: Option<&KubernetesSessionReservationCloseV2>,
    ) -> Result<()> {
        let current = KubernetesSessionReservationCurrentV2 {
            version: response.version,
            snapshot: response.snapshot.clone(),
            last_event_seq: response.receipt.audit_seq,
            last_event_hash: response.receipt.audit_hash.clone(),
        };
        self.validate_current(intent, &current)?;
        let (operation, operation_id, input_hash, state) = match close {
            None => (
                KubernetesSessionReservationOperationV2::Reserve,
                intent.reservation_id(),
                intent.input_hash(&self.scope().tenant_id),
                KubernetesSessionReservationStateV2::Reserved,
            ),
            Some(close) => (
                KubernetesSessionReservationOperationV2::CloseBeforeEffect,
                close.operation_id.as_str(),
                close.input_hash(),
                KubernetesSessionReservationStateV2::ClosedBeforeEffect,
            ),
        };
        let receipt = &response.receipt;
        if receipt.version != 2
            || receipt.operation != operation
            || receipt.operation_id != operation_id
            || receipt.input_hash != input_hash.map_err(|_| SessionControlErrorV2::InvalidInput)?
            || receipt.control_namespace_uuid != self.scope().control_namespace_uuid
            || response.snapshot.state.state != state
            || response.snapshot.state.last_close.as_ref() != close
        {
            return Err(SessionControlErrorV2::Protocol);
        }
        Ok(())
    }
    pub(crate) async fn reserve(
        &self,
        intent: &KubernetesSessionReservationIntentV2,
    ) -> Result<KubernetesSessionReservationMutationResponseV2> {
        self.validate_intent(intent, false)?;
        let bytes = self.transport.request_bytes(intent)?;
        let before = self.protocol().await?.identity;
        let (status, response): (_, KubernetesSessionReservationMutationResponseV2) = self
            .transport
            .exchange(RESERVATIONS_PATH, Some(bytes))
            .await?;
        self.validate_mutation(intent, &response, None)?;
        if status
            != (if response.replayed {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            })
            || self
                .identity()
                .await
                .map_err(|_| SessionControlErrorV2::ReceptionUnknown)?
                != before
        {
            return Err(SessionControlErrorV2::Protocol);
        }
        Ok(response)
    }
    pub(crate) async fn observe(
        &self,
        intent: &KubernetesSessionReservationIntentV2,
    ) -> Result<KubernetesSessionReservationCurrentV2> {
        self.validate_intent(intent, false)?;
        let before = self.protocol().await?.identity;
        let (status, current) = self
            .transport
            .exchange(
                &format!("{RESERVATIONS_PATH}/{}", intent.reservation_id()),
                None,
            )
            .await?;
        self.validate_current(intent, &current)?;
        if status != StatusCode::OK || self.identity().await? != before {
            return Err(SessionControlErrorV2::Protocol);
        }
        Ok(current)
    }
    pub(crate) async fn confirm(
        &self,
        intent: &KubernetesSessionReservationIntentV2,
    ) -> Result<ConfirmedSessionReservationV2> {
        self.validate_intent(intent, false)?;
        let request = SessionConfirmationRequestV2 {
            version: 2,
            request_id: Uuid::new_v4().to_string(),
            reservation_input_hash: intent
                .input_hash(&self.scope().tenant_id)
                .map_err(|_| SessionControlErrorV2::InvalidInput)?,
        };
        let bytes = self.transport.request_bytes(&request)?;
        let before = self.protocol().await?.identity;
        let (status, response): (_, SessionConfirmationResponseV2) = self
            .transport
            .exchange(
                &format!(
                    "{RESERVATIONS_PATH}/{}/confirmations",
                    intent.reservation_id()
                ),
                Some(bytes),
            )
            .await?;
        self.validate_current(intent, &response.current)?;
        if status != StatusCode::OK
            || response.version != 2
            || response.request_id != request.request_id
            || response.identity != before
            || self
                .identity()
                .await
                .map_err(|_| SessionControlErrorV2::ReceptionUnknown)?
                != before
            || response.current.snapshot.input_hash != request.reservation_input_hash
            || response.current.snapshot.state.state
                != KubernetesSessionReservationStateV2::Reserved
        {
            return Err(SessionControlErrorV2::Protocol);
        }
        let ack = LiveSessionConfirmationAckV2 {
            tenant_id: self.scope().tenant_id.clone(),
            current: response.current,
            protected_files: self.transport.protected_files(),
        };
        ConfirmedSessionReservationV2::from_live_http_ack(ack)
            .map_err(|_| SessionControlErrorV2::Protocol)
    }
    /// No automatic retry. After an uncertain exchange, resume the exact
    /// locally retained close; never replace its operation UUID.
    pub(crate) async fn abandon(
        &self,
        intent: &KubernetesSessionReservationIntentV2,
        close: &KubernetesSessionReservationCloseV2,
    ) -> Result<KubernetesSessionReservationMutationResponseV2> {
        self.validate_intent(intent, true)?;
        close
            .validate()
            .map_err(|_| SessionControlErrorV2::InvalidInput)?;
        if close.reservation_id != intent.reservation_id()
            || close.reservation_input_hash
                != intent
                    .input_hash(&self.scope().tenant_id)
                    .map_err(|_| SessionControlErrorV2::InvalidInput)?
            || close.reason != KubernetesSessionReservationCloseReasonV2::PreparationAbandoned
        {
            return Err(SessionControlErrorV2::InvalidInput);
        }
        let bytes = self.transport.request_bytes(close)?;
        let before = self.protocol().await?.identity;
        let (status, response): (_, KubernetesSessionReservationMutationResponseV2) = self
            .transport
            .exchange(
                &format!(
                    "{RESERVATIONS_PATH}/{}/abandonments",
                    intent.reservation_id()
                ),
                Some(bytes),
            )
            .await?;
        self.validate_mutation(intent, &response, Some(close))?;
        if status != StatusCode::OK
            || self
                .identity()
                .await
                .map_err(|_| SessionControlErrorV2::ReceptionUnknown)?
                != before
        {
            return Err(SessionControlErrorV2::Protocol);
        }
        Ok(response)
    }
    /// Central claim of one exact effect request. `EmitOnce` comes back once;
    /// a replay of a lost answer returns the historical `ObserveOnly`.
    pub(crate) async fn claim_effect(
        &self,
        request: &crate::control::kubernetes_session::effects::KubernetesSessionEffectRequestV2,
    ) -> Result<crate::control::kubernetes_session::effects::KubernetesSessionEffectClaimAckV2>
    {
        request
            .validate()
            .map_err(|_| SessionControlErrorV2::InvalidInput)?;
        let path = format!("{RESERVATIONS_PATH}/{}/effects", request.reservation_id);
        let (status, ack): (
            _,
            crate::control::kubernetes_session::effects::KubernetesSessionEffectClaimAckV2,
        ) = self.effect_exchange(&path, request).await?;
        ack.validate()
            .map_err(|_| SessionControlErrorV2::Protocol)?;
        let expected = if ack.replayed {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        };
        if status != expected
            || ack.request != *request
            || ack.receipt.control_namespace_uuid != self.scope().control_namespace_uuid
        {
            return Err(SessionControlErrorV2::Protocol);
        }
        Ok(ack)
    }
    pub(crate) async fn record_effect_result(
        &self,
        result: &crate::control::kubernetes_session::effects::KubernetesSessionEffectResultV2,
    ) -> Result<crate::control::kubernetes_session::effects::KubernetesSessionEffectStateV2> {
        result
            .validate()
            .map_err(|_| SessionControlErrorV2::InvalidInput)?;
        let path = format!(
            "{RESERVATIONS_PATH}/{}/effect-results",
            result.reservation_id
        );
        self.effect_state_exchange(&path, &result.reservation_id, result)
            .await
    }
    pub(crate) async fn record_terminal(
        &self,
        terminal: &crate::control::kubernetes_session::effects::KubernetesSessionTerminalV2,
    ) -> Result<crate::control::kubernetes_session::effects::KubernetesSessionEffectStateV2> {
        terminal
            .validate()
            .map_err(|_| SessionControlErrorV2::InvalidInput)?;
        let path = format!("{RESERVATIONS_PATH}/{}/terminals", terminal.reservation_id);
        self.effect_state_exchange(&path, &terminal.reservation_id, terminal)
            .await
    }
    pub(crate) async fn settle(
        &self,
        settlement: &crate::control::kubernetes_session::effects::KubernetesSessionSettlementV2,
    ) -> Result<crate::control::kubernetes_session::effects::KubernetesSessionEffectStateV2> {
        settlement
            .validate()
            .map_err(|_| SessionControlErrorV2::InvalidInput)?;
        let path = format!(
            "{RESERVATIONS_PATH}/{}/settlements",
            settlement.reservation_id
        );
        self.effect_state_exchange(&path, &settlement.reservation_id, settlement)
            .await
    }
    pub(crate) async fn effect_state(
        &self,
        reservation_id: &str,
    ) -> Result<crate::control::kubernetes_session::effects::KubernetesSessionEffectStateV2> {
        crate::control::kubernetes_session::validation::uuid(reservation_id)
            .map_err(|_| SessionControlErrorV2::InvalidInput)?;
        let before = self.protocol().await?.identity;
        let (status, state): (
            _,
            crate::control::kubernetes_session::effects::KubernetesSessionEffectStateV2,
        ) = self
            .transport
            .exchange(
                &format!("{RESERVATIONS_PATH}/{reservation_id}/effects"),
                None,
            )
            .await?;
        if status != StatusCode::OK
            || state.version != 1
            || state.reservation_id != reservation_id
            || self.identity().await? != before
        {
            return Err(SessionControlErrorV2::Protocol);
        }
        Ok(state)
    }
    /// One effect POST bracketed by the same authenticated broker identity.
    async fn effect_exchange<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<(StatusCode, T)> {
        let bytes = self.transport.request_bytes(body)?;
        let before = self.protocol().await?.identity;
        let response = self.transport.exchange(path, Some(bytes)).await?;
        if self
            .identity()
            .await
            .map_err(|_| SessionControlErrorV2::ReceptionUnknown)?
            != before
        {
            return Err(SessionControlErrorV2::Protocol);
        }
        Ok(response)
    }
    async fn effect_state_exchange<B: serde::Serialize>(
        &self,
        path: &str,
        reservation_id: &str,
        body: &B,
    ) -> Result<crate::control::kubernetes_session::effects::KubernetesSessionEffectStateV2> {
        let (status, state): (
            _,
            crate::control::kubernetes_session::effects::KubernetesSessionEffectStateV2,
        ) = self.effect_exchange(path, body).await?;
        if status != StatusCode::OK || state.version != 1 || state.reservation_id != reservation_id
        {
            return Err(SessionControlErrorV2::Protocol);
        }
        Ok(state)
    }
    pub(crate) fn filesystem_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
        self.transport
            .material_capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| SessionControlErrorV2::Capacity)
    }
}

/// No Clone, serde or constructor outside this module. This is HTTP provenance
/// for before-effect preparation, never a Kubernetes identity or effect permit.
pub(crate) struct LiveSessionConfirmationAckV2 {
    tenant_id: String,
    current: KubernetesSessionReservationCurrentV2,
    protected_files: Vec<PrivateFile>,
}
impl LiveSessionConfirmationAckV2 {
    pub(crate) fn into_parts(
        self,
    ) -> (
        String,
        KubernetesSessionReservationCurrentV2,
        Vec<PrivateFile>,
    ) {
        (self.tenant_id, self.current, self.protected_files)
    }
}
