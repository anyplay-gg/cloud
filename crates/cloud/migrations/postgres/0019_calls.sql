-- The schema of migrations/sqlite/0019_calls.sql for PostgreSQL.
ALTER TABLE party_invitations ADD COLUMN call BIGINT NOT NULL DEFAULT 0;
ALTER TABLE parties ADD COLUMN discord TEXT;

CREATE TABLE missed_calls (
    account_id  BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    from_id     BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    from_handle TEXT   NOT NULL,
    at          BIGINT NOT NULL,
    PRIMARY KEY (account_id, from_id)
);

CREATE INDEX missed_calls_at ON missed_calls (at);
