//! Adapters over the real source wrappers, and borrowed writer continuations.
use super::*;
use crate::{
    audit::{
        AuthorityReceiptV1, ReviewSourceMigrationState, ReviewStoreMaintenance,
        ReviewStoreMaintenanceWriter, ReviewStoreStartup, VerifiedReviewSourceBackup,
        authentication::{
            AuthenticationMaintenanceWriter, AuthenticationReceiptV1,
            AuthenticationReviewMaintenance, AuthenticationReviewStartup,
            AuthenticationSourceMigrationState, VerifiedAuthenticationSourceBackup,
        },
    },
    human_authority::{
        HumanSourcesWriteEpoch, OwnedSourceToken, maintenance::HumanMaintenanceSourceV1,
    },
};
use std::path::Path;

pub(super) struct ObservedSource {
    pub anchor: Option<HumanAuthorityMarkerReceiptV1>,
    pub prefix: Option<HumanAuthorityMarkerReceiptV1>,
}

pub(super) enum SourceBackup<'t> {
    Store {
        token: &'t HumanMaintenanceSourceToken,
        proof: VerifiedReviewSourceBackup<'t>,
    },
    Authentication {
        proof: VerifiedAuthenticationSourceBackup<'t>,
    },
}

fn marker(receipt: AuthorityReceiptV1) -> HumanAuthorityMarkerReceiptV1 {
    HumanAuthorityMarkerReceiptV1 {
        seq: receipt.seq,
        hash: receipt.hash,
    }
}
fn auth_marker(receipt: AuthenticationReceiptV1) -> Result<HumanAuthorityMarkerReceiptV1> {
    Ok(HumanAuthorityMarkerReceiptV1 {
        seq: i64::try_from(receipt.sequence)?,
        hash: receipt.hash,
    })
}

/// Object-safe only to keep heterogeneous borrowed callbacks nested. There is
/// no constructor for a writer or backup here: the real wrappers provide both.
pub(super) trait SourceWriter {
    fn inspect(&self) -> Result<ObservedSource>;
    fn validate_tenant_closure(
        &self,
        tenants: &BTreeSet<String>,
        options: &HumanMigrationSourceOptions,
    ) -> Result<()>;
    fn backup<'t>(
        &self,
        token: &'t HumanMaintenanceSourceToken,
        path: &Path,
        resume: bool,
    ) -> Result<SourceBackup<'t>>;
    fn install(&mut self, backup: &SourceBackup<'_>) -> Result<HumanAuthorityMarkerReceiptV1>;
    fn commit(&mut self) -> Result<()>;
}
impl SourceWriter for ReviewStoreMaintenanceWriter<'_, '_> {
    fn validate_tenant_closure(
        &self,
        _: &BTreeSet<String>,
        _: &HumanMigrationSourceOptions,
    ) -> Result<()> {
        Ok(())
    }
    fn inspect(&self) -> Result<ObservedSource> {
        Ok(match self.inspect()? {
            ReviewSourceMigrationState::Ordinary13 {
                prefix_seq,
                prefix_hash,
            } => ObservedSource {
                anchor: None,
                prefix: Some(HumanAuthorityMarkerReceiptV1 {
                    seq: prefix_seq,
                    hash: prefix_hash,
                }),
            },
            ReviewSourceMigrationState::Converted14 { anchor } => ObservedSource {
                anchor: Some(marker(anchor)),
                prefix: None,
            },
        })
    }
    fn backup<'t>(
        &self,
        token: &'t HumanMaintenanceSourceToken,
        path: &Path,
        resume: bool,
    ) -> Result<SourceBackup<'t>> {
        let proof = if resume {
            self.resume_backup(token, path)?
        } else {
            self.prepare_backup(token, path)?
        };
        Ok(SourceBackup::Store { token, proof })
    }
    fn install(&mut self, backup: &SourceBackup<'_>) -> Result<HumanAuthorityMarkerReceiptV1> {
        let SourceBackup::Store { token, proof } = backup else {
            anyhow::bail!("backup kind differs");
        };
        Ok(marker(self.install_format14(token, proof)?))
    }
    fn commit(&mut self) -> Result<()> {
        self.commit()
    }
}
impl SourceWriter for AuthenticationMaintenanceWriter<'_, '_> {
    fn validate_tenant_closure(
        &self,
        tenants: &BTreeSet<String>,
        options: &HumanMigrationSourceOptions,
    ) -> Result<()> {
        let plan = self
            .capacity_plan()?
            .ok_or_else(|| anyhow::anyhow!("authentication capacity plan absent"))?;
        let HumanMigrationSourceOptions::AuthenticationLedger { capacity_plan, .. } = options
        else {
            anyhow::bail!("authentication source options differ")
        };
        ensure!(
            &plan == capacity_plan,
            "authentication durable capacity plan differs"
        );
        let planned: BTreeSet<_> = plan
            .tenants
            .into_iter()
            .map(|tenant| tenant.tenant_id)
            .collect();
        ensure!(planned == *tenants, "authentication tenant closure differs");
        Ok(())
    }
    fn inspect(&self) -> Result<ObservedSource> {
        Ok(match self.inspect()? {
            AuthenticationSourceMigrationState::Ordinary4 { prefix } => ObservedSource {
                anchor: None,
                prefix: Some(auth_marker(prefix)?),
            },
            AuthenticationSourceMigrationState::Converted5 { anchor } => ObservedSource {
                anchor: Some(auth_marker(anchor)?),
                prefix: None,
            },
        })
    }
    fn backup<'t>(
        &self,
        token: &'t HumanMaintenanceSourceToken,
        path: &Path,
        resume: bool,
    ) -> Result<SourceBackup<'t>> {
        let proof = if resume {
            self.resume_backup(token, path)?
        } else {
            self.prepare_backup(token, path)?
        };
        Ok(SourceBackup::Authentication { proof })
    }
    fn install(&mut self, backup: &SourceBackup<'_>) -> Result<HumanAuthorityMarkerReceiptV1> {
        let SourceBackup::Authentication { proof } = backup else {
            anyhow::bail!("backup kind differs");
        };
        auth_marker(self.install_format5(proof)?)
    }
    fn commit(&mut self) -> Result<()> {
        self.commit()
    }
}

pub(super) enum MaintenanceSource {
    Store(ReviewStoreMaintenance),
    Authentication(AuthenticationReviewMaintenance),
}
impl MaintenanceSource {
    pub(super) fn open(
        source: &HumanMaintenanceSourceV1,
        options: &HumanMigrationSourceOptions,
        token: &HumanMaintenanceSourceToken,
    ) -> Result<Self> {
        Ok(match (source.kind, options) {
            (
                HumanSourceKindV1::TenantStore,
                HumanMigrationSourceOptions::TenantStore {
                    redactor,
                    max_events,
                    ..
                },
            ) => Self::Store(Store::open_review_maintenance_for_tenant(
                &source.storage_binding.path(),
                source
                    .tenant_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("tenant absent"))?,
                redactor.clone(),
                *max_events,
                token,
            )?),
            (
                HumanSourceKindV1::AuthenticationLedger,
                HumanMigrationSourceOptions::AuthenticationLedger {
                    redactor, policy, ..
                },
            ) => Self::Authentication(AuthenticationAudit::open_review_maintenance(
                &source.storage_binding.path(),
                source.installation_id,
                policy.clone(),
                redactor.clone(),
                token,
            )?),
            _ => anyhow::bail!("source options differ"),
        })
    }
    pub(super) fn inspect_committed(
        &self,
        token: &HumanMaintenanceSourceToken,
        backup_path: &Path,
    ) -> Result<ObservedSource> {
        self.with_writer(token, &mut |writer| {
            let mut observed = writer.inspect()?;
            // The retained backup API compares the real source anchor's prefix
            // with the actual backup; journal fields never substitute for it.
            let backup = writer.backup(token, backup_path, true)?;
            observed.prefix = Some(BackupRecord::from_verified(token, &backup)?.prefix);
            Ok(observed)
        })
    }
    fn with_writer<T>(
        &self,
        token: &HumanMaintenanceSourceToken,
        work: &mut impl FnMut(&mut dyn SourceWriter) -> Result<T>,
    ) -> Result<T> {
        match self {
            Self::Store(source) => {
                source.with_review_maintenance_writer(token, |writer| work(writer))
            }
            Self::Authentication(source) => {
                source.with_review_maintenance_writer(token, |writer| work(writer))
            }
        }
    }
}

pub(super) trait WriterSet {
    fn visit(
        &mut self,
        visitor: &mut dyn FnMut(usize, &mut dyn SourceWriter) -> Result<()>,
    ) -> Result<()>;
}
struct EmptyWriters;
impl WriterSet for EmptyWriters {
    fn visit(
        &mut self,
        _: &mut dyn FnMut(usize, &mut dyn SourceWriter) -> Result<()>,
    ) -> Result<()> {
        Ok(())
    }
}
struct WriterNode<'a> {
    previous: &'a mut dyn WriterSet,
    writer: &'a mut dyn SourceWriter,
    index: usize,
}
impl WriterSet for WriterNode<'_> {
    fn visit(
        &mut self,
        visitor: &mut dyn FnMut(usize, &mut dyn SourceWriter) -> Result<()>,
    ) -> Result<()> {
        self.previous.visit(visitor)?;
        visitor(self.index, self.writer)
    }
}
pub(super) fn with_all_writers<T>(
    opened: &[MaintenanceSource],
    tokens: &[HumanMaintenanceSourceToken],
    work: &mut impl FnMut(&mut dyn WriterSet) -> Result<T>,
) -> Result<T> {
    ensure!(
        opened.len() == tokens.len() && !opened.is_empty(),
        "writer closure differs"
    );
    acquire_next(opened, tokens, 0, &mut EmptyWriters, work)
}
fn acquire_next<T>(
    opened: &[MaintenanceSource],
    tokens: &[HumanMaintenanceSourceToken],
    index: usize,
    previous: &mut dyn WriterSet,
    work: &mut impl FnMut(&mut dyn WriterSet) -> Result<T>,
) -> Result<T> {
    if index == opened.len() {
        return work(previous);
    }
    opened[index].with_writer(&tokens[index], &mut |writer| {
        let mut node = WriterNode {
            previous,
            writer,
            index,
        };
        acquire_next(opened, tokens, index + 1, &mut node, work)
    })
}

pub(super) enum StartupSource {
    Store(ReviewStoreStartup),
    Authentication(AuthenticationReviewStartup),
}
pub(super) enum RetainedSource {
    Store(Store),
    Authentication(AuthenticationAudit),
}
impl StartupSource {
    pub(super) fn open(
        source: &HumanMaintenanceSourceV1,
        options: &HumanMigrationSourceOptions,
        token: &OwnedSourceToken,
    ) -> Result<Self> {
        Ok(match (source.kind, options) {
            (
                HumanSourceKindV1::TenantStore,
                HumanMigrationSourceOptions::TenantStore {
                    redactor,
                    max_events,
                    capacity,
                    workload,
                    retention,
                },
            ) => {
                let mut store = Store::open_review_startup_for_tenant(
                    &source.storage_binding.path(),
                    source
                        .tenant_id
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("tenant absent"))?,
                    redactor.clone(),
                    *max_events,
                    token,
                )?;
                store.configure_owned_read_policies(
                    capacity.clone(),
                    workload.clone(),
                    retention.clone(),
                )?;
                Self::Store(store)
            }
            (
                HumanSourceKindV1::AuthenticationLedger,
                HumanMigrationSourceOptions::AuthenticationLedger {
                    redactor,
                    policy,
                    capacity_plan,
                },
            ) => {
                let audit = AuthenticationAudit::open_review_startup(
                    &source.storage_binding.path(),
                    source.installation_id,
                    policy.clone(),
                    redactor.clone(),
                    token,
                )?;
                audit.require_capacity_plan(capacity_plan)?;
                Self::Authentication(audit)
            }
            _ => anyhow::bail!("source options differ"),
        })
    }
    pub(super) fn durable_wall_floor(&self) -> Result<DateTime<Utc>> {
        match self {
            Self::Store(source) => source.durable_wall_floor(),
            Self::Authentication(source) => source.durable_wall_floor(),
        }
    }
    pub(super) fn current_incarnation_revision(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        token: &OwnedSourceToken,
    ) -> Result<u64> {
        epoch.validate_source(token)?;
        let revision = match self {
            Self::Store(source) => source.current_incarnation_revision(epoch)?,
            Self::Authentication(source) => source.current_incarnation_revision(epoch, token)?,
        };
        epoch.validate_source(token)?;
        Ok(revision)
    }
    pub(super) fn append_incarnation(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        token: &OwnedSourceToken,
        operation_id: Uuid,
        revision: u64,
    ) -> Result<()> {
        match self {
            Self::Store(source) => {
                source.append_incarnation(epoch, token, operation_id, revision)?;
            }
            Self::Authentication(source) => {
                source.append_incarnation(epoch, token, operation_id, revision)?;
            }
        }
        Ok(())
    }
    pub(super) fn retain(
        self,
        epoch: &HumanSourcesWriteEpoch,
        token: &OwnedSourceToken,
    ) -> Result<RetainedSource> {
        Ok(match self {
            Self::Store(source) => RetainedSource::Store(source.retain(epoch, token)?),
            Self::Authentication(source) => {
                RetainedSource::Authentication(source.retain(epoch, token)?)
            }
        })
    }
}
