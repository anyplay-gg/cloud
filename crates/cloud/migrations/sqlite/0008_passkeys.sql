-- Device keys, a "not before" time per account, passkeys, the account web's browser sessions and
-- the one-time values around them, and the age and terms record
-- (network ADR 0016, docs/ARCHITECTURE.md "Security").

-- The public half of the key a device made at sign-in. A refresh token of a device that has one
-- is only accepted with a proof signed by it. Existing devices have none: until `legacy_until`
-- (30 days after this migration ran) they may refresh without a proof, and their first refresh
-- that carries a key and a proof binds them. After that date they must sign in again.
ALTER TABLE devices ADD COLUMN device_key BLOB;
ALTER TABLE devices ADD COLUMN legacy_until INTEGER;
UPDATE devices SET legacy_until = CAST(strftime('%s', 'now') AS INTEGER) + 2592000;

-- Access tokens issued before this time are refused ("sign out everywhere").
ALTER TABLE accounts ADD COLUMN tokens_valid_from INTEGER NOT NULL DEFAULT 0;

-- When the player confirmed being old enough, and the version of the terms they accepted with
-- the time. Null for accounts made before this was recorded.
ALTER TABLE accounts ADD COLUMN age_confirmed_at INTEGER;
ALTER TABLE accounts ADD COLUMN terms_version TEXT;
ALTER TABLE accounts ADD COLUMN terms_accepted_at INTEGER;

-- A passkey: the credential ID and the credential as webauthn-rs serialises it (public key,
-- signature counter, flags).
CREATE TABLE passkeys (
    id         BLOB    PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    credential TEXT    NOT NULL,
    created    INTEGER NOT NULL,
    last_used  INTEGER
) STRICT;

CREATE INDEX passkeys_account ON passkeys (account_id);

-- A browser's session on the account web. The cookie holds a random value; only its SHA-256 is
-- stored. Without an account it is the state of a sign-in in progress. `data` is JSON.
CREATE TABLE web_sessions (
    id_hash    BLOB    PRIMARY KEY,
    account_id INTEGER REFERENCES accounts (id) ON DELETE CASCADE,
    csrf       TEXT    NOT NULL,
    data       TEXT    NOT NULL,
    created_ms INTEGER NOT NULL,
    expires_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX web_sessions_account ON web_sessions (account_id);
CREATE INDEX web_sessions_expires ON web_sessions (expires_ms);

-- Values that are used once: passkey ceremonies in progress, codes the browser sign-in sends to
-- the loopback address, and links that open the account web. `kind` says which.
CREATE TABLE web_flows (
    id_hash    BLOB    PRIMARY KEY,
    kind       TEXT    NOT NULL,
    data       TEXT    NOT NULL,
    expires_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX web_flows_expires ON web_flows (expires_ms);

-- Nonces of proofs that were accepted, so that a proof is accepted once.
CREATE TABLE proof_nonces (
    hash       BLOB    PRIMARY KEY,
    expires_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX proof_nonces_expires ON proof_nonces (expires_ms);
