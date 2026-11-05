//! Descriptive held-CREATE contracts. No serialized value is an emission permit.
use crate::runtime::kubernetes::{KubernetesAuthorityV1, KubernetesPreflightResourcesV1};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

mod http;
mod policy;
mod validation;

pub use http::{
    KubernetesCreateMutationResponseV1, KubernetesCreateResultResponseV1,
    KubernetesHeldCreateOperationV1, KubernetesHeldCreateProtocolV1,
};
pub use policy::{KubernetesHeldCreateApprovalV1, KubernetesHeldCreatePolicyV1};

pub const KUBERNETES_HELD_PROFILE_V1: &str = "kubernetes_held_pod_v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KubernetesCreateFailureV1 {
    InvalidRequest,
    Conflict,
    StaleEpoch,
    StaleRevision,
    PermissionDenied,
    SupervisionInactive,
    MigrationRequired,
    NotFound,
    QuotaExceeded,
    Unavailable,
}
impl std::fmt::Display for KubernetesCreateFailureV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "invalid held Kubernetes create request",
            Self::Conflict => "held Kubernetes create conflicts with durable content or state",
            Self::StaleEpoch => "held Kubernetes create result precedes the current create epoch",
            Self::StaleRevision => "held Kubernetes create result precedes the current revision",
            Self::PermissionDenied => "held Kubernetes create authorization is unavailable or outside its authenticated scope",
            Self::SupervisionInactive => "held Kubernetes create requires active supervision",
            Self::MigrationRequired => "explicit held Kubernetes create schema migration is required",
            Self::NotFound => "held Kubernetes create or operation was not found",
            Self::QuotaExceeded => "held Kubernetes create exceeds the shared tenant quota",
            Self::Unavailable => "held Kubernetes create is unavailable",
        })
    }
}
impl std::error::Error for KubernetesCreateFailureV1 {}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesCreateIntentV1 {
    pub version: u32,
    pub create_id: String,
    pub session_id: String,
    pub control_namespace_uuid: String,
    pub broker_id: String,
    pub incarnation_id: String,
    #[serde(deserialize_with = "validation::deserialize_authority")]
    pub authority: KubernetesAuthorityV1,
    pub profile: String,
    pub image: String,
    pub resources: KubernetesPreflightResourcesV1,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesCreateTargetV1 {
    pub version: u32,
    pub create_id: String,
    pub session_id: String,
    pub authority_fingerprint: String,
    pub pod_name: String,
    pub request_sha256: String,
    pub configuration_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesCreateClaimV1 {
    pub version: u32,
    pub operation_id: String,
    pub create_id: String,
    pub intent_hash: String,
    pub expected_revision: u64,
    pub expected_create_epoch: u64,
    pub emitter_id: String,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesCreateResultV1 {
    pub version: u32,
    pub operation_id: String,
    pub create_id: String,
    pub intent_hash: String,
    pub claim_operation_id: String,
    pub expected_revision: u64,
    pub expected_create_epoch: u64,
    pub outcome: KubernetesCreateOutcomeV1,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesCreateCancelV1 {
    pub version: u32,
    pub operation_id: String,
    pub create_id: String,
    pub intent_hash: String,
    pub expected_revision: u64,
    pub expected_create_epoch: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesCreateEmissionWindowV1 {
    pub issued_at: String,
    pub not_after: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesCreateReceiptV1 {
    pub version: u32,
    pub operation_id: String,
    pub input_hash: String,
    pub control_namespace_uuid: String,
    pub audit_seq: i64,
    pub audit_hash: String,
    pub emission_window: Option<KubernetesCreateEmissionWindowV1>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum KubernetesCreateOutcomeV1 {
    CreatedHeld {
        observation: KubernetesCreateObservationV1,
        admitted_spec: Value,
        admitted_configuration_sha256: String,
        observed_at: String,
    },
    OwnershipUnknown {
        reason: KubernetesCreateUnknownReasonV1,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http_status: Option<u16>,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesCreateUnknownReasonV1 {
    TransportUnknown,
    ApiRejected,
    ResponseInvalid,
    ProfileChanged,
    AuthorityChanged,
    CancelledAfterClaim,
    RecoveredAfterClaim,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesCreateObservationV1 {
    pub version: u32,
    pub api_version: String,
    pub kind: String,
    pub metadata: KubernetesCreateMetadataV1,
    pub status: Option<KubernetesCreateStatusV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesCreateMetadataV1 {
    pub name: String,
    pub namespace: String,
    pub uid: String,
    pub resource_version: String,
    pub labels: BTreeMap<String, String>,
    pub annotation_count: u32,
    pub owner_reference_count: u32,
    pub finalizer_count: u32,
    pub deletion_requested: bool,
    pub unexpected_fields_present: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesCreateStatusV1 {
    pub phase: Option<String>,
    pub container_status_count: Option<u32>,
    pub init_container_status_count: Option<u32>,
    pub ephemeral_container_status_count: Option<u32>,
    pub start_time_present: bool,
    pub placement_fields_present: bool,
    pub unexpected_fields_present: bool,
    pub pod_scheduled: Option<KubernetesCreateSchedulingConditionV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesCreateSchedulingConditionV1 {
    pub status: String,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesCreateStateV1 {
    Reserved,
    EmissionPossible,
    CreatedHeld,
    OwnershipUnknown,
    CancelledBeforeEmission,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesCreateSnapshotV1 {
    pub version: u32,
    pub intent: KubernetesCreateIntentV1,
    pub intent_hash: String,
    pub target: KubernetesCreateTargetV1,
    pub state: KubernetesCreateStateV1,
    pub create_epoch: u64,
    pub revision: u64,
    pub quota_held: bool,
    pub claim: Option<KubernetesCreateClaimV1>,
    pub emission_window: Option<KubernetesCreateEmissionWindowV1>,
    pub last_result: Option<KubernetesCreateResultV1>,
    pub last_cancel: Option<KubernetesCreateCancelV1>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesCreateDispositionV1 {
    EmitOnce,
    ObserveOnly,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesCreateClaimResponseV1 {
    pub version: u32,
    pub receipt: KubernetesCreateReceiptV1,
    /// Historical snapshot associated with the receipt, not a current-state view.
    pub snapshot: KubernetesCreateSnapshotV1,
    pub disposition: KubernetesCreateDispositionV1,
}

macro_rules! opaque_debug {
    ($($kind:ty),+ $(,)?) => {$ (
        impl std::fmt::Debug for $kind {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($kind), "(<private>)"))
            }
        }
    )+};
}
opaque_debug!(
    KubernetesCreateIntentV1,
    KubernetesCreateOutcomeV1,
    KubernetesCreateResultV1,
    KubernetesCreateSnapshotV1,
    KubernetesCreateClaimResponseV1,
);
