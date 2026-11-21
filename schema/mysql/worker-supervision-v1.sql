-- Operator-prepared append-only projection for supervised worker intent bindings.
-- Apply only to a fresh target after audit-v1.sql and workers-v1.sql.
-- No runtime DDL, ALTER, migration, or bootstrap row is part of this file.
CREATE TABLE ai_guard_worker_supervision (
    worker_id BINARY(36) NOT NULL PRIMARY KEY,
    intent_hash BINARY(64) NOT NULL UNIQUE,
    payload_json MEDIUMBLOB NOT NULL,
    event_seq BIGINT NOT NULL UNIQUE,
    CONSTRAINT ai_guard_worker_supervision_intent_hash
        CHECK (LOCATE(X'00', intent_hash) = 0),
    CONSTRAINT ai_guard_worker_supervision_payload_size
        CHECK (OCTET_LENGTH(payload_json) BETWEEN 1 AND 131072),
    CONSTRAINT ai_guard_worker_supervision_event_seq
        CHECK (event_seq > 0),
    CONSTRAINT ai_guard_worker_supervision_worker_ref
        FOREIGN KEY (worker_id) REFERENCES ai_guard_worker (worker_id),
    CONSTRAINT ai_guard_worker_supervision_event_ref
        FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event (seq)
) ENGINE=InnoDB;
