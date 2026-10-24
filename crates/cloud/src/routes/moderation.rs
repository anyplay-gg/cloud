//! The moderation routes (`/v1/moderation/...`) and a player's routes about the actions against
//! them (`/v1/me/actions`). All logic is in [`crate::moderation`].

use anyplay_api::{
    ActionRequest, ActionView, AppealDecision, AppealRequest, AppealView, AuditEntry, Dismissal,
    GrantModerator, Lifting, Moderator, ReportView, Statement, Statements,
};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;

use crate::error::Result;
use crate::http::{ApiJson, parse_id};
use crate::moderation::access::{Concerned, Staff};
use crate::moderation::{self};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct ListQuery {
    status: Option<String>,
    limit: Option<i64>,
    account: Option<u64>,
    before: Option<u64>,
}

pub async fn reports(
    State(state): State<AppState>,
    Staff(_): Staff,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<ReportView>>> {
    let status = query.status.as_deref().unwrap_or("open");
    Ok(Json(
        moderation::queue(&state, status, query.limit.unwrap_or(100)).await?,
    ))
}

pub async fn report(
    State(state): State<AppState>,
    Staff(_): Staff,
    Path(id): Path<String>,
) -> Result<Json<ReportView>> {
    Ok(Json(moderation::report(&state, parse_id(&id)?).await?))
}

pub async fn dismiss(
    State(state): State<AppState>,
    Staff(actor): Staff,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<Dismissal>,
) -> Result<Json<ReportView>> {
    Ok(Json(
        moderation::dismiss(&state, &actor, parse_id(&id)?, &request.reason).await?,
    ))
}

pub async fn act(
    State(state): State<AppState>,
    Staff(actor): Staff,
    ApiJson(request): ApiJson<ActionRequest>,
) -> Result<Json<ActionView>> {
    Ok(Json(moderation::act(&state, &actor, request).await?))
}

pub async fn actions(
    State(state): State<AppState>,
    Staff(_): Staff,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<ActionView>>> {
    let account = parse_id(&query.account.unwrap_or(0).to_string())?;
    Ok(Json(
        moderation::actions_about(&state, account, query.limit.unwrap_or(100)).await?,
    ))
}

pub async fn lift(
    State(state): State<AppState>,
    Staff(actor): Staff,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<Lifting>,
) -> Result<Json<ActionView>> {
    Ok(Json(
        moderation::lift(&state, &actor, parse_id(&id)?, &request.reason).await?,
    ))
}

pub async fn appeals(
    State(state): State<AppState>,
    Staff(_): Staff,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<AppealView>>> {
    let status = query.status.as_deref().unwrap_or("open");
    Ok(Json(
        moderation::appeals(&state, status, query.limit.unwrap_or(100)).await?,
    ))
}

pub async fn decide_appeal(
    State(state): State<AppState>,
    Staff(actor): Staff,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<AppealDecision>,
) -> Result<Json<AppealView>> {
    Ok(Json(
        moderation::decide_appeal(&state, &actor, parse_id(&id)?, request).await?,
    ))
}

pub async fn log(
    State(state): State<AppState>,
    Staff(_): Staff,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<AuditEntry>>> {
    Ok(Json(
        moderation::log(
            &state,
            query.before.unwrap_or(0),
            query.limit.unwrap_or(100),
        )
        .await?,
    ))
}

pub async fn moderators(
    State(state): State<AppState>,
    Staff(_): Staff,
) -> Result<Json<Vec<Moderator>>> {
    Ok(Json(moderation::moderators(&state).await?))
}

pub async fn grant(
    State(state): State<AppState>,
    Staff(actor): Staff,
    ApiJson(request): ApiJson<GrantModerator>,
) -> Result<Json<Moderator>> {
    Ok(Json(
        moderation::grant(&state, &actor, &request.handle).await?,
    ))
}

pub async fn revoke(
    State(state): State<AppState>,
    Staff(actor): Staff,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    moderation::revoke(&state, &actor, parse_id(&id)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

// The player's side

pub async fn my_actions(
    State(state): State<AppState>,
    Concerned(account): Concerned,
) -> Result<Json<Statements>> {
    Ok(Json(Statements {
        actions: moderation::statements(&state, account).await?,
    }))
}

pub async fn my_appeal(
    State(state): State<AppState>,
    Concerned(account): Concerned,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<AppealRequest>,
) -> Result<Json<Statement>> {
    Ok(Json(
        moderation::appeal(&state, account, parse_id(&id)?, &request.text).await?,
    ))
}
