//! Trusted operator configuration. Request data never selects these scopes.
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
}
/// A separate explicit operator file. Existing tenant registry format stays closed.
pub fn load_oidc(
    path: &Path,
    initialize_new: bool,
    redactor: agentsandbox::redaction::Redactor,
) -> Result<agentsandbox::api::OidcGateway> {
    use agentsandbox::{
        audit::authentication::AuthenticationAudit,
        control_fs::{PrivateFile, absolute_path},
    };
    let path = absolute_path(path)?;
    let held = PrivateFile::open(&path, 1_048_576)?;
    let file: OidcFile =
        serde_json::from_slice(held.bytes()).context("Invalid OIDC gateway file")?;
    anyhow::ensure!(file.version == 1, "Unsupported OIDC gateway file version");
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
        .map(|path| -> Result<String> {
            let ca = PrivateFile::open(&resolve(path)?, 262_144)?;
            String::from_utf8(ca.bytes().to_vec()).context("Invalid public OIDC CA bundle")
        })
        .transpose()?;
    if let Some(pem) = &ca_bundle_pem {
        agentsandbox::oidc::OidcProvider::new_with_ca(
            file.gateway.provider.clone(),
            pem.as_bytes(),
        )?;
    }
    let audit_path = resolve(file.authentication_audit.path)?;
    let audit = if initialize_new {
        AuthenticationAudit::init_new(
            &audit_path,
            file.authentication_audit.installation_id,
            file.authentication_audit.policy,
            redactor,
        )?
    } else {
        AuthenticationAudit::open_existing(
            &audit_path,
            file.authentication_audit.installation_id,
            file.authentication_audit.policy,
            redactor,
        )?
    };
    held.revalidate()?;
    agentsandbox::api::OidcGateway::new(file.gateway, std::sync::Arc::new(audit), ca_bundle_pem)
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
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
