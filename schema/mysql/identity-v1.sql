-- Prepare the optional identity projection on the operator-selected target.
-- The preparation must link the singleton and rows to existing audit events.
CREATE TABLE ai_guard_identity_meta (
    singleton TINYINT NOT NULL PRIMARY KEY CHECK (singleton = 1),
    authorization_revision BIGINT NOT NULL CHECK (authorization_revision >= 0),
    genesis_seq BIGINT NOT NULL UNIQUE,
    genesis_hash BINARY(64) NOT NULL,
    FOREIGN KEY (genesis_seq) REFERENCES ai_guard_audit_event(seq)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_identity_accounts (
    principal_key BINARY(64) NOT NULL PRIMARY KEY,
    payload_json MEDIUMBLOB NOT NULL,
    event_seq BIGINT NOT NULL,
    FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event(seq)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_identity_credentials (
    credential_id VARBINARY(36) NOT NULL PRIMARY KEY,
    principal_key BINARY(64) NOT NULL,
    digest BINARY(64) NOT NULL UNIQUE,
    active TINYINT NOT NULL CHECK (active IN (0,1)),
    payload_json MEDIUMBLOB NOT NULL,
    event_seq BIGINT NOT NULL,
    FOREIGN KEY (principal_key) REFERENCES ai_guard_identity_accounts(principal_key),
    FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event(seq)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_identity_memberships (
    membership_id VARBINARY(36) NOT NULL PRIMARY KEY,
    principal_key BINARY(64) NOT NULL,
    organization_scope VARBINARY(128) NOT NULL,
    active TINYINT NOT NULL CHECK (active IN (0,1)),
    payload_json MEDIUMBLOB NOT NULL,
    event_seq BIGINT NOT NULL,
    UNIQUE (principal_key, organization_scope),
    FOREIGN KEY (principal_key) REFERENCES ai_guard_identity_accounts(principal_key),
    FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event(seq)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_identity_resources (
    resource_key BINARY(64) NOT NULL PRIMARY KEY,
    resource_kind VARBINARY(16) NOT NULL,
    resource_id VARBINARY(256) NOT NULL,
    owner_key BINARY(64) NOT NULL,
    active TINYINT NOT NULL CHECK (active IN (0,1)),
    payload_json MEDIUMBLOB NOT NULL,
    event_seq BIGINT NOT NULL,
    UNIQUE (resource_kind, resource_id),
    FOREIGN KEY (owner_key) REFERENCES ai_guard_identity_accounts(principal_key),
    FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event(seq)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_identity_shares (
    resource_key BINARY(64) NOT NULL,
    principal_key BINARY(64) NOT NULL,
    permissions_json MEDIUMBLOB NOT NULL,
    PRIMARY KEY (resource_key, principal_key),
    FOREIGN KEY (resource_key) REFERENCES ai_guard_identity_resources(resource_key),
    FOREIGN KEY (principal_key) REFERENCES ai_guard_identity_accounts(principal_key)
) ENGINE=InnoDB;

CREATE TABLE ai_guard_identity_operations (
    operation_id VARBINARY(36) NOT NULL PRIMARY KEY,
    input_hash BINARY(64) NOT NULL,
    event_seq BIGINT NOT NULL UNIQUE,
    FOREIGN KEY (event_seq) REFERENCES ai_guard_audit_event(seq)
) ENGINE=InnoDB;
