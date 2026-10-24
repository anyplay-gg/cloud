-- The schema of migrations/sqlite/0002_shared_state.sql for PostgreSQL.
-- What every node of a cloud must see alike: sign-in challenges, event connections, presence,
-- sessions, invitations and virtual address leases (docs/adr/0005-shared-state-lives-in-the-store.md).
-- Times that a sweep compares are Unix milliseconds (`*_ms`); times the API shows stay seconds.

-- A sign-in code waiting to be entered. Keys and codes are stored as SHA-256 hashes.
CREATE TABLE challenges (
    id_hash    BYTEA   PRIMARY KEY,
    email      TEXT    NOT NULL,
    code_hash  BYTEA   NOT NULL,
    tries      BIGINT  NOT NULL,
    expires_ms BIGINT  NOT NULL
);

-- Permission to create an account with a verified address.
CREATE TABLE signups (
    id_hash    BYTEA   PRIMARY KEY,
    email      TEXT    NOT NULL,
    expires_ms BIGINT  NOT NULL
);

-- The nodes of the cloud and when each last showed it is alive.
CREATE TABLE nodes (
    id           TEXT    PRIMARY KEY,
    heartbeat_ms BIGINT  NOT NULL
);

-- One event connection per device, and the node that holds it.
CREATE TABLE connections (
    device_id  BIGINT  PRIMARY KEY REFERENCES devices (id) ON DELETE CASCADE,
    account_id BIGINT  NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    node       TEXT    NOT NULL,
    serial     BIGINT  NOT NULL,
    opened_ms  BIGINT  NOT NULL
);

CREATE INDEX connections_account ON connections (account_id);
CREATE INDEX connections_node ON connections (node);

-- What friends see of an account. `last_seen_ms` is when its last connection closed, or when it
-- joined a session without one; null while it holds a connection. `sent` is the presence its
-- friends were last told about, as JSON.
CREATE TABLE presence (
    account_id   BIGINT  PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    online       BIGINT  NOT NULL,
    activity     TEXT,
    last_seen_ms BIGINT,
    sent         TEXT    NOT NULL,
    revision     BIGINT  NOT NULL
);

CREATE TABLE sessions (
    id          TEXT    PRIMARY KEY,
    title_id    TEXT    NOT NULL,
    title       TEXT    NOT NULL,
    version     TEXT    NOT NULL,
    build       TEXT    NOT NULL,
    leader      BIGINT  NOT NULL,
    max_members BIGINT  NOT NULL,
    visibility  TEXT    NOT NULL,
    properties  TEXT    NOT NULL,
    created     BIGINT  NOT NULL,
    -- Rises with every change; touching it first orders concurrent changes.
    revision    BIGINT  NOT NULL
);

-- `seq` is the order of joining within the session: the lowest is the longest-standing member.
CREATE TABLE members (
    session_id TEXT    NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    account_id BIGINT  NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    seq        BIGINT  NOT NULL,
    handle     TEXT    NOT NULL,
    endpoint   TEXT    NOT NULL,
    relay      TEXT,
    address    TEXT    NOT NULL,
    joined     BIGINT  NOT NULL,
    PRIMARY KEY (session_id, account_id)
);

CREATE INDEX members_account ON members (account_id);

CREATE TABLE invitations (
    id          TEXT    PRIMARY KEY,
    from_id     BIGINT  NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    from_handle TEXT    NOT NULL,
    to_id       BIGINT  NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    session_id  TEXT    NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    title_id    TEXT    NOT NULL,
    title       TEXT    NOT NULL,
    version     TEXT    NOT NULL,
    expires     BIGINT  NOT NULL,
    deadline_ms BIGINT  NOT NULL
);

CREATE INDEX invitations_to ON invitations (to_id);
CREATE INDEX invitations_session ON invitations (session_id);
CREATE INDEX invitations_from ON invitations (from_id);

-- Virtual addresses: `slot` is the offset in 198.18.0.0/15. `expires_ms` is null while the
-- account holds a connection or is in a session.
CREATE TABLE leases (
    slot       BIGINT  PRIMARY KEY,
    account_id BIGINT  NOT NULL UNIQUE REFERENCES accounts (id) ON DELETE CASCADE,
    expires_ms BIGINT
);
