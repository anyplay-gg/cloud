//! The account: e-mail, passkeys, devices and browser sessions, a copy of the data and deleting
//! the account with a grace period; and step-up, the way to prove again who the player is.
//!
//! What would give an attacker who holds a stolen session a lasting hold (passkeys, signing out
//! everywhere, a new address, deleting) needs a *fresh* session: one that signed in, or passed a
//! step-up, a few minutes ago. Without it the answer is `step_up_required`.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::json;
use webauthn_rs::prelude::RegisterPublicKeyCredential;

use super::{ApiFail, ApiResult, Member, done, json_status};
use crate::db;
use crate::error::AppError;
use crate::http::{ApiJson, ClientIp, parse_id};
use crate::routes::auth::{self, me_of};
use crate::state::AppState;
use crate::util::{is_label, now, now_ms};
use crate::web::passkeys::{self, FinishSignIn};
use crate::web::store::{self, EmailChange, MAX_PASSKEYS, PasskeyRow};
use crate::web::{limit_ip_api, without_cookie};

#[derive(Serialize)]
struct PasskeyView {
    id: String,
    name: String,
    created: u64,
    last_used: Option<u64>,
}

fn passkey_view(row: &PasskeyRow) -> PasskeyView {
    PasskeyView {
        id: URL_SAFE_NO_PAD.encode(&row.id),
        name: row.name.clone(),
        created: row.created as u64,
        last_used: row.last_used.map(|t| t as u64),
    }
}

#[derive(Serialize)]
struct DeviceView {
    id: u64,
    name: String,
    platform: String,
    created: u64,
    last_used: u64,
}

#[derive(Serialize)]
struct SessionRow {
    id: String,
    /// True for the browser that asks.
    current: bool,
    created: u64,
    last_used: u64,
}

/// A browser session's ID in lists: the first 12 bytes of the hash that the store keeps. It
/// names the session to its owner and gives nobody the cookie.
fn session_id(id_hash: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(&id_hash[..id_hash.len().min(12)])
}

fn secs(ms: i64) -> u64 {
    (ms.max(0) / 1000) as u64
}

/// `GET /account`
pub async fn get(State(state): State<AppState>, member: Member) -> ApiResult {
    let id = member.id();
    let passkeys = store::passkeys_of(&state.db, id).await?;
    let devices = db::devices_of(&state.db, id).await?;
    let sessions = store::sessions_of(&state.db, id).await?;
    let deletion = db::deletion_of(&state.db, id).await?;
    let fresh_until = member.session.fresh_until(state.tuning.fresh_for).map(secs);
    Ok(Json(json!({
        "account": me_of(&member.account),
        "passkeys": passkeys.iter().map(passkey_view).collect::<Vec<_>>(),
        "max_passkeys": MAX_PASSKEYS,
        "devices": devices.iter().map(|d| DeviceView {
            id: d.id(),
            name: d.name.clone(),
            platform: d.platform.clone(),
            created: d.created as u64,
            last_used: d.last_used as u64,
        }).collect::<Vec<_>>(),
        "sessions": sessions.iter().map(|s| SessionRow {
            id: session_id(&s.id_hash),
            current: s.id_hash == member.session.id_hash,
            created: secs(s.created_ms),
            last_used: secs(s.used_ms),
        }).collect::<Vec<_>>(),
        "deletion": deletion.map(|at| json!({"at": at})),
        "pending_email": member.session.data.email_change.as_ref().map(|c| c.email.clone()),
        "fresh_until": fresh_until,
    }))
    .into_response())
}

// Passkeys

/// `POST /account/passkeys/start`: the options for `navigator.credentials.create`.
pub async fn passkey_start(
    State(state): State<AppState>,
    member: Member,
    ApiJson(_body): ApiJson<serde_json::Value>,
) -> ApiResult {
    member.require_fresh(&state)?;
    Ok(Json(passkeys::begin_registration(&state, member.id()).await?).into_response())
}

#[derive(Deserialize)]
pub struct FinishBody {
    id: String,
    #[serde(default)]
    name: String,
    credential: RegisterPublicKeyCredential,
}

/// `POST /account/passkeys/finish`
pub async fn passkey_finish(
    State(state): State<AppState>,
    member: Member,
    ApiJson(body): ApiJson<FinishBody>,
) -> ApiResult {
    member.require_fresh(&state)?;
    let id =
        passkeys::finish_registration(&state, member.id(), &body.id, &body.name, &body.credential)
            .await?;
    let row = store::passkey_by_id(&state.db, &id)
        .await?
        .ok_or_else(AppError::internal)?;
    Ok(json_status(
        StatusCode::CREATED,
        json!({"passkey": passkey_view(&row)}),
    ))
}

fn passkey_id(text: &str) -> Result<Vec<u8>, AppError> {
    URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| AppError::not_found())
}

#[derive(Deserialize)]
pub struct RenameBody {
    name: String,
}

/// `PATCH /account/passkeys/{id}`
pub async fn passkey_rename(
    State(state): State<AppState>,
    member: Member,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<RenameBody>,
) -> ApiResult {
    let name = body.name.trim();
    if !is_label(name, 40) {
        return Err(AppError::bad_request("A name has 1 to 40 characters.").into());
    }
    if !store::rename_passkey(&state.db, member.id(), &passkey_id(&id)?, name).await? {
        return Err(AppError::not_found().into());
    }
    done()
}

/// `DELETE /account/passkeys/{id}`
pub async fn passkey_remove(
    State(state): State<AppState>,
    member: Member,
    Path(id): Path<String>,
) -> ApiResult {
    member.require_fresh(&state)?;
    if !store::delete_passkey(&state.db, member.id(), &passkey_id(&id)?).await? {
        return Err(AppError::not_found().into());
    }
    done()
}

// Devices and browser sessions

/// `DELETE /account/devices/{id}`
pub async fn device_sign_out(
    State(state): State<AppState>,
    member: Member,
    Path(id): Path<String>,
) -> ApiResult {
    let device = parse_id(&id)?;
    if db::device_owner(&state.db, device).await? != Some(member.id()) {
        return Err(AppError::not_found().into());
    }
    state.sign_out_device(device).await?;
    done()
}

/// `DELETE /account/sessions/{id}`. Ending the session of the browser that asks signs it out.
pub async fn session_sign_out(
    State(state): State<AppState>,
    member: Member,
    Path(id): Path<String>,
) -> ApiResult {
    let sessions = store::sessions_of(&state.db, member.id()).await?;
    let Some(found) = sessions.iter().find(|s| session_id(&s.id_hash) == id) else {
        return Err(AppError::not_found().into());
    };
    let current = found.id_hash == member.session.id_hash;
    store::end_session_of(&state.db, member.id(), &found.id_hash).await?;
    let response = Json(json!({"ok": true, "signed_out": current})).into_response();
    Ok(if current {
        without_cookie(&state, response)
    } else {
        response
    })
}

/// `POST /account/sign-out-everywhere`: every device and every browser, this one too.
pub async fn sign_out_everywhere(
    State(state): State<AppState>,
    member: Member,
    ApiJson(_body): ApiJson<serde_json::Value>,
) -> ApiResult {
    member.require_fresh(&state)?;
    state.sign_out_everywhere(member.id()).await?;
    Ok(without_cookie(
        &state,
        Json(json!({"signed_out": true})).into_response(),
    ))
}

// A copy of the data

/// `GET /account/export`: everything the cloud stores about the account, as a JSON file.
pub async fn export(State(state): State<AppState>, member: Member) -> ApiResult {
    let data = crate::routes::accounts::export_json(&state, member.id()).await?;
    let body = serde_json::to_vec_pretty(&data).unwrap_or_default();
    let mut response = (StatusCode::OK, body).into_response();
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"anyplay-data.json\""),
    );
    Ok(response)
}

// The e-mail address

#[derive(Deserialize)]
pub struct EmailBody {
    email: String,
}

/// `POST /account/email`: sends a code to the new address.
pub async fn email_start(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    mut member: Member,
    ApiJson(body): ApiJson<EmailBody>,
) -> ApiResult {
    member.require_fresh(&state)?;
    let cannot = || AppError::bad_request("That address cannot be used for this account.");
    let wanted = crate::util::normalize_email(&body.email)
        .ok_or_else(|| AppError::bad_request("That is not an e-mail address."))?;
    if wanted == member.account.email {
        return Err(cannot().into());
    }
    auth::check_email_ban(&state, &wanted).await?;
    let (challenge, email) = auth::start_email_challenge(&state, ip, &wanted).await?;
    member.session.data.email_change = Some(EmailChange {
        challenge,
        email: email.clone(),
    });
    store::save_data(&state.db, &member.session).await?;
    Ok(Json(json!({"step": "code", "email": email})).into_response())
}

#[derive(Deserialize)]
pub struct CodeBody {
    code: String,
}

/// `POST /account/email/confirm`: the code from the new address makes it the account's address.
pub async fn email_confirm(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    mut member: Member,
    ApiJson(body): ApiJson<CodeBody>,
) -> ApiResult {
    member.require_fresh(&state)?;
    let Some(change) = member.session.data.email_change.clone() else {
        return Err(AppError::bad_request("Ask for a code for the new address first.").into());
    };
    limit_ip_api(&state, "code", ip, state.tuning.rates.code_per_ip)?;
    let email = match auth::check_challenge(&state, &change.challenge, body.code.trim()).await {
        Ok(email) => email,
        Err(error) => {
            if error.body.error == anyplay_api::ErrorCode::CodeExpired {
                member.session.data.email_change = None;
                store::save_data(&state.db, &member.session).await?;
            }
            return Err(error.into());
        }
    };
    member.session.data.email_change = None;
    store::save_data(&state.db, &member.session).await?;
    // The code proved the address in the challenge; it must be the one asked for.
    if email != change.email {
        return Err(AppError::bad_request("That code is for another address.").into());
    }
    auth::check_email_ban(&state, &email).await?;
    if !db::set_email(&state.db, member.id(), &email).await? {
        return Err(AppError::bad_request("That address cannot be used for this account.").into());
    }
    Ok(Json(json!({"email": email})).into_response())
}

// Deletion

#[derive(Deserialize)]
pub struct DeletionBody {
    handle: String,
}

/// `POST /account/deletion`: the account is deleted when the grace period ends, unless the
/// player cancels. Every device and every other browser is signed out at once.
pub async fn deletion_start(
    State(state): State<AppState>,
    member: Member,
    ApiJson(body): ApiJson<DeletionBody>,
) -> ApiResult {
    member.require_fresh(&state)?;
    if !body
        .handle
        .trim()
        .eq_ignore_ascii_case(&member.account.handle)
    {
        let mut fail = ApiFail::from(AppError::bad_request(
            "That is not your handle. Type it exactly.",
        ));
        fail.body["field"] = json!("handle");
        return Err(fail);
    }
    let id = member.id();
    let at = now() + state.tuning.deletion_grace.as_secs();
    db::schedule_deletion(&state.db, id, at).await?;
    for device in db::devices_of(&state.db, id).await? {
        state.sign_out_device(device.id()).await?;
    }
    for other in store::sessions_of(&state.db, id).await? {
        if other.id_hash != member.session.id_hash {
            store::end_session_of(&state.db, id, &other.id_hash).await?;
        }
    }
    let at = db::deletion_of(&state.db, id).await?.unwrap_or(at);
    Ok(Json(json!({"deletion": {"at": at}})).into_response())
}

/// `DELETE /account/deletion`
pub async fn deletion_cancel(State(state): State<AppState>, member: Member) -> ApiResult {
    db::cancel_deletion(&state.db, member.id()).await?;
    Ok(Json(json!({"deletion": null})).into_response())
}

// Step-up

fn hint(email: &str) -> String {
    match email.split_once('@') {
        Some((name, domain)) => {
            let first: String = name.chars().take(1).collect();
            format!("{first}***@{domain}")
        }
        None => "***".to_owned(),
    }
}

/// `POST /step-up/email`: sends a code to the account's own address.
pub async fn step_up_email(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    mut member: Member,
    ApiJson(_body): ApiJson<serde_json::Value>,
) -> ApiResult {
    let (challenge, _) =
        auth::start_email_challenge(&state, ip, &member.account.email.clone()).await?;
    member.session.data.step_up = Some(challenge);
    store::save_data(&state.db, &member.session).await?;
    Ok(Json(json!({"step": "code", "email_hint": hint(&member.account.email)})).into_response())
}

/// Marks the session as fresh now.
async fn verified(state: &AppState, mut member: Member) -> ApiResult {
    member.session.data.verified_ms = Some(now_ms());
    member.session.data.step_up = None;
    store::save_data(&state.db, &member.session).await?;
    let until = member.session.fresh_until(state.tuning.fresh_for).map(secs);
    Ok(Json(json!({"fresh_until": until})).into_response())
}

/// `POST /step-up/code`
pub async fn step_up_code(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    mut member: Member,
    ApiJson(body): ApiJson<CodeBody>,
) -> ApiResult {
    let Some(challenge) = member.session.data.step_up.clone() else {
        return Err(AppError::bad_request("Ask for a code first.").into());
    };
    limit_ip_api(&state, "code", ip, state.tuning.rates.code_per_ip)?;
    match auth::check_challenge(&state, &challenge, body.code.trim()).await {
        Ok(email) if email == member.account.email => verified(&state, member).await,
        Ok(_) => Err(AppError::bad_request("That code is for another address.").into()),
        Err(error) => {
            if error.body.error == anyplay_api::ErrorCode::CodeExpired {
                member.session.data.step_up = None;
                store::save_data(&state.db, &member.session).await?;
            }
            Err(error.into())
        }
    }
}

/// `POST /step-up/passkey/finish`: the answer to the options of `POST /passkey/sign-in/start`.
/// The passkey must be one of this account.
pub async fn step_up_passkey(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    member: Member,
    ApiJson(body): ApiJson<FinishSignIn>,
) -> ApiResult {
    limit_ip_api(&state, "passkey", ip, state.tuning.rates.code_per_ip)?;
    let owner = passkeys::verify_assertion(&state, &body).await?;
    if owner.id() != member.id() {
        return Err(AppError::unauthorized("That passkey belongs to another account.").into());
    }
    verified(&state, member).await
}
