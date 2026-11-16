//! Descriptive R2 HTTP envelopes. Deserialization never grants an effect or
//! creates the private live confirmation used by a local preparation journal.
use super::*;
use crate::broker_registry::BrokerIdentityViewV1;

pub(crate) const RESERVATIONS_PATH: &str = "/api/v1/brokers/kubernetes/session-reservations";
pub(crate) const PROTOCOL_PATH: &str = "/api/v1/brokers/kubernetes/session-reservation-protocol";
pub(crate) const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub(crate) const MAX_RESPONSE_BYTES: usize = 128 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionProtocolV2 {
    pub(crate) version: u32,
    pub(crate) identity: BrokerIdentityViewV1,
    pub(crate) reservation_policy_configured: bool,
    pub(crate) recovery_policy_configured: bool,
    pub(crate) effects_available: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionConfirmationRequestV2 {
    pub(crate) version: u32,
    pub(crate) request_id: String,
    pub(crate) reservation_input_hash: String,
}
impl SessionConfirmationRequestV2 {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        validation::require(self.version == 2)?;
        validation::uuid(&self.request_id)?;
        validation::hash(&self.reservation_input_hash)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionConfirmationResponseV2 {
    pub(crate) version: u32,
    pub(crate) request_id: String,
    pub(crate) identity: BrokerIdentityViewV1,
    pub(crate) current: KubernetesSessionReservationCurrentV2,
}
