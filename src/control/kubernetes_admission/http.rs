//! Closed control HTTP data. A response is never a live broker permission.
use super::{KubernetesCreateReceiptV1, KubernetesCreateSnapshotV1};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesHeldCreateOperationV1 {
    Reserve,
    Claim,
    ObserveClaim,
    Result,
    Cancel,
    Recover,
    ObserveRecovery,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesHeldCreateProtocolV1 {
    pub version: u32,
    pub schema_version: u32,
    pub tenant_id: String,
    pub control_namespace_uuid: String,
    pub broker_id: String,
    pub incarnation_id: String,
    pub binding_revision: u64,
    pub grant_revision: u64,
    pub admission_policy_configured: bool,
    pub operations: Vec<KubernetesHeldCreateOperationV1>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesCreateMutationResponseV1 {
    pub version: u32,
    pub receipt: KubernetesCreateReceiptV1,
    pub snapshot: KubernetesCreateSnapshotV1,
    pub replayed: bool,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesCreateResultResponseV1 {
    pub version: u32,
    pub receipt: KubernetesCreateReceiptV1,
    pub snapshot: KubernetesCreateSnapshotV1,
}
