//! Device binding: a device makes a key at sign-in, the cloud keeps its public half, and the
//! requests that matter carry a proof signed by it (network ADR 0016, `spec/cloud-api.md`
//! "Device keys and proofs").
//!
//! A refresh token of a bound device is useless without the key, and a proof is accepted once:
//! its nonce is remembered in the store until it could no longer pass the time window, so every
//! node refuses a replay.

use anyplay_proof::{Checked, ProofError, Subject};
use axum::http::HeaderMap;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::db::Pool;
use crate::error::AppError;
use crate::state::AppState;
use crate::util::{now, sha256};

/// What the store knows of a device's key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// The public key, or `None` for a legacy device.
    pub key: Option<Vec<u8>>,
    /// Until when a device without a key may refresh without a proof (Unix seconds).
    pub legacy_until: Option<u64>,
}

impl Binding {
    /// The key as the proof code reads it.
    pub fn public(&self) -> Option<String> {
        self.key.as_deref().map(|k| URL_SAFE_NO_PAD.encode(k))
    }

    /// True while a device without a key may still refresh without a proof.
    pub fn legacy_open(&self, at: u64) -> bool {
        self.key.is_none() && self.legacy_until.is_some_and(|until| at < until)
    }
}

/// Decodes a device public key from the API. `None` when it is not a key.
pub fn decode_key(text: &str) -> Option<Vec<u8>> {
    if !anyplay_proof::public_key_ok(text) {
        return None;
    }
    URL_SAFE_NO_PAD.decode(text).ok()
}

pub async fn binding_of(pool: &Pool, device: u64) -> sqlx::Result<Option<Binding>> {
    let row: Option<(Option<Vec<u8>>, Option<i64>)> =
        sqlx::query_as("SELECT device_key, legacy_until FROM devices WHERE id = $1")
            .bind(device as i64)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(key, legacy_until)| Binding {
        key,
        legacy_until: legacy_until.map(|t| t as u64),
    }))
}

/// Refuses the account's access tokens issued before `at`.
pub async fn set_tokens_valid_from(pool: &Pool, account: u64, at: u64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE accounts SET tokens_valid_from = $1 WHERE id = $2 AND tokens_valid_from < $1",
    )
    .bind(at as i64)
    .bind(account as i64)
    .execute(pool)
    .await?;
    Ok(())
}

/// The accounts that signed out everywhere, with the time.
pub async fn all_tokens_valid_from(pool: &Pool) -> sqlx::Result<Vec<(u64, u64)>> {
    let rows: Vec<(i64, i64)> =
        sqlx::query_as("SELECT id, tokens_valid_from FROM accounts WHERE tokens_valid_from > 0")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(a, t)| (a as u64, t as u64))
        .collect())
}

/// Inserts a device with its key, or, for a legacy device, with the end of its grace period.
#[allow(clippy::too_many_arguments)]
pub async fn insert_device(
    pool: &Pool,
    id: u64,
    account: u64,
    name: &str,
    platform: &str,
    refresh_hash: &[u8],
    created: u64,
    key: Option<&[u8]>,
    legacy_until: Option<u64>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO devices \
         (id, account_id, name, platform, refresh_hash, created, last_used, device_key, legacy_until) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id as i64)
    .bind(account as i64)
    .bind(name)
    .bind(platform)
    .bind(refresh_hash)
    .bind(created as i64)
    .bind(created as i64)
    .bind(key)
    .bind(legacy_until.map(|t| t as i64))
    .execute(pool)
    .await?;
    Ok(())
}

/// Binds a legacy device to `key`. False when it already has one (another request got there
/// first) or does not exist.
pub async fn bind(pool: &Pool, device: u64, key: &[u8]) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE devices SET device_key = $1, legacy_until = NULL \
         WHERE id = $2 AND device_key IS NULL",
    )
    .bind(key)
    .bind(device as i64)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Remembers a proof's nonce. False when it was seen before.
pub async fn remember_nonce(
    pool: &Pool,
    device: u64,
    nonce: &str,
    expires_ms: i64,
) -> sqlx::Result<bool> {
    let mut material = device.to_le_bytes().to_vec();
    material.extend_from_slice(nonce.as_bytes());
    let result = sqlx::query(
        "INSERT INTO proof_nonces (hash, expires_ms) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(&sha256(&material)[..])
    .bind(expires_ms)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Why a request's proof was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No proof, or one that is not in the right shape.
    Missing,
    /// The signature does not fit the device's key, the request or the token.
    Invalid,
    /// The time in the proof is off by more than a minute.
    Clock,
    /// The proof was used before.
    Replay,
}

impl Refusal {
    pub fn error(self) -> AppError {
        match self {
            Refusal::Clock => AppError::bad_request(
                "The clock of this device differs from the cloud's by more than a minute.",
            ),
            Refusal::Missing | Refusal::Invalid | Refusal::Replay => AppError::unauthorized(
                "This request needs a proof from the device's key. Sign in again.",
            ),
        }
    }
}

/// Checks the proof in `headers` against a device's public key and remembers its nonce.
pub async fn check_proof(
    state: &AppState,
    device: u64,
    public: &str,
    headers: &HeaderMap,
    subject: &Subject<'_>,
) -> Result<(), Refusal> {
    let header = headers
        .get(anyplay_proof::HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(Refusal::Missing)?;
    let Checked { nonce, expires } = anyplay_proof::verify(public, header, subject, now())
        .map_err(|error| match error {
            ProofError::Malformed => Refusal::Missing,
            ProofError::Time => Refusal::Clock,
            ProofError::Signature => Refusal::Invalid,
        })?;
    match remember_nonce(&state.db, device, &nonce, (expires as i64 + 60) * 1000).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(Refusal::Replay),
        // A store that cannot take a nonce cannot promise "once": refuse.
        Err(_) => Err(Refusal::Invalid),
    }
}

/// For a request of a signed-in device that must prove it holds the device key (such as
/// asking for a link to the account web): checks the proof for this request, made over the
/// access token it carries.
pub async fn require_proof(
    state: &AppState,
    device: u64,
    headers: &HeaderMap,
    method: &str,
    path: &str,
) -> Result<(), AppError> {
    let binding = binding_of(&state.db, device)
        .await?
        .ok_or_else(|| AppError::unauthorized("This device is signed out. Sign in again."))?;
    let Some(public) = binding.public() else {
        return Err(AppError::unauthorized(
            "This device has no device key. Sign in again.",
        ));
    };
    let token = crate::http::bearer(headers).unwrap_or_default();
    check_proof(
        state,
        device,
        &public,
        headers,
        &Subject {
            method,
            path,
            credential: token,
        },
    )
    .await
    .map_err(Refusal::error)
}
