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
            Self::Nothing | Self::Failed(_) => None,
        }
    }
}

/// Everything the providers asked came to.
#[derive(Default)]
struct Gathered {
    answers: Vec<Answer>,
    failed: Vec<Failure>,
}

impl Gathered {
    fn add(&mut self, asked: Asked) {
        match asked {
            Asked::Answered(answer) => self.answers.push(*answer),
            Asked::Nothing => {}
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
    let (from_tmdb, from_tvdb, from_skyhook, from_fanart, from_tvmaze) = tokio::join!(
        series_from_tmdb(state, tmdb_id),
        series_from_tvdb(state, tvdb_id),
        series_from_skyhook(state, tvdb_id),
        series_from_fanart(state, tvdb_id),
        series_from_tvmaze(state, tvdb_id, tmdb_id),
    );

    let mut gathered = Gathered::default();
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

    match state.mal.anime(id, kind).await {
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
async fn store(state: &AppState, gathered: Gathered) -> Result<Option<MediaItem>> {
    let Gathered {
        mut answers,
        failed,
    } = gathered;

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

    let failure = failure_note(&failed, &answers);
    let before = match &failure {
        Some(_) => held_before(state, &answers).await,
        None => None,
    };

    if let Some((stored, _)) = &before {
        for answer in &mut answers {
            fill_unanswered_seasons(answer, stored);
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
        hand_back(&mut contributions, &failed, stored, provenance);
    }

    let views = views_of(state, &contributions);

    let Some(mut merged) = merge::combine(contributions, &state.config.provider_priority) else {
        return Ok(None);
    };
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

    if let Some(failure) = &failure {
        tracing::warn!(
            title = %merged.title,
            failure = %failure,
            "a refresh some providers did not answer; what they gave before is kept"
        );
    }

    let stored = persist(state, merged, &snapshots, provenance, failure.as_deref()).await?;
    Ok(Some(stored))
}

/// What did not answer, as the work's `refreshError` says it — `None` when
/// everything asked answered in full.
fn failure_note(failed: &[Failure], answers: &[Answer]) -> Option<String> {
    let mut parts: Vec<String> = failed
        .iter()
        .map(|f| format!("{}: {}", f.provider, f.error))
        .collect();
    for answer in answers.iter().filter(|a| !a.unanswered_seasons.is_empty()) {
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

/// The work these answers describe, as stored, and who gave it what: what
/// stands for a provider that did not answer. `None` for a work not held
/// yet, or one that cannot be read — logged, and the refresh goes on with
/// what answered.
async fn held_before(state: &AppState, answers: &[Answer]) -> Option<(MediaItem, Provenance)> {
    let kind = answers
        .iter()
        .find(|a| !SUPPLEMENTS.contains(&a.provider))?
        .item
        .kind;

    // Its identifiers as the merge will make them: in priority order, the
    // first of each kind.
    let priority = &state.config.provider_priority;
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

    let read = async {
        let Some(id) = super::find_existing(state, &probe).await? else {
            return anyhow::Ok(None);
        };
        let Some(mut stored) = repo::item::get(&state.db, &id).await? else {
            return Ok(None);
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
/// with one: the fresh list stands.
fn hand_back(
    contributions: &mut Vec<Contribution>,
    failed: &[Failure],
    stored: &MediaItem,
    provenance: &Provenance,
) {
    let renumbered = contributions.iter().any(|c| {
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

    if failed
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
    let mut answers = ask_again(state, stored, asking).await;
    // A season asked again that went unanswered keeps what is stored of it,
    // as a refresh keeps it.
    for answer in &mut answers {
        fill_unanswered_seasons(answer, stored);
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

    let mut answer = Answer::new(names::TMDB, raw, tmdb_map::tv_to_item(&tv, &seasons));
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
            wire::sonarr::to_item(&show),
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

    match state.tvdb.series(tvdb_id).await {
        Ok(Some((raw, item))) => Asked::answered(Answer::new(names::TVDB, raw, item)),
        Ok(None) => Asked::Nothing,
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
            tmdb_map::movie_to_item(&movie),
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

    /// The server, on a database in memory, TMDB and TheTVDB keyed, every
    /// provider at `at`, the anime sites, TVmaze and Fanart.tv off.
    async fn server(at: &str) -> AppState {
        let mut config = config::Config::from_env().expect("a configuration");
        config.mode = config::Mode::Single;
        config.database = config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        };
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
        hand_back(&mut contributions, &failed, &stored, &provenance);
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

    /// A season the provider lists again after a person added it by hand,
    /// and an episode likewise, stay as the person wrote them.
    #[tokio::test]
    async fn a_refresh_does_not_write_over_what_was_added_by_hand() {
        let db = &crate::db::Db::connect(&config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .expect("in-memory database");
        db.migrate().await.expect("migrations");

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
}
