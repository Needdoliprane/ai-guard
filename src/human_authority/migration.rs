//! Offline migration of the complete retained tenant/authentication union.
//!
//! The journal records progress, never authority. Every resumed backup and
//! committed marker is checked against its real SQLite source. In particular,
//! a known commit is preserved when a subsequent journal/startup step fails.
//! This module creates no session, Ready value, route, permission or worker.

pub(crate) mod egress_journal;
mod journal;
pub mod operator;
mod sources;

use super::{
    HumanAuthorityClosureV1, HumanAuthorityMarkerReceiptV1, HumanConfigurationFileKindV1,
    HumanSourceDescriptorV1, HumanSourceKindV1, HumanSourcesConfigurationV1, OwnedHumanSources,
    maintenance::{
        HumanMaintenanceSourceToken, HumanSourcesMaintenanceConfigurationV1,
        HumanSourcesMaintenanceGuard, HumanSourcesMaintenancePlanV1,
    },
};
use crate::{
    audit::{
        LegacySessionPrepareAdoptionRefused, Store,
        authentication::{AuthenticationAudit, AuthenticationAuditPolicyV1},
    },
    control_fs::{Directory, PrivateFile},
    redaction::Redactor,
};
use anyhow::{Result, ensure};
use chrono::{DateTime, Utc};
use journal::{BackupRecord, Journal, MigrationIntent};
use sources::{MaintenanceSource, StartupSource, with_all_writers};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
use uuid::Uuid;

/// Resolved loader inputs. Keys must equal the guard's complete target view;
/// neither this DTO nor its source IDs select an authenticated request scope.
pub(crate) enum HumanMigrationSourceOptions {
    TenantStore {
        redactor: Redactor,
        max_events: u64,
        capacity: crate::audit::AuditCapacityPolicyV1,
        workload: crate::audit::WorkloadAuditPolicyV1,
        retention: crate::relay::RelayRetentionPolicyV1,
    },
    AuthenticationLedger {
        redactor: Redactor,
        policy: AuthenticationAuditPolicyV1,
        capacity_plan: crate::audit::authentication::AuthenticationCapacityPlanV1,
    },
}

pub(crate) struct HumanSourcesMigrationRequest {
    pub plan_file: PrivateFile,
    pub maintenance_configuration: HumanSourcesMaintenanceConfigurationV1,
    pub activation_files: Vec<(HumanConfigurationFileKindV1, PrivateFile)>,
    pub sources: BTreeMap<Uuid, HumanMigrationSourceOptions>,
    pub backup_paths: BTreeMap<Uuid, PathBuf>,
    pub journal_directory: Directory,
    pub closure_id: Uuid,
    pub closure_revision: u64,
}

/// A complete, checked private bundle. Its existence is not human readiness.
// Closed migrated-sources bundle; production wiring pending.
#[allow(dead_code)]
pub(crate) struct MigratedHumanSources {
    owner: OwnedHumanSources,
    stores: BTreeMap<Uuid, Store>,
    authentication: Option<AuthenticationAudit>,
    manifest: PrivateFile,
}

// Closed migrated-sources bundle accessors; production wiring pending.
#[allow(dead_code)]
impl MigratedHumanSources {
    pub(crate) fn owner(&self) -> &OwnedHumanSources {
        &self.owner
    }
    pub(crate) fn store(&self, source_id: Uuid) -> Option<&Store> {
        self.stores.get(&source_id)
    }
    pub(crate) fn authentication(&self) -> Option<&AuthenticationAudit> {
        self.authentication.as_ref()
    }
    pub(crate) fn manifest(&self) -> &PrivateFile {
        &self.manifest
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HumanSourcesMigrationFailure {
    InvalidRequest,
    MaintenanceRefused,
    LegacySessionPrepareUnsupported,
    ReconciliationRequired,
    ClockBehind,
    StartupRefused,
}
impl std::fmt::Display for HumanSourcesMigrationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "human_sources_migration_invalid_request",
            Self::MaintenanceRefused => "human_sources_migration_maintenance_refused",
            Self::LegacySessionPrepareUnsupported => {
                "human_sources_migration_legacy_session_prepare_unsupported"
            }
            Self::ReconciliationRequired => "human_sources_migration_reconciliation_required",
            Self::ClockBehind => "human_sources_migration_clock_behind",
            Self::StartupRefused => "human_sources_migration_startup_refused",
        })
    }
}
impl std::error::Error for HumanSourcesMigrationFailure {}

/// No mutable consumer escapes on any failure. Files, backups and already
/// committed databases are deliberately preserved for the same-plan retry.
pub(crate) fn migrate_human_sources(
    request: HumanSourcesMigrationRequest,
) -> std::result::Result<MigratedHumanSources, HumanSourcesMigrationFailure> {
    let plan: HumanSourcesMaintenancePlanV1 = serde_json::from_slice(request.plan_file.bytes())
        .map_err(|_| HumanSourcesMigrationFailure::InvalidRequest)?;
    validate_request(&request, &plan).map_err(|_| HumanSourcesMigrationFailure::InvalidRequest)?;
    let intent = MigrationIntent::new(&request, &plan)
        .map_err(|_| HumanSourcesMigrationFailure::InvalidRequest)?;
    let guard =
        HumanSourcesMaintenanceGuard::acquire(request.plan_file, request.maintenance_configuration)
            .map_err(|_| HumanSourcesMigrationFailure::MaintenanceRefused)?;

    let closure = migrate_under_maintenance(
        &guard,
        &plan,
        &request.sources,
        &request.backup_paths,
        &request.journal_directory,
        &intent,
    )
    .map_err(|error| {
        if error.is::<LegacySessionPrepareAdoptionRefused>() {
            HumanSourcesMigrationFailure::LegacySessionPrepareUnsupported
        } else {
            HumanSourcesMigrationFailure::ReconciliationRequired
        }
    })?;
    guard
        .check()
        .map_err(|_| HumanSourcesMigrationFailure::ReconciliationRequired)?;
    let manifest = journal::publish_manifest(&request.journal_directory, &intent, &closure)
        .map_err(|_| HumanSourcesMigrationFailure::ReconciliationRequired)?;
    guard
        .check()
        .map_err(|_| HumanSourcesMigrationFailure::ReconciliationRequired)?;
    // All maintenance wrappers and their cloned tokens have already dropped.
    // The active acquire rechecks the exact closure/configuration after this
    // unavoidable interstice; a previously checked pathname is no authority.
    drop(guard);
    let configured = HumanSourcesConfigurationV1::new(
        closure.sources.clone(),
        closure.configuration.clone(),
        request.activation_files,
    )
    .map_err(|_| HumanSourcesMigrationFailure::StartupRefused)?;
    let owner = OwnedHumanSources::acquire(manifest.clone(), configured)
        .map_err(|_| HumanSourcesMigrationFailure::StartupRefused)?;
    startup_complete_union(owner, manifest, &plan, &request.sources)
}

fn validate_request(
    request: &HumanSourcesMigrationRequest,
    plan: &HumanSourcesMaintenancePlanV1,
) -> Result<()> {
    ensure!(
        !request.closure_id.is_nil() && request.closure_revision > 0,
        "invalid migration closure"
    );
    let ids: BTreeSet<_> = plan.target.sources.iter().map(|s| s.source_id).collect();
    ensure!(
        ids.len() == plan.target.sources.len()
            && ids == request.sources.keys().copied().collect()
            && ids == request.backup_paths.keys().copied().collect(),
        "incomplete migration sources"
    );
    // This first vertical converts in place. Moving/removing a source requires
    // a separate retirement/copy protocol; a union lock alone cannot prove it.
    if let Some(prior) = &plan.prior {
        ensure!(
            prior.sources.len() == plan.target.sources.len(),
            "unsupported source-set change"
        );
        for (old, new) in prior.sources.iter().zip(&plan.target.sources) {
            ensure!(
                old.source_id == new.source_id
                    && old.kind == new.kind
                    && old.installation_id == new.installation_id
                    && old.durable_namespace_id == new.durable_namespace_id
                    && old.tenant_id == new.tenant_id
                    && old.storage_binding == new.storage_binding,
                "unsupported source replacement"
            );
            ensure!(
                old.required_format
                    == match old.kind {
                        HumanSourceKindV1::TenantStore => 13,
                        HumanSourceKindV1::AuthenticationLedger => 4,
                    },
                "unsupported prior format"
            );
        }
    }
    let mut paths = BTreeSet::new();
    let source_parents: BTreeSet<_> = plan
        .prior
        .iter()
        .flat_map(|p| &p.sources)
        .chain(&plan.target.sources)
        .map(|s| s.storage_binding.parent_identity())
        .collect();
    ensure!(
        !source_parents.contains(&request.journal_directory.physical_identity()?),
        "journal shares a source directory"
    );
    let mut backup_parents = BTreeSet::new();
    for source in &plan.target.sources {
        match request.sources.get(&source.source_id) {
            Some(HumanMigrationSourceOptions::TenantStore {
                max_events,
                capacity,
                workload,
                retention,
                ..
            }) => {
                capacity.validate(*max_events)?;
                workload.validate(*max_events, capacity)?;
                retention.validate()?;
            }
            Some(HumanMigrationSourceOptions::AuthenticationLedger { capacity_plan, .. }) => {
                capacity_plan.validate()?
            }
            None => anyhow::bail!("source options absent"),
        }
        ensure!(
            source.required_format
                == match source.kind {
                    HumanSourceKindV1::TenantStore => 14,
                    HumanSourceKindV1::AuthenticationLedger => 5,
                },
            "unsupported target format"
        );
        ensure!(
            matches!(
                (&source.kind, request.sources.get(&source.source_id)),
                (
                    HumanSourceKindV1::TenantStore,
                    Some(HumanMigrationSourceOptions::TenantStore { .. })
                ) | (
                    HumanSourceKindV1::AuthenticationLedger,
                    Some(HumanMigrationSourceOptions::AuthenticationLedger { .. })
                )
            ),
            "source options differ"
        );
        let path = request
            .backup_paths
            .get(&source.source_id)
            .ok_or_else(|| anyhow::anyhow!("backup absent"))?;
        let leaf = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("backup name invalid"))?;
        ensure!(
            !leaf.is_empty()
                && leaf.len() <= 128
                && leaf != "."
                && leaf != ".."
                && leaf
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)),
            "backup name invalid"
        );
        ensure!(
            crate::control_fs::absolute_path(path)? == *path && paths.insert(path.clone()),
            "backup path differs"
        );
        let parent = Directory::open_existing(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("backup parent absent"))?,
        )?;
        ensure!(
            !source_parents.contains(&parent.physical_identity()?)
                && backup_parents.insert(parent.physical_identity()?),
            "backup parents must be distinct"
        );
        ensure!(path.file_name().is_some(), "backup name absent");
    }
    Ok(())
}

fn writer_order(
    plan: &HumanSourcesMaintenancePlanV1,
) -> Vec<&super::maintenance::HumanMaintenanceSourceV1> {
    let mut ordered: Vec<_> = plan.target.sources.iter().collect();
    ordered.sort_by_key(|s| {
        (
            match s.kind {
                HumanSourceKindV1::AuthenticationLedger => 0,
                HumanSourceKindV1::TenantStore => 1,
            },
            s.durable_namespace_id,
            s.source_id,
        )
    });
    ordered
}

fn migrate_under_maintenance(
    guard: &HumanSourcesMaintenanceGuard,
    plan: &HumanSourcesMaintenancePlanV1,
    settings: &BTreeMap<Uuid, HumanMigrationSourceOptions>,
    backup_paths: &BTreeMap<Uuid, PathBuf>,
    directory: &Directory,
    intent: &MigrationIntent,
) -> Result<HumanAuthorityClosureV1> {
    let ordered = writer_order(plan);
    let tokens: Vec<_> = ordered
        .iter()
        .map(|s| guard.target_source(s.source_id))
        .collect::<std::result::Result<_, _>>()?;
    let opened: Vec<_> = ordered
        .iter()
        .zip(&tokens)
        .map(|(s, token)| {
            MaintenanceSource::open(
                s,
                settings
                    .get(&s.source_id)
                    .ok_or_else(|| anyhow::anyhow!("source options absent"))?,
                token,
            )
        })
        .collect::<Result<_>>()?;
    let mut journal = Journal::load(directory.clone(), intent.clone())?;
    let mut receipts = None;
    with_all_writers(&opened, &tokens, &mut |writers| {
        // This callback is reached only after every real Immediate busy0
        // transaction was acquired. No journal creation/backup/DDL precedes it.
        guard.check()?;
        let mut observed = BTreeMap::new();
        // Inspect the complete locked union before writing an intent or
        // touching a backup. A refusal in the last Store must be effect-free
        // for every preceding tenant and the authentication ledger.
        writers.visit(&mut |index, writer| {
            observed.insert(index, writer.inspect()?);
            Ok(())
        })?;
        let resuming = journal.has_intent();
        if !resuming {
            // Check every complete SQLite namespace before the first intent
            // write. Existing entries cannot establish this attempt's provenance.
            for path in backup_paths.values() {
                let parent = Directory::open_existing(
                    path.parent()
                        .ok_or_else(|| anyhow::anyhow!("backup parent absent"))?,
                )?;
                let name = path
                    .file_name()
                    .ok_or_else(|| anyhow::anyhow!("backup name absent"))?;
                for suffix in ["", "-wal", "-shm", "-journal"] {
                    let mut entry = name.to_os_string();
                    entry.push(suffix);
                    ensure!(
                        parent.private_entry(&entry)?.is_none(),
                        "initial backup SQLite namespace is occupied"
                    );
                }
            }
        }
        journal.ensure_intent()?;
        let mut backups = Vec::with_capacity(tokens.len());
        let mut records = Vec::with_capacity(tokens.len());
        let mut converted_count = 0;
        let mut ordinary_seen = false;
        let tenants: BTreeSet<_> = ordered.iter().filter_map(|s| s.tenant_id.clone()).collect();
        writers.visit(&mut |index, writer| {
            let token = &tokens[index];
            let state = observed
                .remove(&index)
                .ok_or_else(|| anyhow::anyhow!("source observation absent"))?;
            writer.validate_tenant_closure(
                &tenants,
                settings
                    .get(&ordered[index].source_id)
                    .ok_or_else(|| anyhow::anyhow!("source options absent"))?,
            )?;
            if !journal.prepared() {
                ensure!(state.anchor.is_none(), "conversion lacks prepared journal");
            }
            if state.anchor.is_some() {
                ensure!(
                    !ordinary_seen,
                    "converted sources are not a committed prefix"
                );
                converted_count += 1;
            } else {
                ordinary_seen = true;
            }
            if let Some(receipt) = journal.confirmed(index) {
                ensure!(
                    state.anchor.as_ref() == Some(receipt),
                    "journal confirmation differs from source"
                );
            }
            let path = backup_paths
                .get(&token.descriptor().source_id)
                .ok_or_else(|| anyhow::anyhow!("backup absent"))?;
            let parent = Directory::open_existing(
                path.parent()
                    .ok_or_else(|| anyhow::anyhow!("backup parent absent"))?,
            )?;
            let exists = parent
                .private_entry(
                    path.file_name()
                        .ok_or_else(|| anyhow::anyhow!("backup name absent"))?,
                )?
                .is_some();
            // Fresh attempts always use exclusive creation, including when a
            // destination becomes occupied after the complete preflight.
            let backup = writer.backup(token, path, resuming && exists)?;
            let record = BackupRecord::from_verified(token, &backup)?;
            if let Some(prefix) = state.prefix {
                ensure!(record.prefix == prefix, "backup prefix differs");
            }
            records.push(record);
            backups.push(backup);
            Ok(())
        })?;
        ensure!(
            converted_count >= journal.confirmed_count()
                && converted_count <= journal.confirmed_count() + 1,
            "source progress differs from journal"
        );
        journal.prepare(records)?;
        guard.check()?;
        let mut confirmed = Vec::with_capacity(tokens.len());
        writers.visit(&mut |index, writer| {
            let pending = writer.install(&backups[index])?;
            guard.check()?;
            writer.commit()?;
            #[cfg(test)]
            fail_after_commit(index)?;
            // Real re-open and replay after known COMMIT, while all remaining
            // writers and the physical maintenance union remain retained.
            let observed = MaintenanceSource::open(
                ordered[index],
                settings
                    .get(&ordered[index].source_id)
                    .ok_or_else(|| anyhow::anyhow!("source options absent"))?,
                &tokens[index],
            )?
            .inspect_committed(
                &tokens[index],
                backup_paths
                    .get(&ordered[index].source_id)
                    .ok_or_else(|| anyhow::anyhow!("backup absent"))?,
            )?;
            ensure!(
                observed.anchor.as_ref() == Some(&pending),
                "committed anchor differs"
            );
            ensure!(
                observed.prefix.as_ref() == Some(&journal.backups()[index].prefix),
                "committed prefix differs"
            );
            journal.confirm(index, pending.clone())?;
            confirmed.push(pending);
            guard.check()?;
            Ok(())
        })?;
        receipts = Some(confirmed);
        Ok(())
    })?;
    guard.check()?;
    journal.revalidate()?;
    let receipts = receipts.ok_or_else(|| anyhow::anyhow!("migration receipts absent"))?;
    let by_id: BTreeMap<_, _> = ordered
        .iter()
        .zip(receipts)
        .map(|(source, receipt)| (source.source_id, receipt))
        .collect();
    let sources = plan
        .target
        .sources
        .iter()
        .map(|s| {
            Ok(HumanSourceDescriptorV1 {
                source_id: s.source_id,
                kind: s.kind,
                installation_id: s.installation_id,
                durable_namespace_id: s.durable_namespace_id,
                tenant_id: s.tenant_id.clone(),
                storage_binding: s.storage_binding.clone(),
                required_format: s.required_format,
                authority_marker_receipt: by_id
                    .get(&s.source_id)
                    .ok_or_else(|| anyhow::anyhow!("receipt absent"))?
                    .clone(),
            })
        })
        .collect::<Result<_>>()?;
    let mut closure = HumanAuthorityClosureV1 {
        version: 1,
        installation_id: plan.installation_id,
        closure_id: intent.closure_id,
        closure_revision: intent.closure_revision,
        closure_digest: String::new(),
        sources,
        configuration: plan.target.configuration.clone(),
        supported_storage_profile: plan.supported_storage_profile,
    };
    closure.closure_digest = closure.compute_digest()?;
    Ok(closure)
}

fn startup_complete_union(
    owner: OwnedHumanSources,
    manifest: PrivateFile,
    plan: &HumanSourcesMaintenancePlanV1,
    settings: &BTreeMap<Uuid, HumanMigrationSourceOptions>,
) -> std::result::Result<MigratedHumanSources, HumanSourcesMigrationFailure> {
    let mut startup = Vec::new();
    for source in writer_order(plan) {
        let opened = (|| -> Result<_> {
            let token = owner.source_token(source.source_id)?;
            let startup = StartupSource::open(
                source,
                settings
                    .get(&source.source_id)
                    .ok_or_else(|| anyhow::anyhow!("source options absent"))?,
                &token,
            )?;
            let floor = startup.durable_wall_floor()?;
            Ok((source.source_id, token, startup, floor))
        })()
        .map_err(|_| HumanSourcesMigrationFailure::StartupRefused)?;
        startup.push(opened);
    }
    let maximum = startup
        .iter()
        .map(|(_, _, _, floor)| *floor)
        .max()
        .ok_or(HumanSourcesMigrationFailure::StartupRefused)?;
    #[cfg(test)]
    STARTUP_FLOORS_OBSERVER.with(|observer| {
        if let Some(observer) = observer.borrow().as_ref() {
            let floors: Vec<_> = startup
                .iter()
                .map(|(source_id, _, _, floor)| (*source_id, *floor))
                .collect();
            observer(&floors, maximum);
        }
    });
    check_maximum_floor(Utc::now(), maximum)?;
    let mut stores = BTreeMap::new();
    let mut authentication = None;
    for (source_id, token, source, _) in startup {
        let retained = (|| -> Result<_> {
            let epoch = owner.try_write_epoch()?;
            epoch.validate_source(&token)?;
            let revision = source.current_incarnation_revision(&epoch, &token)?;
            source.append_incarnation(&epoch, &token, Uuid::new_v4(), revision)?;
            let retained = source.retain(&epoch, &token)?;
            epoch.validate_source(&token)?;
            Ok(retained)
        })()
        .map_err(|_| HumanSourcesMigrationFailure::StartupRefused)?;
        match retained {
            sources::RetainedSource::Store(store) => {
                stores.insert(source_id, store);
            }
            sources::RetainedSource::Authentication(audit) => {
                authentication = Some(audit);
            }
        }
    }
    // Revalidate every retained source under the same still-live owner before
    // publishing the aggregate. A partial incarnation never publishes a bundle.
    for source in &owner.closure().sources {
        owner
            .source_token(source.source_id)
            .map_err(|_| HumanSourcesMigrationFailure::StartupRefused)?;
    }
    manifest
        .persistent_revalidate()
        .map_err(|_| HumanSourcesMigrationFailure::StartupRefused)?;
    Ok(MigratedHumanSources {
        owner,
        stores,
        authentication,
        manifest,
    })
}

fn check_maximum_floor(
    now: DateTime<Utc>,
    maximum: DateTime<Utc>,
) -> std::result::Result<(), HumanSourcesMigrationFailure> {
    if now < maximum {
        Err(HumanSourcesMigrationFailure::ClockBehind)
    } else {
        Ok(())
    }
}

#[cfg(test)]
thread_local! { static FAIL_AFTER_COMMIT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
fn fail_after_commit(index: usize) -> Result<()> {
    FAIL_AFTER_COMMIT.with(|point| {
        if point.get() == Some(index) {
            point.set(None);
            anyhow::bail!("injected after real commit");
        }
        Ok(())
    })
}

#[cfg(test)]
type StartupFloorsObserver = dyn Fn(&[(Uuid, DateTime<Utc>)], DateTime<Utc>);
#[cfg(test)]
thread_local! {
    static STARTUP_FLOORS_OBSERVER: std::cell::RefCell<Option<Box<StartupFloorsObserver>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) mod tests;
