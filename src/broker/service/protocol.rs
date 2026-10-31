//! Versioned local business protocol. Credentials belong only to the ephemeral
//! envelope, never to Submit bytes, job data, errors or replies.
use crate::{
    broker::journal::{RejectionCodeV1, RejectionPhaseV1},
    control::{RuntimeStatusV1, WorkerStateV1},
};
use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf};
use uuid::Uuid;

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
    /// Confirmed metadata only; available independently of business loading.
    RecoveryStatus {
        operation_id: Uuid,
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
    RecoveryStatus {
        status: RecoveryStatusV1,
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
