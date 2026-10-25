-- The feed (network spec/cloud-api.md, "Feed"). Who sees an account's items is one more part of
-- its privacy, `friends` by default; rows that exist already get the default.
ALTER TABLE privacy ADD COLUMN feed TEXT NOT NULL DEFAULT 'friends';

-- One thing that happened. `kind` is `trophy`, `friend`, `first_played` or `party`; the columns
-- after it hold what the kind needs and are null otherwise. An item holds no name or text: a
-- title and a trophy are numbers. `members` (a party item) are account IDs separated by commas,
-- the leader first. `created` is Unix seconds.
CREATE TABLE feed_items (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id    INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    kind          TEXT    NOT NULL,
    title_id      TEXT,
    trophy_number INTEGER,
    grade         TEXT,
    other_account INTEGER REFERENCES accounts (id) ON DELETE CASCADE,
    members       TEXT,
    created       INTEGER NOT NULL
) STRICT;

-- A page of one account's items; the daily count; the retention sweep.
CREATE INDEX feed_items_account ON feed_items (account_id, id);
CREATE INDEX feed_items_created ON feed_items (created);

-- Who sees a party item: every member, whatever the leader chose.
CREATE TABLE feed_members (
    item_id    INTEGER NOT NULL REFERENCES feed_items (id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    PRIMARY KEY (item_id, account_id)
) STRICT;

CREATE INDEX feed_members_account ON feed_members (account_id, item_id);

-- At most one reaction per account and item: `nice`, `gg`, `wow` or `same`.
CREATE TABLE feed_reactions (
    item_id    INTEGER NOT NULL REFERENCES feed_items (id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    reaction   TEXT    NOT NULL,
    created    INTEGER NOT NULL,
    PRIMARY KEY (item_id, account_id)
) STRICT;

CREATE INDEX feed_reactions_account ON feed_reactions (account_id);

-- The titles an account has played, so that `first_played` is made once even after the retention
-- deleted the old items. Titles the account has trophies or playtime for count as played.
CREATE TABLE feed_firsts (
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    title_id   TEXT    NOT NULL,
    seen       INTEGER NOT NULL,
    PRIMARY KEY (account_id, title_id)
) STRICT;

INSERT INTO feed_firsts (account_id, title_id, seen)
SELECT account_id, title_id, 0 FROM trophies
UNION
SELECT account_id, title_id, 0 FROM playtime;
