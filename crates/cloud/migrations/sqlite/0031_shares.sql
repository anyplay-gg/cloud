-- Share play (network spec/share-play.md, "Cloud API"): a host streams the running game to one
-- member of their party. The share lives while it is `invited` or `active`.

-- One row per share. `created` is Unix seconds; `deadline_ms` is when an invitation that is not
-- accepted ends; `relay_since_ms` is when the share began to be relayed (NULL while it is not).
CREATE TABLE shares (
    id             TEXT    PRIMARY KEY,
    invitation     TEXT    NOT NULL,
    host_id        INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    host_handle    TEXT    NOT NULL,
    guest_id       INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    guest_handle   TEXT    NOT NULL,
    title_id       TEXT    NOT NULL,
    title          TEXT    NOT NULL,
    mode           TEXT    NOT NULL,
    state          TEXT    NOT NULL,
    host_endpoint  TEXT    NOT NULL,
    host_relay     TEXT,
    guest_endpoint TEXT    NOT NULL DEFAULT '',
    guest_relay    TEXT,
    path           TEXT,
    created        INTEGER NOT NULL,
    revision       INTEGER NOT NULL DEFAULT 0,
    deadline_ms    INTEGER NOT NULL,
    relay_since_ms INTEGER
) STRICT;

-- The unique account is what keeps an account in one share at a time, as a party member is in
-- one party, whichever node got there first.
CREATE TABLE share_members (
    account_id INTEGER PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    share_id   TEXT    NOT NULL REFERENCES shares (id) ON DELETE CASCADE
) STRICT;

CREATE INDEX share_members_share ON share_members (share_id);

-- Milliseconds a host's shares were relayed, per UTC day, for the daily allowance.
CREATE TABLE share_relay_use (
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    day        INTEGER NOT NULL,
    ms         INTEGER NOT NULL,
    PRIMARY KEY (account_id, day)
) STRICT;
