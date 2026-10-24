//! Signing in: e-mail code, sign-up, refresh and sign-out.

use std::time::{Duration, Instant};

use anyplay_api::{
    CodeAnswer, CodeRequest, Device, EmailAnswer, EmailRequest, ErrorCode, HandleProblem, Me,
    Platform, RefreshRequest, SignedIn, SignupRequest, Tokens, check_handle,
};
use anyplay_ticket::Claims;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use crate::db::{self, AccountRow, InsertError};
use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth, ClientIp};
use crate::state::{AppState, Challenge, SignupToken};
use crate::util::{self, ct_eq, hex, normalize_email, now, random_id, random_token, sha256};

const CODE_LENGTH: u8 = 6;
/// Challenges and sign-ups a client can make us hold at once.
const MAX_PENDING: usize = 50_000;
/// Devices one account may have signed in. A new one replaces the one used least recently.
const MAX_DEVICES: i64 = 10;
const RESERVED: &[&str] = &[
    "anyplay",
    "admin",
    "administrator",
    "moderator",
    "mod",
    "staff",
    "support",
    "system",
    "root",
    "official",
];
/// Platform names are reserved as prefixes too, so "playstation_fan" is not available.
const RESERVED_PREFIXES: &[&str] = &[
    "sony",
    "playstation",
    "psn",
    "ps5",
    "ps4",
    "xbox",
    "nintendo",
    "steam",
];

/// True when nobody may take the handle.
pub fn is_reserved(handle: &str) -> bool {
    let lower = handle.to_ascii_lowercase();
    RESERVED.contains(&lower.as_str()) || RESERVED_PREFIXES.iter().any(|p| lower.starts_with(p))
}

fn platform_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Windows => "windows",
        Platform::Linux => "linux",
        Platform::Macos => "macos",
        Platform::Other => "other",
    }
}

fn check_device(device: &Device) -> Result<()> {
    if util::is_label(&device.name, 64) {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "The device name must have 1 to 64 printable characters.",
        ))
    }
}

pub async fn email(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(request): ApiJson<EmailRequest>,
) -> Result<Json<EmailAnswer>> {
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("email:ip:{ip}"),
            rates.email_per_ip,
            rates.auth_window,
        )
        .map_err(AppError::rate_limited)?;
    let email = normalize_email(&request.email)
        .ok_or_else(|| AppError::bad_request("That is not an e-mail address."))?;
    state
        .limiter
        .hit(
            &format!("email:address:{}", hex(&sha256(email.as_bytes()))),
            rates.email_per_address,
            rates.auth_window,
        )
        .map_err(AppError::rate_limited)?;

    let code = format!("{:06}", rand::random_range(0..1_000_000u32));
    let challenge = random_token(16);
    let ttl = state.tuning.code_ttl;
    {
        let mut pending = state.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.challenges.len() >= MAX_PENDING {
            return Err(AppError::rate_limited(60));
        }
        pending.challenges.insert(
            challenge.clone(),
            Challenge {
                email: email.clone(),
                code: code.clone(),
                tries: 0,
                deadline: Instant::now() + ttl,
            },
        );
    }
    // The same mail goes out whether or not the address has an account.
    let sent = tokio::time::timeout(
        Duration::from_secs(20),
        state.mailer.send_code(&email, &code),
    )
    .await;
    if !matches!(sent, Ok(Ok(()))) {
        tracing::error!("cannot send a sign-in code");
        let mut pending = state.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.challenges.remove(&challenge);
        return Err(AppError::internal());
    }
    Ok(Json(EmailAnswer {
        challenge,
        expires: now() + ttl.as_secs(),
        code_length: CODE_LENGTH,
    }))
}

pub async fn code(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(request): ApiJson<CodeRequest>,
) -> Result<Json<CodeAnswer>> {
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("code:ip:{ip}"),
            rates.code_per_ip,
            rates.auth_window,
        )
        .map_err(AppError::rate_limited)?;
    check_device(&request.device)?;

    let email = {
        let mut pending = state.pending.lock().unwrap_or_else(|e| e.into_inner());
        let Some(challenge) = pending.challenges.get_mut(&request.challenge) else {
            return Err(AppError::new(
                ErrorCode::CodeExpired,
                "This code is no longer valid. Ask for a new one.",
            ));
        };
        if challenge.deadline <= Instant::now() {
            pending.challenges.remove(&request.challenge);
            return Err(AppError::new(
                ErrorCode::CodeExpired,
                "This code has expired. Ask for a new one.",
            ));
        }
        if !ct_eq(challenge.code.as_bytes(), request.code.trim().as_bytes()) {
            challenge.tries += 1;
            if challenge.tries >= state.tuning.code_tries {
                pending.challenges.remove(&request.challenge);
            }
            return Err(AppError::new(ErrorCode::CodeWrong, "That code is wrong."));
        }
        let email = challenge.email.clone();
        pending.challenges.remove(&request.challenge);
        email
    };

    check_email_ban(&state, &email).await?;
    match db::account_by_email(&state.db, &email).await? {
        Some(account) => {
            if let Some(until) = state.ban_of(account.id()) {
                return Err(AppError::banned(until));
            }
            let signed_in = start_device(&state, &account, &request.device).await?;
            Ok(Json(CodeAnswer::SignedIn {
                tokens: signed_in.tokens,
                me: signed_in.me,
            }))
        }
        None => {
            if state.config.signup == anyplay_api::Signup::Closed {
                return Err(signups_closed());
            }
            let signup = random_token(24);
            let ttl = state.tuning.signup_ttl;
            let mut pending = state.pending.lock().unwrap_or_else(|e| e.into_inner());
            if pending.signups.len() >= MAX_PENDING {
                return Err(AppError::rate_limited(60));
            }
            pending.signups.insert(
                signup.clone(),
                SignupToken {
                    email,
                    deadline: Instant::now() + ttl,
                },
            );
            Ok(Json(CodeAnswer::NewAccount {
                signup,
                expires: now() + ttl.as_secs(),
            }))
        }
    }
}

fn signups_closed() -> AppError {
    AppError::forbidden("Sign-ups are closed on this cloud.")
}

/// A banned person cannot come back with the same address, even after deleting the account.
async fn check_email_ban(state: &AppState, email: &str) -> Result<()> {
    let bans = db::bans_by_email(&state.db, &sha256(email.as_bytes())).await?;
    let now = now();
    if let Some(ban) = bans.iter().find(|b| b.active(now)) {
        return Err(AppError::banned(ban.until()));
    }
    Ok(())
}

pub async fn signup(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(request): ApiJson<SignupRequest>,
) -> Result<Json<SignedIn>> {
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("code:ip:{ip}"),
            rates.code_per_ip,
            rates.auth_window,
        )
        .map_err(AppError::rate_limited)?;
    check_device(&request.device)?;
    if !request.age_confirmed {
        return Err(AppError::bad_request(
            "You must confirm that you are old enough to agree to the terms on your own.",
        ));
    }
    let email = {
        let pending = state.pending.lock().unwrap_or_else(|e| e.into_inner());
        match pending.signups.get(&request.signup) {
            Some(s) if s.deadline > Instant::now() => s.email.clone(),
            _ => {
                return Err(AppError::new(
                    ErrorCode::CodeExpired,
                    "The sign-up has expired. Start again with a new code.",
                ));
            }
        }
    };
    if state.config.signup == anyplay_api::Signup::Closed {
        return Err(signups_closed());
    }
    let handle = request.handle.as_str();
    if let Err(problem) = check_handle(handle) {
        let message = match problem {
            HandleProblem::Length => "A handle has 3 to 16 characters.",
            HandleProblem::FirstCharacter => "A handle starts with a letter.",
            HandleProblem::Character => {
                "A handle has only letters, digits, underscores and hyphens."
            }
        };
        return Err(AppError::new(ErrorCode::HandleInvalid, message));
    }
    if is_reserved(handle) {
        return Err(AppError::new(
            ErrorCode::HandleInvalid,
            "This handle is reserved.",
        ));
    }
    check_email_ban(&state, &email).await?;
    let taken = || AppError::new(ErrorCode::HandleTaken, "Another account has this handle.");
    if db::account_by_handle(&state.db, &handle.to_ascii_lowercase())
        .await?
        .is_some()
    {
        return Err(taken());
    }

    let created = now();
    let mut id = random_id();
    for _ in 0..10 {
        match db::insert_account(&state.db, id, handle, &email, created).await {
            Ok(()) => {
                let account = AccountRow {
                    id: id as i64,
                    handle: handle.to_owned(),
                    email: email.clone(),
                    created: created as i64,
                };
                {
                    let mut pending = state.pending.lock().unwrap_or_else(|e| e.into_inner());
                    pending.signups.remove(&request.signup);
                }
                let signed_in = start_device(&state, &account, &request.device).await?;
                return Ok(Json(signed_in));
            }
            Err(InsertError::Id) => id = random_id(),
            Err(InsertError::Handle) => return Err(taken()),
            Err(InsertError::Email) => {
                // Two sign-ups with one token raced; the other one won.
                return Err(AppError::new(
                    ErrorCode::CodeExpired,
                    "The sign-up has been used already.",
                ));
            }
            Err(InsertError::Other) => return Err(AppError::internal()),
        }
    }
    Err(AppError::internal())
}

/// Registers a device for the account and issues its tokens.
async fn start_device(state: &AppState, account: &AccountRow, device: &Device) -> Result<SignedIn> {
    if let Some(old) = db::device_to_evict(&state.db, account.id(), MAX_DEVICES).await? {
        state.sign_out_device(old).await?;
    }
    let refresh = random_token(32);
    let hash = sha256(refresh.as_bytes());
    let created = now();
    let mut id = random_id();
    for attempt in 0..5 {
        match db::insert_device(
            &state.db,
            id,
            account.id(),
            &device.name,
            platform_name(device.platform),
            &hash,
            created,
        )
        .await
        {
            Ok(()) => break,
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() && attempt < 4 => {
                id = random_id();
            }
            Err(error) => return Err(error.into()),
        }
    }
    state.cache_device(id, account.id());
    Ok(SignedIn {
        tokens: issue_tokens(state, account.id(), id, refresh),
        me: me_of(account),
    })
}

pub fn me_of(account: &AccountRow) -> Me {
    Me {
        id: anyplay_api::AccountId(account.id()),
        handle: account.handle.clone(),
        email: account.email.clone(),
        created: account.created as u64,
    }
}

fn issue_tokens(state: &AppState, account: u64, device: u64, refresh: String) -> Tokens {
    let expires = now() + state.tuning.access_ttl_secs;
    Tokens {
        access: state
            .signer
            .issue(&Claims::access(account, device, expires)),
        access_expires: expires,
        refresh,
        device,
    }
}

pub async fn refresh(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<RefreshRequest>,
) -> Result<Json<Tokens>> {
    let invalid = || AppError::unauthorized("This refresh token is not valid. Sign in again.");
    if request.refresh.len() > 128 {
        return Err(invalid());
    }
    let old = sha256(request.refresh.as_bytes());
    let Some(device) = db::device_by_current(&state.db, &old).await? else {
        if let Some(device) = db::device_by_used(&state.db, &old).await? {
            // Only a copy of the token can come back after it was replaced.
            tracing::warn!(
                device = device.id(),
                "a replaced refresh token was used again"
            );
            state.sign_out_device(device.id()).await?;
        }
        return Err(invalid());
    };
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("refresh:{}", device.id()),
            rates.refresh_per_device,
            rates.refresh_window,
        )
        .map_err(AppError::rate_limited)?;
    let account = device.account_id as u64;
    if let Some(until) = state.ban_of(account) {
        return Err(AppError::banned(until));
    }
    let refresh = random_token(32);
    let rotated = db::rotate_refresh(
        &state.db,
        device.id(),
        &old,
        &sha256(refresh.as_bytes()),
        now(),
    )
    .await?;
    if !rotated {
        // Another request used the token between our lookup and our update.
        state.sign_out_device(device.id()).await?;
        return Err(invalid());
    }
    Ok(Json(issue_tokens(&state, account, device.id(), refresh)))
}

pub async fn signout(State(state): State<AppState>, auth: Auth) -> Result<StatusCode> {
    state.sign_out_device(auth.device).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_handles_are_matched_without_regard_to_case() {
        for handle in ["AnyPlay", "ADMIN", "Moderator", "mod", "Root", "Official"] {
            assert!(is_reserved(handle), "{handle}");
        }
        for handle in [
            "PlayStation_Fan",
            "sonyfan",
            "PSN_alec",
            "ps5x",
            "xbox360",
            "Steamer",
        ] {
            assert!(is_reserved(handle), "{handle}");
        }
        for handle in ["alec", "moderate", "adminton", "play", "any_play", "psx"] {
            assert!(!is_reserved(handle), "{handle}");
        }
    }
}
