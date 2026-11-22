//! Offline, single-tenant activation of the existing E1 maintenance journal.
use super::{observe_private_file_binding, validate_absolute_path};
use crate::{
    api::prepare_successor_technical_credentials_configuration,
    audit::{Store, authentication::AuthenticationAudit, egress::EgressStoreLimitsV1},
    control_fs::{Directory, PrivateFile},
    human_authority::{
        HumanAuthorityClosureV1, HumanConfigurationFileKindV1, HumanSourceKindV1,
        HumanSourcesConfigurationV1, MAX_HUMAN_MANIFEST_BYTES, OwnedHumanSources,
        maintenance::{
            HumanMaintenanceActiveManifestV1, HumanMaintenanceResolvedViewV1,
            HumanMaintenanceViewV1, HumanSourcesMaintenanceConfigurationV1,
            HumanSourcesMaintenanceGuard, HumanSourcesMaintenancePlanV1, from_active_source,
        },
        migration::egress_journal::apply_egress_format15,
        startup::{HumanOwnedSourceOptions, HumanOwnedStartupRequest},
    },
};
use anyhow::{Context, Result, ensure};
use std::{ffi::OsStr, fmt, path::Path};
use uuid::Uuid;

const TECHNICAL: &str = "technical-credentials.json";
const PLAN: &str = "maintenance-plan.json";
const BACKUP: &str = "backup";
const JOURNAL: &str = "journal";
const CLOSURE: &str = "closure";
const MANIFEST: &str = "manifest.json";
const MAX_ENVELOPE_BYTES: u64 = 1_048_576;
const MAX_PLAN_BYTES: u64 = 512 * 1024;
const MAX_OUTPUT_ENTRIES: u32 = 32;
const CLOSURE_STAGE: &str = "closure-stage-";

#[cfg(test)]
thread_local! {
    pub(crate) static STOP_AFTER_STORE15_CONFIRMATION: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

pub struct Store15UpgradeRequest<'a> {
    pub prior: HumanOwnedStartupRequest,
    pub configured_credentials: &'a [(&'a str, &'a str, &'a str)],
    pub tenant: &'a str,
    pub operation_id: Uuid,
    pub output_directory: &'a Path,
    pub resume: bool,
}

/// The closure and genesis were confirmed durably. This proves no readiness.
pub struct Store15UpgradeResult {
    pub closure_revision: u64,
    pub genesis_sequence: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Store15UpgradeRefusal {
    OutputProtected,
    OutputNotFresh,
    PreparationPublicationUnconfirmed,
    TransitionUnconfirmed,
}

impl fmt::Display for Store15UpgradeRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OutputProtected => "store15_output_protected choose_a_new_directory",
            Self::OutputNotFresh => {
                "store15_output_not_fresh preserve_output use_resume_for_the_same_operation"
            }
            Self::PreparationPublicationUnconfirmed => {
                "store15_preparation_publication_unconfirmed preserve_output check_before_resume"
            }
            Self::TransitionUnconfirmed => {
                "store15_transition_unconfirmed preserve_output reconcile_and_resume_same_operation"
            }
        })
    }
}

impl std::error::Error for Store15UpgradeRefusal {}

pub fn upgrade_store15(request: Store15UpgradeRequest<'_>) -> Result<Store15UpgradeResult> {
    use Store15UpgradeRefusal as Refusal;
    validate_absolute_path(request.output_directory)?;
    ensure!(
        !request.operation_id.is_nil(),
        "Store15 operation ID is empty"
    );
    let prior = HumanAuthorityClosureV1::from_bytes(request.prior.manifest.bytes())?;
    let actual = request.prior.configuration;
    ensure!(
        actual.sources == prior.sources && actual.configuration == prior.configuration,
        "Store15 prior configuration differs"
    );
    let position = prior
        .sources
        .iter()
        .position(|source| source.tenant_id.as_deref() == Some(request.tenant))
        .context("Store15 tenant source missing")?;
    let source = &prior.sources[position];
    ensure!(
        source.kind == HumanSourceKindV1::TenantStore && source.required_format == 14,
        "Store15 requires a Store14 source in the retained closure"
    );
    let options = request
        .prior
        .sources
        .get(&source.source_id)
        .context("Store15 tenant options missing")?;
    let HumanOwnedSourceOptions::TenantStore {
        max_events,
        capacity,
        workload,
        retention,
        ..
    } = options
    else {
        anyhow::bail!("Store15 tenant options missing");
    };
    capacity.validate(*max_events)?;
    workload.validate(*max_events, capacity)?;
    retention.validate()?;
    let closure_revision = prior
        .closure_revision
        .checked_add(1)
        .filter(|revision| i64::try_from(*revision).is_ok())
        .context("Store15 closure revision exhausted")?;
    let mut target = prior.clone();
    target.closure_revision = closure_revision;
    target.sources[position].required_format = 15;

    let technical_kind = HumanConfigurationFileKindV1::TechnicalCredentials;
    let old_credentials = actual
        .files
        .get(&technical_kind)
        .context("Store15 envelope missing")?;
    let target_bytes = prepare_successor_technical_credentials_configuration(
        old_credentials.bytes(),
        &prior,
        request.configured_credentials,
    )?;
    request.prior.manifest.persistent_revalidate()?;
    let prior_manifest = HumanMaintenanceActiveManifestV1 {
        storage_binding: observe_private_file_binding(&request.prior.manifest)?,
        closure_id: prior.closure_id,
        closure_revision: prior.closure_revision,
        closure_digest: prior.closure_digest,
    };
    let prior_view = HumanMaintenanceViewV1 {
        sources: prior.sources.iter().map(from_active_source).collect(),
        configuration: prior.configuration,
    };
    let mut plan = HumanSourcesMaintenancePlanV1 {
        version: 1,
        migration_id: request.operation_id,
        installation_id: target.installation_id,
        prior: Some(prior_view.clone()),
        target: HumanMaintenanceViewV1 {
            sources: target.sources.iter().map(from_active_source).collect(),
            configuration: target.configuration.clone(),
        },
        prior_active_manifest: Some(prior_manifest),
        supported_storage_profile: target.supported_storage_profile,
        selected_source_id: None,
    };
    let parent = Directory::open_existing(
        request
            .output_directory
            .parent()
            .context("Store15 output parent missing")?,
    )?;
    let name = request
        .output_directory
        .file_name()
        .and_then(OsStr::to_str)
        .context("Store15 output name invalid")?;
    // The initial activation's closed catalogue holds its technical envelope.
    // With explicit references, the prior manifest/envelope may also be seals
    // of another operation. Preserve their directories in both cases.
    let protected_directories = prior_view
        .sources
        .iter()
        .map(|source| source.storage_binding.parent_path.as_path())
        .chain([
            retained_parent(&request.prior.manifest)?,
            retained_parent(old_credentials)?,
        ])
        .map(Directory::open_existing)
        .collect::<Result<Vec<_>>>()?;
    for protected in &protected_directories {
        ensure!(
            !parent.is_within_directory(protected)?,
            Refusal::OutputProtected
        );
    }
    if !request.resume {
        parent
            .require_absent_child(name)
            .map_err(|_| Refusal::OutputNotFresh)?;
        // This owner takes the real prior authority locks without opening SQLite.
        // After metadata publication, maintenance takes them again over the
        // complete prior/target union before any backup or mutation.
        let prior_owner = OwnedHumanSources::acquire(
            request.prior.manifest.clone(),
            HumanSourcesConfigurationV1::new(
                actual.sources.clone(),
                actual.configuration.clone(),
                actual
                    .files
                    .iter()
                    .map(|(kind, file)| (*kind, file.clone()))
                    .collect(),
            )?,
        )?;
        let stage = parent.exclusive_child(&format!("store15-stage-{}", Uuid::new_v4()))?;
        parent.require_same_mount_child(&stage)?;
        let credentials = stage.persistent_create(TECHNICAL, &target_bytes)?;
        credentials.persistent_sync()?;
        let mut binding = observe_private_file_binding(&credentials)?;
        binding.parent_path = request.output_directory.to_owned();
        update_envelope_binding(&mut plan.target, binding)?;
        let plan_bytes = serde_json::to_vec(&plan)?;
        ensure!(
            plan_bytes.len() as u64 <= MAX_PLAN_BYTES,
            "Store15 plan too large"
        );
        stage
            .persistent_create(PLAN, &plan_bytes)?
            .persistent_sync()?;
        stage.exclusive_child(JOURNAL)?;
        stage.exclusive_child(BACKUP)?;
        parent
            .publish_child_exclusive(&stage, name)
            .map_err(|_| Refusal::PreparationPublicationUnconfirmed)?;
        drop(prior_owner);
    }
    let output = parent.open_existing_child(name)?;
    parent.require_same_mount_child(&output)?;
    for protected in &protected_directories {
        ensure!(
            !protected.is_within_directory(&output)?,
            Refusal::OutputProtected
        );
    }
    for file in actual.files.values() {
        ensure!(
            !file.is_within_directory(output.path())?,
            Refusal::OutputProtected
        );
    }
    output.persistent_entries(MAX_OUTPUT_ENTRIES)?;
    let envelope = read_exact(&output, TECHNICAL, &target_bytes, MAX_ENVELOPE_BYTES)?;
    let binding = observe_private_file_binding(&envelope)?;
    update_envelope_binding(&mut plan.target, binding)?;
    let plan_file = read_exact(&output, PLAN, &serde_json::to_vec(&plan)?, MAX_PLAN_BYTES)?;
    let mut files: Vec<_> = actual.files.into_iter().collect();
    let prior_resolved = HumanMaintenanceResolvedViewV1::new(
        prior_view.sources,
        prior_view.configuration,
        files.clone(),
    )?;
    let (_, file) = files
        .iter_mut()
        .find(|(kind, _)| *kind == technical_kind)
        .context("Store15 target envelope missing")?;
    *file = envelope;
    let target_resolved = HumanMaintenanceResolvedViewV1::new(
        plan.target.sources,
        plan.target.configuration.clone(),
        files,
    )?;
    target.configuration = plan.target.configuration;
    target.closure_digest = target.compute_digest()?;
    let manifest_bytes = serde_json::to_vec(&target)?;
    ensure!(
        HumanAuthorityClosureV1::from_bytes(&manifest_bytes)? == target,
        "Store15 closure encoding differs"
    );
    let guard = HumanSourcesMaintenanceGuard::acquire(
        plan_file,
        HumanSourcesMaintenanceConfigurationV1::new(
            Some(prior_resolved),
            target_resolved,
            Some(request.prior.manifest),
        ),
    )?;
    let token = guard.target_source(source.source_id)?;
    for neighbor in prior
        .sources
        .iter()
        .filter(|neighbor| neighbor.source_id != source.source_id)
    {
        let neighbor_token = guard.target_source(neighbor.source_id)?;
        match (
            neighbor.kind,
            request.prior.sources.get(&neighbor.source_id),
        ) {
            (HumanSourceKindV1::TenantStore, Some(HumanOwnedSourceOptions::TenantStore { .. })) => {
                Store::observe_unchanged_review_store(&neighbor_token)?;
            }
            (
                HumanSourceKindV1::AuthenticationLedger,
                Some(HumanOwnedSourceOptions::AuthenticationLedger {
                    policy,
                    capacity_plan,
                    redactor,
                }),
            ) => {
                AuthenticationAudit::verify_unchanged_review_format5(
                    &neighbor_token,
                    policy,
                    capacity_plan,
                    redactor,
                )?;
            }
            _ => anyhow::bail!("Store15 neighbor options differ"),
        }
    }
    let journal = output.open_existing_child(JOURNAL)?;
    let backup = output.open_existing_child(BACKUP)?;
    output.require_same_mount_child(&journal)?;
    output.require_same_mount_child(&backup)?;
    let outcome = (|| -> Result<_> {
        let published = output.persistent_child(CLOSURE, false)?;
        if let Some(existing) = &published {
            output.require_same_mount_child(existing)?;
            read_exact(
                existing,
                MANIFEST,
                &manifest_bytes,
                MAX_HUMAN_MANIFEST_BYTES as u64,
            )?;
        } else {
            output.persistent_entries(MAX_OUTPUT_ENTRIES - 1)?;
        }
        let receipt = apply_egress_format15(
            &token,
            options,
            journal,
            backup,
            &EgressStoreLimitsV1::default(),
            published.is_some(),
        )?;
        guard.check()?;
        #[cfg(test)]
        if STOP_AFTER_STORE15_CONFIRMATION.with(|stop| stop.replace(false)) {
            anyhow::bail!("fixture interrupted after E1 confirmation");
        }
        if published.is_none() {
            let stage = output.exclusive_child(&format!("{CLOSURE_STAGE}{}", Uuid::new_v4()))?;
            stage
                .persistent_create(MANIFEST, &manifest_bytes)?
                .persistent_sync()?;
            output.require_same_mount_child(&stage)?;
            guard.check()?;
            output
                .publish_child_exclusive(&stage, CLOSURE)
                .map_err(|_| anyhow::anyhow!("Store15 closure publication unconfirmed"))?;
        }
        let closure = output.open_existing_child(CLOSURE)?;
        output.require_same_mount_child(&closure)?;
        read_exact(
            &closure,
            MANIFEST,
            &manifest_bytes,
            MAX_HUMAN_MANIFEST_BYTES as u64,
        )?;
        output.sync()?;
        parent.sync()?;
        guard.check()?;
        Ok(Store15UpgradeResult {
            closure_revision,
            genesis_sequence: receipt.seq,
        })
    })();
    outcome.context(Refusal::TransitionUnconfirmed)
}

fn retained_parent(file: &PrivateFile) -> Result<&Path> {
    file.persistent_revalidate()?;
    file.path()
        .parent()
        .context("Store15 retained file parent missing")
}

fn update_envelope_binding(
    view: &mut HumanMaintenanceViewV1,
    binding: crate::human_authority::SourceStorageBindingV1,
) -> Result<()> {
    let file = view
        .configuration
        .files
        .iter_mut()
        .find(|file| file.kind == HumanConfigurationFileKindV1::TechnicalCredentials)
        .context("Store15 missing configuration envelope")?;
    file.storage_binding = binding;
    Ok(())
}

fn read_exact(
    directory: &Directory,
    name: &str,
    expected: &[u8],
    max_bytes: u64,
) -> Result<PrivateFile> {
    directory.private_entry_same_mount(OsStr::new(name))?;
    let file = directory
        .persistent_read(name, max_bytes, &[])?
        .context("Store15 publication missing")?;
    ensure!(file.bytes() == expected, "Store15 publication differs");
    file.persistent_sync()?;
    directory.sync()?;
    file.persistent_revalidate()?;
    Ok(file)
}
