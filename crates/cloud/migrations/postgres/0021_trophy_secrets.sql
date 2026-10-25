-- The schema of migrations/sqlite/0021_trophy_secrets.sql for PostgreSQL.
CREATE TABLE trophy_secrets (
    account_id BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    title_id   TEXT   NOT NULL,
    number     BIGINT NOT NULL,
    revealed   BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (account_id, title_id, number)
);
