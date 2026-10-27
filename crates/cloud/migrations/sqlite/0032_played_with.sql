-- Played with recently (network spec/cloud-api.md, "Played with recently", ADR 0038): the people an
-- account shared a party or a session with for a while. One row per direction, so that each
-- account's limit, switch and clearing touch its own rows only. `last_ms` is the last time they
-- were together for long enough; the title is that of the session, null after a party. Rows go
-- after 30 days, past 50 per account, and with either account.
CREATE TABLE played_with (
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    other_id   INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    last_ms    INTEGER NOT NULL,
    title_id   TEXT,
    title      TEXT,
    PRIMARY KEY (account_id, other_id),
    CHECK (account_id <> other_id)
) STRICT;
CREATE INDEX played_with_other ON played_with (other_id);
CREATE INDEX played_with_last ON played_with (last_ms);

-- 1 while the cloud records new entries for the account ("Remember people I play with").
ALTER TABLE accounts ADD COLUMN remember_played_with INTEGER NOT NULL DEFAULT 1;
-- When the player last cleared the list: people who were already together then are not put back.
ALTER TABLE accounts ADD COLUMN played_with_cleared_ms INTEGER NOT NULL DEFAULT 0;
