//! Gateway adapter of the broker egress owner, over the runner credential.
//!
//! One attempt per exchange: a lost decision is never replayed into a connect,
//! and any non-success answer refuses the flow.

use super::AuditClient;
use crate::{
    broker::egress_owner::channel::{ControlError, ControlFuture, EgressControl},
    control::egress_decision::{
        EgressDecideRequestV1, EgressDecisionV1, EgressObserveRequestV1, EgressRenewRequestV1,
        EgressRenewResponseV1,
    },
};
use serde::{Serialize, de::DeserializeOwned};

const MAX_RESPONSE_BYTES: usize = 64 * 1024;

pub(crate) struct GatewayEgressControl {
    client: AuditClient,
}

impl GatewayEgressControl {
    pub(crate) fn new(client: AuditClient) -> Self {
        Self { client }
    }

    async fn post<B: Serialize + Sync, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<R, ControlError> {
        let url = self
            .client
            .base
            .join(path)
            .map_err(|_| ControlError::Refused)?;
        let mut response = self
            .client
            .http
            .post(url)
            .bearer_auth(&self.client.token)
            .json(body)
            .send()
            .await
            .map_err(|_| ControlError::Unavailable)?;
        let status = response.status();
        if status.is_client_error() {
            return Err(ControlError::Refused);
        }
        if !status.is_success() {
            return Err(ControlError::Unavailable);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ControlError::Unavailable)?
        {
            if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(ControlError::Unavailable);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| ControlError::Unavailable)
    }
}

impl EgressControl for GatewayEgressControl {
    fn decide(&self, request: EgressDecideRequestV1) -> ControlFuture<'_, EgressDecisionV1> {
        Box::pin(async move { self.post("api/v1/egress/decisions", &request).await })
    }
    fn observe(&self, request: EgressObserveRequestV1) -> ControlFuture<'_, ()> {
        Box::pin(async move {
            let _: serde_json::Value = self.post("api/v1/egress/observations", &request).await?;
            Ok(())
        })
    }
    fn renew(&self, request: EgressRenewRequestV1) -> ControlFuture<'_, EgressRenewResponseV1> {
        Box::pin(async move { self.post("api/v1/egress/renewals", &request).await })
    }
}
