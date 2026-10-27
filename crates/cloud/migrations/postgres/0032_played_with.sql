-- The schema of migrations/sqlite/0032_played_with.sql for PostgreSQL.
CREATE TABLE played_with (
    account_id BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    other_id   BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    last_ms    BIGINT NOT NULL,
    title_id   TEXT,
    title      TEXT,
    PRIMARY KEY (account_id, other_id),
    CHECK (account_id <> other_id)
);
CREATE INDEX played_with_other ON played_with (other_id);
CREATE INDEX played_with_last ON played_with (last_ms);

ALTER TABLE accounts ADD COLUMN remember_played_with BIGINT NOT NULL DEFAULT 1;
ALTER TABLE accounts ADD COLUMN played_with_cleared_ms BIGINT NOT NULL DEFAULT 0;
