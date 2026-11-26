//! Common unpublished supervision tables for MySQL formats 14 and 15.

use super::audit_manifest::{
    ExpectedColumn, ExpectedConstraint, ExpectedForeignKey, ExpectedIndex,
};

const META: &str = "ai_guard_supervision_meta";
const WORKER_SUPERVISION: &str = "ai_guard_worker_supervision";
const EVENT: &str = "ai_guard_audit_event";
const WORKER: &str = "ai_guard_worker";

pub(super) fn ddl() -> Option<[&'static str; 2]> {
    Some([
        single_create(
            include_str!("../../../../schema/mysql/supervision-v1.sql"),
            META,
        )?,
        single_create(
            include_str!("../../../../schema/mysql/worker-supervision-v1.sql"),
            WORKER_SUPERVISION,
        )?,
    ])
}

fn single_create(ddl: &'static str, table: &str) -> Option<&'static str> {
    let mut body = ddl.trim_start();
    while body.starts_with("-- ") {
        let (_, rest) = body.split_once('\n')?;
        body = rest.trim_start();
    }
    let statement = body.trim_end().strip_suffix(';')?;
    let columns = statement
        .strip_prefix("CREATE TABLE ")?
        .strip_prefix(table)?;
    (columns.starts_with(" (") && !statement.contains(';')).then_some(statement)
}

pub(super) const COLUMNS: [ExpectedColumn; 8] = [
    (META, "singleton", "tinyint unsigned", "NO", 1),
    (META, "protocol", "varbinary(32)", "NO", 1),
    (META, "genesis_seq", "bigint", "NO", 1),
    (META, "genesis_hash", "binary(64)", "NO", 1),
    (WORKER_SUPERVISION, "worker_id", "binary(36)", "NO", 1),
    (WORKER_SUPERVISION, "intent_hash", "binary(64)", "NO", 1),
    (WORKER_SUPERVISION, "payload_json", "mediumblob", "NO", 1),
    (WORKER_SUPERVISION, "event_seq", "bigint", "NO", 1),
];

// The existing primary and unique indexes cover all three foreign keys.
pub(super) const INDEXES: [ExpectedIndex; 5] = [
    (META, "PRIMARY", Some("singleton"), 1, 0, 1),
    (META, "genesis_seq", Some("genesis_seq"), 1, 0, 1),
    (WORKER_SUPERVISION, "PRIMARY", Some("worker_id"), 1, 0, 1),
    (WORKER_SUPERVISION, "event_seq", Some("event_seq"), 1, 0, 1),
    (
        WORKER_SUPERVISION,
        "intent_hash",
        Some("intent_hash"),
        1,
        0,
        1,
    ),
];

pub(super) const CONSTRAINTS: [ExpectedConstraint<'static>; 15] = [
    (META, "PRIMARY", "PRIMARY KEY", "YES", None),
    check(
        META,
        "ai_guard_supervision_meta_genesis_hash",
        "(locate(0x00,`genesis_hash`) = 0)",
    ),
    foreign_key(META, "ai_guard_supervision_meta_genesis_ref"),
    check(
        META,
        "ai_guard_supervision_meta_genesis_seq",
        "(`genesis_seq` > 0)",
    ),
    // MySQL 8.4.13 retains backslashes before the binary literal's quotes
    // in the dictionary CHECK_CLAUSE.
    check(
        META,
        "ai_guard_supervision_meta_protocol",
        r"((length(`protocol`) = 21) and (`protocol` = _binary\'broker_supervision_v1\'))",
    ),
    check(
        META,
        "ai_guard_supervision_meta_singleton",
        "(`singleton` = 1)",
    ),
    (META, "genesis_seq", "UNIQUE", "YES", None),
    (WORKER_SUPERVISION, "PRIMARY", "PRIMARY KEY", "YES", None),
    foreign_key(WORKER_SUPERVISION, "ai_guard_worker_supervision_event_ref"),
    check(
        WORKER_SUPERVISION,
        "ai_guard_worker_supervision_event_seq",
        "(`event_seq` > 0)",
    ),
    check(
        WORKER_SUPERVISION,
        "ai_guard_worker_supervision_intent_hash",
        "(locate(0x00,`intent_hash`) = 0)",
    ),
    check(
        WORKER_SUPERVISION,
        "ai_guard_worker_supervision_payload_size",
        "(length(`payload_json`) between 1 and 131072)",
    ),
    foreign_key(WORKER_SUPERVISION, "ai_guard_worker_supervision_worker_ref"),
    (WORKER_SUPERVISION, "event_seq", "UNIQUE", "YES", None),
    (WORKER_SUPERVISION, "intent_hash", "UNIQUE", "YES", None),
];

const fn check(
    table: &'static str,
    name: &'static str,
    clause: &'static str,
) -> ExpectedConstraint<'static> {
    (table, name, "CHECK", "YES", Some(clause))
}

const fn foreign_key(table: &'static str, name: &'static str) -> ExpectedConstraint<'static> {
    (table, name, "FOREIGN KEY", "YES", None)
}

pub(super) const FOREIGN_KEYS: [ExpectedForeignKey; 3] = [
    (
        META,
        "ai_guard_supervision_meta_genesis_ref",
        "genesis_seq",
        EVENT,
        "seq",
        1,
        1,
    ),
    (
        WORKER_SUPERVISION,
        "ai_guard_worker_supervision_event_ref",
        "event_seq",
        EVENT,
        "seq",
        1,
        1,
    ),
    (
        WORKER_SUPERVISION,
        "ai_guard_worker_supervision_worker_ref",
        "worker_id",
        WORKER,
        "worker_id",
        1,
        1,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_supervision_ddl_creates_only_empty_tables_with_covered_foreign_keys() {
        let [meta, worker] = ddl().unwrap();
        assert!(meta.contains("genesis_seq BIGINT NOT NULL UNIQUE"));
        assert!(meta.contains("FOREIGN KEY (genesis_seq) REFERENCES ai_guard_audit_event (seq)"));
        assert!(meta.contains("protocol = _binary 'broker_supervision_v1'"));
        assert!(worker.contains("worker_id BINARY(36) NOT NULL PRIMARY KEY"));
        assert!(worker.contains("intent_hash BINARY(64) NOT NULL UNIQUE"));
        assert!(worker.contains("event_seq BIGINT NOT NULL UNIQUE"));
        assert!(worker.contains("FOREIGN KEY (worker_id) REFERENCES ai_guard_worker (worker_id)"));
        assert!(worker.contains("FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event (seq)"));
        assert!(single_create("CREATE TABLE t (id INT); INSERT INTO t VALUES (1);", "t").is_none());
    }
}
