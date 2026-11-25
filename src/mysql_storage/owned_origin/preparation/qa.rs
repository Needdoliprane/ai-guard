use super::{EmptyTargetOperation, MysqlOwnedPreparationRefusal, prepare_empty_owned_target};
use crate::{
    control_fs::PrivateFile,
    diagnostics::gateway_files::{
        MySqlAuthenticationFiles, SelectedTenantStorage, decode_tenant_registry,
        read_tenant_registry_file, select_tenant_storage,
    },
    mysql_storage::{
        MAX_CERTIFICATE_BYTES, MAX_PASSWORD_BYTES, MAX_PRIVATE_KEY_BYTES, MysqlClientIdentity,
        MysqlCredentials, MysqlEndpoint, audit::MysqlAuditStore,
    },
    redaction::Redactor,
};
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires fresh dedicated loopback MySQL 8.4 QA schemas and TLS operator credentials"]
async fn owned_audit_tables_remain_unpublished_and_refuse_reentry() -> Result<()> {
    let server = Uuid::parse_str(&std::env::var("AI_GUARD_MYSQL_OWNED_AUDIT_QA_SERVER_UUID")?)?;
    for (version, env) in [
        (14, "AI_GUARD_MYSQL_OWNED_AUDIT_14_QA_REGISTRY"),
        (15, "AI_GUARD_MYSQL_OWNED_AUDIT_15_QA_REGISTRY"),
    ] {
        let registry_path = PathBuf::from(std::env::var_os(env).context("QA registry required")?);
        let directory = registry_path
            .parent()
            .context("QA registry directory required")?;
        let registry = decode_tenant_registry(&read_tenant_registry_file(&registry_path)?)?;
        let [tenant] = registry.tenants.as_slice() else {
            anyhow::bail!("QA requires exactly one tenant per registry");
        };
        crate::tenancy::validate_tenant_id(&tenant.tenant_id)?;
        let SelectedTenantStorage::MySql(profile) =
            select_tenant_storage(tenant.data.as_deref(), tenant.storage.as_ref())?
        else {
            anyhow::bail!("QA requires MySQL storage");
        };
        let namespace = profile.namespace.simple().to_string();
        ensure!(
            profile.host == "localhost"
                && !profile.namespace.is_nil()
                && profile.database == format!("qa_audit_{namespace}")
                && profile.username == format!("qa_m_{}", &namespace[..24]),
            "QA requires a dedicated loopback schema and operator account"
        );
        let open = |path: &Path, max: usize| PrivateFile::open(&directory.join(path), max as u64);
        let password = open(&profile.password_file, MAX_PASSWORD_BYTES)?;
        let ca = open(&profile.ca_file, MAX_CERTIFICATE_BYTES)?;
        let client_material = match &profile.authentication {
            MySqlAuthenticationFiles::Password {} => None,
            MySqlAuthenticationFiles::PasswordAndMtls {
                client_cert_file,
                client_key_file,
            } => Some((
                open(client_cert_file, MAX_CERTIFICATE_BYTES)?,
                open(client_key_file, MAX_PRIVATE_KEY_BYTES)?,
            )),
        };
        let password = std::str::from_utf8(password.bytes())?.trim_end_matches(['\r', '\n']);
        let endpoint = || MysqlEndpoint {
            host: &profile.host,
            port: profile.port.get(),
            database: &profile.database,
            username: &profile.username,
        };
        let credentials = || MysqlCredentials {
            password,
            ca_pem: ca.bytes(),
            client_identity: client_material.as_ref().map(|(certificate, key)| {
                MysqlClientIdentity {
                    certificate_chain_pem: certificate.bytes(),
                    private_key_pem: key.bytes(),
                }
            }),
        };
        let install = || {
            prepare_empty_owned_target(
                endpoint(),
                credentials(),
                server,
                version,
                EmptyTargetOperation::InstallAuditTables,
            )
        };
        install().await?;
        let refusal = install()
            .await
            .err()
            .context("unpublished target was accepted")?;
        ensure!(
            refusal.downcast_ref::<MysqlOwnedPreparationRefusal>()
                == Some(&MysqlOwnedPreparationRefusal::TargetNotEmpty),
            "expected a target inventory refusal"
        );
        let legacy = MysqlAuditStore::new(
            endpoint(),
            credentials(),
            &tenant.tenant_id,
            profile.namespace,
            Redactor::new(Vec::new()),
            128,
        )?;
        for error in [legacy.verify().await.err(), legacy.initialize().await.err()] {
            ensure!(
                error
                    .context("Legacy accepted the unpublished target")?
                    .to_string()
                    == "MySQL audit schema was not prepared"
            );
        }
    }
    Ok(())
}
