//! Profiles, avatars and playtime (migration 0006).

use std::collections::{BTreeMap, HashMap};

use anyplay_api::PlaytimeEntry;

use super::{Pool, i, u};

/// The most title-days an account keeps; one title on one day is one row.
pub const MAX_PLAYTIME_ROWS: i64 = 20_000;

/// The text and favourite titles of an account; empty when it has set none.
pub async fn profile_of(pool: &Pool, account: u64) -> sqlx::Result<(String, Vec<String>)> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT about, favourites FROM profiles WHERE account_id = $1")
            .bind(i(account))
            .fetch_optional(pool)
            .await?;
    Ok(row.map_or_else(Default::default, |(about, favourites)| {
        let favourites = favourites
            .split(',')
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .collect();
        (about, favourites)
    }))
}

/// Sets the parts that are given and leaves the others.
pub async fn set_profile(
    writer: &super::Writer,
    pool: &Pool,
    account: u64,
    about: Option<&str>,
    favourites: Option<&[String]>,
    now: u64,
) -> sqlx::Result<()> {
    let mut tx = writer.begin(pool).await?;
    sqlx::query(
        "INSERT INTO profiles (account_id, about, favourites, updated) VALUES ($1, '', '', $2) \
         ON CONFLICT DO NOTHING",
    )
    .bind(i(account))
    .bind(i(now))
    .execute(&mut *tx)
    .await?;
    if let Some(about) = about {
        sqlx::query("UPDATE profiles SET about = $2, updated = $3 WHERE account_id = $1")
            .bind(i(account))
            .bind(about)
            .bind(i(now))
            .execute(&mut *tx)
            .await?;
    }
    if let Some(favourites) = favourites {
        sqlx::query("UPDATE profiles SET favourites = $2, updated = $3 WHERE account_id = $1")
            .bind(i(account))
            .bind(favourites.join(","))
            .bind(i(now))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

/// The stored avatar of an account.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct AvatarRow {
    pub hash: String,
    pub picture: Vec<u8>,
    pub updated: i64,
}

/// Keeps `picture` as the account's avatar, replacing the one it had.
pub async fn put_avatar(
    pool: &Pool,
    account: u64,
    hash: &str,
    picture: &[u8],
    now: u64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO avatars (account_id, hash, picture, updated) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (account_id) DO UPDATE SET hash = $2, picture = $3, updated = $4",
    )
    .bind(i(account))
    .bind(hash)
    .bind(picture)
    .bind(i(now))
    .execute(pool)
    .await?;
    Ok(())
}

/// The hash of the account's avatar, if it has one.
pub async fn avatar_hash_of(pool: &Pool, account: u64) -> sqlx::Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT hash FROM avatars WHERE account_id = $1")
        .bind(i(account))
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(hash,)| hash))
}

/// The avatar hash of each of the accounts that has an avatar, in one query.
pub async fn avatar_hashes_of(
    conn: &mut sqlx::AnyConnection,
    accounts: &[u64],
) -> sqlx::Result<HashMap<u64, String>> {
    if accounts.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(i64, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT account_id, hash FROM avatars WHERE account_id IN ({})",
        super::id_list(accounts)
    )))
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|(id, hash)| (u(id), hash)).collect())
}

/// The whole avatar row of an account, for the export.
pub async fn avatar_of(pool: &Pool, account: u64) -> sqlx::Result<Option<AvatarRow>> {
    sqlx::query_as("SELECT hash, picture, updated FROM avatars WHERE account_id = $1")
        .bind(i(account))
        .fetch_optional(pool)
        .await
}

/// The picture with this hash.
pub async fn avatar_by_hash(pool: &Pool, hash: &str) -> sqlx::Result<Option<Vec<u8>>> {
    let row: Option<(Vec<u8>,)> =
        sqlx::query_as("SELECT picture FROM avatars WHERE hash = $1 LIMIT 1")
            .bind(hash)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(picture,)| picture))
}

/// Removes an account's avatar; true when it had one. The player's own removal calls this, and so
/// can moderation: the picture stops being served at once.
pub async fn remove_avatar(pool: &Pool, account: u64) -> sqlx::Result<bool> {
    let done = sqlx::query("DELETE FROM avatars WHERE account_id = $1")
        .bind(i(account))
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// How many title-days the account has.
pub async fn playtime_rows(pool: &Pool, account: u64) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM playtime WHERE account_id = $1")
        .bind(i(account))
        .fetch_one(pool)
        .await?;
    Ok(n)
}

/// Keeps the highest minutes per title and day. An entry twice in one call counts once, with its
/// higher number. Returns how many entries were new or raised the kept number.
pub async fn add_playtime(
    writer: &super::Writer,
    pool: &Pool,
    account: u64,
    entries: &[PlaytimeEntry],
) -> sqlx::Result<u64> {
    let mut highest: BTreeMap<(&str, &str), u32> = BTreeMap::new();
    for entry in entries {
        let kept = highest
            .entry((entry.title_id.as_str(), entry.day.as_str()))
            .or_default();
        *kept = (*kept).max(entry.minutes);
    }
    let mut tx = writer.begin(pool).await?;
    let mut changed = 0;
    for ((title, day), minutes) in highest {
        changed += sqlx::query(
            "INSERT INTO playtime (account_id, title_id, day, minutes) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (account_id, title_id, day) DO UPDATE SET minutes = $4 \
             WHERE playtime.minutes < $4",
        )
        .bind(i(account))
        .bind(title)
        .bind(day)
        .bind(i64::from(minutes))
        .execute(&mut *tx)
        .await?
        .rows_affected();
    }
    tx.commit().await?;
    Ok(changed)
}

/// Every title-day of an account, for the export.
pub async fn playtime_of(pool: &Pool, account: u64) -> sqlx::Result<Vec<PlaytimeEntry>> {
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT title_id, day, minutes FROM playtime WHERE account_id = $1 \
         ORDER BY title_id, day",
    )
    .bind(i(account))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(title_id, day, minutes)| PlaytimeEntry {
            title_id,
            day,
            minutes: u32::try_from(minutes).unwrap_or(u32::MAX),
        })
        .collect())
}

/// Total minutes per title, the most played first.
pub async fn playtime_totals(
    pool: &Pool,
    account: u64,
) -> sqlx::Result<Vec<anyplay_api::TitlePlaytime>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT title_id, CAST(SUM(minutes) AS BIGINT) FROM playtime WHERE account_id = $1 \
         GROUP BY title_id ORDER BY 2 DESC, title_id",
    )
    .bind(i(account))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(title_id, minutes)| anyplay_api::TitlePlaytime {
            title_id,
            minutes: u(minutes),
        })
        .collect())
}
