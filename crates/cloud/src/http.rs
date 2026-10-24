//! Request extractors: the signed-in account, the client's address and JSON bodies with our
//! errors.

use std::net::{IpAddr, SocketAddr};

use anyplay_api::MAX_BODY;
use anyplay_ticket::{Kind, TicketError};
use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Request};
use axum::http::header::{AUTHORIZATION, HeaderMap};
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use crate::error::AppError;
use crate::state::AppState;
use crate::util;

/// The `Bearer` token of a request, if any.
pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
        .filter(|t| !t.is_empty())
}

/// The account and device behind a valid access token. Checking it also counts the request
/// against the account's rate limit.
#[derive(Clone, Copy, Debug)]
pub struct Auth {
    pub account: u64,
    pub device: u64,
}

pub enum AuthFailure {
    /// No token, or one that was not issued by this cloud.
    Invalid,
    /// A genuine token past its expiry.
    Expired,
}

/// Checks the token alone, without the device, ban and rate limit.
pub fn verify_access(state: &AppState, headers: &HeaderMap) -> Result<Auth, AuthFailure> {
    let token = bearer(headers).ok_or(AuthFailure::Invalid)?;
    match state.verifier.verify(token, Kind::Access, util::now()) {
        Ok(claims) => match claims.device {
            Some(device) => Ok(Auth {
                account: claims.account,
                device,
            }),
            None => Err(AuthFailure::Invalid),
        },
        Err(TicketError::Expired) => Err(AuthFailure::Expired),
        Err(_) => Err(AuthFailure::Invalid),
    }
}

/// Counts a request against the account's limit.
pub fn count_request(state: &AppState, account: u64) -> Result<(), AppError> {
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("account:{account}"),
            rates.account,
            rates.account_window,
        )
        .map_err(AppError::rate_limited)
}

impl FromRequestParts<AppState> for Auth {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth = match verify_access(state, &parts.headers) {
            Ok(auth) => auth,
            Err(AuthFailure::Expired) => {
                return Err(AppError::unauthorized(
                    "The access token has expired. Refresh it.",
                ));
            }
            Err(AuthFailure::Invalid) => {
                return Err(AppError::unauthorized(
                    "Send a valid access token as a Bearer token.",
                ));
            }
        };
        state.check_device(auth.device, auth.account)?;
        count_request(state, auth.account)?;
        Ok(auth)
    }
}

/// The address a request comes from: the peer, or with `trust_proxy` the first hop of
/// `X-Forwarded-For`.
#[derive(Clone, Copy, Debug)]
pub struct ClientIp(pub IpAddr);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip())
            .unwrap_or(IpAddr::from([0, 0, 0, 0]));
        if state.config.trust_proxy
            && let Some(first) = parts
                .headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .and_then(|v| v.trim().parse::<IpAddr>().ok())
        {
            return Ok(ClientIp(first));
        }
        Ok(ClientIp(peer))
    }
}

/// A JSON body of at most 64 KiB, read with our own errors.
pub struct ApiJson<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for ApiJson<T> {
    type Rejection = AppError;

    async fn from_request(request: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let too_large = || AppError::too_large("The request body is too large.");
        let announced = request
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok());
        if announced.is_some_and(|n| n > MAX_BODY) {
            return Err(too_large());
        }
        let bytes = axum::body::to_bytes(request.into_body(), MAX_BODY)
            .await
            .map_err(|_| too_large())?;
        let value = serde_json::from_slice(&bytes)
            .map_err(|e| AppError::bad_request(format!("The body is not valid: {e}")))?;
        Ok(ApiJson(value))
    }
}

/// An account or device ID from a path; anything else cannot exist.
pub fn parse_id(text: &str) -> Result<u64, AppError> {
    match text.parse::<u64>() {
        Ok(id) if (1..=anyplay_api::MAX_ACCOUNT_ID).contains(&id) => Ok(id),
        _ => Err(AppError::not_found()),
    }
}
