//! Asking every provider, and folding the answers into one.
//!
//! Resolution — deciding *which* work a client means — lives in
//! [`super::series`] and [`super::movie`]. This is what happens once that is
//! settled: each enabled provider is asked, concurrently, and what they return
//! is merged by [`crate::merge`] and stored as a single entity with every raw
//! answer kept alongside.
//!
//! A provider that fails is logged and skipped. One source being down should
//! cost detail, not the whole answer.

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::{
    db::repo,
    domain::{ExternalIds, ExternalSource, MediaItem, MediaKind},
    merge::{self, Contribution, provenance::Provenance},
    providers::{names, tmdb::map as tmdb_map},
    service::{anime, persist},
    state::AppState,
    wire,
};

/// The most seasons one series fetch will ask a provider about.
///
/// Each is its own HTTP call. Nothing real comes close — the longest-running
/// television on record is under a hundred — and the number is read off an
/// answer rather than known in advance.
const MAX_SEASONS: usize = 200;

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
}

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
    let (from_tmdb, from_tvdb, from_skyhook, from_fanart, from_tvmaze) = tokio::join!(
        series_from_tmdb(state, tmdb_id),
        series_from_tvdb(state, tvdb_id),
        series_from_skyhook(state, tvdb_id),
        series_from_fanart(state, tvdb_id),
        series_from_tvmaze(state, tvdb_id, tmdb_id),
    );

    let mut answers: Vec<Answer> = [from_tmdb, from_tvdb, from_skyhook, from_fanart, from_tvmaze]
        .into_iter()
        .flatten()
        .collect();

    // The anime sites second: which of their entries to ask about comes from
    // the identifier list, or failing that from the ids Skyhook just returned.
    if anime::enabled(state) && describes_a_work(&answers) {
        let mut known = ExternalIds::default();
        for answer in &answers {
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
            answers.extend(from_anime_sites(state, chosen, MediaKind::Series).await);
        }
    }

    let stored = store(state, answers).await?;
    // The other orders TheTVDB numbers it in, kept beside the aired one.
    if let Some(item) = &stored {
        super::orders::gather(state, item).await;
    }
    Ok(stored)
}

/// Fetch a movie from everything that can address it, and store the result.
pub async fn movie(
    state: &AppState,
    tmdb_id: Option<i64>,
    imdb_id: Option<&str>,
) -> Result<Option<MediaItem>> {
    let (from_tmdb, from_radarr, from_fanart) = tokio::join!(
        movie_from_tmdb(state, tmdb_id),
        movie_from_radarr(state, tmdb_id, imdb_id),
        movie_from_fanart(state, tmdb_id, imdb_id),
    );

    let mut answers: Vec<Answer> = [from_tmdb, from_radarr, from_fanart]
        .into_iter()
        .flatten()
        .collect();

    let pinned = if anime::enabled(state) && describes_a_work(&answers) {
        pinned_ids(state, MediaKind::Movie, tmdb_id, None).await
    } else {
        None
    };
    if anime::enabled(state)
        && describes_a_work(&answers)
        && (tmdb_id.is_some() || pinned.is_some())
    {
        let mapped = match tmdb_id {
            Some(tmdb_id) => Some(anime::for_movie(state, tmdb_id).await),
            None => None,
        };
        let chosen = anime::choose(mapped, pinned.as_ref(), &ExternalIds::default());
        let mut from_sites = from_anime_sites(state, chosen, MediaKind::Movie).await;

        // A film keeps no AniList or MyAnimeList id. TheTVDB files films under
        // the series they belong to, and Skyhook lists them with its entries,
        // so a series already claims most of them — and a work is matched to
        // the stored one by any id it shares, whatever its kind. The film would
        // be written over the series.
        for answer in &mut from_sites {
            answer.item.external_ids = ExternalIds::default();
        }

        answers.extend(from_sites);
    }

    store(state, answers).await
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
        None
    };

    let mut answers = vec![Answer {
        provider: names::FANKAI,
        payload: raw,
        item,
    }];
    answers.extend(wiki);

    store(state, answers).await
}

/// What the Fankai wiki says a production was cut from, and which Fan-Kai
/// follows it, as the production's relations.
///
/// A supplement: its answer carries those and nothing else. The original's
/// title, year and cover come from AniList when that source is on, and from
/// the wiki's own links otherwise; a sequel is only named when Fankai lists it.
async fn fankai_from_wiki(state: &AppState, production: &MediaItem) -> Option<Answer> {
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
            return None;
        }
        Err(e) => {
            tracing::warn!(
                title = %production.title,
                error = format_args!("{e:#}"),
                "the Fankai wiki could not be asked"
            );
            return None;
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
            None => original.relation(),
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
        return None;
    }
    for (index, relation) in relations.iter_mut().enumerate() {
        relation.sort_order = i32::try_from(index).unwrap_or(i32::MAX);
    }

    let mut item = MediaItem::empty(MediaKind::Series);
    item.relations = relations;
    item.homepage = homepage;

    Some(Answer {
        provider: names::FANKAI_WIKI,
        payload: raw,
        item,
    })
}

/// Whether anything that can stand for a work on its own answered.
fn describes_a_work(answers: &[Answer]) -> bool {
    answers.iter().any(|a| !SUPPLEMENTS.contains(&a.provider))
}

/// AniList and MyAnimeList, asked at the same time about the entry `chosen`.
async fn from_anime_sites(state: &AppState, chosen: anime::Chosen, kind: MediaKind) -> Vec<Answer> {
    if chosen.is_empty() {
        return Vec::new();
    }

    let (from_anilist, from_mal) = tokio::join!(
        from_anilist(state, chosen.anilist, kind),
        from_mal(state, chosen.mal, kind),
    );

    [from_anilist, from_mal].into_iter().flatten().collect()
}

async fn from_anilist(state: &AppState, id: Option<i64>, kind: MediaKind) -> Option<Answer> {
    let id = id?;
    if !state.flag("anilist.enabled", false) {
        return None;
    }

    match state.anilist.media(id, kind).await {
        Ok(Some((raw, item))) => Some(Answer {
            provider: names::ANILIST,
            payload: raw,
            item,
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                anilist_id = id,
                error = format_args!("{e:#}"),
                "AniList lookup failed"
            );
            None
        }
    }
}

async fn from_mal(state: &AppState, id: Option<i64>, kind: MediaKind) -> Option<Answer> {
    let id = id?;
    if !state.flag("mal.enabled", false) {
        return None;
    }

    match state.mal.anime(id, kind).await {
        Ok(Some((raw, item))) => Some(Answer {
            provider: names::MAL,
            payload: raw,
            item,
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                mal_id = id,
                error = format_args!("{e:#}"),
                "MyAnimeList lookup failed"
            );
            None
        }
    }
}

/// Merge what came back and write it.
async fn store(state: &AppState, answers: Vec<Answer>) -> Result<Option<MediaItem>> {
    if answers.is_empty() {
        return Ok(None);
    }

    let providers: Vec<&str> = answers.iter().map(|a| a.provider).collect();
    tracing::debug!(?providers, "merging provider answers");

    if !describes_a_work(&answers) {
        tracing::warn!(
            ?providers,
            "only supplementary sources answered; not storing"
        );
        return Ok(None);
    }

    let snapshots: Vec<(String, Value)> = answers
        .iter()
        .map(|a| (a.provider.to_string(), a.payload.clone()))
        .collect();

    let contributions: Vec<Contribution> = answers
        .into_iter()
        .map(|a| Contribution {
            provider: a.provider.to_string(),
            item: a.item,
        })
        .collect();

    let views = views_of(state, &contributions);

    let Some(merged) = merge::combine(contributions, &state.config.provider_priority) else {
        return Ok(None);
    };
    let provenance = merge::provenance::attribute(&merged, &views);

    // Answers from supplements alone were refused above; this is the provider
    // of record that answered with a blank title. Storing it would put a
    // nameless row in the catalogue and hand the client an entry it cannot
    // display.
    if merged.title.trim().is_empty() {
        tracing::warn!(?providers, "no provider named this work; not storing it");
        return Ok(None);
    }

    let stored = persist(state, merged, &snapshots, provenance).await?;
    Ok(Some(stored))
}

/// Each answer taken apart before the merge consumes them, in the order the
/// merge folds them in, to say afterwards who gave what.
fn views_of(state: &AppState, contributions: &[Contribution]) -> Vec<merge::provenance::View> {
    let priority = &state.config.provider_priority;
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
    let priority = &state.config.provider_priority;
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
    let answers = ask_again(state, stored, asking).await;
    let answered: Vec<&'static str> = answers.iter().map(|a| a.provider).collect();
    let silent: Vec<&'static str> = asking
        .iter()
        .copied()
        .filter(|p| !answered.contains(p))
        .collect();

    if answers.is_empty() {
        return Ok(ResyncOutcome::NoneAnswered);
    }

    let snapshots: Vec<(String, Value)> = answers
        .iter()
        .map(|a| (a.provider.to_string(), a.payload.clone()))
        .collect();

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
    let views = views_of(state, &contributions);
    contributions.push(Contribution {
        provider: merge::provenance::STORED.into(),
        item: merge::provenance::leftover(stored, &answered),
    });

    let Some(merged) = merge::combine(contributions, &state.config.provider_priority) else {
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

    let item = match super::persist_sync(state, merged, &snapshots, stored, &now).await? {
        super::SyncWrite::Written(item) => *item,
        super::SyncWrite::Changed => return Ok(ResyncOutcome::Changed),
        super::SyncWrite::Gone => return Ok(ResyncOutcome::Gone),
    };

    if item.kind == MediaKind::Series && answered.contains(&names::TVDB) {
        super::orders::gather(state, &item).await;
    }

    Ok(ResyncOutcome::Synced(Box::new(Resynced {
        item,
        answered,
        silent,
    })))
}

/// Only the providers in `asking`, each by the identifiers the work carries.
async fn ask_again(state: &AppState, stored: &MediaItem, asking: &[&'static str]) -> Vec<Answer> {
    async fn when(asked: bool, answer: impl Future<Output = Option<Answer>>) -> Option<Answer> {
        if asked { answer.await } else { None }
    }

    let ids = &stored.external_ids;
    let wants = |provider: &str| asking.contains(&provider);

    if stored.kind == MediaKind::Series
        && let Some(fankai_id) = ids.fankai
    {
        let production = if wants(names::FANKAI) && state.flag("fankai.enabled", false) {
            match state.fankai.series(fankai_id).await {
                Ok(Some((raw, item))) => Some(Answer {
                    provider: names::FANKAI,
                    payload: raw,
                    item,
                }),
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
            fankai_from_wiki(state, production.as_ref().map_or(stored, |a| &a.item)).await
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

async fn series_from_tmdb(state: &AppState, tmdb_id: Option<i64>) -> Option<Answer> {
    let tmdb_id = tmdb_id?;
    if !state.tmdb.is_configured() {
        return None;
    }

    let (raw, tv) = match state.tmdb.tv(tmdb_id).await {
        Ok(Some(found)) => found,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(
                tmdb_id,
                error = format_args!("{e:#}"),
                "TMDB series fetch failed"
            );
            return None;
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

    let seasons = state.tmdb.tv_seasons(tmdb_id, &numbers).await;

    Some(Answer {
        provider: names::TMDB,
        payload: raw,
        item: tmdb_map::tv_to_item(&tv, &seasons),
    })
}

/// Sonarr's own Skyhook, as a second opinion rather than only a fallback.
///
/// It carries things TMDB has no field for: the broadcast time of day, TVMaze
/// and AniList ids, and the air-order hints Sonarr uses for anime.
async fn series_from_skyhook(state: &AppState, tvdb_id: Option<i64>) -> Option<Answer> {
    if !state.flag("skyhook.enrich", true) {
        return None;
    }
    skyhook_answer(state, tvdb_id).await
}

/// What Skyhook says, whatever enrichment is set to: a sync asks it by name.
async fn skyhook_answer(state: &AppState, tvdb_id: Option<i64>) -> Option<Answer> {
    let tvdb_id = tvdb_id?;

    match state.skyhook.show(tvdb_id).await {
        Ok(Some((raw, show))) => Some(Answer {
            provider: names::SKYHOOK,
            payload: raw,
            item: wire::sonarr::to_item(&show),
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "Skyhook enrichment failed"
            );
            None
        }
    }
}

/// TheTVDB, which is where absolute episode numbering comes from.
async fn series_from_tvdb(state: &AppState, tvdb_id: Option<i64>) -> Option<Answer> {
    let tvdb_id = tvdb_id?;
    if !state.tvdb.is_enabled() {
        return None;
    }

    match state.tvdb.series(tvdb_id).await {
        Ok(Some((raw, item))) => Some(Answer {
            provider: names::TVDB,
            payload: raw,
            item,
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "TheTVDB lookup failed"
            );
            None
        }
    }
}

/// TVmaze, for the instant each episode aired.
///
/// What it says about an episode is only used where its broadcast date agrees
/// with the spine's — see `merge::apply_broadcast_times`.
async fn series_from_tvmaze(
    state: &AppState,
    tvdb_id: Option<i64>,
    tmdb_id: Option<i64>,
) -> Option<Answer> {
    if !state.flag("tvmaze.enabled", false) {
        return None;
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
        return None;
    }

    match state.tvmaze.series(tvdb_id, known).await {
        Ok(Some((raw, item))) => Some(Answer {
            provider: names::TVMAZE,
            payload: raw,
            item,
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "TVmaze lookup failed"
            );
            None
        }
    }
}

/// Fanart.tv, which contributes artwork and nothing else.
///
/// It indexes television on TVDB ids only, so a series with no TVDB id cannot
/// be looked up there at all.
async fn series_from_fanart(state: &AppState, tvdb_id: Option<i64>) -> Option<Answer> {
    let tvdb_id = tvdb_id?;
    if !state.fanart.is_enabled() {
        return None;
    }

    match state.fanart.series(tvdb_id).await {
        Ok(Some((raw, item))) => Some(Answer {
            provider: names::FANART,
            payload: raw,
            item,
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "Fanart.tv lookup failed"
            );
            None
        }
    }
}

async fn movie_from_fanart(
    state: &AppState,
    tmdb_id: Option<i64>,
    imdb_id: Option<&str>,
) -> Option<Answer> {
    if !state.fanart.is_enabled() {
        return None;
    }

    // It accepts either key; TMDB's is the one more titles are indexed under.
    let key = tmdb_id
        .map(|id| id.to_string())
        .or_else(|| imdb_id.map(String::from))?;

    match state.fanart.movie(&key).await {
        Ok(Some((raw, item))) => Some(Answer {
            provider: names::FANART,
            payload: raw,
            item,
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(%key, error = format_args!("{e:#}"), "Fanart.tv lookup failed");
            None
        }
    }
}

async fn movie_from_tmdb(state: &AppState, tmdb_id: Option<i64>) -> Option<Answer> {
    let tmdb_id = tmdb_id?;
    if !state.tmdb.is_configured() {
        return None;
    }

    match state.tmdb.movie(tmdb_id).await {
        Ok(Some((raw, movie))) => Some(Answer {
            provider: names::TMDB,
            payload: raw,
            item: tmdb_map::movie_to_item(&movie),
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                tmdb_id,
                error = format_args!("{e:#}"),
                "TMDB movie fetch failed"
            );
            None
        }
    }
}

/// Radarr's own metadata service, as a second opinion.
///
/// It resolves certifications by country and carries ratings from IMDb,
/// Metacritic and Rotten Tomatoes that TMDB does not have at all.
async fn movie_from_radarr(
    state: &AppState,
    tmdb_id: Option<i64>,
    imdb_id: Option<&str>,
) -> Option<Answer> {
    if !state.flag("radarr.enrich", true) {
        return None;
    }
    radarr_answer(state, tmdb_id, imdb_id).await
}

/// What Radarr's service says, whatever enrichment is set to: a sync asks it
/// by name.
async fn radarr_answer(
    state: &AppState,
    tmdb_id: Option<i64>,
    imdb_id: Option<&str>,
) -> Option<Answer> {
    let found = match tmdb_id {
        Some(id) => state.radarr_metadata.movie(id).await,
        None => match imdb_id {
            Some(id) => state.radarr_metadata.by_imdb_id(id).await,
            None => return None,
        },
    };

    match found {
        Ok(Some((raw, movie))) => Some(Answer {
            provider: names::RADARR,
            payload: raw,
            item: wire::radarr::to_item(&movie),
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                ?tmdb_id,
                error = format_args!("{e:#}"),
                "Radarr metadata enrichment failed"
            );
            None
        }
    }
}
