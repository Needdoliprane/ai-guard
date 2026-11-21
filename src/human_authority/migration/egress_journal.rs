//! Private E1 intention, backup preparation and committed genesis records.

use crate::{
    audit::{
        AuthorityReceiptV1, ReviewStoreMaintenanceWriter, Store,
        egress::{EgressStoreLimitsV1, VerifiedEgressSourceBackup},
    },
    control_fs::{Directory, PrivateEntryIdentity, PrivateFile, publication::PublicationFailure},
    human_authority::{
        HumanAuthorityMarkerReceiptV1, HumanSourceKindV1, SourceStorageBindingV1,
        maintenance::{
            HumanMaintenanceActiveManifestV1, HumanMaintenanceSourceToken,
            HumanMaintenanceSourceV1, HumanSourcesMaintenancePlanV1,
        },
    },
    redaction::Redactor,
};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const INTENT_NAME: &str = "egress15-intent-v2";
const BACKUP_RECORD_NAME: &str = "egress15-backup-v2";
const CONFIRMED_NAME: &str = "egress15-confirmed-v2";
const RECORD_NAME: &str = "record-v1.json";
const STAGE_PREFIX: &str = "egress15-stage-";
const MAX_JOURNAL_ENTRIES: u32 = 32;
// One unrenamed confirmation stage may survive a crash. Preserve a slot
// for its replacement before committing SQLite.
const CONFIRMATION_SLOTS: u32 = 2;
const BACKUP_NAME: &str = "source.sqlite";
const MAX_RECORD_BYTES: u64 = 128 * 1024;
const SQLITE_SUFFIXES: [&str; 4] = ["", "-wal", "-shm", "-journal"];

#[derive(Serialize)]
struct DirectoryBinding<'a> {
    path: &'a Path,
    device: u64,
    inode: u64,
}

impl<'a> DirectoryBinding<'a> {
    fn observe(directory: &'a Directory) -> Result<Self> {
        let (device, inode) = directory.physical_identity()?;
        Ok(Self {
            path: directory.path(),
            device,
            inode,
        })
    }

    fn identity(&self) -> (u64, u64) {
        (self.device, self.inode)
    }
}

// Reconstructed from live admitted inputs rather than deserialized as authority.
#[derive(Serialize)]
struct IntentV1<'a> {
    version: u32,
    migration_id: Uuid,
    plan_digest: &'a str,
    prior_manifest: &'a HumanMaintenanceActiveManifestV1,
    target_closure_revision: u64,
    source: &'a HumanMaintenanceSourceV1,
    prior_anchor: &'a HumanAuthorityMarkerReceiptV1,
    source_prefix: &'a AuthorityReceiptV1,
    limits: &'a EgressStoreLimitsV1,
    journal: DirectoryBinding<'a>,
    backup_parent: DirectoryBinding<'a>,
    backup_name: &'static str,
}

#[derive(Serialize)]
struct BackupV1<'a> {
    version: u32,
    intent_digest: &'a str,
    binding: SourceStorageBindingV1,
    source_prefix: AuthorityReceiptV1,
    event_count: u64,
}

#[derive(Serialize)]
struct ConfirmedV1<'a> {
    version: u32,
    intent_digest: &'a str,
    backup_digest: &'a str,
    receipt: &'a AuthorityReceiptV1,
}

pub(crate) struct RetainedEgressIntent<'a> {
    token: &'a HumanMaintenanceSourceToken,
    journal: Directory,
    backup_parent: Directory,
    file: PrivateFile,
    source_prefix: AuthorityReceiptV1,
    limits: EgressStoreLimitsV1,
}

pub(crate) struct RetainedEgressBackup<'intent, 'token> {
    intent: &'intent RetainedEgressIntent<'token>,
    frame: PrivateFile,
    proof: VerifiedEgressSourceBackup<'token>,
}

impl<'a> RetainedEgressIntent<'a> {
    /// First attempt, with no published record and an absent backup.
    pub(crate) fn prepare(
        token: &'a HumanMaintenanceSourceToken,
        writer: &ReviewStoreMaintenanceWriter<'_, '_>,
        journal: Directory,
        backup_parent: Directory,
        limits: &EgressStoreLimitsV1,
    ) -> Result<Self> {
        Self::enter(token, writer, journal, backup_parent, limits, true)
    }

    /// Reopen a complete intent after loss of acknowledgement.
    pub(crate) fn resume(
        token: &'a HumanMaintenanceSourceToken,
        writer: &ReviewStoreMaintenanceWriter<'_, '_>,
        journal: Directory,
        backup_parent: Directory,
        limits: &EgressStoreLimitsV1,
    ) -> Result<Self> {
        Self::enter(token, writer, journal, backup_parent, limits, false)
    }

    fn enter(
        token: &'a HumanMaintenanceSourceToken,
        writer: &ReviewStoreMaintenanceWriter<'_, '_>,
        journal: Directory,
        backup_parent: Directory,
        limits: &EgressStoreLimitsV1,
        fresh: bool,
    ) -> Result<Self> {
        token.check()?;
        limits.validate()?;
        let (plan, plan_digest) = token.retained_plan();
        let source = token.descriptor();
        require_egress_transition(plan, source)?;
        let prior_manifest = plan
            .prior_active_manifest
            .as_ref()
            .context("egress prior manifest required")?;
        let prior_anchor = token
            .prior_active_anchor(14)?
            .context("egress prior anchor required")?;
        let source_prefix = writer.egress_maintenance_prefix(token, fresh)?;
        let journal_binding = DirectoryBinding::observe(&journal)?;
        let backup_binding = DirectoryBinding::observe(&backup_parent)?;
        ensure!(
            journal_binding.identity() != backup_binding.identity()
                && !backup_parent.path().starts_with(journal.path())
                && plan.target.sources.iter().all(|source| {
                    let parent = source.storage_binding.parent_identity();
                    parent != journal_binding.identity()
                        && parent != backup_binding.identity()
                        && !source
                            .storage_binding
                            .parent_path
                            .starts_with(journal.path())
                }),
            "egress journal, backup or source parent overlaps"
        );
        let intent = IntentV1 {
            version: 1,
            migration_id: token.migration_id(),
            plan_digest,
            prior_manifest,
            target_closure_revision: prior_manifest
                .closure_revision
                .checked_add(1)
                .filter(|revision| i64::try_from(*revision).is_ok())
                .context("egress closure revision exhausted")?,
            source,
            prior_anchor,
            source_prefix: &source_prefix,
            limits,
            journal: journal_binding,
            backup_parent: backup_binding,
            backup_name: BACKUP_NAME,
        };
        let bytes = serde_json::to_vec(&intent)?;
        ensure!(
            u64::try_from(bytes.len())? <= MAX_RECORD_BYTES,
            "egress intent exceeds bound"
        );
        token.check()?;

        let file = if fresh {
            ensure!(
                journal_stage(&journal)? == JournalStage::Empty,
                "egress journal is occupied"
            );
            ensure!(
                sqlite_entries(&backup_parent, BACKUP_NAME)?.is_empty(),
                "egress backup namespace is occupied"
            );
            journal.persistent_entries(MAX_JOURNAL_ENTRIES - CONFIRMATION_SLOTS - 2)?;
            publish_record(token, &journal, &backup_parent, INTENT_NAME, &bytes)?
        } else {
            ensure!(
                journal_stage(&journal)? != JournalStage::Empty,
                "egress intent missing"
            );
            read_exact_record(
                token,
                &journal,
                &backup_parent,
                INTENT_NAME,
                &bytes,
                "egress intent differs",
            )?
        };
        let retained = Self {
            token,
            journal,
            backup_parent,
            file,
            source_prefix,
            limits: limits.clone(),
        };
        retained.revalidate()?;
        Ok(retained)
    }

    pub(crate) fn revalidate(&self) -> Result<()> {
        self.token.check()?;
        ensure!(
            journal_stage(&self.journal)? != JournalStage::Empty,
            "egress intent missing"
        );
        self.file.persistent_revalidate()?;
        self.backup_parent.revalidate()?;
        self.token.check()?;
        Ok(())
    }

    pub(crate) fn backup_path(&self) -> Result<PathBuf> {
        self.revalidate()?;
        Ok(self.backup_parent.path().join(BACKUP_NAME))
    }

    fn revalidate_for_install(&self, writer: &ReviewStoreMaintenanceWriter<'_, '_>) -> Result<()> {
        self.revalidate()?;
        ensure!(
            writer.egress_maintenance_prefix(self.token, true)? == self.source_prefix,
            "egress source prefix differs from intent"
        );
        Ok(())
    }

    /// Prepare only after verifying the intent and the absence of a backup.
    pub(crate) fn prepare_backup(
        &self,
        writer: &ReviewStoreMaintenanceWriter<'_, '_>,
    ) -> Result<RetainedEgressBackup<'_, 'a>> {
        self.revalidate_for_install(writer)?;
        ensure!(
            journal_stage(&self.journal)? == JournalStage::Intent,
            "egress backup preparation already started"
        );
        ensure!(
            sqlite_entries(&self.backup_parent, BACKUP_NAME)?.is_empty(),
            "egress backup namespace is occupied"
        );
        self.journal
            .persistent_entries(MAX_JOURNAL_ENTRIES - CONFIRMATION_SLOTS - 1)?;
        let proof = writer.prepare_egress_backup(self.token, &self.backup_path()?)?;
        let bytes = self.backup_record(&proof)?;
        self.revalidate_for_install(writer)?;
        let frame = publish_record(
            self.token,
            &self.journal,
            &self.backup_parent,
            BACKUP_RECORD_NAME,
            &bytes,
        )?;
        let retained = RetainedEgressBackup {
            intent: self,
            frame,
            proof,
        };
        retained.revalidate_frame()?;
        Ok(retained)
    }

    /// Re-check an existing backup via SQLite and retain the matching record.
    pub(crate) fn resume_backup(
        &self,
        writer: &ReviewStoreMaintenanceWriter<'_, '_>,
    ) -> Result<RetainedEgressBackup<'_, 'a>> {
        self.revalidate()?;
        ensure!(
            matches!(
                journal_stage(&self.journal)?,
                JournalStage::Backup | JournalStage::Confirmed
            ),
            "egress backup record missing"
        );
        let proof = writer.resume_egress_backup(self.token, &self.backup_path()?)?;
        let frame = self.reopen_backup_record(&proof)?;
        let retained = RetainedEgressBackup {
            intent: self,
            frame,
            proof,
        };
        retained.revalidate_frame()?;
        Ok(retained)
    }

    /// Reopen the committed source and persist or re-check its exact receipt.
    pub(crate) fn confirm_committed(
        &self,
        redactor: Redactor,
        max_events: u64,
    ) -> Result<AuthorityReceiptV1> {
        self.revalidate()?;
        ensure!(
            matches!(
                journal_stage(&self.journal)?,
                JournalStage::Backup | JournalStage::Confirmed
            ),
            "egress backup record missing"
        );
        Store::confirm_egress_format15_with(
            self.token,
            redactor,
            max_events,
            &self.backup_path()?,
            &self.limits,
            |proof, receipt| {
                let backup = self.reopen_backup_record(proof)?;
                ensure!(
                    self.source_prefix.seq.checked_add(1) == Some(receipt.seq)
                        && super::super::manifest::valid_digest(&receipt.hash),
                    "egress confirmation does not follow its prefix"
                );
                let bytes = serde_json::to_vec(&ConfirmedV1 {
                    version: 1,
                    intent_digest: self.file.digest(),
                    backup_digest: backup.digest(),
                    receipt: &receipt,
                })?;
                let confirmation = match journal_stage(&self.journal)? {
                    JournalStage::Backup => publish_record(
                        self.token,
                        &self.journal,
                        &self.backup_parent,
                        CONFIRMED_NAME,
                        &bytes,
                    )?,
                    JournalStage::Confirmed => read_exact_record(
                        self.token,
                        &self.journal,
                        &self.backup_parent,
                        CONFIRMED_NAME,
                        &bytes,
                        "egress confirmation record differs",
                    )?,
                    JournalStage::Empty | JournalStage::Intent => {
                        anyhow::bail!("egress backup record missing")
                    }
                };
                self.revalidate()?;
                backup.persistent_revalidate()?;
                ensure!(
                    backup.bytes() == self.backup_record(proof)?,
                    "egress backup record differs"
                );
                confirmation.persistent_revalidate()?;
                ensure!(
                    journal_stage(&self.journal)? == JournalStage::Confirmed,
                    "egress confirmation record missing"
                );
                Ok(receipt)
            },
        )
    }

    fn reopen_backup_record(&self, proof: &VerifiedEgressSourceBackup<'_>) -> Result<PrivateFile> {
        let bytes = self.backup_record(proof)?;
        read_exact_record(
            self.token,
            &self.journal,
            &self.backup_parent,
            BACKUP_RECORD_NAME,
            &bytes,
            "egress backup record differs",
        )
    }

    fn backup_record(&self, proof: &VerifiedEgressSourceBackup<'_>) -> Result<Vec<u8>> {
        let (binding, source_prefix, event_count) = proof.journal_observation(self.token)?;
        binding.validate()?;
        ensure!(
            binding.parent_path == self.backup_parent.path()
                && binding.parent_identity() == self.backup_parent.physical_identity()?
                && binding.file_name == BACKUP_NAME
                && source_prefix == self.source_prefix
                && event_count > 0,
            "egress backup does not match the intent"
        );
        let bytes = serde_json::to_vec(&BackupV1 {
            version: 1,
            intent_digest: self.file.digest(),
            binding,
            source_prefix,
            event_count,
        })?;
        ensure!(
            u64::try_from(bytes.len())? <= MAX_RECORD_BYTES,
            "egress backup record exceeds bound"
        );
        Ok(bytes)
    }
}

impl RetainedEgressBackup<'_, '_> {
    fn revalidate_frame(&self) -> Result<()> {
        self.intent.revalidate()?;
        ensure!(
            matches!(
                journal_stage(&self.intent.journal)?,
                JournalStage::Backup | JournalStage::Confirmed
            ),
            "egress backup record missing"
        );
        self.frame.persistent_revalidate()?;
        ensure!(
            self.frame.bytes() == self.intent.backup_record(&self.proof)?,
            "egress backup record differs"
        );
        Ok(())
    }

    /// Install with the original limits. The caller still commits its writer.
    pub(crate) fn install_format15(
        &self,
        writer: &mut ReviewStoreMaintenanceWriter<'_, '_>,
    ) -> Result<AuthorityReceiptV1> {
        self.intent.revalidate_for_install(writer)?;
        self.revalidate_frame()?;
        ensure!(
            journal_stage(&self.intent.journal)? == JournalStage::Backup,
            "egress installation requires its exact prepared backup"
        );
        self.intent
            .journal
            .persistent_entries(MAX_JOURNAL_ENTRIES - CONFIRMATION_SLOTS)?;
        writer.install_egress_format15(self.intent.token, &self.proof, &self.intent.limits)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JournalStage {
    Empty,
    Intent,
    Backup,
    Confirmed,
}

fn journal_stage(journal: &Directory) -> Result<JournalStage> {
    let entries = journal.persistent_entries(MAX_JOURNAL_ENTRIES)?;
    ensure!(
        entries.iter().all(|entry| entry.directory),
        "egress journal entries differ"
    );
    let mut names = Vec::new();
    for entry in &entries {
        let name = entry.name.as_str();
        if let Some(suffix) = name.strip_prefix(STAGE_PREFIX) {
            ensure!(
                Uuid::parse_str(suffix)
                    .is_ok_and(|uuid| uuid.get_version_num() == 4 && uuid.to_string() == suffix),
                "egress journal stage name differs"
            );
        } else {
            names.push(name);
        }
    }
    names.sort_unstable();
    match names.as_slice() {
        [] => Ok(JournalStage::Empty),
        [INTENT_NAME] => Ok(JournalStage::Intent),
        [BACKUP_RECORD_NAME, INTENT_NAME] => Ok(JournalStage::Backup),
        [BACKUP_RECORD_NAME, CONFIRMED_NAME, INTENT_NAME] => Ok(JournalStage::Confirmed),
        _ => anyhow::bail!("egress journal entries differ"),
    }
}

fn publish_record(
    token: &HumanMaintenanceSourceToken,
    journal: &Directory,
    backup_parent: &Directory,
    name: &str,
    bytes: &[u8],
) -> Result<PrivateFile> {
    token.check()?;
    ensure!(
        !bytes.is_empty() && u64::try_from(bytes.len())? <= MAX_RECORD_BYTES,
        "egress journal record size differs"
    );
    journal.persistent_entries(MAX_JOURNAL_ENTRIES - 1)?;
    journal.require_absent_child(name)?;
    let stage = journal.exclusive_child(&format!("{STAGE_PREFIX}{}", Uuid::new_v4()))?;
    journal.require_same_mount_child(&stage)?;
    let file = stage.persistent_create(RECORD_NAME, bytes)?;
    file.persistent_sync()?;
    stage.private_entry_same_mount(OsStr::new(RECORD_NAME))?;
    journal.require_same_mount_child(&stage)?;
    token.check()?;
    match journal.publish_child_exclusive(&stage, name) {
        Ok(()) => {}
        Err(PublicationFailure::NotPublished) => {
            anyhow::bail!("egress journal record not published")
        }
        Err(PublicationFailure::DurabilityUnknown) => {
            anyhow::bail!("egress journal publication durability unknown")
        }
    }
    drop(file);
    drop(stage);
    read_exact_record(
        token,
        journal,
        backup_parent,
        name,
        bytes,
        "egress journal record differs",
    )
}

fn read_exact_record(
    token: &HumanMaintenanceSourceToken,
    journal: &Directory,
    backup_parent: &Directory,
    name: &str,
    bytes: &[u8],
    mismatch: &'static str,
) -> Result<PrivateFile> {
    token.check()?;
    let slot = journal
        .persistent_child(name, false)?
        .context("egress journal record missing")?;
    journal.require_same_mount_child(&slot)?;
    let entries = slot.persistent_entries(1)?;
    ensure!(
        matches!(entries.as_slice(), [entry] if !entry.directory && entry.name == RECORD_NAME),
        "egress journal slot entries differ"
    );
    let identity = slot.private_entry_same_mount(OsStr::new(RECORD_NAME))?;
    let (plan, _) = token.retained_plan();
    let mut exclusions = sqlite_entries(backup_parent, BACKUP_NAME)?;
    for source in &plan.target.sources {
        let binding = &source.storage_binding;
        let parent = Directory::open_existing(&binding.parent_path)?;
        ensure!(
            parent.physical_identity()? == binding.parent_identity(),
            "egress source parent changed"
        );
        exclusions.extend(sqlite_entries(&parent, &binding.file_name)?);
    }
    let file = slot
        .persistent_read(RECORD_NAME, MAX_RECORD_BYTES, &exclusions)?
        .context("egress journal record missing")?;
    ensure!(file.bytes() == bytes, "{mismatch}");
    file.persistent_sync()?;
    slot.sync()?;
    journal.sync()?;
    journal.require_same_mount_child(&slot)?;
    ensure!(
        slot.private_entry_same_mount(OsStr::new(RECORD_NAME))? == identity,
        "egress journal record identity changed"
    );
    file.persistent_revalidate()?;
    token.check()?;
    Ok(file)
}

fn require_egress_transition(
    plan: &HumanSourcesMaintenancePlanV1,
    selected: &HumanMaintenanceSourceV1,
) -> Result<()> {
    let prior = plan.prior.as_ref().context("egress prior union required")?;
    ensure!(
        selected.kind == HumanSourceKindV1::TenantStore
            && selected.required_format == 15
            && prior.sources.len() == plan.target.sources.len(),
        "egress target is not a single in-place Store15"
    );
    let mut found = false;
    for (prior, target) in prior.sources.iter().zip(&plan.target.sources) {
        let mut expected = prior.clone();
        if expected.source_id == selected.source_id {
            ensure!(
                expected.required_format == 14,
                "egress prior source is not Store14"
            );
            expected.required_format = 15;
            found = true;
        }
        ensure!(&expected == target, "egress source union differs");
    }
    ensure!(found, "egress selected prior source is missing");
    Ok(())
}

fn sqlite_entries(parent: &Directory, file_name: &str) -> Result<Vec<PrivateEntryIdentity>> {
    let mut entries = Vec::new();
    for suffix in SQLITE_SUFFIXES {
        if let Some(identity) = parent.private_entry(OsStr::new(&format!("{file_name}{suffix}")))? {
            entries.push(identity);
        }
    }
    Ok(entries)
}
