-- Review projections shared by MySQL owned formats 14 and 15 on a fresh target.
CREATE TABLE review_authority_meta (
    singleton TINYINT UNSIGNED NOT NULL PRIMARY KEY,
    source_id VARBINARY(36) NOT NULL,
    anchor_seq BIGINT NOT NULL,
    anchor_hash BINARY(64) NOT NULL,
    incarnation_seq BIGINT NULL,
    incarnation_hash BINARY(64) NULL,
    incarnation_revision BIGINT NOT NULL,
    snapshot_json MEDIUMBLOB NOT NULL,
    snapshot_hash BINARY(64) NOT NULL,
    UNIQUE KEY review_authority_source_id (source_id),
    UNIQUE KEY review_authority_anchor_seq (anchor_seq),
    UNIQUE KEY review_authority_incarnation_seq (incarnation_seq),
    CONSTRAINT review_authority_singleton CHECK (singleton = 1),
    CONSTRAINT review_authority_incarnation_revision_nonnegative CHECK (incarnation_revision >= 0),
    CONSTRAINT review_authority_anchor_event
        FOREIGN KEY (anchor_seq) REFERENCES ai_guard_audit_event (seq)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT review_authority_incarnation_event
        FOREIGN KEY (incarnation_seq) REFERENCES ai_guard_audit_event (seq)
        ON DELETE RESTRICT ON UPDATE RESTRICT
) ENGINE=InnoDB;

CREATE TABLE runner_review_bindings (
    binding_id VARBINARY(36) NOT NULL PRIMARY KEY,
    credential_id VARBINARY(36) NOT NULL,
    tenant_id VARBINARY(128) NOT NULL,
    active TINYINT UNSIGNED NOT NULL,
    binding_revision BIGINT NOT NULL,
    policy_revision BIGINT NOT NULL,
    snapshot_json MEDIUMBLOB NOT NULL,
    snapshot_hash BINARY(64) NOT NULL,
    last_event_seq BIGINT NOT NULL,
    active_tenant_id VARBINARY(128) GENERATED ALWAYS AS (IF(active = 1, tenant_id, NULL)) STORED,
    UNIQUE KEY runner_review_bindings_last_event_seq (last_event_seq),
    UNIQUE KEY runner_review_bindings_active_credential (credential_id, active_tenant_id),
    UNIQUE KEY runner_review_bindings_active_tenant (active_tenant_id),
    CONSTRAINT review_binding_active_flag CHECK (active IN (0, 1)),
    CONSTRAINT review_binding_binding_revision_positive CHECK (binding_revision >= 1),
    CONSTRAINT review_binding_policy_revision_positive CHECK (policy_revision >= 1),
    CONSTRAINT review_binding_audit_event
        FOREIGN KEY (last_event_seq) REFERENCES ai_guard_audit_event (seq)
        ON DELETE RESTRICT ON UPDATE RESTRICT
) ENGINE=InnoDB;
