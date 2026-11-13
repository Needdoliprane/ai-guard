use super::*;
use serde::{Deserialize, Deserializer, Serialize};

pub(super) fn explicit_option<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PrivateInputKindV1 {
    TenantRegistry,
    RunnerToken,
    ViewerToken,
    OidcConfiguration,
    OidcTrustBundle,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PrivateInputBindingV1 {
    pub kind: PrivateInputKindV1,
    #[serde(deserialize_with = "explicit_option")]
    pub tenant_id: Option<String>,
    #[serde(deserialize_with = "explicit_option")]
    pub role: Option<TechnicalCredentialRoleV1>,
    pub path: PathBuf,
    pub storage_binding: SourceStorageBindingV1,
    pub raw_content_sha256: String,
    pub raw_len: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TechnicalExpectedIdentityV1 {
    pub tenant_id: String,
    pub role: TechnicalCredentialRoleV1,
    pub credential_id: Uuid,
    pub generation: u64,
    pub input_binding_index: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum LegacyScopeKindV1 {
    ExistingLegacyInPlace,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HumanSourcesPreparationRecoveryV1 {
    pub version: u32,
    pub public_digest_version: u32,
    pub operation_id: Uuid,
    pub installation_id: Uuid,
    pub closure_id: Uuid,
    pub closure_revision: u64,
    pub control_config_revision: u64,
    #[serde(deserialize_with = "explicit_option")]
    pub oidc_mapping_revision: Option<u64>,
    pub preparation_directory: PreparationDirectoryIdentityV1,
    pub tenant_registry_path: PathBuf,
    #[serde(deserialize_with = "explicit_option")]
    pub oidc_configuration_path: Option<PathBuf>,
    pub max_events: u64,
    pub input_bindings: Vec<PrivateInputBindingV1>,
    pub technical_identities: Vec<TechnicalExpectedIdentityV1>,
    pub prior_sources: Vec<ExpectedOwnedSourceV1>,
    pub target_sources: Vec<ExpectedOwnedSourceV1>,
    pub source_profiles: BTreeMap<Uuid, SourceProfileV1>,
    pub backup_paths: BTreeMap<Uuid, PathBuf>,
    pub journal_directory: PathBuf,
    pub legacy_scope_kind: LegacyScopeKindV1,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PreparationMemberV1 {
    pub name: String,
    pub len: u64,
    pub sha256: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PreparationSealV1 {
    pub version: u32,
    pub operation_id: Uuid,
    pub preparation_directory: PreparationDirectoryIdentityV1,
    pub members: Vec<PreparationMemberV1>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum OwnedSourceModeV1 {
    OwnedSources,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HumanSourcesActivationV1 {
    pub version: u32,
    pub public_digest_version: u32,
    pub operation_id: Uuid,
    pub preparation_directory: PreparationDirectoryIdentityV1,
    pub preparation_seal: String,
    pub manifest: PathBuf,
    pub tenant_registry_path: PathBuf,
    pub technical_credentials_envelope: String,
    #[serde(deserialize_with = "explicit_option")]
    pub oidc_configuration_path: Option<PathBuf>,
    pub control_config_revision: u64,
    #[serde(deserialize_with = "explicit_option")]
    pub oidc_mapping_revision: Option<u64>,
    pub max_events: u64,
    pub human_source_mode: OwnedSourceModeV1,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ActivationMemberKindV1 {
    Activation,
    Manifest,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ActivationMemberV1 {
    pub kind: ActivationMemberKindV1,
    pub path: PathBuf,
    pub len: u64,
    pub sha256: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ActivationSealV1 {
    pub version: u32,
    pub operation_id: Uuid,
    pub preparation_seal_sha256: String,
    pub members: Vec<ActivationMemberV1>,
}

impl HumanSourcesPreparationRecoveryV1 {
    pub(super) fn validate(&self, directory: &PrivatePreparationDirectory) -> Result<()> {
        ensure!(
            self.version == 1
                && self.public_digest_version == 1
                && !self.operation_id.is_nil()
                && !self.installation_id.is_nil()
                && !self.closure_id.is_nil()
                && self.closure_revision == 1
                && self.control_config_revision > 0
                && (1..=1_000_000).contains(&self.max_events),
            "human_sources_recovery_invalid"
        );
        ensure!(
            self.preparation_directory == directory.identity()?
                && self.journal_directory == directory.path().join("journal")
                && self.oidc_configuration_path.is_some() == self.oidc_mapping_revision.is_some()
                && self.oidc_mapping_revision.is_none_or(|v| v > 0),
            "human_sources_recovery_invalid"
        );
        validate_absolute_path(&self.tenant_registry_path)?;
        if let Some(path) = &self.oidc_configuration_path {
            validate_absolute_path(path)?;
        }
        ensure!(
            !self.prior_sources.is_empty()
                && self.prior_sources.len() <= 256
                && self.target_sources.len() == self.prior_sources.len()
                && self.source_profiles.len() == self.prior_sources.len()
                && self.backup_paths.len() == self.prior_sources.len()
                && self.input_bindings.len() <= 515
                && self.technical_identities.len() <= 512,
            "human_sources_recovery_bound"
        );
        let mut ids = BTreeSet::new();
        let mut namespaces = BTreeSet::new();
        let mut tenants = BTreeSet::new();
        let mut physical = BTreeSet::new();
        let mut previous = None;
        let mut ledgers = 0;
        for (old, new) in self.prior_sources.iter().zip(&self.target_sources) {
            ensure!(
                previous.is_none_or(|id| id < old.source_id)
                    && !old.source_id.is_nil()
                    && !old.durable_namespace_id.is_nil()
                    && old.installation_id == self.installation_id
                    && ids.insert(old.source_id)
                    && namespaces.insert(old.durable_namespace_id)
                    && physical.insert((
                        old.storage_binding.source_device,
                        old.storage_binding.source_inode
                    )),
                "human_sources_recovery_source_invalid"
            );
            previous = Some(old.source_id);
            let mut target = old.clone();
            target.required_format = match old.kind {
                HumanSourceKindV1::TenantStore => 14,
                HumanSourceKindV1::AuthenticationLedger => 5,
            };
            ensure!(
                &target == new
                    && old.required_format
                        == match old.kind {
                            HumanSourceKindV1::TenantStore => 13,
                            HumanSourceKindV1::AuthenticationLedger => 4,
                        },
                "human_sources_recovery_source_invalid"
            );
            validate_absolute_path(&old.storage_binding.path())?;
            match (
                old.kind,
                &old.tenant_id,
                self.source_profiles.get(&old.source_id),
            ) {
                (
                    HumanSourceKindV1::TenantStore,
                    Some(tenant),
                    Some(SourceProfileV1::TenantStore { max_events }),
                ) => {
                    agentsandbox::tenancy::validate_tenant_id(tenant)?;
                    ensure!(
                        tenants.insert(tenant.as_str()) && *max_events == self.max_events,
                        "human_sources_recovery_source_invalid"
                    );
                }
                (
                    HumanSourceKindV1::AuthenticationLedger,
                    None,
                    Some(SourceProfileV1::AuthenticationLedger { .. }),
                ) => ledgers += 1,
                _ => bail!("human_sources_recovery_source_invalid"),
            }
            let backup = self
                .backup_paths
                .get(&old.source_id)
                .context("human_sources_backup_missing")?;
            validate_absolute_path(backup)?;
            ensure!(
                backup
                    == &directory
                        .path()
                        .join("backups")
                        .join(old.source_id.to_string())
                        .join("source.sqlite"),
                "human_sources_backup_invalid"
            );
        }
        ensure!(
            ledgers == usize::from(self.oidc_configuration_path.is_some())
                && self.technical_identities.len() == 2 * tenants.len()
                && self.input_bindings.len() == 2 * tenants.len() + 1 + 2 * ledgers,
            "human_sources_recovery_inventory_invalid"
        );
        let mut previous = None;
        let mut binding_roles = BTreeSet::new();
        let mut binding_physical = BTreeSet::new();
        for binding in &self.input_bindings {
            let order = (binding.kind, binding.tenant_id.as_deref(), binding.role);
            ensure!(
                previous.is_none_or(|p| p < order)
                    && binding.raw_len > 0
                    && binding.raw_len <= binding.kind.limit() as u64
                    && valid_digest(&binding.raw_content_sha256)
                    && binding.path == binding.storage_binding.path()
                    && binding_physical.insert((
                        binding.storage_binding.source_device,
                        binding.storage_binding.source_inode
                    ))
                    && !physical.contains(&(
                        binding.storage_binding.source_device,
                        binding.storage_binding.source_inode
                    )),
                "human_sources_input_binding_invalid"
            );
            validate_absolute_path(&binding.path)?;
            match (binding.kind, &binding.tenant_id, binding.role) {
                (
                    PrivateInputKindV1::RunnerToken,
                    Some(tenant),
                    Some(TechnicalCredentialRoleV1::Runner),
                )
                | (
                    PrivateInputKindV1::ViewerToken,
                    Some(tenant),
                    Some(TechnicalCredentialRoleV1::Viewer),
                ) => ensure!(
                    tenants.contains(tenant.as_str())
                        && binding_roles.insert((tenant.as_str(), binding.role)),
                    "human_sources_input_binding_invalid"
                ),
                (PrivateInputKindV1::TenantRegistry, None, None) => ensure!(
                    binding.path == self.tenant_registry_path,
                    "human_sources_input_binding_invalid"
                ),
                (PrivateInputKindV1::OidcConfiguration, None, None) => ensure!(
                    Some(&binding.path) == self.oidc_configuration_path.as_ref(),
                    "human_sources_input_binding_invalid"
                ),
                (PrivateInputKindV1::OidcTrustBundle, None, None) => {
                    ensure!(ledgers == 1, "human_sources_input_binding_invalid")
                }
                _ => bail!("human_sources_input_binding_invalid"),
            }
            previous = Some(order);
        }
        let mut stable = BTreeSet::new();
        let mut tuples = BTreeSet::new();
        let mut previous = None;
        for identity in &self.technical_identities {
            let order = (identity.tenant_id.as_str(), identity.role);
            let binding = self
                .input_bindings
                .get(identity.input_binding_index)
                .context("human_sources_credential_binding_invalid")?;
            ensure!(
                previous.is_none_or(|p| p < order)
                    && !identity.credential_id.is_nil()
                    && identity.generation == 1
                    && stable.insert(identity.credential_id)
                    && tuples.insert(order)
                    && binding.tenant_id.as_deref() == Some(identity.tenant_id.as_str())
                    && binding.role == Some(identity.role),
                "human_sources_credential_binding_invalid"
            );
            previous = Some(order);
        }
        operator::validate_preparation_layout(directory, &self.target_sources, &self.backup_paths)?;
        Ok(())
    }
}
impl PrivateInputKindV1 {
    pub(super) fn limit(self) -> usize {
        match self {
            Self::TenantRegistry | Self::OidcConfiguration => 1_048_576,
            Self::RunnerToken | Self::ViewerToken => 1024,
            Self::OidcTrustBundle => 262_144,
        }
    }
}
pub(super) fn valid_digest(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
