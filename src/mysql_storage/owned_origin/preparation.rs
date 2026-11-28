//! Operator publication and audit-chain inspection for an owned MySQL schema.

use super::{
    OwnedAuditLock, decode_row, driver_error,
    fresh_anchor::{ConfiguredTenantNamespace, FreshAnchorRequestV1},
};
use crate::mysql_storage::{
    MysqlAuthentication, MysqlCredentials, MysqlEndpoint, MysqlProbeError, PROBE_TIMEOUT,
    connect_options, open_verified,
};
use crate::redaction::Redactor;
use anyhow::{Context, Result, ensure};
use mysql_async::{Conn, Row, prelude::Queryable};
use serde::Serialize;
use std::fmt;
use uuid::Uuid;

mod audit_manifest;
mod audit_tables;
mod egress_manifest;
mod publication;
#[cfg(test)]
mod qa;
mod supervision_manifest;
mod worker_manifest;

const REQUIRED_GRANTS: [&str; 6] = [
    "SELECT",
    "SHOW VIEW",
    "ALTER ROUTINE",
    "EVENT",
    "TRIGGER",
    "CREATE",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MysqlOwnedPreparationRefusal {
    TargetNotEmpty,
    PublicationMissing,
    CatalogVisibilityUnconfirmed,
    IncompatibleServer,
}

impl fmt::Display for MysqlOwnedPreparationRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TargetNotEmpty => {
                "MySQL owned preparation requires an empty schema; preserve this target"
            }
            Self::PublicationMissing => "MySQL owned verification found an empty schema",
            Self::CatalogVisibilityUnconfirmed => {
                "MySQL owned preparation cannot prove complete catalog visibility with this account"
            }
            Self::IncompatibleServer => "MySQL owned preparation requires writable durable InnoDB",
        })
    }
}

impl std::error::Error for MysqlOwnedPreparationRefusal {}

/// An observation made under the schema lock and returned after disconnect.
/// It cannot authorize later DDL.
#[derive(Serialize)]
pub struct MysqlOwnedPreparationPreflight {
    pub verification_mode: &'static str,
    pub target_schema_version: u16,
    pub configured_authentication: MysqlAuthentication,
    pub tls_version: String,
}

/// Check the exact MySQL target on a dedicated TLS connection. Any future
/// preparation must repeat the check on the connection that will perform DDL,
/// under the same physical-schema lock. An interrupted target remains in place.
pub async fn preflight_owned_preparation(
    endpoint: MysqlEndpoint<'_>,
    credentials: MysqlCredentials<'_>,
    server: Uuid,
    schema_version: u16,
) -> Result<MysqlOwnedPreparationPreflight> {
    let (configured_authentication, tls_version, _) = prepare_empty_owned_target(
        endpoint,
        credentials,
        server,
        schema_version,
        EmptyTargetOperation::Inspect,
    )
    .await?;
    Ok(MysqlOwnedPreparationPreflight {
        verification_mode: "owned_preparation_preflight",
        target_schema_version: schema_version,
        configured_authentication,
        tls_version,
    })
}

/// The durable receipt of a fresh owned publication. It attests the verified
/// head observed after COMMIT under the physical lock; it grants no runtime
/// admission until the gateway opens the configured target independently.
#[derive(Serialize)]
pub struct MysqlOwnedPreparationReceipt {
    pub verification_mode: &'static str,
    pub target_schema_version: u16,
    pub head_seq: i64,
    pub head_hash: String,
    pub configured_authentication: MysqlAuthentication,
    pub tls_version: String,
}

/// Read-only verification of the catalog, exact origin, audit chain and any
/// stored Review authority reference to the creation anchor.
/// The receipt carries no projection, epoch or writer-admission authority.
#[derive(Serialize)]
pub struct MysqlOwnedAuditChainReceipt {
    pub verification_mode: &'static str,
    pub target_schema_version: u16,
    pub head_seq: i64,
    pub head_hash: String,
    pub payload_bytes: u64,
    pub configured_authentication: MysqlAuthentication,
    pub tls_version: String,
}

/// The identity of one explicit publication against a configured operator target.
pub struct MysqlOwnedPreparationRequest<'a> {
    pub tenant_id: &'a str,
    pub namespace_id: Uuid,
    pub schema_version: u16,
    pub server_uuid: Uuid,
    pub operation_id: Uuid,
    pub installation_id: Uuid,
    pub source_id: Uuid,
}

impl MysqlOwnedPreparationRequest<'_> {
    fn anchor(&self, database: &str) -> FreshAnchorRequestV1 {
        FreshAnchorRequestV1 {
            operation_id: self.operation_id,
            schema_version: self.schema_version,
            tenant_id: self.tenant_id.into(),
            namespace_id: self.namespace_id,
            installation_id: self.installation_id,
            source_id: self.source_id,
            server_uuid: self.server_uuid,
            schema: database.into(),
        }
    }

    fn configured(&self) -> ConfiguredTenantNamespace<'_> {
        ConfiguredTenantNamespace {
            tenant_id: self.tenant_id,
            namespace_id: self.namespace_id,
        }
    }
}

enum FreshSourceMode {
    PublishOrVerify,
    VerifyOnly,
}

/// Publish on a dedicated empty schema or verify the same fresh publication
/// read-only. Resolve the endpoint and scope from trusted operator configuration.
pub async fn publish_owned_fresh_source(
    endpoint: MysqlEndpoint<'_>,
    credentials: MysqlCredentials<'_>,
    request: MysqlOwnedPreparationRequest<'_>,
) -> Result<MysqlOwnedPreparationReceipt> {
    prepare_configured_fresh_source(
        endpoint,
        credentials,
        request,
        FreshSourceMode::PublishOrVerify,
    )
    .await
}

/// Verify an existing fresh publication on the exact configured operator target.
/// An empty schema is refused before any installation can occur.
pub async fn verify_owned_fresh_source(
    endpoint: MysqlEndpoint<'_>,
    credentials: MysqlCredentials<'_>,
    request: MysqlOwnedPreparationRequest<'_>,
) -> Result<MysqlOwnedPreparationReceipt> {
    prepare_configured_fresh_source(endpoint, credentials, request, FreshSourceMode::VerifyOnly)
        .await
}

/// Inspect an already published owned source under the physical target lock.
/// Compare the exact creation request and stream every event in one read-only
/// snapshot. In that snapshot, require any Review authority reference to match
/// the creation anchor. The caller needs separate projection and epoch proofs.
/// Coordinate schema administration with the target lock. The catalog observes
/// unrelated objects created outside that lock at the time of inspection.
/// A timeout bounds the caller's wait. It does not attest that MySQL has
/// released the session's target lock.
pub async fn verify_owned_audit_chain(
    endpoint: MysqlEndpoint<'_>,
    credentials: MysqlCredentials<'_>,
    request: MysqlOwnedPreparationRequest<'_>,
) -> Result<MysqlOwnedAuditChainReceipt> {
    let anchor = request.anchor(endpoint.database);
    let redactor = Redactor::new(Vec::new());
    let (configured_authentication, tls_version, head) = prepare_empty_owned_target(
        endpoint,
        credentials,
        request.server_uuid,
        request.schema_version,
        EmptyTargetOperation::VerifyAuditChain {
            request: &anchor,
            configured: request.configured(),
            redactor: &redactor,
        },
    )
    .await?;
    let head = head.context("MySQL owned audit chain receipt is missing")?;
    Ok(MysqlOwnedAuditChainReceipt {
        verification_mode: "owned_audit_chain",
        target_schema_version: request.schema_version,
        head_seq: head.seq,
        head_hash: head.hash,
        payload_bytes: head.payload_bytes,
        configured_authentication,
        tls_version,
    })
}

async fn prepare_configured_fresh_source(
    endpoint: MysqlEndpoint<'_>,
    credentials: MysqlCredentials<'_>,
    request: MysqlOwnedPreparationRequest<'_>,
    mode: FreshSourceMode,
) -> Result<MysqlOwnedPreparationReceipt> {
    let anchor = request.anchor(endpoint.database);
    prepare_owned_fresh_source(
        endpoint,
        credentials,
        &anchor,
        request.configured(),
        &Redactor::new(Vec::new()),
        mode,
    )
    .await
}

/// Resolve a fresh publication under the physical lock. Only publication
/// mode can install the selected format and create its genesis and anchor.
async fn prepare_owned_fresh_source(
    endpoint: MysqlEndpoint<'_>,
    credentials: MysqlCredentials<'_>,
    request: &FreshAnchorRequestV1,
    configured: ConfiguredTenantNamespace<'_>,
    redactor: &Redactor,
    mode: FreshSourceMode,
) -> Result<MysqlOwnedPreparationReceipt> {
    let schema_version = request.schema_version;
    let (configured_authentication, tls_version, head) = prepare_empty_owned_target(
        endpoint,
        credentials,
        request.server_uuid,
        schema_version,
        EmptyTargetOperation::FreshSource {
            request,
            configured,
            redactor,
            mode,
        },
    )
    .await?;
    let head = head.context("MySQL owned publication receipt is missing")?;
    Ok(MysqlOwnedPreparationReceipt {
        verification_mode: "owned_preparation_published",
        target_schema_version: schema_version,
        head_seq: head.seq,
        head_hash: head.hash,
        configured_authentication,
        tls_version,
    })
}

enum EmptyTargetOperation<'a> {
    Inspect,
    // The complete physical manifest and owned publication are prerequisites
    // for exposing installation to an operator.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "internal installation step pending owned publication"
        )
    )]
    InstallUnpublishedTables {
        request: &'a FreshAnchorRequestV1,
        configured: ConfiguredTenantNamespace<'a>,
        redactor: &'a Redactor,
    },
    FreshSource {
        request: &'a FreshAnchorRequestV1,
        configured: ConfiguredTenantNamespace<'a>,
        redactor: &'a Redactor,
        mode: FreshSourceMode,
    },
    VerifyAuditChain {
        request: &'a FreshAnchorRequestV1,
        configured: ConfiguredTenantNamespace<'a>,
        redactor: &'a Redactor,
    },
}

async fn prepare_empty_owned_target(
    endpoint: MysqlEndpoint<'_>,
    credentials: MysqlCredentials<'_>,
    server: Uuid,
    schema_version: u16,
    operation: EmptyTargetOperation<'_>,
) -> Result<(
    MysqlAuthentication,
    String,
    Option<crate::mysql_storage::audit::AuditHead>,
)> {
    ensure!(
        !server.is_nil() && matches!(schema_version, 14 | 15),
        "invalid MySQL owned preparation target"
    );
    ensure!(
        !["information_schema", "mysql", "performance_schema", "sys"]
            .iter()
            .any(|schema| endpoint.database.eq_ignore_ascii_case(schema)),
        MysqlOwnedPreparationRefusal::IncompatibleServer
    );
    let password = credentials.password;
    let (options, configured_authentication) = connect_options(&endpoint, credentials)?;
    tokio::time::timeout(PROBE_TIMEOUT, async {
        let (mut conn, tls_version) =
            open_verified(options.into(), endpoint.database, endpoint.username).await?;
        let ownership = match OwnedAuditLock::acquire(&mut conn, server, endpoint.database).await {
            Ok(ownership) => ownership,
            Err(error) => {
                conn.disconnect().await.map_err(driver_error)?;
                return Err(error);
            }
        };
        let checked = async {
            match operation {
                EmptyTargetOperation::Inspect => check_empty_schema(&mut conn, &ownership)
                    .await
                    .map(|_| None),
                EmptyTargetOperation::InstallUnpublishedTables {
                    request,
                    configured,
                    redactor,
                } => {
                    let redactor = redactor.with_secrets([password.to_owned()]);
                    let anchor = request.plan(
                        schema_version,
                        server,
                        endpoint.database,
                        configured,
                        &redactor,
                    )?;
                    match check_empty_schema(&mut conn, &ownership).await {
                        Ok(target) => audit_tables::install(target, schema_version).await?,
                        Err(error) => {
                            if error.downcast_ref::<MysqlOwnedPreparationRefusal>()
                                == Some(&MysqlOwnedPreparationRefusal::TargetNotEmpty)
                            {
                                anchor.reject_divergence(&mut conn, &ownership).await?;
                            }
                            return Err(error);
                        }
                    }
                    Ok(None)
                }
                EmptyTargetOperation::FreshSource {
                    request,
                    configured,
                    redactor,
                    mode,
                } => {
                    let redactor = redactor.with_secrets([password.to_owned()]);
                    let anchor = request.plan(
                        schema_version,
                        server,
                        endpoint.database,
                        configured,
                        &redactor,
                    )?;
                    if let FreshSourceMode::VerifyOnly = mode {
                        ensure!(
                            schema_has_user_objects(&mut conn, &ownership).await?,
                            MysqlOwnedPreparationRefusal::PublicationMissing
                        );
                        return publication::verify_published(&mut conn, &ownership, &anchor)
                            .await
                            .map(Some);
                    }
                    let target = match check_empty_schema(&mut conn, &ownership).await {
                        Ok(target) => target,
                        Err(error)
                            if error.downcast_ref::<MysqlOwnedPreparationRefusal>()
                                == Some(&MysqlOwnedPreparationRefusal::TargetNotEmpty) =>
                        {
                            let head =
                                publication::verify_published(&mut conn, &ownership, &anchor)
                                    .await?;
                            return Ok(Some(head));
                        }
                        Err(error) => return Err(error),
                    };
                    audit_tables::install(target, schema_version).await?;
                    let target = CheckedEmptyTarget {
                        conn: &mut conn,
                        lock: &ownership,
                    };
                    let head = publication::publish(
                        target,
                        schema_version,
                        &anchor,
                        configured.tenant_id,
                        configured.namespace_id,
                        &redactor,
                    )
                    .await?;
                    let verified =
                        publication::verify_published(&mut conn, &ownership, &anchor).await?;
                    ensure!(
                        verified == head,
                        "MySQL owned publication head changed after commit"
                    );
                    Ok(Some(verified))
                }
                EmptyTargetOperation::VerifyAuditChain {
                    request,
                    configured,
                    redactor,
                } => {
                    let redactor = redactor.with_secrets([password.to_owned()]);
                    let anchor = request.plan(
                        schema_version,
                        server,
                        endpoint.database,
                        configured,
                        &redactor,
                    )?;
                    ensure!(
                        schema_has_user_objects(&mut conn, &ownership).await?,
                        MysqlOwnedPreparationRefusal::PublicationMissing
                    );
                    publication::verify_anchored_chain(&mut conn, &ownership, &anchor)
                        .await
                        .map(Some)
                }
            }
        }
        .await;
        let released = ownership.release(&mut conn).await;
        conn.disconnect().await.map_err(driver_error)?;
        released?;
        let head = checked?;
        Ok((configured_authentication, tls_version, head))
    })
    .await
    .map_err(|_| MysqlProbeError::TimedOut)?
}

struct CheckedEmptyTarget<'a> {
    conn: &'a mut Conn,
    lock: &'a OwnedAuditLock,
}

async fn check_empty_schema<'a>(
    conn: &'a mut Conn,
    lock: &'a OwnedAuditLock,
) -> Result<CheckedEmptyTarget<'a>> {
    ensure!(
        !schema_has_user_objects(conn, lock).await?,
        MysqlOwnedPreparationRefusal::TargetNotEmpty
    );
    let row = conn.query_first::<Row, _>(
        "SELECT @@GLOBAL.innodb_flush_log_at_trx_commit, @@GLOBAL.read_only, @@GLOBAL.super_read_only, \
         EXISTS (SELECT 1 FROM information_schema.ENGINES WHERE ENGINE = 'InnoDB' AND SUPPORT IN ('YES', 'DEFAULT'))",
    ).await.map_err(driver_error)?.context("MySQL owned durability settings unavailable")?;
    ensure!(
        decode_row::<(u64, u8, u8, u8)>(row)? == (1, 0, 0, 1),
        MysqlOwnedPreparationRefusal::IncompatibleServer
    );
    lock.verify(conn).await?;
    Ok(CheckedEmptyTarget { conn, lock })
}

async fn schema_has_user_objects(conn: &mut Conn, lock: &OwnedAuditLock) -> Result<bool> {
    lock.verify(conn).await?;
    require_catalog_visibility(conn).await?;
    let row = conn.query_first::<Row, _>(
        "SELECT EXISTS (SELECT 1 FROM information_schema.TABLES WHERE BINARY TABLE_SCHEMA = BINARY DATABASE()) \
         OR EXISTS (SELECT 1 FROM information_schema.ROUTINES WHERE BINARY ROUTINE_SCHEMA = BINARY DATABASE()) \
         OR EXISTS (SELECT 1 FROM information_schema.EVENTS WHERE BINARY EVENT_SCHEMA = BINARY DATABASE()) \
         OR EXISTS (SELECT 1 FROM information_schema.TRIGGERS WHERE BINARY TRIGGER_SCHEMA = BINARY DATABASE())",
    ).await.map_err(driver_error)?.context("MySQL owned schema inventory unavailable")?;
    Ok(decode_row::<u8>(row)? != 0)
}

async fn require_catalog_visibility(conn: &mut impl Queryable) -> Result<()> {
    // INFORMATION_SCHEMA hides objects from accounts lacking privileges.
    // Only direct global grants prove visibility independently of MySQL's
    // choice of a schema grant, which can depend on another host entry.
    let row = conn
        .query_first::<Row, _>("SELECT CURRENT_ROLE(), @@GLOBAL.partial_revokes")
        .await
        .map_err(driver_error)?
        .context("MySQL owned catalog grants are unavailable")?;
    let (role, partial_revokes): (String, u8) = decode_row(row)?;
    ensure!(
        partial_revokes == 0 || (partial_revokes == 1 && role == "NONE"),
        MysqlOwnedPreparationRefusal::CatalogVisibilityUnconfirmed
    );
    let mut result = conn
        .query_iter("SHOW GRANTS FOR CURRENT_USER()")
        .await
        .map_err(driver_error)?;
    let mut grants = Vec::new();
    while let Some(row) = result.next().await.map_err(driver_error)? {
        let grant: String = decode_row(row)?;
        ensure!(
            grants.len() < 64 && grant.len() <= 8_192,
            MysqlOwnedPreparationRefusal::CatalogVisibilityUnconfirmed
        );
        grants.push(grant);
    }
    result.drop_result().await.map_err(driver_error)?;
    ensure!(
        global_grants_cover_catalog(grants.iter().map(String::as_str)),
        MysqlOwnedPreparationRefusal::CatalogVisibilityUnconfirmed
    );
    Ok(())
}

fn global_grants_cover_catalog<'a>(grants: impl IntoIterator<Item = &'a str>) -> bool {
    let mut has_global_visibility = false;
    for grant in grants {
        if grant.starts_with("REVOKE ") {
            return false;
        }
        let Some((privileges, _)) = grant
            .strip_prefix("GRANT ")
            .and_then(|grant| grant.split_once(" ON *.* TO "))
        else {
            continue;
        };
        has_global_visibility |= privileges_cover_catalog(privileges);
    }
    has_global_visibility
}

fn privileges_cover_catalog(privileges: &str) -> bool {
    // Only accept a plain privilege list. Quoted column names can contain
    // the separators used to read SHOW GRANTS rows.
    privileges == "ALL PRIVILEGES"
        || (privileges
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || matches!(byte, b' ' | b',' | b'_'))
            && REQUIRED_GRANTS.iter().all(|required| {
                privileges
                    .split(", ")
                    .any(|privilege| privilege == *required)
            }))
}

#[cfg(test)]
mod tests {
    use super::global_grants_cover_catalog;

    const GLOBAL: &str = "GRANT SELECT, SHOW VIEW, ALTER ROUTINE, EVENT, TRIGGER, CREATE ON *.* TO `prepare`@`localhost`";

    #[test]
    fn owned_catalog_requires_direct_global_visibility_without_a_partial_revoke() {
        let scoped = "GRANT ALL PRIVILEGES ON `qaowned`.* TO `prepare`@`localhost`";
        let literal = "GRANT SELECT, CREATE, SHOW VIEW, ALTER ROUTINE, EVENT, TRIGGER ON `qa\\_owned`.* TO `prepare`@`localhost`";
        let revoke = "REVOKE EVENT ON `qa_owned`.* FROM `prepare`@`localhost`";
        assert!(global_grants_cover_catalog([GLOBAL]));
        assert!(!global_grants_cover_catalog([scoped]));
        assert!(!global_grants_cover_catalog([literal]));
        assert!(!global_grants_cover_catalog([GLOBAL, revoke]));
        assert!(!global_grants_cover_catalog([
            "GRANT SELECT ON *.* TO `prepare`@`localhost`",
            literal,
        ]));
        assert!(!global_grants_cover_catalog([
            "GRANT `prepare_role`@`localhost` TO `prepare`@`localhost`",
        ]));
    }

    #[test]
    fn owned_catalog_refuses_overlapping_schema_grants() {
        let escaped = "GRANT SELECT, SHOW VIEW ON `qa\\_owned`.* TO `prepare`@`localhost`";
        let wildcard =
            "GRANT ALTER ROUTINE, EVENT, TRIGGER, CREATE ON `qa_owned`.* TO `prepare`@`localhost`";
        let exact = "GRANT ALL PRIVILEGES ON `qa\\_owned`.* TO `prepare`@`localhost`";

        assert!(!global_grants_cover_catalog([escaped, wildcard]));
        assert!(!global_grants_cover_catalog([wildcard, exact]));
        assert!(global_grants_cover_catalog([wildcard, exact, GLOBAL]));
    }
}
