//! Shared extractors.

use std::net::IpAddr;

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::request::Parts,
};

use serde::{Deserialize, Deserializer};

use crate::{auth::ip, error::AppError, state::AppState};

/// The caller's address, as the allowlist and the audit log understand it.
///
/// A guard normally resolves this once and leaves it in the request extensions.
/// Routes that run before any guard — sign-in, most obviously — have no such
/// extension, so this falls back to resolving it here. Both paths honour
/// `X-Forwarded-For` only from configured proxies.
#[derive(Clone, Copy, Debug)]
pub struct ClientIp(pub Option<IpAddr>);

impl ClientIp {
    pub fn as_text(&self) -> Option<String> {
        self.0.map(|ip| ip.to_string())
    }
}

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(addr) = parts
            .extensions
            .get::<crate::auth::middleware::ClientAddr>()
        {
            return Ok(Self(Some(addr.0)));
        }

        let peer = parts
            .extensions
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .map(|ci| ci.0);

        Ok(Self(ip::resolve(
            peer,
            &parts.headers,
            &state.config.server.trusted_proxies,
        )))
    }
}

/// Deserialize an optional value that may arrive as an empty string.
///
/// Radarr builds its search URL unconditionally and sends `year=` with nothing
/// after it when the user did not give one. Serde rejects that for an
/// `Option<i32>`, which turns a perfectly ordinary search into a 400. Treat an
/// empty or blank value as absent, and anything unparsable as absent too — a
/// search that ignores a malformed filter is better than one that fails.
pub fn empty_as_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: std::str::FromStr,
{
    let raw = Option::<String>::deserialize(deserializer)?;

    Ok(raw
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize)]
    struct Query {
        #[serde(default, deserialize_with = "empty_as_none")]
        year: Option<i32>,
    }

    fn parse(query: &str) -> Option<i32> {
        serde_urlencoded::from_str::<Query>(query)
            .expect("should not fail")
            .year
    }

    #[test]
    fn an_empty_value_is_absent_rather_than_an_error() {
        // This is the exact shape Radarr sends when no year was given.
        assert_eq!(parse("year="), None);
        assert_eq!(parse("year=%20"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn a_real_value_still_parses() {
        assert_eq!(parse("year=2016"), Some(2016));
        assert_eq!(parse("year=%202016%20"), Some(2016));
    }

    #[test]
    fn an_unparsable_value_is_ignored_not_fatal() {
        // A search that quietly drops a malformed filter beats one that 400s.
        assert_eq!(parse("year=soon"), None);
    }
}
