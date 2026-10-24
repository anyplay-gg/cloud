-- Linked accounts, shown in the portal as Connections (network spec/portal-api.md, "Connections"):
-- the account of a provider (Discord, Steam) that a player linked to their AnyPlay account. The
-- table holds an identity and never a token. One row per provider and account, and an account of
-- a provider belongs to one AnyPlay account.
CREATE TABLE linked_accounts (
    account_id   INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    provider     TEXT    NOT NULL,
    -- The provider's ID of the account: a Discord user ID or a SteamID64, as text.
    external_id  TEXT    NOT NULL,
    display_name TEXT,
    -- A Discord avatar hash, or the address of a Steam picture. Null when there is none.
    avatar       TEXT,
    -- Who sees the connection on the profile: `friends`, `everyone` or `nobody`.
    audience     TEXT    NOT NULL DEFAULT 'friends',
    -- Unix seconds.
    linked_at    INTEGER NOT NULL,
    PRIMARY KEY (account_id, provider),
    UNIQUE (provider, external_id)
);
