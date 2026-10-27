//! Played with recently: the people an account shared a party or a session with for a while
//! (network `spec/cloud-api.md`, "Played with recently", ADR 0038). Written once with `$1`
//! parameters for SQLite and Postgres, like [`crate::lfg`].
//!
//! The module has three jobs behind a small interface. [`record`] looks at who is together right
//! now and remembers the pairs that have been for long enough; [`list`] answers one account's
//! own list; [`prune`] keeps the table to 30 days and 50 people per account. Nothing else in the
//! cloud knows how a pair is found, so no leave, removal or close of a party or session has to
//! call in here.

use anyplay_api::{
    Account, AccountId, PLAYED_WITH_DAYS, PLAYED_WITH_MAX, PlayedWith, PlayedWithList,
};
use sqlx::FromRow;

use crate::live::Conn;
use crate::state::Inner;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// A pair that is still together is written again only when its entry is older than this, so a
/// party that plays for hours costs a write every few minutes and not every sweep.
const REFRESH_MS: i64 = 5 * 60 * 1000;

fn i(id: u64) -> i64 {
    id as i64
}

fn u(id: i64) -> u64 {
    id as u64
}

/// Remembers the pairs of accounts that are members of the same party or session now and have
/// been for `min_secs`, for the accounts that remember. Both directions of a pair are written,
/// each only if its own account remembers; a block in either direction writes nothing. A pair
/// that shares a session and a party gets the session's title. Returns the rows written.
pub async fn record(conn: &mut Conn, now_ms: i64, min_secs: i64) -> sqlx::Result<u64> {
    let joined_by = now_ms / 1000 - min_secs;
    let result = sqlx::query(
        "INSERT INTO played_with (account_id, other_id, last_ms, title_id, title) \
         SELECT me, other, CAST($2 AS BIGINT), title_id, title FROM ( \
             SELECT t.me AS me, t.other AS other, t.title_id AS title_id, t.title AS title, \
                    ROW_NUMBER() OVER (PARTITION BY t.me, t.other \
                                       ORDER BY t.pref DESC, t.seen DESC) AS n \
             FROM ( \
                 SELECT a.account_id AS me, b.account_id AS other, s.title_id AS title_id, \
                        s.title AS title, 1 AS pref, s.created AS seen \
                 FROM members a \
                 JOIN members b ON b.session_id = a.session_id AND b.account_id <> a.account_id \
                 JOIN sessions s ON s.id = a.session_id \
                 WHERE a.joined <= $1 AND b.joined <= $1 \
                 UNION ALL \
                 SELECT a.account_id, b.account_id, CAST(NULL AS TEXT), CAST(NULL AS TEXT), 0, 0 \
                 FROM party_members a \
                 JOIN party_members b ON b.party_id = a.party_id AND b.account_id <> a.account_id \
                 WHERE a.joined <= $1 AND b.joined <= $1 \
             ) AS t \
             JOIN accounts r ON r.id = t.me AND r.remember_played_with = 1 \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM blocks k \
                 WHERE (k.blocker = t.me AND k.blocked = t.other) \
                    OR (k.blocker = t.other AND k.blocked = t.me)) \
         ) AS ranked WHERE n = 1 \
         ON CONFLICT (account_id, other_id) DO UPDATE \
         SET last_ms = excluded.last_ms, title_id = excluded.title_id, title = excluded.title \
         WHERE played_with.last_ms < excluded.last_ms - $3",
    )
    .bind(joined_by)
    .bind(now_ms)
    .bind(REFRESH_MS)
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// Deletes the entries older than 30 days, and the oldest of an account past 50. Returns the
/// rows deleted.
pub async fn prune(conn: &mut Conn, now_ms: i64) -> sqlx::Result<u64> {
    let old = sqlx::query("DELETE FROM played_with WHERE last_ms < $1")
        .bind(now_ms - PLAYED_WITH_DAYS as i64 * DAY_MS)
        .execute(&mut *conn)
        .await?;
    let extra = sqlx::query(
        "DELETE FROM played_with WHERE (account_id, other_id) IN ( \
             SELECT account_id, other_id FROM ( \
                 SELECT account_id, other_id, \
                        ROW_NUMBER() OVER (PARTITION BY account_id \
                                           ORDER BY last_ms DESC, other_id) AS n \
                 FROM played_with \
                 WHERE account_id IN ( \
                     SELECT account_id FROM played_with \
                     GROUP BY account_id HAVING COUNT(*) > $1) \
             ) AS ranked WHERE n > $1)",
    )
    .bind(PLAYED_WITH_MAX as i64)
    .execute(conn)
    .await?;
    Ok(old.rows_affected() + extra.rows_affected())
}

#[derive(FromRow)]
struct Row {
    other_id: i64,
    handle: String,
    last_ms: i64,
    title_id: Option<String>,
    title: Option<String>,
    friend: i64,
}

/// The account's own list: newest first, at most 50, the last 30 days, without anyone a block lies
/// between, with `friend` set for the ones who are friends now.
pub async fn list(conn: &mut Conn, account: u64, now_ms: i64) -> sqlx::Result<PlayedWithList> {
    let remember = remembers(conn, account).await?.unwrap_or(true);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT p.other_id, a.handle, p.last_ms, p.title_id, p.title, \
                CAST(CASE WHEN EXISTS ( \
                    SELECT 1 FROM friends f \
                    WHERE f.low = CASE WHEN p.account_id < p.other_id \
                                       THEN p.account_id ELSE p.other_id END \
                      AND f.high = CASE WHEN p.account_id < p.other_id \
                                        THEN p.other_id ELSE p.account_id END) \
                    THEN 1 ELSE 0 END AS BIGINT) AS friend \
         FROM played_with p JOIN accounts a ON a.id = p.other_id \
         WHERE p.account_id = $1 AND p.last_ms >= $2 \
           AND NOT EXISTS ( \
               SELECT 1 FROM blocks k \
               WHERE (k.blocker = p.account_id AND k.blocked = p.other_id) \
                  OR (k.blocker = p.other_id AND k.blocked = p.account_id)) \
         ORDER BY p.last_ms DESC, p.other_id LIMIT $3",
    )
    .bind(i(account))
    .bind(now_ms - PLAYED_WITH_DAYS as i64 * DAY_MS)
    .bind(PLAYED_WITH_MAX as i64)
    .fetch_all(conn)
    .await?;
    Ok(PlayedWithList {
        remember,
        people: rows
            .into_iter()
            .map(|row| PlayedWith {
                account: Account {
                    id: AccountId(u(row.other_id)),
                    handle: row.handle,
                },
                last: u(row.last_ms / 1000),
                title_id: row.title_id,
                title: row.title,
                friend: row.friend != 0,
            })
            .collect(),
    })
}

/// Empties the account's list. The entries other accounts hold of it stay theirs.
pub async fn clear(conn: &mut Conn, account: u64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM played_with WHERE account_id = $1")
        .bind(i(account))
        .execute(conn)
        .await?;
    Ok(())
}

/// Whether the account has the switch on; `None` when there is no such account.
pub async fn remembers(conn: &mut Conn, account: u64) -> sqlx::Result<Option<bool>> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT remember_played_with FROM accounts WHERE id = $1")
            .bind(i(account))
            .fetch_optional(conn)
            .await?;
    Ok(row.map(|(on,)| on != 0))
}

/// Sets the switch; false when there is no such account.
pub async fn set_remember(conn: &mut Conn, account: u64, on: bool) -> sqlx::Result<bool> {
    let done = sqlx::query("UPDATE accounts SET remember_played_with = $2 WHERE id = $1")
        .bind(i(account))
        .bind(i64::from(on))
        .execute(conn)
        .await?;
    Ok(done.rows_affected() == 1)
}

/// For the export: the account's entries, newest first, with the other account's ID and handle.
pub async fn export(conn: &mut Conn, account: u64) -> sqlx::Result<Vec<serde_json::Value>> {
    let rows: Vec<(i64, String, i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT p.other_id, a.handle, p.last_ms, p.title_id, p.title \
         FROM played_with p JOIN accounts a ON a.id = p.other_id \
         WHERE p.account_id = $1 ORDER BY p.last_ms DESC, p.other_id",
    )
    .bind(i(account))
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, handle, last_ms, title_id, title)| {
            serde_json::json!({
                "account": {"id": id, "handle": handle},
                "last": last_ms / 1000, "title_id": title_id, "title": title,
            })
        })
        .collect())
}

impl Inner {
    /// The sweep step: records who has been together long enough and prunes. Throttled to
    /// `tuning.played_with_every` per node; several nodes doing it write the same rows.
    pub async fn sweep_played_with(&self, now_ms: i64) -> Result<(), crate::error::AppError> {
        use std::sync::atomic::Ordering;
        let every = self.tuning.played_with_every.as_millis() as i64;
        let last = self.played_with_at.load(Ordering::Relaxed);
        if now_ms < last.saturating_add(every)
            || self
                .played_with_at
                .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            return Ok(());
        }
        let min = i64::try_from(self.tuning.played_with_min.as_secs()).unwrap_or(i64::MAX / 2);
        let work = async {
            let mut conn = crate::db::acquire_maintenance(&self.db).await?;
            record(&mut conn, now_ms, min).await?;
            prune(&mut conn, now_ms).await?;
            Ok::<_, sqlx::Error>(())
        }
        .await;
        if let Err(error) = work {
            self.played_with_at.store(last, Ordering::Relaxed);
            return Err(error.into());
        }
        Ok(())
    }
}
