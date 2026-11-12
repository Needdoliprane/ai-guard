//! Bounded immutable private progress frames. Missing/truncated/divergent
//! frames refuse; no existing journal or backup is repaired or overwritten.
use super::sources::SourceBackup;
use super::*;
use crate::human_authority::{
    SourceStorageBindingV1,
    maintenance::{HumanMaintenanceSourceV1, HumanMaintenanceViewV1},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;

const MAX_FRAME_BYTES: u64 = 1_048_576;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MigrationIntent {
    version: u32,
    migration_id: Uuid,
    plan_digest: String,
    installation_id: Uuid,
    pub closure_id: Uuid,
    pub closure_revision: u64,
    target: HumanMaintenanceViewV1,
    backup_paths: BTreeMap<Uuid, PathBuf>,
    source_profiles: BTreeMap<Uuid, SourceProfile>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SourceProfile {
    TenantStore { max_events: u64 },
    AuthenticationLedger { policy: AuthenticationAuditPolicyV1 },
}
impl MigrationIntent {
    pub(super) fn new(
        request: &HumanSourcesMigrationRequest,
        plan: &HumanSourcesMaintenancePlanV1,
    ) -> Result<Self> {
        request.plan_file.persistent_revalidate()?;
        Ok(Self {
            version: 1,
            migration_id: plan.migration_id,
            plan_digest: request.plan_file.digest().to_owned(),
            installation_id: plan.installation_id,
            closure_id: request.closure_id,
            closure_revision: request.closure_revision,
            target: plan.target.clone(),
            backup_paths: request.backup_paths.clone(),
            source_profiles: request
                .sources
                .iter()
                .map(|(id, options)| {
                    (
                        *id,
                        match options {
                            HumanMigrationSourceOptions::TenantStore { max_events, .. } => {
                                SourceProfile::TenantStore {
                                    max_events: *max_events,
                                }
                            }
                            HumanMigrationSourceOptions::AuthenticationLedger {
                                policy, ..
                            } => SourceProfile::AuthenticationLedger {
                                policy: policy.clone(),
                            },
                        },
                    )
                })
                .collect(),
        })
    }
    fn frame_name(&self, ordinal: usize) -> String {
        format!("migration-{}-{ordinal:04}.json", self.migration_id)
    }
    fn manifest_name(&self) -> String {
        format!("migration-{}-closure.json", self.migration_id)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BackupRecord {
    source: HumanMaintenanceSourceV1,
    backup_binding: SourceStorageBindingV1,
    pub prefix: HumanAuthorityMarkerReceiptV1,
    records: u64,
    serialized_bytes: Option<u64>,
}
impl BackupRecord {
    pub(super) fn from_verified(
        token: &HumanMaintenanceSourceToken,
        backup: &SourceBackup<'_>,
    ) -> Result<Self> {
        token.check()?;
        let (migration_id, (path, identity), prefix, records, serialized_bytes) = match backup {
            SourceBackup::Store { proof, .. } => (
                proof.migration_id(),
                proof.backup_binding(),
                HumanAuthorityMarkerReceiptV1 {
                    seq: proof.prefix().0,
                    hash: proof.prefix().1.clone(),
                },
                proof.event_count(),
                None,
            ),
            SourceBackup::Authentication { proof } => {
                ensure!(
                    proof.prefix().namespace_id == token.descriptor().durable_namespace_id,
                    "backup namespace differs"
                );
                (
                    proof.migration_id(),
                    proof.backup_binding(),
                    HumanAuthorityMarkerReceiptV1 {
                        seq: i64::try_from(proof.prefix().sequence)?,
                        hash: proof.prefix().hash.clone(),
                    },
                    proof.verification().records,
                    Some(proof.verification().serialized_bytes),
                )
            }
        };
        ensure!(
            migration_id == token.migration_id()
                && prefix.seq > 0
                && super::super::manifest::valid_digest(&prefix.hash),
            "backup migration differs"
        );
        let parent_path = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("backup parent absent"))?
            .to_path_buf();
        let parent = Directory::open_existing(&parent_path)?;
        let file_name = path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| anyhow::anyhow!("backup name invalid"))?
            .to_owned();
        ensure!(
            parent.private_entry(OsStr::new(&file_name))? == Some(identity),
            "backup identity differs"
        );
        let (parent_device, parent_inode) = parent.physical_identity()?;
        let (source_device, source_inode) = identity.physical_identity();
        let backup_binding = SourceStorageBindingV1 {
            parent_path,
            file_name,
            parent_device,
            parent_inode,
            source_device,
            source_inode,
        };
        backup_binding.validate()?;
        token.check()?;
        Ok(Self {
            source: token.descriptor().clone(),
            backup_binding,
            prefix,
            records,
            serialized_bytes,
        })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Progress {
    intent: MigrationIntent,
    backups: Option<Vec<BackupRecord>>,
    confirmed: Vec<HumanAuthorityMarkerReceiptV1>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    ordinal: usize,
    previous_digest: Option<String>,
    progress: Progress,
}

pub(super) struct Journal {
    directory: Directory,
    progress: Progress,
    frames: Vec<PrivateFile>,
}
impl Journal {
    pub(super) fn load(directory: Directory, intent: MigrationIntent) -> Result<Self> {
        directory.revalidate()?;
        let mut journal = Self {
            directory,
            progress: Progress {
                intent,
                backups: None,
                confirmed: Vec::new(),
            },
            frames: Vec::new(),
        };
        let mut absent = false;
        // Intent + prepared + one confirmation per source. Scan the entire
        // bounded namespace so a gap never hides later progress.
        for ordinal in 0..journal.progress.intent.target.sources.len() + 3 {
            let name = journal.progress.intent.frame_name(ordinal);
            if journal
                .directory
                .private_entry(OsStr::new(&name))?
                .is_none()
            {
                absent = true;
                continue;
            }
            ensure!(!absent, "journal sequence has a gap");
            let file = journal
                .directory
                .read_private_file(&name, MAX_FRAME_BYTES)?;
            file.persistent_revalidate()?;
            let frame: Frame = serde_json::from_slice(file.bytes())?;
            ensure!(
                frame.ordinal == ordinal
                    && frame.previous_digest.as_deref()
                        == journal.frames.last().map(PrivateFile::digest)
                    && frame.progress.intent == journal.progress.intent,
                "journal binding differs"
            );
            if ordinal == 0 {
                ensure!(
                    frame.progress.backups.is_none() && frame.progress.confirmed.is_empty(),
                    "journal intent contains progress"
                );
            } else if ordinal == 1 {
                ensure!(
                    frame
                        .progress
                        .backups
                        .as_ref()
                        .is_some_and(|b| b.len() == journal.progress.intent.target.sources.len())
                        && frame.progress.confirmed.is_empty(),
                    "journal preparation differs"
                );
                validate_backups(&frame.progress)?;
            } else {
                ensure!(
                    frame.progress.backups == journal.progress.backups
                        && frame.progress.confirmed.len() == journal.progress.confirmed.len() + 1
                        && frame.progress.confirmed[..journal.progress.confirmed.len()]
                            == journal.progress.confirmed,
                    "journal confirmation differs"
                );
                validate_confirmed(&frame.progress)?;
            }
            journal.progress = frame.progress;
            journal.frames.push(file);
        }
        journal.revalidate()?;
        // A preceding attempt may have written a complete frame and then
        // lost its synchronization acknowledgement. Re-sync only validated,
        // unchanged bytes before using that progress for a new conversion.
        for file in &journal.frames {
            file.persistent_sync()?;
        }
        journal.directory.sync()?;
        journal.revalidate()?;
        Ok(journal)
    }
    pub(super) fn has_intent(&self) -> bool {
        !self.frames.is_empty()
    }
    pub(super) fn ensure_intent(&mut self) -> Result<()> {
        if self.frames.is_empty() {
            self.persist(self.progress.clone())?;
        }
        self.revalidate()
    }
    pub(super) fn prepared(&self) -> bool {
        self.progress.backups.is_some()
    }
    pub(super) fn confirmed(&self, index: usize) -> Option<&HumanAuthorityMarkerReceiptV1> {
        self.progress.confirmed.get(index)
    }
    pub(super) fn confirmed_count(&self) -> usize {
        self.progress.confirmed.len()
    }
    pub(super) fn backups(&self) -> &[BackupRecord] {
        self.progress.backups.as_deref().unwrap_or(&[])
    }
    pub(super) fn prepare(&mut self, backups: Vec<BackupRecord>) -> Result<()> {
        self.revalidate()?;
        if let Some(old) = &self.progress.backups {
            ensure!(*old == backups, "retained backups differ");
            return Ok(());
        }
        let mut next = self.progress.clone();
        next.backups = Some(backups);
        validate_backups(&next)?;
        self.persist(next)
    }
    pub(super) fn confirm(
        &mut self,
        index: usize,
        receipt: HumanAuthorityMarkerReceiptV1,
    ) -> Result<()> {
        self.revalidate()?;
        if let Some(old) = self.progress.confirmed.get(index) {
            ensure!(*old == receipt, "committed receipt differs");
            return Ok(());
        }
        ensure!(
            index == self.progress.confirmed.len() && self.progress.backups.is_some(),
            "confirmation order differs"
        );
        let mut next = self.progress.clone();
        next.confirmed.push(receipt);
        validate_confirmed(&next)?;
        self.persist(next)
    }
    fn persist(&mut self, progress: Progress) -> Result<()> {
        self.revalidate()?;
        let ordinal = self.frames.len();
        ensure!(
            ordinal <= progress.intent.target.sources.len() + 1,
            "journal bound exceeded"
        );
        let frame = Frame {
            ordinal,
            previous_digest: self.frames.last().map(|f| f.digest().to_owned()),
            progress: progress.clone(),
        };
        let bytes = serde_json::to_vec(&frame)?;
        ensure!(
            bytes.len() as u64 <= MAX_FRAME_BYTES,
            "journal frame too large"
        );
        let file = create_document(
            &self.directory,
            &progress.intent.frame_name(ordinal),
            &bytes,
            false,
        )?;
        self.frames.push(file);
        self.progress = progress;
        self.revalidate()
    }
    pub(super) fn revalidate(&self) -> Result<()> {
        self.directory.revalidate()?;
        for file in &self.frames {
            file.persistent_revalidate()?;
        }
        Ok(())
    }
}

fn validate_backups(progress: &Progress) -> Result<()> {
    let backups = progress
        .backups
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("backups absent"))?;
    let mut expected: Vec<_> = progress.intent.target.sources.iter().collect();
    expected.sort_by_key(|s| {
        (
            match s.kind {
                HumanSourceKindV1::AuthenticationLedger => 0,
                HumanSourceKindV1::TenantStore => 1,
            },
            s.durable_namespace_id,
            s.source_id,
        )
    });
    ensure!(backups.len() == expected.len(), "backup closure differs");
    let mut identities = BTreeSet::new();
    for (record, source) in backups.iter().zip(expected) {
        record.backup_binding.validate()?;
        ensure!(
            record.source == *source
                && progress.intent.backup_paths.get(&source.source_id)
                    == Some(&record.backup_binding.path())
                && identities.insert(record.backup_binding.source_identity())
                && record.prefix.seq > 0
                && super::super::manifest::valid_digest(&record.prefix.hash)
                && record.records > 0
                && record.serialized_bytes.is_some()
                    == (source.kind == HumanSourceKindV1::AuthenticationLedger),
            "backup record differs"
        );
    }
    Ok(())
}
fn validate_confirmed(progress: &Progress) -> Result<()> {
    let backups = progress
        .backups
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("backups absent"))?;
    ensure!(
        progress.confirmed.len() <= backups.len(),
        "too many confirmations"
    );
    for (receipt, backup) in progress.confirmed.iter().zip(backups) {
        ensure!(
            backup.prefix.seq.checked_add(1) == Some(receipt.seq)
                && super::super::manifest::valid_digest(&receipt.hash),
            "anchor receipt differs"
        );
    }
    Ok(())
}

fn create_document(
    directory: &Directory,
    name: &str,
    bytes: &[u8],
    identical_retry: bool,
) -> Result<PrivateFile> {
    directory.revalidate()?;
    if directory.private_entry(OsStr::new(name))?.is_some() {
        ensure!(identical_retry, "journal destination already exists");
        let existing = directory.read_private_file(name, MAX_FRAME_BYTES)?;
        ensure!(existing.bytes() == bytes, "existing document differs");
        existing.persistent_revalidate()?;
        existing.persistent_sync()?;
        directory.sync()?;
        return Ok(existing);
    }
    let retained = directory.persistent_create(name, bytes)?;
    retained.persistent_sync()?;
    directory.sync()?;
    ensure!(
        retained.bytes() == bytes && retained.digest() == hex::encode(Sha256::digest(bytes)),
        "journal write differs"
    );
    retained.persistent_revalidate()?;
    Ok(retained)
}
pub(super) fn publish_manifest(
    directory: &Directory,
    intent: &MigrationIntent,
    closure: &HumanAuthorityClosureV1,
) -> Result<PrivateFile> {
    let bytes = serde_json::to_vec(closure)?;
    ensure!(
        HumanAuthorityClosureV1::from_bytes(&bytes)? == *closure
            && closure.closure_id == intent.closure_id
            && closure.closure_revision == intent.closure_revision,
        "manifest differs"
    );
    let journal = Journal::load(directory.clone(), intent.clone())?;
    ensure!(
        journal.confirmed_count() == closure.sources.len(),
        "incomplete journal cannot publish"
    );
    let expected: BTreeMap<_, _> = journal
        .backups()
        .iter()
        .zip(&journal.progress.confirmed)
        .map(|(backup, receipt)| (backup.source.source_id, receipt))
        .collect();
    ensure!(
        closure.sources.iter().all(|s| expected
            .get(&s.source_id)
            .is_some_and(|r| **r == s.authority_marker_receipt)),
        "manifest receipts differ"
    );
    create_document(directory, &intent.manifest_name(), &bytes, true)
}
