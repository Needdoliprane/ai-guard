-- Empty egress projections for MySQL owned format 15 on a fresh target.
CREATE TABLE egress_policy_meta (
    singleton TINYINT UNSIGNED NOT NULL PRIMARY KEY,
    version TINYINT UNSIGNED NOT NULL,
    genesis_seq BIGINT NOT NULL,
    genesis_hash BINARY(64) NOT NULL,
    limits_json MEDIUMBLOB NOT NULL,
    UNIQUE KEY egress_meta_genesis_key (genesis_seq),
    CONSTRAINT egress_meta_singleton CHECK (singleton = 1),
    CONSTRAINT egress_meta_version CHECK (version = 1),
    CONSTRAINT egress_meta_genesis_ref FOREIGN KEY (genesis_seq)
        REFERENCES ai_guard_audit_event (seq) ON DELETE RESTRICT ON UPDATE RESTRICT
) ENGINE=InnoDB;

CREATE TABLE egress_policy_revisions (
    scope_kind VARBINARY(7) NOT NULL,
    scope_id VARBINARY(36) NOT NULL,
    revision BIGINT NOT NULL,
    policy_json MEDIUMBLOB NOT NULL,
    policy_digest BINARY(64) NOT NULL,
    event_seq BIGINT NOT NULL,
    PRIMARY KEY (scope_kind, scope_id, revision),
    UNIQUE KEY egress_revisions_event_key (event_seq),
    CONSTRAINT egress_revisions_revision CHECK (revision > 0),
    -- Binary literals for tenant and session keep the selector byte-exact.
    CONSTRAINT egress_revisions_scope_kind CHECK (scope_kind IN (X'74656e616e74', X'73657373696f6e')),
    CONSTRAINT egress_revisions_scope_id CHECK (
        (scope_kind = X'74656e616e74' AND OCTET_LENGTH(scope_id) = 0)
        OR (scope_kind = X'73657373696f6e' AND OCTET_LENGTH(scope_id) = 36)
    ),
    CONSTRAINT egress_revisions_event_ref FOREIGN KEY (event_seq)
        REFERENCES ai_guard_audit_event (seq) ON DELETE RESTRICT ON UPDATE RESTRICT
) ENGINE=InnoDB;

CREATE TABLE egress_policy_heads (
    scope_kind VARBINARY(7) NOT NULL,
    scope_id VARBINARY(36) NOT NULL,
    revision BIGINT NOT NULL,
    epoch BIGINT NOT NULL,
    state VARBINARY(16) NOT NULL,
    event_seq BIGINT NOT NULL,
    PRIMARY KEY (scope_kind, scope_id),
    UNIQUE KEY egress_heads_event_key (event_seq),
    KEY egress_heads_revision_key (scope_kind, scope_id, revision),
    CONSTRAINT egress_heads_revision CHECK (revision > 0),
    CONSTRAINT egress_heads_epoch CHECK (epoch > 0),
    -- active or revoke_requested.
    CONSTRAINT egress_heads_state CHECK (state IN (X'616374697665', X'7265766f6b655f726571756573746564')),
    CONSTRAINT egress_heads_event_ref FOREIGN KEY (event_seq)
        REFERENCES ai_guard_audit_event (seq) ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT egress_heads_revision_ref FOREIGN KEY (scope_kind, scope_id, revision)
        REFERENCES egress_policy_revisions (scope_kind, scope_id, revision)
        ON DELETE RESTRICT ON UPDATE RESTRICT
) ENGINE=InnoDB;

CREATE TABLE egress_policy_operations (
    operation_id VARBINARY(36) NOT NULL PRIMARY KEY,
    action VARBINARY(7) NOT NULL,
    input_hash BINARY(64) NOT NULL,
    receipt_json MEDIUMBLOB NOT NULL,
    event_seq BIGINT NOT NULL,
    UNIQUE KEY egress_operations_event_key (event_seq),
    -- replace or revoke.
    CONSTRAINT egress_operations_action CHECK (action IN (X'7265706c616365', X'7265766f6b65')),
    CONSTRAINT egress_operations_event_ref FOREIGN KEY (event_seq)
        REFERENCES ai_guard_audit_event (seq) ON DELETE RESTRICT ON UPDATE RESTRICT
) ENGINE=InnoDB;

CREATE TABLE egress_policy_reservations (
    scope_kind VARBINARY(7) NOT NULL,
    scope_id VARBINARY(36) NOT NULL,
    events TINYINT UNSIGNED NOT NULL,
    bytes INT UNSIGNED NOT NULL,
    PRIMARY KEY (scope_kind, scope_id),
    CONSTRAINT egress_reservations_events CHECK (events = 1),
    CONSTRAINT egress_reservations_bytes CHECK (bytes = 98304),
    CONSTRAINT egress_reservations_head_ref FOREIGN KEY (scope_kind, scope_id)
        REFERENCES egress_policy_heads (scope_kind, scope_id)
        ON DELETE RESTRICT ON UPDATE RESTRICT
) ENGINE=InnoDB;
