-- Parties, conversations and messages (network spec/cloud-api.md, "Parties" and "Conversations").
-- Times a sweep compares are Unix milliseconds (`*_ms`); times the API shows stay seconds.

-- A party: up to 8 friends. `revision` rises with every change, as a session's does.
CREATE TABLE parties (
    id       TEXT    PRIMARY KEY,
    leader   INTEGER NOT NULL,
    created  INTEGER NOT NULL,
    revision INTEGER NOT NULL
) STRICT;

-- The unique `account_id` is what lets an account be in one party at a time, whichever node
-- writes. `seq` is the order of joining: the lowest is the longest-standing member.
CREATE TABLE party_members (
    party_id   TEXT    NOT NULL REFERENCES parties (id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL UNIQUE REFERENCES accounts (id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    handle     TEXT    NOT NULL,
    joined     INTEGER NOT NULL,
    PRIMARY KEY (party_id, account_id)
) STRICT;

CREATE TABLE party_invitations (
    id          TEXT    PRIMARY KEY,
    party_id    TEXT    NOT NULL REFERENCES parties (id) ON DELETE CASCADE,
    from_id     INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    from_handle TEXT    NOT NULL,
    to_id       INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    expires     INTEGER NOT NULL,
    deadline_ms INTEGER NOT NULL,
    UNIQUE (party_id, to_id)
) STRICT;

CREATE INDEX party_invitations_to ON party_invitations (to_id);
CREATE INDEX party_invitations_from ON party_invitations (from_id);
CREATE INDEX party_invitations_deadline ON party_invitations (deadline_ms);

-- A conversation. `last_seq` is the number of its newest message; a writer takes the next one
-- with an UPDATE of this row inside the transaction that stores the message, so two writers on
-- any nodes never get the same number (docs/adr/0013 in the network repository).
-- `kind` is `direct`, `group` or `party`. A direct conversation names its two accounts in
-- `pair_low` and `pair_high`, so there is one per pair. The party's chat goes with the party.
CREATE TABLE conversations (
    id         TEXT    PRIMARY KEY,
    kind       TEXT    NOT NULL,
    name       TEXT,
    party_id   TEXT    UNIQUE REFERENCES parties (id) ON DELETE CASCADE,
    pair_low   INTEGER,
    pair_high  INTEGER,
    last_seq   INTEGER NOT NULL,
    created    INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL,
    UNIQUE (pair_low, pair_high)
) STRICT;

CREATE INDEX conversations_updated ON conversations (updated_ms);

-- `state`: 0 a request the account has not accepted, 1 accepted, 2 declined (nothing more is
-- shown to the account). `from_seq` is the newest message from before the account joined, which
-- it never sees. `read_seq` is how far it read.
CREATE TABLE conversation_members (
    conversation_id TEXT    NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    account_id      INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    state           INTEGER NOT NULL,
    from_seq        INTEGER NOT NULL,
    read_seq        INTEGER NOT NULL,
    joined          INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, account_id)
) STRICT;

CREATE INDEX conversation_members_account ON conversation_members (account_id);

CREATE TABLE messages (
    conversation_id TEXT    NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    seq             INTEGER NOT NULL,
    sender          INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    body            TEXT    NOT NULL,
    created_ms      INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, seq)
) STRICT;

CREATE INDEX messages_created ON messages (created_ms);
CREATE INDEX messages_sender ON messages (sender);

-- Messages a block kept from an account: it never gets them, now or after an unblock.
CREATE TABLE message_hidden (
    conversation_id TEXT    NOT NULL,
    seq             INTEGER NOT NULL,
    account_id      INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    PRIMARY KEY (conversation_id, seq, account_id),
    FOREIGN KEY (conversation_id, seq) REFERENCES messages (conversation_id, seq) ON DELETE CASCADE
) STRICT;
