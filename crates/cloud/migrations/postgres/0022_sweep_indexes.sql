-- The schema of migrations/sqlite/0022_sweep_indexes.sql for PostgreSQL.
CREATE INDEX presence_due ON presence (last_seen_ms) WHERE online = 1 AND last_seen_ms IS NOT NULL;
