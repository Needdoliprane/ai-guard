use super::*;

pub(super) enum LoadFailure {
    Incomplete,
    Conflict,
}
pub(super) struct LoadedPreparation {
    pub directory: PrivatePreparationDirectory,
    pub recovery: HumanSourcesPreparationRecoveryV1,
    pub recovery_file: PrivateFile,
    pub inputs: ResolvedInputs,
    pub envelope: PrivateFile,
    pub plan_file: PrivateFile,
    pub seal_file: PrivateFile,
}

pub(super) fn prepare(args: PrepareArgs) -> Result<usize> {
    ensure!(
        args.confirm_control_stopped && !args.installation_id.is_nil(),
        "human_sources_control_stop_attestation_required"
    );
    validate_absolute_path(&args.output_dir)?;
    // Refuse an existing destination before opening any SQLite. This is an
    // exclusive observation; create later still enforces O_EXCL against races.
    operator::require_new_preparation_path(&args.output_dir)?;
    let inputs = ResolvedInputs::load(
        &args.tenant_registry,
        args.oidc_config.as_deref(),
        args.control_config_revision,
        args.oidc_mapping_revision,
        args.max_events,
    )?;
    let legacy = inputs.legacy_inputs(args.installation_id, args.max_events)?;
    let (prior_sources, source_profiles) =
        operator::observe_legacy_sources(args.installation_id, legacy)?;
    let mut target_sources = prior_sources.clone();
    for source in &mut target_sources {
        source.required_format = match source.kind {
            HumanSourceKindV1::TenantStore => 14,
            HumanSourceKindV1::AuthenticationLedger => 5,
        };
    }
    inputs.budget.revalidate()?;
    let directory = PrivatePreparationDirectory::create(&args.output_dir)?;
    directory
        .child("journal", true)?
        .context("human_sources_journal_missing")?;
    directory
        .child("backups", true)?
        .context("human_sources_backups_missing")?;
    let mut backup_paths = BTreeMap::new();
    for source in &prior_sources {
        backup_paths.insert(
            source.source_id,
            operator::create_backup_parent(&directory, source.source_id)?,
        );
    }
    let mut technical_identities = Vec::new();
    for (index, binding) in inputs.bindings.iter().enumerate() {
        if let (Some(tenant), Some(role)) = (&binding.tenant_id, binding.role) {
            technical_identities.push(TechnicalExpectedIdentityV1 {
                tenant_id: tenant.clone(),
                role,
                credential_id: Uuid::new_v4(),
                generation: 1,
                input_binding_index: index,
            });
        }
    }
    technical_identities.sort_by(|a, b| (&a.tenant_id, a.role).cmp(&(&b.tenant_id, b.role)));
    let recovery = HumanSourcesPreparationRecoveryV1 {
        version: 1,
        public_digest_version: 1,
        operation_id: Uuid::new_v4(),
        installation_id: args.installation_id,
        closure_id: Uuid::new_v4(),
        closure_revision: 1,
        control_config_revision: args.control_config_revision,
        oidc_mapping_revision: args.oidc_mapping_revision,
        preparation_directory: directory.identity()?,
        tenant_registry_path: args.tenant_registry,
        oidc_configuration_path: args.oidc_config,
        max_events: args.max_events,
        input_bindings: inputs.bindings.clone(),
        technical_identities,
        prior_sources,
        target_sources,
        source_profiles,
        backup_paths,
        journal_directory: directory.path().join("journal"),
        legacy_scope_kind: LegacyScopeKindV1::ExistingLegacyInPlace,
    };
    recovery.validate(&directory)?;
    let bytes = serde_json::to_vec(&recovery)?;
    let file = directory.publish_or_verify_identical("recovery.json", &bytes, 1_048_576)?;
    checkpoint("recovery.json")?;
    let retained: HumanSourcesPreparationRecoveryV1 = decode_closed(file.bytes(), 1_048_576, 515)?;
    ensure!(retained == recovery, "human_sources_recovery_conflict");
    // The first durable document above contains every random identity. All
    // subsequent documents are derived solely from these retained bytes.
    complete(directory, recovery, file, inputs)
}

pub(super) fn resume_prepare(path: &Path) -> Result<usize> {
    let (directory, recovery, recovery_file, inputs) = load_recovery(path)?;
    operator::resync_private_file(&recovery_file)?;
    complete(directory, recovery, recovery_file, inputs)
}

fn complete(
    directory: PrivatePreparationDirectory,
    recovery: HumanSourcesPreparationRecoveryV1,
    recovery_file: PrivateFile,
    mut inputs: ResolvedInputs,
) -> Result<usize> {
    inputs.budget.retain_published(recovery_file.clone())?;
    inputs.verify_recovery(&recovery)?;
    let (expected, envelope_bytes) = inputs.envelope(&recovery)?;
    let envelope = directory.publish_or_verify_identical(
        "technical-credentials.json",
        &envelope_bytes,
        1_048_576,
    )?;
    inputs.budget.retain_published(envelope.clone())?;
    checkpoint("technical-credentials.json")?;
    validate_technical_credentials_configuration(envelope.bytes(), &expected)?;
    let (prior, target) = resolved_views(&recovery, &recovery_file, &envelope, &mut inputs)?;
    let plan_bytes = operator::prepare_maintenance_plan(
        recovery.operation_id,
        recovery.installation_id,
        &prior,
        &target,
    )?;
    let plan =
        directory.publish_or_verify_identical("maintenance-plan.json", &plan_bytes, 524_288)?;
    inputs.budget.retain_published(plan.clone())?;
    checkpoint("maintenance-plan.json")?;
    let seal = preparation_seal(&directory, &recovery, &recovery_file, &envelope, &plan)?;
    let seal_file = directory.publish_or_verify_identical(
        "preparation-complete.json",
        &serde_json::to_vec(&seal)?,
        32_768,
    )?;
    inputs.budget.retain_published(seal_file)?;
    checkpoint("preparation-complete.json")?;
    let count = recovery.target_sources.len();
    // A claimed successful publication is re-read through the same consumer.
    load_preparation(directory.path(), false)
        .map_err(|_| anyhow::anyhow!("human_sources_preparation_revalidation_failed"))?;
    Ok(count)
}

pub(super) fn checkpoint(_name: &'static str) -> Result<()> {
    #[cfg(test)]
    FAILURE_POINT.with(|point| {
        if point.get() == Some(_name) {
            point.set(None);
            bail!("human_sources_injected_after_real_publication");
        }
        Ok(())
    })?;
    Ok(())
}
#[cfg(test)]
thread_local! {pub(super) static FAILURE_POINT:std::cell::Cell<Option<&'static str>>=const{std::cell::Cell::new(None)};}

fn load_recovery(
    path: &Path,
) -> Result<(
    PrivatePreparationDirectory,
    HumanSourcesPreparationRecoveryV1,
    PrivateFile,
    ResolvedInputs,
)> {
    let directory = PrivatePreparationDirectory::open(path)?;
    directory.validate_catalogue()?;
    let mut budget = InputBudget::new();
    let file = budget
        .read(&path.join("recovery.json"), 1_048_576)
        .map_err(|_| anyhow::anyhow!("human_sources_recovery_unavailable_or_conflicting"))?;
    let recovery: HumanSourcesPreparationRecoveryV1 =
        decode_closed(file.bytes(), 1_048_576, 515)
            .map_err(|_| anyhow::anyhow!("human_sources_recovery_unavailable_or_conflicting"))?;
    recovery.validate(&directory)?;
    let inputs = ResolvedInputs::load_with_budget(
        budget,
        &recovery.tenant_registry_path,
        recovery.oidc_configuration_path.as_deref(),
        recovery.control_config_revision,
        recovery.oidc_mapping_revision,
        recovery.max_events,
    )?;
    inputs.verify_recovery(&recovery)?;
    Ok((directory, recovery, file, inputs))
}

pub(super) fn load_preparation(
    path: &Path,
    resync: bool,
) -> std::result::Result<LoadedPreparation, LoadFailure> {
    let (directory, recovery, recovery_file, mut inputs) =
        load_recovery(path).map_err(|_| LoadFailure::Conflict)?;
    // Missing deterministic members are incomplete; existing malformed members
    // are conflicts, never candidates for repair by this read-only consumer.
    let read = |budget: &mut InputBudget,
                name: &str,
                limit|
     -> std::result::Result<PrivateFile, LoadFailure> {
        if !directory
            .contains(name)
            .map_err(|_| LoadFailure::Conflict)?
        {
            return Err(LoadFailure::Incomplete);
        }
        budget
            .read(&path.join(name), limit)
            .map_err(|_| LoadFailure::Conflict)
    };
    let envelope = read(&mut inputs.budget, "technical-credentials.json", 1_048_576)?;
    let plan_file = read(&mut inputs.budget, "maintenance-plan.json", 524_288)?;
    let seal_file = read(&mut inputs.budget, "preparation-complete.json", 32_768)?;
    let verify = || -> Result<()> {
        let (expected, bytes) = inputs.envelope(&recovery)?;
        validate_technical_credentials_configuration(envelope.bytes(), &expected)?;
        ensure!(envelope.bytes() == bytes, "human_sources_envelope_conflict");
        Ok(())
    };
    verify().map_err(|_| LoadFailure::Conflict)?;
    let (prior, target) = resolved_views(&recovery, &recovery_file, &envelope, &mut inputs)
        .map_err(|_| LoadFailure::Conflict)?;
    let plan = operator::prepare_maintenance_plan(
        recovery.operation_id,
        recovery.installation_id,
        &prior,
        &target,
    )
    .map_err(|_| LoadFailure::Conflict)?;
    if plan_file.bytes() != plan {
        return Err(LoadFailure::Conflict);
    }
    let expected = preparation_seal(&directory, &recovery, &recovery_file, &envelope, &plan_file)
        .map_err(|_| LoadFailure::Conflict)?;
    let actual: PreparationSealV1 =
        decode_closed(seal_file.bytes(), 32_768, 3).map_err(|_| LoadFailure::Conflict)?;
    if actual != expected
        || serde_json::to_vec(&expected).map_err(|_| LoadFailure::Conflict)? != seal_file.bytes()
    {
        return Err(LoadFailure::Conflict);
    }
    if resync {
        for file in [&recovery_file, &envelope, &plan_file, &seal_file] {
            operator::resync_private_file(file).map_err(|_| LoadFailure::Conflict)?;
        }
    }
    Ok(LoadedPreparation {
        directory,
        recovery,
        recovery_file,
        inputs,
        envelope,
        plan_file,
        seal_file,
    })
}

pub(super) fn resolved_views(
    recovery: &HumanSourcesPreparationRecoveryV1,
    recovery_file: &PrivateFile,
    envelope: &PrivateFile,
    inputs: &mut ResolvedInputs,
) -> Result<(OperatorResolvedView, OperatorResolvedView)> {
    let (configuration, files) = inputs.configuration(recovery, recovery_file)?;
    let prior = OperatorResolvedView {
        sources: recovery.prior_sources.clone(),
        configuration,
        files,
    };
    let (configuration, files) = inputs.configuration(recovery, envelope)?;
    let target = OperatorResolvedView {
        sources: recovery.target_sources.clone(),
        configuration,
        files,
    };
    Ok((prior, target))
}
fn preparation_seal(
    directory: &PrivatePreparationDirectory,
    recovery: &HumanSourcesPreparationRecoveryV1,
    recovery_file: &PrivateFile,
    envelope: &PrivateFile,
    plan: &PrivateFile,
) -> Result<PreparationSealV1> {
    let mut members = Vec::new();
    for (name, file) in [
        ("maintenance-plan.json", plan),
        ("recovery.json", recovery_file),
        ("technical-credentials.json", envelope),
    ] {
        file.revalidate()?;
        members.push(PreparationMemberV1 {
            name: name.to_owned(),
            len: file.bytes().len() as u64,
            sha256: file.digest().to_owned(),
        });
    }
    Ok(PreparationSealV1 {
        version: 1,
        operation_id: recovery.operation_id,
        preparation_directory: directory.identity()?,
        members,
    })
}
