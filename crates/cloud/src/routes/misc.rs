//! Information, presence, relay tickets and reports.

use anyplay_api::{
    Info, MAX_REPORT_DETAILS, PresenceRequest, Relay, Relays, Report, ReportReason, TicketRequest,
};
use anyplay_ticket::Claims;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use crate::db;
use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth};
use crate::state::AppState;
use crate::util::{is_endpoint, is_label, now};

/// Reports one account may file per day.
const MAX_REPORTS_PER_DAY: i64 = 20;

pub async fn info(State(state): State<AppState>) -> Json<Info> {
    Json(Info {
        name: state.config.name.clone(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        signup: state.config.signup,
        privacy: state.config.privacy_url.clone(),
        terms: state.config.terms_url.clone(),
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
    {
        let mut hub = state.hub.lock().await;
        hub.accounts.entry(auth.account).or_default().activity = activity;
    }
    state.refresh_presence([auth.account]).await;
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
    let expires = now() + state.tuning.relay_ttl_secs;
    let token = state
        .signer
        .issue(&Claims::relay(auth.account, request.endpoint, expires));
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

fn reason_name(reason: ReportReason) -> &'static str {
    match reason {
        ReportReason::Cheating => "cheating",
        ReportReason::Harassment => "harassment",
        ReportReason::Spam => "spam",
        ReportReason::Name => "name",
        ReportReason::Other => "other",
    }
}

pub async fn report(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<Report>,
) -> Result<StatusCode> {
    if request.account.0 == auth.account {
        return Err(AppError::bad_request("You cannot report yourself."));
    }
    if request.details.chars().count() > MAX_REPORT_DETAILS {
        return Err(AppError::bad_request(
            "The details have at most 1,000 characters.",
        ));
    }
    if db::account(&state.db, request.account.0).await?.is_none() {
        return Err(AppError::not_found());
    }
    let day_ago = now().saturating_sub(24 * 60 * 60);
    if db::count_reports_since(&state.db, auth.account, day_ago).await? >= MAX_REPORTS_PER_DAY {
        return Err(AppError::limit(
            "You have filed the most reports allowed today.",
        ));
    }
    db::insert_report(
        &state.db,
        auth.account,
        request.account.0,
        reason_name(request.reason),
        &request.details,
        now(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
