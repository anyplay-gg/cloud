-- The schema of migrations/sqlite/0008_passkeys.sql for PostgreSQL: device keys, a "not before"
-- time per account, passkeys, the account web's browser sessions, one-time values and the age
-- and terms record (network ADR 0016).

ALTER TABLE devices ADD COLUMN device_key BYTEA;
ALTER TABLE devices ADD COLUMN legacy_until BIGINT;
UPDATE devices SET legacy_until = EXTRACT(EPOCH FROM now())::BIGINT + 2592000;

ALTER TABLE accounts ADD COLUMN tokens_valid_from BIGINT NOT NULL DEFAULT 0;

ALTER TABLE accounts ADD COLUMN age_confirmed_at BIGINT;
ALTER TABLE accounts ADD COLUMN terms_version TEXT;
ALTER TABLE accounts ADD COLUMN terms_accepted_at BIGINT;

CREATE TABLE passkeys (
    id         BYTEA  PRIMARY KEY,
    account_id BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    name       TEXT   NOT NULL,
    credential TEXT   NOT NULL,
    created    BIGINT NOT NULL,
    last_used  BIGINT
);

CREATE INDEX passkeys_account ON passkeys (account_id);

CREATE TABLE web_sessions (
    id_hash    BYTEA  PRIMARY KEY,
    account_id BIGINT REFERENCES accounts (id) ON DELETE CASCADE,
    csrf       TEXT   NOT NULL,
    data       TEXT   NOT NULL,
    created_ms BIGINT NOT NULL,
    expires_ms BIGINT NOT NULL
);

CREATE INDEX web_sessions_account ON web_sessions (account_id);
CREATE INDEX web_sessions_expires ON web_sessions (expires_ms);

CREATE TABLE web_flows (
    id_hash    BYTEA  PRIMARY KEY,
    kind       TEXT   NOT NULL,
    data       TEXT   NOT NULL,
    expires_ms BIGINT NOT NULL
);

CREATE INDEX web_flows_expires ON web_flows (expires_ms);

CREATE TABLE proof_nonces (
    hash       BYTEA  PRIMARY KEY,
    expires_ms BIGINT NOT NULL
);

CREATE INDEX proof_nonces_expires ON proof_nonces (expires_ms);
