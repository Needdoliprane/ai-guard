//! Closed physical extension for the five unpublished format-15 projections.

use super::audit_manifest::{
    ExpectedColumn, ExpectedConstraint, ExpectedForeignKey, ExpectedIndex,
};

const HEADS: &str = "egress_policy_heads";
const META: &str = "egress_policy_meta";
const OPERATIONS: &str = "egress_policy_operations";
const RESERVATIONS: &str = "egress_policy_reservations";
const REVISIONS: &str = "egress_policy_revisions";
const AUDIT: &str = "ai_guard_audit_event";

const CREATION_ORDER: [&str; 5] = [META, REVISIONS, HEADS, OPERATIONS, RESERVATIONS];

pub(super) fn ddl(version: u16) -> Option<Vec<&'static str>> {
    match version {
        14 => return Some(Vec::new()),
        15 => (),
        _ => return None,
    }
    let (_, body) =
        include_str!("../../../../schema/mysql/egress-owned-v1.sql").split_once('\n')?;
    let mut statements = body.split(';').map(str::trim).filter(|sql| !sql.is_empty());
    let mut ddl = Vec::with_capacity(CREATION_ORDER.len());
    for table in CREATION_ORDER {
        let prefix = format!("CREATE TABLE {table} (");
        ddl.push(statements.next().filter(|sql| sql.starts_with(&prefix))?);
    }
    statements.next().is_none().then_some(ddl)
}

pub(super) const COLUMNS: [ExpectedColumn; 26] = [
    column(HEADS, "scope_kind", "varbinary(7)"),
    column(HEADS, "scope_id", "varbinary(36)"),
    column(HEADS, "revision", "bigint"),
    column(HEADS, "epoch", "bigint"),
    column(HEADS, "state", "varbinary(16)"),
    column(HEADS, "event_seq", "bigint"),
    column(META, "singleton", "tinyint unsigned"),
    column(META, "version", "tinyint unsigned"),
    column(META, "genesis_seq", "bigint"),
    column(META, "genesis_hash", "binary(64)"),
    column(META, "limits_json", "mediumblob"),
    column(OPERATIONS, "operation_id", "varbinary(36)"),
    column(OPERATIONS, "action", "varbinary(7)"),
    column(OPERATIONS, "input_hash", "binary(64)"),
    column(OPERATIONS, "receipt_json", "mediumblob"),
    column(OPERATIONS, "event_seq", "bigint"),
    column(RESERVATIONS, "scope_kind", "varbinary(7)"),
    column(RESERVATIONS, "scope_id", "varbinary(36)"),
    column(RESERVATIONS, "events", "tinyint unsigned"),
    column(RESERVATIONS, "bytes", "int unsigned"),
    column(REVISIONS, "scope_kind", "varbinary(7)"),
    column(REVISIONS, "scope_id", "varbinary(36)"),
    column(REVISIONS, "revision", "bigint"),
    column(REVISIONS, "policy_json", "mediumblob"),
    column(REVISIONS, "policy_digest", "binary(64)"),
    column(REVISIONS, "event_seq", "bigint"),
];

const fn column(table: &'static str, name: &'static str, ty: &'static str) -> ExpectedColumn {
    (table, name, ty, "NO", 1)
}

pub(super) const INDEXES: [ExpectedIndex; 16] = [
    unique_index(HEADS, "PRIMARY", "scope_kind", 1),
    unique_index(HEADS, "PRIMARY", "scope_id", 2),
    unique_index(HEADS, "egress_heads_event_key", "event_seq", 1),
    (
        HEADS,
        "egress_heads_revision_key",
        Some("scope_kind"),
        1,
        1,
        1,
    ),
    (
        HEADS,
        "egress_heads_revision_key",
        Some("scope_id"),
        2,
        1,
        1,
    ),
    (
        HEADS,
        "egress_heads_revision_key",
        Some("revision"),
        3,
        1,
        1,
    ),
    unique_index(META, "PRIMARY", "singleton", 1),
    unique_index(META, "egress_meta_genesis_key", "genesis_seq", 1),
    unique_index(OPERATIONS, "PRIMARY", "operation_id", 1),
    unique_index(OPERATIONS, "egress_operations_event_key", "event_seq", 1),
    unique_index(RESERVATIONS, "PRIMARY", "scope_kind", 1),
    unique_index(RESERVATIONS, "PRIMARY", "scope_id", 2),
    unique_index(REVISIONS, "PRIMARY", "scope_kind", 1),
    unique_index(REVISIONS, "PRIMARY", "scope_id", 2),
    unique_index(REVISIONS, "PRIMARY", "revision", 3),
    unique_index(REVISIONS, "egress_revisions_event_key", "event_seq", 1),
];

const fn unique_index(
    table: &'static str,
    name: &'static str,
    column: &'static str,
    position: u8,
) -> ExpectedIndex {
    (table, name, Some(column), position, 0, 1)
}

pub(super) const CONSTRAINTS: [ExpectedConstraint<'static>; 26] = [
    primary(HEADS),
    check(HEADS, "egress_heads_epoch", "(`epoch` > 0)"),
    unique(HEADS, "egress_heads_event_key"),
    foreign(HEADS, "egress_heads_event_ref"),
    check(HEADS, "egress_heads_revision", "(`revision` > 0)"),
    foreign(HEADS, "egress_heads_revision_ref"),
    check(
        HEADS,
        "egress_heads_state",
        "(`state` in (0x616374697665,0x7265766f6b655f726571756573746564))",
    ),
    primary(META),
    unique(META, "egress_meta_genesis_key"),
    foreign(META, "egress_meta_genesis_ref"),
    check(META, "egress_meta_singleton", "(`singleton` = 1)"),
    check(META, "egress_meta_version", "(`version` = 1)"),
    primary(OPERATIONS),
    check(
        OPERATIONS,
        "egress_operations_action",
        "(`action` in (0x7265706c616365,0x7265766f6b65))",
    ),
    unique(OPERATIONS, "egress_operations_event_key"),
    foreign(OPERATIONS, "egress_operations_event_ref"),
    primary(RESERVATIONS),
    check(
        RESERVATIONS,
        "egress_reservations_bytes",
        "(`bytes` = 98304)",
    ),
    check(RESERVATIONS, "egress_reservations_events", "(`events` = 1)"),
    foreign(RESERVATIONS, "egress_reservations_head_ref"),
    primary(REVISIONS),
    unique(REVISIONS, "egress_revisions_event_key"),
    foreign(REVISIONS, "egress_revisions_event_ref"),
    check(REVISIONS, "egress_revisions_revision", "(`revision` > 0)"),
    check(
        REVISIONS,
        "egress_revisions_scope_id",
        concat!(
            "(((`scope_kind` = 0x74656e616e74) and (length(`scope_id`) = 0)) or ",
            "((`scope_kind` = 0x73657373696f6e) and (length(`scope_id`) = 36)))",
        ),
    ),
    check(
        REVISIONS,
        "egress_revisions_scope_kind",
        "(`scope_kind` in (0x74656e616e74,0x73657373696f6e))",
    ),
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

pub(super) const FOREIGN_KEYS: [ExpectedForeignKey; 9] = [
    reference(
        HEADS,
        "egress_heads_event_ref",
        "event_seq",
        AUDIT,
        "seq",
        1,
    ),
    reference(
        HEADS,
        "egress_heads_revision_ref",
        "scope_kind",
        REVISIONS,
        "scope_kind",
        1,
    ),
    reference(
        HEADS,
        "egress_heads_revision_ref",
        "scope_id",
        REVISIONS,
        "scope_id",
        2,
    ),
    reference(
        HEADS,
        "egress_heads_revision_ref",
        "revision",
        REVISIONS,
        "revision",
        3,
    ),
    reference(
        META,
        "egress_meta_genesis_ref",
        "genesis_seq",
        AUDIT,
        "seq",
        1,
    ),
    reference(
        OPERATIONS,
        "egress_operations_event_ref",
        "event_seq",
        AUDIT,
        "seq",
        1,
    ),
    reference(
        RESERVATIONS,
        "egress_reservations_head_ref",
        "scope_kind",
        HEADS,
        "scope_kind",
        1,
    ),
    reference(
        RESERVATIONS,
        "egress_reservations_head_ref",
        "scope_id",
        HEADS,
        "scope_id",
        2,
    ),
    reference(
        REVISIONS,
        "egress_revisions_event_ref",
        "event_seq",
        AUDIT,
        "seq",
        1,
    ),
];

const fn reference(
    table: &'static str,
    key: &'static str,
    column: &'static str,
    parent: &'static str,
    target: &'static str,
    position: u8,
) -> ExpectedForeignKey {
    (table, key, column, parent, target, position, 1)
}

#[cfg(test)]
mod tests {
    use super::ddl;

    #[test]
    fn owned_egress_ddl_is_selected_only_for_format_15() {
        assert!(ddl(14).unwrap().is_empty());
        assert!(ddl(16).is_none());
        let statements = ddl(15).unwrap();
        let names: Vec<_> = statements
            .iter()
            .map(|sql| sql.split_ascii_whitespace().nth(2).unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "egress_policy_meta",
                "egress_policy_revisions",
                "egress_policy_heads",
                "egress_policy_operations",
                "egress_policy_reservations",
            ]
        );
        assert!(
            statements[2]
                .contains("REFERENCES egress_policy_revisions (scope_kind, scope_id, revision)")
        );
        assert!(statements[4].contains("REFERENCES egress_policy_heads (scope_kind, scope_id)"));
        assert!(statements[4].contains("CHECK (bytes = 98304)"));
    }
}
