-- The schema of migrations/sqlite/0016_run_reports.sql for PostgreSQL.
-- Run reports: a player's answer to "How did it run?" (network spec/cloud-api.md, "Run reports").
-- One row per account, title, game version and toolkit version; a new answer replaces the old one.
-- A row holds the verdict ("no_start", "menus", "in_game" or "playable"), the system ("windows",
-- "linux" or "macos") and when it was last changed (Unix seconds). Nothing else.
CREATE TABLE run_reports (
    account_id BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    title_id   TEXT   NOT NULL,
    version    TEXT   NOT NULL,
    toolkit    TEXT   NOT NULL,
    verdict    TEXT   NOT NULL,
    os         TEXT   NOT NULL,
    updated    BIGINT NOT NULL,
    PRIMARY KEY (account_id, title_id, version, toolkit)
);

-- The summary of a title, and of one version of it.
CREATE INDEX run_reports_title ON run_reports (title_id, version);
