//! Stable Review credential identity from one retained private server envelope.
//!
//! This proves the configured control credential only. It grants no human
//! permission and proves neither a live Store marker nor complete activation.
//! Durable binding, human source checks and Read/WriteEpoch validation remain
//! mandatory in their own transactions. No bearer, digest or private pathname
//! is included in the proof's public getters or diagnostics.
use super::{CredentialIndex, CredentialRole, TenantState, valid_credential};
use crate::{
    control_fs::PrivateFile,
    human_authority::{
        HumanAuthorityClosureV1, HumanConfigurationFileKindV1, HumanSourceKindV1,
        OwnedHumanSources, OwnedSourceToken,
    },
};
use serde::{Deserialize, Serialize};
pub mod preparation;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fmt, sync::Arc};
use uuid::Uuid;

const MAX_ENVELOPE_BYTES: usize = 1_048_576;
const MAX_ENVELOPE_ENTRIES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReviewCredentialFailure {
    InvalidConfiguration,
    Unavailable,
    Unauthorized,
}
impl fmt::Display for ReviewCredentialFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidConfiguration => "review_credential_configuration_invalid",
            Self::Unavailable => "review_credential_unavailable",
            Self::Unauthorized => "review_credential_unauthorized",
        })
    }
}
impl std::error::Error for ReviewCredentialFailure {}

// Deliberately not Debug or Serialize: these are private credential bytes.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EnvelopeV1 {
    version: u32,
    installation_id: Uuid,
    closure_id: Uuid,
    closure_revision: u64,
    control_config_public_digest: String,
    entries: Vec<EnvelopeEntryV1>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EnvelopeEntryV1 {
    tenant_id: String,
    role: EnvelopeRoleV1,
    credential_id: Uuid,
    generation: u64,
    token: String,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum EnvelopeRoleV1 {
    Runner,
    Viewer,
}
impl EnvelopeRoleV1 {
    fn role(self) -> CredentialRole {
        match self {
            Self::Runner => CredentialRole::Runner,
            Self::Viewer => CredentialRole::Viewer,
        }
    }
}

struct ReviewCredentialEntry {
    tenant_id: String,
    credential_id: Uuid,
    generation: u64,
    source_id: Uuid,
    namespace: Uuid,
}

/// Installation only, before serving requests. Its constructor never performs
/// a Store lookup, accepts a credential UUID from HTTP, or mutates authority.
pub(in crate::api) struct ReviewCredentialRegistry {
    file: PrivateFile,
    owner: OwnedHumanSources,
    entries: HashMap<[u8; 32], ReviewCredentialEntry>,
}

/// Created only by lookup of a bearer in the retained private index below.
/// No Clone, Deserialize or constructor from caller-supplied metadata.
pub(crate) struct AuthenticatedReviewRunnerCredential {
    registry: Arc<ReviewCredentialRegistry>,
    digest: [u8; 32],
    source: OwnedSourceToken,
}

impl fmt::Debug for ReviewCredentialRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReviewCredentialRegistry(<retained private index>)")
    }
}
impl fmt::Debug for AuthenticatedReviewRunnerCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthenticatedReviewRunnerCredential(<retained identity>)")
    }
}

impl ReviewCredentialRegistry {
    pub(in crate::api) fn from_private_file(
        file: PrivateFile,
        owner: OwnedHumanSources,
        expected: &CredentialIndex,
        tenants: &[TenantState],
    ) -> anyhow::Result<Self> {
        // Startup constructor only; never invoke while holding another epoch or
        // transaction. The retained proof's revalidation takes no new gate.
        let epoch = owner.try_read_epoch()?;
        epoch.check()?;
        let binding = owner
            .closure()
            .configuration
            .files
            .iter()
            .find(|entry| entry.kind == HumanConfigurationFileKindV1::TechnicalCredentials)
            .ok_or(ReviewCredentialFailure::InvalidConfiguration)?;
        if file.path() != binding.storage_binding.path() {
            return Err(ReviewCredentialFailure::InvalidConfiguration.into());
        }
        file.persistent_revalidate()
            .map_err(|_| ReviewCredentialFailure::Unavailable)?;
        let tenant_ids = tenants
            .iter()
            .map(|tenant| tenant.tenant_id.clone())
            .collect::<Vec<_>>();
        let entries = resolve_envelope(file.bytes(), expected, &tenant_ids, owner.closure())?;
        file.persistent_revalidate()
            .map_err(|_| ReviewCredentialFailure::Unavailable)?;
        epoch.check()?;
        drop(epoch);
        Ok(Self {
            file,
            owner,
            entries,
        })
    }

    /// Call from the private auth extractor only, after ordinary authentication
    /// and closure-wide collision checks under an epoch, before body parsing.
    pub(super) fn authenticate(
        self: &Arc<Self>,
        bearer: &str,
    ) -> anyhow::Result<AuthenticatedReviewRunnerCredential> {
        if !valid_credential(bearer) {
            return Err(ReviewCredentialFailure::Unauthorized.into());
        }
        let digest = credential_digest(bearer);
        let entry = self
            .entries
            .get(&digest)
            .ok_or(ReviewCredentialFailure::Unauthorized)?;
        let source = self.owner.source_token(entry.source_id)?;
        let credential = AuthenticatedReviewRunnerCredential {
            registry: Arc::clone(self),
            digest,
            source,
        };
        credential.revalidate()?;
        Ok(credential)
    }
}

impl AuthenticatedReviewRunnerCredential {
    // The index is immutable and construction is exclusively from a successful
    // exact lookup. Removal or mutation would require a new retained index.
    fn entry(&self) -> &ReviewCredentialEntry {
        &self.registry.entries[&self.digest]
    }
    pub(crate) fn tenant_id(&self) -> &str {
        &self.entry().tenant_id
    }
    pub(crate) fn credential_id(&self) -> Uuid {
        self.entry().credential_id
    }
    pub(crate) fn credential_generation(&self) -> u64 {
        self.entry().generation
    }
    pub(crate) fn closure_id(&self) -> Uuid {
        self.source.closure().closure_id
    }
    pub(crate) fn closure_revision(&self) -> u64 {
        self.source.closure().closure_revision
    }
    pub(crate) fn control_namespace_uuid(&self) -> Uuid {
        self.entry().namespace
    }
    pub(crate) fn config_public_digest(&self) -> &str {
        &self.source.closure().configuration.public_config_digest
    }
    /// Retained filesystem/index checks only. In particular this must not
    /// authorize_sync, reopen SQLite, take a Store mutex, or acquire a new epoch
    /// while its consumer already holds a target transaction and gate.
    pub(crate) fn revalidate(&self) -> anyhow::Result<()> {
        self.source.revalidate_authority()?;
        self.registry
            .file
            .persistent_revalidate()
            .map_err(|_| ReviewCredentialFailure::Unavailable)?;
        let entry = self.entry();
        let descriptor = self.source.descriptor();
        if descriptor.kind != HumanSourceKindV1::TenantStore
            || !matches!(descriptor.required_format, 14 | 15)
            || descriptor.tenant_id.as_deref() != Some(entry.tenant_id.as_str())
            || descriptor.source_id != entry.source_id
            || descriptor.durable_namespace_id != entry.namespace
        {
            return Err(ReviewCredentialFailure::Unavailable.into());
        }
        self.source.revalidate_authority()?;
        Ok(())
    }
}

fn credential_digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// Real retained-file/index path for offline sibling-module integration tests.
/// This bridge creates no proof from UUID fields: normal envelope validation,
/// owner/source capabilities, private-file checks and bearer lookup all run.
#[cfg(test)]
pub(crate) fn authenticate_review_runner_fixture(
    file: PrivateFile,
    owner: OwnedHumanSources,
    tenant_stores: Vec<(String, Arc<crate::audit::Store>)>,
    bearer: &str,
) -> anyhow::Result<AuthenticatedReviewRunnerCredential> {
    let envelope: EnvelopeV1 = serde_json::from_slice(file.bytes())?;
    let gateways = tenant_stores
        .into_iter()
        .map(|(tenant_id, store)| {
            let token = |role: CredentialRole| -> anyhow::Result<String> {
                let mut entries = envelope
                    .entries
                    .iter()
                    .filter(|entry| entry.tenant_id == tenant_id && entry.role.role() == role);
                let entry = entries
                    .next()
                    .ok_or(ReviewCredentialFailure::InvalidConfiguration)?;
                if entries.next().is_some() {
                    return Err(ReviewCredentialFailure::InvalidConfiguration.into());
                }
                Ok(entry.token.clone())
            };
            Ok(crate::api::TenantGateway {
                runner_token: token(CredentialRole::Runner)?,
                viewer_token: token(CredentialRole::Viewer)?,
                tenant_id,
                store,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let expected = CredentialIndex::for_tenants(&gateways)?;
    let shared = Arc::new(crate::api::store_io::Gate::new(true));
    let tenants = gateways
        .into_iter()
        .map(|gateway| TenantState::from_gateway(gateway, Arc::clone(&shared)))
        .collect::<Vec<_>>();
    let registry = Arc::new(ReviewCredentialRegistry::from_private_file(
        file, owner, &expected, &tenants,
    )?);
    registry.authenticate(bearer)
}

fn resolve_envelope(
    bytes: &[u8],
    expected: &CredentialIndex,
    tenants: &[String],
    closure: &HumanAuthorityClosureV1,
) -> Result<HashMap<[u8; 32], ReviewCredentialEntry>, ReviewCredentialFailure> {
    let invalid = ReviewCredentialFailure::InvalidConfiguration;
    if bytes.is_empty()
        || bytes.len() > MAX_ENVELOPE_BYTES
        || tenants.is_empty()
        || tenants.len() > MAX_ENVELOPE_ENTRIES / 2
    {
        return Err(invalid);
    }
    let validated = preparation::validate_runtime_configuration(bytes, expected, tenants, closure)
        .map_err(|_| invalid)?;
    let mut tenant_indices = HashMap::new();
    let mut sources = HashMap::new();
    for (i, tenant) in tenants.iter().enumerate() {
        if crate::tenancy::validate_tenant_id(tenant).is_err()
            || tenant_indices.insert(tenant.as_str(), i).is_some()
        {
            return Err(invalid);
        }
    }
    for source in &closure.sources {
        if source.kind == HumanSourceKindV1::TenantStore {
            let tenant = source.tenant_id.as_deref().ok_or(invalid)?;
            if !matches!(source.required_format, 14 | 15)
                || !tenant_indices.contains_key(tenant)
                || sources.insert(tenant, source).is_some()
            {
                return Err(invalid);
            }
        }
    }
    if sources.len() != tenants.len() {
        return Err(invalid);
    }
    let mut runners = HashMap::new();
    for (digest, entry) in validated.runners {
        let source = sources.get(entry.tenant_id.as_str()).ok_or(invalid)?;
        runners.insert(
            digest,
            ReviewCredentialEntry {
                tenant_id: entry.tenant_id,
                credential_id: entry.credential_id,
                generation: entry.generation,
                source_id: source.source_id,
                namespace: source.durable_namespace_id,
            },
        );
    }
    if runners.len() != tenants.len() {
        return Err(invalid);
    }
    Ok(runners)
}

#[cfg(test)]
mod tests;
