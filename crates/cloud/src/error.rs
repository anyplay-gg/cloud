//! Errors as the API answers them: an `ApiError` body with the status of its code.

use anyplay_api::{ApiError, ErrorCode, Expected};
use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

/// An error that becomes an HTTP answer.
#[derive(Debug)]
pub struct AppError {
    pub body: ApiError,
    /// Seconds for the `Retry-After` header.
    pub retry_after: Option<u64>,
}

pub type Result<T, E = AppError> = std::result::Result<T, E>;

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            body: ApiError::new(code, message),
            retry_after: None,
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unauthorized, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Forbidden, message)
    }

    pub fn not_found() -> Self {
        Self::new(ErrorCode::NotFound, "There is nothing like that here.")
    }

    pub fn limit(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Limit, message)
    }

    pub fn too_large(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::TooLarge, message)
    }

    pub fn internal() -> Self {
        Self::new(ErrorCode::Internal, "The cloud failed. Try again later.")
    }

    pub fn banned(until: Option<u64>) -> Self {
        let mut error = Self::new(ErrorCode::Banned, "This account is banned.");
        error.body.until = until;
        error
    }

    pub fn version_mismatch(expected: Expected) -> Self {
        let mut error = Self::new(
            ErrorCode::VersionMismatch,
            "The game version or the AnyPS5 build differs from the session's.",
        );
        error.body.expected = Some(expected);
        error
    }

    pub fn rate_limited(retry_after: u64) -> Self {
        let mut error = Self::new(ErrorCode::RateLimited, "Too many requests. Wait a moment.");
        error.retry_after = Some(retry_after.max(1));
        error
    }
}

impl From<sqlx::Error> for AppError {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!(%error, "database error");
        crate::metrics::STORE_ERRORS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self::internal()
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.body.error.status())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut response = (status, Json(self.body)).into_response();
        if let Some(seconds) = self.retry_after
            && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}
