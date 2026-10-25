//! Works brought in by the batch: ids somebody pasted or dropped as a file,
//! read, set beside the catalogue, and imported one after another as a run of
//! its own — the way the seed imports Sonarr's and Radarr's libraries, and
//! the way the import rules bring theirs in on a schedule.
//!
//! A line names one work: `tmdb:603` (with the kind it is, a TMDB id being a
//! film's or a series'), `imdb:tt0133093` or `tt0133093`, `tvdb:81189`,
//! `anilist:21`, `mal:21`, `fankai:12`, or the work's page at TMDB, IMDb,
//! TheTVDB, AniList or MyAnimeList. A comment after `#` is left out, as a
//! text file of ids writes its titles.
//!
//! Before anything is imported, the lines are set beside the catalogue: each
//! is already held here, a candidate — a provider asked of it when there is
//! time, its title said — or not found, or no id at all. Nothing is fetched
//! to say so: a lookup here is an import.
//!
//! The run takes the works in order, a pause after each that had to be
//! fetched; each is written down for it — `imported`, `skipped` when it was
//! held already, `failed` when no provider had it — and it says how many of
//! each as it goes: `3 of 500: 1 already here, 2 imported, 0 not found, 0
//! failed`. It can be stopped between two works.

use std::{
    collections::HashSet,
    sync::{Arc, LazyLock},
    time::Duration,
};

use anyhow::Result;
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    db::repo::{self, item::WorkRef, job},
    domain::{ExternalSource, MediaItem, MediaKind},
    error::{AppError, AppResult},
    jobs::cancel,
    service,
    state::AppState,
};

/// The task's name, as the history files its runs, and what a run is held
/// under among several instances.
pub const KIND: &str = "import.batch";
pub const HOLD: &str = KIND;

/// The most lines one batch takes: a file of a few hundred ids is what this
/// is for, and past this many the run would be hours of a provider's time.
pub const MOST_LINES: usize = 2_000;

/// The most works a selection holds, as the lists' own page has it.
pub const LIST_MOST: usize = 500;

/// The pause after each work that had to be fetched: the providers are asked
/// for a whole file, and are in no hurry.
const PAUSE: Duration = Duration::from_millis(250);

/// How long a preview may spend asking the providers about the candidates,
/// and how many it asks at once. What is left when the time is up is said
/// to be unchecked: it is looked up as it is imported.
const PREVIEW_BUDGET: Duration = Duration::from_secs(12);
const PREVIEW_AT_ONCE: usize = 6;

/// Held by a run, so that two batches never import at once.
static IMPORTING: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

/// Whether a batch is being imported on this instance.
pub fn is_busy() -> bool {
    IMPORTING.try_lock().is_err()
}

// ─── reading the lines ───────────────────────────────────────────────────────

/// Where an id is from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = IdSource)]
pub enum Source {
    Tmdb,
    Imdb,
    Tvdb,
    Anilist,
    Mal,
    Fankai,
}

impl Source {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tmdb => "tmdb",
            Self::Imdb => "imdb",
            Self::Tvdb => "tvdb",
            Self::Anilist => "anilist",
            Self::Mal => "mal",
            Self::Fankai => "fankai",
        }
    }
}

/// One work wanted, by its id at a source.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WantedWork)]
pub struct Wanted {
    pub source: Source,
    /// The id there: a number, or `tt…` for IMDb.
    pub id: String,
    /// `movie` or `series`, where it is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<MediaKind>,
}

impl Wanted {
    /// As a line would write it: `tmdb:603`.
    pub fn label(&self) -> String {
        match self.source {
            Source::Imdb => self.id.clone(),
            source => format!("{}:{}", source.as_str(), self.id),
        }
    }

    /// The ids it is held under here, the first one to look for first.
    fn held_as(&self) -> Vec<ExternalSource> {
        match (self.source, self.kind) {
            (Source::Tmdb, Some(MediaKind::Movie)) => vec![ExternalSource::TmdbMovie],
            (Source::Tmdb, Some(MediaKind::Series)) => vec![ExternalSource::TmdbTv],
            (Source::Tmdb, None) => Vec::new(),
            (Source::Imdb, _) => vec![ExternalSource::Imdb],
            (Source::Tvdb, _) => vec![ExternalSource::TvdbSeries],
            (Source::Anilist, _) => vec![ExternalSource::AniList],
            (Source::Mal, _) => vec![ExternalSource::Mal],
            (Source::Fankai, _) => vec![ExternalSource::Fankai],
        }
    }
}

/// A line of the source, as somebody gave it.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ImportLine)]
pub struct Line {
    /// `tmdb:603`, `imdb:tt0133093` or `tt0133093`, `tvdb:81189`,
    /// `anilist:21`, `mal:21`, `fankai:12`, or the work's page at TMDB,
    /// IMDb, TheTVDB, AniList or MyAnimeList.
    pub text: String,
    /// `movie` or `series`, where the line says: a TMDB id is one or the
    /// other.
    #[serde(default)]
    pub kind: Option<MediaKind>,
    /// What the line calls the work, said beside one not found.
    #[serde(default)]
    pub title: Option<String>,
}

/// Why a line names no work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unread {
    /// Empty, or a comment: passed over without a word.
    Nothing,
    /// Not an id this server reads.
    Unreadable,
    /// A TMDB id with nothing to say whether it is a film's or a series'.
    NeedsKind,
}

/// The work a line names, or why it names none. `kind` stands for a TMDB
/// id's kind where the line does not say.
pub fn read_line(line: &Line, kind: Option<MediaKind>) -> Result<Wanted, Unread> {
    let text = strip_comment(&line.text);
    if text.is_empty() {
        return Err(Unread::Nothing);
    }
    let (term, linked_kind) = match from_link(text) {
        Some((term, kind)) => (term, kind),
        None if is_tconst(text) => (format!("imdb:{text}"), None),
        None => (text.to_string(), None),
    };
    // What the line itself says — its own kind, or its page's. The kind
    // given for all is a TMDB id's only: an IMDb id names a film or a series
    // whatever the batch is mostly of.
    let said = line.kind.or(linked_kind);
    let wanted = |source: Source, id: String, kind: Option<MediaKind>| Wanted { source, id, kind };
    match service::ids::classify(&term) {
        service::ids::TermLookup::Tmdb(id) => match said.or(kind) {
            Some(kind) => Ok(wanted(Source::Tmdb, id.to_string(), Some(kind))),
            None => Err(Unread::NeedsKind),
        },
        service::ids::TermLookup::Imdb(id) => Ok(wanted(Source::Imdb, id, said)),
        service::ids::TermLookup::Tvdb(id) => Ok(wanted(
            Source::Tvdb,
            id.to_string(),
            Some(MediaKind::Series),
        )),
        service::ids::TermLookup::AniList(id) => Ok(wanted(
            Source::Anilist,
            id.to_string(),
            Some(MediaKind::Series),
        )),
        service::ids::TermLookup::Mal(id) => {
            Ok(wanted(Source::Mal, id.to_string(), Some(MediaKind::Series)))
        }
        service::ids::TermLookup::Fankai(id) => Ok(wanted(
            Source::Fankai,
            id.to_string(),
            Some(MediaKind::Series),
        )),
        service::ids::TermLookup::Text(_) => Err(Unread::Unreadable),
    }
}

/// A line without what follows a `#` that starts it or follows a space: a
/// text file of ids writes a work's title so, and an address may hold a
/// `#` of its own.
fn strip_comment(text: &str) -> &str {
    let text = text.trim();
    if text.starts_with('#') {
        return "";
    }
    let cut = text
        .char_indices()
        .find(|(at, c)| *c == '#' && text[..*at].ends_with(char::is_whitespace))
        .map_or(text.len(), |(at, _)| at);
    text[..cut].trim()
}

/// The lookup a provider's page stands for: its address pasted whole, as the
/// `prefix:id` term the search already understands, and the kind of work
/// when the address says.
pub fn from_link(term: &str) -> Option<(String, Option<MediaKind>)> {
    let url = url::Url::parse(term.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }

    let host = url.host_str()?.to_ascii_lowercase();
    let host = host
        .strip_prefix("www.")
        .or_else(|| host.strip_prefix("m."))
        .unwrap_or(&host);
    let parts: Vec<&str> = url
        .path_segments()
        .map(|segments| segments.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();

    // `1396-breaking-bad`, `21`: the number a slug starts with.
    let number = |segment: &str| -> Option<i64> {
        let digits: String = segment.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };

    match (host, parts.as_slice()) {
        ("themoviedb.org", ["tv", id, ..]) => {
            Some((format!("tmdb:{}", number(id)?), Some(MediaKind::Series)))
        }
        ("themoviedb.org", ["movie", id, ..]) => {
            Some((format!("tmdb:{}", number(id)?), Some(MediaKind::Movie)))
        }
        // In the reader's language too: `/fr/title/tt…`.
        ("imdb.com", ["title", id, ..]) | ("imdb.com", [_, "title", id, ..]) if is_tconst(id) => {
            Some((format!("imdb:{id}"), None))
        }
        ("anilist.co", ["anime", id, ..]) => {
            Some((format!("anilist:{}", number(id)?), Some(MediaKind::Series)))
        }
        ("myanimelist.net", ["anime", id, ..]) => {
            Some((format!("mal:{}", number(id)?), Some(MediaKind::Series)))
        }
        // TheTVDB's pages are named by slug; only its older addresses, and the
        // ones it redirects through, carry the number.
        ("thetvdb.com", ["dereferrer", "series", id, ..]) | ("thetvdb.com", ["series", id, ..]) => {
            number(id)
                .filter(|_| id.bytes().all(|b| b.is_ascii_digit()))
                .map(|id| (format!("tvdb:{id}"), Some(MediaKind::Series)))
        }
        ("thetvdb.com", _) => url
            .query_pairs()
            .find(|(key, _)| key == "id" || key == "seriesid")
            .and_then(|(_, value)| value.parse::<i64>().ok())
            .map(|id| (format!("tvdb:{id}"), Some(MediaKind::Series))),
        _ => None,
    }
}

/// `tt0903747`: an IMDb title's id, and nothing else.
pub fn is_tconst(id: &str) -> bool {
    id.len() > 2 && id.starts_with("tt") && id[2..].bytes().all(|b| b.is_ascii_digit())
}

// ─── what is held already ────────────────────────────────────────────────────

/// The work held here under one of the ids a wanted one is held under, or
/// under an id a merge folded into another: its id.
pub async fn held(state: &AppState, wanted: &Wanted) -> Result<Option<String>> {
    for source in wanted.held_as() {
        if let Some(id) = repo::item::find_id_by_external(&state.db, source, &wanted.id).await? {
            return Ok(Some(id));
        }
        if let Some(id) = repo::alias::for_external(&state.db, source, &wanted.id).await? {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

// ─── the preview ─────────────────────────────────────────────────────────────

/// Where a line stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = ImportLineStatus)]
pub enum Status {
    /// Already in the catalogue: passed over, though put in the selections
    /// chosen.
    Held,
    /// To be imported.
    Candidate,
    /// A provider said it has no such work.
    Missing,
    /// Not an id this server reads.
    Unreadable,
    /// A TMDB id, with nothing to say whether it is a film's or a series'.
    NeedsKind,
    /// A line naming a work an earlier line names: taken once.
    Repeated,
}

/// A line, set beside the catalogue.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ImportPreviewLine)]
pub struct Previewed {
    /// Its place among the lines given, from 0.
    pub line: usize,
    pub text: String,
    pub status: Status,
    /// The work it names, when it names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wanted: Option<Wanted>,
    /// The work held here, for one in the catalogue already.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work: Option<WorkRef>,
    /// What a provider calls a candidate, where one was asked — or what the
    /// line called it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    /// Whether a provider was asked of a candidate: one not asked is looked
    /// up as it is imported.
    pub checked: bool,
}

/// How many lines stand where.
#[derive(Clone, Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ImportPreviewCounts)]
pub struct Counts {
    pub held: usize,
    pub candidate: usize,
    pub missing: usize,
    pub unreadable: usize,
    pub needs_kind: usize,
    pub repeated: usize,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ImportPreview)]
pub struct Preview {
    pub lines: Vec<Previewed>,
    pub counts: Counts,
}

/// Set the lines beside the catalogue: what each names, whether it is held,
/// and — for as long as [`PREVIEW_BUDGET`] allows — whether a provider has
/// it, by the lookups that store nothing.
pub async fn preview(state: &AppState, lines: &[Line], kind: Option<MediaKind>) -> Result<Preview> {
    let mut out: Vec<Previewed> = Vec::with_capacity(lines.len());
    let mut seen: HashSet<Wanted> = HashSet::new();
    // The lines found held, by where they are among `out`, and the work.
    let mut held_at: Vec<(usize, String)> = Vec::new();

    for (at, line) in lines.iter().enumerate() {
        let base = Previewed {
            line: at,
            text: line.text.trim().to_string(),
            status: Status::Unreadable,
            wanted: None,
            work: None,
            title: line.title.clone().filter(|t| !t.trim().is_empty()),
            year: None,
            checked: false,
        };
        let wanted = match read_line(line, kind) {
            Ok(wanted) => wanted,
            Err(Unread::Nothing) => continue,
            Err(Unread::Unreadable) => {
                out.push(base);
                continue;
            }
            Err(Unread::NeedsKind) => {
                out.push(Previewed {
                    status: Status::NeedsKind,
                    ..base
                });
                continue;
            }
        };
        if !seen.insert(wanted.clone()) {
            out.push(Previewed {
                status: Status::Repeated,
                wanted: Some(wanted),
                ..base
            });
            continue;
        }
        let status = match held(state, &wanted).await? {
            Some(id) => {
                held_at.push((out.len(), id));
                Status::Held
            }
            None => Status::Candidate,
        };
        out.push(Previewed {
            status,
            wanted: Some(wanted),
            ..base
        });
    }

    // The works held, by their titles, read at once.
    let ids: Vec<String> = held_at.iter().map(|(_, id)| id.clone()).collect();
    let titles = repo::item::titles(&state.db, &ids).await?;
    for (at, id) in held_at {
        if let Some(line) = out.get_mut(at) {
            line.work = titles.get(&id).cloned();
        }
    }

    // The candidates, asked of a provider while there is time.
    let deadline = tokio::time::Instant::now() + PREVIEW_BUDGET;
    let asked: Vec<(usize, Wanted)> = out
        .iter()
        .enumerate()
        .filter(|(_, line)| line.status == Status::Candidate)
        .filter_map(|(at, line)| Some((at, line.wanted.clone()?)))
        .collect();
    let answers: Vec<(usize, Option<Found>)> = futures::stream::iter(asked)
        .map(|(at, wanted)| async move {
            if tokio::time::Instant::now() >= deadline {
                return (at, None);
            }
            let found = tokio::time::timeout_at(deadline, look(state, &wanted))
                .await
                .ok()
                .flatten();
            (at, found)
        })
        .buffer_unordered(PREVIEW_AT_ONCE)
        .collect()
        .await;
    for (at, found) in answers {
        let Some(line) = out.get_mut(at) else {
            continue;
        };
        match found {
            Some(Found::Has { title, year, kind }) => {
                line.checked = true;
                line.title = Some(title);
                line.year = year;
                if let Some(wanted) = &mut line.wanted
                    && wanted.kind.is_none()
                {
                    wanted.kind = Some(kind);
                }
            }
            Some(Found::Not) => {
                line.checked = true;
                line.status = Status::Missing;
            }
            None => {}
        }
    }

    let mut counts = Counts::default();
    for line in &out {
        match line.status {
            Status::Held => counts.held += 1,
            Status::Candidate => counts.candidate += 1,
            Status::Missing => counts.missing += 1,
            Status::Unreadable => counts.unreadable += 1,
            Status::NeedsKind => counts.needs_kind += 1,
            Status::Repeated => counts.repeated += 1,
        }
    }
    Ok(Preview { lines: out, counts })
}

/// What a provider said of a candidate.
enum Found {
    Has {
        title: String,
        year: Option<i32>,
        kind: MediaKind,
    },
    Not,
}

fn year_of(date: Option<&str>) -> Option<i32> {
    date.and_then(|d| d.get(..4)).and_then(|y| y.parse().ok())
}

/// Ask a provider about a candidate without storing anything: TMDB, by the
/// id or through its lookup of IMDb's and TheTVDB's. `None` when nobody
/// could be asked, or answered: the import will look it up.
async fn look(state: &AppState, wanted: &Wanted) -> Option<Found> {
    if !state.tmdb.is_configured() {
        return None;
    }
    match (wanted.source, wanted.kind) {
        (Source::Tmdb, Some(MediaKind::Movie)) => {
            let id = wanted.id.parse().ok()?;
            match state.tmdb.movie(id).await.ok()? {
                Some((_, movie)) => Some(Found::Has {
                    title: movie.title.clone(),
                    year: year_of(movie.release_date.as_deref()),
                    kind: MediaKind::Movie,
                }),
                None => Some(Found::Not),
            }
        }
        (Source::Tmdb, Some(MediaKind::Series)) => {
            let id = wanted.id.parse().ok()?;
            match state.tmdb.tv(id).await.ok()? {
                Some((_, tv)) => Some(Found::Has {
                    title: tv.name.clone(),
                    year: year_of(tv.first_air_date.as_deref()),
                    kind: MediaKind::Series,
                }),
                None => Some(Found::Not),
            }
        }
        (Source::Imdb, _) => {
            let found = state.tmdb.find("imdb_id", &wanted.id).await.ok()?;
            let movie = found.movie_results.first();
            let tv = found.tv_results.first();
            match (wanted.kind, movie, tv) {
                (Some(MediaKind::Series) | None, _, Some(tv)) => Some(Found::Has {
                    title: tv.name.clone(),
                    year: year_of(tv.first_air_date.as_deref()),
                    kind: MediaKind::Series,
                }),
                (Some(MediaKind::Movie) | None, Some(movie), _) => Some(Found::Has {
                    title: movie.title.clone(),
                    year: year_of(movie.release_date.as_deref()),
                    kind: MediaKind::Movie,
                }),
                // TMDB does not know it; TheTVDB or Radarr's own server may,
                // and are asked on import.
                _ => None,
            }
        }
        (Source::Tvdb, _) => {
            let found = state.tmdb.find("tvdb_id", &wanted.id).await.ok()?;
            found.tv_results.first().map(|tv| Found::Has {
                title: tv.name.clone(),
                year: year_of(tv.first_air_date.as_deref()),
                kind: MediaKind::Series,
            })
        }
        _ => None,
    }
}

// ─── importing ───────────────────────────────────────────────────────────────

/// What is done with the works as they come in.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// A language the episode texts of a series are fetched in at once,
    /// besides the catalogue's own.
    pub language: Option<String>,
    /// The selections the works go into, held already or imported: hand-made
    /// ones, checked by whoever asked.
    pub lists: Vec<String>,
    /// Whether the works imported are switched on. Those held already keep
    /// what they are.
    pub enabled: bool,
}

/// What came of each work, counted.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub here: usize,
    pub imported: usize,
    pub missing: usize,
    pub failed: usize,
}

impl Tally {
    /// In the words the seed's runs use, which the interface reads.
    pub fn said(&self) -> String {
        format!(
            "{} already here, {} imported, {} not found, {} failed",
            self.here, self.imported, self.missing, self.failed
        )
    }

    pub fn done(&self) -> usize {
        self.here + self.imported + self.missing + self.failed
    }
}

/// What came of one work.
#[derive(Debug)]
pub enum Came {
    /// Held here already, under this id.
    Here(String),
    Imported(Box<MediaItem>),
    /// No provider had it.
    Missing,
    Failed(String),
}

/// The notes a run leaves on each work, which the interface puts in words:
/// the seed's own, where they say the same.
pub mod notes {
    pub const IMPORTED: &str = crate::arr::seed::notes::IMPORTED;
    pub const HERE: &str = crate::arr::seed::notes::HERE;
    pub const NOT_FOUND: &str = crate::arr::seed::notes::NOT_FOUND;
}

/// Import one wanted work unless it is held: what came of it, and whether a
/// provider was asked.
pub async fn bring(state: &AppState, wanted: &Wanted) -> (Came, bool) {
    match held(state, wanted).await {
        Ok(Some(id)) => return (Came::Here(id), false),
        Ok(None) => {}
        Err(e) => return (Came::Failed(format!("{e:#}")), false),
    }
    // A task nobody waits on: a slow source is given its time.
    let fetched = crate::providers::unhurried(fetch(state, wanted)).await;
    let came = match fetched {
        Ok(Some(item)) => Came::Imported(Box::new(item)),
        Ok(None) => Came::Missing,
        Err(e) => Came::Failed(crate::providers::clip(&format!("{e:#}"), 300)),
    };
    (came, true)
}

/// Fetch a work by the id it is wanted by, the way a client's request for it
/// would: stored as it comes.
async fn fetch(state: &AppState, wanted: &Wanted) -> Result<Option<MediaItem>> {
    let number = || -> Result<i64> {
        wanted
            .id
            .parse()
            .map_err(|_| anyhow::anyhow!("{} is not a number", wanted.id))
    };
    match (wanted.source, wanted.kind) {
        (Source::Tmdb, Some(MediaKind::Movie)) => {
            service::movie::by_tmdb_id(state, number()?).await
        }
        (Source::Tmdb, Some(MediaKind::Series)) => {
            service::series::by_tmdb_id(state, number()?).await
        }
        (Source::Tmdb, None) => {
            anyhow::bail!("a TMDB id needs to say whether it is a film's or a series'")
        }
        (Source::Tvdb, _) => service::series::by_tvdb_id(state, number()?).await,
        (Source::Imdb, Some(MediaKind::Movie)) => {
            service::movie::by_imdb_id(state, &wanted.id).await
        }
        (Source::Imdb, Some(MediaKind::Series)) => {
            service::series::by_imdb_id(state, &wanted.id).await
        }
        (Source::Imdb, None) => {
            // TMDB says which kind it is, where it is asked; otherwise a
            // series is looked for first, then a film.
            let (movie, series) = if state.tmdb.is_configured() {
                let found = state.tmdb.find("imdb_id", &wanted.id).await?;
                (
                    !found.movie_results.is_empty(),
                    !found.tv_results.is_empty(),
                )
            } else {
                (false, false)
            };
            if movie && !series {
                return service::movie::by_imdb_id(state, &wanted.id).await;
            }
            if let Some(item) = service::series::by_imdb_id(state, &wanted.id).await? {
                return Ok(Some(item));
            }
            service::movie::by_imdb_id(state, &wanted.id).await
        }
        (Source::Anilist | Source::Mal, _) => {
            let term = format!("{}:{}", wanted.source.as_str(), wanted.id);
            Ok(service::series::search(state, &term)
                .await?
                .into_iter()
                .next())
        }
        (Source::Fankai, _) => service::series::by_fankai_id(state, number()?).await,
    }
}

/// What a pass over the wanted works came to: the count, the works in their
/// order — held or imported — and whether it was stopped before the end.
pub struct Brought {
    pub tally: Tally,
    pub works: Vec<String>,
    pub stopped: bool,
}

/// Bring every wanted work in, in order, writing each down for the run and
/// saying how far it got; the works imported switched off when asked, their
/// episodes fetched in the language asked. Stopped between two works when
/// the flag says so.
pub async fn bring_all(
    state: &AppState,
    record: &str,
    wanted: &[Wanted],
    options: &Options,
    flag: &cancel::Registered,
) -> Brought {
    let total = wanted.len();
    let mut tally = Tally::default();
    let mut works = Vec::new();
    for (position, one) in wanted.iter().enumerate() {
        if flag.stopped() {
            return Brought {
                tally,
                works,
                stopped: true,
            };
        }
        let (came, fetched) = bring(state, one).await;
        match &came {
            Came::Here(id) => {
                tally.here += 1;
                works.push(id.clone());
            }
            Came::Imported(item) => {
                tally.imported += 1;
                works.push(item.id.clone());
                settle(state, item, options).await;
            }
            Came::Missing => tally.missing += 1,
            Came::Failed(_) => tally.failed += 1,
        }
        note(state, record, position, one, &came).await;
        if let Err(e) = job::progress(
            &state.db,
            record,
            &format!("{} of {total}: {}", tally.done(), tally.said()),
        )
        .await
        {
            tracing::debug!(error = %e, "could not say how far the import got");
        }
        if fetched {
            tokio::time::sleep(PAUSE).await;
        }
    }
    Brought {
        tally,
        works,
        stopped: false,
    }
}

/// What is done with a work just imported: switched off when asked, its
/// episodes fetched in the language asked.
async fn settle(state: &AppState, item: &MediaItem, options: &Options) {
    if !options.enabled {
        match repo::item::set_enabled(&state.db, &item.id, false).await {
            Ok(_) => state.caches.touched(&item.id).await,
            Err(e) => {
                tracing::warn!(id = %item.id, error = %e, "could not switch an imported work off")
            }
        }
    }
    if let Some(language) = options.language.as_deref()
        && item.kind == MediaKind::Series
        && options.enabled
    {
        let mut read = item.clone();
        if let Err(e) = service::language::apply(state, &mut read, language).await {
            tracing::info!(id = %item.id, language, error = format_args!("{e:#}"), "could not fetch an imported series' episodes in another language");
        }
    }
}

/// One work, written down for the run: as it is held here when it is, by
/// the id it was wanted by when it is not.
async fn note(state: &AppState, record: &str, position: usize, wanted: &Wanted, came: &Came) {
    let label = wanted.label();
    let (id, title, kind, outcome, note) = match came {
        Came::Here(id) => (
            id.as_str(),
            None,
            wanted.kind,
            job::Outcome::Skipped,
            notes::HERE,
        ),
        Came::Imported(item) => (
            item.id.as_str(),
            Some(item.title.as_str()),
            Some(item.kind),
            job::Outcome::Imported,
            notes::IMPORTED,
        ),
        Came::Missing => (
            "",
            None,
            wanted.kind,
            job::Outcome::Failed,
            notes::NOT_FOUND,
        ),
        Came::Failed(why) => ("", None, wanted.kind, job::Outcome::Failed, why.as_str()),
    };
    // A work held here is named by its title as it is held.
    let held_title = match came {
        Came::Here(id) => repo::item::titles(&state.db, std::slice::from_ref(id))
            .await
            .ok()
            .and_then(|mut found| found.remove(id))
            .map(|work| work.title),
        _ => None,
    };
    let touched = job::Touched {
        id,
        title: title.or(held_title.as_deref()).or(Some(label.as_str())),
        kind,
    };
    let position = i64::try_from(position).unwrap_or(i64::MAX);
    if let Err(e) = job::add_entry(&state.db, record, position, touched, outcome, Some(note)).await
    {
        tracing::debug!(error = %e, "could not write down what the import did to a work");
    }
}

/// How the works go into a selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = ListPlacement)]
pub enum Placement {
    /// They join it, after its members.
    Add,
    /// It becomes them, in their order.
    Replace,
}

impl Placement {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Replace => "replace",
        }
    }

    pub fn parse(text: &str) -> Self {
        match text {
            "replace" => Self::Replace,
            _ => Self::Add,
        }
    }
}

/// Put works into a hand-made selection: joined to its members, or in
/// their place; never past [`LIST_MOST`]. How many could not be put in for
/// want of room.
pub async fn place(
    state: &AppState,
    list_id: &str,
    works: &[String],
    placement: Placement,
) -> Result<usize> {
    let Some(list) = repo::list::get(&state.db, list_id).await? else {
        anyhow::bail!("the selection is gone");
    };
    if list.mode != repo::list::ListMode::Manual {
        anyhow::bail!(
            "{} is composed by a filter, and holds no works of its own",
            list.name
        );
    }
    let mut members: Vec<String> = match placement {
        Placement::Add => repo::list::member_ids(&state.db, list_id).await?,
        Placement::Replace => Vec::new(),
    };
    let mut seen: HashSet<String> = members.iter().cloned().collect();
    let mut left_out = 0;
    for work in works {
        if !seen.insert(work.clone()) {
            continue;
        }
        if members.len() >= LIST_MOST {
            left_out += 1;
            continue;
        }
        members.push(work.clone());
    }
    repo::list::set_members(&state.db, list_id, &members).await?;
    Ok(left_out)
}

// ─── the batch ───────────────────────────────────────────────────────────────

/// Start a batch, as `by` asked: the run's id at once. Each work wanted
/// once, in the order given.
pub async fn start(
    state: &AppState,
    wanted: Vec<Wanted>,
    options: Options,
    by: &str,
) -> AppResult<String> {
    if state.maintenance.is_on() {
        return Err(AppError::Conflict("a backup is being restored".into()));
    }
    let mut seen = HashSet::new();
    let wanted: Vec<Wanted> = wanted
        .into_iter()
        .filter(|one| seen.insert(one.clone()))
        .collect();
    if wanted.is_empty() {
        return Err(AppError::BadRequest(
            "no line names a work this server can import".into(),
        ));
    }
    let held = IMPORTING.clone().try_lock_owned().map_err(|_| {
        AppError::Conflict("a batch is being imported already; its run is in the history".into())
    })?;
    let shared = state
        .coord
        .hold_for(HOLD, "a batch is being imported")
        .await?;
    let record = job::start_by(&state.db, KIND, None, Some(by)).await?;

    let state = state.clone();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        let _shared = shared;
        let flag = cancel::register(&record);
        let brought = bring_all(&state, &record, &wanted, &options, &flag).await;
        let mut trouble = Vec::new();
        for list in &options.lists {
            match place(&state, list, &brought.works, Placement::Add).await {
                Ok(0) => {}
                Ok(left) => trouble.push(format!(
                    "{left} works were not put in a selection that is full"
                )),
                Err(e) => trouble.push(format!("{e:#}")),
            }
        }
        let summary = format!("{}, of {}", brought.tally.said(), wanted.len());
        let closed = if brought.stopped {
            job::stop(&state.db, &record, &format!("stopped: {summary}")).await
        } else {
            let trouble = (!trouble.is_empty()).then(|| trouble.join("; "));
            job::finish(&state.db, &record, Some(&summary), trouble.as_deref()).await
        };
        if let Err(e) = closed {
            tracing::warn!(error = %e, "could not close the batch's run");
        }
    });
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Line {
        Line {
            text: text.into(),
            ..Line::default()
        }
    }

    #[test]
    fn a_line_names_the_work_it_says() {
        let read = |text: &str, kind: Option<MediaKind>| read_line(&line(text), kind);
        assert_eq!(
            read("tmdb:603", Some(MediaKind::Movie)),
            Ok(Wanted {
                source: Source::Tmdb,
                id: "603".into(),
                kind: Some(MediaKind::Movie),
            })
        );
        // A TMDB id alone says nothing of its kind.
        assert_eq!(read("tmdb:603", None), Err(Unread::NeedsKind));
        // A page does.
        assert_eq!(
            read("https://www.themoviedb.org/tv/1396-breaking-bad", None)
                .unwrap()
                .kind,
            Some(MediaKind::Series)
        );
        assert_eq!(read("tt0133093", None).unwrap().source, Source::Imdb);
        // The kind given for all is a TMDB id's alone.
        assert_eq!(
            read("tt0903747", Some(MediaKind::Movie)).unwrap().kind,
            None
        );
        assert_eq!(read("imdb:tt0133093", None).unwrap().id, "tt0133093");
        assert_eq!(
            read("tvdb:81189", None).unwrap().kind,
            Some(MediaKind::Series)
        );
        assert_eq!(read("anilist:21", None).unwrap().source, Source::Anilist);
        assert_eq!(read("mal:21", None).unwrap().source, Source::Mal);
        assert_eq!(read("fankai:12", None).unwrap().source, Source::Fankai);
        // As a text file of ids writes them: a title after a `#`.
        assert_eq!(
            read("tmdb:949  # Heat (1995)", Some(MediaKind::Movie))
                .unwrap()
                .id,
            "949"
        );
        assert_eq!(read("# a comment", None), Err(Unread::Nothing));
        assert_eq!(read("   ", None), Err(Unread::Nothing));
        assert_eq!(read("The Matrix", None), Err(Unread::Unreadable));
        assert_eq!(read("603", Some(MediaKind::Movie)), Err(Unread::Unreadable));
        // The line's own kind before the one given for all.
        let series = Line {
            text: "tmdb:1396".into(),
            kind: Some(MediaKind::Series),
            title: None,
        };
        assert_eq!(
            read_line(&series, Some(MediaKind::Movie)).unwrap().kind,
            Some(MediaKind::Series)
        );
    }

    #[test]
    fn a_comment_is_what_follows_a_spaced_hash() {
        assert_eq!(strip_comment("tmdb:1  # x"), "tmdb:1");
        assert_eq!(strip_comment("tmdb:1\t#x"), "tmdb:1");
        assert_eq!(
            strip_comment("https://example.com/a#b"),
            "https://example.com/a#b"
        );
        assert_eq!(strip_comment("#tmdb:1"), "");
    }

    #[test]
    fn a_pasted_page_is_the_lookup_it_stands_for() {
        let cases = [
            (
                "https://www.themoviedb.org/tv/1396-breaking-bad?language=fr",
                "tmdb:1396",
                Some(MediaKind::Series),
            ),
            (
                "https://www.themoviedb.org/movie/603-the-matrix",
                "tmdb:603",
                Some(MediaKind::Movie),
            ),
            (
                "https://m.imdb.com/title/tt0903747/",
                "imdb:tt0903747",
                None,
            ),
            (
                "https://www.imdb.com/fr/title/tt0903747/?ref_=nv_sr",
                "imdb:tt0903747",
                None,
            ),
            (
                "https://anilist.co/anime/21/ONE-PIECE/",
                "anilist:21",
                Some(MediaKind::Series),
            ),
            (
                "https://myanimelist.net/anime/21/One_Piece",
                "mal:21",
                Some(MediaKind::Series),
            ),
            (
                "https://thetvdb.com/dereferrer/series/81189",
                "tvdb:81189",
                Some(MediaKind::Series),
            ),
            (
                "https://thetvdb.com/?tab=series&id=81189",
                "tvdb:81189",
                Some(MediaKind::Series),
            ),
        ];
        for (link, lookup, kind) in cases {
            assert_eq!(from_link(link), Some((lookup.to_string(), kind)), "{link}");
        }
    }

    #[test]
    fn anything_else_is_searched_as_typed() {
        for term in [
            "Breaking Bad",
            "tvdb:81189",
            "https://thetvdb.com/series/breaking-bad",
            // Fankai's website numbers its productions otherwise than the
            // metadata service its ids here come from.
            "https://fankai.fr/productions/101",
            "https://www.imdb.com/title/nm0000123/",
            "https://example.com/tv/1396",
            "ftp://themoviedb.org/tv/1396",
        ] {
            assert_eq!(from_link(term), None, "{term}");
        }
    }

    #[test]
    fn a_tally_is_said_in_the_seeds_words() {
        let tally = Tally {
            here: 1,
            imported: 2,
            missing: 3,
            failed: 4,
        };
        assert_eq!(
            tally.said(),
            "1 already here, 2 imported, 3 not found, 4 failed"
        );
        assert_eq!(tally.done(), 10);
    }
}
