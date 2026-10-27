-- The schema of migrations/sqlite/0029_shares.sql for PostgreSQL.
CREATE TABLE shares (
    id             TEXT   PRIMARY KEY,
    invitation     TEXT   NOT NULL,
    host_id        BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    host_handle    TEXT   NOT NULL,
    guest_id       BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    guest_handle   TEXT   NOT NULL,
    title_id       TEXT   NOT NULL,
    title          TEXT   NOT NULL,
    mode           TEXT   NOT NULL,
    state          TEXT   NOT NULL,
    host_endpoint  TEXT   NOT NULL,
    host_relay     TEXT,
    guest_endpoint TEXT   NOT NULL DEFAULT '',
    guest_relay    TEXT,
    path           TEXT,
    created        BIGINT NOT NULL,
    revision       BIGINT NOT NULL DEFAULT 0,
    deadline_ms    BIGINT NOT NULL,
    relay_since_ms BIGINT
);

CREATE TABLE share_members (
    account_id BIGINT PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    share_id   TEXT   NOT NULL REFERENCES shares (id) ON DELETE CASCADE
);

CREATE INDEX share_members_share ON share_members (share_id);

CREATE TABLE share_relay_use (
    account_id BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    day        BIGINT NOT NULL,
    ms         BIGINT NOT NULL,
    PRIMARY KEY (account_id, day)
);
