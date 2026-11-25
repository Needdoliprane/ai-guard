//! Operator preflight and Legacy refusal against disposable loopback MySQL schemas.
use super::{identity::FixtureFiles, *};
use agentsandbox::mysql_storage::{
    MysqlOwnedPreparationPreflight, MysqlOwnedPreparationRefusal, preflight_owned_preparation,
};
use anyhow::{Result, ensure};

#[tokio::test]
#[ignore = "requires a fresh empty dedicated MySQL 8.4 QA database and a preparation account"]
async fn owned_preflight_refuses_interrupted_and_legacy_schemas() -> Result<()> {
    let fixture = FixtureFiles::open_gateway("AI_GUARD_MYSQL_OWNED_PREPARATION_QA_CONFIG")?;
    let mut conn = fixture.maintenance().await?;
    let server = tokio::time::timeout(std::time::Duration::from_secs(8), async {
        let value = conn
            .query_first::<String, _>("SELECT @@GLOBAL.server_uuid")
            .await?
            .context("QA server UUID is absent")?;
        Ok::<_, anyhow::Error>(Uuid::parse_str(&value)?)
    })
    .await??;
    let probe = |schema_version| {
        let endpoint = fixture
            .target
            .endpoint(&fixture.target.maintenance_username);
        let credentials = fixture.credentials(&fixture.maintenance_password);
        async move { preflight_owned_preparation(endpoint, credentials?, server, schema_version).await }
    };
    for schema_version in [14, 15] {
        let receipt = probe(schema_version).await?;
        ensure!(receipt.target_schema_version == schema_version);
        ensure!(
            table_count(&mut conn).await? == 0,
            "preflight created objects"
        );
    }

    // Simulate interruption after one atomic DDL statement and then the
    // beginning of a Legacy install. Both snapshots must remain untouched.
    let mut ddl = include_str!("../../schema/mysql/audit-v1.sql")
        .split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty());
    let first = ddl.next().context("fixture audit DDL is empty")?;
    run_ddl(&mut conn, first).await?;
    require_not_empty(probe(14).await)?;
    ensure!(
        table_count(&mut conn).await? == 1,
        "interrupted schema changed"
    );
    for statement in ddl {
        run_ddl(&mut conn, statement).await?;
    }
    require_not_empty(probe(15).await)?;
    ensure!(table_count(&mut conn).await? == 2, "Legacy schema changed");
    let version = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        conn.query_first::<u16, _>(
            "SELECT schema_version FROM ai_guard_audit_state WHERE singleton=1",
        ),
    )
    .await??;
    ensure!(version == Some(1), "Legacy version was not preserved");
    Ok(())
}

#[tokio::test]
#[ignore = "requires two fresh empty dedicated MySQL 8.4 QA databases"]
async fn legacy_store_refuses_uncommitted_and_owned_version_state() -> Result<()> {
    for (format, config) in [
        (14_u16, "AI_GUARD_MYSQL_OWNED_PREPARATION_14_QA_CONFIG"),
        (15, "AI_GUARD_MYSQL_OWNED_PREPARATION_15_QA_CONFIG"),
    ] {
        let fixture = FixtureFiles::open_gateway(config)?;
        let mut conn = fixture.maintenance().await?;
        ensure!(
            table_count(&mut conn).await? == 0,
            "QA database is not empty"
        );
        let namespace = fixture.target.namespace;
        let store = fixture.target.open(
            &fixture.target.maintenance_username,
            fixture.credentials(&fixture.maintenance_password)?,
            TENANT,
            namespace,
            128,
        )?;

        // Keep the historical columns so the test reaches the version guard.
        // This fixture models the publication boundary, without the Legacy seed.
        let ddl = include_str!("../../schema/mysql/audit-v1.sql").replace(
            "CHECK (schema_version = 1)",
            "CHECK (schema_version IN (14, 15))",
        );
        let mut ddl = ddl.split(';');
        let missing_table = MysqlProbeError::ServerRefused(1146).to_string();
        require_legacy_refusal(&store, &missing_table).await?;
        run_ddl(&mut conn, ddl.next().context("audit event DDL is absent")?).await?;
        require_legacy_refusal(&store, &missing_table).await?;
        run_ddl(&mut conn, ddl.next().context("audit state DDL is absent")?).await?;

        let missing_state = "MySQL audit schema was not prepared";
        require_legacy_refusal(&store, missing_state).await?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
        let mut tx =
            tokio::time::timeout_at(deadline, conn.start_transaction(TxOpts::new())).await??;
        tokio::time::timeout_at(
            deadline,
            tx.exec_drop(
                "INSERT INTO ai_guard_audit_state (singleton, schema_version, tenant_id, namespace_id, head_seq, head_hash, payload_bytes) VALUES (1, ?, ?, ?, 0, ?, 0)",
                (format, TENANT, namespace.to_string(), "0".repeat(64)),
            ),
        )
        .await??;
        // verify uses a separate nonlocking snapshot. The initialize paths
        // take a row lock, so exercise them only outside this open transaction.
        let error = tokio::time::timeout_at(deadline, store.verify())
            .await?
            .err()
            .context("Legacy accepted an uncommitted state row")?;
        ensure!(
            error.to_string() == missing_state,
            "state visible before commit"
        );
        tokio::time::timeout_at(deadline, tx.commit()).await??;

        require_legacy_refusal(&store, "unsupported MySQL audit schema version").await?;
        let state = tokio::time::timeout(std::time::Duration::from_secs(8), conn.query_first::<(u16, i64, u64), _>(
            "SELECT schema_version, head_seq, (SELECT COUNT(*) FROM ai_guard_audit_event) FROM ai_guard_audit_state WHERE singleton=1",
        )).await??;
        ensure!(
            state == Some((format, 0, 0)),
            "Legacy changed the owned fixture"
        );
    }
    Ok(())
}

async fn require_legacy_refusal(store: &MysqlAuditStore, expected: &str) -> Result<()> {
    for error in [
        store.verify().await.err(),
        store.initialize().await.err(),
        store.initialize_supervised().await.err(),
    ] {
        ensure!(
            error
                .context("Legacy accepted the preparation phase")?
                .to_string()
                == expected,
            "Legacy did not reject the preparation phase with {expected}"
        );
    }
    Ok(())
}

fn require_not_empty(result: Result<MysqlOwnedPreparationPreflight>) -> Result<()> {
    let error = result.err().context("nonempty target was accepted")?;
    ensure!(
        error.downcast_ref::<MysqlOwnedPreparationRefusal>()
            == Some(&MysqlOwnedPreparationRefusal::TargetNotEmpty),
        "expected a target inventory refusal"
    );
    Ok(())
}

async fn table_count(conn: &mut Conn) -> Result<u64> {
    tokio::time::timeout(
        std::time::Duration::from_secs(8),
        conn.query_first::<u64, _>(
            "SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE()",
        ),
    )
    .await??
    .context("QA table count is absent")
}

async fn run_ddl(conn: &mut Conn, sql: &str) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(8), conn.query_drop(sql)).await??;
    Ok(())
}
