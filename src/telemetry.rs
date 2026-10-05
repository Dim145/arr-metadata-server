//! Tracing setup, and keeping secrets out of what is traced.

use std::sync::{LazyLock, RwLock};

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Initialise the global subscriber.
///
/// `AMS_LOG` (or `RUST_LOG`) sets the filter. `AMS_LOG_FORMAT=json` switches to
/// structured output for log shippers.
pub fn init() {
    let filter = EnvFilter::try_from_env("AMS_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info,arr_metadata_server=info,tower_http=info"));

    let json = std::env::var("AMS_LOG_FORMAT")
        .map(|v| v.eq_ignore_ascii_case("json"))
        .unwrap_or(false);

    let registry = tracing_subscriber::registry().with(filter);

    if json {
        registry
            .with(fmt::layer().json().flatten_event(true))
            .init();
    } else {
        registry.with(fmt::layer().with_target(true)).init();
    }
}

/// The request span: what `tower_http`'s own records, but the address without
/// the values of the parameters a key travels in. `?api_key=` and `?apikey=`
/// are how TMDB clients, feeds and the import lists carry theirs, and the
/// default span wrote every one of them to the log at `AMS_LOG=debug`.
#[derive(Clone, Copy, Debug, Default)]
pub struct RequestSpan;

impl<B> tower_http::trace::MakeSpan<B> for RequestSpan {
    fn make_span(&mut self, request: &axum::http::Request<B>) -> tracing::Span {
        tracing::debug_span!(
            "request",
            method = %request.method(),
            uri = %redact_uri(request.uri()),
            version = ?request.version(),
        )
    }
}

/// A request's path and query, the query's secrets masked.
pub fn redact_uri(uri: &axum::http::Uri) -> String {
    match uri.query() {
        Some(query) => format!("{}?{}", uri.path(), mask_parameters(query)),
        None => uri.path().to_string(),
    }
}

/// Values that are secrets wherever they turn up: the keys and passwords the
/// configuration holds, the address webhooks are posted to.
static SECRETS: LazyLock<RwLock<Vec<String>>> = LazyLock::new(|| RwLock::new(Vec::new()));

/// What a value shorter than this is: too short to be told from ordinary
/// text, and so left alone rather than masked wherever it happens to occur.
const SHORTEST_SECRET: usize = 8;

/// Add a value to those [`redact`] masks wherever it occurs.
pub fn redact_also(secret: &str) {
    let secret = secret.trim();
    if secret.len() < SHORTEST_SECRET {
        return;
    }
    if let Ok(mut known) = SECRETS.write()
        && !known.iter().any(|k| k == secret)
    {
        known.push(secret.to_string());
        // The longest first, so that a value containing another is masked
        // whole rather than around the shorter one.
        known.sort_by_key(|k| std::cmp::Reverse(k.len()));
    }
}

/// Text fit for the log.
///
/// Masks the values of the parameters keys and tokens travel in, what
/// follows `Bearer` and `Basic`, the value of an `Authorization`, the password
/// of an address, and every value given to [`redact_also`]; and escapes
/// control characters, so that what arrives from outside writes one line and
/// no line of its own.
pub fn redact(text: &str) -> String {
    let mut text = text.to_string();
    if let Ok(known) = SECRETS.read() {
        for secret in known.iter() {
            if text.contains(secret.as_str()) {
                text = text.replace(secret.as_str(), MASK);
            }
        }
    }
    let text = mask_parameters(&text);
    let text = mask_after(&text, &["bearer ", "basic "], is_token_byte);
    let text = mask_authorization(&text);
    let text = mask_url_passwords(&text);
    escape_controls(&text)
}

const MASK: &str = "***";

/// The parameters whose values are secrets: keys, tokens, passwords.
const SECRET_PARAMETERS: &[&str] = &[
    "api_key",
    "apikey",
    "api-key",
    "access_token",
    "refresh_token",
    "id_token",
    "token",
    "client_secret",
    "secret",
    "password",
    "passwd",
    "key",
    "pin",
    "signature",
    "x-amz-signature",
    "x-amz-credential",
];

/// `api_key=abc&language=fr` is `api_key=***&language=fr`, wherever such a
/// pair is: in a query, in a URL inside an error, in a sentence.
fn mask_parameters(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let at_boundary = i == 0
            || matches!(
                bytes[i - 1],
                b'?' | b'&' | b';' | b' ' | b'\t' | b'\n' | b'(' | b'"' | b'\'' | b',' | b'{'
            );
        if at_boundary
            && let Some(name) = SECRET_PARAMETERS.iter().find(|name| {
                bytes.len() > i + name.len()
                    && bytes[i..i + name.len()].eq_ignore_ascii_case(name.as_bytes())
                    && bytes[i + name.len()] == b'='
            })
        {
            let start = i + name.len() + 1;
            let end = text[start..]
                .find(|c: char| {
                    c.is_whitespace() || matches!(c, '&' | '"' | '\'' | ')' | ',' | ';' | '>' | '#')
                })
                .map_or(text.len(), |offset| start + offset);
            out.push_str(&text[i..start]);
            if end > start {
                out.push_str(MASK);
            }
            i = end;
            continue;
        }
        let Some(c) = text[i..].chars().next() else {
            break;
        };
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'+' | b'/' | b'=')
}

/// Mask what follows any of `words` (matched without regard to case, as a
/// word of its own) for as long as `keep` says it is part of the credential
/// — and long enough to be one: "the basic idea" is left as it is.
fn mask_after(text: &str, words: &[&str], keep: fn(u8) -> bool) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let at_word = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if at_word
            && let Some(word) = words.iter().find(|word| {
                bytes.len() > i + word.len()
                    && bytes[i..i + word.len()].eq_ignore_ascii_case(word.as_bytes())
            })
        {
            let start = i + word.len();
            let end = bytes[start..]
                .iter()
                .position(|b| !keep(*b))
                .map_or(text.len(), |offset| start + offset);
            if end - start >= SHORTEST_SECRET {
                out.push_str(&text[i..start]);
                out.push_str(MASK);
                i = end;
                continue;
            }
        }
        let Some(c) = text[i..].chars().next() else {
            break;
        };
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// `authorization: abc`, `"authorization": "abc"`: the value masked, as a
/// header dump or a request's debug print carries it.
fn mask_authorization(text: &str) -> String {
    const WORD: &[u8] = b"authorization";
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if bytes.len() > i + WORD.len() && bytes[i..i + WORD.len()].eq_ignore_ascii_case(WORD) {
            // The separator: a colon or an equals sign, with quotes and
            // spaces around it.
            let mut start = i + WORD.len();
            let mut separated = false;
            while start < bytes.len() && matches!(bytes[start], b'"' | b'\'' | b' ' | b':' | b'=') {
                separated |= matches!(bytes[start], b':' | b'=');
                start += 1;
            }
            if separated {
                let end = bytes[start..]
                    .iter()
                    .position(|b| matches!(b, b'"' | b'\'' | b',' | b'\n' | b'\r' | b'}' | b';'))
                    .map_or(text.len(), |offset| start + offset);
                out.push_str(&text[i..start]);
                if end > start {
                    out.push_str(MASK);
                }
                i = end;
                continue;
            }
        }
        let Some(c) = text[i..].chars().next() else {
            break;
        };
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// `postgres://ams:secret@db/ams` is `postgres://ams:***@db/ams`; a token
/// standing alone before the `@` is masked whole.
fn mask_url_passwords(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let authority_start = at + 3;
        out.push_str(&rest[..authority_start]);
        let tail = &rest[authority_start..];
        let authority_end = tail
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '/' | '?' | '#' | '"' | '\'' | ')' | '>')
            })
            .unwrap_or(tail.len());
        let authority = &tail[..authority_end];
        match authority.rfind('@') {
            Some(userinfo_end) => {
                let userinfo = &authority[..userinfo_end];
                match userinfo.split_once(':') {
                    Some((user, _)) => {
                        out.push_str(user);
                        out.push(':');
                        out.push_str(MASK);
                    }
                    None => out.push_str(MASK),
                }
                out.push_str(&authority[userinfo_end..]);
            }
            None => out.push_str(authority),
        }
        rest = &tail[authority_end..];
    }
    out.push_str(rest);
    out
}

/// Control characters as escapes: a line break written as `\n`, never as one.
fn escape_controls(text: &str) -> String {
    if !text.chars().any(char::is_control) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_in_an_address_are_masked() {
        assert_eq!(
            redact(
                "error sending request for url (https://api.themoviedb.org/3/movie/1?api_key=abc123&language=fr)"
            ),
            "error sending request for url (https://api.themoviedb.org/3/movie/1?api_key=***&language=fr)"
        );
        assert_eq!(
            mask_parameters("apikey=ams_x&term=dune&Token=t0k"),
            "apikey=***&term=dune&Token=***"
        );
        // A name that merely ends like one is left alone.
        assert_eq!(mask_parameters("monkey=1&turkey=2"), "monkey=1&turkey=2");
        assert_eq!(mask_parameters("api_key="), "api_key=");
    }

    #[test]
    fn credentials_after_their_scheme_are_masked() {
        assert_eq!(
            redact("Authorization: Bearer eyJhbGciOi.abc_def"),
            "Authorization: ***"
        );
        assert_eq!(redact("sent bearer ams_secret to"), "sent bearer *** to");
        assert_eq!(redact("the basic idea"), "the basic idea");
        assert_eq!(redact("unbearer ams_secret"), "unbearer ams_secret");
        assert_eq!(
            redact(r#"{"authorization": "Basic dXNlcjpwYXNz", "x": 1}"#),
            r#"{"authorization": "***", "x": 1}"#
        );
        // The word alone, with nothing assigned, is text.
        assert_eq!(redact("authorization failed"), "authorization failed");
    }

    #[test]
    fn an_address_keeps_its_user_and_loses_its_password() {
        assert_eq!(
            redact("cannot connect to postgres://ams:s3cret@db:5432/ams"),
            "cannot connect to postgres://ams:***@db:5432/ams"
        );
        assert_eq!(
            redact("redis://token@cache:6379 refused"),
            "redis://***@cache:6379 refused"
        );
        assert_eq!(
            redact("see https://example.com/a@b"),
            "see https://example.com/a@b"
        );
    }

    #[test]
    fn a_registered_secret_is_masked_wherever_it_is() {
        redact_also("https://discord.com/api/webhooks/1/zZtokenZz");
        redact_also("short");
        assert_eq!(
            redact("posting to https://discord.com/api/webhooks/1/zZtokenZz failed"),
            "posting to *** failed"
        );
        assert_eq!(redact("too short to mask"), "too short to mask");
    }

    #[test]
    fn a_line_break_from_outside_stays_on_the_line() {
        assert_eq!(
            redact("name \"admin\nINFO signed in\"\r"),
            "name \"admin\\nINFO signed in\"\\r"
        );
    }

    #[test]
    fn the_request_span_never_holds_a_key() {
        let uri: axum::http::Uri = "/api/v1/lists/x/sonarr.json?apikey=ams_secret&limit=5"
            .parse()
            .unwrap();
        assert_eq!(
            redact_uri(&uri),
            "/api/v1/lists/x/sonarr.json?apikey=***&limit=5"
        );
        let bare: axum::http::Uri = "/health".parse().unwrap();
        assert_eq!(redact_uri(&bare), "/health");
    }
}
