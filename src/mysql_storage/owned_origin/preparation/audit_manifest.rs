//! Physical manifest of the unpublished audit, decision, review and format-15 egress tables.

use super::{
    OwnedAuditLock, decode_row, driver_error, egress_manifest as egress, require_catalog_visibility,
};
use anyhow::{Result, ensure};
use mysql_async::{
    Conn, IsolationLevel, Row, Transaction, TxOpts,
    prelude::{FromRow, Queryable},
};

const DECISION: &str = "ai_guard_audit_decision";
const EVENT: &str = "ai_guard_audit_event";
const STATE: &str = "ai_guard_audit_state";
const REVIEW_AUTHORITY: &str = "review_authority_meta";
const REVIEW_BINDINGS: &str = "runner_review_bindings";

type Column = (String, String, String, String, u8, String);
type Index = (String, String, Option<String>, u8, u8, u8);
type Constraint = (String, String, String, String, Option<String>);
type ForeignKey = (String, String, String, String, String, u8, u8);

pub(super) type ExpectedColumn = (&'static str, &'static str, &'static str, &'static str, u8);
pub(super) type ExpectedIndex = (&'static str, &'static str, Option<&'static str>, u8, u8, u8);
pub(super) type ExpectedForeignKey = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    u8,
    u8,
);

const FOREIGN_KEYS: [(&str, &str, &str, &str, &str, u8); 4] = [
    (
        DECISION,
        "ai_guard_audit_decision_event_ref",
        "event_seq",
        EVENT,
        "seq",
        1,
    ),
    (
        REVIEW_AUTHORITY,
        "review_authority_anchor_event",
        "anchor_seq",
        EVENT,
        "seq",
        1,
    ),
    (
        REVIEW_AUTHORITY,
        "review_authority_incarnation_event",
        "incarnation_seq",
        EVENT,
        "seq",
        1,
    ),
    (
        REVIEW_BINDINGS,
        "review_binding_audit_event",
        "last_event_seq",
        EVENT,
        "seq",
        1,
    ),
];

// Ordered by table and ordinal position. The final flag requires no non-null
// default or character collation, and either an ordinary or stored column.
const COLUMNS: [ExpectedColumn; 36] = [
    (DECISION, "request_id", "varbinary(64)", "NO", 1),
    (DECISION, "human_approved", "tinyint unsigned", "NO", 1),
    (DECISION, "request_hash", "binary(64)", "NO", 1),
    (DECISION, "event_seq", "bigint", "NO", 1),
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
    (REVIEW_AUTHORITY, "singleton", "tinyint unsigned", "NO", 1),
    (REVIEW_AUTHORITY, "source_id", "varbinary(36)", "NO", 1),
    (REVIEW_AUTHORITY, "anchor_seq", "bigint", "NO", 1),
    (REVIEW_AUTHORITY, "anchor_hash", "binary(64)", "NO", 1),
    (REVIEW_AUTHORITY, "incarnation_seq", "bigint", "YES", 1),
    (REVIEW_AUTHORITY, "incarnation_hash", "binary(64)", "YES", 1),
    (REVIEW_AUTHORITY, "incarnation_revision", "bigint", "NO", 1),
    (REVIEW_AUTHORITY, "snapshot_json", "mediumblob", "NO", 1),
    (REVIEW_AUTHORITY, "snapshot_hash", "binary(64)", "NO", 1),
    (REVIEW_BINDINGS, "binding_id", "varbinary(36)", "NO", 1),
    (REVIEW_BINDINGS, "credential_id", "varbinary(36)", "NO", 1),
    (REVIEW_BINDINGS, "tenant_id", "varbinary(128)", "NO", 1),
    (REVIEW_BINDINGS, "active", "tinyint unsigned", "NO", 1),
    (REVIEW_BINDINGS, "binding_revision", "bigint", "NO", 1),
    (REVIEW_BINDINGS, "policy_revision", "bigint", "NO", 1),
    (REVIEW_BINDINGS, "snapshot_json", "mediumblob", "NO", 1),
    (REVIEW_BINDINGS, "snapshot_hash", "binary(64)", "NO", 1),
    (REVIEW_BINDINGS, "last_event_seq", "bigint", "NO", 1),
    (
        REVIEW_BINDINGS,
        "active_tenant_id",
        "varbinary(128)",
        "YES",
        1,
    ),
];

pub(super) type ExpectedConstraint<'a> = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<&'a str>,
);

fn expected_constraints(version_check: &str) -> [ExpectedConstraint<'_>; 34] {
    // CHECK_CLAUSE contains the MySQL data dictionary's rendering, including
    // grouping, the length alias, and hex literals. Compare the whole clause.
    [
        (DECISION, "PRIMARY", "PRIMARY KEY", "YES", None),
        (
            DECISION,
            "ai_guard_audit_decision_event",
            "UNIQUE",
            "YES",
            None,
        ),
        (
            DECISION,
            "ai_guard_audit_decision_event_ref",
            "FOREIGN KEY",
            "YES",
            None,
        ),
        (
            DECISION,
            "ai_guard_audit_decision_phase",
            "CHECK",
            "YES",
            Some("(`human_approved` in (0,1))"),
        ),
        (
            DECISION,
            "ai_guard_audit_decision_request_size",
            "CHECK",
            "YES",
            Some("(length(`request_id`) between 1 and 64)"),
        ),
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
        (REVIEW_AUTHORITY, "PRIMARY", "PRIMARY KEY", "YES", None),
        (
            REVIEW_AUTHORITY,
            "review_authority_anchor_event",
            "FOREIGN KEY",
            "YES",
            None,
        ),
        (
            REVIEW_AUTHORITY,
            "review_authority_anchor_seq",
            "UNIQUE",
            "YES",
            None,
        ),
        (
            REVIEW_AUTHORITY,
            "review_authority_incarnation_event",
            "FOREIGN KEY",
            "YES",
            None,
        ),
        (
            REVIEW_AUTHORITY,
            "review_authority_incarnation_revision_nonnegative",
            "CHECK",
            "YES",
            Some("(`incarnation_revision` >= 0)"),
        ),
        (
            REVIEW_AUTHORITY,
            "review_authority_incarnation_seq",
            "UNIQUE",
            "YES",
            None,
        ),
        (
            REVIEW_AUTHORITY,
            "review_authority_singleton",
            "CHECK",
            "YES",
            Some("(`singleton` = 1)"),
        ),
        (
            REVIEW_AUTHORITY,
            "review_authority_source_id",
            "UNIQUE",
            "YES",
            None,
        ),
        (REVIEW_BINDINGS, "PRIMARY", "PRIMARY KEY", "YES", None),
        (
            REVIEW_BINDINGS,
            "review_binding_active_flag",
            "CHECK",
            "YES",
            Some("(`active` in (0,1))"),
        ),
        (
            REVIEW_BINDINGS,
            "review_binding_audit_event",
            "FOREIGN KEY",
            "YES",
            None,
        ),
        (
            REVIEW_BINDINGS,
            "review_binding_binding_revision_positive",
            "CHECK",
            "YES",
            Some("(`binding_revision` >= 1)"),
        ),
        (
            REVIEW_BINDINGS,
            "review_binding_policy_revision_positive",
            "CHECK",
            "YES",
            Some("(`policy_revision` >= 1)"),
        ),
        (
            REVIEW_BINDINGS,
            "runner_review_bindings_active_credential",
            "UNIQUE",
            "YES",
            None,
        ),
        (
            REVIEW_BINDINGS,
            "runner_review_bindings_active_tenant",
            "UNIQUE",
            "YES",
            None,
        ),
        (
            REVIEW_BINDINGS,
            "runner_review_bindings_last_event_seq",
            "UNIQUE",
            "YES",
            None,
        ),
    ]
}

fn expected_in_format<T: Copy>(
    common: &[T],
    extension: &[T],
    version: u16,
    table: impl Fn(&T) -> &'static str,
) -> Vec<T> {
    let mut expected = common.to_vec();
    if version == 15 {
        expected.extend_from_slice(extension);
        // Keep ordinal positions and constraint names ordered within each table.
        expected.sort_by_key(table);
    }
    expected
}

fn inventory_matches(actual: &[(u64, u64, u64, u8)], version: u16) -> bool {
    match version {
        14 => actual == [(5, 5, 0, 1)],
        15 => actual == [(10, 5, 5, 1)],
        _ => false,
    }
}

fn columns_match(columns: &[Column], version: u16) -> bool {
    columns
        .iter()
        .map(|(table, column, ty, nullable, shape, _)| {
            (
                table.as_str(),
                column.as_str(),
                ty.as_str(),
                nullable.as_str(),
                *shape,
            )
        })
        .eq(expected_in_format(
            &COLUMNS,
            &egress::COLUMNS,
            version,
            |row| row.0,
        ))
        && columns.iter().all(|(table, column, _, _, _, expression)| {
            generation_matches(table, column, expression)
        })
}

fn generation_matches(table: &str, column: &str, expression: &str) -> bool {
    if table == REVIEW_BINDINGS && column == "active_tenant_id" {
        expressions_match(
            Some(expression),
            Some("if((`active` = 1),`tenant_id`,NULL)"),
        )
    } else {
        expression.is_empty()
    }
}

const INDEXES: [(&str, &str, Option<&str>, u8, u8); 15] = [
    (DECISION, "PRIMARY", Some("request_id"), 1, 1),
    (DECISION, "PRIMARY", Some("human_approved"), 2, 1),
    (
        DECISION,
        "ai_guard_audit_decision_event",
        Some("event_seq"),
        1,
        1,
    ),
    (EVENT, "PRIMARY", Some("seq"), 1, 1),
    (EVENT, "event_id", Some("event_id"), 1, 1),
    (STATE, "PRIMARY", Some("singleton"), 1, 1),
    (REVIEW_AUTHORITY, "PRIMARY", Some("singleton"), 1, 1),
    (
        REVIEW_AUTHORITY,
        "review_authority_anchor_seq",
        Some("anchor_seq"),
        1,
        1,
    ),
    (
        REVIEW_AUTHORITY,
        "review_authority_incarnation_seq",
        Some("incarnation_seq"),
        1,
        1,
    ),
    (
        REVIEW_AUTHORITY,
        "review_authority_source_id",
        Some("source_id"),
        1,
        1,
    ),
    (REVIEW_BINDINGS, "PRIMARY", Some("binding_id"), 1, 1),
    (
        REVIEW_BINDINGS,
        "runner_review_bindings_active_credential",
        Some("credential_id"),
        1,
        1,
    ),
    (
        REVIEW_BINDINGS,
        "runner_review_bindings_active_credential",
        Some("active_tenant_id"),
        2,
        1,
    ),
    (
        REVIEW_BINDINGS,
        "runner_review_bindings_active_tenant",
        Some("active_tenant_id"),
        1,
        1,
    ),
    (
        REVIEW_BINDINGS,
        "runner_review_bindings_last_event_seq",
        Some("last_event_seq"),
        1,
        1,
    ),
];

fn indexes_match(indexes: &[Index], version: u16) -> bool {
    let common = INDEXES
        .map(|(table, name, column, position, shape)| (table, name, column, position, 0, shape));
    indexes
        .iter()
        .map(|(table, name, column, position, non_unique, shape)| {
            (
                table.as_str(),
                name.as_str(),
                column.as_deref(),
                *position,
                *non_unique,
                *shape,
            )
        })
        .eq(expected_in_format(
            &common,
            &egress::INDEXES,
            version,
            |row| row.0,
        ))
}

fn constraints_match(constraints: &[Constraint], version: u16) -> bool {
    let version_check = format!("(`schema_version` = {version})");
    let expected = expected_in_format(
        &expected_constraints(&version_check),
        &egress::CONSTRAINTS,
        version,
        |row| row.0,
    );
    constraints.len() == expected.len()
        && constraints.iter().zip(expected).all(|(actual, expected)| {
            let (table, name, kind, enforced, clause) = actual;
            let (expected_table, expected_name, expected_kind, expected_enforced, rule) = expected;
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
            ) && expressions_match(clause.as_deref(), rule)
        })
}

fn expressions_match(actual: Option<&str>, expected: Option<&str>) -> bool {
    // The server can omit identifier quotes. Only transform the expected
    // expression, preserving the exact actual grouping.
    actual == expected
        || expected.is_some_and(|rule| actual == Some(rule.replace('`', "").as_str()))
}

fn foreign_keys_match(keys: &[ForeignKey], version: u16) -> bool {
    let common = FOREIGN_KEYS.map(|(table, key, column, parent, reference, exact)| {
        (table, key, column, parent, reference, 1, exact)
    });
    keys.iter()
        .map(|(table, key, column, parent, reference, position, exact)| {
            (
                table.as_str(),
                key.as_str(),
                column.as_str(),
                parent.as_str(),
                reference.as_str(),
                *position,
                *exact,
            )
        })
        .eq(expected_in_format(
            &common,
            &egress::FOREIGN_KEYS,
            version,
            |row| row.0,
        ))
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
         ORDER BY BINARY t.TABLE_NAME, BINARY t.CONSTRAINT_NAME LIMIT 61",
        connection,
    ).await
}

pub(super) async fn require_empty_constraints(conn: &mut Conn) -> Result<()> {
    // Require access to enforced CHECK metadata before the first DDL.
    ensure!(
        read_constraints(conn).await?.is_empty(),
        "unexpected MySQL audit constraints before DDL"
    );
    require_constraints_enabled(conn).await
}

async fn require_constraints_enabled(connection: &mut impl Queryable) -> Result<()> {
    let checks: Vec<(u8, u8)> = metadata_rows(
        "SELECT @@SESSION.foreign_key_checks, @@SESSION.unique_checks",
        connection,
    )
    .await?;
    ensure!(
        checks == [(1, 1)],
        "MySQL owned preparation constraints must be enabled"
    );
    Ok(())
}

/// Attest the freshly installed manifest and its empty state.
pub(super) async fn verify_unpublished(
    conn: &mut Conn,
    lock: &OwnedAuditLock,
    version: u16,
) -> Result<()> {
    let mut options = TxOpts::new();
    options
        .with_readonly(true)
        .with_isolation_level(IsolationLevel::RepeatableRead);
    let mut tx = conn
        .start_transaction(options)
        .await
        .map_err(driver_error)?;
    let verification = verify_empty_in_transaction(&mut tx, lock, version).await;
    tx.rollback().await.map_err(driver_error)?;
    verification
}

/// Check the empty manifest on the caller's locked transaction before any seed rows.
pub(super) async fn verify_empty_in_transaction(
    tx: &mut Transaction<'_>,
    lock: &OwnedAuditLock,
    version: u16,
) -> Result<()> {
    ensure!(
        matches!(version, 14 | 15),
        "invalid MySQL owned audit format"
    );
    lock.verify_preparation_target(tx).await?;
    require_catalog_visibility(tx).await?;
    require_constraints_enabled(tx).await?;
    let inventory = metadata_rows(
        "SELECT COUNT(*), COUNT(CASE WHEN TABLE_TYPE = 'BASE TABLE' AND ENGINE = 'InnoDB' \
         AND BINARY TABLE_NAME IN ('ai_guard_audit_decision', 'ai_guard_audit_event', \
             'ai_guard_audit_state', 'review_authority_meta', 'runner_review_bindings') THEN 1 END), \
         COUNT(CASE WHEN TABLE_TYPE = 'BASE TABLE' AND ENGINE = 'InnoDB' \
         AND BINARY TABLE_NAME IN ('egress_policy_meta', 'egress_policy_revisions', \
             'egress_policy_heads', 'egress_policy_operations', 'egress_policy_reservations') THEN 1 END), \
         NOT EXISTS (SELECT 1 FROM information_schema.ROUTINES WHERE BINARY ROUTINE_SCHEMA = BINARY DATABASE()) \
         AND NOT EXISTS (SELECT 1 FROM information_schema.EVENTS WHERE BINARY EVENT_SCHEMA = BINARY DATABASE()) \
         AND NOT EXISTS (SELECT 1 FROM information_schema.TRIGGERS WHERE BINARY TRIGGER_SCHEMA = BINARY DATABASE()) \
         AND NOT EXISTS (SELECT 1 FROM information_schema.PARTITIONS \
                         WHERE BINARY TABLE_SCHEMA = BINARY DATABASE() AND PARTITION_NAME IS NOT NULL) \
         FROM information_schema.TABLES WHERE BINARY TABLE_SCHEMA = BINARY DATABASE()",
        tx,
    ).await?;
    ensure!(
        inventory_matches(&inventory, version),
        "unexpected MySQL owned preparation objects"
    );

    // Read the tables before their column and index metadata to retain their
    // metadata locks for the rest of this snapshot.
    let empty: Vec<u8> = metadata_rows(
        "SELECT NOT EXISTS (SELECT 1 FROM ai_guard_audit_state) \
         AND NOT EXISTS (SELECT 1 FROM ai_guard_audit_event) \
         AND NOT EXISTS (SELECT 1 FROM ai_guard_audit_decision) \
         AND NOT EXISTS (SELECT 1 FROM review_authority_meta) \
         AND NOT EXISTS (SELECT 1 FROM runner_review_bindings)",
        tx,
    )
    .await?;
    ensure!(
        empty == [1],
        "MySQL unpublished audit, decision or review tables contain rows"
    );
    if version == 15 {
        let empty: Vec<u8> = metadata_rows(
            "SELECT NOT EXISTS (SELECT 1 FROM egress_policy_meta) \
             AND NOT EXISTS (SELECT 1 FROM egress_policy_revisions) \
             AND NOT EXISTS (SELECT 1 FROM egress_policy_heads) \
             AND NOT EXISTS (SELECT 1 FROM egress_policy_operations) \
             AND NOT EXISTS (SELECT 1 FROM egress_policy_reservations)",
            tx,
        )
        .await?;
        ensure!(empty == [1], "MySQL unpublished egress tables contain rows");
    }

    let columns = metadata_rows(
        "SELECT TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, \
         COLUMN_DEFAULT IS NULL AND CHARACTER_SET_NAME IS NULL AND COLLATION_NAME IS NULL \
         AND ((BINARY EXTRA = '' AND GENERATION_EXPRESSION = '') \
              OR (BINARY EXTRA = 'STORED GENERATED' AND GENERATION_EXPRESSION <> '')), \
         IF(OCTET_LENGTH(GENERATION_EXPRESSION) <= 512, GENERATION_EXPRESSION, NULL) \
         FROM information_schema.COLUMNS WHERE BINARY TABLE_SCHEMA = BINARY DATABASE() \
         ORDER BY BINARY TABLE_NAME, ORDINAL_POSITION LIMIT 63",
        tx,
    )
    .await?;
    ensure!(
        columns_match(&columns, version),
        "MySQL owned preparation column manifest mismatch"
    );

    let indexes = metadata_rows(
        "SELECT s.TABLE_NAME, s.INDEX_NAME, s.COLUMN_NAME, s.SEQ_IN_INDEX, s.NON_UNIQUE, \
         s.SUB_PART IS NULL AND s.COLLATION = 'A' \
         AND s.INDEX_TYPE = 'BTREE' AND s.IS_VISIBLE = 'YES' AND s.EXPRESSION IS NULL \
         FROM information_schema.STATISTICS AS s WHERE BINARY s.TABLE_SCHEMA = BINARY DATABASE() \
         ORDER BY BINARY s.TABLE_NAME, BINARY s.INDEX_NAME, s.SEQ_IN_INDEX LIMIT 32",
        tx,
    )
    .await?;
    ensure!(
        indexes_match(&indexes, version),
        "MySQL owned preparation index manifest mismatch"
    );
    let constraints = read_constraints(tx).await?;
    ensure!(
        constraints_match(&constraints, version),
        "MySQL owned preparation constraint manifest mismatch"
    );
    let keys = read_foreign_keys(tx).await?;
    ensure!(
        foreign_keys_match(&keys, version),
        "MySQL owned preparation foreign key mismatch"
    );
    lock.verify_preparation_target(tx).await
}

async fn read_foreign_keys(connection: &mut impl Queryable) -> Result<Vec<ForeignKey>> {
    // The checked InnoDB inventory enforces either spelling immediately.
    metadata_rows(
        "SELECT k.TABLE_NAME, k.CONSTRAINT_NAME, k.COLUMN_NAME, \
                k.REFERENCED_TABLE_NAME, k.REFERENCED_COLUMN_NAME, k.ORDINAL_POSITION, \
                k.POSITION_IN_UNIQUE_CONSTRAINT = k.ORDINAL_POSITION \
                AND BINARY k.REFERENCED_TABLE_SCHEMA = BINARY DATABASE() \
                AND r.UNIQUE_CONSTRAINT_NAME = 'PRIMARY' AND r.MATCH_OPTION = 'NONE' \
                AND r.UPDATE_RULE IN ('NO ACTION', 'RESTRICT') \
                AND r.DELETE_RULE IN ('NO ACTION', 'RESTRICT') \
         FROM information_schema.KEY_COLUMN_USAGE AS k \
         JOIN information_schema.REFERENTIAL_CONSTRAINTS AS r \
         ON BINARY r.CONSTRAINT_SCHEMA = BINARY k.CONSTRAINT_SCHEMA \
         AND BINARY r.TABLE_NAME = BINARY k.TABLE_NAME \
         AND BINARY r.CONSTRAINT_NAME = BINARY k.CONSTRAINT_NAME \
         WHERE BINARY k.TABLE_SCHEMA = BINARY DATABASE() AND k.REFERENCED_TABLE_NAME IS NOT NULL \
         ORDER BY BINARY k.TABLE_NAME, BINARY k.CONSTRAINT_NAME, k.ORDINAL_POSITION LIMIT 14",
        connection,
    )
    .await
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
                    if column == "active_tenant_id" {
                        "if((`active` = 1),`tenant_id`,NULL)".into()
                    } else {
                        String::new()
                    },
                )
            })
            .into();
        assert!(columns_match(&columns, 14));
        let mut extended = columns.clone();
        extended.extend(egress::COLUMNS.map(|(t, c, ty, nullable, shape)| {
            (
                t.into(),
                c.into(),
                ty.into(),
                nullable.into(),
                shape,
                String::new(),
            )
        }));
        extended.sort_by(|left, right| left.0.cmp(&right.0));
        assert!(columns_match(&extended, 15));
        assert!(!columns_match(&extended, 14));
        let received_at = columns
            .iter()
            .position(|row| row.0 == EVENT && row.1 == "received_at")
            .unwrap();
        columns[received_at].2 = "varbinary(31)".into();
        assert!(!columns_match(&columns, 14));
        columns[received_at].2 = "varbinary(32)".into();
        assert!(columns_match(&columns, 14));
        columns.push((
            STATE.into(),
            "unexpected".into(),
            "bigint".into(),
            "NO".into(),
            1,
            String::new(),
        ));
        assert!(!columns_match(&columns, 14));

        let mut indexes: [Index; 15] = INDEXES.map(|(table, name, column, position, shape)| {
            (
                table.into(),
                name.into(),
                column.map(str::to_owned),
                position,
                0,
                shape,
            )
        });
        assert!(indexes_match(&indexes, 14));
        let mut extended = indexes.to_vec();
        extended.extend(egress::INDEXES.map(|(t, n, c, pos, non_unique, shape)| {
            (
                t.into(),
                n.into(),
                c.map(str::to_owned),
                pos,
                non_unique,
                shape,
            )
        }));
        extended.sort_by(|left, right| left.0.cmp(&right.0));
        assert!(indexes_match(&extended, 15));
        assert!(!indexes_match(&extended, 14));
        let key = extended
            .iter_mut()
            .find(|row| row.1 == "egress_heads_revision_key")
            .unwrap();
        key.4 = 0;
        assert!(!indexes_match(&extended, 15));
        let decision_phase = indexes
            .iter()
            .position(|row| row.0 == DECISION && row.2.as_deref() == Some("human_approved"))
            .unwrap();
        indexes[decision_phase].3 = 1;
        assert!(!indexes_match(&indexes, 14));
        indexes[decision_phase].3 = 2;
        let decision_event = indexes
            .iter()
            .position(|row| row.1 == "ai_guard_audit_decision_event")
            .unwrap();
        indexes[decision_event].4 = 1;
        assert!(!indexes_match(&indexes, 14));
        indexes[decision_event].4 = 0;
        indexes[decision_event].5 = 0;
        assert!(!indexes_match(&indexes, 14));
        indexes[decision_event].5 = 1;
        assert!(indexes_match(&indexes, 14));
        assert!(!indexes_match(&indexes[..indexes.len() - 1], 14));
    }

    #[test]
    fn owned_review_manifest_rejects_changed_binding_and_audit_links() {
        assert!(generation_matches(
            REVIEW_BINDINGS,
            "active_tenant_id",
            "if((`active` = 1),`tenant_id`,NULL)",
        ));
        assert!(!generation_matches(
            REVIEW_BINDINGS,
            "active_tenant_id",
            "if((`active` = 1),`credential_id`,NULL)",
        ));
        assert!(!generation_matches(REVIEW_BINDINGS, "active_tenant_id", ""));

        let mut links: Vec<ForeignKey> = FOREIGN_KEYS
            .map(|(table, key, column, parent, reference, exact)| {
                (
                    table.into(),
                    key.into(),
                    column.into(),
                    parent.into(),
                    reference.into(),
                    1,
                    exact,
                )
            })
            .into();
        assert!(foreign_keys_match(&links, 14));
        let review_event = links
            .iter()
            .position(|row| row.1 == "review_binding_audit_event")
            .unwrap();
        links[review_event].3 = "other_audit_event".into();
        assert!(!foreign_keys_match(&links, 14));
        links[review_event].3 = EVENT.into();
        let decision_event = links.iter().position(|row| row.0 == DECISION).unwrap();
        links[decision_event].6 = 0;
        assert!(!foreign_keys_match(&links, 14));
    }

    #[test]
    fn owned_egress_manifest_rejects_wrong_format_and_composite_edges() {
        assert!(inventory_matches(&[(5, 5, 0, 1)], 14));
        assert!(inventory_matches(&[(10, 5, 5, 1)], 15));
        // Both formats require the common decision receipt projection.
        assert!(!inventory_matches(&[(4, 4, 0, 1)], 14));
        assert!(!inventory_matches(&[(9, 4, 5, 1)], 15));
        assert!(!inventory_matches(&[(6, 5, 1, 1)], 14));
        assert!(!inventory_matches(&[(9, 5, 4, 1)], 15));
        assert!(!inventory_matches(&[(10, 5, 5, 1)], 14));

        let common = FOREIGN_KEYS.map(|(t, n, c, p, r, exact)| (t, n, c, p, r, 1, exact));
        let mut links: Vec<ForeignKey> =
            expected_in_format(&common, &egress::FOREIGN_KEYS, 15, |row| row.0)
                .into_iter()
                .map(|(t, n, c, p, r, pos, exact)| {
                    (t.into(), n.into(), c.into(), p.into(), r.into(), pos, exact)
                })
                .collect();
        assert!(foreign_keys_match(&links, 15));
        assert!(!foreign_keys_match(&links, 14));
        let scope_id = links
            .iter()
            .position(|row| row.1 == "egress_heads_revision_ref" && row.2 == "scope_id")
            .unwrap();
        links[scope_id].5 = 1;
        assert!(!foreign_keys_match(&links, 15));
        links[scope_id].5 = 2;
        links[scope_id].4 = "scope_kind".into();
        assert!(!foreign_keys_match(&links, 15));
    }

    #[test]
    fn owned_audit_manifest_requires_the_exact_enforced_checks() {
        for version in [14, 15] {
            let clause = format!("(`schema_version` = {version})");
            let mut constraints: Vec<Constraint> = expected_in_format(
                &expected_constraints(&clause),
                &egress::CONSTRAINTS,
                version,
                |row| row.0,
            )
            .into_iter()
            .map(|(table, name, kind, enforced, rule)| {
                (
                    table.into(),
                    name.into(),
                    kind.into(),
                    enforced.into(),
                    rule.map(str::to_owned),
                )
            })
            .collect();
            assert!(constraints_match(&constraints, version));
            assert!(!constraints_match(
                &constraints,
                if version == 14 { 15 } else { 14 }
            ));
            let version_constraint = constraints
                .iter()
                .position(|row| row.1 == "ai_guard_audit_state_schema_version")
                .unwrap();
            constraints[version_constraint].4 = Some(format!("(schema_version = {version})"));
            assert!(constraints_match(&constraints, version));
            if version == 15 {
                let reservation = constraints
                    .iter()
                    .position(|row| row.1 == "egress_reservations_bytes")
                    .unwrap();
                let original = constraints[reservation]
                    .4
                    .replace("(`bytes` = 98303)".into());
                assert!(!constraints_match(&constraints, version));
                constraints[reservation].4 = original;
            }
            let decision_phase = constraints
                .iter()
                .position(|row| row.1 == "ai_guard_audit_decision_phase")
                .unwrap();
            constraints[decision_phase].3 = "NO".into();
            assert!(!constraints_match(&constraints, version));
            constraints[decision_phase].3 = "YES".into();
            let binding = constraints
                .iter()
                .position(|row| row.1 == "ai_guard_audit_state_binding")
                .unwrap();
            constraints[binding].4 =
                Some("((`tenant_id` is null) or (`namespace_id` is null))".into());
            assert!(!constraints_match(&constraints, version));
        }
    }
}
