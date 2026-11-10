//! Offline preparation of a new, private control installation.
//! The input is operator-approved configuration, never a human authentication
//! proof. No identity provider, engine or model is contacted by this module.
use crate::{
    api::OidcGatewayConfigV1,
    audit::authentication::{
        AuthenticationAudit, AuthenticationAuditPolicyV1, AuthenticationCapacityPlanV1,
    },
    control::QuotaPolicyV1,
    control_fs::{absolute_path, read_trusted_projected_file},
    identity::{HumanAccountV1, HumanCapabilityV1, MembershipV1, PrincipalRefV1},
    redaction::Redactor,
};
use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    os::fd::{AsFd, AsRawFd},
    path::PathBuf,
    time::{Duration, Instant},
};
use uuid::Uuid;

mod installation;
mod storage;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod v2_tests;

const MAX_INPUT: u64 = 262_144;
const MAX_RECOVERY: u64 = 1_048_576;
const MAX_EVENTS: u64 = 100_000;
const DEMO_ISSUER: &str = "http://keycloak.ai-guard.test:18080/realms/ai-guard-demo";
const DEMO_ORIGIN: &str = "http://127.0.0.1:4000";

#[derive(Args)]
pub struct SetupArgs {
    #[command(subcommand)]
    command: SetupCommand,
}
#[derive(Subcommand)]
enum SetupCommand {
    /// Prepare a new private installation offline; never adopt an existing destination.
    Prepare(PrepareArgs),
    /// Resume the exact retained preparation, without regenerating credentials.
    Resume(ResumeArgs),
}
#[derive(Args)]
struct PrepareArgs {
    /// Closed version1 or explicit-capacity version2 manifest; '-' reads bounded stdin.
    #[arg(long)]
    input: PathBuf,
    /// Exclusively new directory below an existing private parent.
    #[arg(long)]
    output_dir: PathBuf,
    /// Explicitly check the opt-in demo's five public environment settings.
    #[arg(long)]
    demo_defaults: bool,
}
#[derive(Args)]
struct ResumeArgs {
    /// Retained staging directory, or the final directory after publication.
    #[arg(long)]
    recovery_dir: PathBuf,
    /// Operator attestation that controls using this installation are stopped.
    #[arg(long, required = true)]
    confirm_control_stopped: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupManifestV1 {
    pub version: u32,
    pub gateway: OidcGatewayConfigV1,
    pub authentication_policy: AuthenticationAuditPolicyV1,
    #[serde(default)]
    pub ca_bundle_pem: Option<String>,
    pub tenants: Vec<SetupTenantV1>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupManifestV2 {
    pub version: u32,
    pub gateway: OidcGatewayConfigV1,
    pub authentication_policy: AuthenticationAuditPolicyV1,
    pub capacity_plan: AuthenticationCapacityPlanV1,
    #[serde(default)]
    pub ca_bundle_pem: Option<String>,
    pub tenants: Vec<SetupTenantV1>,
}

/// Internal normalization preserves the exact historical V1 wire fields.
/// V2 is a distinct closed wire type, never an optional plan on public V1.
#[derive(Clone)]
pub(super) struct SetupManifest {
    version: u32,
    common: SetupManifestV1,
    capacity_plan: Option<AuthenticationCapacityPlanV1>,
}
impl std::ops::Deref for SetupManifest {
    type Target = SetupManifestV1;
    fn deref(&self) -> &Self::Target {
        &self.common
    }
}
impl From<SetupManifestV1> for SetupManifest {
    fn from(common: SetupManifestV1) -> Self {
        Self {
            version: common.version,
            common,
            capacity_plan: None,
        }
    }
}
impl From<SetupManifestV2> for SetupManifest {
    fn from(v2: SetupManifestV2) -> Self {
        Self {
            version: v2.version,
            common: SetupManifestV1 {
                version: 1,
                gateway: v2.gateway,
                authentication_policy: v2.authentication_policy,
                ca_bundle_pem: v2.ca_bundle_pem,
                tenants: v2.tenants,
            },
            capacity_plan: Some(v2.capacity_plan),
        }
    }
}
impl Serialize for SetupManifest {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match &self.capacity_plan {
            None => self.common.serialize(serializer),
            Some(plan) => SetupManifestV2 {
                version: self.version,
                gateway: self.gateway.clone(),
                authentication_policy: self.authentication_policy.clone(),
                capacity_plan: plan.clone(),
                ca_bundle_pem: self.ca_bundle_pem.clone(),
                tenants: self.tenants.clone(),
            }
            .serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for SetupManifest {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            V2(SetupManifestV2),
            V1(SetupManifestV1),
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::V1(v1) => v1.into(),
            Wire::V2(v2) => v2.into(),
        })
    }
}
impl SetupManifest {
    fn validate(&self) -> Result<()> {
        ensure!(
            matches!(
                (self.version, self.capacity_plan.as_ref()),
                (1, None) | (2, Some(_))
            ),
            "unsupported setup manifest version/plan combination"
        );
        self.common.validate()?;
        if let Some(plan) = &self.capacity_plan {
            let configured: BTreeSet<_> = self
                .tenants
                .iter()
                .map(|tenant| tenant.tenant_id.as_str())
                .collect();
            let planned: BTreeSet<_> = plan
                .tenants
                .iter()
                .map(|tenant| tenant.tenant_id.as_str())
                .collect();
            ensure!(
                configured == planned,
                "authentication plan must name exactly the setup tenants"
            );
            AuthenticationAudit::validate_new_v4(
                Uuid::from_u128(1),
                &self.authentication_policy,
                plan,
                &Redactor::new(vec![]),
            )?;
        }
        ensure!(
            serde_json::to_vec(self)?.len() as u64 <= MAX_INPUT,
            "setup manifest exceeds its bound"
        );
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupTenantV1 {
    pub tenant_id: String,
    pub organization_id: String,
    #[serde(default)]
    pub worker_quota: QuotaPolicyV1,
    pub identities: Vec<SetupIdentityV1>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupIdentityV1 {
    pub principal: PrincipalRefV1,
    pub display_name: String,
    pub capabilities: Vec<HumanCapabilityV1>,
}

impl SetupManifestV1 {
    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported setup manifest version");
        self.validate_common()
    }

    fn validate_common(&self) -> Result<()> {
        ensure!(
            !self.tenants.is_empty(),
            "unsupported or empty setup manifest"
        );
        ensure!(
            serde_json::to_vec(self)?.len() as u64 <= MAX_INPUT,
            "setup manifest exceeds its bound"
        );
        self.gateway.validate()?;
        self.authentication_policy
            .validate_for_installation(Uuid::from_u128(1), &Redactor::new(vec![]))?;
        let approved: BTreeSet<_> = self
            .authentication_policy
            .approved_clients
            .iter()
            .map(|client| (client.issuer.as_str(), client.client_id.as_str()))
            .collect();
        let provider = &self.gateway.provider;
        ensure!(
            approved
                == BTreeSet::from([
                    (provider.issuer.as_str(), provider.web_client_id.as_str()),
                    (provider.issuer.as_str(), provider.cli_client_id.as_str()),
                ]),
            "authentication policy must approve exactly the configured issuer's Web and CLI clients"
        );
        if let Some(pem) = &self.ca_bundle_pem {
            // The existing provider enforces certificate-only PEM, 64 KiB,
            // at most eight certificates and HTTPS-only transport.
            crate::oidc::OidcProvider::new_with_ca(provider.clone(), pem.as_bytes())?;
        }
        let mut tenants = BTreeSet::new();
        for tenant in &self.tenants {
            crate::tenancy::validate_tenant_id(&tenant.tenant_id)?;
            // ASCII case aliases share a directory on supported macOS volumes.
            ensure!(
                tenants.insert(tenant.tenant_id.to_ascii_lowercase()),
                "duplicate or filesystem-aliased setup tenant"
            );
            tenant.worker_quota.validate()?;
            let mut principals = BTreeSet::new();
            for identity in &tenant.identities {
                ensure!(
                    matches!(&identity.principal, PrincipalRefV1::HumanOidc { issuer, .. } if issuer == &provider.issuer),
                    "setup requires explicit OIDC subjects of the approved issuer"
                );
                ensure!(
                    principals.insert(identity.principal.key()?),
                    "duplicate setup principal within a tenant"
                );
                let capabilities: BTreeSet<_> = identity.capabilities.iter().copied().collect();
                ensure!(
                    !capabilities.contains(&crate::identity::HumanCapabilityV1::ReadBrokerRegistry)
                        && !capabilities
                            .contains(&crate::identity::HumanCapabilityV1::ReadWorkerRegistry),
                    "prepare without infrastructure registry capabilities, explicitly migrate to the required schema, then apply a trusted control membership mutation"
                );
                ensure!(
                    capabilities.len() == identity.capabilities.len(),
                    "duplicate setup capability"
                );
                HumanAccountV1 {
                    version: 1,
                    principal: identity.principal.clone(),
                    display_name: identity.display_name.clone(),
                    revision: 1,
                    active: true,
                }
                .validate()?;
                MembershipV1 {
                    version: 1,
                    membership_id: Uuid::from_u128(1).to_string(),
                    principal: identity.principal.clone(),
                    tenant_id: tenant.tenant_id.clone(),
                    organization_scope: tenant.organization_id.clone(),
                    revision: 1,
                    active: true,
                    capabilities,
                }
                .validate()?;
            }
            // Validate even an explicitly empty tenant's organization scope.
            ensure!(
                !tenant.organization_id.trim().is_empty()
                    && tenant.organization_id.len() <= 128
                    && !tenant.organization_id.chars().any(char::is_control),
                "invalid setup organization"
            );
        }
        Ok(())
    }
}

fn demo_defaults(manifest: &SetupManifestV1) -> Result<()> {
    const PUBLIC: [(&str, &str); 5] = [
        ("AI_GUARD_DEMO_IDP_HOST", "keycloak.ai-guard.test"),
        ("AI_GUARD_DEMO_IDP_PORT", "18080"),
        ("AI_GUARD_DEMO_WEB_ORIGIN", DEMO_ORIGIN),
        ("AI_GUARD_GATEWAY_PORT", "8787"),
        ("AI_GUARD_DASHBOARD_PORT", "4000"),
    ];
    ensure!(
        manifest.gateway.provider.issuer == DEMO_ISSUER
            && manifest.gateway.web_origin == DEMO_ORIGIN
            && PUBLIC
                .iter()
                .all(|(name, expected)| std::env::var_os(name)
                    .is_none_or(|value| value == *expected)),
        "unsupported demo overrides; use the documented fixed demo endpoints or an explicitly reviewed manifest with generic setup prepare"
    );
    Ok(())
}

fn read_input(path: &std::path::Path) -> Result<SetupManifest> {
    let bytes = if path == std::path::Path::new("-") {
        read_stdin()?
    } else {
        read_trusted_projected_file(&absolute_path(path)?, MAX_INPUT)?
    };
    let manifest: SetupManifest = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid closed version1/version2 setup manifest"))?;
    manifest.validate()?;
    Ok(manifest)
}

fn read_stdin() -> Result<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let stdin = std::io::stdin();
    // This command is stdin's sole consumer. Retain its Rust lock and an owned
    // descriptor through poll/read; never change the shared open-file flags
    // or leave a blocked reader running after a timeout.
    let _guard = stdin.lock();
    let owned = stdin
        .as_fd()
        .try_clone_to_owned()
        .context("setup input descriptor unavailable")?;
    let mut input = std::fs::File::from(owned);
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(
            !remaining.is_zero(),
            "setup standard input deadline exceeded (5 seconds)"
        );
        let timeout = i32::try_from(remaining.as_millis().saturating_add(1))
            .context("setup input deadline is invalid")?;
        let mut descriptor = libc::pollfd {
            fd: input.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized pollfd, retained live owned descriptor and
        // a bounded timeout recalculated from the monotonic deadline.
        let ready = unsafe { libc::poll(&mut descriptor, 1, timeout) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error).context("setup input readiness failed");
        }
        ensure!(
            Instant::now() < deadline,
            "setup standard input deadline exceeded (5 seconds)"
        );
        if ready == 0 {
            continue;
        }
        ensure!(
            descriptor.revents & (libc::POLLERR | libc::POLLNVAL) == 0,
            "setup input descriptor failed"
        );
        if descriptor.revents & (libc::POLLIN | libc::POLLHUP) == 0 {
            continue;
        }
        // HUP may accompany remaining pipe bytes. Drain those through the
        // same bound, and accept completion only when read observes EOF.
        let limit = chunk.len().min(MAX_INPUT as usize + 1 - bytes.len());
        let count = match input.read(&mut chunk[..limit]) {
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error).context("setup input unavailable"),
        };
        ensure!(
            Instant::now() < deadline,
            "setup standard input deadline exceeded (5 seconds)"
        );
        if count == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..count]);
        ensure!(
            bytes.len() as u64 <= MAX_INPUT,
            "setup input exceeds 262144 bytes"
        );
    }
}

fn report(path: &std::path::Path) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "version":1,"status":"prepared","installation_dir":path,
        "operator_authentication":"private_control_filesystem_effective_uid",
        "identity_provider_contacted":false,"live_human_sessions_restored":false
    }))?;
    bytes.push(b'\n');
    std::io::stdout()
        .lock()
        .write_all(&bytes)
        .context("setup output unavailable; installation remains retained")
}

pub fn execute(args: SetupArgs) -> Result<()> {
    match args.command {
        SetupCommand::Prepare(args) => {
            let manifest = read_input(&args.input)?;
            if args.demo_defaults {
                demo_defaults(&manifest)?;
            }
            let path = storage::prepare(manifest, &args.output_dir)?;
            report(&path)
        }
        SetupCommand::Resume(args) => {
            ensure!(
                args.confirm_control_stopped,
                "setup resume requires --confirm-control-stopped"
            );
            let path = storage::resume(&args.recovery_dir)?;
            report(&path)
        }
    }
}
