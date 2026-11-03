//! A series as AniList and MyAnimeList split it: one entry for each season
//! or cour, where the catalogue keeps one work for the whole series.
//!
//! The anime identifier list ([`crate::db::repo::anime`]) files each entry
//! under a TheTVDB series, a season and an offset — *Attack on Titan*'s third
//! season is two entries, the second starting twelve episodes in — and the
//! work numbers its own episodes as TheTVDB airs them. Each entry is placed
//! on those episodes, from the one after its offset up to where the next
//! entry of its season starts ([`place`]), and described from them, their
//! corrections included: how many, how many have aired, the first and the
//! last day, how far it has got, the quarter it premiered in, the next to
//! air ([`Cour`]). Worked out on every read and never stored: the list, the
//! episodes and their corrections each move on their own.
//!
//! What only MyAnimeList says of an entry — its titles and synopsis, how many
//! episodes it is to have, its type, what it was adapted from, its studios,
//! the entries related to it — is read from the answer kept with the work for
//! its own entry, and asked of MyAnimeList for the others by a caller willing
//! to wait ([`described`]).

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Datelike, NaiveDate, TimeDelta, Utc};
use futures::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    db::repo::{self, anime::Entry},
    domain::{Episode, MediaItem, MediaKind, fields::Scope},
    providers::mal,
    state::AppState,
};

/// Where an entry stands among the work's episodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    /// The whole work: a film, or a series the list keeps as one entry —
    /// *One Piece* is one, not twenty.
    Work,
    /// Episodes of one season, from `first` to `last` — none while the entry
    /// runs to the season's end.
    Season {
        number: i32,
        first: i32,
        last: Option<i32>,
    },
}

/// How far an entry has aired, in AniList's terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = CourStatus)]
pub enum Status {
    NotYetReleased,
    Releasing,
    Finished,
}

impl Status {
    /// AniList's name for it: its `MediaStatus`.
    pub fn anilist(self) -> &'static str {
        match self {
            Self::NotYetReleased => "NOT_YET_RELEASED",
            Self::Releasing => "RELEASING",
            Self::Finished => "FINISHED",
        }
    }
}

/// A quarter of the calendar, the way the season chart counts them: winter
/// from January.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quarter {
    Winter,
    Spring,
    Summer,
    Autumn,
}

impl Quarter {
    /// The quarter a day falls in, and its year.
    pub fn of(day: &str) -> Option<(Self, i32)> {
        let day = NaiveDate::parse_from_str(day.get(..10)?, "%Y-%m-%d").ok()?;
        let quarter = match day.month() {
            1..=3 => Self::Winter,
            4..=6 => Self::Spring,
            7..=9 => Self::Summer,
            _ => Self::Autumn,
        };
        Some((quarter, day.year()))
    }
}

/// A season of the year as AniList counts them, its `MediaSeason`: winter
/// from December to February, then spring, summer and fall, three months
/// each. Not the season chart's [`Quarter`], whose winter is January's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnilistSeason {
    Winter,
    Spring,
    Summer,
    Fall,
}

impl AnilistSeason {
    /// AniList's season a day falls in — `YYYY-MM-DD`, or a month alone,
    /// `YYYY-MM` — and the year AniList files it under. December opens the
    /// winter of the year that follows: an entry that started on 28 December
    /// 2024 is AniList's winter of 2025, as AniList answered on 2 November
    /// 2026.
    pub fn of(day: &str) -> Option<(Self, i32)> {
        let day = day.trim();
        let (year, month) =
            match NaiveDate::parse_from_str(day.get(..10).unwrap_or(day), "%Y-%m-%d") {
                Ok(date) => (date.year(), date.month()),
                Err(_) => {
                    let (year, month) = day.get(..7)?.split_once('-')?;
                    let month: u32 = month.parse().ok()?;
                    (year.parse().ok()?, month)
                }
            };
        let season = match month {
            12 | 1 | 2 => Self::Winter,
            3..=5 => Self::Spring,
            6..=8 => Self::Summer,
            9..=11 => Self::Fall,
            _ => return None,
        };
        Some((season, if month == 12 { year + 1 } else { year }))
    }

    /// Its name in AniList's schema.
    pub fn anilist(self) -> &'static str {
        match self {
            Self::Winter => "WINTER",
            Self::Spring => "SPRING",
            Self::Summer => "SUMMER",
            Self::Fall => "FALL",
        }
    }

    /// AniList's `seasonInt` for it in `year`: the year's last two digits,
    /// then the season's place — 251 for the winter of 2025, 994 for the
    /// fall of 1999.
    pub fn int(self, year: i32) -> i32 {
        let place = match self {
            Self::Winter => 1,
            Self::Spring => 2,
            Self::Summer => 3,
            Self::Fall => 4,
        };
        year.rem_euclid(100) * 10 + place
    }
}

/// One episode of an entry and when it airs: its id in AniList's dialect,
/// its number within the entry, as AniList counts — the entry's first is 1 —
/// and the instant, in seconds since the epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Airing {
    /// Made of the catalogue's own id for the episode
    /// ([`crate::service::ids::anilist_airing`]): the same however the list
    /// splits the work, from one answer to the next.
    pub id: i64,
    pub episode: i32,
    pub airing_at: i64,
}

/// The next episode of an entry to air.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Next {
    /// Its number within the entry, the way AniList counts: the entry's
    /// first is 1.
    pub episode: i32,
    /// Its number in its season, as the work numbers it.
    pub episode_number: i32,
    /// When it airs, in seconds since the epoch.
    pub airing_at: i64,
    /// Its day, `YYYY-MM-DD`.
    pub day: String,
}

/// One entry of the list, placed on the work and described from its
/// episodes as they are served, corrections included.
#[derive(Clone, Debug, PartialEq)]
pub struct Cour {
    pub anilist: Option<i64>,
    pub mal: Option<i64>,
    /// `TV`, `ONA`, `OVA`, `SPECIAL`, `MOVIE`…, as the list files it.
    pub format: Option<String>,
    /// Where it stands; none where the list does not say.
    pub span: Option<Span>,
    /// Whether it is the whole work: a film, a series kept as one entry, or
    /// the one entry of a series of one season.
    pub whole: bool,
    /// Whether it holds the work's first episode, and its last.
    pub opens_work: bool,
    pub ends_work: bool,
    /// Whether more of the work comes after it: a later entry of its
    /// season, a later season, or the work having ended.
    pub closed: bool,
    /// The work's episodes it covers, and how many of them have aired.
    pub episodes: i64,
    pub aired: i64,
    /// The first and last of them, by their number in the season.
    pub numbers: Option<(i32, i32)>,
    /// Its first episode's day, and its last's once it has finished:
    /// `YYYY-MM-DD`.
    pub starts: Option<String>,
    pub ends: Option<String>,
    pub status: Option<Status>,
    /// The quarter it premiered in, and the year.
    pub quarter: Option<(Quarter, i32)>,
    pub next: Option<Next>,
    /// Whether its schedule was corrected here: an episode's air date or
    /// time locked, or an episode made by hand, among its own.
    pub corrected: bool,
    /// Whether an episode was made by hand among its own.
    pub made_here: bool,
}

/// How long after its last episode an entry with nothing more listed and
/// nothing known to follow it is taken to have finished: a weekly series
/// lists its next episode well before then.
const QUIET_FOR: TimeDelta = TimeDelta::days(14);

/// How many entries one call of [`described`] asks MyAnimeList about, at
/// most: each takes MyAnimeList's spacing.
pub const ASKED_MOST: usize = 24;

/// The kinds of entry, in the order one of them holds a place another
/// starts at too: a series made for television, then one for the web, an
/// OVA, a special, a film.
fn rank(format: Option<&str>) -> u8 {
    match format {
        Some("TV" | "TV_SHORT") => 0,
        Some("ONA") => 1,
        Some("OVA") => 2,
        Some("SPECIAL") => 3,
        Some("MOVIE") => 4,
        _ => 5,
    }
}

/// Where each entry the list files under a series stands among its
/// episodes: those it places first, by season and place, then those it does
/// not, with none.
///
/// - No season: the whole series.
/// - A regular season: from the episode after its offset — none is nought —
///   up to the one where the season's next entry starts. Two entries
///   starting at one place are told apart by their kind ([`rank`]): the
///   series holds it, and the film or the special filed beside it is not
///   placed. Two of one kind cannot be told apart, and neither is placed.
/// - Season zero, where TheTVDB files the films and specials among whatever
///   else aired outside the seasons: a film is the one episode after its
///   offset; anything else only as far as MyAnimeList's count of its
///   episodes goes (`planned`, by MyAnimeList's id), and never past where
///   the next entry starts. Without an offset, nothing says where it is.
pub fn place(entries: &[Entry], planned: &HashMap<i64, i64>) -> Vec<(Entry, Option<Span>)> {
    // The list repeats an entry for each TMDB film it is filed under.
    let mut seen = HashSet::new();
    let entries: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.mal_id.is_some() || e.anilist_id.is_some())
        .filter(|e| seen.insert((e.anilist_id, e.mal_id, e.tvdb_season, e.tvdb_offset)))
        .collect();

    let mut placed: Vec<(Entry, Option<Span>)> = Vec::new();
    let mut unplaced: Vec<Entry> = Vec::new();

    for entry in &entries {
        if entry.tvdb_season.is_none() {
            placed.push(((*entry).clone(), Some(Span::Work)));
        }
    }

    let seasons: BTreeSet<i32> = entries.iter().filter_map(|e| e.tvdb_season).collect();
    for season in seasons {
        let mut here: Vec<&Entry> = entries
            .iter()
            .copied()
            .filter(|e| e.tvdb_season == Some(season))
            .collect();
        here.sort_by_key(|e| (e.tvdb_offset.unwrap_or(0), rank(e.kind.as_deref())));
        let mut offsets: Vec<i32> = here.iter().map(|e| e.tvdb_offset.unwrap_or(0)).collect();
        offsets.dedup();
        let after = |offset: i32| offsets.iter().copied().find(|o| *o > offset);

        if season < 0 {
            unplaced.extend(here.into_iter().cloned());
            continue;
        }

        if season == 0 {
            for entry in here {
                let count = if entry.kind.as_deref() == Some("MOVIE") {
                    Some(1)
                } else {
                    entry
                        .mal_id
                        .and_then(|id| planned.get(&id))
                        .and_then(|n| i32::try_from(*n).ok())
                        .filter(|n| *n > 0)
                };
                match (entry.tvdb_offset, count) {
                    (Some(offset), Some(count)) => {
                        let last = offset.saturating_add(count);
                        let last = after(offset).map_or(last, |next| last.min(next));
                        placed.push((
                            entry.clone(),
                            Some(Span::Season {
                                number: 0,
                                first: offset + 1,
                                last: Some(last),
                            }),
                        ));
                    }
                    _ => unplaced.push(entry.clone()),
                }
            }
            continue;
        }

        for offset in &offsets {
            let at: Vec<&Entry> = here
                .iter()
                .copied()
                .filter(|e| e.tvdb_offset.unwrap_or(0) == *offset)
                .collect();
            let best = at.iter().map(|e| rank(e.kind.as_deref())).min();
            let holders = at
                .iter()
                .filter(|e| Some(rank(e.kind.as_deref())) == best)
                .count();
            for entry in at {
                if holders == 1 && Some(rank(entry.kind.as_deref())) == best {
                    placed.push((
                        entry.clone(),
                        Some(Span::Season {
                            number: season,
                            first: offset + 1,
                            last: after(*offset),
                        }),
                    ));
                } else {
                    unplaced.push(entry.clone());
                }
            }
        }
    }

    placed.extend(unplaced.into_iter().map(|entry| (entry, None)));
    placed
}

/// The entries the list files under a series, placed on the work's
/// episodes and described from them as of `now`. `planned` is MyAnimeList's
/// count of an entry's episodes, by its id, where the caller has it.
pub fn of_series(
    item: &MediaItem,
    entries: &[Entry],
    planned: &HashMap<i64, i64>,
    now: DateTime<Utc>,
) -> Vec<Cour> {
    let mut regular: Vec<&Episode> = item
        .episodes
        .iter()
        .filter(|e| e.season_number > 0)
        .collect();
    regular.sort_by_key(|e| (e.season_number, e.episode_number));
    let held: BTreeSet<i32> = regular.iter().map(|e| e.season_number).collect();
    let ended = item.status.as_deref() == Some("ended");

    let placed = place(entries, planned);
    // The regular seasons the list places an entry in: one announced there
    // follows the seasons before it, episodes or not.
    let listed: BTreeSet<i32> = placed
        .iter()
        .filter_map(|(_, span)| match span {
            Some(Span::Season { number, .. }) if *number > 0 => Some(*number),
            _ => None,
        })
        .collect();
    let seasons: BTreeSet<i32> = held.union(&listed).copied().collect();
    let (first_season, last_season) = (seasons.first().copied(), seasons.last().copied());

    placed
        .into_iter()
        .map(|(entry, span)| {
            let episodes = covered(item, span);
            let standing = match span {
                Some(Span::Work) => Place {
                    whole: true,
                    opens_work: true,
                    ends_work: true,
                    closed: ended,
                },
                Some(Span::Season {
                    number,
                    first,
                    last,
                }) if number > 0 => Place {
                    whole: first == 1
                        && last.is_none()
                        && seasons.iter().all(|season| *season == number),
                    opens_work: first == 1 && Some(number) == first_season,
                    ends_work: last.is_none() && Some(number) == last_season,
                    closed: last.is_some() || ended || Some(number) != last_season,
                },
                // A special's end is its own, and an unplaced entry has none.
                Some(Span::Season { .. }) => Place {
                    closed: true,
                    ..Place::default()
                },
                None => Place::default(),
            };
            describe(item, entry, span, &episodes, standing, now)
        })
        .collect()
}

/// The work's episodes an entry standing at `span` covers, in their order:
/// a season's from `first` to `last`; the whole work's, outside season zero,
/// season by season; none for an entry the list does not place.
fn covered(item: &MediaItem, span: Option<Span>) -> Vec<&Episode> {
    let mut episodes: Vec<&Episode> = match span {
        Some(Span::Work) => item
            .episodes
            .iter()
            .filter(|e| e.season_number > 0)
            .collect(),
        Some(Span::Season {
            number,
            first,
            last,
        }) => item
            .episodes
            .iter()
            .filter(|e| {
                e.season_number == number
                    && e.episode_number >= first
                    && last.is_none_or(|last| e.episode_number <= last)
            })
            .collect(),
        None => Vec::new(),
    };
    episodes.sort_by_key(|e| (e.season_number, e.episode_number));
    episodes
}

/// An episode's own number within the entry standing at `span`, which
/// covers `episodes`: from its season's offset, or along the whole series —
/// the number TheTVDB counts it by there, else its place.
fn number_within(span: Option<Span>, episodes: &[&Episode], episode: &Episode) -> i32 {
    match span {
        Some(Span::Season { first, .. }) => episode.episode_number - first + 1,
        _ => episode.absolute_episode_number.unwrap_or_else(|| {
            episodes
                .iter()
                .position(|e| std::ptr::eq(*e, episode))
                .map_or(0, |at| i32::try_from(at + 1).unwrap_or(i32::MAX))
        }),
    }
}

/// When each of an entry's episodes airs, in the order they do, each by its
/// number within the entry: its schedule, as AniList lists one. The episodes
/// as they are served, corrections included; one with neither a day nor an
/// instant is not on it.
pub fn schedule(item: &MediaItem, cour: &Cour) -> Vec<Airing> {
    let episodes = covered(item, cour.span);
    let mut timed: Vec<(DateTime<Utc>, &Episode)> = episodes
        .iter()
        .filter_map(|episode| Some((airs(episode)?, *episode)))
        .collect();
    timed.sort_by_key(|(at, e)| (*at, e.season_number, e.episode_number));
    timed
        .into_iter()
        .map(|(at, episode)| Airing {
            id: crate::service::ids::anilist_airing(&episode_key(item, episode)),
            episode: number_within(cour.span, &episodes, episode),
            airing_at: at.timestamp(),
        })
        .collect()
}

/// What names an episode for good: the catalogue's id for it, else — one
/// not stored — its work's and its own numbers.
fn episode_key(item: &MediaItem, episode: &Episode) -> String {
    if episode.id.is_empty() {
        format!(
            "{}:{}x{}",
            item.id, episode.season_number, episode.episode_number
        )
    } else {
        episode.id.clone()
    }
}

/// Where an entry stands in the work as a whole: see [`Cour`].
#[derive(Clone, Copy, Debug, Default)]
struct Place {
    whole: bool,
    opens_work: bool,
    ends_work: bool,
    closed: bool,
}

/// An entry, described from its episodes as of `now`.
fn describe(
    item: &MediaItem,
    entry: Entry,
    span: Option<Span>,
    episodes: &[&Episode],
    place: Place,
    now: DateTime<Utc>,
) -> Cour {
    let timed: Vec<(&Episode, DateTime<Utc>)> = episodes
        .iter()
        .filter_map(|e| Some((*e, airs(e)?)))
        .collect();
    let order = |(e, at): &&(&Episode, DateTime<Utc>)| (*at, e.season_number, e.episode_number);
    let aired = timed.iter().filter(|(_, at)| *at <= now).count();
    let first = timed.iter().min_by_key(order);
    let last = timed.iter().max_by_key(order);
    let next = timed.iter().filter(|(_, at)| *at > now).min_by_key(order);

    let status = if episodes.is_empty() {
        None
    } else if aired == 0 {
        Some(Status::NotYetReleased)
    } else if next.is_some() {
        Some(Status::Releasing)
    } else if place.closed {
        Some(Status::Finished)
    } else if timed.len() < episodes.len() || span == Some(Span::Work) {
        // Listed, not dated yet: still to come. And a series kept as one
        // entry that goes on is airing between its episodes, however long
        // its break.
        Some(Status::Releasing)
    } else if last.is_some_and(|(_, at)| *at + QUIET_FOR < now) {
        Some(Status::Finished)
    } else {
        Some(Status::Releasing)
    };
    let starts = first.map(|(e, at)| day_of(e, *at));

    let within = |episode: &Episode| number_within(span, episodes, episode);
    let locked = |episode: &Episode, field: &str| {
        let scope = Scope::Episode {
            season: episode.season_number,
            episode: episode.episode_number,
        };
        item.locked_fields.contains(&format!("{scope}/{field}"))
    };

    Cour {
        anilist: entry.anilist_id,
        mal: entry.mal_id,
        format: entry.kind,
        span,
        whole: place.whole,
        opens_work: place.opens_work,
        ends_work: place.ends_work,
        closed: place.closed,
        episodes: episodes.len() as i64,
        aired: aired as i64,
        numbers: match (span, episodes.first(), episodes.last()) {
            (Some(Span::Season { .. }), Some(first), Some(last)) => {
                Some((first.episode_number, last.episode_number))
            }
            _ => None,
        },
        quarter: starts.as_deref().and_then(Quarter::of),
        starts,
        ends: (status == Some(Status::Finished))
            .then(|| last.map(|(e, at)| day_of(e, *at)))
            .flatten(),
        status,
        next: next.map(|(e, at)| Next {
            episode: within(e),
            episode_number: e.episode_number,
            airing_at: at.timestamp(),
            day: day_of(e, *at),
        }),
        corrected: episodes
            .iter()
            .any(|e| e.is_manual || locked(e, "airDate") || locked(e, "airDateUtc")),
        made_here: episodes.iter().any(|e| e.is_manual),
    }
}

/// A film's own entry: the one the list files under its TMDB id as a film,
/// or — the list not knowing it — the one entry it goes by. The specials
/// filed under the same id are not the film, and have none.
pub fn of_film(item: &MediaItem, entries: &[Entry]) -> Vec<Cour> {
    let own = match crate::service::anime::primary_of_movie(entries) {
        Some(entry) => Some((entry.anilist_id, entry.mal_id, entry.kind.clone())),
        None if entries.is_empty() => match (
            item.external_ids.anilist.as_slice(),
            item.external_ids.mal.as_slice(),
        ) {
            ([anilist], mal) if mal.len() <= 1 => {
                Some((Some(*anilist), mal.first().copied(), None))
            }
            ([], [mal]) => Some((None, Some(*mal), None)),
            _ => None,
        },
        None => None,
    };
    own.map(|(anilist, mal, format)| film(anilist, mal, format))
        .into_iter()
        .collect()
}

/// A film's own entry: the whole film, which no episode describes.
fn film(anilist: Option<i64>, mal: Option<i64>, format: Option<String>) -> Cour {
    Cour {
        anilist,
        mal,
        format,
        span: Some(Span::Work),
        whole: true,
        opens_work: true,
        ends_work: true,
        closed: true,
        episodes: 0,
        aired: 0,
        numbers: None,
        starts: None,
        ends: None,
        status: None,
        quarter: None,
        next: None,
        corrected: false,
        made_here: false,
    }
}

/// A work the list does not split, as the one entry it is, under `anilist`
/// and `mal`: a series described from all its episodes outside season zero,
/// as an entry the list keeps whole is; a film, its own.
pub fn of_unlisted(
    item: &MediaItem,
    anilist: Option<i64>,
    mal: Option<i64>,
    now: DateTime<Utc>,
) -> Cour {
    match item.kind {
        MediaKind::Series => {
            let entry = Entry {
                anilist_id: anilist,
                mal_id: mal,
                kind: item.format.clone(),
                ..Entry::default()
            };
            let episodes = covered(item, Some(Span::Work));
            let place = Place {
                whole: true,
                opens_work: true,
                ends_work: true,
                closed: item.status.as_deref() == Some("ended"),
            };
            describe(item, entry, Some(Span::Work), &episodes, place, now)
        }
        MediaKind::Movie => film(anilist, mal, item.format.clone()),
    }
}

/// The entries a work is, as of `now`: a series' as the list splits it, a
/// film's own. None where the list does not know the work, or cannot be
/// read.
pub async fn of_work(state: &AppState, item: &MediaItem, now: DateTime<Utc>) -> Vec<Cour> {
    match try_of_work(state, item, now).await {
        Ok(cours) => cours,
        Err(e) => {
            tracing::warn!(
                media_id = %item.id,
                error = format_args!("{e:#}"),
                "could not read the anime identifier list"
            );
            Vec::new()
        }
    }
}

/// [`of_work`], failing where the list cannot be read: for a reader that
/// would otherwise take a work the list splits for one it does not.
pub async fn try_of_work(
    state: &AppState,
    item: &MediaItem,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<Cour>> {
    let entries = match item.kind {
        MediaKind::Series => match item.external_ids.tvdb {
            Some(tvdb) => repo::anime::for_series(&state.db, tvdb).await?,
            None => return Ok(Vec::new()),
        },
        MediaKind::Movie => match item.external_ids.tmdb {
            Some(tmdb) => repo::anime::for_movie(&state.db, tmdb).await?,
            None => Vec::new(),
        },
    };
    Ok(match item.kind {
        MediaKind::Series => of_series(item, &entries, &HashMap::new(), now),
        MediaKind::Movie => of_film(item, &entries),
    })
}

/// What the catalogue already says an entry is called, without asking
/// anybody: the title the work's sources gave it where they named it beside
/// the work ([`repo::anime::named_beside`]) — a first season names its
/// sequel.
pub fn kept_title(named: &[repo::anime::Named], cour: &Cour) -> Option<String> {
    named
        .iter()
        .find(|entry| {
            (entry.anilist_id.is_some() && entry.anilist_id == cour.anilist)
                || (entry.mal_id.is_some() && entry.mal_id == cour.mal)
        })
        .map(|entry| entry.title.clone())
        .filter(|title| !title.trim().is_empty())
}

/// MyAnimeList's description of each entry the cours name, by its id
/// there: the work's own entry read from what MyAnimeList answered at its
/// last refresh, the others from those asked lately; and — when `ask`, and
/// MyAnimeList is switched on — the rest asked of it, [`ASKED_MOST`] at the
/// most, each kept a while. When MyAnimeList fails to answer, the rest go
/// without.
pub async fn described(
    state: &AppState,
    item: &MediaItem,
    cours: &[Cour],
    ask: bool,
) -> HashMap<i64, mal::Described> {
    let kept = match repo::snapshot::texts(&state.db, &item.id, &["mal"]).await {
        Ok(kept) => kept,
        Err(e) => {
            tracing::warn!(
                media_id = %item.id,
                error = format_args!("{e:#}"),
                "could not read what MyAnimeList answered for a work"
            );
            Vec::new()
        }
    };
    let kept = kept_described(kept.iter().map(|(_, text)| text.as_str()));
    described_from(state, cours, &kept, ask).await
}

/// The entries MyAnimeList's answers for a work describe, read off the
/// texts [`repo::snapshot::texts`] keeps them as: for a reader that reads
/// them once for each of the work's entries ([`described_among`]).
pub fn kept_described<'t>(texts: impl IntoIterator<Item = &'t str>) -> Vec<mal::Described> {
    texts
        .into_iter()
        .filter_map(|text| mal::described_from_kept(&serde_json::from_str(text).ok()?))
        .collect()
}

/// [`described`], from what [`kept_described`] read of the work's
/// answers: MyAnimeList is not asked.
pub async fn described_among(
    state: &AppState,
    cours: &[Cour],
    kept: &[mal::Described],
) -> HashMap<i64, mal::Described> {
    described_from(state, cours, kept, false).await
}

/// [`described`], from the entries MyAnimeList's answers kept for the work
/// describe, `kept`.
async fn described_from(
    state: &AppState,
    cours: &[Cour],
    kept: &[mal::Described],
    ask: bool,
) -> HashMap<i64, mal::Described> {
    let mut wanted: Vec<i64> = Vec::new();
    for id in cours.iter().filter_map(|c| c.mal) {
        if !wanted.contains(&id) {
            wanted.push(id);
        }
    }
    let mut found = HashMap::new();
    if wanted.is_empty() {
        return found;
    }

    for own in kept {
        if wanted.contains(&own.mal_id) {
            found.insert(own.mal_id, own.clone());
        }
    }

    let mut missing = Vec::new();
    let unread: Vec<i64> = wanted
        .into_iter()
        .filter(|id| !found.contains_key(id))
        .collect();
    for id in unread {
        match state.caches.searches.get(&asked_key(id)).await {
            Some(text) => match serde_json::from_str::<mal::Described>(&text) {
                Ok(described) => {
                    found.insert(id, described);
                }
                Err(_) => missing.push(id),
            },
            None => missing.push(id),
        }
    }
    if !ask || missing.is_empty() || !state.flag("mal.enabled", false) {
        return found;
    }

    missing.truncate(ASKED_MOST);
    let fallback = state.flag("mal.fallback", true);
    let mut asked = stream::iter(missing)
        .map(|id| async move { (id, state.mal.entry(id, fallback).await) })
        .buffer_unordered(4);
    while let Some((id, answer)) = asked.next().await {
        match answer {
            Ok(Some(described)) => {
                if let Ok(text) = serde_json::to_string(&described) {
                    state.caches.searches.insert(asked_key(id), text).await;
                }
                found.insert(id, described);
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    mal_id = id,
                    error = format_args!("{e:#}"),
                    "MyAnimeList could not describe an entry of a series"
                );
                break;
            }
        }
    }
    found
}

/// What MyAnimeList's description of an entry is kept under.
fn asked_key(mal_id: i64) -> String {
    format!("mal-entry:{mal_id}")
}

/// When an episode airs: the instant a provider gave, when it falls within
/// a day of the episode's day — a day corrected here outweighs the time
/// that came with the old one — else midnight UTC of its day.
fn airs(episode: &Episode) -> Option<DateTime<Utc>> {
    let day = episode.air_date.as_deref().and_then(day_in);
    let instant = episode
        .air_date_utc
        .as_deref()
        .and_then(|text| DateTime::parse_from_rfc3339(text.trim()).ok())
        .map(|at| at.with_timezone(&Utc));
    match (instant, day) {
        (Some(at), Some(day)) if (at.date_naive() - day).num_days().abs() <= 1 => Some(at),
        (Some(at), None) => Some(at),
        (_, Some(day)) => day.and_hms_opt(0, 0, 0).map(|midnight| midnight.and_utc()),
        (None, None) => None,
    }
}

/// An episode's day, `YYYY-MM-DD`: the one it is dated, else its instant's
/// in UTC.
fn day_of(episode: &Episode, at: DateTime<Utc>) -> String {
    episode
        .air_date
        .as_deref()
        .and_then(day_in)
        .unwrap_or_else(|| at.date_naive())
        .format("%Y-%m-%d")
        .to_string()
}

/// The day a date, or a date-time, falls on.
fn day_in(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text.trim().get(..10)?, "%Y-%m-%d").ok()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;

    use super::*;
    use crate::{db::repo::child::blank_episode, domain::ExternalIds};

    /// 1 November 2026, noon UTC.
    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap()
    }

    fn entry(anilist: i64, season: Option<i32>, offset: Option<i32>, kind: &str) -> Entry {
        Entry {
            mal_id: Some(anilist + 100_000),
            anilist_id: Some(anilist),
            tvdb_id: Some(9_900_001),
            tvdb_season: season,
            tvdb_offset: offset,
            tmdb_movie: None,
            kind: Some(kind.into()),
        }
    }

    fn episode(season: i32, number: i32, day: &str) -> Episode {
        let mut episode = blank_episode(season, number);
        episode.is_manual = false;
        episode.air_date = Some(day.into());
        episode
    }

    /// A series of two seasons, the second split in two: its first half in
    /// summer, its second from October, airing at 15:00 UTC on Sundays.
    fn split() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.id = "split".into();
        item.title = "A split series".into();
        item.status = Some("continuing".into());
        item.episodes = (1..=12)
            .map(|n| episode(1, n, &format!("2026-01-{:02}", n * 2)))
            .chain((1..=6).map(|n| episode(2, n, &format!("2026-07-{:02}", n * 4))))
            .chain(
                [
                    "2026-10-04",
                    "2026-10-11",
                    "2026-10-18",
                    "2026-10-25",
                    "2026-11-01",
                    "2026-11-08",
                ]
                .iter()
                .zip(7..)
                .map(|(day, n)| {
                    let mut e = episode(2, n, day);
                    e.air_date_utc = Some(format!("{day}T15:00:00Z"));
                    e
                }),
            )
            .collect();
        item
    }

    fn split_entries() -> Vec<Entry> {
        vec![
            entry(1, Some(1), None, "TV"),
            entry(2, Some(2), None, "TV"),
            entry(3, Some(2), Some(6), "TV"),
        ]
    }

    #[test]
    fn an_entry_runs_from_its_offset_to_where_the_next_one_starts() {
        let entries = vec![
            entry(1, Some(1), None, "TV"),
            entry(3, Some(3), Some(12), "TV"),
            entry(2, Some(3), None, "TV"),
            // Twice: the list repeats an entry for each TMDB film.
            entry(3, Some(3), Some(12), "TV"),
            entry(4, Some(0), Some(4), "MOVIE"),
            entry(5, Some(0), Some(1), "OVA"),
            entry(6, Some(0), None, "OVA"),
        ];
        let spans = |planned: &HashMap<i64, i64>| -> Vec<(Option<i64>, Option<Span>)> {
            place(&entries, planned)
                .into_iter()
                .map(|(e, span)| (e.anilist_id, span))
                .collect()
        };

        let season = |number, first, last| {
            Some(Span::Season {
                number,
                first,
                last,
            })
        };
        assert_eq!(
            spans(&HashMap::new()),
            [
                (Some(4), season(0, 5, Some(5))),
                (Some(1), season(1, 1, None)),
                (Some(2), season(3, 1, Some(12))),
                (Some(3), season(3, 13, None)),
                // An OVA in season zero without an offset, and one without
                // MyAnimeList's count, in the order of their places.
                (Some(6), None),
                (Some(5), None),
            ]
        );
        // MyAnimeList's count places the OVA, never past the film after it.
        let placed = spans(&HashMap::from([(100_005, 3)]));
        assert!(placed.contains(&(Some(5), season(0, 2, Some(4)))));
        let placed = spans(&HashMap::from([(100_005, 9)]));
        assert!(placed.contains(&(Some(5), season(0, 2, Some(4)))));
    }

    #[test]
    fn two_entries_at_one_place_are_told_apart_by_their_kind_or_not_at_all() {
        let entries = vec![
            entry(1, Some(1), None, "MOVIE"),
            entry(2, Some(1), Some(0), "TV"),
            entry(3, Some(2), None, "TV"),
            entry(4, Some(2), None, "TV"),
            entry(5, None, None, "TV"),
        ];
        let placed: Vec<(Option<i64>, Option<Span>)> = place(&entries, &HashMap::new())
            .into_iter()
            .map(|(e, span)| (e.anilist_id, span))
            .collect();

        assert_eq!(
            placed,
            [
                (Some(5), Some(Span::Work)),
                (
                    Some(2),
                    Some(Span::Season {
                        number: 1,
                        first: 1,
                        last: None
                    })
                ),
                (Some(1), None),
                (Some(3), None),
                (Some(4), None),
            ]
        );
    }

    #[test]
    fn a_split_season_is_described_part_by_part() {
        let cours = of_series(&split(), &split_entries(), &HashMap::new(), now());
        let [first, summer, autumn] = cours.as_slice() else {
            panic!("{cours:#?}")
        };

        assert_eq!(first.status, Some(Status::Finished), "a season follows it");
        assert!(first.opens_work && !first.ends_work && !first.whole);
        assert_eq!(first.quarter, Some((Quarter::Winter, 2026)));

        assert_eq!((summer.episodes, summer.aired), (6, 6));
        assert_eq!(summer.numbers, Some((1, 6)));
        assert_eq!(summer.status, Some(Status::Finished));
        assert_eq!(
            (summer.starts.as_deref(), summer.ends.as_deref()),
            (Some("2026-07-04"), Some("2026-07-24"))
        );
        assert_eq!(summer.quarter, Some((Quarter::Summer, 2026)));

        assert_eq!(autumn.anilist, Some(3));
        assert_eq!((autumn.episodes, autumn.aired), (6, 4));
        assert_eq!(autumn.numbers, Some((7, 12)));
        assert_eq!(autumn.status, Some(Status::Releasing));
        assert_eq!(autumn.ends, None, "not finished");
        assert_eq!(autumn.quarter, Some((Quarter::Autumn, 2026)));
        assert!(autumn.ends_work && !autumn.opens_work);
        // Today's at 15:00, three hours on: its fifth, the season's 11th.
        let next = autumn.next.as_ref().expect("one to come");
        assert_eq!((next.episode, next.episode_number), (5, 11));
        assert_eq!(next.day, "2026-11-01");
        assert_eq!(next.airing_at, now().timestamp() + 3 * 3600);
        assert!(!autumn.corrected && !autumn.made_here);
    }

    #[test]
    fn an_entry_nothing_follows_has_finished_once_it_has_been_quiet_a_while() {
        let one = |last_day: &str| {
            let mut item = MediaItem::empty(MediaKind::Series);
            item.status = Some("continuing".into());
            item.episodes = vec![episode(1, 1, "2026-09-01"), episode(1, 2, last_day)];
            of_series(
                &item,
                &[entry(1, Some(1), None, "TV")],
                &HashMap::new(),
                now(),
            )
            .remove(0)
        };

        let quiet = one("2026-10-01");
        assert!(quiet.whole, "the one entry of a series of one season");
        assert_eq!(quiet.status, Some(Status::Finished));
        assert_eq!(quiet.ends.as_deref(), Some("2026-10-01"));
        // Its last a week ago: the next may not be listed yet.
        assert_eq!(one("2026-10-25").status, Some(Status::Releasing));

        // One listed without a day is still to come; a series that ended
        // has finished whatever the gap.
        let mut item = MediaItem::empty(MediaKind::Series);
        item.status = Some("continuing".into());
        item.episodes = vec![episode(1, 1, "2026-09-01"), blank_episode(1, 2)];
        let entries = [entry(1, Some(1), None, "TV")];
        let listed = of_series(&item, &entries, &HashMap::new(), now()).remove(0);
        assert_eq!(listed.status, Some(Status::Releasing));
        item.status = Some("ended".into());
        let ended = of_series(&item, &entries, &HashMap::new(), now()).remove(0);
        assert_eq!(ended.status, Some(Status::Finished));
        assert!(ended.made_here, "the undated one was made by hand");

        // Nothing aired yet; nothing held at all.
        item.episodes = vec![episode(1, 1, "2027-01-08")];
        let coming = of_series(&item, &entries, &HashMap::new(), now()).remove(0);
        assert_eq!(coming.status, Some(Status::NotYetReleased));
        assert_eq!(coming.next.map(|n| n.episode), Some(1));
        item.episodes.clear();
        let empty = of_series(&item, &entries, &HashMap::new(), now()).remove(0);
        assert_eq!((empty.status, empty.starts), (None, None));
    }

    #[test]
    fn a_schedule_corrected_here_is_told_entry_by_entry() {
        let mut item = split();
        item.locked_fields = vec!["episode:2x9/airDateUtc".into(), "item/title".into()];
        let cours = of_series(&item, &split_entries(), &HashMap::new(), now());
        assert_eq!(
            cours.iter().map(|c| c.corrected).collect::<Vec<_>>(),
            [false, false, true]
        );

        let mut made = blank_episode(2, 13);
        made.air_date = Some("2026-11-15".into());
        item.episodes.push(made);
        let cours = of_series(&item, &split_entries(), &HashMap::new(), now());
        assert!(cours[2].made_here && cours[2].corrected);
        assert_eq!((cours[2].episodes, cours[2].numbers), (7, Some((7, 13))));
    }

    #[test]
    fn a_day_corrected_here_outweighs_the_time_that_came_with_the_old_one() {
        let mut moved = episode(1, 1, "2026-11-03");
        moved.air_date_utc = Some("2026-11-01T15:00:00Z".into());
        assert_eq!(
            airs(&moved),
            Some(Utc.with_ymd_and_hms(2026, 11, 3, 0, 0, 0).unwrap())
        );
        // A time within a day of its day is its own: late on the 2nd in
        // Tokyo is the 2nd's afternoon in UTC.
        moved.air_date_utc = Some("2026-11-02T15:30:00+00:00".into());
        assert_eq!(
            airs(&moved),
            Some(Utc.with_ymd_and_hms(2026, 11, 2, 15, 30, 0).unwrap())
        );
        assert_eq!(airs(&blank_episode(1, 1)), None);
    }

    #[test]
    fn an_entry_for_the_whole_series_counts_along_it() {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.status = Some("continuing".into());
        item.episodes = vec![
            episode(1, 1, "2026-01-04"),
            episode(1, 2, "2026-01-11"),
            episode(2, 1, "2026-10-25"),
            episode(2, 2, "2026-11-08"),
        ];
        let entries = [entry(1, None, None, "TV")];

        let along = of_series(&item, &entries, &HashMap::new(), now()).remove(0);
        assert!(along.whole && along.opens_work && along.ends_work);
        assert_eq!(along.next.as_ref().map(|n| n.episode), Some(4));
        assert_eq!(along.numbers, None, "no one season's numbers");
        assert_eq!(along.starts.as_deref(), Some("2026-01-04"));

        // TheTVDB's own count along the series, where it gives one.
        item.episodes[3].absolute_episode_number = Some(1093);
        let along = of_series(&item, &entries, &HashMap::new(), now()).remove(0);
        assert_eq!(along.next.map(|n| n.episode), Some(1093));

        // On a long break, nothing listed: still airing, until it ends.
        item.episodes.truncate(3);
        let resting = of_series(&item, &entries, &HashMap::new(), now()).remove(0);
        assert_eq!(resting.status, Some(Status::Releasing));
        item.status = Some("ended".into());
        let ended = of_series(&item, &entries, &HashMap::new(), now()).remove(0);
        assert_eq!(ended.status, Some(Status::Finished));
        assert_eq!(ended.ends.as_deref(), Some("2026-10-25"));
    }

    #[test]
    fn a_work_the_list_does_not_split_is_one_entry_of_all_its_episodes() {
        let item = split();
        let whole = of_unlisted(&item, Some(7), Some(8), now());
        assert!(whole.whole && whole.opens_work && whole.ends_work && !whole.closed);
        assert_eq!((whole.anilist, whole.mal), (Some(7), Some(8)));
        assert_eq!(whole.span, Some(Span::Work));
        assert_eq!(whole.episodes, 24);
        assert_eq!(whole.starts.as_deref(), Some("2026-01-02"));
        assert_eq!(whole.status, Some(Status::Releasing));
        // The second season's eleventh, the series' twenty-third.
        let next = whole.next.expect("one to come");
        assert_eq!((next.episode, next.episode_number), (23, 11));

        let film = of_unlisted(&MediaItem::empty(MediaKind::Movie), Some(9), None, now());
        assert!(film.whole && film.closed && film.episodes == 0);
        assert_eq!((film.anilist, film.span), (Some(9), Some(Span::Work)));
    }

    #[test]
    fn anilists_winter_runs_from_december_into_the_next_year() {
        use AnilistSeason::{Fall, Spring, Summer, Winter};
        // As AniList answered on 2 November 2026: 28 December 2024 is its
        // winter of 2025, `seasonInt` 251; and its own examples.
        assert_eq!(AnilistSeason::of("2024-12-28"), Some((Winter, 2025)));
        assert_eq!(Winter.int(2025), 251);
        assert_eq!(AnilistSeason::of("1999-10-20"), Some((Fall, 1999)));
        assert_eq!(Fall.int(1999), 994);
        assert_eq!(AnilistSeason::of("2023-09-29"), Some((Fall, 2023)));
        assert_eq!(Fall.int(2023), 234);
        assert_eq!(AnilistSeason::of("2016-08-26"), Some((Summer, 2016)));
        assert_eq!(Summer.int(2016), 163);
        // Each boundary, and an instant, and a month alone.
        for (day, season, year) in [
            ("2026-01-01", Winter, 2026),
            ("2026-02-28", Winter, 2026),
            ("2026-03-01", Spring, 2026),
            ("2026-05-31", Spring, 2026),
            ("2026-06-01", Summer, 2026),
            ("2026-08-31", Summer, 2026),
            ("2026-09-01", Fall, 2026),
            ("2026-11-30", Fall, 2026),
            ("2026-12-01", Winter, 2027),
            ("2026-12-31T23:00:00Z", Winter, 2027),
            ("2026-12", Winter, 2027),
            ("2026-04", Spring, 2026),
        ] {
            assert_eq!(AnilistSeason::of(day), Some((season, year)), "{day}");
        }
        for nothing in ["2026", "", "late 2026", "2026-13"] {
            assert_eq!(AnilistSeason::of(nothing), None, "{nothing}");
        }
        assert_eq!(Spring.anilist(), "SPRING");
        assert_eq!(Winter.int(2000), 1);
        // The season chart's quarters are its own still: winter from January.
        assert_eq!(Quarter::of("2026-12-01"), Some((Quarter::Autumn, 2026)));
    }

    #[test]
    fn a_scheduled_episode_keeps_its_id_however_the_list_splits_its_series() {
        let item = split();
        let apart = of_series(&item, &split_entries(), &HashMap::new(), now());
        let together = of_series(
            &item,
            &[entry(1, Some(1), None, "TV"), entry(2, Some(2), None, "TV")],
            &HashMap::new(),
            now(),
        );
        let second_half = schedule(&item, &apart[2]);
        let whole = schedule(&item, &together[1]);
        // The season's seventh episode: the first of its second half, split
        // apart, and the seventh of the season, listed whole.
        assert_eq!((second_half[0].episode, whole[6].episode), (1, 7));
        assert_eq!(second_half[0].id, whole[6].id);
        // Each episode under an id of its own, the catalogue's own where it
        // has one.
        let distinct: std::collections::HashSet<i64> = whole.iter().map(|a| a.id).collect();
        assert_eq!(distinct.len(), whole.len());
        let mut named = item.clone();
        named.episodes[18].id = "an-episode-of-the-catalogue".into();
        let renamed = schedule(&named, &apart[2]);
        assert_eq!(
            renamed[0].id,
            crate::service::ids::anilist_airing("an-episode-of-the-catalogue")
        );
        assert_eq!(renamed[1].id, second_half[1].id);
    }

    #[test]
    fn an_entrys_schedule_is_its_own_episodes_counted_from_its_first() {
        let item = split();
        let cours = of_series(&item, &split_entries(), &HashMap::new(), now());
        let autumn = schedule(&item, &cours[2]);
        assert_eq!(autumn.len(), 6);
        assert_eq!(
            (autumn[0].episode, autumn[0].airing_at),
            (
                1,
                Utc.with_ymd_and_hms(2026, 10, 4, 15, 0, 0)
                    .unwrap()
                    .timestamp()
            )
        );
        assert_eq!(autumn[5].episode, 6);
        // Midnight of its day for an episode with no time.
        let summer = schedule(&item, &cours[1]);
        assert_eq!(summer.len(), 6);
        assert_eq!(
            summer[0].airing_at,
            Utc.with_ymd_and_hms(2026, 7, 4, 0, 0, 0)
                .unwrap()
                .timestamp()
        );

        // Along the whole series, by TheTVDB's count where it gives one; an
        // episode with no day is not on it.
        let mut along = MediaItem::empty(MediaKind::Series);
        along.episodes = vec![
            episode(1, 1, "2026-01-04"),
            episode(2, 1, "2026-10-25"),
            blank_episode(2, 2),
        ];
        along.episodes[1].absolute_episode_number = Some(1093);
        let whole = of_series(
            &along,
            &[entry(1, None, None, "TV")],
            &HashMap::new(),
            now(),
        );
        let listed: Vec<i32> = schedule(&along, &whole[0])
            .iter()
            .map(|a| a.episode)
            .collect();
        assert_eq!(listed, [1, 1093]);

        // An entry the list does not place has none.
        let unplaced = Cour {
            span: None,
            ..whole[0].clone()
        };
        assert!(schedule(&along, &unplaced).is_empty());
    }

    #[test]
    fn a_film_is_its_own_entry() {
        let mut film = MediaItem::empty(MediaKind::Movie);
        film.external_ids = ExternalIds {
            anilist: vec![199],
            mal: vec![199],
            ..ExternalIds::default()
        };
        let listed = [
            Entry {
                tvdb_season: None,
                ..entry(100, None, None, "SPECIAL")
            },
            Entry {
                tmdb_movie: Some(129),
                ..entry(199, None, None, "MOVIE")
            },
        ];
        let own = of_film(&film, &listed);
        assert_eq!(own.len(), 1);
        assert_eq!((own[0].anilist, own[0].whole), (Some(199), true));

        // The list not knowing it: the one entry it goes by.
        assert_eq!(of_film(&film, &[])[0].mal, Some(199));
        film.external_ids.anilist.push(200);
        assert!(of_film(&film, &[]).is_empty(), "two entries, neither sure");
    }

    #[test]
    fn an_entry_named_beside_the_work_gives_its_cour_a_title() {
        let cours = of_series(&split(), &split_entries(), &HashMap::new(), now());
        let named = [
            repo::anime::Named {
                anilist_id: Some(2),
                mal_id: None,
                title: "A split series Season 2".into(),
            },
            repo::anime::Named {
                anilist_id: None,
                mal_id: Some(100_003),
                title: "A split series Season 2 Part 2".into(),
            },
        ];
        assert_eq!(kept_title(&named, &cours[0]), None);
        assert_eq!(
            kept_title(&named, &cours[1]).as_deref(),
            Some("A split series Season 2")
        );
        assert_eq!(
            kept_title(&named, &cours[2]).as_deref(),
            Some("A split series Season 2 Part 2"),
            "by MyAnimeList's id"
        );
    }

    // ─── MyAnimeList ─────────────────────────────────────────────────────────

    /// A stand-in for Tenrai answering every request with `body`, and the
    /// request line of each.
    async fn tenrai(body: String) -> (String, std::sync::Arc<parking_lot::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = format!("http://{}", listener.local_addr().unwrap());
        let asked = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let seen = asked.clone();
        tokio::spawn(async move {
            while let Ok((mut connection, _)) = listener.accept().await {
                let mut request = vec![0u8; 8192];
                let read = connection.read(&mut request).await.unwrap_or(0);
                seen.lock().push(
                    String::from_utf8_lossy(&request[..read])
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                );
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = connection.write_all(answer.as_bytes()).await;
                let _ = connection.shutdown().await;
            }
        });
        (at, asked)
    }

    #[tokio::test]
    async fn myanimelist_describes_the_works_own_entry_from_what_it_kept_and_is_asked_the_rest() {
        let second = serde_json::json!({ "data": {
            "mal_id": 100_003, "title": "Split 2nd Season Part 2", "type": "TV",
            "episodes": 6, "status": "Currently Airing", "source": "Light novel",
            "studios": [{ "name": "Madhouse" }],
            "aired": { "from": "2026-10-04T00:00:00+00:00", "to": null },
            "relations": [{ "relation": "Prequel", "entry": [
                { "mal_id": 100_002, "type": "anime", "name": "Split 2nd Season" },
                { "mal_id": 7, "type": "manga", "name": "The manga" }
            ]}]
        }});
        let (at, asked) = tenrai(second.to_string()).await;

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
        config.mal.enabled = true;
        config.mal.client_id = None;
        config.mal.tenrai_upstream.clone_from(&at);
        config.mal.jikan_upstream.clone_from(&at);
        let state = AppState::bootstrap(config).await.expect("a server");

        let mut item = split();
        item.id = crate::db::new_id();
        item.title = "A split series to describe".into();
        item.slug = crate::domain::make_slug(&item.title, None);
        item.created_at = crate::db::now();
        item.updated_at = crate::db::now();
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        // What MyAnimeList answered for the work's own entry, its first season.
        let own = serde_json::json!({ "data": {
            "mal_id": 100_001, "title": "Split", "title_english": "A split series",
            "type": "TV", "episodes": 12, "status": "Finished Airing", "source": "Manga",
            "synopsis": "It splits.\n\n[Written by MAL Rewrite]"
        }});
        repo::snapshot::put(&state.db, &item.id, "mal", &own.to_string(), None)
            .await
            .unwrap();
        let cours = of_series(&item, &split_entries(), &HashMap::new(), now());

        let at_hand = described(&state, &item, &cours, false).await;
        assert_eq!(at_hand.len(), 1, "{at_hand:?}");
        let first = &at_hand[&100_001];
        assert_eq!(first.synopsis.as_deref(), Some("It splits."));
        assert_eq!(
            (first.status.as_deref(), first.source.as_deref()),
            (Some("finished_airing"), Some("manga"))
        );
        assert!(asked.lock().is_empty(), "nothing asked");

        // Asked: the others, in full — their relations are in that form only.
        let all = described(&state, &item, &cours, true).await;
        assert_eq!(all.len(), 3);
        let autumn = &all[&100_003];
        assert_eq!(autumn.media_type.as_deref(), Some("tv"));
        assert_eq!(autumn.source.as_deref(), Some("light_novel"));
        assert_eq!(autumn.starts.as_deref(), Some("2026-10-04"));
        assert_eq!(autumn.studios, ["Madhouse"]);
        assert_eq!(
            autumn.related,
            [mal::Related {
                mal_id: 100_002,
                relation: "prequel".into(),
                title: Some("Split 2nd Season".into()),
            }]
        );
        let lines = asked.lock().clone();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines.iter().all(|line| line.contains("/full ")),
            "{lines:?}"
        );

        // Kept a while: asked again, nothing goes out.
        assert_eq!(described(&state, &item, &cours, true).await.len(), 3);
        assert_eq!(asked.lock().len(), 2);
    }
}
