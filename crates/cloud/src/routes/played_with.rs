//! `/v1/me/played-with`: the account's own list of the people it played with recently, and the
//! switch that turns the remembering on and off. The rules and the store are in
//! [`crate::played_with`].

use anyplay_api::{PlayedWithList, RememberPlayedWith};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth};
use crate::played_with;
use crate::state::AppState;
use crate::util::now_ms;

/// `GET /v1/me/played-with`
pub async fn list(State(state): State<AppState>, auth: Auth) -> Result<Json<PlayedWithList>> {
    let mut conn = state.db.acquire().await?;
    Ok(Json(
        played_with::list(&mut conn, auth.account, now_ms()).await?,
    ))
}

/// `DELETE /v1/me/played-with`: empties the list.
pub async fn clear(State(state): State<AppState>, auth: Auth) -> Result<StatusCode> {
    let mut conn = state.db.acquire().await?;
    played_with::clear(&mut conn, auth.account, now_ms()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/me/played-with/remember`
pub async fn get_remember(
    State(state): State<AppState>,
    auth: Auth,
) -> Result<Json<RememberPlayedWith>> {
    let mut conn = state.db.acquire().await?;
    let on = played_with::remembers(&mut conn, auth.account)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(Json(RememberPlayedWith {
        remember_played_with: on,
    }))
}

/// `PUT /v1/me/played-with/remember`: the cloud records new entries for the account, or stops.
/// What is on the list stays until the player clears it.
pub async fn set_remember(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(body): ApiJson<RememberPlayedWith>,
) -> Result<Json<RememberPlayedWith>> {
    let mut conn = state.db.acquire().await?;
    if !played_with::set_remember(&mut conn, auth.account, body.remember_played_with).await? {
        return Err(AppError::not_found());
    }
    Ok(Json(body))
}
