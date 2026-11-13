//! One public configuration digest producer shared with future offline tools.
//! No DTO, digest, pathname or expected binding is a source authenticator.
use super::*;
use agentsandbox::{
    audit::{
        AuditCapacityPolicyV1, WorkloadAuditPolicyV1,
        authentication::{AuthenticationAuditPolicyV1, AuthenticationCapacityPlanV1},
    },
    control_fs::{PrivateFile, absolute_path},
    human_authority::{
        HumanAuthorityClosureV1, HumanAuthorityConfigurationV1, HumanConfigurationFileKindV1,
        HumanConfigurationFileV1, HumanOidcConfigurationV1, HumanSourceKindV1,
        HumanSourcesConfigurationV1,
        startup::{
            HumanOwnedSourceOptions, HumanOwnedStartupBundle, HumanOwnedStartupRequest,
            observe_human_storage_binding,
        },
    },
    redaction::Redactor,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const CONTROL_DOMAIN: &[u8] = b"ai-guard/human-control-public/v1\0";
const OIDC_DOMAIN: &[u8] = b"ai-guard/human-oidc-public/v1\0";

#[derive(Clone, Serialize)]
pub(crate) struct ResolvedHumanTenantPublicV1 {
    pub(crate) tenant_id: String,
    pub(crate) worker_quota: agentsandbox::control::QuotaPolicyV1,
    pub(crate) audit_capacity_policy: AuditCapacityPolicyV1,
    pub(crate) workload_audit_policy: WorkloadAuditPolicyV1,
    pub(crate) relay_retention_policy: agentsandbox::relay::RelayRetentionPolicyV1,
    pub(crate) held_create_policy:
        Option<agentsandbox::control::kubernetes_admission::KubernetesHeldCreatePolicyV1>,
    pub(crate) session_reservation_policy: Option<KubernetesSessionReservationPolicyV2>,
    pub(crate) session_recovery_policy: Option<KubernetesSessionRecoveryPolicyV2>,
    pub(crate) session_control_recovery_policy: Option<KubernetesSessionControlRecoveryPolicyV2>,
}
#[derive(Clone, Serialize)]
pub(crate) struct ResolvedHumanOidcPublicV1 {
    pub(crate) gateway: agentsandbox::api::OidcGatewayConfigV1,
    pub(crate) authentication_policy: AuthenticationAuditPolicyV1,
    pub(crate) capacity_plan: AuthenticationCapacityPlanV1,
    pub(crate) mapping_revision: u64,
    pub(crate) trust_bundle_sha256: String,
}
#[derive(Clone, Serialize)]
pub(crate) struct ResolvedHumanControlPublicV1 {
    pub(crate) version: u32,
    pub(crate) human_source_mode: &'static str,
    pub(crate) control_config_revision: u64,
    pub(crate) max_events: u64,
    pub(crate) tenants: Vec<ResolvedHumanTenantPublicV1>,
    pub(crate) oidc: Option<ResolvedHumanOidcPublicV1>,
}

pub(crate) fn canonical_public_json_v1(value: Value) -> Result<Vec<u8>> {
    fn canonical(value: Value) -> Result<Value> {
        Ok(match value {
            Value::Object(object) => {
                let sorted = object
                    .into_iter()
                    .map(|(key, value)| Ok((key, canonical(value)?)))
                    .collect::<Result<BTreeMap<_, _>>>()?;
                Value::Object(sorted.into_iter().collect())
            }
            Value::Array(values) => {
                Value::Array(values.into_iter().map(canonical).collect::<Result<_>>()?)
            }
            Value::Number(number) => {
                anyhow::ensure!(
                    number.as_i64().is_some() || number.as_u64().is_some(),
                    "public configuration numbers must be integers"
                );
                Value::Number(number)
            }
            value => value,
        })
    }
    Ok(serde_json::to_vec(&canonical(value)?)?)
}
fn digest_public(domain: &[u8], bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(bytes);
    hex::encode(digest.finalize())
}
fn canonical_control_bytes(input: &ResolvedHumanControlPublicV1) -> Result<Vec<u8>> {
    anyhow::ensure!(
        input.version == 1
            && input.human_source_mode == "owned_sources"
            && input.control_config_revision > 0
            && (1..=1_000_000).contains(&input.max_events),
        "invalid public control configuration"
    );
    let mut input = input.clone();
    input.tenants.sort_by(|a, b| a.tenant_id.cmp(&b.tenant_id));
    anyhow::ensure!(
        !input.tenants.is_empty()
            && !input
                .tenants
                .windows(2)
                .any(|pair| pair[0].tenant_id == pair[1].tenant_id),
        "ambiguous public tenant configuration"
    );
    canonical_public_json_v1(serde_json::to_value(input)?)
}
pub(crate) fn public_control_digest_v1(input: &ResolvedHumanControlPublicV1) -> Result<String> {
    Ok(digest_public(
        CONTROL_DOMAIN,
        &canonical_control_bytes(input)?,
    ))
}
pub(crate) fn public_oidc_digest_v1(input: &ResolvedHumanOidcPublicV1) -> Result<String> {
    input.gateway.validate()?;
    input.capacity_plan.validate()?;
    anyhow::ensure!(
        input.mapping_revision > 0
            && input.trust_bundle_sha256.len() == 64
            && input
                .trust_bundle_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid public OIDC configuration"
    );
    Ok(digest_public(
        OIDC_DOMAIN,
        &canonical_public_json_v1(serde_json::to_value(input)?)?,
    ))
}
pub(crate) fn resolved_control(
    revision: u64,
    max_events: u64,
    tenants: &[ConfiguredTenant],
    oidc: Option<ResolvedHumanOidcPublicV1>,
) -> Result<ResolvedHumanControlPublicV1> {
    let mut public = Vec::new();
    for tenant in tenants {
        agentsandbox::tenancy::validate_tenant_id(&tenant.tenant_id)?;
        tenant.worker_quota.validate()?;
        let capacity = tenant
            .audit_capacity_policy
            .clone()
            .unwrap_or_else(|| AuditCapacityPolicyV1::resolved(max_events));
        capacity.validate(max_events)?;
        let workload = tenant
            .workload_audit_policy
            .clone()
            .unwrap_or_else(|| WorkloadAuditPolicyV1::resolved(max_events, &capacity));
        workload.validate(max_events, &capacity)?;
        let retention = tenant.relay_retention_policy.clone().unwrap_or_default();
        retention.validate()?;
        public.push(ResolvedHumanTenantPublicV1 {
            tenant_id: tenant.tenant_id.clone(),
            worker_quota: tenant.worker_quota.clone(),
            audit_capacity_policy: capacity,
            workload_audit_policy: workload,
            relay_retention_policy: retention,
            held_create_policy: tenant
                .held_create_policy
                .clone()
                .map(|policy| policy.canonicalized())
                .transpose()?,
            session_reservation_policy: tenant
                .session_reservation_policy
                .clone()
                .map(|policy| policy.canonicalized())
                .transpose()?,
            session_recovery_policy: tenant
                .session_recovery_policy
                .clone()
                .map(|policy| policy.canonicalized())
                .transpose()?,
            session_control_recovery_policy: tenant
                .session_control_recovery_policy
                .clone()
                .map(|policy| policy.canonicalized())
                .transpose()?,
        });
    }
    public.sort_by(|a, b| a.tenant_id.cmp(&b.tenant_id));
    let result = ResolvedHumanControlPublicV1 {
        version: 1,
        human_source_mode: "owned_sources",
        control_config_revision: revision,
        max_events,
        tenants: public,
        oidc,
    };
    canonical_control_bytes(&result)?;
    Ok(result)
}

pub fn validate_human_mode(
    mode: &str,
    manifest: Option<&Path>,
    envelope: Option<&Path>,
    tenants: Option<&Path>,
    initialize_stores: bool,
    initialize_oidc: bool,
) -> Result<()> {
    match mode {
        "legacy" => anyhow::ensure!(
            manifest.is_none() && envelope.is_none(),
            "owned human references require explicit owned_sources mode"
        ),
        "owned_sources" => anyhow::ensure!(
            manifest.is_some()
                && envelope.is_some()
                && tenants.is_some()
                && !initialize_stores
                && !initialize_oidc,
            "owned human sources require an existing manifest, envelope and tenant registry; initialization and automatic migration are refused"
        ),
        _ => anyhow::bail!("unknown human source mode"),
    }
    Ok(())
}

pub fn load_owned_tenants(path: &Path) -> Result<Vec<ConfiguredTenant>> {
    let result = (|| -> Result<_> {
        let file = PrivateFile::open(&absolute_path(path)?, 1_048_576)?;
        let tenants = super::load(file.path())?;
        for tenant in &tenants {
            for (path, expected) in [
                (&tenant.runner_token_file, &tenant.runner_token),
                (&tenant.viewer_token_file, &tenant.viewer_token),
            ] {
                let credential = PrivateFile::open(&absolute_path(path)?, 4096)?;
                anyhow::ensure!(
                    std::str::from_utf8(credential.bytes())?.trim() == expected.as_str(),
                    "technical credential file changed"
                );
                credential.revalidate()?;
            }
        }
        file.revalidate()?;
        Ok(tenants)
    })();
    result.map_err(|_:anyhow::Error|anyhow::anyhow!("Owned human tenant configuration invalid or unavailable; preserve existing sources and check private configuration files"))
}

pub struct PreparedOwnedSources {
    pub bundle: HumanOwnedStartupBundle,
    pub credentials: PrivateFile,
}

/// Pure shared resolution from the retained private OIDC configuration and its
/// actual trust bundle. No authentication ledger, HTTP discovery or owner.
pub(crate) fn resolved_oidc(
    prepared: &super::PreparedOidcFile,
    mapping_revision: u64,
    redactor: &Redactor,
) -> Result<ResolvedHumanOidcPublicV1> {
    prepared
        .file
        .authentication_audit
        .policy
        .validate_for_installation(prepared.file.authentication_audit.installation_id, redactor)?;
    anyhow::ensure!(
        prepared.file.version == 2 && mapping_revision > 0,
        "owned OIDC requires explicit mapping revision and version2 capacity configuration"
    );
    let trust = prepared
        .trust
        .as_ref()
        .context("owned OIDC requires an explicit retained public trust bundle")?;
    trust.revalidate()?;
    let public = ResolvedHumanOidcPublicV1 {
        gateway: prepared.file.gateway.clone(),
        authentication_policy: prepared.file.authentication_audit.policy.clone(),
        capacity_plan: prepared
            .file
            .authentication_audit
            .capacity_plan
            .clone()
            .context("owned OIDC capacity plan required")?,
        mapping_revision,
        trust_bundle_sha256: trust.digest().to_owned(),
    };
    public_oidc_digest_v1(&public)?;
    prepared.held.revalidate()?;
    Ok(public)
}

/// No ordinary Store/AuthLedger opener occurs here. Expected binding snapshots
/// are rechecked by acquire and the actual floor/CAS/retain source protocol.
#[allow(clippy::too_many_arguments)]
pub fn load_owned_sources(
    manifest_path: &Path,
    envelope_path: &Path,
    control_path: &Path,
    oidc_path: Option<&Path>,
    tenants: &[ConfiguredTenant],
    max_events: u64,
    control_revision: u64,
    mapping_revision: Option<u64>,
    redactor: Redactor,
) -> Result<PreparedOwnedSources> {
    let result = (|| -> Result<_> {
        let manifest = PrivateFile::open(
            &absolute_path(manifest_path)?,
            agentsandbox::human_authority::MAX_HUMAN_MANIFEST_BYTES as u64,
        )?;
        let closure = HumanAuthorityClosureV1::from_bytes(manifest.bytes())?;
        let control = PrivateFile::open(&absolute_path(control_path)?, 1_048_576)?;
        let credentials = PrivateFile::open(&absolute_path(envelope_path)?, 1_048_576)?;
        let reloaded = load_owned_tenants(control.path())?;
        anyhow::ensure!(
            tenants.len() == reloaded.len(),
            "tenant configuration changed"
        );
        for tenant in tenants {
            let other = reloaded
                .iter()
                .find(|other| other.tenant_id == tenant.tenant_id)
                .context("tenant configuration changed")?;
            anyhow::ensure!(
                absolute_path(&tenant.data)? == absolute_path(&other.data)?
                    && tenant.runner_token == other.runner_token
                    && tenant.viewer_token == other.viewer_token,
                "tenant configuration changed"
            );
        }
        let mut files = vec![
            (HumanConfigurationFileKindV1::ControlConfiguration, control),
            (
                HumanConfigurationFileKindV1::TechnicalCredentials,
                credentials.clone(),
            ),
        ];
        let (oidc_public, authentication) = match oidc_path {
            None => {
                anyhow::ensure!(
                    mapping_revision.is_none(),
                    "OIDC mapping revision without selected OIDC"
                );
                (None, None)
            }
            Some(path) => {
                let prepared = super::prepare_oidc_file(path)?;
                let revision = mapping_revision
                    .filter(|revision| *revision > 0)
                    .context("explicit OIDC mapping closure revision required")?;
                let public = resolved_oidc(&prepared, revision, &redactor)?;
                let trust = prepared
                    .trust
                    .clone()
                    .context("owned OIDC trust bundle required")?;
                let capacity_tenants = public
                    .capacity_plan
                    .tenants
                    .iter()
                    .map(|tenant| tenant.tenant_id.as_str())
                    .collect::<BTreeSet<_>>();
                anyhow::ensure!(
                    capacity_tenants
                        == tenants
                            .iter()
                            .map(|tenant| tenant.tenant_id.as_str())
                            .collect(),
                    "OIDC tenant capacity union differs"
                );
                files.push((
                    HumanConfigurationFileKindV1::OidcConfiguration,
                    prepared.held,
                ));
                files.push((HumanConfigurationFileKindV1::OidcTrustBundle, trust));
                let capacity_plan = public.capacity_plan.clone();
                (
                    Some(public),
                    Some((
                        prepared.audit_path,
                        prepared.file.authentication_audit.installation_id,
                        prepared.file.authentication_audit.policy,
                        capacity_plan,
                    )),
                )
            }
        };
        let public = resolved_control(control_revision, max_events, &reloaded, oidc_public)?;
        // Compare the first read's fully resolved public values too. A loader
        // race cannot apply policies from a different admitted configuration.
        anyhow::ensure!(
            canonical_control_bytes(&public)?
                == canonical_control_bytes(&resolved_control(
                    control_revision,
                    max_events,
                    tenants,
                    public.oidc.clone()
                )?)?,
            "resolved control configuration changed"
        );
        let oidc_configuration = match public.oidc.as_ref() {
            Some(oidc) => Some(HumanOidcConfigurationV1 {
                public_config_digest: public_oidc_digest_v1(oidc)?,
                mapping_revision: oidc.mapping_revision,
            }),
            None => None,
        };
        let configuration = HumanAuthorityConfigurationV1 {
            control_config_revision: control_revision,
            public_config_digest: public_control_digest_v1(&public)?,
            oidc: oidc_configuration,
            files: files
                .iter()
                .map(|(kind, file)| {
                    Ok(HumanConfigurationFileV1 {
                        kind: *kind,
                        storage_binding: observe_human_storage_binding(file.path())?,
                    })
                })
                .collect::<Result<_>>()?,
        };
        anyhow::ensure!(
            closure.configuration == configuration,
            "owned human configuration differs from retained manifest"
        );
        let mut sources = Vec::new();
        let mut options = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut auth_seen = false;
        for expected in &closure.sources {
            let mut descriptor = expected.clone();
            let option = match expected.kind {
                HumanSourceKindV1::TenantStore => {
                    let tenant = tenants
                        .iter()
                        .find(|tenant| {
                            Some(tenant.tenant_id.as_str()) == expected.tenant_id.as_deref()
                        })
                        .context("configured human tenant source absent")?;
                    anyhow::ensure!(
                        matches!(expected.required_format, 14 | 15)
                            && seen.insert(tenant.tenant_id.clone()),
                        "human source format/union differs; offline migration required"
                    );
                    descriptor.storage_binding = observe_human_storage_binding(&tenant.data)?;
                    let effective = public
                        .tenants
                        .iter()
                        .find(|item| item.tenant_id == tenant.tenant_id)
                        .context("resolved tenant policy absent")?;
                    HumanOwnedSourceOptions::TenantStore {
                        redactor: redactor.clone(),
                        max_events,
                        capacity: effective.audit_capacity_policy.clone(),
                        workload: effective.workload_audit_policy.clone(),
                        retention: effective.relay_retention_policy.clone(),
                    }
                }
                HumanSourceKindV1::AuthenticationLedger => {
                    let (path, installation, policy, capacity_plan) = authentication
                        .as_ref()
                        .context("unexpected authentication source")?;
                    anyhow::ensure!(
                        !auth_seen
                            && expected.required_format == 5
                            && expected.installation_id == *installation,
                        "authentication source selection differs"
                    );
                    auth_seen = true;
                    descriptor.storage_binding = observe_human_storage_binding(path)?;
                    HumanOwnedSourceOptions::AuthenticationLedger {
                        redactor: redactor.clone(),
                        policy: policy.clone(),
                        capacity_plan: capacity_plan.clone(),
                    }
                }
            };
            anyhow::ensure!(
                descriptor == *expected,
                "configured physical source differs"
            );
            options.insert(descriptor.source_id, option);
            sources.push(descriptor);
        }
        anyhow::ensure!(
            seen == tenants
                .iter()
                .map(|tenant| tenant.tenant_id.clone())
                .collect()
                && auth_seen == authentication.is_some(),
            "incomplete human source union"
        );
        for (_, file) in &files {
            file.revalidate()?;
        }
        manifest.revalidate()?;
        let configuration = HumanSourcesConfigurationV1::new(sources, configuration, files)?;
        let bundle = HumanOwnedStartupBundle::open(HumanOwnedStartupRequest {
            manifest,
            configuration,
            sources: options,
        })?;
        Ok(PreparedOwnedSources {
            bundle,
            credentials,
        })
    })();
    result.map_err(|_:anyhow::Error|anyhow::anyhow!("Owned human startup refused; preserve sources, manifests and credentials; verify exact configuration, explicit revisions, formats14/15/Auth5, private files, durable clocks and offline migration status"))
}

#[cfg(test)]
mod tests;
