//! Descriptive session V2 metadata, separate from every V1 CREATE contract.
//! Validation grants no authority, Pod ownership, bootstrap identity or start.
//! The private invocation and full workspace remain separate artifacts; a
//! future trusted handoff must verify both exact commitments before any effect.
use super::{KubernetesAuthorityV1, valid_digest, valid_immutable_image, valid_uid};
use crate::{
    provider::{
        AuthMode, ClientKind, ClientTransport, ProviderChoiceProvenanceV1, ProviderResolutionV1,
    },
    workspace::{Limits, Manifest, validate_relative_path},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt, io::Write, path::Path};
use uuid::Uuid;

const PROFILE: &str = "kubernetes_session_v2";
const MAX_PROFILE_BYTES: usize = 64 * 1024;
const MAX_DEPTH: usize = 64;
const MAX_INVOCATION_BYTES: usize = 32 * 1024;

pub type SessionProfileResult<T> = Result<T, KubernetesSessionProfileError>;

/// Closed diagnostics never include input, paths, argv or provider errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KubernetesSessionProfileError {
    InvalidProfile,
    InputTooLarge,
    BindingMismatch,
    InvocationMismatch,
    WorkspaceMismatch,
}
impl fmt::Display for KubernetesSessionProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidProfile => "invalid Kubernetes session profile",
            Self::InputTooLarge => "Kubernetes session metadata exceeds its input bound",
            Self::BindingMismatch => "Kubernetes session binding differs from retained metadata",
            Self::InvocationMismatch => "Kubernetes session invocation differs from its commitment",
            Self::WorkspaceMismatch => "Kubernetes session workspace differs from its commitment",
        })
    }
}
impl std::error::Error for KubernetesSessionProfileError {}

fn require(valid: bool) -> SessionProfileResult<()> {
    if valid {
        Ok(())
    } else {
        Err(KubernetesSessionProfileError::InvalidProfile)
    }
}

/// Inputs selected by trusted control, never authenticated by this data type.
pub struct KubernetesSessionIdsV2 {
    pub installation_id: Uuid,
    pub control_namespace_uuid: Uuid,
    pub broker_id: Uuid,
    pub incarnation_id: Uuid,
    pub session_id: Uuid,
    pub attempt_id: Uuid,
}

#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct KubernetesSessionBindingV2(BindingWire);
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct BindingWire {
    installation_id: String,
    control_namespace_uuid: String,
    broker_id: String,
    incarnation_id: String,
    session_id: String,
    attempt_id: String,
    authority_fingerprint: String,
}
impl KubernetesSessionBindingV2 {
    pub fn new(
        authority: &KubernetesAuthorityV1,
        ids: KubernetesSessionIdsV2,
    ) -> SessionProfileResult<Self> {
        authority
            .validate()
            .map_err(|_| KubernetesSessionProfileError::InvalidProfile)?;
        let binding = Self(BindingWire {
            installation_id: ids.installation_id.to_string(),
            control_namespace_uuid: ids.control_namespace_uuid.to_string(),
            broker_id: ids.broker_id.to_string(),
            incarnation_id: ids.incarnation_id.to_string(),
            session_id: ids.session_id.to_string(),
            attempt_id: ids.attempt_id.to_string(),
            authority_fingerprint: authority.fingerprint(),
        });
        binding.0.validate()?;
        Ok(binding)
    }
}
impl BindingWire {
    fn validate(&self) -> SessionProfileResult<()> {
        require(
            [
                &self.installation_id,
                &self.control_namespace_uuid,
                &self.broker_id,
                &self.incarnation_id,
                &self.session_id,
                &self.attempt_id,
            ]
            .into_iter()
            .all(|id| valid_uid(id))
                && valid_digest(&self.authority_fingerprint),
        )
    }
}
impl fmt::Debug for KubernetesSessionBindingV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KubernetesSessionBindingV2(<metadata>)")
    }
}

/// Requested resources only; no enforcement or persistent-volume proof.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionResourcesV2 {
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub temporary_mib: u32,
    pub home_mib: u32,
    pub deadline_seconds: u32,
}
impl KubernetesSessionResourcesV2 {
    fn validate(&self) -> SessionProfileResult<()> {
        let memory_filesystems = self
            .temporary_mib
            .checked_add(self.home_mib)
            .ok_or(KubernetesSessionProfileError::InvalidProfile)?;
        require(
            (1..=200_000).contains(&self.cpu_millis)
                && (1..=102_400).contains(&self.memory_mib)
                && self.temporary_mib > 0
                && self.home_mib > 0
                && memory_filesystems < self.memory_mib
                && (1..=86_400).contains(&self.deadline_seconds),
        )
    }
}

/// Borrowed private program/arguments. Neither Serialize nor a raw Debug is
/// implemented; only the structured commitment enters the session profile.
pub struct KubernetesSessionInvocationV2<'a> {
    program: &'a str,
    args: &'a [String],
}
impl<'a> KubernetesSessionInvocationV2<'a> {
    pub fn new(program: &'a str, args: &'a [String]) -> SessionProfileResult<Self> {
        require(program == crate::provider::BOOTSTRAP_PROGRAM && args.len() <= 256)?;
        let mut total = program.len();
        for (index, argument) in args.iter().enumerate() {
            let limit = if index > 0 && args[index - 1] == "--prompt" {
                24 * 1024
            } else {
                4096
            };
            require(argument.len() <= limit && !argument.contains('\0'))?;
            total = total
                .checked_add(argument.len())
                .ok_or(KubernetesSessionProfileError::InputTooLarge)?;
            require(total <= MAX_INVOCATION_BYTES)?;
        }
        Ok(Self { program, args })
    }

    fn commitment(&self, provider: &ProviderResolutionV1) -> SessionProfileResult<String> {
        // The existing parser checks the resolved client/auth/transport/model
        // and its exact preset. This vector is transient private data only.
        let argv: Vec<String> = std::iter::once(self.program.to_owned())
            .chain(self.args.iter().cloned())
            .collect();
        provider
            .validate_argv(&argv)
            .map_err(|_| KubernetesSessionProfileError::InvalidProfile)?;
        // A structured [program, args] array preserves boundaries and order.
        hash_serialized(
            b"ai-guard:kubernetes-session-invocation:v2\0",
            &(self.program, self.args),
        )
    }
}
impl fmt::Debug for KubernetesSessionInvocationV2<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KubernetesSessionInvocationV2(<private>)")
    }
}

// A string projection checks canonical credential UUIDs before the existing
// Uuid parser could normalize them. Every nested input object is closed.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderWire {
    version: u32,
    client: ClientKind,
    auth_mode: AuthMode,
    transport: ClientTransport,
    profile_id: String,
    profile_revision: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    credential_binding_id: String,
    credential_names: Vec<String>,
    invocation_preset_version: u32,
    provenance: ProviderChoiceProvenanceV1,
}
impl ProviderWire {
    fn new(provider: &ProviderResolutionV1) -> SessionProfileResult<Self> {
        provider
            .validate()
            .map_err(|_| KubernetesSessionProfileError::InvalidProfile)?;
        Ok(Self {
            version: provider.version,
            client: provider.client,
            auth_mode: provider.auth_mode,
            transport: provider.transport,
            profile_id: provider.profile_id.clone(),
            profile_revision: provider.profile_revision,
            model: provider.model.clone(),
            credential_binding_id: provider.credential_binding_id.to_string(),
            credential_names: provider.credential_names.clone(),
            invocation_preset_version: provider.invocation_preset_version,
            provenance: provider.provenance.clone(),
        })
    }
    fn resolution(&self) -> SessionProfileResult<ProviderResolutionV1> {
        require(valid_uid(&self.credential_binding_id))?;
        let provider = ProviderResolutionV1 {
            version: self.version,
            client: self.client,
            auth_mode: self.auth_mode,
            transport: self.transport,
            profile_id: self.profile_id.clone(),
            profile_revision: self.profile_revision,
            model: self.model.clone(),
            credential_binding_id: Uuid::parse_str(&self.credential_binding_id)
                .map_err(|_| KubernetesSessionProfileError::InvalidProfile)?,
            credential_names: self.credential_names.clone(),
            invocation_preset_version: self.invocation_preset_version,
            provenance: self.provenance.clone(),
        };
        provider
            .validate()
            .map_err(|_| KubernetesSessionProfileError::InvalidProfile)?;
        Ok(provider)
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct WorkspaceCommitment {
    manifest_sha256: String,
    file_count: u64,
    total_bytes: u64,
}
impl WorkspaceCommitment {
    fn validate(&self) -> SessionProfileResult<()> {
        let limits = Limits::default();
        require(
            valid_digest(&self.manifest_sha256)
                && self.file_count <= limits.max_files as u64
                && self.total_bytes <= limits.max_bytes
                && (self.file_count != 0 || self.total_bytes == 0)
                && self.total_bytes
                    <= self
                        .file_count
                        .checked_mul(limits.max_file_bytes)
                        .ok_or(KubernetesSessionProfileError::InvalidProfile)?,
        )
    }

    fn from_manifest(manifest: &Manifest) -> SessionProfileResult<Self> {
        let limits = Limits::default();
        require(manifest.len() <= limits.max_files)?;
        // Sorted fields match the V2 canonical JSON contract. No profile-wire
        // byte cap is applied to the full, separately retained manifest.
        #[derive(Serialize)]
        struct FileCommitment<'a> {
            bytes: u64,
            mode: u32,
            sha256: &'a str,
        }
        let mut entries = BTreeMap::new();
        let mut total_bytes = 0_u64;
        for (name, fingerprint) in manifest {
            let path = validate_relative_path(Path::new(name))
                .map_err(|_| KubernetesSessionProfileError::InvalidProfile)?;
            require(
                !name.is_empty()
                    && path.to_str() == Some(name.as_str())
                    && path.components().count() <= MAX_DEPTH,
            )?;
            for parent in path
                .ancestors()
                .skip(1)
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                require(
                    parent
                        .to_str()
                        .is_some_and(|parent| !manifest.contains_key(parent)),
                )?;
            }
            let mode = fingerprint
                .mode
                .ok_or(KubernetesSessionProfileError::InvalidProfile)?;
            require(
                matches!(mode, 0o644 | 0o755)
                    && valid_digest(&fingerprint.sha256)
                    && fingerprint.bytes <= limits.max_file_bytes,
            )?;
            total_bytes = total_bytes
                .checked_add(fingerprint.bytes)
                .ok_or(KubernetesSessionProfileError::InvalidProfile)?;
            require(total_bytes <= limits.max_bytes)?;
            entries.insert(
                name.as_str(),
                FileCommitment {
                    bytes: fingerprint.bytes,
                    mode,
                    sha256: &fingerprint.sha256,
                },
            );
        }
        Ok(Self {
            manifest_sha256: hash_serialized(
                b"ai-guard:kubernetes-session-workspace:v2\0",
                &entries,
            )?,
            file_count: manifest.len() as u64,
            total_bytes,
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NetworkRequest {
    Denied,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileWire {
    version: u32,
    profile: String,
    binding: BindingWire,
    image: String,
    network: NetworkRequest,
    resources: KubernetesSessionResourcesV2,
    provider: ProviderWire,
    invocation_sha256: String,
    workspace: WorkspaceCommitment,
}
impl ProfileWire {
    fn validate(&self) -> SessionProfileResult<()> {
        require(
            self.version == 2
                && self.profile == PROFILE
                && valid_immutable_image(&self.image)
                && valid_digest(&self.invocation_sha256),
        )?;
        self.binding.validate()?;
        self.resources.validate()?;
        self.provider.resolution()?;
        self.workspace.validate()
    }
}

/// Metadata only. There is deliberately no Deserialize, conversion from V1,
/// execution factory, Pod renderer, effect method or activation capability.
#[derive(Clone, Serialize)]
#[serde(transparent)]
pub struct KubernetesSessionProfileV2(ProfileWire);
impl fmt::Debug for KubernetesSessionProfileV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KubernetesSessionProfileV2(<private metadata>)")
    }
}
impl KubernetesSessionProfileV2 {
    pub fn build(
        authority: &KubernetesAuthorityV1,
        binding: &KubernetesSessionBindingV2,
        image: &str,
        resources: KubernetesSessionResourcesV2,
        provider: &ProviderResolutionV1,
        invocation: KubernetesSessionInvocationV2<'_>,
        manifest: &Manifest,
    ) -> SessionProfileResult<Self> {
        let profile = Self(ProfileWire {
            version: 2,
            profile: PROFILE.into(),
            binding: binding.0.clone(),
            image: image.into(),
            network: NetworkRequest::Denied,
            resources,
            provider: ProviderWire::new(provider)?,
            invocation_sha256: invocation.commitment(provider)?,
            workspace: WorkspaceCommitment::from_manifest(manifest)?,
        });
        profile.check_binding(authority, binding)?;
        profile.canonical_bytes()?;
        Ok(profile)
    }

    /// The only JSON entry point rejects duplicate keys before typed decoding.
    /// A decoded commitment does not establish that its artifacts are present.
    pub fn from_json(
        bytes: &[u8],
        authority: &KubernetesAuthorityV1,
        binding: &KubernetesSessionBindingV2,
    ) -> SessionProfileResult<Self> {
        if bytes.len() > MAX_PROFILE_BYTES {
            return Err(KubernetesSessionProfileError::InputTooLarge);
        }
        let value = crate::json::decode_unique(bytes, MAX_DEPTH)
            .map_err(|_| KubernetesSessionProfileError::InvalidProfile)?;
        let profile = Self(
            serde_json::from_value(value)
                .map_err(|_| KubernetesSessionProfileError::InvalidProfile)?,
        );
        profile.check_binding(authority, binding)?;
        profile.canonical_bytes()?;
        Ok(profile)
    }

    fn check_binding(
        &self,
        authority: &KubernetesAuthorityV1,
        binding: &KubernetesSessionBindingV2,
    ) -> SessionProfileResult<()> {
        authority
            .validate()
            .map_err(|_| KubernetesSessionProfileError::InvalidProfile)?;
        self.0.validate()?;
        if self.0.binding != binding.0
            || self.0.binding.authority_fingerprint != authority.fingerprint()
        {
            return Err(KubernetesSessionProfileError::BindingMismatch);
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> SessionProfileResult<Vec<u8>> {
        self.0.validate()?;
        let value = serde_json::to_value(&self.0)
            .map_err(|_| KubernetesSessionProfileError::InvalidProfile)?;
        let mut output = BoundedBytes(Vec::new());
        canonical(&value, &mut output, 0)
            .map_err(|_| KubernetesSessionProfileError::InputTooLarge)?;
        Ok(output.0)
    }

    pub fn profile_hash(&self) -> SessionProfileResult<String> {
        let mut hash = Sha256::new();
        hash.update(b"ai-guard:kubernetes-session-profile:v2\0");
        hash.update(self.canonical_bytes()?);
        Ok(hex::encode(hash.finalize()))
    }

    pub fn verify_workspace(&self, manifest: &Manifest) -> SessionProfileResult<()> {
        if WorkspaceCommitment::from_manifest(manifest)? != self.0.workspace {
            return Err(KubernetesSessionProfileError::WorkspaceMismatch);
        }
        Ok(())
    }

    pub fn verify_invocation(
        &self,
        invocation: KubernetesSessionInvocationV2<'_>,
    ) -> SessionProfileResult<()> {
        if invocation.commitment(&self.0.provider.resolution()?)? != self.0.invocation_sha256 {
            return Err(KubernetesSessionProfileError::InvocationMismatch);
        }
        Ok(())
    }
}

struct DigestWriter<'a>(&'a mut Sha256);
impl Write for DigestWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn hash_serialized(domain: &[u8], value: &impl Serialize) -> SessionProfileResult<String> {
    let mut hash = Sha256::new();
    hash.update(domain);
    serde_json::to_writer(DigestWriter(&mut hash), value)
        .map_err(|_| KubernetesSessionProfileError::InvalidProfile)?;
    Ok(hex::encode(hash.finalize()))
}

struct BoundedBytes(Vec<u8>);
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_PROFILE_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("session metadata exceeds its bound"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn canonical(value: &Value, output: &mut BoundedBytes, depth: usize) -> std::io::Result<()> {
    if depth > MAX_DEPTH {
        return Err(std::io::Error::other(
            "session JSON exceeds its depth bound",
        ));
    }
    match value {
        Value::Object(fields) => {
            output.write_all(b"{")?;
            let sorted: BTreeMap<_, _> = fields.iter().collect();
            for (index, (key, value)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    output.write_all(b",")?;
                }
                serde_json::to_writer(&mut *output, key)?;
                output.write_all(b":")?;
                canonical(value, output, depth + 1)?;
            }
            output.write_all(b"}")
        }
        Value::Array(values) => {
            output.write_all(b"[")?;
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.write_all(b",")?;
                }
                canonical(value, output, depth + 1)?;
            }
            output.write_all(b"]")
        }
        scalar => serde_json::to_writer(output, scalar).map_err(std::io::Error::other),
    }
}

#[cfg(test)]
#[path = "session_profile_tests.rs"]
mod tests;
