-- The schema of migrations/sqlite/0023_session_kind.sql for PostgreSQL.
ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'game';
ALTER TABLE invitations ADD COLUMN kind TEXT NOT NULL DEFAULT 'game';
