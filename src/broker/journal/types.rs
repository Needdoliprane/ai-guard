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
    CheckpointReceiptV1
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
    pub(super) operation_id: Uuid,
    pub(super) intent: ExecutionIntentV1,
    pub(super) intent_hash: String,
    pub(super) submit_hash: String,
    pub(super) binding: BindingManifestV1,
    pub(super) source: Option<PrivateFile>,
    pub(super) head: RevisionRefV1,
    pub(super) checkpoint: CheckpointPayloadV1,
    pub(super) chain: Vec<RevisionRefV1>,
}
impl AcceptedJobV1 {
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
    pub fn head(&self) -> &RevisionRefV1 {
        &self.0.head
    }
    pub fn checkpoint(&self) -> &CheckpointPayloadV1 {
        &self.0.checkpoint
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
    Interrupted,
}
pub enum BindingInputs<'a> {
    None,
    Manual { source: &'a PrivateFile },
    Provider { source: &'a PrivateFile },
}
