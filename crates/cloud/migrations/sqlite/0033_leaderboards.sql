-- Leaderboards (network spec/cloud-api.md, "Leaderboards", ADR 0040). A board is a title and the
-- board ID the game passes; it is made by the first accepted score with the defaults, and a
-- moderator sets its real order, bounds and status. An entry is an account's best score on a
-- board. Nothing here is game content: no names, no units, no art.
CREATE TABLE boards (
    title_id   TEXT    NOT NULL,
    board      TEXT    NOT NULL,
    -- 'high' or 'low': which end of the scores is better.
    better     TEXT    NOT NULL DEFAULT 'high',
    -- 'open' or 'closed': a closed board takes no more scores and stays readable.
    status     TEXT    NOT NULL DEFAULT 'open',
    min_score  INTEGER NOT NULL,
    max_score  INTEGER NOT NULL,
    -- The account whose first score made the board; null when a moderator made it or the
    -- account is gone. Bounds the boards one account can make.
    made_by    INTEGER REFERENCES accounts (id) ON DELETE SET NULL,
    created    INTEGER NOT NULL,
    PRIMARY KEY (title_id, board),
    CHECK (min_score <= max_score)
) STRICT;
CREATE INDEX boards_made_by ON boards (made_by);

CREATE TABLE scores (
    title_id       TEXT    NOT NULL,
    board          TEXT    NOT NULL,
    account_id     INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    score          INTEGER NOT NULL,
    -- When the account reached the score, in Unix seconds.
    achieved       INTEGER NOT NULL,
    -- Set when a moderator removed the entry: it is in no view and the account cannot submit
    -- to the board again until it is restored.
    removed_at     INTEGER,
    removed_by     TEXT,
    removed_reason TEXT,
    PRIMARY KEY (title_id, board, account_id),
    FOREIGN KEY (title_id, board) REFERENCES boards (title_id, board) ON DELETE CASCADE
) STRICT;
CREATE INDEX scores_account ON scores (account_id);
CREATE INDEX scores_by_score ON scores (title_id, board, score);

-- Who sees the account's scores: 'friends' (the default, also for an account with no row),
-- 'everyone' or 'nobody'.
CREATE TABLE score_privacy (
    account_id INTEGER PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    audience   TEXT    NOT NULL DEFAULT 'friends'
) STRICT;
