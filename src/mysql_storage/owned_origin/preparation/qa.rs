use super::super::fresh_anchor::{ConfiguredTenantNamespace, FreshAnchorRequestV1};
use super::{
    EmptyTargetOperation, FreshSourceMode, MysqlOwnedPreparationRefusal, OwnedAuditLock,
    prepare_empty_owned_target, prepare_owned_fresh_source,
};
use crate::{
    control_fs::PrivateFile,
    diagnostics::gateway_files::{
        MySqlAuthenticationFiles, SelectedTenantStorage, decode_tenant_registry,
        read_tenant_registry_file, select_tenant_storage,
    },
    mysql_storage::{
        MAX_CERTIFICATE_BYTES, MAX_PASSWORD_BYTES, MAX_PRIVATE_KEY_BYTES, MysqlClientIdentity,
        MysqlCredentials, MysqlEndpoint, MysqlProbeError, audit::MysqlAuditStore, connect_options,
        driver_error, open_verified,
    },
    redaction::Redactor,
};
use anyhow::{Context, Result, ensure};
use mysql_async::prelude::Queryable;
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
        let configured = ConfiguredTenantNamespace {
            tenant_id: &tenant.tenant_id,
            namespace_id: profile.namespace,
        };
        let install = || {
            prepare_empty_owned_target(
                endpoint(),
                credentials(),
                server,
                version,
                EmptyTargetOperation::InstallUnpublishedTables {
                    request: &anchor,
                    configured,
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
        let refusal = prepare_owned_fresh_source(
            endpoint(),
            credentials(),
            &anchor,
            configured,
            &redactor,
            FreshSourceMode::PublishOrVerify,
        )
        .await
        .err()
        .context("unpublished target yielded a publication receipt")?;
        ensure!(
            refusal.to_string() == "MySQL owned publication singleton is absent",
            "expected read-only refusal for the unpublished target"
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
async fn owned_fresh_source_publishes_once_and_replays_exactly() -> Result<()> {
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
                &anchor,
                configured,
                &redactor,
                FreshSourceMode::PublishOrVerify,
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

        // Simulate a lost receipt by resubmitting the exact operation.
        let replay = publish()
            .await
            .context("exact publication replay refused")?;
        ensure!(
            replay.target_schema_version == version
                && replay.head_seq == receipt.head_seq
                && replay.head_hash == receipt.head_hash,
            "exact replay did not return the verified publication head"
        );

        for (field, divergent) in [
            (
                "operation",
                FreshAnchorRequestV1 {
                    operation_id: Uuid::from_u128(701),
                    ..anchor.clone()
                },
            ),
            (
                "installation",
                FreshAnchorRequestV1 {
                    installation_id: Uuid::from_u128(702),
                    ..anchor.clone()
                },
            ),
            (
                "source",
                FreshAnchorRequestV1 {
                    source_id: Uuid::from_u128(703),
                    ..anchor.clone()
                },
            ),
            (
                "namespace",
                FreshAnchorRequestV1 {
                    namespace_id: Uuid::from_u128(704),
                    ..anchor.clone()
                },
            ),
            (
                "tenant",
                FreshAnchorRequestV1 {
                    tenant_id: "foreign-qa-tenant".into(),
                    ..anchor.clone()
                },
            ),
            (
                "server",
                FreshAnchorRequestV1 {
                    server_uuid: Uuid::from_u128(705),
                    ..anchor.clone()
                },
            ),
            (
                "format",
                FreshAnchorRequestV1 {
                    schema_version: if version == 14 { 15 } else { 14 },
                    ..anchor.clone()
                },
            ),
        ] {
            let divergent_scope = ConfiguredTenantNamespace {
                tenant_id: &divergent.tenant_id,
                namespace_id: divergent.namespace_id,
            };
            ensure!(
                prepare_owned_fresh_source(
                    endpoint(),
                    credentials(),
                    &divergent,
                    divergent_scope,
                    &redactor,
                    FreshSourceMode::PublishOrVerify,
                )
                .await
                .is_err(),
                "divergent {field} yielded a publication receipt"
            );
        }
        let after_refusals = publish().await?;
        ensure!(
            after_refusals.head_seq == receipt.head_seq
                && after_refusals.head_hash == receipt.head_hash,
            "divergent replays changed the publication head"
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

        // A projection that was never published must prevent a recovery
        // receipt even when the audit chain and recorded head still match.
        let (options, _) = connect_options(&endpoint(), credentials())?;
        let (mut witness, _) =
            open_verified(options.into(), &profile.database, &profile.username).await?;
        let witness_lock = OwnedAuditLock::acquire(&mut witness, server, &profile.database).await?;
        witness
            .exec_drop(
                "INSERT INTO review_authority_meta \
                 (singleton, source_id, anchor_seq, anchor_hash, incarnation_revision, \
                  snapshot_json, snapshot_hash) VALUES (1, ?, 2, ?, 1, '{}', ?)",
                (
                    anchor.source_id.to_string(),
                    &receipt.head_hash,
                    "0".repeat(64),
                ),
            )
            .await
            .map_err(driver_error)?;
        witness_lock.release(&mut witness).await?;
        ensure!(
            publish()
                .await
                .err()
                .context("divergent projection was accepted")?
                .to_string()
                == "MySQL fresh owned common projections contain rows",
            "expected refusal of the divergent projection"
        );
        let unchanged = witness
            .query_first::<(i64, i64, String), _>(
                "SELECT r.incarnation_revision, s.head_seq, s.head_hash \
                 FROM review_authority_meta r JOIN ai_guard_audit_state s ON s.singleton=r.singleton",
            )
            .await
            .map_err(driver_error)?;
        ensure!(
            unchanged == Some((1, receipt.head_seq, receipt.head_hash)),
            "refusing a divergent projection changed the retained state"
        );
        witness.disconnect().await.map_err(driver_error)?;
    }
    Ok(())
}
