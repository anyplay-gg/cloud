-- The schema of migrations/sqlite/0017_feed.sql for PostgreSQL.
ALTER TABLE privacy ADD COLUMN feed TEXT NOT NULL DEFAULT 'friends';

CREATE TABLE feed_items (
    id            BIGSERIAL PRIMARY KEY,
    account_id    BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    kind          TEXT   NOT NULL,
    title_id      TEXT,
    trophy_number BIGINT,
    grade         TEXT,
    other_account BIGINT REFERENCES accounts (id) ON DELETE CASCADE,
    members       TEXT,
    created       BIGINT NOT NULL
);

CREATE INDEX feed_items_account ON feed_items (account_id, id);
CREATE INDEX feed_items_created ON feed_items (created);

CREATE TABLE feed_members (
    item_id    BIGINT NOT NULL REFERENCES feed_items (id) ON DELETE CASCADE,
    account_id BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    PRIMARY KEY (item_id, account_id)
);

CREATE INDEX feed_members_account ON feed_members (account_id, item_id);

CREATE TABLE feed_reactions (
    item_id    BIGINT NOT NULL REFERENCES feed_items (id) ON DELETE CASCADE,
    account_id BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    reaction   TEXT   NOT NULL,
    created    BIGINT NOT NULL,
    PRIMARY KEY (item_id, account_id)
);

CREATE INDEX feed_reactions_account ON feed_reactions (account_id);

CREATE TABLE feed_firsts (
    account_id BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    title_id   TEXT   NOT NULL,
    seen       BIGINT NOT NULL,
    PRIMARY KEY (account_id, title_id)
);

INSERT INTO feed_firsts (account_id, title_id, seen)
SELECT account_id, title_id, 0 FROM trophies
UNION
SELECT account_id, title_id, 0 FROM playtime;
