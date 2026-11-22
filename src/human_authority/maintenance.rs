//! Offline physical/configuration ownership of the exact prior/target union.
//!
//! The expected sources deliberately contain no future marker receipts. This
//! capability opens no SQLite FD and proves no format, namespace, held writer
//! transaction, migration result, retirement fact, incarnation or readiness.
//! Root must independently resolve the complete old/new server configuration,
//! prepare any new main inode exclusively before its first SQLite opener, and
//! hold actual writer transactions/backup/migration evidence separately.

use super::{
    HumanAuthorityClosureV1, HumanAuthorityConfigurationV1, HumanAuthorityFailure,
    HumanAuthorityMarkerReceiptV1, HumanConfigurationFileKindV1, HumanSourceDescriptorV1,
    HumanSourceKindV1, HumanStorageProfileV1, OwnerFilesystemFailure, RetainedConfigurationFile,
    RetainedParent, SourceStorageBindingV1, acquire_parent_lock, bound_entry, bound_source_entry,
    check_open_deadline, manifest::valid_digest, open_bound_parent,
};
use crate::control_fs::{Directory, PrivateEntryIdentity, PrivateFile};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

/// Proposed offline owned broker identity lifecycle (decision D1). It opens
/// no route, admits no credential at runtime and qualifies nothing.
pub mod broker_identity;

const MAX_SOURCES_PER_VIEW: usize = 256;
const MAX_MAINTENANCE_PLAN_BYTES: usize = 512 * 1024;
const MAX_MAINTENANCE_OPEN_TIME: Duration = Duration::from_secs(10);

/// Expected identity and format, not a SQLite observation or migration receipt.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HumanMaintenanceSourceV1 {
    pub source_id: Uuid,
    pub kind: HumanSourceKindV1,
    pub installation_id: Uuid,
    pub durable_namespace_id: Uuid,
    pub tenant_id: Option<String>,
    pub storage_binding: SourceStorageBindingV1,
    pub required_format: u32,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HumanMaintenanceViewV1 {
    pub sources: Vec<HumanMaintenanceSourceV1>,
    pub configuration: HumanAuthorityConfigurationV1,
}

/// Optional real *prior* active closure. Initial/legacy maintenance has none.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HumanMaintenanceActiveManifestV1 {
    pub storage_binding: SourceStorageBindingV1,
    pub closure_id: Uuid,
    pub closure_revision: u64,
    pub closure_digest: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HumanSourcesMaintenancePlanV1 {
    pub version: u32,
    pub migration_id: Uuid,
    pub installation_id: Uuid,
    /// Actual legacy scope may exist even without an old active closure.
    /// None is valid only when Root's loader resolves no previous scope.
    pub prior: Option<HumanMaintenanceViewV1>,
    pub target: HumanMaintenanceViewV1,
    pub prior_active_manifest: Option<HumanMaintenanceActiveManifestV1>,
    pub supported_storage_profile: HumanStorageProfileV1,
}

impl HumanSourcesMaintenancePlanV1 {
    /// In-place maintenance of an adopted union: prior and target are both
    /// the published active closure, which is also the retained prior active
    /// manifest. No source format, binding or configuration changes.
    pub(crate) fn in_place(
        migration_id: Uuid,
        closure: &HumanAuthorityClosureV1,
        manifest_binding: SourceStorageBindingV1,
    ) -> Self {
        let view = HumanMaintenanceViewV1 {
            sources: closure.sources.iter().map(from_active_source).collect(),
            configuration: closure.configuration.clone(),
        };
        Self {
            version: 1,
            migration_id,
            installation_id: closure.installation_id,
            prior: Some(view.clone()),
            target: view,
            prior_active_manifest: Some(HumanMaintenanceActiveManifestV1 {
                storage_binding: manifest_binding,
                closure_id: closure.closure_id,
                closure_revision: closure.closure_revision,
                closure_digest: closure.closure_digest.clone(),
            }),
            supported_storage_profile: closure.supported_storage_profile,
        }
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, HumanAuthorityFailure> {
        if bytes.is_empty() || bytes.len() > MAX_MAINTENANCE_PLAN_BYTES {
            return Err(HumanAuthorityFailure::InvalidManifest);
        }
        let plan: Self =
            serde_json::from_slice(bytes).map_err(|_| HumanAuthorityFailure::InvalidManifest)?;
        if plan.version != 1 || plan.migration_id.is_nil() || plan.installation_id.is_nil() {
            return Err(HumanAuthorityFailure::InvalidManifest);
        }
        plan.target.validate(plan.installation_id)?;
        if let Some(prior) = &plan.prior {
            prior.validate(plan.installation_id)?;
        }
        if let Some(active) = &plan.prior_active_manifest {
            active.storage_binding.validate()?;
            if plan.prior.is_none()
                || active.closure_id.is_nil()
                || active.closure_revision == 0
                || !valid_digest(&active.closure_digest)
            {
                return Err(HumanAuthorityFailure::InvalidManifest);
            }
        }
        // A moved logical source can have different physical bindings and
        // formats. It cannot silently change tenant, namespace or source kind.
        let mut logical = BTreeMap::<Uuid, &HumanMaintenanceSourceV1>::new();
        let mut physical = BTreeMap::<(u64, u64), &HumanMaintenanceSourceV1>::new();
        for view in plan.prior.iter().chain(std::iter::once(&plan.target)) {
            for source in &view.sources {
                if let Some(previous) = logical.insert(source.source_id, source) {
                    if !same_logical_source(previous, source) {
                        return Err(HumanAuthorityFailure::InvalidManifest);
                    }
                }
                if let Some(previous) =
                    physical.insert(source.storage_binding.source_identity(), source)
                {
                    if !same_logical_source(previous, source)
                        || previous.storage_binding.parent_identity()
                            != source.storage_binding.parent_identity()
                    {
                        return Err(HumanAuthorityFailure::InvalidManifest);
                    }
                }
            }
        }
        Ok(plan)
    }
}

impl HumanMaintenanceViewV1 {
    fn validate(&self, installation: Uuid) -> Result<(), HumanAuthorityFailure> {
        if installation.is_nil()
            || self.sources.is_empty()
            || self.sources.len() > MAX_SOURCES_PER_VIEW
        {
            return Err(HumanAuthorityFailure::InvalidManifest);
        }
        self.configuration.validate()?;
        let mut ids = BTreeSet::new();
        let mut namespaces = BTreeSet::new();
        let mut physical = BTreeSet::new();
        let mut paths = BTreeSet::new();
        let mut tenants = BTreeSet::new();
        let mut previous = None;
        let mut authentication_ledgers = 0;
        for source in &self.sources {
            source.storage_binding.validate()?;
            if source.source_id.is_nil()
                || source.durable_namespace_id.is_nil()
                || source.installation_id != installation
                || source.required_format == 0
                || source.required_format > i32::MAX as u32
                || source.storage_binding.parent_device != source.storage_binding.source_device
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
                (HumanSourceKindV1::TenantStore, Some(tenant)) => {
                    crate::tenancy::validate_tenant_id(tenant)
                        .map_err(|_| HumanAuthorityFailure::InvalidManifest)?;
                    if !tenants.insert(tenant.as_str()) {
                        return Err(HumanAuthorityFailure::InvalidManifest);
                    }
                }
                (HumanSourceKindV1::AuthenticationLedger, None) => authentication_ledgers += 1,
                _ => return Err(HumanAuthorityFailure::InvalidManifest),
            }
        }
        if authentication_ledgers != usize::from(self.configuration.oidc.is_some())
            || self
                .configuration
                .files
                .iter()
                .any(|file| physical.contains(&file.storage_binding.source_identity()))
        {
            return Err(HumanAuthorityFailure::InvalidManifest);
        }
        Ok(())
    }
}

fn same_logical_source(left: &HumanMaintenanceSourceV1, right: &HumanMaintenanceSourceV1) -> bool {
    left.source_id == right.source_id
        && left.kind == right.kind
        && left.installation_id == right.installation_id
        && left.durable_namespace_id == right.durable_namespace_id
        && left.tenant_id == right.tenant_id
}

fn from_active_source(source: &HumanSourceDescriptorV1) -> HumanMaintenanceSourceV1 {
    // Copy expectations from the real retained old manifest; fabricate no receipt.
    HumanMaintenanceSourceV1 {
        source_id: source.source_id,
        kind: source.kind,
        installation_id: source.installation_id,
        durable_namespace_id: source.durable_namespace_id,
        tenant_id: source.tenant_id.clone(),
        storage_binding: source.storage_binding.clone(),
        required_format: source.required_format,
    }
}

/// Independently resolved server configuration plus actual retained private
/// files. Runtime loader input only; it has no Deserialize/plan conversion.
pub(crate) struct HumanMaintenanceResolvedViewV1 {
    view: HumanMaintenanceViewV1,
    files: BTreeMap<HumanConfigurationFileKindV1, PrivateFile>,
}

impl HumanMaintenanceResolvedViewV1 {
    pub(crate) fn new(
        sources: Vec<HumanMaintenanceSourceV1>,
        configuration: HumanAuthorityConfigurationV1,
        files: Vec<(HumanConfigurationFileKindV1, PrivateFile)>,
    ) -> Result<Self, HumanAuthorityFailure> {
        let installation = sources
            .first()
            .ok_or(HumanAuthorityFailure::InvalidManifest)?
            .installation_id;
        let view = HumanMaintenanceViewV1 {
            sources,
            configuration,
        };
        view.validate(installation)?;
        if files.len() != view.configuration.files.len() {
            return Err(HumanAuthorityFailure::ConfigurationMismatch);
        }
        let mut retained = BTreeMap::new();
        for (kind, file) in files {
            let maximum = if kind == HumanConfigurationFileKindV1::OidcTrustBundle {
                262_144
            } else {
                1_048_576
            };
            if file.bytes().len() > maximum || retained.insert(kind, file).is_some() {
                return Err(HumanAuthorityFailure::ConfigurationMismatch);
            }
        }
        if retained.keys().copied().collect::<BTreeSet<_>>()
            != view.configuration.files.iter().map(|f| f.kind).collect()
        {
            return Err(HumanAuthorityFailure::ConfigurationMismatch);
        }
        Ok(Self {
            view,
            files: retained,
        })
    }
}

pub(crate) struct HumanSourcesMaintenanceConfigurationV1 {
    prior: Option<HumanMaintenanceResolvedViewV1>,
    target: HumanMaintenanceResolvedViewV1,
    prior_active_manifest: Option<PrivateFile>,
}

impl HumanSourcesMaintenanceConfigurationV1 {
    pub(crate) fn new(
        prior: Option<HumanMaintenanceResolvedViewV1>,
        target: HumanMaintenanceResolvedViewV1,
        prior_active_manifest: Option<PrivateFile>,
    ) -> Self {
        Self {
            prior,
            target,
            prior_active_manifest,
        }
    }
}

#[derive(Clone)]
pub(crate) struct HumanSourcesMaintenanceGuard {
    owner: Arc<MaintenanceOwner>,
}

#[derive(Clone)]
pub(crate) struct HumanMaintenanceSourceToken {
    owner: Arc<MaintenanceOwner>,
    source_id: Uuid,
    identity: PrivateEntryIdentity,
}

struct MaintenanceOwner {
    plan: HumanSourcesMaintenancePlanV1,
    sources: Vec<RetainedMaintenanceSource>,
    targets: BTreeMap<Uuid, usize>,
    parents: Vec<RetainedParent>,
    configuration: Vec<RetainedConfigurationFile>,
    plan_file: RetainedDocument,
    prior_active: Option<RetainedActiveManifest>,
    invalid: AtomicBool,
}

struct RetainedMaintenanceSource {
    descriptor: HumanMaintenanceSourceV1,
    parent: Directory,
    identity: PrivateEntryIdentity,
}

struct RetainedDocument {
    file: PrivateFile,
    parent: Directory,
    identity: PrivateEntryIdentity,
}

struct RetainedActiveManifest {
    file: RetainedConfigurationFile,
    sources: Vec<HumanSourceDescriptorV1>,
}

macro_rules! private_debug {
    ($($name:ty),+) => {$(impl fmt::Debug for $name {
        fn fmt(&self,f:&mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(concat!(stringify!($name),"(<retained private maintenance>)")) }
    })+};
}
private_debug!(
    HumanMaintenanceResolvedViewV1,
    HumanSourcesMaintenanceConfigurationV1,
    HumanSourcesMaintenanceGuard,
    HumanMaintenanceSourceToken
);

impl HumanSourcesMaintenanceGuard {
    /// Files must already be private and admitted by Root without opening any
    /// SQLite main/SHM inode as a configuration file. No main is created here.
    /// One nonblocking physical-lock attempt; a failed attempt retains no
    /// partially usable capability. New lock files are preserved, never repaired.
    pub(crate) fn acquire(
        plan_file: PrivateFile,
        actual: HumanSourcesMaintenanceConfigurationV1,
    ) -> Result<Self, HumanAuthorityFailure> {
        let deadline = Instant::now() + MAX_MAINTENANCE_OPEN_TIME;
        plan_file
            .persistent_revalidate()
            .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?;
        let plan = HumanSourcesMaintenancePlanV1::from_bytes(plan_file.bytes())?;
        if plan.prior.as_ref() != actual.prior.as_ref().map(|v| &v.view)
            || plan.target != actual.target.view
            || plan.prior_active_manifest.is_some() != actual.prior_active_manifest.is_some()
        {
            return Err(HumanAuthorityFailure::ConfigurationMismatch);
        }
        let prior_active_sources = if let (Some(expected), Some(file)) =
            (&plan.prior_active_manifest, &actual.prior_active_manifest)
        {
            let old = HumanAuthorityClosureV1::from_bytes(file.bytes())?;
            let prior = plan
                .prior
                .as_ref()
                .ok_or(HumanAuthorityFailure::InvalidManifest)?;
            if old.installation_id != plan.installation_id
                || old.closure_id != expected.closure_id
                || old.closure_revision != expected.closure_revision
                || old.closure_digest != expected.closure_digest
                || old.configuration != prior.configuration
                || old
                    .sources
                    .iter()
                    .map(from_active_source)
                    .collect::<Vec<_>>()
                    != prior.sources
            {
                return Err(HumanAuthorityFailure::ConfigurationMismatch);
            }
            Some(old.sources)
        } else {
            None
        };
        let mut sources = Vec::new();
        let mut targets = BTreeMap::new();
        let mut directories = BTreeMap::<(u64, u64), Directory>::new();
        let mut protected = Vec::new();
        for (target_view, view) in actual
            .prior
            .iter()
            .map(|v| (false, v))
            .chain(std::iter::once((true, &actual.target)))
        {
            for descriptor in &view.view.sources {
                check_open_deadline(deadline)?;
                let parent = open_bound_parent(&descriptor.storage_binding)?;
                let identity = bound_source_entry(&parent, &descriptor.storage_binding)?;
                directories
                    .entry(descriptor.storage_binding.parent_identity())
                    .or_insert_with(|| parent.clone());
                if !protected.contains(&identity) {
                    protected.push(identity);
                }
                if target_view {
                    targets.insert(descriptor.source_id, sources.len());
                }
                sources.push(RetainedMaintenanceSource {
                    descriptor: descriptor.clone(),
                    parent,
                    identity,
                });
            }
        }
        let mut configuration = Vec::new();
        let mut config_roles = BTreeMap::new();
        for view in actual.prior.iter().chain(std::iter::once(&actual.target)) {
            for descriptor in &view.view.configuration.files {
                check_open_deadline(deadline)?;
                let file = view
                    .files
                    .get(&descriptor.kind)
                    .ok_or(HumanAuthorityFailure::ConfigurationMismatch)?;
                let parent = open_bound_parent(&descriptor.storage_binding)?;
                let identity = bound_entry(&parent, &descriptor.storage_binding)?;
                if sources.iter().any(|source| source.identity == identity)
                    || file.path() != descriptor.storage_binding.path()
                {
                    return Err(HumanAuthorityFailure::ConfigurationMismatch);
                }
                if let Some(previous) =
                    config_roles.insert(identity.physical_identity(), descriptor.kind)
                {
                    if previous != descriptor.kind {
                        return Err(HumanAuthorityFailure::ConfigurationMismatch);
                    }
                }
                file.persistent_revalidate()
                    .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?;
                if !protected.contains(&identity) {
                    protected.push(identity);
                }
                configuration.push(RetainedConfigurationFile {
                    binding: descriptor.storage_binding.clone(),
                    parent,
                    file: file.clone(),
                });
            }
        }
        let plan_parent_path = plan_file
            .path()
            .parent()
            .ok_or(HumanAuthorityFailure::StorageBindingMismatch)?;
        let plan_name = plan_file
            .path()
            .file_name()
            .ok_or(HumanAuthorityFailure::StorageBindingMismatch)?;
        let parent = Directory::open_existing(plan_parent_path)
            .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?;
        let identity = parent
            .private_entry(plan_name)
            .map_err(|_| HumanAuthorityFailure::StorageBindingMismatch)?
            .ok_or(HumanAuthorityFailure::StorageBindingMismatch)?;
        if protected.contains(&identity) {
            return Err(HumanAuthorityFailure::ConfigurationMismatch);
        }
        protected.push(identity);
        let plan_file = RetainedDocument {
            file: plan_file,
            parent,
            identity,
        };
        let prior_active = match (
            &plan.prior_active_manifest,
            &actual.prior_active_manifest,
            prior_active_sources,
        ) {
            (Some(expected), Some(file), Some(sources)) => {
                let parent = open_bound_parent(&expected.storage_binding)?;
                let identity = bound_entry(&parent, &expected.storage_binding)?;
                if protected.contains(&identity) || file.path() != expected.storage_binding.path() {
                    return Err(HumanAuthorityFailure::ConfigurationMismatch);
                }
                file.persistent_revalidate()
                    .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?;
                protected.push(identity);
                Some(RetainedActiveManifest {
                    file: RetainedConfigurationFile {
                        binding: expected.storage_binding.clone(),
                        parent,
                        file: file.clone(),
                    },
                    sources,
                })
            }
            (None, None, None) => None,
            _ => return Err(HumanAuthorityFailure::ConfigurationMismatch),
        };
        let mut parents = Vec::new();
        for (identity, directory) in directories {
            check_open_deadline(deadline)?;
            let lock = acquire_parent_lock(&directory, &protected)?;
            parents.push(RetainedParent {
                directory,
                identity,
                lock,
            });
        }
        let owner = Arc::new(MaintenanceOwner {
            plan,
            sources,
            targets,
            parents,
            configuration,
            plan_file,
            prior_active,
            invalid: AtomicBool::new(false),
        });
        owner.revalidate()?;
        check_open_deadline(deadline)?;
        Ok(Self { owner })
    }

    pub(crate) fn check(&self) -> Result<(), HumanAuthorityFailure> {
        self.owner.revalidate()
    }

    pub(crate) fn target_source(
        &self,
        source_id: Uuid,
    ) -> Result<HumanMaintenanceSourceToken, HumanAuthorityFailure> {
        self.check()?;
        let index = self
            .owner
            .targets
            .get(&source_id)
            .ok_or(HumanAuthorityFailure::SourceMismatch)?;
        if self.owner.invalid.load(Ordering::Acquire) {
            return Err(HumanAuthorityFailure::OwnerInvalid);
        }
        Ok(HumanMaintenanceSourceToken {
            owner: self.owner.clone(),
            source_id,
            identity: self.owner.sources[*index].identity,
        })
    }
}

impl HumanMaintenanceSourceToken {
    pub(crate) fn check(&self) -> Result<(), HumanAuthorityFailure> {
        self.owner.revalidate()?;
        let index = self
            .owner
            .targets
            .get(&self.source_id)
            .ok_or(HumanAuthorityFailure::SourceMismatch)?;
        if self.owner.sources[*index].identity != self.identity {
            self.owner.invalid.store(true, Ordering::Release);
            return Err(HumanAuthorityFailure::OwnerInvalid);
        }
        Ok(())
    }

    pub(crate) fn same_guard_and_source(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner, &other.owner)
            && self.source_id == other.source_id
            && self.identity == other.identity
    }

    pub(crate) fn descriptor(&self) -> &HumanMaintenanceSourceV1 {
        // Only target_source can create tokens and both maps are immutable.
        &self.owner.sources[self.owner.targets[&self.source_id]].descriptor
    }

    /// Expected receipt for this in-place source; the caller still checks the
    /// live token and verifies the receipt against durable replay.
    pub(crate) fn prior_active_anchor(
        &self,
        prior_format: u32,
    ) -> Result<Option<&HumanAuthorityMarkerReceiptV1>, HumanAuthorityFailure> {
        let Some(prior) = &self.owner.prior_active else {
            return Ok(None);
        };
        let index = prior
            .sources
            .binary_search_by_key(&self.source_id, |source| source.source_id)
            .map_err(|_| HumanAuthorityFailure::SourceMismatch)?;
        let source = &prior.sources[index];
        if source.required_format != prior_format
            || source.storage_binding != self.descriptor().storage_binding
        {
            return Err(HumanAuthorityFailure::SourceMismatch);
        }
        Ok(Some(&source.authority_marker_receipt))
    }

    /// Active anchor of a source that this plan leaves unchanged in place:
    /// the complete prior view equals the target view and the retained prior
    /// active manifest lists the source at the same format and binding. Every
    /// format-changing or initial plan returns `None`. Descriptive only; the
    /// opener still compares the receipt with durable replay.
    pub(crate) fn in_place_anchor(
        &self,
    ) -> Result<Option<&HumanAuthorityMarkerReceiptV1>, HumanAuthorityFailure> {
        let plan = &self.owner.plan;
        if plan.prior.as_ref() != Some(&plan.target) || self.owner.prior_active.is_none() {
            return Ok(None);
        }
        self.prior_active_anchor(self.descriptor().required_format)
    }

    pub(crate) fn migration_id(&self) -> Uuid {
        self.owner.plan.migration_id
    }

    /// Descriptive plan binding. The caller still checks the live token.
    pub(in crate::human_authority) fn retained_plan(
        &self,
    ) -> (&HumanSourcesMaintenancePlanV1, &str) {
        (&self.owner.plan, self.owner.plan_file.file.digest())
    }
}

impl MaintenanceOwner {
    fn revalidate(&self) -> Result<(), HumanAuthorityFailure> {
        if self.invalid.load(Ordering::Acquire) {
            return Err(HumanAuthorityFailure::OwnerInvalid);
        }
        self.complete_revalidation(self.revalidate_filesystem())
    }

    fn complete_revalidation(
        &self,
        result: Result<(), OwnerFilesystemFailure>,
    ) -> Result<(), HumanAuthorityFailure> {
        if self.invalid.load(Ordering::Acquire) {
            return Err(HumanAuthorityFailure::OwnerInvalid);
        }
        match result {
            Ok(()) => Ok(()),
            Err(OwnerFilesystemFailure::DescriptorShortage) => {
                Err(HumanAuthorityFailure::StorageUnavailable)
            }
            Err(OwnerFilesystemFailure::Permanent(_)) => {
                self.invalid.store(true, Ordering::Release);
                Err(HumanAuthorityFailure::OwnerInvalid)
            }
        }
    }

    fn revalidate_filesystem(&self) -> Result<(), OwnerFilesystemFailure> {
        for parent in &self.parents {
            if parent
                .directory
                .physical_identity()
                .map_err(OwnerFilesystemFailure::unavailable)?
                != parent.identity
            {
                return Err(OwnerFilesystemFailure::Permanent(
                    HumanAuthorityFailure::StorageBindingMismatch,
                ));
            }
            parent
                .directory
                .private_entry_same_mount(OsStr::new(super::OWNER_LOCK_NAME))
                .map_err(OwnerFilesystemFailure::binding)?;
            parent
                .lock
                .persistent_revalidate()
                .map_err(OwnerFilesystemFailure::unavailable)?;
            if parent.lock.bytes() != super::OWNER_LOCK_PROTOCOL {
                return Err(OwnerFilesystemFailure::Permanent(
                    HumanAuthorityFailure::StorageBindingMismatch,
                ));
            }
        }
        for source in &self.sources {
            if bound_source_entry(&source.parent, &source.descriptor.storage_binding)?
                != source.identity
            {
                return Err(OwnerFilesystemFailure::Permanent(
                    HumanAuthorityFailure::StorageBindingMismatch,
                ));
            }
        }
        for file in self
            .configuration
            .iter()
            .chain(self.prior_active.iter().map(|prior| &prior.file))
        {
            bound_entry(&file.parent, &file.binding)?;
            file.file
                .persistent_revalidate()
                .map_err(OwnerFilesystemFailure::unavailable)?;
        }
        let name =
            self.plan_file
                .file
                .path()
                .file_name()
                .ok_or(OwnerFilesystemFailure::Permanent(
                    HumanAuthorityFailure::StorageBindingMismatch,
                ))?;
        if self
            .plan_file
            .parent
            .private_entry(name)
            .map_err(OwnerFilesystemFailure::binding)?
            != Some(self.plan_file.identity)
        {
            return Err(OwnerFilesystemFailure::Permanent(
                HumanAuthorityFailure::StorageBindingMismatch,
            ));
        }
        self.plan_file
            .file
            .persistent_revalidate()
            .map_err(OwnerFilesystemFailure::unavailable)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
