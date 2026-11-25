//! Physical manifest for the unpublished MySQL owned tables.

use super::{
    OwnedAuditLock, decode_row, driver_error, egress_manifest as egress,
    require_catalog_visibility, supervision_manifest as supervision, worker_manifest as workers,
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

const fn expected_constraints(version_check: &str) -> [ExpectedConstraint<'_>; 34] {
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
    common: &[&[T]],
    extension: &[T],
    version: u16,
    table: impl Fn(&T) -> &'static str,
) -> Vec<T> {
    let mut expected = common.concat();
    if version == 15 {
        expected.extend_from_slice(extension);
    }
    // Keep ordinal positions and constraint names ordered within each table.
    expected.sort_by_key(table);
    expected
}

fn inventory_matches(actual: &[(u64, u64, u64, u8)], version: u16) -> bool {
    match version {
        14 => actual == [(12, 12, 0, 1)],
        15 => actual == [(17, 12, 5, 1)],
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
            &[&COLUMNS, &workers::COLUMNS, &supervision::COLUMNS],
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
            &[&common, &workers::INDEXES, &supervision::INDEXES],
            &egress::INDEXES,
            version,
            |row| row.0,
        ))
}

fn constraints_match(constraints: &[Constraint], version: u16) -> bool {
    let version_check = format!("(`schema_version` = {version})");
    let expected = expected_in_format(
        &[
            &expected_constraints(&version_check),
            &workers::CONSTRAINTS,
            &supervision::CONSTRAINTS,
        ],
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
            &[&common, &workers::FOREIGN_KEYS, &supervision::FOREIGN_KEYS],
            &egress::FOREIGN_KEYS,
            version,
            |row| row.0,
        ))
}

async fn metadata_rows<T: FromRow>(sql: &str, connection: &mut impl Queryable) -> Result<Vec<T>> {
    let rows: Vec<Row> = connection.query(sql).await.map_err(driver_error)?;
    rows.into_iter().map(decode_row).collect()
}

async fn manifest_rows<T: FromRow>(
    sql: &str,
    max_expected: usize,
    connection: &mut impl Queryable,
) -> Result<Vec<T>> {
    // The extra row exposes overflow. A LIMIT at the expected size could
    // hide extra objects that sort after an otherwise exact manifest.
    metadata_rows(&format!("{sql} LIMIT {}", max_expected + 1), connection).await
}

async fn read_constraints(connection: &mut impl Queryable) -> Result<Vec<Constraint>> {
    let max_expected = expected_constraints("").len()
        + workers::CONSTRAINTS.len()
        + supervision::CONSTRAINTS.len()
        + egress::CONSTRAINTS.len();
    manifest_rows(
        "SELECT t.TABLE_NAME, t.CONSTRAINT_NAME, t.CONSTRAINT_TYPE, t.ENFORCED, \
         IF(OCTET_LENGTH(c.CHECK_CLAUSE) <= 512, c.CHECK_CLAUSE, NULL) \
         FROM information_schema.TABLE_CONSTRAINTS AS t \
         LEFT JOIN information_schema.CHECK_CONSTRAINTS AS c \
         ON t.CONSTRAINT_TYPE = 'CHECK' AND BINARY c.CONSTRAINT_SCHEMA = BINARY t.CONSTRAINT_SCHEMA \
         AND BINARY c.CONSTRAINT_NAME = BINARY t.CONSTRAINT_NAME \
         WHERE BINARY t.TABLE_SCHEMA = BINARY DATABASE() \
         ORDER BY BINARY t.TABLE_NAME, BINARY t.CONSTRAINT_NAME",
        max_expected,
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
             'ai_guard_audit_state', 'review_authority_meta', 'runner_review_bindings', \
             'ai_guard_worker', 'ai_guard_worker_attempt', 'ai_guard_worker_mutation', \
             'ai_guard_worker_observation', 'ai_guard_worker_allocation', \
             'ai_guard_supervision_meta', 'ai_guard_worker_supervision') THEN 1 END), \
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
         AND NOT EXISTS (SELECT 1 FROM runner_review_bindings) \
         AND NOT EXISTS (SELECT 1 FROM ai_guard_worker) \
         AND NOT EXISTS (SELECT 1 FROM ai_guard_worker_attempt) \
         AND NOT EXISTS (SELECT 1 FROM ai_guard_worker_mutation) \
         AND NOT EXISTS (SELECT 1 FROM ai_guard_worker_observation) \
         AND NOT EXISTS (SELECT 1 FROM ai_guard_worker_allocation) \
         AND NOT EXISTS (SELECT 1 FROM ai_guard_supervision_meta) \
         AND NOT EXISTS (SELECT 1 FROM ai_guard_worker_supervision)",
        tx,
    )
    .await?;
    ensure!(empty == [1], "MySQL unpublished common tables contain rows");
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

    verify_physical_structure(tx, version).await?;
    lock.verify_preparation_target(tx).await
}

/// Verify the published manifest on the caller's locked transaction. The seed
/// rows are present, so emptiness is not required; the physical structure,
/// catalog visibility and durable origin must still match the selected format.
pub(super) async fn verify_structure(
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
             'ai_guard_audit_state', 'review_authority_meta', 'runner_review_bindings', \
             'ai_guard_worker', 'ai_guard_worker_attempt', 'ai_guard_worker_mutation', \
             'ai_guard_worker_observation', 'ai_guard_worker_allocation', \
             'ai_guard_supervision_meta', 'ai_guard_worker_supervision') THEN 1 END), \
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
    verify_physical_structure(tx, version).await?;
    lock.verify_preparation_target(tx).await
}

/// Compare the column, index, constraint and foreign-key manifests. The caller
/// has already proven catalog visibility and the exact table inventory.
async fn verify_physical_structure(tx: &mut Transaction<'_>, version: u16) -> Result<()> {
    let columns = manifest_rows(
        "SELECT TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, \
         COLUMN_DEFAULT IS NULL AND CHARACTER_SET_NAME IS NULL AND COLLATION_NAME IS NULL \
         AND ((BINARY EXTRA = '' AND GENERATION_EXPRESSION = '') \
              OR (BINARY EXTRA = 'STORED GENERATED' AND GENERATION_EXPRESSION <> '')), \
         IF(OCTET_LENGTH(GENERATION_EXPRESSION) <= 512, GENERATION_EXPRESSION, NULL) \
         FROM information_schema.COLUMNS WHERE BINARY TABLE_SCHEMA = BINARY DATABASE() \
         ORDER BY BINARY TABLE_NAME, ORDINAL_POSITION",
        COLUMNS.len() + workers::COLUMNS.len() + supervision::COLUMNS.len() + egress::COLUMNS.len(),
        tx,
    )
    .await?;
    ensure!(
        columns_match(&columns, version),
        "MySQL owned preparation column manifest mismatch"
    );

    let indexes = manifest_rows(
        "SELECT s.TABLE_NAME, s.INDEX_NAME, s.COLUMN_NAME, s.SEQ_IN_INDEX, s.NON_UNIQUE, \
         s.SUB_PART IS NULL AND s.COLLATION = 'A' \
         AND s.INDEX_TYPE = 'BTREE' AND s.IS_VISIBLE = 'YES' AND s.EXPRESSION IS NULL \
         FROM information_schema.STATISTICS AS s WHERE BINARY s.TABLE_SCHEMA = BINARY DATABASE() \
         ORDER BY BINARY s.TABLE_NAME, BINARY s.INDEX_NAME, s.SEQ_IN_INDEX",
        INDEXES.len() + workers::INDEXES.len() + supervision::INDEXES.len() + egress::INDEXES.len(),
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
    Ok(())
}

async fn read_foreign_keys(connection: &mut impl Queryable) -> Result<Vec<ForeignKey>> {
    // InnoDB enforces both actions immediately on the verified tables.
    manifest_rows(
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
         ORDER BY BINARY k.TABLE_NAME, BINARY k.CONSTRAINT_NAME, k.ORDINAL_POSITION",
        FOREIGN_KEYS.len()
            + workers::FOREIGN_KEYS.len()
            + supervision::FOREIGN_KEYS.len()
            + egress::FOREIGN_KEYS.len(),
        connection,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_common_inventory_requires_lifecycle_and_supervision_in_both_formats() {
        for (version, egress) in [(14, 0), (15, 5)] {
            assert!(inventory_matches(&[(12 + egress, 12, egress, 1)], version));
            for partial in [5, 8, 10, 11] {
                assert!(!inventory_matches(
                    &[(partial + egress, partial, egress, 1)],
                    version
                ));
            }
            assert!(!inventory_matches(&[(12 + egress, 11, egress, 1)], version));
            assert!(!inventory_matches(&[(13 + egress, 12, egress, 1)], version));
            assert!(!inventory_matches(&[(12 + egress, 12, egress, 0)], version));
        }
    }

    #[test]
    fn canonical_worker_ddl_installs_observations_before_unique_allocations() {
        let [.., observation, allocation] = workers::ddl().unwrap();
        assert!(observation.starts_with("CREATE TABLE ai_guard_worker_observation ("));
        assert!(observation.contains("KEY ai_guard_worker_observation_attempt_idx (attempt_id)"));
        assert!(observation.contains("KEY ai_guard_worker_observation_worker_idx (worker_id)"));
        assert!(allocation.starts_with("CREATE TABLE ai_guard_worker_allocation ("));
        assert!(allocation.contains("attempt_id BINARY(36) NOT NULL PRIMARY KEY"));
        assert!(
            allocation.contains("UNIQUE KEY terminal_observation_id (terminal_observation_id)")
        );
        assert!(allocation.contains("KEY ai_guard_worker_allocation_worker_idx (worker_id)"));
        assert!(allocation.contains("REFERENCES ai_guard_worker_observation (observation_id)"));
    }

    #[test]
    fn canonical_worker_ddl_names_the_required_foreign_key_index() {
        let statements = workers::ddl().unwrap();
        assert_eq!(
            statements.map(|sql| sql.split_ascii_whitespace().nth(2).unwrap()),
            [
                "ai_guard_worker",
                "ai_guard_worker_attempt",
                "ai_guard_worker_mutation",
                "ai_guard_worker_observation",
                "ai_guard_worker_allocation",
            ],
        );
        let [worker, attempt, mutation, ..] = statements;
        assert!(worker.contains("UNIQUE KEY event_seq (event_seq)"));
        assert!(attempt.contains("UNIQUE KEY worker_id (worker_id)"));
        assert!(mutation.contains("KEY ai_guard_worker_mutation_worker_idx (worker_id)"));
        assert!(mutation.contains("CONSTRAINT ai_guard_worker_mutation_worker_ref"));
        assert!(mutation.contains("UNIQUE KEY event_seq (event_seq)"));
    }

    #[test]
    fn owned_audit_manifest_rejects_column_and_index_drift() {
        let mut columns: Vec<Column> = expected_in_format(
            &[&COLUMNS, &workers::COLUMNS, &supervision::COLUMNS],
            &egress::COLUMNS,
            14,
            |row| row.0,
        )
        .into_iter()
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
        .collect();
        assert_eq!(columns.len(), 78);
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
        assert_eq!(extended.len(), 104);
        assert!(columns_match(&extended, 15));
        assert!(!columns_match(&extended, 14));
        for (version, columns) in [(14, &mut columns), (15, &mut extended)] {
            let protocol = columns
                .iter()
                .position(|row| row.0 == "ai_guard_supervision_meta" && row.1 == "protocol")
                .unwrap();
            columns[protocol].2 = "varchar(32)".into();
            assert!(!columns_match(columns, version));
            columns[protocol].2 = "varbinary(32)".into();
            let allocation_snapshot = columns
                .iter()
                .position(|row| row.0 == "ai_guard_worker_allocation" && row.1 == "snapshot")
                .unwrap();
            columns[allocation_snapshot].2 = "blob".into();
            assert!(!columns_match(columns, version));
            columns[allocation_snapshot].2 = "mediumblob".into();
            for missing in [
                "ai_guard_worker_observation",
                "ai_guard_worker_allocation",
                "ai_guard_supervision_meta",
                "ai_guard_worker_supervision",
            ] {
                let mut partial = columns.clone();
                partial.retain(|row| row.0 != missing);
                assert!(!columns_match(&partial, version));
            }
            assert!(columns_match(columns, version));
        }
        let received_at = columns
            .iter()
            .position(|row| row.0 == EVENT && row.1 == "received_at")
            .unwrap();
        columns[received_at].2 = "varbinary(31)".into();
        assert!(!columns_match(&columns, 14));
        columns[received_at].2 = "varbinary(32)".into();
        let mutation_worker = columns
            .iter()
            .position(|row| row.0 == "ai_guard_worker_mutation" && row.1 == "worker_id")
            .unwrap();
        assert_eq!(columns[mutation_worker].3, "YES");
        columns[mutation_worker].3 = "NO".into();
        assert!(!columns_match(&columns, 14));
        columns[mutation_worker].3 = "YES".into();
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

        let common = INDEXES.map(|(table, name, column, position, shape)| {
            (table, name, column, position, 0, shape)
        });
        let mut indexes: Vec<Index> = expected_in_format(
            &[&common, &workers::INDEXES, &supervision::INDEXES],
            &egress::INDEXES,
            14,
            |row| row.0,
        )
        .into_iter()
        .map(|(table, name, column, position, non_unique, shape)| {
            (
                table.into(),
                name.into(),
                column.map(str::to_owned),
                position,
                non_unique,
                shape,
            )
        })
        .collect();
        assert_eq!(indexes.len(), 40);
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
        assert_eq!(extended.len(), 56);
        assert!(indexes_match(&extended, 15));
        assert!(!indexes_match(&extended, 14));
        let key = extended
            .iter()
            .position(|row| row.1 == "egress_heads_revision_key")
            .unwrap();
        extended[key].4 = 0;
        assert!(!indexes_match(&extended, 15));
        extended[key].4 = 1;
        for (version, indexes) in [(14, &mut indexes), (15, &mut extended)] {
            let terminal_observation = indexes
                .iter()
                .position(|row| {
                    row.0 == "ai_guard_worker_allocation" && row.1 == "terminal_observation_id"
                })
                .unwrap();
            assert_eq!(indexes[terminal_observation].4, 0);
            indexes[terminal_observation].4 = 1;
            assert!(!indexes_match(indexes, version));
            indexes[terminal_observation].4 = 0;
            for (table, name, column) in [
                (
                    "ai_guard_worker_supervision",
                    "ai_guard_worker_supervision_event_ref",
                    "event_seq",
                ),
                (
                    "ai_guard_worker_observation",
                    "ai_guard_worker_observation_attempt_ref",
                    "attempt_id",
                ),
            ] {
                let mut surplus = indexes.clone();
                surplus.push((table.into(), name.into(), Some(column.into()), 1, 1, 1));
                surplus.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
                assert!(!indexes_match(&surplus, version));
            }
            assert!(indexes_match(indexes, version));
        }
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
        let covering = indexes
            .iter()
            .position(|row| row.1 == "ai_guard_worker_mutation_worker_idx")
            .unwrap();
        assert_eq!(indexes[covering].4, 1);
        indexes[covering].1 = "ai_guard_worker_mutation_worker_ref".into();
        assert!(!indexes_match(&indexes, 14));
        indexes[covering].1 = "ai_guard_worker_mutation_worker_idx".into();
        indexes[covering].4 = 0;
        assert!(!indexes_match(&indexes, 14));
        indexes[covering].4 = 1;
        let mut extra = indexes.clone();
        extra.insert(covering + 1, indexes[covering].clone());
        assert!(!indexes_match(&extra, 14));
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

        let common = FOREIGN_KEYS.map(|(t, n, c, p, r, exact)| (t, n, c, p, r, 1, exact));
        let mut links: Vec<ForeignKey> = expected_in_format(
            &[&common, &workers::FOREIGN_KEYS, &supervision::FOREIGN_KEYS],
            &egress::FOREIGN_KEYS,
            14,
            |row| row.0,
        )
        .into_iter()
        .map(|(table, key, column, parent, reference, position, exact)| {
            (
                table.into(),
                key.into(),
                column.into(),
                parent.into(),
                reference.into(),
                position,
                exact,
            )
        })
        .collect();
        assert_eq!(links.len(), 18);
        assert!(foreign_keys_match(&links, 14));
        let review_event = links
            .iter()
            .position(|row| row.1 == "review_binding_audit_event")
            .unwrap();
        links[review_event].3 = "other_audit_event".into();
        assert!(!foreign_keys_match(&links, 14));
        links[review_event].3 = EVENT.into();
        let mutation_worker = links
            .iter()
            .position(|row| row.1 == "ai_guard_worker_mutation_worker_ref")
            .unwrap();
        links[mutation_worker].3 = EVENT.into();
        assert!(!foreign_keys_match(&links, 14));
        links[mutation_worker].3 = "ai_guard_worker".into();
        let terminal_observation = links
            .iter()
            .position(|row| row.1 == "ai_guard_worker_allocation_observation_ref")
            .unwrap();
        assert_eq!(links[terminal_observation].3, "ai_guard_worker_observation");
        assert_eq!(links[terminal_observation].4, "observation_id");
        links[terminal_observation].3 = "ai_guard_worker_attempt".into();
        assert!(!foreign_keys_match(&links, 14));
        links[terminal_observation].3 = "ai_guard_worker_observation".into();
        let decision_event = links.iter().position(|row| row.0 == DECISION).unwrap();
        links[decision_event].6 = 0;
        assert!(!foreign_keys_match(&links, 14));
        links[decision_event].6 = 1;
        assert!(foreign_keys_match(&links, 14));
        let supervision_worker = links
            .iter()
            .position(|row| row.1 == "ai_guard_worker_supervision_worker_ref")
            .unwrap();
        links[supervision_worker].3 = EVENT.into();
        assert!(!foreign_keys_match(&links, 14));
    }

    #[test]
    fn owned_egress_manifest_rejects_wrong_format_and_composite_edges() {
        assert!(!inventory_matches(&[(13, 12, 1, 1)], 14));
        assert!(!inventory_matches(&[(16, 12, 4, 1)], 15));
        assert!(!inventory_matches(&[(17, 12, 5, 1)], 14));

        let common = FOREIGN_KEYS.map(|(t, n, c, p, r, exact)| (t, n, c, p, r, 1, exact));
        let mut links: Vec<ForeignKey> = expected_in_format(
            &[&common, &workers::FOREIGN_KEYS, &supervision::FOREIGN_KEYS],
            &egress::FOREIGN_KEYS,
            15,
            |row| row.0,
        )
        .into_iter()
        .map(|(t, n, c, p, r, pos, exact)| {
            (t.into(), n.into(), c.into(), p.into(), r.into(), pos, exact)
        })
        .collect();
        assert_eq!(links.len(), 27);
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
                &[
                    &expected_constraints(&clause),
                    &workers::CONSTRAINTS,
                    &supervision::CONSTRAINTS,
                ],
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
            assert_eq!(constraints.len(), if version == 14 { 94 } else { 120 });
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
            let protocol = constraints
                .iter()
                .position(|row| row.1 == "ai_guard_supervision_meta_protocol")
                .unwrap();
            let dictionary_clause =
                "((length(`protocol`) = 21) and (`protocol` = _binary'broker_supervision_v1'))";
            constraints[protocol].4 = Some(dictionary_clause.into());
            assert!(constraints_match(&constraints, version));
            for changed in [
                dictionary_clause.replace("_binary", "_utf8mb4"),
                dictionary_clause.replace("supervision_v1", "supervision_v2"),
                dictionary_clause.replace(" and ", " or "),
            ] {
                constraints[protocol].4 = Some(changed);
                assert!(!constraints_match(&constraints, version));
            }
            constraints[protocol].4 = Some(dictionary_clause.into());
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
            let worker_revision = constraints
                .iter()
                .position(|row| row.1 == "ai_guard_worker_revision")
                .unwrap();
            let original = constraints[worker_revision]
                .4
                .replace("(`revision` >= 0)".into());
            assert!(!constraints_match(&constraints, version));
            constraints[worker_revision].4 = original;
            let lifecycle_snapshot = constraints
                .iter()
                .position(|row| row.1 == "ai_guard_worker_allocation_snapshot_size")
                .unwrap();
            let original = constraints[lifecycle_snapshot]
                .4
                .replace("(`snapshot` is not null)".into());
            assert!(!constraints_match(&constraints, version));
            constraints[lifecycle_snapshot].4 = original;
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
