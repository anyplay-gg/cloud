//! The session and the flow of signing in and up, as JSON.
//!
//! Every answer of the flow is the same object, the *session view*: whether the browser is
//! signed in, its CSRF token, and `flow.step`, the next thing the page shows. The steps are those
//! of [`crate::web::flow`], the same functions the HTML pages call.

use anyplay_api::Me;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{ApiFail, ApiResult, Visitor};
use crate::db;
use crate::error::AppError;
use crate::http::{ApiJson, ClientIp};
use crate::moderation;
use crate::routes::auth::me_of;
use crate::state::AppState;
use crate::util::now;
use crate::web::flow::{self as steps, DetailsError, Landing};
use crate::web::pages::{AppealPart, Field};
use crate::web::passkeys::{self, FinishSignIn};
use crate::web::store::{self, Login, Session, SessionData};
use crate::web::{Visit, ban, existing, fresh_visit, limit_ip_api, with_cookie, without_cookie};

#[derive(Serialize)]
pub struct TermsInfo {
    pub version: String,
    pub url: Option<String>,
}

/// The next step of the flow.
#[derive(Serialize)]
pub struct FlowView {
    /// `email`, `code`, `details`, `terms`, `banned`, `handoff` or `done`.
    pub step: &'static str,
    /// The address a code was sent to (`code`, `details`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Whether the player is creating an account rather than signing in (`email`, `code`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create: Option<bool>,
    /// With `handoff`: the address of the app on the player's PC, with its one-time code. The
    /// page goes there with `location.assign`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect: Option<String>,
    /// The terms the player has to accept (`details`, `terms`), when the cloud has any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terms: Option<TermsInfo>,
}

impl FlowView {
    fn at(step: &'static str) -> Self {
        Self {
            step,
            email: None,
            create: None,
            redirect: None,
            terms: None,
        }
    }
}

#[derive(Serialize)]
pub struct AuthorizeView {
    /// The name of the PC that waits for the sign-in.
    pub device: String,
    pub create: bool,
}

#[derive(Serialize)]
pub struct CloudView {
    pub name: String,
    pub signup: anyplay_api::Signup,
    pub privacy_url: Option<String>,
    pub terms_url: Option<String>,
}

#[derive(Serialize)]
pub struct SessionView {
    /// The token every write sends in `X-CSRF-Token`. Absent when the session is over.
    pub csrf: Option<String>,
    pub signed_in: bool,
    pub account: Option<Me>,
    /// Unix seconds until which sensitive actions need no step-up. Absent when signed out.
    pub fresh_until: Option<u64>,
    pub flow: FlowView,
    /// Set while the app on the player's PC waits for this sign-in.
    pub authorize: Option<AuthorizeView>,
    /// A line left by the last action, shown once.
    pub notice: Option<String>,
    pub cloud: CloudView,
}

fn terms_info(state: &AppState) -> Option<TermsInfo> {
    state.config.terms().map(|version| TermsInfo {
        version: version.to_owned(),
        url: state.config.terms_url.clone(),
    })
}

fn cloud_view(state: &AppState) -> CloudView {
    CloudView {
        name: state.config.name.clone(),
        signup: state.config.signup,
        privacy_url: state.config.privacy_url.clone(),
        terms_url: state.config.terms_url.clone(),
    }
}

fn flow_of(state: &AppState, session: &Session) -> FlowView {
    let data = &session.data;
    if let Some(url) = &data.handoff {
        return FlowView {
            redirect: Some(url.clone()),
            ..FlowView::at("handoff")
        };
    }
    if data.banned.is_some() {
        return FlowView::at("banned");
    }
    match &data.login {
        Some(Login::Code { email, create, .. }) => FlowView {
            email: Some(email.clone()),
            create: Some(*create),
            ..FlowView::at("code")
        },
        Some(Login::NewAccount { email, .. }) => FlowView {
            email: Some(email.clone()),
            terms: terms_info(state),
            ..FlowView::at("details")
        },
        Some(Login::Terms { .. }) => FlowView {
            terms: terms_info(state),
            ..FlowView::at("terms")
        },
        None if session.account.is_some() => FlowView::at("done"),
        None => FlowView {
            create: Some(data.authorize.as_ref().is_some_and(|a| a.create)),
            ..FlowView::at("email")
        },
    }
}

/// The view of a session. It takes the notice, and forgets a ban that has ended.
pub async fn session_view(
    state: &AppState,
    session: &mut Session,
) -> Result<SessionView, AppError> {
    let mut dirty = false;
    if let Some(account) = session.data.banned
        && db::ban_of(&state.db, account).await?.is_none()
    {
        session.data.banned = None;
        dirty = true;
    }
    let notice = session.data.notice.take();
    dirty |= notice.is_some();
    if dirty {
        store::save_data(&state.db, session).await?;
    }
    let account = match session.account {
        Some(id) => db::account(&state.db, id).await?,
        None => None,
    };
    let fresh_until = session
        .fresh_until(state.tuning.fresh_for)
        .map(|ms| (ms.max(0) / 1000) as u64);
    Ok(SessionView {
        csrf: Some(session.csrf.clone()),
        signed_in: account.is_some(),
        account: account.as_ref().map(me_of),
        fresh_until: account.as_ref().and(fresh_until),
        flow: flow_of(state, session),
        authorize: session.data.authorize.as_ref().map(|a| AuthorizeView {
            device: a.device.clone(),
            create: a.create,
        }),
        notice,
        cloud: cloud_view(state),
    })
}

/// The answer after a step. The hand-off ends the browser session: the page gets the address of
/// the app and nothing of this sign-in stays in the browser.
async fn answer(state: &AppState, mut visit: Visit, landing: Landing) -> ApiResult {
    if landing == Landing::Handoff
        && let Some(url) = visit.session.data.handoff.take()
    {
        store::end_session(&state.db, &visit.session).await?;
        let view = SessionView {
            csrf: None,
            signed_in: false,
            account: None,
            fresh_until: None,
            flow: FlowView {
                redirect: Some(url),
                ..FlowView::at("handoff")
            },
            authorize: None,
            notice: None,
            cloud: cloud_view(state),
        };
        return Ok(without_cookie(state, Json(view).into_response()));
    }
    let view = session_view(state, &mut visit.session).await?;
    Ok(with_cookie(state, &visit, Json(view).into_response()))
}

fn continue_visit(visitor: Visitor) -> Visit {
    Visit {
        session: visitor.session,
        token: None,
    }
}

/// `GET /session`: the browser's session, a new one when it has none.
pub async fn session(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> ApiResult {
    let mut visit = match existing(&state, &headers).await {
        Ok(Some(session)) => Visit {
            session,
            token: None,
        },
        Ok(None) => {
            limit_ip_api(&state, "session", ip, state.tuning.rates.code_per_ip)?;
            fresh_visit(&state, SessionData::default()).await?
        }
        Err(_) => return Err(AppError::internal().into()),
    };
    let view = session_view(&state, &mut visit.session).await?;
    Ok(with_cookie(&state, &visit, Json(view).into_response()))
}

#[derive(Deserialize)]
pub struct EmailBody {
    email: String,
    /// `sign_in` (the default) or `create`.
    #[serde(default)]
    intent: String,
}

/// `POST /sign-in/email`
pub async fn email(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    visitor: Visitor,
    ApiJson(body): ApiJson<EmailBody>,
) -> ApiResult {
    let mut visit = continue_visit(visitor);
    let landing =
        steps::begin_code(&state, ip, &mut visit, &body.email, body.intent == "create").await?;
    answer(&state, visit, landing).await
}

#[derive(Deserialize)]
pub struct CodeBody {
    code: String,
}

/// `POST /sign-in/code`
pub async fn code(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    visitor: Visitor,
    ApiJson(body): ApiJson<CodeBody>,
) -> ApiResult {
    let mut visit = continue_visit(visitor);
    let landing = steps::verify_code(&state, ip, &mut visit, &body.code).await?;
    answer(&state, visit, landing).await
}

/// `POST /sign-in/code/resend`
pub async fn resend(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    visitor: Visitor,
    ApiJson(_body): ApiJson<serde_json::Value>,
) -> ApiResult {
    let mut visit = continue_visit(visitor);
    let landing = steps::resend_code(&state, ip, &mut visit).await?;
    answer(&state, visit, landing).await
}

#[derive(Deserialize)]
pub struct DetailsBody {
    handle: String,
    #[serde(default)]
    age_confirmed: bool,
    #[serde(default)]
    terms_accepted: bool,
}

fn field_name(field: &Field) -> &'static str {
    match field {
        Field::Handle => "handle",
        Field::Age => "age_confirmed",
        Field::Terms => "terms_accepted",
    }
}

/// `POST /sign-up/details`. A problem with one input names it in `field`.
pub async fn details(
    State(state): State<AppState>,
    visitor: Visitor,
    ApiJson(body): ApiJson<DetailsBody>,
) -> ApiResult {
    let mut visit = continue_visit(visitor);
    match steps::create_from_details(
        &state,
        &mut visit,
        &body.handle,
        body.age_confirmed,
        body.terms_accepted,
    )
    .await
    {
        Ok(landing) => answer(&state, visit, landing).await,
        Err(DetailsError { field, error }) => {
            let mut fail = ApiFail::from(*error);
            if let Some(field) = field {
                fail.body["field"] = json!(field_name(&field));
            }
            Err(fail)
        }
    }
}

#[derive(Deserialize)]
pub struct TermsBody {
    accept: bool,
}

/// `POST /terms`
pub async fn terms(
    State(state): State<AppState>,
    visitor: Visitor,
    ApiJson(body): ApiJson<TermsBody>,
) -> ApiResult {
    let mut visit = continue_visit(visitor);
    let landing = steps::accept_terms(&state, &mut visit, body.accept).await?;
    answer(&state, visit, landing).await
}

/// `POST /sign-out`: ends the browser session.
pub async fn sign_out(
    State(state): State<AppState>,
    visitor: Visitor,
    ApiJson(_body): ApiJson<serde_json::Value>,
) -> ApiResult {
    store::end_session(&state.db, &visitor.session).await?;
    Ok(without_cookie(
        &state,
        Json(json!({"signed_out": true})).into_response(),
    ))
}

/// `POST /passkey/sign-in/start`: the options for `navigator.credentials.get`.
pub async fn passkey_start(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    _visitor: Visitor,
    ApiJson(_body): ApiJson<serde_json::Value>,
) -> ApiResult {
    limit_ip_api(&state, "passkey", ip, state.tuning.rates.code_per_ip)?;
    Ok(Json(passkeys::begin_assertion(&state).await?).into_response())
}

/// `POST /passkey/sign-in/finish`
pub async fn passkey_finish(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    visitor: Visitor,
    ApiJson(body): ApiJson<FinishSignIn>,
) -> ApiResult {
    limit_ip_api(&state, "passkey", ip, state.tuning.rates.code_per_ip)?;
    let account = passkeys::verify_assertion(&state, &body).await?;
    let mut visit = continue_visit(visitor);
    let landing = steps::finish_login(&state, &mut visit, &account).await?;
    answer(&state, visit, landing).await
}

// The ban

/// The account the browser proved to be its own and whose ban shows.
fn banned_account(session: &Session) -> Result<u64, AppError> {
    session.data.banned.ok_or_else(AppError::not_found)
}

/// `GET /ban`: the statement of reasons of the ban the session proved to be its own.
pub async fn show_ban(State(state): State<AppState>, visitor: Visitor) -> ApiResult {
    let account = banned_account(&visitor.session)?;
    let Some(row) = db::ban_of(&state.db, account).await? else {
        return Err(AppError::not_found().into());
    };
    let statement = moderation::ban_statement(&state, account).await?;
    let shown = ban::view_of(statement.as_ref(), row.until());
    let appeal = match shown.appeal {
        AppealPart::None => json!({"state": "none"}),
        AppealPart::Form { deadline } => json!({"state": "open", "deadline": deadline}),
        AppealPart::Filed { status, answer } => {
            json!({"state": "filed", "status": status, "answer": answer})
        }
        AppealPart::Over => json!({"state": "over"}),
    };
    Ok(Json(json!({
        "until": shown.until,
        "rule": shown.rule,
        "reason": shown.reason,
        "decided": shown.decided,
        "appeal": appeal,
    }))
    .into_response())
}

#[derive(Deserialize)]
pub struct AppealBody {
    text: String,
}

/// `POST /ban/appeal`
pub async fn appeal(
    State(state): State<AppState>,
    visitor: Visitor,
    ApiJson(body): ApiJson<AppealBody>,
) -> ApiResult {
    let account = banned_account(&visitor.session)?;
    let Some(statement) = moderation::ban_statement(&state, account).await? else {
        return Err(AppError::not_found().into());
    };
    moderation::appeal(&state, account, statement.id, &body.text).await?;
    Ok(Json(json!({"filed": now()})).into_response())
}
