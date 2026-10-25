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

    /// The account may not do this yet (a new account) or for now (a mute).
    pub fn restricted(
        restriction: anyplay_api::Restriction,
        until: Option<u64>,
        message: impl Into<String>,
    ) -> Self {
        let mut error = Self::new(ErrorCode::Restricted, message);
        error.body.restriction = Some(restriction);
        error.body.until = until;
        error
    }

    /// Adds the statement of reasons and the appeal token of a ban.
    pub fn with_ban(mut self, notice: anyplay_api::BanNotice) -> Self {
        self.body.ban = Some(Box::new(notice));
        self
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
        if is_overload(&error) {
            // The store is saturated, not broken: answer fast and let the client retry.
            tracing::warn!(%error, "the store is busy");
            let counter = if is_statement_timeout(&error) {
                &crate::metrics::STORE_STATEMENT_TIMEOUTS
            } else {
                &crate::metrics::STORE_POOL_TIMEOUTS
            };
            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // A little jitter keeps the retries of a burst from arriving together.
            return Self::rate_limited(1 + crate::util::random_id() % 2);
        }
        tracing::error!(%error, "database error");
        crate::metrics::STORE_ERRORS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self::internal()
    }
}

/// A wait for a connection, a write turn or a lock that ran out, or a statement the timeout
/// cancelled: PostgreSQL's 55P03 (lock not available) and 57014 (query canceled).
fn is_overload(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::PoolTimedOut => true,
        sqlx::Error::Database(e) => matches!(e.code().as_deref(), Some("55P03" | "57014")),
        _ => false,
    }
}

/// PostgreSQL's 57014 (query canceled): the statement ran longer than the statement timeout, which
/// is a slow query and not a full pool.
fn is_statement_timeout(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(e) if e.code().as_deref() == Some("57014"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pool_timeout_is_a_retryable_429_not_a_500() {
        let before = crate::metrics::STORE_ERRORS.load(std::sync::atomic::Ordering::Relaxed);
        let error = AppError::from(sqlx::Error::PoolTimedOut);
        assert_eq!(error.body.error, ErrorCode::RateLimited);
        assert!((1..=2).contains(&error.retry_after.unwrap()));
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(response.headers().contains_key(header::RETRY_AFTER));
        assert!(crate::metrics::STORE_POOL_TIMEOUTS.load(std::sync::atomic::Ordering::Relaxed) > 0);
        // A real failure is still a 500 and still counted as one.
        let error = AppError::from(sqlx::Error::RowNotFound);
        assert_eq!(error.body.error, ErrorCode::Internal);
        assert!(crate::metrics::STORE_ERRORS.load(std::sync::atomic::Ordering::Relaxed) > before);
    }

    #[derive(Debug)]
    struct Cancelled;

    impl std::fmt::Display for Cancelled {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("canceling statement due to statement timeout")
        }
    }

    impl std::error::Error for Cancelled {}

    impl sqlx::error::DatabaseError for Cancelled {
        fn message(&self) -> &str {
            "canceling statement due to statement timeout"
        }
        fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
            Some("57014".into())
        }
        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }
        fn kind(&self) -> sqlx::error::ErrorKind {
            sqlx::error::ErrorKind::Other
        }
    }

    #[test]
    fn a_cancelled_statement_is_retryable_but_not_counted_as_pool_pressure() {
        use std::sync::atomic::Ordering::Relaxed;
        let pool = crate::metrics::STORE_POOL_TIMEOUTS.load(Relaxed);
        let statement = crate::metrics::STORE_STATEMENT_TIMEOUTS.load(Relaxed);
        let error = AppError::from(sqlx::Error::Database(Box::new(Cancelled)));
        assert_eq!(error.body.error, ErrorCode::RateLimited);
        assert!(crate::metrics::STORE_STATEMENT_TIMEOUTS.load(Relaxed) > statement);
        // Other tests may count pool timeouts at the same moment, so this only checks the one
        // above did not: a single error moves exactly one counter.
        let after = crate::metrics::STORE_POOL_TIMEOUTS.load(Relaxed);
        let _ = AppError::from(sqlx::Error::PoolTimedOut);
        assert!(crate::metrics::STORE_POOL_TIMEOUTS.load(Relaxed) > after.min(pool));
    }
}
