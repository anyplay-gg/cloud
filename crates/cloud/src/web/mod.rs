//! The account web: sign in, create an account and manage it in the player's browser.
//!
//! It is served by the cloud itself, on the origin of `public_url`, so a cloud that anybody runs
//! has it. The pages are server rendered ([`pages`]) with one small script for passkeys and a
//! strict content security policy; there is no third-party script, font or image. A browser has
//! its own short-lived session cookie ([`store`]), apart from the tokens of devices. See
//! `docs/ARCHITECTURE.md` ("Security") and network ADR 0016.
//!
//! * [`flow`]: sign in by e-mail code, create an account, the terms, and the hand-off to the app
//!   on the player's PC (a one-time code sent to its loopback address, RFC 8252 with PKCE).
//! * [`native`]: the two API calls of that hand-off, and the link from the app to this site.
//! * [`account`]: the account page.
//! * [`passkeys`]: WebAuthn ceremonies (webauthn-rs) for adding a passkey and signing in with one.
//! * [`api`]: the JSON API of the web portal, `/web/api/v1`.
//! * [`portal`]: serves the built portal, a single-page app, from the same origin.

use std::time::Duration;

use axum::Router;
use axum::extract::{FromRequest, Request, State};
use axum::http::header::{self, HeaderName, HeaderValue};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::de::DeserializeOwned;
use webauthn_rs::prelude::{Url, Webauthn, WebauthnBuilder};

use crate::config::Config;
use crate::error::AppError;
use crate::state::AppState;

pub mod account;
pub mod api;
pub mod ban;
pub mod flow;
pub mod native;
pub mod pages;
pub mod passkeys;
pub mod portal;
pub mod store;

mod assets;

use pages::Ctx;
use store::{Session, SessionData};

/// What the account web needs beyond the shared state: its origin and the relying party of
/// passkeys.
pub struct Web {
    /// The origin the pages and passkeys belong to, such as `https://play.example.com`.
    pub origin: String,
    pub webauthn: Webauthn,
    cookie: &'static str,
    secure: bool,
}

impl Web {
    /// `None` when the cloud has no `public_url`.
    pub fn new(config: &Config) -> anyhow::Result<Option<Self>> {
        let Some(origin) = config.web_origin() else {
            return Ok(None);
        };
        let url = Url::parse(&origin)?;
        let host = url
            .domain()
            .ok_or_else(|| anyhow::anyhow!("public_url needs a domain name"))?
            .to_owned();
        let webauthn = WebauthnBuilder::new(&host, &url)?
            .rp_name(&config.name)
            .build()?;
        let secure = origin.starts_with("https://");
        Ok(Some(Self {
            origin,
            webauthn,
            // The `__Host-` prefix makes a browser refuse the cookie unless it is Secure, has
            // no Domain and Path is `/`. It needs https, so a development cloud on localhost
            // uses a plain name.
            cookie: if secure {
                "__Host-anyplay"
            } else {
                "anyplay_web"
            },
            secure,
        }))
    }
}

pub fn router(state: AppState) -> Router<AppState> {
    let mut router = Router::new()
        .route("/web", get(|| async { redirect_plain("/web/account") }))
        .route("/web/authorize", get(flow::authorize))
        .route("/web/sign-in", get(flow::sign_in_page))
        .route("/web/create", get(flow::create_page))
        .route("/web/email", post(flow::email))
        .route("/web/code", get(flow::code_page).post(flow::code))
        .route("/web/code/resend", post(flow::resend))
        .route("/web/details", get(flow::details_page).post(flow::details))
        .route("/web/terms", get(flow::terms_page).post(flow::terms))
        .route("/web/next", get(flow::next))
        .route("/web/banned", get(ban::show))
        .route("/web/appeal", post(ban::appeal))
        .route("/web/link", get(native::link_page).post(native::link_open))
        .route("/web/sign-out", post(account::sign_out))
        .route("/web/account", get(account::page))
        .route(
            "/web/account/passkeys/{id}/remove",
            post(account::remove_passkey),
        )
        .route(
            "/web/account/devices/{id}/sign-out",
            post(account::sign_out_device),
        )
        .route(
            "/web/account/sign-out-everywhere",
            post(account::sign_out_everywhere),
        )
        .route("/web/account/export", get(account::export))
        .route(
            "/web/account/delete",
            get(account::delete_page).post(account::delete),
        )
        .route("/web/account/delete/cancel", post(account::cancel_delete))
        .route(
            "/web/account/passkeys/start",
            post(passkeys::register_start),
        )
        .route(
            "/web/account/passkeys/finish",
            post(passkeys::register_finish),
        )
        .route("/web/passkey/sign-in/start", post(passkeys::sign_in_start))
        .route(
            "/web/passkey/sign-in/finish",
            post(passkeys::sign_in_finish),
        )
        .route("/web/links/{provider}/callback", get(api::links::callback))
        .route("/web/assets/{name}", get(assets::serve))
        .merge(api::router());
    if state.config.portal_dir.is_some() {
        router = router.merge(portal::router(&state));
    }
    router.layer(middleware::from_fn_with_state(state, guard))
}

/// The content security policy of every page: nothing from anywhere but this origin, no inline
/// script or style, no framing, no plugins.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; \
                       img-src 'self'; connect-src 'self'; form-action 'self'; base-uri 'none'; \
                       frame-ancestors 'none'";

/// The policy of the portal's own pages: [`CSP`], and pictures also from the servers of the
/// services whose accounts a player can link, which show their avatar on the profile.
pub const PORTAL_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self';                        img-src 'self' https://cdn.discordapp.com https://*.steamstatic.com;                        connect-src 'self'; form-action 'self'; base-uri 'none';                        frame-ancestors 'none'";

/// Adds the security headers to every answer and turns away a request that another site made
/// the browser send.
async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let secure = state.web.as_ref().is_some_and(|w| w.secure);
    if request.method() != Method::GET && request.method() != Method::HEAD {
        let origin = state.web.as_ref().map(|w| w.origin.as_str());
        let headers = request.headers();
        let origin_ok = match headers.get(header::ORIGIN).map(|v| v.to_str()) {
            None => true,
            Some(Ok(sent)) => origin == Some(sent),
            Some(Err(_)) => false,
        };
        let site_ok = match headers.get("sec-fetch-site").map(|v| v.to_str()) {
            None => true,
            Some(Ok(site)) => matches!(site, "same-origin" | "none"),
            Some(Err(_)) => false,
        };
        if !(origin_ok && site_ok) {
            return WebError::page(
                StatusCode::FORBIDDEN,
                "Request refused",
                "This request came from another site. Open the page from AnyPlay again.",
            )
            .into_response();
        }
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    let set = |headers: &mut HeaderMap, name: &'static str, value: &'static str| {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    };
    if !headers.contains_key("content-security-policy") {
        set(headers, "content-security-policy", CSP);
    }
    set(headers, "x-content-type-options", "nosniff");
    set(headers, "referrer-policy", "no-referrer");
    set(headers, "cross-origin-opener-policy", "same-origin");
    set(headers, "cross-origin-resource-policy", "same-origin");
    set(headers, "x-frame-options", "DENY");
    set(
        headers,
        "permissions-policy",
        "publickey-credentials-get=(self), publickey-credentials-create=(self), camera=(), microphone=(), geolocation=()",
    );
    if !headers.contains_key(header::CACHE_CONTROL) {
        set(headers, "cache-control", "no-store");
    }
    if secure {
        set(headers, "strict-transport-security", "max-age=31536000");
    }
    response
}

// Errors

/// A failure shown as a page (or a redirect).
pub struct WebError {
    status: StatusCode,
    title: String,
    message: String,
    action: Option<(String, String)>,
    redirect: Option<&'static str>,
}

impl WebError {
    pub fn page(status: StatusCode, title: &str, message: &str) -> Self {
        Self {
            status,
            title: title.to_owned(),
            message: message.to_owned(),
            action: None,
            redirect: None,
        }
    }

    /// Adds a button to the page.
    pub fn with_action(mut self, href: &str, label: &str) -> Self {
        self.action = Some((href.to_owned(), label.to_owned()));
        self
    }

    /// Go to another page of ours instead.
    pub fn go(to: &'static str) -> Self {
        Self {
            redirect: Some(to),
            ..Self::page(StatusCode::SEE_OTHER, "", "")
        }
    }

    /// The visitor has no session for this: send them to sign in.
    pub fn sign_in() -> Self {
        Self::go("/web/sign-in")
    }

    /// The form came without a session or with a token that does not fit it.
    pub fn expired() -> Self {
        Self::page(
            StatusCode::FORBIDDEN,
            "This page expired",
            "Go back, reload the page and try again.",
        )
        .with_action("/web/sign-in", "Start again")
    }

    pub fn fresh() -> Self {
        Self::page(
            StatusCode::FORBIDDEN,
            "Sign in again",
            "For your safety, sign in again before you do this.",
        )
        .with_action("/web/sign-in", "Sign in")
    }
}

impl From<AppError> for WebError {
    fn from(error: AppError) -> Self {
        use anyplay_api::ErrorCode as Code;
        let status = StatusCode::from_u16(error.body.error.status())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let title = match error.body.error {
            Code::RateLimited => "Too many tries",
            Code::Banned => "This account is banned",
            Code::Internal => "Something went wrong",
            Code::Unauthorized => "Sign in again",
            _ => "Could not continue",
        };
        let mut message = error.body.message.clone();
        if let Some(statement) = error.body.ban.as_ref().and_then(|b| b.statement.as_ref()) {
            message = format!(
                "{message} Rule: {}. Reason: {}",
                statement.rule.as_str(),
                statement.reason
            );
        }
        let mut page = Self::page(status, title, &message);
        if error.body.error == Code::Unauthorized {
            page = page.with_action("/web/sign-in", "Sign in");
        }
        page
    }
}

impl From<sqlx::Error> for WebError {
    fn from(error: sqlx::Error) -> Self {
        AppError::from(error).into()
    }
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        if let Some(to) = self.redirect {
            return redirect_plain(to);
        }
        let ctx = Ctx {
            cloud_name: "AnyPlay",
            csrf: "",
            handle: None,
            privacy_url: None,
            terms_url: None,
            notice: None,
        };
        let html = pages::error(
            &ctx,
            &self.title,
            &self.message,
            self.action
                .as_ref()
                .map(|(href, label)| (href.as_str(), label.as_str())),
        );
        (self.status, html_headers(), html).into_response()
    }
}

pub type WebResult = Result<Response, WebError>;

fn html_headers() -> [(header::HeaderName, &'static str); 1] {
    [(header::CONTENT_TYPE, "text/html; charset=utf-8")]
}

/// A 303 to a path of ours.
pub fn redirect_plain(to: &str) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    if let Ok(value) = HeaderValue::from_str(to) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

// Forms

/// A `application/x-www-form-urlencoded` body of at most 16 KiB.
pub struct WebForm<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for WebForm<T> {
    type Rejection = WebError;

    async fn from_request(request: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let bad = || {
            WebError::page(
                StatusCode::BAD_REQUEST,
                "Could not read the form",
                "Go back, reload the page and try again.",
            )
        };
        let is_form = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("application/x-www-form-urlencoded"));
        if !is_form {
            return Err(bad());
        }
        let bytes = axum::body::to_bytes(request.into_body(), 16 * 1024)
            .await
            .map_err(|_| bad())?;
        serde_urlencoded::from_bytes(&bytes)
            .map(WebForm)
            .map_err(|_| bad())
    }
}

// Sessions

/// The session of a request, and the cookie to set when it is new.
pub struct Visit {
    pub session: Session,
    token: Option<String>,
}

impl Visit {
    pub fn is_new(&self) -> bool {
        self.token.is_some()
    }
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v)
}

fn web_of(state: &AppState) -> Result<&Web, WebError> {
    state.web.as_ref().ok_or_else(|| {
        WebError::page(
            StatusCode::NOT_FOUND,
            "Not found",
            "This cloud has no account web.",
        )
    })
}

/// The session the browser's cookie names, if it is alive.
pub async fn existing(state: &AppState, headers: &HeaderMap) -> Result<Option<Session>, WebError> {
    let web = web_of(state)?;
    let Some(token) = cookie_value(headers, web.cookie) else {
        return Ok(None);
    };
    Ok(store::load_session(
        &state.db,
        token,
        state.tuning.web_idle,
        state.tuning.web_max,
    )
    .await?)
}

/// The session of the browser, or a new one for a sign-in in progress.
pub async fn visit(
    state: &AppState,
    ip: std::net::IpAddr,
    headers: &HeaderMap,
) -> Result<Visit, WebError> {
    if let Some(session) = existing(state, headers).await? {
        return Ok(Visit {
            session,
            token: None,
        });
    }
    // A page load without a cookie costs a row; it counts in the same bucket as `GET /session`.
    limit_ip(
        state,
        "session",
        ip,
        state.tuning.rates.portal_sessions_per_ip,
    )?;
    Ok(fresh_visit(state, SessionData::default()).await?)
}

/// A new anonymous session with `data`, whatever cookie the browser had.
pub async fn fresh_visit(state: &AppState, data: SessionData) -> Result<Visit, AppError> {
    let (token, session) =
        store::create_session(&state.db, None, &data, state.tuning.web_idle).await?;
    Ok(Visit {
        session,
        token: Some(token),
    })
}

/// A session for a player who signed in, replacing `old`.
pub async fn sign_in_session(
    state: &AppState,
    account: u64,
    data: SessionData,
) -> Result<Visit, AppError> {
    let (token, session) =
        store::create_session(&state.db, Some(account), &data, state.tuning.web_idle).await?;
    Ok(Visit {
        session,
        token: Some(token),
    })
}

fn set_cookie(web: &Web, token: &str) -> String {
    format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Strict{}",
        web.cookie,
        if web.secure { "; Secure" } else { "" }
    )
}

fn clear_cookie(web: &Web) -> String {
    format!(
        "{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}",
        web.cookie,
        if web.secure { "; Secure" } else { "" }
    )
}

/// Adds the cookie of a new session to an answer.
pub fn with_cookie(state: &AppState, visit: &Visit, mut response: Response) -> Response {
    if let (Some(web), Some(token)) = (state.web.as_ref(), visit.token.as_deref())
        && let Ok(value) = HeaderValue::from_str(&set_cookie(web, token))
    {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

/// Tells the browser to forget its cookie.
pub fn without_cookie(state: &AppState, mut response: Response) -> Response {
    if let Some(web) = state.web.as_ref()
        && let Ok(value) = HeaderValue::from_str(&clear_cookie(web))
    {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

/// The answer `html` with the status, and the session's cookie when it is new.
pub fn page(state: &AppState, visit: &Visit, status: StatusCode, html: String) -> Response {
    with_cookie(state, visit, (status, html_headers(), html).into_response())
}

/// A 303 to `to`, with the session's cookie when it is new.
pub fn redirect(state: &AppState, visit: &Visit, to: &str) -> Response {
    with_cookie(state, visit, redirect_plain(to))
}

/// Checks the token a form (or script) sent against the session's.
pub fn check_csrf(session: &Session, sent: &str) -> Result<(), WebError> {
    if session.csrf_ok(sent) {
        Ok(())
    } else {
        Err(WebError::expired())
    }
}

/// Counts a request against a limit per address.
pub fn limit_ip(
    state: &AppState,
    name: &str,
    ip: std::net::IpAddr,
    limit: u32,
) -> Result<(), WebError> {
    state
        .limiter
        .hit(
            &format!("{name}:ip:{}", crate::http::rate_key(ip)),
            limit,
            state.tuning.rates.auth_window,
        )
        .map_err(|seconds| AppError::rate_limited(seconds).into())
}

/// What a page needs to draw itself, owned so that a handler can build the page after changing
/// the session.
pub struct View {
    cloud_name: String,
    csrf: String,
    handle: Option<String>,
    privacy_url: Option<String>,
    terms_url: Option<String>,
    notice: Option<String>,
}

impl View {
    pub fn ctx(&self) -> Ctx<'_> {
        Ctx {
            cloud_name: &self.cloud_name,
            csrf: &self.csrf,
            handle: self.handle.as_deref(),
            privacy_url: self.privacy_url.as_deref(),
            terms_url: self.terms_url.as_deref(),
            notice: self.notice.as_deref(),
        }
    }
}

/// Builds the view of a session. A notice left by the last action is shown once.
pub async fn view(state: &AppState, session: &mut Session) -> Result<View, WebError> {
    let handle = match session.account {
        Some(account) => crate::db::account(&state.db, account)
            .await?
            .map(|a| a.handle),
        None => None,
    };
    let notice = session.data.notice.take();
    if notice.is_some() {
        store::save_data(&state.db, session).await?;
    }
    Ok(View {
        cloud_name: state.config.name.clone(),
        csrf: session.csrf.clone(),
        handle,
        privacy_url: state.config.privacy_url.clone(),
        terms_url: state.config.terms_url.clone(),
        notice,
    })
}

/// How long a one-time value of the hand-off lives.
pub fn link_ttl(state: &AppState) -> Duration {
    state.tuning.link_ttl
}

/// The session of a script request. Scripts get JSON errors, not pages.
pub async fn existing_api(state: &AppState, headers: &HeaderMap) -> Result<Session, AppError> {
    let session = existing(state, headers)
        .await
        .map_err(|_| AppError::internal())?;
    session.ok_or_else(|| AppError::unauthorized("This page expired. Reload it and try again."))
}

/// Checks the CSRF token a script sent in `X-CSRF-Token`.
pub fn check_csrf_api(session: &Session, headers: &HeaderMap) -> Result<(), AppError> {
    let sent = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if session.csrf_ok(sent) {
        Ok(())
    } else {
        Err(AppError::unauthorized(
            "This page expired. Reload it and try again.",
        ))
    }
}

/// Counts a script request against a limit per address.
pub fn limit_ip_api(
    state: &AppState,
    name: &str,
    ip: std::net::IpAddr,
    limit: u32,
) -> Result<(), AppError> {
    state
        .limiter
        .hit(
            &format!("{name}:ip:{}", crate::http::rate_key(ip)),
            limit,
            state.tuning.rates.auth_window,
        )
        .map_err(AppError::rate_limited)
}
