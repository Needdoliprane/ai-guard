//! Pure retained-CREATE profile and response validation. No permission factory,
//! CREATE transport or execution authority is provided by these data helpers.
use super::KubernetesResult;
use crate::control::kubernetes_admission::{
    KubernetesCreateIntentV1, KubernetesCreateObservationV1, KubernetesCreateOutcomeV1,
    KubernetesCreateTargetV1,
};
use serde_json::Value;

#[path = "admission_coordinator.rs"]
pub(crate) mod coordinator;
#[path = "admission_journal.rs"]
pub(crate) mod journal;
#[path = "admission_json.rs"]
mod json;
#[path = "admission_permission.rs"]
pub(crate) mod permission;
#[path = "admission_profile.rs"]
mod profile;
#[path = "admission_response.rs"]
mod response;
#[path = "admission_transport.rs"]
mod transport;

pub(crate) struct PreparedHeldCreate {
    target: KubernetesCreateTargetV1,
    pod_bytes: Vec<u8>,
}
impl PreparedHeldCreate {
    pub(crate) fn target(&self) -> &KubernetesCreateTargetV1 {
        &self.target
    }
    pub(crate) fn pod_bytes(&self) -> &[u8] {
        &self.pod_bytes
    }
}

pub(crate) fn prepare(intent: &KubernetesCreateIntentV1) -> KubernetesResult<PreparedHeldCreate> {
    profile::prepare(intent)
}

pub(crate) fn parse_created_held(
    intent: &KubernetesCreateIntentV1,
    response: &[u8],
    observed_at: &str,
) -> KubernetesResult<KubernetesCreateOutcomeV1> {
    response::parse(intent, response, observed_at)
}

pub(crate) fn validate_created_held(
    intent: &KubernetesCreateIntentV1,
    observation: &KubernetesCreateObservationV1,
    admitted_spec: &Value,
    admitted_configuration_sha256: &str,
) -> KubernetesResult<()> {
    response::validate(
        intent,
        observation,
        admitted_spec,
        admitted_configuration_sha256,
    )
}

#[cfg(test)]
#[path = "admission_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "admission_flow_tests.rs"]
mod flow_tests;

#[cfg(test)]
#[path = "admission_transport_tests.rs"]
mod transport_tests;

#[cfg(test)]
#[path = "admission_coordinator_tests.rs"]
mod coordinator_tests;
