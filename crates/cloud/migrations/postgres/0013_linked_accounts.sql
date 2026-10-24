-- The schema of migrations/sqlite/0013_linked_accounts.sql for PostgreSQL: the account of a
-- provider (Discord, Steam) that a player linked to their AnyPlay account. An identity, never a
-- token.
CREATE TABLE linked_accounts (
    account_id   BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    provider     TEXT   NOT NULL,
    external_id  TEXT   NOT NULL,
    display_name TEXT,
    avatar       TEXT,
    audience     TEXT   NOT NULL DEFAULT 'friends',
    linked_at    BIGINT NOT NULL,
    PRIMARY KEY (account_id, provider),
    UNIQUE (provider, external_id)
);
