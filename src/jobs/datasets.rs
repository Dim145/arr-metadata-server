//! Lists downloaded whole, on a schedule, and asked locally.
//!
//! Some of the further sources are not asked per request at all. The anime
//! identifier list says which AniList and MyAnimeList entries a series or a
//! film is; IMDb republishes its ratings, the titles each work goes by in
//! each region, and the episodes of each series, as files. They are fetched
//! here, kept in the database for the works held, and read from there — one
//! download a day or a week, however many works are asked about in between.
//!
//! Each runs only while a switch that needs it is on, and each is recorded
//! as a job run so an operator can see it happened and why it failed. IMDb's
//! titles and episodes are files of half a gigabyte and fifty megabytes, and
//! three gigabytes and two hundred and sixty megabytes unpacked: they are
//! read as they arrive, a line at a time, and never held whole — in memory or
//! on disk. A run of either says how far it has got, and can be stopped.

use std::{
    collections::{HashMap, HashSet},
    io::{BufRead as _, BufReader, Read},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use bytes::Bytes;
use futures::StreamExt as _;
use serde::Deserialize;

use crate::{
    db::{
        now, parse_rfc3339,
        repo::{
            self,
            anime::Entry,
            imdb::{Rating, Title},
            import::Import,
            job,
        },
    },
    jobs::cancel,
    providers::read_body,
    state::AppState,
};

/// The names imports are recorded under in `data_import`, and the lists are
/// asked for by in `POST /api/v1/datasets/{name}/import`.
pub const ANIME: &str = "anime-lists";
pub const IMDB: &str = "imdb-ratings";
/// The titles IMDb gives each work by region: `title.akas`.
pub const IMDB_AKAS: &str = "imdb-akas";
/// The episodes IMDb lists of each series: `title.episode`.
pub const IMDB_EPISODES: &str = "imdb-episodes";

/// Every list, in the order the schedule looks at them: IMDb's episodes
/// before its ratings, so that switched on together, the episodes' ratings
/// come with the first list of ratings rather than the next.
pub const LISTS: [&str; 4] = [ANIME, IMDB_EPISODES, IMDB, IMDB_AKAS];

/// How often each list is fetched again. The anime list moves by a handful of
/// entries a week; IMDb republishes its ratings daily. Its titles and its
/// episodes it republishes daily too, but they are half a gigabyte and fifty
/// megabytes, and a title or an episode's number changes far less often
/// than a rating: weekly.
pub const ANIME_EVERY: chrono::Duration = chrono::Duration::days(7);
pub const IMDB_EVERY: chrono::Duration = chrono::Duration::days(1);
pub const IMDB_LISTS_EVERY: chrono::Duration = chrono::Duration::days(7);

/// How soon IMDb's ratings are fetched again for works added since the last
/// import. Only the works held here are kept from the file, so a work added an
/// hour after the import would otherwise wait the rest of the day for its
/// rating — and on a fresh install, that is every work there is. The same for
/// the episodes IMDb's episodes have just brought.
const IMDB_CATCH_UP: chrono::Duration = chrono::Duration::hours(1);

/// How long a failed import waits before it is tried again.
pub const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

/// How often the scheduler looks. Short, so a source switched on in the
/// interface has what it needs within a minute rather than at the next day.
const TICK: Duration = Duration::from_secs(60);

/// The largest each list is accepted at: a few times its size today — the
/// anime list is seven and a half megabytes, IMDb's ratings eight compressed
/// and thirty unpacked — so a mirror that has broken, or been broken into,
/// costs a refusal rather than the machine's memory.
const MAX_ANIME_LIST: u64 = 32 * 1024 * 1024;
const MAX_RATINGS_FILE: u64 = 64 * 1024 * 1024;
const MAX_RATINGS_UNPACKED: u64 = 256 * 1024 * 1024;

/// A line of IMDb's ratings file is some thirty bytes.
const MAX_RATINGS_LINE: u64 = 4096;

/// The same for the titles — 519 megabytes compressed in October 2026, three
/// gigabytes unpacked — and the episodes, 55 and 262. Read as they arrive,
/// these bound the time and the bandwidth a broken mirror can cost, not the
/// memory.
const MAX_AKAS_FILE: u64 = 2 * 1024 * 1024 * 1024;
const MAX_AKAS_UNPACKED: u64 = 12 * 1024 * 1024 * 1024;
const MAX_EPISODES_FILE: u64 = 256 * 1024 * 1024;
const MAX_EPISODES_UNPACKED: u64 = 1024 * 1024 * 1024;

/// The longest line of the titles is under nine hundred bytes; of the
/// episodes, thirty-two.
const MAX_LIST_LINE: u64 = 4096;

/// How long a download of the titles or the episodes may take in all, and
/// how long it may go without a byte arriving.
const LONG_DOWNLOAD: Duration = Duration::from_secs(60 * 60);
const STALLED: Duration = Duration::from_secs(120);

/// How often a run says how far it has got.
const SAY_EVERY: Duration = Duration::from_secs(2);

/// Rows written in one transaction: a few milliseconds of SQLite's writer.
const BATCH: usize = 2_000;

/// How many lines a reading goes between two looks at whether it was asked
/// to stop: a few milliseconds' worth.
const LOOK_EVERY: u64 = 65_536;

/// What the lists begin with: a list that begins otherwise is not one this
/// server can read, and is refused before anything is written.
const AKAS_HEADER: &str =
    "titleId\tordering\ttitle\tregion\tlanguage\ttypes\tattributes\tisOriginalTitle";
const EPISODES_HEADER: &str = "tconst\tparentTconst\tseasonNumber\tepisodeNumber";

/// Whether anything needs the anime identifier list.
pub fn anime_wanted(state: &AppState) -> bool {
    state.flag("anilist.enabled", false) || state.flag("mal.enabled", false)
}

/// Whether a list is wanted: the switch that needs it is on.
pub fn wanted(state: &AppState, list: &str) -> bool {
    match list {
        ANIME => anime_wanted(state),
        IMDB => state.flag("imdb.enabled", false),
        IMDB_AKAS => state.flag("imdb.akas", false),
        IMDB_EPISODES => state.flag("imdb.episodes", false),
        _ => false,
    }
}

/// The task a list's runs are filed under.
pub fn kind_of(list: &str) -> &'static str {
    match list {
        ANIME => job::kinds::IMPORT_ANIME,
        IMDB => job::kinds::IMPORT_IMDB,
        IMDB_AKAS => job::kinds::IMDB_AKAS,
        _ => job::kinds::IMDB_EPISODES,
    }
}

/// The list a task downloads.
pub fn list_of(kind: &str) -> Option<&'static str> {
    LISTS.into_iter().find(|list| kind_of(list) == kind)
}

/// How often a list comes due by its own cadence.
pub fn every(list: &str) -> chrono::Duration {
    match list {
        ANIME => ANIME_EVERY,
        IMDB => IMDB_EVERY,
        _ => IMDB_LISTS_EVERY,
    }
}

/// Run the scheduler until the process shuts down.
pub async fn run(state: AppState) {
    // Let the server finish starting before the first download.
    tokio::time::sleep(Duration::from_secs(20)).await;

    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // When each list's last run failed, or was stopped, by its name.
    let mut failed: HashMap<&'static str, tokio::time::Instant> = HashMap::new();

    loop {
        ticker.tick().await;

        // The schedule is the leader's: alone, this instance; among several,
        // the one holding the lease.
        if !state.coord.leads() {
            continue;
        }

        for list in LISTS {
            // Asked again once the lock is held: an import somebody asked for
            // by hand — here or on another instance — may have been what it
            // waited on, and has just done the work.
            if wanted(&state, list)
                && retry_allowed(failed.get(list).copied())
                && due(&state, list).await
            {
                let _importing = IMPORTING.lock().await;
                if let Some(_shared) = state.coord.hold(HOLD).await
                    && due(&state, list).await
                {
                    match import_list(&state, list, None).await {
                        Ok(Ended::Done(_)) => {
                            failed.remove(list);
                        }
                        // Stopped: not started again at the next tick, which
                        // would undo the stop; an hour later, as a failure.
                        Ok(Ended::Stopped(_)) | Err(_) => {
                            failed.insert(list, tokio::time::Instant::now());
                        }
                    }
                }
            }
        }
    }
}

/// What every import is held under among several instances.
pub const HOLD: &str = "import";

fn retry_allowed(failed: Option<tokio::time::Instant>) -> bool {
    failed.is_none_or(|at| at.elapsed() >= RETRY_AFTER)
}

/// The last import of a list, `Some(None)` if there has been none; `None` when
/// the database could not say, which is never a reason to download.
async fn last_import(state: &AppState, name: &str) -> Option<Option<Import>> {
    match repo::import::get(&state.db, name).await {
        Ok(last) => Some(last),
        Err(e) => {
            tracing::warn!(
                name,
                error = format_args!("{e:#}"),
                "could not read when a list was last imported"
            );
            None
        }
    }
}

/// Whether a list is due, by its schedule and its own cadence.
async fn due(state: &AppState, list: &str) -> bool {
    match clock(state, list).await {
        Some(clock) => crate::jobs::schedule::due(state, kind_of(list), clock).await,
        None => false,
    }
}

/// What a list's own cadence says: never imported, at once; else a week
/// after — the anime list, IMDb's titles and its episodes — or a day after —
/// IMDb's ratings, and sooner, though not more than hourly, when works have
/// been added since they were last read, or IMDb's episodes read since; an
/// hour after a failed or a stopped run at the soonest. `None` when the
/// database could not say, which is never a reason to download.
///
/// A schedule set for the task goes by this: a window waits for its
/// opening; a cron expression counts its next moment from the last import.
pub async fn clock(state: &AppState, list: &str) -> Option<crate::jobs::schedule::Clock> {
    let last = last_import(state, list).await?;
    let imported = last.as_ref().and_then(|l| parse_rfc3339(&l.imported_at));
    let now = chrono::Utc::now();
    let natural = match (list, imported, last.as_ref()) {
        (_, None, _) => now,
        (IMDB, Some(at), Some(last)) => {
            let daily = at + IMDB_EVERY;
            let added = repo::imdb::added_since(&state.db, &last.imported_at)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(
                        error = format_args!("{e:#}"),
                        "could not tell whether works were added"
                    );
                    false
                });
            // The episodes read since the ratings were: their ratings wait
            // in the next list.
            let episodes_since = state.flag("imdb.episodes", false)
                && matches!(
                    last_import(state, IMDB_EPISODES).await,
                    Some(Some(episodes)) if parse_rfc3339(&episodes.imported_at)
                        .is_some_and(|when| when > at)
                );
            if added || episodes_since {
                daily.min(at + IMDB_CATCH_UP)
            } else {
                daily
            }
        }
        (_, Some(at), _) => at + every(list),
    };
    let not_before = match job::latest(&state.db, kind_of(list)).await {
        Ok((Some(run), _)) if matches!(run.status, job::Status::Failed | job::Status::Stopped) => {
            run.finished_at
                .as_deref()
                .and_then(parse_rfc3339)
                .and_then(|at| {
                    chrono::Duration::from_std(RETRY_AFTER)
                        .ok()
                        .map(|wait| at + wait)
                })
        }
        _ => None,
    };
    Some(crate::jobs::schedule::Clock {
        natural,
        last_done: imported,
        not_before,
    })
}

/// One import at a time, whether the scheduler or an operator asked for it:
/// two writers replacing the same table at once would each wait out the other.
static IMPORTING: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// What asking for an import now came to.
pub enum Asked {
    Imported(String),
    /// It was asked to stop, and did: what it had written stays.
    Stopped(String),
    /// Another import is running; this one was not started.
    Busy,
    /// No source that needs this list is switched on.
    NotWanted,
    Unknown,
    Failed(anyhow::Error),
}

/// Import one list now, in the background, as `identity` asked from the
/// tasks page: told at once whether it started, its outcome in the history.
/// The lock is taken here and handed to the run, so a started import is
/// always one that happens.
pub async fn import_in_background(
    state: &AppState,
    name: &'static str,
    identity: &crate::auth::Identity,
    ip: crate::api::extract::ClientIp,
) -> crate::error::AppResult<Option<String>> {
    if !LISTS.contains(&name) {
        return Err(crate::error::AppError::NotFound);
    }
    if !wanted(state, name) {
        return Err(crate::error::AppError::Conflict(
            "its source is switched off".into(),
        ));
    }
    let Ok(importing) = IMPORTING.try_lock() else {
        return Err(crate::error::AppError::Conflict(
            "an import is already running".into(),
        ));
    };
    let shared = state.coord.hold_for(HOLD, "an import is running").await?;

    let state = state.clone();
    let identity = identity.clone();
    tokio::spawn(async move {
        let _importing = importing;
        let _shared = shared;
        let by = identity.label();
        if let Asked::Imported(summary) = import_held(&state, name, Some(&by)).await {
            crate::api::audit::record(
                &state,
                crate::api::audit::Event {
                    identity: Some(&identity),
                    ip: &ip,
                    action: repo::audit::Action::DatasetImported,
                    target: Some(name),
                    detail: Some(&summary),
                },
            )
            .await;
        }
    });
    Ok(None)
}

/// Whether an import is running now.
pub fn is_busy() -> bool {
    IMPORTING.try_lock().is_err()
}

/// Import one list now, rather than when the schedule next says.
///
/// For an operator who has just switched a source on, or who suspects a list
/// is stale. Recorded as a job like any other import.
pub async fn import_now(state: &AppState, name: &str, by: Option<&str>) -> Asked {
    if !LISTS.contains(&name) {
        return Asked::Unknown;
    }
    if !wanted(state, name) {
        return Asked::NotWanted;
    }

    let Ok(_importing) = IMPORTING.try_lock() else {
        return Asked::Busy;
    };
    let Some(_shared) = state.coord.hold(HOLD).await else {
        return Asked::Busy;
    };

    import_held(state, name, by).await
}

/// Import one list, the import lock held by the caller.
async fn import_held(state: &AppState, name: &str, by: Option<&str>) -> Asked {
    match import_list(state, name, by).await {
        Ok(Ended::Done(summary)) => Asked::Imported(summary),
        Ok(Ended::Stopped(summary)) => Asked::Stopped(summary),
        Err(e) => Asked::Failed(e),
    }
}

/// How a run ended that did not fail.
pub enum Ended {
    /// What it kept.
    Done(String),
    /// Asked to stop, it did: how far it had got.
    Stopped(String),
}

/// Run one list's import as a job, so it shows in the interface.
async fn import_list(state: &AppState, list: &str, by: Option<&str>) -> Result<Ended> {
    match list {
        ANIME => record(state, job::kinds::IMPORT_ANIME, by, import_anime(state))
            .await
            .map(Ended::Done),
        IMDB => record(state, job::kinds::IMPORT_IMDB, by, import_imdb(state))
            .await
            .map(Ended::Done),
        IMDB_AKAS | IMDB_EPISODES => record_stoppable(state, list, by).await,
        _ => bail!("{list} is not a list this server downloads"),
    }
}

/// Run one import as a job, so it shows in the interface.
async fn record(
    state: &AppState,
    kind: &str,
    by: Option<&str>,
    work: impl Future<Output = Result<String>>,
) -> Result<String> {
    let run = match job::start_by(&state.db, kind, None, by).await {
        Ok(id) => Some(id),
        Err(e) => {
            // Losing the record must not stop the work.
            tracing::warn!(error = %e, "could not open a job run");
            None
        }
    };

    let outcome = work.await;

    if let Some(run) = run {
        let closed = match &outcome {
            Ok(summary) => job::finish(&state.db, &run, Some(summary), None).await,
            Err(e) => job::finish(&state.db, &run, None, Some(&format!("{e:#}"))).await,
        };
        if let Err(e) = closed {
            tracing::warn!(error = %e, "could not close the job run");
        }
    }

    match &outcome {
        Ok(summary) => tracing::info!(kind, summary, "list imported"),
        Err(e) => tracing::warn!(
            kind,
            error = format_args!("{e:#}"),
            "list import failed; the previous one stays"
        ),
    }

    outcome
}

/// A run of an import that can be stopped, as the import sees it: where it
/// says how far it has got, and whether somebody asked it to stop.
struct Run<'a> {
    state: &'a AppState,
    /// None when the history could not be written: the work goes on, unseen.
    id: Option<String>,
    flag: Option<Arc<cancel::Registered>>,
}

impl Run<'_> {
    fn stopped(&self) -> bool {
        self.flag.as_ref().is_some_and(|flag| flag.stopped())
    }

    /// The same question, for the thread reading the list.
    fn stop_check(&self) -> impl Fn() -> bool + Send + 'static {
        let flag = self.flag.clone();
        move || flag.as_ref().is_some_and(|flag| flag.stopped())
    }

    async fn say(&self, detail: &str) {
        if let Some(id) = &self.id
            && let Err(e) = job::progress(&self.state.db, id, detail).await
        {
            tracing::debug!(error = %e, "could not say how far an import has got");
        }
    }
}

/// [`record`], for an import that can be stopped: the run is registered as
/// one, and closed as stopped when it was.
async fn record_stoppable(state: &AppState, list: &str, by: Option<&str>) -> Result<Ended> {
    let kind = kind_of(list);
    let id = match job::start_by(&state.db, kind, None, by).await {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!(error = %e, "could not open a job run");
            None
        }
    };
    let run = Run {
        state,
        flag: id.as_deref().map(|id| Arc::new(cancel::register(id))),
        id,
    };

    let outcome = if list == IMDB_AKAS {
        import_akas(state, &run).await
    } else {
        import_episodes(state, &run).await
    };

    if let Some(id) = &run.id {
        let closed = match &outcome {
            Ok(Ended::Done(summary)) => job::finish(&state.db, id, Some(summary), None).await,
            Ok(Ended::Stopped(detail)) => job::stop(&state.db, id, detail).await,
            Err(e) => job::finish(&state.db, id, None, Some(&format!("{e:#}"))).await,
        };
        if let Err(e) = closed {
            tracing::warn!(error = %e, "could not close the job run");
        }
    }

    match &outcome {
        Ok(Ended::Done(summary)) => tracing::info!(kind, summary, "list imported"),
        Ok(Ended::Stopped(detail)) => tracing::info!(kind, detail, "list import stopped"),
        Err(e) => tracing::warn!(
            kind,
            error = format_args!("{e:#}"),
            "list import failed; what was written stays"
        ),
    }

    outcome
}

async fn download(state: &AppState, url: &str, limit: u64) -> Result<Vec<u8>> {
    let response = state
        .http
        .get(url)
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .with_context(|| format!("could not reach {url}"))?;

    let status = response.status();
    if !status.is_success() {
        bail!("{url} answered {status}");
    }

    read_body(response, limit)
        .await
        .with_context(|| format!("could not read {url}"))
}

// ─── the anime identifier list ───────────────────────────────────────────────

async fn import_anime(state: &AppState) -> Result<String> {
    let started = now();
    let body = download(state, &state.config.anime_mapping.url, MAX_ANIME_LIST).await?;
    let entries = tokio::task::spawn_blocking(move || parse_anime(&body)).await??;

    // An empty answer is a broken mirror, not a list with nothing in it; one
    // far shorter than the last is a format this server no longer reads —
    // elements it cannot read are skipped whole. The list only ever grows, and
    // taking either would forget what this server knew until next week.
    if entries.is_empty() {
        bail!("the list had no usable entries");
    }
    if let Some(previous) = repo::import::get(&state.db, ANIME).await?
        && (entries.len() as i64) < previous.row_count / 2
    {
        bail!(
            "the list has {} usable entries against {} last time; keeping the previous one",
            entries.len(),
            previous.row_count
        );
    }

    repo::anime::replace_all(&state.db, &entries).await?;
    repo::import::record(&state.db, ANIME, entries.len() as i64, &started).await?;

    Ok(format!("{} entries", entries.len()))
}

/// One element of the Fribb list, as far as this server reads it.
#[derive(Debug, Deserialize)]
struct AnimeRecord {
    #[serde(rename = "type")]
    kind: Option<String>,
    mal_id: Option<i64>,
    anilist_id: Option<i64>,
    tvdb_id: Option<i64>,
    season: Option<TvdbPlace>,
    episode_offset: Option<TvdbPlace>,
    /// `{"tv": 1429}` or `{"movie": [372058]}`. Read loosely: the project
    /// changed this field's shape once already.
    themoviedb_id: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct TvdbPlace {
    tvdb: Option<i32>,
}

/// An element of the list: one this server reads, or anything else, skipped
/// whole. Each is read on its own as the list is: read whole as documents
/// first, the list's seven and a half megabytes took the server to 150 MB at
/// a start it was due, where it now reaches 105.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AnimeElement {
    Record(AnimeRecord),
    Other(serde::de::IgnoredAny),
}

/// The rows this server keeps from the list.
///
/// An entry with neither an AniList nor a MyAnimeList id has nothing to look
/// up, and one with neither a TheTVDB series nor a TMDB film has nothing to be
/// looked up from. An entry filed under several TMDB films becomes a row for
/// each. An element that does not read is skipped rather than failing the list.
fn parse_anime(body: &[u8]) -> Result<Vec<Entry>> {
    let elements: Vec<AnimeElement> =
        serde_json::from_slice(body).context("the anime list is not a JSON array")?;

    let mut entries = Vec::new();

    for element in elements {
        let AnimeElement::Record(record) = element else {
            continue;
        };

        if record.mal_id.is_none() && record.anilist_id.is_none() {
            continue;
        }

        let movies: Vec<i64> = record
            .themoviedb_id
            .as_ref()
            .and_then(|v| v.get("movie"))
            .and_then(|v| v.as_array())
            .map(|ids| ids.iter().filter_map(serde_json::Value::as_i64).collect())
            .unwrap_or_default();

        if record.tvdb_id.is_none() && movies.is_empty() {
            continue;
        }

        let entry = Entry {
            mal_id: record.mal_id,
            anilist_id: record.anilist_id,
            tvdb_id: record.tvdb_id,
            tvdb_season: record.season.and_then(|s| s.tvdb),
            tvdb_offset: record.episode_offset.and_then(|o| o.tvdb),
            tmdb_movie: None,
            kind: record.kind,
        };

        if movies.is_empty() {
            entries.push(entry);
        } else {
            entries.extend(movies.into_iter().map(|id| Entry {
                tmdb_movie: Some(id),
                ..entry.clone()
            }));
        }
    }

    Ok(entries)
}

// ─── IMDb's ids ──────────────────────────────────────────────────────────────

/// The number of an IMDb id written as IMDb writes it — `tt` and seven digits
/// or more, with no more zeros in front than make seven — or nothing. A set of
/// these numbers is a few bytes an id, where a set of the strings is forty.
fn tconst_number(text: &[u8]) -> Option<u32> {
    let digits = text.strip_prefix(b"tt")?;
    if !(7..=10).contains(&digits.len()) || (digits.len() > 7 && digits[0] == b'0') {
        return None;
    }
    digits.iter().try_fold(0u32, |number, &digit| {
        if !digit.is_ascii_digit() {
            return None;
        }
        number.checked_mul(10)?.checked_add(u32::from(digit - b'0'))
    })
}

/// An IMDb id, written back as IMDb writes it.
fn tconst_text(number: u32) -> String {
    format!("tt{number:07}")
}

/// The numbers of the ids that are written as IMDb writes them.
fn numbers(tconsts: &[String]) -> HashSet<u32> {
    tconsts
        .iter()
        .filter_map(|tconst| tconst_number(tconst.as_bytes()))
        .collect()
}

// ─── IMDb's ratings ──────────────────────────────────────────────────────────

async fn import_imdb(state: &AppState) -> Result<String> {
    // Before the ids are read: a work stored while this runs is newer than the
    // import, so the catch-up sees it.
    let started = now();
    let works = numbers(&repo::imdb::wanted(&state.db).await?);
    // The episodes IMDb lists of the series held, while they are read.
    let episodes = if state.flag("imdb.episodes", false) {
        numbers(&repo::imdb::episode_ids(&state.db).await?)
    } else {
        HashSet::new()
    };
    let wanted: HashSet<u32> = works.union(&episodes).copied().collect();

    let url = format!(
        "{}/title.ratings.tsv.gz",
        state.config.imdb.datasets.trim_end_matches('/')
    );
    let body = download(state, &url, MAX_RATINGS_FILE).await?;
    let ratings = tokio::task::spawn_blocking(move || parse_ratings(&body, &wanted)).await??;

    let is_work = |rating: &Rating| {
        tconst_number(rating.tconst.as_bytes()).is_some_and(|number| works.contains(&number))
    };
    let works_rated = ratings.iter().filter(|rating| is_work(rating)).count();

    // IMDb rates nearly every title it lists. A file that rates almost none of
    // the works held here was misread, and taking it would wipe their ratings.
    // Episodes are left out of the count: many have too few votes for a rating.
    if works.len() >= 50 && works_rated * 4 < works.len() {
        bail!(
            "only {works_rated} of {} works were rated; keeping the previous ratings",
            works.len()
        );
    }

    // Which works this rescores, for `jobs::listing` to list again: a figure
    // that moved, arrived or went. A vote count moving alone changes no score
    // — the list's figure wins over a stored one on its votes, and the stored
    // one is always the older — and marking every rated work instead listed the
    // whole catalogue again after each list, hourly while works were added.
    // An episode is listed nowhere: its figure is read when its page asks.
    let before = repo::imdb::values(&state.db).await?;
    repo::imdb::replace_all(&state.db, &ratings).await?;

    let kept: HashSet<&str> = ratings
        .iter()
        .filter(|r| is_work(r))
        .map(|r| r.tconst.as_str())
        .collect();
    let rescored: Vec<String> = ratings
        .iter()
        .filter(|r| is_work(r) && before.get(&r.tconst) != Some(&r.rating))
        .map(|r| r.tconst.clone())
        .chain(
            before
                .keys()
                .filter(|t| !kept.contains(t.as_str()))
                .cloned(),
        )
        .collect();
    repo::item::mark_changed_by_imdb(&state.db, &rescored).await?;
    repo::import::record(&state.db, IMDB, ratings.len() as i64, &started).await?;

    // A cached work carries the rating it was loaded with.
    state.caches.invalidate_all().await;

    let summary = format!("{works_rated} of {} works rated", works.len());
    Ok(if episodes.is_empty() {
        summary
    } else {
        format!(
            "{summary}, {} of {} episodes",
            ratings.len() - works_rated,
            episodes.len()
        )
    })
}

/// The ratings in IMDb's `title.ratings.tsv.gz` for the titles in `wanted`.
///
/// Three tab-separated columns under a header: `tconst`, `averageRating`,
/// `numVotes`. Read a line at a time from the compressed file, so the million
/// and a half titles never sit in memory at once.
///
/// Bounded twice over, because eight compressed megabytes can unpack to any
/// size at all: a line may not run past `MAX_RATINGS_LINE`, and the whole file
/// may not unpack past `MAX_RATINGS_UNPACKED`.
fn parse_ratings(gz: &[u8], wanted: &HashSet<u32>) -> Result<Vec<Rating>> {
    let mut reader = std::io::BufReader::new(
        flate2::read::MultiGzDecoder::new(gz).take(MAX_RATINGS_UNPACKED + 1),
    );
    let mut buffer = Vec::new();
    let mut header = true;
    let mut ratings = Vec::new();

    loop {
        buffer.clear();
        let read = (&mut reader)
            .take(MAX_RATINGS_LINE + 1)
            .read_until(b'\n', &mut buffer)
            .context("IMDb's ratings file could not be decompressed")?;

        if read == 0 {
            break;
        }
        if buffer.len() as u64 > MAX_RATINGS_LINE {
            bail!("IMDb's ratings file has a line longer than any it should");
        }
        if reader.get_ref().limit() == 0 {
            bail!("IMDb's ratings file unpacks to more than {MAX_RATINGS_UNPACKED} bytes");
        }

        let line = std::str::from_utf8(&buffer)
            .context("IMDb's ratings file is not text")?
            .trim_end_matches(['\n', '\r']);

        if std::mem::take(&mut header) {
            if !line.starts_with("tconst\taverageRating\tnumVotes") {
                bail!("IMDb's ratings file does not start with the header it should: {line:?}");
            }
            continue;
        }

        let mut columns = line.split('\t');

        let (Some(tconst), Some(average), Some(votes)) =
            (columns.next(), columns.next(), columns.next())
        else {
            continue;
        };

        if !tconst_number(tconst.as_bytes()).is_some_and(|number| wanted.contains(&number)) {
            continue;
        }

        let (Ok(rating), Ok(votes)) = (average.parse::<f64>(), votes.parse::<i64>()) else {
            continue;
        };

        ratings.push(Rating {
            tconst: tconst.to_string(),
            rating,
            votes,
        });
    }

    if header {
        bail!("IMDb's ratings file is empty");
    }

    Ok(ratings)
}

// ─── a list read as it arrives ───────────────────────────────────────────────

/// Asked to stop, the reading did.
#[derive(Debug)]
struct Stopped;

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("asked to stop")
    }
}

impl std::error::Error for Stopped {}

fn was_stopped(error: &anyhow::Error) -> bool {
    error.is::<Stopped>()
}

/// How far a reading has got: the compressed bytes it has read of the list,
/// and the rows it has kept.
#[derive(Default)]
struct Tally {
    read: AtomicU64,
    kept: AtomicU64,
}

impl Tally {
    /// What a run says while it reads, in a form `jobs::tasks::progress_of`
    /// reads back: `212 of 519: megabytes read, 1740 titles kept`, the first
    /// two the progress bar's.
    fn said(&self, total: Option<u64>, what: &str) -> String {
        let megabytes = |bytes: u64| bytes.div_ceil(1_000_000);
        let read = megabytes(self.read.load(Ordering::Relaxed));
        let kept = self.kept.load(Ordering::Relaxed);
        match total.filter(|total| *total > 0) {
            Some(total) => format!(
                "{read} of {}: megabytes read, {kept} {what} kept",
                megabytes(total).max(read)
            ),
            None => format!("{read} megabytes read, {kept} {what} kept"),
        }
    }
}

type Chunks = Pin<Box<dyn futures::Stream<Item = std::io::Result<Bytes>> + Send>>;

/// One of IMDb's lists, opened to be read as it arrives, and its size where
/// IMDb's server says it.
///
/// The bytes count into `tally` as they arrive. Past `limit` the download
/// stops, as it does when nothing arrives for two minutes: a mirror that
/// stalls holds the import lock, and every other list waits on it.
async fn open_list(
    state: &AppState,
    url: &str,
    limit: u64,
    tally: Arc<Tally>,
) -> Result<(tokio_util::io::StreamReader<Chunks, Bytes>, Option<u64>)> {
    let response = state
        .http
        .get(url)
        .timeout(LONG_DOWNLOAD)
        .send()
        .await
        .with_context(|| format!("could not reach {url}"))?;

    let status = response.status();
    if !status.is_success() {
        bail!("{url} answered {status}");
    }
    let total = response.content_length();
    if total.is_some_and(|total| total > limit) {
        bail!("{url} is larger than {limit} bytes");
    }

    let arriving = Box::pin(response.bytes_stream());
    let chunks: Chunks = Box::pin(futures::stream::unfold(
        (arriving, 0u64, tally),
        move |(mut arriving, mut seen, tally)| async move {
            let chunk = match tokio::time::timeout(STALLED, arriving.next()).await {
                Err(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "nothing arrived for two minutes",
                )),
                Ok(None) => return None,
                Ok(Some(Err(e))) => Err(std::io::Error::other(e)),
                Ok(Some(Ok(bytes))) => {
                    seen += bytes.len() as u64;
                    tally.read.store(seen, Ordering::Relaxed);
                    if seen > limit {
                        Err(std::io::Error::other(format!(
                            "the list is larger than {limit} bytes"
                        )))
                    } else {
                        Ok(bytes)
                    }
                }
            };
            Some((chunk, (arriving, seen, tally)))
        },
    ));

    Ok((tokio_util::io::StreamReader::new(chunks), total))
}

/// Read a gzipped list a line at a time, each line after its header handed
/// to `each` without its line break; `what` names the list in what is said
/// of it. On a thread that may block: the list is read as it arrives.
///
/// Bounded as the ratings are: a line may not run past `MAX_LIST_LINE`, the
/// whole may not unpack past `unpacked`. Every [`LOOK_EVERY`] lines it asks
/// `stopped`, and ends in [`Stopped`] when it says so.
fn each_line(
    gz: impl Read,
    unpacked: u64,
    header: &str,
    what: &str,
    stopped: &dyn Fn() -> bool,
    mut each: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let mut reader = BufReader::with_capacity(
        256 * 1024,
        flate2::read::MultiGzDecoder::new(BufReader::with_capacity(256 * 1024, gz))
            .take(unpacked + 1),
    );
    let mut line = Vec::with_capacity(1024);
    let mut lines: u64 = 0;
    let mut headed = false;

    loop {
        line.clear();
        let read = (&mut reader)
            .take(MAX_LIST_LINE + 1)
            .read_until(b'\n', &mut line)
            .with_context(|| format!("{what} could not be read"))?;

        if read == 0 {
            break;
        }
        if line.len() as u64 > MAX_LIST_LINE {
            bail!("{what} has a line longer than any it should");
        }
        if reader.get_ref().limit() == 0 {
            bail!("{what} unpacks to more than {unpacked} bytes");
        }

        lines += 1;
        if lines.is_multiple_of(LOOK_EVERY) && stopped() {
            return Err(Stopped.into());
        }

        let mut text = line.as_slice();
        while let [rest @ .., b'\n' | b'\r'] = text {
            text = rest;
        }

        if !headed {
            headed = true;
            if !text.starts_with(header.as_bytes()) {
                let start = String::from_utf8_lossy(&text[..text.len().min(120)]).into_owned();
                bail!("{what} does not start with the header it should: {start:?}");
            }
            continue;
        }

        each(text)?;
    }

    if !headed {
        bail!("{what} is empty");
    }
    Ok(())
}

/// A column that IMDb writes `\N` when it holds nothing.
fn given(column: &[u8]) -> Option<&[u8]> {
    (column != b"\\N" && !column.is_empty()).then_some(column)
}

/// A whole number as a column writes it.
fn whole(column: &[u8]) -> Option<i64> {
    std::str::from_utf8(given(column)?).ok()?.parse().ok()
}

/// A short code — a region, a language — as a column writes it: letters,
/// digits and dashes, eight at most.
fn code(column: &[u8]) -> Option<String> {
    let column = given(column)?;
    (column.len() <= 8
        && column
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-'))
    .then(|| String::from_utf8_lossy(column).into_owned())
}

// ─── IMDb's titles by region ─────────────────────────────────────────────────

/// One title of `title.akas` kept, before it is written.
#[derive(Clone, Debug, PartialEq)]
struct Aka {
    ordering: i64,
    title: String,
    region: Option<String>,
    language: Option<String>,
    types: String,
    is_original: bool,
}

/// The types of title kept: those Radarr is given.
const KEPT_TYPES: [&[u8]; 3] = [b"imdbDisplay", b"original", b"alternative"];

/// What one line of `title.akas` gives, when it is a title kept: of a work
/// held here, of one of the [`KEPT_TYPES`] or the title it was first released
/// under. Eight columns: `titleId`, `ordering`, `title`, `region`,
/// `language`, `types` and `attributes` — lists, their items parted by the
/// byte 2 — and `isOriginalTitle`.
fn read_aka(line: &[u8], wanted: &HashSet<u32>) -> Option<(u32, Aka)> {
    let mut columns = line.split(|byte| *byte == b'\t');
    let work = tconst_number(columns.next()?)?;
    if !wanted.contains(&work) {
        return None;
    }

    let ordering = whole(columns.next()?)?;
    let title = std::str::from_utf8(given(columns.next()?)?).ok()?.trim();
    let region = code(columns.next()?);
    let language = code(columns.next()?);
    let types: Vec<&str> = given(columns.next()?)
        .map(|types| {
            types
                .split(|byte| *byte == 2)
                .filter(|kind| KEPT_TYPES.contains(kind))
                .filter_map(|kind| std::str::from_utf8(kind).ok())
                .collect()
        })
        .unwrap_or_default();
    let _attributes = columns.next()?;
    let is_original = columns.next()? == b"1";

    if title.is_empty() || (types.is_empty() && !is_original) {
        return None;
    }

    Some((
        work,
        Aka {
            ordering,
            title: title.to_string(),
            region,
            language,
            types: types.join(","),
            is_original,
        },
    ))
}

/// Read IMDb's titles through, keeping those of the works held here and
/// writing them a batch at a time.
async fn import_akas(state: &AppState, run: &Run<'_>) -> Result<Ended> {
    // Before the ids are read, as the ratings' are.
    let started = now();
    let wanted = numbers(&repo::imdb::wanted(&state.db).await?);

    // Nothing held, nothing to read half a gigabyte for.
    if wanted.is_empty() {
        let gone = repo::imdb::titled(&state.db).await?;
        repo::imdb::forget_titles(&state.db, &gone).await?;
        repo::import::record(&state.db, IMDB_AKAS, 0, &started).await?;
        return Ok(Ended::Done("0 titles for 0 of 0 works".into()));
    }

    let url = format!(
        "{}/title.akas.tsv.gz",
        state.config.imdb.datasets.trim_end_matches('/')
    );
    let tally = Arc::new(Tally::default());
    let (reader, total) = open_list(state, &url, MAX_AKAS_FILE, Arc::clone(&tally)).await?;
    let bridge = tokio_util::io::SyncIoBridge::new(reader);

    // A few batches between the reading and the writing, at most: the reading
    // waits on a writing that falls behind rather than filling the memory.
    let (sender, mut batches) = tokio::sync::mpsc::channel::<Vec<(u32, Aka)>>(4);
    // The reading stops when somebody asks, or when the writing has failed:
    // it would otherwise read the rest of the list for nothing.
    let abandoned = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = {
        let asked = run.stop_check();
        let abandoned = Arc::clone(&abandoned);
        move || asked() || abandoned.load(Ordering::Relaxed)
    };
    let counted = Arc::clone(&tally);
    let works = wanted.len();
    let reading = tokio::task::spawn_blocking(move || -> Result<()> {
        let mut batch: Vec<(u32, Aka)> = Vec::with_capacity(BATCH);
        each_line(
            bridge,
            MAX_AKAS_UNPACKED,
            AKAS_HEADER,
            "IMDb's titles",
            &stopped,
            |line| {
                let Some((work, aka)) = read_aka(line, &wanted) else {
                    return Ok(());
                };
                // A work's titles go in one batch: the list is sorted by
                // work, and a full batch waits for the next work to begin.
                if batch.len() >= BATCH && batch.last().is_some_and(|(last, _)| *last != work) {
                    sender
                        .blocking_send(std::mem::take(&mut batch))
                        .map_err(|_| anyhow!("the titles stopped being written"))?;
                }
                batch.push((work, aka));
                counted.kept.fetch_add(1, Ordering::Relaxed);
                Ok(())
            },
        )?;
        if !batch.is_empty() {
            sender
                .blocking_send(batch)
                .map_err(|_| anyhow!("the titles stopped being written"))?;
        }
        Ok(())
    });

    let mut written: HashSet<u32> = HashSet::new();
    let mut titles = 0usize;
    let mut say = tokio::time::interval(SAY_EVERY);
    let writing: Result<()> = async {
        loop {
            tokio::select! {
                batch = batches.recv() => match batch {
                    Some(batch) => titles += write_akas(state, &mut written, batch).await?,
                    None => return Ok(()),
                },
                _ = say.tick() => run.say(&tally.said(total, "titles")).await,
            }
        }
    }
    .await;
    // A reading still at it learns that the writing has stopped: at once
    // when it hands a batch over, else at its next look.
    if writing.is_err() {
        abandoned.store(true, Ordering::Relaxed);
    }
    drop(batches);
    let read = reading
        .await
        .context("the reading of IMDb's titles ended abruptly")?;

    // The writing's error is the one that says what went wrong: the reading
    // then only says it could no longer hand its batches over.
    writing?;
    match read {
        Err(e) if was_stopped(&e) => {
            return Ok(Ended::Stopped(format!(
                "stopped: {titles} titles for {} works",
                written.len()
            )));
        }
        other => other?,
    }

    // Nearly every title IMDb lists has one kept, the one it was first
    // released under. A list in which most of the works held have none was
    // misread: what it brought is written, the rest stays as it was.
    if works >= 50 && written.len() * 4 < works {
        bail!(
            "only {} of {works} works were found in IMDb's titles; the others keep theirs",
            written.len()
        );
    }

    // The works not met in the list — no longer held, or no longer listed —
    // keep nothing.
    let gone: Vec<String> = repo::imdb::titled(&state.db)
        .await?
        .into_iter()
        .filter(|tconst| {
            tconst_number(tconst.as_bytes()).is_none_or(|number| !written.contains(&number))
        })
        .collect();
    repo::imdb::forget_titles(&state.db, &gone).await?;
    repo::import::record(&state.db, IMDB_AKAS, titles as i64, &started).await?;

    Ok(Ended::Done(format!(
        "{titles} titles for {} of {works} works",
        written.len()
    )))
}

/// Write one batch of titles; the works this run had not written yet have
/// theirs replaced. What was written, counted.
async fn write_akas(
    state: &AppState,
    written: &mut HashSet<u32>,
    batch: Vec<(u32, Aka)>,
) -> Result<usize> {
    let mut fresh = Vec::new();
    let mut seen: HashSet<(u32, i64)> = HashSet::with_capacity(batch.len());
    let mut rows = Vec::with_capacity(batch.len());
    for (work, aka) in batch {
        if written.insert(work) {
            fresh.push(tconst_text(work));
        }
        // One row a number: a list that repeated a line would have one
        // statement write a row twice, which PostgreSQL refuses.
        if seen.insert((work, aka.ordering)) {
            rows.push(Title {
                tconst: tconst_text(work),
                ordering: aka.ordering,
                title: aka.title,
                region: aka.region,
                language: aka.language,
                types: aka.types,
                is_original: aka.is_original,
            });
        }
    }
    repo::imdb::write_titles(&state.db, &fresh, &rows).await?;
    Ok(rows.len())
}

// ─── IMDb's episodes ─────────────────────────────────────────────────────────

/// One episode of `title.episode` kept: the series', its own, its season and
/// its number.
type ListedEpisode = (u32, u32, i32, i32);

/// What one line of `title.episode` gives, when it is an episode kept: of a
/// series held here, numbered. Four columns: `tconst`, `parentTconst`,
/// `seasonNumber` and `episodeNumber`.
fn read_episode(line: &[u8], wanted: &HashSet<u32>) -> Option<ListedEpisode> {
    let mut columns = line.split(|byte| *byte == b'\t');
    let episode = columns.next()?;
    let series = tconst_number(columns.next()?)?;
    if !wanted.contains(&series) {
        return None;
    }
    let episode = tconst_number(episode)?;
    let season = i32::try_from(whole(columns.next()?)?).ok()?;
    let number = i32::try_from(whole(columns.next()?)?).ok()?;
    Some((series, episode, season, number))
}

/// Read IMDb's episodes through, keeping those of the series held here.
///
/// The list is sorted by episode rather than by series, so what is kept is
/// gathered whole before it is written — sixteen bytes an episode: a
/// catalogue of two thousand series, two hundred thousand episodes, three
/// megabytes — and then written a batch of series at a time.
async fn import_episodes(state: &AppState, run: &Run<'_>) -> Result<Ended> {
    let started = now();
    let wanted = numbers(&repo::imdb::series(&state.db).await?);
    let series_held = wanted.len();

    if wanted.is_empty() {
        let gone = repo::imdb::listed_series(&state.db).await?;
        repo::imdb::forget_series(&state.db, &gone).await?;
        repo::import::record(&state.db, IMDB_EPISODES, 0, &started).await?;
        return Ok(Ended::Done("0 episodes of 0 of 0 series".into()));
    }

    let url = format!(
        "{}/title.episode.tsv.gz",
        state.config.imdb.datasets.trim_end_matches('/')
    );
    let tally = Arc::new(Tally::default());
    let (reader, total) = open_list(state, &url, MAX_EPISODES_FILE, Arc::clone(&tally)).await?;
    let bridge = tokio_util::io::SyncIoBridge::new(reader);

    let stopped = run.stop_check();
    let counted = Arc::clone(&tally);
    let mut reading = tokio::task::spawn_blocking(move || -> Result<Vec<ListedEpisode>> {
        let mut found = Vec::new();
        each_line(
            bridge,
            MAX_EPISODES_UNPACKED,
            EPISODES_HEADER,
            "IMDb's episodes",
            &stopped,
            |line| {
                if let Some(episode) = read_episode(line, &wanted) {
                    found.push(episode);
                    counted.kept.fetch_add(1, Ordering::Relaxed);
                }
                Ok(())
            },
        )?;
        Ok(found)
    });

    let mut say = tokio::time::interval(SAY_EVERY);
    let read = loop {
        tokio::select! {
            read = &mut reading => break read.context("the reading of IMDb's episodes ended abruptly")?,
            _ = say.tick() => run.say(&tally.said(total, "episodes")).await,
        }
    };
    let mut found = match read {
        Ok(found) => found,
        Err(e) if was_stopped(&e) => {
            return Ok(Ended::Stopped("stopped: 0 episodes for 0 series".into()));
        }
        Err(e) => return Err(e),
    };

    // By series, each series' episodes in their order.
    found.sort_unstable_by_key(|&(series, episode, season, number)| {
        (series, season, number, episode)
    });
    found.dedup_by_key(|&mut (_, episode, _, _)| episode);
    let mut listed: Vec<u32> = found.iter().map(|&(series, ..)| series).collect();
    listed.dedup();

    // Nearly every series IMDb lists numbers its episodes. A list in which
    // most of the series held have none was misread.
    if series_held >= 20 && listed.len() * 4 < series_held {
        bail!(
            "only {} of {series_held} series were found in IMDb's episodes; the others keep theirs",
            listed.len()
        );
    }

    // A batch of whole series at a time; between two, a look at whether the
    // run was asked to stop.
    let mut written: HashSet<u32> = HashSet::new();
    let mut episodes = 0usize;
    let mut at = 0;
    while at < found.len() {
        if run.stopped() {
            return Ok(Ended::Stopped(format!(
                "stopped: {episodes} episodes for {} series",
                written.len()
            )));
        }
        let mut end = (at + BATCH).min(found.len());
        while end < found.len() && found[end].0 == found[end - 1].0 {
            end += 1;
        }
        let batch = &found[at..end];
        let mut fresh = Vec::new();
        for &(series, ..) in batch {
            if written.insert(series) {
                fresh.push(tconst_text(series));
            }
        }
        let rows: Vec<repo::imdb::Episode> = batch
            .iter()
            .map(|&(series, episode, season, number)| repo::imdb::Episode {
                tconst: tconst_text(episode),
                parent: tconst_text(series),
                season_number: season,
                episode_number: number,
            })
            .collect();
        repo::imdb::write_episodes(&state.db, &fresh, &rows).await?;
        episodes += rows.len();
        at = end;
    }

    let gone: Vec<String> = repo::imdb::listed_series(&state.db)
        .await?
        .into_iter()
        .filter(|tconst| {
            tconst_number(tconst.as_bytes()).is_none_or(|number| !written.contains(&number))
        })
        .collect();
    repo::imdb::forget_series(&state.db, &gone).await?;
    repo::import::record(&state.db, IMDB_EPISODES, episodes as i64, &started).await?;

    Ok(Ended::Done(format!(
        "{episodes} episodes of {} of {series_held} series",
        written.len()
    )))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn the_anime_list_becomes_rows_for_what_can_be_looked_up() {
        let body = br#"[
            {"type": "TV", "anidb_id": 9541, "anilist_id": 16498, "mal_id": 16498,
             "themoviedb_id": {"tv": 1429}, "tvdb_id": 267440, "season": {"tvdb": 1, "tmdb": 1}},
            {"type": "TV", "anilist_id": 104578, "mal_id": 38524, "tvdb_id": 267440,
             "season": {"tvdb": 3}, "episode_offset": {"tvdb": 12}},
            {"type": "MOVIE", "anilist_id": 21519, "mal_id": 32281,
             "themoviedb_id": {"movie": [372058]}},
            {"type": "MOVIE", "mal_id": 460, "tvdb_id": 81797, "season": {"tvdb": 0},
             "themoviedb_id": {"movie": [23446, 23447]}},
            {"type": "TV", "anidb_id": 1, "tvdb_id": 5},
            {"type": "TV", "mal_id": 7},
            {"type": "TV", "mal_id": "not a number", "tvdb_id": 8}
        ]"#;

        let rows = parse_anime(body).unwrap();

        assert_eq!(rows.len(), 5, "{rows:#?}");

        assert_eq!(rows[0].tvdb_id, Some(267440));
        assert_eq!(rows[0].tvdb_season, Some(1));
        assert_eq!(
            rows[0].tmdb_movie, None,
            "a series' TMDB id is not a film's"
        );

        assert_eq!(rows[1].tvdb_offset, Some(12));

        assert_eq!(rows[2].tmdb_movie, Some(372058));
        assert_eq!(rows[2].tvdb_id, None);

        // A film TheTVDB files under a series keeps both, once per TMDB id.
        assert_eq!(rows[3].tmdb_movie, Some(23446));
        assert_eq!(rows[4].tmdb_movie, Some(23447));
        assert_eq!(rows[4].tvdb_id, Some(81797));
    }

    #[test]
    fn a_list_that_is_not_json_is_refused() {
        assert!(parse_anime(b"<html>rate limited</html>").is_err());
    }

    fn gzip(text: &str) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    fn wanted(ids: &[&str]) -> HashSet<u32> {
        ids.iter()
            .map(|id| tconst_number(id.as_bytes()).unwrap())
            .collect()
    }

    #[test]
    fn only_the_wanted_ratings_are_kept() {
        let file = gzip(
            "tconst\taverageRating\tnumVotes\n\
             tt0903747\t9.5\t2300000\n\
             tt0000001\t5.7\t2100\n\
             tt2560140\t9.1\t750000\n\
             tt9999999\tnot-a-number\t3\n",
        );

        let ratings =
            parse_ratings(&file, &wanted(&["tt0903747", "tt2560140", "tt9999999"])).unwrap();

        assert_eq!(
            ratings,
            vec![
                Rating {
                    tconst: "tt0903747".into(),
                    rating: 9.5,
                    votes: 2_300_000
                },
                Rating {
                    tconst: "tt2560140".into(),
                    rating: 9.1,
                    votes: 750_000
                },
            ]
        );
    }

    #[test]
    fn a_ratings_file_that_unpacks_into_a_monster_is_refused() {
        // Compresses to almost nothing; would unpack into one line of any size.
        let file = gzip(&format!(
            "tconst\taverageRating\tnumVotes\n{}",
            "a".repeat(100_000)
        ));
        let error = parse_ratings(&file, &HashSet::new()).unwrap_err();

        assert!(
            error.to_string().contains("longer than any it should"),
            "{error:#}"
        );
    }

    #[test]
    fn a_ratings_file_without_its_header_is_refused() {
        let file = gzip("<html>blocked</html>\n");
        assert!(parse_ratings(&file, &HashSet::new()).is_err());
        assert!(parse_ratings(b"not gzip at all", &HashSet::new()).is_err());
    }

    #[test]
    fn an_imdb_id_is_read_as_imdb_writes_it_and_written_back_the_same() {
        assert_eq!(tconst_number(b"tt0903747"), Some(903_747));
        assert_eq!(tconst_number(b"tt10872600"), Some(10_872_600));
        assert_eq!(tconst_text(903_747), "tt0903747");
        assert_eq!(tconst_text(10_872_600), "tt10872600");
        assert_eq!(tconst_text(1), "tt0000001");
        // Not as IMDb writes one: too short, a zero too many, not digits,
        // past what a number of four bytes holds, another prefix.
        for text in [
            "tt903747",
            "tt00903747",
            "tt09O3747",
            "tt99999999999",
            "nm0000151",
            "tt",
            "",
        ] {
            assert_eq!(tconst_number(text.as_bytes()), None, "{text}");
        }
    }

    fn lines_of(file: &[u8], header: &str) -> Result<Vec<String>> {
        let mut seen = Vec::new();
        each_line(file, 1 << 20, header, "the list", &|| false, |line| {
            seen.push(String::from_utf8(line.to_vec()).unwrap());
            Ok(())
        })?;
        Ok(seen)
    }

    #[test]
    fn a_list_is_read_a_line_at_a_time_under_its_header() {
        let file = gzip("a\tb\r\none\ntwo\r\n\nthree");
        assert_eq!(
            lines_of(&file, "a\tb").unwrap(),
            ["one", "two", "", "three"]
        );

        let error = lines_of(&gzip("<html>\n"), "a\tb").unwrap_err();
        assert!(error.to_string().contains("header"), "{error:#}");
        assert!(
            lines_of(&gzip(""), "a\tb")
                .unwrap_err()
                .to_string()
                .contains("empty")
        );
        assert!(lines_of(b"not gzip", "a\tb").is_err());

        // A line past any IMDb writes, and a file that unpacks past its bound.
        let long = gzip(&format!("a\tb\n{}\n", "x".repeat(5_000)));
        assert!(
            lines_of(&long, "a\tb")
                .unwrap_err()
                .to_string()
                .contains("longer than any it should")
        );
        let big = gzip(&format!("a\tb\n{}", "line\n".repeat(300_000)));
        assert!(
            lines_of(&big, "a\tb")
                .unwrap_err()
                .to_string()
                .contains("unpacks to more than")
        );
    }

    #[test]
    fn a_reading_asked_to_stop_stops() {
        let file = gzip(&format!("h\n{}", "line\n".repeat(200_000)));
        let mut seen = 0u64;
        let error = each_line(file.as_slice(), 1 << 30, "h", "the list", &|| true, |_| {
            seen += 1;
            Ok(())
        })
        .unwrap_err();

        assert!(was_stopped(&error), "{error:#}");
        assert!(seen < LOOK_EVERY, "{seen} lines read before the first look");
    }

    #[test]
    fn the_titles_kept_are_a_held_works_of_the_types_radarr_is_given() {
        let held = wanted(&["tt0133093"]);
        let read = |line: &str| read_aka(line.as_bytes(), &held);

        assert_eq!(
            read("tt0133093\t12\tMatrix\tFR\t\\N\timdbDisplay\t\\N\t0"),
            Some((
                133_093,
                Aka {
                    ordering: 12,
                    title: "Matrix".into(),
                    region: Some("FR".into()),
                    language: None,
                    types: "imdbDisplay".into(),
                    is_original: false,
                }
            ))
        );
        // The title it was first released under, with no region.
        let original = read("tt0133093\t1\tThe Matrix\t\\N\t\\N\toriginal\t\\N\t1").unwrap();
        assert!(original.1.is_original);
        assert_eq!(original.1.region, None);
        // A list of types, one of them kept.
        assert_eq!(
            read("tt0133093\t30\tThe Matrix\tXWW\ten\tdvd\u{2}alternative\t\\N\t0")
                .unwrap()
                .1
                .types,
            "alternative"
        );

        // Of no type kept; a work not held; no title; a broken line.
        for line in [
            "tt0133093\t3\tMatrix (Working Title)\tUS\t\\N\tworking\t\\N\t0",
            "tt0133093\t4\tMatrix\tDE\t\\N\t\\N\tliteral title\t0",
            "tt0000001\t1\tCarmencita\tUS\t\\N\timdbDisplay\t\\N\t0",
            "tt0133093\t5\t\\N\tUS\t\\N\timdbDisplay\t\\N\t0",
            "tt0133093\tfive\tMatrix\tUS\t\\N\timdbDisplay\t\\N\t0",
            "tt0133093\t6\tMatrix",
        ] {
            assert_eq!(read(line), None, "{line}");
        }
    }

    #[test]
    fn the_episodes_kept_are_a_held_series_numbered() {
        let held = wanted(&["tt0903747"]);
        let read = |line: &str| read_episode(line.as_bytes(), &held);

        assert_eq!(
            read("tt0959621\ttt0903747\t1\t1"),
            Some((903_747, 959_621, 1, 1))
        );
        for line in [
            "tt0959621\ttt0903747\t\\N\t\\N",
            "tt0959621\ttt0903747\t1\t\\N",
            "tt0041951\ttt0041038\t1\t9",
            "tt0959621\ttt0903747\t1",
            "not-an-id\ttt0903747\t1\t1",
        ] {
            assert_eq!(read(line), None, "{line}");
        }
    }

    #[test]
    fn a_reading_says_how_far_it_has_got_as_the_tasks_page_reads_it() {
        let tally = Tally::default();
        tally.read.store(212_000_000, Ordering::Relaxed);
        tally.kept.store(1740, Ordering::Relaxed);

        let said = tally.said(Some(518_980_095), "titles");
        assert_eq!(said, "212 of 519: megabytes read, 1740 titles kept");
        assert_eq!(
            crate::jobs::tasks::progress_of(Some(&said)),
            Some(crate::jobs::tasks::Progress {
                done: 212,
                total: Some(519)
            })
        );

        let said = tally.said(None, "episodes");
        assert_eq!(said, "212 megabytes read, 1740 episodes kept");
        assert_eq!(
            crate::jobs::tasks::progress_of(Some(&said)),
            Some(crate::jobs::tasks::Progress {
                done: 212,
                total: None
            })
        );
    }

    #[test]
    fn every_list_is_filed_under_its_task_and_back() {
        for list in LISTS {
            assert_eq!(list_of(kind_of(list)), Some(list));
        }
        assert_eq!(list_of(job::kinds::IMDB_AKAS), Some(IMDB_AKAS));
        assert_eq!(list_of("refresh.sweep"), None);
    }

    // ─── read through, from a server of IMDb's lists ─────────────────────────

    use axum::response::IntoResponse as _;

    use crate::{
        domain::{MediaItem, MediaKind},
        settings::registry::Scope,
    };

    const AKAS_HEAD: &str =
        "titleId\tordering\ttitle\tregion\tlanguage\ttypes\tattributes\tisOriginalTitle\n";
    const EPISODES_HEAD: &str = "tconst\tparentTconst\tseasonNumber\tepisodeNumber\n";
    const RATINGS_HEAD: &str = "tconst\taverageRating\tnumVotes\n";

    /// A list as IMDb's server sends it: whole, or its first half at once
    /// and the rest once `hold` lets it go.
    struct Served {
        body: Vec<u8>,
        hold: Option<Arc<tokio::sync::Notify>>,
    }

    /// IMDb's lists, served from here as IMDb serves them, by file name.
    async fn imdb_at(files: Vec<(&'static str, Served)>) -> String {
        let files: Arc<HashMap<String, Served>> = Arc::new(
            files
                .into_iter()
                .map(|(name, served)| (format!("/{name}"), served))
                .collect(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let at = format!("http://{}", listener.local_addr().expect("its address"));
        let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
            let files = Arc::clone(&files);
            async move {
                let Some(served) = files.get(uri.path()) else {
                    return axum::http::StatusCode::NOT_FOUND.into_response();
                };
                let Some(hold) = served.hold.clone() else {
                    return served.body.clone().into_response();
                };
                let (first, rest) = served.body.split_at(served.body.len() / 2);
                let first = Bytes::copy_from_slice(first);
                let rest = Bytes::copy_from_slice(rest);
                let chunks = futures::stream::once(async move { Ok::<_, std::io::Error>(first) })
                    .chain(futures::stream::once(async move {
                        hold.notified().await;
                        Ok(rest)
                    }));
                axum::body::Body::from_stream(chunks).into_response()
            }
        });
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("served");
        });
        at
    }

    fn at_once(text: String) -> Served {
        Served {
            body: gzip(&text),
            hold: None,
        }
    }

    /// [`import_now`], waiting its turn: the import lock is the process's,
    /// and another test's import may be holding it — as another person's
    /// would, answered `Busy`.
    async fn import_when_free(state: &AppState, list: &str, by: Option<&str>) -> Asked {
        for _ in 0..1200 {
            match import_now(state, list, by).await {
                Asked::Busy => tokio::time::sleep(Duration::from_millis(50)).await,
                asked => return asked,
            }
        }
        panic!("the import lock stayed taken for a minute");
    }

    /// The server, IMDb's lists at `at`, its ratings, titles and episodes
    /// switched on.
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
        config.imdb.datasets = at.to_string();

        let state = AppState::bootstrap(config).await.expect("a server");
        for key in ["imdb.enabled", "imdb.akas", "imdb.episodes"] {
            state
                .settings
                .set(Scope::Server, "", key, "true", None)
                .await
                .expect("switched on");
        }
        state
    }

    /// A work held, under an IMDb id, with episodes so numbered.
    async fn held(
        state: &AppState,
        kind: MediaKind,
        title: &str,
        imdb: &str,
        episodes: &[(i32, i32)],
    ) -> MediaItem {
        let mut item = MediaItem::empty(kind);
        item.id = crate::db::new_id();
        item.title = title.into();
        item.slug = crate::domain::make_slug(title, None);
        item.created_at = now();
        item.updated_at = now();
        item.external_ids.imdb = Some(imdb.into());
        item.episodes = episodes
            .iter()
            .map(|&(season, number)| {
                let mut episode = repo::child::blank_episode(season, number);
                episode.id = crate::db::new_id();
                episode.is_manual = false;
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
        item
    }

    fn title(tconst: &str, ordering: i64, text: &str, region: &str) -> Title {
        Title {
            tconst: tconst.into(),
            ordering,
            title: text.into(),
            region: Some(region.into()),
            language: None,
            types: "imdbDisplay".into(),
            is_original: false,
        }
    }

    #[tokio::test]
    async fn imdbs_titles_and_episodes_are_read_for_the_works_held_and_given_where_they_go() {
        let akas = format!(
            "{AKAS_HEAD}\
             tt0000001\t1\tCarmencita\tUS\t\\N\timdbDisplay\t\\N\t0\n\
             tt0133093\t1\tThe Matrix\t\\N\t\\N\toriginal\t\\N\t1\n\
             tt0133093\t2\tMatrix\tFR\t\\N\timdbDisplay\t\\N\t0\n\
             tt0133093\t3\tMatrix : le film\tFR\tfr\talternative\t\\N\t0\n\
             tt0133093\t4\tThe Matrix\tUS\t\\N\timdbDisplay\t\\N\t0\n\
             tt0133093\t5\tThe Matrix (working)\tUS\t\\N\tworking\t\\N\t0\n\
             tt0133093\t6\tMatrice\tCA\tfr\timdbDisplay\t\\N\t0\n"
        );
        let episodes = format!(
            "{EPISODES_HEAD}\
             tt0959621\ttt0903747\t1\t1\n\
             tt1054724\ttt0903747\t1\t2\n\
             tt1054725\ttt0903747\t1\t3\n\
             tt1054726\ttt0903747\t1\t3\n\
             tt1054727\ttt0903747\t\\N\t\\N\n\
             tt0041951\ttt0041038\t1\t9\n"
        );
        let ratings = format!(
            "{RATINGS_HEAD}\
             tt0133093\t8.7\t2200000\n\
             tt0903747\t9.5\t2300000\n\
             tt0959621\t9.0\t41000\n\
             tt0000001\t5.7\t2100\n"
        );
        let at = imdb_at(vec![
            ("title.akas.tsv.gz", at_once(akas)),
            ("title.episode.tsv.gz", at_once(episodes)),
            ("title.ratings.tsv.gz", at_once(ratings)),
        ])
        .await;
        let state = server(&at).await;

        let film = held(&state, MediaKind::Movie, "The Matrix", "tt0133093", &[]).await;
        let series = held(
            &state,
            MediaKind::Series,
            "Breaking Bad",
            "tt0903747",
            &[(1, 1), (1, 2), (1, 3), (2, 1)],
        )
        .await;
        // What a work no longer held left behind.
        repo::imdb::write_titles(&state.db, &[], &[title("tt7654321", 1, "Gone", "FR")])
            .await
            .unwrap();
        repo::imdb::write_episodes(
            &state.db,
            &[],
            &[repo::imdb::Episode {
                tconst: "tt7654322".into(),
                parent: "tt7654321".into(),
                season_number: 1,
                episode_number: 1,
            }],
        )
        .await
        .unwrap();

        // The episodes, then the ratings — which take theirs too — and the
        // titles, as the schedule takes them.
        let said = |asked: Asked| match asked {
            Asked::Imported(summary) => summary,
            Asked::Failed(e) => panic!("{e:#}"),
            _ => panic!("not imported"),
        };
        assert_eq!(
            said(import_when_free(&state, IMDB_EPISODES, None).await),
            "4 episodes of 1 of 1 series"
        );
        assert_eq!(
            said(import_when_free(&state, IMDB, None).await),
            "2 of 2 works rated, 1 of 4 episodes"
        );
        assert_eq!(
            said(import_when_free(&state, IMDB_AKAS, None).await),
            "5 titles for 1 of 2 works"
        );

        // Kept for the works held alone.
        assert_eq!(repo::imdb::titled(&state.db).await.unwrap(), ["tt0133093"]);
        assert_eq!(
            repo::imdb::listed_series(&state.db).await.unwrap(),
            ["tt0903747"]
        );

        // Radarr is given the film's titles in its regions, beside what it
        // is sent already, and only once each.
        let fr = ["FR".to_string()];
        let titles =
            crate::service::imdb::regional_titles(&state, std::slice::from_ref(&film), &fr).await;
        assert_eq!(
            titles[0]
                .iter()
                .map(|t| (t.title.as_str(), t.language.as_deref()))
                .collect::<Vec<_>>(),
            [("Matrix", None), ("Matrix : le film", Some("fr"))]
        );
        let mut resource = crate::wire::radarr::from_item(&film);
        crate::wire::radarr::add_regional_titles(&mut resource, &titles[0]);
        assert_eq!(
            resource
                .alternative_titles
                .iter()
                .map(|t| (
                    t.title.as_str(),
                    t.title_type.as_str(),
                    t.language.as_deref()
                ))
                .collect::<Vec<_>>(),
            [
                ("Matrix", "Tmdb", None),
                ("Matrix : le film", "Tmdb", Some("fr"))
            ]
        );
        let us = ["US".to_string()];
        let titles =
            crate::service::imdb::regional_titles(&state, std::slice::from_ref(&film), &us).await;
        let mut resource = crate::wire::radarr::from_item(&film);
        crate::wire::radarr::add_regional_titles(&mut resource, &titles[0]);
        assert!(
            resource.alternative_titles.is_empty(),
            "the US title is the film's own"
        );

        // Each episode IMDb numbers alike is matched, with its rating where
        // IMDb gives one; a number IMDb gives twice matches nothing.
        let matched = crate::service::imdb::episodes(&state, &series.id, None)
            .await
            .unwrap();
        assert_eq!(
            matched
                .iter()
                .map(|m| (
                    m.season_number,
                    m.episode_number,
                    m.tconst.as_str(),
                    m.rating
                ))
                .collect::<Vec<_>>(),
            [(1, 1, "tt0959621", Some(9.0)), (1, 2, "tt1054724", None)]
        );
        assert_eq!(matched[0].votes, Some(41_000));
        assert!(
            crate::service::imdb::episodes(&state, &series.id, Some(2))
                .await
                .unwrap()
                .is_empty()
        );
        let ids = crate::service::imdb::episode_ids(&state, &series.id).await;
        assert_eq!(
            ids.get(&series.episodes[0].id).map(String::as_str),
            Some("tt0959621")
        );
        assert_eq!(ids.len(), 2);

        // Each where its switch is on, whatever the tables hold.
        state
            .settings
            .set(Scope::Server, "", "imdb.akas", "false", None)
            .await
            .unwrap();
        state
            .settings
            .set(Scope::Server, "", "imdb.enabled", "false", None)
            .await
            .unwrap();
        assert!(crate::service::imdb::regional_titles(&state, &[film], &fr).await[0].is_empty());
        let unrated = crate::service::imdb::episodes(&state, &series.id, None)
            .await
            .unwrap();
        assert_eq!(unrated.len(), 2);
        assert!(unrated.iter().all(|m| m.rating.is_none()));
        state
            .settings
            .set(Scope::Server, "", "imdb.episodes", "false", None)
            .await
            .unwrap();
        assert!(
            crate::service::imdb::episode_ids(&state, &series.id)
                .await
                .is_empty()
        );

        // The tasks say they ran, and come again in a week.
        state
            .settings
            .set(Scope::Server, "", "imdb.akas", "true", None)
            .await
            .unwrap();
        let task = crate::jobs::tasks::state_named(&state, crate::jobs::tasks::TaskId::ImdbAkas)
            .await
            .unwrap();
        assert_eq!(task.mode, crate::jobs::tasks::Mode::Scheduled);
        assert_eq!(task.every_seconds, Some(7 * 24 * 60 * 60));
        assert_eq!(
            task.last.map(|run| run.status),
            Some(job::Status::Succeeded)
        );
        assert!(task.schedulable);
    }

    #[tokio::test]
    async fn a_list_of_titles_holding_almost_none_of_the_works_is_not_taken_for_the_whole() {
        let akas = format!(
            "{AKAS_HEAD}\
             tt1000001\t1\tUn\tFR\t\\N\timdbDisplay\t\\N\t0\n\
             tt1000002\t1\tDeux\tFR\t\\N\timdbDisplay\t\\N\t0\n"
        );
        let at = imdb_at(vec![("title.akas.tsv.gz", at_once(akas))]).await;
        let state = server(&at).await;
        for n in 1..=60 {
            held(
                &state,
                MediaKind::Movie,
                &format!("Film {n}"),
                &tconst_text(1_000_000 + n),
                &[],
            )
            .await;
        }
        // What an earlier run kept of the sixtieth.
        repo::imdb::write_titles(&state.db, &[], &[title("tt1000060", 1, "Kept", "FR")])
            .await
            .unwrap();

        let Asked::Failed(e) = import_when_free(&state, IMDB_AKAS, None).await else {
            panic!("taken");
        };
        assert!(e.to_string().contains("only 2 of 60 works"), "{e:#}");

        // What it read is written; what was kept of the others stays.
        assert_eq!(
            repo::imdb::titled(&state.db).await.unwrap().len(),
            3,
            "the two read, and the one kept"
        );
        let (last, _) = job::latest(&state.db, job::kinds::IMDB_AKAS).await.unwrap();
        assert_eq!(last.map(|run| run.status), Some(job::Status::Failed));
    }

    #[tokio::test]
    async fn a_reading_of_the_titles_stops_when_asked_and_waits_an_hour() {
        // Enough lines that, the first half read, another look comes after
        // the stop is asked for: a look every 65 536 lines.
        let lines: String = (0..400_000)
            .map(|n| {
                format!(
                    "tt{:07}\t1\tTitle {n}\tFR\t\\N\timdbDisplay\t\\N\t0\n",
                    2_000_000 + n
                )
            })
            .collect();
        let hold = Arc::new(tokio::sync::Notify::new());
        let at = imdb_at(vec![(
            "title.akas.tsv.gz",
            Served {
                body: gzip(&format!("{AKAS_HEAD}{lines}")),
                hold: Some(Arc::clone(&hold)),
            },
        )])
        .await;
        let state = server(&at).await;
        held(&state, MediaKind::Movie, "Title 1", "tt2000001", &[]).await;

        let running = tokio::spawn({
            let state = state.clone();
            async move { import_when_free(&state, IMDB_AKAS, Some("tester")).await }
        });

        // As the Jobs page asks: the run, once it can be stopped.
        let mut run = None;
        for _ in 0..3000 {
            if let Ok((Some(job), _)) = job::latest(&state.db, job::kinds::IMDB_AKAS).await
                && job.status == job::Status::Running
                && cancel::is_cancelable(&job.id)
            {
                run = Some(job);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let run = run.expect("a run that can be stopped");
        assert!(crate::jobs::tasks::stops_partway(&run.kind));
        assert!(cancel::cancel(&run.id));
        hold.notify_one();

        let Asked::Stopped(summary) = running.await.unwrap() else {
            panic!("not stopped");
        };
        assert!(summary.starts_with("stopped: "), "{summary}");
        let (last, _) = job::latest(&state.db, job::kinds::IMDB_AKAS).await.unwrap();
        let last = last.unwrap();
        assert_eq!(last.status, job::Status::Stopped);
        assert_eq!(last.triggered_by.as_deref(), Some("tester"));

        // Not started again at the next look: an hour later at the soonest.
        let clock = clock(&state, IMDB_AKAS).await.unwrap();
        let wait = clock.not_before.unwrap() - chrono::Utc::now();
        assert!(wait > chrono::Duration::minutes(55), "{wait}");
    }
}
