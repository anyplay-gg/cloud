//! The signed-in account and lookups of other accounts.

use anyplay_api::{Account, AccountId, Me};
use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{AppError, Result};
use crate::http::{Auth, parse_id};
use crate::routes::auth::me_of;
use crate::state::AppState;
use crate::util::now;
use crate::{db, live};

pub async fn me(State(state): State<AppState>, auth: Auth) -> Result<Json<Me>> {
    let account = db::account(&state.db, auth.account)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(Json(me_of(&account)))
}

pub async fn delete_me(State(state): State<AppState>, auth: Auth) -> Result<StatusCode> {
    state.delete_account(auth.account).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Everything the cloud stores about the account.
pub async fn export(State(state): State<AppState>, auth: Auth) -> Result<Json<Value>> {
    let account = db::account(&state.db, auth.account)
        .await?
        .ok_or_else(AppError::not_found)?;
    let devices = db::devices_of(&state.db, auth.account).await?;
    let related = |rows: Vec<db::Related>| -> Vec<Value> {
        rows.into_iter()
            .map(|r| json!({"account": {"id": r.id(), "handle": r.handle}, "since": r.since()}))
            .collect()
    };
    let friends = db::friends_of(&state.db, auth.account).await?;
    let incoming = db::incoming_requests(&state.db, auth.account).await?;
    let outgoing = db::outgoing_requests(&state.db, auth.account).await?;
    let blocked = db::blocked_by(&state.db, auth.account).await?;
    let reports = db::reports_by(&state.db, auth.account).await?;
    let ban = db::ban_of(&state.db, auth.account).await?;
    let trophies = db::trophies_of(&state.db, auth.account).await?;
    let privacy = db::privacy_of(&state.db, auth.account).await?;
    let sessions = {
        let mut conn = state.db.acquire().await?;
        let mut views = Vec::new();
        for id in live::sessions_of(&mut conn, auth.account).await? {
            if let Some(session) = live::session(&mut conn, &id).await? {
                views.push(session.view());
            }
        }
        views
    };
    Ok(Json(json!({
        "exported": now(),
        "account": me_of(&account),
        "devices": devices.iter().map(|d| json!({
            "id": d.id(), "name": d.name, "platform": d.platform,
            "created": d.created, "last_used": d.last_used,
        })).collect::<Vec<_>>(),
        "friends": related(friends),
        "requests_in": related(incoming),
        "requests_out": related(outgoing),
        "blocked": related(blocked),
        "reports_filed": reports.iter().map(|r| json!({
            "account": {"id": r.reported, "handle": r.reported_handle},
            "reason": r.reason, "details": r.details, "created": r.created,
        })).collect::<Vec<_>>(),
        "ban": ban.map(|b| json!({"until": b.until(), "reason": b.reason, "created": b.created})),
        "sessions": sessions,
        "trophies": trophies.iter().map(|t| json!({
            "title_id": t.title_id, "number": t.number, "grade": t.grade, "earned": t.earned,
        })).collect::<Vec<_>>(),
        "privacy": privacy,
    })))
}

pub async fn by_id(
    State(state): State<AppState>,
    _auth: Auth,
    Path(id): Path<String>,
) -> Result<Json<Account>> {
    let id = parse_id(&id)?;
    let account = db::account(&state.db, id)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(Json(account_of(&account)))
}

#[derive(Deserialize)]
pub struct HandleQuery {
    handle: String,
}

pub async fn by_handle(
    State(state): State<AppState>,
    _auth: Auth,
    query: Result<Query<HandleQuery>, QueryRejection>,
) -> Result<Json<Account>> {
    let Query(query) = query.map_err(|_| AppError::bad_request("Give the handle to look up."))?;
    let account = db::account_by_handle(&state.db, &query.handle.to_ascii_lowercase())
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(Json(account_of(&account)))
}

pub fn account_of(row: &db::AccountRow) -> Account {
    Account {
        id: AccountId(row.id()),
        handle: row.handle.clone(),
    }
}
