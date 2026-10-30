//! Versioned tenant control contracts. Runtime effects belong to the trusted broker.
use crate::model::{CommandSpec, DecisionRequest};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod supervision;
pub use supervision::{
    ArmReceiptV1, ArmResultV1, ArmSubjectV1, ArmV1, ArmedStartClaimV1, BindLegacyIntentV1,
    ControlProtocolV1, ExportRequestV1, HelperClaimResultV1, HelperCleanupV1,
    HelperExecutionPolicyV1, HelperObservationV1, HelperPermissionV1, HelperPurposeV1,
    HelperStartClaimV1, HostClockMethodV1, HostClockSampleV1, ImportVerificationReportV1,
    SupervisedAdmissionV1, SupervisionProtocolV1, SupervisionStateV1, helper_execution_policy_v1,
};

macro_rules! closed_enum {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
    };
}
closed_enum!(DecisionPhaseV1 { Initial, Approved });
closed_enum!(RuntimeKindV1 { Docker, Podman });
closed_enum!(NetworkV1 { None, Bridge });
closed_enum!(SecurityPresetV1 {
    FixedSandboxV1,
    FixedSandboxV2,
    FixedSandboxInteractiveV3
});
impl SecurityPresetV1 {
    /// Whether the trusted runtime must verify that engine output logging is disabled.
    pub const fn requires_no_engine_logs(&self) -> bool {
        matches!(self, Self::FixedSandboxV2 | Self::FixedSandboxInteractiveV3)
    }

    pub const fn is_interactive(&self) -> bool {
        matches!(self, Self::FixedSandboxInteractiveV3)
    }

    /// Selects the private runtime fingerprint without changing historical v1 hashes.
    pub const fn configuration_fingerprint_version(&self) -> u32 {
        match self {
            Self::FixedSandboxV1 => 1,
            Self::FixedSandboxV2 => 2,
            Self::FixedSandboxInteractiveV3 => 3,
        }
    }
}
closed_enum!(RestartPolicyV1 { No });
closed_enum!(WorkerStateV1 {
    Reserved,
    Preparing,
    Created,
    StartIssued,
    Running,
    Stopping,
    Terminal,
    InDoubt,
    Cancelled
});
closed_enum!(ClaimDispositionV1 {
    EmitOnce,
    AlreadyConsumed
});
closed_enum!(AdmissionRefusalV1 { Policy, Quota });
closed_enum!(RuntimeStatusV1 {
    Created,
    Running,
    Exited,
    Dead,
    Unknown
});
closed_enum!(AllocationProvenanceV1 { Observed });

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionReceiptV1 {
    pub version: u32,
    pub request_id: String,
    pub request_hash: String,
    pub phase: DecisionPhaseV1,
    pub audit_seq: i64,
    pub audit_hash: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchSpecV1 {
    pub version: u32,
    pub session_id: String,
    pub command: CommandSpec,
    pub profile: String,
    pub runtime_kind: RuntimeKindV1,
    pub authority_id: String,
    pub image_ref: String,
    pub image_id: String,
    pub network: NetworkV1,
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub pids_limit: u32,
    pub timeout_seconds: u32,
    pub security_preset: SecurityPresetV1,
    pub restart_policy: RestartPolicyV1,
    pub workspace_manifest_digest: String,
    pub workspace_import_policy_version: u32,
    pub provider: String,
    pub capture_output: bool,
    pub credential_binding_id: Option<String>,
    pub credential_names: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::model::deserialize_provider_resolution"
    )]
    pub provider_resolution: Option<crate::provider::ProviderResolutionV1>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionV1 {
    pub version: u32,
    pub operation_id: String,
    pub decision_request: DecisionRequest,
    pub receipt: DecisionReceiptV1,
    pub launch: LaunchSpecV1,
}
impl AdmissionV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1 && self.receipt.version == 1 && self.receipt.audit_seq > 0)?;
        uuid(&self.operation_id)?;
        uuid(&self.decision_request.request_id)?;
        self.launch.validate()?;
        hash(&self.receipt.request_hash)?;
        hash(&self.receipt.audit_hash)?;
        let request = &self.decision_request;
        invalid(crate::policy::valid_provider_context(request))?;
        invalid(
            self.receipt.request_id == request.request_id
                && self.receipt.request_hash == crate::policy::request_hash(request)
                && self.launch.command == request.command
                && self.launch.security_preset.is_interactive() == request.execution_mode.is_some()
                && self.launch.provider_resolution == request.provider_resolution
                && self.launch.profile == request.profile
                && request.session_id.as_deref() == Some(self.launch.session_id.as_str())
                && self.receipt.phase
                    == if request.human_approved {
                        DecisionPhaseV1::Approved
                    } else {
                        DecisionPhaseV1::Initial
                    }
                && (!request.human_approved
                    || request.approval_hash.as_deref()
                        == Some(self.receipt.request_hash.as_str()))
                && (request.human_approved || request.approval_hash.is_none()),
        )?;
        invalid(serde_json::to_vec(self)?.len() <= 65536)
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        content_hash("ai-guard:admission:v1", self)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaPolicyV1 {
    pub version: u32,
    pub max_active_workers: u32,
    pub max_cpu_millis: u32,
    pub max_memory_bytes: u64,
    pub max_timeout_seconds: u32,
}
impl Default for QuotaPolicyV1 {
    fn default() -> Self {
        Self {
            version: 1,
            max_active_workers: 3,
            max_cpu_millis: 6000,
            max_memory_bytes: 3 * 1024 * 1024 * 1024,
            max_timeout_seconds: 3600,
        }
    }
}
impl QuotaPolicyV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(
            self.version == 1
                && self.max_active_workers <= 100
                && self.max_cpu_millis <= 200_000
                && self.max_memory_bytes <= 100 * 1024 * 1024 * 1024
                && self.max_timeout_seconds > 0
                && self.max_timeout_seconds <= 86400,
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeInspectionV1 {
    pub authority_id: String,
    pub runtime_kind: RuntimeKindV1,
    pub container_id: String,
    pub stable_name: String,
    pub volume_name: String,
    pub image_id: String,
    pub spec_hash: String,
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub pids_limit: u32,
    pub network: NetworkV1,
    pub security_preset: SecurityPresetV1,
    pub restart_policy: RestartPolicyV1,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeObservationV1 {
    pub inspection: RuntimeInspectionV1,
    pub observed_at: String,
    pub status: RuntimeStatusV1,
    pub running: Option<bool>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
    pub oom_killed: Option<bool>,
    pub restart_count: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerTransitionV1 {
    Prepare,
    CreateIssued,
    Created {
        inspection: RuntimeInspectionV1,
    },
    StartClaim {
        emitter_id: String,
    },
    Observe {
        observation: RuntimeObservationV1,
    },
    Unknown {
        reason: String,
    },
    Stop,
    CancelReserved,
    CleanupResult {
        removed: bool,
        error: Option<String>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerMutationV1 {
    pub version: u32,
    pub operation_id: String,
    pub worker_id: String,
    pub attempt_id: String,
    pub expected_revision: u64,
    pub transition: WorkerTransitionV1,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerAttemptV1 {
    pub attempt_id: String,
    pub stable_name: String,
    pub volume_name: String,
    pub container_id: Option<String>,
    pub created_inspection: Option<RuntimeInspectionV1>,
    pub create_operation_id: Option<String>,
    pub start_operation_id: Option<String>,
    pub start_emitter_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerV1 {
    pub worker_id: String,
    pub admission_operation_id: String,
    pub request_id: String,
    pub receipt: DecisionReceiptV1,
    pub launch: LaunchSpecV1,
    pub spec_hash: String,
    pub policy_hash: String,
    pub state: WorkerStateV1,
    pub quota_held: bool,
    pub revision: u64,
    pub attempt: WorkerAttemptV1,
    pub observed_started_at: Option<String>,
    pub allocation_inconsistent: bool,
    pub last_observation: Option<RuntimeObservationV1>,
    pub unknown_reason: Option<String>,
    pub cleanup_removed: Option<bool>,
    pub cleanup_error: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationReceiptV1 {
    pub operation_id: String,
    pub worker_id: Option<String>,
    pub revision: Option<u64>,
    pub input_hash: String,
    pub audit_seq: i64,
    pub audit_hash: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionResultV1 {
    pub replayed: bool,
    pub accepted: bool,
    pub refusal: Option<AdmissionRefusalV1>,
    pub worker: Option<WorkerV1>,
    pub receipt: MutationReceiptV1,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationResultV1 {
    pub worker: WorkerV1,
    pub receipt: MutationReceiptV1,
    pub disposition: Option<ClaimDispositionV1>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationV1 {
    pub attempt_id: String,
    pub worker_id: String,
    pub terminal_observation_id: String,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ns: String,
    pub cpu_millicore_nanoseconds: String,
    pub memory_byte_nanoseconds: String,
    pub method_version: u32,
    pub billable: bool,
    pub source: AllocationSourceV1,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationSourceV1 {
    pub authority_id: String,
    pub runtime_kind: RuntimeKindV1,
    pub container_id: String,
    pub spec_hash: String,
    pub provenance: AllocationProvenanceV1,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaUsageV1 {
    pub policy: QuotaPolicyV1,
    pub held_workers: u32,
    pub held_cpu_millis: u64,
    pub held_memory_bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationReportV1 {
    pub from_version: u32,
    pub to_version: u32,
    pub backup_path: String,
    pub genesis_seq: i64,
    pub genesis_hash: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlFailure {
    InvalidRequest,
    Conflict,
    NotFound,
    MigrationRequired,
    SupervisionInactive,
}
impl std::fmt::Display for ControlFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "invalid worker control request",
            Self::Conflict => "worker operation conflicts with durable content or state",
            Self::NotFound => "worker or operation not found",
            Self::MigrationRequired => "explicit control schema migration is required",
            Self::SupervisionInactive => "supervision is inactive: maintenance, reconciliation and runtime fencing are required before new execution",
        })
    }
}
impl std::error::Error for ControlFailure {}
pub(crate) fn invalid(condition: bool) -> anyhow::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(ControlFailure::InvalidRequest.into())
    }
}
pub(crate) fn conflict(condition: bool) -> anyhow::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(ControlFailure::Conflict.into())
    }
}
pub(crate) fn uuid(value: &str) -> anyhow::Result<()> {
    invalid(uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value))
}
pub(crate) fn hash(value: &str) -> anyhow::Result<()> {
    invalid(
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
    )
}
pub(crate) fn bounded(value: &str, max: usize) -> anyhow::Result<()> {
    invalid(!value.is_empty() && value.len() <= max && !value.chars().any(char::is_control))
}
pub fn content_hash<T: Serialize>(domain: &str, value: &T) -> anyhow::Result<String> {
    let mut digest = Sha256::new();
    digest.update(domain.as_bytes());
    digest.update([0]);
    digest.update(serde_json::to_vec(value)?);
    Ok(hex::encode(digest.finalize()))
}
impl LaunchSpecV1 {
    pub fn spec_hash(&self) -> anyhow::Result<String> {
        content_hash("ai-guard:launch:v1", self)
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(
            self.version == 1
                && self.cpu_millis == 2000
                && self.memory_bytes == 1073741824
                && self.pids_limit == 256
                && self.timeout_seconds > 0
                && self.timeout_seconds <= 86400
                && self.workspace_import_policy_version == 1,
        )?;
        invalid(!self.security_preset.is_interactive() || !self.capture_output)?;
        uuid(&self.session_id)?;
        bounded(&self.authority_id, 256)?;
        bounded(&self.image_ref, 512)?;
        let image = self
            .image_id
            .strip_prefix("sha256:")
            .context(ControlFailure::InvalidRequest)?;
        hash(image)?;
        hash(&self.workspace_manifest_digest)?;
        invalid(
            ["strict", "review", "observe"].contains(&self.profile.as_str())
                && (self.network == NetworkV1::None || self.profile == "observe"),
        )?;
        bounded(&self.provider, 80)?;
        if let Some(resolution) = &self.provider_resolution {
            resolution.validate()?;
            resolution.validate_argv(&self.command.argv)?;
            invalid(
                self.provider == resolution.provider_name()
                    && self.credential_binding_id.as_deref()
                        == Some(resolution.credential_binding_id.to_string().as_str())
                    && self.credential_names == resolution.credential_names
                    && self.security_preset.requires_no_engine_logs()
                    && self.security_preset.is_interactive()
                        == resolution.transport.is_interactive()
                    && self.profile == "observe"
                    && self.network == NetworkV1::Bridge,
            )?;
        }
        invalid(self.credential_names.len() <= 64)?;
        if let Some(binding) = &self.credential_binding_id {
            uuid(binding)?;
        }
        invalid(self.credential_names.is_empty() || self.credential_binding_id.is_some())?;
        invalid(
            !self.security_preset.is_interactive()
                || !self.credential_names.iter().any(|name| name == "TERM"),
        )?;
        let mut seen = std::collections::BTreeSet::new();
        for name in &self.credential_names {
            invalid(
                !name.is_empty()
                    && name.len() <= 128
                    && seen.insert(name)
                    && name.bytes().enumerate().all(|(i, b)| {
                        b.is_ascii_uppercase() || b == b'_' || (i > 0 && b.is_ascii_digit())
                    }),
            )?;
        }
        Ok(())
    }
}
