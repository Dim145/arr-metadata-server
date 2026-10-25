//! Tracing setup, the id every request is given, and keeping secrets out of
//! what is traced.
//!
//! Every line goes to standard output, as text or as JSON, and into the
//! [`journal`] the administration reads. Two more ways out are each off
//! until asked for: the spans of a request, sent to a collector over OTLP
//! when the environment names one ([`trace`]), and the error lines, sent to
//! a Sentry or a GlitchTip when the settings name one ([`report`]). Each has
//! its own filter, so that `AMS_LOG=warn` quietens the log without starving
//! the traces or the reports.

mod format;
pub mod journal;
pub mod report;
pub mod trace;

use std::{
    net::SocketAddr,
    sync::{Arc, LazyLock, RwLock},
};

use axum::{
    Router,
    extract::{ConnectInfo, Request},
    http::{HeaderMap, HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use ipnet::IpNet;
use tower_http::request_id::{
    MakeRequestId, PropagateRequestIdLayer, RequestId, SetRequestIdLayer,
};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{
    EnvFilter,
    filter::{FilterExt as _, filter_fn},
    fmt,
    prelude::*,
};

/// What the log writes when `AMS_LOG` and `RUST_LOG` say nothing it can read.
const DEFAULT_FILTER: &str = "info,arr_metadata_server=info,tower_http=info";

/// The target the request span is written under: this module's.
const REQUEST_TARGET: &str = module_path!();

/// Initialise the global subscriber.
///
/// `AMS_LOG` (or `RUST_LOG`) sets the filter, for standard output and the
/// journal alike. `AMS_LOG_FORMAT=json` switches to structured output for
/// log shippers, each line carrying the `trace_id` of the request it was
/// written in. The traces and the error reports have filters of their own.
pub fn init() {
    let json = std::env::var("AMS_LOG_FORMAT")
        .map(|v| v.eq_ignore_ascii_case("json"))
        .unwrap_or(false);

    let (traces, notes) = trace::layer();

    let output = if json {
        fmt::layer()
            .json()
            .flatten_event(true)
            .map_event_format(format::Correlated)
            .boxed()
    } else {
        fmt::layer().with_target(true).boxed()
    };

    tracing_subscriber::registry()
        .with(output.with_filter(log_filter().or(request_spans())))
        .with(journal::layer().with_filter(log_filter().or(request_spans())))
        .with(report::layer())
        .with(crate::metrics::pool_wait_layer())
        .with(traces)
        .init();

    for note in notes {
        note.log();
    }
    panic_hook();
}

/// What `AMS_LOG`, or `RUST_LOG`, asks to be written. Read anew for each
/// layer that writes lines, a filter being of one layer only.
fn log_filter() -> EnvFilter {
    EnvFilter::try_from_env("AMS_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER))
}

/// The request spans, whatever the filter says of their level: the lines a
/// request's handler writes name its ids through them, and `AMS_LOG=warn`
/// must not take the ids off the warnings.
fn request_spans<S>() -> impl tracing_subscriber::layer::Filter<S> {
    filter_fn(|meta| meta.is_span() && meta.target() == REQUEST_TARGET)
        .with_max_level_hint(LevelFilter::INFO)
}

/// Something to say once the subscriber is there to say it with: what the
/// telemetry made of the environment as it was set up.
pub(crate) struct Note {
    pub warning: bool,
    pub text: String,
}

impl Note {
    fn log(self) {
        if self.warning {
            tracing::warn!("{}", self.text);
        } else {
            tracing::info!("{}", self.text);
        }
    }
}

/// A panic, written as an error line before the default hook writes it to
/// standard error: in the JSON log a shipper reads, under the request it
/// happened in, in the journal, and in an error report. A handler's panic is
/// a 500 for its request, so it would otherwise go by unseen. What it says is
/// masked as every logged text is.
fn panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let what = info.payload_as_str().unwrap_or("a panic that said nothing");
        let location = info
            .location()
            .map(|at| format!("{}:{}", at.file(), at.line()))
            .unwrap_or_default();
        tracing::error!(
            target: "panic",
            panic = %redact(what),
            %location,
            "a thread panicked"
        );
        previous(info);
    }));
}

/// Send what is still held to the collector and the error reports, given a
/// few seconds: the last thing a stopping server does.
pub async fn shutdown() {
    tokio::join!(trace::shutdown(), report::shutdown());
}

/// The request span: what `tower_http`'s own records, but the address without
/// the values of the parameters a key travels in — `?api_key=` and `?apikey=`
/// are how TMDB clients, feeds and the import lists carry theirs, and the
/// default span wrote every one of them to the log at `AMS_LOG=debug` — and
/// the request's id, so that every line written while it is served names
/// it. At `info`, and written whatever the filter's level (`request_spans`).
///
/// With traces on, it is made under the span the trace serves the request in
/// ([`trace::serve`]). The trace's id is written as `trace_id` when it is not
/// the request's own id: one a proxy sent, or one the traces made.
#[derive(Clone, Copy, Debug, Default)]
pub struct RequestSpan;

impl<B> tower_http::trace::MakeSpan<B> for RequestSpan {
    fn make_span(&mut self, request: &axum::http::Request<B>) -> tracing::Span {
        let request_id = request
            .extensions()
            .get::<RequestId>()
            .and_then(|id| id.header_value().to_str().ok())
            .unwrap_or_default();
        let served = request.extensions().get::<trace::Served>();
        // Shown, not debugged: `request_id=…` in the text log rather than
        // `request_id="…"`, which is what a `&str` field would be written as.
        let span = tracing::info_span!(
            target: REQUEST_TARGET,
            parent: served.and_then(trace::Served::id),
            "request",
            method = %request.method(),
            uri = %redact_uri(request.uri()),
            version = ?request.version(),
            request_id = %request_id,
            trace_id = tracing::field::Empty,
        );
        if let Some(trace) = trace::trace_id(served, request.headers())
            && trace != request_id
        {
            span.record("trace_id", trace.as_str());
        }
        span
    }
}

/// The header a request's id travels in, in and out.
pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// The headers W3C Trace Context carries a trace in.
pub const TRACEPARENT: HeaderName = HeaderName::from_static("traceparent");
pub const TRACESTATE: HeaderName = HeaderName::from_static("tracestate");

/// How long an id this server takes from outside may be.
const LONGEST_ID: usize = 128;

/// Give every request on `router` an id: carried by the request span, so
/// that every line logged while the request is served names it, and sent
/// back in `x-request-id`, so that whoever met an error can say which one.
/// A trusted proxy's own id is kept, so that its log and this one name a
/// request alike; and its trace, sent in `traceparent`, is the one the
/// request joins — named by the request's id when the proxy sent no id of
/// its own. Anybody else's of either is dropped and an id is made here: a
/// stranger's would be whatever they chose, as long as a header may be, and
/// the same on every request they send. Applied outside the trace, which
/// opens the span the id is read into.
pub fn with_request_id(router: Router, trusted: Vec<IpNet>) -> Router {
    let trusted: Arc<[IpNet]> = trusted.into();
    router
        // Innermost of the three: it copies to the answer the id the layer
        // outside it put on the request.
        .layer(PropagateRequestIdLayer::new(REQUEST_ID))
        .layer(SetRequestIdLayer::new(REQUEST_ID, MakeId))
        .layer(axum::middleware::from_fn(
            move |request: Request, next: Next| {
                let trusted = Arc::clone(&trusted);
                async move { judge_request_id(&trusted, request, next).await }
            },
        ))
}

/// Take the `x-request-id` off a request unless it is a trusted proxy's,
/// given once, and in a form the log can hold: at most [`LONGEST_ID`] bytes,
/// of letters, digits and `-._:/`. `SetRequestIdLayer` keeps whatever header
/// it finds, so what it must not keep is taken away before it looks.
///
/// The trace a request belongs to is judged alike: `traceparent` and
/// `tracestate` are kept when a trusted proxy sent one `traceparent` that
/// reads as W3C Trace Context writes it, and taken off otherwise — a
/// stranger's would choose the trace this server writes its spans into, and
/// whether they are kept. A trusted proxy's trace with no id of its own
/// names the request: its id is the trace's.
async fn judge_request_id(trusted: &[IpNet], mut request: Request, next: Next) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0);
    let trusted = crate::auth::ip::is_trusted_peer(peer, trusted);
    let headers = request.headers_mut();
    let kept = trusted
        && headers.get_all(REQUEST_ID).iter().count() == 1
        && headers.get(REQUEST_ID).is_some_and(acceptable_id);
    if !kept {
        headers.remove(REQUEST_ID);
    }
    let trace = trusted
        .then(|| traced_by(headers).map(str::to_string))
        .flatten();
    match trace {
        Some(trace) => {
            if !headers.contains_key(REQUEST_ID)
                && let Ok(id) = HeaderValue::from_str(&trace)
            {
                headers.insert(REQUEST_ID, id);
            }
        }
        None => {
            headers.remove(TRACEPARENT);
            headers.remove(TRACESTATE);
        }
    }
    next.run(request).await
}

/// The trace a request's `traceparent` names, as W3C Trace Context writes
/// it — `00-<trace>-<parent>-<flags>`, in lower-case hexadecimal, neither id
/// all zeroes — when there is one such header: the trace's id.
pub(crate) fn traced_by(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(TRACEPARENT).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return None;
    };
    let text = value.to_str().ok()?;
    let hex = |part: &str, len: usize| {
        part.len() == len && part.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    };
    let mut parts = text.splitn(5, '-');
    let (version, trace, parent, flags) =
        (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let rest = parts.next();
    // Version 00 is four parts exactly; a later one may carry more after
    // them, which a reader of 00 passes over. `ff` is no version.
    let shaped = hex(version, 2)
        && version != "ff"
        && (rest.is_none() || version != "00")
        && hex(trace, 32)
        && hex(parent, 16)
        && hex(flags, 2)
        && trace.bytes().any(|b| b != b'0')
        && parent.bytes().any(|b| b != b'0');
    shaped.then_some(trace)
}

/// Whether an id from outside is one the log can hold.
fn acceptable_id(value: &HeaderValue) -> bool {
    let bytes = value.as_bytes();
    (1..=LONGEST_ID).contains(&bytes.len())
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':' | b'/'))
}

/// An id made here: sixteen random bytes in lower-case hexadecimal — the
/// form nginx's `$request_id` takes, and a W3C trace id's, so that the trace
/// a request starts is named by the request's own id. Random to the last
/// bit, as a trace id must be for a sampler to keep one in ten by it: a
/// UUID's fixed version and variant bits would have it keep none.
#[derive(Clone, Copy, Debug, Default)]
struct MakeId;

impl MakeRequestId for MakeId {
    fn make_request_id<B>(&mut self, _request: &axum::http::Request<B>) -> Option<RequestId> {
        HeaderValue::from_str(&new_id()).ok().map(RequestId::new)
    }
}

/// Sixteen random bytes, in lower-case hexadecimal, never all zeroes.
fn new_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() || bytes.iter().all(|b| *b == 0) {
        // Not a secret, only a name: the clock's bytes do when the random
        // source does not answer.
        bytes = uuid::Uuid::now_v7().into_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A request's path and query, the query's secrets masked.
pub fn redact_uri(uri: &axum::http::Uri) -> String {
    match uri.query() {
        Some(query) => format!("{}?{}", redact_path(uri.path()), mask_parameters(query)),
        None => redact_path(uri.path()).into_owned(),
    }
}

/// A request's path as a log may hold it: the API key an address of the
/// Plex provider carries, `/plex/<key>/…`, masked.
pub fn redact_path(path: &str) -> std::borrow::Cow<'_, str> {
    match crate::api::plex::key_in_path(path) {
        Some(key) => {
            let at = path.find(key).unwrap_or_default();
            std::borrow::Cow::Owned(format!("{}{MASK}{}", &path[..at], &path[at + key.len()..]))
        }
        None => std::borrow::Cow::Borrowed(path),
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
    fn the_key_a_plex_address_carries_is_masked_in_a_logged_path() {
        let key = "ams_example_example_example";
        assert_eq!(
            redact_path(&format!("/plex/{key}/library/metadata/movie-tmdb-603")),
            "/plex/***/library/metadata/movie-tmdb-603"
        );
        assert_eq!(redact_path(&format!("/plex/{key}")), "/plex/***");
        let uri: axum::http::Uri =
            format!("/plex/{key}/library/metadata/matches?X-Plex-Language=fr")
                .parse()
                .unwrap();
        assert_eq!(
            redact_uri(&uri),
            "/plex/***/library/metadata/matches?X-Plex-Language=fr"
        );

        // The provider's own paths, and every other, are left as they are.
        for path in [
            "/plex",
            "/plex/library/metadata/show-tvdb-81189",
            "/v1/tvdb/shows/en/81189",
        ] {
            assert_eq!(redact_path(path), path);
        }
    }

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

    #[test]
    fn an_id_from_outside_is_taken_in_one_shape_only() {
        let ok = |text: &str| acceptable_id(&HeaderValue::from_str(text).unwrap());
        assert!(ok("0192f5a2c1d07a3b9e4f2c6d8a1b3c5e"));
        assert!(ok("nginx-2026.10.18/req:42"));
        assert!(ok(&"x".repeat(LONGEST_ID)));
        assert!(!ok(""));
        assert!(!ok("two words"));
        assert!(!ok("a\"quote"));
        assert!(!ok(&"x".repeat(LONGEST_ID + 1)));
    }

    /// A router that answers with the id its handler saw, so that the
    /// header on the answer and what the handler read can be compared.
    fn ids(trusted: Vec<IpNet>) -> Router {
        with_request_id(
            Router::new().route(
                "/",
                axum::routing::get(|request: Request| async move {
                    request
                        .headers()
                        .get(REQUEST_ID)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("none")
                        .to_string()
                }),
            ),
            trusted,
        )
    }

    /// The id on the answer, and the one the handler saw.
    async fn asked(router: Router, request: Request) -> (String, String) {
        use tower::ServiceExt as _;
        let response = router.oneshot(request).await.unwrap();
        let answered = response
            .headers()
            .get(REQUEST_ID)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("none")
            .to_string();
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        (answered, String::from_utf8(body.to_vec()).unwrap())
    }

    fn bare() -> Request {
        Request::builder()
            .uri("/")
            .body(axum::body::Body::empty())
            .unwrap()
    }

    fn from(peer: &str, id: &str) -> Request {
        let mut request = Request::builder()
            .uri("/")
            .header(REQUEST_ID, id)
            .body(axum::body::Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo::<SocketAddr>(peer.parse().unwrap()));
        request
    }

    #[tokio::test]
    async fn every_request_is_given_an_id_and_answered_with_it() {
        let (answered, seen) = asked(ids(Vec::new()), bare()).await;
        assert_eq!(answered, seen);
        assert_eq!(answered.len(), 32, "{answered}");
        assert!(
            answered.bytes().all(|b| b.is_ascii_hexdigit()),
            "{answered}"
        );

        let (again, _) = asked(ids(Vec::new()), bare()).await;
        assert_ne!(again, answered, "two requests, one id");
    }

    #[tokio::test]
    async fn a_strangers_id_is_replaced_and_a_trusted_proxys_kept() {
        let proxy: IpNet = "10.0.0.0/8".parse().unwrap();

        // The proxy's, as it sent it.
        let (answered, seen) = asked(ids(vec![proxy]), from("10.0.0.2:4000", "proxy-7")).await;
        assert_eq!((answered.as_str(), seen.as_str()), ("proxy-7", "proxy-7"));

        // A stranger's: replaced on the way in, so the handler and the
        // answer carry the one made here.
        let (answered, seen) = asked(ids(vec![proxy]), from("203.0.113.5:4000", "theirs")).await;
        assert_eq!(answered, seen);
        assert_ne!(answered, "theirs");

        // The proxy's, but not one the log can hold.
        let (answered, seen) = asked(ids(vec![proxy]), from("10.0.0.2:4000", "two words")).await;
        assert_eq!(answered, seen);
        assert_ne!(answered, "two words");

        // A request with no peer at all — one a test makes — is nobody's
        // to trust either.
        let mut nobody = bare();
        nobody
            .headers_mut()
            .insert(REQUEST_ID, HeaderValue::from_static("mine"));
        let (answered, _) = asked(ids(vec![proxy]), nobody).await;
        assert_ne!(answered, "mine");
    }

    #[test]
    fn the_request_span_names_the_id_and_not_the_key() {
        // Written as the span opens, with its fields, into a buffer: what
        // the log shows beside every line of the request.
        use std::sync::Mutex;
        use tower_http::trace::MakeSpan as _;
        use tracing_subscriber::fmt::format::FmtSpan;

        #[derive(Clone, Default)]
        struct Buffer(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buffer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buffer {
            type Writer = Buffer;
            fn make_writer(&'a self) -> Buffer {
                self.clone()
            }
        }

        let written = Buffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(written.clone())
            .with_span_events(FmtSpan::NEW)
            .with_ansi(false)
            .finish();
        let mut request = axum::http::Request::builder()
            .uri("/api/v1/items?apikey=ams_secret")
            .body(())
            .unwrap();
        request
            .extensions_mut()
            .insert(RequestId::new(HeaderValue::from_static(
                "0192f5a2c1d07a3b9e4f2c6d8a1b3c5e",
            )));

        tracing::subscriber::with_default(subscriber, || {
            let span = RequestSpan.make_span(&request);
            let _entered = span.enter();
        });

        let log = String::from_utf8(written.0.lock().unwrap().clone()).unwrap();
        assert!(
            log.contains("request_id=0192f5a2c1d07a3b9e4f2c6d8a1b3c5e"),
            "{log}"
        );
        assert!(log.contains("apikey=***"), "{log}");
        assert!(!log.contains("ams_secret"), "{log}");
    }

    #[test]
    fn a_traceparent_is_read_as_w3c_trace_context_writes_it() {
        let read = |values: &[&str]| {
            let mut headers = HeaderMap::new();
            for value in values {
                headers.append(TRACEPARENT, HeaderValue::from_str(value).unwrap());
            }
            traced_by(&headers).map(str::to_string)
        };
        let trace = "4bf92f3577b34da6a3ce929d0e0e4736";
        assert_eq!(
            read(&[&format!("00-{trace}-00f067aa0ba902b7-01")]).as_deref(),
            Some(trace)
        );
        // A later version may carry more after the four parts.
        assert_eq!(
            read(&[&format!("01-{trace}-00f067aa0ba902b7-01-later")]).as_deref(),
            Some(trace)
        );
        for wrong in [
            format!("00-{trace}-00f067aa0ba902b7-01-more"),
            format!("ff-{trace}-00f067aa0ba902b7-01"),
            format!("00-{}-00f067aa0ba902b7-01", trace.to_uppercase()),
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01".to_string(),
            format!("00-{trace}-0000000000000000-01"),
            format!("00-{trace}-00f067aa0ba902b7"),
            "nonsense".to_string(),
        ] {
            assert_eq!(read(&[&wrong]), None, "{wrong}");
        }
        // Two are none.
        let one = format!("00-{trace}-00f067aa0ba902b7-01");
        assert_eq!(read(&[&one, &one]), None);
    }

    /// A trusted proxy's trace is kept and names the request when the proxy
    /// sent no id; a stranger's is taken off before anything reads it.
    #[tokio::test]
    async fn a_trusted_proxys_trace_names_the_request_and_a_strangers_is_dropped() {
        let proxy: IpNet = "10.0.0.0/8".parse().unwrap();
        let seen = |trusted: Vec<IpNet>| {
            with_request_id(
                Router::new().route(
                    "/",
                    axum::routing::get(|request: Request| async move {
                        request
                            .headers()
                            .get(TRACEPARENT)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("none")
                            .to_string()
                    }),
                ),
                trusted,
            )
        };
        let parent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let traced = |peer: &str| {
            let mut request = from(peer, "unused");
            request.headers_mut().remove(REQUEST_ID);
            request
                .headers_mut()
                .insert(TRACEPARENT, HeaderValue::from_static(parent));
            request
        };

        let (answered, kept) = asked(seen(vec![proxy]), traced("10.0.0.2:4000")).await;
        assert_eq!(answered, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(kept, parent);

        let (answered, kept) = asked(seen(vec![proxy]), traced("203.0.113.5:4000")).await;
        assert_ne!(answered, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(kept, "none");

        // The proxy's own id, when it sent one, stays the request's.
        let mut both = traced("10.0.0.2:4000");
        both.headers_mut()
            .insert(REQUEST_ID, HeaderValue::from_static("proxy-7"));
        let (answered, kept) = asked(seen(vec![proxy]), both).await;
        assert_eq!((answered.as_str(), kept.as_str()), ("proxy-7", parent));
    }

    #[test]
    fn an_id_made_here_is_random_to_the_last_bit() {
        let ids: Vec<String> = (0..64).map(|_| new_id()).collect();
        for id in &ids {
            assert_eq!(id.len(), 32, "{id}");
            assert!(
                id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
                "{id}"
            );
        }
        // No digit fixed by a UUID's version or variant: across 64 ids, the
        // thirteenth and seventeenth take more than one value.
        for at in [12, 16] {
            let values: std::collections::HashSet<u8> =
                ids.iter().map(|id| id.as_bytes()[at]).collect();
            assert!(values.len() > 1, "digit {at} is always {values:?}");
        }
    }

    /// A panic is an error line, masked as any other, before the default
    /// hook writes it to standard error.
    #[test]
    fn a_panic_is_written_as_an_error_line() {
        static INSTALLED: std::sync::Once = std::sync::Once::new();
        INSTALLED.call_once(panic_hook);

        let subscriber = tracing_subscriber::registry().with(journal::layer());
        let caught = tracing::subscriber::with_default(subscriber, || {
            std::panic::catch_unwind(|| {
                panic!("panic-hook-marker at https://x.example/?api_key=abcdef123456")
            })
        });
        assert!(caught.is_err());

        let lines = journal::read(&journal::Query {
            text: Some("panic-hook-marker".into()),
            ..journal::Query::default()
        });
        let line = lines.first().expect("the panic, as a line");
        assert_eq!(line.level, journal::Level::Error);
        assert_eq!(line.target, "panic");
        assert_eq!(line.message, "a thread panicked");
        let text = line.as_text();
        assert!(text.contains("api_key=***"), "{text}");
        assert!(!text.contains("abcdef123456"), "{text}");
        assert!(text.contains("location=src/telemetry.rs:"), "{text}");
    }

    #[test]
    fn a_request_id_is_no_secret_to_mask() {
        let line = "request_id=0192f5a2c1d07a3b9e4f2c6d8a1b3c5e answered in 12ms";
        assert_eq!(redact(line), line);
    }
}
