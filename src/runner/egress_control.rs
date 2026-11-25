//! Gateway adapter of the broker egress owner, over the runner credential.
//!
//! One attempt per exchange: lost decisions or permits cannot authorize a connect,
//! and any non-success answer refuses the flow.

use super::AuditClient;
use crate::{
    broker::egress_owner::channel::{ControlError, ControlFuture, EgressControl},
    control::egress_decision::{
        EgressChannelOpeningReceiptV1, EgressChannelOpeningRequestV1, EgressConnectPermitReceiptV1,
        EgressConnectPermitRequestV1, EgressDecideRequestV1, EgressDecisionV1,
        EgressObserveRequestV1, EgressReceiptV1, EgressRenewRequestV1, EgressRenewResponseV1,
    },
    egress::ids::DigestV1,
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

    async fn post<B: Serialize + Sync, R>(
        &self,
        path: &str,
        body: &B,
        decode: fn(&[u8]) -> Result<R, ControlError>,
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
        decode(&bytes)
    }
}

fn decode_json<R: DeserializeOwned>(bytes: &[u8]) -> Result<R, ControlError> {
    serde_json::from_slice(bytes).map_err(|_| ControlError::Unavailable)
}

fn decode_object<R: DeserializeOwned>(bytes: &[u8]) -> Result<R, ControlError> {
    // The gateway returns a JSON object. Serde also accepts a struct as a sequence.
    if !bytes.trim_ascii_start().starts_with(b"{") {
        return Err(ControlError::Unavailable);
    }
    decode_json(bytes)
}

fn decode_observation_ack(bytes: &[u8]) -> Result<(), ControlError> {
    let receipt: EgressReceiptV1 = decode_object(bytes)?;
    if receipt.audit_seq <= 0 || DigestV1::parse(&receipt.audit_hash).is_err() {
        return Err(ControlError::Unavailable);
    }
    Ok(())
}

impl EgressControl for GatewayEgressControl {
    fn open_channel(
        &self,
        request: EgressChannelOpeningRequestV1,
    ) -> ControlFuture<'_, EgressChannelOpeningReceiptV1> {
        Box::pin(async move {
            self.post("api/v1/egress/channel-openings", &request, decode_object)
                .await
        })
    }
    fn decide(&self, request: EgressDecideRequestV1) -> ControlFuture<'_, EgressDecisionV1> {
        Box::pin(async move {
            self.post("api/v1/egress/decisions", &request, decode_json)
                .await
        })
    }
    fn issue_connect_permit(
        &self,
        request: EgressConnectPermitRequestV1,
    ) -> ControlFuture<'_, EgressConnectPermitReceiptV1> {
        Box::pin(async move {
            self.post("api/v1/egress/connect-permits", &request, decode_object)
                .await
        })
    }
    fn observe(&self, request: EgressObserveRequestV1) -> ControlFuture<'_, ()> {
        Box::pin(async move {
            self.post(
                "api/v1/egress/observations",
                &request,
                decode_observation_ack,
            )
            .await
        })
    }
    fn renew(&self, request: EgressRenewRequestV1) -> ControlFuture<'_, EgressRenewResponseV1> {
        Box::pin(async move {
            self.post("api/v1/egress/renewals", &request, decode_json)
                .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_ack_requires_a_canonical_audit_receipt() {
        let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let valid = format!(r#" {{"audit_seq":21,"audit_hash":"{hash}"}} "#);
        assert_eq!(decode_observation_ack(valid.as_bytes()), Ok(()));

        for invalid in [
            "null".into(),
            "".into(),
            r#"{"audit_seq":"#.into(),
            r#"{"audit_seq":21}"#.into(),
            format!(r#"{{"audit_hash":"{hash}"}}"#),
            format!(r#"{{"audit_seq":"21","audit_hash":"{hash}"}}"#),
            format!(r#"[21,"{hash}"]"#),
            format!(r#"{{"audit_seq":21,"audit_hash":"{hash}","extra":true}}"#),
            format!(r#"{{"audit_seq":21,"audit_seq":22,"audit_hash":"{hash}"}}"#),
            format!(r#"{{"audit_seq":0,"audit_hash":"{hash}"}}"#),
            format!(r#"{{"audit_seq":-1,"audit_hash":"{hash}"}}"#),
            format!(r#"{{"audit_seq":21,"audit_hash":"{}"}}"#, &hash[..63]),
            format!(
                r#"{{"audit_seq":21,"audit_hash":"{}"}}"#,
                hash.to_uppercase()
            ),
            format!(r#"{{"audit_seq":21,"audit_hash":"{}g"}}"#, &hash[..63]),
        ] {
            assert_eq!(
                decode_observation_ack(invalid.as_bytes()),
                Err(ControlError::Unavailable),
                "{invalid:?}"
            );
        }
    }
}
