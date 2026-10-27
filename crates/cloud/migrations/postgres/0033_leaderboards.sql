-- The schema of migrations/sqlite/0033_leaderboards.sql for PostgreSQL.
CREATE TABLE boards (
    title_id   TEXT   NOT NULL,
    board      TEXT   NOT NULL,
    better     TEXT   NOT NULL DEFAULT 'high',
    status     TEXT   NOT NULL DEFAULT 'open',
    min_score  BIGINT NOT NULL,
    max_score  BIGINT NOT NULL,
    made_by    BIGINT REFERENCES accounts (id) ON DELETE SET NULL,
    created    BIGINT NOT NULL,
    PRIMARY KEY (title_id, board),
    CHECK (min_score <= max_score)
);
CREATE INDEX boards_made_by ON boards (made_by);

CREATE TABLE scores (
    title_id       TEXT   NOT NULL,
    board          TEXT   NOT NULL,
    account_id     BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    score          BIGINT NOT NULL,
    achieved       BIGINT NOT NULL,
    removed_at     BIGINT,
    removed_by     TEXT,
    removed_reason TEXT,
    PRIMARY KEY (title_id, board, account_id),
    FOREIGN KEY (title_id, board) REFERENCES boards (title_id, board) ON DELETE CASCADE
);
CREATE INDEX scores_account ON scores (account_id);
CREATE INDEX scores_by_score ON scores (title_id, board, score);

CREATE TABLE score_privacy (
    account_id BIGINT PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    audience   TEXT   NOT NULL DEFAULT 'friends'
);
