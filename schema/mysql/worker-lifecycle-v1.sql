-- Apply once by the operator after audit-v1.sql, decisions-v1.sql, and workers-v1.sql.
-- This is additive preparation only: no runtime migration or movement of existing rows.
-- The application writer receives SELECT/INSERT only on these lifecycle records.
CREATE TABLE ai_guard_worker_observation (
    observation_id BINARY(36) NOT NULL PRIMARY KEY,
    worker_id BINARY(36) NOT NULL,
    attempt_id BINARY(36) NOT NULL,
    input_hash BINARY(64) NOT NULL,
    snapshot MEDIUMBLOB NOT NULL,
    event_seq BIGINT NOT NULL,
    UNIQUE KEY event_seq (event_seq),
    KEY ai_guard_worker_observation_attempt_idx (attempt_id),
    KEY ai_guard_worker_observation_worker_idx (worker_id),
    CONSTRAINT ai_guard_worker_observation_input_hash CHECK (LOCATE(X'00', input_hash) = 0),
    CONSTRAINT ai_guard_worker_observation_snapshot_size
        CHECK (OCTET_LENGTH(snapshot) BETWEEN 1 AND 131072),
    CONSTRAINT ai_guard_worker_observation_event_seq CHECK (event_seq > 0),
    CONSTRAINT ai_guard_worker_observation_worker_ref
        FOREIGN KEY (worker_id) REFERENCES ai_guard_worker (worker_id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT ai_guard_worker_observation_attempt_ref
        FOREIGN KEY (attempt_id) REFERENCES ai_guard_worker_attempt (attempt_id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT ai_guard_worker_observation_event_ref
        FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event (seq)
        ON DELETE RESTRICT ON UPDATE RESTRICT
) ENGINE=InnoDB;

CREATE TABLE ai_guard_worker_allocation (
    attempt_id BINARY(36) NOT NULL PRIMARY KEY,
    worker_id BINARY(36) NOT NULL,
    terminal_observation_id BINARY(36) NOT NULL,
    snapshot MEDIUMBLOB NOT NULL,
    event_seq BIGINT NOT NULL,
    UNIQUE KEY terminal_observation_id (terminal_observation_id),
    UNIQUE KEY event_seq (event_seq),
    KEY ai_guard_worker_allocation_worker_idx (worker_id),
    CONSTRAINT ai_guard_worker_allocation_snapshot_size
        CHECK (OCTET_LENGTH(snapshot) BETWEEN 1 AND 131072),
    CONSTRAINT ai_guard_worker_allocation_event_seq CHECK (event_seq > 0),
    CONSTRAINT ai_guard_worker_allocation_attempt_ref
        FOREIGN KEY (attempt_id) REFERENCES ai_guard_worker_attempt (attempt_id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT ai_guard_worker_allocation_worker_ref
        FOREIGN KEY (worker_id) REFERENCES ai_guard_worker (worker_id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT ai_guard_worker_allocation_observation_ref
        FOREIGN KEY (terminal_observation_id)
        REFERENCES ai_guard_worker_observation (observation_id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT ai_guard_worker_allocation_event_ref
        FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event (seq)
        ON DELETE RESTRICT ON UPDATE RESTRICT
) ENGINE=InnoDB;
