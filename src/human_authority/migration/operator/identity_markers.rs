//! Offline in-place identity-marker maintenance of one adopted owned tenant
//! source (Store14 or Store15): explicit, backed up, journaled, resumable.
//!
//! The plan keeps the published active closure unchanged (prior = target =
//! active manifest). The maintenance guard takes the real owner locks of the
//! whole union, so a running gateway makes it refuse; `--confirm-control-
//! stopped` remains the operator's attestation for other consumers. The
//! verified source backup and the journal are those of the egress step.
use super::{observe_private_file_binding, validate_absolute_path};
use crate::{
    audit::{Store, identity_markers::IdentityMarkersFailure},
    control_fs::{Directory, PrivateFile},
    human_authority::{
        HumanAuthorityClosureV1, HumanAuthorityConfigurationV1, HumanAuthorityFailure,
        HumanConfigurationFileKindV1, HumanSourceKindV1,
        maintenance::{
            HumanMaintenanceResolvedViewV1, HumanSourcesMaintenanceConfigurationV1,
            HumanSourcesMaintenanceGuard, HumanSourcesMaintenancePlanV1,
        },
        migration::egress_journal::RetainedSourceIntent,
    },
    redaction::Redactor,
};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const PLAN_NAME: &str = "plan.json";
const JOURNAL_NAME: &str = "journal";
const BACKUPS_NAME: &str = "backups";
const MAX_PLAN_BYTES: usize = 512 * 1024;

/// Retained, already validated inputs of the published activation. Holding
/// them grants nothing: the guard, opener and journal recheck everything.
pub struct OwnedIdentityMarkersRequest {
    /// Published active closure manifest of the adopted union.
    pub manifest: PrivateFile,
    /// Resolved configuration of that closure and its retained private files.
    pub configuration: HumanAuthorityConfigurationV1,
    pub files: Vec<(HumanConfigurationFileKindV1, PrivateFile)>,
    pub tenant_id: String,
    pub redactor: Redactor,
    pub max_events: u64,
    /// Absent before `install`; the same directory for `resume`.
    pub maintenance_directory: PathBuf,
    /// Directories that must not contain the maintenance directory.
    pub excluded_directories: Vec<PathBuf>,
    pub resume: bool,
}

/// Closed, secret-free refusal codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnedIdentityMarkersRefusalV1 {
    SourceUnknown,
    FormatUnsupported,
    AlreadyInstalled,
    OwnerBusy,
    JournalConflict,
}
impl OwnedIdentityMarkersRefusalV1 {
    pub fn code(self) -> &'static str {
        match self {
            Self::SourceUnknown => "human_rights_source_unknown",
            Self::FormatUnsupported => "human_rights_format_unsupported",
            Self::AlreadyInstalled => "human_rights_already_installed",
            Self::OwnerBusy => "human_rights_owner_busy",
            Self::JournalConflict => "human_rights_journal_conflict",
        }
    }
}
impl std::fmt::Display for OwnedIdentityMarkersRefusalV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for OwnedIdentityMarkersRefusalV1 {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OwnedIdentityMarkerReceiptV1 {
    pub marker: String,
    pub seq: i64,
    pub hash: String,
}

/// Closed report: no secret, credential or SQLite content.
#[derive(Clone, Debug, Serialize)]
pub struct OwnedIdentityMarkersReportV1 {
    pub version: u32,
    pub step: &'static str,
    /// `installed` when this invocation committed the markers, `confirmed`
    /// when an earlier invocation of the same maintenance had committed them.
    pub state: &'static str,
    pub tenant_id: String,
    pub source_id: Uuid,
    pub store_format: u32,
    pub markers: Vec<OwnedIdentityMarkerReceiptV1>,
    pub backup_path: PathBuf,
    pub readiness: &'static str,
}

fn refusal(error: anyhow::Error, default: OwnedIdentityMarkersRefusalV1) -> anyhow::Error {
    let code = if error.chain().any(|cause| {
        cause.downcast_ref::<IdentityMarkersFailure>()
            == Some(&IdentityMarkersFailure::AlreadyInstalled)
    }) {
        OwnedIdentityMarkersRefusalV1::AlreadyInstalled
    } else if error.chain().any(|cause| {
        cause.downcast_ref::<HumanAuthorityFailure>()
            == Some(&HumanAuthorityFailure::OwnerContended)
    }) {
        OwnedIdentityMarkersRefusalV1::OwnerBusy
    } else {
        default
    };
    error.context(code)
}

struct MaintenanceDirectory {
    plan: HumanSourcesMaintenancePlanV1,
    plan_file: PrivateFile,
    journal: Directory,
    backups: Directory,
}

fn leaf(path: &Path) -> Result<(Directory, &str)> {
    let parent = Directory::open_existing(path.parent().context("maintenance parent required")?)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("maintenance directory name invalid")?;
    Ok((parent, name))
}

fn synchronize_plan(plan: &PrivateFile, directory: &Directory, parent: &Directory) -> Result<()> {
    plan.persistent_sync()?;
    directory.sync()?;
    parent.sync()?;
    plan.persistent_revalidate()
}

/// Create the new private directory: journal and backups first, then the
/// plan, synchronized last. A directory without its plan is never adopted.
fn create_directory(
    path: &Path,
    closure: &HumanAuthorityClosureV1,
    manifest: &PrivateFile,
    selected_source_id: Uuid,
) -> Result<MaintenanceDirectory> {
    let (parent, name) = leaf(path)?;
    let directory = parent.exclusive_child(name)?;
    let journal = directory
        .persistent_child(JOURNAL_NAME, true)?
        .context("maintenance journal missing")?;
    let backups = directory
        .persistent_child(BACKUPS_NAME, true)?
        .context("maintenance backups missing")?;
    let plan = HumanSourcesMaintenancePlanV1::in_place(
        Uuid::new_v4(),
        closure,
        observe_private_file_binding(manifest)?,
        selected_source_id,
    );
    let bytes = serde_json::to_vec(&plan)?;
    ensure!(
        bytes.len() <= MAX_PLAN_BYTES,
        "maintenance plan exceeds bound"
    );
    let plan_file = directory.persistent_create(PLAN_NAME, &bytes)?;
    synchronize_plan(&plan_file, &directory, &parent)?;
    Ok(MaintenanceDirectory {
        plan,
        plan_file,
        journal,
        backups,
    })
}

/// Reopen the same directory; its plan must be the exact in-place plan of
/// the unchanged published closure and selected source, with its original id.
fn open_directory(
    path: &Path,
    closure: &HumanAuthorityClosureV1,
    manifest: &PrivateFile,
    selected_source_id: Uuid,
) -> Result<MaintenanceDirectory> {
    let (parent, _) = leaf(path)?;
    let directory = Directory::open_existing(path)?;
    let mut names: Vec<_> = directory
        .persistent_entries(8)?
        .into_iter()
        .map(|entry| (entry.name, entry.directory))
        .collect();
    names.sort();
    ensure!(
        names
            == [
                (BACKUPS_NAME.to_owned(), true),
                (JOURNAL_NAME.to_owned(), true),
                (PLAN_NAME.to_owned(), false),
            ],
        "maintenance directory entries differ"
    );
    let plan_file = directory
        .persistent_read(PLAN_NAME, MAX_PLAN_BYTES as u64, &[])?
        .context("maintenance plan missing")?;
    let recorded: HumanSourcesMaintenancePlanV1 = serde_json::from_slice(plan_file.bytes())?;
    let plan = HumanSourcesMaintenancePlanV1::in_place(
        recorded.migration_id,
        closure,
        observe_private_file_binding(manifest)?,
        selected_source_id,
    );
    ensure!(
        serde_json::to_vec(&plan)? == plan_file.bytes(),
        "maintenance plan differs from the published closure or selected source"
    );
    let journal = directory
        .persistent_child(JOURNAL_NAME, false)?
        .context("maintenance journal missing")?;
    let backups = directory
        .persistent_child(BACKUPS_NAME, false)?
        .context("maintenance backups missing")?;
    // A previous attempt may have written readable bytes without completing
    // the file and directory barriers. Reestablish them before opening SQLite.
    synchronize_plan(&plan_file, &directory, &parent)?;
    Ok(MaintenanceDirectory {
        plan,
        plan_file,
        journal,
        backups,
    })
}

fn check_location(path: &Path, excluded: &[PathBuf]) -> Result<()> {
    validate_absolute_path(path)?;
    ensure!(
        excluded.iter().all(|excluded| !path.starts_with(excluded)),
        "maintenance directory overlaps a protected directory"
    );
    Ok(())
}

/// Install every pending in-place identity marker of the selected tenant
/// source, or finish the same maintenance after an interruption.
pub fn install_owned_identity_markers(
    request: OwnedIdentityMarkersRequest,
) -> Result<OwnedIdentityMarkersReportV1> {
    use OwnedIdentityMarkersRefusalV1 as Refusal;
    let OwnedIdentityMarkersRequest {
        manifest,
        configuration,
        files,
        tenant_id,
        redactor,
        max_events,
        maintenance_directory,
        excluded_directories,
        resume,
    } = request;
    let conflict = |error| refusal(error, Refusal::JournalConflict);
    check_location(&maintenance_directory, &excluded_directories).map_err(conflict)?;
    let closure = HumanAuthorityClosureV1::from_bytes(manifest.bytes())
        .map_err(anyhow::Error::from)
        .and_then(|closure| {
            ensure!(
                closure.configuration == configuration,
                "maintenance configuration differs from the published closure"
            );
            Ok(closure)
        })
        .map_err(conflict)?;
    // Nor inside any source directory of the union.
    let sources: Vec<PathBuf> = closure
        .sources
        .iter()
        .map(|source| source.storage_binding.parent_path.clone())
        .collect();
    check_location(&maintenance_directory, &sources).map_err(conflict)?;
    let selected = closure
        .sources
        .iter()
        .find(|source| {
            source.kind == HumanSourceKindV1::TenantStore
                && source.tenant_id.as_deref() == Some(tenant_id.as_str())
        })
        .cloned()
        .ok_or(Refusal::SourceUnknown)?;
    if !matches!(selected.required_format, 14 | 15) {
        return Err(Refusal::FormatUnsupported.into());
    }
    let maintenance = if resume {
        open_directory(
            &maintenance_directory,
            &closure,
            &manifest,
            selected.source_id,
        )
    } else {
        create_directory(
            &maintenance_directory,
            &closure,
            &manifest,
            selected.source_id,
        )
    }
    .map_err(conflict)?;
    let resolve = || {
        HumanMaintenanceResolvedViewV1::new(
            maintenance.plan.target.sources.clone(),
            configuration.clone(),
            files.clone(),
        )
    };
    let guard = resolve()
        .and_then(|prior| {
            HumanSourcesMaintenanceGuard::acquire(
                maintenance.plan_file.clone(),
                HumanSourcesMaintenanceConfigurationV1::new(
                    Some(prior),
                    resolve()?,
                    Some(manifest.clone()),
                ),
            )
        })
        .map_err(|error| refusal(error.into(), Refusal::JournalConflict))?;
    let token = guard
        .target_source(selected.source_id)
        .map_err(|error| refusal(error.into(), Refusal::JournalConflict))?;
    let opened = Store::open_review_maintenance_for_tenant(
        &selected.storage_binding.path(),
        &tenant_id,
        redactor.clone(),
        max_events,
        &token,
    )
    .map_err(|error| refusal(error, Refusal::FormatUnsupported))?;
    let (intent, already_committed) = opened
        .with_review_maintenance_writer(&token, |writer| {
            let intent = RetainedSourceIntent::enter_identity_markers(
                &token,
                writer,
                maintenance.journal.clone(),
                maintenance.backups.clone(),
            )?;
            if writer.identity_markers_committed(&token)? {
                return Ok((intent, true));
            }
            {
                let backup = if intent.backup_recorded()? {
                    intent.resume_backup(writer)?
                } else {
                    intent.prepare_backup(writer)?
                };
                backup.install_identity_markers(writer)?;
            }
            writer.commit()?;
            Ok((intent, false))
        })
        .map_err(conflict)?;
    drop(opened);
    let receipts = intent
        .confirm_identity_markers(redactor, max_events)
        .map_err(conflict)?;
    let backup_path = intent.backup_path().map_err(conflict)?;
    drop(intent);
    drop(token);
    drop(guard);
    Ok(OwnedIdentityMarkersReportV1 {
        version: 1,
        step: "in_place_identity_markers",
        state: if already_committed {
            "confirmed"
        } else {
            "installed"
        },
        tenant_id,
        source_id: selected.source_id,
        store_format: selected.required_format,
        markers: receipts
            .into_iter()
            .map(|receipt| OwnedIdentityMarkerReceiptV1 {
                marker: receipt.marker.name().to_owned(),
                seq: receipt.seq,
                hash: receipt.hash,
            })
            .collect(),
        backup_path,
        readiness: "not_checked",
    })
}
