//! The routes of Discord's Social SDK tokens (see [`crate::discord_sdk`]).

use anyplay_api::{DiscordCodeExchange, DiscordRefresh, DiscordTokens};
use axum::Json;
use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;

use crate::discord_sdk;
use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth};
use crate::state::AppState;

/// Counts the request against the account's hourly allowance, whatever its outcome.
fn limit(state: &AppState, auth: &Auth) -> Result<()> {
    if state.config.discord().is_none() {
        return Err(AppError::not_found());
    }
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("discord-grant:{}", auth.account),
            rates.discord_grants_per_account,
            rates.discord_grant_window,
        )
        .map_err(AppError::rate_limited)
}

/// A token is never cached by anything between the cloud and the net service.
fn no_store(tokens: DiscordTokens) -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")], Json(tokens))
}

pub async fn token(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<DiscordCodeExchange>,
) -> Result<impl IntoResponse> {
    limit(&state, &auth)?;
    Ok(no_store(
        discord_sdk::exchange(&state, auth.account, &request).await?,
    ))
}

pub async fn refresh(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<DiscordRefresh>,
) -> Result<impl IntoResponse> {
    limit(&state, &auth)?;
    Ok(no_store(discord_sdk::refresh(&state, &request).await?))
}
