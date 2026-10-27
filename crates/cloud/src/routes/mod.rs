//! The HTTP routes. Paths and their `{id}` placeholders come from `anyplay_api::path`.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyplay_api::client::{CLIENT_HEADER, Version, launcher_too_old};
use anyplay_api::path;
use axum::Router;
use axum::extract::{MatchedPath, Request, State};
use axum::http::{Method, StatusCode, header};
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
pub mod lfg;
pub mod media;
pub mod messages;
pub mod misc;
pub mod moderation;
pub mod parties;
pub mod profiles;
pub mod runs;
pub mod sessions;
pub mod shares;
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
        .route(
            "/v1/me/public-profile",
            get(accounts::get_public_profile).put(accounts::set_public_profile),
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
        .route(path::CONVERSATION_MEDIA, post(media::upload))
        .route(path::MEDIA, get(media::download))
        .route(path::MOD_MEDIA, get(media::moderate))
        .route(path::LFG, get(lfg::board).post(lfg::create))
        .route(path::LFG_MINE, get(lfg::mine))
        .route(path::LFG_LISTING, delete(lfg::close))
        .route(path::LFG_ASK, post(lfg::ask).delete(lfg::withdraw))
        .route(path::LFG_REQUEST, delete(lfg::decline))
        .route(path::LFG_REQUEST_ACCEPT, post(lfg::accept))
        .route(path::CONVERSATION_ACCEPT, post(messages::accept))
        .route(path::CONVERSATION_MEMBERS, post(messages::add_members))
        .route(
            path::CONVERSATION_MESSAGES,
            get(messages::messages).post(messages::send),
        )
        .route(
            path::CONVERSATION_MESSAGE,
            patch(messages::edit).delete(messages::unsend),
        )
        .route(
            path::CONVERSATION_REACTION,
            put(messages::react).delete(messages::unreact),
        )
        .route(path::CONVERSATION_TYPING, put(messages::typing))
        .route(path::CONVERSATION_SETTINGS, patch(messages::settings))
        .route(path::CONVERSATION_READ, put(messages::mark_read));
    // Share play (network spec/share-play.md)
    router = router
        .route("/v1/shares", post(shares::create))
        .route(
            "/v1/share",
            get(shares::get).delete(shares::end).patch(shares::update),
        )
        .route("/v1/share/accept", post(shares::accept))
        .route("/v1/share/invitations", get(shares::invitations));
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
        .layer(middleware::from_fn_with_state(
            Arc::new(Guard::new(&state.config)),
            guard,
        ))
        .layer(middleware::from_fn_with_state(
            Arc::new(ClientGate::new(&state.config)),
            client_gate,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), log_requests))
        .with_state(state)
}

/// Retires old launchers (`min_launcher_version`): a request that names itself
/// `X-AnyPlay-Client: launcher/<version>` with a version below the minimum is refused with
/// `client_too_old` (426) before it is handled. It covers every `/v1/` path, the event
/// handshake included, except `GET /v1/info`, which an old client may still read. Anything else
/// passes: no header, a value that does not read, another product. Health checks, metrics and the
/// account web are not under `/v1/`.
struct ClientGate {
    minimum: Option<Version>,
}

impl ClientGate {
    fn new(config: &crate::Config) -> Self {
        // `Config::validate` has refused a value that does not read; a hand-built config that
        // skipped it gets no gate rather than a panic.
        Self {
            minimum: config.min_launcher(),
        }
    }
}

async fn client_gate(
    State(gate): State<Arc<ClientGate>>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(minimum) = &gate.minimum {
        let route = request.uri().path();
        if route.starts_with("/v1/") && route != path::INFO {
            let client = request
                .headers()
                .get(CLIENT_HEADER)
                .and_then(|v| v.to_str().ok());
            if launcher_too_old(client, minimum) {
                let mut error = AppError::new(
                    anyplay_api::ErrorCode::ClientTooOld,
                    format!("This launcher is too old. Update to {minimum} or newer."),
                );
                error.body.minimum = Some(minimum.as_str().into());
                return error.into_response();
            }
        }
    }
    next.run(request).await
}

/// What a node does to keep a slow store from piling requests up: it cuts off a request that takes
/// too long and, when asked to, turns requests away once too many are in flight. Both answer 503
/// with `Retry-After`, so a client backs off instead of waiting.
struct Guard {
    timeout: Option<Duration>,
    permits: Option<Arc<tokio::sync::Semaphore>>,
}

impl Guard {
    fn new(config: &crate::Config) -> Self {
        Self {
            timeout: (config.request_timeout_secs > 0)
                .then(|| Duration::from_secs(config.request_timeout_secs)),
            permits: (config.max_in_flight_requests > 0).then(|| {
                Arc::new(tokio::sync::Semaphore::new(
                    config.max_in_flight_requests as usize,
                ))
            }),
        }
    }
}

/// Routes left out of both limits. The media routes carry files of up to 100 MB and have a
/// deadline and a cap of their own (`routes::media`). The event connection lives for hours: its handler answers the
/// upgrade at once and the socket runs on a task of its own, but it stays out so that stays true.
/// The health checks must answer exactly when the node is loaded: a shed liveness probe would make
/// the orchestrator restart the pod then.
fn is_exempt(route: &str) -> bool {
    matches!(
        route,
        path::EVENTS | HEALTH | READY | path::CONVERSATION_MEDIA | path::MEDIA | path::MOD_MEDIA
    )
}

async fn guard(State(guard): State<Arc<Guard>>, request: Request, next: Next) -> Response {
    let exempt = request
        .extensions()
        .get::<MatchedPath>()
        .is_some_and(|m| is_exempt(m.as_str()));
    if exempt || (guard.timeout.is_none() && guard.permits.is_none()) {
        return next.run(request).await;
    }
    // Shed, never queue: a request that waits for a place only adds to the pile.
    let permit = match &guard.permits {
        Some(permits) => match permits.clone().try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => {
                crate::metrics::REQUESTS_SHED.fetch_add(1, Ordering::Relaxed);
                return busy(1, false);
            }
        },
        None => None,
    };
    let Some(limit) = guard.timeout else {
        let _permit = permit;
        return next.run(request).await;
    };
    // A write that has begun is not cut off: it may have committed, or sit between its steps (the
    // news and pushes after a commit), and dropping it half-way would leave the client to retry
    // something that is already done. It runs to its end on a task of its own and only the wait
    // for its answer stops; its place under the cap is held until it ends. A read has nothing to
    // finish, so it is dropped.
    let writes = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if !writes {
        let _permit = permit;
        return match tokio::time::timeout(limit, next.run(request)).await {
            Ok(response) => response,
            Err(_) => timed_out(limit, false),
        };
    }
    let mut task = tokio::spawn(async move {
        let _permit = permit;
        next.run(request).await
    });
    match tokio::time::timeout(limit, &mut task).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            tracing::error!(%error, "a request handler panicked");
            AppError::internal().into_response()
        }
        // Dropping the handle leaves the task running.
        Err(_) => timed_out(limit, true),
    }
}

fn timed_out(limit: Duration, still_running: bool) -> Response {
    crate::metrics::REQUEST_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
    tracing::warn!(
        limit_secs = limit.as_secs(),
        still_running,
        "a request ran into the request timeout"
    );
    busy(2, still_running)
}

/// The 503 of the guard. For a write that keeps running it says so: the change may still land, so
/// a client checks before it repeats a create.
fn busy(retry_after: u64, write_may_finish: bool) -> Response {
    let message = if write_may_finish {
        "The cloud is busy. Your change may still go through; check before you try again."
    } else {
        "The cloud is busy. Try again in a moment."
    };
    let mut error = AppError::new(anyplay_api::ErrorCode::Internal, message);
    error.retry_after = Some(retry_after);
    let mut response = error.into_response();
    *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
    response
}

/// `/metrics`, for the listener of its own.
pub fn metrics_router(state: AppState) -> Router {
    Router::new()
        .route("/metrics", get(metrics))
        .with_state(state)
}

/// Routes whose address holds a secret. The log shows their template, not the address.
const SECRET_PATHS: [&str; 3] = [
    path::JOIN_CODE_REDEEM,
    "/web/api/v1/join/{code}",
    "/web/api/v1/link/{code}",
];

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
        pool: (
            state.db.size(),
            state.db.num_idle(),
            state.db.options().get_max_connections(),
        ),
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
