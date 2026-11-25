//! Unpublished tables on a verified empty MySQL owned target.

use super::{CheckedEmptyTarget, audit_manifest, driver_error, egress_manifest, worker_manifest};
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

fn review_tables_ddl() -> Result<[&'static str; 2]> {
    let ddl = include_str!("../../../../schema/mysql/review-owned-v1.sql");
    let (_, statements) = ddl
        .split_once('\n')
        .context("review DDL header is missing")?;
    let mut statements = statements
        .split(';')
        .map(str::trim)
        .filter(|sql| !sql.is_empty());
    let authority = statements
        .next()
        .filter(|sql| sql.starts_with("CREATE TABLE review_authority_meta ("))
        .context("review authority DDL is missing")?;
    let bindings = statements
        .next()
        .filter(|sql| sql.starts_with("CREATE TABLE runner_review_bindings ("))
        .context("review bindings DDL is missing")?;
    ensure!(
        statements.next().is_none(),
        "unexpected statements in review DDL"
    );
    Ok([authority, bindings])
}

fn decision_table_ddl() -> Result<&'static str> {
    let ddl = include_str!("../../../../schema/mysql/decisions-v1.sql");
    let (_, body) = ddl
        .split_once('\n')
        .context("decision DDL header is missing")?;
    body.trim()
        .strip_suffix(';')
        .filter(|statement| {
            statement.starts_with("CREATE TABLE ai_guard_audit_decision (")
                && !statement.contains(';')
        })
        .context("unexpected statements in shared decision DDL")
}

/// Called after the full empty-target check on this locked connection.
/// The singleton, audit events and all projections stay empty.
pub(super) async fn install(target: CheckedEmptyTarget<'_>, version: u16) -> Result<()> {
    let CheckedEmptyTarget { conn, lock } = target;
    let (event, state) = audit_tables_ddl(version)?;
    let decision = decision_table_ddl()?;
    let [authority, bindings] = review_tables_ddl()?;
    let workers = worker_manifest::ddl().context("unexpected worker reservation DDL")?;
    let egress =
        egress_manifest::ddl(version).context("unexpected egress DDL for the owned format")?;
    audit_manifest::require_empty_constraints(conn).await?;
    for statement in [event, state.as_str(), decision, authority, bindings]
        .into_iter()
        .chain(workers)
        .chain(egress)
    {
        lock.verify(conn).await?;
        conn.query_drop(statement).await.map_err(driver_error)?;
    }

    audit_manifest::verify_unpublished(conn, lock, version).await
}

#[cfg(test)]
mod tests {
    use super::{audit_tables_ddl, decision_table_ddl, review_tables_ddl};
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
        let decision = decision_table_ddl()?;
        assert!(decision.contains("PRIMARY KEY (request_id, human_approved)"));
        assert!(decision.contains("FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event (seq)"));
        assert!(!decision.contains(';'));
        Ok(())
    }

    #[test]
    fn owned_review_ddl_preserves_active_uniqueness_and_audit_references() -> Result<()> {
        let [authority, bindings] = review_tables_ddl()?;
        assert!(
            authority.contains("FOREIGN KEY (anchor_seq) REFERENCES ai_guard_audit_event (seq)")
        );
        assert!(
            authority
                .contains("FOREIGN KEY (incarnation_seq) REFERENCES ai_guard_audit_event (seq)")
        );
        assert!(
            bindings.contains("FOREIGN KEY (last_event_seq) REFERENCES ai_guard_audit_event (seq)")
        );
        assert!(bindings.contains("AS (IF(active = 1, tenant_id, NULL)) STORED"));
        assert!(bindings.contains(
            "UNIQUE KEY runner_review_bindings_active_credential (credential_id, active_tenant_id)"
        ));
        assert!(
            bindings.contains("UNIQUE KEY runner_review_bindings_active_tenant (active_tenant_id)")
        );
        Ok(())
    }
}
