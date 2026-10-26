-- The schema of migrations/sqlite/0025_public_profile.sql for PostgreSQL.
ALTER TABLE accounts ADD COLUMN public_profile BIGINT NOT NULL DEFAULT 0;
