//! Exact extension15, never installed under the closed14 profile.
pub(in crate::audit) const SCHEMA: [(&str, &str, &str); 5] = [
    (
        "table",
        "egress_policy_meta",
        "CREATE TABLE egress_policy_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1),genesis_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),genesis_hash TEXT NOT NULL,limits_json TEXT NOT NULL)",
    ),
    (
        "table",
        "egress_policy_revisions",
        "CREATE TABLE egress_policy_revisions(scope_kind TEXT NOT NULL CHECK(scope_kind IN ('tenant','session')),scope_id TEXT NOT NULL CHECK((scope_kind='tenant' AND scope_id='') OR (scope_kind='session' AND length(scope_id)=36)),revision INTEGER NOT NULL CHECK(revision>0),policy_json TEXT NOT NULL,policy_digest TEXT NOT NULL,event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),PRIMARY KEY(scope_kind,scope_id,revision))",
    ),
    (
        "table",
        "egress_policy_heads",
        "CREATE TABLE egress_policy_heads(scope_kind TEXT NOT NULL,scope_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),epoch INTEGER NOT NULL CHECK(epoch>0),state TEXT NOT NULL CHECK(state IN ('active','revoke_requested')),event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),PRIMARY KEY(scope_kind,scope_id),FOREIGN KEY(scope_kind,scope_id,revision) REFERENCES egress_policy_revisions(scope_kind,scope_id,revision))",
    ),
    (
        "table",
        "egress_policy_operations",
        "CREATE TABLE egress_policy_operations(operation_id TEXT PRIMARY KEY,action TEXT NOT NULL CHECK(action IN ('replace','revoke')),input_hash TEXT NOT NULL,receipt_json TEXT NOT NULL,event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
    (
        "table",
        "egress_policy_reservations",
        "CREATE TABLE egress_policy_reservations(scope_kind TEXT NOT NULL,scope_id TEXT NOT NULL,events INTEGER NOT NULL CHECK(events=1),bytes INTEGER NOT NULL CHECK(bytes=98304),PRIMARY KEY(scope_kind,scope_id),FOREIGN KEY(scope_kind,scope_id) REFERENCES egress_policy_heads(scope_kind,scope_id) DEFERRABLE INITIALLY DEFERRED)",
    ),
];
pub(in crate::audit) const AUTO_INDEX_COUNTS: [(&str, u32); 5] = [
    ("egress_policy_meta", 1),
    ("egress_policy_revisions", 2),
    ("egress_policy_heads", 2),
    ("egress_policy_operations", 2),
    ("egress_policy_reservations", 1),
];
