//! Common unpublished worker reservation and lifecycle tables for MySQL formats 14 and 15.

use super::audit_manifest::{
    ExpectedColumn, ExpectedConstraint, ExpectedForeignKey, ExpectedIndex,
};

const WORKER: &str = "ai_guard_worker";
const ATTEMPT: &str = "ai_guard_worker_attempt";
const MUTATION: &str = "ai_guard_worker_mutation";
const OBSERVATION: &str = "ai_guard_worker_observation";
const ALLOCATION: &str = "ai_guard_worker_allocation";
const EVENT: &str = "ai_guard_audit_event";

pub(super) fn ddl() -> Option<[&'static str; 5]> {
    let [worker, attempt, mutation] = table_ddl(
        include_str!("../../../../schema/mysql/workers-v1.sql"),
        [WORKER, ATTEMPT, MUTATION],
    )?;
    let [observation, allocation] = table_ddl(
        include_str!("../../../../schema/mysql/worker-lifecycle-v1.sql"),
        [OBSERVATION, ALLOCATION],
    )?;
    Some([worker, attempt, mutation, observation, allocation])
}

fn table_ddl<const N: usize>(
    mut ddl: &'static str,
    tables: [&str; N],
) -> Option<[&'static str; N]> {
    while ddl.starts_with("-- ") {
        (_, ddl) = ddl.split_once('\n')?;
    }
    let mut statements = ddl.split(';').map(str::trim).filter(|sql| !sql.is_empty());
    let mut ordered = [""; N];
    for (table, statement) in tables.into_iter().zip(&mut ordered) {
        let sql = statements.next()?;
        sql.strip_prefix("CREATE TABLE ")?
            .strip_prefix(table)?
            .strip_prefix(" (")?;
        *statement = sql;
    }
    statements.next().is_none().then_some(ordered)
}

pub(super) const COLUMNS: [ExpectedColumn; 34] = [
    column(WORKER, "worker_id", "binary(36)"),
    column(WORKER, "operation_id", "binary(36)"),
    column(WORKER, "request_id", "binary(36)"),
    column(WORKER, "session_id", "binary(36)"),
    column(WORKER, "state", "varbinary(24)"),
    column(WORKER, "quota_held", "tinyint unsigned"),
    column(WORKER, "cpu_millis", "int unsigned"),
    column(WORKER, "memory_bytes", "bigint unsigned"),
    column(WORKER, "revision", "bigint unsigned"),
    column(WORKER, "snapshot", "mediumblob"),
    column(WORKER, "snapshot_hash", "binary(64)"),
    column(WORKER, "event_seq", "bigint"),
    column(ATTEMPT, "attempt_id", "binary(36)"),
    column(ATTEMPT, "worker_id", "binary(36)"),
    column(ATTEMPT, "authority_id", "varbinary(256)"),
    column(ATTEMPT, "stable_name", "varbinary(96)"),
    column(ATTEMPT, "volume_name", "varbinary(96)"),
    column(ATTEMPT, "snapshot", "mediumblob"),
    column(MUTATION, "operation_id", "binary(36)"),
    (MUTATION, "worker_id", "binary(36)", "YES", 1),
    column(MUTATION, "action", "varbinary(32)"),
    column(MUTATION, "input_hash", "binary(64)"),
    column(MUTATION, "event_seq", "bigint"),
    column(OBSERVATION, "observation_id", "binary(36)"),
    column(OBSERVATION, "worker_id", "binary(36)"),
    column(OBSERVATION, "attempt_id", "binary(36)"),
    column(OBSERVATION, "input_hash", "binary(64)"),
    column(OBSERVATION, "snapshot", "mediumblob"),
    column(OBSERVATION, "event_seq", "bigint"),
    column(ALLOCATION, "attempt_id", "binary(36)"),
    column(ALLOCATION, "worker_id", "binary(36)"),
    column(ALLOCATION, "terminal_observation_id", "binary(36)"),
    column(ALLOCATION, "snapshot", "mediumblob"),
    column(ALLOCATION, "event_seq", "bigint"),
];

const fn column(table: &'static str, name: &'static str, ty: &'static str) -> ExpectedColumn {
    (table, name, ty, "NO", 1)
}

pub(super) const INDEXES: [ExpectedIndex; 20] = [
    (WORKER, "PRIMARY", Some("worker_id"), 1, 0, 1),
    unique_index(WORKER, "event_seq"),
    unique_index(WORKER, "operation_id"),
    unique_index(WORKER, "request_id"),
    unique_index(WORKER, "session_id"),
    (ATTEMPT, "PRIMARY", Some("attempt_id"), 1, 0, 1),
    unique_index(ATTEMPT, "stable_name"),
    unique_index(ATTEMPT, "volume_name"),
    unique_index(ATTEMPT, "worker_id"),
    (MUTATION, "PRIMARY", Some("operation_id"), 1, 0, 1),
    covering_index(MUTATION, "ai_guard_worker_mutation_worker_idx", "worker_id"),
    unique_index(MUTATION, "event_seq"),
    (OBSERVATION, "PRIMARY", Some("observation_id"), 1, 0, 1),
    covering_index(
        OBSERVATION,
        "ai_guard_worker_observation_attempt_idx",
        "attempt_id",
    ),
    covering_index(
        OBSERVATION,
        "ai_guard_worker_observation_worker_idx",
        "worker_id",
    ),
    unique_index(OBSERVATION, "event_seq"),
    (ALLOCATION, "PRIMARY", Some("attempt_id"), 1, 0, 1),
    covering_index(
        ALLOCATION,
        "ai_guard_worker_allocation_worker_idx",
        "worker_id",
    ),
    unique_index(ALLOCATION, "event_seq"),
    unique_index(ALLOCATION, "terminal_observation_id"),
];

// When the primary or a unique key does not cover a foreign key, the DDL
// declares a separate nonunique index so no implicit index name is expected.
const fn covering_index(
    table: &'static str,
    name: &'static str,
    column: &'static str,
) -> ExpectedIndex {
    (table, name, Some(column), 1, 1, 1)
}

const fn unique_index(table: &'static str, column: &'static str) -> ExpectedIndex {
    (table, column, Some(column), 1, 0, 1)
}

// Expected MySQL dictionary renderings. The physical 8.4 matrix must show
// these exact names and CHECK_CLAUSE forms (apart from identifier quotes).
pub(super) const CONSTRAINTS: [ExpectedConstraint<'static>; 45] = [
    primary(WORKER),
    check(
        WORKER,
        "ai_guard_worker_cpu",
        "(`cpu_millis` between 1 and 200000)",
    ),
    foreign(WORKER, "ai_guard_worker_event_ref"),
    check(
        WORKER,
        "ai_guard_worker_memory",
        "(`memory_bytes` between 1 and 107374182400)",
    ),
    check(
        WORKER,
        "ai_guard_worker_quota_held",
        "(`quota_held` in (0,1))",
    ),
    check(WORKER, "ai_guard_worker_revision", "(`revision` >= 1)"),
    check(
        WORKER,
        "ai_guard_worker_snapshot_hash",
        "(locate(0x00,`snapshot_hash`) = 0)",
    ),
    check(
        WORKER,
        "ai_guard_worker_snapshot_size",
        "(length(`snapshot`) between 1 and 131072)",
    ),
    check(
        WORKER,
        "ai_guard_worker_state_size",
        "(length(`state`) between 1 and 24)",
    ),
    unique(WORKER, "event_seq"),
    unique(WORKER, "operation_id"),
    unique(WORKER, "request_id"),
    unique(WORKER, "session_id"),
    primary(ATTEMPT),
    check(
        ATTEMPT,
        "ai_guard_worker_attempt_authority_size",
        "(length(`authority_id`) between 1 and 256)",
    ),
    check(
        ATTEMPT,
        "ai_guard_worker_attempt_snapshot_size",
        "(length(`snapshot`) between 1 and 131072)",
    ),
    check(
        ATTEMPT,
        "ai_guard_worker_attempt_stable_name_size",
        "(length(`stable_name`) between 1 and 96)",
    ),
    check(
        ATTEMPT,
        "ai_guard_worker_attempt_volume_name_size",
        "(length(`volume_name`) between 1 and 96)",
    ),
    foreign(ATTEMPT, "ai_guard_worker_attempt_worker_ref"),
    unique(ATTEMPT, "stable_name"),
    unique(ATTEMPT, "volume_name"),
    unique(ATTEMPT, "worker_id"),
    primary(MUTATION),
    check(
        MUTATION,
        "ai_guard_worker_mutation_action_size",
        "(length(`action`) between 1 and 32)",
    ),
    foreign(MUTATION, "ai_guard_worker_mutation_event_ref"),
    check(
        MUTATION,
        "ai_guard_worker_mutation_input_hash",
        "(locate(0x00,`input_hash`) = 0)",
    ),
    foreign(MUTATION, "ai_guard_worker_mutation_worker_ref"),
    unique(MUTATION, "event_seq"),
    primary(OBSERVATION),
    foreign(OBSERVATION, "ai_guard_worker_observation_attempt_ref"),
    foreign(OBSERVATION, "ai_guard_worker_observation_event_ref"),
    check(
        OBSERVATION,
        "ai_guard_worker_observation_event_seq",
        "(`event_seq` > 0)",
    ),
    check(
        OBSERVATION,
        "ai_guard_worker_observation_input_hash",
        "(locate(0x00,`input_hash`) = 0)",
    ),
    check(
        OBSERVATION,
        "ai_guard_worker_observation_snapshot_size",
        "(length(`snapshot`) between 1 and 131072)",
    ),
    foreign(OBSERVATION, "ai_guard_worker_observation_worker_ref"),
    unique(OBSERVATION, "event_seq"),
    primary(ALLOCATION),
    foreign(ALLOCATION, "ai_guard_worker_allocation_attempt_ref"),
    foreign(ALLOCATION, "ai_guard_worker_allocation_event_ref"),
    check(
        ALLOCATION,
        "ai_guard_worker_allocation_event_seq",
        "(`event_seq` > 0)",
    ),
    foreign(ALLOCATION, "ai_guard_worker_allocation_observation_ref"),
    check(
        ALLOCATION,
        "ai_guard_worker_allocation_snapshot_size",
        "(length(`snapshot`) between 1 and 131072)",
    ),
    foreign(ALLOCATION, "ai_guard_worker_allocation_worker_ref"),
    unique(ALLOCATION, "event_seq"),
    unique(ALLOCATION, "terminal_observation_id"),
];

const fn primary(table: &'static str) -> ExpectedConstraint<'static> {
    (table, "PRIMARY", "PRIMARY KEY", "YES", None)
}

const fn unique(table: &'static str, name: &'static str) -> ExpectedConstraint<'static> {
    (table, name, "UNIQUE", "YES", None)
}

const fn foreign(table: &'static str, name: &'static str) -> ExpectedConstraint<'static> {
    (table, name, "FOREIGN KEY", "YES", None)
}

const fn check(
    table: &'static str,
    name: &'static str,
    expression: &'static str,
) -> ExpectedConstraint<'static> {
    (table, name, "CHECK", "YES", Some(expression))
}

pub(super) const FOREIGN_KEYS: [ExpectedForeignKey; 11] = [
    (
        WORKER,
        "ai_guard_worker_event_ref",
        "event_seq",
        EVENT,
        "seq",
        1,
        1,
    ),
    (
        ATTEMPT,
        "ai_guard_worker_attempt_worker_ref",
        "worker_id",
        WORKER,
        "worker_id",
        1,
        1,
    ),
    (
        MUTATION,
        "ai_guard_worker_mutation_event_ref",
        "event_seq",
        EVENT,
        "seq",
        1,
        1,
    ),
    (
        MUTATION,
        "ai_guard_worker_mutation_worker_ref",
        "worker_id",
        WORKER,
        "worker_id",
        1,
        1,
    ),
    (
        OBSERVATION,
        "ai_guard_worker_observation_attempt_ref",
        "attempt_id",
        ATTEMPT,
        "attempt_id",
        1,
        1,
    ),
    (
        OBSERVATION,
        "ai_guard_worker_observation_event_ref",
        "event_seq",
        EVENT,
        "seq",
        1,
        1,
    ),
    (
        OBSERVATION,
        "ai_guard_worker_observation_worker_ref",
        "worker_id",
        WORKER,
        "worker_id",
        1,
        1,
    ),
    (
        ALLOCATION,
        "ai_guard_worker_allocation_attempt_ref",
        "attempt_id",
        ATTEMPT,
        "attempt_id",
        1,
        1,
    ),
    (
        ALLOCATION,
        "ai_guard_worker_allocation_event_ref",
        "event_seq",
        EVENT,
        "seq",
        1,
        1,
    ),
    (
        ALLOCATION,
        "ai_guard_worker_allocation_observation_ref",
        "terminal_observation_id",
        OBSERVATION,
        "observation_id",
        1,
        1,
    ),
    (
        ALLOCATION,
        "ai_guard_worker_allocation_worker_ref",
        "worker_id",
        WORKER,
        "worker_id",
        1,
        1,
    ),
];
