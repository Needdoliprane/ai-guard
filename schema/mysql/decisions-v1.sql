-- Apply after audit-v1.sql in the operator's selected database.
CREATE TABLE ai_guard_audit_decision (
    request_id VARBINARY(64) NOT NULL,
    human_approved TINYINT UNSIGNED NOT NULL,
    request_hash BINARY(64) NOT NULL,
    event_seq BIGINT NOT NULL,
    PRIMARY KEY (request_id, human_approved),
    UNIQUE KEY ai_guard_audit_decision_event (event_seq),
    CONSTRAINT ai_guard_audit_decision_phase CHECK (human_approved IN (0, 1)),
    CONSTRAINT ai_guard_audit_decision_request_size
        CHECK (OCTET_LENGTH(request_id) BETWEEN 1 AND 64),
    CONSTRAINT ai_guard_audit_decision_event_ref
        FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event (seq)
) ENGINE=InnoDB;
