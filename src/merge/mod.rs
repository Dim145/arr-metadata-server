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
//! * **Episodes and seasons** take their *list* from one provider and their
//!   *fields* from all of them. Merging two lists by season and episode number
//!   sounds right and is not: providers disagree about where a season ends, so
//!   unioning One Piece's 1179 TVDB episodes with TMDB's 1181 produced 2352,
//!   most of them phantoms Sonarr would then hunt for files of. One list, many
//!   opinions about each entry.

pub mod provenance;
pub mod rules;

use crate::domain::{
    AlternativeTitle, CoverType, Episode, ExternalIds, Image, MediaItem, Rating, Season,
    Translation,
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

    let mut merged = contributions.remove(0).item;

    // The most trusted provider loses the numbering to the spine, but not what
    // it knows about each episode: TMDB carries overviews and stills that TVDB
    // rarely has. Its own lists fold back in as a field source like any other.
    let displaced_seasons = std::mem::replace(&mut merged.seasons, seasons);
    let displaced_episodes = std::mem::replace(&mut merged.episodes, episodes);
    merge_seasons(&mut merged.seasons, displaced_seasons);
    merge_episodes(&mut merged.episodes, displaced_episodes);

    for contribution in contributions {
        fold(&mut merged, contribution.item);
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

    Some(merged)
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
fn fold(into: &mut MediaItem, other: MediaItem) {
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
    if into.keywords.is_empty() {
        into.keywords = other.keywords;
    }

    // Believed from whichever provider says so. A work only one source knows
    // is adult — AniList, for most of what it flags — must not be served as
    // anything else because a broader source forgot to say.
    into.is_adult = into.is_adult || other.is_adult;

    merge_ids(&mut into.external_ids, other.external_ids);

    union_images(&mut into.images, other.images);
    union_alternative_titles(&mut into.alternative_titles, other.alternative_titles);
    union_ratings(&mut into.ratings, other.ratings);
    union_translations(&mut into.translations, other.translations);

    // One provider's cast, not a blend of several: nothing identifies a person
    // across providers, so a union would list the same actor twice.
    if into.credits.is_empty() {
        into.credits = other.credits;
    }

    // Likewise what a work is filed beside: one provider's relations, whole.
    if into.relations.is_empty() {
        into.relations = other.relations;
    }

    // The spine: whichever provider came first and had a list keeps it. Only
    // when it had none does a later provider supply one.
    if into.seasons.is_empty() {
        into.seasons = other.seasons;
    } else {
        merge_seasons(&mut into.seasons, other.seasons);
    }

    if into.episodes.is_empty() {
        into.episodes = other.episodes;
    } else {
        merge_episodes(&mut into.episodes, other.episodes);
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
    let offset = into
        .iter()
        .map(|i| i.sort_order)
        .max()
        .map(|m| m + 1)
        .unwrap_or(0);

    for mut image in other {
        let duplicate = into
            .iter()
            .any(|existing| existing.url == image.url && existing.cover_type == image.cover_type);

        if duplicate {
            continue;
        }

        image.sort_order += offset;
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

fn union_translations(into: &mut Vec<Translation>, other: Vec<Translation>) {
    for translation in other {
        if !into.iter().any(|e| e.language == translation.language) {
            into.push(translation);
        }
    }
}

/// Fill seasons the spine already has. A season only another provider knows
/// about is dropped, for the same reason as episodes.
fn merge_seasons(into: &mut [Season], other: Vec<Season>) {
    for season in other {
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
    }
}

/// Merge episodes field by field, keyed by their numbering.
///
/// This is where a second provider earns its place: TVDB carries absolute
/// numbering and air-order hints that TMDB has no field for, while TMDB carries
/// overviews and stills that TVDB often lacks.
fn merge_episodes(into: &mut [Episode], other: Vec<Episode>) {
    for episode in other {
        let existing = into.iter_mut().find(|e| {
            e.season_number == episode.season_number && e.episode_number == episode.episode_number
        });

        if let Some(existing) = existing {
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

/// Artwork a provider contributed that nothing else did, for logging.
#[allow(dead_code)]
pub fn image_summary(images: &[Image]) -> String {
    use std::collections::BTreeMap;

    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for image in images {
        *counts.entry(image.cover_type.as_str()).or_default() += 1;
    }

    counts
        .into_iter()
        .map(|(kind, n)| format!("{n} {kind}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A cover type's place in the display order, for callers building artwork.
#[allow(dead_code)]
pub fn cover_priority(cover_type: CoverType) -> u8 {
    cover_type.priority()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CreditType, MediaKind, RatingValue};

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
}
