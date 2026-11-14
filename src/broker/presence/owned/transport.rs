//! Owned control exchanges over the shared protected presence transport. The
//! operation fixes the method, gateway path and bounds; a request field never
//! selects them. A non-200 body is reduced to its closed classification.
use super::*;
use crate::broker_registry::owned_supervision::{
    MAX_OWNED_ERROR_BYTES, OwnedBrokerCheckpointResultV2, OwnedBrokerCheckpointSelfViewV2,
    OwnedBrokerHeartbeatResultV2, OwnedBrokerOperationV2, OwnedBrokerSelfViewV2,
    OwnedCheckpointPendingV2, OwnedHeartbeatPendingV2, classify_remote_failure,
};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;

pub(super) struct OwnedClient {
    http: http::Client,
}
impl OwnedClient {
    pub(super) async fn reconcile(
        &self,
        p: &PreparedOwnedPresence,
        request: &crate::broker_registry::owned_supervision::OwnedReconciliationRequestV2,
    ) -> Result<crate::broker_registry::owned_supervision::OwnedReconciliationResultV2> {
        let bytes = serde_json::to_vec(request).map_err(|_| PresenceError::UnsafeState)?;
        self.exchange(p, request.operation(), Some(bytes)).await
    }

    pub(super) fn new(p: &PreparedOwnedPresence) -> Result<Self> {
        Ok(Self {
            http: http::Client::build(&p.scope.gateway, p.ca.as_ref(), &p.bearer)?,
        })
    }

    async fn exchange<T: DeserializeOwned>(
        &self,
        p: &PreparedOwnedPresence,
        operation: OwnedBrokerOperationV2,
        request: Option<Vec<u8>>,
    ) -> Result<T> {
        let method = match operation.method() {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            _ => return Err(PresenceError::Protocol),
        };
        // Exact closed classification of the owned contract; a bodyless GET
        // never carries a body and a mutation always carries its request.
        if OwnedBrokerOperationV2::classify(operation.method(), operation.path()) != Some(operation)
            || (operation.method() == "POST") != request.is_some()
        {
            return Err(PresenceError::Protocol);
        }
        let limits = http::Limits {
            request: operation.request_limit() as u64,
            success: operation.response_limit() as u64,
            error: MAX_OWNED_ERROR_BYTES as u64,
        };
        let (status, bytes) = self
            .http
            .send_bounded(method, operation.path(), request, limits)
            .await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| PresenceError::Protocol)?;
        if status != StatusCode::OK {
            // Bounded transient memory only: keep the closed classification,
            // never the remote prose, in an error, a journal or a log.
            return Err(block(OwnedPresenceBlock::Remote(classify_remote_failure(
                status.as_u16(),
                &value,
            ))));
        }
        p.screen(&value)?;
        // Typed decoding from the original bytes also refuses duplicate fields.
        serde_json::from_slice(&bytes).map_err(|_| PresenceError::Protocol)
    }

    /// Identity, broker state and latest heartbeat receipt read together.
    pub(super) async fn self_view(
        &self,
        p: &PreparedOwnedPresence,
    ) -> Result<OwnedBrokerSelfViewV2> {
        self.exchange(p, OwnedBrokerOperationV2::SelfView, None)
            .await
    }

    /// Identity and declared checkpoint observation read together.
    pub(super) async fn checkpoint_self(
        &self,
        p: &PreparedOwnedPresence,
    ) -> Result<OwnedBrokerCheckpointSelfViewV2> {
        self.exchange(p, OwnedBrokerOperationV2::CheckpointSelf, None)
            .await
    }

    /// The exact request derived from the durable Pending, on every retry.
    pub(super) async fn heartbeat(
        &self,
        p: &PreparedOwnedPresence,
        pending: &OwnedHeartbeatPendingV2,
    ) -> Result<OwnedBrokerHeartbeatResultV2> {
        let body = serde_json::to_vec(&pending.request()).map_err(|_| PresenceError::Protocol)?;
        self.exchange(p, OwnedBrokerOperationV2::Heartbeat, Some(body))
            .await
    }

    pub(super) async fn checkpoint(
        &self,
        p: &PreparedOwnedPresence,
        pending: &OwnedCheckpointPendingV2,
    ) -> Result<OwnedBrokerCheckpointResultV2> {
        let body = serde_json::to_vec(&pending.request()).map_err(|_| PresenceError::Protocol)?;
        self.exchange(p, OwnedBrokerOperationV2::Checkpoint, Some(body))
            .await
    }
}
