//! The feed: items, who is in a party item, reactions and the titles an account has played
//! (migration 0014). What an item says is a kind and some numbers; names are never stored.

use std::collections::HashMap;

use anyplay_api::{Grade, Reaction, ReactionCounts};

use super::{Conn, Pool, Writer, grade_name, i, id_list, u};

/// The most titles an account remembers as played; past it a new title makes no item.
pub const MAX_FIRSTS: i64 = 2_000;
/// The most members a party item names: a party has at most 8.
const MAX_MEMBERS: usize = 8;

const DAY: u64 = 24 * 60 * 60;

/// What an item says, before it has an ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewItem {
    Trophy {
        title_id: String,
        number: u16,
        grade: Grade,
    },
    Friend {
        with: u64,
    },
    FirstPlayed {
        title_id: String,
    },
    /// The members, the leader first.
    Party {
        members: Vec<u64>,
    },
}

/// An item as stored.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ItemRow {
    pub id: i64,
    pub account_id: i64,
    pub kind: String,
    pub title_id: Option<String>,
    pub trophy_number: Option<i64>,
    pub grade: Option<String>,
    pub other_account: Option<i64>,
    pub members: Option<String>,
    pub created: i64,
}

impl ItemRow {
    pub fn id(&self) -> u64 {
        u(self.id)
    }

    pub fn owner(&self) -> u64 {
        u(self.account_id)
    }

    pub fn is_party(&self) -> bool {
        self.kind == "party"
    }

    /// The account IDs of a party item, the leader first.
    pub fn member_ids(&self) -> Vec<u64> {
        self.members
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .filter_map(|id| id.parse().ok())
            .collect()
    }
}

const COLUMNS: &str =
    "id, account_id, kind, title_id, trophy_number, grade, other_account, members, created";

/// Records items, each for an account, in one transaction. An account gets at most `cap` items per
/// UTC day: the ones past it are dropped. Returns how many were recorded.
pub async fn record(
    writer: &Writer,
    pool: &Pool,
    now: u64,
    cap: u32,
    items: &[(u64, NewItem)],
) -> sqlx::Result<usize> {
    if items.is_empty() {
        return Ok(0);
    }
    let day_start = now - now % DAY;
    let mut tx = writer.begin(pool).await?;
    let mut today: HashMap<u64, i64> = HashMap::new();
    let mut recorded = 0;
    for (account, item) in items {
        if !today.contains_key(account) {
            let (n,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM feed_items WHERE account_id = $1 AND created >= $2",
            )
            .bind(i(*account))
            .bind(i(day_start))
            .fetch_one(&mut *tx)
            .await?;
            today.insert(*account, n);
        }
        let count = today.get_mut(account).expect("counted above");
        if *count >= i64::from(cap) {
            continue;
        }
        *count += 1;
        insert(&mut tx, *account, item, now).await?;
        recorded += 1;
    }
    tx.commit().await?;
    Ok(recorded)
}

async fn insert(conn: &mut Conn, account: u64, item: &NewItem, now: u64) -> sqlx::Result<()> {
    let (kind, title, number, grade, other, members) = match item {
        NewItem::Trophy {
            title_id,
            number,
            grade,
        } => (
            "trophy",
            Some(title_id.as_str()),
            Some(i64::from(*number)),
            Some(grade_name(*grade)),
            None,
            None,
        ),
        NewItem::Friend { with } => ("friend", None, None, None, Some(i(*with)), None),
        NewItem::FirstPlayed { title_id } => (
            "first_played",
            Some(title_id.as_str()),
            None,
            None,
            None,
            None,
        ),
        NewItem::Party { members } => {
            let members: Vec<u64> = members.iter().copied().take(MAX_MEMBERS).collect();
            let text = members
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            ("party", None, None, None, None, Some((text, members)))
        }
    };
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO feed_items (account_id, kind, title_id, trophy_number, grade, \
         other_account, members, created) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(i(account))
    .bind(kind)
    .bind(title)
    .bind(number)
    .bind(grade)
    .bind(other)
    .bind(members.as_ref().map(|(text, _)| text.as_str()))
    .bind(i(now))
    .fetch_one(&mut *conn)
    .await?;
    if let Some((_, members)) = members {
        for member in members {
            sqlx::query(
                "INSERT INTO feed_members (item_id, account_id) VALUES ($1, $2) \
                 ON CONFLICT DO NOTHING",
            )
            .bind(id)
            .bind(i(member))
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
}

/// Remembers that the account has played the title; true when that is new (and so an item is
/// due). An account that remembers [`MAX_FIRSTS`] titles remembers no more.
pub async fn mark_first(pool: &Pool, account: u64, title_id: &str, now: u64) -> sqlx::Result<bool> {
    let (known,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM feed_firsts WHERE account_id = $1")
        .bind(i(account))
        .fetch_one(pool)
        .await?;
    if known >= MAX_FIRSTS {
        return Ok(false);
    }
    let done = sqlx::query(
        "INSERT INTO feed_firsts (account_id, title_id, seen) VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(i(account))
    .bind(title_id)
    .bind(i(now))
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Up to `limit` items about the `owners`, other than party items, older than `before`, newest
/// first. `owners` must not be empty.
pub async fn items_of_owners(
    conn: &mut Conn,
    owners: &[u64],
    before: u64,
    limit: i64,
) -> sqlx::Result<Vec<ItemRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM feed_items WHERE kind <> 'party' AND account_id IN ({}) \
         AND id < $1 ORDER BY id DESC LIMIT $2",
        id_list(owners)
    )))
    .bind(i(before))
    .bind(limit)
    .fetch_all(conn)
    .await
}

/// Up to `limit` party items that name `member`, older than `before`, newest first.
pub async fn party_items_of(
    conn: &mut Conn,
    member: u64,
    before: u64,
    limit: i64,
) -> sqlx::Result<Vec<ItemRow>> {
    sqlx::query_as(
        "SELECT i.id, i.account_id, i.kind, i.title_id, i.trophy_number, i.grade, \
         i.other_account, i.members, i.created FROM feed_members m \
         JOIN feed_items i ON i.id = m.item_id WHERE m.account_id = $1 AND i.id < $2 \
         ORDER BY i.id DESC LIMIT $3",
    )
    .bind(i(member))
    .bind(i(before))
    .bind(limit)
    .fetch_all(conn)
    .await
}

/// One item by ID.
pub async fn item(conn: &mut Conn, id: u64) -> sqlx::Result<Option<ItemRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM feed_items WHERE id = $1"
    )))
    .bind(i(id))
    .fetch_optional(conn)
    .await
}

/// True when the account is one of the members a party item names.
pub async fn is_member(conn: &mut Conn, item: u64, account: u64) -> sqlx::Result<bool> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM feed_members WHERE item_id = $1 AND account_id = $2")
            .bind(i(item))
            .bind(i(account))
            .fetch_optional(conn)
            .await?;
    Ok(row.is_some())
}

/// The reactions to each of the items, and the one `account` gave to each, in two queries.
pub async fn reactions_of(
    conn: &mut Conn,
    items: &[u64],
    account: u64,
) -> sqlx::Result<(HashMap<u64, ReactionCounts>, HashMap<u64, Reaction>)> {
    let mut counts: HashMap<u64, ReactionCounts> = HashMap::new();
    let mut mine = HashMap::new();
    if items.is_empty() {
        return Ok((counts, mine));
    }
    let list = id_list(items);
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT item_id, reaction, COUNT(*) FROM feed_reactions WHERE item_id IN ({list}) \
         GROUP BY item_id, reaction"
    )))
    .fetch_all(&mut *conn)
    .await?;
    for (item, reaction, n) in rows {
        if let Some(reaction) = Reaction::parse(&reaction) {
            let slot = counts.entry(u(item)).or_default();
            for _ in 0..n {
                slot.add(reaction);
            }
        }
    }
    let rows: Vec<(i64, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT item_id, reaction FROM feed_reactions WHERE account_id = $1 \
         AND item_id IN ({list})"
    )))
    .bind(i(account))
    .fetch_all(conn)
    .await?;
    for (item, reaction) in rows {
        if let Some(reaction) = Reaction::parse(&reaction) {
            mine.insert(u(item), reaction);
        }
    }
    Ok((counts, mine))
}

/// Sets the account's reaction to an item, replacing the one it had. True when that changed
/// something; false when the account had this reaction already or the item is gone.
pub async fn set_reaction(
    writer: &Writer,
    pool: &Pool,
    item: u64,
    account: u64,
    reaction: Reaction,
    now: u64,
) -> sqlx::Result<bool> {
    let mut tx = writer.begin(pool).await?;
    let had: Option<(String,)> = sqlx::query_as(
        "SELECT reaction FROM feed_reactions WHERE item_id = $1 AND account_id = $2",
    )
    .bind(i(item))
    .bind(i(account))
    .fetch_optional(&mut *tx)
    .await?;
    if had.is_some_and(|(had,)| had == reaction.as_str()) {
        return Ok(false);
    }
    let done = sqlx::query(
        "INSERT INTO feed_reactions (item_id, account_id, reaction, created) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (item_id, account_id) DO UPDATE SET reaction = $3, created = $4",
    )
    .bind(i(item))
    .bind(i(account))
    .bind(reaction.as_str())
    .bind(i(now))
    .execute(&mut *tx)
    .await;
    match done {
        Ok(_) => {
            tx.commit().await?;
            Ok(true)
        }
        // The retention deleted the item between the check and now.
        Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => Ok(false),
        Err(error) => Err(error),
    }
}

/// Takes the account's reaction back; true when it had one.
pub async fn clear_reaction(pool: &Pool, item: u64, account: u64) -> sqlx::Result<bool> {
    let done = sqlx::query("DELETE FROM feed_reactions WHERE item_id = $1 AND account_id = $2")
        .bind(i(item))
        .bind(i(account))
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// How many accounts have a trophy record for each title, and how many earned each trophy, for
/// all the titles in two queries.
#[derive(Debug, Default)]
pub struct Rarities {
    pub players: HashMap<String, i64>,
    pub earned: HashMap<(String, i64), i64>,
}

pub async fn rarities_of(conn: &mut Conn, titles: &[String]) -> sqlx::Result<Rarities> {
    let mut out = Rarities::default();
    if titles.is_empty() {
        return Ok(out);
    }
    let marks = (1..=titles.len())
        .map(|n| format!("${n}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut players = sqlx::query_as::<_, (String, i64)>(sqlx::AssertSqlSafe(format!(
        "SELECT title_id, COUNT(DISTINCT account_id) FROM trophies \
         WHERE title_id IN ({marks}) GROUP BY title_id"
    )));
    for title in titles {
        players = players.bind(title.as_str());
    }
    let rows = players.fetch_all(&mut *conn).await?;
    out.players = rows.into_iter().collect();
    let mut earned = sqlx::query_as::<_, (String, i64, i64)>(sqlx::AssertSqlSafe(format!(
        "SELECT title_id, number, COUNT(*) FROM trophies WHERE title_id IN ({marks}) \
         GROUP BY title_id, number"
    )));
    for title in titles {
        earned = earned.bind(title.as_str());
    }
    let rows = earned.fetch_all(conn).await?;
    out.earned = rows
        .into_iter()
        .map(|(title, number, n)| ((title, number), n))
        .collect();
    Ok(out)
}

/// Deletes the items recorded before `cut` (Unix seconds); their reactions and party members go
/// with them. Returns how many items went.
pub async fn sweep(pool: &Pool, cut: u64) -> sqlx::Result<u64> {
    let done = sqlx::query("DELETE FROM feed_items WHERE created < $1")
        .bind(i(cut))
        .execute(pool)
        .await?;
    Ok(done.rows_affected())
}

/// The items about an account, for the export.
pub async fn items_about(pool: &Pool, account: u64) -> sqlx::Result<Vec<ItemRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM feed_items WHERE account_id = $1 ORDER BY id"
    )))
    .bind(i(account))
    .fetch_all(pool)
    .await
}

/// The reactions an account gave: item, reaction, time; for the export.
pub async fn reactions_by(pool: &Pool, account: u64) -> sqlx::Result<Vec<(u64, String, u64)>> {
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT item_id, reaction, created FROM feed_reactions WHERE account_id = $1 \
         ORDER BY item_id",
    )
    .bind(i(account))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(item, reaction, created)| (u(item), reaction, u(created)))
        .collect())
}
