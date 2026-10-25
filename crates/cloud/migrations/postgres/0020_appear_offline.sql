-- The schema of migrations/sqlite/0020_appear_offline.sql for PostgreSQL.
ALTER TABLE accounts ADD COLUMN appear_offline BIGINT NOT NULL DEFAULT 0;
