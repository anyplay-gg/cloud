-- Durable data only. Presence, sessions, invitations, sign-in challenges and address leases
-- live in memory (docs/adr/0001-sqlite.md).

CREATE TABLE accounts (
    id         INTEGER PRIMARY KEY,
    handle     TEXT    NOT NULL,
    handle_key TEXT    NOT NULL UNIQUE,
    email      TEXT    NOT NULL UNIQUE,
    created    INTEGER NOT NULL
) STRICT;

CREATE TABLE devices (
    id            INTEGER PRIMARY KEY,
    account_id    INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    name          TEXT    NOT NULL,
    platform      TEXT    NOT NULL,
    refresh_hash  BLOB    NOT NULL UNIQUE,
    created       INTEGER NOT NULL,
    last_used     INTEGER NOT NULL
) STRICT;

CREATE INDEX devices_account ON devices (account_id);

-- One row per friendship, with the lower account ID in `low`.
CREATE TABLE friends (
    low   INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    high  INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    since INTEGER NOT NULL,
    PRIMARY KEY (low, high),
    CHECK (low < high)
) STRICT;

CREATE INDEX friends_high ON friends (high);

CREATE TABLE friend_requests (
    from_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    to_id   INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    since   INTEGER NOT NULL,
    PRIMARY KEY (from_id, to_id)
) STRICT;

CREATE INDEX friend_requests_to ON friend_requests (to_id);

CREATE TABLE blocks (
    blocker INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    blocked INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    since   INTEGER NOT NULL,
    PRIMARY KEY (blocker, blocked)
) STRICT;

CREATE INDEX blocks_blocked ON blocks (blocked);

CREATE TABLE reports (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    reporter INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    reported INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    reason   TEXT    NOT NULL,
    details  TEXT    NOT NULL,
    created  INTEGER NOT NULL
) STRICT;

CREATE INDEX reports_reporter ON reports (reporter, created);
CREATE INDEX reports_reported ON reports (reported);

-- A ban outlives its account: it is kept when the account is deleted, and the hash of the e-mail
-- address stops the same person from signing up again with it. `until` is null for good.
CREATE TABLE bans (
    account_id INTEGER PRIMARY KEY,
    email_hash BLOB    NOT NULL,
    until      INTEGER,
    reason     TEXT    NOT NULL,
    created    INTEGER NOT NULL
) STRICT;

CREATE INDEX bans_email ON bans (email_hash);

-- Refresh tokens that were replaced, so that one coming back is noticed. Only recent ones are
-- kept: an old token is useless anyway, and the table must not grow without bound.
CREATE TABLE used_refresh (
    hash      BLOB    PRIMARY KEY,
    device_id INTEGER NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    used      INTEGER NOT NULL
) STRICT;

CREATE INDEX used_refresh_device ON used_refresh (device_id, used);
