//! Passkeys: the WebAuthn ceremonies, done by webauthn-rs on this side and by the browser's
//! `navigator.credentials` on the other (`assets/passkey.js` carries the messages).
//!
//! The server keeps the state of a ceremony in the store and takes it once, so a response can
//! be used for one ceremony only. Sign-in is discoverable (no e-mail address needed): the
//! authenticator names the account in the user handle, which must be the one this cloud derives
//! for the account that owns the credential. User verification is required, so a passkey is
//! both factors in one.
//!
//! The ceremonies are four functions ([`begin_registration`], [`finish_registration`],
//! [`begin_assertion`], [`verify_assertion`]) that the routes of the HTML pages and of the portal
//! API ([`super::api`]) both call.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::json;
use webauthn_rs::prelude::{
    CredentialID, DiscoverableAuthentication, DiscoverableKey, Passkey, PasskeyRegistration,
    PublicKeyCredential, RegisterPublicKeyCredential, Uuid,
};

use super::store::{self, MAX_PASSKEYS, Session};
use super::{Visit, check_csrf_api, existing_api, flow, limit_ip_api};
use crate::db::{self, AccountRow};
use crate::error::{AppError, Result};
use crate::http::{ApiJson, ClientIp};
use crate::state::AppState;
use crate::util::{is_label, now, sha256};

const REGISTER: &str = "passkey-reg";
const SIGN_IN: &str = "passkey-auth";

/// The WebAuthn user handle of an account: 16 bytes that identify it to the authenticator and
/// reveal nothing about it.
pub fn user_uuid(account: u64) -> Uuid {
    let mut material = b"anyplay-webauthn-user-v1".to_vec();
    material.extend_from_slice(&account.to_le_bytes());
    let digest = sha256(&material);
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

fn web(state: &AppState) -> Result<&super::Web> {
    state.web.as_ref().ok_or_else(AppError::not_found)
}

/// The signed-in session of a script request, with its CSRF token checked and a recent sign-in.
async fn account_session(state: &AppState, headers: &HeaderMap) -> Result<(Session, u64)> {
    let session = existing_api(state, headers).await?;
    check_csrf_api(&session, headers)?;
    let account = session
        .account
        .ok_or_else(|| AppError::unauthorized("Sign in first."))?;
    if !session.fresh(state.tuning.fresh_for) {
        return Err(AppError::unauthorized(
            "Sign in again before you add a passkey.",
        ));
    }
    Ok((session, account))
}

#[derive(Serialize, Deserialize)]
struct RegistrationState {
    account: u64,
    state: PasskeyRegistration,
}

/// Starts adding a passkey to an account: the ceremony's ID and the options for the browser.
pub async fn begin_registration(state: &AppState, account: u64) -> Result<serde_json::Value> {
    let web = web(state)?;
    let existing = store::passkeys_of(&state.db, account).await?;
    if existing.len() >= MAX_PASSKEYS {
        return Err(AppError::limit(format!(
            "An account can have {MAX_PASSKEYS} passkeys. Remove one first."
        )));
    }
    let row = db::account(&state.db, account)
        .await?
        .ok_or_else(AppError::not_found)?;
    let exclude: Vec<CredentialID> = existing.iter().map(|k| k.id.clone().into()).collect();
    let (options, registration) = web
        .webauthn
        .start_passkey_registration(user_uuid(account), &row.handle, &row.handle, Some(exclude))
        .map_err(|error| {
            tracing::warn!(?error, "cannot start a passkey registration");
            AppError::internal()
        })?;
    // Ask for a credential the authenticator can offer without an e-mail address. Only the
    // request to the browser changes; a security key without room for one still registers.
    let mut options = serde_json::to_value(&options).map_err(|_| AppError::internal())?;
    if let Some(selection) = options.pointer_mut("/publicKey/authenticatorSelection") {
        selection["residentKey"] = json!("preferred");
    }
    let id = store::flow_put(
        &state.db,
        REGISTER,
        &serde_json::to_string(&RegistrationState {
            account,
            state: registration,
        })
        .unwrap_or_default(),
        state.tuning.ceremony_ttl,
    )
    .await?;
    Ok(json!({"id": id, "options": options}))
}

/// Finishes adding a passkey. Returns its ID (the credential ID).
pub async fn finish_registration(
    state: &AppState,
    account: u64,
    ceremony: &str,
    name: &str,
    credential: &RegisterPublicKeyCredential,
) -> Result<Vec<u8>> {
    let web = web(state)?;
    let expired = || AppError::bad_request("This passkey request expired. Try again.");
    let data = store::flow_take(&state.db, REGISTER, ceremony)
        .await?
        .ok_or_else(expired)?;
    let held: RegistrationState = serde_json::from_str(&data).map_err(|_| expired())?;
    if held.account != account {
        return Err(expired());
    }
    let passkey = web
        .webauthn
        .finish_passkey_registration(credential, &held.state)
        .map_err(|error| {
            tracing::info!(?error, "a passkey registration was refused");
            AppError::bad_request("Your device did not make a valid passkey. Try again.")
        })?;
    let name = name.trim();
    let existing = store::passkeys_of(&state.db, account).await?;
    if existing.len() >= MAX_PASSKEYS {
        return Err(AppError::limit(format!(
            "An account can have {MAX_PASSKEYS} passkeys. Remove one first."
        )));
    }
    let name = if is_label(name, 40) {
        name.to_owned()
    } else {
        format!("Passkey {}", existing.len() + 1)
    };
    let id = passkey.cred_id().as_slice().to_vec();
    let added = store::insert_passkey(
        &state.db,
        &id,
        account,
        &name,
        &serde_json::to_string(&passkey).unwrap_or_default(),
        now(),
    )
    .await?;
    if !added {
        return Err(AppError::new(
            anyplay_api::ErrorCode::Forbidden,
            "This passkey is already registered.",
        ));
    }
    Ok(id)
}

#[derive(Deserialize)]
pub struct FinishRegistration {
    id: String,
    #[serde(default)]
    name: String,
    credential: RegisterPublicKeyCredential,
}

pub async fn register_start(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(_body): ApiJson<serde_json::Value>,
) -> Result<Json<serde_json::Value>> {
    let (_, account) = account_session(&state, &headers).await?;
    Ok(Json(begin_registration(&state, account).await?))
}

pub async fn register_finish(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<FinishRegistration>,
) -> Result<Json<serde_json::Value>> {
    let (mut session, account) = account_session(&state, &headers).await?;
    finish_registration(&state, account, &body.id, &body.name, &body.credential).await?;
    session.data.notice = Some("Passkey added.".to_owned());
    store::save_data(&state.db, &session).await?;
    Ok(Json(json!({"ok": true})))
}

/// Starts a sign-in with a passkey that names its account: the ceremony's ID and the options.
pub async fn begin_assertion(state: &AppState) -> Result<serde_json::Value> {
    let web = web(state)?;
    let (options, authentication) =
        web.webauthn
            .start_discoverable_authentication()
            .map_err(|error| {
                tracing::warn!(?error, "cannot start a passkey sign-in");
                AppError::internal()
            })?;
    let id = store::flow_put(
        &state.db,
        SIGN_IN,
        &serde_json::to_string(&authentication).unwrap_or_default(),
        state.tuning.ceremony_ttl,
    )
    .await?;
    Ok(json!({"id": id, "options": options}))
}

pub async fn sign_in_start(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>> {
    let session = existing_api(&state, &headers).await?;
    check_csrf_api(&session, &headers)?;
    limit_ip_api(&state, "passkey", ip, state.tuning.rates.code_per_ip)?;
    Ok(Json(begin_assertion(&state).await?))
}

#[derive(Deserialize)]
pub struct FinishSignIn {
    pub id: String,
    pub credential: PublicKeyCredential,
}

fn refused() -> AppError {
    AppError::unauthorized("That passkey did not work. Use an e-mail code instead.")
}

/// Checks a passkey's answer to the ceremony `body.id` that [`begin_assertion`] started. Returns
/// the account that owns the passkey and counts the use. It signs nobody in.
pub async fn verify_assertion(state: &AppState, body: &FinishSignIn) -> Result<AccountRow> {
    let web = web(state)?;
    let held = store::flow_take(&state.db, SIGN_IN, &body.id)
        .await?
        .ok_or_else(refused)?;
    let authentication: DiscoverableAuthentication =
        serde_json::from_str(&held).map_err(|_| refused())?;

    let (user, credential_id) = web
        .webauthn
        .identify_discoverable_authentication(&body.credential)
        .map_err(|error| {
            tracing::info!(?error, "a passkey named no user");
            refused()
        })?;
    let row = store::passkey_by_id(&state.db, credential_id)
        .await?
        .ok_or_else(refused)?;
    let account_id = row.account_id as u64;
    if user_uuid(account_id) != user {
        tracing::warn!("a passkey came with the user handle of another account");
        return Err(refused());
    }
    let mut passkey: Passkey = serde_json::from_str(&row.credential).map_err(|_| refused())?;
    let result = web
        .webauthn
        .finish_discoverable_authentication(
            &body.credential,
            authentication,
            &[DiscoverableKey::from(&passkey)],
        )
        .map_err(|error| {
            tracing::info!(?error, "a passkey sign-in was refused");
            refused()
        })?;
    // The counter and flags move on with every use; a counter that did not rise is refused
    // above as a possible clone.
    passkey.update_credential(&result);
    store::update_passkey(
        &state.db,
        &row.id,
        &serde_json::to_string(&passkey).unwrap_or_default(),
        now(),
    )
    .await?;
    db::account(&state.db, account_id)
        .await?
        .ok_or_else(refused)
}

pub async fn sign_in_finish(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    ApiJson(body): ApiJson<FinishSignIn>,
) -> Result<axum::response::Response> {
    let session = existing_api(&state, &headers).await?;
    check_csrf_api(&session, &headers)?;
    limit_ip_api(&state, "passkey", ip, state.tuning.rates.code_per_ip)?;
    let account = verify_assertion(&state, &body).await?;
    let mut visit = Visit {
        session,
        token: None,
    };
    let landing = flow::finish_login(&state, &mut visit, &account, flow::Proof::Passkey).await?;
    Ok(super::with_cookie(
        &state,
        &visit,
        Json(json!({"next": landing.path()})).into_response(),
    ))
}
