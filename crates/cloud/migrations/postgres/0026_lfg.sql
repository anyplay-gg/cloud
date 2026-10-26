-- The schema of migrations/sqlite/0025_lfg.sql for PostgreSQL.
CREATE TABLE lfg_listings (
    id          TEXT   PRIMARY KEY,
    poster_id   BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    title_id    TEXT   NOT NULL,
    mode        TEXT   NOT NULL,
    language    TEXT,
    note        TEXT   NOT NULL,
    slots       BIGINT NOT NULL,
    created_ms  BIGINT NOT NULL,
    expires_ms  BIGINT NOT NULL,
    closed_ms   BIGINT
);
CREATE INDEX lfg_listings_title ON lfg_listings (title_id, expires_ms);
CREATE INDEX lfg_listings_poster ON lfg_listings (poster_id);
CREATE INDEX lfg_listings_expires ON lfg_listings (expires_ms);

CREATE TABLE lfg_requests (
    listing_id  TEXT   NOT NULL REFERENCES lfg_listings (id) ON DELETE CASCADE,
    account_id  BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    created_ms  BIGINT NOT NULL,
    accepted_ms BIGINT,
    PRIMARY KEY (listing_id, account_id)
);
CREATE INDEX lfg_requests_account ON lfg_requests (account_id);
