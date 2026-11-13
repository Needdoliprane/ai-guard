use super::*;
use agentsandbox::human_authority::{
    HumanSourcesConfigurationV1,
    startup::{HumanOwnedSourceOptions, HumanOwnedStartupRequest},
};
use preparation::LoadedPreparation;

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
