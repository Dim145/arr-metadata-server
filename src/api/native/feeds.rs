//! Feeds: the schedule as a calendar a phone subscribes to, and what arrives
//! and airs as feeds a reader follows.
//!
//! An iCalendar (RFC 5545) of the episodes airing, in a window measured the
//! way Sonarr's own feed measures it — days behind and days ahead — and one
//! per work; Atom (RFC 4287) of the works recently added and of the week's
//! episodes. Read under the same credentials as the pages they mirror: a
//! visitor's under public browsing, otherwise a key, which a calendar app can
//! carry in the query string.

use std::{collections::HashMap, net::SocketAddr};

use axum::{
    Extension,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::{router::OpenApiRouter, routes};

use super::browse::{self, Airing, Calendar};
use crate::{
    auth::{Identity, ip},
    db::repo,
    domain::{Episode, MediaItem, MediaKind},
    error::{AppError, AppResult},
    service,
    state::AppState,
};

/// Filed with the schedule in the documentation.
const TAG: &str = super::items::TAG;

/// How long a reader may keep a feed before asking again.
const CACHE_CONTROL: &str = "private, max-age=900";

/// The most days a calendar feed spans, behind and ahead together: the
/// schedule's own limit.
const MAX_DAYS: i64 = 62;

/// How many works the feed of arrivals carries.
const ADDED: i64 = 50;

/// The socket a request came from, as the listener recorded it.
type Peer = Option<Extension<ConnectInfo<SocketAddr>>>;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(calendar_ics))
        .routes(routes!(work_ics))
        .routes(routes!(added_atom))
        .routes(routes!(airing_atom))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct FeedQuery {
    /// Days behind today to include. 7 when absent, as Sonarr's feed has it.
    pub past_days: Option<i64>,
    /// Days ahead of today to include. 28 when absent.
    pub future_days: Option<i64>,
    /// Titles in this language, where a translation is held; the server's
    /// own when absent.
    pub language: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LanguageQuery {
    /// Titles in this language, where a translation is held; the server's
    /// own when absent.
    pub language: Option<String>,
}

// ─── the schedule, as a calendar ─────────────────────────────────────────────

/// The schedule as an iCalendar: every episode airing in a window of days
/// behind and ahead, an event each, timed where a provider knew the time and
/// all-day otherwise. Subscribe a calendar app to it — as `webcal://` — and
/// the week's episodes are in it, refreshed as the app sees fit.
#[utoipa::path(
    get, path = "/calendar.ics", tag = TAG,
    params(FeedQuery),
    responses(
        (status = 200, description = "An iCalendar document, as text/calendar"),
        (status = 400, description = "The window is longer than 62 days"),
    ),
)]
async fn calendar_ics(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    peer: Peer,
    headers: HeaderMap,
    Query(query): Query<FeedQuery>,
) -> AppResult<Response> {
    let (from, to) = window_of(query.past_days, query.future_days)?;
    let language = language_for(&state, &identity, query.language.as_deref());
    let calendar = browse::window(&state, &identity, from, to, Some(&language)).await?;

    let origin = origin_of(&state, &headers, peer);
    let works = works_by_id(&calendar);
    let events: Vec<Event> = calendar
        .episodes
        .iter()
        .filter_map(|Airing { work_id, episode }| {
            episode_event(&origin, works.get(work_id.as_str())?, episode)
        })
        .collect();

    Ok(ics_response(
        document("Cinémathèque", &events),
        calendar.truncated,
    ))
}

/// One work as a calendar: a series' dated episodes, all of them, or a
/// film's release day.
#[utoipa::path(
    get, path = "/items/{id}/calendar.ics", tag = TAG,
    params(("id" = String, Path, description = "The work's id"), LanguageQuery),
    responses(
        (status = 200, description = "An iCalendar document, as text/calendar"),
        (status = 404, description = "No such work, or none this caller may see"),
    ),
)]
async fn work_ics(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    peer: Peer,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<LanguageQuery>,
) -> AppResult<Response> {
    let mut item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    // As the work's own page decides it: switched off, or kept from this
    // caller by the adult policy, it is not here either.
    let hidden = !item.is_enabled
        || (item.is_adult
            && !state.adult_for(identity.client_id(), identity.peer_id(), Some(true)));
    if hidden && !identity.can_write() {
        return Err(AppError::NotFound);
    }

    let language = language_for(&state, &identity, query.language.as_deref());
    // The stored translations only: a feed is not the moment to fetch a
    // season's worth of episode text.
    service::language::apply_stored(&state, &mut item, &language).await?;
    service::redact_for_reader(&identity, std::slice::from_mut(&mut item));

    let origin = origin_of(&state, &headers, peer);
    let events: Vec<Event> = match item.kind {
        MediaKind::Series => item
            .episodes
            .iter()
            .filter(|e| e.season_number > 0)
            .filter_map(|e| episode_event(&origin, &item, e))
            .collect(),
        MediaKind::Movie => release_event(&origin, &item).into_iter().collect(),
    };

    Ok(ics_response(document(&item.title, &events), false))
}

// ─── what arrives and what airs, as feeds ────────────────────────────────────

/// The works most recently added to the catalogue, as an Atom feed.
#[utoipa::path(
    get, path = "/feed/added.atom", tag = TAG,
    params(LanguageQuery),
    responses((status = 200, description = "An Atom feed, as application/atom+xml")),
)]
async fn added_atom(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    peer: Peer,
    headers: HeaderMap,
    Query(query): Query<LanguageQuery>,
) -> AppResult<Response> {
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
    let mut works = repo::item::search(
        &state.db,
        &repo::item::Query {
            sort: repo::item::Sort::Added,
            descending: None,
            limit: ADDED,
            include_adult: adult,
            ..Default::default()
        },
    )
    .await?;
    service::apply_overrides(&state, &mut works).await?;
    // The stored translations come with the artwork; without them every work
    // keeps its own title whatever language is asked.
    repo::item::load_artwork(&state.db, &mut works).await?;
    let language = language_for(&state, &identity, query.language.as_deref());
    for work in &mut works {
        service::language::apply_shallow(&state, work, &language);
    }
    service::redact_for_reader(&identity, &mut works);

    let origin = origin_of(&state, &headers, peer);
    let now = now_rfc3339();
    let entries: Vec<Entry> = works
        .iter()
        .map(|work| Entry {
            id: format!("urn:ams:work:{}", work.id),
            title: titled(work),
            link: format!("{origin}/work/{}", work.id),
            updated: rfc3339(&work.created_at)
                .filter(|at| *at <= now)
                .unwrap_or_else(|| now.clone()),
            summary: work.overview.clone(),
            categories: work.genres.clone(),
        })
        .collect();
    let updated = latest(&entries).unwrap_or(now);

    Ok(atom_response(
        atom(&Feed {
            origin: &origin,
            path: "/api/v1/feed/added.atom",
            title: "Cinémathèque — recently added",
            updated: &updated,
            language: language_tag(&language).as_deref(),
            entries: &entries,
        }),
        false,
    ))
}

/// The episodes airing from yesterday to a week ahead, as an Atom feed.
#[utoipa::path(
    get, path = "/feed/airing.atom", tag = TAG,
    params(LanguageQuery),
    responses((status = 200, description = "An Atom feed, as application/atom+xml")),
)]
async fn airing_atom(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    peer: Peer,
    headers: HeaderMap,
    Query(query): Query<LanguageQuery>,
) -> AppResult<Response> {
    let (from, to) = window_of(Some(1), Some(7))?;
    let language = language_for(&state, &identity, query.language.as_deref());
    let calendar = browse::window(&state, &identity, from, to, Some(&language)).await?;

    let origin = origin_of(&state, &headers, peer);
    let now = now_rfc3339();
    let works = works_by_id(&calendar);
    let entries: Vec<Entry> = calendar
        .episodes
        .iter()
        .filter_map(|Airing { work_id, episode }| {
            let work = works.get(work_id.as_str())?;
            let event = episode_event(&origin, work, episode)?;
            // An episode still to come is dated by its work's last change
            // rather than ahead: a reader may hide what is dated in the
            // future, and a date that moved every second would never let a
            // poll be answered "unchanged".
            let aired = browse::aired_at(episode)?
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string();
            let updated = if aired > now {
                rfc3339(&work.updated_at).unwrap_or_else(|| now.clone())
            } else {
                aired
            };
            Some(Entry {
                id: format!("urn:ams:episode:{}", episode.id),
                title: event.summary,
                link: event.url,
                updated,
                summary: episode.overview.clone(),
                categories: event.categories,
            })
        })
        .collect();
    let updated = latest(&entries).unwrap_or(now);

    Ok(atom_response(
        atom(&Feed {
            origin: &origin,
            path: "/api/v1/feed/airing.atom",
            title: "Cinémathèque — on the air",
            updated: &updated,
            language: language_tag(&language).as_deref(),
            entries: &entries,
        }),
        calendar.truncated,
    ))
}

// ─── the window, the origin, the language ────────────────────────────────────

/// Today less `past` days at midnight UTC, to today plus `future` days at the
/// end of that day: what a calendar app is shown of the schedule.
fn window_of(past: Option<i64>, future: Option<i64>) -> AppResult<(DateTime<Utc>, DateTime<Utc>)> {
    let past = past.unwrap_or(7);
    let future = future.unwrap_or(28);
    // Each bounded on its own before they are added: the sum of two huge
    // numbers wraps, and a wrapped sum is small.
    let bounded = |days: i64| (0..=MAX_DAYS).contains(&days);
    if !bounded(past) || !bounded(future) || past + future > MAX_DAYS {
        return Err(AppError::BadRequest(format!(
            "pastDays and futureDays may not be negative, and together may not exceed {MAX_DAYS}"
        )));
    }
    let today = Utc::now().date_naive();
    let from = (today - Duration::days(past))
        .and_hms_opt(0, 0, 0)
        .unwrap_or_default()
        .and_utc();
    let to = (today + Duration::days(future + 1))
        .and_hms_opt(0, 0, 0)
        .unwrap_or_default()
        .and_utc();
    Ok((from, to))
}

/// The language the feed is written in: the one asked for, or the one this
/// caller is answered in everywhere else.
///
/// One asked for that is not a language ([`service::language::tag`]) is as
/// one not asked for, rather than a refusal: a feed's address is pasted into
/// a calendar app or a reader once and polled for years, and neither shows
/// an error to anybody — the feed would just stop. Nothing here is fetched
/// for any language, only what is held laid over.
fn language_for(state: &AppState, identity: &Identity, asked: Option<&str>) -> String {
    asked
        .and_then(service::language::tag)
        .unwrap_or_else(|| state.language(identity.client_id(), identity.peer_id()))
}

/// A language as `xml:lang` takes it — `fr`, `pt-BR` — or nothing, rather
/// than whatever was asked for.
fn language_tag(raw: &str) -> Option<String> {
    let tag = raw.trim();
    let mut parts = tag.split('-');
    let primary = parts.next()?;
    let well_formed = (2..=3).contains(&primary.len())
        && primary.bytes().all(|b| b.is_ascii_alphabetic())
        && parts
            .all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()));
    well_formed.then(|| tag.to_string())
}

/// Where this request's feed points: the public address, or the one the
/// request came to, as the proxy forwarded it if the peer is one the operator
/// trusts — the same rule the client address follows.
fn origin_of(state: &AppState, headers: &HeaderMap, peer: Peer) -> String {
    let forwarded = ip::is_trusted_peer(
        peer.map(|Extension(ConnectInfo(addr))| addr),
        &state.config.server.trusted_proxies,
    );
    origin(
        state.config.server.public_url.as_deref(),
        headers,
        forwarded,
    )
}

/// Where the links in a feed point: the public address, or failing that the
/// host and scheme this request came to — from the forwarded headers only
/// when `forwarded` says they may be believed. The host is taken only in the
/// shape a host has; anything else would be a caller writing into the
/// document, and is `localhost` instead.
fn origin(public_url: Option<&str>, headers: &HeaderMap, forwarded: bool) -> String {
    if let Some(url) = public_url {
        return url.to_string();
    }
    // The first of a comma-separated value: each proxy appends its own.
    let value = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let host = forwarded
        .then(|| value("x-forwarded-host"))
        .flatten()
        .or_else(|| value(header::HOST.as_str()))
        .filter(|h| is_host(h))
        .unwrap_or_else(|| "localhost".to_string());
    let scheme = match forwarded.then(|| value("x-forwarded-proto")).flatten() {
        Some(proto) if proto.eq_ignore_ascii_case("https") => "https",
        _ => "http",
    };
    format!("{scheme}://{host}")
}

/// `films.example`, `films.example:8443`, `10.0.0.7`, `[::1]:8080`: a host
/// as a URL carries one, with a port or without, and nothing else.
fn is_host(text: &str) -> bool {
    if let Some(rest) = text.strip_prefix('[') {
        let Some((address, tail)) = rest.split_once(']') else {
            return false;
        };
        let literal = !address.is_empty()
            && address.len() <= 45
            && address
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'.'));
        return literal && (tail.is_empty() || tail.strip_prefix(':').is_some_and(is_port));
    }
    let (name, port) = match text.rsplit_once(':') {
        Some((name, port)) => (name, Some(port)),
        None => (text, None),
    };
    let named = !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    named && port.is_none_or(is_port)
}

/// A port as a URL carries one: a number, and not nought.
fn is_port(text: &str) -> bool {
    !text.is_empty() && text.len() <= 5 && text.parse::<u16>().is_ok_and(|n| n > 0)
}

fn now_rfc3339() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// A stored moment as iCalendar writes one, or the present for a moment
/// that does not read.
fn stamp_of(raw: &str) -> String {
    rfc3339(raw)
        .and_then(|at| DateTime::parse_from_rfc3339(&at).ok())
        .map(|at| at.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

/// A stored moment as Atom wants it: RFC 3339, in UTC. The store writes
/// instants in more than one shape; a feed reader forgives none of them.
fn rfc3339(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let instant = DateTime::parse_from_rfc3339(raw)
        .map(|t| t.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            [
                "%Y-%m-%d %H:%M:%S%.f",
                "%Y-%m-%dT%H:%M:%S%.f",
                "%Y-%m-%d %H:%M:%S",
                "%Y-%m-%dT%H:%M:%S",
            ]
            .iter()
            .find_map(|f| chrono::NaiveDateTime::parse_from_str(raw, f).ok())
            .map(|t| t.and_utc())
        })?;
    Some(instant.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// The works of a window, by id, for the episodes to find theirs.
fn works_by_id(calendar: &Calendar) -> HashMap<&str, &MediaItem> {
    calendar.works.iter().map(|w| (w.id.as_str(), w)).collect()
}

/// A work's title with its year, as a list names it.
fn titled(work: &MediaItem) -> String {
    match work.year {
        Some(year) => format!("{} ({year})", work.title),
        None => work.title.clone(),
    }
}

// ─── iCalendar ───────────────────────────────────────────────────────────────

/// When an event is: a day, or an instant.
enum When {
    Day(NaiveDate),
    Instant(DateTime<Utc>),
}

struct Event {
    uid: String,
    /// When the event was last revised: the work's own last change, so the
    /// document holds still between changes and a poll can be answered
    /// "unchanged".
    stamp: String,
    start: When,
    end: When,
    summary: String,
    description: Option<String>,
    url: String,
    categories: Vec<String>,
}

/// One episode as an event, where it has a day at all.
fn episode_event(origin: &str, work: &MediaItem, episode: &Episode) -> Option<Event> {
    let (start, end) = match episode
        .air_date_utc
        .as_deref()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
    {
        Some(at) => {
            let at = at.with_timezone(&Utc);
            let minutes = episode
                .runtime
                .or(work.runtime)
                .filter(|m| *m > 0)
                .unwrap_or(30);
            (
                When::Instant(at),
                When::Instant(at + Duration::minutes(i64::from(minutes))),
            )
        }
        None => {
            let day =
                NaiveDate::parse_from_str(episode.air_date.as_deref()?.get(..10)?, "%Y-%m-%d")
                    .ok()?;
            (When::Day(day), When::Day(day + Duration::days(1)))
        }
    };

    let code = format!(
        "S{:02}E{:02}",
        episode.season_number, episode.episode_number
    );
    let summary = match episode.title.trim() {
        "" => format!("{} — {code}", work.title),
        title => format!("{} — {code} · {title}", work.title),
    };

    Some(Event {
        uid: format!("episode-{}@arr-metadata-server", episode.id),
        stamp: stamp_of(&work.updated_at),
        start,
        end,
        summary,
        description: episode.overview.clone(),
        url: format!(
            "{origin}/work/{}/season/{}/episode/{}",
            work.id, episode.season_number, episode.episode_number
        ),
        categories: work.network.iter().cloned().collect(),
    })
}

/// A film's release as an all-day event: in cinemas, or failing that its
/// digital or physical release.
fn release_event(origin: &str, work: &MediaItem) -> Option<Event> {
    let day = [
        &work.in_cinemas,
        &work.digital_release,
        &work.physical_release,
    ]
    .into_iter()
    .flatten()
    .find_map(|d| NaiveDate::parse_from_str(d.get(..10)?, "%Y-%m-%d").ok())?;

    Some(Event {
        uid: format!("release-{}@arr-metadata-server", work.id),
        stamp: stamp_of(&work.updated_at),
        start: When::Day(day),
        end: When::Day(day + Duration::days(1)),
        summary: titled(work),
        description: work.overview.clone(),
        url: format!("{origin}/work/{}", work.id),
        categories: work.studio.iter().cloned().collect(),
    })
}

/// The whole document, lines folded and joined as the format requires.
fn document(name: &str, events: &[Event]) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//arr-metadata-server//Cinémathèque//EN".to_string(),
        "CALSCALE:GREGORIAN".to_string(),
        "METHOD:PUBLISH".to_string(),
        format!("X-WR-CALNAME:{}", escape(name)),
        "REFRESH-INTERVAL;VALUE=DURATION:PT6H".to_string(),
        "X-PUBLISHED-TTL:PT6H".to_string(),
    ];
    for event in events {
        lines.push("BEGIN:VEVENT".to_string());
        lines.push(format!("UID:{}", event.uid));
        lines.push(format!("DTSTAMP:{}", event.stamp));
        lines.push(when("DTSTART", &event.start));
        lines.push(when("DTEND", &event.end));
        lines.push(format!("SUMMARY:{}", escape(&event.summary)));
        if let Some(description) = event
            .description
            .as_deref()
            .filter(|d| !d.trim().is_empty())
        {
            lines.push(format!("DESCRIPTION:{}", escape(description)));
        }
        lines.push(format!("URL:{}", event.url));
        if !event.categories.is_empty() {
            let categories: Vec<String> = event.categories.iter().map(|c| escape(c)).collect();
            lines.push(format!("CATEGORIES:{}", categories.join(",")));
        }
        // Free time, not busy: a programme, not an appointment.
        lines.push("TRANSP:TRANSPARENT".to_string());
        lines.push("END:VEVENT".to_string());
    }
    lines.push("END:VCALENDAR".to_string());

    let mut out = String::new();
    for line in lines {
        out.push_str(&fold(&line));
        out.push_str("\r\n");
    }
    out
}

fn when(property: &str, at: &When) -> String {
    match at {
        When::Day(day) => format!("{property};VALUE=DATE:{}", day.format("%Y%m%d")),
        When::Instant(t) => format!("{property}:{}", t.format("%Y%m%dT%H%M%SZ")),
    }
}

/// Text as a property value: the characters the format reserves escaped, and
/// the control characters it has no place for left out.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.replace("\r\n", "\n").chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => {
                out.push('\\');
                out.push(';');
            }
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// A content line no longer than 75 octets, continued on lines that start
/// with a space, cut between characters rather than through one.
fn fold(line: &str) -> String {
    const WIDTH: usize = 75;
    if line.len() <= WIDTH {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + line.len() / WIDTH * 3);
    let mut room = WIDTH;
    for c in line.chars() {
        let len = c.len_utf8();
        if len > room {
            out.push_str("\r\n ");
            // The continuation's leading space counts towards its 75.
            room = WIDTH - 1;
        }
        out.push(c);
        room -= len;
    }
    out
}

fn ics_response(body: String, truncated: bool) -> Response {
    respond("text/calendar; charset=utf-8", body, truncated)
}

// ─── Atom ────────────────────────────────────────────────────────────────────

struct Entry {
    id: String,
    title: String,
    link: String,
    updated: String,
    summary: Option<String>,
    categories: Vec<String>,
}

struct Feed<'a> {
    origin: &'a str,
    path: &'a str,
    title: &'a str,
    updated: &'a str,
    language: Option<&'a str>,
    entries: &'a [Entry],
}

/// The most recent moment among the entries: the feed's own.
fn latest(entries: &[Entry]) -> Option<String> {
    entries.iter().map(|e| e.updated.clone()).max()
}

fn atom(feed: &Feed<'_>) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    match feed.language {
        Some(lang) => out.push_str(&format!(
            "<feed xmlns=\"http://www.w3.org/2005/Atom\" xml:lang=\"{}\">\n",
            xml(lang)
        )),
        None => out.push_str("<feed xmlns=\"http://www.w3.org/2005/Atom\">\n"),
    }
    let (origin, path) = (xml(feed.origin), xml(feed.path));
    out.push_str(&format!("  <title>{}</title>\n", xml(feed.title)));
    out.push_str(&format!("  <id>{origin}{path}</id>\n"));
    out.push_str(&format!("  <link rel=\"self\" href=\"{origin}{path}\"/>\n"));
    out.push_str(&format!("  <link rel=\"alternate\" href=\"{origin}/\"/>\n"));
    out.push_str(&format!("  <updated>{}</updated>\n", xml(feed.updated)));
    // A feed must name an author, or every entry must; the catalogue is
    // the author of its own listings.
    out.push_str("  <author><name>Cinémathèque</name></author>\n");
    out.push_str("  <generator>arr-metadata-server</generator>\n");
    for entry in feed.entries {
        out.push_str("  <entry>\n");
        out.push_str(&format!("    <id>{}</id>\n", xml(&entry.id)));
        out.push_str(&format!("    <title>{}</title>\n", xml(&entry.title)));
        out.push_str(&format!(
            "    <link rel=\"alternate\" href=\"{}\"/>\n",
            xml(&entry.link)
        ));
        out.push_str(&format!("    <updated>{}</updated>\n", xml(&entry.updated)));
        if let Some(summary) = entry.summary.as_deref().filter(|s| !s.trim().is_empty()) {
            out.push_str(&format!("    <summary>{}</summary>\n", xml(summary)));
        }
        for category in &entry.categories {
            out.push_str(&format!("    <category term=\"{}\"/>\n", xml(category)));
        }
        out.push_str("  </entry>\n");
    }
    out.push_str("</feed>\n");
    out
}

/// Text inside an element or an attribute: the five characters XML reserves
/// escaped, and the characters it does not admit at all left out.
fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if c.is_control() || matches!(c, '\u{FFFE}' | '\u{FFFF}') => {}
            c => out.push(c),
        }
    }
    out
}

fn atom_response(body: String, truncated: bool) -> Response {
    respond("application/atom+xml; charset=utf-8", body, truncated)
}

fn respond(content_type: &'static str, body: String, truncated: bool) -> Response {
    let mut response = (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, CACHE_CONTROL),
        ],
        body,
    )
        .into_response();
    if truncated {
        // The window held more episodes than one answer carries, and the
        // latest are missing: said here, since neither format has a line
        // for it that a reader would show.
        response
            .headers_mut()
            .insert("x-ams-truncated", HeaderValue::from_static("true"));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::child::blank_episode;

    #[test]
    fn the_reserved_characters_are_escaped_and_the_controls_dropped() {
        assert_eq!(
            escape("a;b,c\\d\r\ne"),
            "a\\".to_string() + ";b\\,c\\\\d\\ne"
        );
        assert_eq!(escape("lone\rreturn"), "lonereturn");
        assert_eq!(escape("tab\tkept\u{0}bell\u{7}gone"), "tab\tkeptbellgone");

        assert_eq!(xml("a\u{0}b\u{B}c\td\u{FFFE}"), "abc\td");
    }

    #[test]
    fn long_lines_are_folded_between_characters() {
        let check = |line: &str| {
            let folded = fold(line);
            for (index, piece) in folded.split("\r\n").enumerate() {
                assert!(piece.len() <= 75, "piece {index} is {} octets", piece.len());
                assert!(index == 0 || piece.starts_with(' '), "piece {index}");
                assert!(std::str::from_utf8(piece.as_bytes()).is_ok());
            }
            assert_eq!(folded.replace("\r\n ", ""), line);
            folded
        };
        check(&format!("SUMMARY:{}", "é".repeat(60)));
        // A four-byte character that does not fit the room left moves whole.
        let folded = check(&format!("SUMMARY:{}😀", "a".repeat(66)));
        assert!(folded.starts_with(&format!("SUMMARY:{}\r\n 😀", "a".repeat(66))));
        // Exactly the width is left alone; one more is cut.
        assert_eq!(fold(&"x".repeat(75)), "x".repeat(75));
        assert_eq!(fold(&"x".repeat(76)), format!("{}\r\n x", "x".repeat(75)));
        assert_eq!(fold("short"), "short");
    }

    #[test]
    fn an_episode_is_timed_when_its_moment_is_known_and_all_day_otherwise() {
        let mut work = MediaItem::empty(MediaKind::Series);
        work.id = "w1".into();
        work.title = "Dark".into();
        work.runtime = Some(60);
        work.network = Some("Netflix".into());

        let mut timed = blank_episode(2, 3);
        timed.id = "e1".into();
        timed.title = "Alibi".into();
        timed.air_date = Some("2019-06-21".into());
        timed.air_date_utc = Some("2019-06-21T07:00:00Z".into());
        let event = episode_event("https://films.example", &work, &timed).unwrap();
        assert_eq!(event.summary, "Dark — S02E03 · Alibi");
        assert_eq!(when("DTSTART", &event.start), "DTSTART:20190621T070000Z");
        assert_eq!(when("DTEND", &event.end), "DTEND:20190621T080000Z");
        assert_eq!(
            event.url,
            "https://films.example/work/w1/season/2/episode/3"
        );
        assert_eq!(event.categories, ["Netflix"]);

        // No runtime worth the name: half an hour.
        timed.runtime = Some(0);
        work.runtime = Some(-5);
        let event = episode_event("https://films.example", &work, &timed).unwrap();
        assert_eq!(when("DTEND", &event.end), "DTEND:20190621T073000Z");

        let mut dated = blank_episode(2, 4);
        dated.id = "e2".into();
        dated.air_date = Some("2019-06-28".into());
        let event = episode_event("https://films.example", &work, &dated).unwrap();
        assert_eq!(when("DTSTART", &event.start), "DTSTART;VALUE=DATE:20190628");
        assert_eq!(when("DTEND", &event.end), "DTEND;VALUE=DATE:20190629");
        assert_eq!(event.summary, "Dark — S02E04");

        let undated = blank_episode(2, 5);
        assert!(episode_event("https://films.example", &work, &undated).is_none());
        let mut short = blank_episode(2, 6);
        short.air_date = Some("2019".into());
        assert!(episode_event("https://films.example", &work, &short).is_none());
        let mut odd = blank_episode(2, 7);
        odd.air_date_utc = Some("yesterday".into());
        assert!(episode_event("https://films.example", &work, &odd).is_none());
    }

    #[test]
    fn a_film_is_its_release_day() {
        let mut film = MediaItem::empty(MediaKind::Movie);
        film.id = "f1".into();
        film.title = "Le Parrain".into();
        film.year = Some(1972);
        film.digital_release = Some("2016-06-30T00:00:00Z".into());
        let event = release_event("http://localhost", &film).unwrap();
        assert_eq!(when("DTSTART", &event.start), "DTSTART;VALUE=DATE:20160630");
        assert_eq!(event.summary, "Le Parrain (1972)");
        assert!(release_event("http://localhost", &MediaItem::empty(MediaKind::Movie)).is_none());
    }

    #[test]
    fn a_document_is_a_calendar_of_its_events() {
        let doc = document("Test, calendar", &[]);
        assert!(doc.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"));
        assert!(doc.contains("X-WR-CALNAME:Test\\, calendar\r\n"));
        assert!(doc.ends_with("END:VCALENDAR\r\n"));
        assert!(!doc.contains("VEVENT"));
    }

    #[test]
    fn a_feed_escapes_what_xml_reserves() {
        let entries = [Entry {
            id: "urn:ams:work:1".into(),
            title: "Tom & Jerry <3".into(),
            link: "http://localhost/work/1?x=\"y\"".into(),
            updated: "2026-09-25T10:00:00Z".into(),
            summary: Some("a 'b'".into()),
            categories: vec!["Kids & Family".into()],
        }];
        let feed = atom(&Feed {
            origin: "http://localhost",
            path: "/api/v1/feed/added.atom",
            title: "Feed",
            updated: "2026-09-25T10:00:00Z",
            language: Some("fr"),
            entries: &entries,
        });
        assert!(feed.contains("<feed xmlns=\"http://www.w3.org/2005/Atom\" xml:lang=\"fr\">"));
        assert!(feed.contains("<author><name>Cinémathèque</name></author>"));
        assert!(feed.contains("<title>Tom &amp; Jerry &lt;3</title>"));
        assert!(feed.contains("href=\"http://localhost/work/1?x=&quot;y&quot;\""));
        assert!(feed.contains("<category term=\"Kids &amp; Family\"/>"));
        assert!(feed.contains("<summary>a &apos;b&apos;</summary>"));
        assert_eq!(latest(&entries).as_deref(), Some("2026-09-25T10:00:00Z"));

        assert_eq!(language_tag("fr").as_deref(), Some("fr"));
        assert_eq!(language_tag(" pt-BR ").as_deref(), Some("pt-BR"));
        assert_eq!(language_tag("fra").as_deref(), Some("fra"));
        assert!(language_tag("f").is_none());
        assert!(language_tag("fr\"><x").is_none());
        assert!(language_tag("").is_none());
    }

    #[test]
    fn the_window_is_bounded_whatever_is_asked() {
        assert!(window_of(Some(30), Some(33)).is_err());
        assert!(window_of(Some(-1), None).is_err());
        assert!(window_of(None, Some(-1)).is_err());
        // Two numbers whose sum wraps must not add up to a small one.
        assert!(window_of(Some(i64::MAX), Some(1)).is_err());
        assert!(window_of(Some(1), Some(i64::MAX)).is_err());
        assert!(window_of(Some(i64::MAX), Some(i64::MAX)).is_err());
        assert!(window_of(Some(i64::MIN), Some(i64::MIN)).is_err());
        assert!(window_of(Some(62), Some(1)).is_err());
        assert!(window_of(Some(62), Some(0)).is_ok());
        let (from, to) = window_of(Some(7), Some(28)).unwrap();
        assert_eq!((to - from).num_days(), 36);
        let (from, to) = window_of(None, None).unwrap();
        assert_eq!((to - from).num_days(), 36);
    }

    #[test]
    fn the_origin_is_the_public_address_or_a_host_the_request_came_to() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "films.example:8443".parse().unwrap());
        headers.insert("x-forwarded-host", "front.example, second".parse().unwrap());
        headers.insert("x-forwarded-proto", "HTTPS".parse().unwrap());

        // The public address wins outright.
        assert_eq!(
            origin(Some("https://cinema.example"), &headers, true),
            "https://cinema.example"
        );
        // From a peer that is not a trusted proxy, the forwarded headers are
        // its own invention.
        assert_eq!(origin(None, &headers, false), "http://films.example:8443");
        // From one that is, the first forwarded host and the scheme count.
        assert_eq!(origin(None, &headers, true), "https://front.example");

        // A host that is not the shape of a host is not written into the feed.
        for bad in [
            "bad host;x",
            "]]]:::",
            "a..b",
            "-a.example",
            "x:99999",
            "x:0",
            "[::1",
            "evil\\x",
        ] {
            headers.insert(header::HOST, bad.parse().unwrap());
            assert_eq!(origin(None, &headers, false), "http://localhost", "{bad}");
        }
        for good in [
            "localhost",
            "10.0.0.7:8479",
            "[::1]:8080",
            "[2001:db8::1]",
            "a.b-c.example",
        ] {
            assert!(is_host(good), "{good}");
        }
        assert!(!is_host(""));
        assert!(!is_host("[::1]x"));

        assert_eq!(
            rfc3339("2026-09-25 10:00:00").as_deref(),
            Some("2026-09-25T10:00:00Z")
        );
        assert_eq!(
            rfc3339("2026-09-25T12:00:00+02:00").as_deref(),
            Some("2026-09-25T10:00:00Z")
        );
        assert_eq!(
            rfc3339("2026-09-25T10:00:00.123456").as_deref(),
            Some("2026-09-25T10:00:00Z")
        );
        assert_eq!(
            rfc3339("2026-09-25T10:00:00.123Z").as_deref(),
            Some("2026-09-25T10:00:00Z")
        );
        assert!(rfc3339("yesterday").is_none());
    }
}
