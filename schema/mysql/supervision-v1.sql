-- Apply once by the operator after audit-v1.sql, decisions-v1.sql, workers-v1.sql, and worker-lifecycle-v1.sql.
-- This metadata projection is deliberately empty at schema preparation time.
-- The trusted maintenance bootstrap inserts its anchor after verifying the audit genesis.
-- The executing agent receives SELECT only on this projection.
CREATE TABLE ai_guard_supervision_meta (
    singleton TINYINT UNSIGNED NOT NULL PRIMARY KEY,
    protocol VARBINARY(32) NOT NULL,
    genesis_seq BIGINT NOT NULL UNIQUE,
    genesis_hash BINARY(64) NOT NULL,
    CONSTRAINT ai_guard_supervision_meta_singleton CHECK (singleton = 1),
    CONSTRAINT ai_guard_supervision_meta_protocol CHECK (
        OCTET_LENGTH(protocol) = 21
        AND protocol = _binary 'broker_supervision_v1'
    ),
    CONSTRAINT ai_guard_supervision_meta_genesis_seq CHECK (genesis_seq > 0),
    CONSTRAINT ai_guard_supervision_meta_genesis_hash CHECK (LOCATE(X'00', genesis_hash) = 0),
    CONSTRAINT ai_guard_supervision_meta_genesis_ref
        FOREIGN KEY (genesis_seq) REFERENCES ai_guard_audit_event (seq)
        ON DELETE RESTRICT ON UPDATE RESTRICT
) ENGINE=InnoDB;
