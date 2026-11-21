-- Prepare the administrative broker registry on the operator-selected audit target.
-- Apply after audit-v1.sql. The operator supplies the bound genesis and projections.
CREATE TABLE ai_guard_broker_registry_meta (
    singleton TINYINT NOT NULL PRIMARY KEY CHECK (singleton = 1),
    genesis_seq BIGINT NOT NULL UNIQUE,
    genesis_hash BINARY(64) NOT NULL,
    FOREIGN KEY (genesis_seq) REFERENCES ai_guard_audit_event(seq)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_broker_registry_brokers (
    broker_id BINARY(36) NOT NULL PRIMARY KEY,
    grant_revision BIGINT NOT NULL CHECK (grant_revision > 0),
    heartbeat_revision BIGINT NOT NULL CHECK (heartbeat_revision >= 0),
    payload_json MEDIUMBLOB NOT NULL CHECK (OCTET_LENGTH(payload_json) BETWEEN 1 AND 131072),
    event_seq BIGINT NOT NULL,
    FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event(seq)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_broker_registry_authorities (
    authority_id VARBINARY(256) NOT NULL PRIMARY KEY,
    broker_id BINARY(36) NOT NULL,
    runtime_kind VARBINARY(10) NOT NULL,
    FOREIGN KEY (broker_id) REFERENCES ai_guard_broker_registry_brokers(broker_id)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_broker_registry_incarnations (
    incarnation_id BINARY(36) NOT NULL PRIMARY KEY,
    broker_id BINARY(36) NOT NULL,
    event_seq BIGINT NOT NULL UNIQUE,
    FOREIGN KEY (broker_id) REFERENCES ai_guard_broker_registry_brokers(broker_id),
    FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event(seq)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_broker_registry_operations (
    operation_id BINARY(36) NOT NULL PRIMARY KEY,
    input_hash BINARY(64) NOT NULL,
    event_seq BIGINT NOT NULL UNIQUE,
    broker_id BINARY(36) NOT NULL,
    result_json MEDIUMBLOB NOT NULL CHECK (OCTET_LENGTH(result_json) BETWEEN 1 AND 131072),
    KEY ai_guard_broker_registry_operations_prefix (broker_id,event_seq),
    FOREIGN KEY (broker_id) REFERENCES ai_guard_broker_registry_brokers(broker_id),
    FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event(seq)
) ENGINE=InnoDB;
