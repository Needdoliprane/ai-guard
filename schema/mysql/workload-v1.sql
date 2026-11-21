-- Prepare the workload projection on the operator-selected, bound audit target.
-- Apply after the audit and worker schemas. The maintenance account prepares
-- the capability marker and corresponding journal and projection rows.
CREATE TABLE ai_guard_workload_meta (
    singleton TINYINT NOT NULL PRIMARY KEY CHECK (singleton = 1),
    genesis_seq BIGINT NOT NULL UNIQUE,
    genesis_hash BINARY(64) NOT NULL,
    FOREIGN KEY (genesis_seq) REFERENCES ai_guard_audit_event(seq)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_workload_bindings (
    credential_id BINARY(36) NOT NULL PRIMARY KEY,
    revision BIGINT NOT NULL CHECK (revision > 0),
    sha256_digest BINARY(64) NOT NULL UNIQUE,
    subject_key BINARY(64) NOT NULL,
    payload_json MEDIUMBLOB NOT NULL,
    event_seq BIGINT NOT NULL,
    KEY ai_guard_workload_bindings_subject (subject_key),
    FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event(seq)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_workload_binding_history (
    credential_id BINARY(36) NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    payload_json MEDIUMBLOB NOT NULL,
    event_seq BIGINT NOT NULL UNIQUE,
    PRIMARY KEY (credential_id, revision),
    KEY ai_guard_workload_binding_history_prefix (credential_id, event_seq),
    FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event(seq)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_workload_digests (
    sha256_digest BINARY(64) NOT NULL PRIMARY KEY,
    credential_id BINARY(36) NOT NULL,
    FOREIGN KEY (credential_id) REFERENCES ai_guard_workload_bindings(credential_id)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_workload_operations (
    operation_id BINARY(36) NOT NULL PRIMARY KEY,
    input_hash BINARY(64) NOT NULL,
    event_seq BIGINT NOT NULL UNIQUE,
    credential_id BINARY(36) NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event(seq)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_workload_worker_endpoints (
    worker_id BINARY(36) NOT NULL PRIMARY KEY,
    attempt_id BINARY(36) NOT NULL UNIQUE,
    generation BIGINT NOT NULL CHECK (generation > 0),
    event_seq BIGINT NOT NULL UNIQUE,
    FOREIGN KEY (worker_id) REFERENCES ai_guard_worker(worker_id),
    FOREIGN KEY (attempt_id) REFERENCES ai_guard_worker_attempt(attempt_id),
    FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event(seq)
) ENGINE=InnoDB;
