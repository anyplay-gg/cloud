-- The schema of migrations/sqlite/0006_profiles.sql for PostgreSQL.
-- Profiles, avatars and playtime (network spec/cloud-api.md, "Profiles, avatars and playtime").
-- Each part of a profile has its own visibility next to `trophies`: `friends`, `everyone` or
-- `nobody`. Rows that exist already get the default, `friends`.
ALTER TABLE privacy ADD COLUMN about      TEXT NOT NULL DEFAULT 'friends';
ALTER TABLE privacy ADD COLUMN favourites TEXT NOT NULL DEFAULT 'friends';
ALTER TABLE privacy ADD COLUMN avatar     TEXT NOT NULL DEFAULT 'friends';
ALTER TABLE privacy ADD COLUMN playtime   TEXT NOT NULL DEFAULT 'friends';

CREATE TABLE profiles (
    account_id BIGINT PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    about      TEXT   NOT NULL,
    favourites TEXT   NOT NULL,
    updated    BIGINT NOT NULL
);

CREATE TABLE avatars (
    account_id BIGINT PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    hash       TEXT   NOT NULL,
    picture    BYTEA  NOT NULL,
    updated    BIGINT NOT NULL
);

CREATE INDEX avatars_hash ON avatars (hash);

CREATE TABLE playtime (
    account_id BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    title_id   TEXT   NOT NULL,
    day        TEXT   NOT NULL,
    minutes    BIGINT NOT NULL,
    PRIMARY KEY (account_id, title_id, day)
);
