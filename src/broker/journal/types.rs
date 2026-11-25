use super::*;
use serde::{Deserialize, Deserializer, de::Error as _};

macro_rules! opaque {
    ($($name:ty),+ $(,)?) => { $(impl fmt::Debug for $name {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(concat!(stringify!($name), "(<private>)")) }
    })+ };
}
opaque!(
    Journal,
    JournalScopeV1,
    FsExclusions,
    ControlSecrets,
    JournalLimitsV1,
    ReservationTicket,
    AcceptedJobV1,
    BindingManifestV1,
    RevisionRefV1,
    CheckpointReceiptV1,
    RejectionReceiptV1,
    JournalStatusV1,
    JournalEntryV1,
    JournalPageV1
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalError {
    InvalidInput,
    UnsupportedVersion,
    TooLarge,
    LimitExceeded,
    Busy,
    Conflict,
    Interrupted,
    Corrupt,
    UnsafeFilesystem,
    ControlSecret,
    Io,
}
impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "invalid private journal input",
            Self::UnsupportedVersion => "unsupported private journal version",
            Self::TooLarge => "private journal object exceeds its bound",
            Self::LimitExceeded => "private journal capacity exceeded",
            Self::Busy => "private journal already has a writer",
            Self::Conflict => "private journal identity or revision conflict",
            Self::Interrupted => "private journal preparation was interrupted",
            Self::Corrupt => "private journal data is inconsistent",
            Self::UnsafeFilesystem => "private journal filesystem validation failed",
            Self::ControlSecret => "control secret rejected from private job data",
            Self::Io => "private journal persistence failed",
        })
    }
}
impl std::error::Error for JournalError {}
pub type JournalResult<T> = Result<T, JournalError>;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ScopeData {
    pub version: u32,
    pub installation_id: Uuid,
    pub tenant_id: String,
    pub gateway_origin: String,
    pub control_namespace_uuid: Uuid,
    pub profile_id: String,
}
#[derive(Clone, Serialize)]
#[serde(transparent)]
pub struct JournalScopeV1(pub(super) ScopeData);
impl<'de> Deserialize<'de> for JournalScopeV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let data = ScopeData::deserialize(d)
            .map_err(|_| D::Error::custom("invalid private journal scope"))?;
        let scope = Self(data);
        scope
            .validate()
            .map_err(|_| D::Error::custom("invalid private journal scope"))?;
        Ok(scope)
    }
}
impl JournalScopeV1 {
    pub fn installation_id(&self) -> Uuid {
        self.0.installation_id
    }
    pub fn tenant_id(&self) -> &str {
        &self.0.tenant_id
    }
    pub fn gateway_origin(&self) -> &str {
        &self.0.gateway_origin
    }
    pub fn control_namespace_uuid(&self) -> Uuid {
        self.0.control_namespace_uuid
    }
    pub fn profile_id(&self) -> &str {
        &self.0.profile_id
    }
    pub fn new(
        installation_id: Uuid,
        tenant_id: &str,
        gateway_origin: &str,
        control_namespace_uuid: Uuid,
        profile_id: &str,
    ) -> JournalResult<Self> {
        let scope = Self(ScopeData {
            version: 1,
            installation_id,
            tenant_id: tenant_id.into(),
            gateway_origin: gateway_origin.into(),
            control_namespace_uuid,
            profile_id: profile_id.into(),
        });
        scope.validate()?;
        Ok(scope)
    }
    pub(super) fn validate(&self) -> JournalResult<()> {
        let d = &self.0;
        if d.version != 1 {
            return Err(JournalError::UnsupportedVersion);
        }
        crate::tenancy::validate_tenant_id(&d.tenant_id).map_err(|_| JournalError::InvalidInput)?;
        let origin =
            reqwest::Url::parse(&d.gateway_origin).map_err(|_| JournalError::InvalidInput)?;
        let loopback = matches!(
            origin.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
        );
        if d.installation_id.is_nil()
            || d.control_namespace_uuid.is_nil()
            || d.profile_id.is_empty()
            || d.profile_id.len() > 80
            || !d
                .profile_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || d.gateway_origin.len() > 2048
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
            || !(origin.scheme() == "https" || (origin.scheme() == "http" && loopback))
            || origin.host_str().is_none()
        {
            return Err(JournalError::InvalidInput);
        }
        Ok(())
    }
}
#[derive(Default, Clone)]
pub struct FsExclusions(pub(super) Vec<crate::control_fs::PrivateEntryIdentity>);
impl FsExclusions {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn add_known_file(&mut self, parent: &Directory, name: &str) -> JournalResult<()> {
        let identity = fscheck(parent.private_entry(std::ffi::OsStr::new(name)))?
            .ok_or(JournalError::InvalidInput)?;
        if !self.0.contains(&identity) {
            if self.0.len() >= 64 {
                return Err(JournalError::TooLarge);
            }
            self.0.push(identity);
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct ControlSecrets(Vec<String>);
impl ControlSecrets {
    pub fn new(values: Vec<String>) -> JournalResult<Self> {
        if values.len() > 64
            || values.iter().any(|v| v.is_empty() || v.len() > 4096)
            || values.iter().map(String::len).sum::<usize>() > 256 * 1024
        {
            return Err(JournalError::TooLarge);
        }
        Ok(Self(values))
    }
    pub(super) fn bytes(&self, bytes: &[u8]) -> JournalResult<()> {
        if self
            .0
            .iter()
            .any(|s| bytes.windows(s.len()).any(|w| w == s.as_bytes()))
        {
            return Err(JournalError::ControlSecret);
        }
        Ok(())
    }
    pub(super) fn value(&self, value: &Value) -> JournalResult<()> {
        match value {
            Value::String(s) => self.bytes(s.as_bytes()),
            Value::Object(fields) => {
                for (k, v) in fields {
                    self.bytes(k.as_bytes())?;
                    self.value(v)?;
                }
                Ok(())
            }
            Value::Array(items) => {
                for v in items {
                    self.value(v)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    pub(super) fn record(&self, bytes: &[u8]) -> JournalResult<()> {
        self.bytes(bytes)?;
        let value: Value = serde_json::from_slice(bytes).map_err(|_| JournalError::Corrupt)?;
        self.value(&value)
    }
}
#[derive(Clone)]
pub struct JournalLimitsV1 {
    pub(super) job_bytes: u64,
    pub(super) job_entries: u32,
    pub(super) revisions: u32,
    pub(super) scope_bytes: u64,
    pub(super) scope_entries: u32,
    pub(super) jobs: u32,
}
impl Default for JournalLimitsV1 {
    fn default() -> Self {
        Self {
            job_bytes: 256 * 1024 * 1024,
            job_entries: 512,
            revisions: 256,
            scope_bytes: 1024 * 1024 * 1024,
            scope_entries: 65536,
            jobs: 256,
        }
    }
}
impl JournalLimitsV1 {
    pub fn new(
        job_bytes: u64,
        job_entries: u32,
        max_revisions: u32,
        scope_bytes: u64,
        scope_entries: u32,
        max_jobs: u32,
    ) -> JournalResult<Self> {
        if job_bytes == 0
            || job_entries == 0
            || max_revisions == 0
            || scope_bytes == 0
            || scope_entries == 0
            || max_jobs == 0
            || job_bytes > 8 * 1024 * 1024 * 1024
            || job_entries > 131072
            || max_revisions > 65536
            || scope_bytes > 1024 * 1024 * 1024 * 1024
            || scope_entries > 4194304
            || max_jobs > 16384
            || job_bytes > scope_bytes
            || job_entries > scope_entries
            || max_revisions > job_entries
            || max_jobs > scope_entries
        {
            return Err(JournalError::InvalidInput);
        }
        Ok(Self {
            job_bytes,
            job_entries,
            revisions: max_revisions,
            scope_bytes,
            scope_entries,
            jobs: max_jobs,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingKindV1 {
    None,
    Manual,
    Provider,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub(super) enum BindingData {
    None,
    Manual {
        binding_id: Uuid,
        file_name: String,
        names: Vec<String>,
        source_sha256: String,
        stored_sha256: String,
        handoff_sha256: String,
        stored_bytes: u64,
    },
    Provider {
        binding_id: Uuid,
        file_name: String,
        names: Vec<String>,
        source_sha256: String,
        stored_sha256: String,
        handoff_sha256: String,
        stored_bytes: u64,
    },
}
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct BindingManifestV1(pub(super) BindingData);
impl<'de> Deserialize<'de> for BindingManifestV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        BindingData::deserialize(d)
            .map(Self)
            .map_err(|_| D::Error::custom("invalid private binding manifest"))
    }
}
impl BindingManifestV1 {
    pub fn kind(&self) -> BindingKindV1 {
        match self.0 {
            BindingData::None => BindingKindV1::None,
            BindingData::Manual { .. } => BindingKindV1::Manual,
            BindingData::Provider { .. } => BindingKindV1::Provider,
        }
    }
    pub fn binding_id(&self) -> Option<Uuid> {
        match &self.0 {
            BindingData::None => None,
            BindingData::Manual { binding_id, .. } | BindingData::Provider { binding_id, .. } => {
                Some(*binding_id)
            }
        }
    }
    pub fn names(&self) -> &[String] {
        match &self.0 {
            BindingData::None => &[],
            BindingData::Manual { names, .. } | BindingData::Provider { names, .. } => names,
        }
    }
    pub fn file_name(&self) -> Option<&str> {
        match &self.0 {
            BindingData::None => None,
            BindingData::Manual { file_name, .. } | BindingData::Provider { file_name, .. } => {
                Some(file_name)
            }
        }
    }
    pub fn source_sha(&self) -> Option<&str> {
        match &self.0 {
            BindingData::None => None,
            BindingData::Manual { source_sha256, .. }
            | BindingData::Provider { source_sha256, .. } => Some(source_sha256),
        }
    }
    pub fn stored_sha(&self) -> Option<&str> {
        match &self.0 {
            BindingData::None => None,
            BindingData::Manual { stored_sha256, .. }
            | BindingData::Provider { stored_sha256, .. } => Some(stored_sha256),
        }
    }
    pub fn handoff_sha(&self) -> Option<&str> {
        match &self.0 {
            BindingData::None => None,
            BindingData::Manual { handoff_sha256, .. }
            | BindingData::Provider { handoff_sha256, .. } => Some(handoff_sha256),
        }
    }
    pub fn stored_bytes(&self) -> u64 {
        match &self.0 {
            BindingData::None => 0,
            BindingData::Manual { stored_bytes, .. }
            | BindingData::Provider { stored_bytes, .. } => *stored_bytes,
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionRefV1 {
    pub(super) revision: u64,
    pub(super) hash: String,
}
impl<'de> Deserialize<'de> for RevisionRefV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            revision: u64,
            hash: String,
        }
        let wire = Wire::deserialize(d)
            .map_err(|_| D::Error::custom("invalid private revision reference"))?;
        Self::new(wire.revision, &wire.hash)
            .map_err(|_| D::Error::custom("invalid private revision reference"))
    }
}
impl RevisionRefV1 {
    pub fn new(revision: u64, hash: &str) -> JournalResult<Self> {
        valid_hash(hash)?;
        Ok(Self {
            revision,
            hash: hash.into(),
        })
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn hash(&self) -> &str {
        &self.hash
    }
}
pub struct AcceptedJobV1(pub(super) Box<AcceptedData>);
pub(super) struct AcceptedData {
    pub(super) scope: JournalScopeV1,
    pub(super) operation_id: Uuid,
    pub(super) intent: ExecutionIntentV1,
    pub(super) intent_hash: String,
    pub(super) submit_hash: String,
    pub(super) binding: BindingManifestV1,
    pub(super) source: Option<PrivateFile>,
    pub(super) launch_binding: Option<crate::runtime::launch_profile::DockerLaunchProfileBindingV1>,
    pub(super) launch_profile: Option<crate::runtime::launch_profile::RetainedDockerLaunchProfile>,
    pub(super) head: RevisionRefV1,
    pub(super) checkpoint: LoadedCheckpoint,
    pub(super) legacy_managed_seen: bool,
    pub(super) chain: Vec<RevisionRefV1>,
    pub(super) command_receipts: HashMap<Uuid, LocalCommandReceiptV1>,
    pub(super) outbox_receipts: HashMap<(Uuid, Option<i64>), OutboxReceiptV1>,
    pub(super) last_cancel_operations: Option<usize>,
}
#[derive(Clone)]
pub(super) enum LoadedCheckpoint {
    V1(CheckpointPayloadV1),
    V2(super::super::checkpoint::BrokerCheckpointV2),
    V3(BrokerCheckpointV3),
    V4(BrokerCheckpointV4),
    V5(super::super::retirement_checkpoint::BrokerCheckpointV5),
}
impl LoadedCheckpoint {
    pub(super) fn retirement(
        &self,
    ) -> Option<&super::super::retirement_checkpoint::BrokerCheckpointV5> {
        match self {
            Self::V5(value) => Some(value),
            _ => None,
        }
    }
    pub(super) fn runner(&self) -> &CheckpointPayloadV1 {
        match self {
            Self::V1(value) => value,
            Self::V2(value) => value.runner(),
            Self::V3(value) => value.broker().runner(),
            Self::V4(value) => value.lifecycle().broker().runner(),
            Self::V5(value) => value.outbox().lifecycle().broker().runner(),
        }
    }
    pub(super) fn supervised(&self) -> Option<&super::super::checkpoint::BrokerCheckpointV2> {
        match self {
            Self::V1(_) => None,
            Self::V2(value) => Some(value),
            Self::V3(value) => Some(value.broker()),
            Self::V4(value) => Some(value.lifecycle().broker()),
            Self::V5(value) => Some(value.outbox().lifecycle().broker()),
        }
    }
    pub(super) fn lifecycle(&self) -> Option<&BrokerCheckpointV3> {
        match self {
            Self::V3(value) => Some(value),
            Self::V4(value) => Some(value.lifecycle()),
            Self::V5(value) => Some(value.outbox().lifecycle()),
            _ => None,
        }
    }
    pub(super) fn outbox(&self) -> Option<&BrokerCheckpointV4> {
        match self {
            Self::V4(value) => Some(value),
            Self::V5(value) => Some(value.outbox()),
            _ => None,
        }
    }
}
impl AcceptedJobV1 {
    pub fn scope(&self) -> &JournalScopeV1 {
        &self.0.scope
    }
    pub fn operation_id(&self) -> Uuid {
        self.0.operation_id
    }
    pub fn intent(&self) -> &ExecutionIntentV1 {
        &self.0.intent
    }
    pub fn intent_hash(&self) -> &str {
        &self.0.intent_hash
    }
    pub fn submit_hash(&self) -> &str {
        &self.0.submit_hash
    }
    pub fn binding_manifest(&self) -> &BindingManifestV1 {
        &self.0.binding
    }
    pub fn binding_source(&self) -> Option<&PrivateFile> {
        self.0.source.as_ref()
    }
    pub(crate) fn launch_profile(
        &self,
    ) -> Option<&crate::runtime::launch_profile::RetainedDockerLaunchProfile> {
        self.0.launch_profile.as_ref()
    }
    pub(crate) fn launch_profile_binding(
        &self,
    ) -> Option<&crate::runtime::launch_profile::DockerLaunchProfileBindingV1> {
        self.0.launch_binding.as_ref()
    }
    pub fn head(&self) -> &RevisionRefV1 {
        &self.0.head
    }
    pub fn checkpoint(&self) -> &CheckpointPayloadV1 {
        self.0.checkpoint.runner()
    }
    pub fn broker_checkpoint_v2(&self) -> Option<&super::super::checkpoint::BrokerCheckpointV2> {
        self.0.checkpoint.supervised()
    }
    pub fn broker_checkpoint_v3(&self) -> Option<&BrokerCheckpointV3> {
        self.0.checkpoint.lifecycle()
    }
    pub(crate) fn broker_checkpoint_v5(
        &self,
    ) -> Option<&super::super::retirement_checkpoint::BrokerCheckpointV5> {
        self.0.checkpoint.retirement()
    }
    pub fn broker_checkpoint_v4(&self) -> Option<&BrokerCheckpointV4> {
        self.0.checkpoint.outbox()
    }
    pub(crate) fn outbox_receipt(
        &self,
        event_id: Uuid,
        audit_seq: Option<i64>,
    ) -> Option<&OutboxReceiptV1> {
        self.0.outbox_receipts.get(&(event_id, audit_seq))
    }
}
pub struct CheckpointReceiptV1 {
    pub(super) operation_id: Uuid,
    pub(super) head: RevisionRefV1,
}
impl CheckpointReceiptV1 {
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }
    pub fn head(&self) -> &RevisionRefV1 {
        &self.head
    }
}
#[derive(Debug)]
pub enum ReserveResult {
    New(ReservationTicket),
    InProgress,
    Accepted(AcceptedJobV1),
    Rejected(RejectionReceiptV1),
    Interrupted,
}
pub enum BindingInputs<'a> {
    None,
    Manual { source: &'a PrivateFile },
    Provider { source: &'a PrivateFile },
}

macro_rules! rejection_enum {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                #[derive(Deserialize)]
                #[serde(rename_all = "snake_case")]
                enum Wire { $($variant),+ }
                Ok(match Wire::deserialize(decoder)
                    .map_err(|_| D::Error::custom("invalid private rejection code"))? {
                    $(Wire::$variant => Self::$variant),+
                })
            }
        }
    };
}
rejection_enum!(RejectionPhaseV1 {
    SubmissionPreparation
});
rejection_enum!(RejectionCodeV1 {
    PreparationFailed,
    CancelledBeforeAcceptance,
    RetainedEgressRuntimeUnavailable
});

pub struct RejectionReceiptV1 {
    pub(super) operation_id: Uuid,
    pub(super) submit_hash: String,
    pub(super) phase: RejectionPhaseV1,
    pub(super) code: RejectionCodeV1,
    pub(super) egress: Option<EgressLaunchV1>,
    pub(super) record_hash: String,
}
impl RejectionReceiptV1 {
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }
    pub fn submit_hash(&self) -> &str {
        &self.submit_hash
    }
    pub fn phase(&self) -> RejectionPhaseV1 {
        self.phase
    }
    pub fn code(&self) -> RejectionCodeV1 {
        self.code
    }
    pub fn egress(&self) -> Option<&EgressLaunchV1> {
        self.egress.as_ref()
    }
    pub fn record_hash(&self) -> &str {
        &self.record_hash
    }
}
pub enum JournalStatusV1 {
    ReservedInProcess,
    Interrupted,
    Rejected(RejectionReceiptV1),
    Accepted(AcceptedJobV1),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalEntryKindV1 {
    ReservedInProcess,
    Interrupted,
    Rejected,
    Accepted,
}
#[derive(Clone)]
pub struct JournalEntryV1 {
    pub(super) operation_id: Uuid,
    pub(super) kind: JournalEntryKindV1,
    pub(super) head: Option<RevisionRefV1>,
    pub(super) cancel_requested: bool,
    pub(super) approval_submitted: bool,
    pub(super) failure_count: usize,
    pub(super) pending_rpc_count: Option<usize>,
    pub(super) worker_state: Option<crate::control::WorkerStateV1>,
    pub(super) quota_held: Option<bool>,
    pub(super) runtime_status: Option<crate::control::RuntimeStatusV1>,
    pub(super) last_observed_at: Option<String>,
}
impl JournalEntryV1 {
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }
    pub fn kind(&self) -> JournalEntryKindV1 {
        self.kind
    }
    pub fn head(&self) -> Option<&RevisionRefV1> {
        self.head.as_ref()
    }
    pub fn cancel_requested(&self) -> bool {
        self.cancel_requested
    }
    pub fn approval_submitted(&self) -> bool {
        self.approval_submitted
    }
    pub fn failure_count(&self) -> usize {
        self.failure_count
    }
    pub fn pending_rpc_count(&self) -> Option<usize> {
        self.pending_rpc_count
    }
    pub fn worker_state(&self) -> Option<&crate::control::WorkerStateV1> {
        self.worker_state.as_ref()
    }
    pub fn quota_held(&self) -> Option<bool> {
        self.quota_held
    }
    pub fn runtime_status(&self) -> Option<&crate::control::RuntimeStatusV1> {
        self.runtime_status.as_ref()
    }
    pub fn last_observed_at(&self) -> Option<&str> {
        self.last_observed_at.as_deref()
    }
}
pub struct JournalPageV1 {
    pub(super) entries: Vec<JournalEntryV1>,
    pub(super) next_after: Option<Uuid>,
}
impl JournalPageV1 {
    pub fn entries(&self) -> &[JournalEntryV1] {
        &self.entries
    }
    pub fn next_after(&self) -> Option<Uuid> {
        self.next_after
    }
}
