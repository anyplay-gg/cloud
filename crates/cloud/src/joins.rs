//! Join codes: a short random token that stands for an account and lets someone who is not a
//! friend ask to join the game that account offers (network `spec/cloud-api.md`, "Join codes",
//! and ADR 0022).
//!
//! The store holds a SHA-256 of the code and never the code, one row per account: issuing a new
//! code replaces the old. A code does not name a session or a party. What it opens is decided
//! when it is read or redeemed ([`offered`]): the session the account joined last whose
//! visibility is not `closed`, else the party it leads. Redeeming makes an ordinary invitation
//! ([`crate::routes::join_codes`]). Like [`crate::live`], every query is written once with `$1`
//! parameters for SQLite and PostgreSQL.

use anyplay_api::{JOIN_CODE_ALPHABET, JOIN_CODE_LEN};
use rand::TryRng as _;
use rand::rngs::SysRng;

use crate::error::AppError;
use crate::live::{self, Conn, SessionState};
use crate::party::{self, PartyState};
use crate::util::sha256;

fn i(id: u64) -> i64 {
    id as i64
}

fn u(id: i64) -> u64 {
    id as u64
}

/// A new code: [`JOIN_CODE_LEN`] characters of [`JOIN_CODE_ALPHABET`], drawn from the operating
/// system's random generator. The alphabet has 32 characters, so five bits of a random byte pick
/// one with no bias.
pub fn generate() -> Result<String, AppError> {
    let alphabet = JOIN_CODE_ALPHABET.as_bytes();
    debug_assert_eq!(alphabet.len(), 32);
    let mut bytes = [0u8; JOIN_CODE_LEN];
    SysRng.try_fill_bytes(&mut bytes).map_err(|error| {
        tracing::error!(%error, "the system's random generator failed");
        AppError::internal()
    })?;
    Ok(bytes
        .iter()
        .map(|b| char::from(alphabet[usize::from(b & 31)]))
        .collect())
}

/// The code in the form the store keys it by: lower case, and well formed. None for anything
/// that cannot be a code, which the API answers like an unknown one.
pub fn normalize(text: &str) -> Option<String> {
    let code = text.to_ascii_lowercase();
    (code.len() == JOIN_CODE_LEN && code.chars().all(|c| JOIN_CODE_ALPHABET.contains(c)))
        .then_some(code)
}

fn hash(code: &str) -> Vec<u8> {
    sha256(code.as_bytes()).to_vec()
}

// The store

/// Stores `code` for the account, replacing the one it had. `expires` is Unix seconds.
pub async fn replace(
    conn: &mut Conn,
    account: u64,
    code: &str,
    expires: u64,
    created: u64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO join_codes (account_id, code_hash, expires, created) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (account_id) DO UPDATE SET code_hash = excluded.code_hash, \
         expires = excluded.expires, created = excluded.created",
    )
    .bind(i(account))
    .bind(hash(code))
    .bind(i(expires))
    .bind(i(created))
    .execute(conn)
    .await?;
    Ok(())
}

/// Removes the account's code. Harmless when there is none.
pub async fn revoke(conn: &mut Conn, account: u64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM join_codes WHERE account_id = $1")
        .bind(i(account))
        .execute(conn)
        .await?;
    Ok(())
}

/// The account a code was issued to, if it has not run out at `now` (Unix seconds).
pub async fn owner(conn: &mut Conn, code: &str, now: u64) -> sqlx::Result<Option<u64>> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT account_id FROM join_codes WHERE code_hash = $1 AND expires > $2")
            .bind(hash(code))
            .bind(i(now))
            .fetch_optional(conn)
            .await?;
    Ok(row.map(|(account,)| u(account)))
}

/// Removes the codes that ran out at `now` (Unix seconds).
pub async fn sweep_expired(conn: &mut Conn, now: u64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM join_codes WHERE expires <= $1")
        .bind(i(now))
        .execute(conn)
        .await?;
    Ok(())
}

// What a code opens

/// What an account offers to join at this moment.
pub enum Offer {
    Session(SessionState),
    Party(PartyState),
}

/// The ID of the session the account joined last whose visibility is not `closed`.
pub async fn offered_session(conn: &mut Conn, account: u64) -> sqlx::Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT s.id FROM members m JOIN sessions s ON s.id = m.session_id \
         WHERE m.account_id = $1 AND s.visibility <> 'closed' \
         ORDER BY m.joined DESC, m.seq DESC, s.id LIMIT 1",
    )
    .bind(i(account))
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|(id,)| id))
}

/// The ID of the party the account leads.
pub async fn offered_party(conn: &mut Conn, account: u64) -> sqlx::Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT id FROM parties WHERE leader = $1")
        .bind(i(account))
        .fetch_optional(conn)
        .await?;
    Ok(row.map(|(id,)| id))
}

/// What the account offers now, read without locking: its session, else its party, else
/// nothing.
pub async fn offered(conn: &mut Conn, account: u64) -> sqlx::Result<Option<Offer>> {
    if let Some(id) = offered_session(conn, account).await?
        && let Some(session) = live::session(conn, &id).await?
    {
        return Ok(Some(Offer::Session(session)));
    }
    if let Some(id) = offered_party(conn, account).await?
        && let Some(party) = party::party(conn, &id).await?
    {
        return Ok(Some(Offer::Party(party)));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_has_ten_characters_of_the_alphabet() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let code = generate().unwrap();
            assert_eq!(code.len(), JOIN_CODE_LEN);
            assert!(code.chars().all(|c| JOIN_CODE_ALPHABET.contains(c)));
            seen.extend(code.chars());
        }
        // 2,000 draws from 32 characters reach nearly all of them.
        assert!(seen.len() > 28, "{} different characters", seen.len());
    }

    #[test]
    fn normalizing_ignores_case_and_refuses_what_is_no_code() {
        assert_eq!(normalize("K3X9M2PQRA").as_deref(), Some("k3x9m2pqra"));
        assert_eq!(normalize("k3x9m2pqr0"), None);
        assert_eq!(normalize("k3x9m2pqr"), None);
        assert_eq!(normalize("k3x9m2pqraa"), None);
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("k3x9m2pqr\u{e4}"), None);
    }
}
