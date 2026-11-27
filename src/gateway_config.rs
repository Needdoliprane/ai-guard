//! Trusted operator configuration. Request data never selects these scopes.
use agentsandbox::control::kubernetes_session::{
    KubernetesSessionControlRecoveryPolicyV2, KubernetesSessionRecoveryPolicyV2,
    KubernetesSessionReservationPolicyV2,
};
#[cfg(feature = "mysql-storage")]
use agentsandbox::diagnostics::gateway_files::{
    MAX_REGISTRY_BYTES, MySqlAuthenticationFiles, MySqlStorageFiles, TenantRegistryFiles,
    TenantStorageFiles,
};
use agentsandbox::diagnostics::gateway_files::{
    SelectedTenantStorage, decode_tenant_registry, read_tenant_registry_file, storage_for_gateway,
};
pub(crate) mod human_sources;
use anyhow::{Context, Result};
pub use human_sources::{load_owned_sources, load_owned_tenants, validate_human_mode};
use serde::Deserialize;
#[cfg(feature = "mysql-storage")]
use std::collections::BTreeMap;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub struct ConfiguredTenant {
    pub tenant_id: String,
    pub storage: ConfiguredStorage,
    pub runner_token_file: PathBuf,
    pub viewer_token_file: PathBuf,
    pub runner_token: String,
    pub viewer_token: String,
    pub worker_quota: agentsandbox::control::QuotaPolicyV1,
    pub audit_capacity_policy: Option<agentsandbox::audit::AuditCapacityPolicyV1>,
    pub workload_audit_policy: Option<agentsandbox::audit::WorkloadAuditPolicyV1>,
    pub relay_retention_policy: Option<agentsandbox::relay::RelayRetentionPolicyV1>,
    pub egress_policy: Option<agentsandbox::control::egress_decision::EgressTenantPolicyV1>,
    pub held_create_policy:
        Option<agentsandbox::control::kubernetes_admission::KubernetesHeldCreatePolicyV1>,
    pub session_reservation_policy: Option<KubernetesSessionReservationPolicyV2>,
    pub session_recovery_policy: Option<KubernetesSessionRecoveryPolicyV2>,
    pub session_control_recovery_policy: Option<KubernetesSessionControlRecoveryPolicyV2>,
    pub session_effect_policy:
        Option<agentsandbox::control::kubernetes_session::effects::KubernetesSessionEffectPolicyV2>,
}

/// Interim, explicit refusal: the owned-sources profile (formats 14/15, a
/// retained local source owner and storage binding) is not yet defined for a
/// MySQL target. It never degrades to legacy human sources or to SQLite.
pub fn refuse_owned_sources_on_mysql(tenants: &[ConfiguredTenant], owned: bool) -> Result<()> {
    #[cfg(feature = "mysql-storage")]
    anyhow::ensure!(
        !owned
            || !tenants
                .iter()
                .any(|tenant| matches!(tenant.storage, ConfiguredStorage::Mysql(_))),
        "owned human sources are not available on a MySQL tenant yet; use the SQLite profile for owned_sources or the legacy human source mode"
    );
    #[cfg(not(feature = "mysql-storage"))]
    let _ = (tenants, owned);
    Ok(())
}

/// The tenant's selected audit backend with its private material, already read.
/// Secrets stay in memory only and never reach Debug, logs or diagnostics.
#[derive(Clone)]
pub enum ConfiguredStorage {
    Sqlite(PathBuf),
    #[cfg(feature = "mysql-storage")]
    Mysql(Box<MysqlTarget>),
}

impl ConfiguredStorage {
    /// Offline maintenance and the human-source migration target SQLite files.
    pub fn sqlite_path(&self) -> Result<&Path> {
        match self {
            Self::Sqlite(path) => Ok(path),
            #[cfg(feature = "mysql-storage")]
            Self::Mysql(_) => anyhow::bail!(
                "This operation targets a SQLite tenant database; the tenant uses MySQL storage"
            ),
        }
    }
}

#[cfg(feature = "mysql-storage")]
#[derive(Clone)]
pub struct MysqlTarget {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub namespace: uuid::Uuid,
    pub password: String,
    pub ca_pem: Vec<u8>,
    pub client_identity: Option<(Vec<u8>, Vec<u8>)>,
}

#[cfg(feature = "mysql-storage")]
impl std::fmt::Debug for MysqlTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MysqlTarget")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("namespace", &self.namespace)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "mysql-storage")]
impl MysqlTarget {
    pub(crate) fn endpoint(&self) -> agentsandbox::mysql_storage::MysqlEndpoint<'_> {
        agentsandbox::mysql_storage::MysqlEndpoint {
            host: &self.host,
            port: self.port,
            database: &self.database,
            username: &self.username,
        }
    }

    pub(crate) fn credentials(&self) -> agentsandbox::mysql_storage::MysqlCredentials<'_> {
        use agentsandbox::mysql_storage::{MysqlClientIdentity, MysqlCredentials};
        MysqlCredentials {
            password: &self.password,
            ca_pem: &self.ca_pem,
            client_identity: self
                .client_identity
                .as_ref()
                .map(|(cert, key)| MysqlClientIdentity {
                    certificate_chain_pem: cert,
                    private_key_pem: key,
                }),
        }
    }

    fn load(
        profile: &agentsandbox::diagnostics::gateway_files::MySqlStorageFiles,
        resolve: &dyn Fn(PathBuf) -> Result<PathBuf>,
    ) -> Result<Self> {
        use agentsandbox::{control_fs::PrivateFile, mysql_storage as limits};
        let read = |path: &Path, maximum: u64, what: &str| -> Result<PrivateFile> {
            let file = PrivateFile::open(&resolve(path.to_owned())?, maximum)
                .with_context(|| format!("MySQL {what} reference is not a private regular file"))?;
            Ok(file)
        };
        let password_file = read(
            &profile.password_file,
            limits::MAX_PASSWORD_BYTES as u64 + 2,
            "password",
        )?;
        let mut password = std::str::from_utf8(password_file.bytes())
            .context("MySQL password file is not UTF-8")?;
        password = password.strip_suffix('\n').unwrap_or(password);
        anyhow::ensure!(
            !password.is_empty()
                && password.len() <= limits::MAX_PASSWORD_BYTES
                && !password.chars().any(char::is_control),
            "MySQL password file does not hold one bounded password"
        );
        let ca = read(
            &profile.ca_file,
            limits::MAX_CERTIFICATE_BYTES as u64,
            "CA bundle",
        )?;
        let client_identity = match &profile.authentication {
            MySqlAuthenticationFiles::Password {} => None,
            MySqlAuthenticationFiles::PasswordAndMtls {
                client_cert_file,
                client_key_file,
            } => {
                let cert = read(
                    client_cert_file,
                    limits::MAX_CERTIFICATE_BYTES as u64,
                    "client certificate",
                )?;
                let key = read(
                    client_key_file,
                    limits::MAX_PRIVATE_KEY_BYTES as u64,
                    "client key",
                )?;
                Some((cert.bytes().to_vec(), key.bytes().to_vec()))
            }
        };
        Ok(Self {
            host: profile.host.clone(),
            port: profile.port.get(),
            database: profile.database.clone(),
            username: profile.username.clone(),
            namespace: profile.namespace,
            password: password.to_owned(),
            ca_pem: ca.bytes().to_vec(),
            client_identity,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OidcFile {
    pub(crate) version: u32,
    pub(crate) gateway: agentsandbox::api::OidcGatewayConfigV1,
    pub(crate) authentication_audit: AuthenticationAuditFile,
    pub(crate) ca_bundle_file: Option<PathBuf>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuthenticationAuditFile {
    pub(crate) path: PathBuf,
    pub(crate) installation_id: uuid::Uuid,
    pub(crate) policy: agentsandbox::audit::authentication::AuthenticationAuditPolicyV1,
    pub(crate) capacity_plan:
        Option<agentsandbox::audit::authentication::AuthenticationCapacityPlanV1>,
}

pub(crate) struct PreparedOidcFile {
    pub(crate) held: agentsandbox::control_fs::PrivateFile,
    pub(crate) file: OidcFile,
    pub(crate) audit_path: PathBuf,
    pub(crate) ca_bundle_pem: Option<String>,
    pub(crate) trust: Option<agentsandbox::control_fs::PrivateFile>,
}

/// Closed, private configuration and local references only. Provider validation
/// constructs an offline transport; it never discovers or contacts the IdP.
fn prepare_oidc_file(path: &Path) -> Result<PreparedOidcFile> {
    use agentsandbox::control_fs::{PrivateFile, absolute_path};
    let path = absolute_path(path)?;
    let held = PrivateFile::open(&path, 1_048_576)?;
    prepare_oidc_from_retained(held, |path| PrivateFile::open(path, 262_144))
}

pub(crate) fn prepare_oidc_from_retained(
    held: agentsandbox::control_fs::PrivateFile,
    mut load_ca: impl FnMut(&Path) -> Result<agentsandbox::control_fs::PrivateFile>,
) -> Result<PreparedOidcFile> {
    use agentsandbox::control_fs::absolute_path;
    let path = held.path().to_owned();
    let file: OidcFile = agentsandbox::human_authority::migration::operator::decode_closed(
        held.bytes(),
        1_048_576,
        256,
    )
    .context("Invalid OIDC gateway file")?;
    anyhow::ensure!(
        matches!(
            (
                file.version,
                file.authentication_audit.capacity_plan.as_ref()
            ),
            (1, None) | (2, Some(_))
        ),
        "OIDC configuration requires version1 without a capacity plan or version2 with an explicit plan"
    );
    if let Some(plan) = &file.authentication_audit.capacity_plan {
        plan.validate()?;
    }
    file.gateway.validate()?;
    let parent = path.parent().context("OIDC file has no parent")?;
    let resolve = |path: PathBuf| -> Result<PathBuf> {
        anyhow::ensure!(!path.as_os_str().is_empty(), "Empty OIDC file reference");
        absolute_path(&if path.is_absolute() {
            path
        } else {
            parent.join(path)
        })
    };
    let trust = file
        .ca_bundle_file
        .as_ref()
        .map(|path| load_ca(&resolve(path.clone())?))
        .transpose()?;
    let ca_bundle_pem = trust
        .as_ref()
        .map(|ca| -> Result<String> {
            let pem =
                String::from_utf8(ca.bytes().to_vec()).context("Invalid public OIDC CA bundle")?;
            agentsandbox::oidc::OidcProvider::new_with_ca(
                file.gateway.provider.clone(),
                pem.as_bytes(),
            )?;
            ca.revalidate()?;
            Ok(pem)
        })
        .transpose()?;
    let audit_path = resolve(file.authentication_audit.path.clone())?;
    held.revalidate()?;
    Ok(PreparedOidcFile {
        held,
        file,
        audit_path,
        ca_bundle_pem,
        trust,
    })
}

/// A separate explicit operator file. Existing tenant registry format stays closed.
pub fn load_oidc(
    path: &Path,
    initialize_new: bool,
    redactor: agentsandbox::redaction::Redactor,
) -> Result<agentsandbox::api::OidcGateway> {
    use agentsandbox::audit::authentication::AuthenticationAudit;
    let PreparedOidcFile {
        held,
        file,
        audit_path,
        ca_bundle_pem,
        trust: _,
    } = prepare_oidc_file(path)?;
    let capacity_plan = file.authentication_audit.capacity_plan.clone();
    let audit = if initialize_new {
        match &capacity_plan {
            Some(plan) => AuthenticationAudit::init_new_v4(
                &audit_path,
                file.authentication_audit.installation_id,
                file.authentication_audit.policy,
                plan.clone(),
                redactor,
            )?,
            None => AuthenticationAudit::init_new(
                &audit_path,
                file.authentication_audit.installation_id,
                file.authentication_audit.policy,
                redactor,
            )?,
        }
    } else {
        AuthenticationAudit::open_existing(
            &audit_path,
            file.authentication_audit.installation_id,
            file.authentication_audit.policy,
            redactor,
        )?
    };
    match (file.version, &capacity_plan, audit.format_version()?) {
        (1, None, 3) => {}
        (2, Some(plan), 4) => anyhow::ensure!(
            audit.capacity_plan()?.as_ref() == Some(plan),
            "Authentication capacity plan differs from the durable plan; preserve the audit and its configuration"
        ),
        _ => anyhow::bail!(
            "Authentication audit migration required: stop all consumers and migrate one explicit version at a time with a new private backup; target4 requires version2 configuration with its capacity plan; never initialize a replacement audit"
        ),
    }
    held.revalidate()?;
    agentsandbox::api::OidcGateway::new(file.gateway, std::sync::Arc::new(audit), ca_bundle_pem)
}

/// Explicit operator maintenance. No OIDC gateway, discovery, token or provider
/// request is created. Opening the existing ledger cannot upgrade its format.
pub fn migrate_authentication(
    path: &Path,
    backup: &Path,
    target_version: u32,
    redactor: agentsandbox::redaction::Redactor,
) -> Result<agentsandbox::audit::authentication::AuthenticationVerificationV1> {
    use agentsandbox::audit::authentication::AuthenticationAudit;
    anyhow::ensure!(
        matches!(target_version, 2 | 3 | 4),
        "Unsupported authentication migration target"
    );
    // Discard parser/filesystem chains: operator-controlled inputs may include
    // sensitive values in unknown fields, malformed types or path references.
    let prepared = prepare_oidc_file(path).map_err(|_| {
        anyhow::anyhow!(
            "Authentication migration configuration invalid or unavailable; check the closed configuration version, private ownership/permissions and local audit/CA references"
        )
    })?;
    anyhow::ensure!(
        matches!((target_version, prepared.file.version), (2 | 3, 1) | (4, 2)),
        "Authentication targets2/3 require configuration version1; target4 requires configuration version2 with an explicit capacity plan"
    );
    let capacity_plan = prepared.file.authentication_audit.capacity_plan.clone();
    let audit = AuthenticationAudit::open_existing(
        &prepared.audit_path,
        prepared.file.authentication_audit.installation_id,
        prepared.file.authentication_audit.policy,
        redactor,
    )
    .map_err(|_| {
        anyhow::anyhow!(
            "Existing authentication audit cannot be verified; preserve it and check installation identity, unchanged policy, private storage and integrity"
        )
    })?;
    prepared.held.revalidate().map_err(|_| {
        anyhow::anyhow!(
            "Authentication configuration changed during preparation; preserve the audit and retry with a stable private configuration"
        )
    })?;
    let migration = match target_version {
        2 => audit.migrate_verified_initial_v2(backup),
        3 => audit.migrate_native_logout_v3(backup),
        4 => audit.migrate_tenant_capacity_v4(
            backup,
            capacity_plan.context("Authentication target4 requires a capacity plan")?,
        ),
        _ => anyhow::bail!("Unsupported authentication migration target"),
    };
    migration.map_err(|_| {
        anyhow::anyhow!(
            "Authentication migration refused; requires the verified immediately prior format and a new private backup destination; preserve the audit and any backup, check the explicit target, permissions and stopped controls before retrying"
        )
    })
}

pub fn load(path: &Path) -> Result<Vec<ConfiguredTenant>> {
    let bytes = read_tenant_registry_file(path)?;
    load_registry_bytes(path, &bytes, crate::read_token)
}

#[cfg(feature = "mysql-storage")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MysqlPreparationProfileRefusal {
    SharedSchema,
    SharedUser,
    SharedGatewayUser,
    TargetMismatch,
}

#[cfg(feature = "mysql-storage")]
impl std::fmt::Display for MysqlPreparationProfileRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SharedSchema => "Each MySQL host, port and schema must belong to one tenant",
            Self::SharedUser => "MySQL publication requires different operator and gateway usernames",
            Self::SharedGatewayUser => {
                "MySQL gateway tenants on the same configured host and port require distinct usernames"
            }
            Self::TargetMismatch => {
                "MySQL operator and gateway profiles must select the same host, port, schema and namespace"
            }
        })
    }
}

#[cfg(feature = "mysql-storage")]
impl std::error::Error for MysqlPreparationProfileRefusal {}

/// Validate both private registry snapshots before opening authentication
/// material. Only the selected operator profile supplies it.
#[cfg(feature = "mysql-storage")]
pub(crate) fn load_mysql_preparation_target(
    operator_path: &Path,
    gateway_path: &Path,
    tenant_id: &str,
) -> Result<MysqlTarget> {
    use agentsandbox::control_fs::{PrivateFile, absolute_path};

    let operator_file = PrivateFile::open(&absolute_path(operator_path)?, MAX_REGISTRY_BYTES)?;
    let gateway_file = PrivateFile::open(&absolute_path(gateway_path)?, MAX_REGISTRY_BYTES)?;
    let operator_registry = decode_tenant_registry(operator_file.bytes())?;
    let gateway_registry = decode_tenant_registry(gateway_file.bytes())?;
    let operator_profiles = mysql_registry_profiles(&operator_registry)?;
    let gateway_profiles = mysql_gateway_profiles(&gateway_registry)?;
    let (Some(operator), Some(gateway)) = (
        operator_profiles.get(tenant_id).copied(),
        gateway_profiles.get(tenant_id).copied(),
    ) else {
        anyhow::bail!("requested tenant must have a MySQL profile in both registries");
    };
    anyhow::ensure!(
        operator.host == gateway.host
            && operator.port == gateway.port
            && operator.database == gateway.database
            && operator.namespace == gateway.namespace,
        MysqlPreparationProfileRefusal::TargetMismatch
    );
    anyhow::ensure!(
        !gateway_profiles.values().any(|runtime| {
            runtime.host.eq_ignore_ascii_case(&operator.host)
                && runtime.port == operator.port
                && runtime.username == operator.username
        }),
        MysqlPreparationProfileRefusal::SharedUser
    );

    let directory = operator_file
        .path()
        .parent()
        .context("operator registry has no parent")?;
    let target = MysqlTarget::load(operator, &|path| absolute_path(&directory.join(path)))?;
    operator_file.revalidate()?;
    gateway_file.revalidate()?;
    Ok(target)
}

#[cfg(feature = "mysql-storage")]
fn mysql_registry_profiles(
    registry: &TenantRegistryFiles,
) -> Result<BTreeMap<&str, &MySqlStorageFiles>> {
    use agentsandbox::tenancy::validate_tenant_id;

    let mut tenant_ids = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    let mut profiles = BTreeMap::new();
    for tenant in &registry.tenants {
        validate_tenant_id(&tenant.tenant_id)?;
        anyhow::ensure!(
            tenant_ids.insert(tenant.tenant_id.as_str()),
            "duplicate tenant identity in the registry"
        );
        if let SelectedTenantStorage::MySql(profile) =
            storage_for_gateway(tenant.data.as_deref(), tenant.storage.as_ref())?
        {
            // Compare configured endpoints without resolving aliases. Host case
            // is ignored, while the port and schema name must match exactly.
            let destination = (
                profile.host.to_ascii_lowercase(),
                profile.port,
                profile.database.as_str(),
            );
            anyhow::ensure!(
                destinations.insert(destination),
                MysqlPreparationProfileRefusal::SharedSchema
            );
            profiles.insert(tenant.tenant_id.as_str(), profile);
        }
    }
    Ok(profiles)
}

#[cfg(feature = "mysql-storage")]
fn mysql_gateway_profiles(
    registry: &TenantRegistryFiles,
) -> Result<BTreeMap<&str, &MySqlStorageFiles>> {
    let profiles = mysql_registry_profiles(registry)?;
    let mut accounts = BTreeSet::new();
    for profile in profiles.values() {
        // Compare configured endpoints. Physical identity and grants require
        // separate deployment evidence.
        let account = (
            profile.host.to_ascii_lowercase(),
            profile.port,
            profile.username.as_str(),
        );
        anyhow::ensure!(
            accounts.insert(account),
            MysqlPreparationProfileRefusal::SharedGatewayUser
        );
    }
    Ok(profiles)
}

pub(crate) fn load_from_retained_registry(
    file: &agentsandbox::control_fs::PrivateFile,
    load_token: impl FnMut(&Path) -> Result<String>,
) -> Result<Vec<ConfiguredTenant>> {
    file.revalidate()?;
    let result = load_registry_bytes(file.path(), file.bytes(), load_token)?;
    file.revalidate()?;
    Ok(result)
}

fn load_registry_bytes(
    path: &Path,
    bytes: &[u8],
    mut load_token: impl FnMut(&Path) -> Result<String>,
) -> Result<Vec<ConfiguredTenant>> {
    let registry = decode_tenant_registry(bytes)?;
    #[cfg(feature = "mysql-storage")]
    if registry
        .tenants
        .iter()
        .any(|tenant| matches!(tenant.storage.as_ref(), Some(TenantStorageFiles::MySql(_))))
    {
        mysql_gateway_profiles(&registry)?;
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()
        .context("Répertoire du registre inaccessible")?;
    let resolve = |value: PathBuf| -> Result<PathBuf> {
        anyhow::ensure!(!value.as_os_str().is_empty(), "Référence de fichier vide");
        Ok(if value.is_absolute() {
            value
        } else {
            parent.join(value)
        })
    };
    // An unsupported or invalid backend selection must stop the whole registry
    // before any tenant's credentials are opened.
    let tenants = registry
        .tenants
        .into_iter()
        .map(|tenant| {
            let selected = storage_for_gateway(tenant.data.as_deref(), tenant.storage.as_ref())?;
            let data = match selected {
                SelectedTenantStorage::Sqlite(path) => {
                    ConfiguredStorage::Sqlite(resolve(path.to_owned())?)
                }
                #[cfg(feature = "mysql-storage")]
                SelectedTenantStorage::MySql(profile) => {
                    ConfiguredStorage::Mysql(Box::new(MysqlTarget::load(profile, &resolve)?))
                }
                // `storage_for_gateway` already refused this profile.
                #[cfg(not(feature = "mysql-storage"))]
                SelectedTenantStorage::MySql(_) => {
                    return Err(agentsandbox::diagnostics::gateway_files::TenantStorageRefusal::MySqlUnavailable.into());
                }
            };
            Ok((tenant, data))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut ids = BTreeSet::new();
    let mut credentials = BTreeSet::new();
    let mut result = Vec::with_capacity(tenants.len());
    for (tenant, data) in tenants {
        tenant
            .worker_quota
            .validate()
            .context("Quota workers invalide")?;
        let held_create_policy = tenant
            .held_create_policy
            .map(|policy| policy.canonicalized())
            .transpose()
            .context("Politique Kubernetes du tenant invalide")?;
        let session_reservation_policy = tenant
            .session_reservation_policy
            .map(|policy| policy.canonicalized())
            .transpose()
            .context("Politique de réservation de session Kubernetes invalide")?;
        let session_recovery_policy = tenant
            .session_recovery_policy
            .map(|policy| policy.canonicalized())
            .transpose()
            .context("Politique de récupération broker de session Kubernetes invalide")?;
        let session_control_recovery_policy = tenant
            .session_control_recovery_policy
            .map(|policy| policy.canonicalized())
            .transpose()
            .context("Politique de récupération contrôle de session Kubernetes invalide")?;
        agentsandbox::tenancy::validate_tenant_id(&tenant.tenant_id)?;
        anyhow::ensure!(
            ids.insert(tenant.tenant_id.clone()),
            "Identifiant de tenant dupliqué"
        );
        let runner_token_file = resolve(tenant.runner_token_file)?;
        let viewer_token_file = resolve(tenant.viewer_token_file)?;
        let runner_token = load_token(&runner_token_file)?;
        let viewer_token = load_token(&viewer_token_file)?;
        for credential in [&runner_token, &viewer_token] {
            // Keep only hashes in the uniqueness index; raw credentials stay in
            // the configured identities and the redactor, never diagnostics.
            use sha2::Digest;
            anyhow::ensure!(
                credentials.insert(sha2::Sha256::digest(credential.as_bytes())),
                "Credentials ambigus : chaque identité doit avoir un secret distinct"
            );
        }
        result.push(ConfiguredTenant {
            tenant_id: tenant.tenant_id,
            storage: data,
            runner_token_file,
            viewer_token_file,
            runner_token,
            viewer_token,
            worker_quota: tenant.worker_quota,
            audit_capacity_policy: tenant.audit_capacity_policy,
            workload_audit_policy: tenant.workload_audit_policy,
            relay_retention_policy: tenant.relay_retention_policy,
            egress_policy: tenant.egress_policy,
            held_create_policy,
            session_reservation_policy,
            session_recovery_policy,
            session_control_recovery_policy,
            session_effect_policy: tenant.session_effect_policy,
        });
    }
    Ok(result)
}

/// Pure validation of every configured tenant before opening/creating a Store
/// or preparing any backup directory. Defaults come from the Store contract.
pub fn validate_policies(tenants: &[ConfiguredTenant], max_events: u64) -> Result<()> {
    validate_policies_for(
        tenants,
        max_events,
        agentsandbox::audit::DEFAULT_PHYSICAL_BYTES,
    )
}

/// `validate_policies` for an explicitly selected SQLite payload cap.
pub fn validate_policies_for(
    tenants: &[ConfiguredTenant],
    max_events: u64,
    physical_bytes: u64,
) -> Result<()> {
    anyhow::ensure!(
        (1..=1_000_000).contains(&max_events),
        "max-events doit être compris entre 1 et 1000000"
    );
    anyhow::ensure!(
        (agentsandbox::audit::DEFAULT_PHYSICAL_BYTES
            ..=agentsandbox::audit::MAX_SQLITE_PHYSICAL_BYTES)
            .contains(&physical_bytes),
        "audit-max-bytes doit être compris entre 256 Mio et 2 Gio"
    );
    let mut credentials = BTreeSet::new();
    for tenant in tenants {
        let physical_bytes = match &tenant.storage {
            ConfiguredStorage::Sqlite(_) => physical_bytes,
            #[cfg(feature = "mysql-storage")]
            ConfiguredStorage::Mysql(_) => {
                use agentsandbox::tenant_store::StorageCapabilityUnavailable;

                if let Some(field) = [
                    (
                        "workload_audit_policy",
                        tenant.workload_audit_policy.is_some(),
                    ),
                    (
                        "relay_retention_policy",
                        tenant.relay_retention_policy.is_some(),
                    ),
                    ("egress_policy", tenant.egress_policy.is_some()),
                    (
                        "session_effect_policy",
                        tenant.session_effect_policy.is_some(),
                    ),
                    ("held_create_policy", tenant.held_create_policy.is_some()),
                    (
                        "session_reservation_policy",
                        tenant.session_reservation_policy.is_some(),
                    ),
                    (
                        "session_recovery_policy",
                        tenant.session_recovery_policy.is_some(),
                    ),
                    (
                        "session_control_recovery_policy",
                        tenant.session_control_recovery_policy.is_some(),
                    ),
                ]
                .into_iter()
                .find_map(|(field, configured)| configured.then_some(field))
                {
                    return Err(anyhow::Error::new(StorageCapabilityUnavailable(field))
                        .context(format!("{field} requires SQLite storage")));
                }
                agentsandbox::audit::DEFAULT_PHYSICAL_BYTES
            }
        };
        let defaults =
            agentsandbox::audit::AuditCapacityPolicyV1::resolved_for(max_events, physical_bytes);
        tenant
            .worker_quota
            .validate()
            .context("Quota workers invalide")?;
        if let Some(policy) = &tenant.held_create_policy {
            policy
                .validate()
                .context("Politique Kubernetes du tenant invalide")?;
        }
        if let Some(policy) = &tenant.session_reservation_policy {
            policy
                .validate()
                .context("Politique de réservation de session Kubernetes invalide")?;
        }
        if let Some(policy) = &tenant.session_recovery_policy {
            policy
                .validate()
                .context("Politique de récupération broker de session Kubernetes invalide")?;
        }
        if let Some(policy) = &tenant.session_control_recovery_policy {
            policy
                .validate()
                .context("Politique de récupération contrôle de session Kubernetes invalide")?;
        }
        if let Some(policy) = &tenant.session_effect_policy {
            policy
                .validate()
                .context("Politique d'effets de session Kubernetes invalide")?;
        }
        agentsandbox::tenancy::validate_tenant_id(&tenant.tenant_id)?;
        for credential in [&tenant.runner_token, &tenant.viewer_token] {
            use sha2::Digest;
            anyhow::ensure!(
                credentials.insert(sha2::Sha256::digest(credential.as_bytes())),
                "Credentials ambigus : chaque identité doit avoir un secret distinct"
            );
        }
        if let Some(policy) = &tenant.audit_capacity_policy {
            policy.validate_for(max_events, physical_bytes)?;
        }
        if let Some(policy) = &tenant.workload_audit_policy {
            policy.validate_for(
                max_events,
                physical_bytes,
                tenant.audit_capacity_policy.as_ref().unwrap_or(&defaults),
            )?;
        }
        if let Some(policy) = &tenant.egress_policy {
            policy
                .snapshot()
                .context("Politique egress du tenant invalide")?;
        }
        if let Some(policy) = &tenant.relay_retention_policy {
            policy
                .validate()
                .context("Politique de rétention du relais invalide")?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "workload/gateway_config_tests.rs"]
mod workload_tests;

#[cfg(test)]
#[path = "gateway_config_policy_tests.rs"]
mod policy_tests;

#[cfg(test)]
#[path = "gateway_config_capacity_tests.rs"]
mod capacity_tests;

#[cfg(test)]
#[path = "gateway_config_diagnostics_parity_tests.rs"]
mod diagnostics_parity_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(unix)]
    pub(super) fn oidc_v1_fixture() -> (tempfile::TempDir, PathBuf, OidcFile) {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = dir.path().join("oidc.json");
        let mut document: serde_json::Value = serde_json::from_str(include_str!(
            "../docs/setup/sso/examples/gateway.https.json"
        ))
        .unwrap();
        document["version"] = json!(1);
        document["authentication_audit"]
            .as_object_mut()
            .unwrap()
            .remove("capacity_plan");
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        serde_json::to_writer(&mut output, &document).unwrap();
        let config = serde_json::from_value(document).unwrap();
        (dir, path, config)
    }

    // The real new V3 writer must be usable through the normal config consumer;
    // a stale ==2 guard rejects it before a gateway can be constructed.
    #[cfg(unix)]
    #[tokio::test]
    async fn normal_oidc_configuration_accepts_a_new_v3_audit() {
        use agentsandbox::{audit::authentication::AuthenticationAudit, redaction::Redactor};
        let (dir, path, config) = oidc_v1_fixture();
        let loaded = load_oidc(&path, true, Redactor::new(vec![]));
        let audit = AuthenticationAudit::open_existing(
            &dir.path().join(config.authentication_audit.path),
            config.authentication_audit.installation_id,
            config.authentication_audit.policy,
            Redactor::new(vec![]),
        )
        .unwrap();
        assert_eq!(audit.format_version().unwrap(), 3, "V3 writer precondition");
        assert!(
            loaded.is_ok(),
            "new V3 audit refused by normal config: {:?}",
            loaded.err().map(|error| error.to_string())
        );
    }

    // A common-wire genesis marked V2 is a synthetic old-format fixture, not a
    // production downgrade or proof of an old binary's behavior.
    #[cfg(unix)]
    #[tokio::test]
    async fn normal_oidc_configuration_refuses_v2_without_implicit_migration() {
        use agentsandbox::{audit::authentication::AuthenticationAudit, redaction::Redactor};
        let (dir, path, config) = oidc_v1_fixture();
        let data = dir.path().join(config.authentication_audit.path);
        let audit = AuthenticationAudit::init_new(
            &data,
            config.authentication_audit.installation_id,
            config.authentication_audit.policy.clone(),
            Redactor::new(vec![]),
        )
        .unwrap();
        assert_eq!(audit.format_version().unwrap(), 3, "V3 writer precondition");
        let before = audit.verify().unwrap();
        drop(audit);
        let db = rusqlite::Connection::open(&data).unwrap();
        db.pragma_update(None, "user_version", 1_000_002_i64)
            .unwrap();
        drop(db);
        let loaded = load_oidc(&path, false, Redactor::new(vec![]));
        let audit = AuthenticationAudit::open_existing(
            &data,
            config.authentication_audit.installation_id,
            config.authentication_audit.policy,
            Redactor::new(vec![]),
        )
        .unwrap();
        assert_eq!(audit.format_version().unwrap(), 2);
        assert_eq!(audit.verify().unwrap(), before);
        assert!(
            loaded.is_err(),
            "V2 normal startup must require explicit migration"
        );
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, serde_json::Value) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("runner"), "r".repeat(48)).unwrap();
        std::fs::write(dir.path().join("viewer"), "v".repeat(48)).unwrap();
        let path = dir.path().join("tenants.json");
        let value = json!({"version":1,"tenants":[{"tenant_id":"tenant-a","data":"a.sqlite","runner_token_file":"runner","viewer_token_file":"viewer"}]});
        (dir, path, value)
    }

    #[test]
    fn reads_validated_worker_quota_from_trusted_tenant_configuration() {
        let (_dir, path, mut value) = fixture();
        value["tenants"][0]["worker_quota"] =
            serde_json::to_value(agentsandbox::control::QuotaPolicyV1::default()).unwrap();
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(
            load(&path).is_ok(),
            "configured worker limits must be supported"
        );
        value["tenants"][0]["worker_quota"]["max_active_workers"] = json!(101);
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(
            load(&path).is_err(),
            "limits outside the supported range must fail"
        );
        assert!(!path.parent().unwrap().join("a.sqlite").exists());
    }

    #[test]
    fn audit_capacity_policy_is_optional_and_closed_in_tenant_registry() {
        let (_dir, path, mut value) = fixture();
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(load(&path).unwrap()[0].audit_capacity_policy.is_none());
        let capacity = |events, bytes| json!({"events": events, "bytes": bytes});
        value["tenants"][0]["audit_capacity_policy"] = json!({
            "version": 1,
            "human_tenant": capacity(50, 128 * 1024 * 1024),
            "human_principal": capacity(12, 32 * 1024 * 1024),
            "control_reserve": capacity(25, 64 * 1024 * 1024),
            "identity_reserve": capacity(12, 32 * 1024 * 1024)
        });
        std::fs::write(&path, value.to_string()).unwrap();
        load(&path).unwrap()[0]
            .audit_capacity_policy
            .as_ref()
            .unwrap()
            .validate(100)
            .unwrap();
        value["tenants"][0]["audit_capacity_policy"]["unknown"] = json!(true);
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn relay_retention_policy_is_optional_closed_and_validated_before_any_store_opens() {
        let (_dir, path, mut value) = fixture();
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(load(&path).unwrap()[0].relay_retention_policy.is_none());
        let policy = agentsandbox::relay::RelayRetentionPolicyV1 {
            sender: agentsandbox::relay::RelayCapacityV1 {
                messages: 10,
                bytes: 1_048_576,
            },
            grant: agentsandbox::relay::RelayCapacityV1 {
                messages: 5,
                bytes: 524_288,
            },
            automatic_prune_batch: 4,
            ..Default::default()
        };
        value["tenants"][0]["relay_retention_policy"] = serde_json::to_value(&policy).unwrap();
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(
            load(&path).is_ok(),
            "the tenant registry refuses a relay retention policy"
        );
        let configured = load(&path).unwrap();
        assert_eq!(configured[0].relay_retention_policy, Some(policy));
        validate_policies(&configured, 100_000).unwrap();
        // One sender able to exhaust the tenant is refused before any Store exists.
        value["tenants"][0]["relay_retention_policy"]["sender"]["messages"] = json!(513);
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(validate_policies(&load(&path).unwrap(), 100_000).is_err());
        value["tenants"][0]["relay_retention_policy"]["sender"]["messages"] = json!(10);
        value["tenants"][0]["relay_retention_policy"]["burst"] = json!(1);
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(
            load(&path).is_err(),
            "an unknown retention field was accepted"
        );
        assert!(!path.parent().unwrap().join("a.sqlite").exists());
    }

    #[test]
    fn resolves_file_references_relative_to_registry_and_does_not_create_database() {
        let (_dir, path, value) = fixture();
        std::fs::write(&path, value.to_string()).unwrap();
        let tenants = load(&path).unwrap();
        assert_eq!(tenants.len(), 1);
        assert_eq!(tenants[0].tenant_id, "tenant-a");
        assert_eq!(
            tenants[0].storage.sqlite_path().unwrap(),
            path.parent()
                .unwrap()
                .canonicalize()
                .unwrap()
                .join("a.sqlite")
        );
        assert!(!tenants[0].storage.sqlite_path().unwrap().exists());
        assert_eq!(tenants[0].runner_token, "r".repeat(48));
        assert_eq!(tenants[0].viewer_token, "v".repeat(48));
    }

    #[test]
    fn explicit_sqlite_resolves_the_same_relative_path_as_historical_data() {
        use agentsandbox::diagnostics::gateway_files::inspect_registry;

        let (_dir, path, mut registry) = fixture();
        std::fs::write(&path, registry.to_string()).unwrap();
        let expected = load(&path)
            .unwrap()
            .remove(0)
            .storage
            .sqlite_path()
            .unwrap()
            .to_owned();
        let tenant = registry["tenants"][0].as_object_mut().unwrap();
        tenant.remove("data").unwrap();
        tenant.insert(
            "storage".into(),
            json!({"backend":"sqlite","path":"a.sqlite"}),
        );
        std::fs::write(&path, registry.to_string()).unwrap();
        assert_eq!(
            load(&path)
                .unwrap()
                .remove(0)
                .storage
                .sqlite_path()
                .unwrap(),
            expected
        );
        assert!(!expected.exists());
        let inspection = inspect_registry(&path, 100_000);
        assert!(inspection.refusals.is_empty());
        assert_eq!(inspection.tenants[0].data.as_ref(), Some(&expected));
    }

    #[cfg(feature = "mysql-storage")]
    fn mysql_files(
        dir: &Path,
        mtls: bool,
    ) -> agentsandbox::diagnostics::gateway_files::MySqlStorageFiles {
        use std::os::unix::fs::PermissionsExt;
        for (name, body) in [
            ("sql-password", "pw-fixture\n".to_owned()),
            ("sql-ca.pem", "-----BEGIN CERTIFICATE-----\n".to_owned()),
            ("sql-client.pem", "-----BEGIN CERTIFICATE-----\n".to_owned()),
            ("sql-client.key", "-----BEGIN PRIVATE KEY-----\n".to_owned()),
        ] {
            let file = dir.join(name);
            std::fs::write(&file, body).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let authentication = if mtls {
            json!({"mode":"password_and_mtls","client_cert_file":dir.join("sql-client.pem"),"client_key_file":dir.join("sql-client.key")})
        } else {
            json!({"mode":"password"})
        };
        serde_json::from_value(json!({
            "host":"db.example.invalid","port":3306,"database":"audit_a",
            "username":"writer_a","namespace":"5657019d-5302-4ced-922f-406aa3d8ec5c",
            "password_file":dir.join("sql-password"),"ca_file":dir.join("sql-ca.pem"),
            "authentication":authentication
        }))
        .unwrap()
    }

    #[cfg(feature = "mysql-storage")]
    fn mysql_preparation_storage(username: &str) -> serde_json::Value {
        json!({
            "backend":"mysql","host":"db.example.invalid","port":3306,"database":"audit_a",
            "username":username,"namespace":"5657019d-5302-4ced-922f-406aa3d8ec5c",
            "password_file":"sql-password","ca_file":"sql-ca.pem",
            "authentication":{
                "mode":"password_and_mtls",
                "client_cert_file":"sql-client.pem","client_key_file":"sql-client.key"
            }
        })
    }

    #[cfg(feature = "mysql-storage")]
    fn write_mysql_preparation_registry(path: &Path, profiles: &[(&str, serde_json::Value)]) {
        use std::os::unix::fs::OpenOptionsExt;

        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        let tenants = profiles
            .iter()
            .map(|(tenant_id, storage)| {
                json!({
                    "tenant_id":tenant_id,"runner_token_file":"absent-runner",
                    "viewer_token_file":"absent-viewer","storage":storage
                })
            })
            .collect::<Vec<_>>();
        let registry = json!({"version":1,"tenants":tenants});
        serde_json::to_writer(file, &registry).unwrap();
    }

    #[cfg(feature = "mysql-storage")]
    #[test]
    fn mysql_owned_preparation_refuses_the_same_user_before_loading_material() {
        let dir = tempfile::tempdir().unwrap();
        let operator = dir.path().join("operator.json");
        let gateway = dir.path().join("gateway.json");
        write_mysql_preparation_registry(
            &operator,
            &[("tenant-a", mysql_preparation_storage("shared_user"))],
        );
        let mut gateway_storage = mysql_preparation_storage("shared_user");
        gateway_storage["password_file"] = json!("different-password-reference");
        write_mysql_preparation_registry(&gateway, &[("tenant-a", gateway_storage)]);

        let error = load_mysql_preparation_target(&operator, &gateway, "tenant-a").unwrap_err();
        assert_eq!(
            error.downcast_ref::<MysqlPreparationProfileRefusal>(),
            Some(&MysqlPreparationProfileRefusal::SharedUser)
        );
    }

    #[cfg(feature = "mysql-storage")]
    #[test]
    fn mysql_owned_preparation_refuses_a_shared_schema_in_either_registry() {
        let dir = tempfile::tempdir().unwrap();
        let operator = dir.path().join("operator.json");
        let gateway = dir.path().join("gateway.json");
        let mut conflicting_storage = mysql_preparation_storage("another_user");
        conflicting_storage["host"] = json!("DB.example.invalid");
        conflicting_storage["namespace"] = json!("6657019d-5302-4ced-922f-406aa3d8ec5c");

        for shared_registry in [&operator, &gateway] {
            let mut operator_profiles = vec![("tenant-a", mysql_preparation_storage("operator_a"))];
            let mut gateway_profiles = vec![("tenant-a", mysql_preparation_storage("gateway_a"))];
            let profiles = if shared_registry == &operator {
                &mut operator_profiles
            } else {
                &mut gateway_profiles
            };
            profiles.push(("tenant-b", conflicting_storage.clone()));
            write_mysql_preparation_registry(&operator, &operator_profiles);
            write_mysql_preparation_registry(&gateway, &gateway_profiles);

            let error = load_mysql_preparation_target(&operator, &gateway, "tenant-a").unwrap_err();
            assert_eq!(
                error.downcast_ref::<MysqlPreparationProfileRefusal>(),
                Some(&MysqlPreparationProfileRefusal::SharedSchema)
            );
        }
    }

    #[cfg(feature = "mysql-storage")]
    #[test]
    fn mysql_gateway_refuses_shared_schema_or_account_before_loading_material() {
        let dir = tempfile::tempdir().unwrap();
        let operator = dir.path().join("operator.json");
        let gateway = dir.path().join("gateway.json");
        write_mysql_preparation_registry(
            &operator,
            &[("tenant-a", mysql_preparation_storage("operator_a"))],
        );
        let primary = mysql_preparation_storage("gateway_a");
        let mut secondary = mysql_preparation_storage("gateway_b");
        secondary["host"] = json!("DB.example.invalid");
        secondary["namespace"] = json!("6657019d-5302-4ced-922f-406aa3d8ec5c");
        secondary["authentication"] = json!({"mode": "password"});

        for (database, username, expected) in [
            (
                "audit_a",
                "gateway_b",
                MysqlPreparationProfileRefusal::SharedSchema,
            ),
            (
                "audit_b",
                "gateway_a",
                MysqlPreparationProfileRefusal::SharedGatewayUser,
            ),
        ] {
            secondary["database"] = json!(database);
            secondary["username"] = json!(username);
            write_mysql_preparation_registry(
                &gateway,
                &[
                    ("tenant-a", primary.clone()),
                    ("tenant-b", secondary.clone()),
                ],
            );

            let error = load(&gateway).err().unwrap();
            assert_eq!(
                error.downcast_ref::<MysqlPreparationProfileRefusal>(),
                Some(&expected)
            );
            let error = load_mysql_preparation_target(&operator, &gateway, "tenant-a").unwrap_err();
            assert_eq!(
                error.downcast_ref::<MysqlPreparationProfileRefusal>(),
                Some(&expected)
            );
        }

        secondary["username"] = json!("gateway_b");
        write_mysql_preparation_registry(
            &gateway,
            &[
                ("tenant-a", primary.clone()),
                ("tenant-b", secondary.clone()),
            ],
        );
        let registry = decode_tenant_registry(&std::fs::read(&gateway).unwrap()).unwrap();
        assert_eq!(mysql_gateway_profiles(&registry).unwrap().len(), 2);

        secondary["username"] = json!("gateway_a");
        secondary["port"] = json!(3307);
        write_mysql_preparation_registry(
            &gateway,
            &[("tenant-a", primary), ("tenant-b", secondary)],
        );
        let registry = decode_tenant_registry(&std::fs::read(&gateway).unwrap()).unwrap();
        assert_eq!(mysql_gateway_profiles(&registry).unwrap().len(), 2);
    }

    #[cfg(feature = "mysql-storage")]
    #[test]
    fn mysql_owned_preparation_refuses_its_operator_as_another_tenants_runtime_user() {
        let dir = tempfile::tempdir().unwrap();
        let operator = dir.path().join("operator.json");
        let gateway = dir.path().join("gateway.json");
        write_mysql_preparation_registry(
            &operator,
            &[("tenant-a", mysql_preparation_storage("operator_a"))],
        );
        let mut another_runtime = mysql_preparation_storage("operator_a");
        another_runtime["host"] = json!("DB.example.invalid");
        another_runtime["database"] = json!("audit_b");
        another_runtime["namespace"] = json!("6657019d-5302-4ced-922f-406aa3d8ec5c");
        write_mysql_preparation_registry(
            &gateway,
            &[
                ("tenant-a", mysql_preparation_storage("gateway_a")),
                ("tenant-b", another_runtime),
            ],
        );

        let error = load_mysql_preparation_target(&operator, &gateway, "tenant-a").unwrap_err();
        assert_eq!(
            error.downcast_ref::<MysqlPreparationProfileRefusal>(),
            Some(&MysqlPreparationProfileRefusal::SharedUser)
        );
    }

    #[cfg(feature = "mysql-storage")]
    #[test]
    fn mysql_owned_preparation_refuses_each_target_divergence_before_loading_material() {
        let dir = tempfile::tempdir().unwrap();
        let operator = dir.path().join("operator.json");
        let gateway = dir.path().join("gateway.json");
        write_mysql_preparation_registry(
            &operator,
            &[("tenant-a", mysql_preparation_storage("operator_a"))],
        );
        for (field, divergent) in [
            ("host", json!("other.example.invalid")),
            ("port", json!(3307)),
            ("database", json!("audit_b")),
            ("namespace", json!("6657019d-5302-4ced-922f-406aa3d8ec5c")),
        ] {
            let mut gateway_storage = mysql_preparation_storage("gateway_a");
            gateway_storage[field] = divergent;
            write_mysql_preparation_registry(&gateway, &[("tenant-a", gateway_storage)]);

            let error = load_mysql_preparation_target(&operator, &gateway, "tenant-a").unwrap_err();
            assert_eq!(
                error.downcast_ref::<MysqlPreparationProfileRefusal>(),
                Some(&MysqlPreparationProfileRefusal::TargetMismatch),
                "accepted a different {field}"
            );
        }
    }

    #[cfg(feature = "mysql-storage")]
    #[test]
    fn mysql_owned_preparation_loads_only_operator_material_for_matching_profiles() {
        let operator_dir = tempfile::tempdir().unwrap();
        let gateway_dir = tempfile::tempdir().unwrap();
        let operator = operator_dir.path().join("operator.json");
        let gateway = gateway_dir.path().join("gateway.json");
        let _ = mysql_files(operator_dir.path(), true);
        let mut other_operator = mysql_preparation_storage("operator_a");
        other_operator["database"] = json!("audit_b");
        other_operator["password_file"] = json!("another-operator-password");
        write_mysql_preparation_registry(
            &operator,
            &[
                ("tenant-a", mysql_preparation_storage("operator_a")),
                ("tenant-b", other_operator),
            ],
        );
        let mut other_runtime = mysql_preparation_storage("operator_a");
        other_runtime["port"] = json!(3307);
        write_mysql_preparation_registry(
            &gateway,
            &[
                ("tenant-a", mysql_preparation_storage("gateway_a")),
                ("tenant-b", other_runtime),
            ],
        );

        let loaded = load_mysql_preparation_target(&operator, &gateway, "tenant-a").unwrap();
        assert_eq!(loaded.endpoint().username, "operator_a");
        assert_eq!(loaded.endpoint().database, "audit_a");
        assert_eq!(
            loaded.namespace.to_string(),
            "5657019d-5302-4ced-922f-406aa3d8ec5c"
        );
        assert_eq!(loaded.credentials().password, "pw-fixture");
        assert!(loaded.credentials().client_identity.is_some());
    }

    #[cfg(feature = "mysql-storage")]
    #[test]
    fn mysql_private_material_is_read_once_and_never_printed() {
        let dir = tempfile::tempdir().unwrap();
        let resolve = |path: PathBuf| -> Result<PathBuf> { Ok(path) };
        let target = MysqlTarget::load(&mysql_files(dir.path(), true), &resolve).unwrap();
        assert_eq!(target.password, "pw-fixture");
        assert!(target.client_identity.is_some());
        let debug = format!("{target:?}");
        assert!(!debug.contains("pw-fixture") && !debug.contains("PRIVATE KEY"));
        let plain = MysqlTarget::load(&mysql_files(dir.path(), false), &resolve).unwrap();
        assert!(plain.client_identity.is_none());
        // A MySQL tenant never gives an offline SQLite maintenance path.
        let storage = ConfiguredStorage::Mysql(Box::new(plain));
        assert!(storage.sqlite_path().is_err());
    }

    #[cfg(feature = "mysql-storage")]
    #[test]
    fn mysql_private_material_refuses_shared_oversized_or_malformed_files() {
        use std::os::unix::fs::PermissionsExt;
        let resolve = |path: PathBuf| -> Result<PathBuf> { Ok(path) };
        let write = |dir: &Path, name: &str, body: &[u8], mode: u32| {
            let file = dir.join(name);
            std::fs::write(&file, body).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        for (name, body, mode) in [
            ("sql-password", b"pw\n".as_slice(), 0o644),
            ("sql-password", b"pw\r\n".as_slice(), 0o600),
            ("sql-password", b"\n".as_slice(), 0o600),
            ("sql-password", b"a\x07b".as_slice(), 0o600),
            ("sql-password", &[b'p'; 1100], 0o600),
            ("sql-ca.pem", &vec![b'c'; 70_000], 0o600),
            ("sql-ca.pem", b"ca".as_slice(), 0o644),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let files = mysql_files(dir.path(), false);
            write(dir.path(), name, body, mode);
            assert!(
                MysqlTarget::load(&files, &resolve).is_err(),
                "{name} {mode:o} {} bytes",
                body.len()
            );
        }
    }

    #[test]
    fn storage_refusals_match_diagnostics_before_any_credential_is_read() {
        use agentsandbox::diagnostics::{
            catalogue::FindingCode,
            gateway_files::{TenantStorageRefusal, inspect_registry},
        };

        let (_dir, path, mut registry) = fixture();
        registry["tenants"].as_array_mut().unwrap().push(json!({
            "tenant_id":"tenant-b","runner_token_file":"second-runner","viewer_token_file":"second-viewer"
        }));
        let mysql = json!({
            "backend":"mysql","host":"db.example.invalid","port":3306,"database":"audit_b",
            "username":"writer_b","namespace":"5657019d-5302-4ced-922f-406aa3d8ec5c",
            "password_file":"sql/password","ca_file":"sql/ca.pem","authentication":{"mode":"password"}
        });
        let mut mtls = mysql.clone();
        mtls["authentication"] = json!({
            "mode":"password_and_mtls","client_cert_file":"sql/client.pem","client_key_file":"sql/client.key"
        });
        let mut incomplete_mtls = mtls.clone();
        incomplete_mtls["authentication"]
            .as_object_mut()
            .unwrap()
            .remove("client_key_file");
        let mut password_with_cert = mysql.clone();
        password_with_cert["authentication"]["client_cert_file"] = json!("sql/client.pem");
        let mut invalid_host = mysql.clone();
        invalid_host["host"] = json!("https://db.example.invalid:3306");
        let mut invalid_port = mysql.clone();
        invalid_port["port"] = json!(0);
        let mut unknown = mysql.clone();
        unknown["backend"] = json!("fixture-value-must-not-appear");
        use TenantStorageRefusal::{InvalidMySqlProfile, InvalidSelection, MySqlUnavailable};

        let mut check = |data, storage, cause| {
            let code = match cause {
                Some(MySqlUnavailable) => FindingCode::RegistryMySqlUnavailable,
                Some(_) => FindingCode::RegistryStorageInvalid,
                None => FindingCode::RegistryJsonInvalid,
            };
            let tenant = registry["tenants"][1].as_object_mut().unwrap();
            tenant.remove("data");
            tenant.remove("storage");
            for (field, entry) in [("data", data), ("storage", storage)] {
                if let Some(entry) = entry {
                    tenant.insert(field.into(), entry);
                }
            }
            let bytes = serde_json::to_vec(&registry).unwrap();
            let mut credentials_read = 0;
            let result = load_registry_bytes(&path, &bytes, |_| {
                credentials_read += 1;
                anyhow::bail!("fixture credential reader called");
            });
            assert_eq!(credentials_read, 0, "{code:?}");
            let refusal = result.err().expect("the gateway accepted the storage");
            assert_eq!(
                refusal.downcast_ref::<TenantStorageRefusal>().copied(),
                cause
            );
            if cause.is_none() {
                assert_eq!(format!("{refusal:#}"), "Registre de tenants invalide");
            }
            std::fs::write(&path, bytes).unwrap();
            let inspection = inspect_registry(&path, 100_000);
            assert_eq!(
                inspection
                    .refusals
                    .iter()
                    .map(|refusal| refusal.code)
                    .collect::<Vec<_>>(),
                [code]
            );
        };
        // Without the feature the profile is refused before any credential read.
        #[cfg(not(feature = "mysql-storage"))]
        {
            check(None, Some(mysql.clone()), Some(MySqlUnavailable));
            check(None, Some(mtls.clone()), Some(MySqlUnavailable));
        }
        check(
            Some(json!("b.sqlite")),
            Some(mysql.clone()),
            Some(InvalidSelection),
        );
        check(None, None, Some(InvalidSelection));
        check(None, Some(invalid_host), Some(InvalidMySqlProfile));
        check(Some(json!("b.sqlite")), Some(json!(null)), None);
        check(
            Some(json!(null)),
            Some(json!({"backend":"sqlite","path":"b.sqlite"})),
            None,
        );
        for malformed in [incomplete_mtls, password_with_cert, invalid_port, unknown] {
            check(None, Some(malformed), None);
        }
        assert!(!path.with_file_name("a.sqlite").exists());
        assert!(!path.with_file_name("b.sqlite").exists());
        // With the feature, a valid profile is selected: diagnostics report no
        // storage refusal and the gateway refuses only the unreadable private
        // material, never as a storage-selection refusal, before any token.
        #[cfg(feature = "mysql-storage")]
        for storage in [mysql, mtls] {
            let tenant = registry["tenants"][1].as_object_mut().unwrap();
            tenant.remove("data");
            tenant.insert("storage".into(), storage);
            let bytes = serde_json::to_vec(&registry).unwrap();
            let mut credentials_read = 0;
            let refusal = load_registry_bytes(&path, &bytes, |_| {
                credentials_read += 1;
                anyhow::bail!("fixture credential reader called");
            })
            .err()
            .expect("absent private SQL material must refuse");
            assert_eq!(credentials_read, 0);
            assert!(refusal.downcast_ref::<TenantStorageRefusal>().is_none());
            std::fs::write(&path, bytes).unwrap();
            assert!(inspect_registry(&path, 100_000).refusals.is_empty());
        }
    }

    #[test]
    fn unknown_version_fields_empty_registry_and_duplicate_identities_are_refused() {
        let (_dir, path, value) = fixture();
        let mut cases = vec![
            json!({"version":2,"tenants":[]}),
            json!({"version":1,"tenants":[]}),
        ];
        let mut unknown = value.clone();
        unknown["tenants"][0]["runner_token"] = json!("inline-secret-not-permitted");
        cases.push(unknown);
        let mut duplicate = value.clone();
        duplicate["tenants"]
            .as_array_mut()
            .unwrap()
            .push(value["tenants"][0].clone());
        cases.push(duplicate);
        let mut ambiguous = value;
        ambiguous["tenants"][0]["viewer_token_file"] = json!("runner");
        cases.push(ambiguous);
        for case in cases {
            std::fs::write(&path, case.to_string()).unwrap();
            assert!(load(&path).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_credential_and_registry_are_refused() {
        let (_dir, path, mut value) = fixture();
        let link = path.with_file_name("runner-link");
        std::os::unix::fs::symlink(path.with_file_name("runner"), &link).unwrap();
        value["tenants"][0]["runner_token_file"] = json!("runner-link");
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(load(&path).is_err());
        let registry_link = path.with_file_name("registry-link");
        std::os::unix::fs::symlink(&path, &registry_link).unwrap();
        assert!(load(&registry_link).is_err());
    }

    #[cfg(feature = "mysql-storage")]
    #[test]
    fn mysql_preflight_refuses_sqlite_only_kubernetes_policies() {
        use agentsandbox::tenant_store::StorageCapabilityUnavailable;

        let (dir, path, registry) = fixture();
        let files = mysql_files(dir.path(), false);
        let target = MysqlTarget::load(&files, &Ok).unwrap();
        let storage = ConfiguredStorage::Mysql(Box::new(target));

        std::fs::write(&path, registry.to_string()).unwrap();
        let mut tenants = load(&path).unwrap();
        tenants[0].storage = storage.clone();
        validate_policies(&tenants, 10_000).expect("MySQL supports the default policies");

        let recovery = json!({"version": 2, "decision_ttl_seconds": 30, "allowed_broker_ids": []});
        for (field, policy) in [
            (
                "held_create_policy",
                json!({
                    "version": 1,
                    "decision_ttl_seconds": 30,
                    "approvals": [{
                        "authority_fingerprint": "a".repeat(64),
                        "image": format!("worker-a@sha256:{}", "d".repeat(64)),
                        "profile": "kubernetes_held_pod_v1"
                    }]
                }),
            ),
            (
                "session_reservation_policy",
                json!({"version": 2, "decision_ttl_seconds": 30, "approvals": []}),
            ),
            ("session_recovery_policy", recovery.clone()),
            ("session_control_recovery_policy", recovery),
        ] {
            let mut configured = registry.clone();
            configured["tenants"][0][field] = policy;
            std::fs::write(&path, configured.to_string()).unwrap();
            let mut tenants = load(&path).unwrap();
            validate_policies(&tenants, 10_000).expect("SQLite supports the configured policy");
            tenants[0].storage = storage.clone();
            let refusal = validate_policies(&tenants, 10_000).unwrap_err();
            assert_eq!(
                refusal.downcast_ref::<StorageCapabilityUnavailable>(),
                Some(&StorageCapabilityUnavailable(field))
            );
            assert_eq!(
                refusal.to_string(),
                format!("{field} requires SQLite storage")
            );
        }
        assert!(!dir.path().join("a.sqlite").exists());
    }

    #[cfg(feature = "mysql-storage")]
    #[test]
    fn owned_sources_are_refused_explicitly_on_a_mysql_tenant() {
        let dir = tempfile::tempdir().unwrap();
        let resolve = |path: PathBuf| -> Result<PathBuf> { Ok(path) };
        let target = MysqlTarget::load(&mysql_files(dir.path(), false), &resolve).unwrap();
        let (_dir, path, value) = fixture();
        std::fs::write(&path, value.to_string()).unwrap();
        std::fs::write(path.with_file_name("runner"), "r".repeat(48)).unwrap();
        let mut tenants = load(&path).unwrap();
        refuse_owned_sources_on_mysql(&tenants, true).unwrap();
        tenants[0].storage = ConfiguredStorage::Mysql(Box::new(target));
        refuse_owned_sources_on_mysql(&tenants, false).unwrap();
        let refusal = refuse_owned_sources_on_mysql(&tenants, true).unwrap_err();
        assert!(
            refusal
                .to_string()
                .contains("not available on a MySQL tenant")
        );
    }
}
