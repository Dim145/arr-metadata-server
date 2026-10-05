//! One error type for every handler.
//!
//! Internal causes are logged in full and replaced by a generic message in the
//! response, so provider URLs, API keys and SQL never reach a client.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

pub type AppResult<T> = std::result::Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,

    #[error("{0}")]
    BadRequest(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("{0}")]
    Conflict(String),

    /// Refused for a reason the caller may be told, under a code the interface
    /// reads: an account waiting for approval, a key limit reached.
    #[error("{message}")]
    Refused { code: &'static str, message: String },

    /// Switched off by an administrator, such as a whole API surface. Told as
    /// a 503, which is what Sonarr and Radarr treat as "try again later".
    #[error("{message}")]
    Disabled { code: &'static str, message: String },

    #[error("upstream provider unavailable")]
    UpstreamUnavailable(#[source] anyhow::Error),

    #[error("this server has no TMDB API key configured")]
    ProviderNotConfigured,

    #[error("too many requests")]
    RateLimited,

    /// This server called a hostname that resolves back to itself.
    #[error("this request came from this server; an upstream provider resolves to it")]
    LoopDetected,

    #[error(transparent)]
    Internal(anyhow::Error),

    /// A body larger than the route takes.
    #[error("{0}")]
    PayloadTooLarge(String),
}

/// Most failures are internal, but a unique-constraint violation is the caller
/// being told something is already there — and by the time it arrives here it
/// has usually been through `anyhow`, so the original has to be dug back out.
impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        match e.downcast::<sqlx::Error>() {
            Ok(sql) => Self::from(sql),
            Err(other) => Self::Internal(other),
        }
    }
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Refused { .. } => StatusCode::FORBIDDEN,
            Self::Disabled { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::UpstreamUnavailable(_) => StatusCode::BAD_GATEWAY,
            Self::ProviderNotConfigured => StatusCode::SERVICE_UNAVAILABLE,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::LoopDetected => StatusCode::LOOP_DETECTED,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Stable machine-readable discriminator, so clients don't parse prose.
    fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::BadRequest(_) => "bad_request",
            Self::PayloadTooLarge(_) => "payload_too_large",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::Conflict(_) => "conflict",
            Self::Refused { code, .. } | Self::Disabled { code, .. } => code,
            Self::UpstreamUnavailable(_) => "upstream_unavailable",
            Self::ProviderNotConfigured => "provider_not_configured",
            Self::RateLimited => "rate_limited",
            Self::LoopDetected => "loop_detected",
            Self::Internal(_) => "internal",
        }
    }

    /// What the client is allowed to see.
    fn public_message(&self) -> String {
        match self {
            // These carry no internal detail, so the Display form is safe to expose.
            Self::NotFound
            | Self::BadRequest(_)
            | Self::PayloadTooLarge(_)
            | Self::Unauthorized
            | Self::Forbidden
            | Self::Conflict(_)
            | Self::Refused { .. }
            | Self::Disabled { .. }
            | Self::ProviderNotConfigured
            | Self::RateLimited
            | Self::LoopDetected => self.to_string(),
            // These wrap a cause that may name hosts, queries or credentials.
            Self::UpstreamUnavailable(_) => "upstream provider unavailable".into(),
            Self::Internal(_) => "internal server error".into(),
        }
    }

    /// What the log is told: the whole chain of causes on one line, with
    /// whatever in it is a secret masked — a provider's key in the URL an
    /// HTTP client error repeats, a token, a password in an address — and
    /// what came from outside unable to start a line of its own.
    fn log_text(&self) -> String {
        let text = match self {
            Self::Internal(cause) => format!("{cause:#}"),
            Self::UpstreamUnavailable(cause) => format!("{self}: {cause:#}"),
            other => other.to_string(),
        };
        crate::telemetry::redact(&text)
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();

        // A surface an administrator switched off answers 503 on purpose:
        // a Sonarr library refresh against it is not a line of errors.
        if status.is_server_error() && !matches!(self, Self::Disabled { .. }) {
            tracing::error!(error = %self.log_text(), "request failed");
        } else {
            tracing::debug!(error = %self.log_text(), "request rejected");
        }

        let body = ErrorBody {
            error: self.code(),
            message: self.public_message(),
        };

        (status, Json(body)).into_response()
    }
}

/// Anything convertible into `anyhow::Error` becomes an internal error, except
/// where a handler maps it to something more specific first.
impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        if e.as_database_error()
            .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
        {
            // Something is already there. That is a 409 and a sentence the
            // caller can act on, not a 500 — adding a poster by hand whose URL
            // a provider had already supplied used to look like a crash.
            return Self::Conflict("that already exists here".into());
        }

        match e {
            sqlx::Error::RowNotFound => Self::NotFound,
            other => Self::Internal(other.into()),
        }
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        Self::UpstreamUnavailable(e.into())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        Self::Internal(e.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_log_is_told_keeps_the_causes_and_loses_the_secrets() {
        let failed = AppError::UpstreamUnavailable(
            anyhow::anyhow!("error decoding response body for url (https://api.themoviedb.org/3/tv/1?api_key=0123456789abcdef)")
                .context("TMDB answer could not be read"),
        );
        let text = failed.log_text();
        assert!(
            text.starts_with("upstream provider unavailable: TMDB answer could not be read: "),
            "{text}"
        );
        assert!(text.contains("api_key=***"), "{text}");
        assert!(!text.contains("0123456789abcdef"), "{text}");

        let internal = AppError::Internal(
            anyhow::anyhow!("connection refused").context("postgres://ams:hunter2@db/ams"),
        );
        let text = internal.log_text();
        assert_eq!(text, "postgres://ams:***@db/ams: connection refused");

        // A rejection quoting what a caller sent stays on its line.
        let rejected = AppError::BadRequest("unknown action \"x\nINFO forged\"".into());
        assert!(!rejected.log_text().contains('\n'));

        // What the client is told does not change: generic, as before.
        assert_eq!(failed.public_message(), "upstream provider unavailable");
        assert_eq!(internal.public_message(), "internal server error");
    }
}
