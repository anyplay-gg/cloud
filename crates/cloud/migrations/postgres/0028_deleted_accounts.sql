-- The schema of migrations/sqlite/0028_deleted_accounts.sql for PostgreSQL.
CREATE TABLE deleted_accounts (
    account_id BIGINT PRIMARY KEY,
    deleted_ms BIGINT NOT NULL
);
CREATE INDEX deleted_accounts_time ON deleted_accounts (deleted_ms);
