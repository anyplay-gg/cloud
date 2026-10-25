-- The schema of migrations/sqlite/0024_message_changes.sql for PostgreSQL.
ALTER TABLE conversations ADD COLUMN rev BIGINT NOT NULL DEFAULT 0;
ALTER TABLE messages ADD COLUMN reply_seq BIGINT;
ALTER TABLE messages ADD COLUMN edited_ms BIGINT;
ALTER TABLE messages ADD COLUMN deleted_ms BIGINT;
ALTER TABLE messages ADD COLUMN rev BIGINT NOT NULL DEFAULT 0;
CREATE INDEX messages_rev ON messages (conversation_id, rev);

ALTER TABLE conversation_members ADD COLUMN delivered_seq BIGINT NOT NULL DEFAULT 0;
ALTER TABLE conversation_members ADD COLUMN muted BIGINT NOT NULL DEFAULT 0;
ALTER TABLE conversation_members ADD COLUMN pinned BIGINT NOT NULL DEFAULT 0;

CREATE TABLE message_reactions (
    conversation_id TEXT   NOT NULL,
    seq             BIGINT NOT NULL,
    account_id      BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    reaction        TEXT   NOT NULL,
    created_ms      BIGINT NOT NULL,
    PRIMARY KEY (conversation_id, seq, account_id),
    FOREIGN KEY (conversation_id, seq) REFERENCES messages (conversation_id, seq) ON DELETE CASCADE
);

CREATE TABLE message_holds (
    conversation_id TEXT   NOT NULL,
    seq             BIGINT NOT NULL,
    body            TEXT   NOT NULL,
    held_ms         BIGINT NOT NULL,
    PRIMARY KEY (conversation_id, seq, held_ms),
    FOREIGN KEY (conversation_id, seq) REFERENCES messages (conversation_id, seq) ON DELETE CASCADE
);
CREATE INDEX message_holds_held ON message_holds (held_ms);

CREATE INDEX message_reactions_account ON message_reactions (account_id);
