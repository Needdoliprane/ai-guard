//! Narrow offline operator port. All values here are configuration or observed
//! metadata; only the existing coordinator may create an owned-source receipt.
use crate::control_fs::{Directory, PrivateFile, absolute_path};
use crate::human_authority::SourceStorageBindingV1;
use crate::{
    audit::{
        AuditCapacityPolicyV1, Store, WorkloadAuditPolicyV1,
        authentication::{
            AuthenticationAudit, AuthenticationAuditPolicyV1, AuthenticationCapacityPlanV1,
        },
    },
    human_authority::{
        HumanAuthorityConfigurationV1, HumanConfigurationFileKindV1, HumanSourceKindV1,
        HumanStorageProfileV1,
    },
    redaction::Redactor,
    relay::RelayRetentionPolicyV1,
};
use anyhow::{Result, ensure};
use serde::{
    Deserialize, Serialize,
    de::{self, DeserializeOwned, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
};
use uuid::Uuid;

mod store15;
pub use store15::{
    Store15UpgradeRefusal, Store15UpgradeRequest, Store15UpgradeResult, upgrade_store15,
};

/// Non-authorizing source expectation, identical to the maintenance schema.
/// Conversion into the private maintenance type is checked by its real guard.
#[derive(Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedOwnedSourceV1 {
    pub source_id: Uuid,
    pub kind: HumanSourceKindV1,
    pub installation_id: Uuid,
    pub durable_namespace_id: Uuid,
    #[serde(deserialize_with = "required_option")]
    pub tenant_id: Option<String>,
    pub storage_binding: SourceStorageBindingV1,
    pub required_format: u32,
}
fn required_option<'de, D: serde::Deserializer<'de>, T: serde::Deserialize<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

#[derive(Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceProfileV1 {
    TenantStore { max_events: u64 },
    AuthenticationLedger { policy: AuthenticationAuditPolicyV1 },
}

/// Effective policies resolved from retained private configuration. No tokens,
/// identities or source proof can be constructed from this options object.
pub enum OperatorSourceOptions {
    TenantStore {
        redactor: Redactor,
        max_events: u64,
        capacity: AuditCapacityPolicyV1,
        workload: WorkloadAuditPolicyV1,
        retention: RelayRetentionPolicyV1,
    },
    AuthenticationLedger {
        redactor: Redactor,
        policy: AuthenticationAuditPolicyV1,
        capacity_plan: AuthenticationCapacityPlanV1,
    },
}

pub struct LegacySourceInput {
    pub path: PathBuf,
    pub tenant_id: Option<String>,
    pub options: OperatorSourceOptions,
}

/// Explicitly mutating SQLite open, only for prepare after the stopped-controls
/// attestation. Never called by check, resume-prepare or activation loading.
pub fn observe_legacy_sources(
    installation_id: Uuid,
    inputs: Vec<LegacySourceInput>,
) -> Result<(Vec<ExpectedOwnedSourceV1>, BTreeMap<Uuid, SourceProfileV1>)> {
    ensure!(
        !installation_id.is_nil() && !inputs.is_empty() && inputs.len() <= MAX_SOURCES,
        "human_sources_legacy_invalid"
    );
    let tenants: std::collections::BTreeSet<_> =
        inputs.iter().filter_map(|s| s.tenant_id.clone()).collect();
    let ledgers = inputs.iter().filter(|s| s.tenant_id.is_none()).count();
    ensure!(
        !tenants.is_empty() && tenants.len() + ledgers == inputs.len() && ledgers <= 1,
        "human_sources_legacy_inventory_invalid"
    );
    let mut observations = Vec::new();
    let mut preflight_physical = std::collections::BTreeSet::new();
    for input in &inputs {
        validate_absolute_path(&input.path)?;
        match (&input.tenant_id, &input.options) {
            (
                Some(tenant),
                OperatorSourceOptions::TenantStore {
                    max_events,
                    capacity,
                    workload,
                    retention,
                    ..
                },
            ) => {
                crate::tenancy::validate_tenant_id(tenant)?;
                ensure!(
                    (1..=1_000_000).contains(max_events),
                    "human_sources_legacy_capacity_invalid"
                );
                capacity.validate(*max_events)?;
                workload.validate(*max_events, capacity)?;
                retention.validate()?;
            }
            (
                None,
                OperatorSourceOptions::AuthenticationLedger {
                    policy,
                    capacity_plan,
                    redactor,
                },
            ) => {
                policy.validate_for_installation(installation_id, redactor)?;
                capacity_plan.validate()?;
                ensure!(
                    capacity_plan
                        .tenants
                        .iter()
                        .map(|t| t.tenant_id.clone())
                        .collect::<std::collections::BTreeSet<_>>()
                        == tenants,
                    "human_sources_authentication_capacity_tenants_differ"
                );
            }
            _ => anyhow::bail!("human_sources_legacy_inventory_invalid"),
        }
        let binding = super::super::startup::observe_human_storage_binding(&input.path)?;
        ensure!(
            preflight_physical.insert((binding.source_device, binding.source_inode)),
            "human_sources_source_alias"
        );
        observations.push(binding);
    }
    let mut sources = Vec::new();
    let mut profiles = BTreeMap::new();
    let mut physical = std::collections::BTreeSet::new();
    let mut namespaces = std::collections::BTreeSet::new();
    for (input, before) in inputs.into_iter().zip(observations) {
        validate_absolute_path(&input.path)?;
        ensure!(
            physical.insert((before.source_device, before.source_inode)),
            "human_sources_source_alias"
        );
        let (kind, namespace, profile) = match input.options {
            OperatorSourceOptions::TenantStore {
                redactor,
                max_events,
                capacity,
                workload,
                retention,
            } => {
                let tenant = input
                    .tenant_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("human_sources_tenant_invalid"))?;
                let store =
                    Store::open_existing_for_tenant(&input.path, tenant, redactor, max_events)?
                        .with_capacity_policy(capacity)?
                        .with_workload_audit_policy(workload)?
                        .with_relay_retention_policy(retention)?;
                let state = store.supervision_state()?;
                ensure!(
                    state.schema_version == 13 && !state.active && store.integrity()?.valid,
                    "human_sources_legacy_format_or_state"
                );
                let namespace = Uuid::parse_str(
                    state
                        .control_namespace_uuid
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("human_sources_namespace_missing"))?,
                )?;
                (
                    HumanSourceKindV1::TenantStore,
                    namespace,
                    SourceProfileV1::TenantStore { max_events },
                )
            }
            OperatorSourceOptions::AuthenticationLedger {
                redactor,
                policy,
                capacity_plan,
            } => {
                ensure!(
                    input.tenant_id.is_none(),
                    "human_sources_authentication_scope_invalid"
                );
                let audit = AuthenticationAudit::open_existing(
                    &input.path,
                    installation_id,
                    policy.clone(),
                    redactor,
                )?;
                ensure!(
                    audit.format_version()? == 4
                        && audit.capacity_plan()?.as_ref() == Some(&capacity_plan),
                    "human_sources_authentication_format_or_capacity"
                );
                audit.validate_capacity_tenants(&tenants)?;
                (
                    HumanSourceKindV1::AuthenticationLedger,
                    audit.verify()?.namespace_id,
                    SourceProfileV1::AuthenticationLedger { policy },
                )
            }
        };
        let after = super::super::startup::observe_human_storage_binding(&input.path)?;
        ensure!(
            before == after && namespaces.insert(namespace),
            "human_sources_legacy_binding_changed"
        );
        let source_id = Uuid::new_v4();
        profiles.insert(source_id, profile);
        sources.push(ExpectedOwnedSourceV1 {
            source_id,
            kind,
            installation_id,
            durable_namespace_id: namespace,
            tenant_id: input.tenant_id,
            storage_binding: after,
            required_format: match kind {
                HumanSourceKindV1::TenantStore => 13,
                HumanSourceKindV1::AuthenticationLedger => 4,
            },
        });
    }
    sources.sort_by_key(|s| s.source_id);
    Ok((sources, profiles))
}

fn maintenance_source(
    source: &ExpectedOwnedSourceV1,
) -> super::super::maintenance::HumanMaintenanceSourceV1 {
    super::super::maintenance::HumanMaintenanceSourceV1 {
        source_id: source.source_id,
        kind: source.kind,
        installation_id: source.installation_id,
        durable_namespace_id: source.durable_namespace_id,
        tenant_id: source.tenant_id.clone(),
        storage_binding: source.storage_binding.clone(),
        required_format: source.required_format,
    }
}

pub struct OperatorResolvedView {
    pub sources: Vec<ExpectedOwnedSourceV1>,
    pub configuration: HumanAuthorityConfigurationV1,
    pub files: Vec<(HumanConfigurationFileKindV1, PrivateFile)>,
}
impl OperatorResolvedView {
    fn resolve(self) -> Result<super::super::maintenance::HumanMaintenanceResolvedViewV1> {
        super::super::maintenance::HumanMaintenanceResolvedViewV1::new(
            self.sources.iter().map(maintenance_source).collect(),
            self.configuration,
            self.files,
        )
        .map_err(Into::into)
    }
}

/// Build only a maintenance expectation; there is deliberately no active prior
/// manifest, prepared head or invented old credential envelope.
pub fn prepare_maintenance_plan(
    operation_id: Uuid,
    installation_id: Uuid,
    prior: &OperatorResolvedView,
    target: &OperatorResolvedView,
) -> Result<Vec<u8>> {
    ensure!(
        !operation_id.is_nil() && !installation_id.is_nil(),
        "human_sources_plan_invalid"
    );
    let view = |v: &OperatorResolvedView| super::super::maintenance::HumanMaintenanceViewV1 {
        sources: v.sources.iter().map(maintenance_source).collect(),
        configuration: v.configuration.clone(),
    };
    let plan = super::super::maintenance::HumanSourcesMaintenancePlanV1 {
        version: 1,
        migration_id: operation_id,
        installation_id,
        prior: Some(view(prior)),
        target: view(target),
        prior_active_manifest: None,
        supported_storage_profile: HumanStorageProfileV1::LocalPosixFlockV1,
    };
    let bytes = serde_json::to_vec(&plan)?;
    ensure!(bytes.len() <= 512 * 1024, "human_sources_plan_bound");
    // Check the actual retained files and closed source union before emitting.
    let clone_view = |v: &OperatorResolvedView| OperatorResolvedView {
        sources: v.sources.clone(),
        configuration: v.configuration.clone(),
        files: v.files.clone(),
    };
    let _ = super::super::maintenance::HumanSourcesMaintenanceConfigurationV1::new(
        Some(clone_view(prior).resolve()?),
        clone_view(target).resolve()?,
        None,
    );
    Ok(bytes)
}

pub struct OperatorMigrationRequest {
    pub plan_file: PrivateFile,
    pub prior: OperatorResolvedView,
    pub target: OperatorResolvedView,
    pub sources: BTreeMap<Uuid, OperatorSourceOptions>,
    pub backup_paths: BTreeMap<Uuid, PathBuf>,
    pub journal_directory: Directory,
    pub closure_id: Uuid,
    pub closure_revision: u64,
}

/// The only migration port. The existing coordinator retains the real union,
/// writers, journal, backups, floors and incarnation CAS. On success explicitly
/// close admission, drain and drop every temporary owner/source handle.
pub async fn apply_and_close(request: OperatorMigrationRequest) -> Result<PrivateFile> {
    let activation_files = request.target.files.clone();
    let maintenance_configuration =
        super::super::maintenance::HumanSourcesMaintenanceConfigurationV1::new(
            Some(request.prior.resolve()?),
            request.target.resolve()?,
            None,
        );
    let sources = request
        .sources
        .into_iter()
        .map(|(id, options)| {
            (
                id,
                match options {
                    OperatorSourceOptions::TenantStore {
                        redactor,
                        max_events,
                        capacity,
                        workload,
                        retention,
                    } => super::HumanMigrationSourceOptions::TenantStore {
                        redactor,
                        max_events,
                        capacity,
                        workload,
                        retention,
                    },
                    OperatorSourceOptions::AuthenticationLedger {
                        redactor,
                        policy,
                        capacity_plan,
                    } => super::HumanMigrationSourceOptions::AuthenticationLedger {
                        redactor,
                        policy,
                        capacity_plan,
                    },
                },
            )
        })
        .collect();
    let migrated = super::migrate_human_sources(super::HumanSourcesMigrationRequest {
        plan_file: request.plan_file,
        maintenance_configuration,
        activation_files,
        sources,
        backup_paths: request.backup_paths,
        journal_directory: request.journal_directory,
        closure_id: request.closure_id,
        closure_revision: request.closure_revision,
    })?;
    close_migrated(migrated).await
}

async fn close_migrated(migrated: super::MigratedHumanSources) -> Result<PrivateFile> {
    let manifest = migrated.manifest.clone();
    migrated.owner.begin_close();
    migrated.owner.drain().await?;
    drop(migrated);
    manifest.persistent_revalidate()?;
    Ok(manifest)
}

/// Metadata only. Missing intent cannot start a new migration through resume;
/// malformed or later frames are still rejected by the real coordinator.
pub fn migration_intent_present(directory: &Directory, operation_id: Uuid) -> Result<bool> {
    directory.revalidate()?;
    Ok(directory
        .private_entry(std::ffi::OsStr::new(&format!(
            "migration-{operation_id}-0000.json"
        )))?
        .is_some())
}
pub fn directory_has_entries(directory: &Directory) -> Result<bool> {
    Ok(!directory.persistent_entries(260)?.is_empty())
}
pub fn resync_private_file(file: &PrivateFile) -> Result<()> {
    file.persistent_revalidate()?;
    file.persistent_sync()?;
    Directory::open_existing(
        file.path()
            .parent()
            .ok_or_else(|| anyhow::anyhow!("human_sources_parent_missing"))?,
    )?
    .sync()?;
    file.persistent_revalidate()
}
pub fn require_new_preparation_path(path: &Path) -> Result<()> {
    validate_absolute_path(path)?;
    let parent = Directory::open_existing(
        path.parent()
            .ok_or_else(|| anyhow::anyhow!("human_sources_parent_missing"))?,
    )?;
    parent.require_absent_child(
        path.file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("human_sources_directory_invalid"))?,
    )
}
pub fn create_backup_parent(
    directory: &PrivatePreparationDirectory,
    source_id: Uuid,
) -> Result<PathBuf> {
    ensure!(!source_id.is_nil(), "human_sources_source_invalid");
    let backups = directory
        .child("backups", false)?
        .ok_or_else(|| anyhow::anyhow!("human_sources_backups_missing"))?;
    let parent = backups.exclusive_child(&source_id.to_string())?;
    Ok(parent.path().join("source.sqlite"))
}

/// Layout metadata only. No SQLite main, WAL, SHM, backup or journal frame is
/// opened. Their contents remain the coordinator's responsibility at apply.
pub fn validate_preparation_layout(
    directory: &PrivatePreparationDirectory,
    sources: &[ExpectedOwnedSourceV1],
    backup_paths: &BTreeMap<Uuid, PathBuf>,
) -> Result<()> {
    let source_parents: std::collections::BTreeSet<_> = sources
        .iter()
        .map(|s| {
            (
                s.storage_binding.parent_device,
                s.storage_binding.parent_inode,
            )
        })
        .collect();
    let journal = directory
        .child("journal", false)?
        .ok_or_else(|| anyhow::anyhow!("human_sources_journal_missing"))?;
    let backups = directory
        .child("backups", false)?
        .ok_or_else(|| anyhow::anyhow!("human_sources_backups_missing"))?;
    ensure!(
        !source_parents.contains(&journal.physical_identity()?)
            && journal.physical_identity()? != backups.physical_identity()?,
        "human_sources_layout_alias"
    );
    let mut parents = std::collections::BTreeSet::new();
    for source in sources {
        source.storage_binding.validate()?;
        let path = backup_paths
            .get(&source.source_id)
            .ok_or_else(|| anyhow::anyhow!("human_sources_backup_missing"))?;
        ensure!(
            path == &backups
                .path()
                .join(source.source_id.to_string())
                .join("source.sqlite"),
            "human_sources_backup_path_invalid"
        );
        let parent = Directory::open_existing(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("human_sources_backup_parent_missing"))?,
        )?;
        let identity = parent.physical_identity()?;
        ensure!(
            !source_parents.contains(&identity)
                && identity != journal.physical_identity()?
                && parents.insert(identity),
            "human_sources_backup_parent_alias"
        );
    }
    Ok(())
}

pub const MAX_RETAINED_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_INPUT_REFERENCES: usize = 515;
pub const MAX_SOURCES: usize = 256;

/// Shared file-token rule. HTTP's independent bounded bearer parser retains its
/// wider limit; preparing a credential never relaxes the operator file rule.
pub fn parse_private_token_file(bytes: &[u8]) -> Result<String> {
    ensure!(bytes.len() <= 1024, "human_sources_token_file_bound");
    let token = std::str::from_utf8(bytes)?.trim();
    ensure!(
        (32..=512).contains(&token.len()) && !token.chars().any(char::is_whitespace),
        "human_sources_token_file_invalid"
    );
    Ok(token.to_owned())
}

/// Decode after a caller's bounded read. The visitor refuses the next item
/// before decoding/allocating it and rejects duplicate object keys before
/// insertion. serde_ignored also closes nested legacy policy DTOs.
pub fn decode_closed<T: DeserializeOwned>(
    bytes: &[u8],
    max_bytes: usize,
    max_sequence: usize,
) -> Result<T> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= max_bytes,
        "human_sources_document_bound"
    );
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = JsonSeed {
        depth: 0,
        max_sequence,
        max_map: MAX_INPUT_REFERENCES,
        uuid_keys: false,
    }
    .deserialize(&mut decoder)?;
    decoder.end()?;
    let mut ignored = false;
    let parsed = serde_ignored::deserialize(value, |_| ignored = true)?;
    ensure!(!ignored, "human_sources_unknown_field");
    Ok(parsed)
}

struct JsonSeed {
    depth: usize,
    max_sequence: usize,
    max_map: usize,
    uuid_keys: bool,
}
struct RejectExtraElement;
impl<'de> DeserializeSeed<'de> for RejectExtraElement {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, _: D) -> std::result::Result<(), D::Error> {
        Err(de::Error::custom("human_sources_sequence_bound"))
    }
}
impl JsonSeed {
    fn child(&self) -> Self {
        Self {
            depth: self.depth + 1,
            max_sequence: self.max_sequence,
            max_map: self.max_map,
            uuid_keys: false,
        }
    }
}
impl<'de> DeserializeSeed<'de> for JsonSeed {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> std::result::Result<Value, D::Error> {
        if self.depth > 32 {
            return Err(de::Error::custom("human_sources_json_depth"));
        }
        decoder.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for JsonSeed {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded private JSON")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Value, E> {
        Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| de::Error::custom("human_sources_number"))
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Value, E> {
        if v.len() > 4096 {
            return Err(de::Error::custom("human_sources_scalar_bound"));
        }
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Value, E> {
        if v.len() > 4096 {
            return Err(de::Error::custom("human_sources_scalar_bound"));
        }
        Ok(Value::String(v))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let mut values = Vec::new();
        loop {
            if values.len() == self.max_sequence {
                let _ = seq.next_element_seed(RejectExtraElement)?;
                break;
            }
            let Some(value) = seq.next_element_seed(self.child())? else {
                break;
            };
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut fields: A) -> std::result::Result<Value, A::Error> {
        let mut values = Map::new();
        let mut uuid_keys = std::collections::BTreeSet::new();
        while let Some(key) = fields.next_key::<String>()? {
            if values.len() >= self.max_map || key.len() > 4096 || values.contains_key(&key) {
                return Err(de::Error::custom("human_sources_duplicate_or_map_bound"));
            }
            if self.uuid_keys {
                let id = Uuid::parse_str(&key)
                    .map_err(|_| de::Error::custom("human_sources_uuid_key_invalid"))?;
                if id.is_nil() || !uuid_keys.insert(id) {
                    return Err(de::Error::custom("human_sources_duplicate_uuid_key"));
                }
            }
            let mut child = self.child();
            child.max_sequence = match key.as_str() {
                "prior_sources" | "target_sources" | "sources" | "tenants" => {
                    child.max_sequence.min(256)
                }
                "entries" | "technical_identities" => child.max_sequence.min(512),
                "input_bindings" => child.max_sequence.min(515),
                _ => child.max_sequence,
            };
            if matches!(key.as_str(), "source_profiles" | "backup_paths") {
                child.max_map = 256;
                child.uuid_keys = true;
            }
            values.insert(key, fields.next_value_seed(child)?);
        }
        Ok(Value::Object(values))
    }
}

pub struct InputBudget {
    remaining: usize,
    files: BTreeMap<PathBuf, PrivateFile>,
    identities: BTreeMap<(u64, u64), PathBuf>,
}
impl Default for InputBudget {
    fn default() -> Self {
        Self::new()
    }
}
impl InputBudget {
    pub fn new() -> Self {
        Self {
            remaining: MAX_RETAINED_BYTES,
            files: BTreeMap::new(),
            identities: BTreeMap::new(),
        }
    }
    pub fn remaining(&self) -> usize {
        self.remaining
    }
    pub fn reserve(&mut self, length: usize) -> Result<()> {
        ensure!(length <= self.remaining, "human_sources_aggregate_bound");
        self.remaining -= length;
        Ok(())
    }
    pub fn read(&mut self, path: &Path, maximum: usize) -> Result<PrivateFile> {
        validate_absolute_path(path)?;
        if let Some(file) = self.files.get(path) {
            ensure!(file.bytes().len() <= maximum, "human_sources_file_bound");
            file.persistent_revalidate()?;
            return Ok(file.clone());
        }
        ensure!(
            self.files.len() < MAX_INPUT_REFERENCES + 8,
            "human_sources_reference_bound"
        );
        // Reserve the complete admitted bound before the first byte is read.
        self.reserve(maximum)?;
        let file = PrivateFile::open(path, maximum as u64)?;
        let binding = observe_private_file_binding(&file)?;
        let identity = (binding.source_device, binding.source_inode);
        ensure!(
            !self.identities.contains_key(&identity),
            "human_sources_input_alias"
        );
        self.remaining += maximum - file.bytes().len();
        self.identities.insert(identity, path.to_owned());
        self.files.insert(path.to_owned(), file.clone());
        Ok(file)
    }
    pub fn retain_published(&mut self, file: PrivateFile) -> Result<()> {
        if let Some(existing) = self.files.get(file.path()) {
            ensure!(
                existing.bytes() == file.bytes()
                    && observe_private_file_binding(existing)?
                        == observe_private_file_binding(&file)?,
                "human_sources_retained_file_conflict"
            );
            return Ok(());
        }
        ensure!(
            self.files.len() < MAX_INPUT_REFERENCES + 8,
            "human_sources_reference_bound"
        );
        self.reserve(file.bytes().len())?;
        let binding = observe_private_file_binding(&file)?;
        let identity = (binding.source_device, binding.source_inode);
        ensure!(
            !self.identities.contains_key(&identity),
            "human_sources_input_alias"
        );
        self.identities.insert(identity, file.path().to_owned());
        self.files.insert(file.path().to_owned(), file);
        Ok(())
    }
    pub fn revalidate(&self) -> Result<()> {
        for file in self.files.values() {
            file.persistent_revalidate()?;
        }
        Ok(())
    }
    /// Explicit writer retry only: resynchronize every retained configured
    /// input and publication through the same PrivateFile descriptors. This
    /// neither reopens a file by pathname nor starts a source/SQLite owner.
    pub fn resync_all(&self) -> Result<()> {
        self.revalidate()?;
        for file in self.files.values() {
            resync_private_file(file)?;
        }
        self.revalidate()
    }
}

pub fn validate_absolute_path(path: &Path) -> Result<()> {
    let value = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("human_sources_path_invalid"))?;
    ensure!(
        !value.is_empty()
            && value.len() <= 4096
            && path.is_absolute()
            && absolute_path(path)? == path,
        "human_sources_path_invalid"
    );
    Ok(())
}

#[derive(Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationDirectoryIdentityV1 {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
}

pub struct PrivatePreparationDirectory {
    directory: Directory,
}
impl PrivatePreparationDirectory {
    pub fn create(path: &Path) -> Result<Self> {
        validate_absolute_path(path)?;
        let parent = Directory::open_existing(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("human_sources_directory_invalid"))?,
        )?;
        let leaf = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("human_sources_directory_invalid"))?;
        let directory = parent.exclusive_child(leaf)?;
        directory.sync()?;
        parent.sync()?;
        Ok(Self { directory })
    }
    pub fn open(path: &Path) -> Result<Self> {
        validate_absolute_path(path)?;
        Ok(Self {
            directory: Directory::open_existing(path)?,
        })
    }
    pub fn path(&self) -> &Path {
        self.directory.path()
    }
    pub fn contains(&self, name: &str) -> Result<bool> {
        Ok(self
            .directory
            .private_entry(std::ffi::OsStr::new(name))?
            .is_some())
    }
    pub fn identity(&self) -> Result<PreparationDirectoryIdentityV1> {
        let (device, inode) = self.directory.physical_identity()?;
        Ok(PreparationDirectoryIdentityV1 {
            path: self.path().to_owned(),
            device,
            inode,
        })
    }
    pub fn child(&self, name: &str, create: bool) -> Result<Option<Directory>> {
        self.directory.persistent_child(name, create)
    }
    pub fn read(&self, name: &str, maximum: usize) -> Result<Option<PrivateFile>> {
        self.directory.persistent_read(name, maximum as u64, &[])
    }
    pub fn validate_catalogue(&self) -> Result<()> {
        for entry in self.directory.persistent_entries(64)? {
            ensure!(
                matches!(
                    entry.name.as_str(),
                    "recovery.json"
                        | "technical-credentials.json"
                        | "maintenance-plan.json"
                        | "preparation-complete.json"
                        | "activation.json"
                        | "activation-complete.json"
                        | "journal"
                        | "backups"
                ),
                "human_sources_unexpected_child"
            );
            ensure!(
                entry.directory == matches!(entry.name.as_str(), "journal" | "backups"),
                "human_sources_child_type_conflict"
            );
        }
        Ok(())
    }
    pub fn publish_or_verify_identical(
        &self,
        name: &str,
        expected: &[u8],
        maximum: usize,
    ) -> Result<PrivateFile> {
        let product_bound = match name {
            "recovery.json" | "technical-credentials.json" => 1_048_576,
            "maintenance-plan.json" => 524_288,
            "preparation-complete.json" | "activation.json" | "activation-complete.json" => 32_768,
            _ => anyhow::bail!("human_sources_publication_name_invalid"),
        };
        ensure!(
            maximum <= product_bound,
            "human_sources_publication_bound_invalid"
        );
        ensure!(expected.len() <= maximum, "human_sources_file_bound");
        let file = match self.read(name, maximum)? {
            Some(file) => {
                ensure!(
                    file.bytes() == expected,
                    "human_sources_publication_conflict"
                );
                file
            }
            None => self.directory.persistent_create(name, expected)?,
        };
        file.persistent_revalidate()?;
        file.persistent_sync()?;
        self.directory.sync()?;
        file.persistent_revalidate()?;
        Ok(file)
    }
}

pub fn observe_private_file_binding(file: &PrivateFile) -> Result<SourceStorageBindingV1> {
    file.persistent_revalidate()?;
    super::super::startup::observe_human_storage_binding(file.path()).map_err(Into::into)
}

#[cfg(test)]
mod tests;
