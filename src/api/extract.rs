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
///
/// The second field is the trusted proxy that forwarded a chain nobody can
/// read, when that is why the address is unknown: kept for the record — the
/// journal names it, with a note — and never decided on.
#[derive(Clone, Copy, Debug)]
pub struct ClientIp(pub Option<IpAddr>, Option<IpAddr>);

impl ClientIp {
    pub fn as_text(&self) -> Option<String> {
        match (self.0, self.1) {
            (Some(ip), _) => Some(ip.to_string()),
            (None, Some(proxy)) => Some(format!(
                "{proxy} (a proxy; the client's address could not be read)"
            )),
            (None, None) => None,
        }
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
            return Ok(Self(Some(addr.0), None));
        }

        let peer = parts
            .extensions
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .map(|ci| ci.0);

        Ok(
            match ip::caller(peer, &parts.headers, &state.config.server.trusted_proxies) {
                ip::Caller::Known(ip) => Self(Some(ip), None),
                ip::Caller::Unreadable { proxy } => Self(None, Some(proxy)),
                ip::Caller::Missing => Self(None, None),
            },
        )
    }
}

/// The `language` a request asked for, as a tag
/// ([`crate::service::language::tag`]): nothing when it was left out or
/// empty, and a 400 when it is not a language.
///
/// Checked here, where it arrives, because it travels far: into the key
/// translations are filed and cached under, into the parameters and the
/// paths of provider requests, into the log. The value itself is not
/// repeated in the answer.
pub fn language(asked: Option<&str>) -> Result<Option<String>, AppError> {
    match asked.map(str::trim).filter(|l| !l.is_empty()) {
        None => Ok(None),
        Some(raw) => crate::service::language::tag(raw).map(Some).ok_or_else(|| {
            AppError::BadRequest(
                "language is a language tag: two or three letters, then at most a region or a \
                 script after a hyphen — fr, fra, pt-BR"
                    .into(),
            )
        }),
    }
}

/// A language a caller files with something it adds — an image, an
/// alternative title: the shape of a tag ([`language`]), kept as it was
/// written. Nothing when left out or empty.
pub fn filed_language(asked: Option<String>) -> Result<Option<String>, AppError> {
    let asked = asked
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty());
    match asked {
        Some(code) if language(Some(&code))?.is_some() => Ok(Some(code)),
        _ => Ok(None),
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

    #[test]
    fn a_language_is_a_tag_or_refused() {
        assert_eq!(language(None).unwrap(), None);
        assert_eq!(language(Some("  ")).unwrap(), None);
        assert_eq!(language(Some("fr")).unwrap().as_deref(), Some("fr"));
        assert_eq!(language(Some(" fr-fr ")).unwrap().as_deref(), Some("fr-FR"));
        assert_eq!(language(Some("pt_BR")).unwrap().as_deref(), Some("pt-BR"));

        // A path, a query, a string the size of a request: refused, and the
        // answer does not repeat them — it is the same whatever was sent.
        let refusal = |odd: &str| match language(Some(odd)) {
            Err(AppError::BadRequest(why)) => why,
            other => panic!("{odd:?} gave {other:?}"),
        };
        let said = refusal("zz-9");
        for odd in [
            "../../search?query=x#",
            "fr/../x",
            "fr-FR&x=1",
            "f",
            "français",
            "en-US-x-private",
            "zz1",
        ] {
            assert_eq!(refusal(odd), said, "{odd:?}");
        }
        assert_eq!(refusal(&"a".repeat(60_000)), said);

        // Filed as written, once it has the shape of one.
        assert_eq!(
            filed_language(Some(" usa ".into())).unwrap().as_deref(),
            Some("usa")
        );
        assert_eq!(filed_language(Some(String::new())).unwrap(), None);
        assert!(filed_language(Some("<b>".into())).is_err());
    }
}
