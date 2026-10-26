-- Looking for group (network spec/cloud-api.md, "Looking for group"): a short listing for a game
-- that lives for an hour, and the requests of other players to join it.
--
-- `closed_ms` is set when the poster closes the listing or it is full. A listing nobody can see
-- any more (closed or past `expires_ms`) is deleted a day after the later of the two by the
-- social sweep. Both tables go with the account.
CREATE TABLE lfg_listings (
    id          TEXT    PRIMARY KEY,
    poster_id   INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    title_id    TEXT    NOT NULL,
    mode        TEXT    NOT NULL,
    language    TEXT,
    note        TEXT    NOT NULL,
    slots       INTEGER NOT NULL,
    created_ms  INTEGER NOT NULL,
    expires_ms  INTEGER NOT NULL,
    closed_ms   INTEGER
) STRICT;
CREATE INDEX lfg_listings_title ON lfg_listings (title_id, expires_ms);
CREATE INDEX lfg_listings_poster ON lfg_listings (poster_id);
CREATE INDEX lfg_listings_expires ON lfg_listings (expires_ms);

CREATE TABLE lfg_requests (
    listing_id  TEXT    NOT NULL REFERENCES lfg_listings (id) ON DELETE CASCADE,
    account_id  INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    created_ms  INTEGER NOT NULL,
    accepted_ms INTEGER,
    PRIMARY KEY (listing_id, account_id)
) STRICT;
CREATE INDEX lfg_requests_account ON lfg_requests (account_id);
