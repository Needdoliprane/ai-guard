//! The two audit tables on a freshly verified empty MySQL owned target.

use super::{CheckedEmptyTarget, driver_error};
use anyhow::{Context, Result, ensure};
use mysql_async::{Conn, prelude::Queryable};

const LEGACY_VERSION_CHECK: &str =
    "CONSTRAINT ai_guard_audit_state_schema_version CHECK (schema_version = 1)";

fn audit_tables_ddl(version: u16) -> Result<(&'static str, String)> {
    ensure!(
        matches!(version, 14 | 15),
        "invalid MySQL owned audit format"
    );
    let ddl = include_str!("../../../../schema/mysql/audit-v1.sql");
    let (_, statements) = ddl
        .split_once('\n')
        .context("audit DDL header is missing")?;
    let mut statements = statements
        .split(';')
        .map(str::trim)
        .filter(|sql| !sql.is_empty());
    let event = statements
        .next()
        .filter(|sql| sql.starts_with("CREATE TABLE ai_guard_audit_event ("))
        .context("audit event DDL is missing")?;
    let state = statements
        .next()
        .filter(|sql| sql.starts_with("CREATE TABLE ai_guard_audit_state ("))
        .context("audit state DDL is missing")?;
    ensure!(
        statements
            .next()
            .is_some_and(|sql| sql.starts_with("INSERT INTO ai_guard_audit_state ("))
            && statements.next().is_none(),
        "unexpected statements in shared audit DDL"
    );
    let (prefix, suffix) = state
        .split_once(LEGACY_VERSION_CHECK)
        .context("shared audit version guard is missing")?;
    let state = format!(
        "{prefix}CONSTRAINT ai_guard_audit_state_schema_version CHECK (schema_version = {version}){suffix}"
    );
    Ok((event, state))
}

/// Called after the full empty-target check on this locked connection.
/// The singleton and all audit rows remain absent until owned publication.
pub(super) async fn install(target: CheckedEmptyTarget<'_>, version: u16) -> Result<()> {
    let CheckedEmptyTarget { conn, lock } = target;
    let (event, state) = audit_tables_ddl(version)?;
    // Check that the server can report enforced constraints before issuing DDL.
    ensure!(
        version_guard(conn).await?.is_none(),
        "audit version guard already exists"
    );
    lock.verify(conn).await?;
    conn.query_drop(event).await.map_err(driver_error)?;
    lock.verify(conn).await?;
    conn.query_drop(state).await.map_err(driver_error)?;

    let tables = conn
        .query_first::<(u64, u64), _>(
            "SELECT COUNT(*), COUNT(CASE WHEN TABLE_TYPE = 'BASE TABLE' AND ENGINE = 'InnoDB' \
             AND BINARY TABLE_NAME IN ('ai_guard_audit_event', 'ai_guard_audit_state') THEN 1 END) \
             FROM information_schema.TABLES WHERE BINARY TABLE_SCHEMA = BINARY DATABASE()",
        )
        .await
        .map_err(driver_error)?;
    ensure!(
        tables == Some((2, 2)),
        "unexpected MySQL owned audit tables"
    );
    let guard = version_guard(conn)
        .await?
        .context("owned audit version guard is missing")?;
    // MySQL renders identifiers quoted and may enclose the expression in parentheses.
    let guard: String = guard
        .chars()
        .filter(|&ch| !ch.is_ascii_whitespace() && !matches!(ch, '`' | '(' | ')'))
        .collect();
    ensure!(
        guard == format!("schema_version={version}"),
        "MySQL owned audit version guard mismatch"
    );
    let empty = conn
        .query_first::<u8, _>(
            "SELECT NOT EXISTS (SELECT 1 FROM ai_guard_audit_state) \
             AND NOT EXISTS (SELECT 1 FROM ai_guard_audit_event)",
        )
        .await
        .map_err(driver_error)?;
    ensure!(
        empty == Some(1),
        "MySQL owned audit tables contain unexpected rows"
    );
    lock.verify(conn).await
}

async fn version_guard(conn: &mut Conn) -> Result<Option<String>> {
    let guard = conn
        .query_first::<String, _>(
            "SELECT c.CHECK_CLAUSE FROM information_schema.TABLE_CONSTRAINTS AS t \
             JOIN information_schema.CHECK_CONSTRAINTS AS c \
             ON BINARY c.CONSTRAINT_SCHEMA = BINARY t.CONSTRAINT_SCHEMA \
             AND BINARY c.CONSTRAINT_NAME = BINARY t.CONSTRAINT_NAME \
             WHERE BINARY t.TABLE_SCHEMA = BINARY DATABASE() \
             AND BINARY t.TABLE_NAME = 'ai_guard_audit_state' \
             AND BINARY t.CONSTRAINT_NAME = 'ai_guard_audit_state_schema_version' \
             AND t.CONSTRAINT_TYPE = 'CHECK' AND t.ENFORCED = 'YES'",
        )
        .await
        .map_err(driver_error)?;
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::audit_tables_ddl;
    use anyhow::Result;

    #[test]
    fn owned_audit_ddl_creates_only_tables_with_the_selected_version_guard() -> Result<()> {
        for version in [14, 15] {
            let (event, state) = audit_tables_ddl(version)?;
            assert!(event.starts_with("CREATE TABLE ai_guard_audit_event ("));
            assert!(state.starts_with("CREATE TABLE ai_guard_audit_state ("));
            assert!(state.contains(&format!("CHECK (schema_version = {version})")));
            assert!(!state.contains("CHECK (schema_version = 1)"));
            assert_eq!(state.matches("CHECK (schema_version = ").count(), 1);
            assert!(!event.contains(';') && !state.contains(';'));
        }
        assert!(audit_tables_ddl(1).is_err());
        Ok(())
    }
}
