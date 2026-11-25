//! Read-only operator preflight for a fresh owned MySQL schema.

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
    CatalogVisibilityUnconfirmed,
    IncompatibleServer,
}

impl fmt::Display for MysqlOwnedPreparationRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TargetNotEmpty => {
                "MySQL owned preparation requires an empty schema; preserve this target"
            }
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
    let (configured_authentication, tls_version) = prepare_empty_owned_target(
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
}

async fn prepare_empty_owned_target(
    endpoint: MysqlEndpoint<'_>,
    credentials: MysqlCredentials<'_>,
    server: Uuid,
    schema_version: u16,
    operation: EmptyTargetOperation<'_>,
) -> Result<(MysqlAuthentication, String)> {
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
                EmptyTargetOperation::Inspect => {
                    check_empty_schema(&mut conn, &ownership).await.map(|_| ())
                }
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
                        Ok(target) => audit_tables::install(target, schema_version).await,
                        Err(error) => {
                            if error.downcast_ref::<MysqlOwnedPreparationRefusal>()
                                == Some(&MysqlOwnedPreparationRefusal::TargetNotEmpty)
                            {
                                anchor.reject_divergence(&mut conn, &ownership).await?;
                            }
                            Err(error)
                        }
                    }
                }
            }
        }
        .await;
        let released = ownership.release(&mut conn).await;
        conn.disconnect().await.map_err(driver_error)?;
        released?;
        checked?;
        Ok((configured_authentication, tls_version))
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
    lock.verify(conn).await?;
    require_catalog_visibility(conn).await?;
    let row = conn.query_first::<Row, _>(
        "SELECT EXISTS (SELECT 1 FROM information_schema.TABLES WHERE BINARY TABLE_SCHEMA = BINARY DATABASE()) \
         OR EXISTS (SELECT 1 FROM information_schema.ROUTINES WHERE BINARY ROUTINE_SCHEMA = BINARY DATABASE()) \
         OR EXISTS (SELECT 1 FROM information_schema.EVENTS WHERE BINARY EVENT_SCHEMA = BINARY DATABASE()) \
         OR EXISTS (SELECT 1 FROM information_schema.TRIGGERS WHERE BINARY TRIGGER_SCHEMA = BINARY DATABASE())",
    ).await.map_err(driver_error)?.context("MySQL owned schema inventory unavailable")?;
    ensure!(
        decode_row::<u8>(row)? == 0,
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
