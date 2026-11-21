-- Apply after audit-v1.sql and decisions-v1.sql in the operator's selected database.
-- Admission records are durable reservations only; this schema does not imply a runtime effect.
CREATE TABLE ai_guard_worker (
    worker_id BINARY(36) NOT NULL PRIMARY KEY,
    operation_id BINARY(36) NOT NULL UNIQUE,
    request_id BINARY(36) NOT NULL UNIQUE,
    session_id BINARY(36) NOT NULL UNIQUE,
    state VARBINARY(24) NOT NULL,
    quota_held TINYINT UNSIGNED NOT NULL,
    cpu_millis INT UNSIGNED NOT NULL,
    memory_bytes BIGINT UNSIGNED NOT NULL,
    revision BIGINT UNSIGNED NOT NULL,
    snapshot MEDIUMBLOB NOT NULL,
    snapshot_hash BINARY(64) NOT NULL,
    event_seq BIGINT NOT NULL UNIQUE,
    CONSTRAINT ai_guard_worker_state_size CHECK (OCTET_LENGTH(state) BETWEEN 1 AND 24),
    CONSTRAINT ai_guard_worker_quota_held CHECK (quota_held IN (0, 1)),
    CONSTRAINT ai_guard_worker_cpu CHECK (cpu_millis BETWEEN 1 AND 200000),
    CONSTRAINT ai_guard_worker_memory CHECK (memory_bytes BETWEEN 1 AND 107374182400),
    CONSTRAINT ai_guard_worker_revision CHECK (revision >= 1),
    CONSTRAINT ai_guard_worker_snapshot_size CHECK (OCTET_LENGTH(snapshot) BETWEEN 1 AND 131072),
    CONSTRAINT ai_guard_worker_snapshot_hash CHECK (LOCATE(X'00', snapshot_hash) = 0),
    CONSTRAINT ai_guard_worker_event_ref
        FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event (seq)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_worker_attempt (
    attempt_id BINARY(36) NOT NULL PRIMARY KEY,
    worker_id BINARY(36) NOT NULL UNIQUE,
    authority_id VARBINARY(256) NOT NULL,
    stable_name VARBINARY(96) NOT NULL UNIQUE,
    volume_name VARBINARY(96) NOT NULL UNIQUE,
    snapshot MEDIUMBLOB NOT NULL,
    CONSTRAINT ai_guard_worker_attempt_authority_size
        CHECK (OCTET_LENGTH(authority_id) BETWEEN 1 AND 256),
    CONSTRAINT ai_guard_worker_attempt_stable_name_size
        CHECK (OCTET_LENGTH(stable_name) BETWEEN 1 AND 96),
    CONSTRAINT ai_guard_worker_attempt_volume_name_size
        CHECK (OCTET_LENGTH(volume_name) BETWEEN 1 AND 96),
    CONSTRAINT ai_guard_worker_attempt_snapshot_size CHECK (OCTET_LENGTH(snapshot) BETWEEN 1 AND 131072),
    CONSTRAINT ai_guard_worker_attempt_worker_ref
        FOREIGN KEY (worker_id) REFERENCES ai_guard_worker (worker_id)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_worker_mutation (
    operation_id BINARY(36) NOT NULL PRIMARY KEY,
    worker_id BINARY(36) NULL,
    action VARBINARY(32) NOT NULL,
    input_hash BINARY(64) NOT NULL,
    event_seq BIGINT NOT NULL UNIQUE,
    CONSTRAINT ai_guard_worker_mutation_action_size CHECK (OCTET_LENGTH(action) BETWEEN 1 AND 32),
    CONSTRAINT ai_guard_worker_mutation_input_hash CHECK (LOCATE(X'00', input_hash) = 0),
    CONSTRAINT ai_guard_worker_mutation_worker_ref
        FOREIGN KEY (worker_id) REFERENCES ai_guard_worker (worker_id),
    CONSTRAINT ai_guard_worker_mutation_event_ref
        FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event (seq)
) ENGINE=InnoDB;
