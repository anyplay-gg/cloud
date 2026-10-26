-- Screenshots and clips sent to a friend (network spec/cloud-api.md, "Media in messages").
--
-- One row per file. The file itself is in the media store under the key `id`, 128 random bits.
-- There is no foreign key to the message on purpose: a row is the only thing that knows its file
-- exists, so it must outlive the message (the 30-day message sweep) until the file is deleted.
-- `removed_ms` is set when the sender unsends the message; `hold_until_ms` when a report names
-- it, and the file then stays for moderation until that time. The sweep deletes the file, then
-- the row, of every row that is expired or removed and not held.
CREATE TABLE media (
    id             TEXT    PRIMARY KEY,
    conversation_id TEXT   NOT NULL,
    seq            INTEGER NOT NULL,
    uploader_id    INTEGER NOT NULL,
    kind           TEXT    NOT NULL,
    content_type   TEXT    NOT NULL,
    bytes          INTEGER NOT NULL,
    width          INTEGER,
    height         INTEGER,
    duration_ms    INTEGER,
    created_ms     INTEGER NOT NULL,
    expires_ms     INTEGER NOT NULL,
    removed_ms     INTEGER,
    hold_until_ms  INTEGER
) STRICT;
CREATE UNIQUE INDEX media_message ON media (conversation_id, seq);
CREATE INDEX media_uploader ON media (uploader_id, expires_ms);
CREATE INDEX media_expires ON media (expires_ms);
