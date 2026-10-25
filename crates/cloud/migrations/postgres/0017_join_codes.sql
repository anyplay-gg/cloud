-- The schema of migrations/sqlite/0017_join_codes.sql for PostgreSQL: a hash of the join code an
-- account issued, never the code. At most one per account.
CREATE TABLE join_codes (
    account_id BIGINT PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    code_hash  BYTEA  NOT NULL UNIQUE,
    expires    BIGINT NOT NULL,
    created    BIGINT NOT NULL
);

CREATE INDEX join_codes_expires ON join_codes (expires);
