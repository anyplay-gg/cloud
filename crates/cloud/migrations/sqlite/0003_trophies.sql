-- Trophy records and what a player shows (spec/cloud-api.md, "Trophies and privacy"). A record
-- holds no name, text or picture: only the title, the trophy's number in the game, its grade and
-- when it was earned (Unix seconds). A trophy recorded keeps its first time and grade.
CREATE TABLE trophies (
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    title_id   TEXT    NOT NULL,
    number     INTEGER NOT NULL,
    grade      TEXT    NOT NULL,
    earned     INTEGER NOT NULL,
    PRIMARY KEY (account_id, title_id, number)
) STRICT;

-- Rarity counts the records of one title.
CREATE INDEX trophies_title ON trophies (title_id, number);

-- Who sees the account's trophies: `friends`, `everyone` or `nobody`. An account without a row
-- has the default, `friends`.
CREATE TABLE privacy (
    account_id INTEGER PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    trophies   TEXT    NOT NULL
) STRICT;
