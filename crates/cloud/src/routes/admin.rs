//! The operator's routes, behind the admin token. They exist only when `admin_token` is set, and
//! they are the same routes the `anyplay-cloud admin` commands call. Everything they do goes
//! through [`crate::moderation`] under the name `operator`, so it is audited and the player gets
//! a statement of reasons. Moderators use the `/v1/moderation/...` routes instead; the operator
//! can call those with the admin token too.

use anyplay_api::{Account, AccountId, ActionKind, ActionRequest, ActionView, ReportView, Rule};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};

use crate::db;
use crate::error::{AppError, Result};
use crate::http::{ApiJson, ClientIp, bearer};
use crate::mail::{self, MailClass};
use crate::moderation::{self, Actor};
use crate::state::AppState;
use crate::util::ct_eq;

pub const BANS: &str = "/admin/v1/bans";
pub const BAN: &str = "/admin/v1/bans/{handle}";
pub const REPORTS: &str = "/admin/v1/reports";
pub const ACTIONS: &str = "/admin/v1/actions";
pub const MAIL_TEST: &str = "/admin/v1/mail-test";

#[derive(Debug, Serialize, Deserialize)]
pub struct BanRequest {
    pub handle: String,
    /// For how many days; absent means for good.
    #[serde(default)]
    pub days: Option<u64>,
    /// What the banned player reads. Required.
    #[serde(default)]
    pub reason: Option<String>,
    /// The rule of the terms; `other` when absent.
    #[serde(default)]
    pub rule: Option<Rule>,
}

/// A warning, mute or ban of an account named by its handle.
#[derive(Debug, Serialize, Deserialize)]
pub struct ActionByHandle {
    pub handle: String,
    pub kind: ActionKind,
    #[serde(default)]
    pub rule: Option<Rule>,
    pub reason: String,
    #[serde(default)]
    pub days: Option<u64>,
    #[serde(default)]
    pub report: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BanAnswer {
    pub account: Account,
    pub until: Option<u64>,
}

/// Send one test mail through the configured transport.
#[derive(Debug, Serialize, Deserialize)]
pub struct MailTestRequest {
    pub to: String,
}

/// How the test mail went. A failure is an answer, not an HTTP error: it is what the operator
/// asked for.
#[derive(Debug, Serialize, Deserialize)]
pub struct MailTestAnswer {
    /// `smtp`, or `log` when the cloud only writes mail to its log.
    pub mode: String,
    pub sent: bool,
    #[serde(default)]
    pub class: Option<MailClass>,
    #[serde(default)]
    pub status: Option<u16>,
    /// The server's reply, with addresses taken out.
    #[serde(default)]
    pub reply: Option<String>,
}

#[derive(Deserialize)]
pub struct ReportQuery {
    limit: Option<i64>,
    status: Option<String>,
}

/// Fails unless the request carries the admin token.
fn authorize(state: &AppState, ip: std::net::IpAddr, headers: &HeaderMap) -> Result<()> {
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("admin:{ip}"),
            rates.admin_per_ip,
            rates.account_window,
        )
        .map_err(AppError::rate_limited)?;
    let expected = state
        .config
        .admin_token
        .as_deref()
        .ok_or_else(AppError::not_found)?;
    match bearer(headers) {
        Some(token) if ct_eq(token.as_bytes(), expected.as_bytes()) => Ok(()),
        _ => Err(AppError::unauthorized(
            "The admin token is missing or wrong.",
        )),
    }
}

pub async fn ban(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    ApiJson(request): ApiJson<BanRequest>,
) -> Result<Json<BanAnswer>> {
    authorize(&state, ip, &headers)?;
    let account = db::account_by_handle(&state.db, &request.handle.to_ascii_lowercase())
        .await?
        .ok_or_else(AppError::not_found)?;
    let view = moderation::act(
        &state,
        &Actor::operator(),
        ActionRequest {
            account: AccountId(account.id()),
            kind: ActionKind::Ban,
            rule: request.rule.unwrap_or(Rule::Other),
            reason: request.reason.unwrap_or_default(),
            days: request.days,
            report: None,
        },
    )
    .await?;
    Ok(Json(BanAnswer {
        account: view.account,
        until: view.statement.until,
    }))
}

pub async fn act(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    ApiJson(request): ApiJson<ActionByHandle>,
) -> Result<Json<ActionView>> {
    authorize(&state, ip, &headers)?;
    let account = db::account_by_handle(&state.db, &request.handle.to_ascii_lowercase())
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(Json(
        moderation::act(
            &state,
            &Actor::operator(),
            ActionRequest {
                account: AccountId(account.id()),
                kind: request.kind,
                rule: request.rule.unwrap_or(Rule::Other),
                reason: request.reason,
                days: request.days,
                report: request.report,
            },
        )
        .await?,
    ))
}

pub async fn mail_test(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    ApiJson(request): ApiJson<MailTestRequest>,
) -> Result<Json<MailTestAnswer>> {
    authorize(&state, ip, &headers)?;
    let name = &state.config.name;
    let result = mail::within(
        std::time::Duration::from_secs(20),
        state.mailer.send_notice(
            request.to.trim(),
            &format!("{name} mail test"),
            &format!(
                "This is a test mail from {name}. If you can read it, sign-in codes can reach you.\n"
            ),
        ),
    )
    .await;
    let mode = match state.config.mail {
        crate::config::MailMode::Smtp => "smtp",
        crate::config::MailMode::Log => "log",
    };
    match &result {
        Ok(()) => tracing::info!(mode, "mail test sent"),
        Err(e) => tracing::warn!(
            mode,
            class = %e.class,
            status = e.status,
            reply = e.detail.as_deref(),
            "mail test failed"
        ),
    }
    Ok(Json(MailTestAnswer {
        mode: mode.into(),
        sent: result.is_ok(),
        class: result.as_ref().err().map(|e| e.class),
        status: result.as_ref().err().and_then(|e| e.status),
        reply: result.err().and_then(|e| e.detail),
    }))
}

pub async fn unban(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Path(handle): Path<String>,
) -> Result<StatusCode> {
    authorize(&state, ip, &headers)?;
    let account = db::account_by_handle(&state.db, &handle.to_ascii_lowercase())
        .await?
        .ok_or_else(AppError::not_found)?;
    if state.ban_of(account.id()).is_none() && db::ban_of(&state.db, account.id()).await?.is_none()
    {
        return Err(AppError::new(
            anyplay_api::ErrorCode::NotFound,
            "This account is not banned.",
        ));
    }
    moderation::unban(&state, &Actor::operator(), account.id()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The newest reports of every status.
pub async fn reports(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Query(query): Query<ReportQuery>,
) -> Result<Json<Vec<ReportView>>> {
    authorize(&state, ip, &headers)?;
    let status = query.status.as_deref().unwrap_or("all");
    Ok(Json(
        moderation::queue(&state, status, query.limit.unwrap_or(100)).await?,
    ))
}
