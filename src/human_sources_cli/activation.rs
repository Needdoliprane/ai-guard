use super::*;
use crate::gateway_config::{
    ConfiguredTenant,
    human_sources::{ResolvedOwnedSources, resolve_owned_sources},
    load_owned_tenants,
};
use agentsandbox::human_authority::{
    HumanSourcesConfigurationV1,
    startup::{HumanOwnedSourceOptions, HumanOwnedStartupRequest},
};
use preparation::LoadedPreparation;

pub(super) const DEFAULT_MAX_EVENTS: u64 = 100_000;

/// Explicit references of an already published owned-source closure.
#[derive(Args)]
pub(super) struct ExplicitOwnedSourcesArgs {
    /// Existing private closure manifest.
    #[arg(
        long,
        requires_all = [
            "human_credentials_envelope",
            "tenants_file",
            "human_control_config_revision",
        ]
    )]
    pub(super) human_sources_manifest: Option<PathBuf>,
    /// Existing private technical credential envelope of that manifest.
    #[arg(long, requires = "human_sources_manifest")]
    pub(super) human_credentials_envelope: Option<PathBuf>,
    /// Owned tenant registry of that manifest.
    #[arg(long, requires = "human_sources_manifest")]
    pub(super) tenants_file: Option<PathBuf>,
    /// Explicit positive revision of the retained public control configuration.
    #[arg(long, requires = "human_sources_manifest")]
    pub(super) human_control_config_revision: Option<u64>,
    /// Private OIDC configuration of that manifest, when OIDC is selected.
    #[arg(long, requires_all = ["human_sources_manifest", "human_oidc_mapping_revision"])]
    pub(super) oidc_config: Option<PathBuf>,
    /// Explicit positive OIDC mapping closure revision.
    #[arg(long, requires = "oidc_config")]
    pub(super) human_oidc_mapping_revision: Option<u64>,
    /// Audit event bound of the explicit route (default 100000).
    #[arg(long, requires = "human_sources_manifest")]
    pub(super) max_events: Option<u64>,
}

/// Configured technical tokens used by the redaction dictionary and bearer checks.
pub(super) fn technical_tokens(tenants: &[ConfiguredTenant]) -> Vec<String> {
    tenants
        .iter()
        .flat_map(|tenant| [tenant.runner_token.clone(), tenant.viewer_token.clone()])
        .collect()
}

impl ExplicitOwnedSourcesArgs {
    /// Resolve and check the explicit files without acquiring a startup owner.
    pub(super) fn resolve_for_maintenance(&self) -> Result<(ResolvedOwnedSources, Redactor, u64)> {
        let (Some(manifest), Some(envelope), Some(tenants_file), Some(revision)) = (
            self.human_sources_manifest.as_deref(),
            self.human_credentials_envelope.as_deref(),
            self.tenants_file.as_deref(),
            self.human_control_config_revision,
        ) else {
            bail!("owned_sources_options_incomplete");
        };
        let tenants = load_owned_tenants(tenants_file)?;
        let redactor = Redactor::new(technical_tokens(&tenants));
        let max_events = self.max_events.unwrap_or(DEFAULT_MAX_EVENTS);
        let resolved = resolve_owned_sources(
            manifest,
            envelope,
            tenants_file,
            self.oidc_config.as_deref(),
            &tenants,
            max_events,
            revision,
            self.human_oidc_mapping_revision,
            redactor.clone(),
        )?;
        resolved.validate_for_maintenance(&tenants)?;
        Ok((resolved, redactor, max_events))
    }
}

#[cfg(test)]
type FinalAckResyncHook = Box<dyn FnOnce()>;
#[cfg(test)]
thread_local! {
    pub(super) static BEFORE_FINAL_ACK_RESYNC: std::cell::RefCell<Option<FinalAckResyncHook>>=const{std::cell::RefCell::new(None)};
}

#[derive(Debug)]
pub(super) struct ActivationIncomplete;
impl std::fmt::Display for ActivationIncomplete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("human_sources_activation_incomplete")
    }
}
impl std::error::Error for ActivationIncomplete {}

pub(super) fn expected_activation(loaded: &LoadedPreparation) -> HumanSourcesActivationV1 {
    let r = &loaded.recovery;
    HumanSourcesActivationV1 {
        version: 1,
        public_digest_version: 1,
        operation_id: r.operation_id,
        preparation_directory: r.preparation_directory.clone(),
        preparation_seal: "preparation-complete.json".to_owned(),
        manifest: r
            .journal_directory
            .join(format!("migration-{}-closure.json", r.operation_id)),
        tenant_registry_path: r.tenant_registry_path.clone(),
        technical_credentials_envelope: "technical-credentials.json".to_owned(),
        oidc_configuration_path: r.oidc_configuration_path.clone(),
        control_config_revision: r.control_config_revision,
        oidc_mapping_revision: r.oidc_mapping_revision,
        max_events: r.max_events,
        human_source_mode: OwnedSourceModeV1::OwnedSources,
    }
}

fn expected_seal(
    loaded: &LoadedPreparation,
    activation: &PrivateFile,
    manifest: &PrivateFile,
) -> ActivationSealV1 {
    ActivationSealV1 {
        version: 1,
        operation_id: loaded.recovery.operation_id,
        preparation_seal_sha256: loaded.seal_file.digest().to_owned(),
        members: vec![
            ActivationMemberV1 {
                kind: ActivationMemberKindV1::Activation,
                path: activation.path().to_owned(),
                len: activation.bytes().len() as u64,
                sha256: activation.digest().to_owned(),
            },
            ActivationMemberV1 {
                kind: ActivationMemberKindV1::Manifest,
                path: manifest.path().to_owned(),
                len: manifest.bytes().len() as u64,
                sha256: manifest.digest().to_owned(),
            },
        ],
    }
}

/// Files only, including the published manifest bytes. The marker receipts in
/// that document remain expectations; this path opens no SQLite or owner.
pub(super) fn validate_activation(
    loaded: &mut LoadedPreparation,
) -> Result<Option<(PrivateFile, PrivateFile, PrivateFile)>> {
    let present = loaded.directory.contains("activation.json")?;
    let sealed = loaded.directory.contains("activation-complete.json")?;
    if !present && !sealed {
        return Ok(None);
    }
    ensure!(present, "human_sources_activation_conflict");
    let activation = loaded
        .inputs
        .budget
        .read(&loaded.directory.path().join("activation.json"), 32_768)?;
    let actual: HumanSourcesActivationV1 = decode_closed(activation.bytes(), 32_768, 515)?;
    let expected = expected_activation(loaded);
    ensure!(
        actual == expected && serde_json::to_vec(&expected)? == activation.bytes(),
        "human_sources_activation_conflict"
    );
    if !sealed {
        return Err(ActivationIncomplete.into());
    }
    let seal = loaded.inputs.budget.read(
        &loaded.directory.path().join("activation-complete.json"),
        32_768,
    )?;
    let manifest = loaded.inputs.budget.read(&expected.manifest, 262_144)?;
    validate_manifest(loaded, &manifest)?;
    let actual: ActivationSealV1 = decode_closed(seal.bytes(), 32_768, 2)?;
    let expected = expected_seal(loaded, &activation, &manifest);
    ensure!(
        actual == expected && serde_json::to_vec(&expected)? == seal.bytes(),
        "human_sources_activation_seal_conflict"
    );
    loaded.inputs.budget.revalidate()?;
    Ok(Some((activation, seal, manifest)))
}

fn validate_manifest(
    loaded: &mut LoadedPreparation,
    manifest: &PrivateFile,
) -> Result<HumanAuthorityClosureV1> {
    // The existing parser validates the complete closure digest, closed formats,
    // receipts and identities; these still gain authority only at real startup.
    let _: serde_json::Value = decode_closed(manifest.bytes(), 262_144, 256)?;
    let closure = HumanAuthorityClosureV1::from_bytes(manifest.bytes())?;
    let r = &loaded.recovery;
    ensure!(
        closure.installation_id == r.installation_id
            && closure.closure_id == r.closure_id
            && closure.closure_revision == r.closure_revision
            && closure.sources.len() == r.target_sources.len(),
        "human_sources_manifest_identity_conflict"
    );
    for (actual, expected) in closure.sources.iter().zip(&r.target_sources) {
        ensure!(
            actual.source_id == expected.source_id
                && actual.kind == expected.kind
                && actual.installation_id == expected.installation_id
                && actual.durable_namespace_id == expected.durable_namespace_id
                && actual.tenant_id == expected.tenant_id
                && actual.storage_binding == expected.storage_binding
                && actual.required_format == expected.required_format,
            "human_sources_manifest_union_conflict"
        );
    }
    let (configuration, _) = loaded.inputs.configuration(r, &loaded.envelope)?;
    ensure!(
        closure.configuration == configuration,
        "human_sources_manifest_configuration_conflict"
    );
    Ok(closure)
}

pub(super) async fn apply(path: &Path, resume_only: bool) -> Result<(&'static str, usize)> {
    let mut loaded = preparation::load_preparation(path, true)
        .map_err(|_| anyhow::anyhow!("human_sources_preparation_incomplete_or_conflicting"))?;
    if loaded.directory.contains("activation-complete.json")? {
        let _ = validate_activation(&mut loaded)?.context("human_sources_activation_conflict")?;
        #[cfg(test)]
        BEFORE_FINAL_ACK_RESYNC.with(|hook| {
            if let Some(work) = hook.borrow_mut().take() {
                work();
            }
        });
        loaded.inputs.budget.resync_all()?;
        return Ok((
            "already_published_files",
            loaded.recovery.target_sources.len(),
        ));
    }
    // Partial activation bytes must already be deterministic and complete. They
    // cannot bypass this invocation's coordinator/startup/close obligations.
    if loaded.directory.contains("activation-complete.json")? {
        bail!("human_sources_activation_seal_conflict");
    }
    let expected = expected_activation(&loaded);
    if loaded.directory.contains("activation.json")? {
        let file = loaded
            .inputs
            .budget
            .read(&path.join("activation.json"), 32_768)?;
        ensure!(
            file.bytes() == serde_json::to_vec(&expected)?,
            "human_sources_activation_conflict"
        );
    }
    let journal = loaded
        .directory
        .child("journal", false)?
        .context("human_sources_journal_missing")?;
    ensure!(
        !resume_only || operator::migration_intent_present(&journal, loaded.recovery.operation_id)?,
        "human_sources_migration_not_started"
    );
    let (prior, target) = preparation::resolved_views(
        &loaded.recovery,
        &loaded.recovery_file,
        &loaded.envelope,
        &mut loaded.inputs,
    )?;
    let sources = loaded
        .recovery
        .target_sources
        .iter()
        .map(|source| {
            Ok((
                source.source_id,
                loaded
                    .inputs
                    .options_for(source, loaded.recovery.max_events)?,
            ))
        })
        .collect::<Result<_>>()?;
    let manifest = operator::apply_and_close(OperatorMigrationRequest {
        plan_file: loaded.plan_file.clone(),
        prior,
        target,
        sources,
        backup_paths: loaded.recovery.backup_paths.clone(),
        journal_directory: journal,
        closure_id: loaded.recovery.closure_id,
        closure_revision: loaded.recovery.closure_revision,
    })
    .await?;
    loaded.inputs.budget.retain_published(manifest.clone())?;
    preparation::checkpoint("manifest")?;
    ensure!(
        manifest.path() == expected.manifest,
        "human_sources_manifest_path_conflict"
    );
    validate_manifest(&mut loaded, &manifest)?;
    let activation = loaded.directory.publish_or_verify_identical(
        "activation.json",
        &serde_json::to_vec(&expected)?,
        32_768,
    )?;
    loaded.inputs.budget.retain_published(activation.clone())?;
    preparation::checkpoint("activation.json")?;
    let seal = expected_seal(&loaded, &activation, &manifest);
    let seal_file = loaded.directory.publish_or_verify_identical(
        "activation-complete.json",
        &serde_json::to_vec(&seal)?,
        32_768,
    )?;
    loaded.inputs.budget.retain_published(seal_file)?;
    preparation::checkpoint("activation-complete.json")?;
    validate_activation(&mut loaded)?.context("human_sources_activation_incomplete")?;
    Ok((
        "activation_files_valid",
        loaded.recovery.target_sources.len(),
    ))
}

pub(crate) struct LoadedHumanSourcesActivation {
    pub tenants: Vec<crate::gateway_config::ConfiguredTenant>,
    pub request: HumanOwnedStartupRequest,
    pub credentials: PrivateFile,
}

pub(crate) fn load_human_sources_activation(path: &Path) -> Result<LoadedHumanSourcesActivation> {
    let result = (|| -> Result<_> {
        validate_absolute_path(path)?;
        ensure!(
            path.file_name().and_then(|n| n.to_str()) == Some("activation.json"),
            "human_sources_activation_leaf_invalid"
        );
        let directory = path
            .parent()
            .context("human_sources_activation_parent_missing")?;
        let mut loaded = preparation::load_preparation(directory, false)
            .map_err(|_| anyhow::anyhow!("human_sources_preparation_invalid"))?;
        let (_, _, manifest) =
            validate_activation(&mut loaded)?.context("human_sources_activation_incomplete")?;
        let closure = validate_manifest(&mut loaded, &manifest)?;
        let (configuration, files) = loaded
            .inputs
            .configuration(&loaded.recovery, &loaded.envelope)?;
        let configuration =
            HumanSourcesConfigurationV1::new(closure.sources, configuration, files)?;
        let sources = loaded
            .recovery
            .target_sources
            .iter()
            .map(|source| {
                let options = match loaded
                    .inputs
                    .options_for(source, loaded.recovery.max_events)?
                {
                    OperatorSourceOptions::TenantStore {
                        redactor,
                        max_events,
                        capacity,
                        workload,
                        retention,
                    } => HumanOwnedSourceOptions::TenantStore {
                        redactor,
                        max_events,
                        capacity,
                        workload,
                        retention,
                    },
                    OperatorSourceOptions::AuthenticationLedger {
                        redactor,
                        policy,
                        capacity_plan,
                    } => HumanOwnedSourceOptions::AuthenticationLedger {
                        redactor,
                        policy,
                        capacity_plan,
                    },
                };
                Ok((source.source_id, options))
            })
            .collect::<Result<_>>()?;
        // Only independently reconstructed expectations and retained files
        // escape this loader. Root opens the real startup after all other
        // fallible configuration, immediately before authentic state/cleanup.
        loaded.inputs.budget.revalidate()?;
        let request = HumanOwnedStartupRequest {
            manifest,
            configuration,
            sources,
        };
        Ok(LoadedHumanSourcesActivation {
            tenants: loaded.inputs.tenants,
            request,
            credentials: loaded.envelope,
        })
    })();
    result.map_err(|_: anyhow::Error| {
        anyhow::anyhow!(
            "human_sources_activation_refused; preserve_all_files; check_or_resume_same_operation"
        )
    })
}

/// Inputs of the in-place identity-marker maintenance. Files and metadata;
/// opening these inputs never acquires an owner or opens SQLite.
pub(super) struct LoadedIdentityMarkerInputs {
    pub manifest: PrivateFile,
    pub configuration: HumanAuthorityConfigurationV1,
    pub files: Vec<(HumanConfigurationFileKindV1, PrivateFile)>,
    pub excluded_directories: Vec<PathBuf>,
    pub redactor: Redactor,
    pub max_events: u64,
}

pub(super) fn load_identity_marker_inputs(
    path: &Path,
    tenant_id: &str,
) -> Result<LoadedIdentityMarkerInputs> {
    validate_absolute_path(path)?;
    ensure!(
        path.file_name().and_then(|n| n.to_str()) == Some("activation.json"),
        "human_sources_activation_leaf_invalid"
    );
    let directory = path
        .parent()
        .context("human_sources_activation_parent_missing")?;
    let mut loaded = preparation::load_preparation(directory, false)
        .map_err(|_| anyhow::anyhow!("human_sources_preparation_invalid"))?;
    let (_, _, manifest) =
        validate_activation(&mut loaded)?.context("human_sources_activation_incomplete")?;
    validate_manifest(&mut loaded, &manifest)?;
    let (configuration, files) = loaded
        .inputs
        .configuration(&loaded.recovery, &loaded.envelope)?;
    let source = loaded
        .recovery
        .target_sources
        .iter()
        .find(|source| {
            source.kind == HumanSourceKindV1::TenantStore
                && source.tenant_id.as_deref() == Some(tenant_id)
        })
        .context("human_rights_source_unknown")?;
    let OperatorSourceOptions::TenantStore {
        redactor,
        max_events,
        ..
    } = loaded
        .inputs
        .options_for(source, loaded.recovery.max_events)?
    else {
        bail!("human_rights_source_unknown");
    };
    loaded.inputs.budget.revalidate()?;
    Ok(LoadedIdentityMarkerInputs {
        manifest,
        configuration,
        files,
        excluded_directories: vec![loaded.directory.path().to_owned()],
        redactor,
        max_events,
    })
}

pub(super) fn load_explicit_identity_marker_inputs(
    args: &ExplicitOwnedSourcesArgs,
) -> Result<LoadedIdentityMarkerInputs> {
    let (resolved, redactor, max_events) = args.resolve_for_maintenance()?;
    let mut excluded_directories = std::iter::once(&resolved.manifest)
        .chain(resolved.files.iter().map(|(_, file)| file))
        .chain(&resolved.token_files)
        .map(|file| {
            file.revalidate()?;
            file.path()
                .parent()
                .map(Path::to_owned)
                .context("owned source reference parent missing")
        })
        .collect::<Result<Vec<_>>>()?;
    excluded_directories.sort();
    excluded_directories.dedup();
    Ok(LoadedIdentityMarkerInputs {
        manifest: resolved.manifest,
        configuration: resolved.configuration,
        files: resolved.files,
        excluded_directories,
        redactor,
        max_events,
    })
}
