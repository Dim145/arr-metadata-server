//! One error type for every handler.
//!
//! Internal causes are logged in full and replaced by a generic message in the
//! response, so provider URLs, API keys and SQL never reach a client.

use std::time::Duration;

use axum::{
    Json,
    http::{HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

pub type AppResult<T> = std::result::Result<T, AppError>;

/// What a refusal for going too fast tells the caller: when a request would
/// be let through again, and — where the limit is a quota over a window —
/// that quota, as the `RateLimit` and `RateLimit-Policy` fields of the IETF
/// httpapi draft say it. Without one, `Retry-After` alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Throttle {
    /// How long until a request would be let through.
    pub retry_after: Duration,
    /// The quota that was spent, when the limit is one.
    pub quota: Option<Quota>,
}

/// A number of requests a window lets through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quota {
    pub requests: u32,
    pub window: Duration,
}

/// The fields the draft names, lower-cased as the wire carries them.
const RATELIMIT: HeaderName = HeaderName::from_static("ratelimit");
const RATELIMIT_POLICY: HeaderName = HeaderName::from_static("ratelimit-policy");

impl Throttle {
    /// Come back after `wait`; no quota to tell of.
    pub const fn after(wait: Duration) -> Self {
        Self {
            retry_after: wait,
            quota: None,
        }
    }

    /// Come back after `wait`: the `requests` a `window` lets through are
    /// spent.
    pub const fn spent(requests: u32, window: Duration, wait: Duration) -> Self {
        Self {
            retry_after: wait,
            quota: Some(Quota { requests, window }),
        }
    }

    /// The wait in whole seconds, rounded up and never nought: told `0`, a
    /// client comes straight back into the same refusal.
    fn seconds(&self) -> u64 {
        let whole = self.retry_after.as_secs() + u64::from(self.retry_after.subsec_nanos() > 0);
        whole.max(1)
    }

    /// The fields a refusal carries. `t`, when it is there, is the wait
    /// `Retry-After` gives: the draft asks that a client told both is never
    /// told to come back before the window it was refused in is over.
    fn headers(&self) -> Vec<(HeaderName, HeaderValue)> {
        let seconds = self.seconds();
        let mut headers = vec![(header::RETRY_AFTER, HeaderValue::from(seconds))];
        if let Some(quota) = self.quota {
            let window = quota.window.as_secs().max(1);
            let fields = [
                (RATELIMIT, format!("\"default\";r=0;t={seconds}")),
                (
                    RATELIMIT_POLICY,
                    format!("\"default\";q={};w={window}", quota.requests),
                ),
            ];
            for (name, value) in fields {
                if let Ok(value) = HeaderValue::try_from(value) {
                    headers.push((name, value));
                }
            }
        }
        headers
    }
}

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

    /// Too many requests: when to come back, and the quota spent where the
    /// limit is one.
    #[error("too many requests")]
    RateLimited(Throttle),

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
            Self::RateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
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
            Self::RateLimited(_) => "rate_limited",
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
            | Self::RateLimited(_)
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

        let mut response = (status, Json(body)).into_response();
        if let Self::RateLimited(throttle) = &self {
            response.headers_mut().extend(throttle.headers());
        }
        response
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

    fn field<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    }

    #[test]
    fn a_spent_quota_says_when_to_come_back_and_what_the_quota_is() {
        let response = AppError::RateLimited(Throttle::spent(
            600,
            Duration::from_secs(60),
            Duration::from_millis(11_200),
        ))
        .into_response();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        // Whole seconds, rounded up: told 11, a client would be early.
        assert_eq!(field(&response, "retry-after"), Some("12"));
        assert_eq!(field(&response, "ratelimit"), Some("\"default\";r=0;t=12"));
        assert_eq!(
            field(&response, "ratelimit-policy"),
            Some("\"default\";q=600;w=60")
        );
    }

    #[test]
    fn a_limit_without_a_window_says_when_alone() {
        let response =
            AppError::RateLimited(Throttle::after(Duration::from_secs(5))).into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(field(&response, "retry-after"), Some("5"));
        assert_eq!(field(&response, "ratelimit"), None);
        assert_eq!(field(&response, "ratelimit-policy"), None);

        // Never nought, which would send the client straight back.
        let response = AppError::RateLimited(Throttle::after(Duration::ZERO)).into_response();
        assert_eq!(field(&response, "retry-after"), Some("1"));
        let response =
            AppError::RateLimited(Throttle::after(Duration::from_millis(100))).into_response();
        assert_eq!(field(&response, "retry-after"), Some("1"));

        // And nothing of the kind on any other refusal.
        let response = AppError::Forbidden.into_response();
        assert_eq!(field(&response, "retry-after"), None);
    }
}
