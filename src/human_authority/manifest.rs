//! Private, bounded expected bindings. These DTOs prove no SQLite marker.

use super::{HumanAuthorityFailure, OWNER_LOCK_NAME};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::PathBuf};
use uuid::Uuid;

pub const MAX_HUMAN_SOURCES: usize = 256;
pub const MAX_HUMAN_MANIFEST_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanSourceKindV1 {
    TenantStore,
    AuthenticationLedger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanStorageProfileV1 {
    LocalPosixFlockV1,
}

/// Private expected physical binding. The filename is one closed component.
/// Native identities are provenance, never authorization or SQLite receipts.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceStorageBindingV1 {
    pub parent_path: PathBuf,
    pub file_name: String,
    pub parent_device: u64,
    pub parent_inode: u64,
    pub source_device: u64,
    pub source_inode: u64,
}

impl SourceStorageBindingV1 {
    pub fn path(&self) -> PathBuf {
        self.parent_path.join(&self.file_name)
    }

    pub(crate) fn parent_identity(&self) -> (u64, u64) {
        (self.parent_device, self.parent_inode)
    }

    pub(crate) fn source_identity(&self) -> (u64, u64) {
        (self.source_device, self.source_inode)
    }

    fn validate(&self) -> Result<(), HumanAuthorityFailure> {
        let normalized = crate::control_fs::absolute_path(&self.parent_path)
            .map_err(|_| HumanAuthorityFailure::InvalidManifest)?;
        if normalized != self.parent_path
            || self.parent_path.as_os_str().len() > 4096
            || !valid_leaf(&self.file_name)
            || self.file_name == OWNER_LOCK_NAME
            || self.file_name == ".ai-guard-sqlite-open.lock"
            || self.parent_inode == 0
            || self.source_inode == 0
        {
            return Err(HumanAuthorityFailure::InvalidManifest);
        }
        Ok(())
    }
}

/// An expected receipt must still be verified by the modern source replay.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanAuthorityMarkerReceiptV1 {
    pub seq: i64,
    pub hash: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanSourceDescriptorV1 {
    pub source_id: Uuid,
    pub kind: HumanSourceKindV1,
    pub installation_id: Uuid,
    pub durable_namespace_id: Uuid,
    pub tenant_id: Option<String>,
    pub storage_binding: SourceStorageBindingV1,
    pub required_format: u32,
    pub authority_marker_receipt: HumanAuthorityMarkerReceiptV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanConfigurationFileKindV1 {
    ControlConfiguration,
    TechnicalCredentials,
    OidcConfiguration,
    OidcTrustBundle,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanConfigurationFileV1 {
    pub kind: HumanConfigurationFileKindV1,
    pub storage_binding: SourceStorageBindingV1,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanOidcConfigurationV1 {
    pub public_config_digest: String,
    pub mapping_revision: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanAuthorityConfigurationV1 {
    pub control_config_revision: u64,
    /// Digest of the public, resolved configuration only. Never secret bytes.
    pub public_config_digest: String,
    pub oidc: Option<HumanOidcConfigurationV1>,
    pub files: Vec<HumanConfigurationFileV1>,
}

impl HumanAuthorityConfigurationV1 {
    pub(crate) fn validate(&self) -> Result<(), HumanAuthorityFailure> {
        if self.control_config_revision == 0
            || !valid_digest(&self.public_config_digest)
            || self.files.len() < 2
            || self.files.len() > 4
        {
            return Err(HumanAuthorityFailure::InvalidManifest);
        }
        let mut kinds = BTreeSet::new();
        let mut identities = BTreeSet::new();
        let mut previous = None;
        for file in &self.files {
            file.storage_binding.validate()?;
            if !kinds.insert(file.kind)
                || !identities.insert(file.storage_binding.source_identity())
                || previous.is_some_and(|kind| kind >= file.kind)
            {
                return Err(HumanAuthorityFailure::InvalidManifest);
            }
            previous = Some(file.kind);
        }
        let mut required = BTreeSet::from([
            HumanConfigurationFileKindV1::ControlConfiguration,
            HumanConfigurationFileKindV1::TechnicalCredentials,
        ]);
        if let Some(oidc) = &self.oidc {
            if oidc.mapping_revision == 0 || !valid_digest(&oidc.public_config_digest) {
                return Err(HumanAuthorityFailure::InvalidManifest);
            }
            required.insert(HumanConfigurationFileKindV1::OidcConfiguration);
            required.insert(HumanConfigurationFileKindV1::OidcTrustBundle);
        }
        if kinds != required {
            return Err(HumanAuthorityFailure::InvalidManifest);
        }
        Ok(())
    }
}

/// Exact authority closure. This file is private and never a public event.
/// A successful parse is not a format check, owner or activation receipt.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanAuthorityClosureV1 {
    pub version: u32,
    pub installation_id: Uuid,
    pub closure_id: Uuid,
    pub closure_revision: u64,
    pub closure_digest: String,
    pub sources: Vec<HumanSourceDescriptorV1>,
    pub configuration: HumanAuthorityConfigurationV1,
    pub supported_storage_profile: HumanStorageProfileV1,
}

impl HumanAuthorityClosureV1 {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, HumanAuthorityFailure> {
        if bytes.is_empty() || bytes.len() > MAX_HUMAN_MANIFEST_BYTES {
            return Err(HumanAuthorityFailure::InvalidManifest);
        }
        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|_| HumanAuthorityFailure::InvalidManifest)?;
        manifest.validate_structure()?;
        if !valid_digest(&manifest.closure_digest)
            || manifest.compute_digest()? != manifest.closure_digest
        {
            return Err(HumanAuthorityFailure::InvalidManifest);
        }
        Ok(manifest)
    }

    /// SHA256 of this typed canonical JSON with closure_digest set to empty.
    /// Sources and config kinds must already have their unique canonical order.
    /// Never log this private manifest, its bytes or credential-file digests.
    pub fn compute_digest(&self) -> Result<String, HumanAuthorityFailure> {
        self.validate_structure()?;
        let mut canonical = self.clone();
        canonical.closure_digest.clear();
        let bytes =
            serde_json::to_vec(&canonical).map_err(|_| HumanAuthorityFailure::InvalidManifest)?;
        if bytes.len() > MAX_HUMAN_MANIFEST_BYTES {
            return Err(HumanAuthorityFailure::InvalidManifest);
        }
        Ok(hex::encode(Sha256::digest(bytes)))
    }

    fn validate_structure(&self) -> Result<(), HumanAuthorityFailure> {
        if self.version != 1
            || self.installation_id.is_nil()
            || self.closure_id.is_nil()
            || self.closure_revision == 0
        {
            return Err(HumanAuthorityFailure::InvalidManifest);
        }
        self.configuration.validate()?;
        validate_sources(&self.sources, self.installation_id)?;
        let authentication_ledgers = self
            .sources
            .iter()
            .filter(|source| source.kind == HumanSourceKindV1::AuthenticationLedger)
            .count();
        if authentication_ledgers != usize::from(self.configuration.oidc.is_some()) {
            return Err(HumanAuthorityFailure::InvalidManifest);
        }
        let source_inodes: BTreeSet<_> = self
            .sources
            .iter()
            .map(|source| source.storage_binding.source_identity())
            .collect();
        if self
            .configuration
            .files
            .iter()
            .any(|file| source_inodes.contains(&file.storage_binding.source_identity()))
        {
            return Err(HumanAuthorityFailure::InvalidManifest);
        }
        Ok(())
    }
}

pub(crate) fn validate_sources(
    sources: &[HumanSourceDescriptorV1],
    installation_id: Uuid,
) -> Result<(), HumanAuthorityFailure> {
    if sources.is_empty() || sources.len() > MAX_HUMAN_SOURCES || installation_id.is_nil() {
        return Err(HumanAuthorityFailure::InvalidManifest);
    }
    let mut ids = BTreeSet::new();
    let mut namespaces = BTreeSet::new();
    let mut physical = BTreeSet::new();
    let mut tenants = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut previous = None;
    for source in sources {
        source.storage_binding.validate()?;
        if source.source_id.is_nil()
            || source.storage_binding.parent_device != source.storage_binding.source_device
            || source.durable_namespace_id.is_nil()
            || source.installation_id != installation_id
            || source.required_format == 0
            || source.required_format > i32::MAX as u32
            || source.authority_marker_receipt.seq <= 0
            || !valid_digest(&source.authority_marker_receipt.hash)
            || !ids.insert(source.source_id)
            || !namespaces.insert(source.durable_namespace_id)
            || !physical.insert(source.storage_binding.source_identity())
            || !paths.insert(source.storage_binding.path())
            || previous.is_some_and(|id| id >= source.source_id)
        {
            return Err(HumanAuthorityFailure::InvalidManifest);
        }
        previous = Some(source.source_id);
        match (&source.kind, &source.tenant_id) {
            (HumanSourceKindV1::TenantStore, Some(tenant_id)) => {
                crate::tenancy::validate_tenant_id(tenant_id)
                    .map_err(|_| HumanAuthorityFailure::InvalidManifest)?;
                if !tenants.insert(tenant_id.as_str()) {
                    return Err(HumanAuthorityFailure::InvalidManifest);
                }
            }
            (HumanSourceKindV1::AuthenticationLedger, None) => {}
            _ => return Err(HumanAuthorityFailure::InvalidManifest),
        }
    }
    Ok(())
}

pub(crate) fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_leaf(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}
