//! The live state in the store: sign-in challenges, event connections, presence, sessions,
//! invitations (`docs/adr/0005-shared-state-lives-in-the-store.md`).
//!
//! Every node reads and writes the same rows, so any node can answer any request and a node can
//! stop without taking state with it. A change to a session starts by touching its row
//! ([`lock_session`]), which holds a lock in both databases until the transaction ends, so the
//! checks that follow read a state nobody else is changing. Sweeps are conditional statements
//! that every node runs; each step takes effect once, and the node whose statement changed rows
//! sends the news. Deadlines are Unix milliseconds. Like [`crate::db`], every query is written
//! once with `$1` parameters for SQLite and Postgres.

use std::collections::HashMap;

use anyplay_api::{
    Account, AccountId, Activity, Invitation, Member, Presence, Session, SessionRef, Visibility,
};
use sqlx::{AnyConnection, FromRow};

use crate::util::{ct_eq, sha256};

/// A connection of the store's pool or of a transaction (`&mut *tx`).
pub type Conn = AnyConnection;

fn i(id: u64) -> i64 {
    id as i64
}

fn u(id: i64) -> u64 {
    id as u64
}

// Sign-in challenges and sign-ups

/// Challenges and sign-ups waiting, to cap what a flood can make the store hold.
pub async fn count_pending(conn: &mut Conn) -> sqlx::Result<i64> {
    let (n,): (i64,) =
        sqlx::query_as("SELECT (SELECT COUNT(*) FROM challenges) + (SELECT COUNT(*) FROM signups)")
            .fetch_one(conn)
            .await?;
    Ok(n)
}

pub async fn insert_challenge(
    conn: &mut Conn,
    challenge: &str,
    email: &str,
    code: &str,
    expires_ms: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO challenges (id_hash, email, code_hash, tries, expires_ms) \
         VALUES ($1, $2, $3, 0, $4)",
    )
    .bind(&sha256(challenge.as_bytes())[..])
    .bind(email)
    .bind(&sha256(code.as_bytes())[..])
    .bind(expires_ms)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn delete_challenge(conn: &mut Conn, challenge: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM challenges WHERE id_hash = $1")
        .bind(&sha256(challenge.as_bytes())[..])
        .execute(conn)
        .await?;
    Ok(())
}

/// What a code typed for a challenge turned out to be.
#[derive(Debug, PartialEq, Eq)]
pub enum CodeCheck {
    /// No such challenge, or its tries are used up, or another request used it.
    Unknown,
    Expired,
    Wrong,
    /// The code matched; the challenge is used up.
    Right {
        email: String,
    },
}

/// Checks a code. Each attempt counts before the comparison, in one statement, so parallel
/// guesses cannot get past `max_tries` between them.
pub async fn check_code(
    conn: &mut Conn,
    challenge: &str,
    code: &str,
    max_tries: u8,
    now_ms: i64,
) -> sqlx::Result<CodeCheck> {
    let key = sha256(challenge.as_bytes());
    let row: Option<(String, Vec<u8>, i64, i64)> = sqlx::query_as(
        "UPDATE challenges SET tries = tries + 1 WHERE id_hash = $1 AND tries < $2 \
         RETURNING email, code_hash, expires_ms, tries",
    )
    .bind(&key[..])
    .bind(i64::from(max_tries))
    .fetch_optional(&mut *conn)
    .await?;
    let Some((email, code_hash, expires_ms, tries)) = row else {
        return Ok(CodeCheck::Unknown);
    };
    let delete = "DELETE FROM challenges WHERE id_hash = $1 RETURNING email";
    if expires_ms <= now_ms {
        sqlx::query(delete).bind(&key[..]).execute(conn).await?;
        return Ok(CodeCheck::Expired);
    }
    if !ct_eq(&code_hash, &sha256(code.trim().as_bytes())) {
        if tries >= i64::from(max_tries) {
            sqlx::query(delete).bind(&key[..]).execute(conn).await?;
        }
        return Ok(CodeCheck::Wrong);
    }
    // Only the request that deletes the row may use it.
    let used: Option<(String,)> = sqlx::query_as(delete)
        .bind(&key[..])
        .fetch_optional(conn)
        .await?;
    Ok(match used {
        Some(_) => CodeCheck::Right { email },
        None => CodeCheck::Unknown,
    })
}

pub async fn insert_signup(
    conn: &mut Conn,
    signup: &str,
    email: &str,
    expires_ms: i64,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO signups (id_hash, email, expires_ms) VALUES ($1, $2, $3)")
        .bind(&sha256(signup.as_bytes())[..])
        .bind(email)
        .bind(expires_ms)
        .execute(conn)
        .await?;
    Ok(())
}

/// The address a sign-up was made for, while it is valid.
pub async fn signup_email(
    conn: &mut Conn,
    signup: &str,
    now_ms: i64,
) -> sqlx::Result<Option<String>> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT email FROM signups WHERE id_hash = $1 AND expires_ms > $2")
            .bind(&sha256(signup.as_bytes())[..])
            .bind(now_ms)
            .fetch_optional(conn)
            .await?;
    Ok(row.map(|(email,)| email))
}

pub async fn delete_signup(conn: &mut Conn, signup: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM signups WHERE id_hash = $1")
        .bind(&sha256(signup.as_bytes())[..])
        .execute(conn)
        .await?;
    Ok(())
}

// Nodes and event connections

pub async fn heartbeat(conn: &mut Conn, node: &str, now_ms: i64) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO nodes (id, heartbeat_ms) VALUES ($1, $2) \
         ON CONFLICT (id) DO UPDATE SET heartbeat_ms = excluded.heartbeat_ms",
    )
    .bind(node)
    .bind(now_ms)
    .execute(conn)
    .await?;
    Ok(())
}

/// Records the event connection of a device on a node and marks the account as connected.
pub async fn open_connection(
    conn: &mut Conn,
    account: u64,
    device: u64,
    node: &str,
    serial: u64,
    now_ms: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO connections (device_id, account_id, node, serial, opened_ms) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (device_id) DO UPDATE SET account_id = excluded.account_id, \
         node = excluded.node, serial = excluded.serial, opened_ms = excluded.opened_ms",
    )
    .bind(i(device))
    .bind(i(account))
    .bind(node)
    .bind(i(serial))
    .bind(now_ms)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO presence (account_id, online, activity, last_seen_ms, sent, revision) \
         VALUES ($1, 1, NULL, NULL, $2, 0) \
         ON CONFLICT (account_id) DO UPDATE SET online = 1, last_seen_ms = NULL",
    )
    .bind(i(account))
    .bind(offline_json())
    .execute(conn)
    .await?;
    Ok(())
}

/// Forgets a connection that ended. When it was the account's last one anywhere, the grace
/// periods start from now. False when a newer connection of the device replaced it already.
pub async fn close_connection(
    conn: &mut Conn,
    account: u64,
    device: u64,
    node: &str,
    serial: u64,
    now_ms: i64,
) -> sqlx::Result<bool> {
    let closed =
        sqlx::query("DELETE FROM connections WHERE device_id = $1 AND node = $2 AND serial = $3")
            .bind(i(device))
            .bind(node)
            .bind(i(serial))
            .execute(&mut *conn)
            .await?
            .rows_affected()
            == 1;
    if closed {
        last_seen_if_gone(conn, account, now_ms).await?;
    }
    Ok(closed)
}

/// Starts the grace periods of an account that holds no connection any more.
async fn last_seen_if_gone(conn: &mut Conn, account: u64, now_ms: i64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE presence SET last_seen_ms = $2 WHERE account_id = $1 \
         AND NOT EXISTS (SELECT 1 FROM connections c WHERE c.account_id = $1)",
    )
    .bind(i(account))
    .bind(now_ms)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn is_connected(conn: &mut Conn, account: u64) -> sqlx::Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM connections WHERE account_id = $1")
        .bind(i(account))
        .fetch_optional(conn)
        .await?;
    Ok(row.is_some())
}

/// Removes the connections of nodes whose heartbeat is older than `cut_ms`, as if they had
/// closed then, and forgets those nodes.
pub async fn sweep_dead_nodes(conn: &mut Conn, cut_ms: i64) -> sqlx::Result<()> {
    let dead: Vec<(String, i64)> =
        sqlx::query_as("SELECT id, heartbeat_ms FROM nodes WHERE heartbeat_ms < $1")
            .bind(cut_ms)
            .fetch_all(&mut *conn)
            .await?;
    for (node, heartbeat_ms) in dead {
        let gone: Vec<(i64,)> =
            sqlx::query_as("DELETE FROM connections WHERE node = $1 RETURNING account_id")
                .bind(&node)
                .fetch_all(&mut *conn)
                .await?;
        for (account,) in gone {
            last_seen_if_gone(conn, u(account), heartbeat_ms).await?;
        }
        sqlx::query("DELETE FROM nodes WHERE id = $1 AND heartbeat_ms < $2")
            .bind(&node)
            .bind(cut_ms)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

// Presence

fn offline_json() -> String {
    serde_json::to_string(&Presence::default()).unwrap_or_default()
}

/// Makes sure an account in a session without a connection leaves it in time: its grace periods
/// start now unless they are running already.
pub async fn arm_grace(conn: &mut Conn, account: u64, now_ms: i64) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO presence (account_id, online, activity, last_seen_ms, sent, revision) \
         VALUES ($1, 0, NULL, $2, $3, 0) ON CONFLICT (account_id) DO NOTHING",
    )
    .bind(i(account))
    .bind(now_ms)
    .bind(offline_json())
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE presence SET last_seen_ms = $2 WHERE account_id = $1 AND last_seen_ms IS NULL \
         AND NOT EXISTS (SELECT 1 FROM connections c WHERE c.account_id = $1)",
    )
    .bind(i(account))
    .bind(now_ms)
    .execute(conn)
    .await?;
    Ok(())
}

/// What the account plays, as friends will see it. The cloud fills in the session.
pub async fn set_activity(
    conn: &mut Conn,
    account: u64,
    activity: Option<&Activity>,
) -> sqlx::Result<()> {
    let activity = activity.and_then(|a| serde_json::to_string(a).ok());
    sqlx::query("UPDATE presence SET activity = $2 WHERE account_id = $1")
        .bind(i(account))
        .bind(activity)
        .execute(conn)
        .await?;
    Ok(())
}

#[derive(FromRow)]
struct PresenceRow {
    account_id: i64,
    online: i64,
    activity: Option<String>,
    revision: i64,
}

#[derive(FromRow)]
struct JoinableRow {
    account_id: i64,
    id: String,
    title_id: String,
    max_members: i64,
    members: i64,
}

fn presence(row: &PresenceRow, joinable: &[JoinableRow]) -> Presence {
    let revision = u(row.revision);
    if row.online == 0 {
        return Presence {
            revision,
            ..Presence::default()
        };
    }
    let activity = row
        .activity
        .as_deref()
        .and_then(|text| serde_json::from_str::<Activity>(text).ok())
        .map(|mut activity| {
            // The rows come oldest first; the oldest joinable session of the title is the one.
            activity.session = joinable
                .iter()
                .find(|s| s.account_id == row.account_id && s.title_id == activity.title_id)
                .map(|s| SessionRef {
                    id: s.id.clone(),
                    joinable: s.members < s.max_members,
                    members: s.members as u16,
                    max_members: s.max_members as u16,
                });
            activity
        });
    Presence {
        online: true,
        activity,
        revision,
    }
}

const JOINABLE: &str = "SELECT m.account_id AS account_id, s.id AS id, s.title_id AS title_id, \
     s.max_members AS max_members, \
     (SELECT COUNT(*) FROM members x WHERE x.session_id = s.id) AS members \
     FROM members m JOIN sessions s ON s.id = m.session_id \
     WHERE s.visibility = 'friends' AND m.account_id";

/// What the friends of an account see of it now, with the revision they were last told.
pub async fn presence_of(conn: &mut Conn, account: u64) -> sqlx::Result<Presence> {
    let row: Option<PresenceRow> = sqlx::query_as(
        "SELECT account_id, online, activity, revision FROM presence WHERE account_id = $1",
    )
    .bind(i(account))
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Ok(Presence::default());
    };
    if row.online == 0 {
        return Ok(presence(&row, &[]));
    }
    let joinable: Vec<JoinableRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{JOINABLE} = $1 ORDER BY s.created, s.id"
    )))
    .bind(i(account))
    .fetch_all(conn)
    .await?;
    Ok(presence(&row, &joinable))
}

/// The presence of every friend of an account, in two queries however many friends it has.
pub async fn presence_of_friends(
    conn: &mut Conn,
    account: u64,
) -> sqlx::Result<HashMap<u64, Presence>> {
    let friend = "(SELECT CASE WHEN f.low = $1 THEN f.high ELSE f.low END FROM friends f \
                  WHERE f.low = $1 OR f.high = $1)";
    let rows: Vec<PresenceRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT account_id, online, activity, revision FROM presence WHERE account_id IN {friend}"
    )))
    .bind(i(account))
    .fetch_all(&mut *conn)
    .await?;
    let joinable: Vec<JoinableRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{JOINABLE} IN {friend} ORDER BY s.created, s.id"
    )))
    .bind(i(account))
    .fetch_all(conn)
    .await?;
    Ok(rows
        .iter()
        .map(|row| (u(row.account_id), presence(row, &joinable)))
        .collect())
}

/// Locks the presence row of an account until the transaction ends, and says whether there is
/// one. PostgreSQL only: on SQLite the write transaction holds the whole store already.
pub async fn lock_presence(conn: &mut Conn, account: u64) -> sqlx::Result<bool> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT account_id FROM presence WHERE account_id = $1 FOR UPDATE")
            .bind(i(account))
            .fetch_optional(conn)
            .await?;
    Ok(row.is_some())
}

/// Records that friends are told about this presence now, and returns the revision to tell them
/// with. `None` when they were told it already, by this node or another: then nobody sends it
/// again.
pub async fn mark_sent(
    conn: &mut Conn,
    account: u64,
    presence: &Presence,
) -> sqlx::Result<Option<u64>> {
    // What friends saw, without the revision it was told with.
    let text = serde_json::to_string(&Presence {
        revision: 0,
        ..presence.clone()
    })
    .unwrap_or_default();
    let revision: Option<(i64,)> = sqlx::query_as(
        "UPDATE presence SET sent = $2, revision = revision + 1 \
         WHERE account_id = $1 AND sent <> $2 RETURNING revision",
    )
    .bind(i(account))
    .bind(text)
    .fetch_optional(conn)
    .await?;
    Ok(revision.map(|(r,)| u(r)))
}

/// Takes accounts offline whose grace period ended before `cut_ms`. Returns them.
pub async fn sweep_offline(conn: &mut Conn, cut_ms: i64) -> sqlx::Result<Vec<u64>> {
    let gone: Vec<(i64,)> = sqlx::query_as(
        "UPDATE presence SET online = 0, activity = NULL \
         WHERE online = 1 AND last_seen_ms IS NOT NULL AND last_seen_ms <= $1 \
         AND NOT EXISTS (SELECT 1 FROM connections c WHERE c.account_id = presence.account_id) \
         RETURNING account_id",
    )
    .bind(cut_ms)
    .fetch_all(&mut *conn)
    .await?;
    Ok(gone.into_iter().map(|(a,)| u(a)).collect())
}

/// Takes an account offline now, as for a ban.
pub async fn set_offline(conn: &mut Conn, account: u64, now_ms: i64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE presence SET online = 0, activity = NULL, last_seen_ms = $2 WHERE account_id = $1",
    )
    .bind(i(account))
    .bind(now_ms)
    .execute(conn)
    .await?;
    Ok(())
}

/// Accounts in a session whose session grace ended before `cut_ms` without a connection.
pub async fn due_to_leave(conn: &mut Conn, cut_ms: i64) -> sqlx::Result<Vec<u64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT DISTINCT m.account_id FROM members m \
         JOIN presence p ON p.account_id = m.account_id \
         WHERE p.last_seen_ms IS NOT NULL AND p.last_seen_ms <= $1 \
         AND NOT EXISTS (SELECT 1 FROM connections c WHERE c.account_id = m.account_id)",
    )
    .bind(cut_ms)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|(a,)| u(a)).collect())
}

// Sessions

#[derive(Clone, Debug, FromRow)]
pub struct SessionRow {
    pub id: String,
    pub title_id: String,
    pub title: String,
    pub version: String,
    pub build: String,
    pub leader: i64,
    pub max_members: i64,
    pub visibility: String,
    pub properties: String,
    pub created: i64,
    /// Rises with every change; 0 for a new session.
    pub revision: i64,
}

#[derive(Clone, Debug, FromRow)]
pub struct MemberRow {
    pub account_id: i64,
    pub handle: String,
    pub endpoint: String,
    pub relay: Option<String>,
    pub address: String,
    pub joined: i64,
}

/// A session and its members, longest-standing first.
#[derive(Clone, Debug)]
pub struct SessionState {
    pub row: SessionRow,
    pub members: Vec<MemberRow>,
}

impl SessionState {
    pub fn leader(&self) -> u64 {
        u(self.row.leader)
    }

    pub fn max_members(&self) -> usize {
        self.row.max_members as usize
    }

    pub fn visibility(&self) -> Visibility {
        visibility_from(&self.row.visibility)
    }

    pub fn member_ids(&self) -> Vec<u64> {
        self.members.iter().map(|m| u(m.account_id)).collect()
    }

    pub fn is_member(&self, account: u64) -> bool {
        self.members.iter().any(|m| u(m.account_id) == account)
    }

    pub fn view(&self) -> Session {
        Session {
            id: self.row.id.clone(),
            title_id: self.row.title_id.clone(),
            title: self.row.title.clone(),
            version: self.row.version.clone(),
            build: self.row.build.clone(),
            leader: AccountId(self.leader()),
            max_members: self.row.max_members as u16,
            visibility: self.visibility(),
            properties: self.row.properties.clone(),
            members: self
                .members
                .iter()
                .map(|m| Member {
                    account: Account {
                        id: AccountId(u(m.account_id)),
                        handle: m.handle.clone(),
                    },
                    endpoint: m.endpoint.clone(),
                    relay: m.relay.clone(),
                    address: m.address.clone(),
                    joined: u(m.joined),
                })
                .collect(),
            created: u(self.row.created),
            revision: u(self.row.revision),
        }
    }
}

pub fn visibility_text(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Friends => "friends",
        Visibility::Invite => "invite",
        Visibility::Closed => "closed",
    }
}

fn visibility_from(text: &str) -> Visibility {
    match text {
        "friends" => Visibility::Friends,
        "invite" => Visibility::Invite,
        _ => Visibility::Closed,
    }
}

const SESSION_COLUMNS: &str = "id, title_id, title, version, build, leader, max_members, \
     visibility, properties, created, revision";

async fn members_of(conn: &mut Conn, id: &str) -> sqlx::Result<Vec<MemberRow>> {
    sqlx::query_as(
        "SELECT account_id, handle, endpoint, relay, address, joined FROM members \
         WHERE session_id = $1 ORDER BY seq",
    )
    .bind(id)
    .fetch_all(conn)
    .await
}

/// A session as it is now, for reading only.
pub async fn session(conn: &mut Conn, id: &str) -> sqlx::Result<Option<SessionState>> {
    let row: Option<SessionRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SESSION_COLUMNS} FROM sessions WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let members = members_of(conn, id).await?;
    Ok(Some(SessionState { row, members }))
}

/// Starts a change to a session inside a transaction: touches its row, which keeps every other
/// change to it waiting until this transaction ends, and reads it.
pub async fn lock_session(conn: &mut Conn, id: &str) -> sqlx::Result<Option<SessionState>> {
    let row: Option<SessionRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE sessions SET revision = revision + 1 WHERE id = $1 RETURNING {SESSION_COLUMNS}"
    )))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let members = members_of(conn, id).await?;
    Ok(Some(SessionState { row, members }))
}

/// The sessions an account is in, oldest first.
pub async fn sessions_of(conn: &mut Conn, account: u64) -> sqlx::Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT s.id FROM members m JOIN sessions s ON s.id = m.session_id \
         WHERE m.account_id = $1 ORDER BY s.created, s.id",
    )
    .bind(i(account))
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub async fn count_sessions_of(conn: &mut Conn, account: u64) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM members WHERE account_id = $1")
        .bind(i(account))
        .fetch_one(conn)
        .await?;
    Ok(n)
}

pub async fn insert_session(conn: &mut Conn, row: &SessionRow) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO sessions (id, title_id, title, version, build, leader, max_members, \
         visibility, properties, created, revision) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 0)",
    )
    .bind(&row.id)
    .bind(&row.title_id)
    .bind(&row.title)
    .bind(&row.version)
    .bind(&row.build)
    .bind(row.leader)
    .bind(row.max_members)
    .bind(&row.visibility)
    .bind(&row.properties)
    .bind(row.created)
    .execute(conn)
    .await?;
    Ok(())
}

/// Adds a member after the longest-standing ones.
pub async fn add_member(conn: &mut Conn, session: &str, member: &MemberRow) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO members (session_id, account_id, seq, handle, endpoint, relay, address, joined) \
         VALUES ($1, $2, \
         (SELECT COALESCE(MAX(seq), 0) + 1 FROM members WHERE session_id = $1), \
         $3, $4, $5, $6, $7)",
    )
    .bind(session)
    .bind(member.account_id)
    .bind(&member.handle)
    .bind(&member.endpoint)
    .bind(&member.relay)
    .bind(&member.address)
    .bind(member.joined)
    .execute(conn)
    .await?;
    Ok(())
}

/// A member that joins again moves its endpoint and relay.
pub async fn move_member(
    conn: &mut Conn,
    session: &str,
    account: u64,
    endpoint: &str,
    relay: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE members SET endpoint = $3, relay = $4 WHERE session_id = $1 AND account_id = $2",
    )
    .bind(session)
    .bind(i(account))
    .bind(endpoint)
    .bind(relay)
    .execute(conn)
    .await?;
    Ok(())
}

/// Writes the changeable fields of a session back.
pub async fn update_session(conn: &mut Conn, row: &SessionRow) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE sessions SET leader = $2, max_members = $3, visibility = $4, properties = $5 \
         WHERE id = $1",
    )
    .bind(&row.id)
    .bind(row.leader)
    .bind(row.max_members)
    .bind(&row.visibility)
    .bind(&row.properties)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn delete_member(conn: &mut Conn, session: &str, account: u64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM members WHERE session_id = $1 AND account_id = $2")
        .bind(session)
        .bind(i(account))
        .execute(conn)
        .await?;
    Ok(())
}

/// Deletes an empty session. Its invitations go with it; take them first to tell their
/// recipients.
pub async fn delete_session(conn: &mut Conn, session: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(session)
        .execute(conn)
        .await?;
    Ok(())
}

// Invitations

#[derive(Clone, Debug, FromRow)]
pub struct InvitationRow {
    pub id: String,
    pub from_id: i64,
    pub from_handle: String,
    pub to_id: i64,
    pub session_id: String,
    pub title_id: String,
    pub title: String,
    pub version: String,
    pub expires: i64,
}

impl InvitationRow {
    pub fn to(&self) -> u64 {
        u(self.to_id)
    }

    pub fn view(&self) -> Invitation {
        Invitation {
            id: self.id.clone(),
            from: Account {
                id: AccountId(u(self.from_id)),
                handle: self.from_handle.clone(),
            },
            session: self.session_id.clone(),
            title_id: self.title_id.clone(),
            title: self.title.clone(),
            version: self.version.clone(),
            expires: u(self.expires),
        }
    }
}

/// An invitation that ended: its ID and the account that held it.
#[derive(Clone, Debug, FromRow)]
pub struct Gone {
    pub id: String,
    pub to_id: i64,
}

impl Gone {
    pub fn to(&self) -> u64 {
        u(self.to_id)
    }
}

pub async fn insert_invitation(
    conn: &mut Conn,
    invitation: &InvitationRow,
    deadline_ms: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO invitations (id, from_id, from_handle, to_id, session_id, title_id, title, \
         version, expires, deadline_ms) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(&invitation.id)
    .bind(invitation.from_id)
    .bind(&invitation.from_handle)
    .bind(invitation.to_id)
    .bind(&invitation.session_id)
    .bind(&invitation.title_id)
    .bind(&invitation.title)
    .bind(&invitation.version)
    .bind(invitation.expires)
    .bind(deadline_ms)
    .execute(conn)
    .await?;
    Ok(())
}

/// True when `to` holds an invitation to the session, and, with `id`, that very one.
pub async fn is_invited(
    conn: &mut Conn,
    to: u64,
    session: &str,
    id: Option<&str>,
) -> sqlx::Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM invitations WHERE to_id = $1 AND session_id = $2 \
         AND ($3 IS NULL OR id = $3)",
    )
    .bind(i(to))
    .bind(session)
    .bind(id)
    .fetch_optional(conn)
    .await?;
    Ok(row.is_some())
}

/// The invitation `to` holds to the session from `from`, if it has not run out at `now_ms`.
pub async fn invitation_from(
    conn: &mut Conn,
    to: u64,
    session: &str,
    from: u64,
    now_ms: i64,
) -> sqlx::Result<Option<InvitationRow>> {
    sqlx::query_as(
        "SELECT id, from_id, from_handle, to_id, session_id, title_id, title, version, expires          FROM invitations WHERE to_id = $1 AND session_id = $2 AND from_id = $3          AND deadline_ms > $4 ORDER BY deadline_ms DESC LIMIT 1",
    )
    .bind(i(to))
    .bind(session)
    .bind(i(from))
    .bind(now_ms)
    .fetch_optional(conn)
    .await
}

/// Open invitations of a session, and those an account holds.
pub async fn count_invitations(
    conn: &mut Conn,
    session: &str,
    to: u64,
) -> sqlx::Result<(i64, i64)> {
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM invitations WHERE session_id = $1), \
         (SELECT COUNT(*) FROM invitations WHERE to_id = $2)",
    )
    .bind(session)
    .bind(i(to))
    .fetch_one(conn)
    .await?;
    Ok(counts)
}

/// Removes the invitations `to` holds for a session: they are used up by joining it.
pub async fn take_used(conn: &mut Conn, to: u64, session: &str) -> sqlx::Result<Vec<Gone>> {
    sqlx::query_as(
        "DELETE FROM invitations WHERE to_id = $1 AND session_id = $2 RETURNING id, to_id",
    )
    .bind(i(to))
    .bind(session)
    .fetch_all(conn)
    .await
}

/// Removes every invitation of a session.
pub async fn take_of_session(conn: &mut Conn, session: &str) -> sqlx::Result<Vec<Gone>> {
    sqlx::query_as("DELETE FROM invitations WHERE session_id = $1 RETURNING id, to_id")
        .bind(session)
        .fetch_all(conn)
        .await
}

/// Removes the invitations between two accounts, in both directions.
pub async fn take_between(conn: &mut Conn, a: u64, b: u64) -> sqlx::Result<Vec<Gone>> {
    sqlx::query_as(
        "DELETE FROM invitations WHERE (from_id = $1 AND to_id = $2) OR (from_id = $2 AND to_id = $1) \
         RETURNING id, to_id",
    )
    .bind(i(a))
    .bind(i(b))
    .fetch_all(conn)
    .await
}

/// Removes every invitation an account sent or holds.
pub async fn take_of_account(conn: &mut Conn, account: u64) -> sqlx::Result<Vec<Gone>> {
    sqlx::query_as("DELETE FROM invitations WHERE from_id = $1 OR to_id = $1 RETURNING id, to_id")
        .bind(i(account))
        .fetch_all(conn)
        .await
}

/// Removes an invitation its recipient declines. Empty when it is not theirs.
pub async fn take_declined(conn: &mut Conn, id: &str, to: u64) -> sqlx::Result<Vec<Gone>> {
    sqlx::query_as("DELETE FROM invitations WHERE id = $1 AND to_id = $2 RETURNING id, to_id")
        .bind(id)
        .bind(i(to))
        .fetch_all(conn)
        .await
}

// Sweeps

/// Removes what ran out: invitations (returned, so their recipients hear of it), challenges,
/// and sign-ups.
pub async fn sweep_expired(conn: &mut Conn, now_ms: i64) -> sqlx::Result<Vec<Gone>> {
    let gone: Vec<Gone> =
        sqlx::query_as("DELETE FROM invitations WHERE deadline_ms <= $1 RETURNING id, to_id")
            .bind(now_ms)
            .fetch_all(&mut *conn)
            .await?;
    // Join codes keep Unix seconds, like the times the API shows.
    crate::joins::sweep_expired(&mut *conn, (now_ms / 1000) as u64).await?;
    for table in [
        "challenges",
        "signups",
        "web_sessions",
        "web_flows",
        "proof_nonces",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE expires_ms <= $1"
        )))
        .bind(now_ms)
        .execute(&mut *conn)
        .await?;
    }
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, Location, Pool};

    const LATER: i64 = 600_000;

    /// A store in a temporary SQLite file with `accounts` accounts, 1 to n.
    async fn store(accounts: u64) -> (Pool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(Location::File(&dir.path().join("live.db")), 4)
            .await
            .unwrap();
        for id in 1..=accounts {
            db::insert_account(
                &pool,
                id,
                &format!("p{id}"),
                &format!("{id}@example.com"),
                1,
            )
            .await
            .unwrap();
        }
        (pool, dir)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parallel_guesses_get_no_more_tries_than_one_after_another() {
        let (pool, _dir) = store(0).await;
        insert_challenge(
            &mut pool.acquire().await.unwrap(),
            "c",
            "a@example.com",
            "123456",
            LATER,
        )
        .await
        .unwrap();
        // Collected first, so that all twenty run at once.
        let guesses: Vec<_> = (0..20)
            .map(|n| {
                let pool = pool.clone();
                tokio::spawn(async move {
                    let mut conn = pool.acquire().await.unwrap();
                    check_code(&mut conn, "c", &format!("{n:06}"), 5, 0)
                        .await
                        .unwrap()
                })
            })
            .collect();
        let mut wrong = 0;
        for guess in guesses {
            match guess.await.unwrap() {
                CodeCheck::Wrong => wrong += 1,
                CodeCheck::Unknown => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(wrong, 5, "five tries, however the guesses interleave");
        let late = check_code(&mut pool.acquire().await.unwrap(), "c", "123456", 5, 0)
            .await
            .unwrap();
        assert_eq!(late, CodeCheck::Unknown, "the challenge is used up");
    }

    #[tokio::test]
    async fn a_right_code_signs_in_once() {
        let (pool, _dir) = store(0).await;
        let mut conn = pool.acquire().await.unwrap();
        insert_challenge(&mut conn, "c", "a@example.com", "123456", LATER)
            .await
            .unwrap();
        assert_eq!(
            check_code(&mut conn, "c", " 123456 ", 5, 0).await.unwrap(),
            CodeCheck::Right {
                email: "a@example.com".into()
            }
        );
        assert_eq!(
            check_code(&mut conn, "c", "123456", 5, 0).await.unwrap(),
            CodeCheck::Unknown
        );
        insert_challenge(&mut conn, "old", "a@example.com", "123456", 10)
            .await
            .unwrap();
        assert_eq!(
            check_code(&mut conn, "old", "123456", 5, 20).await.unwrap(),
            CodeCheck::Expired
        );
    }

    #[tokio::test]
    async fn the_connections_of_a_dead_node_close_and_its_accounts_go_offline_after_the_grace() {
        let (pool, _dir) = store(1).await;
        db::insert_device(&pool, 9, 1, "pc", "linux", b"refresh", 1)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        heartbeat(&mut conn, "dead", 1_000).await.unwrap();
        open_connection(&mut conn, 1, 9, "dead", 1, 1_000)
            .await
            .unwrap();
        // Before the node counts as dead, nothing happens.
        sweep_dead_nodes(&mut conn, 500).await.unwrap();
        assert!(is_connected(&mut conn, 1).await.unwrap());
        sweep_dead_nodes(&mut conn, 2_000).await.unwrap();
        assert!(!is_connected(&mut conn, 1).await.unwrap());
        // The grace runs from the node's last heartbeat.
        assert!(sweep_offline(&mut conn, 999).await.unwrap().is_empty());
        assert_eq!(sweep_offline(&mut conn, 1_000).await.unwrap(), [1]);
        assert_eq!(
            presence_of(&mut conn, 1).await.unwrap(),
            Presence::default()
        );
    }
}
