//! Who may call the moderation routes and the player's routes about actions.

use anyplay_ticket::Kind;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use super::{Actor, store};
use crate::db;
use crate::error::AppError;
use crate::http::{Auth, AuthFailure, ClientIp, bearer, count_request, verify_access};
use crate::state::AppState;
use crate::util::{ct_eq, now};

/// A moderator, or the operator who sends the admin token. Moderators use their normal access
/// token; their role is read from the store on every request, so taking it back works at once.
pub struct Staff(pub Actor);

impl FromRequestParts<AppState> for Staff {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let (Some(expected), Some(token)) =
            (state.config.admin_token.as_deref(), bearer(&parts.headers))
            && ct_eq(token.as_bytes(), expected.as_bytes())
        {
            let ClientIp(ip) = ClientIp::from_request_parts(parts, state).await?;
            let rates = &state.tuning.rates;
            state
                .limiter
                .hit(
                    &format!("admin:{}", crate::http::rate_key(ip)),
                    rates.admin_per_ip,
                    rates.account_window,
                )
                .map_err(AppError::rate_limited)?;
            return Ok(Staff(Actor::operator()));
        }
        let auth = Auth::from_request_parts(parts, state).await?;
        let mut conn = state.db.acquire().await?;
        if store::moderator_since(&mut conn, auth.account)
            .await?
            .is_none()
        {
            return Err(AppError::forbidden("This account is not a moderator."));
        }
        // The connection goes back before the next read asks the pool: holding one while asking
        // for another stops the node when as many moderators call at once as the pool has
        // connections (tests/one_connection.rs).
        drop(conn);
        let account = db::account(&state.db, auth.account)
            .await?
            .ok_or_else(AppError::not_found)?;
        Ok(Staff(Actor::account(auth.account, account.handle)))
    }
}

/// The account behind a request about its own actions. It may be banned: an appeal token, or the
/// access token of a device that is still signed in, is enough.
pub struct Concerned(pub u64);

impl FromRequestParts<AppState> for Concerned {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(token) = bearer(&parts.headers)
            && let Ok(claims) = state.verifier.verify(token, Kind::Appeal, now())
        {
            count_request(state, claims.account)?;
            return Ok(Concerned(claims.account));
        }
        let auth = match verify_access(state, &parts.headers) {
            Ok(auth) => auth,
            Err(AuthFailure::Expired) => {
                return Err(AppError::unauthorized(
                    "The access token has expired. Refresh it.",
                ));
            }
            Err(AuthFailure::Invalid) => {
                return Err(AppError::unauthorized(
                    "Send a valid access token or appeal token as a Bearer token.",
                ));
            }
        };
        state
            .check_device_owner(auth.device, auth.account, auth.issued)
            .await?;
        count_request(state, auth.account)?;
        Ok(Concerned(auth.account))
    }
}
