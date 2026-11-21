-- Apply once to a fresh database selected by the operator.
CREATE TABLE ai_guard_audit_event (
    seq BIGINT NOT NULL PRIMARY KEY,
    event_id VARBINARY(64) NOT NULL UNIQUE,
    received_at VARBINARY(32) NOT NULL,
    prev_hash BINARY(64) NOT NULL,
    hash BINARY(64) NOT NULL,
    payload MEDIUMBLOB NOT NULL,
    CONSTRAINT ai_guard_audit_event_seq CHECK (seq > 0),
    CONSTRAINT ai_guard_audit_event_id_size CHECK (OCTET_LENGTH(event_id) BETWEEN 1 AND 64),
    CONSTRAINT ai_guard_audit_event_received_size CHECK (OCTET_LENGTH(received_at) BETWEEN 1 AND 32),
    CONSTRAINT ai_guard_audit_event_hash_size CHECK (
        LOCATE(X'00', prev_hash) = 0 AND LOCATE(X'00', hash) = 0
    ),
    CONSTRAINT ai_guard_audit_event_payload_size CHECK (OCTET_LENGTH(payload) BETWEEN 1 AND 131072)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_audit_state (
    singleton TINYINT UNSIGNED NOT NULL PRIMARY KEY,
    schema_version SMALLINT UNSIGNED NOT NULL,
    tenant_id VARBINARY(128) NULL,
    namespace_id VARBINARY(36) NULL,
    head_seq BIGINT NOT NULL,
    head_hash BINARY(64) NOT NULL,
    payload_bytes BIGINT UNSIGNED NOT NULL,
    CONSTRAINT ai_guard_audit_state_singleton CHECK (singleton = 1),
    CONSTRAINT ai_guard_audit_state_schema_version CHECK (schema_version = 1),
    CONSTRAINT ai_guard_audit_state_binding CHECK (
        (tenant_id IS NULL AND namespace_id IS NULL)
        OR (
            tenant_id IS NOT NULL AND namespace_id IS NOT NULL
            AND OCTET_LENGTH(tenant_id) BETWEEN 1 AND 128
            AND OCTET_LENGTH(namespace_id) = 36
        )
    ),
    CONSTRAINT ai_guard_audit_state_head_seq CHECK (head_seq >= 0),
    CONSTRAINT ai_guard_audit_state_head_hash_size CHECK (LOCATE(X'00', head_hash) = 0)
) ENGINE=InnoDB;

INSERT INTO ai_guard_audit_state (
    singleton, schema_version, tenant_id, namespace_id, head_seq, head_hash, payload_bytes
) VALUES (
    1, 1, NULL, NULL, 0, _binary '0000000000000000000000000000000000000000000000000000000000000000', 0
);
