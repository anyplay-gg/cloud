-- The schema of migrations/sqlite/0009_party_endpoints.sql for PostgreSQL.
ALTER TABLE party_members ADD COLUMN endpoint TEXT NOT NULL DEFAULT '';
ALTER TABLE party_members ADD COLUMN relay TEXT;
ALTER TABLE party_members ADD COLUMN voice BIGINT NOT NULL DEFAULT 0;
