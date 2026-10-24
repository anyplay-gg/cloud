//! Moderation routes for the operator. They exist only when `admin_token` is set, and they are
//! the same routes the `anyplay-cloud admin` commands call.

use anyplay_api::{Account, AccountId, ErrorCode};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};

use crate::db;
use crate::error::{AppError, Result};
use crate::http::{ApiJson, ClientIp, bearer};
use crate::state::AppState;
use crate::util::{ct_eq, now, sha256};

pub const BANS: &str = "/admin/v1/bans";
pub const BAN: &str = "/admin/v1/bans/{handle}";
pub const REPORTS: &str = "/admin/v1/reports";

#[derive(Debug, Serialize, Deserialize)]
pub struct BanRequest {
    pub handle: String,
    /// For how many days; absent means for good.
    #[serde(default)]
    pub days: Option<u64>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BanAnswer {
    pub account: Account,
    pub until: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReportView {
    pub id: u64,
    pub reporter: Account,
    pub reported: Account,
    pub reason: String,
    pub details: String,
    pub created: u64,
}

#[derive(Deserialize)]
pub struct ReportQuery {
    limit: Option<i64>,
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
    let until = match request.days {
        Some(0) => return Err(AppError::bad_request("days must be at least 1.")),
        Some(days) => Some(now().saturating_add(days.saturating_mul(24 * 60 * 60))),
        None => None,
    };
    let reason: String = request
        .reason
        .unwrap_or_default()
        .chars()
        .take(500)
        .collect();
    db::upsert_ban(
        &state.db,
        account.id(),
        &sha256(account.email.as_bytes()),
        until,
        &reason,
        now(),
    )
    .await?;
    state.apply_ban(account.id(), until).await;
    tracing::warn!(account = account.id(), ?until, "account banned");
    Ok(Json(BanAnswer {
        account: Account {
            id: AccountId(account.id()),
            handle: account.handle,
        },
        until,
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
    if !db::delete_ban(&state.db, account.id()).await? {
        return Err(AppError::new(
            ErrorCode::NotFound,
            "This account is not banned.",
        ));
    }
    state.lift_ban(account.id());
    tracing::warn!(account = account.id(), "account unbanned");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn reports(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Query(query): Query<ReportQuery>,
) -> Result<Json<Vec<ReportView>>> {
    authorize(&state, ip, &headers)?;
    let limit = query.limit.unwrap_or(100).clamp(1, 1000);
    let rows = db::latest_reports(&state.db, limit).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| ReportView {
                id: r.id as u64,
                reporter: Account {
                    id: AccountId(r.reporter as u64),
                    handle: r.reporter_handle,
                },
                reported: Account {
                    id: AccountId(r.reported as u64),
                    handle: r.reported_handle,
                },
                reason: r.reason,
                details: r.details,
                created: r.created as u64,
            })
            .collect(),
    ))
}
