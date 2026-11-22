//! Private E1 intention, backup preparation and committed genesis records.
//!
//! The same journal machinery records the in-place identity-marker
//! maintenance of an adopted source (formats 14 and 15) under its own record
//! names. The egress step keeps its exact record names, bytes and messages.

use crate::{
    audit::{
        AuthorityReceiptV1, ReviewStoreMaintenanceWriter, Store,
        egress::{EgressStoreLimitsV1, VerifiedEgressSourceBackup},
        identity_markers::{IdentityMarkerReceiptV1, InPlaceIdentityMarkerV1},
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

/// Record names and message label of one journaled source step.
struct JournalNames {
    intent: &'static str,
    backup: &'static str,
    confirmed: &'static str,
    stage_prefix: &'static str,
    label: &'static str,
}

const EGRESS_NAMES: JournalNames = JournalNames {
    intent: "egress15-intent-v2",
    backup: "egress15-backup-v2",
    confirmed: "egress15-confirmed-v2",
    stage_prefix: "egress15-stage-",
    label: "egress",
};
const IDENTITY_MARKER_NAMES: JournalNames = JournalNames {
    intent: "identity-markers-intent-v1",
    backup: "identity-markers-backup-v1",
    confirmed: "identity-markers-confirmed-v1",
    stage_prefix: "identity-markers-stage-",
    label: "identity marker",
};
const IDENTITY_MARKER_STEP: &str = "in_place_identity_markers";
const RECORD_NAME: &str = "record-v1.json";
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

// In place: the plan keeps the active closure, so there is no new revision.
#[derive(Serialize)]
struct IdentityMarkersIntentV1<'a> {
    version: u32,
    step: &'static str,
    migration_id: Uuid,
    plan_digest: &'a str,
    prior_manifest: &'a HumanMaintenanceActiveManifestV1,
    source: &'a HumanMaintenanceSourceV1,
    prior_anchor: &'a HumanAuthorityMarkerReceiptV1,
    source_prefix: &'a AuthorityReceiptV1,
    markers: &'a [InPlaceIdentityMarkerV1],
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

#[derive(Serialize)]
struct ConfirmedIdentityMarkersV1<'a> {
    version: u32,
    intent_digest: &'a str,
    backup_digest: &'a str,
    receipts: &'a [IdentityMarkerReceiptV1],
}

/// The journaled step of one selected source.
enum SourceStepV1 {
    /// Store14 to Store15 with the original egress limits.
    Egress15(EgressStoreLimitsV1),
    /// Every identity marker still pending at intent time, in order.
    IdentityMarkers(Vec<InPlaceIdentityMarkerV1>),
}

impl SourceStepV1 {
    fn names(&self) -> &'static JournalNames {
        match self {
            Self::Egress15(_) => &EGRESS_NAMES,
            Self::IdentityMarkers(_) => &IDENTITY_MARKER_NAMES,
        }
    }
}

enum StepRequest<'l> {
    // The egress operator producer (G05) is not wired yet; only the
    // fixtures construct it. The step itself is unchanged.
    #[cfg_attr(not(test), allow(dead_code))]
    Egress15(&'l EgressStoreLimitsV1),
    IdentityMarkers,
}

pub(crate) struct RetainedSourceIntent<'a> {
    token: &'a HumanMaintenanceSourceToken,
    journal: Directory,
    backup_parent: Directory,
    file: PrivateFile,
    source_prefix: AuthorityReceiptV1,
    step: SourceStepV1,
}
/// Egress callers keep their names; the egress step is unchanged.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) type RetainedEgressIntent<'a> = RetainedSourceIntent<'a>;

pub(crate) struct RetainedSourceBackup<'intent, 'token> {
    intent: &'intent RetainedSourceIntent<'token>,
    frame: PrivateFile,
    proof: VerifiedEgressSourceBackup<'token>,
}
// Kept for the egress operator producer (G05); no caller names it yet.
#[allow(dead_code)]
pub(crate) type RetainedEgressBackup<'intent, 'token> = RetainedSourceBackup<'intent, 'token>;

impl<'a> RetainedSourceIntent<'a> {
    /// First attempt, with no published record and an absent backup.
    pub(crate) fn prepare(
        token: &'a HumanMaintenanceSourceToken,
        writer: &ReviewStoreMaintenanceWriter<'_, '_>,
        journal: Directory,
        backup_parent: Directory,
        limits: &EgressStoreLimitsV1,
    ) -> Result<Self> {
        Self::enter(
            token,
            writer,
            journal,
            backup_parent,
            StepRequest::Egress15(limits),
            true,
        )
    }

    /// Reopen a complete intent after loss of acknowledgement.
    pub(crate) fn resume(
        token: &'a HumanMaintenanceSourceToken,
        writer: &ReviewStoreMaintenanceWriter<'_, '_>,
        journal: Directory,
        backup_parent: Directory,
        limits: &EgressStoreLimitsV1,
    ) -> Result<Self> {
        Self::enter(
            token,
            writer,
            journal,
            backup_parent,
            StepRequest::Egress15(limits),
            false,
        )
    }

    /// Use only published identity-marker records to choose the phase. An
    /// unpublished draft survives for inspection and supplies no intent bytes.
    pub(crate) fn enter_identity_markers(
        token: &'a HumanMaintenanceSourceToken,
        writer: &ReviewStoreMaintenanceWriter<'_, '_>,
        journal: Directory,
        backup_parent: Directory,
    ) -> Result<Self> {
        let fresh = journal_stage(&journal, &IDENTITY_MARKER_NAMES)? == JournalStage::Empty;
        Self::enter(
            token,
            writer,
            journal,
            backup_parent,
            StepRequest::IdentityMarkers,
            fresh,
        )
    }

    /// First in-place identity-marker attempt on an adopted source, with an
    /// empty journal and backup namespace. Refused when nothing is pending.
    #[cfg(test)]
    pub(crate) fn prepare_identity_markers(
        token: &'a HumanMaintenanceSourceToken,
        writer: &ReviewStoreMaintenanceWriter<'_, '_>,
        journal: Directory,
        backup_parent: Directory,
    ) -> Result<Self> {
        Self::enter(
            token,
            writer,
            journal,
            backup_parent,
            StepRequest::IdentityMarkers,
            true,
        )
    }

    /// Reopen a complete in-place intent after loss of acknowledgement.
    #[cfg(test)]
    pub(crate) fn resume_identity_markers(
        token: &'a HumanMaintenanceSourceToken,
        writer: &ReviewStoreMaintenanceWriter<'_, '_>,
        journal: Directory,
        backup_parent: Directory,
    ) -> Result<Self> {
        Self::enter(
            token,
            writer,
            journal,
            backup_parent,
            StepRequest::IdentityMarkers,
            false,
        )
    }

    fn enter(
        token: &'a HumanMaintenanceSourceToken,
        writer: &ReviewStoreMaintenanceWriter<'_, '_>,
        journal: Directory,
        backup_parent: Directory,
        request: StepRequest<'_>,
        fresh: bool,
    ) -> Result<Self> {
        token.check()?;
        let (plan, plan_digest) = token.retained_plan();
        let source = token.descriptor();
        let (step, prior_manifest, prior_anchor, source_prefix) = match request {
            StepRequest::Egress15(limits) => {
                limits.validate()?;
                require_egress_transition(plan, source)?;
                let prior_manifest = plan
                    .prior_active_manifest
                    .as_ref()
                    .context("egress prior manifest required")?;
                let prior_anchor = token
                    .prior_active_anchor(14)?
                    .context("egress prior anchor required")?;
                let source_prefix = writer.egress_maintenance_prefix(token, fresh)?;
                (
                    SourceStepV1::Egress15(limits.clone()),
                    prior_manifest,
                    prior_anchor,
                    source_prefix,
                )
            }
            StepRequest::IdentityMarkers => {
                require_in_place_transition(plan, source)?;
                let prior_manifest = plan
                    .prior_active_manifest
                    .as_ref()
                    .context("identity marker prior manifest required")?;
                let prior_anchor = token
                    .in_place_anchor()?
                    .context("identity marker prior anchor required")?;
                let (source_prefix, markers) =
                    writer.identity_markers_maintenance_prefix(token, fresh)?;
                (
                    SourceStepV1::IdentityMarkers(markers),
                    prior_manifest,
                    prior_anchor,
                    source_prefix,
                )
            }
        };
        let names = step.names();
        let label = names.label;
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
            "{label} journal, backup or source parent overlaps"
        );
        let bytes = match &step {
            SourceStepV1::Egress15(limits) => serde_json::to_vec(&IntentV1 {
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
            })?,
            SourceStepV1::IdentityMarkers(markers) => {
                serde_json::to_vec(&IdentityMarkersIntentV1 {
                    version: 1,
                    step: IDENTITY_MARKER_STEP,
                    migration_id: token.migration_id(),
                    plan_digest,
                    prior_manifest,
                    source,
                    prior_anchor,
                    source_prefix: &source_prefix,
                    markers,
                    journal: journal_binding,
                    backup_parent: backup_binding,
                    backup_name: BACKUP_NAME,
                })?
            }
        };
        ensure!(
            u64::try_from(bytes.len())? <= MAX_RECORD_BYTES,
            "{label} intent exceeds bound"
        );
        token.check()?;

        let file = if fresh {
            ensure!(
                journal_stage(&journal, names)? == JournalStage::Empty,
                "{label} journal is occupied"
            );
            ensure!(
                sqlite_entries(&backup_parent, BACKUP_NAME)?.is_empty(),
                "{label} backup namespace is occupied"
            );
            journal.persistent_entries(MAX_JOURNAL_ENTRIES - CONFIRMATION_SLOTS - 2)?;
            publish_record(token, &journal, &backup_parent, names, names.intent, &bytes)?
        } else {
            ensure!(
                journal_stage(&journal, names)? != JournalStage::Empty,
                "{label} intent missing"
            );
            read_exact_record(
                token,
                &journal,
                &backup_parent,
                names,
                names.intent,
                &bytes,
                &format!("{label} intent differs"),
            )?
        };
        let retained = Self {
            token,
            journal,
            backup_parent,
            file,
            source_prefix,
            step,
        };
        retained.revalidate()?;
        Ok(retained)
    }

    fn names(&self) -> &'static JournalNames {
        self.step.names()
    }

    pub(crate) fn revalidate(&self) -> Result<()> {
        self.token.check()?;
        ensure!(
            journal_stage(&self.journal, self.names())? != JournalStage::Empty,
            "{} intent missing",
            self.names().label
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

    /// Markers recorded by an in-place intent; empty for egress.
    #[cfg(test)]
    pub(crate) fn identity_markers(&self) -> &[InPlaceIdentityMarkerV1] {
        match &self.step {
            SourceStepV1::IdentityMarkers(markers) => markers,
            SourceStepV1::Egress15(_) => &[],
        }
    }

    /// Whether this intent's backup record exists (backup or confirmed stage).
    pub(crate) fn backup_recorded(&self) -> Result<bool> {
        self.revalidate()?;
        Ok(matches!(
            journal_stage(&self.journal, self.names())?,
            JournalStage::Backup | JournalStage::Confirmed
        ))
    }

    fn revalidate_for_install(&self, writer: &ReviewStoreMaintenanceWriter<'_, '_>) -> Result<()> {
        self.revalidate()?;
        let unchanged = match &self.step {
            SourceStepV1::Egress15(_) => {
                writer.egress_maintenance_prefix(self.token, true)? == self.source_prefix
            }
            SourceStepV1::IdentityMarkers(markers) => {
                let (prefix, pending) =
                    writer.identity_markers_maintenance_prefix(self.token, true)?;
                prefix == self.source_prefix && pending == *markers
            }
        };
        ensure!(
            unchanged,
            "{} source prefix differs from intent",
            self.names().label
        );
        Ok(())
    }

    /// Prepare only after verifying the intent and the absence of a backup.
    pub(crate) fn prepare_backup(
        &self,
        writer: &ReviewStoreMaintenanceWriter<'_, '_>,
    ) -> Result<RetainedSourceBackup<'_, 'a>> {
        let names = self.names();
        self.revalidate_for_install(writer)?;
        ensure!(
            journal_stage(&self.journal, names)? == JournalStage::Intent,
            "{} backup preparation already started",
            names.label
        );
        ensure!(
            sqlite_entries(&self.backup_parent, BACKUP_NAME)?.is_empty(),
            "{} backup namespace is occupied",
            names.label
        );
        self.journal
            .persistent_entries(MAX_JOURNAL_ENTRIES - CONFIRMATION_SLOTS - 1)?;
        let proof = match &self.step {
            SourceStepV1::Egress15(_) => {
                writer.prepare_egress_backup(self.token, &self.backup_path()?)?
            }
            SourceStepV1::IdentityMarkers(_) => {
                writer.prepare_identity_markers_backup(self.token, &self.backup_path()?)?
            }
        };
        let bytes = self.backup_record(&proof)?;
        self.revalidate_for_install(writer)?;
        let frame = publish_record(
            self.token,
            &self.journal,
            &self.backup_parent,
            names,
            names.backup,
            &bytes,
        )?;
        let retained = RetainedSourceBackup {
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
    ) -> Result<RetainedSourceBackup<'_, 'a>> {
        self.revalidate()?;
        ensure!(
            matches!(
                journal_stage(&self.journal, self.names())?,
                JournalStage::Backup | JournalStage::Confirmed
            ),
            "{} backup record missing",
            self.names().label
        );
        let proof = match &self.step {
            SourceStepV1::Egress15(_) => {
                writer.resume_egress_backup(self.token, &self.backup_path()?)?
            }
            SourceStepV1::IdentityMarkers(_) => {
                writer.resume_identity_markers_backup(self.token, &self.backup_path()?)?
            }
        };
        let frame = self.reopen_backup_record(&proof)?;
        let retained = RetainedSourceBackup {
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
        let SourceStepV1::Egress15(limits) = &self.step else {
            anyhow::bail!("egress confirmation requires an egress intent");
        };
        self.require_backup_recorded()?;
        Store::confirm_egress_format15_with(
            self.token,
            redactor,
            max_events,
            &self.backup_path()?,
            limits,
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
                self.record_confirmation(proof, &backup, &bytes)?;
                Ok(receipt)
            },
        )
    }

    /// Reopen the committed source, replay every marker of this intent and
    /// persist or re-check the confirmation listing all their receipts.
    pub(crate) fn confirm_identity_markers(
        &self,
        redactor: Redactor,
        max_events: u64,
    ) -> Result<Vec<IdentityMarkerReceiptV1>> {
        let SourceStepV1::IdentityMarkers(markers) = &self.step else {
            anyhow::bail!("identity marker confirmation requires an identity marker intent");
        };
        self.require_backup_recorded()?;
        Store::confirm_identity_markers_with(
            self.token,
            redactor,
            max_events,
            &self.backup_path()?,
            markers,
            |proof, receipts| {
                let backup = self.reopen_backup_record(proof)?;
                let mut previous = self.source_prefix.seq;
                for receipt in &receipts {
                    ensure!(
                        previous.checked_add(1) == Some(receipt.seq)
                            && super::super::manifest::valid_digest(&receipt.hash),
                        "identity marker confirmation does not follow its prefix"
                    );
                    previous = receipt.seq;
                }
                ensure!(
                    receipts
                        .iter()
                        .map(|r| r.marker)
                        .eq(markers.iter().copied()),
                    "identity marker confirmation differs from its intent"
                );
                let bytes = serde_json::to_vec(&ConfirmedIdentityMarkersV1 {
                    version: 1,
                    intent_digest: self.file.digest(),
                    backup_digest: backup.digest(),
                    receipts: &receipts,
                })?;
                self.record_confirmation(proof, &backup, &bytes)?;
                Ok(receipts)
            },
        )
    }

    fn require_backup_recorded(&self) -> Result<()> {
        self.revalidate()?;
        ensure!(
            matches!(
                journal_stage(&self.journal, self.names())?,
                JournalStage::Backup | JournalStage::Confirmed
            ),
            "{} backup record missing",
            self.names().label
        );
        Ok(())
    }

    fn record_confirmation(
        &self,
        proof: &VerifiedEgressSourceBackup<'_>,
        backup: &PrivateFile,
        bytes: &[u8],
    ) -> Result<()> {
        let names = self.names();
        let label = names.label;
        let confirmation = match journal_stage(&self.journal, names)? {
            JournalStage::Backup => publish_record(
                self.token,
                &self.journal,
                &self.backup_parent,
                names,
                names.confirmed,
                bytes,
            )?,
            JournalStage::Confirmed => read_exact_record(
                self.token,
                &self.journal,
                &self.backup_parent,
                names,
                names.confirmed,
                bytes,
                &format!("{label} confirmation record differs"),
            )?,
            JournalStage::Empty | JournalStage::Intent => {
                anyhow::bail!("{label} backup record missing")
            }
        };
        self.revalidate()?;
        backup.persistent_revalidate()?;
        ensure!(
            backup.bytes() == self.backup_record(proof)?,
            "{label} backup record differs"
        );
        confirmation.persistent_revalidate()?;
        ensure!(
            journal_stage(&self.journal, names)? == JournalStage::Confirmed,
            "{label} confirmation record missing"
        );
        Ok(())
    }

    fn reopen_backup_record(&self, proof: &VerifiedEgressSourceBackup<'_>) -> Result<PrivateFile> {
        let names = self.names();
        let bytes = self.backup_record(proof)?;
        read_exact_record(
            self.token,
            &self.journal,
            &self.backup_parent,
            names,
            names.backup,
            &bytes,
            &format!("{} backup record differs", names.label),
        )
    }

    fn backup_record(&self, proof: &VerifiedEgressSourceBackup<'_>) -> Result<Vec<u8>> {
        let label = self.names().label;
        let (binding, source_prefix, event_count) = proof.journal_observation(self.token)?;
        binding.validate()?;
        ensure!(
            binding.parent_path == self.backup_parent.path()
                && binding.parent_identity() == self.backup_parent.physical_identity()?
                && binding.file_name == BACKUP_NAME
                && source_prefix == self.source_prefix
                && event_count > 0,
            "{label} backup does not match the intent"
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
            "{label} backup record exceeds bound"
        );
        Ok(bytes)
    }
}

impl RetainedSourceBackup<'_, '_> {
    fn revalidate_frame(&self) -> Result<()> {
        let label = self.intent.names().label;
        self.intent.revalidate()?;
        ensure!(
            matches!(
                journal_stage(&self.intent.journal, self.intent.names())?,
                JournalStage::Backup | JournalStage::Confirmed
            ),
            "{label} backup record missing"
        );
        self.frame.persistent_revalidate()?;
        ensure!(
            self.frame.bytes() == self.intent.backup_record(&self.proof)?,
            "{label} backup record differs"
        );
        Ok(())
    }

    fn require_prepared(&self, writer: &ReviewStoreMaintenanceWriter<'_, '_>) -> Result<()> {
        self.intent.revalidate_for_install(writer)?;
        self.revalidate_frame()?;
        ensure!(
            journal_stage(&self.intent.journal, self.intent.names())? == JournalStage::Backup,
            "{} installation requires its exact prepared backup",
            self.intent.names().label
        );
        self.intent
            .journal
            .persistent_entries(MAX_JOURNAL_ENTRIES - CONFIRMATION_SLOTS)?;
        Ok(())
    }

    /// Install with the original limits. The caller still commits its writer.
    pub(crate) fn install_format15(
        &self,
        writer: &mut ReviewStoreMaintenanceWriter<'_, '_>,
    ) -> Result<AuthorityReceiptV1> {
        let SourceStepV1::Egress15(limits) = &self.intent.step else {
            anyhow::bail!("egress installation requires an egress intent");
        };
        self.require_prepared(writer)?;
        writer.install_egress_format15(self.intent.token, &self.proof, limits)
    }

    /// Append every marker of the intent in the writer's one transaction,
    /// after its exact verified backup. The caller still commits its writer.
    pub(crate) fn install_identity_markers(
        &self,
        writer: &mut ReviewStoreMaintenanceWriter<'_, '_>,
    ) -> Result<Vec<IdentityMarkerReceiptV1>> {
        let SourceStepV1::IdentityMarkers(markers) = &self.intent.step else {
            anyhow::bail!("identity marker installation requires an identity marker intent");
        };
        self.require_prepared(writer)?;
        writer.install_identity_markers(self.intent.token, &self.proof, markers)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JournalStage {
    Empty,
    Intent,
    Backup,
    Confirmed,
}

fn journal_stage(journal: &Directory, names: &JournalNames) -> Result<JournalStage> {
    let label = names.label;
    let entries = journal.persistent_entries(MAX_JOURNAL_ENTRIES)?;
    ensure!(
        entries.iter().all(|entry| entry.directory),
        "{label} journal entries differ"
    );
    let mut found = Vec::new();
    for entry in &entries {
        let name = entry.name.as_str();
        if let Some(suffix) = name.strip_prefix(names.stage_prefix) {
            ensure!(
                Uuid::parse_str(suffix)
                    .is_ok_and(|uuid| uuid.get_version_num() == 4 && uuid.to_string() == suffix),
                "{label} journal stage name differs"
            );
        } else {
            found.push(name);
        }
    }
    let has = |name: &str| found.contains(&name);
    match found.len() {
        0 => Ok(JournalStage::Empty),
        1 if has(names.intent) => Ok(JournalStage::Intent),
        2 if has(names.intent) && has(names.backup) => Ok(JournalStage::Backup),
        3 if has(names.intent) && has(names.backup) && has(names.confirmed) => {
            Ok(JournalStage::Confirmed)
        }
        _ => anyhow::bail!("{label} journal entries differ"),
    }
}

fn publish_record(
    token: &HumanMaintenanceSourceToken,
    journal: &Directory,
    backup_parent: &Directory,
    names: &JournalNames,
    name: &str,
    bytes: &[u8],
) -> Result<PrivateFile> {
    let label = names.label;
    token.check()?;
    ensure!(
        !bytes.is_empty() && u64::try_from(bytes.len())? <= MAX_RECORD_BYTES,
        "{label} journal record size differs"
    );
    journal.persistent_entries(MAX_JOURNAL_ENTRIES - 1)?;
    journal.require_absent_child(name)?;
    let stage = journal.exclusive_child(&format!("{}{}", names.stage_prefix, Uuid::new_v4()))?;
    journal.require_same_mount_child(&stage)?;
    let file = stage.persistent_create(RECORD_NAME, bytes)?;
    file.persistent_sync()?;
    stage.private_entry_same_mount(OsStr::new(RECORD_NAME))?;
    journal.require_same_mount_child(&stage)?;
    token.check()?;
    match journal.publish_child_exclusive(&stage, name) {
        Ok(()) => {}
        Err(PublicationFailure::NotPublished) => {
            anyhow::bail!("{label} journal record not published")
        }
        Err(PublicationFailure::DurabilityUnknown) => {
            anyhow::bail!("{label} journal publication durability unknown")
        }
    }
    drop(file);
    drop(stage);
    read_exact_record(
        token,
        journal,
        backup_parent,
        names,
        name,
        bytes,
        &format!("{label} journal record differs"),
    )
}

fn read_exact_record(
    token: &HumanMaintenanceSourceToken,
    journal: &Directory,
    backup_parent: &Directory,
    names: &JournalNames,
    name: &str,
    bytes: &[u8],
    mismatch: &str,
) -> Result<PrivateFile> {
    let label = names.label;
    token.check()?;
    let slot = journal
        .persistent_child(name, false)?
        .with_context(|| format!("{label} journal record missing"))?;
    journal.require_same_mount_child(&slot)?;
    let entries = slot.persistent_entries(1)?;
    ensure!(
        matches!(entries.as_slice(), [entry] if !entry.directory && entry.name == RECORD_NAME),
        "{label} journal slot entries differ"
    );
    let identity = slot.private_entry_same_mount(OsStr::new(RECORD_NAME))?;
    let (plan, _) = token.retained_plan();
    let mut exclusions = sqlite_entries(backup_parent, BACKUP_NAME)?;
    for source in &plan.target.sources {
        let binding = &source.storage_binding;
        let parent = Directory::open_existing(&binding.parent_path)?;
        ensure!(
            parent.physical_identity()? == binding.parent_identity(),
            "{label} source parent changed"
        );
        exclusions.extend(sqlite_entries(&parent, &binding.file_name)?);
    }
    let file = slot
        .persistent_read(RECORD_NAME, MAX_RECORD_BYTES, &exclusions)?
        .with_context(|| format!("{label} journal record missing"))?;
    ensure!(file.bytes() == bytes, "{mismatch}");
    file.persistent_sync()?;
    slot.sync()?;
    journal.sync()?;
    journal.require_same_mount_child(&slot)?;
    ensure!(
        slot.private_entry_same_mount(OsStr::new(RECORD_NAME))? == identity,
        "{label} journal record identity changed"
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

/// In place: the whole union, formats and configuration stay unchanged and
/// the prior active manifest is retained. Only tenant Store14/15 qualify.
fn require_in_place_transition(
    plan: &HumanSourcesMaintenancePlanV1,
    selected: &HumanMaintenanceSourceV1,
) -> Result<()> {
    ensure!(
        plan.prior.as_ref() == Some(&plan.target)
            && plan.prior_active_manifest.is_some()
            && selected.kind == HumanSourceKindV1::TenantStore
            && matches!(selected.required_format, 14 | 15),
        "identity marker target is not an unchanged adopted Store14 or Store15"
    );
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
