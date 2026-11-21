use super::*;
use crate::gateway_config::human_sources::{
    ResolvedHumanOidcPublicV1, public_control_digest_v1, public_oidc_digest_v1, resolved_control,
    resolved_oidc,
};
use crate::gateway_config::{ConfiguredTenant, PreparedOidcFile};

pub(super) struct ResolvedInputs {
    pub budget: InputBudget,
    pub tenants: Vec<ConfiguredTenant>,
    pub oidc: Option<PreparedOidcFile>,
    pub bindings: Vec<PrivateInputBindingV1>,
    pub public_digest: String,
    pub oidc_public: Option<ResolvedHumanOidcPublicV1>,
}

impl ResolvedInputs {
    pub fn load(
        registry: &Path,
        oidc: Option<&Path>,
        revision: u64,
        mapping_revision: Option<u64>,
        max_events: u64,
    ) -> Result<Self> {
        Self::load_with_budget(
            InputBudget::new(),
            registry,
            oidc,
            revision,
            mapping_revision,
            max_events,
        )
    }
    pub fn load_with_budget(
        mut budget: InputBudget,
        registry: &Path,
        oidc: Option<&Path>,
        revision: u64,
        mapping_revision: Option<u64>,
        max_events: u64,
    ) -> Result<Self> {
        ensure!(
            revision > 0
                && (1..=1_000_000).contains(&max_events)
                && oidc.is_some() == mapping_revision.is_some()
                && mapping_revision.is_none_or(|v| v > 0),
            "human_sources_configuration_invalid"
        );
        let registry_file = budget.read(registry, 1_048_576)?;
        let mut bindings = vec![input_binding(
            PrivateInputKindV1::TenantRegistry,
            None,
            None,
            &registry_file,
        )?];
        let tenants = crate::gateway_config::load_from_retained_registry(&registry_file, |path| {
            let token_file = budget.read(path, 1024)?;
            operator::parse_private_token_file(token_file.bytes())
        })?;
        ensure!(
            !tenants.is_empty() && tenants.len() <= 256 - usize::from(oidc.is_some()),
            "human_sources_source_bound"
        );
        crate::gateway_config::validate_policies(&tenants, max_events)?;
        for tenant in &tenants {
            for (kind, role, path) in [
                (
                    PrivateInputKindV1::RunnerToken,
                    TechnicalCredentialRoleV1::Runner,
                    &tenant.runner_token_file,
                ),
                (
                    PrivateInputKindV1::ViewerToken,
                    TechnicalCredentialRoleV1::Viewer,
                    &tenant.viewer_token_file,
                ),
            ] {
                let file = budget.read(path, 1024)?;
                bindings.push(input_binding(
                    kind,
                    Some(tenant.tenant_id.clone()),
                    Some(role),
                    &file,
                )?);
            }
        }
        let oidc = oidc
            .map(|path| -> Result<_> {
                let file = budget.read(path, 1_048_576)?;
                bindings.push(input_binding(
                    PrivateInputKindV1::OidcConfiguration,
                    None,
                    None,
                    &file,
                )?);
                let prepared = crate::gateway_config::prepare_oidc_from_retained(file, |path| {
                    let file = budget.read(path, 262_144)?;
                    bindings.push(input_binding(
                        PrivateInputKindV1::OidcTrustBundle,
                        None,
                        None,
                        &file,
                    )?);
                    Ok(file)
                })?;
                // OIDC, even when its HTTP routes are closed, must carry the exact
                // finite tenant capacity plan before any legacy SQLite observation.
                ensure!(
                    prepared.file.version == 2
                        && prepared.file.authentication_audit.capacity_plan.is_some()
                        && prepared.file.ca_bundle_file.is_some(),
                    "human_sources_oidc_configuration_requires_auth4_and_trust_bundle"
                );
                let plan = prepared
                    .file
                    .authentication_audit
                    .capacity_plan
                    .as_ref()
                    .context("human_sources_authentication_capacity_missing")?;
                let planned: BTreeSet<_> =
                    plan.tenants.iter().map(|t| t.tenant_id.as_str()).collect();
                let configured: BTreeSet<_> =
                    tenants.iter().map(|t| t.tenant_id.as_str()).collect();
                ensure!(
                    planned == configured,
                    "human_sources_authentication_capacity_tenants_differ"
                );
                Ok(prepared)
            })
            .transpose()?;
        bindings
            .sort_by(|a, b| (a.kind, &a.tenant_id, a.role).cmp(&(b.kind, &b.tenant_id, b.role)));
        let redactor = Redactor::new(
            tenants
                .iter()
                .flat_map(|tenant| [tenant.runner_token.clone(), tenant.viewer_token.clone()])
                .collect(),
        );
        let oidc_public = oidc
            .as_ref()
            .map(|prepared| {
                resolved_oidc(
                    prepared,
                    mapping_revision.context("human_sources_mapping_revision_missing")?,
                    &redactor,
                )
            })
            .transpose()?;
        let public_digest = public_control_digest_v1(&resolved_control(
            revision,
            max_events,
            &tenants,
            oidc_public.clone(),
        )?)?;
        budget.revalidate()?;
        Ok(Self {
            budget,
            tenants,
            oidc,
            bindings,
            public_digest,
            oidc_public,
        })
    }
    pub fn redactor(&self) -> Redactor {
        Redactor::new(
            self.tenants
                .iter()
                .flat_map(|tenant| [tenant.runner_token.clone(), tenant.viewer_token.clone()])
                .collect(),
        )
    }
    pub fn options_for(
        &self,
        source: &ExpectedOwnedSourceV1,
        max_events: u64,
    ) -> Result<OperatorSourceOptions> {
        match source.kind {
            HumanSourceKindV1::TenantStore => {
                let tenant = self
                    .tenants
                    .iter()
                    .find(|t| Some(t.tenant_id.as_str()) == source.tenant_id.as_deref())
                    .context("human_sources_tenant_missing")?;
                let capacity = tenant.audit_capacity_policy.clone().unwrap_or_else(|| {
                    agentsandbox::audit::AuditCapacityPolicyV1::resolved(max_events)
                });
                let workload = tenant.workload_audit_policy.clone().unwrap_or_else(|| {
                    agentsandbox::audit::WorkloadAuditPolicyV1::resolved(max_events, &capacity)
                });
                Ok(OperatorSourceOptions::TenantStore {
                    redactor: self.redactor(),
                    max_events,
                    capacity,
                    workload,
                    retention: tenant.relay_retention_policy.clone().unwrap_or_default(),
                })
            }
            HumanSourceKindV1::AuthenticationLedger => {
                let oidc = self.oidc.as_ref().context("human_sources_oidc_missing")?;
                Ok(OperatorSourceOptions::AuthenticationLedger {
                    redactor: self.redactor(),
                    policy: oidc.file.authentication_audit.policy.clone(),
                    capacity_plan: oidc
                        .file
                        .authentication_audit
                        .capacity_plan
                        .clone()
                        .context("human_sources_authentication_capacity_missing")?,
                })
            }
        }
    }
    pub fn legacy_inputs(
        &self,
        installation_id: Uuid,
        max_events: u64,
    ) -> Result<Vec<LegacySourceInput>> {
        let mut result = Vec::new();
        for tenant in &self.tenants {
            let source = ExpectedOwnedSourceV1 {
                source_id: Uuid::nil(),
                kind: HumanSourceKindV1::TenantStore,
                installation_id,
                durable_namespace_id: Uuid::nil(),
                tenant_id: Some(tenant.tenant_id.clone()),
                storage_binding:
                    agentsandbox::human_authority::startup::observe_human_storage_binding(
                        tenant.storage.sqlite_path()?,
                    )?,
                required_format: 13,
            };
            result.push(LegacySourceInput {
                path: tenant.storage.sqlite_path()?.to_owned(),
                tenant_id: source.tenant_id.clone(),
                options: self.options_for(&source, max_events)?,
            });
        }
        if let Some(oidc) = &self.oidc {
            ensure!(
                oidc.file.authentication_audit.installation_id == installation_id,
                "human_sources_installation_mismatch"
            );
            let source = ExpectedOwnedSourceV1 {
                source_id: Uuid::nil(),
                kind: HumanSourceKindV1::AuthenticationLedger,
                installation_id,
                durable_namespace_id: Uuid::nil(),
                tenant_id: None,
                storage_binding:
                    agentsandbox::human_authority::startup::observe_human_storage_binding(
                        &oidc.audit_path,
                    )?,
                required_format: 4,
            };
            result.push(LegacySourceInput {
                path: oidc.audit_path.clone(),
                tenant_id: None,
                options: self.options_for(&source, max_events)?,
            });
        }
        let mut physical = BTreeSet::new();
        for source in &result {
            let binding = agentsandbox::human_authority::startup::observe_human_storage_binding(
                &source.path,
            )?;
            let identity = (binding.source_device, binding.source_inode);
            ensure!(
                physical.insert(identity)
                    && !self.bindings.iter().any(|input| (
                        input.storage_binding.source_device,
                        input.storage_binding.source_inode
                    ) == identity),
                "human_sources_source_or_configuration_alias"
            );
        }
        Ok(result)
    }
    pub fn verify_recovery(&self, recovery: &HumanSourcesPreparationRecoveryV1) -> Result<()> {
        ensure!(
            self.bindings == recovery.input_bindings,
            "human_sources_inputs_changed"
        );
        let configured: BTreeSet<_> = self
            .tenants
            .iter()
            .map(|t| {
                t.storage
                    .sqlite_path()
                    .map(|path| (t.tenant_id.as_str(), path))
            })
            .collect::<Result<_>>()?;
        let expected: BTreeSet<_> = recovery
            .target_sources
            .iter()
            .filter_map(|s| {
                s.tenant_id
                    .as_deref()
                    .map(|t| (t, s.storage_binding.path()))
            })
            .collect();
        ensure!(
            configured.len() == expected.len()
                && configured
                    .iter()
                    .all(|(tenant, path)| expected.contains(&(*tenant, path.to_path_buf()))),
            "human_sources_inventory_changed"
        );
        for source in &recovery.target_sources {
            // Metadata only, no source file FD or SQLite query. Prepare's
            // recorded namespace/format remains an expectation until apply.
            ensure!(
                agentsandbox::human_authority::startup::observe_human_storage_binding(
                    &source.storage_binding.path()
                )? == source.storage_binding,
                "human_sources_physical_source_changed"
            );
            let profile = match self.options_for(source, recovery.max_events)? {
                OperatorSourceOptions::TenantStore { max_events, .. } => {
                    SourceProfileV1::TenantStore { max_events }
                }
                OperatorSourceOptions::AuthenticationLedger { policy, .. } => {
                    SourceProfileV1::AuthenticationLedger { policy }
                }
            };
            ensure!(
                recovery.source_profiles.get(&source.source_id) == Some(&profile),
                "human_sources_profile_changed"
            );
        }
        if let Some(oidc) = &self.oidc {
            ensure!(
                oidc.file.authentication_audit.installation_id == recovery.installation_id
                    && recovery
                        .target_sources
                        .iter()
                        .any(|s| s.kind == HumanSourceKindV1::AuthenticationLedger
                            && s.storage_binding.path() == oidc.audit_path),
                "human_sources_oidc_inventory_changed"
            );
        }
        self.budget.revalidate()
    }
    pub fn envelope(
        &self,
        recovery: &HumanSourcesPreparationRecoveryV1,
    ) -> Result<(EnvelopeExpectationsV1, Vec<u8>)> {
        let tenants = self
            .tenants
            .iter()
            .map(|t| t.tenant_id.clone())
            .collect::<Vec<_>>();
        let inputs = recovery
            .technical_identities
            .iter()
            .map(|identity| {
                let tenant = self
                    .tenants
                    .iter()
                    .find(|t| t.tenant_id == identity.tenant_id)
                    .context("human_sources_credential_tenant_missing")?;
                Ok(TechnicalCredentialInput {
                    tenant_id: &identity.tenant_id,
                    role: identity.role,
                    credential_id: identity.credential_id,
                    generation: identity.generation,
                    token: match identity.role {
                        TechnicalCredentialRoleV1::Runner => &tenant.runner_token,
                        TechnicalCredentialRoleV1::Viewer => &tenant.viewer_token,
                    },
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let expected = EnvelopeExpectationsV1::from_configured_inputs(
            recovery.installation_id,
            recovery.closure_id,
            recovery.closure_revision,
            self.public_digest.clone(),
            &tenants,
            &inputs,
        )?;
        let bytes = prepare_envelope_configuration(&expected, &inputs)?;
        Ok((expected, bytes))
    }
    pub fn configuration(
        &mut self,
        recovery: &HumanSourcesPreparationRecoveryV1,
        technical: &PrivateFile,
    ) -> Result<(
        HumanAuthorityConfigurationV1,
        Vec<(HumanConfigurationFileKindV1, PrivateFile)>,
    )> {
        let mut files = vec![
            (
                HumanConfigurationFileKindV1::ControlConfiguration,
                self.budget
                    .read(&recovery.tenant_registry_path, 1_048_576)?,
            ),
            (
                HumanConfigurationFileKindV1::TechnicalCredentials,
                technical.clone(),
            ),
        ];
        for binding in &self.bindings {
            let kind = match binding.kind {
                PrivateInputKindV1::OidcConfiguration => {
                    HumanConfigurationFileKindV1::OidcConfiguration
                }
                PrivateInputKindV1::OidcTrustBundle => {
                    HumanConfigurationFileKindV1::OidcTrustBundle
                }
                _ => continue,
            };
            files.push((kind, self.budget.read(&binding.path, binding.kind.limit())?));
        }
        files.sort_by_key(|(kind, _)| *kind);
        let configuration = HumanAuthorityConfigurationV1 {
            control_config_revision: recovery.control_config_revision,
            public_config_digest: self.public_digest.clone(),
            oidc: self
                .oidc_public
                .as_ref()
                .map(|oidc| -> Result<_> {
                    Ok(HumanOidcConfigurationV1 {
                        public_config_digest: public_oidc_digest_v1(oidc)?,
                        mapping_revision: recovery
                            .oidc_mapping_revision
                            .context("human_sources_mapping_revision_missing")?,
                    })
                })
                .transpose()?,
            files: files
                .iter()
                .map(|(kind, file)| {
                    Ok(HumanConfigurationFileV1 {
                        kind: *kind,
                        storage_binding: observe_private_file_binding(file)?,
                    })
                })
                .collect::<Result<_>>()?,
        };
        Ok((configuration, files))
    }
}

fn input_binding(
    kind: PrivateInputKindV1,
    tenant_id: Option<String>,
    role: Option<TechnicalCredentialRoleV1>,
    file: &PrivateFile,
) -> Result<PrivateInputBindingV1> {
    Ok(PrivateInputBindingV1 {
        kind,
        tenant_id,
        role,
        path: file.path().to_owned(),
        storage_binding: observe_private_file_binding(file)?,
        raw_content_sha256: file.digest().to_owned(),
        raw_len: file.bytes().len() as u64,
    })
}
