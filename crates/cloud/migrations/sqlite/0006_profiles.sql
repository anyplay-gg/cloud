-- Profiles, avatars and playtime (network spec/cloud-api.md, "Profiles, avatars and playtime").
-- Each part of a profile has its own visibility next to `trophies`: `friends`, `everyone` or
-- `nobody`. Rows that exist already get the default, `friends`.
ALTER TABLE privacy ADD COLUMN about      TEXT NOT NULL DEFAULT 'friends';
ALTER TABLE privacy ADD COLUMN favourites TEXT NOT NULL DEFAULT 'friends';
ALTER TABLE privacy ADD COLUMN avatar     TEXT NOT NULL DEFAULT 'friends';
ALTER TABLE privacy ADD COLUMN playtime   TEXT NOT NULL DEFAULT 'friends';

-- The short text and the favourite titles (title IDs separated by commas). An account without a
-- row has neither.
CREATE TABLE profiles (
    account_id INTEGER PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    about      TEXT    NOT NULL,
    favourites TEXT    NOT NULL,
    updated    INTEGER NOT NULL
) STRICT;

-- The avatar: the finished 256 by 256 PNG, kept in the store so that nodes stay stateless. One per
-- account; a new one replaces the old. `hash` is the SHA-256 of `picture` in lowercase hex and is
-- the address the picture is served under.
CREATE TABLE avatars (
    account_id INTEGER PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    hash       TEXT    NOT NULL,
    picture    BLOB    NOT NULL,
    updated    INTEGER NOT NULL
) STRICT;

CREATE INDEX avatars_hash ON avatars (hash);

-- Minutes played per title and UTC day (`day` is `YYYY-MM-DD`). The highest number seen is kept.
CREATE TABLE playtime (
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    title_id   TEXT    NOT NULL,
    day        TEXT    NOT NULL,
    minutes    INTEGER NOT NULL,
    PRIMARY KEY (account_id, title_id, day)
) STRICT;
