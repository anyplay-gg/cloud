-- Trophies a game keeps secret (spec/cloud-api.md, "Trophies and privacy"). A row marks one trophy
-- number of a title as secret for the account; until `revealed` is 1, only the account sees the
-- record. The mark can come before the record. No name or text: numbers only.
CREATE TABLE trophy_secrets (
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    title_id   TEXT    NOT NULL,
    number     INTEGER NOT NULL,
    revealed   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (account_id, title_id, number)
) STRICT;
