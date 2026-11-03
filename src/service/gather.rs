//! Asking every provider, and folding the answers into one.
//!
//! Resolution — deciding *which* work a client means — lives in
//! [`super::series`] and [`super::movie`]. This is what happens once that is
//! settled: each enabled provider is asked, concurrently, and what they return
//! is merged by [`crate::merge`] and stored as a single entity with every raw
//! answer kept alongside.
//!
//! A provider that fails is logged and skipped. One source being down should
//! cost detail, not the whole answer — and not what it said last time: a
//! failure is not an answer, so what it gave the stored work is handed back
//! to the merge in its place, the episode list included when it is the one
//! that numbers it, and the work is written as refreshed in part, to be
//! tried again sooner (see [`store`]).
//!
//! Until TheTVDB has not answered for a series long enough: then TMDB
//! numbers it in TheTVDB's place, reversibly (see [`Numbering`]). TheTVDB
//! answering that it has no such series is no failure, but stands for one
//! all the same: what it gave before stands — and once it has said so long
//! enough, its numbering is kept as it was served (`service::numbering`).

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use chrono::{TimeDelta, Utc};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::{
    db::{repo, repo::audit::Action},
    domain::{Episode, ExternalIds, ExternalSource, MediaItem, MediaKind, OrderKind},
    merge::{self, Contribution, provenance::Provenance},
    providers::{names, tmdb::map as tmdb_map},
    service::{anime, numbering, orders},
    state::AppState,
    wire,
};

/// The most seasons one series fetch will ask a provider about.
///
/// Each is its own HTTP call. Nothing real comes close — the longest-running
/// television on record is under a hundred — and the number is read off an
/// answer rather than known in advance.
pub(crate) const MAX_SEASONS: usize = 200;

/// Providers that add to a work and cannot describe one alone: artwork,
/// broadcast times, the anime sites' scores and titles.
///
/// When nothing else answered, what these returned is not stored. There is no
/// work to attach it to — only a name and some pictures filed under an id, or
/// a series with no episodes that would reach Sonarr as one.
const SUPPLEMENTS: &[&str] = &[
    names::FANART,
    names::TVMAZE,
    names::ANILIST,
    names::MAL,
    names::FANKAI_WIKI,
];

/// What one provider returned: its raw body, and the canonical form of it.
struct Answer {
    provider: &'static str,
    payload: Value,
    item: MediaItem,
    /// The seasons whose episodes it could not give this time: their
    /// request failed. What is stored of them stands; see
    /// [`fill_unanswered_seasons`].
    unanswered_seasons: Vec<i32>,
}

impl Answer {
    fn new(provider: &'static str, payload: Value, item: MediaItem) -> Self {
        Self {
            provider,
            payload,
            item,
            unanswered_seasons: Vec::new(),
        }
    }
}

/// A provider that was asked and did not answer, and why.
#[derive(Clone)]
struct Failure {
    provider: &'static str,
    error: String,
}

/// What asking one provider came to.
enum Asked {
    /// It answered.
    Answered(Box<Answer>),
    /// It was not asked — switched off, or no id to ask by — or it knows
    /// nothing of the work, which is an answer too.
    Nothing,
    /// It was asked by an id of its own, and answered that it has no such
    /// work: TheTVDB, for a series it deleted. Nothing of the work either;
    /// for one held under that id, what it said last time stands, and how
    /// long it has said so is counted ([`Numbering`]).
    Missing(&'static str),
    /// It was asked and said nothing: an error, a timeout, a rate limit, a
    /// queue that declined. Not a word about the work, so what it said last
    /// time stands.
    Failed(Failure),
}

impl Asked {
    fn answered(answer: Answer) -> Self {
        Self::Answered(Box::new(answer))
    }

    fn failed(provider: &'static str, error: &anyhow::Error) -> Self {
        Self::Failed(Failure {
            provider,
            error: crate::providers::clip(&format!("{error:#}"), crate::providers::MESSAGE_CHARS),
        })
    }

    fn answer(self) -> Option<Answer> {
        match self {
            Self::Answered(answer) => Some(*answer),
            Self::Nothing | Self::Missing(_) | Self::Failed(_) => None,
        }
    }
}

/// Everything the providers asked came to.
#[derive(Default)]
struct Gathered {
    answers: Vec<Answer>,
    failed: Vec<Failure>,
    /// Those that answered they have no such work.
    missing: Vec<&'static str>,
    /// The TheTVDB id TheTVDB was asked by, for a series.
    asked_tvdb: Option<i64>,
    /// Whether that is the id TheTVDB merged the series into, which the work
    /// takes once TheTVDB answers for it.
    merged: bool,
}

impl Gathered {
    fn add(&mut self, asked: Asked) {
        match asked {
            Asked::Answered(answer) => self.answers.push(*answer),
            Asked::Nothing => {}
            Asked::Missing(provider) => self.missing.push(provider),
            Asked::Failed(failure) => self.failed.push(failure),
        }
    }
}

/// The sources whose word on the adult flag a work may have from nobody
/// else: see `merge::fold`.
const ADULT_AUTHORITIES: &[&str] = &[names::ANILIST, names::MAL];

/// Fetch a series from everything that can address it, and store the result.
///
/// `tmdb_id` and `tvdb_id` are what resolution worked out; either may be absent.
/// No language: each provider now reads the one setting that says which, and
/// Skyhook speaks only English whatever anyone asks for.
pub async fn series(
    state: &AppState,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
) -> Result<Option<MediaItem>> {
    // A series TheTVDB merged into another is asked about by the id TheTVDB
    // kept: the work takes it, and answers to the one it had besides.
    let (tvdb_id, merged) = match tvdb_id {
        Some(id) => {
            let (asked, merged) = numbering::asked_by(state, id).await;
            (Some(asked), merged)
        }
        None => (None, false),
    };

    // The fan-out on the heap: every provider's state machine at once, which
    // inline would sit in the future of whoever awaits this — a client's
    // handler, the sweep — on a thread with two megabytes of stack in a
    // debug build, which the Plex match path overflowed. Boxed here, it is
    // boxed for every caller.
    let (from_tmdb, from_tvdb, from_skyhook, from_fanart, from_tvmaze) = Box::pin(async {
        tokio::join!(
            series_from_tmdb(state, tmdb_id),
            series_from_tvdb(state, tvdb_id),
            series_from_skyhook(state, tvdb_id),
            series_from_fanart(state, tvdb_id),
            series_from_tvmaze(state, tvdb_id, tmdb_id),
        )
    })
    .await;

    // What the series' other orders are read from: the episode groups TMDB
    // lists with its answer and its own seasons, read before the merge
    // numbers the series as TheTVDB does, and TheTVDB, when it answered.
    let (tmdb_groups, tmdb_aired) = match &from_tmdb {
        Asked::Answered(answer) => (
            tmdb_map::episode_groups(&answer.payload),
            Some(orders::TmdbAired::of(
                &answer.item.episodes,
                &answer.unanswered_seasons,
            )),
        ),
        _ => (None, None),
    };
    let tvdb_answered = matches!(from_tvdb, Asked::Answered(_));
    // TVmaze's episodes, which the merge holds out of the list: what follows
    // the numbering TheTVDB last gave a series is read from them where
    // nothing else lists any.
    let tvmaze_episodes = tvmaze_episodes(&from_tvmaze);

    let mut gathered = Gathered {
        asked_tvdb: tvdb_id,
        merged,
        ..Gathered::default()
    };
    for asked in [from_tmdb, from_tvdb, from_skyhook, from_fanart, from_tvmaze] {
        gathered.add(asked);
    }

    // The anime sites second: which of their entries to ask about comes from
    // the identifier list, or failing that from the ids Skyhook just returned.
    if anime::enabled(state) && describes_a_work(&gathered.answers) {
        let mut known = ExternalIds::default();
        for answer in &gathered.answers {
            known.mal.extend(&answer.item.external_ids.mal);
            known.anilist.extend(&answer.item.external_ids.anilist);
        }

        let pinned = pinned_ids(state, MediaKind::Series, tmdb_id, tvdb_id).await;
        let mapped = match tvdb_id {
            Some(tvdb_id) => Some(anime::for_series(state, tvdb_id, &known).await),
            None => None,
        };
        let chosen = anime::choose(mapped, pinned.as_ref(), &known);
        if !chosen.is_empty() {
            for asked in from_anime_sites(state, chosen, MediaKind::Series).await {
                gathered.add(asked);
            }
        }
    }

    let stored = store(state, gathered).await?;
    // The other orders TheTVDB numbers it in, and TMDB's groups, kept beside
    // the aired one — not by a refresh that is only previewed — and what
    // aired since TheTVDB last numbered a series it no longer knows.
    if let Some(item) = stored.as_ref().filter(|_| !super::is_previewing()) {
        orders::gather(
            state,
            item,
            orders::Read {
                tvdb: tvdb_answered,
                tmdb_groups,
                tmdb_aired,
            },
        )
        .await;
        numbering::follow(state, item, tvmaze_episodes.as_deref()).await;
    }
    Ok(stored)
}

/// The episodes TVmaze answered with, as it numbers them.
fn tvmaze_episodes(asked: &Asked) -> Option<Vec<Episode>> {
    match asked {
        Asked::Answered(answer) if answer.provider == names::TVMAZE => {
            Some(answer.item.episodes.clone())
        }
        _ => None,
    }
}

/// Fetch a movie from everything that can address it, and store the result.
pub async fn movie(
    state: &AppState,
    tmdb_id: Option<i64>,
    imdb_id: Option<&str>,
) -> Result<Option<MediaItem>> {
    // On the heap, as a series' fan-out is.
    let (from_tmdb, from_radarr, from_fanart) = Box::pin(async {
        tokio::join!(
            movie_from_tmdb(state, tmdb_id),
            movie_from_radarr(state, tmdb_id, imdb_id),
            movie_from_fanart(state, tmdb_id, imdb_id),
        )
    })
    .await;

    let mut gathered = Gathered::default();
    for asked in [from_tmdb, from_radarr, from_fanart] {
        gathered.add(asked);
    }

    let pinned = if anime::enabled(state) && describes_a_work(&gathered.answers) {
        pinned_ids(state, MediaKind::Movie, tmdb_id, None).await
    } else {
        None
    };
    if anime::enabled(state)
        && describes_a_work(&gathered.answers)
        && (tmdb_id.is_some() || pinned.is_some())
    {
        let mapped = match tmdb_id {
            Some(tmdb_id) => Some(anime::for_movie(state, tmdb_id).await),
            None => None,
        };
        let chosen = anime::choose(mapped, pinned.as_ref(), &ExternalIds::default());

        for mut asked in from_anime_sites(state, chosen, MediaKind::Movie).await {
            // A film keeps no AniList or MyAnimeList id. TheTVDB files films
            // under the series they belong to, and Skyhook lists them with
            // its entries, so a series already claims most of them — and a
            // work is matched to the stored one by any id it shares, whatever
            // its kind. The film would be written over the series.
            if let Asked::Answered(answer) = &mut asked {
                answer.item.external_ids = ExternalIds::default();
            }
            gathered.add(asked);
        }
    }

    store(state, gathered).await
}

/// Fetch a Fan-Kai production from Fankai, and store it.
///
/// Fankai alone: it is the only source that lists a recut, and the ids the
/// other providers would need are absent by design — TheTVDB's or TMDB's would
/// name the anime it was cut from, and merge the two into one work. A failure
/// is the caller's to see: with one source there is nothing to go on without.
pub async fn fankai_series(state: &AppState, fankai_id: i64) -> Result<Option<MediaItem>> {
    if !state.flag("fankai.enabled", false) {
        return Ok(None);
    }

    let Some((raw, item)) = state.fankai.series(fankai_id).await? else {
        return Ok(None);
    };

    let wiki = if state.flag("fankai.wiki", false) {
        fankai_from_wiki(state, &item).await
    } else {
        Asked::Nothing
    };

    let mut gathered = Gathered::default();
    gathered.add(Asked::answered(Answer::new(names::FANKAI, raw, item)));
    gathered.add(wiki);

    store(state, gathered).await
}

/// What the Fankai wiki says a production was cut from, and which Fan-Kai
/// follows it, as the production's relations.
///
/// A supplement: its answer carries those and nothing else. The original's
/// title, year and cover come from AniList when that source is on, and from
/// the wiki's own links otherwise; a sequel is only named when Fankai lists it.
///
/// The wiki is anybody's to edit, and says nothing of an entry's audience:
/// an original only it names is taken for one for adults — kept from the
/// readers who may not see those — unless AniList, or the work the catalogue
/// holds under that entry, says otherwise.
async fn fankai_from_wiki(state: &AppState, production: &MediaItem) -> Asked {
    use crate::providers::fankai::fold;

    // The wiki keeps a page per cut; whoever made this one tells them apart.
    let kaieurs: Vec<&str> = production
        .credits
        .iter()
        .filter(|c| {
            c.character_name
                .as_deref()
                .is_some_and(|r| fold(r) == "kaieur")
        })
        .map(|c| c.person_name.as_str())
        .collect();

    let (raw, page) = match state.fankai_wiki.page(&production.title, &kaieurs).await {
        Ok(Some(found)) => found,
        Ok(None) => {
            tracing::debug!(title = %production.title, "the Fankai wiki has no page for this production");
            return Asked::Nothing;
        }
        Err(e) => {
            tracing::warn!(
                title = %production.title,
                error = format_args!("{e:#}"),
                "the Fankai wiki could not be asked"
            );
            return Asked::failed(names::FANKAI_WIKI, &e);
        }
    };

    let mut relations = Vec::new();

    for original in &page.originals {
        let from_anilist = match original.anilist {
            Some(id) if state.flag("anilist.enabled", false) => {
                match state.anilist.entry(id, "ORIGINAL").await {
                    Ok(found) => found,
                    Err(e) => {
                        tracing::warn!(
                            anilist_id = id,
                            error = format_args!("{e:#}"),
                            "AniList could not describe a Fan-Kai's original; using the wiki's"
                        );
                        None
                    }
                }
            }
            _ => None,
        };

        let relation = match from_anilist {
            Some(mut relation) => {
                relation.mal_id = relation.mal_id.or(original.mal);
                Some(relation)
            }
            None => match original.relation() {
                Some(mut relation) => {
                    relation.is_adult = held_adult(state, original).await.unwrap_or(true);
                    Some(relation)
                }
                None => None,
            },
        };
        relations.extend(relation);
    }

    for sequel in &page.sequels {
        let name = crate::providers::fankai_wiki::base_name(sequel);
        match state.fankai.relation(name, "SEQUEL").await {
            Ok(Some(relation)) if Some(relation.external_id) != production.external_ids.fankai => {
                relations.push(relation);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(
                sequel = %sequel,
                error = format_args!("{e:#}"),
                "the Fan-Kai that follows could not be looked up"
            ),
        }
    }

    // The production's page on the wiki: the one public page about it whose
    // address is known. Fankai's website numbers its productions its own way.
    let homepage = state.fankai_wiki.page_url(&page.title);

    if relations.is_empty() && homepage.is_none() {
        return Asked::Nothing;
    }
    for (index, relation) in relations.iter_mut().enumerate() {
        relation.sort_order = i32::try_from(index).unwrap_or(i32::MAX);
    }

    let mut item = MediaItem::empty(MediaKind::Series);
    item.relations = relations;
    item.homepage = homepage;

    Asked::answered(Answer::new(names::FANKAI_WIKI, raw, item))
}

/// Whether the work the catalogue holds under an original's AniList or
/// MyAnimeList entry is for adults; `None` when it holds none, or cannot say.
async fn held_adult(
    state: &AppState,
    original: &crate::providers::fankai_wiki::Original,
) -> Option<bool> {
    let by = [
        (ExternalSource::AniList, original.anilist),
        (ExternalSource::Mal, original.mal),
    ];
    for (source, id) in by {
        let Some(id) = id else { continue };
        if let Ok(Some(work)) =
            repo::item::find_id_by_external(&state.db, source, &id.to_string()).await
            && let Ok(Some(held)) = repo::item::get(&state.db, &work).await
        {
            return Some(held.is_adult);
        }
    }
    None
}

/// Whether anything that can stand for a work on its own answered.
fn describes_a_work(answers: &[Answer]) -> bool {
    answers.iter().any(|a| !SUPPLEMENTS.contains(&a.provider))
}

/// AniList and MyAnimeList, asked at the same time about the entry `chosen`.
async fn from_anime_sites(state: &AppState, chosen: anime::Chosen, kind: MediaKind) -> Vec<Asked> {
    if chosen.is_empty() {
        return Vec::new();
    }

    let (from_anilist, from_mal) = tokio::join!(
        from_anilist(state, chosen.anilist, kind),
        from_mal(state, chosen.mal, kind),
    );

    vec![from_anilist, from_mal]
}

async fn from_anilist(state: &AppState, id: Option<i64>, kind: MediaKind) -> Asked {
    let Some(id) = id else { return Asked::Nothing };
    if !state.flag("anilist.enabled", false) {
        return Asked::Nothing;
    }

    match state.anilist.media(id, kind).await {
        Ok(Some((raw, item))) => Asked::answered(Answer::new(names::ANILIST, raw, item)),
        Ok(None) => Asked::Nothing,
        Err(e) => {
            tracing::warn!(
                anilist_id = id,
                error = format_args!("{e:#}"),
                "AniList lookup failed"
            );
            Asked::failed(names::ANILIST, &e)
        }
    }
}

async fn from_mal(state: &AppState, id: Option<i64>, kind: MediaKind) -> Asked {
    let Some(id) = id else { return Asked::Nothing };
    if !state.flag("mal.enabled", false) {
        return Asked::Nothing;
    }

    match state
        .mal
        .anime(id, kind, state.flag("mal.fallback", true))
        .await
    {
        Ok(Some((raw, item))) => Asked::answered(Answer::new(names::MAL, raw, item)),
        Ok(None) => Asked::Nothing,
        Err(e) => {
            tracing::warn!(
                mal_id = id,
                error = format_args!("{e:#}"),
                "MyAnimeList lookup failed"
            );
            Asked::failed(names::MAL, &e)
        }
    }
}

/// What each provider answered, as the text it is kept as, the document
/// itself let go of: the merge reads the works made of the answers, not the
/// answers. Parsed, a long series' answers weigh well over twice their text —
/// near ninety megabytes for a soap's ten thousand episodes, some thirty as
/// text — and a parsed copy of them was held through the merge and the write
/// beside the originals.
fn snapshots_of(answers: &mut [Answer]) -> Vec<(String, String)> {
    answers
        .iter_mut()
        .map(|answer| {
            let text = std::mem::take(&mut answer.payload).to_string();
            (answer.provider.to_string(), text)
        })
        .collect()
}

/// Merge what came back and write it.
///
/// When a provider did not answer, or not in full, what it said last time is
/// what it says now, as far as the stored work and its provenance remember:
/// its values, pictures, translations and ratings are handed back to the
/// merge, weighed against the others by the usual priority, as a sync hands
/// back those it does not ask; a season it could not give keeps the episodes
/// stored for it; the list keeps its numbering when the source numbering it
/// failed; and the adult flag stands when the source that says so failed.
/// The work is then written as refreshed in part: the failure is recorded on
/// it, and it is tried again sooner than its interval says.
///
/// TheTVDB answering that it has no series by the id a work is held under
/// stands for it failing — what it gave before stands, its numbering
/// included, and the work keeps that id — and is said so until its
/// numbering is kept as TheTVDB last gave it (see [`Numbering`]): from then
/// on it is the way of things, and no failure.
async fn store(state: &AppState, gathered: Gathered) -> Result<Option<MediaItem>> {
    let Gathered {
        mut answers,
        failed,
        missing: unknown,
        asked_tvdb,
        merged: rekeyed,
    } = gathered;
    // The id TheTVDB merged the series into: the work takes it, whatever
    // TMDB still says of the one it had — and keeps it while TheTVDB does
    // not answer, for TMDB's word would hand it back the one it had.
    let taken_tvdb = asked_tvdb.filter(|_| rekeyed);

    // The work held under the TheTVDB id TheTVDB has no series by, and that
    // id: what TheTVDB gave it before stands, however few others answered.
    let gone = match asked_tvdb {
        Some(tvdb) if unknown.contains(&names::TVDB) => {
            match repo::item::find_id_by_external(
                &state.db,
                ExternalSource::TvdbSeries,
                &tvdb.to_string(),
            )
            .await
            {
                Ok(held) => held.map(|id| (id, tvdb)),
                Err(e) => {
                    tracing::warn!(tvdb_id = tvdb, error = %e, "could not look up the series TheTVDB has no record of");
                    None
                }
            }
        }
        _ => None,
    };

    if answers.is_empty() && gone.is_none() {
        return Ok(None);
    }

    let providers: Vec<&str> = answers.iter().map(|a| a.provider).collect();
    tracing::debug!(?providers, "merging provider answers");

    if !describes_a_work(&answers) && gone.is_none() {
        tracing::warn!(
            ?providers,
            "only supplementary sources answered; not storing"
        );
        return Ok(None);
    }

    // The episode groups TMDB lists, read before its answer is let go of:
    // numbering by the group of the original air date wants them.
    let tmdb_groups = answers
        .iter()
        .find(|a| a.provider == names::TMDB)
        .and_then(|a| tmdb_map::episode_groups(&a.payload));
    // AniList's answer asked without its characters says nothing of them
    // (`providers::anilist::asked_characters`): it is given those it gave
    // before in their place, once the stored work is read.
    let characters_unasked = answers.iter().any(|a| {
        a.provider == names::ANILIST && !crate::providers::anilist::asked_characters(&a.payload)
    });
    let snapshots = snapshots_of(&mut answers);

    // Who numbers a series this time: TheTVDB, unless it has not answered
    // for it long enough that TMDB does in its place, or it no longer knows
    // the series.
    let mut numbering = numbering_of(state, &answers, &failed, gone.as_ref(), rekeyed).await;

    // TheTVDB having no such series stands for it failing: what it gave
    // before is handed back.
    let gone_failure = gone.as_ref().map(|(_, tvdb)| Failure {
        provider: names::TVDB,
        error: format!("TheTVDB has no series {tvdb} (404)"),
    });
    let handed: Vec<Failure> = failed.iter().cloned().chain(gone_failure.clone()).collect();

    let unanswered = answers.iter().any(|a| !a.unanswered_seasons.is_empty());
    let before = match unanswered || !handed.is_empty() || numbering.by_tmdb || characters_unasked {
        false => None,
        true => held_before(state, &answers, gone.as_ref()).await,
    };

    // The numbering of a series TheTVDB no longer knows is kept as it was
    // served — the work as it stood, by which Sonarr's files are matched —
    // before anything of this refresh is written, in one write with what
    // says why, and said so at once. Not kept, the series is numbered as it
    // was, and kept at the next refresh.
    if numbering.change == Some(NumberingChange::Dropped) && !super::is_previewing() {
        let kept = match &before {
            Some((held, _)) => numbering::keep_last(state, held, &numbering.kept).await,
            None => Err(anyhow::anyhow!("the work as held could not be read")),
        };
        match (kept, &before) {
            (Ok(()), Some((held, _))) => {
                let detail = numbering::dropped_detail(
                    &held.title,
                    numbering::TvdbDropped::Missing,
                    gone.as_ref().map(|(_, tvdb)| *tvdb),
                    None,
                );
                numbering::tell_dropped(state, &held.id, &held.title, &detail).await;
            }
            (Err(e), _) => {
                tracing::warn!(
                    id = numbering.work.as_deref().unwrap_or_default(),
                    error = format_args!("{e:#}"),
                    "could not keep the numbering of a series TheTVDB no longer knows; it is numbered as it was"
                );
                numbering.undrop();
            }
            (Ok(()), None) => {}
        }
    }

    // Said while it is counted; once the numbering is kept as TheTVDB last
    // gave it, it is the way of things.
    let noted: Vec<Failure> = failed
        .into_iter()
        .chain(gone_failure.filter(|_| numbering.kept.dropped.is_none()))
        .collect();
    let failed = handed;

    // Everything that did not answer in full: what is kept from before, and
    // what the log says.
    let missing = failure_note(&noted, &answers, |_| true);
    // What the work is said to have failed by — its refresh error, and the
    // notification of it — counts the sources it is made from. A supplement
    // that did not answer leaves it refreshed, with what it gave before.
    let failure = failure_note(&noted, &answers, |provider| {
        !SUPPLEMENTS.contains(&provider)
    });

    if let Some((stored, _)) = &before {
        for answer in &mut answers {
            fill_unanswered_seasons(answer, stored);
            if characters_unasked && answer.provider == names::ANILIST {
                answer.item.characters =
                    merge::provenance::characters_given(stored, names::ANILIST);
            }
        }
    }

    // TheTVDB's list, when it gave one TMDB numbers the series over: the ids
    // its episodes carry are still the episodes'.
    let mut tvdb_list: Vec<Episode> = Vec::new();
    let stored_episodes: &[Episode] = before.as_ref().map_or(&[], |(stored, _)| &stored.episodes);
    if numbering.by_tmdb {
        number_by_air_date_group(state, &mut answers, tmdb_groups, stored_episodes).await;
        for answer in &mut answers {
            if merge::TVDB_NUMBERED.contains(&answer.provider) {
                tvdb_list.append(&mut answer.item.episodes);
                answer.item.seasons.clear();
            }
        }
    }

    let mut contributions: Vec<Contribution> = answers
        .into_iter()
        .map(|a| Contribution {
            provider: a.provider.to_string(),
            item: a.item,
        })
        .collect();

    if let Some((stored, provenance)) = &before {
        hand_back(
            &mut contributions,
            &failed,
            stored,
            provenance,
            numbering.by_tmdb,
        );
    }

    // The order as it stands now, read once: who gave what is said by the
    // same order the merge folds by, whatever is changed meanwhile.
    let priority = state.provider_priority();
    let views = views_of(&priority, &contributions);

    let Some(mut merged) = merge::combine(contributions, &priority) else {
        return Ok(None);
    };
    if numbering.by_tmdb {
        let held: &[Episode] = match &tvdb_list[..] {
            [] => stored_episodes,
            fresh => fresh,
        };
        keep_tvdb_ids(&mut merged.episodes, held, stored_episodes);
    }
    // The id TheTVDB has no series by is still the one Sonarr holds the
    // series under, and the one it merged the series into is the work's
    // from now on: whatever TMDB says of either.
    if let Some(tvdb) = gone.as_ref().map(|(_, tvdb)| *tvdb).or(taken_tvdb) {
        merged.external_ids.tvdb = Some(tvdb);
    }
    let provenance = merge::provenance::attribute(&merged, &views);

    if let Some((stored, _)) = &before {
        keep_adult(&mut merged, stored, &failed);
    }

    // Answers from supplements alone were refused above; this is the provider
    // of record that answered with a blank title. Storing it would put a
    // nameless row in the catalogue and hand the client an entry it cannot
    // display.
    if merged.title.trim().is_empty() {
        tracing::warn!(?providers, "no provider named this work; not storing it");
        return Ok(None);
    }

    if let Some(missing) = &missing {
        tracing::warn!(
            title = %merged.title,
            failure = %missing,
            "a refresh some providers did not answer; what they gave before is kept"
        );
    }

    // The seasons and the episodes this refresh moved, it moved by handing
    // the numbering over, when it did: the journal says so.
    let stored = super::persist_as(
        state,
        merged,
        &snapshots,
        provenance,
        failure.as_deref(),
        numbering.change,
    )
    .await?;
    if !super::is_previewing() {
        keep_numbering(state, &stored, &numbering).await;
    }
    Ok(Some(stored))
}

// ─── who numbers a series ────────────────────────────────────────────────────

/// How many refreshes in a row TheTVDB may leave a series unanswered before
/// TMDB numbers it, when `numbering.fallbackAfter` says nothing.
pub const FALLBACK_AFTER: i64 = 3;

/// How far apart two refreshes TheTVDB did not answer must be to count as
/// two: the most part of a day. A refresh that failed is tried again within
/// hours, and an outage of TheTVDB's of an afternoon would otherwise hand
/// every series it held to TMDB — and Sonarr would renumber its library.
const SILENCE_APART: TimeDelta = TimeDelta::hours(20);

/// Who numbers a series this refresh, TheTVDB's silence weighed — and
/// whether TheTVDB still knows it.
///
/// TheTVDB numbers a series — Sonarr addresses it by TheTVDB's id and its
/// numbers — and a refresh it does not answer keeps the numbering it gave
/// last (`hand_back`). When it has not answered for `numbering.fallbackAfter`
/// refreshes in a row (three unless set; none, never), twenty hours apart at
/// the least, and the series has a TMDB id TMDB answered for, TMDB numbers
/// it: its seasons and episodes — or, where TMDB keeps a group of the
/// original air date that numbers the same episodes, that group's — each
/// keeping the TheTVDB id the episode of its numbers carried. The audit
/// trail says so, `item.numbering_fallback`, and the notifications that want
/// it are told, `numbering.fallback`; the work's page and its editor carry a
/// banner while it lasts.
///
/// It lasts until TheTVDB answers again, when its numbering is taken back
/// of itself — `item.numbering_restored` — unless somebody froze one of the
/// work's orders as the aired numbering meanwhile: what they froze was
/// TMDB's, and stays so until they give the numbering back by hand
/// (`POST /items/{id}/numbering/restore`).
///
/// TheTVDB answering that it has no such series is counted apart, the same
/// twenty hours apart: it keeps the numbering TheTVDB gave last, as a
/// silence does, and at the second — or when TheTVDB's feed of updates says
/// it deleted the series (`service::numbering::watch`) — the numbering as it
/// is served then is kept and frozen as the aired numbering, the episodes
/// that air afterwards placed after it; TMDB numbers the work meanwhile, as
/// above, where it answers with a list. The same trail and notifications say
/// so. It lasts until TheTVDB answers for the series again; for a series
/// TheTVDB merged into another, until the numbering is given back by hand.
/// `numbering.fallbackAfter` at `0` keeps it from happening at all.
#[derive(Debug, Default)]
struct Numbering {
    /// The work as held, by id: nothing is counted for one not held yet.
    work: Option<String>,
    /// TMDB numbers it this time.
    by_tmdb: bool,
    /// What was kept of it before this refresh, and what is after it.
    held: repo::order::Numbering,
    kept: repo::order::Numbering,
    /// What changed, to tell.
    change: Option<NumberingChange>,
}

impl Numbering {
    /// The numbering of a series TheTVDB no longer knows not kept after
    /// all: it is numbered as it was before this refresh — TMDB going on
    /// numbering it only where it did in TheTVDB's silence — and the 404s
    /// stay counted, so that the next refresh keeps it.
    fn undrop(&mut self) {
        self.kept = repo::order::Numbering {
            gone: self.kept.gone,
            gone_at: self.kept.gone_at.clone(),
            ..self.held.clone()
        };
        self.by_tmdb = self.by_tmdb && self.held.since.is_some();
        self.change = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NumberingChange {
    /// TMDB took over from TheTVDB.
    ToTmdb,
    /// TheTVDB took its numbering back.
    BackToTvdb,
    /// TheTVDB answered at refresh after refresh that it has no such series:
    /// its numbering is kept as it was served.
    Dropped,
    /// TheTVDB knows the series again, and numbers it.
    Known,
}

/// What TheTVDB did on one refresh of a series.
#[derive(Clone, Copy, Debug, Default)]
struct Seen {
    /// TheTVDB's numbering came: TheTVDB, or Skyhook which hands it on,
    /// gave a list.
    listed: bool,
    /// TheTVDB itself answered with the series.
    known: bool,
    /// TheTVDB answered that it has no such series.
    gone: bool,
    /// TheTVDB was asked and failed, and nobody gave its numbering.
    silent: bool,
    /// TMDB answered with a list.
    tmdb_listed: bool,
    /// An order somebody froze as the aired numbering: read only where it
    /// can matter, while TMDB numbers the series and TheTVDB lists it.
    frozen_by_hand: bool,
    /// TheTVDB was asked by the id it merged the series into, which the
    /// work takes: TheTVDB numbers it under that one, and its silence is a
    /// silence.
    rekeyed: bool,
}

/// Weigh what TheTVDB did on this refresh against what is kept of the
/// work's numbering (`held`), as of `now`, the fallback set to `after`:
/// what is kept after it, whether TMDB numbers the work this time, and what
/// changed.
fn weigh(
    held: &repo::order::Numbering,
    seen: Seen,
    now: chrono::DateTime<Utc>,
    after: i64,
) -> (repo::order::Numbering, bool, Option<NumberingChange>) {
    use crate::domain::orders::TvdbDropped;

    let at = crate::db::to_rfc3339(now);
    let apart = |last: Option<&str>| {
        last.and_then(crate::db::parse_rfc3339)
            .is_none_or(|last| now - last >= SILENCE_APART)
    };
    let mut kept = held.clone();
    let mut change = None;

    // Whether TheTVDB knows the series.
    if seen.known {
        kept.gone = 0;
        kept.gone_at = None;
        // TheTVDB numbering the series again: a list, not its record alone.
        // A series TheTVDB merged into another is answered for under the id
        // it kept: Sonarr's files are matched by the one it had, whose
        // numbering is given back by hand.
        if seen.listed && kept.dropped.is_some() && kept.dropped_why != Some(TvdbDropped::Merged) {
            kept.dropped = None;
            kept.dropped_why = None;
            kept.merged_into = None;
            change = Some(NumberingChange::Known);
        }
    }
    if seen.gone {
        if apart(kept.gone_at.as_deref()) {
            kept.gone = kept.gone.saturating_add(1);
            kept.gone_at = Some(at.clone());
        }
        if kept.dropped.is_none() && after > 0 && kept.gone >= numbering::DROP_AFTER {
            kept = kept.without_silence();
            kept.dropped = Some(at.clone());
            kept.dropped_why = Some(TvdbDropped::Missing);
            change = Some(NumberingChange::Dropped);
        }
    }
    // TMDB numbers a series TheTVDB no longer knows while nobody numbers it
    // as TheTVDB does; what the aired order answers with is kept apart. One
    // TheTVDB merged into another it numbers under that one, the series'
    // own: TheTVDB failing for it there is weighed as any silence, below.
    let numbered_under_the_merge = seen.rekeyed && kept.dropped_why == Some(TvdbDropped::Merged);
    if kept.dropped.is_some() && !numbered_under_the_merge {
        let by_tmdb = seen.tmdb_listed && !seen.listed;
        return (kept, by_tmdb, change);
    }

    // Whether TheTVDB answers for it at all.
    if seen.listed {
        if held.since.is_some() && seen.tmdb_listed && seen.frozen_by_hand {
            // Somebody froze the numbering TMDB gave: it stays TMDB's until
            // they give it back.
            let kept = repo::order::Numbering {
                silent: 0,
                silent_at: None,
                ..kept
            };
            return (kept, true, change);
        }
        let back = held.since.is_some();
        return (
            kept.without_silence(),
            false,
            change.or(back.then_some(NumberingChange::BackToTvdb)),
        );
    }
    if seen.silent && apart(kept.silent_at.as_deref()) {
        kept.silent = kept.silent.saturating_add(1);
        kept.silent_at = Some(at.clone());
    }
    if !(seen.silent || seen.gone) {
        return (kept, false, change);
    }
    if kept.since.is_some() {
        return (kept, seen.tmdb_listed, change);
    }
    if seen.silent && after > 0 && i64::from(kept.silent) >= after && seen.tmdb_listed {
        kept.since = Some(at);
        return (kept, true, Some(NumberingChange::ToTmdb));
    }
    (kept, false, change)
}

/// Weigh what TheTVDB did on this refresh against what it did on the last.
/// `gone` is the work held under the TheTVDB id TheTVDB answered it has no
/// series by, and that id; `rekeyed`, whether TheTVDB was asked by the id
/// it merged the series into.
async fn numbering_of(
    state: &AppState,
    answers: &[Answer],
    failed: &[Failure],
    gone: Option<&(String, i64)>,
    rekeyed: bool,
) -> Numbering {
    let listed = |provider: &str| {
        answers
            .iter()
            .any(|a| a.provider == provider && !a.item.episodes.is_empty())
    };
    // TheTVDB's numbering answered when TheTVDB or Skyhook, which hands it
    // on, gave a list; it was silent when TheTVDB was asked and failed, and
    // nobody gave it.
    let tvdb_listed = merge::TVDB_NUMBERED.iter().any(|p| listed(p));
    let tvdb_known = answers.iter().any(|a| a.provider == names::TVDB);
    let tvdb_silent = !tvdb_listed && failed.iter().any(|f| f.provider == names::TVDB);
    let is_series = gone.is_some()
        || answers
            .iter()
            .find(|a| !SUPPLEMENTS.contains(&a.provider))
            .is_some_and(|a| a.item.kind == MediaKind::Series);
    if !is_series || !(tvdb_listed || tvdb_silent || tvdb_known || gone.is_some()) {
        return Numbering::default();
    }
    let work = match gone {
        Some((work, _)) => Some(work.clone()),
        None => held_id(state, answers).await,
    };
    let Some(work) = work else {
        return Numbering::default();
    };
    let held = match repo::order::numbering(&state.db, &work).await {
        Ok(held) => held,
        Err(e) => {
            tracing::warn!(id = %work, error = %e, "could not read who numbers the series");
            return Numbering::default();
        }
    };
    let tmdb_listed = listed(names::TMDB);
    let frozen_by_hand = held.dropped.is_none()
        && held.since.is_some()
        && tvdb_listed
        && tmdb_listed
        && frozen_by_hand(state, &work).await;
    let after = state
        .settings
        .int_at("numbering.fallbackAfter", None, None)
        .unwrap_or(FALLBACK_AFTER);

    let seen = Seen {
        listed: tvdb_listed,
        known: tvdb_known,
        gone: gone.is_some(),
        silent: tvdb_silent,
        tmdb_listed,
        frozen_by_hand,
        rekeyed,
    };
    let (kept, by_tmdb, change) = weigh(&held, seen, Utc::now(), after);
    Numbering {
        work: Some(work),
        by_tmdb,
        held,
        kept,
        change,
    }
}

/// Whether one of a work's orders is frozen as its aired numbering by
/// somebody: the numbering TheTVDB last gave, frozen of itself, is not one.
async fn frozen_by_hand(state: &AppState, work: &str) -> bool {
    repo::order::frozen(&state.db, work)
        .await
        .ok()
        .flatten()
        .is_some_and(|key| key != numbering::TVDB_LAST)
}

/// TMDB's episodes numbered by its group of the original air date, when it
/// keeps one that numbers the same episodes, each once: what TMDB itself
/// says the series aired as, where its own seasons are cut otherwise. A
/// group that adds episodes, or leaves some out, is someone's other order
/// under that type, and TMDB's own seasons stand.
///
/// They stand too where the group puts fewer episodes than they do at the
/// numbers the series is `known` by — the work as held, which TheTVDB
/// numbered before TMDB took over, or TMDB since: the numbering that keeps
/// Sonarr's files where they are is the one to serve. Stranger Things' group
/// of the original air date cuts its fifth season in three, by the parts it
/// aired in, where TMDB's own seasons keep it whole as TheTVDB does. A
/// series known by no episode TMDB names is numbered by the group.
async fn number_by_air_date_group(
    state: &AppState,
    answers: &mut [Answer],
    tmdb_groups: Option<Vec<crate::domain::EpisodeOrder>>,
    known: &[Episode],
) {
    let Some(answer) = answers.iter_mut().find(|a| a.provider == names::TMDB) else {
        return;
    };
    let known_at: HashMap<i64, (i32, i32)> = known
        .iter()
        .filter(|e| e.season_number > 0)
        .filter_map(|e| Some((e.tmdb_id?, (e.season_number, e.episode_number))))
        .collect();
    let groups: Vec<String> = tmdb_groups
        .unwrap_or_default()
        .into_iter()
        .filter(|group| group.kind == OrderKind::Aired)
        .filter_map(|group| group.source_ref)
        .collect();
    let listed: HashSet<i64> = answer
        .item
        .episodes
        .iter()
        .filter(|e| e.season_number > 0)
        .filter_map(|e| e.tmdb_id)
        .collect();
    if listed.is_empty() {
        return;
    }

    for group in groups {
        let places = match state.tmdb.episode_group(&group).await {
            Ok(Some(detail)) => tmdb_map::episode_group_places(&detail),
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(
                    group = %group,
                    error = format_args!("{e:#}"),
                    "TMDB's group of the original air date could not be read"
                );
                continue;
            }
        };
        let regular: Vec<_> = places.iter().filter(|p| p.season_number > 0).collect();
        let named: HashSet<i64> = regular.iter().filter_map(|p| p.tmdb_id).collect();
        if named != listed || named.len() != regular.len() {
            continue;
        }
        let at: HashMap<i64, (i32, i32)> = places
            .iter()
            .filter_map(|p| Some((p.tmdb_id?, (p.season_number, p.episode_number))))
            .collect();
        let agreeing = |numbered: &dyn Fn(&Episode) -> Option<(i32, i32)>| {
            answer
                .item
                .episodes
                .iter()
                .filter(|e| {
                    e.tmdb_id
                        .and_then(|id| known_at.get(&id))
                        .is_some_and(|known| numbered(e).as_ref() == Some(known))
                })
                .count()
        };
        let by_group = agreeing(&|e| e.tmdb_id.and_then(|id| at.get(&id).copied()));
        let by_seasons = agreeing(&|e| Some((e.season_number, e.episode_number)));
        if by_group + by_seasons > 0 && by_group <= by_seasons {
            continue;
        }
        for episode in &mut answer.item.episodes {
            if let Some((season, number)) = episode.tmdb_id.and_then(|id| at.get(&id).copied()) {
                episode.season_number = season;
                episode.episode_number = number;
            }
        }
        // A special the group leaves out keeps its place, where no episode
        // of the group took it.
        let mut taken = HashSet::new();
        answer
            .item
            .episodes
            .retain(|e| taken.insert((e.season_number, e.episode_number)));
        answer
            .item
            .episodes
            .sort_by_key(|e| (e.season_number, e.episode_number));
        // Its seasons are the group's: TMDB's of the same number keep their
        // names and pictures, and a number TMDB has none of is listed bare.
        let numbers: Vec<i32> = {
            let mut numbers: Vec<i32> = answer
                .item
                .episodes
                .iter()
                .map(|e| e.season_number)
                .collect();
            numbers.dedup();
            numbers
        };
        answer
            .item
            .seasons
            .retain(|season| numbers.contains(&season.season_number));
        for number in numbers {
            if !answer
                .item
                .seasons
                .iter()
                .any(|season| season.season_number == number)
            {
                let mut bare = repo::child::blank_season(number);
                bare.is_manual = false;
                answer.item.seasons.push(bare);
            }
        }
        answer
            .item
            .seasons
            .sort_by_key(|season| season.season_number);
        return;
    }
}

/// Each episode TMDB numbers keeps the TheTVDB id it had, where it carries
/// none of its own: what Sonarr, and the orders kept beside the series, know
/// it by. The id the episode of the same TMDB id carried — in TheTVDB's list
/// or in the work as held (`stored`) — first, since TMDB may number it
/// otherwise than TheTVDB did, and the episode of its numbers in TheTVDB's
/// list is then another; that one's for an episode no TMDB id names.
fn keep_tvdb_ids(episodes: &mut [Episode], tvdb_list: &[Episode], stored: &[Episode]) {
    // TheTVDB's own list over the work as held, where both name an episode.
    let by_tmdb: HashMap<i64, i64> = stored
        .iter()
        .chain(tvdb_list)
        .filter_map(|e| Some((e.tmdb_id?, e.tvdb_id?)))
        .collect();
    let by_numbers: HashMap<(i32, i32), i64> = tvdb_list
        .iter()
        .filter_map(|e| Some(((e.season_number, e.episode_number), e.tvdb_id?)))
        .collect();
    let mut given: HashSet<i64> = episodes.iter().filter_map(|e| e.tvdb_id).collect();
    for episode in episodes.iter_mut().filter(|e| e.tvdb_id.is_none()) {
        if let Some(id) = episode.tmdb_id.and_then(|tmdb| by_tmdb.get(&tmdb))
            && given.insert(*id)
        {
            episode.tvdb_id = Some(*id);
        }
    }
    for episode in episodes.iter_mut().filter(|e| e.tvdb_id.is_none()) {
        if let Some(id) = by_numbers.get(&(episode.season_number, episode.episode_number))
            && given.insert(*id)
        {
            episode.tvdb_id = Some(*id);
        }
    }
}

/// Keep what this refresh made of the series' numbering, and tell what
/// changed. The numbering of a series TheTVDB no longer knows was kept, and
/// said so, before the work was written (`store`).
async fn keep_numbering(state: &AppState, stored: &MediaItem, numbering: &Numbering) {
    if numbering.work.is_none() {
        return;
    }
    // Written only when it moved: a series TheTVDB answers for, as most are,
    // costs its refresh no write here. Kept already for one dropped now.
    if numbering.kept != numbering.held
        && numbering.change != Some(NumberingChange::Dropped)
        && let Err(e) = repo::order::set_numbering(&state.db, &stored.id, &numbering.kept).await
    {
        tracing::warn!(id = %stored.id, error = %e, "could not keep who numbers the series");
    }
    match numbering.change {
        Some(NumberingChange::Known) => {
            numbering::forget_last(state, &stored.id).await;
            let detail = format!(
                "“{}”: TheTVDB knows the series again and numbers it",
                stored.title
            );
            tracing::info!(id = %stored.id, title = %stored.title, "{detail}");
            crate::api::audit::record_system(
                state,
                Action::ItemNumberingRestored,
                &stored.id,
                &detail,
            )
            .await;
        }
        Some(NumberingChange::Dropped | NumberingChange::ToTmdb | NumberingChange::BackToTvdb)
        | None => {}
    }
    match numbering.change {
        Some(NumberingChange::ToTmdb) => {
            let detail = format!(
                "“{}”: TheTVDB has not answered for {} refreshes; TMDB numbers the series until it \
                 does",
                stored.title, numbering.kept.silent
            );
            tracing::warn!(id = %stored.id, title = %stored.title, "{detail}");
            crate::api::audit::record_system(
                state,
                Action::ItemNumberingFallback,
                &stored.id,
                &detail,
            )
            .await;
            crate::service::webhook::tell(
                state,
                crate::service::webhook::Fact::NumberingFallback,
                Some(stored.id.clone()),
                Some(detail),
            );
        }
        Some(NumberingChange::BackToTvdb) => {
            let detail = format!(
                "“{}”: TheTVDB answers again and numbers the series",
                stored.title
            );
            tracing::info!(id = %stored.id, title = %stored.title, "{detail}");
            crate::api::audit::record_system(
                state,
                Action::ItemNumberingRestored,
                &stored.id,
                &detail,
            )
            .await;
        }
        Some(NumberingChange::Dropped | NumberingChange::Known) | None => {}
    }
}

/// What did not answer among the providers `counts` names, as the work's
/// `refreshError` says it — `None` when all of those asked answered in full.
fn failure_note(
    failed: &[Failure],
    answers: &[Answer],
    counts: impl Fn(&str) -> bool,
) -> Option<String> {
    let mut parts: Vec<String> = failed
        .iter()
        .filter(|f| counts(f.provider))
        .map(|f| format!("{}: {}", f.provider, f.error))
        .collect();
    for answer in answers
        .iter()
        .filter(|a| counts(a.provider) && !a.unanswered_seasons.is_empty())
    {
        let seasons: Vec<String> = answer
            .unanswered_seasons
            .iter()
            .map(i32::to_string)
            .collect();
        parts.push(format!(
            "{}: season {} went unanswered",
            answer.provider,
            seasons.join(", ")
        ));
    }

    (!parts.is_empty()).then(|| {
        crate::providers::clip(
            &format!(
                "refreshed in part; kept what these gave before — {}",
                parts.join("; ")
            ),
            super::REFRESH_ERROR_CHARS,
        )
    })
}

/// The id of the work these answers describe, when the catalogue holds it.
async fn held_id(state: &AppState, answers: &[Answer]) -> Option<String> {
    let kind = answers
        .iter()
        .find(|a| !SUPPLEMENTS.contains(&a.provider))?
        .item
        .kind;

    // Its identifiers as the merge will make them: in priority order, the
    // first of each kind.
    let priority = &state.provider_priority();
    let rank = |provider: &str| {
        priority
            .iter()
            .position(|p| p == provider)
            .unwrap_or(usize::MAX)
    };
    let mut ranked: Vec<&Answer> = answers.iter().collect();
    ranked.sort_by_key(|a| rank(a.provider));
    let mut probe = MediaItem::empty(kind);
    for answer in ranked {
        merge::merge_ids(&mut probe.external_ids, answer.item.external_ids.clone());
    }

    match super::find_existing(state, &probe).await {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "could not look the stored work up; what failed providers gave it is not kept"
            );
            None
        }
    }
}

/// The work these answers describe, as stored, and who gave it what: what
/// stands for a provider that did not answer. `None` for a work not held
/// yet, or one that cannot be read — logged, and the refresh goes on with
/// what answered. `gone` names it when TheTVDB has no series by the id it
/// is held under: then the supplements alone may have answered, which name
/// no work of themselves.
async fn held_before(
    state: &AppState,
    answers: &[Answer],
    gone: Option<&(String, i64)>,
) -> Option<(MediaItem, Provenance)> {
    let id = match gone {
        Some((id, _)) => id.clone(),
        None => held_id(state, answers).await?,
    };

    let read = async {
        let Some(mut stored) = repo::item::get(&state.db, &id).await? else {
            return anyhow::Ok(None);
        };
        repo::item::load_children(&state.db, &mut stored).await?;
        let provenance = repo::item::provenance(&state.db, &id)
            .await?
            .unwrap_or_default();
        Ok(Some((stored, provenance)))
    };

    match read.await {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "could not read the stored work; what failed providers gave it is not kept"
            );
            None
        }
    }
}

/// The stored episodes of the seasons `answer` could not give, put back
/// into it under the numbers they are held by, as it gave them last time.
///
/// Without them the season came back empty from its numbering source, the
/// write deleted its episodes, and Sonarr deletes every episode a series it
/// refreshes no longer lists — unlinking their files until a later refresh.
fn fill_unanswered_seasons(answer: &mut Answer, stored: &MediaItem) {
    if answer.unanswered_seasons.is_empty() {
        return;
    }
    for &number in &answer.unanswered_seasons {
        if answer
            .item
            .episodes
            .iter()
            .any(|e| e.season_number == number)
        {
            continue;
        }
        answer.item.episodes.extend(
            stored
                .episodes
                .iter()
                .filter(|e| e.season_number == number && !e.is_manual)
                .cloned(),
        );
    }
    answer
        .item
        .episodes
        .sort_by_key(|e| (e.season_number, e.episode_number));
}

/// Fold back in what each provider that failed gave the work last time, as
/// the sync does for the providers it does not ask — and keep the stored
/// episode list when a source that numbers it failed, which left alone hands
/// the numbering to TMDB under a TheTVDB id.
///
/// A source that numbers the list and failed hands back its values and
/// pictures, not its list, when another numbering it the same way answered
/// with one: the fresh list stands. Nor while TMDB numbers the series in
/// TheTVDB's place (`by_tmdb`, see [`Numbering`]): its list is the one.
fn hand_back(
    contributions: &mut Vec<Contribution>,
    failed: &[Failure],
    stored: &MediaItem,
    provenance: &Provenance,
    by_tmdb: bool,
) {
    let renumbered = by_tmdb
        || contributions.iter().any(|c| {
            merge::TVDB_NUMBERED.contains(&c.provider.as_str()) && !c.item.episodes.is_empty()
        });

    for failure in failed {
        if contributions.iter().any(|c| c.provider == failure.provider) {
            continue;
        }
        let Some(mut item) = merge::provenance::reconstruct(stored, provenance, failure.provider)
        else {
            continue;
        };
        if renumbered && merge::TVDB_NUMBERED.contains(&failure.provider) {
            item.episodes.clear();
            item.seasons.clear();
        }
        contributions.push(Contribution {
            provider: failure.provider.to_string(),
            item,
        });
    }

    if !by_tmdb
        && failed
            .iter()
            .any(|f| merge::TVDB_NUMBERED.contains(&f.provider))
    {
        merge::provenance::protect_numbering(contributions, stored, provenance);
    }
}

/// The adult flag stands when a source whose word it may be failed: AniList
/// and MyAnimeList often flag what nobody else does, and a refresh they did
/// not answer would otherwise write the work as for everyone — to callers
/// and servers that hide adult titles — until the next one.
fn keep_adult(merged: &mut MediaItem, stored: &MediaItem, failed: &[Failure]) {
    if stored.is_adult
        && failed
            .iter()
            .any(|f| ADULT_AUTHORITIES.contains(&f.provider))
    {
        merged.is_adult = true;
    }
}

/// Each answer taken apart before the merge consumes them, in the order the
/// merge folds them in, to say afterwards who gave what.
fn views_of(priority: &[String], contributions: &[Contribution]) -> Vec<merge::provenance::View> {
    let rank = |provider: &str| {
        priority
            .iter()
            .position(|p| p == provider)
            .unwrap_or(usize::MAX)
    };

    let mut views: Vec<(usize, usize, merge::provenance::View)> = contributions
        .iter()
        .enumerate()
        .map(|(at, c)| {
            (
                rank(&c.provider),
                at,
                merge::provenance::view(&c.provider, &c.item),
            )
        })
        .collect();
    views.sort_by_key(|(rank, at, _)| (*rank, *at));
    views.into_iter().map(|(_, _, view)| view).collect()
}

// ─── a sync from chosen sources ──────────────────────────────────────────────

/// Why a provider cannot be asked about a work now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum Unavailable {
    /// It is switched off, or has no key to be asked with.
    Off,
    /// The work carries no identifier it can be asked by.
    NoId,
}

/// A provider that could describe a work, and whether it can be asked now.
pub struct Askable {
    pub provider: &'static str,
    pub unavailable: Option<Unavailable>,
    /// Asked along with it, because part of what it gives is theirs to
    /// finish: TVmaze's broadcast instants go onto the episode list, so asking
    /// the list again without them would put TheTVDB's back.
    pub brings: Vec<&'static str>,
}

/// The identifiers a person locked on the work these ids stand for, when
/// they did: which AniList and MyAnimeList entries they meant, asked about
/// before the identifier list's guess.
async fn pinned_ids(
    state: &AppState,
    kind: MediaKind,
    tmdb: Option<i64>,
    tvdb: Option<i64>,
) -> Option<ExternalIds> {
    let by = [
        (ExternalSource::tmdb_for(kind), tmdb),
        (ExternalSource::tvdb_for(kind), tvdb),
    ];
    for (source, id) in by {
        let Some(id) = id else { continue };
        let Ok(Some(work)) =
            repo::item::find_id_by_external(&state.db, source, &id.to_string()).await
        else {
            continue;
        };
        let locks = repo::override_field::list(&state.db, &work).await.ok()?;
        return locks
            .into_iter()
            .find(|o| o.scope == "item" && o.field == "externalIds")
            .and_then(|o| o.value)
            .and_then(|value| serde_json::from_value(value).ok());
    }
    None
}

/// Whether a provider is switched on and able to answer, whatever the work.
pub fn switched_on(state: &AppState, provider: &str) -> bool {
    let anime = anime::enabled(state);
    match provider {
        names::TMDB => state.tmdb.is_configured(),
        names::TVDB => state.tvdb.is_enabled(),
        // Asked on every fetch, or only when nothing else answered: either
        // way it answers when asked.
        names::SKYHOOK => {
            state.flag("skyhook.enrich", true) || state.flag("skyhook.fallback", true)
        }
        names::RADARR => state.flag("radarr.enrich", true) || state.flag("radarr.fallback", true),
        names::FANART => state.fanart.is_enabled(),
        names::TVMAZE => state.flag("tvmaze.enabled", false),
        names::ANILIST => anime && state.flag("anilist.enabled", false),
        names::MAL => anime && state.flag("mal.enabled", false),
        names::FANKAI => state.flag("fankai.enabled", false),
        names::FANKAI_WIKI => {
            state.flag("fankai.enabled", false) && state.flag("fankai.wiki", false)
        }
        _ => false,
    }
}

/// Every provider that could describe `stored`, in the order a refresh asks
/// them.
pub fn askable(
    state: &AppState,
    stored: &MediaItem,
    provenance: Option<&Provenance>,
) -> Vec<Askable> {
    let ids = &stored.external_ids;
    let one = |provider: &'static str, addressed: bool| Askable {
        provider,
        unavailable: if !switched_on(state, provider) {
            Some(Unavailable::Off)
        } else if !addressed {
            Some(Unavailable::NoId)
        } else {
            None
        },
        brings: Vec::new(),
    };

    let mut sources = match stored.kind {
        // A Fan-Kai has its own source and nothing else: see `fankai_series`.
        MediaKind::Series if ids.fankai.is_some() => {
            vec![one(names::FANKAI, true), one(names::FANKAI_WIKI, true)]
        }
        MediaKind::Series => {
            let tvdb = ids.tvdb.is_some();
            vec![
                one(names::TMDB, ids.tmdb.is_some()),
                one(names::TVDB, tvdb),
                one(names::SKYHOOK, tvdb),
                one(names::FANART, tvdb),
                one(names::TVMAZE, tvdb || ids.tvmaze.is_some()),
                one(names::ANILIST, tvdb || !ids.anilist.is_empty()),
                one(names::MAL, tvdb || !ids.mal.is_empty()),
            ]
        }
        MediaKind::Movie => {
            let tmdb = ids.tmdb.is_some();
            let either = tmdb || ids.imdb.is_some();
            vec![
                one(names::TMDB, tmdb),
                one(names::RADARR, either),
                one(names::FANART, either),
                one(names::ANILIST, tmdb || !ids.anilist.is_empty()),
                one(names::MAL, tmdb || !ids.mal.is_empty()),
            ]
        }
    };

    let answers = |provider: &str| {
        sources
            .iter()
            .any(|s| s.provider == provider && s.unavailable.is_none())
    };
    let tvmaze = answers(names::TVMAZE);
    // Whoever numbers the episodes, by the rule the sync keeps the list by,
    // when it can be asked now.
    let numbering = merge::provenance::numbering_of(stored, provenance).filter(|n| answers(n));
    let priority = &state.provider_priority();
    let rank = |p: &str| priority.iter().position(|q| q == p).unwrap_or(usize::MAX);
    let fillers: Vec<&'static str> = FILLS_EPISODES
        .iter()
        .copied()
        .filter(|f| Some(*f) != numbering && answers(f))
        .collect();

    for source in &mut sources {
        let provider = source.provider;
        if Some(provider) == numbering {
            if tvmaze {
                source.brings.push(names::TVMAZE);
            }
        } else if let Some(numbering) = numbering
            && fillers.contains(&provider)
        {
            // What a source gave the episodes is folded into the list, where
            // the list's own values come first: asked alone, it would find its
            // old values in the stored list and fill nothing. With the list
            // asked beside it, and every source that fills it in ahead of this
            // one, the episodes are merged as a refresh merges them. TVmaze's
            // gaps are filled last whatever its rank, so it needs nobody.
            source.brings.push(numbering);
            if provider != names::TVMAZE {
                source.brings.extend(
                    fillers.iter().copied().filter(|f| {
                        *f != names::TVMAZE && *f != provider && rank(f) < rank(provider)
                    }),
                );
                if tvmaze {
                    source.brings.push(names::TVMAZE);
                }
            }
        }
    }

    sources
}

/// Sources that fill in episodes someone else numbers.
const FILLS_EPISODES: &[&str] = &[names::TMDB, names::SKYHOOK, names::TVMAZE];

/// What a sync from chosen sources did.
pub struct Resynced {
    pub item: MediaItem,
    /// Asked, and answered: what they said now stands.
    pub answered: Vec<&'static str>,
    /// Asked, and failed or had nothing: what they gave before stands.
    pub silent: Vec<&'static str>,
}

/// How a sync from chosen sources ended.
pub enum ResyncOutcome {
    Synced(Box<Resynced>),
    /// None of the sources asked answered: nothing was written.
    NoneAnswered,
    /// Something else wrote the work while the sources were being asked:
    /// nothing was written, so as not to undo it.
    Changed,
    /// The work was deleted while the sources were being asked.
    Gone,
}

/// Ask `asking` again, and only them, and fold what they say into the work as
/// every other source last described it.
///
/// The others are handed back what they gave, from the provenance, and the
/// usual merge weighs the fresh answers against them by the usual priority:
/// the result is what a full refresh would have made had nobody else changed
/// their mind. Beneath all of them lies the stored work, so a value the asked
/// sources no longer give is kept rather than lost until a full refresh
/// settles it, and the episode list stands unless whoever numbers it gave one
/// anew.
///
/// Written into the work it was read from, with its own identifiers and
/// schedule — a sync is not a refresh — and only if nothing else wrote it
/// meanwhile.
pub async fn resync(
    state: &AppState,
    stored: &MediaItem,
    provenance: &Provenance,
    asking: &[&'static str],
) -> Result<ResyncOutcome> {
    let mut answers = ask_again(state, stored, asking).await;
    // A season asked again that went unanswered keeps what is stored of it,
    // as a refresh keeps it; and AniList asked without its characters, those
    // it gave before.
    for answer in &mut answers {
        fill_unanswered_seasons(answer, stored);
        if answer.provider == names::ANILIST
            && !crate::providers::anilist::asked_characters(&answer.payload)
        {
            answer.item.characters = merge::provenance::characters_given(stored, names::ANILIST);
        }
    }
    let answered: Vec<&'static str> = answers.iter().map(|a| a.provider).collect();
    let silent: Vec<&'static str> = asking
        .iter()
        .copied()
        .filter(|p| !answered.contains(p))
        .collect();

    if answers.is_empty() {
        return Ok(ResyncOutcome::NoneAnswered);
    }

    // The episode groups TMDB lists, and its own seasons, when it was asked
    // again; and TVmaze's episodes, when it was.
    let tmdb = answers.iter().find(|a| a.provider == names::TMDB);
    let tmdb_groups = tmdb.and_then(|a| tmdb_map::episode_groups(&a.payload));
    let tmdb_aired = tmdb.map(|a| orders::TmdbAired::of(&a.item.episodes, &a.unanswered_seasons));
    let tvmaze_episodes: Option<Vec<Episode>> = answers
        .iter()
        .find(|a| a.provider == names::TVMAZE)
        .map(|a| a.item.episodes.clone());

    let snapshots = snapshots_of(&mut answers);

    let mut contributions: Vec<Contribution> = answers
        .into_iter()
        .map(|a| Contribution {
            provider: a.provider.to_string(),
            item: a.item,
        })
        .collect();

    for provider in provenance.providers() {
        if answered.contains(&provider) {
            continue;
        }
        if let Some(item) = merge::provenance::reconstruct(stored, provenance, provider) {
            contributions.push(Contribution {
                provider: provider.to_string(),
                item,
            });
        }
    }

    merge::provenance::protect_numbering(&mut contributions, stored, provenance);

    // In the order a refresh gathers them: providers the priority does not
    // name share a rank, and the merge keeps their order among themselves.
    let gathered = |provider: &str| {
        merge::rules::PROVIDERS
            .iter()
            .position(|p| *p == provider)
            .unwrap_or(usize::MAX)
    };
    contributions.sort_by_key(|c| gathered(&c.provider));

    // Who gave what is said of the providers alone: the stored work beneath
    // them is not one.
    let priority = state.provider_priority();
    let views = views_of(&priority, &contributions);
    contributions.push(Contribution {
        provider: merge::provenance::STORED.into(),
        item: merge::provenance::leftover(stored, &answered),
    });

    let Some(merged) = merge::combine(contributions, &priority) else {
        return Ok(ResyncOutcome::NoneAnswered);
    };
    if merged.title.trim().is_empty() {
        return Ok(ResyncOutcome::NoneAnswered);
    }

    let mut now = merge::provenance::attribute(&merged, &views);
    merge::provenance::carry_over(
        &mut now,
        provenance,
        &merge::provenance::Returned::of(&merged),
    );

    let item =
        match super::persist_sync(state, merged, &snapshots, stored, provenance, &now).await? {
            super::SyncWrite::Written(item) => *item,
            super::SyncWrite::Changed => return Ok(ResyncOutcome::Changed),
            super::SyncWrite::Gone => return Ok(ResyncOutcome::Gone),
        };

    if item.kind == MediaKind::Series && !super::is_previewing() {
        orders::gather(
            state,
            &item,
            orders::Read {
                tvdb: answered.contains(&names::TVDB),
                tmdb_groups,
                tmdb_aired,
            },
        )
        .await;
        numbering::follow(state, &item, tvmaze_episodes.as_deref()).await;
    }
    // A film's collection with it, when the one kept is due, as a refresh
    // keeps it (`collection::keep_for` never asks while only previewed).
    if item.kind == MediaKind::Movie {
        super::collection::keep_for(state, &item).await;
    }

    Ok(ResyncOutcome::Synced(Box::new(Resynced {
        item,
        answered,
        silent,
    })))
}

/// Only the providers in `asking`, each by the identifiers the work carries.
async fn ask_again(state: &AppState, stored: &MediaItem, asking: &[&'static str]) -> Vec<Answer> {
    async fn when(asked: bool, answer: impl Future<Output = Asked>) -> Option<Answer> {
        if asked { answer.await.answer() } else { None }
    }

    let ids = &stored.external_ids;
    let wants = |provider: &str| asking.contains(&provider);

    if stored.kind == MediaKind::Series
        && let Some(fankai_id) = ids.fankai
    {
        let production = if wants(names::FANKAI) && state.flag("fankai.enabled", false) {
            match state.fankai.series(fankai_id).await {
                Ok(Some((raw, item))) => Some(Answer::new(names::FANKAI, raw, item)),
                Ok(None) => None,
                Err(e) => {
                    tracing::warn!(
                        fankai_id,
                        error = format_args!("{e:#}"),
                        "Fankai lookup failed"
                    );
                    None
                }
            }
        } else {
            None
        };

        // The wiki is asked by the production's title and who cut it: the
        // stored ones do when Fankai is not asked again.
        let wiki = if wants(names::FANKAI_WIKI) && state.flag("fankai.wiki", false) {
            fankai_from_wiki(state, production.as_ref().map_or(stored, |a| &a.item))
                .await
                .answer()
        } else {
            None
        };

        return production.into_iter().chain(wiki).collect();
    }

    let mut answers: Vec<Answer> = match stored.kind {
        MediaKind::Series => {
            let (tmdb, tvdb, skyhook, fanart, tvmaze) = tokio::join!(
                when(wants(names::TMDB), series_from_tmdb(state, ids.tmdb)),
                when(wants(names::TVDB), series_from_tvdb(state, ids.tvdb)),
                when(wants(names::SKYHOOK), skyhook_answer(state, ids.tvdb)),
                when(wants(names::FANART), series_from_fanart(state, ids.tvdb)),
                when(
                    wants(names::TVMAZE),
                    series_from_tvmaze(state, ids.tvdb, ids.tmdb)
                ),
            );
            [tmdb, tvdb, skyhook, fanart, tvmaze]
                .into_iter()
                .flatten()
                .collect()
        }
        MediaKind::Movie => {
            let imdb = ids.imdb.as_deref();
            let (tmdb, radarr, fanart) = tokio::join!(
                when(wants(names::TMDB), movie_from_tmdb(state, ids.tmdb)),
                when(wants(names::RADARR), radarr_answer(state, ids.tmdb, imdb)),
                when(
                    wants(names::FANART),
                    movie_from_fanart(state, ids.tmdb, imdb)
                ),
            );
            [tmdb, radarr, fanart].into_iter().flatten().collect()
        }
    };

    if (wants(names::ANILIST) || wants(names::MAL)) && anime::enabled(state) {
        let own = ExternalIds {
            mal: ids.mal.clone(),
            anilist: ids.anilist.clone(),
            ..ExternalIds::default()
        };
        let mapped = match (stored.kind, ids.tvdb, ids.tmdb) {
            (MediaKind::Series, Some(tvdb_id), _) => {
                Some(anime::for_series(state, tvdb_id, &own).await)
            }
            (MediaKind::Movie, _, Some(tmdb_id)) => Some(anime::for_movie(state, tmdb_id).await),
            _ => None,
        };
        // Identifiers locked by hand name the entry the person meant.
        let pinned = stored
            .locked_fields
            .iter()
            .any(|f| f == "item/externalIds")
            .then_some(&own);
        let chosen = Some(anime::choose(mapped, pinned, &own)).filter(|c| !c.is_empty());

        if let Some(chosen) = chosen {
            let (anilist, mal) = tokio::join!(
                when(
                    wants(names::ANILIST),
                    from_anilist(state, chosen.anilist, stored.kind)
                ),
                when(wants(names::MAL), from_mal(state, chosen.mal, stored.kind)),
            );
            for mut answer in [anilist, mal].into_iter().flatten() {
                // As `movie` has it: a film keeps no anime-site id.
                if stored.kind == MediaKind::Movie {
                    answer.item.external_ids = ExternalIds::default();
                }
                answers.push(answer);
            }
        }
    }

    answers
}

// ─── per provider ────────────────────────────────────────────────────────────

async fn series_from_tmdb(state: &AppState, tmdb_id: Option<i64>) -> Asked {
    let Some(tmdb_id) = tmdb_id else {
        return Asked::Nothing;
    };
    if !state.tmdb.is_configured() {
        return Asked::Nothing;
    }

    let (raw, tv) = match state.tmdb.tv(tmdb_id).await {
        Ok(Some(found)) => found,
        Ok(None) => return Asked::Nothing,
        Err(e) => {
            tracing::warn!(
                tmdb_id,
                error = format_args!("{e:#}"),
                "TMDB series fetch failed"
            );
            return Asked::failed(names::TMDB, &e);
        }
    };

    // Capped, because this is one outbound call per entry and the pool they
    // queue in is shared by everything else this process is doing. The season
    // count comes from the answer, and the answer comes from a URL an operator
    // can point elsewhere.
    let numbers: Vec<i32> = tv
        .seasons
        .iter()
        .map(|s| s.season_number)
        .take(MAX_SEASONS)
        .collect();

    if tv.seasons.len() > MAX_SEASONS {
        tracing::warn!(
            tmdb_id,
            seasons = tv.seasons.len(),
            "more seasons than this server will fetch; taking the first {MAX_SEASONS}"
        );
    }

    let (seasons, unanswered) = state.tmdb.tv_seasons(tmdb_id, &numbers).await;

    let item = tmdb_map::tv_to_item(&tv, &seasons, tmdb_map::Limits::of(state));
    let mut answer = Answer::new(names::TMDB, raw, item);
    answer.unanswered_seasons = unanswered;
    Asked::answered(answer)
}

/// Sonarr's own Skyhook, as a second opinion rather than only a fallback.
///
/// It carries things TMDB has no field for: the broadcast time of day, TVMaze
/// and AniList ids, and the air-order hints Sonarr uses for anime.
async fn series_from_skyhook(state: &AppState, tvdb_id: Option<i64>) -> Asked {
    if !state.flag("skyhook.enrich", true) {
        return Asked::Nothing;
    }
    skyhook_answer(state, tvdb_id).await
}

/// What Skyhook says, whatever enrichment is set to: a sync asks it by name.
async fn skyhook_answer(state: &AppState, tvdb_id: Option<i64>) -> Asked {
    let Some(tvdb_id) = tvdb_id else {
        return Asked::Nothing;
    };

    match state.skyhook.show(tvdb_id).await {
        Ok(Some((raw, show))) => Asked::answered(Answer::new(
            names::SKYHOOK,
            raw,
            crate::providers::skyhook::to_item(&show),
        )),
        Ok(None) => Asked::Nothing,
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "Skyhook enrichment failed"
            );
            Asked::failed(names::SKYHOOK, &e)
        }
    }
}

/// TheTVDB, which is where absolute episode numbering comes from.
async fn series_from_tvdb(state: &AppState, tvdb_id: Option<i64>) -> Asked {
    let Some(tvdb_id) = tvdb_id else {
        return Asked::Nothing;
    };
    if !state.tvdb.is_enabled() {
        return Asked::Nothing;
    }

    let cast = crate::providers::tvdb::cast_limit(&state.settings);
    match state.tvdb.series(tvdb_id, cast).await {
        Ok(Some((raw, item))) => Asked::answered(Answer::new(names::TVDB, raw, item)),
        // It answered 404: it has no such series.
        Ok(None) => Asked::Missing(names::TVDB),
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "TheTVDB lookup failed"
            );
            Asked::failed(names::TVDB, &e)
        }
    }
}

/// TVmaze, for the instant each episode aired.
///
/// What it says about an episode is only used where its broadcast date agrees
/// with the spine's — see `merge::apply_broadcast_times`.
async fn series_from_tvmaze(state: &AppState, tvdb_id: Option<i64>, tmdb_id: Option<i64>) -> Asked {
    if !state.flag("tvmaze.enabled", false) {
        return Asked::Nothing;
    }

    // A series fetched before has its TVmaze id on file — Skyhook and TheTVDB
    // both carry it, or a person set it — which turns three requests into
    // one, and asks TVmaze about a series TheTVDB does not know at all.
    let mut known = None;
    for (source, id) in [
        (ExternalSource::TvdbSeries, tvdb_id),
        (ExternalSource::tmdb_for(MediaKind::Series), tmdb_id),
    ] {
        let Some(id) = id else { continue };
        if let Ok(Some(work)) =
            repo::item::find_id_by_external(&state.db, source, &id.to_string()).await
        {
            known = repo::item::load_external_ids(&state.db, &work)
                .await
                .ok()
                .and_then(|ids| ids.tvmaze);
            break;
        }
    }
    if tvdb_id.is_none() && known.is_none() {
        return Asked::Nothing;
    }

    match state.tvmaze.series(tvdb_id, known).await {
        Ok(Some((raw, item))) => Asked::answered(Answer::new(names::TVMAZE, raw, item)),
        Ok(None) => Asked::Nothing,
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "TVmaze lookup failed"
            );
            Asked::failed(names::TVMAZE, &e)
        }
    }
}

/// Fanart.tv, which contributes artwork and nothing else.
///
/// It indexes television on TVDB ids only, so a series with no TVDB id cannot
/// be looked up there at all.
async fn series_from_fanart(state: &AppState, tvdb_id: Option<i64>) -> Asked {
    let Some(tvdb_id) = tvdb_id else {
        return Asked::Nothing;
    };
    if !state.fanart.is_enabled() {
        return Asked::Nothing;
    }

    match state.fanart.series(tvdb_id).await {
        Ok(Some((raw, item))) => Asked::answered(Answer::new(names::FANART, raw, item)),
        Ok(None) => Asked::Nothing,
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "Fanart.tv lookup failed"
            );
            Asked::failed(names::FANART, &e)
        }
    }
}

async fn movie_from_fanart(state: &AppState, tmdb_id: Option<i64>, imdb_id: Option<&str>) -> Asked {
    if !state.fanart.is_enabled() {
        return Asked::Nothing;
    }

    // It accepts either key; TMDB's is the one more titles are indexed under.
    let Some(key) = tmdb_id
        .map(|id| id.to_string())
        .or_else(|| imdb_id.map(String::from))
    else {
        return Asked::Nothing;
    };

    match state.fanart.movie(&key).await {
        Ok(Some((raw, item))) => Asked::answered(Answer::new(names::FANART, raw, item)),
        Ok(None) => Asked::Nothing,
        Err(e) => {
            tracing::warn!(%key, error = format_args!("{e:#}"), "Fanart.tv lookup failed");
            Asked::failed(names::FANART, &e)
        }
    }
}

async fn movie_from_tmdb(state: &AppState, tmdb_id: Option<i64>) -> Asked {
    let Some(tmdb_id) = tmdb_id else {
        return Asked::Nothing;
    };
    if !state.tmdb.is_configured() {
        return Asked::Nothing;
    }

    match state.tmdb.movie(tmdb_id).await {
        Ok(Some((raw, movie))) => Asked::answered(Answer::new(
            names::TMDB,
            raw,
            tmdb_map::movie_to_item(&movie, tmdb_map::Limits::of(state)),
        )),
        Ok(None) => Asked::Nothing,
        Err(e) => {
            tracing::warn!(
                tmdb_id,
                error = format_args!("{e:#}"),
                "TMDB movie fetch failed"
            );
            Asked::failed(names::TMDB, &e)
        }
    }
}

/// Radarr's own metadata service, as a second opinion.
///
/// It resolves certifications by country and carries ratings from IMDb,
/// Metacritic and Rotten Tomatoes that TMDB does not have at all.
async fn movie_from_radarr(state: &AppState, tmdb_id: Option<i64>, imdb_id: Option<&str>) -> Asked {
    if !state.flag("radarr.enrich", true) {
        return Asked::Nothing;
    }
    radarr_answer(state, tmdb_id, imdb_id).await
}

/// What Radarr's service says, whatever enrichment is set to: a sync asks it
/// by name.
async fn radarr_answer(state: &AppState, tmdb_id: Option<i64>, imdb_id: Option<&str>) -> Asked {
    let found = match tmdb_id {
        Some(id) => state.radarr_metadata.movie(id).await,
        None => match imdb_id {
            Some(id) => state.radarr_metadata.by_imdb_id(id).await,
            None => return Asked::Nothing,
        },
    };

    match found {
        Ok(Some((raw, movie))) => Asked::answered(Answer::new(
            names::RADARR,
            raw,
            wire::radarr::to_item(&movie),
        )),
        Ok(None) => Asked::Nothing,
        Err(e) => {
            tracing::warn!(
                ?tmdb_id,
                error = format_args!("{e:#}"),
                "Radarr metadata enrichment failed"
            );
            Asked::failed(names::RADARR, &e)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use axum::{http::StatusCode, response::IntoResponse};
    use serde_json::json;

    use super::*;
    use crate::config;

    /// What the providers answer, by path: changed between two refreshes
    /// to make one of them fail.
    type Routes = Arc<parking_lot::Mutex<HashMap<String, (u16, Value)>>>;

    /// Every provider at one address, answering what `Routes` says and 404
    /// for the rest.
    async fn providers() -> (String, Routes) {
        let routes: Routes = Arc::default();
        let app = axum::Router::new().fallback({
            let routes = routes.clone();
            move |uri: axum::http::Uri| {
                let found = routes.lock().get(uri.path()).cloned();
                async move {
                    match found {
                        Some((status, body)) => (
                            StatusCode::from_u16(status).expect("a status"),
                            axum::Json(body),
                        )
                            .into_response(),
                        None => StatusCode::NOT_FOUND.into_response(),
                    }
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let at = format!("http://{}", listener.local_addr().expect("its address"));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (at, routes)
    }

    /// The server, on a database of its own, TMDB and TheTVDB keyed, every
    /// provider at `at`, the anime sites, TVmaze and Fanart.tv off.
    async fn server(at: &str) -> AppState {
        let mut config = config::Config::from_env().expect("a configuration");
        config.mode = config::Mode::Single;
        config.database = crate::db::testing::database().await;
        config.security.bootstrap_admin = None;
        config.cache.redis_url = None;
        config.media.storage = config::MediaStorage::Off;
        config.clients = None;
        config.tmdb.api_key = Some("key".into());
        config.tmdb.language = "en-US".into();
        config.tvdb.api_key = Some("key".into());
        config.tvdb.enabled = true;
        config.fanart.api_key = None;
        config.tvmaze.enabled = false;
        config.anilist.enabled = false;
        config.mal.enabled = false;
        config.skyhook.enrich = true;
        for upstream in [
            &mut config.tmdb.upstream,
            &mut config.tvdb.upstream,
            &mut config.fanart.upstream,
            &mut config.skyhook.upstream,
            &mut config.tvmaze.upstream,
            &mut config.anilist.upstream,
            &mut config.mal.upstream,
            &mut config.mal.jikan_upstream,
            &mut config.mal.tenrai_upstream,
        ] {
            upstream.clone_from(&at.to_string());
        }
        AppState::bootstrap(config).await.expect("a server")
    }

    fn tmdb_episode(season: i32, number: i32, name: &str) -> Value {
        json!({ "season_number": season, "episode_number": number, "name": name, "overview": format!("{name}, as TMDB tells it") })
    }

    fn numbers(item: &MediaItem) -> Vec<(i32, i32)> {
        item.episodes
            .iter()
            .map(|e| (e.season_number, e.episode_number))
            .collect()
    }

    /// The deadline a work written now is given, at most, when its refresh
    /// failed in part: the failure's, not an ended series' week.
    fn retried_soon(item: &MediaItem) -> bool {
        let due = item
            .refresh_after
            .as_deref()
            .and_then(crate::db::parse_rfc3339)
            .expect("a deadline");
        due <= chrono::Utc::now() + chrono::TimeDelta::hours(6) + chrono::TimeDelta::minutes(1)
    }

    /// A season TMDB could not give this time keeps the episodes stored for
    /// it: written without them, Sonarr deleted every one, and the refresh
    /// said it was complete for a week.
    #[tokio::test]
    async fn a_season_that_went_unanswered_keeps_its_episodes() {
        let (at, routes) = providers().await;
        let state = server(&at).await;
        {
            let mut routes = routes.lock();
            routes.insert(
                "/3/tv/1399".into(),
                (
                    200,
                    json!({ "id": 1399, "name": "A Series", "status": "Ended",
                            "seasons": [{ "season_number": 1 }, { "season_number": 2 }] }),
                ),
            );
            routes.insert(
                "/3/tv/1399/season/1".into(),
                (
                    200,
                    json!({ "season_number": 1, "episodes": [tmdb_episode(1, 1, "One"), tmdb_episode(1, 2, "Two")] }),
                ),
            );
            routes.insert(
                "/3/tv/1399/season/2".into(),
                (
                    200,
                    json!({ "season_number": 2, "episodes": [
                        tmdb_episode(2, 1, "Three"), tmdb_episode(2, 2, "Four"), tmdb_episode(2, 3, "Five")
                    ] }),
                ),
            );
        }

        let first = series(&state, Some(1399), None)
            .await
            .expect("stored")
            .expect("a work");
        assert_eq!(first.episodes.len(), 5);
        assert_eq!(first.refresh_error, None);

        routes.lock().insert(
            "/3/tv/1399/season/2".into(),
            (500, json!({ "status_message": "boom" })),
        );
        let again = series(&state, Some(1399), None)
            .await
            .expect("stored")
            .expect("a work");

        assert_eq!(again.id, first.id);
        assert_eq!(
            numbers(&again),
            [(1, 1), (1, 2), (2, 1), (2, 2), (2, 3)],
            "season 2 stands as it was stored"
        );
        let error = again
            .refresh_error
            .as_deref()
            .expect("the failure, recorded");
        assert!(error.contains("season 2"), "{error}");
        assert!(retried_soon(&again), "{:?}", again.refresh_after);
    }

    /// TheTVDB failing while TMDB answers keeps TheTVDB's numbering: left
    /// alone, TMDB's list became the spine under a TheTVDB id.
    #[tokio::test]
    async fn the_numbering_stands_when_the_source_numbering_it_failed() {
        let (at, routes) = providers().await;
        let state = server(&at).await;
        let series_doc = json!({ "data": {
            "id": 81189, "name": "Breaking Bad", "status": { "name": "Ended" },
            "seasons": [{ "id": 1, "number": 1, "type": { "type": "official" } }],
            "remoteIds": [{ "id": "1396", "sourceName": "TheMovieDB.com" }]
        } });
        {
            let mut routes = routes.lock();
            routes.insert("/login".into(), (200, json!({ "data": { "token": "t" } })));
            routes.insert("/series/81189/extended".into(), (200, series_doc));
            routes.insert(
                "/series/81189/episodes/official/eng".into(),
                (
                    200,
                    json!({ "data": { "episodes": [
                        { "id": 11, "seasonNumber": 1, "number": 1, "name": "Pilot" },
                        { "id": 12, "seasonNumber": 1, "number": 2, "name": "Cat's in the Bag..." },
                        { "id": 13, "seasonNumber": 1, "number": 3, "name": "...And the Bag's in the River" }
                    ] }, "links": { "next": null } }),
                ),
            );
            // TMDB numbers the same episodes its own way.
            routes.insert(
                "/3/tv/1396".into(),
                (
                    200,
                    json!({ "id": 1396, "name": "Breaking Bad", "status": "Ended",
                            "external_ids": { "tvdb_id": 81189 },
                            "seasons": [{ "season_number": 1 }, { "season_number": 2 }] }),
                ),
            );
            routes.insert(
                "/3/tv/1396/season/1".into(),
                (
                    200,
                    json!({ "season_number": 1, "episodes": [tmdb_episode(1, 1, "Pilot"), tmdb_episode(1, 2, "Cat")] }),
                ),
            );
            routes.insert(
                "/3/tv/1396/season/2".into(),
                (
                    200,
                    json!({ "season_number": 2, "episodes": [tmdb_episode(2, 1, "River")] }),
                ),
            );
        }

        let first = series(&state, Some(1396), Some(81189))
            .await
            .expect("stored")
            .expect("a work");
        assert_eq!(numbers(&first), [(1, 1), (1, 2), (1, 3)], "TheTVDB's list");

        routes.lock().insert(
            "/series/81189/extended".into(),
            (500, json!({ "status": "failure" })),
        );
        let again = series(&state, Some(1396), Some(81189))
            .await
            .expect("stored")
            .expect("a work");

        assert_eq!(again.id, first.id);
        assert_eq!(
            numbers(&again),
            [(1, 1), (1, 2), (1, 3)],
            "still TheTVDB's numbering, not TMDB's"
        );
        assert_eq!(again.episodes[2].title, "...And the Bag's in the River");
        let error = again
            .refresh_error
            .as_deref()
            .expect("the failure, recorded");
        assert!(error.contains("tvdb"), "{error}");
        assert!(retried_soon(&again), "{:?}", again.refresh_after);
        let provenance = repo::item::provenance(&state.db, &again.id)
            .await
            .expect("read")
            .expect("recorded");
        assert_eq!(provenance.episodes.as_deref(), Some("tvdb"));

        // TheTVDB back: written as complete again, on its interval.
        routes.lock().insert(
            "/series/81189/extended".into(),
            (
                200,
                json!({ "data": {
                    "id": 81189, "name": "Breaking Bad", "status": { "name": "Ended" },
                    "seasons": [{ "id": 1, "number": 1, "type": { "type": "official" } }]
                } }),
            ),
        );
        let healed = series(&state, Some(1396), Some(81189))
            .await
            .expect("stored")
            .expect("a work");
        assert_eq!(healed.refresh_error, None);
        assert!(!retried_soon(&healed), "{:?}", healed.refresh_after);
    }

    /// The adult flag AniList gave stands while AniList does not answer: a
    /// failure is not a "no", and the work was written as for everyone.
    #[test]
    fn the_adult_flag_stands_when_the_source_that_says_so_failed() {
        let mut stored = MediaItem::empty(MediaKind::Series);
        stored.title = "A Work".into();
        stored.is_adult = true;
        let provenance = merge::provenance::attribute(
            &stored,
            &[
                merge::provenance::view(names::TMDB, &{
                    let mut tmdb = MediaItem::empty(MediaKind::Series);
                    tmdb.title = "A Work".into();
                    tmdb
                }),
                merge::provenance::view(names::ANILIST, &stored),
            ],
        );
        assert_eq!(provenance.fields["isAdult"].from, names::ANILIST);

        let failed = [Failure {
            provider: names::ANILIST,
            error: "AniList's rate limit was reached".into(),
        }];
        let mut tmdb = MediaItem::empty(MediaKind::Series);
        tmdb.title = "A Work".into();
        let mut contributions = vec![Contribution {
            provider: names::TMDB.into(),
            item: tmdb,
        }];
        hand_back(&mut contributions, &failed, &stored, &provenance, false);
        let merged =
            merge::combine(contributions, &["tmdb".into(), "anilist".into()]).expect("merged");
        assert!(merged.is_adult, "handed back from the provenance");

        // Recorded before provenance was: the stored flag stands all the same.
        let mut bare = MediaItem::empty(MediaKind::Series);
        bare.title = "A Work".into();
        keep_adult(&mut bare, &stored, &failed);
        assert!(bare.is_adult);

        // Answered, it is AniList's word that counts — and a source that is
        // no authority on it failing changes nothing.
        let mut answered = MediaItem::empty(MediaKind::Series);
        let other = [Failure {
            provider: names::FANART,
            error: "timed out".into(),
        }];
        keep_adult(&mut answered, &stored, &other);
        assert!(!answered.is_adult);
    }

    #[test]
    fn a_supplement_that_did_not_answer_is_kept_from_before_and_fails_nothing() {
        let principal = |provider: &str| !SUPPLEMENTS.contains(&provider);
        let tvmaze = [Failure {
            provider: names::TVMAZE,
            error: "timed out".into(),
        }];
        // What it gave before is kept, and the log says why…
        assert!(
            failure_note(&tvmaze, &[], |_| true)
                .is_some_and(|note| note.contains("tvmaze: timed out"))
        );
        // …but the work did not fail: no error on it, nobody told.
        assert_eq!(failure_note(&tvmaze, &[], principal), None);

        // A source the work is made from failing is the work's failure, and
        // it is said alone.
        let both = [
            Failure {
                provider: names::TVDB,
                error: "401 Unauthorized".into(),
            },
            Failure {
                provider: names::FANART,
                error: "timed out".into(),
            },
        ];
        let said = failure_note(&both, &[], principal).expect("TheTVDB's failure is the work's");
        assert!(said.contains("tvdb: 401 Unauthorized"), "{said}");
        assert!(!said.contains("fanart"), "{said}");
    }

    /// A season the provider lists again after a person added it by hand,
    /// and an episode likewise, stay as the person wrote them.
    #[tokio::test]
    async fn a_refresh_does_not_write_over_what_was_added_by_hand() {
        let db = &crate::db::testing::db().await;

        let mut work = MediaItem::empty(MediaKind::Series);
        work.id = crate::db::new_id();
        work.title = "Host".into();
        work.slug = "host".into();
        work.created_at = crate::db::now();
        work.updated_at = crate::db::now();
        work.episodes = vec![{
            let mut e = repo::child::blank_episode(1, 1);
            e.is_manual = false;
            e.title = "From The Provider".into();
            e
        }];
        repo::item::upsert(
            db,
            repo::item::ItemWrite {
                item: &work,
                replace_children: true,
            },
        )
        .await
        .expect("stored");

        let mut season = repo::child::blank_season(2);
        season.title = Some("Typed By Hand".into());
        repo::child::add_season(db, &work.id, &season)
            .await
            .expect("added");
        let mut episode = repo::child::blank_episode(1, 2);
        episode.title = "Typed By Hand".into();
        episode.overview = Some("What the person wrote".into());
        repo::child::add_episode(db, &work.id, &episode)
            .await
            .expect("added");

        // The provider now lists both numbers.
        let mut refreshed = work.clone();
        refreshed.seasons = vec![{
            let mut s = repo::child::blank_season(2);
            s.is_manual = false;
            s.title = Some("The Provider's".into());
            s
        }];
        refreshed.episodes.push({
            let mut e = repo::child::blank_episode(1, 2);
            e.is_manual = false;
            e.title = "The Provider's".into();
            e.overview = Some("What the provider wrote".into());
            e
        });
        repo::item::upsert(
            db,
            repo::item::ItemWrite {
                item: &refreshed,
                replace_children: true,
            },
        )
        .await
        .expect("refreshed");

        let mut read = repo::item::get(db, &work.id)
            .await
            .expect("read")
            .expect("held");
        repo::item::load_children(db, &mut read)
            .await
            .expect("children");
        let season = read
            .seasons
            .iter()
            .find(|s| s.season_number == 2)
            .expect("season 2");
        assert!(season.is_manual);
        assert_eq!(season.title.as_deref(), Some("Typed By Hand"));
        let episode = read
            .episodes
            .iter()
            .find(|e| e.season_number == 1 && e.episode_number == 2)
            .expect("1x02");
        assert!(episode.is_manual);
        assert_eq!(episode.title, "Typed By Hand");
        assert_eq!(episode.overview.as_deref(), Some("What the person wrote"));
        // And the provider's own row is still written as the provider says.
        assert_eq!(read.episodes[0].title, "From The Provider");
    }

    /// Breaking Bad as both sources number it: TheTVDB three episodes in its
    /// first season, TMDB two there and one in a second — and TheTVDB
    /// answering or not as `tvdb_up` says.
    fn breaking_bad(routes: &Routes, tvdb_up: bool) {
        let mut routes = routes.lock();
        routes.insert("/login".into(), (200, json!({ "data": { "token": "t" } })));
        routes.insert(
            "/series/81189/extended".into(),
            if tvdb_up {
                (
                    200,
                    json!({ "data": {
                        "id": 81189, "name": "Breaking Bad", "status": { "name": "Ended" },
                        "seasons": [{ "id": 1, "number": 1, "type": { "type": "official" } }],
                        "remoteIds": [{ "id": "1396", "sourceName": "TheMovieDB.com" }]
                    } }),
                )
            } else {
                (500, json!({ "status": "failure" }))
            },
        );
        routes.insert(
            "/series/81189/episodes/official/eng".into(),
            (
                200,
                json!({ "data": { "episodes": [
                    { "id": 11, "seasonNumber": 1, "number": 1, "name": "Pilot" },
                    { "id": 12, "seasonNumber": 1, "number": 2, "name": "Cat's in the Bag..." },
                    { "id": 13, "seasonNumber": 1, "number": 3, "name": "...And the Bag's in the River" }
                ] }, "links": { "next": null } }),
            ),
        );
        routes.insert(
            "/3/tv/1396".into(),
            (
                200,
                json!({ "id": 1396, "name": "Breaking Bad", "status": "Ended",
                        "external_ids": { "tvdb_id": 81189 },
                        "seasons": [{ "season_number": 1 }, { "season_number": 2 }],
                        "episode_groups": { "results": [] } }),
            ),
        );
        routes.insert(
            "/3/tv/1396/season/1".into(),
            (
                200,
                json!({ "season_number": 1, "episodes": [tmdb_episode(1, 1, "Pilot"), tmdb_episode(1, 2, "Cat")] }),
            ),
        );
        routes.insert(
            "/3/tv/1396/season/2".into(),
            (
                200,
                json!({ "season_number": 2, "episodes": [tmdb_episode(2, 1, "River")] }),
            ),
        );
    }

    async fn refreshed(state: &AppState) -> MediaItem {
        series(state, Some(1396), Some(81189))
            .await
            .expect("stored")
            .expect("a work")
    }

    /// The last silent refresh counted as a day ago: the next one counts.
    async fn a_day_on(state: &AppState, work: &MediaItem) {
        let mut held = repo::order::numbering(&state.db, &work.id)
            .await
            .expect("read");
        held.silent_at = Some(crate::db::to_rfc3339(Utc::now() - TimeDelta::hours(21)));
        repo::order::set_numbering(&state.db, &work.id, &held)
            .await
            .expect("kept");
    }

    async fn trail(state: &AppState) -> Vec<String> {
        repo::audit::list(
            &state.db,
            &repo::audit::Query {
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .expect("the trail")
        .into_iter()
        .map(|entry| entry.action)
        .collect()
    }

    /// TheTVDB silent on refresh after refresh, a day apart: TMDB numbers
    /// the series once it has been silent for as many as the setting says,
    /// each episode keeping the TheTVDB id of the one its numbers were;
    /// TheTVDB answering again, it takes its numbering back.
    #[tokio::test]
    async fn tmdb_numbers_a_series_thetvdb_stopped_answering_for_until_it_answers_again() {
        let (at, routes) = providers().await;
        let state = server(&at).await;
        state
            .settings
            .set(
                crate::settings::registry::Scope::Server,
                "",
                "numbering.fallbackAfter",
                "2",
                None,
            )
            .await
            .expect("set");
        breaking_bad(&routes, true);
        let first = refreshed(&state).await;
        assert_eq!(numbers(&first), [(1, 1), (1, 2), (1, 3)], "TheTVDB's list");

        breaking_bad(&routes, false);
        let silent = refreshed(&state).await;
        assert_eq!(numbers(&silent), [(1, 1), (1, 2), (1, 3)], "kept, the once");
        assert_eq!(
            repo::order::numbering(&state.db, &first.id)
                .await
                .unwrap()
                .silent,
            1
        );
        // Asked again within the hour: the same silence, counted once.
        refreshed(&state).await;
        let held = repo::order::numbering(&state.db, &first.id).await.unwrap();
        assert_eq!((held.silent, held.since.as_deref()), (1, None));

        a_day_on(&state, &first).await;
        let fallen = refreshed(&state).await;
        assert_eq!(fallen.id, first.id);
        assert_eq!(
            numbers(&fallen),
            [(1, 1), (1, 2), (2, 1)],
            "TMDB's numbering"
        );
        assert_eq!(
            fallen
                .episodes
                .iter()
                .map(|e| e.tvdb_id)
                .collect::<Vec<_>>(),
            [Some(11), Some(12), None],
            "the TheTVDB ids of the episodes of the same numbers"
        );
        let held = repo::order::numbering(&state.db, &first.id).await.unwrap();
        assert_eq!(held.silent, 2);
        assert!(held.since.is_some());
        let provenance = repo::item::provenance(&state.db, &first.id)
            .await
            .unwrap()
            .expect("recorded");
        assert_eq!(provenance.episodes.as_deref(), Some("tmdb"));
        let orders = repo::order::list(&state.db, &first.id).await.unwrap();
        assert_eq!(orders::aired(&orders).unwrap().source, "tmdb");
        assert!(
            trail(&state)
                .await
                .iter()
                .any(|a| a == "item.numbering_fallback")
        );

        // Still silent: still TMDB's.
        a_day_on(&state, &first).await;
        let still = refreshed(&state).await;
        assert_eq!(numbers(&still), [(1, 1), (1, 2), (2, 1)]);

        // TheTVDB back: its numbering back, and nothing held of the silence.
        breaking_bad(&routes, true);
        let back = refreshed(&state).await;
        assert_eq!(numbers(&back), [(1, 1), (1, 2), (1, 3)]);
        assert_eq!(
            repo::order::numbering(&state.db, &first.id).await.unwrap(),
            repo::order::Numbering::default()
        );
        assert!(
            trail(&state)
                .await
                .iter()
                .any(|a| a == "item.numbering_restored")
        );
        let orders = repo::order::list(&state.db, &first.id).await.unwrap();
        assert_eq!(orders::aired(&orders).unwrap().source, "tvdb");
    }

    /// An order frozen while TMDB numbers the series keeps TMDB's numbering
    /// when TheTVDB answers again; given back by hand — the silence cleared,
    /// as the button does — TheTVDB numbers it again all the same. And never
    /// at all with the setting at zero.
    #[tokio::test]
    async fn a_frozen_numbering_stays_with_tmdb_until_it_is_given_back_by_hand() {
        let (at, routes) = providers().await;
        let state = server(&at).await;
        let set = |value: &'static str| {
            let state = state.clone();
            async move {
                state
                    .settings
                    .set(
                        crate::settings::registry::Scope::Server,
                        "",
                        "numbering.fallbackAfter",
                        value,
                        None,
                    )
                    .await
                    .expect("set");
            }
        };
        breaking_bad(&routes, true);
        let first = refreshed(&state).await;

        // Zero: TheTVDB's numbering stands, however long it is silent.
        set("0").await;
        breaking_bad(&routes, false);
        for _ in 0..3 {
            a_day_on(&state, &first).await;
            let silent = refreshed(&state).await;
            assert_eq!(numbers(&silent), [(1, 1), (1, 2), (1, 3)]);
        }

        set("1").await;
        a_day_on(&state, &first).await;
        let fallen = refreshed(&state).await;
        assert_eq!(numbers(&fallen), [(1, 1), (1, 2), (2, 1)]);

        // Somebody freezes the numbering TMDB gave.
        let mine = EpisodeOrderFixture::from_aired(&fallen);
        repo::order::create(&state.db, &fallen.id, &mine)
            .await
            .expect("made");
        assert!(
            repo::order::freeze(&state.db, &fallen.id, Some(&mine.key))
                .await
                .expect("frozen")
        );

        breaking_bad(&routes, true);
        let kept = refreshed(&state).await;
        assert_eq!(numbers(&kept), [(1, 1), (1, 2), (2, 1)], "still TMDB's");
        assert!(
            repo::order::numbering(&state.db, &first.id)
                .await
                .unwrap()
                .since
                .is_some()
        );

        // Given back by hand.
        repo::order::set_numbering(&state.db, &first.id, &repo::order::Numbering::default())
            .await
            .expect("cleared");
        let back = refreshed(&state).await;
        assert_eq!(numbers(&back), [(1, 1), (1, 2), (1, 3)]);
    }

    /// An order made from the aired one, as the editor makes one.
    struct EpisodeOrderFixture;

    impl EpisodeOrderFixture {
        fn from_aired(item: &MediaItem) -> crate::domain::EpisodeOrder {
            crate::domain::EpisodeOrder {
                key: "ams-mine".into(),
                kind: OrderKind::Custom,
                source: repo::order::LOCAL.into(),
                source_ref: None,
                label: "Mine".into(),
                is_default: false,
                is_frozen: false,
                announced: None,
                updated_at: crate::db::now(),
                episodes: orders::places_from(item, None),
            }
        }
    }

    /// TMDB's group of the original air date numbers the series when it
    /// numbers the same episodes, each once; one that adds an episode is
    /// someone's other order, and TMDB's own seasons stand.
    #[tokio::test]
    async fn tmdbs_group_of_the_original_air_date_numbers_the_same_episodes_or_nothing() {
        let (at, routes) = providers().await;
        let state = server(&at).await;
        breaking_bad(&routes, true);
        let tmdb_episode_with_id = |season: i32, number: i32, id: i64, name: &str| {
            let mut episode = tmdb_episode(season, number, name);
            episode["id"] = json!(id);
            episode
        };
        {
            let mut routes = routes.lock();
            routes.insert(
                "/3/tv/1396/season/1".into(),
                (
                    200,
                    json!({ "season_number": 1, "episodes": [
                        tmdb_episode_with_id(1, 1, 101, "Pilot"),
                        tmdb_episode_with_id(1, 2, 102, "Cat")
                    ] }),
                ),
            );
            routes.insert(
                "/3/tv/1396/season/2".into(),
                (
                    200,
                    json!({ "season_number": 2, "episodes": [tmdb_episode_with_id(2, 1, 103, "River")] }),
                ),
            );
        }
        let group = |episodes: Value| json!({ "id": "g1", "type": 1, "groups": [{ "order": 1, "episodes": episodes }] });
        let entry = |id: i64, order: i32| {
            let mut episode = tmdb_episode(1, 1, "x");
            episode["id"] = json!(id);
            episode["order"] = json!(order);
            episode
        };

        // TMDB's one season of the original air date: the same three.
        let mut answers = vec![Answer::new(
            names::TMDB,
            json!({ "episode_groups": { "results": [
                { "id": "g1", "name": "Original air date", "type": 1, "episode_count": 3 }
            ] } }),
            {
                let mut item = MediaItem::empty(MediaKind::Series);
                for (season, number, id) in [(1, 1, 101), (1, 2, 102), (2, 1, 103)] {
                    let mut episode = repo::child::blank_episode(season, number);
                    episode.tmdb_id = Some(id);
                    episode.is_manual = false;
                    item.episodes.push(episode);
                }
                item
            },
        )];
        // Read off TMDB's answer before it is let go of, as `store` does.
        let groups = tmdb_map::episode_groups(&answers[0].payload);
        routes.lock().insert(
            "/3/tv/episode_group/g1".into(),
            (
                200,
                group(json!([entry(101, 0), entry(102, 1), entry(103, 2)])),
            ),
        );
        number_by_air_date_group(&state, &mut answers, groups.clone(), &[]).await;
        assert_eq!(numbers(&answers[0].item), [(1, 1), (1, 2), (1, 3)]);
        assert_eq!(
            answers[0]
                .item
                .seasons
                .iter()
                .map(|s| s.season_number)
                .collect::<Vec<_>>(),
            [1]
        );

        // One that adds an episode: TMDB's own seasons stand.
        for episode in &mut answers[0].item.episodes {
            if episode.tmdb_id == Some(103) {
                episode.season_number = 2;
                episode.episode_number = 1;
            }
        }
        routes.lock().insert(
            "/3/tv/episode_group/g1".into(),
            (
                200,
                group(json!([
                    entry(101, 0),
                    entry(102, 1),
                    entry(103, 2),
                    entry(104, 3)
                ])),
            ),
        );
        number_by_air_date_group(&state, &mut answers, groups.clone(), &[]).await;
        assert_eq!(numbers(&answers[0].item), [(1, 1), (1, 2), (2, 1)]);

        // The same three again, for a series known as TMDB's own seasons cut
        // it: they stand. Known as the group cuts it, the group numbers it.
        routes.lock().insert(
            "/3/tv/episode_group/g1".into(),
            (
                200,
                group(json!([entry(101, 0), entry(102, 1), entry(103, 2)])),
            ),
        );
        let known = |at: [(i32, i32, i64); 3]| {
            at.map(|(season, number, id)| {
                let mut episode = repo::child::blank_episode(season, number);
                episode.tmdb_id = Some(id);
                episode
            })
        };
        let as_seasons = known([(1, 1, 101), (1, 2, 102), (2, 1, 103)]);
        number_by_air_date_group(&state, &mut answers, groups.clone(), &as_seasons).await;
        assert_eq!(numbers(&answers[0].item), [(1, 1), (1, 2), (2, 1)]);
        let as_group = known([(1, 1, 101), (1, 2, 102), (1, 3, 103)]);
        number_by_air_date_group(&state, &mut answers, groups, &as_group).await;
        assert_eq!(numbers(&answers[0].item), [(1, 1), (1, 2), (1, 3)]);
    }

    #[test]
    fn an_episode_tmdb_numbers_otherwise_keeps_its_thetvdb_id_by_its_tmdb_id() {
        let episode = |season: i32, number: i32, tmdb: Option<i64>, tvdb: Option<i64>| {
            let mut episode = repo::child::blank_episode(season, number);
            episode.tmdb_id = tmdb;
            episode.tvdb_id = tvdb;
            episode
        };
        // TheTVDB's fifth season, as the work held it.
        let stored = [
            episode(5, 1, Some(501), Some(9001)),
            episode(5, 2, Some(502), Some(9002)),
            episode(5, 3, None, Some(9003)),
        ];
        // TMDB cuts it otherwise: the first two in a sixth season, and an
        // episode of its own where TheTVDB's first was.
        let mut tmdb = [
            episode(6, 1, Some(501), None),
            episode(6, 2, Some(502), None),
            episode(5, 3, None, None),
            episode(5, 1, Some(777), None),
        ];
        keep_tvdb_ids(&mut tmdb, &stored, &stored);
        assert_eq!(
            tmdb.iter().map(|e| e.tvdb_id).collect::<Vec<_>>(),
            [Some(9001), Some(9002), Some(9003), None]
        );
    }

    /// *Breaking Bad* dated and with TMDB's ids: TheTVDB three episodes in
    /// its first season, TMDB the third in a second season of its own.
    fn breaking_bad_dated(routes: &Routes, tmdb_second_season: (u16, Value)) {
        breaking_bad(routes, true);
        let tmdb_episode_on = |season: i32, number: i32, id: i64, aired: &str, name: &str| {
            let mut episode = tmdb_episode(season, number, name);
            episode["id"] = json!(id);
            episode["air_date"] = json!(aired);
            episode
        };
        let mut routes = routes.lock();
        routes.insert(
            "/series/81189/episodes/official/eng".into(),
            (
                200,
                json!({ "data": { "episodes": [
                    { "id": 11, "seasonNumber": 1, "number": 1, "name": "Pilot", "aired": "2008-01-20" },
                    { "id": 12, "seasonNumber": 1, "number": 2, "name": "Cat's in the Bag...", "aired": "2008-01-27" },
                    { "id": 13, "seasonNumber": 1, "number": 3, "name": "...And the Bag's in the River", "aired": "2008-02-10" }
                ] }, "links": { "next": null } }),
            ),
        );
        routes.insert(
            "/3/tv/1396/season/1".into(),
            (
                200,
                json!({ "season_number": 1, "episodes": [
                    tmdb_episode_on(1, 1, 62085, "2008-01-20", "Pilot"),
                    tmdb_episode_on(1, 2, 62086, "2008-01-27", "Cat")
                ] }),
            ),
        );
        let second = match tmdb_second_season {
            (200, _) => (
                200,
                json!({ "season_number": 2, "episodes": [
                    tmdb_episode_on(2, 1, 62087, "2008-02-10", "River")
                ] }),
            ),
            failed => failed,
        };
        routes.insert("/3/tv/1396/season/2".into(), second);
    }

    /// TMDB's own numbering is kept as an order at every refresh that read
    /// TMDB's seasons, beside TheTVDB's numbering, which still numbers the
    /// series: each place at TMDB's numbers names the episode the series
    /// holds for it — TMDB's second season's first is TheTVDB's third, aired
    /// the same day — and a season TMDB could not give the next time keeps
    /// the places kept for it, the order frozen still.
    #[tokio::test]
    async fn tmdbs_own_numbering_is_kept_beside_thetvdbs_at_every_refresh() {
        let (at, routes) = providers().await;
        let state = server(&at).await;
        breaking_bad_dated(&routes, (200, Value::Null));

        let first = refreshed(&state).await;
        assert_eq!(numbers(&first), [(1, 1), (1, 2), (1, 3)], "TheTVDB's list");
        let provenance = repo::item::provenance(&state.db, &first.id)
            .await
            .unwrap()
            .expect("recorded");
        assert_eq!(provenance.episodes.as_deref(), Some("tvdb"));

        let orders = repo::order::list(&state.db, &first.id).await.unwrap();
        let own = orders::named(&orders, orders::TMDB_AIRED).expect("kept");
        assert_eq!(own.source, "tmdb");
        assert_eq!(own.kind, OrderKind::Aired);
        // Where, by TheTVDB's id and TMDB's, and named how.
        type Seen = (i32, i32, Option<i64>, Option<i64>, String);
        let placed: Vec<Seen> = orders::placements(&first, own)
            .into_iter()
            .map(|p| {
                (
                    p.season_number,
                    p.episode_number,
                    p.episode.tvdb_id,
                    p.tmdb_id,
                    p.episode.title.clone(),
                )
            })
            .collect();
        assert_eq!(
            placed,
            [
                (1, 1, Some(11), Some(62085), "Pilot".to_string()),
                (
                    1,
                    2,
                    Some(12),
                    Some(62086),
                    "Cat's in the Bag...".to_string()
                ),
                (
                    2,
                    1,
                    Some(13),
                    Some(62087),
                    "...And the Bag's in the River".to_string()
                ),
            ]
        );
        // Offered where TMDB's numbering is: the site and Plex.
        assert!(
            orders::offered(&first, &orders)
                .iter()
                .any(|o| o.key == orders::TMDB_AIRED)
        );

        // Frozen as the aired numbering, then refreshed with TMDB's second
        // season unanswered: frozen still, and that season's place kept.
        assert!(
            repo::order::freeze(&state.db, &first.id, Some(orders::TMDB_AIRED))
                .await
                .unwrap()
        );
        breaking_bad_dated(&routes, (500, json!({ "status_message": "boom" })));
        let again = refreshed(&state).await;
        assert_eq!(again.id, first.id);
        let kept = repo::order::get(&state.db, &first.id, orders::TMDB_AIRED)
            .await
            .unwrap()
            .expect("kept");
        assert!(kept.is_frozen);
        assert_eq!(
            kept.episodes
                .iter()
                .map(|p| (p.season_number, p.episode_number, p.tmdb_id, p.tvdb_id))
                .collect::<Vec<_>>(),
            [
                (1, 1, Some(62085), Some(11)),
                (1, 2, Some(62086), Some(12)),
                (2, 1, Some(62087), Some(13)),
            ]
        );
    }

    // ─── a series TheTVDB no longer knows ───────────────────────────────────

    /// A series both sources number, dated, under ids of its own: TheTVDB
    /// three episodes in its first season, TMDB the third in a second season
    /// of its own.
    struct Show {
        tvdb: i64,
        tmdb: i64,
        title: &'static str,
    }

    /// What TheTVDB answers for a [`Show`].
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Tvdb {
        /// The series.
        Up,
        /// 404: it has no such series.
        Gone,
    }

    impl Show {
        /// The routes that answer for it: TheTVDB as `tvdb` says, TMDB with
        /// the episodes `since` after its three.
        fn answer(&self, routes: &Routes, tvdb: Tvdb, since: &[(i32, i32, &str, &str)]) {
            let mut routes = routes.lock();
            routes.insert("/login".into(), (200, json!({ "data": { "token": "t" } })));
            let document = format!("/series/{}/extended", self.tvdb);
            match tvdb {
                Tvdb::Up => {
                    routes.insert(
                        document,
                        (
                            200,
                            json!({ "data": {
                                "id": self.tvdb, "name": self.title, "status": { "name": "Continuing" },
                                "seasons": [{ "id": 1, "number": 1, "type": { "type": "official" } }],
                                "remoteIds": [{ "id": self.tmdb.to_string(), "sourceName": "TheMovieDB.com" }]
                            } }),
                        ),
                    );
                }
                Tvdb::Gone => {
                    routes.remove(&document);
                }
            }
            let tvdb_id = |n: i64| self.tvdb * 10 + n;
            routes.insert(
                format!("/series/{}/episodes/official/eng", self.tvdb),
                (
                    200,
                    json!({ "data": { "episodes": [
                        { "id": tvdb_id(1), "seasonNumber": 1, "number": 1, "name": "Pilot", "aired": "2008-01-20" },
                        { "id": tvdb_id(2), "seasonNumber": 1, "number": 2, "name": "Second", "aired": "2008-01-27" },
                        { "id": tvdb_id(3), "seasonNumber": 1, "number": 3, "name": "Third", "aired": "2008-02-10" }
                    ] }, "links": { "next": null } }),
                ),
            );
            let mut listed: Vec<i32> = vec![1, 2];
            listed.extend(since.iter().map(|(season, ..)| *season));
            listed.sort_unstable();
            listed.dedup();
            let seasons_listed: Vec<Value> = listed
                .iter()
                .map(|season| json!({ "season_number": season }))
                .collect();
            routes.insert(
                format!("/3/tv/{}", self.tmdb),
                (
                    200,
                    json!({ "id": self.tmdb, "name": self.title, "status": "Returning Series",
                            "external_ids": { "tvdb_id": self.tvdb },
                            "seasons": seasons_listed,
                            "episode_groups": { "results": [] } }),
                ),
            );
            let episode = |season: i32, number: i32, name: &str, aired: &str| {
                let mut episode = tmdb_episode(season, number, name);
                episode["id"] = json!(self.tmdb * 10 + i64::from(season * 100 + number));
                episode["air_date"] = json!(aired);
                episode
            };
            let mut seasons: std::collections::BTreeMap<i32, Vec<Value>> = Default::default();
            seasons.entry(1).or_default().extend([
                episode(1, 1, "Pilot", "2008-01-20"),
                episode(1, 2, "Second", "2008-01-27"),
            ]);
            seasons
                .entry(2)
                .or_default()
                .push(episode(2, 1, "Third", "2008-02-10"));
            for (season, number, name, aired) in since {
                seasons
                    .entry(*season)
                    .or_default()
                    .push(episode(*season, *number, name, aired));
            }
            for (season, episodes) in seasons {
                routes.insert(
                    format!("/3/tv/{}/season/{season}", self.tmdb),
                    (
                        200,
                        json!({ "season_number": season, "episodes": episodes }),
                    ),
                );
            }
        }

        async fn refreshed(&self, state: &AppState) -> MediaItem {
            series(state, Some(self.tmdb), Some(self.tvdb))
                .await
                .expect("stored")
                .expect("a work")
        }
    }

    /// The last 404 TheTVDB answered counted as a day ago: the next counts.
    async fn gone_a_day_ago(state: &AppState, work: &MediaItem) {
        let mut held = repo::order::numbering(&state.db, &work.id)
            .await
            .expect("read");
        held.gone_at = Some(crate::db::to_rfc3339(Utc::now() - TimeDelta::hours(21)));
        repo::order::set_numbering(&state.db, &work.id, &held)
            .await
            .expect("kept");
    }

    /// What the aired order answers with: the order frozen as it, served,
    /// or the work's own episodes; by numbers and title.
    async fn served_as_aired(state: &AppState, work: &MediaItem) -> Vec<(i32, i32, String)> {
        let all = repo::order::list(&state.db, &work.id).await.expect("read");
        match orders::as_official(work, &all) {
            Some(placed) => placed
                .into_iter()
                .map(|p| (p.season_number, p.episode_number, p.episode.title.clone()))
                .collect(),
            None => work
                .episodes
                .iter()
                .map(|e| (e.season_number, e.episode_number, e.title.clone()))
                .collect(),
        }
    }

    /// The defect wave P found: TheTVDB answering 404 was neither an answer
    /// nor a failure, and with Skyhook silent too the merge took TMDB's list
    /// as the numbering — TMDB's second season under a TheTVDB id, without a
    /// word. Now it stands for a failure: TheTVDB's numbering stands, the
    /// work keeps its TheTVDB id, and the refresh says why it is in part.
    #[tokio::test]
    async fn a_series_thetvdb_has_no_record_of_keeps_thetvdbs_numbering() {
        let (at, routes) = providers().await;
        let state = server(&at).await;
        let show = Show {
            tvdb: 770_001,
            tmdb: 770_101,
            title: "Gone Once P4",
        };
        show.answer(&routes, Tvdb::Up, &[]);
        let first = show.refreshed(&state).await;
        assert_eq!(numbers(&first), [(1, 1), (1, 2), (1, 3)], "TheTVDB's list");

        show.answer(&routes, Tvdb::Gone, &[]);
        let again = show.refreshed(&state).await;
        assert_eq!(again.id, first.id);
        // TMDB numbers it otherwise, which is what it was served as before.
        let all = repo::order::list(&state.db, &again.id).await.unwrap();
        let tmdbs: Vec<(i32, i32)> = orders::named(&all, orders::TMDB_AIRED)
            .expect("TMDB's own numbering")
            .episodes
            .iter()
            .map(|p| (p.season_number, p.episode_number))
            .collect();
        assert_eq!(tmdbs, [(1, 1), (1, 2), (2, 1)]);
        assert_eq!(
            numbers(&again),
            [(1, 1), (1, 2), (1, 3)],
            "TheTVDB's numbering, not TMDB's"
        );
        assert_eq!(again.external_ids.tvdb, Some(770_001));
        let error = again.refresh_error.as_deref().expect("said");
        assert!(error.contains("TheTVDB has no series 770001"), "{error}");
        assert!(retried_soon(&again), "{:?}", again.refresh_after);
        let provenance = repo::item::provenance(&state.db, &again.id)
            .await
            .unwrap()
            .expect("recorded");
        assert_eq!(provenance.episodes.as_deref(), Some("tvdb"));
        let held = repo::order::numbering(&state.db, &again.id).await.unwrap();
        assert_eq!((held.gone, held.dropped.as_deref()), (1, None));
        assert!(orders::named(&all, orders::TVDB_LAST).is_none());

        // Asked again within the hour: the same 404, counted once.
        show.refreshed(&state).await;
        let held = repo::order::numbering(&state.db, &again.id).await.unwrap();
        assert_eq!((held.gone, held.dropped.as_deref()), (1, None));
    }

    /// TheTVDB answering 404 at two refreshes a day apart: the numbering the
    /// series was served by is kept and frozen as the aired numbering, TMDB
    /// numbers the work meanwhile, and an episode aired since goes on after
    /// TheTVDB's last — in its season, since TMDB's second season holds it.
    /// The trail says so. TheTVDB knowing the series again takes it back.
    #[tokio::test]
    async fn a_series_thetvdb_no_longer_knows_keeps_its_numbering_and_what_airs_follows() {
        let (at, routes) = providers().await;
        let state = server(&at).await;
        let show = Show {
            tvdb: 770_002,
            tmdb: 770_102,
            title: "Dropped Twice P4",
        };
        show.answer(&routes, Tvdb::Up, &[]);
        let first = show.refreshed(&state).await;
        let theirs = vec![
            (1, 1, "Pilot".to_string()),
            (1, 2, "Second".to_string()),
            (1, 3, "Third".to_string()),
        ];
        assert_eq!(served_as_aired(&state, &first).await, theirs);

        show.answer(&routes, Tvdb::Gone, &[]);
        show.refreshed(&state).await;
        gone_a_day_ago(&state, &first).await;
        let dropped = show.refreshed(&state).await;
        assert_eq!(dropped.id, first.id);
        assert_eq!(dropped.external_ids.tvdb, Some(770_002));
        assert_eq!(numbers(&dropped), [(1, 1), (1, 2), (2, 1)], "TMDB's");
        assert_eq!(dropped.refresh_error, None, "the way of things now");
        let held = repo::order::numbering(&state.db, &first.id).await.unwrap();
        assert_eq!(held.gone, 2);
        assert!(held.dropped.is_some());
        assert_eq!(
            held.dropped_why,
            Some(crate::domain::orders::TvdbDropped::Missing)
        );
        let all = repo::order::list(&state.db, &first.id).await.unwrap();
        let kept = orders::named(&all, orders::TVDB_LAST).expect("kept");
        assert!(kept.is_frozen);
        assert_eq!(
            served_as_aired(&state, &dropped).await,
            theirs,
            "served as TheTVDB numbered it"
        );
        assert!(
            trail(&state)
                .await
                .iter()
                .any(|a| a == "item.numbering_fallback")
        );

        // An episode aired since, in TMDB's second season.
        show.answer(&routes, Tvdb::Gone, &[(2, 2, "Fourth", "2008-03-02")]);
        let since = show.refreshed(&state).await;
        assert_eq!(
            numbers(&since),
            [(1, 1), (1, 2), (2, 1), (2, 2)],
            "TMDB's own"
        );
        let mut more = theirs.clone();
        more.push((1, 4, "Fourth".to_string()));
        assert_eq!(served_as_aired(&state, &since).await, more);
        // Placed once, it stays at the next refresh.
        let again = show.refreshed(&state).await;
        assert_eq!(served_as_aired(&state, &again).await, more);

        // TheTVDB knows the series again: its numbering back, nothing kept.
        show.answer(&routes, Tvdb::Up, &[(2, 2, "Fourth", "2008-03-02")]);
        let back = show.refreshed(&state).await;
        assert_eq!(numbers(&back), [(1, 1), (1, 2), (1, 3)]);
        assert_eq!(
            repo::order::numbering(&state.db, &first.id).await.unwrap(),
            repo::order::Numbering::default()
        );
        let all = repo::order::list(&state.db, &first.id).await.unwrap();
        assert!(orders::named(&all, orders::TVDB_LAST).is_none());
        assert_eq!(orders::aired(&all).unwrap().source, "tvdb");
        assert!(
            trail(&state)
                .await
                .iter()
                .any(|a| a == "item.numbering_restored")
        );
    }

    /// `numbering.fallbackAfter` at 0: TheTVDB's numbering stands however
    /// often TheTVDB answers 404, and is never frozen.
    #[tokio::test]
    async fn no_fallback_keeps_thetvdbs_numbering_and_freezes_nothing() {
        let (at, routes) = providers().await;
        let state = server(&at).await;
        state
            .settings
            .set(
                crate::settings::registry::Scope::Server,
                "",
                "numbering.fallbackAfter",
                "0",
                None,
            )
            .await
            .expect("set");
        let show = Show {
            tvdb: 770_003,
            tmdb: 770_103,
            title: "Never Dropped P4",
        };
        show.answer(&routes, Tvdb::Up, &[]);
        let first = show.refreshed(&state).await;
        show.answer(&routes, Tvdb::Gone, &[]);
        for _ in 0..3 {
            gone_a_day_ago(&state, &first).await;
            let gone = show.refreshed(&state).await;
            assert_eq!(numbers(&gone), [(1, 1), (1, 2), (1, 3)]);
        }
        let held = repo::order::numbering(&state.db, &first.id).await.unwrap();
        assert_eq!((held.gone, held.dropped.as_deref()), (3, None));
        let all = repo::order::list(&state.db, &first.id).await.unwrap();
        assert!(orders::named(&all, orders::TVDB_LAST).is_none());
    }

    /// TheTVDB's feed says it merged the series into another: the numbering
    /// is kept as it was served, the work is read under the id TheTVDB kept
    /// at once, takes it, and answers to the one Sonarr holds it by; TheTVDB
    /// answering for it under the new id does not give the numbering back.
    #[tokio::test]
    async fn a_series_thetvdb_merged_into_another_is_served_under_both_ids() {
        let (at, routes) = providers().await;
        let state = server(&at).await;
        let show = Show {
            tvdb: 770_004,
            tmdb: 770_104,
            title: "Merged Away P4",
        };
        show.answer(&routes, Tvdb::Up, &[]);
        let first = show.refreshed(&state).await;
        let theirs = served_as_aired(&state, &first).await;

        // TheTVDB merges it into 770005, which numbers it in two seasons.
        show.answer(&routes, Tvdb::Gone, &[]);
        {
            let mut routes = routes.lock();
            routes.insert(
                "/updates".into(),
                (
                    200,
                    json!({ "data": [
                        { "entityType": "series", "methodInt": 3, "method": "delete",
                          "recordId": 770_004, "timeStamp": Utc::now().timestamp(),
                          "mergeToId": 770_005, "mergeToEntityType": "series" }
                    ], "links": { "next": null } }),
                ),
            );
            routes.insert(
                "/series/770005/extended".into(),
                (
                    200,
                    json!({ "data": {
                        "id": 770_005, "name": "Merged Away P4", "status": { "name": "Continuing" },
                        "seasons": [{ "id": 1, "number": 1, "type": { "type": "official" } },
                                    { "id": 2, "number": 2, "type": { "type": "official" } }],
                        "remoteIds": [{ "id": "770104", "sourceName": "TheMovieDB.com" }]
                    } }),
                ),
            );
            routes.insert(
                "/series/770005/episodes/official/eng".into(),
                (
                    200,
                    json!({ "data": { "episodes": [
                        { "id": 7_700_051, "seasonNumber": 1, "number": 1, "name": "Pilot", "aired": "2008-01-20" },
                        { "id": 7_700_052, "seasonNumber": 1, "number": 2, "name": "Second", "aired": "2008-01-27" },
                        { "id": 7_700_053, "seasonNumber": 2, "number": 1, "name": "Third", "aired": "2008-02-10" },
                        { "id": 7_700_054, "seasonNumber": 2, "number": 2, "name": "Fourth", "aired": "2008-03-02" }
                    ] }, "links": { "next": null } }),
                ),
            );
        }

        let said = numbering::watch(&state).await.expect("read");
        assert!(said.contains("Merged Away P4"), "{said}");
        assert!(said.contains("merged into 770005"), "{said}");

        let work = repo::item::get(&state.db, &first.id)
            .await
            .unwrap()
            .expect("held");
        assert_eq!(work.external_ids.tvdb, Some(770_005), "the id TheTVDB kept");
        let mut held_work = work.clone();
        repo::item::load_children(&state.db, &mut held_work)
            .await
            .unwrap();
        assert_eq!(
            numbers(&held_work),
            [(1, 1), (1, 2), (2, 1), (2, 2)],
            "TheTVDB's numbering under 770005"
        );
        // Served as it was, and the episode aired since after it.
        let mut more = theirs.clone();
        more.push((1, 4, "Fourth".to_string()));
        assert_eq!(served_as_aired(&state, &held_work).await, more);

        // Both ids answer with it.
        let by_old = crate::service::series::by_tvdb_id(&state, 770_004)
            .await
            .unwrap()
            .expect("answered");
        assert_eq!(by_old.id, first.id);
        let by_new = crate::service::series::by_tvdb_id(&state, 770_005)
            .await
            .unwrap()
            .expect("answered");
        assert_eq!(by_new.id, first.id);

        // TheTVDB answers for it under 770005: the numbering stays kept, and
        // the work keeps 770005 though TMDB still says 770004.
        let again = series(&state, Some(770_104), Some(770_005))
            .await
            .unwrap()
            .expect("a work");
        assert_eq!(again.external_ids.tvdb, Some(770_005));
        assert_eq!(served_as_aired(&state, &again).await, more);
        let held = repo::order::numbering(&state.db, &first.id).await.unwrap();
        assert_eq!(held.merged_into, Some(770_005));
        assert!(held.dropped.is_some());

        // TheTVDB failing for it under 770005: it keeps 770005 all the same,
        // TMDB's 770004 notwithstanding.
        routes.lock().insert(
            "/series/770005/extended".into(),
            (500, json!({ "status": "failure" })),
        );
        let silent = series(&state, Some(770_104), Some(770_005))
            .await
            .unwrap()
            .expect("a work");
        assert_eq!(silent.id, first.id);
        assert_eq!(silent.external_ids.tvdb, Some(770_005));
        assert_eq!(served_as_aired(&state, &silent).await, more);

        // Read again: kept once, and said so — the id deleted being one the
        // work only answers to now.
        let said = numbering::watch(&state).await.expect("read");
        assert!(said.contains("1 held here numbered so already"), "{said}");
        assert!(!said.contains("none held here"), "{said}");
    }

    /// TheTVDB's feed merges a series nothing else names — no TMDB id — into
    /// another: it stays one work, which takes the id TheTVDB kept and
    /// answers to the one it had, rather than a second work under the new
    /// one, which the TheTVDB ids alone could not tell for the same.
    #[tokio::test]
    async fn a_series_without_a_tmdb_id_thetvdb_merged_into_another_stays_one_work() {
        let (at, routes) = providers().await;
        let state = server(&at).await;
        let series_doc = |id: i64| {
            json!({ "data": {
                "id": id, "name": "Merged Without TMDB P4", "status": { "name": "Continuing" },
                "seasons": [{ "id": 1, "number": 1, "type": { "type": "official" } }],
                "remoteIds": [{ "id": "tt7700110", "sourceName": "IMDB" }]
            } })
        };
        let episodes = |base: i64, more: bool| {
            let mut list = vec![
                json!({ "id": base + 1, "seasonNumber": 1, "number": 1, "name": "Pilot", "aired": "2009-01-20" }),
                json!({ "id": base + 2, "seasonNumber": 1, "number": 2, "name": "Second", "aired": "2009-01-27" }),
            ];
            if more {
                list.push(json!({ "id": base + 3, "seasonNumber": 1, "number": 3, "name": "Third", "aired": "2009-02-03" }));
            }
            json!({ "data": { "episodes": list }, "links": { "next": null } })
        };
        {
            let mut routes = routes.lock();
            routes.insert("/login".into(), (200, json!({ "data": { "token": "t" } })));
            routes.insert("/series/770011/extended".into(), (200, series_doc(770_011)));
            routes.insert(
                "/series/770011/episodes/official/eng".into(),
                (200, episodes(7_700_110, false)),
            );
        }
        let first = series(&state, None, Some(770_011))
            .await
            .expect("stored")
            .expect("a work");
        assert_eq!(first.external_ids.tmdb, None);
        assert_eq!(first.external_ids.imdb.as_deref(), Some("tt7700110"));

        // TheTVDB merges it into 770012, which has an episode more.
        {
            let mut routes = routes.lock();
            routes.remove("/series/770011/extended");
            routes.insert("/series/770012/extended".into(), (200, series_doc(770_012)));
            routes.insert(
                "/series/770012/episodes/official/eng".into(),
                (200, episodes(7_700_120, true)),
            );
            routes.insert(
                "/updates".into(),
                (
                    200,
                    json!({ "data": [
                        { "entityType": "series", "methodInt": 3, "method": "delete",
                          "recordId": 770_011, "timeStamp": Utc::now().timestamp(),
                          "mergeToId": 770_012, "mergeToType": "series" }
                    ], "links": { "next": null } }),
                ),
            );
        }
        let said = numbering::watch(&state).await.expect("read");
        assert!(said.contains("merged into 770012"), "{said}");

        // One work, under the id TheTVDB kept, answering to both.
        let holder =
            repo::item::find_id_by_external(&state.db, ExternalSource::TvdbSeries, "770012")
                .await
                .unwrap();
        assert_eq!(holder.as_deref(), Some(first.id.as_str()), "the same work");
        for asked in [770_011, 770_012] {
            let answered = crate::service::series::by_tvdb_id(&state, asked)
                .await
                .unwrap()
                .expect("answered");
            assert_eq!(answered.id, first.id, "asked by {asked}");
        }
        // Served as it was, the episode TheTVDB lists since after it.
        let mut work = repo::item::get(&state.db, &first.id)
            .await
            .unwrap()
            .expect("held");
        repo::item::load_children(&state.db, &mut work)
            .await
            .unwrap();
        assert_eq!(
            served_as_aired(&state, &work).await,
            [
                (1, 1, "Pilot".to_string()),
                (1, 2, "Second".to_string()),
                (1, 3, "Third".to_string()),
            ]
        );

        // Refreshed again under the id it holds: still one work.
        let again = series(&state, None, Some(770_012))
            .await
            .unwrap()
            .expect("a work");
        assert_eq!(again.id, first.id);
    }

    /// The id TheTVDB merged a series into, which another work here answers
    /// to — merged here from one that held it: the series does not take it,
    /// nor is it asked by it, and keeps its own, numbered as a series
    /// TheTVDB deleted.
    #[tokio::test]
    async fn the_id_thetvdb_kept_that_another_work_answers_to_is_not_taken() {
        let (at, routes) = providers().await;
        let state = server(&at).await;
        let show = Show {
            tvdb: 770_031,
            tmdb: 770_131,
            title: "Merged Into Taken P4",
        };
        let other = Show {
            tvdb: 770_033,
            tmdb: 770_133,
            title: "Answers For It P4",
        };
        show.answer(&routes, Tvdb::Up, &[]);
        other.answer(&routes, Tvdb::Up, &[]);
        let work = show.refreshed(&state).await;
        let answering = other.refreshed(&state).await;
        {
            let mut tx = state.db.begin_write().await.unwrap();
            repo::alias::write(
                &state.db,
                &mut tx,
                "a-work-merged-here",
                &answering.id,
                &[("tvdb_series".to_string(), "770032".to_string())],
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }
        assert!(!numbering::takes_id(&state, &work.id, 770_032).await);
        assert!(numbering::takes_id(&state, &answering.id, 770_032).await);

        show.answer(&routes, Tvdb::Gone, &[]);
        routes.lock().insert(
            "/updates".into(),
            (
                200,
                json!({ "data": [
                    { "entityType": "series", "methodInt": 3, "method": "delete",
                      "recordId": 770_031, "timeStamp": Utc::now().timestamp(),
                      "mergeToId": 770_032, "mergeToType": "series" }
                ], "links": { "next": null } }),
            ),
        );
        numbering::watch(&state).await.expect("read");
        assert_eq!(numbering::asked_by(&state, 770_031).await, (770_031, false));
        assert_eq!(
            repo::alias::for_external(&state.db, ExternalSource::TvdbSeries, "770031")
                .await
                .unwrap(),
            None,
            "no other id to answer to"
        );
        let kept = repo::item::get(&state.db, &work.id)
            .await
            .unwrap()
            .expect("held");
        assert_eq!(kept.external_ids.tvdb, Some(770_031));
        let held = repo::order::numbering(&state.db, &work.id).await.unwrap();
        assert!(held.dropped.is_some());
        assert_eq!(held.merged_into, Some(770_032));
    }

    /// TheTVDB's feed deletes an id a work only answers to — merged here from
    /// a work that held it — which TheTVDB still knows under the id it holds:
    /// nothing is kept, and the work is numbered by TheTVDB as before.
    #[tokio::test]
    async fn an_id_a_work_only_answers_to_deleted_by_thetvdb_freezes_nothing() {
        let (at, routes) = providers().await;
        let state = server(&at).await;
        let show = Show {
            tvdb: 770_021,
            tmdb: 770_121,
            title: "Answers To Another P4",
        };
        show.answer(&routes, Tvdb::Up, &[]);
        let work = show.refreshed(&state).await;
        // It answers to 770020 too, as a work merged here from one that
        // held it does.
        {
            let mut tx = state.db.begin_write().await.unwrap();
            repo::alias::write(
                &state.db,
                &mut tx,
                "a-work-merged-here",
                &work.id,
                &[("tvdb_series".to_string(), "770020".to_string())],
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }
        routes.lock().insert(
            "/updates".into(),
            (
                200,
                json!({ "data": [
                    { "entityType": "series", "methodInt": 3, "method": "delete",
                      "recordId": 770_020, "timeStamp": Utc::now().timestamp(),
                      "mergeToId": 770_021, "mergeToType": "series" },
                    { "entityType": "series", "methodInt": 3, "method": "delete",
                      "recordId": 770_020, "timeStamp": Utc::now().timestamp() }
                ], "links": { "next": null } }),
            ),
        );
        let said = numbering::watch(&state).await.expect("read");
        assert!(said.contains("none held here"), "{said}");
        assert_eq!(
            repo::order::numbering(&state.db, &work.id).await.unwrap(),
            repo::order::Numbering::default()
        );
        let all = repo::order::list(&state.db, &work.id).await.unwrap();
        assert!(orders::named(&all, orders::TVDB_LAST).is_none());
        assert_eq!(
            served_as_aired(&state, &work).await,
            [
                (1, 1, "Pilot".to_string()),
                (1, 2, "Second".to_string()),
                (1, 3, "Third".to_string()),
            ]
        );
    }

    /// A reading of TheTVDB's feed that could not keep the numbering of a
    /// series it deleted fails, and is not counted as made: the next one
    /// reads from the same instant.
    #[tokio::test]
    async fn a_reading_that_could_not_keep_a_numbering_is_read_again() {
        let (at, routes) = providers().await;
        let state = server(&at).await;
        let show = Show {
            tvdb: 770_022,
            tmdb: 770_122,
            title: "Not Kept This Time P4",
        };
        show.answer(&routes, Tvdb::Up, &[]);
        show.refreshed(&state).await;
        routes.lock().insert(
            "/updates".into(),
            (
                200,
                json!({ "data": [
                    { "entityType": "series", "methodInt": 3, "method": "delete",
                      "recordId": 770_022, "timeStamp": Utc::now().timestamp() }
                ], "links": { "next": null } }),
            ),
        );
        // What says who numbers a series cannot be read.
        sqlx::query(state.db.sql("DROP TABLE media_numbering"))
            .execute(state.db.pool())
            .await
            .unwrap();
        let failed = numbering::watch(&state).await.expect_err("not kept");
        assert!(format!("{failed:#}").contains("770022"), "{failed:#}");
        assert!(
            repo::import::get(&state.db, numbering::FEED)
                .await
                .unwrap()
                .is_none(),
            "the reading is not counted as made"
        );
    }

    /// A numbering that could not be kept when TheTVDB answered 404 a second
    /// time leaves the series numbered as it was — TheTVDB's — nothing said
    /// of it but the 404s counted, and kept at a later refresh.
    #[tokio::test]
    async fn a_numbering_that_could_not_be_kept_leaves_the_series_as_it_was() {
        let (at, routes) = providers().await;
        let state = server(&at).await;
        let show = Show {
            tvdb: 770_023,
            tmdb: 770_123,
            title: "Kept Later P4",
        };
        show.answer(&routes, Tvdb::Up, &[]);
        let first = show.refreshed(&state).await;
        show.answer(&routes, Tvdb::Gone, &[]);
        show.refreshed(&state).await;
        gone_a_day_ago(&state, &first).await;
        // The orders' places cannot be written.
        sqlx::query(
            state
                .db
                .sql("ALTER TABLE media_order_episode RENAME TO kept_aside"),
        )
        .execute(state.db.pool())
        .await
        .unwrap();
        let not_kept = show.refreshed(&state).await;
        assert_eq!(
            numbers(&not_kept),
            [(1, 1), (1, 2), (1, 3)],
            "TheTVDB's numbering, not TMDB's"
        );
        let held = repo::order::numbering(&state.db, &first.id).await.unwrap();
        assert_eq!((held.gone, held.dropped.as_deref()), (2, None));
        assert!(
            !trail(&state)
                .await
                .iter()
                .any(|a| a == "item.numbering_fallback")
        );

        // Writable again: kept at the next refresh.
        sqlx::query(
            state
                .db
                .sql("ALTER TABLE kept_aside RENAME TO media_order_episode"),
        )
        .execute(state.db.pool())
        .await
        .unwrap();
        let kept = show.refreshed(&state).await;
        assert_eq!(numbers(&kept), [(1, 1), (1, 2), (2, 1)], "TMDB's");
        let held = repo::order::numbering(&state.db, &first.id).await.unwrap();
        assert!(held.dropped.is_some());
        let all = repo::order::list(&state.db, &first.id).await.unwrap();
        assert!(orders::named(&all, orders::TVDB_LAST).is_some_and(|o| o.is_frozen));
    }

    /// What one refresh makes of what TheTVDB did, the clock and the
    /// setting given.
    #[test]
    fn thetvdb_answering_404_is_counted_a_day_apart_and_kept_after_two() {
        use crate::domain::orders::TvdbDropped;

        let now = Utc::now();
        let gone = Seen {
            gone: true,
            tmdb_listed: true,
            ..Seen::default()
        };
        let held = repo::order::Numbering::default();

        // The first: counted, TheTVDB's numbering kept.
        let (once, by_tmdb, change) = weigh(&held, gone, now, 3);
        assert_eq!((once.gone, by_tmdb, change), (1, false, None));
        // Within the twenty hours: the same, once.
        let (same, _, change) = weigh(&once, gone, now + TimeDelta::hours(6), 3);
        assert_eq!((same.gone, change), (1, None));
        // A day on: kept as it was served, TMDB numbering the work.
        let later = now + TimeDelta::hours(21);
        let (twice, by_tmdb, change) = weigh(&once, gone, later, 3);
        assert_eq!(
            (twice.gone, by_tmdb, change),
            (2, true, Some(NumberingChange::Dropped))
        );
        assert_eq!(twice.dropped_why, Some(TvdbDropped::Missing));
        // Off: counted, never kept.
        let (off, by_tmdb, change) = weigh(&once, gone, later, 0);
        assert_eq!(
            (off.gone, off.dropped.is_some(), by_tmdb, change),
            (2, false, false, None)
        );

        // Skyhook still hands TheTVDB's list on: counted, and it numbers.
        let handed_on = Seen {
            listed: true,
            ..gone
        };
        let (still, by_tmdb, _) = weigh(&twice, handed_on, later, 3);
        assert!(still.dropped.is_some() && !by_tmdb);

        // A series TheTVDB merged into another, asked under that one, which
        // TheTVDB fails for: a silence like any, counted — TMDB does not
        // number it, TheTVDB's list stands.
        let merged_away = repo::order::Numbering {
            dropped_why: Some(TvdbDropped::Merged),
            merged_into: Some(9),
            ..twice.clone()
        };
        let silent_there = Seen {
            silent: true,
            tmdb_listed: true,
            rekeyed: true,
            ..Seen::default()
        };
        let (counted, by_tmdb, change) = weigh(&merged_away, silent_there, later, 3);
        assert_eq!((counted.silent, by_tmdb, change), (1, false, None));
        assert!(counted.dropped.is_some());

        // TheTVDB answering with its record and no episode: it knows the
        // series, and does not number it — nothing given back.
        let record_alone = Seen {
            known: true,
            tmdb_listed: true,
            ..Seen::default()
        };
        let (still, by_tmdb, change) = weigh(&twice, record_alone, later, 3);
        assert!(still.dropped.is_some() && by_tmdb && change.is_none());
        assert_eq!(still.gone, 0, "it knows the series");

        // TheTVDB knows it again: given back.
        let known = Seen {
            known: true,
            listed: true,
            tmdb_listed: true,
            ..Seen::default()
        };
        let (back, by_tmdb, change) = weigh(&twice, known, later, 3);
        assert_eq!(
            (back, by_tmdb, change),
            (
                repo::order::Numbering::default(),
                false,
                Some(NumberingChange::Known)
            )
        );
        // Not for a series it merged into another: by hand.
        let merged = repo::order::Numbering {
            dropped_why: Some(TvdbDropped::Merged),
            merged_into: Some(2),
            ..twice.clone()
        };
        let (kept, by_tmdb, change) = weigh(&merged, known, later, 3);
        assert_eq!(
            (kept.dropped.is_some(), by_tmdb, change),
            (true, false, None)
        );

        // TMDB numbering the series in TheTVDB's silence, then a 404: TMDB
        // goes on numbering it while the 404 is counted.
        let silence = repo::order::Numbering {
            silent: 3,
            since: Some(crate::db::to_rfc3339(now)),
            ..Default::default()
        };
        let (counted, by_tmdb, change) = weigh(&silence, gone, now, 3);
        assert_eq!((counted.gone, by_tmdb, change), (1, true, None));
        assert!(counted.since.is_some());
        // And kept, the silence forgotten: the series is dropped now.
        let (dropped, by_tmdb, change) = weigh(&counted, gone, later, 3);
        assert_eq!(change, Some(NumberingChange::Dropped));
        assert!(by_tmdb && dropped.since.is_none() && dropped.silent == 0);
    }
}
