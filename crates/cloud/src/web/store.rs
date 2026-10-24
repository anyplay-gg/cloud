//! What the account web keeps in the store: browser sessions, one-time flows and passkeys.
//!
//! Like [`crate::db`], every query is written once with `$1` parameters for SQLite and
//! PostgreSQL. Cookie values, flow IDs and codes are random; only their SHA-256 is stored, so a
//! copy of the database is no way into a session.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::db::Pool;
use crate::util::{now_ms, random_token, sha256};

/// The request a native client started at `/web/authorize`, kept while the player signs in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authorize {
    /// A loopback address; checked when the request came in.
    pub redirect_uri: String,
    pub state: String,
    /// The PKCE challenge (S256).
    pub challenge: String,
    /// The player wants to create an account rather than sign in.
    pub create: bool,
    /// The name of the device that signs in, as it called itself. Shown, never trusted.
    pub device: String,
}

/// Where a sign-in by e-mail code stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Login {
    /// A code is out; the player has to type it.
    Code {
        challenge: String,
        email: String,
        create: bool,
    },
    /// The address is verified and has no account yet; the player chooses a handle.
    NewAccount { signup: String, email: String },
    /// The player is known and has to accept the terms before going on.
    Terms { account: u64 },
}

/// The state of a session that is not an account's: what the player is in the middle of.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorize: Option<Authorize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<Login>,
    /// A line to show once on the next page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    /// The account whose ban the player has proved to be theirs (by code or passkey), so that
    /// the statement shows and an appeal can be filed for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banned: Option<u64>,
    /// The address of the app on the player PC, with its one-time code, to send the browser to
    /// once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<String>,
    /// When the player last proved who they are again in the middle of a session (Unix
    /// milliseconds). A session counts as fresh after that as well as after its sign-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_ms: Option<i64>,
    /// The challenge of a code sent to the account's own address, to prove the player is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_up: Option<String>,
    /// A new address waiting for its code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_change: Option<EmailChange>,
}

/// A change of the account's e-mail address that waits for the code sent to the new address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailChange {
    pub challenge: String,
    pub email: String,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub id_hash: Vec<u8>,
    /// The account that signed in, or `None` for a session of a sign-in in progress.
    pub account: Option<u64>,
    /// The token that forms and scripts of this session must send back.
    pub csrf: String,
    pub data: SessionData,
    pub created_ms: i64,
}

/// A browser session as its owner sees it in a list.
#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub id_hash: Vec<u8>,
    pub created_ms: i64,
    pub used_ms: i64,
}

impl Session {
    /// True when `sent` is this session's CSRF token.
    pub fn csrf_ok(&self, sent: &str) -> bool {
        crate::util::ct_eq(self.csrf.as_bytes(), sent.as_bytes())
    }

    /// True when the player signed in or proved who they are again during the last `fresh_for`.
    pub fn fresh(&self, fresh_for: Duration) -> bool {
        self.fresh_until(fresh_for)
            .is_some_and(|until| until > now_ms())
    }

    /// When the session stops being fresh, in Unix milliseconds. `None` without an account.
    pub fn fresh_until(&self, fresh_for: Duration) -> Option<i64> {
        self.account?;
        let last = self.data.verified_ms.unwrap_or(0).max(self.created_ms);
        Some(last + fresh_for.as_millis() as i64)
    }
}

fn hash(token: &str) -> Vec<u8> {
    sha256(token.as_bytes()).to_vec()
}

/// A new session. Returns the cookie value, which is not stored anywhere else.
pub async fn create_session(
    pool: &Pool,
    account: Option<u64>,
    data: &SessionData,
    idle: Duration,
) -> sqlx::Result<(String, Session)> {
    let token = random_token(32);
    let csrf = random_token(16);
    let created_ms = now_ms();
    let id_hash = hash(&token);
    sqlx::query(
        "INSERT INTO web_sessions (id_hash, account_id, csrf, data, created_ms, expires_ms, used_ms) \
         VALUES ($1, $2, $3, $4, $5, $6, $5)",
    )
    .bind(&id_hash[..])
    .bind(account.map(|a| a as i64))
    .bind(&csrf)
    .bind(serde_json::to_string(data).unwrap_or_default())
    .bind(created_ms)
    .bind(created_ms + idle.as_millis() as i64)
    .execute(pool)
    .await?;
    Ok((
        token,
        Session {
            id_hash,
            account,
            csrf,
            data: data.clone(),
            created_ms,
        },
    ))
}

/// The session a cookie value stands for, if it is still alive. Using it moves its end
/// `idle` ahead, but never past `max` after its start.
pub async fn load_session(
    pool: &Pool,
    token: &str,
    idle: Duration,
    max: Duration,
) -> sqlx::Result<Option<Session>> {
    if token.len() > 128 {
        return Ok(None);
    }
    let id_hash = hash(token);
    let now = now_ms();
    let row: Option<(Option<i64>, String, String, i64)> = sqlx::query_as(
        "SELECT account_id, csrf, data, created_ms FROM web_sessions \
         WHERE id_hash = $1 AND expires_ms > $2",
    )
    .bind(&id_hash[..])
    .bind(now)
    .fetch_optional(pool)
    .await?;
    let Some((account, csrf, data, created_ms)) = row else {
        return Ok(None);
    };
    let end = (now + idle.as_millis() as i64).min(created_ms + max.as_millis() as i64);
    if end <= now {
        destroy_session(pool, token).await?;
        return Ok(None);
    }
    sqlx::query("UPDATE web_sessions SET expires_ms = $1, used_ms = $3 WHERE id_hash = $2")
        .bind(end)
        .bind(&id_hash[..])
        .bind(now)
        .execute(pool)
        .await?;
    Ok(Some(Session {
        id_hash,
        account: account.map(|a| a as u64),
        csrf,
        data: serde_json::from_str(&data).unwrap_or_default(),
        created_ms,
    }))
}

pub async fn save_data(pool: &Pool, session: &Session) -> sqlx::Result<()> {
    sqlx::query("UPDATE web_sessions SET data = $1 WHERE id_hash = $2")
        .bind(serde_json::to_string(&session.data).unwrap_or_default())
        .bind(&session.id_hash[..])
        .execute(pool)
        .await?;
    Ok(())
}

/// Ends a session whose cookie value is not at hand.
pub async fn end_session(pool: &Pool, session: &Session) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM web_sessions WHERE id_hash = $1")
        .bind(&session.id_hash[..])
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn destroy_session(pool: &Pool, token: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM web_sessions WHERE id_hash = $1")
        .bind(&hash(token)[..])
        .execute(pool)
        .await?;
    Ok(())
}

/// Ends every browser session of an account.
pub async fn end_sessions_of(pool: &Pool, account: u64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM web_sessions WHERE account_id = $1")
        .bind(account as i64)
        .execute(pool)
        .await?;
    Ok(())
}

/// The live browser sessions of an account, the oldest first.
pub async fn sessions_of(pool: &Pool, account: u64) -> sqlx::Result<Vec<SessionInfo>> {
    let rows: Vec<(Vec<u8>, i64, Option<i64>)> = sqlx::query_as(
        "SELECT id_hash, created_ms, used_ms FROM web_sessions \
         WHERE account_id = $1 AND expires_ms > $2 ORDER BY created_ms, id_hash",
    )
    .bind(account as i64)
    .bind(now_ms())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id_hash, created_ms, used_ms)| SessionInfo {
            id_hash,
            created_ms,
            used_ms: used_ms.unwrap_or(created_ms),
        })
        .collect())
}

/// Ends one browser session of an account; true when it had it.
pub async fn end_session_of(pool: &Pool, account: u64, id_hash: &[u8]) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM web_sessions WHERE id_hash = $1 AND account_id = $2")
        .bind(id_hash)
        .bind(account as i64)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() == 1)
}

// One-time flows

/// Stores a value that can be taken once, for `ttl`. Returns its random ID.
pub async fn flow_put(pool: &Pool, kind: &str, data: &str, ttl: Duration) -> sqlx::Result<String> {
    let id = random_token(24);
    sqlx::query("INSERT INTO web_flows (id_hash, kind, data, expires_ms) VALUES ($1, $2, $3, $4)")
        .bind(&hash(&id)[..])
        .bind(kind)
        .bind(data)
        .bind(now_ms() + ttl.as_millis() as i64)
        .execute(pool)
        .await?;
    Ok(id)
}

/// Takes the value: it is gone afterwards, so a second try finds nothing. A deleted row is
/// returned only to the request whose statement deleted it, even when two race.
pub async fn flow_take(pool: &Pool, kind: &str, id: &str) -> sqlx::Result<Option<String>> {
    if id.len() > 128 {
        return Ok(None);
    }
    let row: Option<(String,)> = sqlx::query_as(
        "DELETE FROM web_flows WHERE id_hash = $1 AND kind = $2 AND expires_ms > $3 \
         RETURNING data",
    )
    .bind(&hash(id)[..])
    .bind(kind)
    .bind(now_ms())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(data,)| data))
}

/// Reads a value without taking it.
pub async fn flow_peek(pool: &Pool, kind: &str, id: &str) -> sqlx::Result<Option<String>> {
    if id.len() > 128 {
        return Ok(None);
    }
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT data FROM web_flows WHERE id_hash = $1 AND kind = $2 AND expires_ms > $3",
    )
    .bind(&hash(id)[..])
    .bind(kind)
    .bind(now_ms())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(data,)| data))
}

// Passkeys

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct PasskeyRow {
    pub id: Vec<u8>,
    pub account_id: i64,
    pub name: String,
    pub credential: String,
    pub created: i64,
    pub last_used: Option<i64>,
}

const PASSKEY_COLUMNS: &str = "id, account_id, name, credential, created, last_used";

/// Passkeys one account may have.
pub const MAX_PASSKEYS: usize = 10;

pub async fn passkeys_of(pool: &Pool, account: u64) -> sqlx::Result<Vec<PasskeyRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {PASSKEY_COLUMNS} FROM passkeys WHERE account_id = $1 ORDER BY created, id"
    )))
    .bind(account as i64)
    .fetch_all(pool)
    .await
}

pub async fn passkey_by_id(pool: &Pool, id: &[u8]) -> sqlx::Result<Option<PasskeyRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {PASSKEY_COLUMNS} FROM passkeys WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// False when a passkey with this credential ID exists already (for any account).
pub async fn insert_passkey(
    pool: &Pool,
    id: &[u8],
    account: u64,
    name: &str,
    credential: &str,
    created: u64,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "INSERT INTO passkeys (id, account_id, name, credential, created) \
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(account as i64)
    .bind(name)
    .bind(credential)
    .bind(created as i64)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Renames a passkey; true when the account had it.
pub async fn rename_passkey(
    pool: &Pool,
    account: u64,
    id: &[u8],
    name: &str,
) -> sqlx::Result<bool> {
    let result = sqlx::query("UPDATE passkeys SET name = $1 WHERE id = $2 AND account_id = $3")
        .bind(name)
        .bind(id)
        .bind(account as i64)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn update_passkey(
    pool: &Pool,
    id: &[u8],
    credential: &str,
    last_used: u64,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE passkeys SET credential = $1, last_used = $2 WHERE id = $3")
        .bind(credential)
        .bind(last_used as i64)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// True when the account had this passkey.
pub async fn delete_passkey(pool: &Pool, account: u64, id: &[u8]) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM passkeys WHERE id = $1 AND account_id = $2")
        .bind(id)
        .bind(account as i64)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() == 1)
}

// The age and terms record

/// The version of the terms the account accepted, when it did.
pub async fn terms_of(pool: &Pool, account: u64) -> sqlx::Result<Option<String>> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT terms_version FROM accounts WHERE id = $1")
            .bind(account as i64)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|(version,)| version))
}

pub async fn accept_terms(pool: &Pool, account: u64, version: &str, at: u64) -> sqlx::Result<()> {
    sqlx::query("UPDATE accounts SET terms_version = $1, terms_accepted_at = $2 WHERE id = $3")
        .bind(version)
        .bind(at as i64)
        .bind(account as i64)
        .execute(pool)
        .await?;
    Ok(())
}
