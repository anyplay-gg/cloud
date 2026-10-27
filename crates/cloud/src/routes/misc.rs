//! Information, presence, relay tickets and reports.

use anyplay_api::{Info, PresenceRequest, Relay, Relays, Report, TicketRequest};
use anyplay_ticket::Claims;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth};
use crate::live;
use crate::state::AppState;
use crate::util::{is_endpoint, is_label, now};

pub async fn info(State(state): State<AppState>) -> Json<Info> {
    Json(Info {
        name: state.config.name.clone(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        signup: state.config.signup,
        privacy: state.config.privacy_url.clone(),
        terms: state.config.terms_url.clone(),
        web: state.config.web_origin(),
        share: Some(state.share_info()),
    })
}

pub async fn set_presence(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<PresenceRequest>,
) -> Result<StatusCode> {
    let activity = match request.activity {
        Some(mut activity) => {
            if !is_label(&activity.title_id, 64)
                || !is_label(&activity.title, 128)
                || !is_label(&activity.version, 32)
            {
                return Err(AppError::bad_request(
                    "title_id, title and version must be short printable texts.",
                ));
            }
            // The cloud decides what the session part says.
            activity.session = None;
            Some(activity)
        }
        None => None,
    };
    live::set_activity(
        &mut *state.db.acquire().await?,
        auth.account,
        activity.as_ref(),
    )
    .await?;
    state.refresh_presence([auth.account]).await;
    if activity.is_none() {
        // The game ended: a share of it ends too.
        state.check_shares(&[auth.account]).await;
    }
    if let Some(activity) = &activity {
        state
            .record_first_played(auth.account, &activity.title_id)
            .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn relay_tickets(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<TicketRequest>,
) -> Result<Json<Relays>> {
    if !is_endpoint(&request.endpoint) {
        return Err(AppError::bad_request(
            "The endpoint is 64 lowercase hex digits.",
        ));
    }
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("relay-tickets:{}", auth.account),
            rates.relay_tickets_per_account,
            rates.relay_ticket_window,
        )
        .map_err(AppError::rate_limited)?;
    let issued = now();
    let expires = issued + state.tuning.relay_ttl_secs;
    let token = state.signer.issue(&Claims::relay(
        auth.account,
        request.endpoint,
        issued,
        expires,
    ));
    Ok(Json(Relays {
        relays: state
            .config
            .relays
            .iter()
            .map(|url| Relay {
                url: url.clone(),
                token: token.clone(),
                expires,
            })
            .collect(),
    }))
}

pub async fn report(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<Report>,
) -> Result<StatusCode> {
    crate::moderation::file_report(&state, auth.account, request).await?;
    Ok(StatusCode::NO_CONTENT)
}
