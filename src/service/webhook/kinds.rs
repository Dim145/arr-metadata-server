//! How each kind of destination is posted to: the address, the headers and
//! the body, made from what is told. Nothing here sends anything, so each
//! kind is tested as it would leave.
//!
//! - **standard** — [Standard Webhooks]: `{type, timestamp, data}`, signed
//!   with the destination's own secret in `webhook-id`, `webhook-timestamp`
//!   and `webhook-signature`, so the receiver can tell it came from here.
//! - **json** — what the single webhook address was posted until this
//!   version: the fields, and a one-line `content` and `text` beside them.
//! - **discord**, **slack** — that line alone, as each takes it: Discord's
//!   `content`, with mentions switched off, and Slack's `text`, escaped.
//! - **ntfy** — the line as the body, posted to the topic's address, its
//!   title, tags and priority in headers; a token, when the topic wants one.
//! - **gotify** — `{title, message, priority}` to the server's `/message`,
//!   with the application's token.
//! - **apprise** — `{title, body, type}` to an Apprise API's `/notify/…`.
//!
//! [Standard Webhooks]: https://github.com/standard-webhooks/standard-webhooks/blob/main/spec/standard-webhooks.md

use anyhow::Context as _;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use hmac::{Hmac, KeyInit as _, Mac as _};
use reqwest::header::{CONTENT_TYPE, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use utoipa::ToSchema;

/// What every message is signed with: the name the interface goes by.
const NAME: &str = "Cinémathèque";

/// What a signing secret begins with, as Standard Webhooks writes one.
pub const SECRET_PREFIX: &str = "whsec_";

/// Bytes in a signing secret made here: within the 24 to 64 the
/// specification asks for.
const SECRET_BYTES: usize = 32;

/// The most of the line Discord takes: 2000 characters, with room to spare.
const DISCORD_CHARS: usize = 1990;

/// The kinds of destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Standard,
    Json,
    Discord,
    Slack,
    Ntfy,
    Gotify,
    Apprise,
}

/// What a kind keeps as its secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Secret {
    /// Nothing: whatever lets it post is in its address.
    None,
    /// A signing secret, made here and shown once.
    Signing,
    /// A token its service gave, which only some topics or servers ask for.
    Token { required: bool },
}

impl Kind {
    /// Every kind, in the order the interface offers them.
    pub const ALL: &'static [Kind] = &[
        Self::Standard,
        Self::Discord,
        Self::Slack,
        Self::Ntfy,
        Self::Gotify,
        Self::Apprise,
        Self::Json,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Json => "json",
            Self::Discord => "discord",
            Self::Slack => "slack",
            Self::Ntfy => "ntfy",
            Self::Gotify => "gotify",
            Self::Apprise => "apprise",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.as_str() == s)
    }

    pub const fn secret(self) -> Secret {
        match self {
            Self::Standard => Secret::Signing,
            Self::Ntfy => Secret::Token { required: false },
            Self::Gotify => Secret::Token { required: true },
            Self::Json | Self::Discord | Self::Slack | Self::Apprise => Secret::None,
        }
    }
}

impl Kind {
    /// Whether people read what it is posted — a chat, a phone — rather
    /// than a program: they are told of failed refreshes together, by
    /// default (see `digest`).
    pub fn read_by_people(self) -> bool {
        matches!(
            self,
            Self::Discord | Self::Slack | Self::Ntfy | Self::Gotify | Self::Apprise
        )
    }
}

/// One thing told, as a delivery keeps it: the event, who did it, to what,
/// what is worth saying of it, and when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = WebhookMessage)]
pub struct Told {
    pub event: String,
    pub actor: Option<String>,
    pub target: Option<String>,
    pub detail: Option<String>,
    /// When it happened, in RFC 3339.
    pub at: String,
    /// How many things it tells of together — failed refreshes grouped for
    /// a destination — when there are more than one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

/// A post, ready to send.
#[derive(Debug)]
pub struct Post {
    pub url: String,
    pub headers: Vec<(HeaderName, HeaderValue)>,
    pub body: Vec<u8>,
}

/// How much an event matters to somebody reading it on a phone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Weight {
    Info,
    Warning,
    Failure,
}

/// A work that would not refresh and a medium given up on are failures; a
/// sign-in refused is worth a look; the rest is news.
fn weight(event: &str) -> Weight {
    use super::Fact;
    if event == Fact::RefreshFailed.as_str() || event == Fact::MediaFailed.as_str() {
        Weight::Failure
    } else if event == crate::db::repo::audit::Action::SignInFailed.as_str() {
        Weight::Warning
    } else {
        Weight::Info
    }
}

/// The post for a destination of this kind at this address, for what is
/// told under this message id, signed — for a standard one — at `now`, in
/// seconds since the epoch. Refused, with the reason, when it cannot be
/// made: a standard destination without its secret, a token that cannot
/// travel in a header.
pub fn post(
    kind: Kind,
    url: &str,
    secret: Option<&str>,
    message_id: &str,
    told: &Told,
    now: i64,
) -> Result<Post, String> {
    let json_type = (CONTENT_TYPE, HeaderValue::from_static("application/json"));
    match kind {
        Kind::Standard => {
            let secret = secret.ok_or("this destination has no signing secret: make one")?;
            let body = to_vec(&json!({
                "type": told.event,
                "timestamp": told.at,
                "data": {
                    "actor": told.actor,
                    "target": told.target,
                    "detail": told.detail,
                },
            }));
            let signature = sign(secret, message_id, now, &body)
                .ok_or("this destination's signing secret is not one: make another")?;
            Ok(Post {
                url: url.to_string(),
                headers: vec![
                    json_type,
                    (HeaderName::from_static("webhook-id"), header(message_id)?),
                    (
                        HeaderName::from_static("webhook-timestamp"),
                        header(&now.to_string())?,
                    ),
                    (
                        HeaderName::from_static("webhook-signature"),
                        header(&signature)?,
                    ),
                ],
                body,
            })
        }
        Kind::Json => Ok(Post {
            url: url.to_string(),
            headers: vec![json_type],
            body: to_vec(&legacy(told)),
        }),
        Kind::Discord => Ok(Post {
            url: url.to_string(),
            headers: vec![json_type],
            body: to_vec(&json!({
                "content": crate::providers::clip(&line(told), DISCORD_CHARS),
                // A title or a name told here is somebody else's text: an
                // `@everyone` in it is not a ping.
                "allowed_mentions": { "parse": [] },
            })),
        }),
        Kind::Slack => Ok(Post {
            url: url.to_string(),
            headers: vec![json_type],
            body: to_vec(&json!({ "text": slack_escaped(&line(told)) })),
        }),
        Kind::Ntfy => {
            let mut headers = vec![
                (
                    CONTENT_TYPE,
                    HeaderValue::from_static("text/plain; charset=utf-8"),
                ),
                (HeaderName::from_static("title"), header_text(&title(told))),
                (HeaderName::from_static("tags"), header_text(&tags(told))),
                (
                    HeaderName::from_static("priority"),
                    HeaderValue::from_static(match weight(&told.event) {
                        Weight::Info => "3",
                        Weight::Warning | Weight::Failure => "4",
                    }),
                ),
            ];
            if let Some(token) = secret {
                headers.push((
                    reqwest::header::AUTHORIZATION,
                    header(&format!("Bearer {token}"))?,
                ));
            }
            Ok(Post {
                url: url.to_string(),
                headers,
                body: message(told).into_bytes(),
            })
        }
        Kind::Gotify => {
            let token = secret.ok_or("a Gotify destination needs its application's token")?;
            Ok(Post {
                url: gotify_url(url)?,
                headers: vec![
                    json_type,
                    (HeaderName::from_static("x-gotify-key"), header(token)?),
                ],
                body: to_vec(&json!({
                    "title": title(told),
                    "message": message(told),
                    "priority": match weight(&told.event) {
                        Weight::Info => 5,
                        Weight::Warning | Weight::Failure => 8,
                    },
                })),
            })
        }
        Kind::Apprise => Ok(Post {
            url: url.to_string(),
            headers: vec![json_type],
            body: to_vec(&json!({
                "title": title(told),
                "body": message(told),
                "type": match weight(&told.event) {
                    Weight::Info => "info",
                    Weight::Warning => "warning",
                    Weight::Failure => "failure",
                },
            })),
        }),
    }
}

/// A value as JSON. A `Value` always serialises.
fn to_vec(value: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_default()
}

/// A header's value, or why it cannot be one: only visible characters
/// travel in a header.
fn header(value: &str) -> Result<HeaderValue, String> {
    HeaderValue::from_str(value)
        .map_err(|_| "a value of this destination cannot travel in a header".to_string())
}

/// Text for a header that may not be ASCII: as it is when it is, and
/// otherwise encoded as RFC 2047 says, which is how ntfy reads a title in
/// another alphabet — `Cinémathèque` is not ASCII.
fn header_text(text: &str) -> HeaderValue {
    let text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if text.is_ascii()
        && let Ok(value) = HeaderValue::from_str(&text)
    {
        return value;
    }
    let encoded = format!("=?UTF-8?B?{}?=", STANDARD.encode(text.as_bytes()));
    HeaderValue::from_str(&encoded).unwrap_or_else(|_| HeaderValue::from_static(""))
}

/// The line a chat shows: `Cinémathèque · item.imported · series 01… · by admin`.
pub fn line(told: &Told) -> String {
    let line = [
        Some(told.event.clone()),
        told.detail.clone(),
        told.actor.as_ref().map(|a| format!("by {a}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    format!("{NAME} · {line}")
}

/// A notification's title: what it is about.
fn title(told: &Told) -> String {
    match told.count {
        Some(count) if count > 1 => format!("{NAME} · {} × {count}", told.event),
        _ => format!("{NAME} · {}", told.event),
    }
}

/// A notification's body: to what, what, by whom. The target is left out
/// where the detail already names it, as a fact's always does.
fn message(told: &Told) -> String {
    let target = told
        .target
        .as_deref()
        .filter(|target| !told.detail.as_deref().is_some_and(|d| d.contains(target)));
    let parts: Vec<String> = [
        target.map(str::to_string),
        told.detail.clone(),
        told.actor.as_ref().map(|a| format!("by {a}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        told.event.clone()
    } else {
        parts.join(" · ")
    }
}

/// ntfy's tags: the event's name, and — for what went wrong — the one ntfy
/// shows as a warning sign.
fn tags(told: &Told) -> String {
    match weight(&told.event) {
        Weight::Info => told.event.clone(),
        Weight::Warning | Weight::Failure => format!("warning,{}", told.event),
    }
}

/// What the single webhook address was posted until this version, field for
/// field: what a `json` destination still is.
pub fn legacy(told: &Told) -> serde_json::Value {
    let line = line(told);
    json!({
        "event": told.event,
        "actor": told.actor,
        "target": told.target,
        "detail": told.detail,
        "at": told.at,
        "content": line,
        "text": line,
    })
}

/// Text for Slack, whose `&`, `<` and `>` start a mention or a link: a
/// title with `<!channel>` in it is shown, not obeyed.
fn slack_escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Where a Gotify server takes a message: its `/message`, under whatever
/// path it is served at, unless the address already ends there.
fn gotify_url(base: &str) -> Result<String, String> {
    let mut url = url::Url::parse(base).map_err(|_| "the address is not one".to_string())?;
    let path = url.path().trim_end_matches('/').to_string();
    if !path.ends_with("/message") {
        url.set_path(&format!("{path}/message"));
    }
    Ok(url.to_string())
}

/// Standard Webhooks' signature: `v1,` and the base64 of the HMAC-SHA256,
/// under the secret's bytes, of `{id}.{timestamp}.{body}`. None when the
/// secret is not one.
pub fn sign(secret: &str, message_id: &str, timestamp: i64, body: &[u8]) -> Option<String> {
    let key = STANDARD
        .decode(secret.strip_prefix(SECRET_PREFIX).unwrap_or(secret))
        .ok()
        .filter(|key| !key.is_empty())?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).ok()?;
    mac.update(message_id.as_bytes());
    mac.update(b".");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    Some(format!(
        "v1,{}",
        STANDARD.encode(mac.finalize().into_bytes())
    ))
}

/// A new signing secret: thirty-two random bytes, in base64, after `whsec_`.
pub fn new_secret() -> anyhow::Result<String> {
    let mut bytes = [0u8; SECRET_BYTES];
    getrandom::fill(&mut bytes).context("failed to read from the system CSPRNG")?;
    Ok(format!("{SECRET_PREFIX}{}", STANDARD.encode(bytes)))
}

/// A new message id: Standard Webhooks' `msg_` and a time-ordered id, with
/// no `.` that would run into the signed content's own.
pub fn message_id() -> String {
    format!("msg_{}", uuid::Uuid::now_v7().simple())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn told(event: &str, target: Option<&str>, detail: Option<&str>) -> Told {
        Told {
            event: event.into(),
            actor: Some("admin".into()),
            target: target.map(String::from),
            detail: detail.map(String::from),
            at: "2026-10-11T08:00:00Z".into(),
            count: None,
        }
    }

    fn header_of<'a>(post: &'a Post, name: &str) -> Option<&'a str> {
        post.headers
            .iter()
            .find(|(n, _)| n.as_str() == name)
            .and_then(|(_, v)| v.to_str().ok())
    }

    fn body(post: &Post) -> serde_json::Value {
        serde_json::from_slice(&post.body).expect("a JSON body")
    }

    /// The example the Standard Webhooks reference library signs, from the
    /// specification's own repository.
    #[test]
    fn the_signature_is_the_specification_s() {
        assert_eq!(
            sign(
                "whsec_C2FVsBQIhrscChlQIMV+b5sSYspob7oD",
                "msg_27UH4WbU6Z5A5EzD8u03UvzRbpk",
                1_649_367_553,
                br#"{"email":"test@example.com","username":"test_user"}"#,
            )
            .as_deref(),
            Some("v1,tZ1I4/hDygAJgO5TYxiSd6Sd0kDW6hPenDe+bTa3Kkw=")
        );
        // Not a secret at all: nothing is signed with it.
        assert_eq!(sign("whsec_", "msg_1", 1, b"{}"), None);
        assert_eq!(sign("whsec_not base64!", "msg_1", 1, b"{}"), None);
    }

    #[test]
    fn a_secret_made_here_is_one_the_specification_takes() {
        let secret = new_secret().expect("a secret");
        let key = STANDARD
            .decode(secret.strip_prefix(SECRET_PREFIX).expect("whsec_"))
            .expect("base64");
        assert!((24..=64).contains(&key.len()), "{} bytes", key.len());
        assert_ne!(secret, new_secret().expect("another"));
        assert!(!message_id().contains('.'));
        assert!(message_id().starts_with("msg_"));
    }

    #[test]
    fn a_standard_post_is_signed_over_what_it_carries() {
        let secret = "whsec_C2FVsBQIhrscChlQIMV+b5sSYspob7oD";
        let told = told(
            "item.imported",
            Some("Blade Runner 2099"),
            Some("series 01a0"),
        );
        let post = post(
            Kind::Standard,
            "https://hooks.example.com/in",
            Some(secret),
            "msg_1",
            &told,
            1_760_000_000,
        )
        .expect("a post");

        assert_eq!(header_of(&post, "webhook-id"), Some("msg_1"));
        assert_eq!(header_of(&post, "webhook-timestamp"), Some("1760000000"));
        assert_eq!(header_of(&post, "content-type"), Some("application/json"));
        // What the receiver checks: the signature of exactly these bytes.
        assert_eq!(
            header_of(&post, "webhook-signature"),
            sign(secret, "msg_1", 1_760_000_000, &post.body).as_deref()
        );
        assert_eq!(
            body(&post),
            json!({
                "type": "item.imported",
                "timestamp": "2026-10-11T08:00:00Z",
                "data": {"actor": "admin", "target": "Blade Runner 2099", "detail": "series 01a0"},
            })
        );
        // The fields in the order the specification gives them.
        assert!(
            String::from_utf8_lossy(&post.body)
                .starts_with(r#"{"type":"item.imported","timestamp""#)
        );

        // Without its secret, nothing is sent rather than something unsigned.
        assert!(
            super::post(
                Kind::Standard,
                "https://h.example/x",
                None,
                "msg_1",
                &told,
                1
            )
            .is_err()
        );
    }

    #[test]
    fn a_json_post_is_the_single_address_s_shape() {
        let post = post(
            Kind::Json,
            "https://hooks.example.com/x",
            None,
            "msg_1",
            &told(
                "item.imported",
                Some("Blade Runner 2099"),
                Some("series 01a0"),
            ),
            0,
        )
        .expect("a post");
        let body = body(&post);
        let mut fields: Vec<&str> = body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            [
                "actor", "at", "content", "detail", "event", "target", "text"
            ]
        );
        assert_eq!(body["at"], "2026-10-11T08:00:00Z");
        assert_eq!(
            body["content"],
            "Cinémathèque · item.imported · series 01a0 · by admin"
        );
        assert_eq!(body["content"], body["text"]);
    }

    #[test]
    fn discord_and_slack_are_given_the_line_and_obey_none_of_it() {
        let told = told(
            "item.imported",
            Some("x"),
            Some("@everyone <!channel> & friends"),
        );
        let discord = post(
            Kind::Discord,
            "https://discord.com/api/webhooks/1/t",
            None,
            "m",
            &told,
            0,
        )
        .expect("a post");
        assert_eq!(
            body(&discord),
            json!({
                "content": "Cinémathèque · item.imported · @everyone <!channel> & friends · by admin",
                "allowed_mentions": {"parse": []},
            })
        );

        let slack = post(
            Kind::Slack,
            "https://hooks.slack.com/services/T/B/x",
            None,
            "m",
            &told,
            0,
        )
        .expect("a post");
        assert_eq!(
            body(&slack),
            json!({
                "text": "Cinémathèque · item.imported · @everyone &lt;!channel&gt; &amp; friends · by admin",
            })
        );

        // A line longer than Discord takes is cut, not refused.
        let long = Told {
            detail: Some("x".repeat(5000)),
            ..told
        };
        let discord = post(
            Kind::Discord,
            "https://discord.com/api/webhooks/1/t",
            None,
            "m",
            &long,
            0,
        )
        .expect("a post");
        let content = body(&discord)["content"].as_str().unwrap().to_string();
        assert!(content.chars().count() <= 2000);
    }

    #[test]
    fn ntfy_is_posted_the_line_with_its_title_tags_and_priority_in_headers() {
        let failed = told(
            "refresh.failed",
            Some("01a0"),
            Some("“Blade Runner 2099” (series 01a0): TMDB could not be reached"),
        );
        let post = post(
            Kind::Ntfy,
            "https://ntfy.sh/ams-alerts",
            Some("tk_secret"),
            "m",
            &failed,
            0,
        )
        .expect("a post");
        assert_eq!(post.url, "https://ntfy.sh/ams-alerts");
        assert_eq!(header_of(&post, "authorization"), Some("Bearer tk_secret"));
        assert_eq!(header_of(&post, "priority"), Some("4"));
        assert_eq!(header_of(&post, "tags"), Some("warning,refresh.failed"));
        // The title is not ASCII, so it travels as RFC 2047 says.
        let title = header_of(&post, "title").expect("a title");
        let encoded = title
            .strip_prefix("=?UTF-8?B?")
            .and_then(|t| t.strip_suffix("?="))
            .expect("an encoded word");
        assert_eq!(
            String::from_utf8(STANDARD.decode(encoded).unwrap()).unwrap(),
            "Cinémathèque · refresh.failed"
        );
        // The id is in the detail already, and said once.
        assert_eq!(
            String::from_utf8(post.body).unwrap(),
            "“Blade Runner 2099” (series 01a0): TMDB could not be reached · by admin"
        );

        // A topic without a token sends none.
        let open = super::post(
            Kind::Ntfy,
            "https://ntfy.sh/ams-alerts",
            None,
            "m",
            &told("item.created", Some("01a0"), Some("movie \"Heat\"")),
            0,
        )
        .expect("a post");
        assert_eq!(header_of(&open, "authorization"), None);
        assert_eq!(header_of(&open, "priority"), Some("3"));
        assert_eq!(header_of(&open, "tags"), Some("item.created"));
        assert_eq!(
            String::from_utf8(open.body).unwrap(),
            "01a0 · movie \"Heat\" · by admin"
        );
    }

    #[test]
    fn gotify_is_posted_to_its_message_route_with_its_token() {
        let told = told("auth.sign_in_failed", None, None);
        for (base, at) in [
            ("https://push.example.de", "https://push.example.de/message"),
            (
                "https://push.example.de/",
                "https://push.example.de/message",
            ),
            (
                "https://example.de/gotify/",
                "https://example.de/gotify/message",
            ),
            (
                "https://push.example.de/message",
                "https://push.example.de/message",
            ),
        ] {
            let post = post(Kind::Gotify, base, Some("AbCdEf"), "m", &told, 0).expect("a post");
            assert_eq!(post.url, at, "{base}");
            assert_eq!(header_of(&post, "x-gotify-key"), Some("AbCdEf"));
            assert_eq!(
                body(&post),
                json!({
                    "title": "Cinémathèque · auth.sign_in_failed",
                    "message": "by admin",
                    "priority": 8,
                })
            );
        }
        assert!(super::post(Kind::Gotify, "https://push.example.de", None, "m", &told, 0).is_err());
    }

    #[test]
    fn apprise_is_posted_a_title_a_body_and_a_type() {
        let post = post(
            Kind::Apprise,
            "http://apprise.lan:8000/notify/ams",
            None,
            "m",
            &told("media.failed", Some("https://img.example/a.jpg"), Some("the poster of “Heat” (movie 1) was given up on: 404 — https://img.example/a.jpg")),
            0,
        )
        .expect("a post");
        assert_eq!(post.url, "http://apprise.lan:8000/notify/ams");
        assert_eq!(
            body(&post),
            json!({
                "title": "Cinémathèque · media.failed",
                "body": "the poster of “Heat” (movie 1) was given up on: 404 — https://img.example/a.jpg · by admin",
                "type": "failure",
            })
        );
        let news = super::post(
            Kind::Apprise,
            "http://apprise.lan:8000/notify/ams",
            None,
            "m",
            &told("update.available", Some("0.8.0"), Some("0.8.0 is out")),
            0,
        )
        .expect("a post");
        assert_eq!(body(&news)["type"], "info");
    }

    #[test]
    fn every_kind_reads_back_as_itself() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.as_str()), Some(*kind));
        }
        assert_eq!(Kind::parse("smtp"), None);
        assert_eq!(Kind::Standard.secret(), Secret::Signing);
        assert_eq!(Kind::Gotify.secret(), Secret::Token { required: true });
        assert_eq!(Kind::Discord.secret(), Secret::None);
    }
}
