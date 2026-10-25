//! What the app on the player's PC calls: the token exchange that ends the browser sign-in, and
//! the link that opens the account web.

use anyplay_api::{AccountLink, BrowserTokenRequest, ErrorCode, SignedIn, path};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};

use super::flow::{AUTH_CODE, Grant};
use super::store::{self, SessionData};
use super::{
    WebError, WebForm, WebResult, check_csrf, existing, page, pages, sign_in_session, view,
};
use crate::binding;
use crate::db;
use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth, ClientIp};
use crate::routes::auth;
use crate::state::AppState;
use crate::util::now;

/// The kind of one-time value behind a link to the account web.
pub(crate) const LINK: &str = "link";

#[derive(Serialize, Deserialize)]
pub(crate) struct LinkGrant {
    pub account: u64,
    pub device: u64,
}

/// `POST /v1/auth/browser/token`: the app trades the code the browser brought to its loopback
/// address, and the PKCE verifier it made the challenge from, for the tokens of its device. The
/// code is used up by the first try, right or wrong.
pub async fn token(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(request): ApiJson<BrowserTokenRequest>,
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
    auth::check_device(&request.device)?;
    let key = auth::device_key_of(&state, &request.device)?;
    let expired = || {
        AppError::new(
            ErrorCode::CodeExpired,
            "This sign-in has expired or was used already. Start again.",
        )
    };
    let data = store::flow_take(&state.db, AUTH_CODE, &request.code)
        .await?
        .ok_or_else(expired)?;
    let grant: Grant = serde_json::from_str(&data).map_err(|_| expired())?;
    if !anyplay_proof::pkce::matches(&request.verifier, &grant.challenge) {
        return Err(AppError::new(
            ErrorCode::CodeWrong,
            "The sign-in could not be verified. Start again.",
        ));
    }
    let account = db::account(&state.db, grant.account)
        .await?
        .ok_or_else(expired)?;
    if let Some(until) = state.ban_of(account.id()) {
        return Err(crate::moderation::banned_error(&state, account.id(), until).await);
    }
    Ok(Json(
        auth::start_device(&state, &account, &request.device, key).await?,
    ))
}

/// `POST /v1/auth/browser/link`: a one-time link that opens the account web signed in as the
/// caller. The request must carry a proof from the device key, so a stolen access token cannot
/// ask for one.
pub async fn link(
    State(state): State<AppState>,
    auth: Auth,
    headers: HeaderMap,
) -> Result<Json<AccountLink>> {
    binding::require_proof(
        &state,
        auth.device,
        &headers,
        "POST",
        path::AUTH_BROWSER_LINK,
    )
    .await?;
    let Some(web) = state.web.as_ref() else {
        return Err(AppError::not_found());
    };
    let grant = LinkGrant {
        account: auth.account,
        device: auth.device,
    };
    let id = store::flow_put(
        &state.db,
        LINK,
        &serde_json::to_string(&grant).unwrap_or_default(),
        state.tuning.link_ttl,
    )
    .await?;
    // With the account web on another service, the link opens its page; a cloud that serves
    // its own account web keeps the page of the cloud.
    let path = if state.config.portal_external {
        "/link"
    } else {
        "/web/link"
    };
    Ok(Json(AccountLink {
        url: format!("{}{path}?c={id}", web.origin),
        expires: now() + state.tuning.link_ttl.as_secs(),
    }))
}

#[derive(Deserialize)]
pub struct LinkQuery {
    c: Option<String>,
}

fn expired_link() -> WebError {
    WebError::page(
        StatusCode::BAD_REQUEST,
        "This link has expired",
        "Open your account page from AnyPlay again.",
    )
}

/// The page a link opens. It only asks; the link is used up by the button, so a program that
/// fetches the address (a preview, a scanner) cannot sign anybody in.
pub async fn link_page(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(query): Query<LinkQuery>,
) -> WebResult {
    // Launchers of before the move hold links to this address.
    if state.config.portal_external {
        let to = match &query.c {
            Some(code) => format!(
                "/link?{}",
                serde_urlencoded::to_string([("c", code)]).unwrap_or_default()
            ),
            None => "/link".to_owned(),
        };
        return Ok(super::redirect_plain(&to));
    }
    let code = query.c.ok_or_else(expired_link)?;
    let data = store::flow_peek(&state.db, LINK, &code)
        .await?
        .ok_or_else(expired_link)?;
    let grant: LinkGrant = serde_json::from_str(&data).map_err(|_| expired_link())?;
    let account = db::account(&state.db, grant.account)
        .await?
        .ok_or_else(expired_link)?;
    let mut visit = super::visit(&state, &headers).await?;
    let v = view(&state, &mut visit.session).await?;
    let html = pages::link(&v.ctx(), &account.handle, &code);
    Ok(page(&state, &visit, StatusCode::OK, html))
}

#[derive(Deserialize)]
pub struct LinkForm {
    csrf: String,
    code: String,
}

pub async fn link_open(
    State(state): State<AppState>,
    headers: HeaderMap,
    WebForm(form): WebForm<LinkForm>,
) -> WebResult {
    let session = existing(&state, &headers)
        .await?
        .ok_or_else(WebError::expired)?;
    check_csrf(&session, &form.csrf)?;
    let data = store::flow_take(&state.db, LINK, &form.code)
        .await?
        .ok_or_else(expired_link)?;
    let grant: LinkGrant = serde_json::from_str(&data).map_err(|_| expired_link())?;
    // The device that asked must still be signed in, and the account not banned.
    if db::device_owner(&state.db, grant.device).await? != Some(grant.account) {
        return Err(expired_link());
    }
    if let Some(until) = state.ban_of(grant.account) {
        return Err(
            crate::moderation::banned_error(&state, grant.account, until)
                .await
                .into(),
        );
    }
    store::end_session(&state.db, &session).await?;
    let visit = sign_in_session(&state, grant.account, SessionData::default()).await?;
    Ok(super::redirect(&state, &visit, "/web/account"))
}
