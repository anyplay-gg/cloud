-- The schema of migrations/sqlite/0007_parties_messages.sql for PostgreSQL.

CREATE TABLE parties (
    id       TEXT   PRIMARY KEY,
    leader   BIGINT NOT NULL,
    created  BIGINT NOT NULL,
    revision BIGINT NOT NULL
);

CREATE TABLE party_members (
    party_id   TEXT   NOT NULL REFERENCES parties (id) ON DELETE CASCADE,
    account_id BIGINT NOT NULL UNIQUE REFERENCES accounts (id) ON DELETE CASCADE,
    seq        BIGINT NOT NULL,
    handle     TEXT   NOT NULL,
    joined     BIGINT NOT NULL,
    PRIMARY KEY (party_id, account_id)
);

CREATE TABLE party_invitations (
    id          TEXT   PRIMARY KEY,
    party_id    TEXT   NOT NULL REFERENCES parties (id) ON DELETE CASCADE,
    from_id     BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    from_handle TEXT   NOT NULL,
    to_id       BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    expires     BIGINT NOT NULL,
    deadline_ms BIGINT NOT NULL,
    UNIQUE (party_id, to_id)
);

CREATE INDEX party_invitations_to ON party_invitations (to_id);
CREATE INDEX party_invitations_from ON party_invitations (from_id);
CREATE INDEX party_invitations_deadline ON party_invitations (deadline_ms);

CREATE TABLE conversations (
    id         TEXT   PRIMARY KEY,
    kind       TEXT   NOT NULL,
    name       TEXT,
    party_id   TEXT   UNIQUE REFERENCES parties (id) ON DELETE CASCADE,
    pair_low   BIGINT,
    pair_high  BIGINT,
    last_seq   BIGINT NOT NULL,
    created    BIGINT NOT NULL,
    updated_ms BIGINT NOT NULL,
    UNIQUE (pair_low, pair_high)
);

CREATE INDEX conversations_updated ON conversations (updated_ms);

CREATE TABLE conversation_members (
    conversation_id TEXT   NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    account_id      BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    state           BIGINT NOT NULL,
    from_seq        BIGINT NOT NULL,
    read_seq        BIGINT NOT NULL,
    joined          BIGINT NOT NULL,
    PRIMARY KEY (conversation_id, account_id)
);

CREATE INDEX conversation_members_account ON conversation_members (account_id);

CREATE TABLE messages (
    conversation_id TEXT   NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    seq             BIGINT NOT NULL,
    sender          BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    body            TEXT   NOT NULL,
    created_ms      BIGINT NOT NULL,
    PRIMARY KEY (conversation_id, seq)
);

CREATE INDEX messages_created ON messages (created_ms);
CREATE INDEX messages_sender ON messages (sender);

CREATE TABLE message_hidden (
    conversation_id TEXT   NOT NULL,
    seq             BIGINT NOT NULL,
    account_id      BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    PRIMARY KEY (conversation_id, seq, account_id),
    FOREIGN KEY (conversation_id, seq) REFERENCES messages (conversation_id, seq) ON DELETE CASCADE
);
