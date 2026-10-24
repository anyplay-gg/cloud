//! The JSON API of the web portal, under `/web/api/v1` (network `spec/portal-api.md`).
//!
//! The portal is a single-page app served from the cloud's own origin. It talks to the cloud
//! with `fetch`, on the browser session cookie of the account web ([`super::store`]), and sends
//! the session's CSRF token in `X-CSRF-Token` with every write. These routes sit behind the same
//! [`super::guard`] as the HTML pages (a write from another site is refused, the security headers
//! are set). On top of that every write must say `Content-Type: application/json`, which a form
//! on another site cannot send.
//!
//! * [`flow`]: the session, and signing in or up as steps of the flow of [`super::flow`].
//! * [`account`]: the account, its passkeys, devices and sessions, step-up, export, deletion.
//! * [`social`]: friends, the player's own profile and trophies, and public profiles.
//! * [`links`]: connections, the player's linked Discord and Steam accounts.

use axum::Router;
use axum::extract::{FromRequestParts, Request};
use axum::http::header::{self, HeaderValue};
use axum::http::request::Parts;
use axum::http::{Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use serde_json::{Value, json};

use super::store::{self, Session};
use super::{check_csrf_api, existing_api};
use crate::db::{self, AccountRow};
use crate::error::AppError;
use crate::http::count_request;
use crate::state::AppState;

pub mod account;
pub mod flow;
pub mod links;
pub mod social;

/// The base path of every route here.
pub const BASE: &str = "/web/api/v1";

/// A failed request. The body is the cloud's error shape, `{"error": <code>, "message": <text>}`
/// with the same codes as `/v1`, plus the codes of the portal (`step_up_required`,
/// `unsupported_media_type`).
#[derive(Debug)]
pub struct ApiFail {
    status: StatusCode,
    body: Value,
    retry_after: Option<u64>,
}

impl ApiFail {
    pub fn new(status: StatusCode, code: &str, message: &str) -> Self {
        Self {
            status,
            body: json!({"error": code, "message": message}),
            retry_after: None,
        }
    }

    /// The action needs a recent proof of who the player is: see `POST /step-up/*`. `methods`
    /// names the ways that count.
    pub fn step_up(methods: &[&str]) -> Self {
        let mut fail = Self::new(
            StatusCode::FORBIDDEN,
            "step_up_required",
            "Confirm that it is you to do this.",
        );
        fail.body["methods"] = json!(methods);
        fail
    }
}

impl From<AppError> for ApiFail {
    fn from(error: AppError) -> Self {
        let status = StatusCode::from_u16(error.body.error.status())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut body = serde_json::to_value(&error.body).unwrap_or_default();
        // The appeal token of the API never reaches the page: the browser session files appeals.
        if let Some(ban) = body.get_mut("ban").and_then(Value::as_object_mut) {
            ban.remove("appeal_token");
        }
        Self {
            status,
            body,
            retry_after: error.retry_after,
        }
    }
}

impl From<sqlx::Error> for ApiFail {
    fn from(error: sqlx::Error) -> Self {
        AppError::from(error).into()
    }
}

impl IntoResponse for ApiFail {
    fn into_response(self) -> Response {
        let mut response = (self.status, axum::Json(self.body)).into_response();
        if let Some(seconds) = self.retry_after
            && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

pub type ApiResult<T = Response> = Result<T, ApiFail>;

fn is_write(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

/// The browser session of a request: the cookie must name a live session, and a write must carry
/// its CSRF token.
pub struct Visitor {
    pub session: Session,
}

impl FromRequestParts<AppState> for Visitor {
    type Rejection = ApiFail;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiFail> {
        let session = existing_api(state, &parts.headers).await?;
        if is_write(&parts.method) {
            check_csrf_api(&session, &parts.headers)?;
        }
        Ok(Self { session })
    }
}

/// A signed-in player: a [`Visitor`] whose session has an account that exists and is not banned.
/// Each request counts against the account's rate limit.
pub struct Member {
    pub session: Session,
    pub account: AccountRow,
}

impl FromRequestParts<AppState> for Member {
    type Rejection = ApiFail;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiFail> {
        let Visitor { mut session } = Visitor::from_request_parts(parts, state).await?;
        let unauthorized = || AppError::unauthorized("Sign in first.");
        let id = session.account.ok_or_else(unauthorized)?;
        let account = db::account(&state.db, id).await?.ok_or_else(unauthorized)?;
        if let Some(until) = state.ban_of(id) {
            // The page learns of it from the session and shows the statement.
            session.data.banned = Some(id);
            store::save_data(&state.db, &session).await?;
            return Err(AppError::banned(until).into());
        }
        count_request(state, id)?;
        Ok(Self { session, account })
    }
}

impl Member {
    pub fn id(&self) -> u64 {
        self.account.id()
    }

    /// Sensitive actions need a session that signed in, or proved who the player is again, a few
    /// minutes ago.
    pub fn require_fresh(&self, state: &AppState) -> ApiResult<()> {
        if self.session.fresh(state.tuning.fresh_for) {
            Ok(())
        } else {
            Err(ApiFail::step_up(&["passkey", "email_code"]))
        }
    }

    /// For the actions that would move the account to another address: a session that is
    /// fresh, and when the account has a passkey, one that a passkey made fresh, since a code
    /// sent to an address is what the change is about.
    pub async fn require_strong(&self, state: &AppState) -> ApiResult<()> {
        self.require_fresh(state)?;
        let has_passkey = !store::passkeys_of(&state.db, self.id()).await?.is_empty();
        if has_passkey && !self.session.passkey_fresh(state.tuning.fresh_for) {
            return Err(ApiFail::step_up(&["passkey"]));
        }
        Ok(())
    }
}

/// Every write says it carries JSON (the picture of an avatar says it is a picture). A form on
/// another site cannot send either content type.
async fn require_json(request: Request, next: Next) -> Response {
    if is_write(request.method()) {
        let media = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(|v| v.trim().to_ascii_lowercase());
        // Inside the nested router the path has lost its prefix.
        let avatar = request.uri().path().ends_with("/me/avatar");
        let ok = match media.as_deref() {
            Some("application/json") => true,
            Some("image/png" | "image/jpeg") => avatar,
            _ => false,
        };
        if !ok {
            return ApiFail::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                "Send the request with Content-Type: application/json.",
            )
            .into_response();
        }
    }
    next.run(request).await
}

pub fn router() -> Router<AppState> {
    let api = Router::new()
        // The session and the flow of signing in and up
        .route("/session", get(flow::session))
        .route("/sign-in/email", post(flow::email))
        .route("/sign-in/code", post(flow::code))
        .route("/sign-in/code/resend", post(flow::resend))
        .route("/sign-up/details", post(flow::details))
        .route("/terms", post(flow::terms))
        .route("/sign-out", post(flow::sign_out))
        .route("/passkey/sign-in/start", post(flow::passkey_start))
        .route("/passkey/sign-in/finish", post(flow::passkey_finish))
        .route("/ban", get(flow::show_ban))
        .route("/ban/appeal", post(flow::appeal))
        // The account
        .route("/account", get(account::get))
        .route("/account/passkeys/start", post(account::passkey_start))
        .route("/account/passkeys/finish", post(account::passkey_finish))
        .route(
            "/account/passkeys/{id}",
            patch(account::passkey_rename).delete(account::passkey_remove),
        )
        .route("/account/devices/{id}", delete(account::device_sign_out))
        .route("/account/sessions/{id}", delete(account::session_sign_out))
        .route(
            "/account/sign-out-everywhere",
            post(account::sign_out_everywhere),
        )
        .route("/account/export", get(account::export))
        .route("/account/email", post(account::email_start))
        .route("/account/email/confirm", post(account::email_confirm))
        .route(
            "/account/deletion",
            post(account::deletion_start).delete(account::deletion_cancel),
        )
        .route("/step-up/email", post(account::step_up_email))
        .route("/step-up/code", post(account::step_up_code))
        .route("/step-up/passkey/finish", post(account::step_up_passkey))
        // Friends
        .route("/friends", get(social::friend_lists))
        .route("/friends/requests", post(social::request_friend))
        .route("/friends/requests/{id}", delete(social::cancel_request))
        .route(
            "/friends/requests/{id}/accept",
            post(social::accept_request),
        )
        .route("/friends/{id}", delete(social::unfriend))
        .route("/blocks/{id}", put(social::block).delete(social::unblock))
        // The player's own profile and trophies
        .route(
            "/me/profile",
            get(social::my_profile).patch(social::update_profile),
        )
        .route(
            "/me/privacy",
            get(social::my_privacy).put(social::set_privacy),
        )
        .route(
            "/me/avatar",
            put(social::set_avatar).delete(social::remove_avatar),
        )
        .route("/me/trophies", get(social::my_trophies))
        // Connections
        .route("/me/links", get(links::list))
        .route(
            "/me/links/{provider}",
            patch(links::set_visibility).delete(links::unlink),
        )
        .route("/me/links/{provider}/start", post(links::start))
        // Other players
        .route("/users/{name}", get(social::user))
        .route("/users/{name}/trophies", get(social::user_trophies))
        .layer(middleware::from_fn(require_json));
    Router::new().nest(BASE, api)
}

/// The answer for a request that has nothing to say beyond "done".
pub fn done() -> ApiResult {
    Ok(axum::Json(json!({"ok": true})).into_response())
}

/// An answer with a status other than 200.
pub fn json_status(status: StatusCode, body: Value) -> Response {
    (status, axum::Json(body)).into_response()
}

// Public reads

/// Counts a read of a public profile against the limit per address.
pub fn limit_public(state: &AppState, ip: std::net::IpAddr) -> Result<(), AppError> {
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("portal:ip:{ip}"),
            rates.portal_public,
            rates.portal_public_window,
        )
        .map_err(AppError::rate_limited)
}

/// The account behind a handle, from the name in the address of a profile page.
pub async fn owner_of(state: &AppState, name: &str) -> Result<db::AccountRow, AppError> {
    let valid = (1..=16).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !valid {
        return Err(AppError::not_found());
    }
    db::account_by_handle(&state.db, &name.to_ascii_lowercase())
        .await?
        .ok_or_else(AppError::not_found)
}
