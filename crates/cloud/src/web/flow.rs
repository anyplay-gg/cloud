//! Signing in and creating an account in the browser, and handing the result to the app on the
//! player's PC.
//!
//! The app starts at [`authorize`] with a loopback address and a PKCE challenge (RFC 8252). The
//! player signs in by e-mail code or by passkey, creates an account when the address is new,
//! accepts the terms when they changed, and the cloud sends the browser to the loopback address
//! with a one-time code. The app trades the code and its verifier for tokens ([`super::native`]).

use anyplay_api::ErrorCode;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};

use super::pages::{self, Field, TermsView};
use super::store::{self, Authorize, Login, SessionData};
use super::{
    Visit, WebError, WebForm, WebResult, check_csrf, existing, fresh_visit, limit_ip, page,
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
/// the URL.
pub async fn authorize(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    query: Result<Query<AuthorizeQuery>, axum::extract::rejection::QueryRejection>,
) -> WebResult {
    let Ok(Query(q)) = query else {
        return Err(invalid_request());
    };
    limit_ip(&state, "authorize", ip, state.tuning.rates.code_per_ip)?;
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
    Ok(redirect(
        &state,
        &visit,
        if create {
            "/web/create"
        } else {
            "/web/sign-in"
        },
    ))
}

fn device_of(session: &store::Session) -> Option<&str> {
    session.data.authorize.as_ref().map(|a| a.device.as_str())
}

pub async fn sign_in_page(State(state): State<AppState>, headers: HeaderMap) -> WebResult {
    let mut visit = super::visit(&state, &headers).await?;
    if visit.session.account.is_some() && visit.session.data.authorize.is_none() {
        return Ok(redirect(&state, &visit, "/web/account"));
    }
    let v = view(&state, &mut visit.session).await?;
    let html = pages::sign_in(&v.ctx(), device_of(&visit.session), None);
    Ok(page(&state, &visit, StatusCode::OK, html))
}

pub async fn create_page(State(state): State<AppState>, headers: HeaderMap) -> WebResult {
    let mut visit = super::visit(&state, &headers).await?;
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
    match auth::start_email_challenge(&state, ip, &form.email).await {
        Ok((challenge, email)) => {
            visit.session.data.login = Some(Login::Code {
                challenge,
                email,
                create,
            });
            store::save_data(&state.db, &visit.session).await?;
            Ok(redirect(&state, &visit, "/web/code"))
        }
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

pub async fn code_page(State(state): State<AppState>, headers: HeaderMap) -> WebResult {
    let mut visit = super::visit(&state, &headers).await?;
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
    let Some(Login::Code {
        challenge,
        email,
        create,
    }) = visit.session.data.login.clone()
    else {
        return Ok(redirect(&state, &visit, "/web/sign-in"));
    };
    limit_ip(&state, "code", ip, state.tuning.rates.code_per_ip)?;
    match auth::check_challenge(&state, &challenge, form.code.trim()).await {
        Ok(email) => {
            visit.session.data.login = None;
            if let Err(error) = auth::check_email_ban(&state, &email).await {
                return banned_by_address(&state, visit, &email, error).await;
            }
            match db::account_by_email(&state.db, &email).await? {
                Some(account) => {
                    let (visit, to) = finish_login(&state, visit, &account).await?;
                    Ok(redirect(&state, &visit, to))
                }
                None => {
                    if state.config.signup == anyplay_api::Signup::Closed {
                        return Err(auth::signups_closed().into());
                    }
                    let (signup, _) = auth::new_signup(&state, &email).await?;
                    visit.session.data.login = Some(Login::NewAccount { signup, email });
                    store::save_data(&state.db, &visit.session).await?;
                    Ok(redirect(&state, &visit, "/web/details"))
                }
            }
        }
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
            visit.session.data.login = None;
            store::save_data(&state.db, &visit.session).await?;
            let v = view(&state, &mut visit.session).await?;
            let html = pages::sign_in(
                &v.ctx(),
                device_of(&visit.session),
                Some("That code has expired. Get a new one."),
            );
            Ok(page(&state, &visit, StatusCode::BAD_REQUEST, html))
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
    let Some(Login::Code { email, create, .. }) = visit.session.data.login.clone() else {
        return Ok(redirect(&state, &visit, "/web/sign-in"));
    };
    let (challenge, email) = auth::start_email_challenge(&state, ip, &email).await?;
    visit.session.data.login = Some(Login::Code {
        challenge,
        email,
        create,
    });
    store::save_data(&state.db, &visit.session).await?;
    visit.session.data.notice = Some("We sent a new code.".to_owned());
    store::save_data(&state.db, &visit.session).await?;
    Ok(redirect(&state, &visit, "/web/code"))
}

pub async fn details_page(State(state): State<AppState>, headers: HeaderMap) -> WebResult {
    let mut visit = super::visit(&state, &headers).await?;
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
    let Some(Login::NewAccount { signup, email }) = visit.session.data.login.clone() else {
        return Ok(redirect(&state, &visit, "/web/create"));
    };
    let terms_version = state.config.terms();
    let handle = form.handle.trim().to_owned();
    let problem = if form.age.as_deref() != Some("yes") {
        Some((
            Field::Age,
            "Confirm that you are old enough to agree to the terms on your own.".to_owned(),
        ))
    } else if terms_version.is_some() && form.terms.as_deref() != Some("yes") {
        Some((Field::Terms, "Accept the terms to continue.".to_owned()))
    } else {
        None
    };
    let result = match problem {
        Some(message) => Err(message),
        None => match auth::create_account(&state, &signup, &handle, true, terms_version).await {
            Ok(account) => Ok(account),
            Err(error) => match error.body.error {
                ErrorCode::HandleInvalid | ErrorCode::HandleTaken | ErrorCode::BadRequest => {
                    Err((Field::Handle, error.body.message))
                }
                ErrorCode::Banned => {
                    return banned_by_address(&state, visit, &email, error).await;
                }
                ErrorCode::CodeExpired => {
                    visit.session.data.login = None;
                    store::save_data(&state.db, &visit.session).await?;
                    let v = view(&state, &mut visit.session).await?;
                    let html = pages::create(
                        &v.ctx(),
                        device_of(&visit.session),
                        Some("Your sign-up expired. Start again with a new code."),
                    );
                    return Ok(page(&state, &visit, StatusCode::BAD_REQUEST, html));
                }
                _ => return Err(error.into()),
            },
        },
    };
    match result {
        Ok(account) => {
            visit.session.data.login = None;
            let (visit, to) = finish_login(&state, visit, &account).await?;
            Ok(redirect(&state, &visit, to))
        }
        Err((field, message)) => {
            let v = view(&state, &mut visit.session).await?;
            let terms = terms_version.map(|version| TermsView { version });
            let html = pages::details(
                &v.ctx(),
                &email,
                terms.as_ref(),
                &handle,
                Some((field, &message)),
            );
            Ok(page(&state, &visit, StatusCode::BAD_REQUEST, html))
        }
    }
}

pub async fn terms_page(State(state): State<AppState>, headers: HeaderMap) -> WebResult {
    let mut visit = super::visit(&state, &headers).await?;
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
    let (Some(Login::Terms { account }), Some(version)) =
        (visit.session.data.login.clone(), state.config.terms())
    else {
        return Ok(redirect(&state, &visit, "/web/sign-in"));
    };
    if form.accept.as_deref() != Some("yes") {
        let v = view(&state, &mut visit.session).await?;
        let html = pages::terms(
            &v.ctx(),
            &TermsView { version },
            Some("Accept the terms to continue."),
        );
        return Ok(page(&state, &visit, StatusCode::BAD_REQUEST, html));
    }
    store::accept_terms(&state.db, account, version, now()).await?;
    visit.session.data.login = None;
    let (visit, to) = complete_login(&state, visit, account).await?;
    Ok(redirect(&state, &visit, to))
}

/// An address with an active ban: the statement shows. When the account still exists the player
/// can appeal; when it is gone, only the statement shows.
async fn banned_by_address(
    state: &AppState,
    visit: Visit,
    email: &str,
    error: AppError,
) -> WebResult {
    if error.body.error != ErrorCode::Banned {
        return Err(error.into());
    }
    if let Some(account) = db::account_by_email(&state.db, email).await? {
        let (visit, to) = super::ban::send_to(state, visit, account.id()).await?;
        return Ok(redirect(state, &visit, to));
    }
    let mut visit = visit;
    let v = view(state, &mut visit.session).await?;
    let html = super::ban::without_account(&v.ctx(), &error);
    Ok(page(state, &visit, StatusCode::FORBIDDEN, html))
}

/// A player proved who they are (code or passkey). Checks what stands between them and the
/// account: a ban, and terms they have not accepted. Returns the session to continue with and
/// the page to go to.
pub async fn finish_login(
    state: &AppState,
    mut visit: Visit,
    account: &AccountRow,
) -> Result<(Visit, &'static str), AppError> {
    if state.ban_of(account.id()).is_some() {
        return super::ban::send_to(state, visit, account.id()).await;
    }
    if let Some(version) = state.config.terms()
        && store::terms_of(&state.db, account.id()).await?.as_deref() != Some(version)
    {
        visit.session.data.login = Some(Login::Terms {
            account: account.id(),
        });
        store::save_data(&state.db, &visit.session).await?;
        return Ok((visit, "/web/terms"));
    }
    complete_login(state, visit, account.id()).await
}

/// The last step of a sign-in. For the app on the player's PC it makes the one-time code and
/// keeps the address to send the browser to; for the account page it starts a browser session.
pub async fn complete_login(
    state: &AppState,
    visit: Visit,
    account: u64,
) -> Result<(Visit, &'static str), AppError> {
    let Visit { mut session, token } = visit;
    if let Some(authorize) = session.data.authorize.take() {
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
        session.data.login = None;
        session.data.handoff = Some(format!(
            "{}?code={code}&state={}",
            authorize.redirect_uri, authorize.state
        ));
        store::save_data(&state.db, &session).await?;
        return Ok((Visit { session, token }, "/web/next"));
    }
    store::end_session(&state.db, &session).await?;
    let visit = sign_in_session(state, account, SessionData::default()).await?;
    Ok((visit, "/web/account"))
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
