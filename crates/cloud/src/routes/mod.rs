//! The HTTP routes. Paths and their `{id}` placeholders come from `anyplay_api::path`.

use anyplay_api::path;
use axum::Router;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, get, post, put};

use crate::error::AppError;
use crate::state::AppState;

pub mod accounts;
pub mod admin;
pub mod auth;
pub mod events;
pub mod friends;
pub mod misc;
pub mod sessions;

pub fn router(state: AppState) -> Router {
    let mut router = Router::new()
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
        .route(path::SESSIONS, post(sessions::create))
        .route(path::SESSION, get(sessions::get).patch(sessions::update))
        .route(path::SESSION_JOIN, post(sessions::join))
        .route(path::SESSION_LEAVE, post(sessions::leave))
        .route(path::SESSION_MEMBER, delete(sessions::remove_member))
        .route(path::SESSION_INVITATIONS, post(sessions::invite))
        .route(path::INVITATION, delete(sessions::decline))
        .route(path::RELAY_TICKETS, post(misc::relay_tickets))
        .route(path::REPORTS, post(misc::report))
        .route(path::EVENTS, get(events::events));
    if state.config.admin_token.is_some() {
        router = router
            .route(admin::BANS, post(admin::ban))
            .route(admin::BAN, delete(admin::unban))
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
        .layer(middleware::from_fn(log_requests))
        .with_state(state)
}

/// Logs each request's method, path, status and time. Query strings and bodies are left out.
async fn log_requests(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let started = std::time::Instant::now();
    let response = next.run(request).await;
    tracing::debug!(
        %method,
        %path,
        status = response.status().as_u16(),
        ms = started.elapsed().as_millis() as u64,
        "request"
    );
    response
}
