-- The schema of migrations/sqlite/0026_media.sql for PostgreSQL.
CREATE TABLE media (
    id             TEXT   PRIMARY KEY,
    conversation_id TEXT  NOT NULL,
    seq            BIGINT NOT NULL,
    uploader_id    BIGINT NOT NULL,
    kind           TEXT   NOT NULL,
    content_type   TEXT   NOT NULL,
    bytes          BIGINT NOT NULL,
    width          BIGINT,
    height         BIGINT,
    duration_ms    BIGINT,
    created_ms     BIGINT NOT NULL,
    expires_ms     BIGINT NOT NULL,
    removed_ms     BIGINT,
    hold_until_ms  BIGINT
);
CREATE UNIQUE INDEX media_message ON media (conversation_id, seq);
CREATE INDEX media_uploader ON media (uploader_id, expires_ms);
CREATE INDEX media_expires ON media (expires_ms);
