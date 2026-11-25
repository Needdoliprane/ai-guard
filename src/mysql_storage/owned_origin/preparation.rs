//! Read-only operator preflight for a fresh owned MySQL schema.

use super::{OwnedAuditLock, decode_row, driver_error};
use crate::mysql_storage::{
    MysqlAuthentication, MysqlCredentials, MysqlEndpoint, MysqlProbeError, PROBE_TIMEOUT,
    connect_options, open_verified,
};
use anyhow::{Context, Result, ensure};
use mysql_async::{Conn, Row, prelude::Queryable};
use serde::Serialize;
use std::fmt;
use uuid::Uuid;

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
        let checked = check_empty_schema(&mut conn, &ownership, endpoint.database).await;
        let released = ownership.release(&mut conn).await;
        conn.disconnect().await.map_err(driver_error)?;
        released?;
        checked?;
        Ok(MysqlOwnedPreparationPreflight {
            verification_mode: "owned_preparation_preflight",
            target_schema_version: schema_version,
            configured_authentication,
            tls_version,
        })
    })
    .await
    .map_err(|_| MysqlProbeError::TimedOut)?
}

async fn check_empty_schema(
    conn: &mut Conn,
    ownership: &OwnedAuditLock,
    database: &str,
) -> Result<()> {
    ownership.verify(conn).await?;
    require_catalog_visibility(conn, database).await?;
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
    ownership.verify(conn).await
}

async fn require_catalog_visibility(conn: &mut Conn, database: &str) -> Result<()> {
    // INFORMATION_SCHEMA hides objects from accounts lacking privileges.
    // Require direct grants spanning the selected schema or the server. A
    // role-only grant or partial revoke yields no proof in this preflight.
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
        grants_cover_catalog(
            grants.iter().map(String::as_str),
            database,
            partial_revokes == 1,
            role != "NONE",
        ),
        MysqlOwnedPreparationRefusal::CatalogVisibilityUnconfirmed
    );
    Ok(())
}

fn grants_cover_catalog<'a>(
    grants: impl IntoIterator<Item = &'a str>,
    database: &str,
    partial_revokes: bool,
    roles_active: bool,
) -> bool {
    let name = database.replace('`', "``");
    let literal_name = if partial_revokes {
        name
    } else {
        name.replace('%', "\\%").replace('_', "\\_")
    };
    let literal_scope = format!("`{literal_name}`.* TO ");
    let mut global_proves_visibility = false;
    let mut schema_grants = 0;
    let mut schema_proves_visibility = false;
    for grant in grants {
        if grant.starts_with("REVOKE ") {
            return false;
        }
        let Some((privileges, target)) = grant
            .strip_prefix("GRANT ")
            .and_then(|grant| grant.split_once(" ON "))
        else {
            continue;
        };
        if target.starts_with("*.* TO ") {
            global_proves_visibility |= privileges_cover_catalog(privileges);
        } else if target.contains(".* TO ") {
            // MySQL can use only the first matching schema grant. Count any
            // other schema grant as a possible competitor without matching patterns.
            schema_grants += 1;
            schema_proves_visibility |=
                target.starts_with(&literal_scope) && privileges_cover_catalog(privileges);
        }
    }
    global_proves_visibility
        || (!roles_active
            && !database.contains('\\')
            && schema_grants == 1
            && schema_proves_visibility)
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
    use super::grants_cover_catalog;

    const GLOBAL: &str = "GRANT SELECT, SHOW VIEW, ALTER ROUTINE, EVENT, TRIGGER, CREATE ON *.* TO `prepare`@`localhost`";

    #[test]
    fn owned_catalog_requires_schema_wide_visibility_without_a_partial_revoke() {
        let schema = "qa_owned";
        let scoped = "GRANT ALL PRIVILEGES ON `qa_owned`.* TO `prepare`@`localhost`";
        let literal = "GRANT SELECT, CREATE, SHOW VIEW, ALTER ROUTINE, EVENT, TRIGGER ON `qa\\_owned`.* TO `prepare`@`localhost`";
        let revoke = "REVOKE EVENT ON `qa_owned`.* FROM `prepare`@`localhost`";
        assert!(!grants_cover_catalog([scoped], schema, false, false));
        assert!(grants_cover_catalog([scoped], schema, true, false));
        assert!(grants_cover_catalog([GLOBAL], schema, true, false));
        assert!(grants_cover_catalog([literal], schema, false, false));
        assert!(!grants_cover_catalog([literal], schema, false, true));
        assert!(!grants_cover_catalog([literal], schema, true, false));
        assert!(!grants_cover_catalog([GLOBAL, revoke], schema, true, false));
        assert!(!grants_cover_catalog([scoped], "qa_other", false, false));
        assert!(!grants_cover_catalog(
            ["GRANT SELECT ON `qa_owned`.* TO `prepare`@`localhost`"],
            schema,
            false,
            false
        ));
        assert!(!grants_cover_catalog(
            ["GRANT `prepare_role`@`localhost` TO `prepare`@`localhost`"],
            schema,
            false,
            false
        ));
    }

    #[test]
    fn owned_catalog_refuses_overlapping_schema_grants() {
        let escaped = "GRANT SELECT, SHOW VIEW ON `qa\\_owned`.* TO `prepare`@`localhost`";
        let wildcard =
            "GRANT ALTER ROUTINE, EVENT, TRIGGER, CREATE ON `qa_owned`.* TO `prepare`@`localhost`";
        let exact = "GRANT ALL PRIVILEGES ON `qa\\_owned`.* TO `prepare`@`localhost`";

        assert!(!grants_cover_catalog(
            [escaped, wildcard],
            "qa_owned",
            false,
            false
        ));
        assert!(!grants_cover_catalog(
            [wildcard, exact],
            "qa_owned",
            false,
            false
        ));
        assert!(grants_cover_catalog(
            [wildcard, exact, GLOBAL],
            "qa_owned",
            false,
            false
        ));
    }
}
