-- Messages that change in place, reactions, and the per-member settings of a conversation
-- (docs/adr/0030 in the network repository).
--
-- `conversations.rev` counts the edits, unsends and reactions of a conversation. A writer takes
-- the next one with an UPDATE of the conversation's row, under the same lock as `last_seq`, and
-- stamps the message with it (`messages.rev`), so a client asks for the messages with a `rev`
-- above the one it holds. `edited_ms` is the time of the last edit, `deleted_ms` the time the
-- writer unsent the message (its `body` is then empty), `reply_seq` the number of the message
-- it answers.
ALTER TABLE conversations ADD COLUMN rev INTEGER NOT NULL DEFAULT 0;
ALTER TABLE messages ADD COLUMN reply_seq INTEGER;
ALTER TABLE messages ADD COLUMN edited_ms INTEGER;
ALTER TABLE messages ADD COLUMN deleted_ms INTEGER;
ALTER TABLE messages ADD COLUMN rev INTEGER NOT NULL DEFAULT 0;
CREATE INDEX messages_rev ON messages (conversation_id, rev);

-- `delivered_seq` is how far a device of the member fetched the conversation (never below
-- `read_seq`); `muted` and `pinned` are the member's own settings.
ALTER TABLE conversation_members ADD COLUMN delivered_seq INTEGER NOT NULL DEFAULT 0;
ALTER TABLE conversation_members ADD COLUMN muted INTEGER NOT NULL DEFAULT 0;
ALTER TABLE conversation_members ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;

-- One reaction per account and message. It goes with the message (retention, unsend) and with
-- the account.
CREATE TABLE message_reactions (
    conversation_id TEXT    NOT NULL,
    seq             INTEGER NOT NULL,
    account_id      INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    reaction        TEXT    NOT NULL,
    created_ms      INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, seq, account_id),
    FOREIGN KEY (conversation_id, seq) REFERENCES messages (conversation_id, seq) ON DELETE CASCADE
) STRICT;

-- The text an unsend removed or an edit replaced, kept for seven days so that a report can still
-- be judged on what its reporter saw. Only a report reads it.
CREATE TABLE message_holds (
    conversation_id TEXT    NOT NULL,
    seq             INTEGER NOT NULL,
    body            TEXT    NOT NULL,
    held_ms         INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, seq, held_ms),
    FOREIGN KEY (conversation_id, seq) REFERENCES messages (conversation_id, seq) ON DELETE CASCADE
) STRICT;
CREATE INDEX message_holds_held ON message_holds (held_ms);

CREATE INDEX message_reactions_account ON message_reactions (account_id);
