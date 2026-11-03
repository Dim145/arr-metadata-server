//! The orders a series' episodes come in, and where each places them.
//!
//! The aired order is the work's own: its episodes as the merge numbered
//! them. Every other order — TheTVDB's DVD, absolute and further orders,
//! TMDB's own numbering, a TMDB episode group, one made here — is a sequence
//! of places kept beside it ([`crate::db::repo::order`]), each naming an
//! episode the work holds or carrying the record of one it does not; worked
//! out here, once, for every client that numbers a series by an order:
//! Sonarr v5 by its season type, Plex by its episode order, the TheTVDB relay
//! by the season type in its path, the public site's season pages.
//!
//! One order may be frozen as the aired numbering: what a client asking for
//! the aired order is then given, Sonarr v4 and Plex with no order named
//! among them ([`as_official`]).

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
};

use anyhow::Result;
use chrono::NaiveDate;
use futures::{StreamExt, stream};

use crate::{
    db::repo::{self, order::LOCAL},
    domain::{
        Episode, EpisodeOrder, MediaItem, MediaKind, OrderKind, OrderRecord, PlacedEpisode, Season,
    },
    service::ids,
    state::AppState,
};

pub use crate::{
    db::repo::order::{AIRED, AIRED_LABEL},
    domain::orders::{KEY_MOST, MADE_HERE, TMDB_AIRED, TMDB_GROUP, TVDB_LAST},
};

/// The most places one order holds: the longest series ever made is far
/// short of it, and an order written by hand is not let past it.
pub const PLACES_MOST: usize = 10_000;

/// The aired order among a work's orders: the work's own episodes.
pub fn aired(orders: &[EpisodeOrder]) -> Option<&EpisodeOrder> {
    orders.iter().find(|order| order.is_default)
}

/// The order `key` names among those the catalogue keeps for a work. None
/// for the aired order, and none for a key the work has no order of: a
/// client asks in the order a person chose, and is answered in the aired one
/// rather than refused — a refusal would fail its refresh.
pub fn named<'a>(orders: &'a [EpisodeOrder], key: &str) -> Option<&'a EpisodeOrder> {
    let wanted = key.trim();
    if wanted.eq_ignore_ascii_case(AIRED) {
        return None;
    }
    orders
        .iter()
        .find(|order| !order.is_default && order.key.eq_ignore_ascii_case(wanted))
}

/// [`named`], when the order places an episode. One in name only — its
/// places all name episodes the work no longer holds and carry no record of
/// their own, as after a refresh without TheTVDB — is no order to answer in:
/// a client set to it would be sent no episode at all. The aired order
/// stands in.
pub fn in_use<'a>(
    item: &'a MediaItem,
    orders: &'a [EpisodeOrder],
    key: &str,
) -> Option<&'a EpisodeOrder> {
    named(orders, key).filter(|order| !placements(item, order).is_empty())
}

/// The order frozen as the aired numbering, when one is and it places an
/// episode.
pub fn frozen<'a>(item: &'a MediaItem, orders: &'a [EpisodeOrder]) -> Option<&'a EpisodeOrder> {
    orders
        .iter()
        .find(|order| order.is_frozen && !order.is_default)
        .filter(|order| !placements(item, order).is_empty())
}

/// One episode where an order puts it.
#[derive(Clone, Debug)]
pub struct Placed<'a> {
    pub season_number: i32,
    pub episode_number: i32,
    pub absolute_number: Option<i32>,
    /// The episode: one the work holds, as it holds it — its own numbers
    /// are the aired ones — or the order's own record of one it does not,
    /// numbered by its place.
    pub episode: Cow<'a, Episode>,
    /// Whether it is the order's own record.
    pub own: bool,
    /// The episode's id on TMDB: as one of TMDB's orders names the place,
    /// which is TMDB's word for it where the merge gave the episode held the
    /// id of another — or the episode's own.
    pub tmdb_id: Option<i64>,
}

/// The episodes an order places, in its sequence, each place once: the
/// episode the work holds for it — found by its TheTVDB id, its TMDB id, or
/// the row it is kept in — or else the record the order carries of it. A
/// place that names an episode the work no longer holds and carries no
/// record is left out; a place given twice is kept once — Sonarr threw on a
/// season and number it met twice, and Plex keys an episode by them.
pub fn placements<'a>(item: &'a MediaItem, order: &'a EpisodeOrder) -> Vec<Placed<'a>> {
    let held = Held::of(item);
    let tmdbs = order.source == crate::providers::names::TMDB;
    let looked = Look::of(order);
    let mut seen = HashSet::new();
    order
        .episodes
        .iter()
        .filter_map(|place| {
            let (episode, own) = match held.find(place, looked) {
                Some(episode) => (Cow::Borrowed(episode), false),
                None => (Cow::Owned(own_episode(place, place.record.as_ref()?)), true),
            };
            let tmdb_id = if tmdbs {
                place.tmdb_id.or(episode.tmdb_id)
            } else {
                episode.tmdb_id.or(place.tmdb_id)
            };
            seen.insert((place.season_number, place.episode_number))
                .then_some(Placed {
                    season_number: place.season_number,
                    episode_number: place.episode_number,
                    absolute_number: place.absolute_number,
                    episode,
                    own,
                    tmdb_id,
                })
        })
        .collect()
}

/// The work's own episodes where the aired order puts them: as they are
/// numbered.
pub fn aired_placements(item: &MediaItem) -> Vec<Placed<'_>> {
    item.episodes
        .iter()
        .map(|episode| Placed {
            season_number: episode.season_number,
            episode_number: episode.episode_number,
            absolute_number: episode.absolute_episode_number,
            episode: Cow::Borrowed(episode),
            own: false,
            tmdb_id: episode.tmdb_id,
        })
        .collect()
}

/// The episodes an order numbers, as a client is served them: its
/// placements, and — for an order made here, and for one frozen as the aired
/// numbering — every episode the work holds that the order does not place
/// and that airs after it was last written, under its aired numbers where
/// the order leaves them free. A frozen numbering does not keep an episode
/// that airs next from Sonarr; one an order made from the DVDs leaves out on
/// purpose aired before it, and stays out.
pub fn served<'a>(item: &'a MediaItem, order: &'a EpisodeOrder) -> Vec<Placed<'a>> {
    let mut placed = placements(item, order);
    if order.source != LOCAL && !order.is_frozen {
        return placed;
    }
    // The numbering TheTVDB last gave places what aired since itself, after
    // its last episode (`service::numbering`): the work's own episodes are
    // TMDB's meanwhile, and under TMDB's numbers they would stand beside
    // the same episodes placed. What was added here by hand is served all
    // the same, whenever it is dated: the order is written again at every
    // refresh, and the day it was last written says nothing of when a
    // person added an episode.
    let by_hand_only = order.key == TVDB_LAST;

    let cutoff = order.updated_at.get(..10).unwrap_or_default();
    let taken: HashSet<(i32, i32)> = placed
        .iter()
        .map(|p| (p.season_number, p.episode_number))
        .collect();
    let placed_ids: HashSet<&str> = placed
        .iter()
        .filter(|p| !p.own)
        .map(|p| p.episode.id.as_str())
        .collect();
    let later: Vec<Placed<'a>> = item
        .episodes
        .iter()
        .filter(|episode| !placed_ids.contains(episode.id.as_str()))
        .filter(|episode| match by_hand_only {
            true => episode.is_manual,
            false => match episode.air_date.as_deref() {
                Some(day) => day.get(..10).unwrap_or(day) > cutoff,
                // Not dated yet: one still to air, in a season of its own; an
                // undated special says nothing of when it came.
                None => episode.season_number > 0,
            },
        })
        .filter(|episode| !taken.contains(&(episode.season_number, episode.episode_number)))
        .map(|episode| Placed {
            season_number: episode.season_number,
            episode_number: episode.episode_number,
            absolute_number: episode.absolute_episode_number,
            episode: Cow::Borrowed(episode),
            own: false,
            tmdb_id: episode.tmdb_id,
        })
        .collect();
    placed.extend(later);
    placed
}

/// The episodes the aired order answers with when an order is frozen as it:
/// that order's, served ([`served`]). None while no order is.
pub fn as_official<'a>(item: &'a MediaItem, orders: &'a [EpisodeOrder]) -> Option<Vec<Placed<'a>>> {
    frozen(item, orders).map(|order| served(item, order))
}

/// The work numbered as the aired order answers, for a client that reads
/// one numbering and no orders — Sonarr v4, a `.nfo` document: its episodes
/// renumbered by the order frozen as the aired numbering, when one is, and
/// its seasons those that numbering has. A season of the order lends the
/// aired season of its number its pictures, and not its name: the DVDs'
/// first season is not the first season as it aired. Nothing moves while no
/// order is frozen.
///
/// Answers the ids of the episodes it numbered that are the order's own —
/// records of episodes the work does not hold —, for a client that knows
/// every episode by an id to give them one ([`own_episode_id`]).
pub fn number_as_official(item: &mut MediaItem, orders: &[EpisodeOrder]) -> HashSet<String> {
    let Some(placed) = as_official(item, orders) else {
        return HashSet::new();
    };
    let own: HashSet<String> = placed
        .iter()
        .filter(|placed| placed.own)
        .map(|placed| placed.episode.id.clone())
        .collect();
    let episodes: Vec<Episode> = placed.into_iter().map(numbered).collect();
    let used: Vec<i32> = {
        let mut used: Vec<i32> = episodes.iter().map(|e| e.season_number).collect();
        used.sort_unstable();
        used.dedup();
        used
    };
    let seasons: Vec<Season> = used
        .iter()
        .map(|&number| {
            let mut season = item
                .seasons
                .iter()
                .find(|s| s.season_number == number)
                .cloned()
                .unwrap_or_else(|| {
                    let mut bare = crate::db::repo::child::blank_season(number);
                    bare.is_manual = false;
                    bare
                });
            season.title = None;
            season.overview = None;
            season
        })
        .collect();
    item.episodes = episodes;
    item.seasons = seasons;
    own
}

/// The id an episode of an order's own is known by where a client needs one
/// and TheTVDB gave it none — Sonarr v4 and v5, a page of the TheTVDB relay:
/// one made of what names it, its TMDB id or else its place
/// ([`ids::own_episode`]), negative, never one of TheTVDB's, and the same at
/// every answer and for every client.
pub fn own_episode_id(episode: &Episode) -> i64 {
    match episode.tmdb_id {
        Some(tmdb) => ids::own_episode(&format!("tmdb:{tmdb}")),
        None => ids::own_episode(&format!("place:{}", episode.id)),
    }
}

/// An episode as an order numbers it: its place's numbers on it.
pub fn numbered(placed: Placed<'_>) -> Episode {
    let mut episode = placed.episode.into_owned();
    episode.season_number = placed.season_number;
    episode.episode_number = placed.episode_number;
    episode.absolute_episode_number = placed.absolute_number;
    // Where a special airs among the aired seasons says nothing of where
    // an order puts it.
    episode.aired_after_season_number = None;
    episode.aired_before_season_number = None;
    episode.aired_before_episode_number = None;
    episode
}

/// The orders a client is offered besides the aired one: those that place
/// an episode — or, still to be read, say they hold some — but for one the
/// source numbering the series keeps already.
///
/// TheTVDB and TMDB often both keep a series' DVD order, or its absolute
/// one: the two are the same order twice, and Sonarr would list both. The
/// one from the source that numbers the series always stands — TheTVDB's,
/// unless TMDB numbers it while TheTVDB does not answer — and the other's
/// is offered beside it only when it holds more episodes: then it is another
/// order, a Blu-ray's with its extras, more than a copy. Of the aired kind,
/// a group is offered beside the aired order when it holds more episodes
/// than the series does, and not otherwise. TheTVDB's further orders, TMDB's
/// own numbering — Plex's TMDB agent offers it whatever the series — and
/// the orders made here are each their own and are always offered. One not
/// offered is still answered when a client asks for it by its key.
///
/// TMDB's own numbering is no order of TheTVDB's documents: Sonarr's
/// `/v2/tvdb` leaves it out of what this offers, and its TMDB edition
/// answers by it (`wire::sonarr_v2`).
pub fn offered<'a>(item: &'a MediaItem, orders: &'a [EpisodeOrder]) -> Vec<&'a EpisodeOrder> {
    let spine = aired(orders).map_or("tvdb", |order| order.source.as_str());
    let sized: Vec<(&EpisodeOrder, usize)> = orders
        .iter()
        .filter(|order| !order.is_default)
        .map(|order| (order, size(item, order)))
        .filter(|(_, size)| *size > 0)
        .collect();
    let aired_size = item.episodes.len();

    let outdone = |order: &EpisodeOrder, mine: usize| -> bool {
        if order.key == TMDB_AIRED {
            return false;
        }
        // The numbering TheTVDB last gave is the aired order while it is
        // frozen as it, and an order of its own once thawed.
        if order.key == TVDB_LAST {
            return order.is_frozen;
        }
        match order.kind {
            OrderKind::TvdbAlt | OrderKind::Custom => false,
            OrderKind::Aired => mine <= aired_size,
            _ if order.source == spine => false,
            kind => sized.iter().any(|(other, theirs)| {
                other.kind == kind && other.source == spine && *theirs >= mine
            }),
        }
    };

    sized
        .iter()
        .filter(|(order, mine)| !outdone(order, *mine))
        .map(|(order, _)| *order)
        .collect()
}

/// The name each of `offered` goes by where a client lists the orders to
/// choose from — Sonarr v5's season types, Plex's — in the same order. Its
/// label; but where another of them, or the aired order as that client names
/// it (`aired_name`), goes by the same name, one that is not from the source
/// numbering the series says its source after it — "Superfan Order (TMDB)"
/// beside TheTVDB's "Superfan Order" — and two of one source still alike say
/// how many episodes they hold as well. The label kept in the catalogue is
/// left as it is: the site's pages tell the orders apart by their source
/// beside the name.
pub fn client_names(
    item: &MediaItem,
    orders: &[EpisodeOrder],
    offered: &[&EpisodeOrder],
    aired_name: &str,
) -> Vec<String> {
    let spine = aired(orders).map_or("tvdb", |order| order.source.as_str());
    let folded = |name: &str| name.trim().to_lowercase();
    let uses = |names: &[String]| {
        let mut uses: HashMap<String, usize> = HashMap::new();
        for name in std::iter::once(aired_name).chain(names.iter().map(String::as_str)) {
            *uses.entry(folded(name)).or_default() += 1;
        }
        uses
    };
    let shared = |uses: &HashMap<String, usize>, name: &str| {
        uses.get(&folded(name)).copied().unwrap_or_default() > 1
    };

    let labels: Vec<String> = offered.iter().map(|order| order.label.clone()).collect();
    let first = uses(&labels);
    let named: Vec<String> = offered
        .iter()
        .zip(labels)
        .map(|(order, label)| {
            if shared(&first, &label) && order.source != spine {
                format!("{label} ({})", source_name(&order.source))
            } else {
                label
            }
        })
        .collect();

    let again = uses(&named);
    offered
        .iter()
        .zip(named)
        .map(|(order, name)| {
            if shared(&again, &name) {
                format!(
                    "{} ({}, {} episodes)",
                    order.label,
                    source_name(&order.source),
                    size(item, order)
                )
            } else {
                name
            }
        })
        .collect()
}

/// What a source is called after an order's name in a client's list.
fn source_name(source: &str) -> &str {
    match source {
        "tvdb" => "TheTVDB",
        "tmdb" => "TMDB",
        LOCAL => "made here",
        other => other,
    }
}

/// How many episodes an order places, or says it holds while it is to be
/// read.
pub fn size(item: &MediaItem, order: &EpisodeOrder) -> usize {
    if order.episodes.is_empty() {
        order
            .announced
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0)
    } else {
        placements(item, order).len()
    }
}

/// An order as the work names its episodes now, for whoever shows or edits
/// it: each place that names an episode the work holds carries that
/// episode's id as the work gives it — a refresh writes a source's episodes
/// again under new ones — and its ids elsewhere, without the record its
/// source gave of it; one it does not hold carries its record, as kept. One
/// of TMDB's orders keeps TMDB's id for each place: see [`Placed::tmdb_id`].
pub fn resolved(item: &MediaItem, order: &EpisodeOrder) -> EpisodeOrder {
    let held = Held::of(item);
    let tmdbs = order.source == crate::providers::names::TMDB;
    let looked = Look::of(order);
    let mut resolved = order.clone();
    for place in &mut resolved.episodes {
        if let Some(episode) = held.find(place, looked) {
            place.episode_id = Some(episode.id.clone());
            place.tvdb_id = episode.tvdb_id.or(place.tvdb_id);
            place.tmdb_id = if tmdbs {
                place.tmdb_id.or(episode.tmdb_id)
            } else {
                episode.tmdb_id.or(place.tmdb_id)
            };
            place.record = None;
        }
    }
    resolved
}

/// The places of a new order made from `from`: an episode the work holds by
/// the ids that name it, a record of its own as it is. From the aired order,
/// the work's episodes as they are numbered.
pub fn places_from(item: &MediaItem, from: Option<&EpisodeOrder>) -> Vec<PlacedEpisode> {
    let placed: Vec<Placed<'_>> = match from {
        Some(order) if !order.is_default => placements(item, order),
        _ => aired_placements(item),
    };
    placed
        .into_iter()
        .map(|placed| {
            let episode = placed.episode.as_ref();
            PlacedEpisode {
                id: crate::db::new_id(),
                season_number: placed.season_number,
                episode_number: placed.episode_number,
                absolute_number: placed.absolute_number,
                episode_id: (!placed.own).then(|| episode.id.clone()),
                tvdb_id: episode.tvdb_id,
                tmdb_id: placed.tmdb_id,
                record: placed.own.then(|| record_of(episode)),
            }
        })
        .collect()
}

/// A place naming an episode the work holds, by the ids that name it from
/// one refresh to the next.
pub fn place_of(
    episode: &Episode,
    season_number: i32,
    episode_number: i32,
    absolute_number: Option<i32>,
) -> PlacedEpisode {
    PlacedEpisode {
        id: crate::db::new_id(),
        season_number,
        episode_number,
        absolute_number,
        episode_id: Some(episode.id.clone()),
        tvdb_id: episode.tvdb_id,
        tmdb_id: episode.tmdb_id,
        record: None,
    }
}

/// The key of a new order made here, from its name: `ams-` and the name's
/// words, lower case, joined by hyphens — `ams-my-dvd-order` — with a number
/// after it when the work has an order of that key already. Letters, digits
/// and hyphens alone: it is a segment of Sonarr's address for the series,
/// and Plex keys an episode by it.
pub fn key_for(label: &str, taken: &[String]) -> String {
    let words = slug::slugify(label);
    let mut stem: String = words
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect();
    stem = stem.trim_matches('-').to_string();
    if stem.is_empty() {
        stem = "order".to_string();
    }
    // Room for the prefix and a number after it.
    let room = KEY_MOST - MADE_HERE.len() - 4;
    if stem.len() > room {
        stem.truncate(room);
        stem = stem.trim_end_matches('-').to_string();
    }
    let first = format!("{MADE_HERE}{stem}");
    if !taken.contains(&first) {
        return first;
    }
    (2..)
        .map(|n| format!("{MADE_HERE}{stem}-{n}"))
        .find(|key| !taken.contains(key))
        .expect("an unused number")
}

/// The episodes the work holds, by every id a place may name them by.
struct Held<'a> {
    by_tvdb: HashMap<i64, &'a Episode>,
    by_tmdb: HashMap<i64, &'a Episode>,
    by_row: HashMap<&'a str, &'a Episode>,
}

impl<'a> Held<'a> {
    fn of(item: &'a MediaItem) -> Self {
        let mut held = Self {
            by_tvdb: HashMap::new(),
            by_tmdb: HashMap::new(),
            by_row: HashMap::new(),
        };
        for episode in &item.episodes {
            if let Some(id) = episode.tvdb_id {
                held.by_tvdb.entry(id).or_insert(episode);
            }
            if let Some(id) = episode.tmdb_id {
                held.by_tmdb.entry(id).or_insert(episode);
            }
            if !episode.id.is_empty() {
                held.by_row.insert(episode.id.as_str(), episode);
            }
        }
        held
    }

    /// The episode a place names: by TheTVDB's id first, the one Sonarr
    /// knows it by, then TMDB's, then the row it was kept in.
    ///
    /// In TMDB's own numbering ([`Look::TmdbOwn`]), not by TMDB's id when
    /// the episode of that id aired days apart from the place's record: the
    /// merge gives an episode TheTVDB numbers the TMDB id of TMDB's at the
    /// same numbers, which is another episode where the two number a series
    /// otherwise. The episode TMDB's place stands for is then named by its
    /// TheTVDB id or its row, when it is known ([`linked`]), and the place
    /// is otherwise served from its record.
    ///
    /// In the numbering TheTVDB last gave ([`Look::Kept`]), by no id the
    /// episode found aired days apart from the place's record — TMDB numbers
    /// the work meanwhile, and its episodes keep TheTVDB's ids as best they
    /// can — and by TMDB's id where TheTVDB's names none the work holds: the
    /// series TheTVDB merged it into has ids of its own for each episode.
    fn find(&self, place: &PlacedEpisode, look: Look) -> Option<&'a Episode> {
        let fits = |found: &Episode| match look {
            Look::Plain => true,
            Look::TmdbOwn | Look::Kept => !aired_apart(place.record.as_ref(), found),
        };
        if let Some(id) = place.tvdb_id
            && let Some(found) = self.by_tvdb.get(&id).copied()
            && (look != Look::Kept || fits(found))
        {
            return Some(found);
        }
        // A place that names its episode by TheTVDB's id names one the work
        // does not hold when that id is not found: TMDB's id, filled in by
        // the merge where the numbers matched, is no surer — but for the
        // numbering TheTVDB last gave.
        if (place.tvdb_id.is_none() || look == Look::Kept)
            && let Some(id) = place.tmdb_id
            && let Some(found) = self.by_tmdb.get(&id).copied()
            && fits(found)
        {
            return Some(found);
        }
        place
            .episode_id
            .as_deref()
            .and_then(|row| self.by_row.get(row).copied())
    }
}

/// How an order's places are looked for among the episodes the work holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Look {
    Plain,
    /// TMDB's own numbering: see [`Held::find`].
    TmdbOwn,
    /// The numbering TheTVDB last gave: see [`Held::find`].
    Kept,
}

impl Look {
    fn of(order: &EpisodeOrder) -> Self {
        match order.key.as_str() {
            TMDB_AIRED => Self::TmdbOwn,
            TVDB_LAST => Self::Kept,
            _ => Self::Plain,
        }
    }
}

/// The day a date names, from its first ten characters.
fn day_of(date: Option<&str>) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(date?.trim().get(..10)?, "%Y-%m-%d").ok()
}

/// Whether a record and an episode aired more than a day apart: two
/// episodes, not one dated in two time zones. Not when either is undated.
fn aired_apart(record: Option<&OrderRecord>, episode: &Episode) -> bool {
    match (
        day_of(record.and_then(|r| r.aired.as_deref())),
        day_of(episode.air_date.as_deref()),
    ) {
        (Some(theirs), Some(ours)) => (theirs - ours).num_days().abs() > 1,
        _ => false,
    }
}

/// An episode made of the record an order carries, numbered by its place.
fn own_episode(place: &PlacedEpisode, record: &OrderRecord) -> Episode {
    let mut episode =
        crate::db::repo::child::blank_episode(place.season_number, place.episode_number);
    episode.id = place.id.clone();
    episode.absolute_episode_number = place.absolute_number;
    episode.title = record.title.clone();
    episode.overview = record.overview.clone();
    episode.air_date = record.aired.clone();
    episode.runtime = record.runtime;
    episode.image = record.image.clone();
    episode.finale_type = record.finale_type.clone();
    episode.tvdb_id = place.tvdb_id;
    episode.tmdb_id = place.tmdb_id;
    episode.is_manual = false;
    episode
}

/// The record of an episode, to carry in a place of its own.
pub(crate) fn record_of(episode: &Episode) -> OrderRecord {
    OrderRecord {
        title: episode.title.clone(),
        overview: episode.overview.clone(),
        aired: episode.air_date.clone(),
        runtime: episode.runtime,
        image: episode.image.clone(),
        finale_type: episode.finale_type.clone(),
    }
}

// ─── reading them ────────────────────────────────────────────────────────────

/// What a refresh read of a series' orders: TheTVDB's, when it was asked;
/// the episode groups TMDB lists, and TMDB's own numbering, when it
/// answered.
#[derive(Default)]
pub struct Read {
    pub tvdb: bool,
    pub tmdb_groups: Option<Vec<EpisodeOrder>>,
    pub tmdb_aired: Option<TmdbAired>,
}

/// TMDB's own numbering of a series, as one answer of TMDB's gives it: a
/// place for each episode its seasons list, at TMDB's numbers, carrying
/// TMDB's record of it — what the order serves where the work holds no
/// episode for the place — and the seasons it could not give this time.
#[derive(Clone, Debug, Default)]
pub struct TmdbAired {
    places: Vec<PlacedEpisode>,
    unanswered: Vec<i32>,
}

impl TmdbAired {
    /// From the episodes TMDB's answer lists as TMDB numbers them, before
    /// the merge numbers the series as TheTVDB does: those of a season that
    /// went unanswered — put back from the work as held, under its numbers —
    /// are not TMDB's, and the places kept for that season stand instead.
    pub fn of(episodes: &[Episode], unanswered: &[i32]) -> Self {
        Self {
            places: episodes
                .iter()
                .filter(|episode| !unanswered.contains(&episode.season_number))
                .map(|episode| PlacedEpisode {
                    id: crate::db::new_id(),
                    season_number: episode.season_number,
                    episode_number: episode.episode_number,
                    absolute_number: None,
                    episode_id: None,
                    tvdb_id: None,
                    tmdb_id: episode.tmdb_id,
                    record: Some(record_of(episode)),
                })
                .collect(),
            unanswered: unanswered.to_vec(),
        }
    }
}

/// Ask TheTVDB how else it numbers this series, and keep the answer beside
/// the groups TMDB listed for it, each group new or changed since it was
/// last read read now — a refresh asks TMDB already, and a page that shows
/// the orders asks it nothing ([`read_unread`]) — and beside TMDB's own
/// numbering, as its seasons cut the series this time. Best effort: a
/// failure is logged, and what was kept before stays.
pub async fn gather(state: &AppState, item: &MediaItem, read: Read) {
    if item.kind != MediaKind::Series {
        return;
    }

    if let Some(groups) = read.tmdb_groups {
        match repo::order::list_groups(&state.db, &item.id, &groups).await {
            Ok(()) => read_unread(state, item).await,
            Err(e) => {
                tracing::warn!(id = %item.id, error = %e, "could not keep TMDB's episode groups");
            }
        }
    }

    if let Some(aired) = read.tmdb_aired {
        keep_tmdb_aired(state, item, aired).await;
    }

    if !read.tvdb || !state.tvdb.is_enabled() {
        return;
    }
    let Some(tvdb_id) = item.external_ids.tvdb else {
        return;
    };

    let orders = match state.tvdb.orders(tvdb_id).await {
        Ok(orders) => orders,
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "TheTVDB's other episode orders could not be read"
            );
            return;
        }
    };

    if let Err(e) = repo::order::replace_source(&state.db, &item.id, "tvdb", &orders).await {
        tracing::warn!(id = %item.id, error = %e, "could not keep the episode orders");
    }
}

/// A work's orders as a client asking in the order `key` names is to be
/// served them: those kept, that one read first from its source when it is
/// a TMDB group still to be read — one listed by a refresh that could not
/// read it — or TMDB's own numbering of a work no refresh has kept it for
/// yet. The others are offered by the count their source listed them with,
/// and read when they are asked in, or at the next refresh.
pub async fn for_clients(
    state: &AppState,
    item: &MediaItem,
    key: Option<&str>,
) -> Result<Vec<EpisodeOrder>> {
    if item.kind != MediaKind::Series {
        return Ok(Vec::new());
    }
    if let Some(key) = key.map(str::trim).filter(|key| !key.is_empty()) {
        if key.eq_ignore_ascii_case(TMDB_AIRED) {
            let kept = repo::order::list(&state.db, &item.id).await?;
            if named(&kept, TMDB_AIRED).is_some() {
                return Ok(kept);
            }
            read_tmdb_aired(state, item).await;
            return repo::order::list(&state.db, &item.id).await;
        }
        let to_read: Vec<(String, String)> = repo::order::unread(&state.db, &item.id)
            .await?
            .into_iter()
            .filter(|(held, _)| held.eq_ignore_ascii_case(key))
            .collect();
        if !to_read.is_empty() {
            read_groups(state, item, &to_read).await;
        }
    }
    repo::order::list(&state.db, &item.id).await
}

/// Keep TMDB's own numbering of a work, as one answer of TMDB's gave it:
/// each place naming the episode the work holds for it, where one is known
/// to be that one ([`linked`]); a season TMDB could not give this time keeps
/// the places kept for it. Best effort, as [`gather`] is.
async fn keep_tmdb_aired(state: &AppState, item: &MediaItem, aired: TmdbAired) {
    let TmdbAired {
        mut places,
        unanswered,
    } = aired;
    if !unanswered.is_empty() {
        match repo::order::get(&state.db, &item.id, TMDB_AIRED).await {
            Ok(Some(kept)) => {
                places.extend(
                    kept.episodes
                        .into_iter()
                        .filter(|place| unanswered.contains(&place.season_number)),
                );
                places.sort_by_key(|place| (place.season_number, place.episode_number));
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(id = %item.id, error = %e, "could not read TMDB's numbering as kept");
            }
        }
    }
    let places = linked(item, places);
    if let Err(e) = repo::order::write_tmdb_aired(&state.db, &item.id, &places).await {
        tracing::warn!(id = %item.id, error = %e, "could not keep TMDB's numbering");
    }
}

/// TMDB's places, each naming the episode the work holds for it where one
/// is known to be that one: the episode of the same TMDB id when the two
/// aired the same day, give or take one — the merge gives an episode TheTVDB
/// numbers the TMDB id of TMDB's at the same numbers, another episode where
/// the two number a series otherwise — or else the one episode the work
/// holds that aired the day TMDB's did, when that day has one on each side,
/// a special matched with a special and a regular episode with a regular
/// one. Named by its TheTVDB id and its row, which [`Held::find`] reads
/// first. A place none is known for names none, and is served from its
/// record.
fn linked(item: &MediaItem, mut places: Vec<PlacedEpisode>) -> Vec<PlacedEpisode> {
    let mut by_tmdb: HashMap<i64, &Episode> = HashMap::new();
    for episode in &item.episodes {
        if let Some(id) = episode.tmdb_id {
            by_tmdb.entry(id).or_insert(episode);
        }
    }
    let mut taken: HashSet<&str> = HashSet::new();
    let name = |place: &mut PlacedEpisode, episode: &Episode| {
        place.tvdb_id = episode.tvdb_id;
        place.episode_id = Some(episode.id.clone());
    };

    for place in &mut places {
        place.tvdb_id = None;
        place.episode_id = None;
        let Some(episode) = place.tmdb_id.and_then(|id| by_tmdb.get(&id).copied()) else {
            continue;
        };
        if !aired_apart(place.record.as_ref(), episode) && taken.insert(episode.id.as_str()) {
            name(place, episode);
        }
    }

    // The day each place left aired, and each episode left, special or not.
    let day = |place: &PlacedEpisode| {
        day_of(place.record.as_ref().and_then(|r| r.aired.as_deref()))
            .map(|day| (place.season_number == 0, day))
    };
    let mut places_on: HashMap<(bool, NaiveDate), usize> = HashMap::new();
    for place in places.iter().filter(|p| p.episode_id.is_none()) {
        if let Some(on) = day(place) {
            *places_on.entry(on).or_default() += 1;
        }
    }
    let mut held_on: HashMap<(bool, NaiveDate), Vec<&Episode>> = HashMap::new();
    for episode in item
        .episodes
        .iter()
        .filter(|e| !e.id.is_empty() && !taken.contains(e.id.as_str()))
    {
        if let Some(on) = day_of(episode.air_date.as_deref()) {
            held_on
                .entry((episode.season_number == 0, on))
                .or_default()
                .push(episode);
        }
    }
    for place in places.iter_mut().filter(|p| p.episode_id.is_none()) {
        let Some(on) = day(place) else { continue };
        if places_on.get(&on) != Some(&1) {
            continue;
        }
        let only: Option<&Episode> = match held_on.get(&on).map(Vec::as_slice) {
            Some([episode]) => Some(*episode),
            _ => None,
        };
        if let Some(episode) = only
            && taken.insert(episode.id.as_str())
        {
            name(place, episode);
        }
    }
    places
}

/// Read TMDB's own numbering of a work from TMDB, and keep it: for a work
/// asked in it that no refresh has kept it for since this server began
/// keeping it. Best effort: nothing is kept when TMDB does not answer, and
/// a client is answered in the numbering it falls back on.
async fn read_tmdb_aired(state: &AppState, item: &MediaItem) {
    let Some(tmdb_id) = item.external_ids.tmdb else {
        return;
    };
    if !state.tmdb.is_configured() {
        return;
    }
    let tv = match state.tmdb.tv(tmdb_id).await {
        Ok(Some((_, tv))) => tv,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(
                tmdb_id,
                error = format_args!("{e:#}"),
                "TMDB's numbering of a series could not be read"
            );
            return;
        }
    };
    let numbers: Vec<i32> = tv
        .seasons
        .iter()
        .map(|season| season.season_number)
        .take(crate::service::gather::MAX_SEASONS)
        .collect();
    let (seasons, unanswered) = state.tmdb.tv_seasons(tmdb_id, &numbers).await;
    let episodes = crate::providers::tmdb::map::tv_to_item(&tv, &seasons, 0).episodes;
    keep_tmdb_aired(state, item, TmdbAired::of(&episodes, &unanswered)).await;
}

/// Read every TMDB group of a work still to be read — listed for the first
/// time, or holding another count of episodes than when it was read. Best
/// effort: see [`read_groups`].
async fn read_unread(state: &AppState, item: &MediaItem) {
    match repo::order::unread(&state.db, &item.id).await {
        Ok(to_read) if !to_read.is_empty() => read_groups(state, item, &to_read).await,
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(id = %item.id, error = %e, "could not tell which TMDB groups to read");
        }
    }
}

/// How many of a work's TMDB groups are asked for at once: The Simpsons'
/// two dozen are not sent to TMDB in one burst.
const GROUPS_AT_ONCE: usize = 4;

/// Read TMDB's episode groups, by key and group id, and keep their places.
/// Best effort, each on its own: a group that cannot be read stays to be
/// read, and is offered by the count TMDB listed it with.
async fn read_groups(state: &AppState, item: &MediaItem, groups: &[(String, String)]) {
    if !state.tmdb.is_configured() {
        return;
    }
    // Owned, each: futures that borrowed the list would not be `Send` to
    // the scheduler that runs a refresh.
    let read: Vec<_> = stream::iter(groups.to_vec())
        .map(|(key, group)| async move {
            let answer = state.tmdb.episode_group(&group).await;
            (key, group, answer)
        })
        .buffer_unordered(GROUPS_AT_ONCE)
        .collect()
        .await;
    for (key, group, answer) in read {
        let places = match answer {
            Ok(Some(detail)) => crate::providers::tmdb::map::episode_group_places(&detail),
            // Gone at TMDB since it was listed: nothing to place, and read.
            Ok(None) => Vec::new(),
            Err(e) => {
                tracing::warn!(
                    id = %item.id,
                    group = %group,
                    error = format_args!("{e:#}"),
                    "a TMDB episode group could not be read"
                );
                continue;
            }
        };
        if let Err(e) = repo::order::write_read(&state.db, &item.id, &key, &places).await {
            tracing::warn!(id = %item.id, %key, error = %e, "could not keep a TMDB group's episodes");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::repo::child::blank_episode,
        domain::orders::{TMDB_AIRED_LABEL, is_key},
    };

    fn place(tvdb_id: i64, season: i32, number: i32) -> PlacedEpisode {
        PlacedEpisode::of_tvdb(tvdb_id, season, number)
    }

    fn own(title: &str, season: i32, number: i32, tvdb_id: Option<i64>) -> PlacedEpisode {
        PlacedEpisode {
            tvdb_id,
            record: Some(OrderRecord {
                title: title.into(),
                overview: None,
                aired: Some("2005-03-24".into()),
                runtime: Some(38),
                image: None,
                finale_type: None,
            }),
            ..PlacedEpisode::of_tvdb(0, season, number)
        }
    }

    fn order(
        key: &str,
        source: &str,
        kind: OrderKind,
        episodes: Vec<PlacedEpisode>,
    ) -> EpisodeOrder {
        EpisodeOrder {
            key: key.into(),
            kind,
            source: source.into(),
            source_ref: None,
            label: key.to_uppercase(),
            is_default: false,
            is_frozen: false,
            announced: None,
            updated_at: "2026-10-19T00:00:00.000Z".into(),
            episodes,
        }
    }

    fn aired_order(source: &str) -> EpisodeOrder {
        EpisodeOrder {
            is_default: true,
            label: AIRED_LABEL.into(),
            ..order(AIRED, source, OrderKind::Aired, Vec::new())
        }
    }

    fn series() -> MediaItem {
        let mut it = MediaItem::empty(MediaKind::Series);
        it.episodes = [
            (1, 1, 11, "2005-03-24"),
            (1, 2, 12, "2005-03-29"),
            (2, 1, 21, "2005-09-20"),
        ]
        .into_iter()
        .map(|(season, number, tvdb_id, aired)| {
            let mut episode = blank_episode(season, number);
            episode.id = format!("row-{tvdb_id}");
            episode.title = format!("Episode {tvdb_id}");
            episode.tvdb_id = Some(tvdb_id);
            episode.tmdb_id = Some(tvdb_id * 100);
            episode.air_date = Some(aired.into());
            episode.is_manual = false;
            episode
        })
        .collect();
        it
    }

    fn numbers(placed: &[Placed<'_>]) -> Vec<(i32, i32, String, bool)> {
        placed
            .iter()
            .map(|p| {
                (
                    p.season_number,
                    p.episode_number,
                    p.episode.title.clone(),
                    p.own,
                )
            })
            .collect()
    }

    #[test]
    fn an_order_places_the_episodes_the_work_has_once_each() {
        let it = series();
        let dvd = order(
            "dvd",
            "tvdb",
            OrderKind::Dvd,
            // Swapped; an episode the work no longer has; a place given twice.
            vec![
                place(12, 1, 1),
                place(11, 1, 2),
                place(99, 1, 3),
                place(11, 1, 2),
            ],
        );
        assert_eq!(
            numbers(&placements(&it, &dvd)),
            [
                (1, 1, "Episode 12".to_string(), false),
                (1, 2, "Episode 11".to_string(), false)
            ]
        );

        let orders = vec![aired_order("tvdb"), dvd];
        assert_eq!(named(&orders, " DVD ").map(|o| o.key.as_str()), Some("dvd"));
        assert!(named(&orders, AIRED).is_none());
        assert!(named(&orders, "absolute").is_none());
        assert!(in_use(&it, &orders, "dvd").is_some());
    }

    #[test]
    fn a_place_the_work_holds_no_episode_for_is_served_from_its_own_record() {
        // The Office's Superfan order: extended cuts, each an episode of its
        // own on TheTVDB, which the aired list does not hold.
        let it = series();
        let superfan = order(
            "alttwo",
            "tvdb",
            OrderKind::TvdbAlt,
            vec![
                own("Pilot (Extended Cut)", 1, 1, Some(10718410)),
                place(12, 1, 2),
            ],
        );
        let placed = placements(&it, &superfan);
        assert_eq!(
            numbers(&placed),
            [
                (1, 1, "Pilot (Extended Cut)".to_string(), true),
                (1, 2, "Episode 12".to_string(), false)
            ]
        );
        // The record keeps the id TheTVDB gave it, and its own date.
        assert_eq!(placed[0].episode.tvdb_id, Some(10718410));
        assert_eq!(placed[0].episode.air_date.as_deref(), Some("2005-03-24"));
        assert_eq!(placed[0].episode.runtime, Some(38));
        assert!(!placed[0].episode.is_manual);

        // A record the work does hold is served as the work holds it.
        let mut held = own("Pilot, as TheTVDB's DVD names it", 1, 1, Some(11));
        held.episode_id = None;
        let dvd = order("dvd", "tvdb", OrderKind::Dvd, vec![held]);
        assert_eq!(
            numbers(&placements(&it, &dvd)),
            [(1, 1, "Episode 11".to_string(), false)]
        );

        // An order of records alone is in use: Sonarr set to it is sent them.
        let orders = vec![
            aired_order("tvdb"),
            order(
                "alttwo",
                "tvdb",
                OrderKind::TvdbAlt,
                vec![own("Pilot (Extended Cut)", 1, 1, Some(10718410))],
            ),
        ];
        assert!(in_use(&it, &orders, "alttwo").is_some());
    }

    #[test]
    fn a_place_is_found_by_tmdb_s_id_or_its_row_when_it_names_no_tvdb_id() {
        let it = series();
        let by_tmdb = PlacedEpisode {
            tmdb_id: Some(2100),
            tvdb_id: None,
            ..place(0, 1, 1)
        };
        let by_row = PlacedEpisode {
            episode_id: Some("row-11".into()),
            tvdb_id: None,
            ..place(0, 1, 2)
        };
        // A TheTVDB id the work does not hold is not taken for TMDB's.
        let elsewhere = PlacedEpisode {
            tmdb_id: Some(1200),
            ..place(99, 1, 3)
        };
        let mine = order(
            "ams-mine",
            LOCAL,
            OrderKind::Custom,
            vec![by_tmdb, by_row, elsewhere],
        );
        assert_eq!(
            numbers(&placements(&it, &mine)),
            [
                (1, 1, "Episode 21".to_string(), false),
                (1, 2, "Episode 11".to_string(), false)
            ]
        );
    }

    #[test]
    fn an_order_in_name_only_is_none() {
        let it = series();
        let orders = vec![order(
            "regional",
            "tvdb",
            OrderKind::TvdbAlt,
            vec![place(99, 1, 1)],
        )];
        assert!(named(&orders, "regional").is_some());
        assert!(in_use(&it, &orders, "regional").is_none());
    }

    #[test]
    fn a_frozen_order_answers_for_the_aired_one_and_lets_a_later_episode_in() {
        let mut it = series();
        // Aired after the order was frozen, and not yet dated.
        let mut next = blank_episode(2, 2);
        next.id = "row-22".into();
        next.title = "Episode 22".into();
        next.tvdb_id = Some(22);
        next.air_date = Some("2026-11-01".into());
        next.is_manual = false;
        let mut tba = blank_episode(2, 3);
        tba.id = "row-23".into();
        tba.title = "Episode 23".into();
        tba.is_manual = false;
        it.episodes.extend([next, tba]);

        let mut mine = order(
            "ams-mine",
            LOCAL,
            OrderKind::Custom,
            // The second season's first episode is left out on purpose: it
            // aired before the order was written.
            vec![place(12, 1, 1), place(11, 1, 2), own("Extra", 1, 3, None)],
        );
        let mut orders = vec![aired_order("tvdb"), mine.clone()];
        assert!(as_official(&it, &orders).is_none(), "nothing is frozen yet");

        mine.is_frozen = true;
        orders[1] = mine;
        let official = as_official(&it, &orders).expect("frozen");
        assert_eq!(
            numbers(&official),
            [
                (1, 1, "Episode 12".to_string(), false),
                (1, 2, "Episode 11".to_string(), false),
                (1, 3, "Extra".to_string(), true),
                (2, 2, "Episode 22".to_string(), false),
                (2, 3, "Episode 23".to_string(), false),
            ]
        );

        // A client that reads one numbering is given that one.
        let mut v4 = it.clone();
        number_as_official(&mut v4, &orders);
        assert_eq!(
            v4.episodes
                .iter()
                .map(|e| (e.season_number, e.episode_number, e.title.as_str()))
                .collect::<Vec<_>>(),
            [
                (1, 1, "Episode 12"),
                (1, 2, "Episode 11"),
                (1, 3, "Extra"),
                (2, 2, "Episode 22"),
                (2, 3, "Episode 23")
            ]
        );
        assert_eq!(
            v4.seasons
                .iter()
                .map(|s| s.season_number)
                .collect::<Vec<_>>(),
            [1, 2]
        );

        // Thawed, the work is numbered as it aired again.
        orders[1].is_frozen = false;
        let mut thawed = it.clone();
        number_as_official(&mut thawed, &orders);
        assert_eq!(thawed.episodes.len(), it.episodes.len());
        assert_eq!(thawed.episodes[0].title, "Episode 11");
    }

    #[test]
    fn a_source_s_order_asked_by_its_key_serves_its_places_alone() {
        let it = series();
        let dvd = order("dvd", "tvdb", OrderKind::Dvd, vec![place(12, 1, 1)]);
        assert_eq!(served(&it, &dvd).len(), 1);
    }

    #[test]
    fn the_same_order_from_two_sources_is_offered_once() {
        let it = series();
        let tvdb_dvd = order(
            "dvd",
            "tvdb",
            OrderKind::Dvd,
            vec![place(12, 1, 1), place(11, 1, 2)],
        );
        // As many episodes: the source that numbers the series stands alone.
        let tmdb_dvd = EpisodeOrder {
            announced: Some(2),
            source_ref: Some("a1".into()),
            ..order("tmdb-a1", "tmdb", OrderKind::Dvd, Vec::new())
        };
        // More episodes: another order, offered beside it.
        let tmdb_absolute = EpisodeOrder {
            announced: Some(4),
            ..order("tmdb-a2", "tmdb", OrderKind::Absolute, Vec::new())
        };
        let tvdb_absolute = order(
            "absolute",
            "tvdb",
            OrderKind::Absolute,
            vec![place(11, 1, 1)],
        );
        // Of the aired kind, and no more episodes than the series has.
        let tmdb_aired = EpisodeOrder {
            announced: Some(3),
            ..order("tmdb-a3", "tmdb", OrderKind::Aired, Vec::new())
        };
        let superfan = order(
            "alttwo",
            "tvdb",
            OrderKind::TvdbAlt,
            vec![own("Pilot (Extended Cut)", 1, 1, Some(10718410))],
        );
        let tmdb_superfan = EpisodeOrder {
            announced: Some(186),
            ..order("tmdb-a4", "tmdb", OrderKind::Digital, Vec::new())
        };
        let mut orders = vec![
            aired_order("tvdb"),
            tvdb_dvd,
            tvdb_absolute,
            superfan,
            tmdb_dvd,
            tmdb_absolute,
            tmdb_aired,
            tmdb_superfan,
        ];
        let keys = |orders: &[EpisodeOrder]| -> Vec<String> {
            offered(&it, orders)
                .into_iter()
                .map(|o| o.key.clone())
                .collect()
        };
        assert_eq!(
            keys(&orders),
            ["dvd", "absolute", "alttwo", "tmdb-a2", "tmdb-a4"]
        );

        // While TMDB numbers the series, its orders stand, and TheTVDB's of
        // the same kind holding no more episodes are not offered.
        orders[0] = aired_order("tmdb");
        assert_eq!(keys(&orders), ["alttwo", "tmdb-a1", "tmdb-a2", "tmdb-a4"]);
    }

    #[test]
    fn two_orders_of_one_name_are_told_apart_by_the_source_not_numbering_the_series() {
        let it = series();
        let named = |key: &str, source: &str, label: &str, announced: i32| EpisodeOrder {
            label: label.into(),
            announced: Some(announced),
            ..order(key, source, OrderKind::TvdbAlt, Vec::new())
        };
        let mut orders = vec![
            aired_order("tvdb"),
            named("alttwo", "tvdb", "Superfan Order", 194),
            named("tmdb-a4", "tmdb", "Superfan order", 186),
            named("tmdb-a5", "tmdb", "Peacock", 242),
            // Two of one source alike: their counts tell them apart.
            named("tmdb-a6", "tmdb", "BluRay", 280),
            named("tmdb-a7", "tmdb", "BluRay", 277),
            // One made here under the aired order's own name.
            named("ams-aired-order", LOCAL, "Aired Order", 3),
        ];
        let names = |orders: &[EpisodeOrder]| {
            let all: Vec<&EpisodeOrder> = orders.iter().filter(|o| !o.is_default).collect();
            client_names(&it, orders, &all, AIRED_LABEL)
        };
        assert_eq!(
            names(&orders),
            [
                "Superfan Order",
                "Superfan order (TMDB)",
                "Peacock",
                "BluRay (TMDB, 280 episodes)",
                "BluRay (TMDB, 277 episodes)",
                "Aired Order (made here)",
            ]
        );
        // The labels kept are untouched.
        assert_eq!(orders[2].label, "Superfan order");

        // While TMDB numbers the series, TheTVDB's is the one named apart.
        orders[0] = aired_order("tmdb");
        assert_eq!(
            names(&orders)[..2],
            ["Superfan Order (TheTVDB)", "Superfan order"]
        );
    }

    #[test]
    fn a_new_order_takes_its_places_from_the_one_it_is_made_from() {
        let it = series();
        let from_aired = places_from(&it, None);
        assert_eq!(from_aired.len(), 3);
        assert_eq!(from_aired[0].episode_id.as_deref(), Some("row-11"));
        assert_eq!(from_aired[0].tvdb_id, Some(11));
        assert_eq!(from_aired[0].record, None);

        let superfan = order(
            "alttwo",
            "tvdb",
            OrderKind::TvdbAlt,
            vec![
                own("Pilot (Extended Cut)", 1, 1, Some(10718410)),
                place(12, 1, 2),
            ],
        );
        let from_superfan = places_from(&it, Some(&superfan));
        assert_eq!(from_superfan.len(), 2);
        assert_eq!(
            from_superfan[0].record.as_ref().map(|r| r.title.as_str()),
            Some("Pilot (Extended Cut)")
        );
        assert_eq!(from_superfan[0].tvdb_id, Some(10718410));
        assert_eq!(from_superfan[1].episode_id.as_deref(), Some("row-12"));
        assert_eq!(from_superfan[1].record, None);
    }

    #[test]
    fn a_key_is_made_of_the_name_and_never_taken_twice() {
        assert_eq!(key_for("My DVD order", &[]), "ams-my-dvd-order");
        assert_eq!(
            key_for("My DVD order", &["ams-my-dvd-order".into()]),
            "ams-my-dvd-order-2"
        );
        assert_eq!(
            key_for("Épisodes à l'endroit", &[]),
            "ams-episodes-a-l-endroit"
        );
        assert_eq!(key_for("!!!", &[]), "ams-order");
        let long = key_for(&"word ".repeat(40), &[]);
        assert!(is_key(&long) && long.len() <= KEY_MOST, "{long}");
        assert!(is_key("tmdb-5acf93e60e0a26346d0000ce"));
        assert!(is_key("alttwo"));
        assert!(!is_key("../dvd"));
        assert!(!is_key("DVD"));
        assert!(!is_key(""));
    }

    /// *Breaking Bad* as TheTVDB numbers it: three episodes in a first
    /// season, then a second; the merge gave the first two TMDB's ids of
    /// TMDB's at the same numbers, and the second season's first TMDB's id
    /// of its first — which TMDB numbers the first season's third.
    fn thetvdbs() -> MediaItem {
        let mut it = MediaItem::empty(MediaKind::Series);
        it.episodes = [
            (1, 1, 11, Some(101), "2008-01-20", "Pilot"),
            (1, 2, 12, Some(102), "2008-01-27", "Cat's in the Bag..."),
            (
                1,
                3,
                13,
                None,
                "2008-02-10",
                "...And the Bag's in the River",
            ),
            (2, 1, 21, Some(103), "2009-03-08", "Seven Thirty-Seven"),
        ]
        .into_iter()
        .map(|(season, number, tvdb_id, tmdb_id, aired, title)| {
            let mut episode = blank_episode(season, number);
            episode.id = format!("row-{tvdb_id}");
            episode.title = title.into();
            episode.tvdb_id = Some(tvdb_id);
            episode.tmdb_id = tmdb_id;
            episode.air_date = Some(aired.into());
            episode.is_manual = false;
            episode
        })
        .collect();
        it
    }

    /// TMDB's own seasons of it: the first season's third in a second
    /// season of its own, and TheTVDB's second season one further on.
    fn tmdbs() -> Vec<Episode> {
        [
            (1, 1, 101, "2008-01-20", "Pilot"),
            (1, 2, 102, "2008-01-27", "Cat's in the Bag..."),
            (2, 1, 103, "2008-02-10", "...And the Bag's in the River"),
            (2, 2, 104, "2009-03-08", "Seven Thirty-Seven"),
            (2, 3, 105, "2009-03-15", "Grilled"),
        ]
        .into_iter()
        .map(|(season, number, tmdb_id, aired, title)| {
            let mut episode = blank_episode(season, number);
            episode.title = format!("{title} (TMDB)");
            episode.tmdb_id = Some(tmdb_id);
            episode.air_date = Some(aired.into());
            episode.is_manual = false;
            episode
        })
        .collect()
    }

    fn tmdb_aired(places: Vec<PlacedEpisode>) -> EpisodeOrder {
        EpisodeOrder {
            label: TMDB_AIRED_LABEL.into(),
            ..order(TMDB_AIRED, "tmdb", OrderKind::Aired, places)
        }
    }

    #[test]
    fn tmdbs_own_numbering_names_the_episode_each_place_is_whatever_the_merge_said() {
        let it = thetvdbs();
        let read = TmdbAired::of(&tmdbs(), &[]);
        let own = tmdb_aired(linked(&it, read.places));

        // Where, by TheTVDB's id and TMDB's, named how, and whether the
        // order's own record.
        type Seen<'a> = (i32, i32, Option<i64>, Option<i64>, &'a str, bool);
        let placed = placements(&it, &own);
        let served: Vec<Seen<'_>> = placed
            .iter()
            .map(|p| {
                (
                    p.season_number,
                    p.episode_number,
                    p.episode.tvdb_id,
                    p.tmdb_id,
                    p.episode.title.as_str(),
                    p.own,
                )
            })
            .collect();
        assert_eq!(
            served,
            [
                // The same episode at the same numbers, by TMDB's id.
                (1, 1, Some(11), Some(101), "Pilot", false),
                (1, 2, Some(12), Some(102), "Cat's in the Bag...", false),
                // Not the episode the merge gave TMDB's id: the one that
                // aired that day, TheTVDB's third, at TMDB's numbers.
                (
                    2,
                    1,
                    Some(13),
                    Some(103),
                    "...And the Bag's in the River",
                    false
                ),
                (2, 2, Some(21), Some(104), "Seven Thirty-Seven", false),
                // One the work does not hold: TMDB's record of it.
                (2, 3, None, Some(105), "Grilled (TMDB)", true),
            ]
        );

        // A place no episode is known for, whose TMDB id the merge gave
        // another episode aired a year apart, is served from its record.
        let unlinked = tmdb_aired(TmdbAired::of(&tmdbs(), &[]).places);
        let third = placements(&it, &unlinked)
            .into_iter()
            .find(|p| (p.season_number, p.episode_number) == (2, 1))
            .expect("placed");
        assert!(third.own);
        assert_eq!(third.episode.title, "...And the Bag's in the River (TMDB)");
        // Any other order still finds the episode by TMDB's id.
        let group = EpisodeOrder {
            key: "tmdb-g1".into(),
            ..unlinked.clone()
        };
        let third = placements(&it, &group)
            .into_iter()
            .find(|p| (p.season_number, p.episode_number) == (2, 1))
            .expect("placed");
        assert_eq!(third.episode.tvdb_id, Some(21));
    }

    #[test]
    fn a_day_with_two_episodes_on_either_side_names_none_by_its_date() {
        let mut it = thetvdbs();
        // A double bill on TheTVDB's side, the day TMDB's (2,1) aired.
        let mut second = blank_episode(1, 4);
        second.id = "row-14".into();
        second.tvdb_id = Some(14);
        second.air_date = Some("2008-02-10".into());
        second.is_manual = false;
        it.episodes.push(second);

        let own = linked(&it, TmdbAired::of(&tmdbs(), &[]).places);
        let place = own
            .iter()
            .find(|p| (p.season_number, p.episode_number) == (2, 1))
            .expect("kept");
        assert_eq!((place.tvdb_id, place.episode_id.as_deref()), (None, None));
    }

    #[test]
    fn a_season_tmdb_did_not_give_is_none_of_its_places() {
        let read = TmdbAired::of(&tmdbs(), &[2]);
        assert_eq!(
            read.places
                .iter()
                .map(|p| (p.season_number, p.episode_number))
                .collect::<Vec<_>>(),
            [(1, 1), (1, 2)]
        );
        assert_eq!(read.unanswered, [2]);
        // Each a record of TMDB's, named by TMDB's id alone.
        assert!(read.places.iter().all(|p| p.record.is_some()
            && p.tvdb_id.is_none()
            && p.episode_id.is_none()
            && p.tmdb_id.is_some()));
    }

    #[test]
    fn tmdbs_own_numbering_is_offered_whatever_else_there_is() {
        let it = series();
        // As the aired order numbers the work, every episode at its own
        // numbers: offered all the same, as Plex's TMDB agent offers it.
        let same = tmdb_aired(
            it.episodes
                .iter()
                .map(|e| PlacedEpisode {
                    tmdb_id: e.tmdb_id,
                    tvdb_id: None,
                    ..place(0, e.season_number, e.episode_number)
                })
                .collect(),
        );
        let orders = vec![aired_order("tvdb"), same];
        assert_eq!(
            offered(&it, &orders)
                .into_iter()
                .map(|o| o.key.as_str())
                .collect::<Vec<_>>(),
            [TMDB_AIRED]
        );
        // Not when it places nothing.
        let empty = vec![aired_order("tvdb"), tmdb_aired(Vec::new())];
        assert!(offered(&it, &empty).is_empty());
        assert!(in_use(&it, &empty, TMDB_AIRED).is_none());
    }

    /// The numbering TheTVDB last gave, frozen and written again since: an
    /// episode a person added by hand is served whenever it is dated — the
    /// day the order was last written says nothing of when it was added —
    /// and the work's own episodes it does not place are not, under their
    /// own numbers.
    #[test]
    fn the_numbering_thetvdb_last_gave_serves_every_episode_added_by_hand() {
        let mut it = series();
        let mut by_hand = blank_episode(1, 3);
        by_hand.id = "row-by-hand".into();
        by_hand.title = "Added By Hand".into();
        by_hand.air_date = Some("2005-04-02".into());
        by_hand.is_manual = true;
        it.episodes.push(by_hand);

        let mut last = order(
            TVDB_LAST,
            crate::providers::names::TVDB,
            OrderKind::Aired,
            vec![place(11, 1, 1), place(12, 1, 2)],
        );
        last.is_frozen = true;
        // Written again at a refresh, long after.
        last.updated_at = "2026-11-03T00:00:00.000Z".into();
        let orders = vec![aired_order("tmdb"), last];
        assert_eq!(
            numbers(&as_official(&it, &orders).expect("frozen")),
            [
                (1, 1, "Episode 11".to_string(), false),
                (1, 2, "Episode 12".to_string(), false),
                (1, 3, "Added By Hand".to_string(), false),
            ],
            "the second season's episode, TMDB's, is not served under its own numbers"
        );
    }
}
