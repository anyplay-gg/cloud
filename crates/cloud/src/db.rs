//! The store: accounts, devices, friends, blocks, reports and bans, in a SQLite file for one
//! process or in PostgreSQL for a cluster (`docs/adr/0003-postgres-for-a-cluster.md`).
//!
//! Every query is written once and runs through sqlx's `Any` driver. Parameters are `$1`, `$2`,
//! which both databases bind by number, and the SQL keeps to what both understand. Only the
//! migrations exist twice, with the same tables and columns. IDs are at most 2^53 - 1, so they
//! are stored as `i64` and converted at the edge. Every value reaches SQL as a bound parameter.

use std::path::Path;

use sqlx::any::{AnyPoolOptions, install_default_drivers};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{ConnectOptions as _, Connection as _, Executor as _, FromRow};

pub type Pool = sqlx::AnyPool;

/// Where the store keeps its data.
#[derive(Clone, Copy, Debug)]
pub enum Location<'a> {
    /// A SQLite file, created when missing.
    File(&'a Path),
    /// A PostgreSQL database.
    Postgres(&'a str),
}

/// Opens the store with at most `connections` connections and brings its tables up to date.
pub async fn open(location: Location<'_>, connections: u32) -> anyhow::Result<Pool> {
    install_default_drivers();
    let options = AnyPoolOptions::new().max_connections(connections);
    match location {
        Location::File(path) => {
            // WAL belongs to the file, so it is set once, before the pool opens connections.
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .connect()
                .await?
                .close()
                .await?;
            let pool = options
                .after_connect(|conn, _| {
                    Box::pin(async move {
                        // With WAL this loses nothing on a crash of the process, only on one of
                        // the machine, and then at most the last moments.
                        conn.execute("PRAGMA synchronous = NORMAL").await?;
                        Ok(())
                    })
                })
                .connect(&sqlite_url(path))
                .await?;
            sqlx::migrate!("./migrations/sqlite").run(&pool).await?;
            Ok(pool)
        }
        Location::Postgres(url) => {
            let pool = options.connect(url).await?;
            sqlx::migrate!("./migrations/postgres").run(&pool).await?;
            Ok(pool)
        }
    }
}

/// The `sqlite:` URL of a file, with the characters a URL reads otherwise escaped. Folders may
/// hold spaces, `#` and `%`.
fn sqlite_url(path: &Path) -> String {
    let mut url = String::from("sqlite:");
    for c in path.to_string_lossy().chars() {
        match c {
            '\\' => url.push('/'),
            '%' => url.push_str("%25"),
            '?' => url.push_str("%3F"),
            '#' => url.push_str("%23"),
            ' ' => url.push_str("%20"),
            c => url.push(c),
        }
    }
    url.push_str("?mode=rwc");
    url
}

fn i(id: u64) -> i64 {
    id as i64
}

fn u(id: i64) -> u64 {
    id as u64
}

#[derive(Clone, Debug, FromRow)]
pub struct AccountRow {
    pub id: i64,
    pub handle: String,
    pub email: String,
    pub created: i64,
}

#[derive(Clone, Debug, FromRow)]
pub struct DeviceRow {
    pub id: i64,
    pub account_id: i64,
    pub name: String,
    pub platform: String,
    pub refresh_hash: Vec<u8>,
    pub created: i64,
    pub last_used: i64,
}

#[derive(Clone, Debug, FromRow)]
pub struct BanRow {
    pub account_id: i64,
    pub until: Option<i64>,
    pub reason: String,
    pub created: i64,
}

impl BanRow {
    pub fn until(&self) -> Option<u64> {
        self.until.map(u)
    }

    /// True while the ban applies at `now`.
    pub fn active(&self, now: u64) -> bool {
        self.until().is_none_or(|until| until > now)
    }
}

/// An account with the time a relationship began.
#[derive(Clone, Debug, FromRow)]
pub struct Related {
    pub id: i64,
    pub handle: String,
    pub since: i64,
}

#[derive(Clone, Debug, FromRow)]
pub struct ReportRow {
    pub id: i64,
    pub reporter: i64,
    pub reporter_handle: String,
    pub reported: i64,
    pub reported_handle: String,
    pub reason: String,
    pub details: String,
    pub created: i64,
}

impl AccountRow {
    pub fn id(&self) -> u64 {
        u(self.id)
    }
}

impl DeviceRow {
    pub fn id(&self) -> u64 {
        u(self.id)
    }
}

impl Related {
    pub fn id(&self) -> u64 {
        u(self.id)
    }

    pub fn since(&self) -> u64 {
        u(self.since)
    }
}

// Accounts

pub async fn account(pool: &Pool, id: u64) -> sqlx::Result<Option<AccountRow>> {
    sqlx::query_as("SELECT id, handle, email, created FROM accounts WHERE id = $1")
        .bind(i(id))
        .fetch_optional(pool)
        .await
}

/// `key` is the handle in lower case.
pub async fn account_by_handle(pool: &Pool, key: &str) -> sqlx::Result<Option<AccountRow>> {
    sqlx::query_as("SELECT id, handle, email, created FROM accounts WHERE handle_key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await
}

pub async fn account_by_email(pool: &Pool, email: &str) -> sqlx::Result<Option<AccountRow>> {
    sqlx::query_as("SELECT id, handle, email, created FROM accounts WHERE email = $1")
        .bind(email)
        .fetch_optional(pool)
        .await
}

/// What stopped an insert.
#[derive(Debug, PartialEq, Eq)]
pub enum InsertError {
    Id,
    Handle,
    Email,
    Other,
}

pub async fn insert_account(
    pool: &Pool,
    id: u64,
    handle: &str,
    email: &str,
    created: u64,
) -> Result<(), InsertError> {
    let result = sqlx::query(
        "INSERT INTO accounts (id, handle, handle_key, email, created) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(i(id))
    .bind(handle)
    .bind(handle.to_ascii_lowercase())
    .bind(email)
    .bind(i(created))
    .execute(pool)
    .await;
    match result {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            let message = e.message();
            Err(if message.contains("handle_key") {
                InsertError::Handle
            } else if message.contains("email") {
                InsertError::Email
            } else {
                InsertError::Id
            })
        }
        Err(error) => {
            tracing::error!(%error, "cannot insert an account");
            Err(InsertError::Other)
        }
    }
}

/// Deletes an account and everything that hangs on it. Bans stay.
pub async fn delete_account(pool: &Pool, id: u64) -> sqlx::Result<()> {
    // Foreign keys cascade to devices, friends, requests, blocks and reports.
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(i(id))
        .execute(pool)
        .await?;
    Ok(())
}

// Devices

pub async fn all_devices(pool: &Pool) -> sqlx::Result<Vec<(u64, u64)>> {
    let rows: Vec<(i64, i64)> = sqlx::query_as("SELECT id, account_id FROM devices")
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|(d, a)| (u(d), u(a))).collect())
}

pub async fn devices_of(pool: &Pool, account: u64) -> sqlx::Result<Vec<DeviceRow>> {
    sqlx::query_as(
        "SELECT id, account_id, name, platform, refresh_hash, created, last_used \
         FROM devices WHERE account_id = $1 ORDER BY created",
    )
    .bind(i(account))
    .fetch_all(pool)
    .await
}

pub async fn insert_device(
    pool: &Pool,
    id: u64,
    account: u64,
    name: &str,
    platform: &str,
    refresh_hash: &[u8],
    now: u64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO devices (id, account_id, name, platform, refresh_hash, created, last_used) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(i(id))
    .bind(i(account))
    .bind(name)
    .bind(platform)
    .bind(refresh_hash)
    .bind(i(now))
    .bind(i(now))
    .execute(pool)
    .await?;
    Ok(())
}

/// The device that least recently refreshed, when the account has at least `keep` devices.
pub async fn device_to_evict(pool: &Pool, account: u64, keep: i64) -> sqlx::Result<Option<u64>> {
    let row: Option<(i64, i64)> = sqlx::query_as(
        "SELECT id, (SELECT COUNT(*) FROM devices WHERE account_id = $1) AS n FROM devices \
         WHERE account_id = $1 ORDER BY last_used ASC, created ASC LIMIT 1",
    )
    .bind(i(account))
    .fetch_optional(pool)
    .await?;
    Ok(row.filter(|(_, n)| *n >= keep).map(|(id, _)| u(id)))
}

pub async fn delete_device(pool: &Pool, id: u64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM devices WHERE id = $1")
        .bind(i(id))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn device_by_current(pool: &Pool, hash: &[u8]) -> sqlx::Result<Option<DeviceRow>> {
    sqlx::query_as(
        "SELECT id, account_id, name, platform, refresh_hash, created, last_used \
         FROM devices WHERE refresh_hash = $1",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await
}

/// The device a replaced refresh token belonged to, if it was replaced recently.
pub async fn device_by_used(pool: &Pool, hash: &[u8]) -> sqlx::Result<Option<DeviceRow>> {
    sqlx::query_as(
        "SELECT d.id, d.account_id, d.name, d.platform, d.refresh_hash, d.created, d.last_used \
         FROM used_refresh u JOIN devices d ON d.id = u.device_id WHERE u.hash = $1",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await
}

/// How long a replaced refresh token is remembered.
const USED_REFRESH_KEEP: u64 = 30 * 24 * 60 * 60;

/// Replaces the refresh hash if it is still `old`, and remembers `old`. False when another
/// request got there first.
pub async fn rotate_refresh(
    pool: &Pool,
    device: u64,
    old: &[u8],
    new: &[u8],
    now: u64,
) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let result = sqlx::query(
        "UPDATE devices SET refresh_hash = $1, last_used = $2 WHERE id = $3 AND refresh_hash = $4",
    )
    .bind(new)
    .bind(i(now))
    .bind(i(device))
    .bind(old)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() != 1 {
        return Ok(false);
    }
    sqlx::query("INSERT INTO used_refresh (hash, device_id, used) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(old)
        .bind(i(device))
        .bind(i(now))
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM used_refresh WHERE device_id = $1 AND used < $2")
        .bind(i(device))
        .bind(i(now.saturating_sub(USED_REFRESH_KEEP)))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

// Friends and requests

fn pair(a: u64, b: u64) -> (i64, i64) {
    (i(a.min(b)), i(a.max(b)))
}

pub async fn friends_of(pool: &Pool, account: u64) -> sqlx::Result<Vec<Related>> {
    sqlx::query_as(
        "SELECT a.id AS id, a.handle AS handle, f.since AS since FROM friends f \
         JOIN accounts a ON a.id = CASE WHEN f.low = $1 THEN f.high ELSE f.low END \
         WHERE f.low = $1 OR f.high = $1 ORDER BY a.handle_key",
    )
    .bind(i(account))
    .fetch_all(pool)
    .await
}

pub async fn friend_ids(pool: &Pool, account: u64) -> sqlx::Result<Vec<u64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT CASE WHEN low = $1 THEN high ELSE low END FROM friends WHERE low = $1 OR high = $1",
    )
    .bind(i(account))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| u(id)).collect())
}

pub async fn are_friends(pool: &Pool, a: u64, b: u64) -> sqlx::Result<bool> {
    let (low, high) = pair(a, b);
    let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM friends WHERE low = $1 AND high = $2")
        .bind(low)
        .bind(high)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

pub async fn count_friends(pool: &Pool, account: u64) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM friends WHERE low = $1 OR high = $1")
        .bind(i(account))
        .fetch_one(pool)
        .await?;
    Ok(n)
}

/// Turns a request into a friendship, or adds one directly. Removes requests both ways.
pub async fn make_friends(pool: &Pool, a: u64, b: u64, now: u64) -> sqlx::Result<()> {
    let (low, high) = pair(a, b);
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM friend_requests WHERE (from_id = $1 AND to_id = $2) OR (from_id = $2 AND to_id = $1)",
    )
    .bind(low)
    .bind(high)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO friends (low, high, since) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(low)
    .bind(high)
    .bind(i(now))
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

pub async fn remove_friendship(pool: &Pool, a: u64, b: u64) -> sqlx::Result<bool> {
    let (low, high) = pair(a, b);
    let result = sqlx::query("DELETE FROM friends WHERE low = $1 AND high = $2")
        .bind(low)
        .bind(high)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn request_exists(pool: &Pool, from: u64, to: u64) -> sqlx::Result<bool> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM friend_requests WHERE from_id = $1 AND to_id = $2")
            .bind(i(from))
            .bind(i(to))
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some())
}

pub async fn insert_request(pool: &Pool, from: u64, to: u64, now: u64) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO friend_requests (from_id, to_id, since) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(i(from))
        .bind(i(to))
        .bind(i(now))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_request(pool: &Pool, from: u64, to: u64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM friend_requests WHERE from_id = $1 AND to_id = $2")
        .bind(i(from))
        .bind(i(to))
        .execute(pool)
        .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn incoming_requests(pool: &Pool, account: u64) -> sqlx::Result<Vec<Related>> {
    sqlx::query_as(
        "SELECT a.id AS id, a.handle AS handle, r.since AS since FROM friend_requests r \
         JOIN accounts a ON a.id = r.from_id WHERE r.to_id = $1 ORDER BY r.since DESC",
    )
    .bind(i(account))
    .fetch_all(pool)
    .await
}

pub async fn outgoing_requests(pool: &Pool, account: u64) -> sqlx::Result<Vec<Related>> {
    sqlx::query_as(
        "SELECT a.id AS id, a.handle AS handle, r.since AS since FROM friend_requests r \
         JOIN accounts a ON a.id = r.to_id WHERE r.from_id = $1 ORDER BY r.since DESC",
    )
    .bind(i(account))
    .fetch_all(pool)
    .await
}

pub async fn count_incoming(pool: &Pool, account: u64) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM friend_requests WHERE to_id = $1")
        .bind(i(account))
        .fetch_one(pool)
        .await?;
    Ok(n)
}

pub async fn count_outgoing(pool: &Pool, account: u64) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM friend_requests WHERE from_id = $1")
        .bind(i(account))
        .fetch_one(pool)
        .await?;
    Ok(n)
}

// Blocks

pub async fn blocked_by(pool: &Pool, blocker: u64) -> sqlx::Result<Vec<Related>> {
    sqlx::query_as(
        "SELECT a.id AS id, a.handle AS handle, b.since AS since FROM blocks b \
         JOIN accounts a ON a.id = b.blocked WHERE b.blocker = $1 ORDER BY a.handle_key",
    )
    .bind(i(blocker))
    .fetch_all(pool)
    .await
}

pub async fn count_blocks(pool: &Pool, blocker: u64) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM blocks WHERE blocker = $1")
        .bind(i(blocker))
        .fetch_one(pool)
        .await?;
    Ok(n)
}

/// True when `blocker` blocked `blocked`.
pub async fn has_blocked(pool: &Pool, blocker: u64, blocked: u64) -> sqlx::Result<bool> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM blocks WHERE blocker = $1 AND blocked = $2")
            .bind(i(blocker))
            .bind(i(blocked))
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some())
}

/// Adds a block and removes friendship and requests between the two, in one step. Returns whether
/// a friendship or a request existed in either direction.
pub async fn block(pool: &Pool, blocker: u64, blocked: u64, now: u64) -> sqlx::Result<bool> {
    let (low, high) = pair(blocker, blocked);
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO blocks (blocker, blocked, since) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(i(blocker))
    .bind(i(blocked))
    .bind(i(now))
    .execute(&mut *tx)
    .await?;
    let friends = sqlx::query("DELETE FROM friends WHERE low = $1 AND high = $2")
        .bind(low)
        .bind(high)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let requests = sqlx::query(
        "DELETE FROM friend_requests WHERE (from_id = $1 AND to_id = $2) OR (from_id = $2 AND to_id = $1)",
    )
    .bind(low)
    .bind(high)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(friends + requests > 0)
}

pub async fn unblock(pool: &Pool, blocker: u64, blocked: u64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM blocks WHERE blocker = $1 AND blocked = $2")
        .bind(i(blocker))
        .bind(i(blocked))
        .execute(pool)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// True when `account` blocked any of `others` or any of them blocked `account`.
pub async fn blocked_between_any(pool: &Pool, account: u64, others: &[u64]) -> sqlx::Result<bool> {
    for other in others {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT 1 FROM blocks WHERE (blocker = $1 AND blocked = $2) OR (blocker = $2 AND blocked = $1)",
        )
        .bind(i(account))
        .bind(i(*other))
        .fetch_optional(pool)
        .await?;
        if row.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

// Reports

pub async fn insert_report(
    pool: &Pool,
    reporter: u64,
    reported: u64,
    reason: &str,
    details: &str,
    now: u64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO reports (reporter, reported, reason, details, created) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(i(reporter))
    .bind(i(reported))
    .bind(reason)
    .bind(details)
    .bind(i(now))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn count_reports_since(pool: &Pool, reporter: u64, since: u64) -> sqlx::Result<i64> {
    let (n,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM reports WHERE reporter = $1 AND created >= $2")
            .bind(i(reporter))
            .bind(i(since))
            .fetch_one(pool)
            .await?;
    Ok(n)
}

pub async fn reports_by(pool: &Pool, reporter: u64) -> sqlx::Result<Vec<ReportRow>> {
    sqlx::query_as(REPORT_SELECT_BY)
        .bind(i(reporter))
        .fetch_all(pool)
        .await
}

const REPORT_SELECT_BY: &str = "SELECT r.id, r.reporter, ar.handle AS reporter_handle, \
     r.reported, ad.handle AS reported_handle, r.reason, r.details, r.created \
     FROM reports r JOIN accounts ar ON ar.id = r.reporter JOIN accounts ad ON ad.id = r.reported \
     WHERE r.reporter = $1 ORDER BY r.created DESC";

pub async fn latest_reports(pool: &Pool, limit: i64) -> sqlx::Result<Vec<ReportRow>> {
    sqlx::query_as(
        "SELECT r.id, r.reporter, ar.handle AS reporter_handle, \
         r.reported, ad.handle AS reported_handle, r.reason, r.details, r.created \
         FROM reports r JOIN accounts ar ON ar.id = r.reporter JOIN accounts ad ON ad.id = r.reported \
         ORDER BY r.created DESC, r.id DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

// Bans

pub async fn upsert_ban(
    pool: &Pool,
    account: u64,
    email_hash: &[u8],
    until: Option<u64>,
    reason: &str,
    now: u64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO bans (account_id, email_hash, until, reason, created) VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (account_id) DO UPDATE SET email_hash = excluded.email_hash, \
         until = excluded.until, reason = excluded.reason, created = excluded.created",
    )
    .bind(i(account))
    .bind(email_hash)
    .bind(until.map(i))
    .bind(reason)
    .bind(i(now))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_ban(pool: &Pool, account: u64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM bans WHERE account_id = $1")
        .bind(i(account))
        .execute(pool)
        .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn ban_of(pool: &Pool, account: u64) -> sqlx::Result<Option<BanRow>> {
    sqlx::query_as("SELECT account_id, until, reason, created FROM bans WHERE account_id = $1")
        .bind(i(account))
        .fetch_optional(pool)
        .await
}

pub async fn all_bans(pool: &Pool) -> sqlx::Result<Vec<BanRow>> {
    sqlx::query_as("SELECT account_id, until, reason, created FROM bans")
        .fetch_all(pool)
        .await
}

pub async fn bans_by_email(pool: &Pool, email_hash: &[u8]) -> sqlx::Result<Vec<BanRow>> {
    sqlx::query_as("SELECT account_id, until, reason, created FROM bans WHERE email_hash = $1")
        .bind(email_hash)
        .fetch_all(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_file_opens_in_a_folder_whose_name_a_url_would_misread() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Alec's 100% #1 games");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("anyplay-cloud.db");
        let pool = open(Location::File(&file), 2).await.unwrap();
        insert_account(&pool, 7, "Alec", "alec@example.com", 1)
            .await
            .unwrap();
        let (journal,): (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(journal, "wal");
        let (synchronous,): (i64,) = sqlx::query_as("PRAGMA synchronous")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(synchronous, 1, "NORMAL");
        pool.close().await;

        assert!(file.exists(), "the file is where it was asked for");
        let pool = open(Location::File(&file), 2).await.unwrap();
        let account = account(&pool, 7).await.unwrap().unwrap();
        assert_eq!(account.handle, "Alec");
    }

    #[test]
    fn a_sqlite_url_escapes_what_a_url_reads_otherwise() {
        assert_eq!(
            sqlite_url(Path::new("/srv/my games/50% #1?/cloud.db")),
            "sqlite:/srv/my%20games/50%25%20%231%3F/cloud.db?mode=rwc"
        );
    }
}
