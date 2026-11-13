//! Versioned local business protocol. Credentials belong only to the ephemeral
//! envelope, never to Submit bytes, job data, errors or replies.
use crate::{
    broker::journal::{RejectionCodeV1, RejectionPhaseV1},
    control::{RuntimeStatusV1, WorkerStateV1},
};
use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf};
use uuid::Uuid;

pub use super::recovery_inventory::{
    RecoveryInventoryCoverageV1, RecoveryInventoryEntryV1, RecoveryInventoryIssueCodeV1,
    RecoveryInventoryIssueV1, RecoveryInventoryPhaseV1, RecoveryInventoryV1,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EnvelopeV1 {
    pub(super) version: u32,
    pub(super) profile_id: String,
    pub(super) credential: String,
    pub(super) request: BrokerCommandV1,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum BrokerCommandV1 {
    Submit {
        payload: String,
    },
    Status {
        operation_id: Uuid,
    },
    /// Local relay attachment metadata only; never a runtime or workload proof.
    RelayStatus {
        operation_id: Uuid,
    },
    /// Confirmed metadata only; available independently of business loading.
    RecoveryStatus {
        operation_id: Uuid,
    },
    /// Explicit D5 diagnostic; the historical RecoveryStatus remains V1.
    RecoveryStatusV2 {
        operation_id: Uuid,
    },
    /// Discover accepted job UUIDs from confirmed STOP metadata only.
    RecoveryInventory {
        after: Option<Uuid>,
        limit: u32,
    },
    List {
        after: Option<Uuid>,
        limit: u32,
    },
    Approve {
        operation_id: Uuid,
        command_id: Uuid,
        request_hash: String,
    },
    Cancel {
        operation_id: Uuid,
        command_id: Uuid,
    },
    /// Audit an operator request to inhibit one original Arm before its claim.
    AbandonBeforeClaim {
        request: super::AbandonBeforeClaimV1,
    },
    Resume {
        operation_id: Uuid,
        command_id: Uuid,
    },
    Export {
        operation_id: Uuid,
        command_id: Uuid,
        destination: PathBuf,
    },
}
impl fmt::Debug for BrokerCommandV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BrokerCommandV1(<private request>)")
    }
}

macro_rules! error_codes {
    ($($variant:ident),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum BrokerErrorCodeV1 { $($variant),* }
        impl<'de> Deserialize<'de> for BrokerErrorCodeV1 {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                use serde::de::Error as _;
                #[derive(Deserialize)]
                #[serde(rename_all = "snake_case")]
                enum Wire { $($variant),* }
                Ok(match Wire::deserialize(deserializer)
                    .map_err(|_| D::Error::custom("invalid broker error code"))? {
                    $(Wire::$variant => Self::$variant),*
                })
            }
        }
    }
}
error_codes!(
    Unauthorized,
    Forbidden,
    Unavailable,
    IncompatibleControl,
    Busy,
    InvalidInput,
    Conflict,
    Interrupted,
    LimitExceeded,
    NeedsReconciliation
);

impl fmt::Display for BrokerErrorCodeV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unauthorized => "broker credential is not authenticated",
            Self::Forbidden => "broker request is forbidden for this identity",
            Self::Unavailable => "broker dependency is unavailable",
            Self::IncompatibleControl => "broker control is incompatible or inactive",
            Self::Busy => "broker capacity is temporarily busy",
            Self::InvalidInput => "invalid broker request",
            Self::Conflict => "broker operation identity or revision conflicts",
            Self::Interrupted => "broker preparation was interrupted",
            Self::LimitExceeded => "broker configured capacity is exhausted",
            Self::NeedsReconciliation => "broker operation needs reconciliation",
        })
    }
}
impl std::error::Error for BrokerErrorCodeV1 {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStateV1 {
    Absent,
    ReservedInProcess,
    Interrupted,
    Rejected,
    Accepted,
}

/// Cached control/journal facts, explicitly distinct from a fresh engine probe.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobStatusV1 {
    pub operation_id: Uuid,
    pub state: JobStateV1,
    pub head_revision: Option<u64>,
    pub head_hash: Option<String>,
    pub intent_hash: Option<String>,
    pub cancel_requested: bool,
    pub approval_submitted: bool,
    pub failure_count: usize,
    pub pending_rpc_count: Option<usize>,
    pub worker_state: Option<WorkerStateV1>,
    pub quota_held: Option<bool>,
    pub runtime_status: Option<RuntimeStatusV1>,
    pub last_observed_at: Option<String>,
    pub review_request_hash: Option<String>,
    pub last_decision: Option<crate::model::Decision>,
    pub executor_task_running: bool,
    pub executor_error: Option<BrokerErrorCodeV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayAttachmentPhaseV1 {
    AwaitingIssuedWorkload,
    Admitted,
    Unknown,
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayStatusErrorV1 {
    Unavailable,
    InvalidMaterial,
    Scope,
    Expired,
    Clock,
    Cancelled,
    Capacity,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayStatusSourceV1 {
    BrokerRetainedRegistry,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayStatusFreshnessV1 {
    RetainedAtLastObservation,
    NotObserved,
}
/// A distinct closed diagnostic keeps the historical Status/Page V1 shape.
/// No credential ID, proof, private path or raw remote/runtime error is exposed.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayStatusV1 {
    pub version: u32,
    pub operation_id: Uuid,
    pub tenant_id: String,
    pub configured: bool,
    pub source: RelayStatusSourceV1,
    pub freshness: RelayStatusFreshnessV1,
    pub phase: Option<RelayAttachmentPhaseV1>,
    pub endpoint: Option<crate::relay::RelayEndpointV1>,
    pub channel_id: Option<String>,
    pub epoch: Option<u64>,
    pub core_state: Option<crate::relay::admission::RelayChannelStateV1>,
    pub last_observed_at: Option<String>,
    pub last_error: Option<RelayStatusErrorV1>,
    pub local_joined: Option<bool>,
    pub reconciliation_required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalModeV1 {
    Full,
    StopOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryDataSourceV1 {
    ConfirmedJournal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryRuntimeEvidenceV1 {
    NotQueried,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryCommandV1 {
    pub command_id: Uuid,
    pub revision: u64,
    pub checkpoint_hash: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryTargetKindV1 {
    Worker,
    Helper { helper_id: String },
}

/// Original acknowledged Arm facts, never a new runtime observation or permit.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryTargetV1 {
    pub arm_id: String,
    pub worker_id: String,
    pub attempt_id: String,
    pub subject: RecoveryTargetKindV1,
    pub original_clock: crate::control::HostClockSampleV1,
    pub original_timeout_seconds: u32,
    pub original_deadline_ns_decimal: String,
    pub cancel_command_id: Option<Uuid>,
}

/// Current-process arbitration metadata, never runtime or permission proof.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryPreclaimV1 {
    pub version: u32,
    pub expected_epoch: Option<Uuid>,
    pub generation: Option<u64>,
    pub state: super::PreclaimStateV1,
    pub source: PreclaimSourceV1,
    pub freshness: PreclaimFreshnessV1,
    pub command_id: Option<Uuid>,
    pub signal_audit_confirmed: bool,
    pub retirement_continuation_retained: Option<bool>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreclaimSourceV1 {
    ProcessLocal,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreclaimFreshnessV1 {
    CurrentProcessState,
    Unknown,
}

/// A separate reply keeps existing Status/Page payloads unchanged. Its source
/// and runtime evidence are explicit, including in degraded operation.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryStatusV1 {
    pub version: u32,
    pub operation_id: Uuid,
    pub tenant_id: String,
    pub journal_mode: JournalModeV1,
    pub source: RecoveryDataSourceV1,
    pub runtime_evidence: RecoveryRuntimeEvidenceV1,
    pub head_revision: u64,
    pub head_hash: String,
    pub incomplete: bool,
    pub reconciliation_required: bool,
    pub cancel_commands: Vec<RecoveryCommandV1>,
    pub known_arms: Vec<RecoveryTargetV1>,
    pub pending_arms: usize,
}

/// Explicit extended diagnostic. Journal/retirements share a confirmed snapshot;
/// process-local preclaim data is sampled separately, not an atomic joined view.
/// Neither source establishes runtime evidence or execution permission.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryStatusV2 {
    pub version: u32,
    pub journal: RecoveryStatusV1,
    pub preclaim: Vec<RecoveryPreclaimTargetV2>,
    pub retirements: Vec<RecoveryRetirementV1>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryPreclaimTargetV2 {
    pub arm_id: String,
    pub preclaim: RecoveryPreclaimV1,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetirementPhaseV1 {
    Pending,
    Final,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRetirementV1 {
    pub arm_id: Uuid,
    pub retirement_id: Uuid,
    pub phase: RetirementPhaseV1,
    pub intention_audit_confirmed: bool,
    pub guardian_receipt_hash: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReplyEnvelopeV1 {
    pub(super) version: u32,
    pub(super) reply: BrokerReplyV1,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum BrokerReplyV1 {
    Accepted {
        operation_id: Uuid,
        intent_hash: String,
        submit_hash: String,
        revision: u64,
        checkpoint_hash: String,
        replayed: bool,
        provider_resolution: Option<crate::provider::ProviderResolutionV1>,
    },
    Rejected {
        operation_id: Uuid,
        phase: RejectionPhaseV1,
        code: RejectionCodeV1,
        record_hash: String,
        replayed: bool,
    },
    Status {
        status: JobStatusV1,
    },
    RelayStatus {
        status: RelayStatusV1,
    },
    RecoveryStatus {
        status: RecoveryStatusV1,
    },
    RecoveryStatusV2 {
        status: RecoveryStatusV2,
    },
    RecoveryInventory {
        inventory: RecoveryInventoryV1,
    },
    Page {
        entries: Vec<JobStatusV1>,
        next_after: Option<Uuid>,
    },
    CommandAcknowledged {
        operation_id: Uuid,
        command_id: Uuid,
        input_hash: String,
        revision: u64,
        checkpoint_hash: String,
    },
    /// Only the signal is audit-ACKed. This does not assert Closing or Final.
    AbandonRequested {
        operation_id: Uuid,
        arm_id: Uuid,
        command_id: Uuid,
        input_hash: String,
        revision: u64,
        checkpoint_hash: String,
    },
    AbandonedBeforeClaim {
        operation_id: Uuid,
        arm_id: Uuid,
        container_id: String,
        final_revision: u64,
        final_checkpoint_hash: String,
    },
    ExportAcknowledged {
        operation_id: Uuid,
        request_id: Uuid,
        manifest_digest: String,
        files: usize,
    },
    Error {
        code: BrokerErrorCodeV1,
    },
}

#[cfg(test)]
mod export_report_compatibility_tests {
    use super::BrokerReplyV1;

    // Frozen subset of the closed V1 decoder shipped at 5e52807. It must read
    // the current producer's acknowledgement, not only a legacy input fixture.
    #[derive(serde::Deserialize, Debug)]
    #[serde(rename_all = "snake_case", deny_unknown_fields)]
    enum LegacyV1Reply {
        ExportAcknowledged {
            operation_id: uuid::Uuid,
            request_id: uuid::Uuid,
            manifest_digest: String,
            files: usize,
        },
    }

    #[test]
    fn new_export_reply_is_readable_by_the_legacy_v1_client() {
        let operation_id = uuid::Uuid::from_u128(1);
        let request_id = uuid::Uuid::from_u128(2);
        let current = BrokerReplyV1::ExportAcknowledged {
            operation_id,
            request_id,
            manifest_digest: "synthetic-manifest".into(),
            files: 2,
        };
        let bytes = serde_json::to_vec(&current).unwrap();
        let legacy: LegacyV1Reply = serde_json::from_slice(&bytes)
            .expect("the current V1 reply must remain readable by the legacy client");
        let LegacyV1Reply::ExportAcknowledged {
            operation_id: observed_operation,
            request_id: observed_request,
            manifest_digest,
            files,
        } = legacy;
        assert_eq!(observed_operation, operation_id);
        assert_eq!(observed_request, request_id);
        assert_eq!(manifest_digest, "synthetic-manifest");
        assert_eq!(files, 2);
    }

    #[test]
    fn legacy_export_reply_preserves_unknown_omissions() {
        let legacy = r#"{"export_acknowledged":{"operation_id":"00000000-0000-4000-8000-000000000001","request_id":"00000000-0000-4000-8000-000000000002","manifest_digest":"synthetic-manifest","files":2}}"#;
        let reply: BrokerReplyV1 = serde_json::from_str(legacy).unwrap();
        assert!(matches!(&reply, BrokerReplyV1::ExportAcknowledged { .. }));
        assert_eq!(serde_json::to_string(&reply).unwrap(), legacy);
    }
}

#[cfg(test)]
mod relay_status_compatibility_tests {
    use super::*;

    // Frozen closed V1 fields: a separate relay command must not change the
    // shape accepted by an existing Status/Page consumer.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyStatus {
        operation_id: Uuid,
        state: JobStateV1,
        head_revision: Option<u64>,
        head_hash: Option<String>,
        intent_hash: Option<String>,
        cancel_requested: bool,
        approval_submitted: bool,
        failure_count: usize,
        pending_rpc_count: Option<usize>,
        worker_state: Option<WorkerStateV1>,
        quota_held: Option<bool>,
        runtime_status: Option<RuntimeStatusV1>,
        last_observed_at: Option<String>,
        review_request_hash: Option<String>,
        last_decision: Option<crate::model::Decision>,
        executor_task_running: bool,
        executor_error: Option<BrokerErrorCodeV1>,
    }
    #[test]
    fn relay_diagnostic_keeps_the_legacy_status_payload_closed_and_unchanged() {
        let fixture = serde_json::json!({
            "operation_id":Uuid::from_u128(1), "state":"absent", "head_revision":null, "head_hash":null,
            "intent_hash":null, "cancel_requested":false, "approval_submitted":false, "failure_count":0,
            "pending_rpc_count":null, "worker_state":null, "quota_held":null, "runtime_status":null,
            "last_observed_at":null, "review_request_hash":null, "last_decision":null,
            "executor_task_running":false, "executor_error":null
        });
        let status: JobStatusV1 = serde_json::from_value(fixture.clone()).unwrap();
        let produced = serde_json::to_value(BrokerReplyV1::Status { status }).unwrap();
        assert_eq!(produced, serde_json::json!({"status":{"status":fixture}}));
        let legacy: LegacyStatus =
            serde_json::from_value(produced["status"]["status"].clone()).unwrap();
        assert_eq!(legacy.operation_id, Uuid::from_u128(1));
        let _all_legacy_fields = (
            legacy.state,
            legacy.head_revision,
            legacy.head_hash,
            legacy.intent_hash,
            legacy.cancel_requested,
            legacy.approval_submitted,
            legacy.failure_count,
            legacy.pending_rpc_count,
            legacy.worker_state,
            legacy.quota_held,
            legacy.runtime_status,
            legacy.last_observed_at,
            legacy.review_request_hash,
            legacy.last_decision,
            legacy.executor_task_running,
            legacy.executor_error,
        );
        let command = serde_json::to_value(BrokerCommandV1::Status {
            operation_id: Uuid::from_u128(1),
        })
        .unwrap();
        assert_eq!(
            command,
            serde_json::json!({"status":{"operation_id":Uuid::from_u128(1)}})
        );
    }
}
