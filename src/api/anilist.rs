//! AniList compatibility: `graphql.anilist.co`, on the clients' door.
//!
//! AniList has one address and one method — a GraphQL query posted to it —
//! and the clients that use it have that address compiled in: Yamtrack's
//! imports, the anime trackers. Resolved to this server, their queries are
//! handed on to AniList and the answers handed back, with the fields a person
//! locked here written into every entry they carry — and into an entry that
//! is one season or cour of a work, as the anime identifier list places it
//! ([`service::cour`]), what was corrected of when that one airs.
//!
//! AniList has no API key. A client that reads somebody's private lists
//! carries that person's own token, which travels with the request as it
//! came: it is theirs, for AniList, and nothing of this server's. A key issued
//! here, presented as a bearer token or a header, is this server's business
//! and never leaves. An answer to a query that carried a token, to a
//! mutation, or about somebody — their lists, their profile, what they did —
//! is never kept, never shared with another caller, and never stands in for
//! AniList: it is one person's.
//!
//! Any other answer is kept an hour, half a day for the genre and tag lists
//! asked alone; callers asking the same at once share one exchange; and when
//! AniList fails — no answer, a 429, a 5xx, or the 403 of its API switched
//! off — what it answered last stands in, for a day at the most
//! ([`relay::answer`]). Nothing of it is written to the database. Every
//! request goes through AniList's door here, the one the server's own
//! fetches take: the same few at once, and the same pause after a 429,
//! answered with a 429 that says when to come back when nothing kept can
//! stand in.

use std::collections::HashMap;

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::any,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use utoipa_axum::router::OpenApiRouter;

use crate::{
    api::relay,
    auth::secrets,
    cache,
    db::repo,
    domain::{CoverType, ExternalSource, MediaItem, MediaKind},
    error::{AppError, AppResult},
    service::{
        self,
        cour::{Cour, Next, Status},
    },
    state::AppState,
};

/// The name AniList's clients call it by.
pub const HOSTS: &[&str] = &["graphql.anilist.co"];

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "AniList compatibility";

/// The most a query may weigh. AniList's own ceiling on a query's depth
/// keeps the real ones to a few kilobytes.
const REQUEST_LIMIT: usize = 1024 * 1024;

/// The most an answer may weigh: a page of fifty entries with their
/// relations runs to a few hundred kilobytes.
const ANSWER_LIMIT: u64 = 16 * 1024 * 1024;

/// AniList's genres and its tags: a query that asks for them and nothing
/// else is kept half a day ([`cache::ANILIST_COLLECTIONS`]), where any other
/// is kept an hour.
const COLLECTIONS: &[&str] = &["GenreCollection", "MediaTagCollection"];

/// What a relayed request keeps of the caller's headers: what describes it.
const ASKED_WITH: &[&str] = &["user-agent", "accept", "accept-language", "content-type"];

/// How many entries of one answer are looked up here, in the order the
/// answer lists them. A page is fifty at most; a user's whole list, in one
/// answer, is not patched beyond this.
const MOST_PATCHED: usize = 200;

/// AniList's formats for an anime.
const ANIME_FORMATS: &[&str] = &["TV", "TV_SHORT", "MOVIE", "SPECIAL", "OVA", "ONA", "MUSIC"];

/// AniList's formats for a manga, a light novel or a one-shot.
const MANGA_FORMATS: &[&str] = &["MANGA", "NOVEL", "ONE_SHOT"];

/// What only an anime has a value for: a manga's are null.
const ANIME_ONLY: &[&str] = &[
    "episodes",
    "season",
    "seasonYear",
    "duration",
    "nextAiringEpisode",
];

/// What only a manga has a value for: an anime's are null.
const MANGA_ONLY: &[&str] = &["chapters", "volumes"];

/// The names in a document that make its answer one person's: a user, what
/// they keep, did, wrote or were told, or what is picked by them. Never
/// kept, shared with another caller or served stale, even when nobody signed
/// the query — a public list is still somebody's, read for its latest state.
///
/// Read off AniList's schema, by introspection, on 1 November 2026: every
/// field a query can reach whose value is a user; every root and `Page`
/// field whose entries are a person's doing; and the two filters by a user
/// that roots of their own take. The other filters by one — `userName`,
/// `userId_in`, `replyUserId`, `isFollowing` — only the roots named here
/// take.
const PERSONAL: &[&str] = &[
    // A user.
    "Viewer",
    "User",
    "user",
    "users",
    "Follower",
    "Following",
    "MutualFollower",
    "followers",
    "following",
    "mutualFollowers",
    "Like",
    "likes",
    "messenger",
    "recipient",
    "replyUser",
    "submitter",
    // What they keep, did, wrote or were told.
    "MediaListCollection",
    "MediaList",
    "mediaList",
    "mediaListEntry",
    "mediaListOptions",
    "favourites",
    "Activity",
    "activities",
    "ActivityReply",
    "activityReplies",
    "Notification",
    "notifications",
    "Thread",
    "threads",
    "ThreadComment",
    "threadComments",
    "Review",
    "reviews",
    "AniChartUser",
    // Picked by somebody: a user's, or the viewer's own list.
    "userId",
    "onList",
];

pub fn router() -> OpenApiRouter<AppState> {
    // Registered by hand: AniList's one path is the root, which `routes!`
    // documents as the root of this server. `ApiDoc` names it.
    OpenApiRouter::new().route("/", any(relay))
}

/// Whether a request was addressed to `graphql.anilist.co`.
pub fn is_anilist_host(headers: &HeaderMap, uri: &Uri) -> bool {
    relay::addressed_to(headers, uri, HOSTS)
}

fn loop_refused() -> AppError {
    // A warning, not an error: anybody past the guard can send the header.
    tracing::warn!(
        "graphql.anilist.co was asked by an instance of this server: it resolves back here; set \
         AMS_ANILIST_UPSTREAM to AniList's real address"
    );
    AppError::LoopDetected
}

/// Relay a GraphQL query to AniList, with this server's locks written into
/// the entries of the answer.
///
/// Only reached by the name `graphql.anilist.co`, on the clients' door: the
/// interface's door serves its own page at this path.
#[utoipa::path(
    post, path = "/", tag = TAG,
    operation_id = "anilist_relay",
    responses(
        (status = 200, description = "AniList's answer, with the fields locked here written into each entry that is a work of this catalogue. Reached by the name graphql.anilist.co on the clients' door; a GET with the query in the address is relayed too. When AniList fails — no answer, 429, 5xx, or 403 — its last answer to the same query stands in, with Age, a day old at the most; never for a query about somebody"),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 429, description = "AniList's rate limit, handed back with its headers; or AniList's door here, shut after one, with the seconds left in Retry-After, when nothing kept stands in"),
        (status = 503, description = "The relay is off"),
    ),
    security(("apiKey" = []), ("bearer" = [])),
)]
pub async fn relay(State(state): State<AppState>, request: Request) -> AppResult<Response> {
    if relay::looped(request.headers()) {
        return Err(loop_refused());
    }
    if !state.config.anilist.passthrough {
        return Err(AppError::ProviderNotConfigured);
    }
    if !matches!(*request.method(), Method::POST | Method::GET) {
        return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }

    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, REQUEST_LIMIT)
        .await
        .map_err(|_| AppError::PayloadTooLarge("a GraphQL query is a few kilobytes".into()))?;

    // The caller's own AniList token travels; a key of this server does not
    // — as the token, nor anywhere else in the header that would carry it.
    let theirs = relay::bearer(&parts.headers)
        .filter(|token| !secrets::is_issued_here(token))
        .filter(|_| !relay::authorization_carries_key_issued_here(&parts.headers))
        .and_then(|_| parts.headers.get(header::AUTHORIZATION).cloned());

    // What AniList answered, kept for a query anybody may ask: one asked in
    // somebody's name, one that changes something, or one about somebody,
    // is nobody else's — asked alone, and never stood in for.
    let document = query_text(&parts.uri, &body);
    let anybodys = theirs.is_none()
        && document
            .as_deref()
            .is_some_and(|query| !has_mutation(query) && !is_personal(query));
    let ask = relay::Ask {
        service: relay::Service::Anilist,
        key: anybodys.then(|| relay_key(&state, &parts.method, &parts.uri, &body)),
        path: if document.as_deref().is_some_and(asks_collections_alone) {
            cache::ANILIST_COLLECTIONS
        } else {
            cache::ANILIST_QUERY
        }
        .to_string(),
        // AniList's terms want nothing of it stored.
        reference: None,
    };
    let fetched = relay::answer(&state, ask, {
        let (state, parts) = (state.clone(), parts.clone());
        async move { send(&state, &parts, body, theirs).await }
    })
    .await?;
    let relay::Fetched {
        status,
        headers,
        body: bytes,
        ..
    } = fetched;

    if relay::is_json(&headers)
        && let Ok(mut answer) = serde_json::from_slice::<Value>(&bytes)
        && patch(&state, &mut answer).await?
    {
        let mut headers = headers;
        relay::without_validators(&mut headers);
        return Ok((status, headers, Json(answer)).into_response());
    }

    Ok((status, headers, Body::from(bytes)).into_response())
}

/// Ask AniList, and read what it answered — a refusal for its rate limit
/// included, with the headers that say when to ask again.
///
/// Through AniList's door here, the one this server's own fetches take
/// ([`crate::providers::anilist::AnilistClient::gate`]): its 429 shuts it for
/// both, and a request that finds it shut is not sent — a 429 that says when
/// it opens ([`relay::unsent`]). Never through their spacing: a client is
/// waiting.
async fn send(
    state: &AppState,
    parts: &axum::http::request::Parts,
    body: Bytes,
    theirs: Option<HeaderValue>,
) -> AppResult<relay::Fetched> {
    // AniList has one address; the query travels in the body, or in the
    // address. Whatever path the client spelt, that is where it goes, with
    // the query it carried.
    let base = state.config.anilist.upstream.trim_end_matches('/');
    let target = match relay::query_without(&parts.uri, relay::OUR_PARAMS) {
        Some(query) => format!("{base}?{query}"),
        None => base.to_string(),
    };

    let mut outbound = relay::CLIENT
        .request(parts.method.clone(), &target)
        .headers(relay::asked_with(&parts.headers, ASKED_WITH))
        // This name is one this server answers on: a resolver sending it
        // back here would otherwise have it call itself.
        .header(crate::providers::radarr::LOOP_HEADER, &state.instance)
        .timeout(std::time::Duration::from_secs(60));
    if let Some(token) = theirs {
        outbound = outbound.header(header::AUTHORIZATION, token);
    }
    if !body.is_empty() {
        outbound = outbound.body(body);
    }

    // Its place at the door is held until the answer is read.
    let (response, _place) = state
        .anilist
        .gate()
        .send_now(outbound)
        .await
        .map_err(|e| relay::unsent(relay::Service::Anilist, e))?;

    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if status == StatusCode::LOOP_DETECTED {
        tracing::warn!(
            "graphql.anilist.co resolves to this server; set AMS_ANILIST_UPSTREAM to AniList's \
             real address"
        );
    }
    let headers = relay::kept(response.headers());
    let bytes = crate::providers::read_body(response, ANSWER_LIMIT)
        .await
        .map_err(|e| AppError::UpstreamUnavailable(e.context("AniList's answer")))?;
    Ok(relay::Fetched::new(status, headers, Bytes::from(bytes)))
}

/// The GraphQL document asked: the body's `query`, or a GET's `query`
/// parameter.
fn query_text(uri: &Uri, body: &[u8]) -> Option<String> {
    if !body.is_empty() {
        let parsed: Value = serde_json::from_slice(body).ok()?;
        return parsed.get("query")?.as_str().map(str::to_string);
    }
    url::form_urlencoded::parse(uri.query()?.as_bytes())
        .find(|(name, _)| name == "query")
        .map(|(_, value)| value.into_owned())
}

/// What the relay reads of a GraphQL document, piece by piece: its names,
/// its brackets, and the marks that change what a name stands for. Strings
/// and comments are left out, and everything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token<'a> {
    Name(&'a str),
    /// `{`, `(` or `[`.
    Open(u8),
    Close,
    /// After an alias.
    Colon,
    /// `...`: a fragment, spread or inline.
    Spread,
    /// Before a directive.
    At,
}

/// The pieces of a GraphQL document: see [`Token`].
fn tokens(document: &str) -> Vec<Token<'_>> {
    let bytes = document.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' => {
                if bytes[i..].starts_with(b"\"\"\"") {
                    i += 3;
                    while i + 3 <= bytes.len() && &bytes[i..i + 3] != b"\"\"\"" {
                        i += 1;
                    }
                    i = (i + 3).min(bytes.len());
                } else {
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'"' {
                        if bytes[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                    i += 1;
                }
            }
            open @ (b'{' | b'(' | b'[') => {
                found.push(Token::Open(open));
                i += 1;
            }
            b'}' | b')' | b']' => {
                found.push(Token::Close);
                i += 1;
            }
            b':' => {
                found.push(Token::Colon);
                i += 1;
            }
            b'@' => {
                found.push(Token::At);
                i += 1;
            }
            b'.' if bytes[i..].starts_with(b"...") => {
                found.push(Token::Spread);
                i += 3;
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                found.push(Token::Name(&document[start..i]));
            }
            _ => i += 1,
        }
    }
    found
}

/// The names in a GraphQL document, each with how deep it stands — zero
/// outside every selection, argument list and variable list — leaving out
/// strings and comments.
fn names(document: &str) -> Vec<(i32, &str)> {
    let mut depth = 0i32;
    let mut found = Vec::new();
    for token in tokens(document) {
        match token {
            Token::Open(_) => depth += 1,
            Token::Close => depth -= 1,
            Token::Name(name) => found.push((depth, name)),
            Token::Colon | Token::Spread | Token::At => {}
        }
    }
    found
}

/// The fields every operation of a document asks for at its root, read
/// through their aliases — or nothing, when the document asks in a way read
/// no further here: a fragment, or a directive, among them, or a fragment
/// defined beside them.
fn root_fields(document: &str) -> Option<Vec<&str>> {
    let tokens = tokens(document);
    let mut fields = Vec::new();
    // The brackets the reading stands in: one brace alone is an
    // operation's own selection.
    let mut within: Vec<u8> = Vec::new();
    let mut at = 0;
    while let Some(&token) = tokens.get(at) {
        at += 1;
        let at_root = matches!(within.as_slice(), [b'{']);
        match token {
            Token::Open(open) => within.push(open),
            Token::Close => {
                within.pop();
            }
            Token::Name("fragment") if within.is_empty() => return None,
            Token::Spread | Token::At if at_root => return None,
            Token::Name(name) if at_root => match tokens.get(at) {
                // `alias: Field` — the field is what is asked.
                Some(Token::Colon) => match tokens.get(at + 1) {
                    Some(Token::Name(field)) => {
                        fields.push(*field);
                        at += 2;
                    }
                    _ => return None,
                },
                _ => fields.push(name),
            },
            Token::Name(_) | Token::Colon | Token::Spread | Token::At => {}
        }
    }
    Some(fields)
}

/// Whether a document asks for AniList's genres or its tags, and nothing
/// else: kept half a day ([`COLLECTIONS`]).
fn asks_collections_alone(document: &str) -> bool {
    root_fields(document).is_some_and(|fields| {
        fields.iter().any(|field| COLLECTIONS.contains(field))
            && fields
                .iter()
                .all(|field| COLLECTIONS.contains(field) || *field == "__typename")
    })
}

/// Whether a GraphQL document changes something: any of its operations is a
/// mutation. A document that names several operations and picks one with
/// `operationName` is kept out of the cache if any is a mutation, rather
/// than guessed at.
fn has_mutation(document: &str) -> bool {
    names(document)
        .into_iter()
        .any(|(depth, name)| depth == 0 && name.eq_ignore_ascii_case("mutation"))
}

/// Whether a GraphQL document asks about somebody: their lists, their
/// account, what they did.
fn is_personal(document: &str) -> bool {
    names(document)
        .into_iter()
        .any(|(_, name)| PERSONAL.contains(&name))
}

/// What a relayed answer is filed under: the query asked — its text, its
/// variables, whatever else the body says — digested, under the generation.
fn relay_key(state: &AppState, method: &Method, uri: &Uri, body: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(method.as_str().as_bytes());
    digest.update(b"\n");
    digest.update(relay::sorted_query(uri, relay::OUR_PARAMS).as_bytes());
    digest.update(b"\n");
    digest.update(body);
    let digest: String = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(
        "anilist:{}:{}:{digest}",
        state.caches.generation(),
        state.config.anilist.upstream,
    )
}

/// The identifiers an answer's entries carry: AniList's own, and
/// MyAnimeList's — an anime's only, see [`is_anime`] — which a client may
/// ask for in its place: Yamtrack's import does, and never asks AniList's.
#[derive(Default)]
struct Named {
    anilist: Vec<i64>,
    mal: Vec<i64>,
}

impl Named {
    fn len(&self) -> usize {
        self.anilist.len() + self.mal.len()
    }
}

/// The works of the catalogue an answer's entries are, by either
/// identifier, when a person locked something on them.
struct Held {
    works: HashMap<String, Work>,
    by_anilist: HashMap<i64, String>,
    by_mal: HashMap<i64, String>,
}

/// A work an answer's entries are, and the entries the anime identifier
/// list splits it into ([`service::cour`]).
struct Work {
    item: MediaItem,
    cours: Vec<Cour>,
}

impl Held {
    /// The work an entry is: by AniList's id, which numbers anime and manga
    /// alike in one sequence; or, on an anime alone, by MyAnimeList's. And
    /// which of its seasons or cours the entry is, where the list places it;
    /// and whether it is the whole work as far as the list says — one the
    /// list does not split, or the one entry it keeps whole.
    fn of(&self, object: &Map<String, Value>) -> Option<(&MediaItem, Option<&Cour>, bool)> {
        let anilist = object.get("id").and_then(Value::as_i64);
        let mal = object
            .get("idMal")
            .and_then(Value::as_i64)
            .filter(|_| is_anime(object));
        let id = match anilist.and_then(|id| self.by_anilist.get(&id)) {
            Some(id) => id,
            None => mal.and_then(|id| self.by_mal.get(&id))?,
        };
        let work = self.works.get(id)?;
        let placed = |found: &dyn Fn(&Cour) -> bool| {
            work.cours
                .iter()
                .find(|cour| cour.span.is_some() && found(cour))
        };
        let cour = placed(&|cour| anilist.is_some() && cour.anilist == anilist)
            .or_else(|| placed(&|cour| mal.is_some() && cour.mal == mal));
        let whole = cour.map_or(work.cours.is_empty(), |cour| cour.whole);
        Some((&work.item, cour, whole))
    }
}

/// Write this server's locks into every entry of the answer that is a work
/// of this catalogue. Whether anything changed.
///
/// The entries are read once, in the order the answer lists them, their
/// identifiers looked up together, and the answer walked once more, writing
/// into every entry whose work holds a lock.
async fn patch(state: &AppState, answer: &mut Value) -> AppResult<bool> {
    let Some(data) = answer.get_mut("data") else {
        return Ok(false);
    };
    let mut named = Named::default();
    collect_entries(data, &mut named);
    if named.len() == 0 {
        return Ok(false);
    }

    let as_text = |ids: &[i64]| ids.iter().map(ToString::to_string).collect::<Vec<_>>();
    let anilist =
        repo::item::held_external_ids(&state.db, ExternalSource::AniList, &as_text(&named.anilist))
            .await?;
    let mal =
        repo::item::held_external_ids(&state.db, ExternalSource::Mal, &as_text(&named.mal)).await?;

    let now = chrono::Utc::now();
    let mut held = Held {
        works: HashMap::new(),
        by_anilist: HashMap::new(),
        by_mal: HashMap::new(),
    };
    for (value, media_id) in anilist.iter().chain(mal.iter()) {
        if held.works.contains_key(media_id) {
            continue;
        }
        let Some(mut item) = service::load(state, media_id).await? else {
            continue;
        };
        if item.locked_fields.is_empty() {
            continue;
        }
        state.media.for_clients(&mut item);
        let cours = service::cour::of_work(state, &item, now).await;
        held.works.insert(media_id.clone(), Work { item, cours });
        // The same value may name a work under both sources; each map says
        // which one it is under.
        let _ = value;
    }
    for (value, media_id) in &anilist {
        if held.works.contains_key(media_id)
            && let Ok(id) = value.parse::<i64>()
        {
            held.by_anilist.insert(id, media_id.clone());
        }
    }
    for (value, media_id) in &mal {
        if held.works.contains_key(media_id)
            && let Ok(id) = value.parse::<i64>()
        {
            held.by_mal.insert(id, media_id.clone());
        }
    }
    if held.works.is_empty() {
        return Ok(false);
    }
    Ok(write_entries(data, &held, now.timestamp()))
}

/// Whether an object is an entry of AniList's — a `Media` — this catalogue
/// may hold: identified by AniList's id or MyAnimeList's; not manga, by its
/// type or by its format; and told by what only a media entry carries — its
/// title as an object, its format, its episodes — or by being named a
/// `Media`, or an anime. Characters, staff, users, threads and a media's
/// tags have ids of their own in the same small numbers, and descriptions;
/// a tag says whether it is for adults, as a media does, so that says
/// nothing.
fn is_entry(object: &serde_json::Map<String, Value>) -> bool {
    let identified = object.get("id").is_some_and(Value::is_i64)
        || object.get("idMal").is_some_and(Value::is_i64);
    if !identified {
        return false;
    }
    let typename = object.get("__typename").and_then(Value::as_str);
    if typename.is_some_and(|name| name != "Media") {
        return false;
    }
    let kind = object.get("type").and_then(Value::as_str);
    if kind.is_some_and(|kind| kind != "ANIME") {
        return false;
    }
    let format = object.get("format").and_then(Value::as_str);
    if format.is_some_and(|format| MANGA_FORMATS.contains(&format)) {
        return false;
    }
    typename == Some("Media")
        || kind == Some("ANIME")
        || object.get("title").is_some_and(Value::is_object)
        || format.is_some()
        || object.contains_key("episodes")
}

/// Whether an entry is an anime, as far as it says: its type, when it was
/// asked; its format, when that was; otherwise a value only an anime has —
/// a count of episodes, a season, a duration — and none only a manga has.
///
/// MyAnimeList numbers its anime and its manga apart, in sequences that
/// overlap — anime 1 is Cowboy Bebop, manga 1 is Monster — and this
/// catalogue holds anime: an entry is matched by MyAnimeList's id only when
/// it says it is one. An anime still airing with no count of episodes yet,
/// asked neither its type nor its format, says nothing a manga could not,
/// and is left as it came.
fn is_anime(object: &serde_json::Map<String, Value>) -> bool {
    if let Some(kind) = object.get("type").and_then(Value::as_str) {
        return kind == "ANIME";
    }
    if let Some(format) = object.get("format").and_then(Value::as_str) {
        return ANIME_FORMATS.contains(&format);
    }
    let carries = |field: &&str| object.get(*field).is_some_and(|value| !value.is_null());
    !MANGA_ONLY.iter().any(carries) && ANIME_ONLY.iter().any(carries)
}

/// The identifiers of the entries an answer carries, in the order it lists
/// them, each once, the first [`MOST_PATCHED`] of them: AniList's of every
/// entry, MyAnimeList's of an anime's.
fn collect_entries(value: &Value, named: &mut Named) {
    if named.len() >= MOST_PATCHED {
        return;
    }
    match value {
        Value::Object(object) => {
            if is_entry(object) {
                if let Some(id) = object.get("id").and_then(Value::as_i64)
                    && !named.anilist.contains(&id)
                {
                    named.anilist.push(id);
                }
                if is_anime(object)
                    && let Some(id) = object.get("idMal").and_then(Value::as_i64)
                    && !named.mal.contains(&id)
                {
                    named.mal.push(id);
                }
            }
            for child in object.values() {
                collect_entries(child, named);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_entries(child, named);
            }
        }
        _ => {}
    }
}

/// Write each work's locks into every entry of the answer that is it, as
/// of `now`, in seconds since the epoch.
fn write_entries(value: &mut Value, held: &Held, now: i64) -> bool {
    let mut changed = false;
    match value {
        Value::Object(object) => {
            if is_entry(object)
                && let Some((item, cour, whole)) = held.of(object)
            {
                changed |= write_entry(object, item, cour, whole, now);
            }
            for child in object.values_mut() {
                changed |= write_entries(child, held, now);
            }
        }
        Value::Array(items) => {
            for child in items {
                changed |= write_entries(child, held, now);
            }
        }
        _ => {}
    }
    changed
}

/// The work's locks, in AniList's names: only the fields the client asked
/// for, since a GraphQL answer carries nothing it was not asked. Where the
/// entry is one of the work's seasons or cours (`cour`), what was corrected
/// of when the work airs is written as it bears on that one.
///
/// A title and an overview are the work's: written into an entry that is
/// the whole of it (`whole`), never into one of its seasons or cours, which
/// AniList names and tells of apart — the work's locked title over each
/// would leave a client one name for every season of it.
fn write_entry(
    object: &mut Map<String, Value>,
    item: &MediaItem,
    cour: Option<&Cour>,
    whole: bool,
    now: i64,
) -> bool {
    let mut changed = false;
    if whole
        && relay::is_locked(item, "title")
        && let Some(Value::Object(title)) = object.get_mut("title")
    {
        // The English and the preferred name are what a client shows; the
        // romaji and the native one are the work's own, and stay.
        for key in ["english", "userPreferred"] {
            if title.contains_key(key) {
                title.insert(key.to_string(), Value::String(item.title.clone()));
                changed = true;
            }
        }
    }
    if whole
        && relay::is_locked(item, "overview")
        && let Some(overview) = &item.overview
        && object.contains_key("description")
    {
        object.insert("description".to_string(), Value::String(overview.clone()));
        changed = true;
    }
    if relay::is_locked(item, "genres") && object.contains_key("genres") {
        object.insert(
            "genres".to_string(),
            Value::Array(item.genres.iter().cloned().map(Value::String).collect()),
        );
        changed = true;
    }
    if relay::is_locked(item, "primaryPoster")
        && let Some(url) = relay::chosen_image(item, CoverType::Poster)
        && let Some(Value::Object(cover)) = object.get_mut("coverImage")
    {
        for key in ["extraLarge", "large", "medium"] {
            if cover.contains_key(key) {
                cover.insert(key.to_string(), Value::String(url.clone()));
                changed = true;
            }
        }
    }
    if relay::is_locked(item, "primaryFanart")
        && let Some(url) = relay::chosen_image(item, CoverType::Fanart)
        && object.contains_key("bannerImage")
    {
        object.insert("bannerImage".to_string(), Value::String(url));
        changed = true;
    }
    changed |= take_hidden_synonyms(object, item);
    if relay::is_locked(item, "isAdult") && object.contains_key("isAdult") {
        object.insert("isAdult".to_string(), Value::Bool(item.is_adult));
        changed = true;
    }
    if relay::is_locked(item, "studio")
        && let Some(name) = item.studio.as_deref().filter(|n| !n.trim().is_empty())
        && let Some(Value::Object(studios)) = object.get_mut("studios")
    {
        changed |= write_studio(studios, name);
    }
    if let Some(cour) = cour {
        changed |= write_airing(object, item, cour, now);
    }
    changed
}

/// A name or a title as it is compared: its case and the blanks around it
/// aside.
fn folded(text: &str) -> String {
    text.trim().to_lowercase()
}

/// AniList's other names for an entry, less those a person hid here among
/// the work's other titles: told by their text alone, as TheTVDB's are,
/// since AniList's carry no language. Whether any was taken out.
fn take_hidden_synonyms(object: &mut Map<String, Value>, item: &MediaItem) -> bool {
    if item.hidden.alternative_titles.is_empty() {
        return false;
    }
    let Some(Value::Array(synonyms)) = object.get_mut("synonyms") else {
        return false;
    };
    let hidden: Vec<String> = item
        .hidden
        .alternative_titles
        .iter()
        .map(|title| folded(&title.title))
        .collect();
    let before = synonyms.len();
    synonyms.retain(|synonym| {
        !synonym
            .as_str()
            .is_some_and(|text| hidden.contains(&folded(text)))
    });
    synonyms.len() != before
}

/// What a client may ask of a studio that a name locked here can be
/// written over: anything else — its id, its address, its favourites —
/// names a studio of AniList's, which a name cannot become.
const STUDIO_RENAMABLE: &[&str] = &["name", "isAnimationStudio", "__typename"];

/// The main studio locked here, in AniList's list of an entry's studios —
/// its `nodes`, its `edges`, or both, as the client asked them.
///
/// A studio AniList lists under that name is put first, and made the one
/// main studio where the list says which are. One it does not list is
/// written over the name of its first main studio, only where the client
/// asked nothing else that names a studio of AniList's ([`STUDIO_RENAMABLE`]):
/// an id cannot be made up, and a studio's id beside another's name sends a
/// client to the wrong one. Whether anything changed.
fn write_studio(studios: &mut Map<String, Value>, name: &str) -> bool {
    let wanted = folded(name);
    let named = |node: &Value| {
        node.get("name")
            .and_then(Value::as_str)
            .is_some_and(|theirs| folded(theirs) == wanted)
    };
    let renamable = |node: &Map<String, Value>| {
        node.keys()
            .all(|key| STUDIO_RENAMABLE.contains(&key.as_str()))
    };
    let mut changed = false;

    if let Some(Value::Array(nodes)) = studios.get_mut("nodes") {
        match nodes.iter().position(named) {
            Some(0) => {}
            Some(at) => {
                let node = nodes.remove(at);
                nodes.insert(0, node);
                changed = true;
            }
            None => {
                if let Some(Value::Object(first)) = nodes.first_mut()
                    && renamable(first)
                {
                    first.insert("name".to_string(), Value::String(name.to_string()));
                    changed = true;
                }
            }
        }
    }

    if let Some(Value::Array(edges)) = studios.get_mut("edges") {
        let chosen = match edges
            .iter()
            .position(|edge| edge.get("node").is_some_and(named))
        {
            Some(at) => Some(at),
            None => {
                let main = edges
                    .iter()
                    .position(|edge| edge.get("isMain") == Some(&Value::Bool(true)))
                    .unwrap_or(0);
                match edges.get_mut(main).and_then(|edge| edge.get_mut("node")) {
                    Some(Value::Object(theirs)) if renamable(theirs) => {
                        theirs.insert("name".to_string(), Value::String(name.to_string()));
                        changed = true;
                        Some(main)
                    }
                    _ => None,
                }
            }
        };
        if let Some(at) = chosen {
            if at != 0 {
                let edge = edges.remove(at);
                edges.insert(0, edge);
                changed = true;
            }
            for (place, edge) in edges.iter_mut().enumerate() {
                if let Value::Object(edge) = edge
                    && let Some(main) = edge.get_mut("isMain")
                    && *main != Value::Bool(place == 0)
                {
                    *main = Value::Bool(place == 0);
                    changed = true;
                }
            }
        }
    }
    changed
}

/// What a person corrected here of when the work airs, written into the
/// entry that is one of its seasons or cours, or the whole of it.
///
/// - **Its schedule.** Where an air date or time was locked on one of the
///   entry's episodes, or an episode made by hand among them, the entry is
///   described from the episodes as they are served ([`Cour`]): its first
///   day, how far it has got, the next episode to air — its number counted
///   from the entry's first — or none once it has finished, and its last
///   day then. Its count of episodes is written only where an episode made
///   here is among them and it has finished: a count still growing says
///   nothing a correction made.
/// - **The work's own.** Its status, locked: ended is every entry
///   finished, upcoming every one not yet released, and continuing says
///   something only of an entry that is the whole work. Its first day,
///   locked, is the start of the entry that opens the work; its last day,
///   locked, the end of the entry that closes a work that ended. A film's
///   day in cinemas is its start and its end, as AniList dates one.
///
/// What the work says outweighs what its episodes do. A lock left empty
/// leaves AniList's value.
fn write_airing(object: &mut Map<String, Value>, item: &MediaItem, cour: &Cour, now: i64) -> bool {
    let mut start: Option<String> = None;
    let mut end: Option<String> = None;
    let mut status: Option<Status> = None;
    let mut episodes: Option<i64> = None;
    // Some(None) is none to air.
    let mut next: Option<Option<&Next>> = None;

    if cour.corrected {
        start.clone_from(&cour.starts);
        end.clone_from(&cour.ends);
        status = cour.status;
        if cour.made_here && cour.status == Some(Status::Finished) {
            episodes = Some(cour.episodes);
        }
        next = match (&cour.next, cour.status) {
            (Some(coming), _) => Some(Some(coming)),
            (None, Some(Status::Finished)) => Some(None),
            _ => None,
        };
    }

    let film = item.kind == MediaKind::Movie;
    if relay::is_locked(item, "status") {
        let said = match (item.status.as_deref(), film) {
            (Some("ended"), false) | (Some("released" | "inCinemas"), true) => {
                Some(Status::Finished)
            }
            (Some("upcoming"), false) | (Some("announced" | "tba"), true) => {
                Some(Status::NotYetReleased)
            }
            (Some("continuing"), false) if cour.whole => Some(Status::Releasing),
            _ => None,
        };
        if let Some(said) = said {
            status = Some(said);
            if said == Status::Finished {
                next = Some(None);
            }
        }
    }
    let day_of = |date: &Option<String>| {
        date.as_deref()
            .and_then(|d| d.trim().get(..10))
            .filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok())
            .map(String::from)
    };
    if film {
        if relay::is_locked(item, "inCinemas")
            && let Some(day) = day_of(&item.in_cinemas)
        {
            start = Some(day.clone());
            end = Some(day);
        }
    } else {
        if relay::is_locked(item, "firstAired")
            && cour.opens_work
            && let Some(day) = day_of(&item.first_aired)
        {
            start = Some(day);
        }
        if relay::is_locked(item, "lastAired")
            && cour.ends_work
            && item.status.as_deref() == Some("ended")
            && let Some(day) = day_of(&item.last_aired)
        {
            end = Some(day);
        }
    }

    let mut changed = false;
    if let Some(day) = start {
        changed |= write_date(object, "startDate", &day);
    }
    if let Some(day) = end {
        changed |= write_date(object, "endDate", &day);
    }
    if let Some(status) = status
        && object.contains_key("status")
    {
        object.insert(
            "status".to_string(),
            Value::String(status.anilist().to_string()),
        );
        changed = true;
    }
    if let Some(count) = episodes
        && object.contains_key("episodes")
    {
        object.insert("episodes".to_string(), Value::from(count));
        changed = true;
    }
    match next {
        Some(None) => {
            if object
                .get("nextAiringEpisode")
                .is_some_and(|theirs| !theirs.is_null())
            {
                object.insert("nextAiringEpisode".to_string(), Value::Null);
                changed = true;
            }
        }
        // Over the one AniList gave: what a client asked of it is known
        // only from what came back, and its id is AniList's.
        Some(Some(coming)) => {
            if let Some(Value::Object(theirs)) = object.get_mut("nextAiringEpisode") {
                for (key, value) in [
                    ("airingAt", coming.airing_at),
                    ("timeUntilAiring", coming.airing_at - now),
                    ("episode", i64::from(coming.episode)),
                ] {
                    if let Some(slot) = theirs.get_mut(key) {
                        *slot = Value::from(value);
                        changed = true;
                    }
                }
            }
        }
        None => {}
    }
    changed
}

/// A day, `YYYY-MM-DD`, written into AniList's date of an entry — its
/// `year`, `month` and `day`, those the client asked for.
fn write_date(object: &mut Map<String, Value>, key: &str, day: &str) -> bool {
    let Some(Value::Object(date)) = object.get_mut(key) else {
        return false;
    };
    let Ok(day) = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d") else {
        return false;
    };
    use chrono::Datelike as _;
    let mut changed = false;
    for (part, value) in [
        ("year", day.year()),
        ("month", day.month() as i32),
        ("day", day.day() as i32),
    ] {
        if let Some(slot) = date.get_mut(part) {
            *slot = Value::from(value);
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use super::*;
    use serde_json::json;

    #[test]
    fn a_mutation_is_told_from_a_query() {
        assert!(has_mutation(
            "mutation { SaveMediaListEntry(mediaId: 1) { id } }"
        ));
        assert!(has_mutation("  Mutation ($id: Int) { … }"));
        // Behind a comment, or second among several operations.
        assert!(has_mutation(
            "# a comment first\nmutation { SaveMediaListEntry(mediaId: 1) { id } }"
        ));
        assert!(has_mutation(
            "query A { Media(id: 1) { id } } mutation B { SaveMediaListEntry(mediaId: 1) { id } }"
        ));
        assert!(has_mutation(
            "fragment f on Media { id } mutation { SaveMediaListEntry(mediaId: 1) { ...f } }"
        ));

        assert!(!has_mutation("query ($id: Int) { Media(id: $id) { id } }"));
        assert!(!has_mutation("{ Media(id: 1) { id } }"));
        // The word inside a selection, an argument or a string is not an
        // operation.
        assert!(!has_mutation(
            "{ Media(search: \"mutation\") { id mutation } }"
        ));
        assert!(!has_mutation(
            "{ Media(id: 1) { description(asHtml: false) } } # mutation"
        ));
        assert!(!has_mutation(
            "fragment f on Media { id } query { Media(id: 1) { ...f } }"
        ));
        assert!(!has_mutation(""));
    }

    #[test]
    fn a_query_about_somebody_is_told_from_one_about_a_work() {
        assert!(is_personal(
            "query ($userName: String) { MediaListCollection(userName: $userName, type: ANIME) { lists { entries { media { idMal } } } } }"
        ));
        assert!(is_personal("{ Viewer { id name } }"));
        assert!(is_personal("{ User(name: \"x\") { id } }"));
        assert!(is_personal("{ Page { mediaList(userId: 1) { id } } }"));
        assert!(is_personal(
            "{ Media(id: 1) { id mediaListEntry { status } } }"
        ));

        assert!(!is_personal("{ Media(id: 1) { id title { english } } }"));
        assert!(!is_personal(
            "{ Page(page: 1) { media(search: \"user\") { id } } }"
        ));
    }

    /// Every name AniList's schema files a user under, or a person's doing,
    /// or picks by one — read by introspection on 1 November 2026.
    #[test]
    fn whatever_the_schema_files_under_somebody_is_theirs() {
        for somebodys in [
            "{ Page { users(search: \"x\") { name } } }",
            "{ MutualFollower(userId: 1) { id } }",
            "{ Page { mutualFollowers(userId: 1) { name } } }",
            "{ Like(likeableId: 1, type: THREAD) { name } }",
            "{ Page { likes(likeableId: 1, type: ACTIVITY) { name } } }",
            "{ ActivityReply(id: 1) { text } }",
            "{ Page { activityReplies(activityId: 1) { text } } }",
            "{ Page { threads(mediaCategoryId: 1) { title } } }",
            "{ Page { threadComments(threadId: 1) { comment } } }",
            "{ AniChartUser { settings } }",
            "{ Review(mediaId: 1) { summary } }",
            "{ Page { reviews(mediaId: 1) { summary } } }",
            "{ Staff(id: 1) { name { full } submitter { id } } }",
            "{ Thread(id: 1) { replyUser { id } } }",
            "{ Page { recommendations(userId: 1) { rating } } }",
            "{ Page { media(onList: true) { id } } }",
        ] {
            assert!(is_personal(somebodys), "{somebodys}");
        }
        // A media's recommendations and its staff are anybody's to ask.
        assert!(!is_personal(
            "{ Media(id: 1) { recommendations { nodes { mediaRecommendation { id } } } staff { nodes { name { full } } } } }"
        ));
        assert!(!is_personal("{ GenreCollection }"));
    }

    #[test]
    fn the_genre_and_tag_lists_asked_alone_are_told_from_any_other_query() {
        for alone in [
            "{ GenreCollection }",
            "# Kometa's\nquery { MediaTagCollection { name category isAdult } }",
            "query Lists { genres: GenreCollection tags: MediaTagCollection(status: 0) { name } __typename }",
            "query ($status: Int = 1) { MediaTagCollection(status: $status) { name } }",
        ] {
            assert!(asks_collections_alone(alone), "{alone}");
        }
        for other in [
            "{ Media(id: 1) { id } GenreCollection }",
            "{ Media(id: 1) { genres } }",
            "{ Page { media(genre: \"GenreCollection\") { id } } }",
            "query A { GenreCollection } query B { Media(id: 1) { id } }",
            "{ __typename }",
            "",
            // Read no further: whatever they stand for.
            "{ GenreCollection @include(if: true) }",
            "{ ... on Query { GenreCollection } }",
            "{ ...Lists } fragment Lists on Query { GenreCollection }",
            "fragment Tag on MediaTag { name } query { MediaTagCollection { ...Tag } }",
        ] {
            assert!(!asks_collections_alone(other), "{other}");
        }
    }

    // ─── the relay, before a stand-in for AniList ────────────────────────────

    /// What the stand-in answers a request with, and after how long.
    #[derive(Clone)]
    struct Reply {
        status: &'static str,
        headers: &'static str,
        body: &'static str,
        after: Duration,
    }

    const fn reply(status: &'static str, body: &'static str) -> Reply {
        Reply {
            status,
            headers: "",
            body,
            after: Duration::ZERO,
        }
    }

    /// A stand-in for AniList: each request is answered with the next of
    /// `replies`, the last again once they run out. And how many came.
    async fn stand_in(replies: Vec<Reply>) -> (String, Arc<AtomicUsize>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = format!("http://{}", listener.local_addr().unwrap());
        let asked = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&asked);
        tokio::spawn(async move {
            while let Ok((mut connection, _)) = listener.accept().await {
                let turn = counted.fetch_add(1, Ordering::SeqCst);
                let reply = replies[turn.min(replies.len() - 1)].clone();
                tokio::spawn(async move {
                    // The head, then as much body as it announces.
                    let mut request = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        let read = connection.read(&mut chunk).await.unwrap_or(0);
                        if read == 0 {
                            break;
                        }
                        request.extend_from_slice(&chunk[..read]);
                        let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                            continue;
                        };
                        let length = String::from_utf8_lossy(&request[..end])
                            .to_ascii_lowercase()
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")?.trim().parse().ok()
                            })
                            .unwrap_or(0usize);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                    tokio::time::sleep(reply.after).await;
                    let answer = format!(
                        "HTTP/1.1 {}\r\nContent-Type: application/json\r\n{}Content-Length: {}\r\n\
                         Connection: close\r\n\r\n{}",
                        reply.status,
                        reply.headers,
                        reply.body.len(),
                        reply.body
                    );
                    let _ = connection.write_all(answer.as_bytes()).await;
                    let _ = connection.shutdown().await;
                });
            }
        });
        (at, asked)
    }

    /// This server, relaying AniList to `at`.
    async fn server(at: &str) -> AppState {
        let mut config = crate::config::Config::from_env().expect("a configuration");
        config.mode = crate::config::Mode::Single;
        config.database = crate::db::testing::database().await;
        config.security.bootstrap_admin = None;
        config.cache.redis_url = None;
        config.media.storage = crate::config::MediaStorage::Off;
        config.clients = None;
        config.tmdb.api_key = None;
        config.tvdb.api_key = None;
        config.tvdb.enabled = false;
        config.anilist.upstream = at.to_string();
        config.anilist.passthrough = true;
        AppState::bootstrap(config).await.expect("a server")
    }

    /// What a client posts to ask `query`.
    fn posted(query: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({ "query": query })).unwrap()
    }

    /// What the relay answers `query`, asked with somebody's own AniList
    /// token when there is one.
    async fn relayed(state: &AppState, query: &str, token: Option<&str>) -> Response {
        let mut request = axum::http::Request::builder()
            .method(Method::POST)
            .uri("/")
            .header(header::HOST, "graphql.anilist.co")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = request.body(Body::from(posted(query))).unwrap();
        match relay(State(state.clone()), request).await {
            Ok(response) => response,
            Err(error) => error.into_response(),
        }
    }

    async fn text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn seconds(response: &Response, name: header::HeaderName) -> Option<u64> {
        response.headers().get(name)?.to_str().ok()?.parse().ok()
    }

    /// What the relay space holds for `query`.
    async fn kept(state: &AppState, query: &str) -> Option<cache::Relayed> {
        let key = relay_key(state, &Method::POST, &Uri::from_static("/"), &posted(query));
        state.caches.relay.get(&key).await
    }

    /// What was kept for `query`, made `seconds` old — as if AniList had
    /// answered it then.
    async fn aged(state: &AppState, query: &str, seconds: u64) {
        let key = relay_key(state, &Method::POST, &Uri::from_static("/"), &posted(query));
        let mut kept = state.caches.relay.get(&key).await.expect("kept");
        kept.expires_at =
            cache::now_secs() + cache::relay_ttl(cache::ANILIST_QUERY).as_secs() - seconds;
        state.caches.relay.insert(key, kept).await;
    }

    #[tokio::test]
    async fn when_anilist_fails_its_last_answer_stands_in_for_a_day() {
        const KEPT: &str = r#"{"data":{"Media":{"id":9300001,"title":{"romaji":"Kept"}}}}"#;
        let (at, asked) = stand_in(vec![
            reply("200 OK", KEPT),
            reply("500 Internal Server Error", "{}"),
            reply("502 Bad Gateway", ""),
            reply("503 Service Unavailable", ""),
            reply("504 Gateway Timeout", ""),
            // Its API switched off.
            reply(
                "403 Forbidden",
                r#"{"errors":[{"message":"The API is temporarily disabled.","status":403}],"data":null}"#,
            ),
            reply("502 Bad Gateway", ""),
            reply("429 Too Many Requests", "{}"),
        ])
        .await;
        let state = server(&at).await;
        let query = "{ Media(id: 9300001) { id title { romaji } } }";
        assert_eq!(text(relayed(&state, query, None).await).await, KEPT);

        // Two hours on: past the hour a query is good for, inside the day.
        aged(&state, query, 2 * 3600).await;
        for failure in ["500", "502", "503", "504", "403"] {
            let stale = relayed(&state, query, None).await;
            assert_eq!(stale.status(), StatusCode::OK, "{failure}");
            let age = seconds(&stale, header::AGE).expect("its age");
            assert!((2 * 3600..2 * 3600 + 60).contains(&age), "{failure}: {age}");
            assert_eq!(text(stale).await, KEPT, "{failure}");
        }

        // A day and a minute: too old, and AniList's failure is handed on.
        aged(&state, query, 86_400 + 60).await;
        let failed = relayed(&state, query, None).await;
        assert_eq!(failed.status(), StatusCode::BAD_GATEWAY);
        assert!(failed.headers().get(header::AGE).is_none());

        // Its 429 is stood in for too, while what was kept is young enough.
        aged(&state, query, 2 * 3600).await;
        let stale = relayed(&state, query, None).await;
        assert_eq!(stale.status(), StatusCode::OK);
        assert_eq!(text(stale).await, KEPT);
        assert_eq!(
            asked.load(Ordering::SeqCst),
            8,
            "every failure was AniList's"
        );
    }

    #[tokio::test]
    async fn a_shut_door_is_answered_with_what_was_kept_or_a_429_that_says_when() {
        const KEPT: &str = r#"{"data":{"Media":{"id":9300011}}}"#;
        let (at, asked) = stand_in(vec![
            reply("200 OK", KEPT),
            Reply {
                headers: "Retry-After: 30\r\n",
                ..reply(
                    "429 Too Many Requests",
                    r#"{"errors":[{"message":"Too Many Requests.","status":429}],"data":null}"#,
                )
            },
            reply("200 OK", "{}"),
        ])
        .await;
        let state = server(&at).await;
        let kept_query = "{ Media(id: 9300011) { id } }";
        relayed(&state, kept_query, None).await;
        aged(&state, kept_query, 2 * 3600).await;

        // AniList's 429 is handed back as it came, and shuts the door.
        let refused = relayed(&state, "{ Media(id: 9300012) { id } }", None).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(seconds(&refused, header::RETRY_AFTER), Some(30));
        assert!(text(refused).await.contains("Too Many Requests."));

        // Shut, nothing is sent: what was kept stands in…
        let stale = relayed(&state, kept_query, None).await;
        assert_eq!(stale.status(), StatusCode::OK);
        assert!(stale.headers().contains_key(header::AGE));
        assert_eq!(text(stale).await, KEPT);
        // …and where nothing was, a 429 says when it opens, for a query
        // about somebody as for any other.
        for (query, token) in [
            ("{ Media(id: 9300013) { id } }", None),
            ("{ Viewer { id } }", Some("their-anilist-token")),
        ] {
            let shut = relayed(&state, query, token).await;
            assert_eq!(shut.status(), StatusCode::TOO_MANY_REQUESTS, "{query}");
            let left = seconds(&shut, header::RETRY_AFTER).expect("when it opens");
            assert!((1..=30).contains(&left), "{query}: {left}");
        }
        // This server's own fetches find it shut too.
        let error = state
            .anilist
            .media(9_300_014, MediaKind::Series)
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("left alone"), "{error:#}");
        assert_eq!(asked.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_query_about_somebody_is_asked_alone_each_time_and_never_stood_in_for() {
        const USERS: &str = r#"{"data":{"Page":{"users":[{"name":"somebody"}]}}}"#;
        const LATE: Duration = Duration::from_millis(300);
        let (at, asked) = stand_in(vec![
            Reply {
                after: LATE,
                ..reply("200 OK", USERS)
            },
            Reply {
                after: LATE,
                ..reply("200 OK", USERS)
            },
            reply("200 OK", USERS),
            reply("502 Bad Gateway", ""),
            reply("200 OK", r#"{"data":{"Media":{"id":9300021}}}"#),
            reply("502 Bad Gateway", ""),
        ])
        .await;
        let state = server(&at).await;

        // Asked by two callers at once: two exchanges.
        let users = "{ Page { users(search: \"somebody\") { name } } }";
        let (first, second) =
            tokio::join!(relayed(&state, users, None), relayed(&state, users, None));
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(asked.load(Ordering::SeqCst), 2, "one exchange each");
        // Kept for nobody, and nothing stands in when AniList fails.
        assert_eq!(relayed(&state, users, None).await.status(), StatusCode::OK);
        assert!(kept(&state, users).await.is_none());
        let failed = relayed(&state, users, None).await;
        assert_eq!(failed.status(), StatusCode::BAD_GATEWAY);
        assert!(failed.headers().get(header::AGE).is_none());
        assert_eq!(asked.load(Ordering::SeqCst), 4);

        // What anybody may ask, asked with somebody's own token, is theirs:
        // what was kept for the others does not stand in for it.
        let media = "{ Media(id: 9300021) { id } }";
        relayed(&state, media, None).await;
        aged(&state, media, 2 * 3600).await;
        let theirs = relayed(&state, media, Some("their-anilist-token")).await;
        assert_eq!(theirs.status(), StatusCode::BAD_GATEWAY);
        assert!(theirs.headers().get(header::AGE).is_none());
        assert_eq!(asked.load(Ordering::SeqCst), 6);
    }

    #[tokio::test]
    async fn the_same_query_asked_at_once_is_one_exchange_with_anilist() {
        const MEDIA: &str = r#"{"data":{"Media":{"id":9300031}}}"#;
        let (at, asked) = stand_in(vec![Reply {
            after: Duration::from_millis(300),
            ..reply("200 OK", MEDIA)
        }])
        .await;
        let state = server(&at).await;
        let query = "{ Media(id: 9300031) { id } }";

        let answers = futures::future::join_all((0..5).map(|_| relayed(&state, query, None))).await;
        for answer in answers {
            assert_eq!(answer.status(), StatusCode::OK);
            assert_eq!(text(answer).await, MEDIA);
        }
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        // Kept: the next one is answered here.
        assert_eq!(text(relayed(&state, query, None).await).await, MEDIA);
        assert_eq!(asked.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn the_genre_and_tag_lists_asked_alone_are_kept_half_a_day() {
        let (at, asked) = stand_in(vec![
            reply(
                "200 OK",
                r#"{"data":{"GenreCollection":["Action","Drama"]}}"#,
            ),
            reply(
                "200 OK",
                r#"{"data":{"Media":{"id":9300041},"GenreCollection":["Action"]}}"#,
            ),
        ])
        .await;
        let state = server(&at).await;
        let left = |kept: cache::Relayed| kept.expires_at - cache::now_secs();

        let genres = "query { GenreCollection }";
        relayed(&state, genres, None).await;
        let kept_for = left(kept(&state, genres).await.expect("kept"));
        assert!(kept_for > 11 * 3600 && kept_for <= 12 * 3600, "{kept_for}");

        // Beside anything else, an hour.
        let both = "{ Media(id: 9300041) { id } GenreCollection }";
        relayed(&state, both, None).await;
        let kept_for = left(kept(&state, both).await.expect("kept"));
        assert!(kept_for <= 3600, "{kept_for}");

        relayed(&state, genres, None).await;
        assert_eq!(asked.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_query_is_read_from_the_body_or_the_address() {
        let uri: Uri = "/".parse().unwrap();
        let body = br#"{"query":"query { Media(id: 1) { id } }","variables":{}}"#;
        assert_eq!(
            query_text(&uri, body).as_deref(),
            Some("query { Media(id: 1) { id } }")
        );
        let get: Uri = "/?query=%7B%20Media(id%3A%201)%20%7B%20id%20%7D%20%7D"
            .parse()
            .unwrap();
        assert_eq!(
            query_text(&get, b"").as_deref(),
            Some("{ Media(id: 1) { id } }")
        );
        assert_eq!(query_text(&uri, b"not json"), None);
        assert_eq!(query_text(&uri, b""), None);
    }

    fn locked_anime() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Mon titre".into();
        item.overview = Some("Mon résumé".into());
        item.genres = vec!["Drame".into()];
        item.locked_fields = vec![
            "item/genres".into(),
            "item/overview".into(),
            "item/title".into(),
        ];
        item
    }

    /// Now, as the writers are given it: 1 November 2026, 12:00 UTC.
    const NOW: i64 = 1_793_534_400;

    fn held(anilist: &[i64], mal: &[i64], item: MediaItem) -> Held {
        held_split(anilist, mal, item, Vec::new())
    }

    /// [`held`], with the entries the list splits the work into.
    fn held_split(anilist: &[i64], mal: &[i64], item: MediaItem, cours: Vec<Cour>) -> Held {
        let mut held = Held {
            works: HashMap::new(),
            by_anilist: HashMap::new(),
            by_mal: HashMap::new(),
        };
        held.works.insert("work".into(), Work { item, cours });
        for id in anilist {
            held.by_anilist.insert(*id, "work".into());
        }
        for id in mal {
            held.by_mal.insert(*id, "work".into());
        }
        held
    }

    #[test]
    fn every_entry_of_an_answer_is_found_and_patched_where_it_was_asked_for() {
        let mut answer = json!({ "data": {
            "Page": { "media": [
                { "id": 1, "type": "ANIME", "title": { "romaji": "Shingeki", "english": "Attack on Titan", "userPreferred": "Shingeki" }, "description": "AniList's", "genres": ["Action"] },
                { "id": 2, "type": "ANIME", "title": { "romaji": "Other" } },
                { "id": 3, "type": "MANGA", "title": { "english": "A manga" } }
            ]},
            "Media": { "id": 1, "title": { "native": "進撃の巨人" }, "coverImage": { "large": "https://s4.anilist.co/x.jpg" } }
        }});

        let mut named = Named::default();
        collect_entries(&answer["data"], &mut named);
        assert_eq!(named.anilist, vec![1, 2]);
        assert!(named.mal.is_empty());

        assert!(write_entries(
            &mut answer["data"],
            &held(&[1], &[], locked_anime()),
            NOW
        ));

        let first = &answer["data"]["Page"]["media"][0];
        assert_eq!(first["title"]["english"], json!("Mon titre"));
        assert_eq!(first["title"]["userPreferred"], json!("Mon titre"));
        assert_eq!(first["title"]["romaji"], json!("Shingeki"));
        assert_eq!(first["description"], json!("Mon résumé"));
        assert_eq!(first["genres"], json!(["Drame"]));
        // Not asked for: not added.
        let single = &answer["data"]["Media"];
        assert_eq!(single["title"], json!({ "native": "進撃の巨人" }));
        assert!(single.get("description").is_none());
        // Another entry, and the manga, as they came.
        assert_eq!(
            answer["data"]["Page"]["media"][1]["title"]["romaji"],
            json!("Other")
        );
        assert_eq!(
            answer["data"]["Page"]["media"][2]["title"]["english"],
            json!("A manga")
        );
    }

    #[test]
    fn an_entry_named_by_its_myanimelist_id_alone_is_found_too() {
        // Yamtrack's import asks neither `id` nor `type`: the preferred
        // title, the cover, MyAnimeList's id and the counts.
        let mut answer = json!({ "data": { "anime": { "lists": [{ "entries": [
            { "id": 77, "status": "COMPLETED", "media": { "title": { "userPreferred": "Shingeki" }, "coverImage": { "large": "x" }, "idMal": 16498, "episodes": 25 } }
        ] }] } } });
        let mut named = Named::default();
        collect_entries(&answer["data"], &mut named);
        assert!(named.anilist.is_empty(), "{:?}", named.anilist);
        assert_eq!(named.mal, vec![16498]);

        assert!(write_entries(
            &mut answer["data"],
            &held(&[], &[16498], locked_anime()),
            NOW
        ));
        assert_eq!(
            answer["data"]["anime"]["lists"][0]["entries"][0]["media"]["title"]["userPreferred"],
            json!("Mon titre")
        );
    }

    #[test]
    fn what_is_not_a_media_entry_is_left_alone_whatever_its_id() {
        // A character, a staff member, a user and a thread have ids in the
        // same small numbers, and descriptions or titles of their own.
        for other in [
            json!({ "id": 1, "name": { "full": "Eren" }, "description": "A character" }),
            json!({ "id": 1, "description": "A staff member", "primaryOccupations": [] }),
            json!({ "id": 1, "name": "somebody", "bannerImage": "x" }),
            json!({ "id": 1, "title": "A thread", "body": "…" }),
            json!({ "id": 1, "__typename": "Character", "title": { "english": "x" } }),
            json!({ "id": 77, "status": "COMPLETED", "media": { "id": 5, "episodes": 12 } }),
        ] {
            let mut named = Named::default();
            collect_entries(&json!({ "x": other }), &mut named);
            assert!(
                !named.anilist.contains(&1),
                "{other} was taken for an entry"
            );
        }
        // A media entry named as such is one, whatever else it carries.
        let mut named = Named::default();
        collect_entries(
            &json!({ "id": 1, "__typename": "Media", "description": "x" }),
            &mut named,
        );
        assert_eq!(named.anilist, vec![1]);
    }

    #[test]
    fn a_media_tag_is_not_an_entry_whatever_its_id() {
        // A tag has an id in the same small numbers, a description, and
        // says whether it is for adults, as a media does.
        let tag = json!({ "id": 1, "name": "Isekai", "description": "A tag", "isAdult": false, "rank": 90 });
        let Value::Object(object) = &tag else {
            unreachable!()
        };
        assert!(!is_entry(object));

        let mut answer = json!({ "data": { "Media": {
            "id": 21, "title": { "english": "One Piece" }, "description": "AniList's",
            "tags": [ { "id": 1, "name": "Pirates", "description": "A tag", "isAdult": false } ]
        }}});
        let mut named = Named::default();
        collect_entries(&answer["data"], &mut named);
        assert_eq!(named.anilist, vec![21]);

        // The work held under AniList's 1 is not the tag numbered 1.
        assert!(!write_entries(
            &mut answer["data"],
            &held(&[1], &[], locked_anime()),
            NOW
        ));
        assert_eq!(
            answer["data"]["Media"]["tags"][0]["description"],
            json!("A tag")
        );
    }

    #[test]
    fn an_entry_is_an_anime_only_when_it_says_so() {
        let anime = |value: Value| {
            let Value::Object(object) = value else {
                unreachable!()
            };
            is_anime(&object)
        };
        // By its type, or its format.
        assert!(anime(json!({ "idMal": 1, "type": "ANIME" })));
        assert!(!anime(
            json!({ "idMal": 1, "type": "MANGA", "episodes": 12 })
        ));
        assert!(anime(json!({ "idMal": 1, "format": "TV" })));
        assert!(anime(json!({ "idMal": 1, "format": "MOVIE" })));
        assert!(!anime(json!({ "idMal": 1, "format": "MANGA" })));
        assert!(!anime(json!({ "idMal": 1, "format": "NOVEL" })));
        assert!(!anime(json!({ "idMal": 1, "format": "ONE_SHOT" })));
        // Neither asked: a value only an anime has, and none a manga has.
        assert!(anime(
            json!({ "idMal": 1, "episodes": 26, "chapters": null })
        ));
        assert!(anime(json!({ "idMal": 1, "seasonYear": 1998 })));
        assert!(anime(json!({ "idMal": 1, "duration": 24 })));
        assert!(!anime(json!({ "idMal": 1, "chapters": 162 })));
        assert!(!anime(
            json!({ "idMal": 1, "volumes": 18, "episodes": null })
        ));
        // Nothing a manga could not say.
        assert!(!anime(
            json!({ "idMal": 1, "title": { "userPreferred": "x" } })
        ));
        assert!(!anime(
            json!({ "idMal": 1, "episodes": null, "chapters": null })
        ));
    }

    #[test]
    fn yamtracks_import_patches_the_anime_and_not_the_manga_of_the_same_number() {
        // Yamtrack's import asks both lists in one query, without `type`
        // and without AniList's `id`: the anime with their counts, the manga
        // with nothing but a title, a cover and MyAnimeList's id — whose
        // numbers are the anime's too. Anime 1 is Cowboy Bebop and 21 One
        // Piece; manga 1 is Monster and 21 Death Note.
        let entry = |media: Value| json!({ "media": media, "status": "COMPLETED", "progress": 1 });
        let mut answer = json!({ "data": {
            "anime": { "lists": [{ "isCustomList": false, "entries": [
                entry(json!({ "title": { "userPreferred": "Cowboy Bebop" }, "coverImage": { "large": "https://s4.anilist.co/bebop.jpg" }, "idMal": 1, "chapters": null, "episodes": 26 })),
                entry(json!({ "title": { "userPreferred": "One Piece" }, "coverImage": { "large": "https://s4.anilist.co/op.jpg" }, "idMal": 21, "chapters": null, "episodes": null })),
            ]}]},
            "manga": { "lists": [{ "isCustomList": false, "entries": [
                entry(json!({ "title": { "userPreferred": "Monster" }, "coverImage": { "large": "https://s4.anilist.co/monster.jpg" }, "idMal": 1 })),
                entry(json!({ "title": { "userPreferred": "Death Note" }, "coverImage": { "large": "https://s4.anilist.co/dn.jpg" }, "idMal": 21 })),
            ]}]}
        }});

        let mut named = Named::default();
        collect_entries(&answer["data"], &mut named);
        assert!(named.anilist.is_empty());
        // Only an entry that says it is an anime is looked up by its number.
        assert_eq!(named.mal, vec![1]);

        let mut item = locked_anime();
        item.locked_fields.push("item/primaryPoster".into());
        item.images.push(crate::domain::Image {
            id: "img1".into(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: "https://ams.example/media/chosen".into(),
            language: None,
            sort_order: 0,
            source: None,
            is_manual: true,
        });
        item.primary_images.poster = Some("img1".into());
        assert!(write_entries(
            &mut answer["data"],
            &held(&[], &[1, 21], item),
            NOW
        ));

        let anime = &answer["data"]["anime"]["lists"][0]["entries"];
        assert_eq!(
            anime[0]["media"]["title"]["userPreferred"],
            json!("Mon titre")
        );
        assert_eq!(
            anime[0]["media"]["coverImage"]["large"],
            json!("https://ams.example/media/chosen")
        );
        // Still airing, no count yet, neither type nor format asked:
        // nothing tells it from a manga, so it is left as it came.
        assert_eq!(
            anime[1]["media"]["title"]["userPreferred"],
            json!("One Piece")
        );

        let manga = &answer["data"]["manga"]["lists"][0]["entries"];
        assert_eq!(
            manga[0]["media"]["title"]["userPreferred"],
            json!("Monster")
        );
        assert_eq!(
            manga[0]["media"]["coverImage"]["large"],
            json!("https://s4.anilist.co/monster.jpg")
        );
        assert_eq!(
            manga[1]["media"]["title"]["userPreferred"],
            json!("Death Note")
        );
    }

    #[test]
    fn a_manga_told_by_its_format_is_not_an_entry() {
        for manga in [
            json!({ "id": 30001, "format": "MANGA", "title": { "english": "x" } }),
            json!({ "id": 30001, "format": "NOVEL", "idMal": 1 }),
            json!({ "id": 30001, "type": "MANGA", "title": { "english": "x" } }),
        ] {
            let Value::Object(object) = &manga else {
                unreachable!()
            };
            assert!(!is_entry(object), "{manga}");
        }
        for anime in [
            json!({ "id": 1, "format": "TV" }),
            json!({ "id": 1, "type": "ANIME" }),
            json!({ "idMal": 1, "episodes": null }),
            json!({ "id": 1, "title": { "romaji": "x" } }),
        ] {
            let Value::Object(object) = &anime else {
                unreachable!()
            };
            assert!(is_entry(object), "{anime}");
        }
    }

    #[test]
    fn a_chosen_poster_names_the_address_this_server_serves() {
        let mut item = locked_anime();
        item.locked_fields.push("item/primaryPoster".into());
        item.images.push(crate::domain::Image {
            id: "img1".into(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: "https://ams.example/media/abc".into(),
            language: None,
            sort_order: 0,
            source: None,
            is_manual: true,
        });
        item.primary_images.poster = Some("img1".into());
        let mut object = json!({ "id": 1, "coverImage": { "extraLarge": "https://s4.anilist.co/a.jpg", "color": "#fff" } });
        assert!(write_entry(
            object.as_object_mut().unwrap(),
            &item,
            None,
            true,
            NOW
        ));
        assert_eq!(
            object["coverImage"]["extraLarge"],
            json!("https://ams.example/media/abc")
        );
        assert_eq!(object["coverImage"]["color"], json!("#fff"));
        assert!(object["coverImage"].get("large").is_none());
    }

    // ─── the locks written in this wave ──────────────────────────────────────

    use crate::service::cour::Span;

    fn object(value: Value) -> Map<String, Value> {
        let Value::Object(object) = value else {
            unreachable!()
        };
        object
    }

    #[test]
    fn a_title_hidden_here_leaves_anilists_other_names() {
        let mut item = locked_anime();
        item.hidden.alternative_titles = vec![crate::domain::AlternativeTitle {
            id: "t".into(),
            title: "Hidden One".into(),
            title_type: None,
            language: Some("eng".into()),
            is_manual: false,
        }];
        let mut entry = object(json!({ "id": 1, "synonyms": [" hidden one ", "Kept", 3] }));
        assert!(write_entry(&mut entry, &item, None, true, NOW));
        assert_eq!(entry["synonyms"], json!(["Kept", 3]));
        // Nothing hidden: nothing taken.
        let mut entry = object(json!({ "id": 1, "synonyms": ["Kept"] }));
        assert!(!take_hidden_synonyms(&mut entry, &locked_anime()));
    }

    #[test]
    fn a_locked_adult_flag_is_written_where_it_was_asked() {
        let mut item = locked_anime();
        item.is_adult = true;
        let mut entry = object(json!({ "id": 1, "isAdult": false }));
        write_entry(&mut entry, &item, None, true, NOW);
        assert_eq!(entry["isAdult"], json!(false), "not locked");

        item.locked_fields.push("item/isAdult".into());
        assert!(write_entry(&mut entry, &item, None, true, NOW));
        assert_eq!(entry["isAdult"], json!(true));
        let mut unasked = object(json!({ "id": 1 }));
        write_entry(&mut unasked, &item, None, true, NOW);
        assert!(unasked.get("isAdult").is_none());
    }

    #[test]
    fn a_locked_main_studio_leads_and_never_takes_another_studios_id() {
        let studios = |value: Value| object(value);

        // Listed under that name: put first.
        let mut listed =
            studios(json!({ "nodes": [{ "name": "Aniplex" }, { "name": "madhouse " }] }));
        assert!(write_studio(&mut listed, "Madhouse"));
        assert_eq!(
            listed["nodes"],
            json!([{ "name": "madhouse " }, { "name": "Aniplex" }])
        );

        // Not listed: written over the first, where nothing names it.
        let mut named =
            studios(json!({ "nodes": [{ "name": "Other", "isAnimationStudio": true }] }));
        assert!(write_studio(&mut named, "Madhouse"));
        assert_eq!(
            named["nodes"][0],
            json!({ "name": "Madhouse", "isAnimationStudio": true })
        );
        // …and left as it came where its id was asked.
        let mut kept = studios(json!({ "nodes": [{ "id": 5, "name": "Other" }] }));
        assert!(!write_studio(&mut kept, "Madhouse"));
        assert_eq!(kept["nodes"][0]["name"], json!("Other"));

        // Edges: the one of that name first, and the one main studio.
        let mut edges = studios(json!({ "edges": [
            { "isMain": false, "node": { "id": 1, "name": "Aniplex" } },
            { "isMain": true, "node": { "id": 11, "name": "Other" } },
            { "isMain": false, "node": { "id": 12, "name": "Madhouse" } }
        ]}));
        assert!(write_studio(&mut edges, "Madhouse"));
        assert_eq!(
            edges["edges"],
            json!([
                { "isMain": true, "node": { "id": 12, "name": "Madhouse" } },
                { "isMain": false, "node": { "id": 1, "name": "Aniplex" } },
                { "isMain": false, "node": { "id": 11, "name": "Other" } }
            ])
        );
        let mut unknown = studios(
            json!({ "edges": [{ "isMain": true, "node": { "id": 11, "name": "Other" } }] }),
        );
        assert!(!write_studio(&mut unknown, "Madhouse"));
        let mut bare = studios(json!({ "edges": [
            { "node": { "name": "Aniplex" } },
            { "isMain": true, "node": { "name": "Other" } }
        ]}));
        assert!(write_studio(&mut bare, "Madhouse"));
        assert_eq!(
            bare["edges"],
            json!([
                { "isMain": true, "node": { "name": "Madhouse" } },
                { "node": { "name": "Aniplex" } }
            ])
        );
    }

    /// The second half of a split season, airing, its next episode — the
    /// season's 11th, its own 5th — three hours on.
    fn second_half() -> Cour {
        Cour {
            anilist: Some(3),
            mal: None,
            format: Some("TV".into()),
            span: Some(Span::Season {
                number: 2,
                first: 7,
                last: None,
            }),
            whole: false,
            opens_work: false,
            ends_work: true,
            closed: false,
            episodes: 6,
            aired: 4,
            numbers: Some((7, 12)),
            starts: Some("2026-10-04".into()),
            ends: None,
            status: Some(Status::Releasing),
            quarter: None,
            next: Some(Next {
                episode: 5,
                episode_number: 11,
                airing_at: NOW + 3 * 3600,
                day: "2026-11-01".into(),
            }),
            corrected: true,
            made_here: false,
        }
    }

    fn as_anilist_has_it() -> Map<String, Value> {
        object(json!({
            "id": 3,
            "status": "FINISHED",
            "startDate": { "year": 2026, "month": 10, "day": 5 },
            "endDate": { "year": 2026, "month": 12, "day": 20 },
            "episodes": 12,
            "nextAiringEpisode": { "id": 77, "airingAt": 1, "timeUntilAiring": 1, "episode": 6 }
        }))
    }

    #[test]
    fn a_locked_title_and_overview_are_written_into_the_whole_work_alone() {
        let item = locked_anime();
        let season = || {
            object(json!({
                "id": 3,
                "title": {
                    "romaji": "Shingeki no Kyojin Season 2",
                    "english": "Attack on Titan Season 2",
                    "userPreferred": "Shingeki no Kyojin Season 2"
                },
                "description": "AniList's, of the second season",
                "genres": ["Action"]
            }))
        };
        // One of its seasons or cours: AniList's names and words for it
        // stay; the work's genres are written all the same.
        let mut cour = season();
        assert!(write_entry(
            &mut cour,
            &item,
            Some(&second_half()),
            false,
            NOW
        ));
        assert_eq!(cour["title"]["english"], json!("Attack on Titan Season 2"));
        assert_eq!(
            cour["title"]["userPreferred"],
            json!("Shingeki no Kyojin Season 2")
        );
        assert_eq!(
            cour["description"],
            json!("AniList's, of the second season")
        );
        assert_eq!(cour["genres"], json!(["Drame"]));
        // The whole work: the locked ones.
        let mut whole = season();
        assert!(write_entry(&mut whole, &item, None, true, NOW));
        assert_eq!(whole["title"]["english"], json!("Mon titre"));
        assert_eq!(
            whole["title"]["romaji"],
            json!("Shingeki no Kyojin Season 2")
        );
        assert_eq!(whole["description"], json!("Mon résumé"));
    }

    #[test]
    fn an_entry_is_the_whole_work_unless_the_list_splits_it_otherwise() {
        let entry = |id: i64| object(json!({ "id": id, "type": "ANIME" }));
        // A work the list does not split: any entry of it stands for it.
        let unsplit = held(&[1], &[], locked_anime());
        assert!(
            unsplit
                .of(&entry(1))
                .is_some_and(|(_, cour, whole)| cour.is_none() && whole)
        );
        // Split: the half of a season is not the whole work, the entry the
        // list keeps whole is, and one it files beside them without placing
        // it is not either.
        let mut kept_whole = second_half();
        kept_whole.anilist = Some(4);
        kept_whole.whole = true;
        let split = held_split(
            &[3, 4, 5],
            &[],
            locked_anime(),
            vec![second_half(), kept_whole],
        );
        assert!(
            split
                .of(&entry(3))
                .is_some_and(|(_, cour, whole)| cour.is_some() && !whole)
        );
        assert!(split.of(&entry(4)).is_some_and(|(_, _, whole)| whole));
        assert!(
            split
                .of(&entry(5))
                .is_some_and(|(_, cour, whole)| cour.is_none() && !whole)
        );
    }

    #[test]
    fn a_cour_whose_schedule_was_corrected_here_is_written_as_its_episodes_say() {
        let item = locked_anime();
        let mut entry = as_anilist_has_it();
        assert!(write_entry(
            &mut entry,
            &item,
            Some(&second_half()),
            false,
            NOW
        ));
        assert_eq!(entry["status"], json!("RELEASING"));
        assert_eq!(
            entry["startDate"],
            json!({ "year": 2026, "month": 10, "day": 4 })
        );
        // Not finished: its end and its count are AniList's.
        assert_eq!(entry["endDate"]["day"], json!(20));
        assert_eq!(entry["episodes"], json!(12));
        assert_eq!(
            entry["nextAiringEpisode"],
            json!({ "id": 77, "airingAt": NOW + 10_800, "timeUntilAiring": 10_800, "episode": 5 })
        );

        // Nothing corrected: AniList's, all of it.
        let untouched = Cour {
            corrected: false,
            ..second_half()
        };
        let mut entry = as_anilist_has_it();
        assert!(!write_airing(&mut entry, &item, &untouched, NOW));
        assert_eq!(entry, as_anilist_has_it());

        // Finished, with an episode made here: its end, its count, nothing
        // more to air.
        let finished = Cour {
            closed: true,
            made_here: true,
            episodes: 7,
            status: Some(Status::Finished),
            ends: Some("2026-11-15".into()),
            next: None,
            ..second_half()
        };
        let mut entry = as_anilist_has_it();
        assert!(write_airing(&mut entry, &item, &finished, NOW));
        assert_eq!(
            entry["endDate"],
            json!({ "year": 2026, "month": 11, "day": 15 })
        );
        assert_eq!(entry["episodes"], json!(7));
        assert_eq!(entry["nextAiringEpisode"], Value::Null);
        assert_eq!(entry["status"], json!("FINISHED"));
    }

    #[test]
    fn the_works_own_dates_and_status_say_what_they_can_of_each_entry() {
        let mut item = locked_anime();
        item.status = Some("ended".into());
        item.first_aired = Some("2026-01-02".into());
        item.last_aired = Some("2026-11-08".into());
        item.locked_fields.extend([
            "item/firstAired".into(),
            "item/lastAired".into(),
            "item/status".into(),
        ]);
        let untouched = |cour: Cour| Cour {
            corrected: false,
            ..cour
        };

        // The entry that closes it: its end, and finished.
        let mut last = as_anilist_has_it();
        assert!(write_airing(
            &mut last,
            &item,
            &untouched(second_half()),
            NOW
        ));
        assert_eq!(last["status"], json!("FINISHED"));
        assert_eq!(
            last["endDate"],
            json!({ "year": 2026, "month": 11, "day": 8 })
        );
        assert_eq!(
            last["startDate"]["day"],
            json!(5),
            "it does not open the work"
        );
        assert_eq!(last["nextAiringEpisode"], Value::Null);

        // The one that opens it: its start.
        let opening = untouched(Cour {
            opens_work: true,
            ends_work: false,
            ..second_half()
        });
        let mut first = as_anilist_has_it();
        assert!(write_airing(&mut first, &item, &opening, NOW));
        assert_eq!(
            first["startDate"],
            json!({ "year": 2026, "month": 1, "day": 2 })
        );
        assert_eq!(first["endDate"]["month"], json!(12), "it does not close it");

        // Continuing says something of the whole work alone.
        item.status = Some("continuing".into());
        let mut part = as_anilist_has_it();
        write_airing(&mut part, &item, &untouched(second_half()), NOW);
        assert_eq!(part["status"], json!("FINISHED"), "AniList's");
        assert_eq!(
            part["endDate"]["month"],
            json!(12),
            "a work going on has no end"
        );
        let whole = untouched(Cour {
            whole: true,
            ..second_half()
        });
        let mut all = as_anilist_has_it();
        write_airing(&mut all, &item, &whole, NOW);
        assert_eq!(all["status"], json!("RELEASING"));

        // A film's day in cinemas is its start and its end.
        let mut film = locked_anime();
        film.kind = MediaKind::Movie;
        film.in_cinemas = Some("2001-07-20".into());
        film.locked_fields.push("item/inCinemas".into());
        let mut entry = as_anilist_has_it();
        assert!(write_airing(&mut entry, &film, &whole, NOW));
        assert_eq!(
            entry["startDate"],
            json!({ "year": 2001, "month": 7, "day": 20 })
        );
        assert_eq!(entry["endDate"], entry["startDate"]);
    }

    #[tokio::test]
    async fn each_entry_of_a_split_series_is_written_as_the_cour_it_is() {
        use chrono::{Datelike as _, TimeDelta, Utc};

        use crate::domain::fields::Scope;

        let state = server("http://127.0.0.1:9").await;
        let now = Utc::now();
        let at = |days: i64| now + TimeDelta::days(days);
        let mut item = MediaItem::empty(MediaKind::Series);
        item.id = crate::db::new_id();
        item.title = "A cour-split series the relay writes".into();
        item.slug = crate::domain::make_slug(&item.title, None);
        item.created_at = crate::db::now();
        item.updated_at = crate::db::now();
        item.status = Some("continuing".into());
        item.external_ids.tvdb = Some(9_900_101);
        item.external_ids.anilist = vec![9_900_111, 9_900_112];
        item.alternative_titles = vec![crate::domain::AlternativeTitle {
            id: crate::db::new_id(),
            title: "Hidden one".into(),
            title_type: None,
            language: Some("eng".into()),
            is_manual: false,
        }];
        // Six weeks in summer; then three weekly up to a week ago, and
        // three from three days on.
        item.episodes = (1..=12)
            .map(|n| {
                let day = match n {
                    1..=6 => at(-140 + 7 * n),
                    7..=9 => at(-70 + 7 * n),
                    _ => at(-67 + 7 * n),
                };
                let mut episode = crate::db::repo::child::blank_episode(1, n as i32);
                episode.is_manual = false;
                episode.air_date = Some(day.format("%Y-%m-%d").to_string());
                episode.air_date_utc = Some(day.format("%Y-%m-%dT%H:%M:%SZ").to_string());
                episode
            })
            .collect();
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        repo::anime::replace_all(
            &state.db,
            &[9_900_111, 9_900_112].map(|anilist| repo::anime::Entry {
                anilist_id: Some(anilist),
                tvdb_id: Some(9_900_101),
                tvdb_season: Some(1),
                tvdb_offset: (anilist == 9_900_112).then_some(6),
                kind: Some("TV".into()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        // The tenth episode — the second half's fourth — moved a day earlier.
        let moved = (at(2) + TimeDelta::hours(1))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        repo::override_field::set(
            &state.db,
            &item.id,
            Scope::Episode {
                season: 1,
                episode: 10,
            },
            "airDateUtc",
            Some(&json!(moved)),
            None,
        )
        .await
        .unwrap();
        repo::override_field::set(
            &state.db,
            &item.id,
            Scope::Item,
            "isAdult",
            Some(&json!(true)),
            None,
        )
        .await
        .unwrap();
        repo::override_field::set_in(
            &state.db,
            &item.id,
            "alternativeTitle:eng:Hidden one",
            "hidden",
            "",
            Some(&json!(true)),
            None,
        )
        .await
        .unwrap();

        let first = json!({ "id": 9_900_111, "type": "ANIME", "status": "RELEASING",
            "startDate": { "year": 2000, "month": 1, "day": 1 }, "nextAiringEpisode": null });
        let mut answer = json!({ "data": { "Page": { "media": [
            first.clone(),
            { "id": 9_900_112, "type": "ANIME", "status": "NOT_YET_RELEASED", "isAdult": false,
              "startDate": { "year": 2000, "month": 1, "day": 1 },
              "endDate": { "year": null, "month": null, "day": null }, "episodes": 99,
              "nextAiringEpisode": { "id": 1, "airingAt": 0, "timeUntilAiring": 0, "episode": 9 },
              "synonyms": ["Hidden one", "Kept"] }
        ]}}});
        assert!(patch(&state, &mut answer).await.unwrap());

        let media = &answer["data"]["Page"]["media"];
        // The first half: nothing of it was corrected, and it is AniList's.
        assert_eq!(media[0], first);
        let second = &media[1];
        assert_eq!(second["status"], json!("RELEASING"));
        let started = at(-21);
        assert_eq!(
            second["startDate"],
            json!({ "year": started.year(), "month": started.month(), "day": started.day() })
        );
        assert_eq!(second["endDate"]["year"], Value::Null);
        assert_eq!(second["episodes"], json!(99), "its count is still growing");
        let moved_at = chrono::DateTime::parse_from_rfc3339(&moved)
            .unwrap()
            .timestamp();
        assert_eq!(second["nextAiringEpisode"]["airingAt"], json!(moved_at));
        assert_eq!(second["nextAiringEpisode"]["episode"], json!(4));
        assert_eq!(second["nextAiringEpisode"]["id"], json!(1), "AniList's own");
        assert_eq!(second["isAdult"], json!(true));
        assert_eq!(second["synonyms"], json!(["Kept"]));
    }
}
