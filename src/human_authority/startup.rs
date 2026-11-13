//! Reopen one complete, already provisioned human source union.
//! Configuration is expected input; actual retained source APIs prove startup.
use super::*;
use crate::{
    audit::{
        AuditCapacityPolicyV1, ReviewStoreStartup, Store, WorkloadAuditPolicyV1,
        authentication::{
            AuthenticationAudit, AuthenticationAuditPolicyV1, AuthenticationCapacityPlanV1,
            AuthenticationReviewStartup,
        },
    },
    redaction::Redactor,
};
use anyhow::{Result, ensure};

/// A metadata snapshot for expected configuration only. It owns no lock or
/// SQLite connection and cannot authenticate a source across this TOCTOU gap.
/// acquire/startup must revalidate the real retained filesystem and source.
pub fn observe_human_storage_binding(path: &std::path::Path) -> Result<SourceStorageBindingV1> {
    let path = crate::control_fs::absolute_path(path)?;
    let parent_path = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("human binding parent absent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("human binding name absent"))?;
    let parent = Directory::open_existing(parent_path)?;
    let (parent_device, parent_inode) = parent.physical_identity()?;
    let (source_device, source_inode) = parent
        .private_entry(name)?
        .ok_or_else(|| anyhow::anyhow!("human binding source absent"))?
        .physical_identity();
    parent.revalidate()?;
    Ok(SourceStorageBindingV1 {
        parent_path: parent_path.to_owned(),
        file_name: name
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("human binding filename invalid"))?
            .to_owned(),
        parent_device,
        parent_inode,
        source_device,
        source_inode,
    })
}

/// Resolved from the retained control configuration by the operator loader.
/// These values select limits, never request identity or permission.
pub enum HumanOwnedSourceOptions {
    TenantStore {
        redactor: Redactor,
        max_events: u64,
        capacity: AuditCapacityPolicyV1,
        workload: WorkloadAuditPolicyV1,
        retention: crate::relay::RelayRetentionPolicyV1,
    },
    AuthenticationLedger {
        redactor: Redactor,
        policy: AuthenticationAuditPolicyV1,
        capacity_plan: AuthenticationCapacityPlanV1,
    },
}

pub struct HumanOwnedStartupRequest {
    pub manifest: PrivateFile,
    pub configuration: HumanSourcesConfigurationV1,
    pub sources: BTreeMap<Uuid, HumanOwnedSourceOptions>,
}

/// No public owner/Store getter: only the adopted gateway constructor consumes
/// this result. An intermediate source is never published after partial CAS.
pub struct HumanOwnedStartupBundle {
    pub(crate) owner: OwnedHumanSources,
    pub(crate) stores: BTreeMap<Uuid, Store>,
    pub(crate) authentication: Option<AuthenticationAudit>,
    pub(crate) transferred: bool,
}

enum Opening {
    Store(ReviewStoreStartup),
    Authentication(AuthenticationReviewStartup),
}

impl HumanOwnedStartupBundle {
    pub fn open(request: HumanOwnedStartupRequest) -> Result<Self> {
        // Check exact source set and resolve all policy constraints before
        // SQLite. The owner retains manifest/configuration/physical locks.
        let expected: BTreeSet<_> = request
            .configuration
            .sources
            .iter()
            .map(|source| source.source_id)
            .collect();
        ensure!(
            expected == request.sources.keys().copied().collect(),
            "human startup source union differs"
        );
        for options in request.sources.values() {
            if let HumanOwnedSourceOptions::TenantStore {
                max_events,
                capacity,
                workload,
                retention,
                ..
            } = options
            {
                capacity.validate(*max_events)?;
                workload.validate(*max_events, capacity)?;
                retention.validate()?;
            } else if let HumanOwnedSourceOptions::AuthenticationLedger { capacity_plan, .. } =
                options
            {
                capacity_plan.validate()?;
            }
        }
        let owner = OwnedHumanSources::acquire(request.manifest, request.configuration)?;
        let result = Self::open_owned(owner.clone(), request.sources);
        if result.is_err() {
            owner.begin_close();
        }
        result
    }

    fn open_owned(
        owner: OwnedHumanSources,
        mut options: BTreeMap<Uuid, HumanOwnedSourceOptions>,
    ) -> Result<Self> {
        let mut opened = Vec::new();
        for descriptor in &owner.closure().sources {
            let source = owner.source_token(descriptor.source_id)?;
            let configuration = options
                .remove(&descriptor.source_id)
                .ok_or_else(|| anyhow::anyhow!("source options absent"))?;
            let mut opening = match (&configuration, descriptor.kind) {
                (
                    HumanOwnedSourceOptions::TenantStore {
                        redactor,
                        max_events,
                        ..
                    },
                    HumanSourceKindV1::TenantStore,
                ) => Opening::Store(Store::open_review_startup_for_tenant(
                    &descriptor.storage_binding.path(),
                    descriptor
                        .tenant_id
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("source tenant absent"))?,
                    redactor.clone(),
                    *max_events,
                    &source,
                )?),
                (
                    HumanOwnedSourceOptions::AuthenticationLedger {
                        redactor, policy, ..
                    },
                    HumanSourceKindV1::AuthenticationLedger,
                ) => Opening::Authentication(AuthenticationAudit::open_review_startup(
                    &descriptor.storage_binding.path(),
                    descriptor.installation_id,
                    policy.clone(),
                    redactor.clone(),
                    &source,
                )?),
                _ => anyhow::bail!("source options kind differs"),
            };
            if let (
                Opening::Authentication(audit),
                HumanOwnedSourceOptions::AuthenticationLedger { capacity_plan, .. },
            ) = (&opening, &configuration)
            {
                audit.require_capacity_plan(capacity_plan)?;
            }
            if let (
                Opening::Store(store),
                HumanOwnedSourceOptions::TenantStore {
                    capacity,
                    workload,
                    retention,
                    ..
                },
            ) = (&mut opening, &configuration)
            {
                store.configure_owned_read_policies(
                    capacity.clone(),
                    workload.clone(),
                    retention.clone(),
                )?;
            }
            let floor = match &opening {
                Opening::Store(store) => store.durable_wall_floor()?,
                Opening::Authentication(audit) => audit.durable_wall_floor()?,
            };
            opened.push((source, opening, configuration, floor));
        }
        let maximum = opened
            .iter()
            .map(|(_, _, _, floor)| *floor)
            .max()
            .ok_or_else(|| anyhow::anyhow!("empty human source union"))?;
        ensure!(
            chrono::Utc::now() >= maximum,
            "human startup clock below durable floor"
        );
        let mut stores = BTreeMap::new();
        let mut authentication = None;
        for (source, opening, configuration, _) in opened {
            let epoch = owner.try_write_epoch()?;
            match (opening, configuration) {
                (Opening::Store(store), HumanOwnedSourceOptions::TenantStore { .. }) => {
                    let revision = store.current_incarnation_revision(&epoch)?;
                    store.append_incarnation(&epoch, &source, Uuid::new_v4(), revision)?;
                    let store = store.retain(&epoch, &source)?;
                    stores.insert(source.source_id(), store);
                }
                (
                    Opening::Authentication(audit),
                    HumanOwnedSourceOptions::AuthenticationLedger { .. },
                ) => {
                    let revision = audit.current_incarnation_revision(&epoch, &source)?;
                    audit.append_incarnation(&epoch, &source, Uuid::new_v4(), revision)?;
                    ensure!(authentication.is_none(), "duplicate authentication source");
                    authentication = Some(audit.retain(&epoch, &source)?);
                }
                _ => anyhow::bail!("source options kind differs"),
            }
            epoch.validate_source(&source)?;
        }
        for source in &owner.closure().sources {
            owner.source_token(source.source_id)?;
        }
        Ok(Self {
            owner,
            stores,
            authentication,
            transferred: false,
        })
    }
}

impl Drop for HumanOwnedStartupBundle {
    fn drop(&mut self) {
        if !self.transferred {
            self.owner.begin_close();
        }
    }
}

#[cfg(test)]
mod tests;
