//! The account page: e-mail, passkeys, devices, a copy of the data and deleting the account.
//! Every change is a POST with the session's CSRF token. What would give an attacker who holds
//! a stolen session a lasting hold (passkeys, signing out, deleting) needs a session that signed
//! in a few minutes ago.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;

use super::pages::{self, AccountView, DeviceView, PasskeyView};
use super::store::{self, Session};
use super::{
    Visit, WebError, WebForm, WebResult, check_csrf, existing, redirect, redirect_plain, view,
    without_cookie,
};
use crate::db::{self, AccountRow};
use crate::error::AppError;
use crate::state::AppState;

/// The session of a signed-in player and their account.
pub async fn signed_in(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(Session, AccountRow), WebError> {
    let session = existing(state, headers)
        .await?
        .ok_or_else(WebError::sign_in)?;
    let account = session.account.ok_or_else(WebError::sign_in)?;
    let row = db::account(&state.db, account)
        .await?
        .ok_or_else(WebError::sign_in)?;
    if state.ban_of(account).is_some() {
        let mut session = session;
        session.data.banned = Some(account);
        store::save_data(&state.db, &session).await?;
        return Err(WebError::go("/web/banned"));
    }
    Ok((session, row))
}

/// Like [`signed_in`] for a POST: the token must fit and, for what matters, the sign-in must be
/// recent.
async fn posted(
    state: &AppState,
    headers: &HeaderMap,
    csrf: &str,
    needs_fresh: bool,
) -> Result<(Session, AccountRow), WebError> {
    let (session, account) = signed_in(state, headers).await?;
    check_csrf(&session, csrf)?;
    if needs_fresh && !session.fresh(state.tuning.fresh_for) {
        return Err(WebError::fresh());
    }
    Ok((session, account))
}

pub async fn page(State(state): State<AppState>, headers: HeaderMap) -> WebResult {
    let (mut session, account) = signed_in(&state, &headers).await?;
    let passkeys = store::passkeys_of(&state.db, account.id()).await?;
    let devices = db::devices_of(&state.db, account.id()).await?;
    let account_view = AccountView {
        handle: account.handle.clone(),
        email: account.email.clone(),
        created: account.created as u64,
        passkeys: passkeys
            .iter()
            .map(|k| PasskeyView {
                id: URL_SAFE_NO_PAD.encode(&k.id),
                name: k.name.clone(),
                created: k.created as u64,
                last_used: k.last_used.map(|t| t as u64),
            })
            .collect(),
        devices: devices
            .iter()
            .map(|d| DeviceView {
                id: d.id(),
                name: d.name.clone(),
                platform: d.platform.clone(),
                last_used: d.last_used as u64,
            })
            .collect(),
        max_passkeys: store::MAX_PASSKEYS,
    };
    let v = view(&state, &mut session).await?;
    let html = pages::account(&v.ctx(), &account_view);
    let visit = Visit {
        session,
        token: None,
    };
    Ok(super::page(&state, &visit, StatusCode::OK, html))
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf: String,
}

async fn done(state: &AppState, mut session: Session, notice: &str) -> Result<Response, WebError> {
    session.data.notice = Some(notice.to_owned());
    store::save_data(&state.db, &session).await?;
    Ok(redirect(
        state,
        &Visit {
            session,
            token: None,
        },
        "/web/account",
    ))
}

pub async fn sign_out(
    State(state): State<AppState>,
    headers: HeaderMap,
    WebForm(form): WebForm<CsrfForm>,
) -> WebResult {
    let (session, _) = posted(&state, &headers, &form.csrf, false).await?;
    store::end_session(&state.db, &session).await?;
    Ok(without_cookie(&state, redirect_plain("/web/sign-in")))
}

pub async fn remove_passkey(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    WebForm(form): WebForm<CsrfForm>,
) -> WebResult {
    let (session, account) = posted(&state, &headers, &form.csrf, true).await?;
    let id = URL_SAFE_NO_PAD
        .decode(&id)
        .map_err(|_| AppError::not_found())?;
    if !store::delete_passkey(&state.db, account.id(), &id).await? {
        return Err(AppError::not_found().into());
    }
    done(&state, session, "Passkey removed.").await
}

pub async fn sign_out_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    WebForm(form): WebForm<CsrfForm>,
) -> WebResult {
    let (session, account) = posted(&state, &headers, &form.csrf, false).await?;
    let device = crate::http::parse_id(&id)?;
    if db::device_owner(&state.db, device).await? != Some(account.id()) {
        return Err(AppError::not_found().into());
    }
    state.sign_out_device(device).await?;
    done(&state, session, "Device signed out.").await
}

pub async fn sign_out_everywhere(
    State(state): State<AppState>,
    headers: HeaderMap,
    WebForm(form): WebForm<CsrfForm>,
) -> WebResult {
    let (_, account) = posted(&state, &headers, &form.csrf, true).await?;
    state.sign_out_everywhere(account.id()).await?;
    let error = WebError::page(
        StatusCode::OK,
        "Signed out everywhere",
        "Every device and browser is signed out. Sign in again to use your account.",
    )
    .with_action("/web/sign-in", "Sign in");
    Ok(without_cookie(&state, error.into_response()))
}

/// A copy of everything the cloud stores about the account.
pub async fn export(State(state): State<AppState>, headers: HeaderMap) -> WebResult {
    let (_, account) = signed_in(&state, &headers).await?;
    let data = crate::routes::accounts::export_json(&state, account.id()).await?;
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

pub async fn delete_page(State(state): State<AppState>, headers: HeaderMap) -> WebResult {
    let (mut session, account) = signed_in(&state, &headers).await?;
    if !session.fresh(state.tuning.fresh_for) {
        return Err(WebError::fresh());
    }
    let v = view(&state, &mut session).await?;
    let html = pages::delete(&v.ctx(), &account.handle, None);
    let visit = Visit {
        session,
        token: None,
    };
    Ok(super::page(&state, &visit, StatusCode::OK, html))
}

#[derive(Deserialize)]
pub struct DeleteForm {
    csrf: String,
    handle: String,
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    WebForm(form): WebForm<DeleteForm>,
) -> WebResult {
    let (mut session, account) = posted(&state, &headers, &form.csrf, true).await?;
    if !form.handle.trim().eq_ignore_ascii_case(&account.handle) {
        let v = view(&state, &mut session).await?;
        let html = pages::delete(
            &v.ctx(),
            &account.handle,
            Some("That is not your handle. Type it exactly to confirm."),
        );
        let visit = Visit {
            session,
            token: None,
        };
        return Ok(super::page(&state, &visit, StatusCode::BAD_REQUEST, html));
    }
    // The account is deleted when the grace period ends. Until then this browser stays signed
    // in; every device and every other browser is signed out.
    state
        .schedule_deletion(account.id(), Some(&session.id_hash))
        .await?;
    let days = state.tuning.deletion_grace.as_secs() / 86_400;
    let page = WebError::page(
        StatusCode::OK,
        "Account deletion scheduled",
        &format!(
            "Your account and its data will be deleted from this cloud in {days} days. Every              device and every other browser is signed out. To keep the account, sign in on the              AnyPlay portal and cancel the deletion before then."
        ),
    );
    Ok(page.into_response())
}
