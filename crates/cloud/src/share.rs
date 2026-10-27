//! Share play in the store: a host streams the running game to one member of their party
//! (network `spec/share-play.md`, "Cloud API"). The wire types are in `anyplay-api`.
//!
//! A share is one row that lives while it is `invited` or `active`; `share_members` holds one
//! row per account in it, and its primary key is what keeps an account in one share at a time on
//! every node. Like a session or a party, a change starts by touching the row ([`lock_of`]),
//! which holds a lock until the transaction ends and gives the change its revision. A share ends
//! when the row is deleted, and only the request or sweep whose statement deleted it sends the
//! news, so the news goes out once however many nodes notice. Written once with `$1` parameters
//! for SQLite and Postgres, like [`crate::live`].

use anyplay_api::{
    Account, AccountId, Activity, Event, Share, ShareGoneReason, ShareInfo, ShareInvitation,
    ShareMode, SharePath, ShareState,
};
use sqlx::FromRow;

use crate::error::AppError;
use crate::live::Conn;
use crate::state::Inner;
use crate::{party, util};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

fn i(id: u64) -> i64 {
    id as i64
}

fn u(id: i64) -> u64 {
    id as u64
}

pub fn mode_str(mode: ShareMode) -> &'static str {
    match mode {
        ShareMode::HandOver => "hand_over",
        ShareMode::Together => "together",
        _ => "watch",
    }
}

fn mode_of(text: &str) -> ShareMode {
    match text {
        "hand_over" => ShareMode::HandOver,
        "together" => ShareMode::Together,
        _ => ShareMode::Watch,
    }
}

pub fn path_str(path: SharePath) -> &'static str {
    match path {
        SharePath::Relay => "relay",
        _ => "direct",
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct ShareRow {
    pub id: String,
    pub invitation: String,
    pub host_id: i64,
    pub host_handle: String,
    pub guest_id: i64,
    pub guest_handle: String,
    pub title_id: String,
    pub title: String,
    pub mode: String,
    pub state: String,
    pub host_endpoint: String,
    pub host_relay: Option<String>,
    pub guest_endpoint: String,
    pub guest_relay: Option<String>,
    pub path: Option<String>,
    pub created: i64,
    pub revision: i64,
    pub deadline_ms: i64,
    pub relay_since_ms: Option<i64>,
}

const COLUMNS: &str = "id, invitation, host_id, host_handle, guest_id, guest_handle, title_id, \
     title, mode, state, host_endpoint, host_relay, guest_endpoint, guest_relay, path, created, \
     revision, deadline_ms, relay_since_ms";

impl ShareRow {
    pub fn host(&self) -> u64 {
        u(self.host_id)
    }

    pub fn guest(&self) -> u64 {
        u(self.guest_id)
    }

    pub fn is_active(&self) -> bool {
        self.state == "active"
    }

    pub fn is_party(&self, account: u64) -> bool {
        self.host() == account || self.guest() == account
    }

    /// The share as `viewer` may see it: the invited guest does not see where the host is until
    /// it has accepted. None for anyone else.
    pub fn view(&self, viewer: u64) -> Option<Share> {
        if !self.is_party(viewer) {
            return None;
        }
        let hide = viewer == self.guest() && !self.is_active();
        Some(Share {
            id: self.id.clone(),
            host: Account {
                id: AccountId(self.host()),
                handle: self.host_handle.clone(),
            },
            guest: Account {
                id: AccountId(self.guest()),
                handle: self.guest_handle.clone(),
            },
            title_id: self.title_id.clone(),
            title: self.title.clone(),
            mode: mode_of(&self.mode),
            state: if self.is_active() {
                ShareState::Active
            } else {
                ShareState::Invited
            },
            host_endpoint: if hide {
                String::new()
            } else {
                self.host_endpoint.clone()
            },
            host_relay: if hide { None } else { self.host_relay.clone() },
            guest_endpoint: self.guest_endpoint.clone(),
            guest_relay: self.guest_relay.clone(),
            path: match self.path.as_deref() {
                Some("direct") => Some(SharePath::Direct),
                Some("relay") => Some(SharePath::Relay),
                _ => None,
            },
            created: u(self.created),
            revision: u(self.revision),
        })
    }

    pub fn invitation_view(&self) -> ShareInvitation {
        ShareInvitation {
            id: self.invitation.clone(),
            from: Account {
                id: AccountId(self.host()),
                handle: self.host_handle.clone(),
            },
            share: self.id.clone(),
            title_id: self.title_id.clone(),
            title: self.title.clone(),
            mode: mode_of(&self.mode),
            expires: u(self.deadline_ms) / 1000,
        }
    }
}

// The store

/// Inserts a share with its two members. False when either account is in a share already; the
/// caller then drops its transaction, which takes the first row back.
pub async fn insert(conn: &mut Conn, row: &ShareRow) -> sqlx::Result<bool> {
    sqlx::query(
        "INSERT INTO shares (id, invitation, host_id, host_handle, guest_id, guest_handle, \
         title_id, title, mode, state, host_endpoint, host_relay, guest_endpoint, guest_relay, \
         path, created, revision, deadline_ms, relay_since_ms) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, '', NULL, NULL, $13, 0, $14, NULL)",
    )
    .bind(&row.id)
    .bind(&row.invitation)
    .bind(row.host_id)
    .bind(&row.host_handle)
    .bind(row.guest_id)
    .bind(&row.guest_handle)
    .bind(&row.title_id)
    .bind(&row.title)
    .bind(&row.mode)
    .bind(&row.state)
    .bind(&row.host_endpoint)
    .bind(&row.host_relay)
    .bind(row.created)
    .bind(row.deadline_ms)
    .execute(&mut *conn)
    .await?;
    for account in [row.host_id, row.guest_id] {
        let done = sqlx::query(
            "INSERT INTO share_members (account_id, share_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(account)
        .bind(&row.id)
        .execute(&mut *conn)
        .await?;
        if done.rows_affected() != 1 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The share an account is in, for reading only.
pub async fn share_of(conn: &mut Conn, account: u64) -> sqlx::Result<Option<ShareRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM shares WHERE id = (SELECT share_id FROM share_members WHERE account_id = $1)"
    )))
    .bind(i(account))
    .fetch_optional(conn)
    .await
}

/// Starts a change to the share an account is in: touches its row and reads it. The revision is
/// the new one.
pub async fn lock_of(conn: &mut Conn, account: u64) -> sqlx::Result<Option<ShareRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE shares SET revision = revision + 1 \
         WHERE id = (SELECT share_id FROM share_members WHERE account_id = $1) RETURNING {COLUMNS}"
    )))
    .bind(i(account))
    .fetch_optional(conn)
    .await
}

/// The invitations an account holds that have not run out at `now_ms`, newest first.
pub async fn invitations_of(
    conn: &mut Conn,
    guest: u64,
    now_ms: i64,
) -> sqlx::Result<Vec<ShareRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM shares WHERE guest_id = $1 AND state = 'invited' AND deadline_ms > $2 \
         ORDER BY deadline_ms DESC"
    )))
    .bind(i(guest))
    .bind(now_ms)
    .fetch_all(conn)
    .await
}

/// Removes a share and returns it. Empty when another request or node ended it first.
async fn take(conn: &mut Conn, id: &str) -> sqlx::Result<Option<ShareRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "DELETE FROM shares WHERE id = $1 RETURNING {COLUMNS}"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await
}

pub async fn set_accepted(
    conn: &mut Conn,
    id: &str,
    endpoint: &str,
    relay: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE shares SET state = 'active', guest_endpoint = $2, guest_relay = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(endpoint)
    .bind(relay)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn set_mode(conn: &mut Conn, id: &str, mode: ShareMode) -> sqlx::Result<()> {
    sqlx::query("UPDATE shares SET mode = $2 WHERE id = $1")
        .bind(id)
        .bind(mode_str(mode))
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn set_path(
    conn: &mut Conn,
    id: &str,
    path: SharePath,
    relay_since_ms: Option<i64>,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE shares SET path = $2, relay_since_ms = $3 WHERE id = $1")
        .bind(id)
        .bind(path_str(path))
        .bind(relay_since_ms)
        .execute(conn)
        .await?;
    Ok(())
}

/// What an account plays now, as its presence says.
pub async fn activity_of(conn: &mut Conn, account: u64) -> sqlx::Result<Option<Activity>> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT activity FROM presence WHERE account_id = $1")
            .bind(i(account))
            .fetch_optional(conn)
            .await?;
    Ok(row
        .and_then(|(text,)| text)
        .and_then(|text| serde_json::from_str(&text).ok()))
}

/// Adds the time of a relayed stretch to the host's use, split at UTC midnights.
pub async fn add_relay_use(
    conn: &mut Conn,
    host: u64,
    since_ms: i64,
    until_ms: i64,
) -> sqlx::Result<()> {
    let mut from = since_ms;
    while from < until_ms {
        let day = from / DAY_MS;
        let to = until_ms.min((day + 1) * DAY_MS);
        sqlx::query(
            "INSERT INTO share_relay_use (account_id, day, ms) VALUES ($1, $2, $3) \
             ON CONFLICT (account_id, day) DO UPDATE SET ms = share_relay_use.ms + excluded.ms",
        )
        .bind(i(host))
        .bind(day)
        .bind(to - from)
        .execute(&mut *conn)
        .await?;
        from = to;
    }
    Ok(())
}

/// Milliseconds the host's shares were relayed on the UTC day of `now_ms`, with the stretch that
/// is running (`since_ms`) counted up to now.
pub async fn relay_used_today(
    conn: &mut Conn,
    host: u64,
    since_ms: Option<i64>,
    now_ms: i64,
) -> sqlx::Result<i64> {
    let day = now_ms / DAY_MS;
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT ms FROM share_relay_use WHERE account_id = $1 AND day = $2")
            .bind(i(host))
            .bind(day)
            .fetch_optional(&mut *conn)
            .await?;
    let running = since_ms.map_or(0, |since| (now_ms - since.max(day * DAY_MS)).max(0));
    Ok(row.map_or(0, |(ms,)| ms) + running)
}

/// Forgets the relay use of days before yesterday.
async fn sweep_relay_use(conn: &mut Conn, now_ms: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM share_relay_use WHERE day < $1")
        .bind(now_ms / DAY_MS - 1)
        .execute(conn)
        .await?;
    Ok(())
}

/// Shares whose invitation ran out.
async fn expired(conn: &mut Conn, now_ms: i64) -> sqlx::Result<Vec<String>> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT id FROM shares WHERE state = 'invited' AND deadline_ms <= $1")
            .bind(now_ms)
            .fetch_all(conn)
            .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Shares in which an account has had no event connection since before `cut_ms`.
async fn absent(conn: &mut Conn, cut_ms: i64) -> sqlx::Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT s.id FROM shares s \
         JOIN presence p ON p.account_id = s.host_id OR p.account_id = s.guest_id \
         WHERE p.last_seen_ms IS NOT NULL AND p.last_seen_ms <= $1 \
         AND NOT EXISTS (SELECT 1 FROM connections c WHERE c.account_id = p.account_id)",
    )
    .bind(cut_ms)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Active shares that are being relayed: id, host and when the stretch began.
async fn relayed(conn: &mut Conn) -> sqlx::Result<Vec<(String, i64, i64)>> {
    sqlx::query_as(
        "SELECT id, host_id, relay_since_ms FROM shares WHERE state = 'active' AND relay_since_ms IS NOT NULL",
    )
    .fetch_all(conn)
    .await
}

// What the node does

impl Inner {
    pub fn share_info(&self) -> ShareInfo {
        ShareInfo {
            enabled: self.config.share_enabled,
            relay_kbps: self.config.share_relay_kbps,
        }
    }

    /// Ends a share and tells its parties, once: only the call whose statement deleted the row
    /// sends the news. Returns whether it did.
    pub async fn end_share(&self, id: &str, reason: ShareGoneReason) -> Result<bool, AppError> {
        let now_ms = util::now_ms();
        let mut tx = self.begin_write().await?;
        let Some(row) = take(&mut tx, id).await? else {
            return Ok(false);
        };
        if let Some(since) = row.relay_since_ms {
            add_relay_use(&mut tx, row.host(), since, now_ms).await?;
        }
        tx.commit().await?;

        let gone = Event::ShareGone {
            id: row.id.clone(),
            reason,
        };
        self.news.push(row.host(), &gone);
        if row.is_active() {
            self.news.push(row.guest(), &gone);
        } else {
            self.news.push(
                row.guest(),
                &Event::ShareInvitationGone {
                    id: row.invitation.clone(),
                },
            );
        }
        Ok(true)
    }

    /// The reason a share must end now, if there is one: a ban, a block, leaving the party, the
    /// end of the host's game.
    async fn why_ended(&self, row: &ShareRow) -> Result<Option<ShareGoneReason>, AppError> {
        if self.ban_of(row.host()).is_some() || self.ban_of(row.guest()).is_some() {
            return Ok(Some(ShareGoneReason::Banned));
        }
        let mut conn = self.db.acquire().await?;
        if crate::db::blocked_between_any(&mut conn, row.host(), &[row.guest()]).await? {
            return Ok(Some(ShareGoneReason::Blocked));
        }
        let host_party = party::party_id_of(&mut conn, row.host()).await?;
        let guest_party = party::party_id_of(&mut conn, row.guest()).await?;
        if host_party.is_none() || host_party != guest_party {
            return Ok(Some(ShareGoneReason::Left));
        }
        if activity_of(&mut conn, row.host()).await?.is_none() {
            return Ok(Some(ShareGoneReason::GameEnded));
        }
        Ok(None)
    }

    /// Ends the shares of these accounts that no longer hold: call it after anything that can
    /// break one (leaving a party, a block, a ban, a game that ended). A failure is logged.
    pub async fn check_shares(&self, accounts: &[u64]) {
        if let Err(error) = self.check_shares_inner(accounts).await {
            tracing::error!(error = %error.body.message, "cannot check shares");
        }
    }

    async fn check_shares_inner(&self, accounts: &[u64]) -> Result<(), AppError> {
        for account in accounts {
            let Some(row) = share_of(&mut *self.db.acquire().await?, *account).await? else {
                continue;
            };
            if let Some(reason) = self.why_ended(&row).await? {
                self.end_share(&row.id, reason).await?;
            }
        }
        Ok(())
    }

    /// The part of the sweep that is about time: invitations that ran out, accounts without an
    /// event connection for too long, and relayed minutes used up. Every node runs it.
    pub async fn sweep_shares(&self, now_ms: i64) -> Result<(), AppError> {
        let (late, away, relayed) = {
            let mut conn = self.db.acquire().await?;
            let late = expired(&mut conn, now_ms).await?;
            let away = absent(
                &mut conn,
                now_ms - self.tuning.share_absent.as_millis() as i64,
            )
            .await?;
            let relayed = relayed(&mut conn).await?;
            (late, away, relayed)
        };
        for id in late.iter().chain(&away) {
            self.end_share(id, ShareGoneReason::Timeout).await?;
        }
        let allowance = i64::from(self.config.share_relay_minutes_per_day)
            * self.tuning.share_minute.as_millis() as i64;
        for (id, host, since) in relayed {
            let used =
                relay_used_today(&mut *self.db.acquire().await?, u(host), Some(since), now_ms)
                    .await?;
            if used >= allowance {
                self.end_share(&id, ShareGoneReason::Quota).await?;
            }
        }
        sweep_relay_use(&mut *self.db.acquire().await?, now_ms).await?;
        Ok(())
    }
}
