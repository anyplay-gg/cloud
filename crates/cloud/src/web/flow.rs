//! Signing in and creating an account in the browser, and handing the result to the app on the
//! player's PC.
//!
//! The app starts at [`authorize`] with a loopback address and a PKCE challenge (RFC 8252). The
//! player signs in by e-mail code or by passkey, creates an account when the address is new,
//! accepts the terms when they changed, and the cloud sends the browser to the loopback address
//! with a one-time code. The app trades the code and its verifier for tokens ([`super::native`]).
//!
//! The steps are functions that change the visit's session and say where the player goes next
//! ([`Landing`]). The HTML handlers below and the JSON API of the portal
//! ([`super::api`]) both call them, so there is one state machine.

use std::net::IpAddr;

use anyplay_api::ErrorCode;
use axum::extract::{Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};

use super::pages::{self, Field, TermsView};
use super::store::{self, Authorize, Login, SessionData};
use super::{
    Visit, WebError, WebForm, WebResult, check_csrf, existing, fresh_visit, limit_ip_api, page,
    redirect, redirect_plain, sign_in_session, view, without_cookie,
};
use crate::db::{self, AccountRow};
use crate::error::AppError;
use crate::http::ClientIp;
use crate::routes::auth;
use crate::state::AppState;
use crate::util::now;

/// The kind of one-time value that carries a finished sign-in to the app.
pub const AUTH_CODE: &str = "authcode";

/// What a code of the hand-off stands for.
#[derive(Serialize, Deserialize)]
pub struct Grant {
    pub account: u64,
    /// The PKCE challenge the app must answer with its verifier.
    pub challenge: String,
}

/// How the player proved who they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proof {
    /// A code sent to their address.
    Code,
    /// A passkey.
    Passkey,
}

/// Where a step of the flow leaves the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Landing {
    /// Back to the start of signing in.
    SignIn,
    /// Back to the start of creating an account.
    Create,
    /// A code is out; the player types it.
    Code,
    /// The address is verified; the player chooses a handle.
    Details,
    /// The player has to accept the terms.
    Terms,
    /// The sign-in is done and the app on the player's PC waits for its code.
    Handoff,
    /// The sign-in is done; the browser has its own session.
    Account,
    /// The account is banned: the statement shows.
    Banned,
}

impl Landing {
    /// The page of the HTML account web.
    pub fn path(self) -> &'static str {
        match self {
            Landing::SignIn => "/web/sign-in",
            Landing::Create => "/web/create",
            Landing::Code => "/web/code",
            Landing::Details => "/web/details",
            Landing::Terms => "/web/terms",
            Landing::Handoff => "/web/next",
            Landing::Account => "/web/account",
            Landing::Banned => "/web/banned",
        }
    }
}

#[derive(Deserialize)]
pub struct AuthorizeQuery {
    client: Option<String>,
    intent: Option<String>,
    redirect_uri: Option<String>,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    device: Option<String>,
}

fn invalid_request() -> WebError {
    WebError::page(
        StatusCode::BAD_REQUEST,
        "This sign-in link is not valid",
        "Start the sign-in again from AnyPlay.",
    )
}

/// The page the app opens in the system browser. Everything about the request is checked here;
/// what passes is kept in a session of its own, so no later page takes a redirect address from
/// the URL. The player goes on in the portal when the cloud or another service serves one, else on the HTML pages.
pub async fn authorize(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    query: Result<Query<AuthorizeQuery>, axum::extract::rejection::QueryRejection>,
) -> WebResult {
    let Ok(Query(q)) = query else {
        return Err(invalid_request());
    };
    limit_ip_api(&state, "authorize", ip, state.tuning.rates.code_per_ip)?;
    let redirect_uri = q
        .redirect_uri
        .as_deref()
        .and_then(anyplay_proof::loopback_redirect)
        .ok_or_else(invalid_request)?;
    let challenge = q
        .code_challenge
        .as_deref()
        .filter(|c| anyplay_proof::pkce::challenge_ok(c))
        .ok_or_else(invalid_request)?;
    if q.code_challenge_method.as_deref() != Some("S256") || q.client.as_deref() != Some("net") {
        return Err(invalid_request());
    }
    let sent_state = q
        .state
        .as_deref()
        .filter(|s| {
            (16..=128).contains(&s.len())
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .ok_or_else(invalid_request)?;
    let device = q
        .device
        .as_deref()
        .map(str::trim)
        .filter(|d| crate::util::is_label(d, 64))
        .unwrap_or("this PC")
        .to_owned();
    // This address replaces the browser's session with a new one, so a page of another site must
    // not be able to send the browser here: that would sign the player out of the account web
    // (SameSite=Strict only keeps the cookie off the request, it does not stop the new one).
    // The app's own start is opened by the system (`Sec-Fetch-Site: none`). A request that
    // another site started gets a page of ours that waits for a click, which is a same-origin
    // request and goes on.
    if headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|site| site == "cross-site")
    {
        let href = format!("/web/authorize?{}", raw.unwrap_or_default());
        return Err(WebError::page(
            StatusCode::OK,
            "Continue signing in to AnyPlay?",
            "AnyPlay on your PC asked for this sign-in, but another website opened this page. If you did not just start a sign-in in AnyPlay, close this tab.",
        )
        .with_action(&href, "Continue"));
    }
    let create = q.intent.as_deref() == Some("create");
    let data = SessionData {
        authorize: Some(Authorize {
            redirect_uri: redirect_uri.to_owned(),
            state: sent_state.to_owned(),
            challenge: challenge.to_owned(),
            create,
            device,
        }),
        ..SessionData::default()
    };
    let visit = fresh_visit(&state, data).await?;
    let portal = state.config.portal_landing();
    Ok(redirect(
        &state,
        &visit,
        match (portal, create) {
            (true, true) => "/create",
            (true, false) => "/sign-in",
            (false, true) => Landing::Create.path(),
            (false, false) => Landing::SignIn.path(),
        },
    ))
}

fn device_of(session: &store::Session) -> Option<&str> {
    session.data.authorize.as_ref().map(|a| a.device.as_str())
}

// The steps

/// Sends a code to `email` and waits for it in the session.
pub async fn begin_code(
    state: &AppState,
    ip: IpAddr,
    visit: &mut Visit,
    email: &str,
    create: bool,
) -> Result<Landing, AppError> {
    let (challenge, email) = auth::start_email_challenge(state, ip, email).await?;
    visit.session.data.login = Some(Login::Code {
        challenge,
        email,
        create,
    });
    store::save_data(&state.db, &visit.session).await?;
    Ok(Landing::Code)
}

/// Checks the code the player typed. A wrong code leaves the session as it is; an expired one
/// starts over. A right one signs the player in, or starts the sign-up when the address is new.
pub async fn verify_code(
    state: &AppState,
    ip: IpAddr,
    visit: &mut Visit,
    code: &str,
) -> Result<Landing, AppError> {
    let Some(Login::Code { challenge, .. }) = visit.session.data.login.clone() else {
        return Ok(Landing::SignIn);
    };
    limit_ip_api(state, "code", ip, state.tuning.rates.code_per_ip)?;
    match auth::check_challenge(state, &challenge, code.trim()).await {
        Ok(email) => {
            visit.session.data.login = None;
            if let Err(error) = auth::check_email_ban(state, &email).await {
                return ban_by_address(state, visit, &email, error).await;
            }
            match db::account_by_email(&state.db, &email).await? {
                Some(account) => finish_login(state, visit, &account, Proof::Code).await,
                None => {
                    if state.config.signup == anyplay_api::Signup::Closed {
                        return Err(auth::signups_closed());
                    }
                    let (signup, _) = auth::new_signup(state, &email).await?;
                    visit.session.data.login = Some(Login::NewAccount { signup, email });
                    store::save_data(&state.db, &visit.session).await?;
                    Ok(Landing::Details)
                }
            }
        }
        Err(error) if error.body.error == ErrorCode::CodeExpired => {
            visit.session.data.login = None;
            store::save_data(&state.db, &visit.session).await?;
            Err(error)
        }
        Err(error) => Err(error),
    }
}

/// Sends a new code to the address the player gave.
pub async fn resend_code(
    state: &AppState,
    ip: IpAddr,
    visit: &mut Visit,
) -> Result<Landing, AppError> {
    let Some(Login::Code { email, create, .. }) = visit.session.data.login.clone() else {
        return Ok(Landing::SignIn);
    };
    let (challenge, email) = auth::start_email_challenge(state, ip, &email).await?;
    visit.session.data.login = Some(Login::Code {
        challenge,
        email,
        create,
    });
    visit.session.data.notice = Some("We sent a new code.".to_owned());
    store::save_data(&state.db, &visit.session).await?;
    Ok(Landing::Code)
}

/// Why the details of a new account were not taken. `field` names what to mark.
pub struct DetailsError {
    pub field: Option<Field>,
    pub error: Box<AppError>,
}

impl DetailsError {
    fn of(error: AppError) -> Self {
        Self {
            field: None,
            error: Box::new(error),
        }
    }

    fn at(field: Field, error: AppError) -> Self {
        Self {
            field: Some(field),
            error: Box::new(error),
        }
    }
}

/// Makes the account for a verified address with the handle the player chose.
pub async fn create_from_details(
    state: &AppState,
    visit: &mut Visit,
    handle: &str,
    age_confirmed: bool,
    terms_accepted: bool,
) -> Result<Landing, DetailsError> {
    let Some(Login::NewAccount { signup, email }) = visit.session.data.login.clone() else {
        return Ok(Landing::Create);
    };
    let terms_version = state.config.terms();
    let handle = handle.trim();
    if !age_confirmed {
        return Err(DetailsError::at(
            Field::Age,
            AppError::bad_request(
                "Confirm that you are old enough to agree to the terms on your own.",
            ),
        ));
    }
    if terms_version.is_some() && !terms_accepted {
        return Err(DetailsError::at(
            Field::Terms,
            AppError::bad_request("Accept the terms to continue."),
        ));
    }
    match auth::create_account(state, &signup, handle, true, terms_version).await {
        Ok(account) => {
            visit.session.data.login = None;
            finish_login(state, visit, &account, Proof::Code)
                .await
                .map_err(DetailsError::of)
        }
        Err(error) => match error.body.error {
            ErrorCode::HandleInvalid | ErrorCode::HandleTaken | ErrorCode::BadRequest => {
                Err(DetailsError::at(Field::Handle, error))
            }
            ErrorCode::Banned => ban_by_address(state, visit, &email, error)
                .await
                .map_err(DetailsError::of),
            ErrorCode::CodeExpired => {
                visit.session.data.login = None;
                store::save_data(&state.db, &visit.session)
                    .await
                    .map_err(|e| DetailsError::of(e.into()))?;
                Err(DetailsError::of(error))
            }
            _ => Err(DetailsError::of(error)),
        },
    }
}

/// Records that the player accepted the terms they were asked about at sign-in.
pub async fn accept_terms(
    state: &AppState,
    visit: &mut Visit,
    accept: bool,
) -> Result<Landing, AppError> {
    let (Some(Login::Terms { account, passkey }), Some(version)) =
        (visit.session.data.login.clone(), state.config.terms())
    else {
        return Ok(Landing::SignIn);
    };
    if !accept {
        return Err(AppError::bad_request("Accept the terms to continue."));
    }
    store::accept_terms(&state.db, account, version, now()).await?;
    visit.session.data.login = None;
    let proof = if passkey { Proof::Passkey } else { Proof::Code };
    complete_login(state, visit, account, proof).await
}

/// An address with an active ban. When the account still exists the player can see the statement
/// and appeal; the answer is [`Landing::Banned`]. When it is gone, only the statement shows: the
/// error carries it.
async fn ban_by_address(
    state: &AppState,
    visit: &mut Visit,
    email: &str,
    error: AppError,
) -> Result<Landing, AppError> {
    if error.body.error != ErrorCode::Banned {
        return Err(error);
    }
    if let Some(account) = db::account_by_email(&state.db, email).await? {
        return super::ban::send_to(state, visit, account.id()).await;
    }
    Err(error)
}

/// A player proved who they are (code or passkey). Checks what stands between them and the
/// account: a ban, and terms they have not accepted.
pub async fn finish_login(
    state: &AppState,
    visit: &mut Visit,
    account: &AccountRow,
    proof: Proof,
) -> Result<Landing, AppError> {
    if state.ban_of(account.id()).is_some() {
        return super::ban::send_to(state, visit, account.id()).await;
    }
    if let Some(version) = state.config.terms()
        && store::terms_of(&state.db, account.id()).await?.as_deref() != Some(version)
    {
        visit.session.data.login = Some(Login::Terms {
            account: account.id(),
            passkey: proof == Proof::Passkey,
        });
        store::save_data(&state.db, &visit.session).await?;
        return Ok(Landing::Terms);
    }
    complete_login(state, visit, account.id(), proof).await
}

/// The last step of a sign-in. For the app on the player's PC it makes the one-time code and
/// keeps the address to send the browser to; for the account page it starts a browser session.
pub async fn complete_login(
    state: &AppState,
    visit: &mut Visit,
    account: u64,
    proof: Proof,
) -> Result<Landing, AppError> {
    if let Some(authorize) = visit.session.data.authorize.take() {
        let grant = Grant {
            account,
            challenge: authorize.challenge.clone(),
        };
        let code = store::flow_put(
            &state.db,
            AUTH_CODE,
            &serde_json::to_string(&grant).unwrap_or_default(),
            state.tuning.link_ttl,
        )
        .await?;
        visit.session.data.login = None;
        visit.session.data.handoff = Some(format!(
            "{}?code={code}&state={}",
            authorize.redirect_uri, authorize.state
        ));
        store::save_data(&state.db, &visit.session).await?;
        return Ok(Landing::Handoff);
    }
    store::end_session(&state.db, &visit.session).await?;
    let data = SessionData {
        passkey_ms: (proof == Proof::Passkey).then(crate::util::now_ms),
        ..SessionData::default()
    };
    *visit = sign_in_session(state, account, data).await?;
    Ok(Landing::Account)
}

// The pages

pub async fn sign_in_page(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> WebResult {
    let mut visit = super::visit(&state, ip, &headers).await?;
    if visit.session.account.is_some() && visit.session.data.authorize.is_none() {
        return Ok(redirect(&state, &visit, "/web/account"));
    }
    let v = view(&state, &mut visit.session).await?;
    let html = pages::sign_in(&v.ctx(), device_of(&visit.session), None);
    Ok(page(&state, &visit, StatusCode::OK, html))
}

pub async fn create_page(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> WebResult {
    let mut visit = super::visit(&state, ip, &headers).await?;
    if visit.session.account.is_some() && visit.session.data.authorize.is_none() {
        return Ok(redirect(&state, &visit, "/web/account"));
    }
    let v = view(&state, &mut visit.session).await?;
    let html = pages::create(&v.ctx(), device_of(&visit.session), None);
    Ok(page(&state, &visit, StatusCode::OK, html))
}

/// The session a form was posted from. A form without one is stale.
async fn posted(state: &AppState, headers: &HeaderMap, csrf: &str) -> Result<Visit, WebError> {
    let session = existing(state, headers)
        .await?
        .ok_or_else(WebError::expired)?;
    check_csrf(&session, csrf)?;
    Ok(Visit {
        session,
        token: None,
    })
}

#[derive(Deserialize)]
pub struct EmailForm {
    csrf: String,
    email: String,
    #[serde(default)]
    intent: String,
}

pub async fn email(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    WebForm(form): WebForm<EmailForm>,
) -> WebResult {
    let mut visit = posted(&state, &headers, &form.csrf).await?;
    let create = form.intent == "create";
    match begin_code(&state, ip, &mut visit, &form.email, create).await {
        Ok(landing) => Ok(redirect(&state, &visit, landing.path())),
        Err(error) if error.body.error == ErrorCode::BadRequest => {
            let v = view(&state, &mut visit.session).await?;
            let device = device_of(&visit.session);
            let html = if create {
                pages::create(&v.ctx(), device, Some(&error.body.message))
            } else {
                pages::sign_in(&v.ctx(), device, Some(&error.body.message))
            };
            Ok(page(&state, &visit, StatusCode::BAD_REQUEST, html))
        }
        Err(error) => Err(error.into()),
    }
}

pub async fn code_page(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> WebResult {
    let mut visit = super::visit(&state, ip, &headers).await?;
    let Some(Login::Code { email, create, .. }) = visit.session.data.login.clone() else {
        return Ok(redirect(&state, &visit, "/web/sign-in"));
    };
    let v = view(&state, &mut visit.session).await?;
    let html = pages::code(&v.ctx(), &email, create, None);
    Ok(page(&state, &visit, StatusCode::OK, html))
}

#[derive(Deserialize)]
pub struct CodeForm {
    csrf: String,
    code: String,
}

pub async fn code(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    WebForm(form): WebForm<CodeForm>,
) -> WebResult {
    let mut visit = posted(&state, &headers, &form.csrf).await?;
    let (email, create) = match &visit.session.data.login {
        Some(Login::Code { email, create, .. }) => (email.clone(), *create),
        _ => return Ok(redirect(&state, &visit, "/web/sign-in")),
    };
    match verify_code(&state, ip, &mut visit, &form.code).await {
        Ok(landing) => Ok(redirect(&state, &visit, landing.path())),
        Err(error) if error.body.error == ErrorCode::CodeWrong => {
            let v = view(&state, &mut visit.session).await?;
            let html = pages::code(
                &v.ctx(),
                &email,
                create,
                Some("That code is wrong. Check your e-mail and try again."),
            );
            Ok(page(&state, &visit, StatusCode::BAD_REQUEST, html))
        }
        Err(error) if error.body.error == ErrorCode::CodeExpired => {
            let v = view(&state, &mut visit.session).await?;
            let html = pages::sign_in(
                &v.ctx(),
                device_of(&visit.session),
                Some("That code has expired. Get a new one."),
            );
            Ok(page(&state, &visit, StatusCode::BAD_REQUEST, html))
        }
        Err(error) if error.body.error == ErrorCode::Banned => {
            banned_without_account(&state, &mut visit, &error).await
        }
        Err(error) => Err(error.into()),
    }
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf: String,
}

pub async fn resend(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    WebForm(form): WebForm<CsrfForm>,
) -> WebResult {
    let mut visit = posted(&state, &headers, &form.csrf).await?;
    let landing = resend_code(&state, ip, &mut visit).await?;
    Ok(redirect(&state, &visit, landing.path()))
}

pub async fn details_page(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> WebResult {
    let mut visit = super::visit(&state, ip, &headers).await?;
    let Some(Login::NewAccount { email, .. }) = visit.session.data.login.clone() else {
        return Ok(redirect(&state, &visit, "/web/create"));
    };
    let v = view(&state, &mut visit.session).await?;
    let terms = state.config.terms().map(|version| TermsView { version });
    let html = pages::details(&v.ctx(), &email, terms.as_ref(), "", None);
    Ok(page(&state, &visit, StatusCode::OK, html))
}

#[derive(Deserialize)]
pub struct DetailsForm {
    csrf: String,
    handle: String,
    age: Option<String>,
    terms: Option<String>,
}

pub async fn details(
    State(state): State<AppState>,
    headers: HeaderMap,
    WebForm(form): WebForm<DetailsForm>,
) -> WebResult {
    let mut visit = posted(&state, &headers, &form.csrf).await?;
    let Some(Login::NewAccount { email, .. }) = visit.session.data.login.clone() else {
        return Ok(redirect(&state, &visit, "/web/create"));
    };
    let handle = form.handle.trim().to_owned();
    let result = create_from_details(
        &state,
        &mut visit,
        &handle,
        form.age.as_deref() == Some("yes"),
        form.terms.as_deref() == Some("yes"),
    )
    .await;
    match result {
        Ok(landing) => Ok(redirect(&state, &visit, landing.path())),
        Err(DetailsError {
            field: Some(field),
            error,
        }) => {
            let v = view(&state, &mut visit.session).await?;
            let terms = state.config.terms().map(|version| TermsView { version });
            let html = pages::details(
                &v.ctx(),
                &email,
                terms.as_ref(),
                &handle,
                Some((field, &error.body.message)),
            );
            Ok(page(&state, &visit, StatusCode::BAD_REQUEST, html))
        }
        Err(DetailsError { field: None, error }) => match error.body.error {
            ErrorCode::Banned => banned_without_account(&state, &mut visit, &error).await,
            ErrorCode::CodeExpired => {
                let v = view(&state, &mut visit.session).await?;
                let html = pages::create(
                    &v.ctx(),
                    device_of(&visit.session),
                    Some("Your sign-up expired. Start again with a new code."),
                );
                Ok(page(&state, &visit, StatusCode::BAD_REQUEST, html))
            }
            _ => Err((*error).into()),
        },
    }
}

pub async fn terms_page(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> WebResult {
    let mut visit = super::visit(&state, ip, &headers).await?;
    let (Some(Login::Terms { .. }), Some(version)) =
        (visit.session.data.login.clone(), state.config.terms())
    else {
        return Ok(redirect(&state, &visit, "/web/sign-in"));
    };
    let v = view(&state, &mut visit.session).await?;
    let html = pages::terms(&v.ctx(), &TermsView { version }, None);
    Ok(page(&state, &visit, StatusCode::OK, html))
}

#[derive(Deserialize)]
pub struct TermsForm {
    csrf: String,
    accept: Option<String>,
}

pub async fn terms(
    State(state): State<AppState>,
    headers: HeaderMap,
    WebForm(form): WebForm<TermsForm>,
) -> WebResult {
    let mut visit = posted(&state, &headers, &form.csrf).await?;
    match accept_terms(&state, &mut visit, form.accept.as_deref() == Some("yes")).await {
        Ok(landing) => Ok(redirect(&state, &visit, landing.path())),
        Err(error) if error.body.error == ErrorCode::BadRequest => {
            let Some(version) = state.config.terms() else {
                return Err(error.into());
            };
            let v = view(&state, &mut visit.session).await?;
            let html = pages::terms(&v.ctx(), &TermsView { version }, Some(&error.body.message));
            Ok(page(&state, &visit, StatusCode::BAD_REQUEST, html))
        }
        Err(error) => Err(error.into()),
    }
}

/// An address with an active ban and no account: only the statement shows.
async fn banned_without_account(
    state: &AppState,
    visit: &mut Visit,
    error: &AppError,
) -> WebResult {
    let v = view(state, &mut visit.session).await?;
    let html = super::ban::without_account(&v.ctx(), error);
    Ok(page(state, visit, StatusCode::FORBIDDEN, html))
}

/// Hands the browser back to the app: shows the page that goes to the loopback address.
pub async fn next(State(state): State<AppState>, headers: HeaderMap) -> WebResult {
    let Some(mut session) = existing(&state, &headers).await? else {
        return Ok(redirect_plain("/web/sign-in"));
    };
    let Some(url) = session.data.handoff.take() else {
        return Ok(redirect_plain("/web/account"));
    };
    // The session has done its job: nothing of this sign-in stays in the browser.
    store::end_session(&state.db, &session).await?;
    let v = super::View {
        cloud_name: state.config.name.clone(),
        csrf: String::new(),
        handle: None,
        privacy_url: state.config.privacy_url.clone(),
        terms_url: state.config.terms_url.clone(),
        notice: None,
    };
    let html = pages::back_to_app(&v.ctx(), &url);
    let response = (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    );
    Ok(without_cookie(
        &state,
        axum::response::IntoResponse::into_response(response),
    ))
}
