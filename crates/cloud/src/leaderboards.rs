//! Leaderboards: boards of best scores, named by a title and what the game passes (network
//! `spec/cloud-api.md`, "Leaderboards", ADR 0041). Written once with `$1` parameters for SQLite
//! and Postgres, like [`crate::played_with`].
//!
//! The module has four jobs behind a small interface. [`submit`] keeps an account's best score on
//! a board and makes the board on its first score; [`read`] answers a range of a board as one
//! viewer may see it (the one rule for who sees an entry is in [`visible`]); [`boards_of`] lists
//! the boards of a title; the moderator's calls ([`configure`], [`remove`], [`restore`]) are the
//! data that decides order, bounds and what stays. Nothing here knows a title: boards are rows.

use anyplay_api::{
    Account, AccountId, Audience, BOARDS_PER_ACCOUNT_MAKE, BOARDS_PER_TITLE_MAX, BoardChange,
    BoardEntry, BoardOrder, BoardSettings, BoardStatus, BoardSummary, BoardView, SCORE_MAX_DEFAULT,
    SCORE_MIN_DEFAULT, SCORES_PER_ACCOUNT_MAX,
};
use sqlx::FromRow;

use crate::db::{Pool, Writer, audience_name, audience_of};
use crate::live::Conn;

fn i(id: u64) -> i64 {
    id as i64
}

fn u(id: i64) -> u64 {
    id as u64
}

/// What the cloud knows of a board: how it ranks and what it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board {
    pub order: BoardOrder,
    pub status: BoardStatus,
    pub min_score: i64,
    pub max_score: i64,
}

impl Board {
    fn new() -> Self {
        Self {
            order: BoardOrder::High,
            status: BoardStatus::Open,
            min_score: SCORE_MIN_DEFAULT,
            max_score: SCORE_MAX_DEFAULT,
        }
    }

    fn takes(&self, score: i64) -> bool {
        (self.min_score..=self.max_score).contains(&score)
    }

    /// True when `score` ranks above `other` on this board.
    fn beats(&self, score: i64, other: i64) -> bool {
        match self.order {
            BoardOrder::High => score > other,
            BoardOrder::Low => score < other,
        }
    }
}

fn order_name(order: BoardOrder) -> &'static str {
    match order {
        BoardOrder::High => "high",
        BoardOrder::Low => "low",
    }
}

fn status_name(status: BoardStatus) -> &'static str {
    match status {
        BoardStatus::Open => "open",
        BoardStatus::Closed => "closed",
    }
}

#[derive(FromRow)]
struct BoardRow {
    better: String,
    status: String,
    min_score: i64,
    max_score: i64,
}

impl BoardRow {
    fn board(self) -> Board {
        Board {
            order: if self.better == "low" {
                BoardOrder::Low
            } else {
                BoardOrder::High
            },
            status: if self.status == "closed" {
                BoardStatus::Closed
            } else {
                BoardStatus::Open
            },
            min_score: self.min_score,
            max_score: self.max_score,
        }
    }
}

/// The board, or `None` when no score ever made it.
pub async fn board(conn: &mut Conn, title: &str, board: &str) -> sqlx::Result<Option<Board>> {
    let row: Option<BoardRow> = sqlx::query_as(
        "SELECT better, status, min_score, max_score FROM boards \
         WHERE title_id = $1 AND board = $2",
    )
    .bind(title)
    .bind(board)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(BoardRow::board))
}

// Submitting

/// Why a score was not kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The board takes no more scores.
    Closed,
    /// A moderator removed the account's entry and has not restored it.
    Removed,
    /// The score is outside the bounds of the board.
    OutOfRange,
    /// The title has the most boards it may have.
    TitleFull,
    /// The account made the most boards it may make.
    MakeLimit,
    /// The account keeps the most scores it may keep.
    ScoreLimit,
}

/// The account's best on a board after a submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Saved {
    pub accepted: bool,
    pub score: i64,
    pub achieved: u64,
}

async fn count_of_title(conn: &mut Conn, title: &str) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM boards WHERE title_id = $1")
        .bind(title)
        .fetch_one(conn)
        .await?;
    Ok(n)
}

async fn count_made_by(conn: &mut Conn, account: u64) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM boards WHERE made_by = $1")
        .bind(i(account))
        .fetch_one(conn)
        .await?;
    Ok(n)
}

async fn count_scores(conn: &mut Conn, account: u64) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scores WHERE account_id = $1")
        .bind(i(account))
        .fetch_one(conn)
        .await?;
    Ok(n)
}

/// Keeps `score` as the account's best on the board when it beats the one it has, and makes the
/// board with the defaults when it is the board's first. A closed board, a removed entry, a score
/// out of bounds and the limits refuse; nothing is written then.
pub async fn submit(
    writer: &Writer,
    pool: &Pool,
    account: u64,
    title: &str,
    board_id: &str,
    score: i64,
    now: u64,
) -> sqlx::Result<Result<Saved, Refusal>> {
    let mut tx = writer.begin(pool).await?;
    let board = match self::board(&mut tx, title, board_id).await? {
        Some(board) => board,
        None => {
            let fresh = Board::new();
            if !fresh.takes(score) {
                return Ok(Err(Refusal::OutOfRange));
            }
            if count_of_title(&mut tx, title).await? >= i64::from(BOARDS_PER_TITLE_MAX) {
                return Ok(Err(Refusal::TitleFull));
            }
            if count_made_by(&mut tx, account).await? >= i64::from(BOARDS_PER_ACCOUNT_MAKE) {
                return Ok(Err(Refusal::MakeLimit));
            }
            sqlx::query(
                "INSERT INTO boards (title_id, board, better, status, min_score, max_score, \
                                     made_by, created) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
                 ON CONFLICT (title_id, board) DO NOTHING",
            )
            .bind(title)
            .bind(board_id)
            .bind(order_name(fresh.order))
            .bind(status_name(fresh.status))
            .bind(fresh.min_score)
            .bind(fresh.max_score)
            .bind(i(account))
            .bind(i(now))
            .execute(&mut *tx)
            .await?;
            // Another account may have made it a moment ago, with other rules.
            self::board(&mut tx, title, board_id)
                .await?
                .unwrap_or(fresh)
        }
    };
    if board.status == BoardStatus::Closed {
        return Ok(Err(Refusal::Closed));
    }
    if !board.takes(score) {
        return Ok(Err(Refusal::OutOfRange));
    }
    let held: Option<(i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT score, achieved, removed_at FROM scores \
         WHERE title_id = $1 AND board = $2 AND account_id = $3",
    )
    .bind(title)
    .bind(board_id)
    .bind(i(account))
    .fetch_optional(&mut *tx)
    .await?;
    let saved = match held {
        Some((_, _, Some(_))) => return Ok(Err(Refusal::Removed)),
        Some((best, achieved, None)) if !board.beats(score, best) => Saved {
            accepted: false,
            score: best,
            achieved: u(achieved),
        },
        Some(_) => {
            sqlx::query(
                "UPDATE scores SET score = $4, achieved = $5 \
                 WHERE title_id = $1 AND board = $2 AND account_id = $3",
            )
            .bind(title)
            .bind(board_id)
            .bind(i(account))
            .bind(score)
            .bind(i(now))
            .execute(&mut *tx)
            .await?;
            Saved {
                accepted: true,
                score,
                achieved: now,
            }
        }
        None => {
            if count_scores(&mut tx, account).await? >= i64::from(SCORES_PER_ACCOUNT_MAX) {
                return Ok(Err(Refusal::ScoreLimit));
            }
            sqlx::query(
                "INSERT INTO scores (title_id, board, account_id, score, achieved) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(title)
            .bind(board_id)
            .bind(i(account))
            .bind(score)
            .bind(i(now))
            .execute(&mut *tx)
            .await?;
            Saved {
                accepted: true,
                score,
                achieved: now,
            }
        }
    };
    tx.commit().await?;
    Ok(Ok(saved))
}

// Reading

/// The condition under which the viewer sees the entry `s` (a row of `scores` joined with
/// `score_privacy` as `p`). `me` and `now` are the placeholders of the viewer's account ID (0 for
/// a visitor) and of the time in Unix seconds, so that a query can number its own parameters.
///
/// - The viewer sees their own entry.
/// - Anyone else's entry is hidden by a block in either direction, and by a ban.
/// - A friend's entry is seen unless its owner shows scores to nobody; a stranger's only if the
///   owner shows them to everyone. In the friends view only friends count, whatever they show.
fn visible(friends_only: bool, me: &str, now: &str) -> String {
    let me = format!("CAST({me} AS BIGINT)");
    let friend = format!(
        "EXISTS (SELECT 1 FROM friends f \
           WHERE f.low = CASE WHEN s.account_id < {me} THEN s.account_id ELSE {me} END \
             AND f.high = CASE WHEN s.account_id < {me} THEN {me} ELSE s.account_id END)"
    );
    let shown = if friends_only {
        format!("COALESCE(p.audience, 'friends') <> 'nobody' AND {friend}")
    } else {
        format!(
            "COALESCE(p.audience, 'friends') = 'everyone' \
             OR (COALESCE(p.audience, 'friends') = 'friends' AND {friend})"
        )
    };
    format!(
        "s.removed_at IS NULL \
         AND NOT EXISTS (SELECT 1 FROM bans b WHERE b.account_id = s.account_id \
                         AND (b.until IS NULL OR b.until > CAST({now} AS BIGINT))) \
         AND (s.account_id = {me} OR ( \
             NOT EXISTS (SELECT 1 FROM blocks k \
                         WHERE (k.blocker = {me} AND k.blocked = s.account_id) \
                            OR (k.blocker = s.account_id AND k.blocked = {me})) \
             AND ({shown})))"
    )
}

/// Every entry of a board the viewer sees, ranked: `place` is the rank (equal scores share one)
/// and `pos` the position from 1, with ties settled by the earlier time and then the account.
/// Parameters: `$1` title, `$2` board, `$3` viewer, `$4` now.
fn view_sql(order: BoardOrder, friends_only: bool) -> String {
    let dir = match order {
        BoardOrder::High => "DESC",
        BoardOrder::Low => "ASC",
    };
    format!(
        "SELECT s.account_id AS account_id, a.handle AS handle, s.score AS score, \
                s.achieved AS achieved, \
                RANK() OVER (ORDER BY s.score {dir}) AS place, \
                ROW_NUMBER() OVER (ORDER BY s.score {dir}, s.achieved ASC, s.account_id ASC) AS pos \
         FROM scores s \
         JOIN accounts a ON a.id = s.account_id \
         LEFT JOIN score_privacy p ON p.account_id = s.account_id \
         WHERE s.title_id = $1 AND s.board = $2 AND {}",
        visible(friends_only, "$3", "$4")
    )
}

#[derive(FromRow)]
struct Line {
    account_id: i64,
    handle: String,
    score: i64,
    achieved: i64,
    place: i64,
    pos: i64,
}

impl Line {
    fn entry(&self) -> BoardEntry {
        BoardEntry {
            rank: u32::try_from(self.place).unwrap_or(u32::MAX),
            account: Account {
                id: AccountId(u(self.account_id)),
                handle: self.handle.clone(),
            },
            score: self.score,
            achieved: u(self.achieved),
        }
    }
}

/// A range of a board as one viewer sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Range {
    /// The entries the viewer may see in this view.
    pub total: u32,
    /// The position of the first entry returned, from 0.
    pub start: u32,
    pub entries: Vec<BoardEntry>,
    /// The viewer's own line in this view.
    pub me: Option<BoardEntry>,
}

/// Reads `count` entries of `view` from position `start` (from 0); for [`BoardView::Around`]
/// `start` is ignored and the window is centred on the viewer. `viewer` is 0 for a visitor.
#[allow(clippy::too_many_arguments)]
pub async fn read(
    conn: &mut Conn,
    board: &Board,
    title: &str,
    board_id: &str,
    viewer: u64,
    view: BoardView,
    start: u32,
    count: u32,
    now: u64,
) -> sqlx::Result<Range> {
    let inner = view_sql(board.order, view == BoardView::Friends);
    let total: (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH v AS ({inner}) SELECT COUNT(*) FROM v"
    )))
    .bind(title)
    .bind(board_id)
    .bind(i(viewer))
    .bind(i(now))
    .fetch_one(&mut *conn)
    .await?;
    let total = u32::try_from(total.0).unwrap_or(u32::MAX);
    let mine: Option<Line> = if viewer == 0 {
        None
    } else {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "WITH v AS ({inner}) \
             SELECT account_id, handle, score, achieved, place, pos FROM v WHERE account_id = $3"
        )))
        .bind(title)
        .bind(board_id)
        .bind(i(viewer))
        .bind(i(now))
        .fetch_optional(&mut *conn)
        .await?
    };
    let me = mine.as_ref().map(Line::entry);
    let first = match view {
        BoardView::Around => match &mine {
            Some(line) => u64::try_from(line.pos - 1)
                .unwrap_or(0)
                .saturating_sub(u64::from(count / 2)),
            None => {
                return Ok(Range {
                    total,
                    start: 0,
                    entries: Vec::new(),
                    me,
                });
            }
        },
        BoardView::Top | BoardView::Friends => u64::from(start),
    };
    let lines: Vec<Line> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH v AS ({inner}) \
         SELECT account_id, handle, score, achieved, place, pos FROM v \
         WHERE pos > $5 AND pos <= $6 ORDER BY pos"
    )))
    .bind(title)
    .bind(board_id)
    .bind(i(viewer))
    .bind(i(now))
    .bind(first as i64)
    .bind(first as i64 + i64::from(count))
    .fetch_all(conn)
    .await?;
    Ok(Range {
        total,
        start: u32::try_from(first).unwrap_or(u32::MAX),
        entries: lines.iter().map(Line::entry).collect(),
        me,
    })
}

/// The boards of a title that have at least one entry the viewer may see.
pub async fn boards_of(
    conn: &mut Conn,
    title: &str,
    viewer: u64,
    now: u64,
) -> sqlx::Result<Vec<BoardSummary>> {
    let counted: Vec<(String, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT s.board, COUNT(*) FROM scores s \
         LEFT JOIN score_privacy p ON p.account_id = s.account_id \
         WHERE s.title_id = $1 AND {} GROUP BY s.board",
        visible(false, "$2", "$3")
    )))
    .bind(title)
    .bind(i(viewer))
    .bind(i(now))
    .fetch_all(&mut *conn)
    .await?;
    let rows: Vec<(String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT board, better, status, min_score, max_score FROM boards \
         WHERE title_id = $1 ORDER BY board",
    )
    .bind(title)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(board, better, status, min_score, max_score)| {
            let n = counted.iter().find(|(b, _)| *b == board)?.1;
            let kept = BoardRow {
                better,
                status,
                min_score,
                max_score,
            }
            .board();
            Some(BoardSummary {
                board,
                order: kept.order,
                status: kept.status,
                entries: u32::try_from(n).unwrap_or(u32::MAX),
            })
        })
        .collect())
}

// What the account shows

/// Who sees the account's scores; friends until the account says otherwise.
pub async fn audience(conn: &mut Conn, account: u64) -> sqlx::Result<Audience> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT audience FROM score_privacy WHERE account_id = $1")
            .bind(i(account))
            .fetch_optional(conn)
            .await?;
    Ok(row.map_or(Audience::Friends, |(name,)| audience_of(&name)))
}

pub async fn set_audience(conn: &mut Conn, account: u64, audience: Audience) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO score_privacy (account_id, audience) VALUES ($1, $2) \
         ON CONFLICT (account_id) DO UPDATE SET audience = $2",
    )
    .bind(i(account))
    .bind(audience_name(audience))
    .execute(conn)
    .await?;
    Ok(())
}

// What a moderator decides

/// Why a board could not be set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigRefusal {
    /// The lowest score is above the highest, or a bound is beyond what JSON reads exactly.
    Bounds,
    /// The title has the most boards it may have.
    TitleFull,
}

/// Sets what `change` names on the board, making the board (with the defaults) when there is
/// none. Stored scores are untouched; the order and the bounds apply from now on.
pub async fn configure(
    writer: &Writer,
    pool: &Pool,
    title: &str,
    board_id: &str,
    change: &BoardChange,
    now: u64,
) -> sqlx::Result<Result<BoardSettings, ConfigRefusal>> {
    let mut tx = writer.begin(pool).await?;
    let known = self::board(&mut tx, title, board_id).await?;
    let mut board = known.unwrap_or_else(Board::new);
    if let Some(order) = change.order {
        board.order = order;
    }
    if let Some(status) = change.status {
        board.status = status;
    }
    if let Some(min) = change.min_score {
        board.min_score = min;
    }
    if let Some(max) = change.max_score {
        board.max_score = max;
    }
    let limit = SCORE_MAX_DEFAULT;
    if board.min_score > board.max_score || board.min_score < -limit || board.max_score > limit {
        return Ok(Err(ConfigRefusal::Bounds));
    }
    if known.is_none() {
        if count_of_title(&mut tx, title).await? >= i64::from(BOARDS_PER_TITLE_MAX) {
            return Ok(Err(ConfigRefusal::TitleFull));
        }
        sqlx::query(
            "INSERT INTO boards (title_id, board, better, status, min_score, max_score, \
                                 made_by, created) \
             VALUES ($1, $2, $3, $4, $5, $6, NULL, $7)",
        )
        .bind(title)
        .bind(board_id)
        .bind(order_name(board.order))
        .bind(status_name(board.status))
        .bind(board.min_score)
        .bind(board.max_score)
        .bind(i(now))
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(
            "UPDATE boards SET better = $3, status = $4, min_score = $5, max_score = $6 \
             WHERE title_id = $1 AND board = $2",
        )
        .bind(title)
        .bind(board_id)
        .bind(order_name(board.order))
        .bind(status_name(board.status))
        .bind(board.min_score)
        .bind(board.max_score)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Ok(BoardSettings {
        title_id: title.to_owned(),
        board: board_id.to_owned(),
        order: board.order,
        status: board.status,
        min_score: board.min_score,
        max_score: board.max_score,
    }))
}

/// Marks the account's entry removed, with who did it and why. False when there is no entry or
/// it is removed already.
pub async fn remove(
    conn: &mut Conn,
    title: &str,
    board_id: &str,
    account: u64,
    by: &str,
    reason: &str,
    now: u64,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE scores SET removed_at = $4, removed_by = $5, removed_reason = $6 \
         WHERE title_id = $1 AND board = $2 AND account_id = $3 AND removed_at IS NULL",
    )
    .bind(title)
    .bind(board_id)
    .bind(i(account))
    .bind(i(now))
    .bind(by)
    .bind(reason)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Takes the removal mark off. False when the entry is not removed.
pub async fn restore(
    conn: &mut Conn,
    title: &str,
    board_id: &str,
    account: u64,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE scores SET removed_at = NULL, removed_by = NULL, removed_reason = NULL \
         WHERE title_id = $1 AND board = $2 AND account_id = $3 AND removed_at IS NOT NULL",
    )
    .bind(title)
    .bind(board_id)
    .bind(i(account))
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// For the export: the account's entries, with the removal mark an entry carries.
pub async fn export(conn: &mut Conn, account: u64) -> sqlx::Result<Vec<serde_json::Value>> {
    let rows: Vec<(String, String, i64, i64, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT title_id, board, score, achieved, removed_at, removed_reason FROM scores \
         WHERE account_id = $1 ORDER BY title_id, board",
    )
    .bind(i(account))
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(title, board, score, achieved, removed_at, reason)| {
            serde_json::json!({
                "title_id": title, "board": board, "score": score, "achieved": achieved,
                "removed_at": removed_at, "removed_reason": reason,
            })
        })
        .collect())
}
