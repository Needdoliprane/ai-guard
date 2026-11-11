//! Retained local source ownership and the shared human-authority gate.
//!
//! This is filesystem ownership only. Neither a manifest DTO nor a successful
//! acquisition verifies SQLite formats, source markers, an incarnation record,
//! a person's authority or readiness. Modern Store/AuthLedger constructors must
//! compose those checks before exposing any participating mutable handle.
//!
//! No SQLite, backend, network or runtime work occurs here. Callers finish
//! external work before taking an epoch, close source transactions before
//! opening a target transaction, and validate the epoch/token after SQLite
//! waits, before/after commit and before returning an emission permission.
//! `run_read`/`run_write` retain accepted synchronous work even if its observer
//! is cancelled. A dropped future is never evidence that a commit stopped.
#![cfg(unix)]

mod manifest;
pub use manifest::*;

use crate::control_fs::{Directory, PrivateEntryIdentity, PrivateFile};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use tokio::sync::{Notify, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use uuid::Uuid;

const OWNER_LOCK_NAME: &str = ".ai-guard-human-authority-v1.lock";
const OWNER_LOCK_PROTOCOL: &[u8] =
    b"ai-guard-human-authority-owner\nversion=1\nprofile=local_posix_flock_v1\n";
const MAX_OWNER_OPEN_TIME: Duration = Duration::from_secs(5);
const MAX_GATE_WAIT: Duration = Duration::from_millis(250);
const MAX_EPOCH_TIME: Duration = Duration::from_secs(2);

/// Errors deliberately contain no path, source bytes or chained IO/parser error.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HumanAuthorityFailure {
    InvalidManifest,
    ConfigurationMismatch,
    StorageBindingMismatch,
    StorageUnavailable,
    OwnerContended,
    OwnerInvalid,
    Closing,
    GateBusy,
    EpochExpired,
    SourceMismatch,
    TaskFailed,
    OpeningTimedOut,
}

impl fmt::Display for HumanAuthorityFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidManifest => "human_authority_invalid_manifest",
            Self::ConfigurationMismatch => "human_authority_configuration_mismatch",
            Self::StorageBindingMismatch => "human_authority_storage_binding_mismatch",
            Self::StorageUnavailable => "human_authority_storage_unavailable",
            Self::OwnerContended => "human_authority_owner_contended",
            Self::OwnerInvalid => "human_authority_owner_invalid",
            Self::Closing => "human_authority_closing",
            Self::GateBusy => "human_authority_gate_busy",
            Self::EpochExpired => "human_authority_epoch_expired",
            Self::SourceMismatch => "human_authority_source_mismatch",
            Self::TaskFailed => "human_authority_task_failed",
            Self::OpeningTimedOut => "human_authority_opening_timed_out",
        })
    }
}

impl fmt::Debug for HumanAuthorityFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for HumanAuthorityFailure {}

/// Internal assertion of a discordance proven by the real source/configuration
/// validators. This is not a DTO, a readiness proof or a conversion from errors.
/// Invalid HTTP input, a foreign/expired human proof, permission refusal, busy
/// or unavailable capacity must never be classified as either of these reasons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HumanAuthorityInvalidationReason {
    DurableAuthorityMismatch,
    RetainedConfigurationMismatch,
}

/// Private distinction made before sanitized public error mapping. Only an
/// actual structured descriptor-shortage cause can avoid the permanent latch;
/// the public StorageUnavailable category is deliberately not this evidence.
enum OwnerFilesystemFailure {
    DescriptorShortage,
    Permanent(HumanAuthorityFailure),
}

impl OwnerFilesystemFailure {
    fn classify(error: anyhow::Error, permanent: HumanAuthorityFailure) -> Self {
        if error.chain().any(|cause| {
            cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
                matches!(io.raw_os_error(), Some(libc::EMFILE) | Some(libc::ENFILE))
            })
        }) {
            Self::DescriptorShortage
        } else {
            Self::Permanent(permanent)
        }
    }

    fn unavailable(error: anyhow::Error) -> Self {
        Self::classify(error, HumanAuthorityFailure::StorageUnavailable)
    }

    fn binding(error: anyhow::Error) -> Self {
        Self::classify(error, HumanAuthorityFailure::StorageBindingMismatch)
    }
}

impl From<OwnerFilesystemFailure> for HumanAuthorityFailure {
    fn from(failure: OwnerFilesystemFailure) -> Self {
        match failure {
            OwnerFilesystemFailure::DescriptorShortage => Self::StorageUnavailable,
            OwnerFilesystemFailure::Permanent(failure) => failure,
        }
    }
}

/// Actual resolved control configuration, not deserializable runtime evidence.
/// Root must supply *all* configured tenants and the AuthLedger if OIDC is on.
/// Files must have been admitted before opening, without aliasing SQLite main
/// or SHM. Credential bytes/digests never enter the public configuration digest.
pub struct HumanSourcesConfigurationV1 {
    sources: Vec<HumanSourceDescriptorV1>,
    configuration: HumanAuthorityConfigurationV1,
    files: BTreeMap<HumanConfigurationFileKindV1, PrivateFile>,
}

impl HumanSourcesConfigurationV1 {
    pub fn new(
        sources: Vec<HumanSourceDescriptorV1>,
        configuration: HumanAuthorityConfigurationV1,
        files: Vec<(HumanConfigurationFileKindV1, PrivateFile)>,
    ) -> Result<Self, HumanAuthorityFailure> {
        let installation_id = sources
            .first()
            .ok_or(HumanAuthorityFailure::InvalidManifest)?
            .installation_id;
        manifest::validate_sources(&sources, installation_id)?;
        configuration.validate()?;
        let mut retained = BTreeMap::new();
        if files.len() != configuration.files.len() {
            return Err(HumanAuthorityFailure::ConfigurationMismatch);
        }
        for (kind, file) in files {
            let maximum = if kind == HumanConfigurationFileKindV1::OidcTrustBundle {
                262_144
            } else {
                1_048_576
            };
            if file.bytes().len() > maximum {
                return Err(HumanAuthorityFailure::ConfigurationMismatch);
            }
            if retained.insert(kind, file).is_some() {
                return Err(HumanAuthorityFailure::ConfigurationMismatch);
            }
        }
        if retained.keys().copied().collect::<BTreeSet<_>>()
            != configuration.files.iter().map(|file| file.kind).collect()
        {
            return Err(HumanAuthorityFailure::ConfigurationMismatch);
        }
        Ok(Self {
            sources,
            configuration,
            files: retained,
        })
    }
}

#[derive(Clone)]
pub struct OwnedHumanSources {
    owner: Arc<Owner>,
}

/// This token owns the actual retained owner, not just a copied UUID or flag.
#[derive(Clone)]
pub struct OwnedSourceToken {
    owner: Arc<Owner>,
    source_id: Uuid,
    incarnation: Uuid,
}

pub struct HumanSourcesReadEpoch {
    // Guard is dropped before the lease marks the accepted work as drained.
    _guard: OwnedRwLockReadGuard<()>,
    lease: EpochLease,
}

pub struct HumanSourcesWriteEpoch {
    _guard: OwnedRwLockWriteGuard<()>,
    lease: EpochLease,
}

struct Owner {
    closure: HumanAuthorityClosureV1,
    incarnation: Uuid,
    sources: BTreeMap<Uuid, RetainedSource>,
    parents: Vec<RetainedParent>,
    manifest: PrivateFile,
    manifest_parent: Directory,
    manifest_identity: PrivateEntryIdentity,
    configuration_files: Vec<RetainedConfigurationFile>,
    gate: Arc<RwLock<()>>,
    state: Mutex<OwnerState>,
    drained: Notify,
}

struct RetainedSource {
    descriptor: HumanSourceDescriptorV1,
    parent: Directory,
    identity: PrivateEntryIdentity,
}

struct RetainedParent {
    directory: Directory,
    identity: (u64, u64),
    lock: PrivateFile,
}

struct RetainedConfigurationFile {
    binding: SourceStorageBindingV1,
    parent: Directory,
    file: PrivateFile,
}

struct OwnerState {
    invalid: bool,
    closing: bool,
    active: usize,
}

struct EpochLease {
    owner: Arc<Owner>,
    incarnation: Uuid,
    deadline: Instant,
}

macro_rules! opaque_debug {
    ($($type:ty),+ $(,)?) => {$(
        impl fmt::Debug for $type {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($type), "(<retained private authority>)"))
            }
        }
    )+};
}
opaque_debug!(
    HumanSourcesConfigurationV1,
    OwnedHumanSources,
    OwnedSourceToken,
    HumanSourcesReadEpoch,
    HumanSourcesWriteEpoch
);

impl OwnedHumanSources {
    /// One nonblocking attempt in canonical physical parent order. No SQLite
    /// descriptor is opened here. Existing locks are never replaced or removed.
    /// File-only mounts and unobservable mount domains refuse through the
    /// control_fs `private_entry_same_mount` guard, including on unsupported OS.
    pub fn acquire(
        manifest_file: PrivateFile,
        configured: HumanSourcesConfigurationV1,
    ) -> Result<Self, HumanAuthorityFailure> {
        let deadline = Instant::now() + MAX_OWNER_OPEN_TIME;
        manifest_file
            .persistent_revalidate()
            .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?;
        let closure = HumanAuthorityClosureV1::from_bytes(manifest_file.bytes())?;
        if closure.sources != configured.sources
            || closure.configuration != configured.configuration
        {
            return Err(HumanAuthorityFailure::ConfigurationMismatch);
        }
        let mut sources = BTreeMap::new();
        let mut directories: BTreeMap<(u64, u64), Directory> = BTreeMap::new();
        let mut protected_identities = Vec::new();
        for descriptor in &closure.sources {
            check_open_deadline(deadline)?;
            let binding = &descriptor.storage_binding;
            let parent = open_bound_parent(binding)?;
            let identity = bound_source_entry(&parent, binding)?;
            if protected_identities.contains(&identity) {
                return Err(HumanAuthorityFailure::StorageBindingMismatch);
            }
            protected_identities.push(identity);
            directories
                .entry(binding.parent_identity())
                .or_insert_with(|| parent.clone());
            sources.insert(
                descriptor.source_id,
                RetainedSource {
                    descriptor: descriptor.clone(),
                    parent,
                    identity,
                },
            );
        }

        let mut configuration_files = Vec::new();
        for descriptor in &closure.configuration.files {
            check_open_deadline(deadline)?;
            let file = configured
                .files
                .get(&descriptor.kind)
                .ok_or(HumanAuthorityFailure::ConfigurationMismatch)?;
            let parent = open_bound_parent(&descriptor.storage_binding)?;
            let identity = bound_entry(&parent, &descriptor.storage_binding)?;
            if file.path() != descriptor.storage_binding.path()
                || protected_identities.contains(&identity)
            {
                return Err(HumanAuthorityFailure::ConfigurationMismatch);
            }
            file.persistent_revalidate()
                .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?;
            protected_identities.push(identity);
            configuration_files.push(RetainedConfigurationFile {
                binding: descriptor.storage_binding.clone(),
                parent,
                file: file.clone(),
            });
        }
        // Admit the manifest's pathname by metadata only, before lock opens.
        let manifest_parent_path = manifest_file
            .path()
            .parent()
            .ok_or(HumanAuthorityFailure::StorageBindingMismatch)?;
        let manifest_name = manifest_file
            .path()
            .file_name()
            .ok_or(HumanAuthorityFailure::StorageBindingMismatch)?;
        let manifest_parent = Directory::open_existing(manifest_parent_path)
            .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?;
        let manifest_identity = manifest_parent
            .private_entry(manifest_name)
            .map_err(|_| HumanAuthorityFailure::StorageBindingMismatch)?
            .ok_or(HumanAuthorityFailure::StorageBindingMismatch)?;
        if protected_identities.contains(&manifest_identity) {
            return Err(HumanAuthorityFailure::ConfigurationMismatch);
        }
        protected_identities.push(manifest_identity);

        let mut parents = Vec::new();
        // BTreeMap ordering uses physical (device,inode), never path spelling.
        for (identity, directory) in directories {
            check_open_deadline(deadline)?;
            let lock = acquire_parent_lock(&directory, &protected_identities)?;
            parents.push(RetainedParent {
                directory,
                identity,
                lock,
            });
        }
        let owner = Arc::new(Owner {
            closure,
            incarnation: Uuid::new_v4(),
            sources,
            parents,
            manifest: manifest_file,
            manifest_parent,
            manifest_identity,
            configuration_files,
            gate: Arc::new(RwLock::new(())),
            state: Mutex::new(OwnerState {
                invalid: false,
                closing: false,
                active: 0,
            }),
            drained: Notify::new(),
        });
        owner.revalidate()?;
        check_open_deadline(deadline)?;
        Ok(Self { owner })
    }

    /// Expected private manifest only; consumers must verify actual source replay.
    pub fn closure(&self) -> &HumanAuthorityClosureV1 {
        &self.owner.closure
    }

    /// New random runtime incarnation. It becomes durable only through Root's
    /// verified incarnation-record protocol, never by calling this getter.
    pub fn incarnation(&self) -> Uuid {
        self.owner.incarnation
    }

    /// Latch the entire closure after an internally proven durable authority or
    /// retained-configuration mismatch. Existing epochs fail their next check;
    /// the actual owner FDs remain retained until accepted work/references end.
    /// Callers must not derive this assertion from unverified request data or
    /// ordinary per-command/proof/capacity failures. Normal shutdown uses close.
    pub(crate) fn invalidate_authority(&self, reason: HumanAuthorityInvalidationReason) {
        self.owner.invalidate_authority(reason);
    }

    pub fn source_token(&self, source_id: Uuid) -> Result<OwnedSourceToken, HumanAuthorityFailure> {
        self.owner.check_admission()?;
        self.owner.revalidate()?;
        if !self.owner.sources.contains_key(&source_id) {
            return Err(HumanAuthorityFailure::SourceMismatch);
        }
        // Close racing with IO above must not issue a fresh token afterwards.
        self.owner.check_admission()?;
        Ok(OwnedSourceToken {
            owner: self.owner.clone(),
            source_id,
            incarnation: self.owner.incarnation,
        })
    }

    pub async fn read_epoch(&self) -> Result<HumanSourcesReadEpoch, HumanAuthorityFailure> {
        self.owner.check_admission()?;
        let guard = tokio::time::timeout(MAX_GATE_WAIT, self.owner.gate.clone().read_owned())
            .await
            .map_err(|_| HumanAuthorityFailure::GateBusy)?;
        let lease = self.owner.accept_epoch()?;
        Ok(HumanSourcesReadEpoch {
            _guard: guard,
            lease,
        })
    }

    pub async fn write_epoch(&self) -> Result<HumanSourcesWriteEpoch, HumanAuthorityFailure> {
        self.owner.check_admission()?;
        let guard = tokio::time::timeout(MAX_GATE_WAIT, self.owner.gate.clone().write_owned())
            .await
            .map_err(|_| HumanAuthorityFailure::GateBusy)?;
        let lease = self.owner.accept_epoch()?;
        Ok(HumanSourcesWriteEpoch {
            _guard: guard,
            lease,
        })
    }

    pub fn try_read_epoch(&self) -> Result<HumanSourcesReadEpoch, HumanAuthorityFailure> {
        self.owner.check_admission()?;
        let guard = self
            .owner
            .gate
            .clone()
            .try_read_owned()
            .map_err(|_| HumanAuthorityFailure::GateBusy)?;
        let lease = self.owner.accept_epoch()?;
        Ok(HumanSourcesReadEpoch {
            _guard: guard,
            lease,
        })
    }

    pub fn try_write_epoch(&self) -> Result<HumanSourcesWriteEpoch, HumanAuthorityFailure> {
        self.owner.check_admission()?;
        let guard = self
            .owner
            .gate
            .clone()
            .try_write_owned()
            .map_err(|_| HumanAuthorityFailure::GateBusy)?;
        let lease = self.owner.accept_epoch()?;
        Ok(HumanSourcesWriteEpoch {
            _guard: guard,
            lease,
        })
    }

    /// Only synchronous source/target work belongs in this closure. No async
    /// child may outlive it. The closure must validate after SQLite waits and
    /// immediately before/after commit; an outer recheck cannot substitute.
    pub async fn run_read<T, F>(&self, work: F) -> Result<T, HumanAuthorityFailure>
    where
        T: Send + 'static,
        F: FnOnce(&HumanSourcesReadEpoch) -> Result<T, HumanAuthorityFailure> + Send + 'static,
    {
        let epoch = self.read_epoch().await?;
        tokio::task::spawn_blocking(move || {
            // Includes blocking-pool queue delay in the logical epoch budget.
            epoch.check()?;
            let result = work(&epoch);
            epoch.check()?;
            result
        })
        .await
        .map_err(|_| HumanAuthorityFailure::TaskFailed)?
    }

    pub async fn run_write<T, F>(&self, work: F) -> Result<T, HumanAuthorityFailure>
    where
        T: Send + 'static,
        F: FnOnce(&HumanSourcesWriteEpoch) -> Result<T, HumanAuthorityFailure> + Send + 'static,
    {
        let epoch = self.write_epoch().await?;
        tokio::task::spawn_blocking(move || {
            epoch.check()?;
            let result = work(&epoch);
            epoch.check()?;
            result
        })
        .await
        .map_err(|_| HumanAuthorityFailure::TaskFailed)?
    }

    /// Stops new admissions. Existing epochs remain usable within their budget
    /// and retain the OS owner until their accepted work has actually returned.
    pub fn begin_close(&self) {
        match self.owner.state.lock() {
            Ok(mut state) => state.closing = true,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.invalid = true;
                state.closing = true;
            }
        }
        self.owner.drained.notify_waiters();
    }

    /// Returns when accepted epochs/tasks have finished. It does not unlock
    /// remaining tokens/handles and promises no bound on a storage syscall.
    /// Calling this while retaining one's own epoch would wait on that epoch.
    pub async fn drain(&self) -> Result<(), HumanAuthorityFailure> {
        self.begin_close();
        loop {
            let notification = self.owner.drained.notified();
            tokio::pin!(notification);
            notification.as_mut().enable();
            let (active, invalid) = {
                let state = self.owner.lock_state()?;
                (state.active, state.invalid)
            };
            if active == 0 {
                return if invalid {
                    Err(HumanAuthorityFailure::OwnerInvalid)
                } else {
                    Ok(())
                };
            }
            notification.await;
        }
    }
}

impl OwnedSourceToken {
    /// Same global invalidator as the owner, reachable from a retained source
    /// transaction only after its validator has proven the technical mismatch.
    /// No permission/proof refusal, expiry or capacity error is such a proof.
    pub(crate) fn invalidate_authority(&self, reason: HumanAuthorityInvalidationReason) {
        self.owner.invalidate_authority(reason);
    }

    pub fn source_id(&self) -> Uuid {
        self.source_id
    }

    pub fn incarnation(&self) -> Uuid {
        self.incarnation
    }

    /// Expected descriptor; not a checked source-format or marker receipt.
    pub fn descriptor(&self) -> &HumanSourceDescriptorV1 {
        // Tokens can only be constructed after exact membership admission.
        &self.owner.sources[&self.source_id].descriptor
    }

    pub fn closure(&self) -> &HumanAuthorityClosureV1 {
        &self.owner.closure
    }
}

impl HumanSourcesReadEpoch {
    pub fn check(&self) -> Result<(), HumanAuthorityFailure> {
        self.lease.check()
    }

    pub fn validate_source(&self, token: &OwnedSourceToken) -> Result<(), HumanAuthorityFailure> {
        self.lease.validate_source(token)
    }
}

impl HumanSourcesWriteEpoch {
    pub fn check(&self) -> Result<(), HumanAuthorityFailure> {
        self.lease.check()
    }

    pub fn validate_source(&self, token: &OwnedSourceToken) -> Result<(), HumanAuthorityFailure> {
        self.lease.validate_source(token)
    }
}

impl Owner {
    fn lock_state(&self) -> Result<MutexGuard<'_, OwnerState>, HumanAuthorityFailure> {
        self.state
            .lock()
            .map_err(|_| HumanAuthorityFailure::OwnerInvalid)
    }

    fn check_admission(&self) -> Result<(), HumanAuthorityFailure> {
        let state = self.lock_state()?;
        if state.invalid {
            Err(HumanAuthorityFailure::OwnerInvalid)
        } else if state.closing {
            Err(HumanAuthorityFailure::Closing)
        } else {
            Ok(())
        }
    }

    fn latch_invalid(&self) {
        match self.state.lock() {
            Ok(mut state) => state.invalid = true,
            Err(poisoned) => poisoned.into_inner().invalid = true,
        }
        self.drained.notify_waiters();
    }

    fn invalidate_authority(&self, reason: HumanAuthorityInvalidationReason) {
        match reason {
            HumanAuthorityInvalidationReason::DurableAuthorityMismatch
            | HumanAuthorityInvalidationReason::RetainedConfigurationMismatch => {
                self.latch_invalid()
            }
        }
    }

    fn accept_epoch(self: &Arc<Self>) -> Result<EpochLease, HumanAuthorityFailure> {
        let deadline = Instant::now() + MAX_EPOCH_TIME;
        self.revalidate()?;
        if Instant::now() >= deadline {
            return Err(HumanAuthorityFailure::EpochExpired);
        }
        let mut state = self.lock_state()?;
        if state.invalid {
            return Err(HumanAuthorityFailure::OwnerInvalid);
        }
        if state.closing {
            return Err(HumanAuthorityFailure::Closing);
        }
        state.active = state
            .active
            .checked_add(1)
            .ok_or(HumanAuthorityFailure::GateBusy)?;
        Ok(EpochLease {
            owner: self.clone(),
            incarnation: self.incarnation,
            deadline,
        })
    }

    fn revalidate(&self) -> Result<(), HumanAuthorityFailure> {
        if self.lock_state()?.invalid {
            return Err(HumanAuthorityFailure::OwnerInvalid);
        }
        // Always run the complete validation. No successful snapshot is cached
        // or reused after a shortage; this call never retries an IO operation.
        self.complete_filesystem_revalidation(self.revalidate_filesystem())
    }

    fn complete_filesystem_revalidation(
        &self,
        result: Result<(), OwnerFilesystemFailure>,
    ) -> Result<(), HumanAuthorityFailure> {
        // Another read epoch can have detected a failure concurrently.
        if self.lock_state()?.invalid {
            return Err(HumanAuthorityFailure::OwnerInvalid);
        }
        match result {
            Ok(()) => Ok(()),
            Err(OwnerFilesystemFailure::DescriptorShortage) => {
                // Refuse this operation without poisoning or releasing the
                // retained owner/guard. Every later access starts above anew.
                Err(HumanAuthorityFailure::StorageUnavailable)
            }
            Err(OwnerFilesystemFailure::Permanent(_)) => {
                self.latch_invalid();
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
                .private_entry_same_mount(OsStr::new(OWNER_LOCK_NAME))
                .map_err(OwnerFilesystemFailure::binding)?;
            parent
                .lock
                .persistent_revalidate()
                .map_err(OwnerFilesystemFailure::unavailable)?;
            if parent.lock.bytes() != OWNER_LOCK_PROTOCOL {
                return Err(OwnerFilesystemFailure::Permanent(
                    HumanAuthorityFailure::StorageBindingMismatch,
                ));
            }
        }
        for source in self.sources.values() {
            if bound_source_entry(&source.parent, &source.descriptor.storage_binding)?
                != source.identity
            {
                return Err(OwnerFilesystemFailure::Permanent(
                    HumanAuthorityFailure::StorageBindingMismatch,
                ));
            }
        }
        for file in &self.configuration_files {
            bound_entry(&file.parent, &file.binding)?;
            file.file
                .persistent_revalidate()
                .map_err(OwnerFilesystemFailure::unavailable)?;
        }
        let manifest_name =
            self.manifest
                .path()
                .file_name()
                .ok_or(OwnerFilesystemFailure::Permanent(
                    HumanAuthorityFailure::StorageBindingMismatch,
                ))?;
        if self
            .manifest_parent
            .private_entry(manifest_name)
            .map_err(OwnerFilesystemFailure::binding)?
            != Some(self.manifest_identity)
        {
            return Err(OwnerFilesystemFailure::Permanent(
                HumanAuthorityFailure::StorageBindingMismatch,
            ));
        }
        self.manifest
            .persistent_revalidate()
            .map_err(OwnerFilesystemFailure::unavailable)?;
        Ok(())
    }
}

impl EpochLease {
    fn check(&self) -> Result<(), HumanAuthorityFailure> {
        // Expiration alone is not an owner fault and must never latch it.
        if Instant::now() >= self.deadline {
            return Err(HumanAuthorityFailure::EpochExpired);
        }
        if self.incarnation != self.owner.incarnation {
            self.owner.latch_invalid();
            return Err(HumanAuthorityFailure::OwnerInvalid);
        }
        self.owner.revalidate()?;
        if Instant::now() >= self.deadline {
            return Err(HumanAuthorityFailure::EpochExpired);
        }
        Ok(())
    }

    fn validate_source(&self, token: &OwnedSourceToken) -> Result<(), HumanAuthorityFailure> {
        if !Arc::ptr_eq(&self.owner, &token.owner)
            || self.incarnation != token.incarnation
            || !self.owner.sources.contains_key(&token.source_id)
        {
            return Err(HumanAuthorityFailure::SourceMismatch);
        }
        self.check()
    }
}

impl Drop for EpochLease {
    fn drop(&mut self) {
        match self.owner.state.lock() {
            Ok(mut state) => {
                if let Some(active) = state.active.checked_sub(1) {
                    state.active = active;
                } else {
                    state.invalid = true;
                }
            }
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.active = state.active.saturating_sub(1);
                state.invalid = true;
            }
        }
        self.owner.drained.notify_waiters();
    }
}

fn open_bound_parent(binding: &SourceStorageBindingV1) -> Result<Directory, HumanAuthorityFailure> {
    let parent = Directory::open_existing(&binding.parent_path)
        .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?;
    if parent
        .physical_identity()
        .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?
        != binding.parent_identity()
    {
        return Err(HumanAuthorityFailure::StorageBindingMismatch);
    }
    Ok(parent)
}

fn bound_entry(
    parent: &Directory,
    binding: &SourceStorageBindingV1,
) -> Result<PrivateEntryIdentity, OwnerFilesystemFailure> {
    if parent
        .physical_identity()
        .map_err(OwnerFilesystemFailure::unavailable)?
        != binding.parent_identity()
    {
        return Err(OwnerFilesystemFailure::Permanent(
            HumanAuthorityFailure::StorageBindingMismatch,
        ));
    }
    let identity = parent
        .private_entry(OsStr::new(&binding.file_name))
        .map_err(OwnerFilesystemFailure::binding)?
        .ok_or(OwnerFilesystemFailure::Permanent(
            HumanAuthorityFailure::StorageBindingMismatch,
        ))?;
    if identity.physical_identity() != binding.source_identity() {
        return Err(OwnerFilesystemFailure::Permanent(
            HumanAuthorityFailure::StorageBindingMismatch,
        ));
    }
    Ok(identity)
}

/// Only SQLite sources choose the parent lock domain. Private configuration
/// files remain identity/content pinned even if separately projected. Such a
/// projection must still satisfy every existing Directory/PrivateFile guard.
fn bound_source_entry(
    parent: &Directory,
    binding: &SourceStorageBindingV1,
) -> Result<PrivateEntryIdentity, OwnerFilesystemFailure> {
    let expected = bound_entry(parent, binding)?;
    let observed = parent
        .private_entry_same_mount(OsStr::new(&binding.file_name))
        .map_err(OwnerFilesystemFailure::binding)?;
    if expected != observed {
        return Err(OwnerFilesystemFailure::Permanent(
            HumanAuthorityFailure::StorageBindingMismatch,
        ));
    }
    Ok(observed)
}

fn acquire_parent_lock(
    parent: &Directory,
    exclusions: &[PrivateEntryIdentity],
) -> Result<PrivateFile, HumanAuthorityFailure> {
    if parent
        .private_entry(OsStr::new(OWNER_LOCK_NAME))
        .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?
        .is_some()
    {
        let identity = parent
            .private_entry_same_mount(OsStr::new(OWNER_LOCK_NAME))
            .map_err(|_| HumanAuthorityFailure::StorageBindingMismatch)?;
        if exclusions.contains(&identity) {
            return Err(HumanAuthorityFailure::StorageBindingMismatch);
        }
    }
    let lock = match parent
        .persistent_read(OWNER_LOCK_NAME, 128, exclusions)
        .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?
    {
        Some(file) => file,
        None => match parent.persistent_create(OWNER_LOCK_NAME, OWNER_LOCK_PROTOCOL) {
            Ok(file) => {
                file.persistent_sync()
                    .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?;
                parent
                    .sync()
                    .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?;
                file
            }
            // A concurrent creator may have won. There is no repair, rewrite,
            // backoff or second ownership attempt; incomplete bytes refuse.
            Err(_) => parent
                .persistent_read(OWNER_LOCK_NAME, 128, exclusions)
                .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?
                .ok_or(HumanAuthorityFailure::StorageUnavailable)?,
        },
    };
    parent
        .private_entry_same_mount(OsStr::new(OWNER_LOCK_NAME))
        .map_err(|_| HumanAuthorityFailure::StorageBindingMismatch)?;
    if lock.bytes() != OWNER_LOCK_PROTOCOL {
        return Err(HumanAuthorityFailure::StorageBindingMismatch);
    }
    if !lock
        .persistent_try_lock()
        .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?
    {
        return Err(HumanAuthorityFailure::OwnerContended);
    }
    lock.persistent_revalidate()
        .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?;
    parent
        .revalidate()
        .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?;
    Ok(lock)
}

fn check_open_deadline(deadline: Instant) -> Result<(), HumanAuthorityFailure> {
    if Instant::now() >= deadline {
        Err(HumanAuthorityFailure::OpeningTimedOut)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
