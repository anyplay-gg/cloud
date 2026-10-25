//! The HTTP routes. Paths and their `{id}` placeholders come from `anyplay_api::path`.

use std::sync::atomic::Ordering;
use std::time::Duration;

use anyplay_api::path;
use axum::Router;
use axum::extract::{MatchedPath, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};

use crate::error::AppError;
use crate::metrics::Snapshot;
use crate::state::AppState;

pub mod accounts;
pub mod admin;
pub mod auth;
pub mod discord;
pub mod events;
pub mod feed;
pub mod friends;
pub mod join_codes;
pub mod messages;
pub mod misc;
pub mod moderation;
pub mod parties;
pub mod profiles;
pub mod runs;
pub mod sessions;
pub mod trophies;

/// Answers while the process runs.
pub const HEALTH: &str = "/healthz";
/// Answers while the node can take players: its store answers, NATS is connected when it has
/// one, and it is not draining.
pub const READY: &str = "/readyz";

pub fn router(state: AppState) -> Router {
    let mut router = Router::new()
        .route(HEALTH, get(|| async { "ok" }))
        .route(READY, get(ready))
        .route(path::INFO, get(misc::info))
        .route(path::AUTH_EMAIL, post(auth::email))
        .route(path::AUTH_CODE, post(auth::code))
        .route(path::AUTH_SIGNUP, post(auth::signup))
        .route(path::AUTH_REFRESH, post(auth::refresh))
        .route(path::AUTH_SIGNOUT, post(auth::signout))
        .route(path::ME, get(accounts::me).delete(accounts::delete_me))
        .route(path::ME_EXPORT, get(accounts::export))
        .route(path::ACCOUNTS, get(accounts::by_handle))
        .route(path::ACCOUNT, get(accounts::by_id))
        .route(path::FRIENDS, get(friends::list))
        .route(path::FRIEND, delete(friends::unfriend))
        .route(path::FRIEND_REQUESTS, post(friends::request))
        .route(path::FRIEND_REQUEST, delete(friends::cancel))
        .route(path::FRIEND_REQUEST_ACCEPT, post(friends::accept))
        .route(path::BLOCK, put(friends::block).delete(friends::unblock))
        .route(path::PRESENCE, put(misc::set_presence))
        .route(
            path::APPEAR_OFFLINE,
            get(accounts::get_appear_offline).put(accounts::set_appear_offline),
        )
        .route(path::SESSIONS, post(sessions::create))
        .route(path::SESSION, get(sessions::get).patch(sessions::update))
        .route(path::SESSION_JOIN, post(sessions::join))
        .route(path::SESSION_LEAVE, post(sessions::leave))
        .route(path::SESSION_MEMBER, delete(sessions::remove_member))
        .route(path::SESSION_INVITATIONS, post(sessions::invite))
        .route(path::INVITATION, delete(sessions::decline))
        .route(path::MY_TROPHIES, put(trophies::upload))
        .route(path::MY_TROPHIES_SECRET, put(trophies::mark_secret))
        .route(path::MY_TROPHIES_REVEAL, put(trophies::reveal_secret))
        .route(path::COMPARE, get(trophies::compare))
        .route(path::ACCOUNT_TROPHIES, get(trophies::of_account))
        .route(path::RARITY, get(trophies::rarity))
        .route(path::MY_RUN_REPORTS, put(runs::put))
        .route(path::RUNS, get(runs::summary))
        .route(path::CARDS, get(trophies::cards))
        .route(
            path::PRIVACY,
            get(trophies::get_privacy).put(trophies::set_privacy),
        )
        .route(path::ACCOUNT_PROFILE, get(profiles::of_account))
        .route(path::MY_PROFILE, patch(profiles::update))
        .route(
            path::MY_AVATAR,
            put(profiles::set_avatar).delete(profiles::remove_avatar),
        )
        .route(path::AVATAR, get(profiles::avatar))
        .route(path::FEED, get(feed::page))
        .route(path::FEED_REACTION, put(feed::react).delete(feed::unreact))
        .route(path::MY_PLAYTIME, put(profiles::upload_playtime))
        .route(path::RELAY_TICKETS, post(misc::relay_tickets))
        .route(path::ME_DISCORD_TOKEN, post(discord::token))
        .route(path::ME_DISCORD_REFRESH, post(discord::refresh))
        .route(path::REPORTS, post(misc::report))
        .route(path::EVENTS, get(events::events))
        .route(
            path::PARTY,
            get(parties::get)
                .post(parties::create)
                .patch(parties::update),
        )
        .route(path::PARTY_LEAVE, post(parties::leave))
        .route(path::PARTY_ENDPOINT, put(parties::set_endpoint))
        .route(path::PARTY_SESSION, post(parties::move_to_session))
        .route(path::PARTY_MEMBER, delete(parties::kick))
        .route(
            path::PARTY_INVITATIONS,
            get(parties::invitations).post(parties::invite),
        )
        .route(path::PARTY_INVITATION, delete(parties::decline))
        .route(
            path::CALLS_MISSED,
            get(parties::missed_calls).delete(parties::calls_seen),
        )
        .route(path::CALLS_MISSED_FROM, delete(parties::calls_seen_from))
        .route(path::PARTY_INVITATION_ACCEPT, post(parties::accept))
        .route(path::JOIN_CODES, post(join_codes::issue))
        .route(path::JOIN_CODE_CURRENT, delete(join_codes::revoke))
        .route(path::JOIN_CODE_REDEEM, post(join_codes::redeem))
        .route(
            path::CONVERSATIONS,
            get(messages::list).post(messages::create),
        )
        .route(
            path::CONVERSATION,
            get(messages::get).delete(messages::leave),
        )
        .route(path::CONVERSATION_ACCEPT, post(messages::accept))
        .route(path::CONVERSATION_MEMBERS, post(messages::add_members))
        .route(
            path::CONVERSATION_MESSAGES,
            get(messages::messages).post(messages::send),
        )
        .route(path::CONVERSATION_READ, put(messages::mark_read));
    // Moderation (docs/moderation.md)
    router = router
        .route(path::ME_ACTIONS, get(moderation::my_actions))
        .route(path::ME_ACTION_APPEAL, post(moderation::my_appeal))
        .route(path::MOD_REPORTS, get(moderation::reports))
        .route(path::MOD_REPORT, get(moderation::report))
        .route(path::MOD_REPORT_DISMISS, post(moderation::dismiss))
        .route(
            path::MOD_ACTIONS,
            get(moderation::actions).post(moderation::act),
        )
        .route(path::MOD_ACTION_LIFT, post(moderation::lift))
        .route(path::MOD_APPEALS, get(moderation::appeals))
        .route(path::MOD_APPEAL_DECISION, post(moderation::decide_appeal))
        .route(path::MOD_LOG, get(moderation::log))
        .route(
            path::MOD_MODERATORS,
            get(moderation::moderators).post(moderation::grant),
        )
        .route(path::MOD_MODERATOR, delete(moderation::revoke));
    if state.web.is_some() {
        router = router
            .route(path::AUTH_BROWSER_TOKEN, post(crate::web::native::token))
            .route(path::AUTH_BROWSER_LINK, post(crate::web::native::link))
            .merge(crate::web::router(state.clone()));
    }
    if state.config.admin_token.is_some() {
        router = router
            .route(admin::BANS, post(admin::ban))
            .route(admin::BAN, delete(admin::unban))
            .route(admin::ACTIONS, post(admin::act))
            .route(admin::MAIL_TEST, post(admin::mail_test))
            .route(admin::REPORTS, get(admin::reports));
    }
    router
        .fallback(|| async { AppError::not_found() })
        .method_not_allowed_fallback(|| async {
            let body = anyplay_api::ApiError::new(
                anyplay_api::ErrorCode::BadRequest,
                "This path does not take this method.",
            );
            (axum::http::StatusCode::METHOD_NOT_ALLOWED, axum::Json(body))
        })
        .layer(middleware::from_fn_with_state(state.clone(), log_requests))
        .with_state(state)
}

/// `/metrics`, for the listener of its own.
pub fn metrics_router(state: AppState) -> Router {
    Router::new()
        .route("/metrics", get(metrics))
        .with_state(state)
}

/// Routes whose address holds a secret. The log shows their template, not the address.
const SECRET_PATHS: [&str; 2] = [path::JOIN_CODE_REDEEM, "/web/api/v1/join/{code}"];

/// Logs each request's method, path, status and time, and counts it by route. Query strings and
/// bodies are left out.
async fn log_requests(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let mut path = request.uri().path().to_owned();
    // The route template, such as `/v1/sessions/{id}`, so the metrics keep few labels.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".to_owned());
    // A join code in the address is a secret that stands for a player: the log gets the template.
    if SECRET_PATHS.contains(&route.as_str()) {
        path.clone_from(&route);
    }
    let started = std::time::Instant::now();
    let response = next.run(request).await;
    let took = started.elapsed();
    state
        .metrics
        .request(&route, response.status().as_u16(), took);
    tracing::debug!(
        %method,
        %path,
        status = response.status().as_u16(),
        ms = took.as_millis() as u64,
        "request"
    );
    response
}

async fn ready(State(state): State<AppState>) -> Response {
    if state.draining.load(Ordering::Relaxed) {
        return (StatusCode::SERVICE_UNAVAILABLE, "draining").into_response();
    }
    let store = tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query("SELECT 1").execute(&state.db),
    )
    .await;
    if !matches!(store, Ok(Ok(_))) {
        return (StatusCode::SERVICE_UNAVAILABLE, "the store does not answer").into_response();
    }
    if state.news.nats_connected() == Some(false) {
        return (StatusCode::SERVICE_UNAVAILABLE, "NATS is not connected").into_response();
    }
    "ready".into_response()
}

async fn metrics(State(state): State<AppState>) -> Response {
    let (connections, news_sent, news_received) = state.news.counts();
    let text = state.metrics.render(&Snapshot {
        node: &state.node,
        connections,
        news_sent,
        news_received,
        push: state.push.as_ref().map(|p| {
            use std::sync::atomic::Ordering::Relaxed;
            (
                p.sent.load(Relaxed),
                p.failed.load(Relaxed),
                p.pruned.load(Relaxed),
            )
        }),
    });
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], text).into_response()
}
