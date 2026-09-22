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
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::Conflict(_) => StatusCode::CONFLICT,
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
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::Conflict(_) => "conflict",
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
            | Self::Unauthorized
            | Self::Forbidden
            | Self::Conflict(_)
            | Self::ProviderNotConfigured
            | Self::RateLimited
            | Self::LoopDetected => self.to_string(),
            // These wrap a cause that may name hosts, queries or credentials.
            Self::UpstreamUnavailable(_) => "upstream provider unavailable".into(),
            Self::Internal(_) => "internal server error".into(),
        }
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

        if status.is_server_error() {
            tracing::error!(error = ?self, "request failed");
        } else {
            tracing::debug!(error = %self, "request rejected");
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
