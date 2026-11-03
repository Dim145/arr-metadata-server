//! The numbering of a series TheTVDB no longer knows (wave P, P4).
//!
//! TheTVDB numbers a series; Sonarr holds it by TheTVDB's id and has matched
//! its files by those numbers. When TheTVDB no longer knows the series —
//! it answered at two refreshes in a row, the most part of a day apart, that
//! it has no such series, or its feed of updates says it deleted the series
//! or merged it into another — nothing will number it as TheTVDB did again.
//! Its numbering as it was then served is kept as an order, [`TVDB_LAST`],
//! frozen as the aired numbering: every client asking for the aired order
//! goes on being answered in it, under the TheTVDB id it holds the series by.
//! The episodes that air afterwards are placed after its last episode, by the
//! day they aired ([`following`]): read from TheTVDB under the id it merged
//! the series into, else from TMDB's own numbering (`tmdb-aired`), else from
//! TVmaze.
//!
//! Meanwhile the work itself is numbered as when TheTVDB stops answering for
//! it (`service::gather`): by TMDB, where TMDB answers with a list, each
//! episode keeping the TheTVDB id it had; by TheTVDB again for a series it
//! merged into another, under the id it kept. It lasts until TheTVDB knows
//! the series again, or — for one it merged — until the numbering is given
//! back by hand.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use chrono::{NaiveDate, TimeDelta, Utc};

use crate::{
    db::repo::{self, audit::Action},
    domain::{
        Episode, EpisodeOrder, ExternalSource, MediaItem, MediaKind, OrderRecord, PlacedEpisode,
    },
    providers::names,
    service::orders,
    state::AppState,
};

pub use crate::domain::orders::{TVDB_LAST, TvdbDropped};

/// How many refreshes in a row TheTVDB answers that it has no such series,
/// the most part of a day apart, before the series is numbered as TheTVDB
/// last gave it.
pub const DROP_AFTER: i32 = 2;

/// What the id of a place that follows TheTVDB's last numbering begins with:
/// an episode aired since, placed after it. The order is written whole at
/// each refresh, and these alone are written anew; the others are TheTVDB's
/// numbering as it was kept, and never move.
pub const NEXT: &str = "next-";

/// Whether a place of the kept numbering follows it: an episode aired since.
pub fn is_next(place: &PlacedEpisode) -> bool {
    place.id.starts_with(NEXT)
}

/// Whether the fallback numbering may be used at all: `numbering.fallbackAfter`
/// at `0` turns it off, for a series TheTVDB stopped answering for and for
/// one it no longer knows alike.
pub fn allowed(state: &AppState) -> bool {
    state
        .settings
        .int_at("numbering.fallbackAfter", None, None)
        .unwrap_or(crate::service::gather::FALLBACK_AFTER)
        > 0
}

// ─── keeping it ──────────────────────────────────────────────────────────────

/// The places a work's numbering is kept as: what the aired order answers
/// with now — the order frozen as it, when one is, else the work's own
/// episodes as they are numbered — each naming the episode by every id it
/// has, and carrying the record of it, served wherever no episode the work
/// holds stands for the place any more.
pub fn kept_places(item: &MediaItem, orders: &[EpisodeOrder]) -> Vec<PlacedEpisode> {
    orders::as_official(item, orders)
        .unwrap_or_else(|| orders::aired_placements(item))
        .into_iter()
        .map(|placed| {
            let episode = placed.episode.as_ref();
            PlacedEpisode {
                id: crate::db::new_id(),
                season_number: placed.season_number,
                episode_number: placed.episode_number,
                absolute_number: placed.absolute_number,
                episode_id: (!placed.own && !episode.id.is_empty()).then(|| episode.id.clone()),
                tvdb_id: episode.tvdb_id,
                tmdb_id: placed.tmdb_id,
                record: Some(orders::record_of(episode)),
            }
        })
        .collect()
}

/// Keep a work's numbering as it is served now, and freeze it as the aired
/// numbering: TheTVDB no longer knows the series. `held` is the work as it
/// stands — its episodes as TheTVDB numbered them last, or TMDB while
/// TheTVDB was silent. Another order frozen meanwhile is thawed: its
/// numbering is the one kept. Written in one with `numbering`, what is kept
/// of who numbers the work: neither is written without the other.
pub async fn keep_last(
    state: &AppState,
    held: &MediaItem,
    numbering: &repo::order::Numbering,
) -> Result<()> {
    let all = repo::order::list(&state.db, &held.id).await?;
    let places = kept_places(held, &all);
    repo::order::keep_last(&state.db, &held.id, &places, numbering).await?;
    state.caches.touched(&held.id).await;
    Ok(())
}

/// What the audit trail and the notifications say of a series whose
/// numbering is now kept as TheTVDB last gave it.
pub fn dropped_detail(
    title: &str,
    why: TvdbDropped,
    tvdb_id: Option<i64>,
    merged_into: Option<i64>,
) -> String {
    let id = tvdb_id.map(|id| format!(" {id}")).unwrap_or_default();
    let reason = match (why, merged_into) {
        (TvdbDropped::Merged, Some(into)) => {
            format!("TheTVDB merged the series{id} into {into}")
        }
        (TvdbDropped::Merged | TvdbDropped::Deleted, _) => {
            format!("TheTVDB deleted the series{id}")
        }
        (TvdbDropped::Missing, _) => {
            format!("TheTVDB answered at {DROP_AFTER} refreshes in a row that it has no series{id}")
        }
    };
    format!(
        "“{title}”: {reason}; its numbering as it was served is kept and frozen as the aired \
         numbering, and the episodes that air afterwards are placed after it"
    )
}

/// What a series whose numbering TheTVDB's feed says to keep comes to.
#[derive(Debug, PartialEq, Eq)]
pub enum Dropped {
    /// Kept now — or kept already, and TheTVDB's feed says more of why: a
    /// series that went missing was deleted, or merged into another.
    Kept,
    /// Kept already, and the feed says nothing more.
    Already,
    /// Not a series held here under the id TheTVDB deleted: none at all, or
    /// one that only answers to it — merged here from a work that held it,
    /// or holding since the id TheTVDB merged it into — which TheTVDB still
    /// knows under the id it holds.
    NotHeld,
}

/// Whether what TheTVDB's feed says (`why`, `merged_into`) says more than
/// what is kept of a series already dropped (`held`).
fn says_more(held: Option<TvdbDropped>, why: TvdbDropped, merged_into: Option<i64>) -> bool {
    match (held, why) {
        (Some(TvdbDropped::Merged), _) => false,
        (_, TvdbDropped::Merged) => merged_into.is_some(),
        (Some(TvdbDropped::Deleted), _) => false,
        (_, TvdbDropped::Deleted) => true,
        (_, TvdbDropped::Missing) => false,
    }
}

/// Keep the numbering of the series held as `media_id` under TheTVDB id
/// `deleted`, which TheTVDB's feed says it deleted — or merged into
/// `merged_into` — as it is served now, and say so: in the audit trail,
/// `item.numbering_fallback`, and to the notifications that want
/// `numbering.fallback`. A series it merged into another takes that one's
/// id at its next refresh, unless the catalogue holds that one as another
/// work, and goes on answering to the one it had.
///
/// Nothing for a work that only answers to `deleted`, holding another
/// TheTVDB id: TheTVDB still knows it, under that one.
pub async fn drop_held(
    state: &AppState,
    media_id: &str,
    deleted: i64,
    why: TvdbDropped,
    merged_into: Option<i64>,
) -> Result<Dropped> {
    let Some(mut item) = repo::item::get(&state.db, media_id).await? else {
        return Ok(Dropped::NotHeld);
    };
    if item.kind != MediaKind::Series {
        return Ok(Dropped::NotHeld);
    }
    let mut held = repo::order::numbering(&state.db, media_id).await?;
    // A merge read before, whether its numbering is still kept or was given
    // back since: the feed's readings overlap.
    let merged_before = held.merged_into.is_some() && held.merged_into == merged_into;
    if item.external_ids.tvdb != Some(deleted) {
        return Ok(match merged_before {
            true => Dropped::Already,
            false => Dropped::NotHeld,
        });
    }
    if merged_before || (held.dropped.is_some() && !says_more(held.dropped_why, why, merged_into)) {
        return Ok(Dropped::Already);
    }
    repo::item::load_children(&state.db, &mut item).await?;

    let first = held.dropped.is_none();
    held = held.without_silence();
    held.dropped = held.dropped.or(Some(crate::db::now()));
    held.dropped_why = Some(why);
    held.merged_into = merged_into;
    // Kept once, with what says why: a series known as missing first, and
    // as merged since, keeps the numbering it was kept as then.
    if first {
        keep_last(state, &item, &held).await?;
    } else {
        repo::order::set_numbering(&state.db, media_id, &held).await?;
    }

    if let Some(into) = merged_into
        && into != deleted
        && takes_id(state, media_id, into).await
    {
        answer_to(state, media_id, deleted).await;
    }

    let detail = dropped_detail(&item.title, why, Some(deleted), merged_into);
    tell_dropped(state, &item.id, &item.title, &detail).await;
    Ok(Dropped::Kept)
}

/// Say a series' numbering is now kept as TheTVDB last gave it.
pub async fn tell_dropped(state: &AppState, media_id: &str, title: &str, detail: &str) {
    tracing::warn!(id = %media_id, %title, "{detail}");
    crate::api::audit::record_system(state, Action::ItemNumberingFallback, media_id, detail).await;
    crate::service::webhook::tell(
        state,
        crate::service::webhook::Fact::NumberingFallback,
        Some(media_id.to_string()),
        Some(detail.to_string()),
    );
}

/// Whether the work `media_id` may take TheTVDB id `into`: the catalogue
/// holds no other work under it, nor answers with another for it — a work
/// merged here from one that held it — and of the works TheTVDB merged into
/// it, this one's numbering was kept first ([`repo::order::merge_heir`]).
/// One the catalogue holds already is a duplicate here as it was at
/// TheTVDB, for a person to merge; meanwhile each keeps its own id.
pub(crate) async fn takes_id(state: &AppState, media_id: &str, into: i64) -> bool {
    let value = into.to_string();
    let weighed = async {
        if let Some(holder) =
            repo::item::find_id_by_external(&state.db, ExternalSource::TvdbSeries, &value).await?
        {
            return anyhow::Ok(holder == media_id);
        }
        if let Some(answering) =
            repo::alias::for_external(&state.db, ExternalSource::TvdbSeries, &value).await?
            && answering != media_id
        {
            return Ok(false);
        }
        Ok(repo::order::merge_heir(&state.db, into)
            .await?
            .is_none_or(|heir| heir == media_id))
    };
    match weighed.await {
        Ok(takes) => takes,
        Err(e) => {
            tracing::warn!(id = %media_id, error = %e, "could not tell who holds the series TheTVDB kept");
            false
        }
    }
}

/// File the TheTVDB id a work had as one it answers to, the way the ids of a
/// work merged here are: once it holds the id TheTVDB merged it into, Sonarr
/// asking by the one it holds the series under is answered with it. Filed
/// under a name of its own, which no undo of a merge here takes back.
async fn answer_to(state: &AppState, media_id: &str, had: i64) {
    let write = async {
        let mut tx = state.db.begin_write().await?;
        repo::alias::write(
            &state.db,
            &mut tx,
            &format!("{}:{had}", names::TVDB),
            media_id,
            &[(
                ExternalSource::TvdbSeries.as_str().to_string(),
                had.to_string(),
            )],
        )
        .await?;
        tx.commit().await?;
        anyhow::Ok(())
    };
    if let Err(e) = write.await {
        tracing::warn!(id = %media_id, tvdb_id = had, error = %e, "could not keep the TheTVDB id the series had");
    }
}

/// The TheTVDB id a series held under `tvdb_id` is asked about by: the one
/// TheTVDB merged it into, when it did — what TheTVDB knows the series by
/// now — unless the catalogue holds that one as another work; `tvdb_id`
/// itself otherwise. And whether it is the one TheTVDB merged the work
/// into, which the work holds whatever TMDB still says of the one it had.
pub async fn asked_by(state: &AppState, tvdb_id: i64) -> (i64, bool) {
    match repo::order::merged_into(&state.db, tvdb_id).await {
        Ok(Some((_, into))) if into == tvdb_id => (tvdb_id, true),
        Ok(Some((work, into))) if takes_id(state, &work, into).await => (into, true),
        Ok(_) => (tvdb_id, false),
        Err(e) => {
            tracing::warn!(tvdb_id, error = %e, "could not tell whether TheTVDB merged the series");
            (tvdb_id, false)
        }
    }
}

/// TheTVDB knows the series again: its numbering as TheTVDB last gave it is
/// forgotten, and the aired order is TheTVDB's own again.
pub async fn forget_last(state: &AppState, media_id: &str) {
    match repo::order::forget_last(&state.db, media_id).await {
        Ok(true) => state.caches.touched(media_id).await,
        Ok(false) => {}
        Err(e) => {
            tracing::warn!(id = %media_id, error = %e, "could not forget the numbering TheTVDB last gave");
        }
    }
}

// ─── what follows it ─────────────────────────────────────────────────────────

/// One episode of the source the series is followed by since TheTVDB
/// dropped it, as that source numbers and describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Fresh {
    /// Its season and number at its source.
    pub season: i32,
    pub number: i32,
    pub tvdb_id: Option<i64>,
    pub tmdb_id: Option<i64>,
    /// The episode the work holds for it, by its row, when one is known.
    pub episode_id: Option<String>,
    pub record: OrderRecord,
}

impl Fresh {
    /// One of the work's own episodes: for a series TheTVDB numbers again
    /// under the id it merged it into.
    pub fn of_episode(episode: &Episode) -> Self {
        Self {
            season: episode.season_number,
            number: episode.episode_number,
            tvdb_id: episode.tvdb_id,
            tmdb_id: episode.tmdb_id,
            episode_id: (!episode.id.is_empty()).then(|| episode.id.clone()),
            record: orders::record_of(episode),
        }
    }

    /// One of TVmaze's episodes, which names none the work holds.
    pub fn of_tvmaze(episode: &Episode) -> Self {
        Self {
            episode_id: None,
            tvdb_id: None,
            tmdb_id: None,
            ..Self::of_episode(episode)
        }
    }

    /// A place of TMDB's own numbering as it is kept: its record, and the
    /// episode the work holds for it where one is known to be that one.
    pub fn of_place(place: &PlacedEpisode) -> Option<Self> {
        Some(Self {
            season: place.season_number,
            number: place.episode_number,
            tvdb_id: place.tvdb_id,
            tmdb_id: place.tmdb_id,
            episode_id: place.episode_id.clone(),
            record: place.record.clone()?,
        })
    }

    fn day(&self) -> Option<NaiveDate> {
        day_of(self.record.aired.as_deref())
    }
}

/// The day a date names, from its first ten characters.
fn day_of(date: Option<&str>) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(date?.trim().get(..10)?, "%Y-%m-%d").ok()
}

/// How many days apart an episode an id names may be dated from the kept
/// one of that id and still be it: a day for a time zone, more for a
/// channel that showed it later or a source that dated it anew. A week is
/// the next episode of a weekly series, which an id misplaced by numbers
/// would name.
const ID_DAYS: i64 = 3;

/// Whether two days are more than `days` apart. Not when either is unknown.
fn apart_by(a: Option<NaiveDate>, b: Option<NaiveDate>, days: i64) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).num_days().abs() > days,
        _ => false,
    }
}

fn kept_day(place: &PlacedEpisode) -> Option<NaiveDate> {
    day_of(place.record.as_ref().and_then(|r| r.aired.as_deref()))
}

/// A title as two of its sources are compared by.
fn folded(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Which of `kept` each episode of `fresh` is, where one is: the same
/// TheTVDB id, or the same TMDB id, at the same numbers or aired within
/// [`ID_DAYS`] of it — an episode its source dated anew is still the one
/// kept, and is not placed after it a second time; else the one place left
/// that aired the day it did, when that day has one on each side, a special
/// with a special and a regular episode with a regular one. By index into
/// `fresh`, the index into `kept`.
fn pair(kept: &[PlacedEpisode], fresh: &[Fresh]) -> HashMap<usize, usize> {
    let mut paired: HashMap<usize, usize> = HashMap::new();
    let mut taken: HashSet<usize> = HashSet::new();
    let by_tvdb: HashMap<i64, usize> = kept
        .iter()
        .enumerate()
        .filter_map(|(at, place)| Some((place.tvdb_id?, at)))
        .collect();
    let by_tmdb: HashMap<i64, usize> = kept
        .iter()
        .enumerate()
        .filter_map(|(at, place)| Some((place.tmdb_id?, at)))
        .collect();

    for (at, one) in fresh.iter().enumerate() {
        let is_it = |k: &&usize| {
            let place = &kept[**k];
            (one.season, one.number) == (place.season_number, place.episode_number)
                || !apart_by(one.day(), kept_day(place), ID_DAYS)
        };
        let found = one
            .tvdb_id
            .and_then(|id| by_tvdb.get(&id))
            .filter(is_it)
            .or_else(|| one.tmdb_id.and_then(|id| by_tmdb.get(&id)).filter(is_it))
            .copied();
        if let Some(k) = found
            && taken.insert(k)
        {
            paired.insert(at, k);
        }
    }

    // By the day, where nothing else named them.
    let on = |special: bool, day: NaiveDate| (special, day);
    let mut kept_on: HashMap<(bool, NaiveDate), Vec<usize>> = HashMap::new();
    for (at, place) in kept
        .iter()
        .enumerate()
        .filter(|(at, _)| !taken.contains(at))
    {
        if let Some(day) = kept_day(place) {
            kept_on
                .entry(on(place.season_number == 0, day))
                .or_default()
                .push(at);
        }
    }
    let mut fresh_on: HashMap<(bool, NaiveDate), Vec<usize>> = HashMap::new();
    for (at, one) in fresh
        .iter()
        .enumerate()
        .filter(|(at, _)| !paired.contains_key(at))
    {
        if let Some(day) = one.day() {
            fresh_on
                .entry(on(one.season == 0, day))
                .or_default()
                .push(at);
        }
    }
    for (day, ones) in fresh_on {
        if let ([one], Some([k])) = (ones.as_slice(), kept_on.get(&day).map(Vec::as_slice))
            && taken.insert(*k)
        {
            paired.insert(*one, *k);
        }
    }
    paired
}

/// Which earlier place that followed the kept numbering an episode is: the
/// same TMDB id, else the same TheTVDB id, else the same day and title, else
/// the one place of its day.
fn placed_before(one: &Fresh, before: &[PlacedEpisode], used: &HashSet<usize>) -> Option<usize> {
    let free = |at: &usize| !used.contains(at);
    let index = |test: &dyn Fn(&PlacedEpisode) -> bool| {
        (0..before.len()).filter(free).find(|at| test(&before[*at]))
    };
    let day = one.day();
    one.tmdb_id
        .and_then(|id| index(&|place| place.tmdb_id == Some(id)))
        .or_else(|| {
            one.tvdb_id
                .and_then(|id| index(&|place| place.tvdb_id == Some(id)))
        })
        .or_else(|| {
            let title = folded(&one.record.title);
            (!title.is_empty()).then_some(()).and_then(|()| {
                index(&|place| {
                    kept_day(place) == day
                        && place
                            .record
                            .as_ref()
                            .is_some_and(|r| folded(&r.title) == title)
                })
            })
        })
        .or_else(|| {
            let day = day?;
            let mut same: Vec<usize> = (0..before.len())
                .filter(free)
                .filter(|at| kept_day(&before[*at]) == Some(day))
                .collect();
            (same.len() == 1).then(|| same.remove(0))
        })
}

/// The numbers taken so far: the last of each season, and the last
/// absolute one.
#[derive(Default)]
struct Taken {
    last: HashMap<i32, i32>,
    absolute: Option<i32>,
}

impl Taken {
    fn take(&mut self, place: &PlacedEpisode) {
        let last = self
            .last
            .entry(place.season_number)
            .or_insert(place.episode_number);
        *last = (*last).max(place.episode_number);
        if let Some(absolute) = place.absolute_number {
            self.absolute = Some(self.absolute.map_or(absolute, |held| held.max(absolute)));
        }
    }

    /// The next number free in `season`.
    fn next_in(&mut self, season: i32) -> i32 {
        let next = self
            .last
            .get(&season)
            .copied()
            .unwrap_or(0)
            .saturating_add(1);
        self.last.insert(season, next);
        next
    }

    fn next_absolute(&mut self) -> i32 {
        let next = self.absolute.unwrap_or(0).saturating_add(1);
        self.absolute = Some(next);
        next
    }

    /// The last regular season taken.
    fn last_season(&self) -> i32 {
        self.last
            .keys()
            .copied()
            .filter(|s| *s > 0)
            .max()
            .unwrap_or(0)
    }
}

/// The places that follow a series' kept numbering: each episode of `fresh`
/// that no kept place stands for and that aired after the last of them —
/// or is not dated yet, in a regular season, after every one paired — placed
/// after it, by the day it aired.
///
/// An episode of a source season that holds kept episodes goes on in the
/// kept season of the last of them; one of a source season that holds none
/// opens a season after the last kept one — or, where no episode of the
/// source could be told for a kept one at all, goes on in the kept season of
/// its number, when there is one; a special goes after the last kept
/// special. One placed before keeps its place (`before`, matched by its TMDB
/// or TheTVDB id, or its day and title): an episode does not move in Sonarr
/// once it is filed. Absolute numbers go on where the kept ones have them.
/// Each place carries the source's record of its episode as it is now, and
/// names the episode the work holds for it where one is known.
///
/// `since` is when TheTVDB dropped the series: the day after which an
/// episode is new when no kept episode is dated.
pub fn following(
    kept: &[PlacedEpisode],
    before: &[PlacedEpisode],
    fresh: &[Fresh],
    since: Option<NaiveDate>,
) -> Vec<PlacedEpisode> {
    let paired = pair(kept, fresh);
    let last_day = kept.iter().filter_map(kept_day).max().or(since);
    // The last episode paired in source order, among the regular ones: an
    // undated one after it is still to air.
    let last_paired = paired
        .keys()
        .map(|at| &fresh[*at])
        .filter(|one| one.season > 0)
        .map(|one| (one.season, one.number))
        .max();

    let mut new: Vec<usize> = (0..fresh.len())
        .filter(|at| !paired.contains_key(at))
        .filter(|at| {
            let one = &fresh[*at];
            match (one.day(), last_day) {
                (Some(day), Some(last)) => day > last,
                (Some(_), None) => true,
                (None, _) => {
                    one.season > 0 && last_paired.is_none_or(|last| (one.season, one.number) > last)
                }
            }
        })
        .collect();
    new.sort_by_key(|at| {
        let one = &fresh[*at];
        (one.day().unwrap_or(NaiveDate::MAX), one.season, one.number)
    });

    // Where each source season goes: on in the kept season of the last kept
    // episode it holds.
    let mut season_of: HashMap<i32, i32> = HashMap::new();
    for (at, k) in &paired {
        let source = fresh[*at].season;
        let place = &kept[*k];
        if source <= 0 || place.season_number <= 0 {
            continue;
        }
        let at_most = season_of.entry(source).or_insert(place.season_number);
        *at_most = (*at_most).max(place.season_number);
    }
    // Nothing told for a kept episode: the seasons of the same numbers are
    // the likeliest guess.
    if paired.is_empty() {
        for place in kept.iter().filter(|place| place.season_number > 0) {
            season_of.insert(place.season_number, place.season_number);
        }
    }

    // What is taken: the kept places, then those placed before that stand.
    let mut taken = Taken::default();
    for place in kept {
        taken.take(place);
    }
    let numbered_through = kept.iter().any(|place| place.absolute_number.is_some());

    let placed_as =
        |one: &Fresh, id: String, season: i32, number: i32, absolute: Option<i32>| PlacedEpisode {
            id,
            season_number: season,
            episode_number: number,
            absolute_number: absolute,
            episode_id: one.episode_id.clone(),
            tvdb_id: one.tvdb_id,
            tmdb_id: one.tmdb_id,
            record: Some(one.record.clone()),
        };

    let mut used: HashSet<usize> = HashSet::new();
    let mut next: Vec<PlacedEpisode> = Vec::new();
    let mut waiting: Vec<usize> = Vec::new();
    for at in new {
        let one = &fresh[at];
        let Some(was) = placed_before(one, before, &used) else {
            waiting.push(at);
            continue;
        };
        used.insert(was);
        let place = &before[was];
        if one.season > 0 && place.season_number > 0 {
            season_of.entry(one.season).or_insert(place.season_number);
        }
        taken.take(place);
        next.push(placed_as(
            one,
            place.id.clone(),
            place.season_number,
            place.episode_number,
            place.absolute_number,
        ));
    }

    for at in waiting {
        let one = &fresh[at];
        let season = if one.season <= 0 {
            0
        } else if let Some(season) = season_of.get(&one.season) {
            *season
        } else {
            let opened = taken.last_season().saturating_add(1);
            season_of.insert(one.season, opened);
            opened
        };
        let number = taken.next_in(season);
        let absolute = (numbered_through && season > 0).then(|| taken.next_absolute());
        next.push(placed_as(
            one,
            format!("{NEXT}{}", crate::db::new_id()),
            season,
            number,
            absolute,
        ));
    }

    next.sort_by_key(|place| (place.season_number, place.episode_number));
    next
}

/// Where the episodes that follow a series' kept numbering are read: TheTVDB
/// under the id it merged the series into, when it numbers the work again so;
/// else TMDB's own numbering as it is kept; else TVmaze's episodes, from this
/// refresh. None when none of them has any.
fn source_of(
    item: &MediaItem,
    all: &[EpisodeOrder],
    merged_into: Option<i64>,
    tvmaze: Option<&[Episode]>,
) -> Option<Vec<Fresh>> {
    let numbered_by_tvdb = orders::aired(all).is_some_and(|aired| aired.source == names::TVDB);
    if numbered_by_tvdb && merged_into.is_some() && merged_into == item.external_ids.tvdb {
        let own: Vec<Fresh> = item
            .episodes
            .iter()
            .filter(|episode| !episode.is_manual)
            .map(Fresh::of_episode)
            .collect();
        if !own.is_empty() {
            return Some(own);
        }
    }
    if let Some(tmdb) = orders::named(all, orders::TMDB_AIRED) {
        let places: Vec<Fresh> = tmdb.episodes.iter().filter_map(Fresh::of_place).collect();
        if !places.is_empty() {
            return Some(places);
        }
    }
    let tvmaze: Vec<Fresh> = tvmaze
        .unwrap_or_default()
        .iter()
        .map(Fresh::of_tvmaze)
        .collect();
    (!tvmaze.is_empty()).then_some(tvmaze)
}

/// After a refresh — or a sync — of a work whose numbering is kept as
/// TheTVDB last gave it: the episodes aired since, placed after it
/// ([`following`]), read from what the refresh kept or, for TVmaze's,
/// brought (`tvmaze`). Written only when they moved; best effort, as the
/// orders are: a failure is logged, and what followed stands.
pub async fn follow(state: &AppState, item: &MediaItem, tvmaze: Option<&[Episode]>) {
    if item.kind != MediaKind::Series || crate::service::is_previewing() {
        return;
    }
    let work = async {
        let held = repo::order::numbering(&state.db, &item.id).await?;
        let Some(dropped) = held.dropped.as_deref() else {
            return anyhow::Ok(false);
        };
        let all = repo::order::list(&state.db, &item.id).await?;
        let Some(last) = all.iter().find(|order| order.key == TVDB_LAST) else {
            return Ok(false);
        };
        let Some(fresh) = source_of(item, &all, held.merged_into, tvmaze) else {
            return Ok(false);
        };
        let (before, kept): (Vec<PlacedEpisode>, Vec<PlacedEpisode>) =
            last.episodes.iter().cloned().partition(is_next);
        let since = day_of(Some(dropped));
        let next = following(&kept, &before, &fresh, since);
        if next == before {
            return Ok(false);
        }
        let mut places = kept;
        places.extend(next);
        repo::order::write_last(&state.db, &item.id, &places, false).await?;
        Ok(true)
    };
    match work.await {
        Ok(true) => state.caches.touched(&item.id).await,
        Ok(false) => {}
        Err(e) => {
            tracing::warn!(id = %item.id, error = %e, "could not place the episodes aired since TheTVDB dropped the series");
        }
    }
}

// ─── TheTVDB's feed ──────────────────────────────────────────────────────────

/// What the last reading of TheTVDB's feed is kept under, beside the lists
/// brought in: when it began, which the next one reads from.
pub const FEED: &str = "tvdb-deleted-series";

/// How far back a first reading looks, and the most any reading does: the
/// series deleted before are found when TheTVDB answers that it has no such
/// series, at two refreshes in a row.
const LOOK_BACK: TimeDelta = TimeDelta::days(7);

/// How far before the last reading began the next one starts, so that what
/// TheTVDB filed late is read all the same: a series read twice is kept once.
const OVERLAP: TimeDelta = TimeDelta::hours(1);

/// Read TheTVDB's feed of updates for the series it deleted since the last
/// reading, and keep the numbering of each the catalogue holds as it is
/// served now ([`drop_held`]); each is then refreshed, which places the
/// episodes aired since and, for one TheTVDB merged into another, reads it
/// under the id TheTVDB kept. What the reading came to, for the task's run.
///
/// Nothing is read while TheTVDB is not configured, or with
/// `numbering.fallbackAfter` at `0`. A reading TheTVDB did not answer is an
/// error, and the next one starts from where this one would have.
pub async fn watch(state: &AppState) -> Result<String> {
    if !state.tvdb.is_enabled() {
        return Ok("TheTVDB is not configured: its feed was not read".into());
    }
    if !allowed(state) {
        return Ok(
            "numbering.fallbackAfter is 0: the series TheTVDB deletes are numbered as they were"
                .into(),
        );
    }
    let started = Utc::now();
    let since = match repo::import::get(&state.db, FEED).await? {
        Some(last) => crate::db::parse_rfc3339(&last.imported_at)
            .map_or(started - LOOK_BACK, |at| {
                (at - OVERLAP).max(started - LOOK_BACK)
            }),
        None => started - LOOK_BACK,
    };
    let deleted = state.tvdb.deleted_series(since.timestamp()).await?;

    let mut kept: Vec<String> = Vec::new();
    let mut already = 0;
    // Those whose numbering could not be kept: the reading is not counted
    // as made, and the next one reads them again.
    let mut failed: Vec<String> = Vec::new();
    for one in &deleted {
        // The work holding the id, or answering to it since it took the one
        // TheTVDB merged it into — `drop_held` tells the two apart.
        let found = repo::alias::work_for_external(
            &state.db,
            ExternalSource::TvdbSeries,
            &one.tvdb_id.to_string(),
        )
        .await;
        let id = match found {
            Ok(Some(id)) => id,
            Ok(None) => continue,
            Err(e) => {
                failed.push(format!("{}: {e:#}", one.tvdb_id));
                continue;
            }
        };
        let why = match one.merged_into {
            Some(_) => TvdbDropped::Merged,
            None => TvdbDropped::Deleted,
        };
        match drop_held(state, &id, one.tvdb_id, why, one.merged_into).await {
            Ok(Dropped::Kept) => match repo::item::get(&state.db, &id).await {
                Ok(Some(item)) => {
                    kept.push(match one.merged_into {
                        Some(into) => format!("“{}” (merged into {into})", item.title),
                        None => format!("“{}”", item.title),
                    });
                    if let Err(e) = crate::jobs::refresh::refresh_one(state, &item).await {
                        tracing::warn!(id = %id, error = format_args!("{e:#}"), "a series TheTVDB dropped could not be refreshed");
                    }
                }
                Ok(None) => {}
                Err(e) => failed.push(format!("{}: {e:#}", one.tvdb_id)),
            },
            Ok(Dropped::Already) => already += 1,
            Ok(Dropped::NotHeld) => {}
            Err(e) => {
                tracing::warn!(id = %id, error = format_args!("{e:#}"), "could not keep the numbering of a series TheTVDB dropped");
                failed.push(format!("{}: {e:#}", one.tvdb_id));
            }
        }
    }
    if !failed.is_empty() {
        anyhow::bail!(
            "TheTVDB deleted {} series since {}; the numbering of {} held here could not be kept, and \
             they are read again at the next reading: {}",
            deleted.len(),
            since.format("%Y-%m-%d %H:%M UTC"),
            failed.len(),
            failed.join("; ")
        );
    }

    repo::import::record(
        &state.db,
        FEED,
        i64::try_from(deleted.len()).unwrap_or(i64::MAX),
        &crate::db::to_rfc3339(started),
    )
    .await?;

    let mut said = format!(
        "TheTVDB deleted {} series since {}",
        deleted.len(),
        since.format("%Y-%m-%d %H:%M UTC")
    );
    if !kept.is_empty() {
        said.push_str(&format!(
            "; numbered as they were served: {}",
            kept.join(", ")
        ));
    }
    if already > 0 {
        said.push_str(&format!("; {already} held here numbered so already"));
    }
    if kept.is_empty() && already == 0 {
        said.push_str("; none held here");
    }
    Ok(said)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(title: &str, aired: Option<&str>) -> OrderRecord {
        OrderRecord {
            title: title.into(),
            overview: None,
            aired: aired.map(str::to_string),
            runtime: None,
            image: None,
            finale_type: None,
        }
    }

    /// A kept place: TheTVDB's episode `tvdb`, TMDB's `tmdb`, aired `aired`.
    fn kept(season: i32, number: i32, tvdb: i64, tmdb: Option<i64>, aired: &str) -> PlacedEpisode {
        PlacedEpisode {
            tmdb_id: tmdb,
            record: Some(record(&format!("TheTVDB {tvdb}"), Some(aired))),
            ..PlacedEpisode::of_tvdb(tvdb, season, number)
        }
    }

    /// One of TMDB's episodes, at its own numbers.
    fn tmdb(season: i32, number: i32, id: i64, aired: Option<&str>) -> Fresh {
        Fresh {
            season,
            number,
            tvdb_id: None,
            tmdb_id: Some(id),
            episode_id: None,
            record: record(&format!("TMDB {id}"), aired),
        }
    }

    fn at(places: &[PlacedEpisode]) -> Vec<(i32, i32, Option<i64>)> {
        places
            .iter()
            .map(|p| (p.season_number, p.episode_number, p.tmdb_id))
            .collect()
    }

    /// TheTVDB's three seasons of four, the last of them TMDB's second
    /// half of its second season: TMDB cuts the series in two.
    fn three_seasons() -> Vec<PlacedEpisode> {
        vec![
            kept(1, 1, 11, Some(101), "2020-01-01"),
            kept(1, 2, 12, Some(102), "2020-01-08"),
            kept(2, 1, 21, Some(201), "2021-01-01"),
            kept(2, 2, 22, Some(202), "2021-01-08"),
            kept(3, 1, 31, Some(203), "2022-01-01"),
            kept(3, 2, 32, None, "2022-01-08"),
        ]
    }

    /// An episode TMDB numbers in the season that holds TheTVDB's last goes
    /// on in TheTVDB's last season; one of a season TMDB opens since opens
    /// one after it; an episode TMDB lists that aired before TheTVDB's last
    /// is no new one, and TheTVDB's numbering never held it.
    #[test]
    fn the_episodes_aired_since_go_on_after_thetvdbs_last_by_the_day_they_aired() {
        let fresh = vec![
            tmdb(1, 1, 101, Some("2020-01-01")),
            tmdb(1, 2, 102, Some("2020-01-08")),
            // TMDB's own, aired while TheTVDB knew the series: not placed.
            tmdb(1, 3, 103, Some("2020-01-15")),
            tmdb(2, 1, 201, Some("2021-01-01")),
            tmdb(2, 2, 202, Some("2021-01-08")),
            tmdb(2, 3, 203, Some("2022-01-01")),
            // TheTVDB's last, which TMDB names by no id it carried: the day.
            tmdb(2, 4, 204, Some("2022-01-08")),
            // Since: on in TheTVDB's third season, then a fourth.
            tmdb(2, 5, 205, Some("2022-01-15")),
            tmdb(3, 1, 301, Some("2023-01-01")),
            tmdb(3, 2, 302, None),
            // A special since, after TheTVDB's specials — none here.
            tmdb(0, 1, 901, Some("2022-06-01")),
        ];
        let next = following(&three_seasons(), &[], &fresh, None);
        assert_eq!(
            at(&next),
            [
                (0, 1, Some(901)),
                (3, 3, Some(205)),
                (4, 1, Some(301)),
                (4, 2, Some(302)),
            ]
        );
        assert!(next.iter().all(is_next));
        assert_eq!(
            next[1].record.as_ref().map(|r| r.title.as_str()),
            Some("TMDB 205")
        );
    }

    /// TMDB's list as it numbers TheTVDB's [`three_seasons`], and `since`
    /// after it.
    fn tmdbs_list(since: &[Fresh]) -> Vec<Fresh> {
        let mut list = vec![
            tmdb(1, 1, 101, Some("2020-01-01")),
            tmdb(1, 2, 102, Some("2020-01-08")),
            tmdb(2, 1, 201, Some("2021-01-01")),
            tmdb(2, 2, 202, Some("2021-01-08")),
            tmdb(2, 3, 203, Some("2022-01-01")),
            tmdb(2, 4, 204, Some("2022-01-08")),
        ];
        list.extend_from_slice(since);
        list
    }

    /// Placed once, an episode keeps its place — its id, its numbers — at
    /// the next reading, whatever came before it since; one new is placed
    /// after it. Absolute numbers go on where TheTVDB's had them.
    #[test]
    fn an_episode_placed_once_stays_where_it_was_placed() {
        let mut kept_places = three_seasons();
        for (at, place) in kept_places.iter_mut().enumerate() {
            place.absolute_number = Some(i32::try_from(at).unwrap() + 1);
        }
        let first = following(
            &kept_places,
            &[],
            &tmdbs_list(&[tmdb(2, 5, 205, Some("2022-01-15"))]),
            None,
        );
        assert_eq!(at(&first), [(3, 3, Some(205))]);
        assert_eq!(first[0].absolute_number, Some(7));

        // TMDB lists another between, and one after.
        let fresh = tmdbs_list(&[
            tmdb(2, 5, 206, Some("2022-01-12")),
            tmdb(2, 6, 205, Some("2022-01-15")),
            tmdb(2, 7, 207, Some("2022-01-22")),
        ]);
        let again = following(&kept_places, &first, &fresh, None);
        assert_eq!(
            at(&again),
            [(3, 3, Some(205)), (3, 4, Some(206)), (3, 5, Some(207))]
        );
        assert_eq!(again[0].id, first[0].id, "the same place");
        assert_eq!(
            again.iter().map(|p| p.absolute_number).collect::<Vec<_>>(),
            [Some(7), Some(8), Some(9)]
        );
    }

    /// TVmaze names no episode by an id: its are told from TheTVDB's by the
    /// day they aired, and placed after them; read again, each keeps its
    /// place by its day and title.
    #[test]
    fn tvmazes_episodes_are_told_from_thetvdbs_by_their_day() {
        let tvmaze = |season: i32, number: i32, title: &str, aired: &str| Fresh {
            season,
            number,
            tvdb_id: None,
            tmdb_id: None,
            episode_id: None,
            record: record(title, Some(aired)),
        };
        let kept_places = vec![
            kept(1, 1, 11, None, "2020-01-01"),
            kept(1, 2, 12, None, "2020-01-08"),
        ];
        let fresh = [
            tvmaze(1, 1, "Pilot", "2020-01-01"),
            tvmaze(1, 2, "Second", "2020-01-08"),
            tvmaze(1, 3, "Third", "2020-01-15"),
            tvmaze(2, 1, "Return", "2021-03-01"),
        ];
        let next = following(&kept_places, &[], &fresh, None);
        assert_eq!(at(&next), [(1, 3, None), (2, 1, None)]);

        let again = following(&kept_places, &next, &fresh, None);
        assert_eq!(
            again.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            next.iter().map(|p| p.id.as_str()).collect::<Vec<_>>()
        );
    }

    /// No kept episode dated: what aired after TheTVDB dropped the series
    /// is new, and nothing before it; and with no episode of the source told
    /// for a kept one, a season goes on in the kept season of its number.
    #[test]
    fn with_nothing_dated_kept_what_aired_after_the_drop_is_new() {
        let mut undated = kept(1, 1, 11, None, "2020-01-01");
        undated.record = Some(record("Pilot", None));
        let fresh = [
            tmdb(1, 2, 102, Some("2024-05-01")),
            tmdb(1, 3, 103, Some("2024-07-01")),
            tmdb(2, 1, 201, Some("2025-01-01")),
        ];
        let next = following(&[undated], &[], &fresh, NaiveDate::from_ymd_opt(2024, 6, 1));
        assert_eq!(at(&next), [(1, 2, Some(103)), (2, 1, Some(201))]);
    }

    /// An episode its source dated anew is still the one kept by its id: a
    /// few days off, or weeks off at the very numbers kept. Neither is
    /// placed after the kept numbering a second time; one a week off at
    /// other numbers is another.
    #[test]
    fn an_episode_its_source_dated_anew_is_not_placed_twice() {
        let kept_places = vec![
            kept(1, 1, 11, Some(101), "2022-01-01"),
            kept(1, 2, 12, Some(102), "2022-01-08"),
            kept(1, 3, 13, Some(103), "2022-01-15"),
        ];
        let fresh = [
            tmdb(1, 1, 101, Some("2022-01-01")),
            // Weeks off, at the same numbers.
            tmdb(1, 2, 102, Some("2022-02-20")),
            // Three days off, at other numbers.
            tmdb(2, 1, 103, Some("2022-01-18")),
            // Since.
            tmdb(2, 2, 104, Some("2022-01-22")),
        ];
        let next = following(&kept_places, &[], &fresh, None);
        assert_eq!(at(&next), [(1, 4, Some(104))]);

        // A week off at other numbers: another episode — of a season of
        // TMDB's that holds no kept one, so opening one — placed after.
        let week_off = [
            tmdb(1, 1, 101, Some("2022-01-01")),
            tmdb(1, 2, 102, Some("2022-01-08")),
            tmdb(2, 1, 103, Some("2022-01-22")),
        ];
        let next = following(&kept_places, &[], &week_off, None);
        assert_eq!(at(&next), [(2, 1, Some(103))]);
    }

    /// Numbers read off a source never overflow: the last of a season at
    /// the most there is stays there.
    #[test]
    fn numbers_at_their_most_stay_there() {
        let mut most = kept(1, i32::MAX, 11, Some(101), "2022-01-01");
        most.absolute_number = Some(i32::MAX);
        let fresh = [
            tmdb(1, 1, 101, Some("2022-01-01")),
            tmdb(1, 2, 102, Some("2022-01-08")),
        ];
        let next = following(&[most], &[], &fresh, None);
        assert_eq!(
            next.iter()
                .map(|p| (p.season_number, p.episode_number, p.absolute_number))
                .collect::<Vec<_>>(),
            [(1, i32::MAX, Some(i32::MAX))]
        );
    }
}
