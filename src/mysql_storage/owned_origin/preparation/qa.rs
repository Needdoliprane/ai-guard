use super::super::fresh_anchor::{ConfiguredTenantNamespace, FreshAnchorRequestV1};
use super::{
    EmptyTargetOperation, MysqlOwnedPreparationRefusal, prepare_empty_owned_target,
    prepare_owned_fresh_source,
};
use crate::{
    control_fs::PrivateFile,
    diagnostics::gateway_files::{
        MySqlAuthenticationFiles, SelectedTenantStorage, decode_tenant_registry,
        read_tenant_registry_file, select_tenant_storage,
    },
    mysql_storage::{
        MAX_CERTIFICATE_BYTES, MAX_PASSWORD_BYTES, MAX_PRIVATE_KEY_BYTES, MysqlClientIdentity,
        MysqlCredentials, MysqlEndpoint, MysqlProbeError, audit::MysqlAuditStore,
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
        let anchor = FreshAnchorRequestV1 {
            operation_id: Uuid::from_u128(300 + u128::from(version)),
            schema_version: version,
            tenant_id: tenant.tenant_id.clone(),
            namespace_id: profile.namespace,
            installation_id: Uuid::from_u128(400),
            source_id: Uuid::from_u128(400 + u128::from(version)),
            server_uuid: server,
            schema: profile.database.clone(),
        };
        let redactor = Redactor::new(Vec::new());
        let install = || {
            prepare_empty_owned_target(
                endpoint(),
                credentials(),
                server,
                version,
                EmptyTargetOperation::InstallUnpublishedTables {
                    request: &anchor,
                    configured: ConfiguredTenantNamespace {
                        tenant_id: &tenant.tenant_id,
                        namespace_id: profile.namespace,
                    },
                    redactor: &redactor,
                },
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
        let require_refusal = |operation: &str, error: Option<anyhow::Error>| -> Result<()> {
            let error = error.with_context(|| {
                format!("Legacy accepted the unpublished target ({operation}, Store{version})")
            })?;
            ensure!(
                !matches!(
                    error.downcast_ref::<MysqlProbeError>(),
                    Some(MysqlProbeError::ServerRefused(1142))
                ),
                "Legacy {operation} on Store{version}: QA privilege refusal (MySQL 1142)"
            );
            ensure!(
                error.to_string() == "MySQL audit schema was not prepared",
                "Legacy {operation} on Store{version} did not observe the unpublished-state refusal"
            );
            Ok(())
        };
        require_refusal("verify", legacy.verify().await.err())?;
        require_refusal("initialize", legacy.initialize().await.err())?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires fresh dedicated loopback MySQL 8.4 QA schemas and TLS operator credentials"]
async fn owned_fresh_source_publishes_once_and_refuses_reentry() -> Result<()> {
    let server = Uuid::parse_str(&std::env::var("AI_GUARD_MYSQL_OWNED_AUDIT_QA_SERVER_UUID")?)?;
    for (version, env) in [
        (14, "AI_GUARD_MYSQL_OWNED_PUBLISH_14_QA_REGISTRY"),
        (15, "AI_GUARD_MYSQL_OWNED_PUBLISH_15_QA_REGISTRY"),
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
        let anchor = FreshAnchorRequestV1 {
            operation_id: Uuid::from_u128(500 + u128::from(version)),
            schema_version: version,
            tenant_id: tenant.tenant_id.clone(),
            namespace_id: profile.namespace,
            installation_id: Uuid::from_u128(600),
            source_id: Uuid::from_u128(600 + u128::from(version)),
            server_uuid: server,
            schema: profile.database.clone(),
        };
        let configured = ConfiguredTenantNamespace {
            tenant_id: &tenant.tenant_id,
            namespace_id: profile.namespace,
        };
        let redactor = Redactor::new(Vec::new());
        let publish = || {
            prepare_owned_fresh_source(
                endpoint(),
                credentials(),
                server,
                version,
                &anchor,
                configured,
                &redactor,
            )
        };
        let receipt = publish().await?;
        ensure!(
            receipt.target_schema_version == version && receipt.head_seq == 2,
            "fresh publication did not pose the genesis and anchor"
        );
        ensure!(
            receipt.head_hash.len() == 64,
            "fresh publication head is not a digest"
        );

        // A second attempt must refuse the now-nonempty target without effect.
        let refusal = publish()
            .await
            .err()
            .context("published target was accepted")?;
        ensure!(
            refusal.downcast_ref::<MysqlOwnedPreparationRefusal>()
                == Some(&MysqlOwnedPreparationRefusal::TargetNotEmpty),
            "expected a target inventory refusal on reentry"
        );

        // The old gateway remains closed to the owned published version.
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
                    .context("Legacy accepted the owned published target")?
                    .to_string()
                    == "unsupported MySQL audit schema version"
            );
        }
    }
    Ok(())
}
