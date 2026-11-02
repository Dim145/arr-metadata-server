//! Combining what several providers say about one work.
//!
//! Each provider is mapped to a canonical [`MediaItem`] on its own, and those
//! are folded together here in priority order. The rules differ by field, and
//! the differences are the point:
//!
//! * **Scalars** take the first provider that has anything. A lower-priority
//!   provider fills a gap rather than overwriting an answer.
//! * **Artwork, alternative titles, ratings and translations** are unioned.
//!   Fanart.tv exists to add artwork; discarding it because TMDB already had a
//!   poster would defeat the point.
//! * **Credits** come from one provider — the highest-priority one that has any.
//!   Unioning them would list the same actor two or three times, since nothing
//!   reliably identifies a person across providers.
//! * **Companies** come a role at a time from the first provider that lists
//!   any in it — TMDB's networks, TheTVDB's studios — and the others complete
//!   those that are the same company with their ids and logos. **Videos** are
//!   unioned; TMDB's **keywords**, as entities, are taken whole.
//! * **Episodes and seasons** take their *list* from one provider and their
//!   *fields* from all of them. Merging two lists by season and episode number
//!   sounds right and is not: providers disagree about where a season ends, so
//!   unioning One Piece's 1179 TVDB episodes with TMDB's 1181 produced 2352,
//!   most of them phantoms Sonarr would then hunt for files of. One list, many
//!   opinions about each entry — about a special, only from a provider that
//!   dates it as the list does, since specials are where numberings part most.

pub mod provenance;
pub mod rules;

use crate::domain::{
    AlternativeTitle, CompanyCredit, CountryRelease, Episode, ExternalIds, Image, MediaItem,
    Rating, Season, Translation, Video,
};

/// What one provider had to say.
pub struct Contribution {
    pub provider: String,
    pub item: MediaItem,
}

/// Providers whose episode numbering is TVDB's.
///
/// Sonarr addresses a series by its TVDB id and expects the episode numbering
/// that goes with it. TMDB numbers the same series differently — it splits One
/// Piece into 23 seasons where TVDB has 21 — so serving TMDB's numbering under
/// a TVDB id would have Sonarr map files to the wrong episodes. When one of
/// these has a list, it is the list, whatever the general priority says.
pub(crate) const TVDB_NUMBERED: &[&str] = &["tvdb", "skyhook"];

/// Providers whose episodes are matched to the spine by broadcast date as well
/// as by number, and whose broadcast instant replaces anyone else's.
pub(crate) const DATE_CHECKED: &[&str] = &["tvmaze"];

/// Providers whose studio replaces anyone else's.
///
/// They only ever describe anime, and for anime their main studio is the one
/// that animated it. What TMDB supplies as a studio is the first production
/// company it happens to list: Production I.G for *Attack on Titan*, whose
/// first three seasons Wit Studio animated.
pub(crate) const STUDIO_AUTHORITIES: &[&str] = &["anilist", "mal"];

/// The providers whose series names are TheTVDB's: Skyhook passes them on.
pub(crate) const TVDB_NAMED: &[&str] = &["tvdb", "skyhook"];

/// Fold contributions into one entity, most trusted first.
///
/// `priority` names providers in order; anything not named sorts last, keeping
/// its relative order, so an unconfigured provider still contributes rather
/// than being silently dropped.
pub fn combine(mut contributions: Vec<Contribution>, priority: &[String]) -> Option<MediaItem> {
    if contributions.is_empty() {
        return None;
    }

    let rank = |provider: &str| {
        priority
            .iter()
            .position(|p| p == provider)
            .unwrap_or(priority.len())
    };

    contributions.sort_by_key(|c| rank(&c.provider));

    // TVmaze's episodes are held out of the merge below: it numbers some series
    // its own way, so it may neither supply the list nor fill it by number
    // alone. What it knows is applied at the end, where its broadcast date
    // agrees with the spine's; see `apply_broadcast_times`.
    let broadcasts: Vec<Episode> = contributions
        .iter_mut()
        .filter(|c| DATE_CHECKED.contains(&c.provider.as_str()))
        .flat_map(|c| {
            c.item.seasons.clear();
            std::mem::take(&mut c.item.episodes)
        })
        .collect();

    let studio = contributions
        .iter()
        .filter(|c| STUDIO_AUTHORITIES.contains(&c.provider.as_str()))
        .find_map(|c| c.item.studio.clone().filter(|s| !s.trim().is_empty()));

    // Decide whose episode numbering this is before anything is folded, because
    // every later provider fills fields *into* that list.
    //
    // A numbering is an episode list. Seasons alone are not one, and treating
    // them as one was how a TheTVDB answer that carried its six seasons but no
    // episodes — its episode endpoint having failed while the rest succeeded —
    // became the spine, and took every other provider's episodes with it: the
    // merged list was empty, and `persist` writes what the merge produced.
    let numbered = |c: &Contribution| !c.item.episodes.is_empty();

    let spine = contributions
        .iter()
        .position(|c| TVDB_NUMBERED.contains(&c.provider.as_str()) && numbered(c))
        .or_else(|| contributions.iter().position(numbered))
        // Nobody has episodes: a film, or a series nothing could answer for.
        // Seasons can still come from somewhere, and the rule they exist to
        // protect does not apply when there is nothing to number.
        .or_else(|| {
            contributions
                .iter()
                .position(|c| {
                    TVDB_NUMBERED.contains(&c.provider.as_str()) && !c.item.seasons.is_empty()
                })
                .or_else(|| {
                    contributions
                        .iter()
                        .position(|c| !c.item.seasons.is_empty())
                })
        });

    let (seasons, episodes) = match spine {
        Some(index) => {
            let item = &mut contributions[index].item;
            (
                std::mem::take(&mut item.seasons),
                std::mem::take(&mut item.episodes),
            )
        }
        None => (Vec::new(), Vec::new()),
    };

    // Skyhook republishes TheTVDB, so either one's specials are numbered as
    // the other's are. Anyone else's are lined up by date first; see
    // `align_specials`.
    let tvdb_spine =
        spine.is_some_and(|index| TVDB_NUMBERED.contains(&contributions[index].provider.as_str()));
    let numbered_alike = |provider: &str| tvdb_spine && TVDB_NUMBERED.contains(&provider);

    // TheTVDB's names for the series, before they are folded away: what they
    // add to a name the work goes by is how Sonarr tells it from a homonym.
    let tvdb_names: Vec<String> = contributions
        .iter()
        .filter(|c| TVDB_NAMED.contains(&c.provider.as_str()))
        .map(|c| c.item.title.clone())
        .collect();

    let first = contributions.remove(0);
    let mut merged = first.item;

    // The most trusted provider loses the numbering to the spine, but not what
    // it knows about each episode: TMDB carries overviews and stills that TVDB
    // rarely has. Its own lists fold back in as a field source like any other.
    let displaced_seasons = std::mem::replace(&mut merged.seasons, seasons);
    let mut displaced_episodes = std::mem::replace(&mut merged.episodes, episodes);
    let by_day = !numbered_alike(&first.provider);
    if by_day {
        align_specials(&merged.episodes, &mut displaced_episodes);
    }
    let alike = Alike::of(&merged.episodes, &displaced_episodes, by_day);
    merge_seasons(&mut merged.seasons, displaced_seasons, &alike);
    merge_episodes(&mut merged.episodes, displaced_episodes, by_day);

    for mut contribution in contributions {
        let by_day = !numbered_alike(&contribution.provider);
        if by_day {
            align_specials(&merged.episodes, &mut contribution.item.episodes);
        }
        fold(&mut merged, contribution.item, by_day);
    }

    apply_broadcast_times(&mut merged.episodes, broadcasts);

    if studio.is_some() {
        merged.studio = studio;
    }

    // Whichever provider ended up supplying the spine, a client reads it in order.
    merged.seasons.sort_by_key(|s| s.season_number);
    merged
        .episodes
        .sort_by_key(|e| (e.season_number, e.episode_number));
    merged
        .images
        .sort_by_key(|i| (i.cover_type.priority(), i.sort_order));
    merged.companies.sort_by_key(|c| (c.role, c.sort_order));
    merged.videos.sort_by_key(|v| v.sort_order);

    merged.title_qualifier = title_qualifier(&merged, &tvdb_names);

    Some(merged)
}

/// What TheTVDB adds to a series' name to tell it from another of the same
/// name — `2023` in *Rurouni Kenshin (2023)*, `US` in *The Office (US)* — when
/// the rest of its name is one the work goes by here: its title, its original
/// title, or one of its translated ones, compared as Sonarr compares titles.
fn title_qualifier(item: &MediaItem, tvdb_names: &[String]) -> Option<String> {
    use crate::service::scene::clean_title;

    if item.kind != crate::domain::MediaKind::Series {
        return None;
    }
    let known: Vec<String> = std::iter::once(item.title.as_str())
        .chain(item.original_title.as_deref())
        .chain(item.translations.iter().filter_map(|t| t.title.as_deref()))
        .map(clean_title)
        .filter(|c| !c.trim().is_empty())
        .collect();
    tvdb_names.iter().find_map(|name| {
        let (base, qualifier) = split_qualifier(name)?;
        known
            .contains(&clean_title(base))
            .then(|| qualifier.to_string())
    })
}

/// A name's trailing ` (…)`, when it holds what TheTVDB tells homonyms apart
/// by: a year, or a country's code.
pub(crate) fn split_qualifier(name: &str) -> Option<(&str, &str)> {
    let inner = name.trim().strip_suffix(')')?;
    let open = inner.rfind(" (")?;
    let (base, qualifier) = (inner[..open].trim(), &inner[open + 2..]);
    let year = qualifier.len() == 4
        && qualifier
            .parse::<i32>()
            .is_ok_and(|y| (1900..=2100).contains(&y));
    let country =
        (2..=3).contains(&qualifier.len()) && qualifier.chars().all(|c| c.is_ascii_uppercase());
    (!base.is_empty() && (year || country)).then_some((base, qualifier))
}

/// Drop the alternative titles that are only the work's own title again.
///
/// Several providers list it anyway, under whichever country they filed it
/// for. Applied to what is about to be stored rather than inside the merge: a
/// merge that removed every title a work had would come back empty, and an
/// empty list is how a refresh says nobody answered — so the stored list, its
/// copies of the title included, would be put back.
pub fn drop_own_title(item: &mut MediaItem) {
    let title = item.title.trim().to_string();
    item.alternative_titles
        .retain(|t| t.is_manual || !t.title.trim().eq_ignore_ascii_case(&title));
}

/// Take from `other` whatever `into` is missing.
/// Fold one more provider in: `by_day` when it numbers the series otherwise
/// than the list does, so that its episodes are paired by the day they aired
/// ([`merge_episodes`]).
fn fold(into: &mut MediaItem, other: MediaItem, by_day: bool) {
    fill(&mut into.sort_title, other.sort_title);
    fill(&mut into.original_title, other.original_title);
    fill(&mut into.overview, other.overview);
    fill(&mut into.status, other.status);
    fill(&mut into.original_language, other.original_language);
    fill(&mut into.original_country, other.original_country);
    fill(&mut into.first_aired, other.first_aired);
    fill(&mut into.last_aired, other.last_aired);
    fill(&mut into.in_cinemas, other.in_cinemas);
    fill(&mut into.physical_release, other.physical_release);
    fill(&mut into.digital_release, other.digital_release);
    fill(&mut into.air_time, other.air_time);
    fill(&mut into.network, other.network);
    fill(&mut into.studio, other.studio);
    fill(&mut into.content_rating, other.content_rating);
    fill(
        &mut into.content_rating_country,
        other.content_rating_country,
    );
    fill(&mut into.homepage, other.homepage);
    fill(&mut into.trailer_youtube_id, other.trailer_youtube_id);
    fill(&mut into.theme_music, other.theme_music);
    fill(&mut into.tagline, other.tagline);
    fill(&mut into.format, other.format);

    if into.title.trim().is_empty() {
        into.title = other.title;
    }
    if into.runtime.is_none() {
        into.runtime = other.runtime;
    }
    if into.year.is_none() {
        into.year = other.year;
    }
    if into.popularity.is_none() {
        into.popularity = other.popularity;
    }
    if into.collection_tmdb_id.is_none() {
        into.collection_tmdb_id = other.collection_tmdb_id;
    }
    if into.genres.is_empty() {
        into.genres = other.genres;
    }
    // Every source's, gathered: AniList's tags say what TMDB's keywords do
    // not, and taken whole they were gone the moment TMDB answered.
    union_keywords(&mut into.keywords, other.keywords);
    if into.production_countries.is_empty() {
        into.production_countries = other.production_countries;
    }
    if into.production_companies.is_empty() {
        into.production_companies = other.production_companies;
    }
    union_releases(&mut into.releases, other.releases);

    // Believed from whichever provider says so. A work only one source knows
    // is adult — AniList, for most of what it flags — must not be served as
    // anything else because a broader source forgot to say.
    into.is_adult = into.is_adult || other.is_adult;

    merge_ids(&mut into.external_ids, other.external_ids);

    union_images(&mut into.images, other.images);
    for (address, details) in other.image_details {
        into.image_details.entry(address).or_insert(details);
    }
    union_alternative_titles(&mut into.alternative_titles, other.alternative_titles);
    union_ratings(&mut into.ratings, other.ratings);
    union_translations(&mut into.translations, other.translations);
    union_companies(&mut into.companies, other.companies);
    union_videos(&mut into.videos, other.videos);
    if into.keyword_entities.is_empty() {
        into.keyword_entities = other.keyword_entities;
    }

    // One provider's cast, not a blend of several, but for TheTVDB's people,
    // matched into it: see `rules::merge_cast`.
    rules::merge_cast(
        &mut into.credits,
        &mut into.people,
        other.credits,
        other.people,
    );

    // Likewise what a work is filed beside: one provider's relations, whole.
    if into.relations.is_empty() {
        into.relations = other.relations;
    }

    // The spine: whichever provider came first and had a list keeps it. Only
    // when it had none does a later provider supply one.
    if into.seasons.is_empty() {
        into.seasons = other.seasons;
    } else {
        let alike = Alike::of(&into.episodes, &other.episodes, by_day);
        merge_seasons(&mut into.seasons, other.seasons, &alike);
    }

    if into.episodes.is_empty() {
        into.episodes = other.episodes;
    } else {
        merge_episodes(&mut into.episodes, other.episodes, by_day);
    }
}

/// What TVmaze knows about each episode, applied where it provably means the
/// same episode.
///
/// Same season, same number *and* the same broadcast date: TVmaze numbers some
/// series differently from TheTVDB, and a date is the one thing both sides can
/// check each other against. Where they agree, its instant replaces whatever
/// else was there — it keeps a time per episode, in the network's timezone,
/// where TheTVDB keeps one per series and Skyhook stamps it on every episode.
/// Measured on *The Big Bang Theory*: Skyhook puts all of season three at
/// 20:00 when it aired at 21:30. Where they do not agree, nothing is taken.
///
/// A date only tells episodes apart when it is theirs alone. On a day with
/// several — a double bill, a season released at once — the two sides line up
/// only if they count the same number of episodes that day; then the instants
/// are taken, in order, but not the text or the still, which would belong to a
/// neighbour if the order within the day differed. A day the sides count
/// differently is one where TVmaze numbers the release its own way, and
/// nothing from it is taken.
fn apply_broadcast_times(into: &mut [Episode], from: Vec<Episode>) {
    use std::collections::HashMap;

    fn per_day(episodes: &[Episode]) -> HashMap<(i32, String), usize> {
        let mut days = HashMap::new();
        for e in episodes {
            if let Some(date) = &e.air_date {
                *days.entry((e.season_number, date.clone())).or_default() += 1;
            }
        }
        days
    }

    let ours_per_day = per_day(into);
    let theirs_per_day = per_day(&from);

    for theirs in from {
        let Some(ours) = into.iter_mut().find(|e| {
            e.season_number == theirs.season_number && e.episode_number == theirs.episode_number
        }) else {
            continue;
        };

        let Some(date) = ours
            .air_date
            .clone()
            .filter(|d| Some(d) == theirs.air_date.as_ref())
        else {
            continue;
        };

        let day = (ours.season_number, date);
        let on_the_day = ours_per_day.get(&day).copied().unwrap_or(0);
        if theirs_per_day.get(&day).copied().unwrap_or(0) != on_the_day {
            continue;
        }

        if theirs.air_date_utc.is_some() {
            ours.air_date_utc = theirs.air_date_utc;
        }

        if on_the_day == 1 {
            fill(&mut ours.overview, theirs.overview);
            fill(&mut ours.image, theirs.image);
            ours.runtime = ours.runtime.or(theirs.runtime);
        }
    }
}

/// Fill an absent or blank optional string.
fn fill(into: &mut Option<String>, other: Option<String>) {
    let empty = into.as_deref().map(str::trim).is_none_or(str::is_empty);

    if empty && let Some(value) = other.filter(|v| !v.trim().is_empty()) {
        *into = Some(value);
    }
}

/// Identifiers are additive: each provider knows some the others do not.
pub(crate) fn merge_ids(into: &mut ExternalIds, other: ExternalIds) {
    into.tmdb = into.tmdb.or(other.tmdb);
    into.tvdb = into.tvdb.or(other.tvdb);
    into.imdb = into.imdb.take().or(other.imdb);
    into.tvmaze = into.tvmaze.or(other.tvmaze);
    into.tvrage = into.tvrage.or(other.tvrage);
    into.trakt = into.trakt.or(other.trakt);
    into.wikidata = into.wikidata.take().or(other.wikidata);

    for id in other.mal {
        if !into.mal.contains(&id) {
            into.mal.push(id);
        }
    }
    for id in other.anilist {
        if !into.anilist.contains(&id) {
            into.anilist.push(id);
        }
    }
}

/// Union artwork, keeping every provider's.
///
/// Sort order is preserved so the first provider's choice still leads: a lower
/// priority provider's poster is added after, not in front.
fn union_images(into: &mut Vec<Image>, other: Vec<Image>) {
    // Saturating: a sort order is a provider's number, and one at `i32::MAX`
    // panics a debug build and wraps a release one to the front.
    let offset = into
        .iter()
        .map(|i| i.sort_order)
        .max()
        .map(|m| m.saturating_add(1))
        .unwrap_or(0);

    for mut image in other {
        let duplicate = into
            .iter()
            .any(|existing| existing.url == image.url && existing.cover_type == image.cover_type);

        if duplicate {
            continue;
        }

        image.sort_order = image.sort_order.saturating_add(offset);
        into.push(image);
    }
}

/// Union alternative titles, one copy of each.
///
/// The same title filed for two countries is two entries, because a client may
/// ask for the one used in its own. A title filed for none says nothing a copy
/// filed for a country does not, so it is a duplicate of any — the anime sites
/// list every synonym that way, and *AOT* reached the catalogue three times.
fn union_alternative_titles(into: &mut Vec<AlternativeTitle>, other: Vec<AlternativeTitle>) {
    for title in other {
        let duplicate = into.iter().any(|e| {
            e.title.eq_ignore_ascii_case(&title.title)
                && (e.language == title.language
                    || e.language.is_none()
                    || title.language.is_none())
        });

        if !duplicate {
            into.push(title);
        }
    }
}

/// One rating per source. A second provider reporting `tmdb` does not replace
/// the first — they are the same number from the same place.
fn union_ratings(into: &mut Vec<Rating>, other: Vec<Rating>) {
    for rating in other {
        if !into.iter().any(|e| e.source == rating.source) {
            into.push(rating);
        }
    }
}

/// Every source's keywords, each once whatever its case: TMDB writes
/// `kaiju` where AniList writes `Kaiju`, and the first spelling stays.
fn union_keywords(into: &mut Vec<String>, other: Vec<String>) {
    for keyword in other {
        let keyword = keyword.trim();
        if keyword.is_empty() {
            continue;
        }
        let lower = keyword.to_lowercase();
        if !into.iter().any(|kept| kept.trim().to_lowercase() == lower) {
            into.push(keyword.to_string());
        }
    }
}

/// A country's releases come whole from the first source that lists any for
/// it, as a language's translation does — two sources' days for the same
/// country cannot be told apart from two releases. A later source's rating
/// fills a country the first gave days without one: TMDB lists a release in
/// France with no rating where Radarr has it.
fn union_releases(into: &mut Vec<CountryRelease>, other: Vec<CountryRelease>) {
    let held = |into: &[CountryRelease], country: &str| {
        into.iter().any(|r| r.country.eq_ignore_ascii_case(country))
    };
    let rated = |into: &[CountryRelease], country: &str| {
        into.iter().any(|r| {
            r.country.eq_ignore_ascii_case(country)
                && r.certification
                    .as_deref()
                    .is_some_and(|c| !c.trim().is_empty())
        })
    };

    let mut added: Vec<CountryRelease> = Vec::new();
    for release in other {
        if !held(into, &release.country) {
            added.push(release);
        } else if release
            .certification
            .as_deref()
            .is_some_and(|c| !c.trim().is_empty())
            && !rated(into, &release.country)
            && !rated(&added, &release.country)
        {
            added.push(CountryRelease {
                release_type: None,
                date: None,
                note: None,
                ..release
            });
        }
    }
    into.extend(added);
}

fn union_translations(into: &mut Vec<Translation>, other: Vec<Translation>) {
    for translation in other {
        if !into.iter().any(|e| e.language == translation.language) {
            into.push(translation);
        }
    }
}

/// A role's companies come whole from the first source that lists any in
/// it, as a list of credits does: two sources' lists of one role cannot be
/// blended in an order either would give. A later source's companies of a
/// role already listed complete those that are the same company — TheTVDB's
/// id of TMDB's AMC, TMDB's logo of TheTVDB's — and add nothing.
fn union_companies(into: &mut Vec<CompanyCredit>, other: Vec<CompanyCredit>) {
    let listed: Vec<crate::domain::CompanyRole> = into.iter().map(|c| c.role).collect();
    for credit in other {
        if !listed.contains(&credit.role) {
            into.push(credit);
            continue;
        }
        if let Some(same) = into
            .iter_mut()
            .find(|kept| kept.role == credit.role && kept.company.is_same(&credit.company))
        {
            same.company.complete(&credit.company);
        }
    }
}

/// Every source's videos, each once by its site and its key, the first
/// source's ahead: a later one's numbered on after them, as pictures are.
fn union_videos(into: &mut Vec<Video>, other: Vec<Video>) {
    let offset = into
        .iter()
        .map(|v| v.sort_order)
        .max()
        .map(|m| m.saturating_add(1))
        .unwrap_or(0);
    for mut video in other {
        if into
            .iter()
            .any(|kept| kept.site == video.site && kept.key == video.key)
        {
            continue;
        }
        video.sort_order = video.sort_order.saturating_add(offset);
        into.push(video);
    }
}

/// Fill seasons the spine already has. A season only another provider knows
/// about is dropped, for the same reason as episodes; so is one of the same
/// number that is another season ([`Alike`]).
fn merge_seasons(into: &mut [Season], other: Vec<Season>, alike: &Alike) {
    for season in other {
        if !alike.is(season.season_number) {
            continue;
        }
        let Some(existing) = into
            .iter_mut()
            .find(|s| s.season_number == season.season_number)
        else {
            continue;
        };

        fill(&mut existing.title, season.title);
        fill(&mut existing.overview, season.overview);
        fill(&mut existing.air_date, season.air_date);
        existing.tmdb_id = existing.tmdb_id.or(season.tmdb_id);
        existing.tvdb_id = existing.tvdb_id.or(season.tvdb_id);
        union_images(&mut existing.images, season.images);
        union_translations(&mut existing.translations, season.translations);
    }
}

/// Which of another provider's seasons are the list's of their number, as
/// their episodes say once paired ([`paired`]): those none of whose
/// episodes went into another of the list's seasons, and into whose number
/// of the list none of another season's came — every one, for a provider
/// that numbers the series as the list does, or that lists no episode to
/// tell by.
///
/// TMDB cuts One Piece into sagas where TheTVDB keeps arcs: TMDB's second
/// season is not TheTVDB's second, and its name, its synopsis, its first
/// day, its id, its pictures and its names in other languages are another
/// season's. Paired by number, TheTVDB's second took all of them, and the
/// clients that read a season's name in their language were given an arc
/// for another.
struct Alike {
    /// The other's seasons an episode of went into another of the list's.
    strayed: std::collections::HashSet<i32>,
    /// The list's seasons an episode of another season was paired into.
    crowded: std::collections::HashSet<i32>,
}

impl Alike {
    fn of(into: &[Episode], other: &[Episode], by_day: bool) -> Self {
        let mut alike = Alike {
            strayed: Default::default(),
            crowded: Default::default(),
        };
        for (episode, pair) in other.iter().zip(paired(into, other, by_day)) {
            if let Some(index) = pair
                && into[index].season_number != episode.season_number
            {
                alike.strayed.insert(episode.season_number);
                alike.crowded.insert(into[index].season_number);
            }
        }
        alike
    }

    fn is(&self, season: i32) -> bool {
        !self.strayed.contains(&season) && !self.crowded.contains(&season)
    }
}

/// Keep only those of another provider's specials that provably are one of the
/// spine's, each under the spine's number for it.
///
/// Season 0 is where numberings part. TheTVDB and TMDB file films, OVAs and
/// unaired episodes in orders of their own, and a special one of them lacks
/// moves every number after it. *Rurouni Kenshin*'s first special is a 1997
/// film on TheTVDB; on TMDB it is the series' last episode, which TheTVDB
/// counts in season 3. Paired by number, the film took that episode's
/// synopsis, its still and its TMDB id. The TMDB id kept here is also what
/// places TMDB's text in another language (`service::language`). The regular
/// seasons keep their numbers where the day each episode aired agrees with
/// them, and are paired by that day where it does not ([`paired`]).
///
/// What both sides can be checked against is the day a special came out:
///
/// * the only special either side has on a day is the same one, whatever
///   number each gives it;
/// * on a day with several, they pair by number, and only when both sides
///   number that day's specials alike — a shift by one would otherwise hand
///   each part its neighbour's text, as on TVmaze's double bills
///   (`apply_broadcast_times`);
/// * a special with no date, or on a day the spine has none, is nobody's: it
///   gives nothing, and is not added under a number that is another special's.
fn align_specials(spine: &[Episode], other: &mut Vec<Episode>) {
    use std::collections::HashMap;

    fn day(episode: &Episode) -> Option<&str> {
        episode
            .air_date
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
    }

    fn by_day(episodes: &[Episode]) -> HashMap<String, Vec<i32>> {
        let mut days: HashMap<String, Vec<i32>> = HashMap::new();
        for e in episodes.iter().filter(|e| e.season_number == 0) {
            if let Some(date) = day(e) {
                days.entry(date.to_string())
                    .or_default()
                    .push(e.episode_number);
            }
        }
        for numbers in days.values_mut() {
            numbers.sort_unstable();
        }
        days
    }

    let ours = by_day(spine);
    let theirs = by_day(other);

    other.retain_mut(|e| {
        if e.season_number != 0 {
            return true;
        }
        let Some(date) = day(e) else {
            return false;
        };
        let (Some(ours), Some(theirs)) = (ours.get(date), theirs.get(date)) else {
            return false;
        };
        e.episode_number = match (ours.as_slice(), theirs.as_slice()) {
            ([only], [_]) => *only,
            _ if ours == theirs => e.episode_number,
            _ => return false,
        };
        true
    });
}

/// Merge episodes field by field, keyed by their numbering — checked against
/// the day each aired for a provider that numbers the series its own way
/// (`by_day`).
///
/// This is where a second provider earns its place: TVDB carries absolute
/// numbering and air-order hints that TMDB has no field for, while TMDB carries
/// overviews and stills that TVDB often lacks.
///
/// But TMDB cuts some series into seasons of its own — One Piece into 23 where
/// TheTVDB has 21, an anime's second cour into a season of its own — and its
/// 2×01 is then TheTVDB's 1×13. Paired by number alone, TheTVDB's 2×01 took
/// that episode's TMDB id, and with it its synopsis, its still and the text
/// read in another language by that id (`service::language`); TMDB's episode
/// groups, and the TMDB relay's locks, placed by that id, followed it. See
/// [`paired`] for how the day decides.
fn merge_episodes(into: &mut [Episode], other: Vec<Episode>, by_day: bool) {
    let pairs = paired(into, &other, by_day);
    for (episode, pair) in other.into_iter().zip(pairs) {
        if let Some(existing) = pair.map(|index| &mut into[index]) {
            if existing.title.trim().is_empty() {
                existing.title = episode.title;
            }
            fill(&mut existing.overview, episode.overview);
            fill(&mut existing.air_date, episode.air_date);
            fill(&mut existing.air_date_utc, episode.air_date_utc);
            fill(&mut existing.finale_type, episode.finale_type);
            fill(&mut existing.image, episode.image);

            existing.runtime = existing.runtime.or(episode.runtime);
            existing.tvdb_id = existing.tvdb_id.or(episode.tvdb_id);
            existing.tmdb_id = existing.tmdb_id.or(episode.tmdb_id);
            existing.rating = existing.rating.or(episode.rating);

            // The reason this merge exists.
            existing.absolute_episode_number = existing
                .absolute_episode_number
                .or(episode.absolute_episode_number);
            existing.aired_after_season_number = existing
                .aired_after_season_number
                .or(episode.aired_after_season_number);
            existing.aired_before_season_number = existing
                .aired_before_season_number
                .or(episode.aired_before_season_number);
            existing.aired_before_episode_number = existing
                .aired_before_episode_number
                .or(episode.aired_before_episode_number);
        }
    }
}

/// Which of the list's episodes each of another provider's is, by its index
/// in the list: the one of its season and number — and, `by_day`, as the day
/// it aired says:
///
/// * the one of its number, when the two aired within a day of each other —
///   a broadcast across midnight, or two time zones apart;
/// * else the list's only episode that aired that day; on a day the list has
///   several, the one of its rank in the day, when both sides list as many
///   that day — a double bill each numbers its own way;
/// * else, the list having none that day, its only one of the day before or
///   after;
/// * else the one of its number, when that one has no day to tell by;
/// * else none: an episode the day speaks against, and that nothing else
///   claims, gives nothing.
///
/// An episode with no day of its own keeps its number: nothing says otherwise.
fn paired(into: &[Episode], other: &[Episode], by_day: bool) -> Vec<Option<usize>> {
    use std::collections::HashMap;

    use chrono::NaiveDate;

    fn day(episode: &Episode) -> Option<NaiveDate> {
        let date = episode.air_date.as_deref()?.trim();
        NaiveDate::parse_from_str(date.get(..10)?, "%Y-%m-%d").ok()
    }

    // A list's episodes by the day they aired, each day's in number order.
    fn by_days(episodes: &[Episode]) -> HashMap<NaiveDate, Vec<usize>> {
        let mut days: HashMap<NaiveDate, Vec<usize>> = HashMap::new();
        for (index, episode) in episodes.iter().enumerate() {
            if let Some(aired) = day(episode) {
                days.entry(aired).or_default().push(index);
            }
        }
        for indices in days.values_mut() {
            indices.sort_by_key(|&i| (episodes[i].season_number, episodes[i].episode_number));
        }
        days
    }

    let numbers: HashMap<(i32, i32), usize> = into
        .iter()
        .enumerate()
        .map(|(index, e)| ((e.season_number, e.episode_number), index))
        .collect();
    let by_number = |e: &Episode| numbers.get(&(e.season_number, e.episode_number)).copied();

    if !by_day {
        return other.iter().map(by_number).collect();
    }

    let ours = by_days(into);
    let theirs = by_days(other);

    other
        .iter()
        .enumerate()
        .map(|(index, episode)| {
            let numbered = by_number(episode);
            let Some(aired) = day(episode) else {
                return numbered;
            };
            let numbered_day = numbered.map(|i| day(&into[i]));
            if let Some(Some(listed)) = numbered_day
                && (listed - aired).num_days().abs() <= 1
            {
                return numbered;
            }

            if let Some(that_day) = ours.get(&aired) {
                if let [only] = that_day.as_slice() {
                    return Some(*only);
                }
                let mine = theirs.get(&aired)?;
                return (mine.len() == that_day.len())
                    .then(|| mine.iter().position(|&i| i == index))
                    .flatten()
                    .map(|rank| that_day[rank]);
            }

            let near: Vec<usize> = [aired.pred_opt(), aired.succ_opt()]
                .into_iter()
                .flatten()
                .filter_map(|d| ours.get(&d))
                .flatten()
                .copied()
                .collect();
            if let [only] = near.as_slice() {
                return Some(*only);
            }

            match numbered_day {
                Some(None) => numbered,
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CoverType, CreditType, MediaKind, RatingValue};

    fn priority() -> Vec<String> {
        ["tmdb", "tvdb", "skyhook", "fanart"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn base(provider: &str, title: &str) -> Contribution {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = title.into();
        Contribution {
            provider: provider.into(),
            item,
        }
    }

    fn episode(season: i32, number: i32) -> Episode {
        crate::db::repo::child::blank_episode(season, number)
    }

    fn image(kind: CoverType, url: &str) -> Image {
        let mut i = crate::db::repo::child::blank_image(kind, url.into());
        i.is_manual = false;
        i
    }

    #[test]
    fn what_thetvdb_adds_to_a_homonyms_name_is_kept() {
        // TMDB names both series "Rurouni Kenshin"; TheTVDB, and Skyhook after
        // it, the second "Rurouni Kenshin (2023)".
        let mut tmdb = base("tmdb", "Rurouni Kenshin");
        tmdb.item.original_title = Some("るろうに剣心 －明治剣客浪漫譚－".into());
        let merged = combine(
            vec![
                tmdb,
                base("tvdb", "るろうに剣心 －明治剣客浪漫譚－ (2023)"),
                base("skyhook", "Rurouni Kenshin (2023)"),
            ],
            &priority(),
        )
        .unwrap();
        assert_eq!(merged.title, "Rurouni Kenshin");
        assert_eq!(merged.title_qualifier.as_deref(), Some("2023"));

        // The first of the name has none, as on TheTVDB.
        let merged = combine(
            vec![
                base("tmdb", "Rurouni Kenshin"),
                base("skyhook", "Rurouni Kenshin"),
            ],
            &priority(),
        )
        .unwrap();
        assert_eq!(merged.title_qualifier, None);

        // A country tells them apart too.
        let merged = combine(
            vec![base("tmdb", "The Office"), base("tvdb", "The Office (US)")],
            &priority(),
        )
        .unwrap();
        assert_eq!(merged.title_qualifier.as_deref(), Some("US"));
    }

    #[test]
    fn a_title_that_ends_otherwise_is_no_qualifier() {
        // Part of the name itself, or another name altogether.
        let merged = combine(
            vec![
                base("tmdb", "Kaguya-sama"),
                base("tvdb", "Kaguya-sama (Love Is War)"),
                base("skyhook", "Something Else (2020)"),
            ],
            &priority(),
        )
        .unwrap();
        assert_eq!(merged.title_qualifier, None);
        assert_eq!(split_qualifier("Title (1850)"), None);
        assert_eq!(split_qualifier("Title (us)"), None);
        assert_eq!(split_qualifier("(2023)"), None);
    }

    #[test]
    fn an_air_time_from_a_provider_that_knows_it_reaches_the_spine() {
        // TheTVDB has the date and not the time; Skyhook has both. The spine is
        // TheTVDB's, and its episodes used to carry an invented midnight UTC
        // that the fill-only merge then kept — so all eighty episodes of
        // Breaking Bad reached Sonarr twenty-six hours early.
        let mut tvdb = base("tvdb", "Breaking Bad");
        let mut pilot = episode(1, 1);
        pilot.air_date = Some("2008-01-20".into());
        pilot.air_date_utc = None;
        tvdb.item.episodes = vec![pilot];

        let mut skyhook = base("skyhook", "Breaking Bad");
        let mut known = episode(1, 1);
        known.air_date = Some("2008-01-20".into());
        known.air_date_utc = Some("2008-01-21T02:00:00Z".into());
        skyhook.item.episodes = vec![known];

        let merged = combine(vec![tvdb, skyhook], &priority()).unwrap();

        assert_eq!(
            merged.episodes[0].air_date_utc.as_deref(),
            Some("2008-01-21T02:00:00Z")
        );
    }

    #[test]
    fn a_numbered_provider_with_no_episodes_does_not_take_the_others_down_with_it() {
        // TheTVDB's episode endpoint failed while the rest of its answer came
        // back, so it carries seasons and nothing else. It is still the one that
        // settles numbering — but it has not settled any, and letting it be the
        // spine here emptied the merged list and deleted the stored episodes.
        let mut tvdb = base("tvdb", "Breaking Bad");
        tvdb.item.seasons = vec![
            crate::db::repo::child::blank_season(1),
            crate::db::repo::child::blank_season(2),
        ];

        let mut tmdb = base("tmdb", "Breaking Bad");
        tmdb.item.episodes = vec![episode(1, 1), episode(1, 2), episode(2, 1)];

        let merged = combine(vec![tvdb, tmdb], &priority()).unwrap();

        assert_eq!(merged.episodes.len(), 3, "TMDB's episodes survived");
        assert_eq!(merged.seasons.len(), 2, "TheTVDB's seasons still applied");
    }

    #[test]
    fn a_numbered_provider_that_did_answer_still_wins_the_numbering() {
        // The rule the constant exists for, unchanged: when TheTVDB has a list,
        // it is the list, whatever the general priority says.
        let mut tvdb = base("tvdb", "One Piece");
        tvdb.item.episodes = vec![episode(1, 1), episode(1, 2)];

        let mut tmdb = base("tmdb", "One Piece");
        tmdb.item.episodes = vec![episode(1, 1), episode(2, 1), episode(3, 1)];

        let merged = combine(vec![tvdb, tmdb], &priority()).unwrap();

        assert_eq!(merged.episodes.len(), 2, "TheTVDB's numbering, not TMDB's");
    }

    #[test]
    fn the_highest_priority_provider_supplies_the_shape() {
        let mut low = base("skyhook", "From Skyhook");
        low.item.overview = Some("Skyhook's overview.".into());

        let mut high = base("tmdb", "From TMDB");
        high.item.overview = Some("TMDB's overview.".into());

        // Deliberately out of order: priority decides, not argument order.
        let merged = combine(vec![low, high], &priority()).unwrap();

        assert_eq!(merged.title, "From TMDB");
        assert_eq!(merged.overview.as_deref(), Some("TMDB's overview."));
    }

    #[test]
    fn a_lower_priority_provider_fills_gaps_without_overwriting() {
        let mut high = base("tmdb", "Title");
        high.item.overview = Some("Kept.".into());
        high.item.network = None;

        let mut low = base("skyhook", "Other Title");
        low.item.overview = Some("Discarded.".into());
        low.item.network = Some("AMC".into());
        low.item.air_time = Some("21:00".into());

        let merged = combine(vec![high, low], &priority()).unwrap();

        assert_eq!(merged.overview.as_deref(), Some("Kept."));
        assert_eq!(merged.network.as_deref(), Some("AMC"), "a gap was filled");
        assert_eq!(merged.air_time.as_deref(), Some("21:00"));
    }

    #[test]
    fn a_blank_value_counts_as_missing() {
        let mut high = base("tmdb", "Title");
        high.item.overview = Some("   ".into());

        let mut low = base("skyhook", "Title");
        low.item.overview = Some("Real text.".into());

        let merged = combine(vec![high, low], &priority()).unwrap();
        assert_eq!(merged.overview.as_deref(), Some("Real text."));
    }

    #[test]
    fn identifiers_from_every_provider_are_kept() {
        let mut a = base("tmdb", "T");
        a.item.external_ids = ExternalIds {
            tmdb: Some(1396),
            ..Default::default()
        };

        let mut b = base("skyhook", "T");
        b.item.external_ids = ExternalIds {
            tvdb: Some(81189),
            imdb: Some("tt0903747".into()),
            tvmaze: Some(169),
            mal: vec![7],
            ..Default::default()
        };

        let merged = combine(vec![a, b], &priority()).unwrap();

        assert_eq!(merged.external_ids.tmdb, Some(1396));
        assert_eq!(merged.external_ids.tvdb, Some(81189));
        assert_eq!(merged.external_ids.imdb.as_deref(), Some("tt0903747"));
        assert_eq!(merged.external_ids.tvmaze, Some(169));
        assert_eq!(merged.external_ids.mal, vec![7]);
    }

    #[test]
    fn artwork_is_unioned_with_the_first_providers_choice_still_leading() {
        // This is the whole reason for an artwork provider.
        let mut a = base("tmdb", "T");
        a.item.images = vec![image(CoverType::Poster, "https://tmdb/p.jpg")];

        let mut b = base("fanart", "T");
        b.item.images = vec![
            image(CoverType::Poster, "https://fanart/p.jpg"),
            image(CoverType::Clearlogo, "https://fanart/logo.png"),
        ];

        let merged = combine(vec![a, b], &priority()).unwrap();

        assert_eq!(merged.images.len(), 3);
        let posters: Vec<&str> = merged
            .images
            .iter()
            .filter(|i| i.cover_type == CoverType::Poster)
            .map(|i| i.url.as_str())
            .collect();
        assert_eq!(posters, vec!["https://tmdb/p.jpg", "https://fanart/p.jpg"]);
        assert!(
            merged
                .images
                .iter()
                .any(|i| i.cover_type == CoverType::Clearlogo)
        );
    }

    #[test]
    fn the_same_image_from_two_providers_is_not_listed_twice() {
        let mut a = base("tmdb", "T");
        a.item.images = vec![image(CoverType::Poster, "https://same/p.jpg")];
        let mut b = base("skyhook", "T");
        b.item.images = vec![image(CoverType::Poster, "https://same/p.jpg")];

        assert_eq!(combine(vec![a, b], &priority()).unwrap().images.len(), 1);
    }

    #[test]
    fn credits_come_from_one_provider_rather_than_being_blended() {
        // Nothing identifies a person across providers, so a union would list
        // the same actor twice under slightly different spellings.
        let mut a = base("tmdb", "T");
        a.item.credits = vec![crate::db::repo::child::blank_credit(
            CreditType::Actor,
            "Bryan Cranston".into(),
        )];

        let mut b = base("skyhook", "T");
        b.item.credits = vec![
            crate::db::repo::child::blank_credit(CreditType::Actor, "Bryan Cranston".into()),
            crate::db::repo::child::blank_credit(CreditType::Actor, "Aaron Paul".into()),
        ];

        let merged = combine(vec![a, b], &priority()).unwrap();
        assert_eq!(merged.credits.len(), 1);
        assert_eq!(merged.credits[0].person_name, "Bryan Cranston");
    }

    #[test]
    fn related_works_come_from_the_first_provider_that_has_them() {
        let related = |external_id: i64| crate::domain::Relation {
            id: String::new(),
            relation_type: "SEQUEL".into(),
            source: "anilist".into(),
            external_id,
            mal_id: None,
            title: "Next".into(),
            medium: "anime".into(),
            format: None,
            year: None,
            image: None,
            is_adult: false,
            work_id: None,
            sort_order: 0,
        };
        let a = base("tmdb", "T");
        let mut b = base("anilist", "T");
        b.item.relations = vec![related(1)];
        let mut c = base("mal", "T");
        c.item.relations = vec![related(2), related(3)];

        let merged = combine(vec![a, b, c], &priority()).unwrap();
        assert_eq!(
            merged
                .relations
                .iter()
                .map(|r| r.external_id)
                .collect::<Vec<_>>(),
            [1]
        );
    }

    #[test]
    fn credits_are_taken_from_the_next_provider_when_the_first_has_none() {
        let a = base("tmdb", "T");
        let mut b = base("skyhook", "T");
        b.item.credits = vec![crate::db::repo::child::blank_credit(
            CreditType::Actor,
            "Aaron Paul".into(),
        )];

        let merged = combine(vec![a, b], &priority()).unwrap();
        assert_eq!(merged.credits.len(), 1);
    }

    #[test]
    fn an_episode_gathers_what_each_provider_knows() {
        // TVDB supplies the list and the absolute number; TMDB fills the gaps
        // in it — the overview and the still TVDB often lacks.
        let mut a = base("tmdb", "T");
        let mut from_tmdb = episode(1, 1);
        from_tmdb.title = "Pilot".into();
        from_tmdb.overview = Some("It begins.".into());
        from_tmdb.image = Some("https://tmdb/still.jpg".into());
        a.item.episodes = vec![from_tmdb];

        let mut b = base("tvdb", "T");
        let mut from_tvdb = episode(1, 1);
        from_tvdb.absolute_episode_number = Some(1);
        from_tvdb.tvdb_id = Some(349232);
        from_tvdb.runtime = Some(58);
        b.item.episodes = vec![from_tvdb];

        let merged = combine(vec![a, b], &priority()).unwrap();
        assert_eq!(merged.episodes.len(), 1);

        let e = &merged.episodes[0];
        assert_eq!(e.title, "Pilot", "TVDB had none, so TMDB's filled in");
        assert_eq!(e.overview.as_deref(), Some("It begins."));
        assert_eq!(e.image.as_deref(), Some("https://tmdb/still.jpg"));
        assert_eq!(e.absolute_episode_number, Some(1));
        assert_eq!(e.tvdb_id, Some(349232));
        assert_eq!(e.runtime, Some(58));
    }

    #[test]
    fn the_episode_list_comes_from_a_tvdb_numbered_provider() {
        // Clients address a series by its TVDB id, so the numbering has to be
        // TVDB's even though TMDB outranks it for everything else.
        let mut tmdb = base("tmdb", "From TMDB");
        tmdb.item.episodes = vec![episode(1, 1), episode(1, 2), episode(1, 3)];

        let mut skyhook = base("skyhook", "From Skyhook");
        let mut only = episode(1, 1);
        only.title = "TVDB numbering".into();
        skyhook.item.episodes = vec![only];

        let merged = combine(vec![tmdb, skyhook], &priority()).unwrap();

        assert_eq!(merged.title, "From TMDB", "scalars still follow priority");
        assert_eq!(merged.episodes.len(), 1, "but the list is Skyhook's");
        assert_eq!(merged.episodes[0].title, "TVDB numbering");
    }

    #[test]
    fn tmdbs_numbering_is_used_when_nothing_tvdb_numbered_answered() {
        let mut tmdb = base("tmdb", "T");
        tmdb.item.episodes = vec![episode(1, 1), episode(1, 2)];
        let fanart = base("fanart", "T");

        let merged = combine(vec![tmdb, fanart], &priority()).unwrap();
        assert_eq!(merged.episodes.len(), 2);
    }

    #[test]
    fn a_second_provider_does_not_add_episodes_to_the_list() {
        // Providers disagree about season boundaries. Unioning One Piece's 1179
        // TVDB episodes with TMDB's 1181 produced 2352, most of them phantoms.
        let mut a = base("tmdb", "T");
        a.item.episodes = vec![episode(1, 1)];

        let mut b = base("tvdb", "T");
        b.item.episodes = vec![episode(1, 1), episode(1, 2), episode(9, 9)];

        // TVDB supplies the list here, so TMDB's single episode is what gets
        // dropped — the rule is one list, not "the first one".
        let merged = combine(vec![a, b], &priority()).unwrap();

        assert_eq!(merged.episodes.len(), 3);
    }

    #[test]
    fn the_list_comes_from_the_next_provider_when_the_first_has_none() {
        // Fanart.tv contributes artwork and no episodes at all; that must not
        // leave a series with an empty run.
        let a = base("fanart", "T");
        let mut b = base("tmdb", "T");
        b.item.episodes = vec![episode(2, 1), episode(1, 1)];

        // Ordered so the episode-less provider is folded in first.
        let merged = combine(vec![b, a], &priority()).unwrap();

        assert_eq!(merged.episodes.len(), 2);
        let order: Vec<(i32, i32)> = merged
            .episodes
            .iter()
            .map(|e| (e.season_number, e.episode_number))
            .collect();
        assert_eq!(order, vec![(1, 1), (2, 1)]);
    }

    #[test]
    fn a_season_outside_the_spine_is_not_added() {
        // TMDB outranks Skyhook everywhere else, and still cannot add a season
        // to a list Skyhook owns — only fill the ones already in it.
        let mut a = base("tmdb", "T");
        a.item.seasons = vec![
            Season {
                id: "s".into(),
                season_number: 1,
                title: Some("Named".into()),
                overview: None,
                air_date: None,
                tmdb_id: None,
                tvdb_id: Some(7),
                is_manual: false,
                images: Vec::new(),
                primary_images: Default::default(),
                translations: Vec::new(),
            },
            Season {
                id: "s".into(),
                season_number: 42,
                title: None,
                overview: None,
                air_date: None,
                tmdb_id: None,
                tvdb_id: None,
                is_manual: false,
                images: Vec::new(),
                primary_images: Default::default(),
                translations: Vec::new(),
            },
        ];

        let mut b = base("skyhook", "T");
        b.item.seasons = vec![Season {
            id: "s".into(),
            season_number: 1,
            title: None,
            overview: None,
            air_date: None,
            tmdb_id: None,
            tvdb_id: None,
            is_manual: false,
            images: Vec::new(),
            primary_images: Default::default(),
            translations: Vec::new(),
        }];

        let merged = combine(vec![a, b], &priority()).unwrap();

        assert_eq!(merged.seasons.len(), 1);
        assert_eq!(
            merged.seasons[0].title.as_deref(),
            Some("Named"),
            "fields still fill"
        );
        assert_eq!(merged.seasons[0].tvdb_id, Some(7));
    }

    #[test]
    fn ratings_from_different_sources_are_all_kept() {
        let mut a = base("tmdb", "T");
        a.item.ratings = vec![Rating {
            source: "tmdb".into(),
            value: Some(8.9),
            votes: Some(100),
            rating_type: None,
        }];

        let mut b = base("skyhook", "T");
        b.item.ratings = vec![
            Rating {
                source: "tmdb".into(),
                value: Some(1.0),
                votes: Some(1),
                rating_type: None,
            },
            Rating {
                source: "tvdb".into(),
                value: Some(9.1),
                votes: Some(50),
                rating_type: None,
            },
        ];

        let merged = combine(vec![a, b], &priority()).unwrap();

        assert_eq!(merged.ratings.len(), 2);
        let tmdb = merged.ratings.iter().find(|r| r.source == "tmdb").unwrap();
        assert_eq!(tmdb.value, Some(8.9), "the trusted provider's number");
    }

    #[test]
    fn an_unnamed_provider_still_contributes() {
        let a = base("tmdb", "T");
        let mut b = base("something-new", "T");
        b.item.network = Some("A Network".into());

        let merged = combine(vec![a, b], &priority()).unwrap();
        assert_eq!(merged.network.as_deref(), Some("A Network"));
    }

    #[test]
    fn one_contribution_passes_through_unchanged() {
        let mut only = base("tmdb", "Alone");
        only.item.episodes = vec![episode(1, 1)];
        only.item.rating("tmdb");

        let merged = combine(vec![only], &priority()).unwrap();
        assert_eq!(merged.title, "Alone");
        assert_eq!(merged.episodes.len(), 1);
    }

    #[test]
    fn nothing_to_merge_yields_nothing() {
        assert!(combine(Vec::new(), &priority()).is_none());
    }

    #[test]
    fn episode_ratings_are_filled_rather_than_replaced() {
        let mut a = base("tmdb", "T");
        let mut rated = episode(1, 1);
        rated.rating = Some(RatingValue {
            value: 8.0,
            votes: 10,
        });
        a.item.episodes = vec![rated];

        let mut b = base("tvdb", "T");
        let mut other = episode(1, 1);
        other.rating = Some(RatingValue {
            value: 9.0,
            votes: 20,
        });
        b.item.episodes = vec![other];

        // TVDB owns the list here, so its rating is the one already in place.
        let merged = combine(vec![a, b], &priority()).unwrap();
        assert_eq!(merged.episodes[0].rating.map(|r| r.value), Some(9.0));
    }

    /// An episode as TVmaze gives it: a date, and the moment on that date.
    fn broadcast(season: i32, number: i32, date: &str, instant: &str) -> Episode {
        let mut e = episode(season, number);
        e.air_date = Some(date.into());
        e.air_date_utc = Some(instant.into());
        e
    }

    #[test]
    fn a_broadcast_time_replaces_the_series_slot_when_the_dates_agree() {
        // Skyhook stamps the series' time slot on every episode; TVmaze knows
        // the slot moved. Measured on The Big Bang Theory, season three.
        let mut skyhook = base("skyhook", "The Big Bang Theory");
        skyhook.item.episodes = vec![broadcast(3, 1, "2009-09-21", "2009-09-22T00:00:00Z")];

        let mut tvmaze = base("tvmaze", "The Big Bang Theory");
        tvmaze.item.episodes = vec![broadcast(3, 1, "2009-09-21", "2009-09-22T01:30:00Z")];

        let merged = combine(vec![skyhook, tvmaze], &priority()).unwrap();

        assert_eq!(
            merged.episodes[0].air_date_utc.as_deref(),
            Some("2009-09-22T01:30:00Z")
        );
    }

    #[test]
    fn a_broadcast_time_for_a_different_date_is_not_taken() {
        // Same numbers, different days: TVmaze means another episode. Nothing
        // it says about this one can be trusted, time or text.
        let mut tvdb = base("tvdb", "One Piece");
        let mut ours = broadcast(1, 1, "1999-10-20", "1999-10-20T10:30:00Z");
        ours.overview = None;
        tvdb.item.episodes = vec![ours];

        let mut tvmaze = base("tvmaze", "One Piece");
        let mut theirs = broadcast(1, 1, "2004-09-18", "2004-09-18T14:00:00Z");
        theirs.overview = Some("Another episode entirely.".into());
        tvmaze.item.episodes = vec![theirs];

        let merged = combine(vec![tvdb, tvmaze], &priority()).unwrap();

        assert_eq!(
            merged.episodes[0].air_date_utc.as_deref(),
            Some("1999-10-20T10:30:00Z")
        );
        assert_eq!(merged.episodes[0].overview, None);
    }

    #[test]
    fn a_broadcast_fills_what_the_spine_is_missing_about_the_same_episode() {
        let mut tvdb = base("tvdb", "Breaking Bad");
        let mut pilot = episode(1, 1);
        pilot.air_date = Some("2008-01-20".into());
        pilot.overview = None;
        tvdb.item.episodes = vec![pilot];

        let mut tvmaze = base("tvmaze", "Breaking Bad");
        let mut theirs = broadcast(1, 1, "2008-01-20", "2008-01-21T03:00:00Z");
        theirs.overview = Some("A teacher learns he is ill.".into());
        theirs.runtime = Some(58);
        tvmaze.item.episodes = vec![theirs];

        let merged = combine(vec![tvdb, tvmaze], &priority()).unwrap();
        let pilot = &merged.episodes[0];

        assert_eq!(pilot.air_date_utc.as_deref(), Some("2008-01-21T03:00:00Z"));
        assert_eq!(
            pilot.overview.as_deref(),
            Some("A teacher learns he is ill.")
        );
        assert_eq!(pilot.runtime, Some(58));
    }

    #[test]
    fn on_a_double_bill_the_instants_are_taken_but_not_the_text() {
        let mut tvdb = base("tvdb", "Finale");
        tvdb.item.episodes = (23..=24)
            .map(|n| {
                let mut e = episode(12, n);
                e.air_date = Some("2019-05-16".into());
                e.overview = None;
                e
            })
            .collect();

        let mut tvmaze = base("tvmaze", "Finale");
        tvmaze.item.episodes = vec![
            broadcast(12, 23, "2019-05-16", "2019-05-17T00:00:00Z"),
            broadcast(12, 24, "2019-05-16", "2019-05-17T00:30:00Z"),
        ];
        for e in &mut tvmaze.item.episodes {
            e.overview = Some("Which of the two is this?".into());
        }

        let merged = combine(vec![tvdb, tvmaze], &priority()).unwrap();

        let instants: Vec<_> = merged
            .episodes
            .iter()
            .map(|e| e.air_date_utc.as_deref())
            .collect();
        assert_eq!(
            instants,
            [Some("2019-05-17T00:00:00Z"), Some("2019-05-17T00:30:00Z")]
        );
        assert!(merged.episodes.iter().all(|e| e.overview.is_none()));
    }

    #[test]
    fn a_release_day_the_sides_count_differently_is_left_alone() {
        // A season released at once, where TVmaze counts a two-part premiere
        // as one episode: its numbers are one behind from the second on.
        let mut tvdb = base("tvdb", "Binge");
        tvdb.item.episodes = (1..=3)
            .map(|n| {
                let mut e = episode(1, n);
                e.air_date = Some("2021-09-17".into());
                e.air_date_utc = Some("2021-09-17T07:00:00Z".into());
                e.overview = None;
                e
            })
            .collect();

        let mut tvmaze = base("tvmaze", "Binge");
        tvmaze.item.episodes = (1..=2)
            .map(|n| {
                let mut e = broadcast(1, n, "2021-09-17", "2021-09-17T08:00:00Z");
                e.overview = Some(format!("TVmaze's episode {n}"));
                e
            })
            .collect();

        let merged = combine(vec![tvdb, tvmaze], &priority()).unwrap();

        assert!(merged.episodes.iter().all(|e| e.air_date_utc.as_deref()
            == Some("2021-09-17T07:00:00Z")
            && e.overview.is_none()));
    }

    #[test]
    fn tvmaze_never_supplies_the_episode_list() {
        // Its numbering is its own, so even when it is the only provider with
        // episodes it does not become the list Sonarr maps files against.
        let tmdb = base("tmdb", "Somewhere");

        let mut tvmaze = base("tvmaze", "Somewhere");
        tvmaze.item.episodes = vec![
            broadcast(1, 1, "2020-01-01", "2020-01-01T20:00:00Z"),
            broadcast(1, 2, "2020-01-08", "2020-01-08T20:00:00Z"),
        ];

        let merged = combine(vec![tmdb, tvmaze], &priority()).unwrap();

        assert!(merged.episodes.is_empty());
        assert!(merged.seasons.is_empty());
    }

    #[test]
    fn tvmaze_does_not_add_episodes_the_spine_does_not_have() {
        let mut tvdb = base("tvdb", "Short");
        let mut first = episode(1, 1);
        first.air_date = Some("2020-01-01".into());
        tvdb.item.episodes = vec![first];

        let mut tvmaze = base("tvmaze", "Short");
        tvmaze.item.episodes = vec![
            broadcast(1, 1, "2020-01-01", "2020-01-01T20:00:00Z"),
            broadcast(1, 2, "2020-01-08", "2020-01-08T20:00:00Z"),
        ];

        let merged = combine(vec![tvdb, tvmaze], &priority()).unwrap();

        assert_eq!(merged.episodes.len(), 1);
    }

    /// A special as TheTVDB lists one: a number, the day it came out, a name.
    /// One as a provider lists it: its numbers, the day it aired, and — from
    /// TMDB — its id and a synopsis naming it.
    fn aired(season: i32, number: i32, date: Option<&str>, tmdb_id: Option<i64>) -> Episode {
        let mut e = episode(season, number);
        e.air_date = date.map(String::from);
        e.tmdb_id = tmdb_id;
        e.overview = tmdb_id.map(|id| format!("TMDB's {id}."));
        e
    }

    fn tmdb_ids(merged: &MediaItem) -> Vec<Option<i64>> {
        merged.episodes.iter().map(|e| e.tmdb_id).collect()
    }

    #[test]
    fn a_season_tmdb_cuts_otherwise_is_paired_by_the_day_each_episode_aired() {
        // TMDB makes a season of TheTVDB's last two episodes of the first, so
        // its 2×01 is TheTVDB's 1×03. By number, TheTVDB's 2×01 took TMDB's
        // 2×01 — its id, its synopsis — and 1×03 took nothing. By the day,
        // each takes its own; TMDB's 3×02, which aired on no day TheTVDB
        // lists, gives nothing.
        let mut tvdb = base("tvdb", "Cut");
        tvdb.item.episodes = vec![
            aired(1, 1, Some("2024-01-07"), None),
            aired(1, 2, Some("2024-01-14"), None),
            aired(1, 3, Some("2024-01-21"), None),
            aired(1, 4, Some("2024-01-28"), None),
            aired(2, 1, Some("2024-07-07"), None),
        ];
        let mut tmdb = base("tmdb", "Cut");
        tmdb.item.episodes = vec![
            aired(1, 1, Some("2024-01-07"), Some(101)),
            aired(1, 2, Some("2024-01-14"), Some(102)),
            aired(2, 1, Some("2024-01-21"), Some(103)),
            aired(2, 2, Some("2024-01-28"), Some(104)),
            aired(3, 1, Some("2024-07-07"), Some(105)),
            aired(3, 2, Some("2024-07-31"), Some(106)),
        ];

        let merged = combine(vec![tmdb, tvdb], &priority()).unwrap();
        assert_eq!(merged.episodes.len(), 5, "TheTVDB's list");
        assert_eq!(
            tmdb_ids(&merged),
            [Some(101), Some(102), Some(103), Some(104), Some(105)]
        );
        assert_eq!(merged.episodes[2].overview.as_deref(), Some("TMDB's 103."));
        assert_eq!(merged.episodes[4].overview.as_deref(), Some("TMDB's 105."));
    }

    #[test]
    fn a_season_tmdb_cuts_otherwise_lends_the_list_nothing_of_its_own() {
        // TMDB's first season holds TheTVDB's first and the start of its
        // second: its name, its id and its names in other languages are a
        // saga's, not either arc's. TheTVDB's third is TMDB's third, cut
        // alike: it takes what TMDB has of it.
        let named = |number: i32, title: &str, tmdb_id: Option<i64>| Season {
            id: format!("s{number}"),
            season_number: number,
            title: Some(title.into()),
            overview: Some(format!("{title}, told.")),
            air_date: None,
            tmdb_id,
            tvdb_id: None,
            is_manual: false,
            images: Vec::new(),
            primary_images: Default::default(),
            translations: vec![Translation {
                language: "fra".into(),
                title: Some(format!("{title}, en français")),
                overview: None,
                tagline: None,
                is_manual: false,
            }],
        };
        let bare = |number: i32| Season {
            title: None,
            overview: None,
            translations: Vec::new(),
            ..named(number, "", None)
        };
        let mut tvdb = base("tvdb", "Sagas");
        tvdb.item.seasons = vec![bare(1), bare(2), bare(3)];
        tvdb.item.episodes = vec![
            aired(1, 1, Some("2024-01-07"), None),
            aired(1, 2, Some("2024-01-14"), None),
            aired(2, 1, Some("2024-01-21"), None),
            aired(2, 2, Some("2024-01-28"), None),
            aired(3, 1, Some("2024-07-07"), None),
        ];
        let mut tmdb = base("tmdb", "Sagas");
        tmdb.item.seasons = vec![
            named(1, "Saga of the East", Some(901)),
            named(2, "Saga of the Sea", Some(902)),
            named(3, "Arc of the Summer", Some(903)),
        ];
        tmdb.item.episodes = vec![
            aired(1, 1, Some("2024-01-07"), Some(101)),
            aired(1, 2, Some("2024-01-14"), Some(102)),
            aired(1, 3, Some("2024-01-21"), Some(103)),
            aired(2, 1, Some("2024-01-28"), Some(104)),
            aired(3, 1, Some("2024-07-07"), Some(105)),
        ];

        let merged = combine(vec![tmdb, tvdb], &priority()).unwrap();
        let season = |n: i32| {
            merged
                .seasons
                .iter()
                .find(|s| s.season_number == n)
                .unwrap()
        };
        // TMDB's first went partly into TheTVDB's second, and so into its
        // second came another season's: neither lends a thing.
        for n in [1, 2] {
            assert_eq!(season(n).title, None, "season {n}");
            assert_eq!(season(n).overview, None, "season {n}");
            assert_eq!(season(n).tmdb_id, None, "season {n}");
            assert!(season(n).translations.is_empty(), "season {n}");
        }
        assert_eq!(season(3).title.as_deref(), Some("Arc of the Summer"));
        assert_eq!(season(3).tmdb_id, Some(903));
        assert_eq!(season(3).translations.len(), 1);
        // The episodes, paired by the day, take theirs all the same.
        assert_eq!(
            tmdb_ids(&merged),
            [Some(101), Some(102), Some(103), Some(104), Some(105)]
        );
    }

    #[test]
    fn a_double_bill_numbered_otherwise_pairs_by_its_rank_in_the_day() {
        // Two episodes aired one evening, TheTVDB's 1×01 and 1×02, TMDB's
        // 2×05 and 2×06: each takes the one of its rank that day. Where TMDB
        // lists one episode on a day TheTVDB lists two, nothing says which.
        let mut tvdb = base("tvdb", "Double");
        tvdb.item.episodes = vec![
            aired(1, 1, Some("2024-03-01"), None),
            aired(1, 2, Some("2024-03-01"), None),
            aired(1, 3, Some("2024-03-08"), None),
            aired(1, 4, Some("2024-03-08"), None),
        ];
        let mut tmdb = base("tmdb", "Double");
        tmdb.item.episodes = vec![
            aired(2, 5, Some("2024-03-01"), Some(205)),
            aired(2, 6, Some("2024-03-01"), Some(206)),
            aired(2, 7, Some("2024-03-08"), Some(207)),
        ];

        let merged = combine(vec![tmdb, tvdb], &priority()).unwrap();
        assert_eq!(tmdb_ids(&merged), [Some(205), Some(206), None, None]);
    }

    #[test]
    fn a_day_apart_or_no_day_at_all_lets_the_number_stand() {
        // A broadcast across midnight, or in another time zone, is a day out:
        // the same episode. An episode with no day of its own, or whose
        // number TheTVDB lists with no day, has nothing to be checked by.
        let mut tvdb = base("tvdb", "Night");
        tvdb.item.episodes = vec![
            aired(1, 1, Some("2024-01-07"), None),
            aired(1, 2, Some("2024-01-14"), None),
            aired(1, 3, None, None),
        ];
        let mut tmdb = base("tmdb", "Night");
        tmdb.item.episodes = vec![
            aired(1, 1, Some("2024-01-08"), Some(11)),
            aired(1, 2, None, Some(12)),
            aired(1, 3, Some("2024-01-21"), Some(13)),
        ];

        let merged = combine(vec![tmdb, tvdb], &priority()).unwrap();
        assert_eq!(tmdb_ids(&merged), [Some(11), Some(12), Some(13)]);
    }

    fn special(number: i32, date: Option<&str>, title: &str) -> Episode {
        let mut e = episode(0, number);
        e.air_date = date.map(String::from);
        e.title = title.into();
        e
    }

    /// One as TMDB lists it, with the synopsis, still and id TheTVDB lacks.
    fn tmdb_special(number: i32, date: Option<&str>, title: &str, tmdb_id: i64) -> Episode {
        let mut e = special(number, date, title);
        e.overview = Some(format!("TMDB's synopsis of {title}."));
        e.image = Some(format!("https://tmdb/{tmdb_id}.jpg"));
        e.tmdb_id = Some(tmdb_id);
        e.runtime = Some(45);
        e
    }

    #[test]
    fn a_special_takes_nothing_from_one_numbered_alike_that_came_out_another_day() {
        // Rurouni Kenshin: TheTVDB's first special is the 1997 film; TMDB's is
        // the series' last episode, which TheTVDB counts in season 3. Paired by
        // number, the film took that episode's synopsis, still and TMDB id —
        // and its runtime, rating, a date it lacked, a name it lacked.
        let mut tvdb = base("tvdb", "Rurouni Kenshin");
        tvdb.item.episodes = vec![special(
            1,
            Some("1997-12-20"),
            "Requiem for the Ishin Patriots",
        )];

        let mut tmdb = base("tmdb", "Rurouni Kenshin");
        tmdb.item.episodes = vec![tmdb_special(
            1,
            Some("1998-12-02"),
            "End of Wanderings",
            1506818,
        )];

        let merged = combine(vec![tmdb, tvdb], &priority()).unwrap();

        assert_eq!(merged.episodes.len(), 1, "TMDB's special is not added");
        let film = &merged.episodes[0];
        assert_eq!(film.title, "Requiem for the Ishin Patriots");
        assert_eq!(film.air_date.as_deref(), Some("1997-12-20"));
        assert_eq!(film.overview, None);
        assert_eq!(film.image, None);
        assert_eq!(film.tmdb_id, None);
        assert_eq!(film.runtime, None);
    }

    #[test]
    fn a_special_left_unnamed_is_not_given_another_specials_name() {
        // A language TheTVDB has no name in for a special leaves it blank; the
        // name TMDB gives its own special of that number belongs to another.
        // Unnamed, Sonarr shows TBA and takes the name once there is one.
        let mut tvdb = base("tvdb", "Kenshin le vagabond");
        tvdb.item.episodes = vec![special(1, Some("1997-12-20"), "")];

        let mut tmdb = base("tmdb", "Kenshin le vagabond");
        tmdb.item.episodes = vec![tmdb_special(
            1,
            Some("1998-12-02"),
            "Un autre épisode spécial",
            1506818,
        )];

        let merged = combine(vec![tmdb, tvdb], &priority()).unwrap();
        assert_eq!(merged.episodes[0].title, "");
    }

    #[test]
    fn a_special_is_filled_from_the_one_another_provider_has_on_the_same_day() {
        // A special TMDB lacks puts its specials one behind TheTVDB's from
        // there: its first is TheTVDB's second. The only special either side
        // has on a day is the same one, whatever number each gives it.
        let mut tvdb = base("tvdb", "T");
        tvdb.item.episodes = vec![
            special(1, Some("1997-12-20"), "The film"),
            special(2, Some("1999-02-20"), "Act 1"),
        ];

        let mut tmdb = base("tmdb", "T");
        tmdb.item.episodes = vec![tmdb_special(1, Some("1999-02-20"), "Act 1", 703204)];

        let merged = combine(vec![tmdb, tvdb], &priority()).unwrap();

        let (film, act) = (&merged.episodes[0], &merged.episodes[1]);
        assert_eq!(film.tmdb_id, None, "TMDB's first is not the film");
        assert_eq!(film.overview, None);
        assert_eq!(act.episode_number, 2, "the spine's number stands");
        assert_eq!(act.tmdb_id, Some(703204));
        assert_eq!(act.overview.as_deref(), Some("TMDB's synopsis of Act 1."));
        assert_eq!(act.image.as_deref(), Some("https://tmdb/703204.jpg"));
    }

    #[test]
    fn specials_out_the_same_day_pair_only_when_both_sides_number_them_alike() {
        // Both parts of Reflection came out on one day. Numbered alike on both
        // sides, each part is its namesake.
        let tvdb = || {
            let mut tvdb = base("tvdb", "T");
            tvdb.item.episodes = vec![
                special(6, Some("2001-12-03"), "Part 1"),
                special(7, Some("2001-12-03"), "Part 2"),
            ];
            tvdb
        };

        let mut tmdb = base("tmdb", "T");
        tmdb.item.episodes = vec![
            tmdb_special(6, Some("2001-12-03"), "(1)", 703206),
            tmdb_special(7, Some("2001-12-03"), "(2)", 703200),
        ];
        let merged = combine(vec![tmdb, tvdb()], &priority()).unwrap();
        let ids: Vec<_> = merged.episodes.iter().map(|e| e.tmdb_id).collect();
        assert_eq!(ids, [Some(703206), Some(703200)]);

        // One behind, as after a special TMDB lacks, the same number on the
        // same day is TheTVDB's first part and TMDB's second: the day is left
        // alone rather than each part given its neighbour's text.
        let mut shifted = base("tmdb", "T");
        shifted.item.episodes = vec![
            tmdb_special(5, Some("2001-12-03"), "(1)", 703206),
            tmdb_special(6, Some("2001-12-03"), "(2)", 703200),
        ];
        let merged = combine(vec![shifted, tvdb()], &priority()).unwrap();
        assert!(
            merged
                .episodes
                .iter()
                .all(|e| e.tmdb_id.is_none() && e.overview.is_none())
        );
    }

    #[test]
    fn a_special_without_a_day_to_check_keeps_what_the_spine_gave_it() {
        // With no date on one side or the other, the number is all there is,
        // and in season 0 the same number is too often another special.
        for (ours, theirs) in [
            (None, None),
            (Some("2003-03-03"), None),
            (None, Some("2003-03-03")),
        ] {
            let mut tvdb = base("tvdb", "T");
            tvdb.item.episodes = vec![special(14, ours, "Kengi Taizen")];

            let mut tmdb = base("tmdb", "T");
            tmdb.item.episodes = vec![tmdb_special(14, theirs, "Another special", 99)];

            let merged = combine(vec![tmdb, tvdb], &priority()).unwrap();
            let kept = &merged.episodes[0];
            assert_eq!(kept.title, "Kengi Taizen");
            assert_eq!(kept.tmdb_id, None, "{ours:?} against {theirs:?}");
            assert_eq!(kept.overview, None, "{ours:?} against {theirs:?}");
            assert_eq!(kept.air_date.as_deref(), ours, "no date taken either");
        }
    }

    #[test]
    fn the_regular_seasons_still_pair_by_number_alone() {
        // There the providers number alike, and a day apart is a time zone
        // rather than another episode: a Japanese evening is the American
        // morning before. TMDB's text fills TheTVDB's gaps as it always has.
        let mut tvdb = base("tvdb", "T");
        let mut ours = episode(1, 1);
        ours.air_date = Some("1996-01-10".into());
        tvdb.item.episodes = vec![ours];

        let mut tmdb = base("tmdb", "T");
        let mut theirs = episode(1, 1);
        theirs.air_date = Some("1996-01-09".into());
        theirs.overview = Some("TMDB's synopsis.".into());
        theirs.tmdb_id = Some(1);
        tmdb.item.episodes = vec![theirs];

        let merged = combine(vec![tmdb, tvdb], &priority()).unwrap();
        assert_eq!(
            merged.episodes[0].overview.as_deref(),
            Some("TMDB's synopsis.")
        );
        assert_eq!(merged.episodes[0].tmdb_id, Some(1));
    }

    #[test]
    fn skyhooks_specials_are_thetvdbs_and_pair_by_number() {
        // Skyhook republishes TheTVDB, numbering and all: its specials need no
        // date to be matched with TheTVDB's.
        let mut tvdb = base("tvdb", "T");
        tvdb.item.episodes = vec![special(14, None, "Kengi Taizen")];

        let mut skyhook = base("skyhook", "T");
        let mut same = special(14, None, "Kengi Taizen");
        same.overview = Some("Skyhook's synopsis.".into());
        skyhook.item.episodes = vec![same];

        let merged = combine(vec![tvdb, skyhook], &priority()).unwrap();
        assert_eq!(
            merged.episodes[0].overview.as_deref(),
            Some("Skyhook's synopsis.")
        );
    }

    #[test]
    fn a_list_tmdb_numbers_keeps_its_own_specials() {
        // With nothing from TheTVDB the list is TMDB's, specials included:
        // there is nothing to line them up with, and nothing is dropped.
        let mut tmdb = base("tmdb", "T");
        tmdb.item.episodes = vec![
            tmdb_special(1, None, "Undated", 1),
            tmdb_special(2, Some("2001-01-01"), "Dated", 2),
        ];

        let merged = combine(vec![tmdb, base("fanart", "T")], &priority()).unwrap();
        assert_eq!(merged.episodes.len(), 2);
        assert_eq!(merged.episodes[0].tmdb_id, Some(1));
    }

    fn alternative(title: &str, language: Option<&str>) -> AlternativeTitle {
        AlternativeTitle {
            id: crate::db::new_id(),
            title: title.into(),
            title_type: None,
            language: language.map(String::from),
            is_manual: false,
        }
    }

    #[test]
    fn an_alternative_title_filed_for_no_country_is_not_a_second_copy() {
        let mut tmdb = base("tmdb", "Attack on Titan");
        tmdb.item.alternative_titles = vec![
            alternative("AOT", Some("usa")),
            alternative("Shingeki no Kyojin", Some("jpn")),
        ];

        let mut anilist = base("anilist", "Attack on Titan");
        anilist.item.alternative_titles = vec![
            alternative("aot", None),
            alternative("SnK", None),
            alternative("Shingeki no Kyojin", Some("jpn")),
        ];

        let merged = combine(vec![tmdb, anilist], &priority()).unwrap();
        let titles: Vec<&str> = merged
            .alternative_titles
            .iter()
            .map(|t| t.title.as_str())
            .collect();

        assert_eq!(titles, ["AOT", "Shingeki no Kyojin", "SnK"]);
    }

    #[test]
    fn the_same_title_for_two_countries_is_still_two_entries() {
        let mut tmdb = base("tmdb", "Amélie");
        tmdb.item.alternative_titles = vec![
            alternative("Die fabelhafte Welt der Amélie", Some("deu")),
            alternative("Die fabelhafte Welt der Amélie", Some("aut")),
        ];

        let merged = combine(vec![tmdb], &priority()).unwrap();
        assert_eq!(merged.alternative_titles.len(), 2);
    }

    #[test]
    fn a_works_own_title_is_not_listed_among_its_alternatives() {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Attack on Titan".into();
        let mut manual = alternative("Attack on Titan", None);
        manual.is_manual = true;
        item.alternative_titles = vec![
            alternative("Attack On Titan", Some("usa")),
            alternative(" attack on titan ", Some("deu")),
            alternative("Shingeki no Kyojin", Some("jpn")),
            manual,
        ];

        drop_own_title(&mut item);
        let titles: Vec<(&str, bool)> = item
            .alternative_titles
            .iter()
            .map(|t| (t.title.as_str(), t.is_manual))
            .collect();

        // What a person entered stays, whatever it says.
        assert_eq!(
            titles,
            [("Shingeki no Kyojin", false), ("Attack on Titan", true)]
        );
    }

    #[test]
    fn an_anime_sites_studio_replaces_the_first_production_company() {
        let tmdb = || {
            let mut tmdb = base("tmdb", "Attack on Titan");
            tmdb.item.studio = Some("Production I.G".into());
            tmdb
        };

        let mut anilist = base("anilist", "Attack on Titan");
        anilist.item.studio = Some("WIT STUDIO".into());

        let mut mal = base("mal", "Attack on Titan");
        mal.item.studio = Some("Wit Studio".into());

        // In the order the default priority gives them: AniList before MAL.
        let order: Vec<String> = ["tmdb", "anilist", "mal"].map(String::from).to_vec();
        let merged = combine(vec![tmdb(), mal, anilist], &order).unwrap();
        assert_eq!(merged.studio.as_deref(), Some("WIT STUDIO"));

        // Nothing from them, nothing replaced.
        let merged = combine(vec![tmdb()], &order).unwrap();
        assert_eq!(merged.studio.as_deref(), Some("Production I.G"));
    }

    #[test]
    fn a_work_any_provider_calls_adult_is_adult() {
        let tmdb = base("tmdb", "Somewhere");
        let mut anilist = base("anilist", "Somewhere");
        anilist.item.is_adult = true;

        let merged = combine(vec![tmdb, anilist], &priority()).unwrap();
        assert!(merged.is_adult);
    }

    fn company(
        role: crate::domain::CompanyRole,
        name: &str,
        tmdb: Option<i64>,
        tvdb: Option<i64>,
        source: &str,
        order: i32,
    ) -> CompanyCredit {
        CompanyCredit {
            company: crate::domain::Company {
                id: String::new(),
                kind: role.kind(),
                name: name.into(),
                tmdb_id: tmdb,
                tvdb_id: tvdb,
                logo: tmdb.map(|id| format!("https://image.tmdb.org/t/p/original/{id}.png")),
                country: None,
            },
            role,
            sort_order: order,
            source: Some(source.into()),
        }
    }

    /// A role's companies come from the first source that lists any; the
    /// others give the ids and the logos of the same companies, and a role
    /// only they list.
    #[test]
    fn a_roles_companies_come_from_the_first_source_and_the_others_complete_them() {
        use crate::domain::CompanyRole::{Network, Production, Studio};

        let mut tmdb = base("tmdb", "Breaking Bad");
        tmdb.item.companies = vec![
            company(Network, "AMC", Some(174), None, "tmdb", 0),
            company(
                Production,
                "Sony Pictures Television",
                Some(11073),
                None,
                "tmdb",
                0,
            ),
        ];
        let mut tvdb = base("tvdb", "Breaking Bad");
        tvdb.item.companies = vec![
            company(Network, "amc ", None, Some(389), "tvdb", 0),
            company(Network, "Netflix", None, Some(535), "tvdb", 1),
            company(
                Production,
                "Sony Pictures Television",
                None,
                Some(39667),
                "tvdb",
                0,
            ),
            company(
                Production,
                "High Bridge Productions",
                None,
                Some(49088),
                "tvdb",
                1,
            ),
            company(Studio, "A Studio", None, Some(77), "tvdb", 0),
        ];

        let merged = combine(vec![tvdb, tmdb], &priority()).unwrap();
        // Its role, its name, its ids, whether it has a logo, whose it is.
        type Read<'a> = (
            crate::domain::CompanyRole,
            &'a str,
            Option<i64>,
            Option<i64>,
            bool,
            &'a str,
        );
        let read: Vec<Read<'_>> = merged
            .companies
            .iter()
            .map(|c| {
                (
                    c.role,
                    c.company.name.as_str(),
                    c.company.tmdb_id,
                    c.company.tvdb_id,
                    c.company.logo.is_some(),
                    c.source.as_deref().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            read,
            [
                (Network, "AMC", Some(174), Some(389), true, "tmdb"),
                (
                    Production,
                    "Sony Pictures Television",
                    Some(11073),
                    Some(39667),
                    true,
                    "tmdb"
                ),
                (Studio, "A Studio", None, Some(77), false, "tvdb"),
            ],
            "TMDB's lists, completed; TheTVDB's studios, which TMDB has none of"
        );
    }

    /// Every source's videos once each; TMDB's keywords whole; what is said
    /// of a picture, the first source's.
    #[test]
    fn videos_are_gathered_keywords_taken_whole_and_picture_details_kept_first() {
        let video = |key: &str, order: i32, source: &str| crate::domain::Video {
            id: String::new(),
            site: "YouTube".into(),
            key: key.into(),
            name: None,
            video_type: Some("Trailer".into()),
            language: None,
            country: None,
            official: true,
            published_at: None,
            size: None,
            tmdb_id: None,
            source: Some(source.into()),
            sort_order: order,
        };
        let keyword = |id: i64, name: &str| crate::domain::Keyword {
            id: String::new(),
            tmdb_id: Some(id),
            name: name.into(),
        };
        let details = |votes: f64| crate::domain::ImageDetails {
            vote_average: Some(votes),
            ..Default::default()
        };

        let mut tmdb = base("tmdb", "T");
        tmdb.item.videos = vec![video("a", 0, "tmdb"), video("b", 1, "tmdb")];
        tmdb.item.keyword_entities = vec![keyword(1, "one")];
        tmdb.item
            .image_details
            .insert("https://p".into(), details(5.0));
        let mut other = base("radarr", "T");
        other.item.videos = vec![video("b", 0, "radarr"), video("c", 0, "radarr")];
        other.item.keyword_entities = vec![keyword(2, "two")];
        other
            .item
            .image_details
            .insert("https://p".into(), details(9.0));
        other
            .item
            .image_details
            .insert("https://q".into(), details(7.0));

        let merged = combine(vec![other, tmdb], &priority()).unwrap();
        let keys: Vec<(&str, i32)> = merged
            .videos
            .iter()
            .map(|v| (v.key.as_str(), v.sort_order))
            .collect();
        assert_eq!(keys, [("a", 0), ("b", 1), ("c", 2)]);
        assert_eq!(merged.keyword_entities, [keyword(1, "one")]);
        assert_eq!(merged.image_details["https://p"].vote_average, Some(5.0));
        assert_eq!(merged.image_details["https://q"].vote_average, Some(7.0));
    }

    /// A season's translations come from whoever has them, whoever numbers
    /// the seasons: one a language — the numbering source's, as its own
    /// fields are, then the others' in the languages it lacks.
    #[test]
    fn a_seasons_translations_are_gathered_one_a_language() {
        let translated = |language: &str, title: &str| Translation {
            language: language.into(),
            title: Some(title.into()),
            overview: None,
            tagline: None,
            is_manual: false,
        };
        let mut tvdb = base("tvdb", "T");
        tvdb.item.episodes = vec![episode(1, 1)];
        tvdb.item.seasons = vec![crate::db::repo::child::blank_season(1)];
        tvdb.item.seasons[0].translations = vec![translated("deu", "Staffel eins")];
        let mut tmdb = base("tmdb", "T");
        tmdb.item.seasons = vec![crate::db::repo::child::blank_season(1)];
        tmdb.item.seasons[0].translations = vec![
            translated("fra", "Saison un"),
            translated("deu", "Erste Staffel"),
        ];

        let merged = combine(vec![tvdb, tmdb], &priority()).unwrap();
        let mut titles: Vec<(&str, &str)> = merged.seasons[0]
            .translations
            .iter()
            .map(|t| (t.language.as_str(), t.title.as_deref().unwrap_or_default()))
            .collect();
        titles.sort_unstable();
        assert_eq!(titles, [("deu", "Staffel eins"), ("fra", "Saison un")]);
    }
}
