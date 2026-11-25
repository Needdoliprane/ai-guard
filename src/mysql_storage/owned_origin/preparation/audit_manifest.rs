//! Physical manifest of the unpublished audit tables from schema/mysql/audit-v1.sql.

use super::{OwnedAuditLock, decode_row, driver_error};
use anyhow::{Result, ensure};
use mysql_async::{
    Conn, IsolationLevel, Row, TxOpts,
    prelude::{FromRow, Queryable},
};

const EVENT: &str = "ai_guard_audit_event";
const STATE: &str = "ai_guard_audit_state";

type Column = (String, String, String, String, u8);
type Index = (String, String, Option<String>, u8);
type Constraint = (String, String, String, String, Option<String>);

// Ordered by table and ordinal position. The final flag requires an ordinary
// column with no non-null default, generation expression, or character collation.
const COLUMNS: [(&str, &str, &str, &str, u8); 13] = [
    (EVENT, "seq", "bigint", "NO", 1),
    (EVENT, "event_id", "varbinary(64)", "NO", 1),
    (EVENT, "received_at", "varbinary(32)", "NO", 1),
    (EVENT, "prev_hash", "binary(64)", "NO", 1),
    (EVENT, "hash", "binary(64)", "NO", 1),
    (EVENT, "payload", "mediumblob", "NO", 1),
    (STATE, "singleton", "tinyint unsigned", "NO", 1),
    (STATE, "schema_version", "smallint unsigned", "NO", 1),
    (STATE, "tenant_id", "varbinary(128)", "YES", 1),
    (STATE, "namespace_id", "varbinary(36)", "YES", 1),
    (STATE, "head_seq", "bigint", "NO", 1),
    (STATE, "head_hash", "binary(64)", "NO", 1),
    (STATE, "payload_bytes", "bigint unsigned", "NO", 1),
];

type ExpectedConstraint<'a> = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<&'a str>,
);

fn expected_constraints(version_check: &str) -> [ExpectedConstraint<'_>; 13] {
    // CHECK_CLAUSE contains the MySQL data dictionary's rendering, including
    // grouping, the length alias, and hex literals. Compare the whole clause.
    [
        (EVENT, "PRIMARY", "PRIMARY KEY", "YES", None),
        (
            EVENT,
            "ai_guard_audit_event_hash_size",
            "CHECK",
            "YES",
            Some("((locate(0x00,`prev_hash`) = 0) and (locate(0x00,`hash`) = 0))"),
        ),
        (
            EVENT,
            "ai_guard_audit_event_id_size",
            "CHECK",
            "YES",
            Some("(length(`event_id`) between 1 and 64)"),
        ),
        (
            EVENT,
            "ai_guard_audit_event_payload_size",
            "CHECK",
            "YES",
            Some("(length(`payload`) between 1 and 131072)"),
        ),
        (
            EVENT,
            "ai_guard_audit_event_received_size",
            "CHECK",
            "YES",
            Some("(length(`received_at`) between 1 and 32)"),
        ),
        (
            EVENT,
            "ai_guard_audit_event_seq",
            "CHECK",
            "YES",
            Some("(`seq` > 0)"),
        ),
        (EVENT, "event_id", "UNIQUE", "YES", None),
        (STATE, "PRIMARY", "PRIMARY KEY", "YES", None),
        (
            STATE,
            "ai_guard_audit_state_binding",
            "CHECK",
            "YES",
            Some(concat!(
                "(((`tenant_id` is null) and (`namespace_id` is null)) or ",
                "((`tenant_id` is not null) and (`namespace_id` is not null) and ",
                "(length(`tenant_id`) between 1 and 128) and (length(`namespace_id`) = 36)))",
            )),
        ),
        (
            STATE,
            "ai_guard_audit_state_head_hash_size",
            "CHECK",
            "YES",
            Some("(locate(0x00,`head_hash`) = 0)"),
        ),
        (
            STATE,
            "ai_guard_audit_state_head_seq",
            "CHECK",
            "YES",
            Some("(`head_seq` >= 0)"),
        ),
        (
            STATE,
            "ai_guard_audit_state_schema_version",
            "CHECK",
            "YES",
            Some(version_check),
        ),
        (
            STATE,
            "ai_guard_audit_state_singleton",
            "CHECK",
            "YES",
            Some("(`singleton` = 1)"),
        ),
    ]
}

fn columns_match(columns: &[Column]) -> bool {
    columns
        .iter()
        .map(|(table, column, ty, nullable, plain)| {
            (
                table.as_str(),
                column.as_str(),
                ty.as_str(),
                nullable.as_str(),
                *plain,
            )
        })
        .eq(COLUMNS)
}

fn indexes_match(indexes: &[Index]) -> bool {
    indexes
        .iter()
        .map(|(table, name, column, plain)| {
            (table.as_str(), name.as_str(), column.as_deref(), *plain)
        })
        .eq([
            (EVENT, "PRIMARY", Some("seq"), 1),
            (EVENT, "event_id", Some("event_id"), 1),
            (STATE, "PRIMARY", Some("singleton"), 1),
        ])
}

fn constraints_match(constraints: &[Constraint], version: u16) -> bool {
    let version_check = format!("(`schema_version` = {version})");
    let expected = expected_constraints(&version_check);
    constraints.len() == expected.len()
        && constraints.iter().zip(expected).all(|(actual, expected)| {
            let (table, name, kind, enforced, clause) = actual;
            let (expected_table, expected_name, expected_kind, expected_enforced, rule) = expected;
            let clause = clause.as_deref();
            (
                table.as_str(),
                name.as_str(),
                kind.as_str(),
                enforced.as_str(),
            ) == (
                expected_table,
                expected_name,
                expected_kind,
                expected_enforced,
            ) && (clause == rule
                || rule.is_some_and(|rule| {
                    // The server can omit identifier quotes. Only transform the
                    // expected expression, preserving the exact actual grouping.
                    clause == Some(rule.replace('`', "").as_str())
                }))
        })
}

async fn metadata_rows<T: FromRow>(sql: &str, connection: &mut impl Queryable) -> Result<Vec<T>> {
    let rows: Vec<Row> = connection.query(sql).await.map_err(driver_error)?;
    rows.into_iter().map(decode_row).collect()
}

async fn read_constraints(connection: &mut impl Queryable) -> Result<Vec<Constraint>> {
    metadata_rows(
        "SELECT t.TABLE_NAME, t.CONSTRAINT_NAME, t.CONSTRAINT_TYPE, t.ENFORCED, \
         IF(OCTET_LENGTH(c.CHECK_CLAUSE) <= 512, c.CHECK_CLAUSE, NULL) \
         FROM information_schema.TABLE_CONSTRAINTS AS t \
         LEFT JOIN information_schema.CHECK_CONSTRAINTS AS c \
         ON t.CONSTRAINT_TYPE = 'CHECK' AND BINARY c.CONSTRAINT_SCHEMA = BINARY t.CONSTRAINT_SCHEMA \
         AND BINARY c.CONSTRAINT_NAME = BINARY t.CONSTRAINT_NAME \
         WHERE BINARY t.TABLE_SCHEMA = BINARY DATABASE() \
         ORDER BY BINARY t.TABLE_NAME, BINARY t.CONSTRAINT_NAME LIMIT 14",
        connection,
    ).await
}

pub(super) async fn require_empty_constraints(conn: &mut Conn) -> Result<()> {
    // Require access to enforced CHECK metadata before the first DDL.
    ensure!(
        read_constraints(conn).await?.is_empty(),
        "unexpected MySQL audit constraints before DDL"
    );
    Ok(())
}

/// Attest the freshly installed audit tables, including the absent singleton.
pub(super) async fn verify_unpublished(
    conn: &mut Conn,
    lock: &OwnedAuditLock,
    version: u16,
) -> Result<()> {
    ensure!(
        matches!(version, 14 | 15),
        "invalid MySQL owned audit format"
    );
    let mut options = TxOpts::new();
    options
        .with_readonly(true)
        .with_isolation_level(IsolationLevel::RepeatableRead);
    let mut tx = conn
        .start_transaction(options)
        .await
        .map_err(driver_error)?;
    let verification = async {
        lock.verify(&mut tx).await?;
        let inventory: Vec<(u64, u64, u8)> = metadata_rows(
            "SELECT COUNT(*), COUNT(CASE WHEN TABLE_TYPE = 'BASE TABLE' AND ENGINE = 'InnoDB' \
             AND BINARY TABLE_NAME IN ('ai_guard_audit_event', 'ai_guard_audit_state') THEN 1 END), \
             NOT EXISTS (SELECT 1 FROM information_schema.ROUTINES WHERE BINARY ROUTINE_SCHEMA = BINARY DATABASE()) \
             AND NOT EXISTS (SELECT 1 FROM information_schema.EVENTS WHERE BINARY EVENT_SCHEMA = BINARY DATABASE()) \
             AND NOT EXISTS (SELECT 1 FROM information_schema.TRIGGERS WHERE BINARY TRIGGER_SCHEMA = BINARY DATABASE()) \
             AND NOT EXISTS (SELECT 1 FROM information_schema.PARTITIONS \
                             WHERE BINARY TABLE_SCHEMA = BINARY DATABASE() AND PARTITION_NAME IS NOT NULL) \
             FROM information_schema.TABLES WHERE BINARY TABLE_SCHEMA = BINARY DATABASE()",
            &mut tx,
        ).await?;
        ensure!(inventory == [(2, 2, 1)], "unexpected MySQL owned audit objects");

        // Read both tables before their column and index metadata to retain their
        // metadata locks for the rest of this snapshot.
        let empty: Vec<u8> = metadata_rows(
            "SELECT NOT EXISTS (SELECT 1 FROM ai_guard_audit_state) \
             AND NOT EXISTS (SELECT 1 FROM ai_guard_audit_event)",
            &mut tx,
        ).await?;
        ensure!(empty == [1], "MySQL owned audit tables contain unexpected rows");

        let columns = metadata_rows(
            "SELECT TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, \
             COLUMN_DEFAULT IS NULL AND EXTRA = '' AND GENERATION_EXPRESSION = '' \
             AND CHARACTER_SET_NAME IS NULL AND COLLATION_NAME IS NULL \
             FROM information_schema.COLUMNS WHERE BINARY TABLE_SCHEMA = BINARY DATABASE() \
             ORDER BY BINARY TABLE_NAME, ORDINAL_POSITION LIMIT 14",
            &mut tx,
        ).await?;
        ensure!(columns_match(&columns), "MySQL owned audit column manifest mismatch");

        let indexes = metadata_rows(
            "SELECT s.TABLE_NAME, s.INDEX_NAME, s.COLUMN_NAME, \
             s.NON_UNIQUE = 0 AND s.SEQ_IN_INDEX = 1 AND s.SUB_PART IS NULL AND s.COLLATION = 'A' \
             AND s.INDEX_TYPE = 'BTREE' AND s.IS_VISIBLE = 'YES' AND s.EXPRESSION IS NULL \
             FROM information_schema.STATISTICS AS s WHERE BINARY s.TABLE_SCHEMA = BINARY DATABASE() \
             ORDER BY BINARY s.TABLE_NAME, BINARY s.INDEX_NAME, s.SEQ_IN_INDEX LIMIT 4",
            &mut tx,
        ).await?;
        ensure!(indexes_match(&indexes), "MySQL owned audit index manifest mismatch");
        let constraints = read_constraints(&mut tx).await?;
        ensure!(constraints_match(&constraints, version), "MySQL owned audit constraint manifest mismatch");
        lock.verify(&mut tx).await
    }.await;
    tx.rollback().await.map_err(driver_error)?;
    verification
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_audit_manifest_rejects_column_and_index_drift() {
        let mut columns: Vec<Column> = COLUMNS
            .map(|(table, column, sql_type, nullable, plain)| {
                (
                    table.into(),
                    column.into(),
                    sql_type.into(),
                    nullable.into(),
                    plain,
                )
            })
            .into();
        assert!(columns_match(&columns));
        columns[2].2 = "varbinary(31)".into();
        assert!(!columns_match(&columns));
        columns[2].2 = "varbinary(32)".into();
        columns.push((
            STATE.into(),
            "unexpected".into(),
            "bigint".into(),
            "NO".into(),
            1,
        ));
        assert!(!columns_match(&columns));

        let mut indexes = [
            (EVENT.into(), "PRIMARY".into(), Some("seq".into()), 1),
            (EVENT.into(), "event_id".into(), Some("event_id".into()), 1),
            (STATE.into(), "PRIMARY".into(), Some("singleton".into()), 1),
        ];
        assert!(indexes_match(&indexes));
        indexes[1].3 = 0;
        assert!(!indexes_match(&indexes));
        assert!(!indexes_match(&indexes[..2]));
    }

    #[test]
    fn owned_audit_manifest_requires_the_exact_enforced_checks() {
        for version in [14, 15] {
            let clause = format!("(`schema_version` = {version})");
            let mut constraints: Vec<Constraint> = expected_constraints(&clause)
                .map(|(table, name, kind, enforced, rule)| {
                    (
                        table.into(),
                        name.into(),
                        kind.into(),
                        enforced.into(),
                        rule.map(str::to_owned),
                    )
                })
                .into();
            assert!(constraints_match(&constraints, version));
            assert!(!constraints_match(
                &constraints,
                if version == 14 { 15 } else { 14 }
            ));
            constraints[11].4 = Some(format!("(schema_version = {version})"));
            assert!(constraints_match(&constraints, version));
            constraints[5].3 = "NO".into();
            assert!(!constraints_match(&constraints, version));
            constraints[5].3 = "YES".into();
            constraints[8].4 = Some("((`tenant_id` is null) or (`namespace_id` is null))".into());
            assert!(!constraints_match(&constraints, version));
        }
    }
}
