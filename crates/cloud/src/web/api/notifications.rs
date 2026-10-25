//! Notifications: the browsers that get Web Push, and which kinds each wants (network
//! `spec/portal-api.md`, "Notifications (Web Push)"). The sending is [`crate::push`].

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;

use super::{ApiFail, ApiResult, Member, done};
use crate::error::AppError;
use crate::http::ApiJson;
use crate::push;
use crate::state::AppState;

/// `GET /push`: whether the cloud sends notifications, the key the browser subscribes with, and
/// the browsers of this account.
pub async fn get(State(state): State<AppState>, member: Member) -> ApiResult {
    let subscriptions = push::subscriptions_of(&state.db, member.id()).await?;
    Ok(Json(json!({
        "enabled": state.push.is_some(),
        "public_key": state.push.as_ref().map(push::Pusher::public_key),
        "subscriptions": subscriptions,
    }))
    .into_response())
}

/// `PUT /push/subscription`: subscribes a browser, or changes the kinds and name of one that is
/// subscribed.
pub async fn subscribe(
    State(state): State<AppState>,
    member: Member,
    ApiJson(body): ApiJson<push::SubscribeBody>,
) -> ApiResult {
    if state.push.is_none() {
        return Err(AppError::forbidden("This cloud does not send notifications.").into());
    }
    let view = push::subscribe(&state.db, member.id(), body, state.tuning.push_any_host)
        .await
        .map_err(ApiFail::from)?;
    Ok(Json(view).into_response())
}

#[derive(Deserialize)]
pub struct Unsubscribe {
    endpoint: String,
}

/// `DELETE /push/subscription`: the browser stops getting notifications. Nothing to remove is
/// not an error.
pub async fn unsubscribe(
    State(state): State<AppState>,
    member: Member,
    ApiJson(body): ApiJson<Unsubscribe>,
) -> ApiResult {
    push::unsubscribe(&state.db, member.id(), &body.endpoint).await?;
    done()
}
