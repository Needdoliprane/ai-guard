//! The two audit tables on a freshly verified empty MySQL owned target.

use super::{CheckedEmptyTarget, audit_manifest, driver_error};
use anyhow::{Context, Result, ensure};
use mysql_async::prelude::Queryable;

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
    audit_manifest::require_empty_constraints(conn).await?;
    lock.verify(conn).await?;
    conn.query_drop(event).await.map_err(driver_error)?;
    lock.verify(conn).await?;
    conn.query_drop(state).await.map_err(driver_error)?;

    audit_manifest::verify_unpublished(conn, lock, version).await
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
