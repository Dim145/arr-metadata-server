//! The import rules: what is brought into the catalogue on a schedule, and
//! into which selection.
//!
//! A rule names a source — TMDB's most popular works of the last days
//! ("every week, the top of the quarter"), or a list another server
//! publishes, followed — a cron expression saying when, and a selection the
//! works go into, joined to its members or in their place. Each run is a run
//! of the task `import.rules`, naming the rule as its target, and imports as
//! the batch does: what is held is passed over, what is not is fetched, each
//! work written down.
//!
//! A list followed is read as a text file of ids — this server's `ids.txt`,
//! Kometa's — or as an Atom or RSS feed whose entries link to a work's page
//! at TMDB, IMDb, TheTVDB, AniList or MyAnimeList, or carry TMDB's ids as
//! Letterboxd's do. This server's own Atom feed of a selection names its
//! works by their pages here, which say nothing of their ids elsewhere: its
//! text file beside it is read instead. The address is fetched by the client
//! that fetches pictures, which never reaches an address only this machine
//! can.
//!
//! The leader runs what is due, one rule at a time, at its minute's tick;
//! a rule that ran is due again at its expression's next moment.

use std::{
    collections::HashSet,
    sync::{Arc, LazyLock},
    time::Duration,
};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    db::repo::{self, import_rule::Rule, job},
    domain::MediaKind,
    error::{AppError, AppResult},
    jobs::{
        cancel,
        import::{self, Line, Options, Placement, Source, Wanted},
        schedule::Cron,
    },
    state::AppState,
};

/// The task's name, as the history files the rules' runs, and what a run is
/// held under among several instances.
pub const KIND: &str = "import.rules";
pub const HOLD: &str = KIND;

/// How often the leader looks for a rule that is due.
const TICK: Duration = Duration::from_secs(60);

/// The most a followed list is read to: a feed larger than this is not a
/// list of works.
const MOST_BYTES: u64 = 4 * 1024 * 1024;

/// The most works one run takes from its source.
pub const MOST_WORKS: usize = 2_000;

/// How far back TMDB's stretch may reach, and how many works it may give.
pub const MOST_DAYS: u32 = 3_650;
pub const MOST_FROM_TMDB: u32 = 500;

/// Held by a run, so that two rules never import at once.
static RUNNING: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

/// Whether a rule is running on this instance.
pub fn is_busy() -> bool {
    RUNNING.try_lock().is_err()
}

/// Where a rule's works come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = RuleSource)]
pub enum SourceKind {
    /// TMDB's most popular works of the last days.
    Tmdb,
    /// A list another server publishes.
    Feed,
}

impl SourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tmdb => "tmdb",
            Self::Feed => "feed",
        }
    }
}

/// What a rule's source is asked, as `params` keeps it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    /// TMDB: films or series. A list: what a bare `tmdb:` line stands for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<MediaKind>,
    /// TMDB: how many days back the stretch reaches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<u32>,
    /// TMDB: how many works, the most popular first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// TMDB: works first made in this language only, ISO 639-1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
    /// A list: its address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// A rule, read: its source, what it is asked, and when it runs.
pub struct Read {
    pub source: SourceKind,
    pub params: Params,
    pub cron: Cron,
}

/// A rule as it is kept, read — or why it cannot be.
pub fn read(rule: &Rule) -> Result<Read, String> {
    let source = match rule.source.as_str() {
        "tmdb" => SourceKind::Tmdb,
        "feed" => SourceKind::Feed,
        other => return Err(format!("{other} is not a source this version knows")),
    };
    let params: Params = serde_json::from_str(&rule.params)
        .map_err(|e| format!("what the source is asked could not be read: {e}"))?;
    let cron: Cron = rule.cron.parse()?;
    Ok(Read {
        source,
        params,
        cron,
    })
}

/// When a rule runs next: its expression's first moment since it last ran,
/// or since it was last changed — a rule written on Tuesday for Mondays
/// waits for Monday.
pub fn next_at(rule: &Rule, cron: &Cron) -> Option<DateTime<Utc>> {
    let since = [rule.last_run_at.as_deref(), Some(rule.updated_at.as_str())]
        .into_iter()
        .flatten()
        .filter_map(crate::db::parse_rfc3339)
        .max()
        .unwrap_or_else(Utc::now);
    cron.next_after(since)
}

/// The rules switched on, each with when it runs next.
pub async fn scheduled(state: &AppState) -> Result<Vec<(Rule, Option<DateTime<Utc>>)>> {
    Ok(repo::import_rule::list(&state.db)
        .await?
        .into_iter()
        .filter(|rule| rule.is_enabled)
        .map(|rule| {
            let next = read(&rule).ok().and_then(|read| next_at(&rule, &read.cron));
            (rule, next)
        })
        .collect())
}

// ─── the sources ─────────────────────────────────────────────────────────────

/// The works a source gives now, in its order, each once.
pub async fn gather(state: &AppState, source: SourceKind, params: &Params) -> Result<Vec<Wanted>> {
    let mut wanted = match source {
        SourceKind::Tmdb => from_tmdb(state, params).await?,
        SourceKind::Feed => from_feed(state, params).await?,
    };
    let mut seen = HashSet::new();
    wanted.retain(|one| seen.insert(one.clone()));
    wanted.truncate(MOST_WORKS);
    Ok(wanted)
}

/// TMDB's most popular works first released in the last `days`.
async fn from_tmdb(state: &AppState, params: &Params) -> Result<Vec<Wanted>> {
    if !state.tmdb.is_configured() {
        anyhow::bail!("TMDB is not configured: set its key, then the rule can ask it");
    }
    let kind = params.kind.unwrap_or(MediaKind::Movie);
    let days = params.days.unwrap_or(90).clamp(1, MOST_DAYS);
    let limit = params.limit.unwrap_or(20).clamp(1, MOST_FROM_TMDB) as usize;
    let today = Utc::now().date_naive();
    let from = (today - chrono::Duration::days(i64::from(days)))
        .format("%Y-%m-%d")
        .to_string();
    let to = today.format("%Y-%m-%d").to_string();
    let original = params.original_language.as_deref();

    let mut out = Vec::new();
    let mut page = 1u32;
    // TMDB gives twenty a page; five hundred is twenty-five pages.
    while out.len() < limit && page <= 25 {
        let (ids, more): (Vec<i64>, bool) = match kind {
            MediaKind::Movie => {
                let (found, more) = state
                    .tmdb
                    .discover_movies(&from, &to, original, None, page)
                    .await?;
                (found.iter().map(|m| m.id).collect(), more)
            }
            MediaKind::Series => {
                let (found, more) = state
                    .tmdb
                    .discover_tv(&from, &to, original, None, page)
                    .await?;
                (found.iter().map(|t| t.id).collect(), more)
            }
        };
        out.extend(ids.into_iter().map(|id| Wanted {
            source: Source::Tmdb,
            id: id.to_string(),
            kind: Some(kind),
        }));
        if !more {
            break;
        }
        page += 1;
    }
    out.truncate(limit);
    Ok(out)
}

/// The works of a list another server publishes.
async fn from_feed(state: &AppState, params: &Params) -> Result<Vec<Wanted>> {
    let url = params
        .url
        .as_deref()
        .context("the rule names no address to follow")?;
    let mut out = Vec::new();
    for (address, kind) in addresses(url) {
        let body = fetch(state, &address).await?;
        out.extend(read_feed(&body, kind.or(params.kind)));
    }
    Ok(out)
}

/// What is fetched to read a list at `url`, each with the kind of work it
/// holds when it says. This server's own selections are read as their text
/// file of ids, once for each kind: its Atom feed names its works by their
/// pages here, and a `tmdb:` line of a list of both kinds is a film's or a
/// series' as the kind asked for says.
pub fn addresses(url: &str) -> Vec<(String, Option<MediaKind>)> {
    let Ok(parsed) = url::Url::parse(url.trim()) else {
        return vec![(url.trim().to_string(), None)];
    };
    let segments: Vec<&str> = parsed
        .path_segments()
        .map(|s| s.collect())
        .unwrap_or_default();
    let ours = matches!(
        segments.as_slice(),
        [.., "api", "v1", "lists", _, "feed.atom" | "ids.txt"]
    );
    if !ours {
        return vec![(parsed.to_string(), None)];
    }
    let asked_kind = parsed
        .query_pairs()
        .find(|(key, _)| key == "kind")
        .map(|(_, value)| value.into_owned());
    let mut text = parsed.clone();
    if let Ok(mut path) = text.path_segments_mut() {
        path.pop().push("ids.txt");
    }
    let with_kind = |kind: &str| {
        let mut one = text.clone();
        let kept: Vec<(String, String)> = text
            .query_pairs()
            .filter(|(key, _)| key != "kind" && key != "language")
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        one.query_pairs_mut()
            .clear()
            .extend_pairs(kept)
            .append_pair("kind", kind);
        one.to_string()
    };
    match asked_kind.as_deref() {
        Some("movie") => vec![(with_kind("movie"), Some(MediaKind::Movie))],
        Some("series") => vec![(with_kind("series"), Some(MediaKind::Series))],
        _ => vec![
            (with_kind("movie"), Some(MediaKind::Movie)),
            (with_kind("series"), Some(MediaKind::Series)),
        ],
    }
}

/// An address, fetched as text by the client that follows addresses somebody
/// else chose.
async fn fetch(state: &AppState, url: &str) -> Result<String> {
    if state.media.guard.refuses_url(url).await {
        anyhow::bail!(
            "{} is an address only this server can reach, which it does not follow",
            shown_url(url)
        );
    }
    let response = state
        .media
        .http
        .get(url)
        .header(
            reqwest::header::ACCEPT,
            "text/plain, application/atom+xml, application/rss+xml, application/xml;q=0.9, */*;q=0.5",
        )
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{} could not be fetched: {e}", shown_url(url)))?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("{} answered {status}", shown_url(url));
    }
    let bytes = crate::providers::read_body(response, MOST_BYTES)
        .await
        .with_context(|| format!("{} could not be read", shown_url(url)))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The works a list names, read as a feed when it is one, else a line at a
/// time.
pub fn read_feed(body: &str, kind: Option<MediaKind>) -> Vec<Wanted> {
    let body = body.trim_start_matches('\u{feff}').trim_start();
    if body.starts_with('<') {
        return read_xml(body, kind);
    }
    body.lines()
        .filter_map(|text| {
            import::read_line(
                &Line {
                    text: text.to_string(),
                    ..Line::default()
                },
                kind,
            )
            .ok()
        })
        .collect()
}

/// An Atom or RSS feed's works: each entry's, or item's, first link to a
/// work's page that this server reads, or TMDB's id where the feed carries
/// it.
fn read_xml(xml: &str, kind: Option<MediaKind>) -> Vec<Wanted> {
    let mut blocks = elements(xml, "entry");
    if blocks.is_empty() {
        blocks = elements(xml, "item");
    }
    blocks
        .into_iter()
        .filter_map(|block| {
            let tmdb = [
                ("tmdb:movieId", MediaKind::Movie),
                ("tmdb:tvId", MediaKind::Series),
            ]
            .into_iter()
            .filter_map(|(tag, kind)| {
                let id = text_of(block, tag)?;
                Some((format!("tmdb:{}", id.trim()), Some(kind)))
            });
            let links = hrefs(block)
                .into_iter()
                .chain(
                    ["link", "guid", "id"]
                        .iter()
                        .filter_map(|tag| text_of(block, tag)),
                )
                .map(|text| (text, None));
            tmdb.chain(links).find_map(|(text, own)| {
                import::read_line(
                    &Line {
                        text,
                        kind: own,
                        title: None,
                    },
                    kind,
                )
                .ok()
            })
        })
        .collect()
}

/// What each `<tag …>…</tag>` of a document holds.
fn elements<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find(&open) {
        let after = &rest[at + open.len()..];
        // `<entry>` or `<entry …>`, not `<entryfoo>`.
        if !after.starts_with(['>', ' ', '\t', '\n', '\r', '/']) {
            rest = after;
            continue;
        }
        let Some(start) = after.find('>') else {
            break;
        };
        let inside = &after[start + 1..];
        let Some(end) = inside.find(&close) else {
            break;
        };
        out.push(&inside[..end]);
        rest = &inside[end + close.len()..];
    }
    out
}

/// The text of the first `<tag>` in a block, its entities read and a CDATA
/// section unwrapped.
fn text_of(block: &str, tag: &str) -> Option<String> {
    let inner = elements(block, tag).into_iter().next()?;
    let inner = inner.trim();
    let inner = inner
        .strip_prefix("<![CDATA[")
        .and_then(|rest| rest.strip_suffix("]]>"))
        .unwrap_or(inner);
    let text = unescape(inner.trim());
    (!text.is_empty()).then_some(text)
}

/// Every `href` of a block's `<link …>` tags, in order.
fn hrefs(block: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = block;
    while let Some(at) = rest.find("<link") {
        let after = &rest[at + 5..];
        let Some(end) = after.find('>') else {
            break;
        };
        let tag = &after[..end];
        if let Some(found) = tag.find("href=") {
            let value = &tag[found + 5..];
            if let Some(quote) = value.chars().next().filter(|c| *c == '"' || *c == '\'')
                && let Some(close) = value[1..].find(quote)
            {
                out.push(unescape(&value[1..=close]));
            }
        }
        rest = &after[end..];
    }
    out
}

/// The five entities XML has, and characters written by their number.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        let Some(end) = after.find(';').filter(|end| *end <= 10) else {
            out.push('&');
            rest = &after[1..];
            continue;
        };
        let name = &after[1..end];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The query parameters an address carries a key in, which are shown only
/// as a mark: a list another server keeps private is read with a key.
const KEYS: &[&str] = &["api_key", "apikey", "key", "token", "access_token", "auth"];

/// How a key is shown in place of its value.
pub const HIDDEN: &str = "hidden";

/// An address as the pages show it: a key in it hidden.
pub fn shown_url(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_string();
    };
    let pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .map(|(k, v)| {
            let hidden = KEYS.iter().any(|key| k.eq_ignore_ascii_case(key));
            (
                k.into_owned(),
                if hidden {
                    HIDDEN.to_string()
                } else {
                    v.into_owned()
                },
            )
        })
        .collect();
    if pairs.is_empty() {
        return parsed.to_string();
    }
    parsed.query_pairs_mut().clear().extend_pairs(pairs);
    parsed.to_string()
}

// ─── running ─────────────────────────────────────────────────────────────────

/// Run one rule now, in the background, as `by` asked — or, with `by` none,
/// as its schedule does: the run's id at once.
pub async fn start(state: &AppState, rule_id: &str, by: Option<&str>) -> AppResult<String> {
    let rule = repo::import_rule::get(&state.db, rule_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let read = read(&rule).map_err(AppError::BadRequest)?;
    let (held, shared) = hold(state).await?;
    let record = job::start_by(&state.db, KIND, Some(&rule.id), by).await?;
    if let Err(e) =
        repo::import_rule::ran(&state.db, &rule.id, &crate::db::now(), Some(&record)).await
    {
        tracing::warn!(rule = %rule.id, error = %e, "could not say that a rule ran");
    }

    let state = state.clone();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        let _shared = shared;
        run_one(&state, &rule, read, &record).await;
    });
    Ok(id)
}

/// The import rules' task as the tasks page shows it: whether any rule is
/// switched on, and when the soonest runs next.
pub async fn standing(state: &AppState) -> Result<(bool, Option<DateTime<Utc>>)> {
    let rules = scheduled(state).await?;
    let next = rules.iter().filter_map(|(_, next)| *next).min();
    Ok((!rules.is_empty(), next))
}

/// Run every rule switched on now, one after the other, as `by` asked: the
/// first one's run at once.
pub async fn start_all(state: &AppState, by: &str) -> AppResult<Option<String>> {
    let rules: Vec<(Rule, Read)> = repo::import_rule::list(&state.db)
        .await?
        .into_iter()
        .filter(|rule| rule.is_enabled)
        .filter_map(|rule| read(&rule).ok().map(|read| (rule, read)))
        .collect();
    if rules.is_empty() {
        return Err(AppError::Disabled {
            code: "no_rules",
            message: "no import rule is switched on: write one on the Lists page".into(),
        });
    }
    let (held, shared) = hold(state).await?;
    // The first one's run opened now, to be named at once; each of the
    // others as its turn comes, so that none reads as running while it
    // waits.
    let first = job::start_by(&state.db, KIND, Some(&rules[0].0.id), Some(by)).await?;

    let state = state.clone();
    let by = by.to_string();
    let id = first.clone();
    tokio::spawn(async move {
        let _held = held;
        let _shared = shared;
        let mut record = Some(first);
        for (rule, read) in rules {
            let record = match record.take() {
                Some(record) => record,
                None => match job::start_by(&state.db, KIND, Some(&rule.id), Some(&by)).await {
                    Ok(record) => record,
                    Err(e) => {
                        tracing::warn!(rule = %rule.id, error = %e, "could not open a rule's run");
                        continue;
                    }
                },
            };
            if let Err(e) =
                repo::import_rule::ran(&state.db, &rule.id, &crate::db::now(), Some(&record)).await
            {
                tracing::warn!(rule = %rule.id, error = %e, "could not say that a rule ran");
            }
            // Stopped, the rules after it are not run either.
            if run_one(&state, &rule, read, &record).await {
                break;
            }
        }
    });
    Ok(Some(id))
}

async fn hold(
    state: &AppState,
) -> AppResult<(tokio::sync::OwnedMutexGuard<()>, crate::coord::Held)> {
    if state.maintenance.is_on() {
        return Err(AppError::Conflict("a backup is being restored".into()));
    }
    let held = RUNNING.clone().try_lock_owned().map_err(|_| {
        AppError::Conflict("an import rule is running already; its run is in the history".into())
    })?;
    let shared = state
        .coord
        .hold_for(HOLD, "an import rule is running")
        .await?;
    Ok((held, shared))
}

/// A rule's run, from its source to its selection, closed with what came of
/// it. Whether it was stopped partway.
async fn run_one(state: &AppState, rule: &Rule, read: Read, record: &str) -> bool {
    let flag = cancel::register(record);
    let wanted = match gather(state, read.source, &read.params).await {
        Ok(wanted) => wanted,
        Err(e) => {
            let why = format!("{e:#}");
            tracing::info!(rule = %rule.id, error = %why, "an import rule could not read its source");
            if let Err(e) = job::finish(&state.db, record, None, Some(&why)).await {
                tracing::warn!(error = %e, "could not close a rule's run");
            }
            return false;
        }
    };
    let options = Options {
        language: rule.language.clone(),
        lists: Vec::new(),
        enabled: rule.enable_works,
    };
    let brought = import::bring_all(state, record, &wanted, &options, &flag).await;
    let mut trouble = None;
    if let Some(list) = rule.list_id.as_deref() {
        match import::place(
            state,
            list,
            &brought.works,
            Placement::parse(&rule.list_mode),
        )
        .await
        {
            Ok(0) => {}
            Ok(left) => {
                trouble = Some(format!(
                    "{left} works were not put in the selection, which is full"
                ))
            }
            Err(e) => trouble = Some(format!("{e:#}")),
        }
    }
    let summary = format!("{}, of {}", brought.tally.said(), wanted.len());
    let closed = if brought.stopped {
        job::stop(&state.db, record, &format!("stopped: {summary}")).await
    } else {
        job::finish(&state.db, record, Some(&summary), trouble.as_deref()).await
    };
    if let Err(e) = closed {
        tracing::warn!(error = %e, "could not close a rule's run");
    }
    brought.stopped
}

/// The rule due soonest, when one is due now.
async fn due_now(state: &AppState) -> Result<Option<Rule>> {
    let now = Utc::now();
    Ok(scheduled(state)
        .await?
        .into_iter()
        .filter_map(|(rule, next)| Some((rule, next?)))
        .filter(|(_, next)| *next <= now)
        .min_by_key(|(_, next)| *next)
        .map(|(rule, _)| rule))
}

/// The rules' schedule, for as long as the server runs: the leader starts
/// the rule due soonest at each tick, one at a time.
pub async fn run(state: AppState) {
    // Let the server finish starting, and the others announce themselves.
    tokio::time::sleep(Duration::from_secs(45)).await;
    loop {
        if state.coord.leads() && !state.maintenance.is_on() && !is_busy() {
            match due_now(&state).await {
                Ok(Some(rule)) => {
                    if let Err(e) = start(&state, &rule.id, None).await {
                        tracing::info!(rule = %rule.id, error = %e, "an import rule that was due did not start");
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        error = format_args!("{e:#}"),
                        "could not read the import rules"
                    )
                }
            }
        }
        tokio::time::sleep(TICK).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_file_of_ids_is_read_a_line_at_a_time() {
        let body = "# Films\n# For Kometa's Text File builder\ntmdb:949  # Heat (1995)\ntt0133093\n\nnot an id\ntvdb:81189\n";
        let read = read_feed(body, Some(MediaKind::Movie));
        assert_eq!(
            read.iter().map(Wanted::label).collect::<Vec<_>>(),
            ["tmdb:949", "tt0133093", "tvdb:81189"]
        );
        assert_eq!(read[0].kind, Some(MediaKind::Movie));
        // Without a kind, a bare TMDB id is passed over.
        assert_eq!(read_feed("tmdb:949\n", None).len(), 0);
    }

    #[test]
    fn a_feed_is_read_by_the_pages_its_entries_link_to() {
        let atom = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Someone's list</title>
  <link rel="self" href="https://example.org/feed.atom"/>
  <entry>
    <id>urn:x:1</id>
    <title>The Matrix</title>
    <link rel="alternate" href="https://www.themoviedb.org/movie/603-the-matrix?a=1&amp;b=2"/>
  </entry>
  <entry>
    <title>Nothing we read</title>
    <link href="https://example.org/somewhere"/>
  </entry>
  <entry><link href='https://www.imdb.com/title/tt0903747/'/></entry>
</feed>"#;
        let read = read_feed(atom, None);
        assert_eq!(
            read.iter().map(Wanted::label).collect::<Vec<_>>(),
            ["tmdb:603", "tt0903747"]
        );
        assert_eq!(read[0].kind, Some(MediaKind::Movie));

        // RSS, with TMDB's ids as Letterboxd's feeds carry them.
        let rss = r#"<rss version="2.0" xmlns:tmdb="https://themoviedb.org"><channel>
  <item><title>Heat</title><link>https://letterboxd.com/x/film/heat/</link><tmdb:movieId>949</tmdb:movieId></item>
  <item><title>A page</title><link><![CDATA[https://thetvdb.com/?tab=series&id=81189]]></link></item>
</channel></rss>"#;
        let read = read_feed(rss, None);
        assert_eq!(
            read.iter().map(Wanted::label).collect::<Vec<_>>(),
            ["tmdb:949", "tvdb:81189"]
        );
    }

    #[test]
    fn this_servers_own_selections_are_read_as_their_text_file_once_a_kind() {
        let both = addresses(
            "https://other.example/api/v1/lists/top/feed.atom?api_key=s3cret&language=fr",
        );
        assert_eq!(
            both,
            vec![
                (
                    "https://other.example/api/v1/lists/top/ids.txt?api_key=s3cret&kind=movie"
                        .to_string(),
                    Some(MediaKind::Movie)
                ),
                (
                    "https://other.example/api/v1/lists/top/ids.txt?api_key=s3cret&kind=series"
                        .to_string(),
                    Some(MediaKind::Series)
                ),
            ]
        );
        let one = addresses("https://other.example/api/v1/lists/top/ids.txt?kind=series");
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].1, Some(MediaKind::Series));
        // Anybody else's address is read as it is.
        assert_eq!(
            addresses("https://example.org/list.txt"),
            vec![("https://example.org/list.txt".to_string(), None)]
        );
    }

    #[test]
    fn a_key_in_an_address_is_shown_hidden() {
        let url = "https://other.example/api/v1/lists/top/ids.txt?api_key=s3cret&kind=movie";
        let shown = shown_url(url);
        assert!(!shown.contains("s3cret"), "{shown}");
        assert!(shown.contains("api_key=hidden"), "{shown}");
        assert!(shown.contains("kind=movie"), "{shown}");
        assert_eq!(
            shown_url("https://example.org/list.txt"),
            "https://example.org/list.txt"
        );
    }

    #[test]
    fn entities_are_read_and_a_stray_ampersand_kept() {
        assert_eq!(unescape("a&amp;b&lt;&#65;&#x42;&quot;"), "a&b<AB\"");
        assert_eq!(unescape("fish & chips"), "fish & chips");
        assert_eq!(unescape("&unknown;"), "&unknown;");
    }

    #[test]
    fn a_rule_runs_at_its_expressions_moment_since_it_last_ran_or_changed() {
        let rule = Rule {
            id: "r".into(),
            name: "Weekly".into(),
            source: "tmdb".into(),
            params: r#"{"kind":"movie","days":90,"limit":20}"#.into(),
            list_id: None,
            list_mode: "add".into(),
            cron: "0 5 * * 1".into(),
            language: None,
            enable_works: true,
            is_enabled: true,
            last_run_at: Some("2026-10-19T05:00:00Z".into()),
            last_job_id: None,
            created_by: None,
            created_at: "2026-10-01T00:00:00Z".into(),
            updated_at: "2026-10-21T10:00:00Z".into(),
        };
        let read = read(&rule).unwrap();
        assert_eq!(read.source, SourceKind::Tmdb);
        assert_eq!(read.params.limit, Some(20));
        // Changed on Wednesday the 21st: Monday the 26th.
        assert_eq!(
            next_at(&rule, &read.cron),
            crate::db::parse_rfc3339("2026-10-26T05:00:00Z")
        );
        let broken = Rule {
            cron: "61 * * * *".into(),
            ..rule
        };
        assert!(super::read(&broken).is_err());
    }
}
