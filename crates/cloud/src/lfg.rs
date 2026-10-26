//! Looking for group in the store: listings of a game that live for an hour, and the requests of
//! other players to join them (network `spec/cloud-api.md`, "Looking for group"). Written once
//! with `$1` parameters for SQLite and Postgres, like [`crate::chat`].
//!
//! The cloud takes on trust that a player who asks for the listings of a game has the game, so
//! the filter by title is a convenience and not an access rule.

use anyplay_api::{Account, AccountId, LfgAsk, LfgMode, LfgRequest, Listing};
use sqlx::FromRow;

use crate::live::Conn;

fn i(id: u64) -> i64 {
    id as i64
}

fn u(id: i64) -> u64 {
    id as u64
}

/// A listing with the poster's handle and how many requests were accepted.
#[derive(Clone, Debug, FromRow)]
pub struct ListingRow {
    pub id: String,
    pub poster_id: i64,
    pub handle: String,
    pub title_id: String,
    pub mode: String,
    pub language: Option<String>,
    pub note: String,
    pub slots: i64,
    pub created_ms: i64,
    pub expires_ms: i64,
    pub closed_ms: Option<i64>,
    pub filled: i64,
}

impl ListingRow {
    pub fn poster(&self) -> u64 {
        u(self.poster_id)
    }

    /// Open: not closed, not full and not past its time.
    pub fn is_open(&self, now_ms: i64) -> bool {
        self.closed_ms.is_none() && self.expires_ms > now_ms && self.filled < self.slots
    }

    /// The listing as a viewer sees it.
    pub fn view(&self, ask: Option<LfgAsk>, requests: Vec<LfgRequest>) -> Listing {
        Listing {
            id: self.id.clone(),
            poster: Account {
                id: AccountId(self.poster()),
                handle: self.handle.clone(),
            },
            title_id: self.title_id.clone(),
            mode: LfgMode::parse(&self.mode).unwrap_or(LfgMode::Unknown),
            language: self.language.clone(),
            note: self.note.clone(),
            slots: self.slots.clamp(0, 255) as u8,
            filled: self.filled.clamp(0, 255) as u8,
            created: u(self.created_ms / 1000),
            expires: u(self.expires_ms / 1000),
            ask,
            requests,
        }
    }
}

const SELECT: &str = "SELECT l.id, l.poster_id, a.handle, l.title_id, l.mode, l.language, \
     l.note, l.slots, l.created_ms, l.expires_ms, l.closed_ms, \
     (SELECT COUNT(*) FROM lfg_requests r WHERE r.listing_id = l.id AND r.accepted_ms IS NOT NULL) \
     AS filled \
     FROM lfg_listings l JOIN accounts a ON a.id = l.poster_id";

pub struct NewListing<'a> {
    pub id: &'a str,
    pub poster: u64,
    pub title_id: &'a str,
    pub mode: LfgMode,
    pub language: Option<&'a str>,
    pub note: &'a str,
    pub slots: u8,
    pub created_ms: i64,
    pub expires_ms: i64,
}

pub async fn insert(conn: &mut Conn, listing: &NewListing<'_>) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO lfg_listings \
         (id, poster_id, title_id, mode, language, note, slots, created_ms, expires_ms) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(listing.id)
    .bind(i(listing.poster))
    .bind(listing.title_id)
    .bind(listing.mode.as_str())
    .bind(listing.language)
    .bind(listing.note)
    .bind(i64::from(listing.slots))
    .bind(listing.created_ms)
    .bind(listing.expires_ms)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn listing(conn: &mut Conn, id: &str) -> sqlx::Result<Option<ListingRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE l.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await
}

/// How many listings an account has open, and whether one is for `title`.
pub async fn open_of(conn: &mut Conn, poster: u64, now_ms: i64) -> sqlx::Result<Vec<ListingRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE l.poster_id = $1 AND l.closed_ms IS NULL AND l.expires_ms > $2 \
         ORDER BY l.created_ms DESC"
    )))
    .bind(i(poster))
    .bind(now_ms)
    .fetch_all(conn)
    .await
}

/// The open listings of a game that `viewer` may see: not their own, not by an account a block
/// lies between them and. `mode` and `language` narrow the list when not empty; a listing
/// without a language matches every language.
pub async fn open_for_title(
    conn: &mut Conn,
    viewer: u64,
    title_id: &str,
    mode: &str,
    language: &str,
    now_ms: i64,
    limit: i64,
) -> sqlx::Result<Vec<ListingRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE l.title_id = $1 AND l.closed_ms IS NULL AND l.expires_ms > $2 \
         AND l.poster_id <> $3 \
         AND ($4 = '' OR l.mode = $4) AND ($5 = '' OR l.language IS NULL OR l.language = $5) \
         AND NOT EXISTS (SELECT 1 FROM blocks b WHERE (b.blocker = $3 AND b.blocked = l.poster_id) \
         OR (b.blocker = l.poster_id AND b.blocked = $3)) \
         ORDER BY l.created_ms DESC LIMIT $6"
    )))
    .bind(title_id)
    .bind(now_ms)
    .bind(i(viewer))
    .bind(mode)
    .bind(language)
    .bind(limit)
    .fetch_all(conn)
    .await
}

/// The listings `account` asked to join that are still open, with the state of the request.
pub async fn asked_by(
    conn: &mut Conn,
    account: u64,
    now_ms: i64,
) -> sqlx::Result<Vec<(ListingRow, LfgAsk)>> {
    let rows: Vec<ListingRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE l.closed_ms IS NULL AND l.expires_ms > $2 AND EXISTS \
         (SELECT 1 FROM lfg_requests q WHERE q.listing_id = l.id AND q.account_id = $1) \
         ORDER BY l.created_ms DESC"
    )))
    .bind(i(account))
    .bind(now_ms)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::new();
    for row in rows {
        let state = ask_of(conn, &row.id, account).await?;
        if let Some(state) = state {
            out.push((row, state));
        }
    }
    Ok(out)
}

/// The state of an account's request to a listing, if it made one.
pub async fn ask_of(conn: &mut Conn, listing: &str, account: u64) -> sqlx::Result<Option<LfgAsk>> {
    let row: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT accepted_ms FROM lfg_requests WHERE listing_id = $1 AND account_id = $2",
    )
    .bind(listing)
    .bind(i(account))
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|(accepted,)| {
        if accepted.is_some() {
            LfgAsk::Accepted
        } else {
            LfgAsk::Pending
        }
    }))
}

/// The accounts among `listings` that `account` asked, with the state.
pub async fn asks_among(
    conn: &mut Conn,
    account: u64,
) -> sqlx::Result<std::collections::HashMap<String, LfgAsk>> {
    let rows: Vec<(String, Option<i64>)> =
        sqlx::query_as("SELECT listing_id, accepted_ms FROM lfg_requests WHERE account_id = $1")
            .bind(i(account))
            .fetch_all(conn)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, accepted)| {
            (
                id,
                if accepted.is_some() {
                    LfgAsk::Accepted
                } else {
                    LfgAsk::Pending
                },
            )
        })
        .collect())
}

pub async fn requests_of(conn: &mut Conn, listing: &str) -> sqlx::Result<Vec<LfgRequest>> {
    let rows: Vec<(i64, String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT q.account_id, a.handle, q.created_ms, q.accepted_ms FROM lfg_requests q \
         JOIN accounts a ON a.id = q.account_id WHERE q.listing_id = $1 \
         ORDER BY q.created_ms, q.account_id",
    )
    .bind(listing)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, handle, created, accepted)| LfgRequest {
            from: Account {
                id: AccountId(u(id)),
                handle,
            },
            created: u(created / 1000),
            accepted: accepted.is_some(),
        })
        .collect())
}

pub async fn count_requests(conn: &mut Conn, listing: &str) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM lfg_requests WHERE listing_id = $1")
        .bind(listing)
        .fetch_one(conn)
        .await?;
    Ok(n)
}

/// Adds a request; false when the account asked already.
pub async fn insert_request(
    conn: &mut Conn,
    listing: &str,
    account: u64,
    now_ms: i64,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "INSERT INTO lfg_requests (listing_id, account_id, created_ms) VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(listing)
    .bind(i(account))
    .bind(now_ms)
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn delete_request(conn: &mut Conn, listing: &str, account: u64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM lfg_requests WHERE listing_id = $1 AND account_id = $2")
        .bind(listing)
        .bind(i(account))
        .execute(conn)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// Marks a pending request accepted; false when there is none pending.
pub async fn accept_request(
    conn: &mut Conn,
    listing: &str,
    account: u64,
    now_ms: i64,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE lfg_requests SET accepted_ms = $3 \
         WHERE listing_id = $1 AND account_id = $2 AND accepted_ms IS NULL",
    )
    .bind(listing)
    .bind(i(account))
    .bind(now_ms)
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Closes a listing; false when it was closed already.
pub async fn close(conn: &mut Conn, id: &str, now_ms: i64) -> sqlx::Result<bool> {
    let result =
        sqlx::query("UPDATE lfg_listings SET closed_ms = $2 WHERE id = $1 AND closed_ms IS NULL")
            .bind(id)
            .bind(now_ms)
            .execute(conn)
            .await?;
    Ok(result.rows_affected() == 1)
}

/// Closes every listing of an account and drops its requests to other listings, for a ban.
pub async fn forget_account(conn: &mut Conn, account: u64, now_ms: i64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE lfg_listings SET closed_ms = $2 WHERE poster_id = $1 AND closed_ms IS NULL",
    )
    .bind(i(account))
    .bind(now_ms)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM lfg_requests WHERE account_id = $1")
        .bind(i(account))
        .execute(conn)
        .await?;
    Ok(())
}

/// Deletes listings that closed or ran out before `cut_ms`, with their requests.
pub async fn sweep(conn: &mut Conn, cut_ms: i64) -> sqlx::Result<u64> {
    let result = sqlx::query("DELETE FROM lfg_listings WHERE COALESCE(closed_ms, expires_ms) < $1")
        .bind(cut_ms)
        .execute(conn)
        .await?;
    Ok(result.rows_affected())
}

/// For the export: the listings an account posted and the requests it made, as text lines.
pub async fn export(conn: &mut Conn, account: u64) -> sqlx::Result<Vec<serde_json::Value>> {
    let posted: Vec<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT id, title_id, mode, note, created_ms FROM lfg_listings WHERE poster_id = $1 \
         ORDER BY created_ms",
    )
    .bind(i(account))
    .fetch_all(&mut *conn)
    .await?;
    let asked: Vec<(String, i64)> = sqlx::query_as(
        "SELECT listing_id, created_ms FROM lfg_requests WHERE account_id = $1 ORDER BY created_ms",
    )
    .bind(i(account))
    .fetch_all(conn)
    .await?;
    let mut out: Vec<serde_json::Value> = posted
        .into_iter()
        .map(|(id, title, mode, note, created)| {
            serde_json::json!({"listing": id, "title_id": title, "mode": mode, "note": note,
                "created": created / 1000})
        })
        .collect();
    out.extend(
        asked
            .into_iter()
            .map(|(id, created)| serde_json::json!({"asked": id, "created": created / 1000})),
    );
    Ok(out)
}
