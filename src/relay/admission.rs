//! Closed control contracts. Runtime statements are broker reports, not gateway inspections.
use super::RelayEndpointV1;
use crate::control::{RuntimeInspectionV1, content_hash};
use serde::{Deserialize, Serialize};

pub const MAX_CHANNEL_FACTS: usize = 4096;
pub const MAX_ACTIVE_CHANNELS: usize = 100;
pub const CHANNEL_EVENT_KIND: &str = "relay_channel.v1";

/// Reserved outside historical control-record parsers. No generic Event writer
/// may manufacture a channel fact, including an unknown version of this prefix.
pub(crate) fn reserved_kind(kind: &str) -> bool {
    crate::model::event_kind_in_namespace(kind, "relay_channel")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayAdmissionFailure {
    InvalidRequest,
    Conflict,
    Forbidden,
    NotFound,
    Capacity,
}
impl std::fmt::Display for RelayAdmissionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "invalid relay channel request",
            Self::Conflict => "relay channel identity or state conflicts with durable evidence",
            Self::Forbidden => {
                "relay channel authority is unavailable or outside its current scope"
            }
            Self::NotFound => "relay channel not found in authenticated scope",
            Self::Capacity => "relay channel journal or active capacity is exhausted",
        })
    }
}
impl std::error::Error for RelayAdmissionFailure {}
pub(crate) fn require(value: bool, error: RelayAdmissionFailure) -> anyhow::Result<()> {
    if value { Ok(()) } else { Err(error.into()) }
}
fn valid(value: bool) -> anyhow::Result<()> {
    require(value, RelayAdmissionFailure::InvalidRequest)
}
fn uuid(value: &str) -> anyhow::Result<()> {
    valid(uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value))
}
fn hash(value: &str) -> anyhow::Result<()> {
    valid(crate::control::hash(value).is_ok())
}
pub(crate) fn time(value: &str) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
    valid(!value.is_empty() && value.len() <= 64)?;
    let parsed = chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|_| RelayAdmissionFailure::InvalidRequest)?
        .with_timezone(&chrono::Utc);
    valid(parsed.timestamp() > 0)?;
    Ok(parsed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayRuntimeSourceV1 {
    BrokerReportedRuntime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayChannelAdmissionV1 {
    pub version: u32,
    pub operation_id: String,
    pub channel_id: String,
    pub epoch: u64,
    pub control_namespace_uuid: String,
    pub endpoint: RelayEndpointV1,
    pub credential_id: String,
    pub binding_revision: u64,
    pub credential_sha256_digest: String,
    pub session_id: String,
    pub expected_worker_revision: u64,
    pub inspection: RuntimeInspectionV1,
    pub started_at: String,
    pub runtime_source: RelayRuntimeSourceV1,
    pub authority_binding_hash: String,
    pub configuration_fingerprint_version: u32,
    pub configuration_fingerprint: String,
    pub expires_at: String,
}
impl RelayChannelAdmissionV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        valid(
            self.version == 1
                && self.epoch > 0
                && self.epoch < i64::MAX as u64
                && self.binding_revision > 0
                && self.binding_revision < i64::MAX as u64
                && self.expected_worker_revision > 0
                && self.expected_worker_revision < i64::MAX as u64,
        )?;
        for id in [
            &self.operation_id,
            &self.channel_id,
            &self.control_namespace_uuid,
            &self.credential_id,
            &self.session_id,
        ] {
            uuid(id)?;
        }
        valid(self.endpoint.validate().is_ok())?;
        for value in [
            &self.credential_sha256_digest,
            &self.authority_binding_hash,
            &self.configuration_fingerprint,
            &self.inspection.spec_hash,
            &self.inspection.container_id,
        ] {
            hash(value)?;
        }
        valid(
            self.configuration_fingerprint_version
                == self
                    .inspection
                    .security_preset
                    .configuration_fingerprint_version()
                && self.inspection.security_preset.requires_no_engine_logs()
                && self.inspection.network == crate::control::NetworkV1::None,
        )?;
        time(&self.started_at)?;
        time(&self.expires_at)?;
        valid(serde_json::to_vec(self)?.len() <= 8192)
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        content_hash("ai-guard:relay-channel:reservation:v1", self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayChannelClaimV1 {
    pub version: u32,
    pub operation_id: String,
    pub channel_id: String,
    pub epoch: u64,
    pub expected_revision: u64,
    pub admission_input_hash: String,
}
impl RelayChannelClaimV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        header(
            self.version,
            &self.operation_id,
            &self.channel_id,
            self.epoch,
            self.expected_revision,
            &self.admission_input_hash,
        )
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        content_hash("ai-guard:relay-channel:claim-exec:v1", self)
    }
}
fn header(
    version: u32,
    operation: &str,
    channel: &str,
    epoch: u64,
    revision: u64,
    digest: &str,
) -> anyhow::Result<()> {
    valid(
        version == 1
            && epoch > 0
            && epoch < i64::MAX as u64
            && revision > 0
            && revision < i64::MAX as u64,
    )?;
    uuid(operation)?;
    uuid(channel)?;
    hash(digest)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RelayChannelTransitionKindV1 {
    CancelReserved,
    KnownRefusedBeforeSpawn,
    Ready,
    Closing,
    Unknown,
    CloseExactWorkerTerminal {
        worker_revision: u64,
        terminal_observation_id: String,
    },
}
impl RelayChannelTransitionKindV1 {
    /// State precondition only: authority, audit, identity and terminal evidence
    /// remain separate mandatory checks before a mutation can be accepted.
    pub fn target_state(&self, state: RelayChannelStateV1) -> Option<RelayChannelStateV1> {
        use RelayChannelStateV1 as S;
        match (self, state) {
            (Self::CancelReserved, S::Reserved) => Some(S::CancelledBeforeClaim),
            (Self::KnownRefusedBeforeSpawn, S::Reserved) => Some(S::KnownRefusedBeforeSpawn),
            (Self::Ready, S::AcceptedBeforeExec) => Some(S::Ready),
            (Self::Closing, S::AcceptedBeforeExec | S::Ready) => Some(S::Closing),
            (Self::Unknown, S::AcceptedBeforeExec | S::Ready | S::Closing) => Some(S::Unknown),
            (
                Self::CloseExactWorkerTerminal { .. },
                S::AcceptedBeforeExec | S::Ready | S::Closing | S::Unknown,
            ) => Some(S::Closed),
            _ => None,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayChannelTransitionV1 {
    pub version: u32,
    pub operation_id: String,
    pub channel_id: String,
    pub epoch: u64,
    pub expected_revision: u64,
    pub admission_input_hash: String,
    pub transition: RelayChannelTransitionKindV1,
}
impl RelayChannelTransitionV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        header(
            self.version,
            &self.operation_id,
            &self.channel_id,
            self.epoch,
            self.expected_revision,
            &self.admission_input_hash,
        )?;
        if let RelayChannelTransitionKindV1::CloseExactWorkerTerminal {
            worker_revision,
            terminal_observation_id,
        } = &self.transition
        {
            valid(*worker_revision > 0 && *worker_revision < i64::MAX as u64)?;
            uuid(terminal_observation_id)?;
        }
        Ok(())
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        content_hash("ai-guard:relay-channel:transition:v1", self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayChannelStateV1 {
    Reserved,
    CancelledBeforeClaim,
    KnownRefusedBeforeSpawn,
    AcceptedBeforeExec,
    Ready,
    Closing,
    Unknown,
    Closed,
}
impl RelayChannelStateV1 {
    /// Pure state precondition, never proof of current execution authority.
    pub fn accepts_claim(self) -> bool {
        self == Self::Reserved
    }
    pub fn active(self) -> bool {
        !matches!(
            self,
            Self::CancelledBeforeClaim | Self::KnownRefusedBeforeSpawn | Self::Closed
        )
    }
    pub fn current_dispatch(self) -> bool {
        matches!(self, Self::AcceptedBeforeExec | Self::Ready)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayChannelSnapshotV1 {
    pub version: u32,
    pub tenant_id: String,
    pub control_namespace_uuid: String,
    pub channel_id: String,
    pub epoch: u64,
    pub admission_operation_id: String,
    pub admission_input_hash: String,
    pub endpoint: RelayEndpointV1,
    pub credential_id: String,
    pub binding_revision: u64,
    pub session_id: String,
    pub expires_at: String,
    pub revision: u64,
    pub state: RelayChannelStateV1,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayChannelReceiptV1 {
    pub version: u32,
    pub operation_id: String,
    pub channel_id: String,
    pub epoch: u64,
    pub input_hash: String,
    pub audit_seq: i64,
    pub audit_hash: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayChannelResultV1 {
    pub version: u32,
    pub replayed: bool,
    pub channel: RelayChannelSnapshotV1,
    pub receipt: RelayChannelReceiptV1,
}

/// An exact retained intention to look up, never an instruction to apply it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "request",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RelayChannelPendingV1 {
    Claim(RelayChannelClaimV1),
    Transition(RelayChannelTransitionV1),
}
impl RelayChannelPendingV1 {
    pub fn accepts_state(&self, state: RelayChannelStateV1) -> bool {
        match self {
            Self::Claim(_) => state.accepts_claim(),
            Self::Transition(input) => input.transition.target_state(state).is_some(),
        }
    }
    pub fn validate(&self, admission: &RelayChannelAdmissionV1) -> anyhow::Result<()> {
        let (channel, epoch, admission_hash) = match self {
            Self::Claim(input) => {
                input.validate()?;
                (&input.channel_id, input.epoch, &input.admission_input_hash)
            }
            Self::Transition(input) => {
                input.validate()?;
                (&input.channel_id, input.epoch, &input.admission_input_hash)
            }
        };
        valid(
            channel == &admission.channel_id
                && epoch == admission.epoch
                && admission_hash == &admission.input_hash()?,
        )
    }
    pub fn operation_id(&self) -> &str {
        match self {
            Self::Claim(input) => &input.operation_id,
            Self::Transition(input) => &input.operation_id,
        }
    }
    pub fn expected_revision(&self) -> u64 {
        match self {
            Self::Claim(input) => input.expected_revision,
            Self::Transition(input) => input.expected_revision,
        }
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        match self {
            Self::Claim(input) => input.input_hash(),
            Self::Transition(input) => input.input_hash(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayChannelReconcileV1 {
    pub version: u32,
    pub admission: RelayChannelAdmissionV1,
    pub pending: Option<RelayChannelPendingV1>,
}
impl RelayChannelReconcileV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        valid(self.version == 1)?;
        self.admission.validate()?;
        if let Some(pending) = &self.pending {
            pending.validate(&self.admission)?;
        }
        valid(serde_json::to_vec(self)?.len() <= 16_384)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RelayChannelPendingResolutionV1 {
    Recorded {
        receipt: RelayChannelReceiptV1,
    },
    /// Absent in this snapshot and its exact CAS can never pass again.
    InapplicableCas,
    /// Absent, same CAS revision and incompatible state. Every possible later
    /// state change increments that revision, so this intention cannot apply.
    InapplicableState,
    Unresolved,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayChannelPendingHistoryV1 {
    pub pending: RelayChannelPendingV1,
    pub resolution: RelayChannelPendingResolutionV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
// Durable serde history outcome contract; preserve the value shape.
#[allow(clippy::large_enum_variant)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RelayChannelHistoryOutcomeV1 {
    Historical {
        channel: RelayChannelSnapshotV1,
        admission_receipt: RelayChannelReceiptV1,
        pending: Option<RelayChannelPendingHistoryV1>,
    },
    /// Absence at a snapshot is insufficient to resolve an in-flight Reserve.
    AdmissionUnresolved,
    /// The exact incarnation is durably terminal under the same serialized
    /// snapshot. This proves no admission can arrive later, never process join.
    AdmissionNeverRecordedExactTerminal {
        admission_operation_id: String,
        channel_id: String,
        epoch: u64,
        endpoint: RelayEndpointV1,
        worker_revision: u64,
        terminal_observation_receipt: crate::control::MutationReceiptV1,
    },
}
/// A verified historical snapshot, with no conversion into an exec capability.
/// The head is a real observed audit prefix, not a new receipt or signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayChannelReconciliationV1 {
    pub version: u32,
    pub tenant_id: String,
    pub control_namespace_uuid: String,
    pub admission_input_hash: String,
    pub audit_seq: i64,
    pub audit_hash: String,
    pub outcome: RelayChannelHistoryOutcomeV1,
}
