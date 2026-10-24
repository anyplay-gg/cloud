-- The schema of migrations/sqlite/0010_portal.sql for PostgreSQL: a deletion with a grace period
-- and the time a browser session was last used.

ALTER TABLE accounts ADD COLUMN deletion_at BIGINT;

CREATE INDEX accounts_deletion ON accounts (deletion_at) WHERE deletion_at IS NOT NULL;

ALTER TABLE web_sessions ADD COLUMN used_ms BIGINT;
