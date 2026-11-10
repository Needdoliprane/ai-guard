//! Trusted operator configuration. Request data never selects these scopes.
use agentsandbox::control::kubernetes_session::{
    KubernetesSessionControlRecoveryPolicyV2, KubernetesSessionRecoveryPolicyV2,
    KubernetesSessionReservationPolicyV2,
};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    version: u32,
    tenants: Vec<TenantFiles>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TenantFiles {
    tenant_id: String,
    data: PathBuf,
    runner_token_file: PathBuf,
    viewer_token_file: PathBuf,
    #[serde(default)]
    worker_quota: agentsandbox::control::QuotaPolicyV1,
    audit_capacity_policy: Option<agentsandbox::audit::AuditCapacityPolicyV1>,
    workload_audit_policy: Option<agentsandbox::audit::WorkloadAuditPolicyV1>,
    held_create_policy:
        Option<agentsandbox::control::kubernetes_admission::KubernetesHeldCreatePolicyV1>,
    session_reservation_policy: Option<KubernetesSessionReservationPolicyV2>,
    session_recovery_policy: Option<KubernetesSessionRecoveryPolicyV2>,
    session_control_recovery_policy: Option<KubernetesSessionControlRecoveryPolicyV2>,
}

pub struct ConfiguredTenant {
    pub tenant_id: String,
    pub data: PathBuf,
    pub runner_token_file: PathBuf,
    pub viewer_token_file: PathBuf,
    pub runner_token: String,
    pub viewer_token: String,
    pub worker_quota: agentsandbox::control::QuotaPolicyV1,
    pub audit_capacity_policy: Option<agentsandbox::audit::AuditCapacityPolicyV1>,
    pub workload_audit_policy: Option<agentsandbox::audit::WorkloadAuditPolicyV1>,
    pub held_create_policy:
        Option<agentsandbox::control::kubernetes_admission::KubernetesHeldCreatePolicyV1>,
    pub session_reservation_policy: Option<KubernetesSessionReservationPolicyV2>,
    pub session_recovery_policy: Option<KubernetesSessionRecoveryPolicyV2>,
    pub session_control_recovery_policy: Option<KubernetesSessionControlRecoveryPolicyV2>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OidcFile {
    version: u32,
    gateway: agentsandbox::api::OidcGatewayConfigV1,
    authentication_audit: AuthenticationAuditFile,
    ca_bundle_file: Option<PathBuf>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthenticationAuditFile {
    path: PathBuf,
    installation_id: uuid::Uuid,
    policy: agentsandbox::audit::authentication::AuthenticationAuditPolicyV1,
    capacity_plan: Option<agentsandbox::audit::authentication::AuthenticationCapacityPlanV1>,
}

struct PreparedOidcFile {
    held: agentsandbox::control_fs::PrivateFile,
    file: OidcFile,
    audit_path: PathBuf,
    ca_bundle_pem: Option<String>,
}

/// Closed, private configuration and local references only. Provider validation
/// constructs an offline transport; it never discovers or contacts the IdP.
fn prepare_oidc_file(path: &Path) -> Result<PreparedOidcFile> {
    use agentsandbox::control_fs::{PrivateFile, absolute_path};
    let path = absolute_path(path)?;
    let held = PrivateFile::open(&path, 1_048_576)?;
    let file: OidcFile =
        serde_json::from_slice(held.bytes()).context("Invalid OIDC gateway file")?;
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
    let ca_bundle_pem = file
        .ca_bundle_file
        .as_ref()
        .map(|path| -> Result<String> {
            let ca = PrivateFile::open(&resolve(path.clone())?, 262_144)?;
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
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .context("Registre de tenants inaccessible")?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "Le registre doit être un fichier régulier"
    );
    let mut bytes = Vec::new();
    file.take(1_048_577).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= 1_048_576,
        "Registre de tenants trop volumineux"
    );
    let registry: Registry =
        serde_json::from_slice(&bytes).context("Registre de tenants invalide")?;
    anyhow::ensure!(
        registry.version == 1,
        "Version de registre de tenants inconnue"
    );
    anyhow::ensure!(
        !registry.tenants.is_empty(),
        "Le registre de tenants est vide"
    );
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
    let mut ids = BTreeSet::new();
    let mut credentials = BTreeSet::new();
    let mut result = Vec::new();
    for tenant in registry.tenants {
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
        let data = resolve(tenant.data)?;
        let runner_token_file = resolve(tenant.runner_token_file)?;
        let viewer_token_file = resolve(tenant.viewer_token_file)?;
        let runner_token = crate::read_token(&runner_token_file)?;
        let viewer_token = crate::read_token(&viewer_token_file)?;
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
            data,
            runner_token_file,
            viewer_token_file,
            runner_token,
            viewer_token,
            worker_quota: tenant.worker_quota,
            audit_capacity_policy: tenant.audit_capacity_policy,
            workload_audit_policy: tenant.workload_audit_policy,
            held_create_policy,
            session_reservation_policy,
            session_recovery_policy,
            session_control_recovery_policy,
        });
    }
    Ok(result)
}

/// Pure validation of every configured tenant before opening/creating a Store
/// or preparing any backup directory. Defaults come from the Store contract.
pub fn validate_policies(tenants: &[ConfiguredTenant], max_events: u64) -> Result<()> {
    anyhow::ensure!(
        (1..=1_000_000).contains(&max_events),
        "max-events doit être compris entre 1 et 1000000"
    );
    let defaults = agentsandbox::audit::AuditCapacityPolicyV1::resolved(max_events);
    let mut credentials = BTreeSet::new();
    for tenant in tenants {
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
        agentsandbox::tenancy::validate_tenant_id(&tenant.tenant_id)?;
        for credential in [&tenant.runner_token, &tenant.viewer_token] {
            use sha2::Digest;
            anyhow::ensure!(
                credentials.insert(sha2::Sha256::digest(credential.as_bytes())),
                "Credentials ambigus : chaque identité doit avoir un secret distinct"
            );
        }
        if let Some(policy) = &tenant.audit_capacity_policy {
            policy.validate(max_events)?;
        }
        if let Some(policy) = &tenant.workload_audit_policy {
            policy.validate(
                max_events,
                tenant.audit_capacity_policy.as_ref().unwrap_or(&defaults),
            )?;
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
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(unix)]
    pub(super) fn oidc_fixture() -> (tempfile::TempDir, PathBuf, OidcFile) {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = dir.path().join("oidc.json");
        let bytes = include_bytes!("../docs/setup/sso/examples/gateway.https.json");
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut output, bytes).unwrap();
        let config = serde_json::from_slice(bytes).unwrap();
        (dir, path, config)
    }

    // The real new V3 writer must be usable through the normal config consumer;
    // a stale ==2 guard rejects it before a gateway can be constructed.
    #[cfg(unix)]
    #[tokio::test]
    async fn normal_oidc_configuration_accepts_a_new_v3_audit() {
        use agentsandbox::{audit::authentication::AuthenticationAudit, redaction::Redactor};
        let (dir, path, config) = oidc_fixture();
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
        let (dir, path, config) = oidc_fixture();
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
    fn resolves_file_references_relative_to_registry_and_does_not_create_database() {
        let (_dir, path, value) = fixture();
        std::fs::write(&path, value.to_string()).unwrap();
        let tenants = load(&path).unwrap();
        assert_eq!(tenants.len(), 1);
        assert_eq!(tenants[0].tenant_id, "tenant-a");
        assert_eq!(
            tenants[0].data,
            path.parent()
                .unwrap()
                .canonicalize()
                .unwrap()
                .join("a.sqlite")
        );
        assert!(!tenants[0].data.exists());
        assert_eq!(tenants[0].runner_token, "r".repeat(48));
        assert_eq!(tenants[0].viewer_token, "v".repeat(48));
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
}
