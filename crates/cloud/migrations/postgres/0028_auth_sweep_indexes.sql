-- The schema of migrations/sqlite/0028_auth_sweep_indexes.sql for PostgreSQL.
CREATE INDEX challenges_expires ON challenges (expires_ms);
CREATE INDEX challenges_email ON challenges (email);
CREATE INDEX signups_expires ON signups (expires_ms);
CREATE INDEX invitations_deadline ON invitations (deadline_ms);
