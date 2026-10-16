//! Sonarr's Skyhook format, as Sonarr v5 reads it.
//!
//! Sonarr v5 asks `skyhook.sonarr.tv/v2/tvdb/…` and reads the answer with the
//! resources under `NzbDrone.Core/MetadataSource/SkyHook/Resource/` of its
//! `v5-develop` branch: what v4 reads ([`super::sonarr`]), and besides it the
//! languages a series is held in, the orders its episodes come in, a
//! season's title and year. The names follow the real service's answers,
//! which call an episode's ids `metadataShowId` and `metadataId` where the
//! client still reads `tvdbShowId` and `tvdbId`: both pairs are written,
//! since a reader passes over a name it does not know.
//!
//! Written only. What this server reads from the real Skyhook is still its
//! first edition: see `providers::skyhook`.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;
use utoipa::ToSchema;

use super::sonarr::{
    self, ActorResource, AlternativeTitleResource, ImageResource, RatingResource, TimeOfDay,
};
use crate::{
    domain::{Episode, EpisodeOrder, MediaItem, Season, lead_images},
    service::{
        ids,
        language,
        // Where an order places the episodes is the same for every client
        // that numbers them by one; see `service::orders`.
        orders::{named as order_named, placements},
    },
};

/// What Skyhook calls the aired order: the one every series has, and the
/// one answered for a type the series has no order of.
pub const OFFICIAL: &str = crate::service::orders::AIRED;

/// A series as Sonarr v5 reads one.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShowResourceV2 {
    pub tvdb_id: i64,
    pub title: String,
    /// The title in the series' own language. Sonarr derives it from
    /// `translations` by `originalLanguage` rather than reading it; written
    /// all the same, as the real service writes it.
    pub original_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    pub slug: String,
    /// `unknown`, or `anime` for a series the anime sites list. Sonarr v5
    /// reads nothing here; the real service's search answers also name
    /// `scripted`, `miniseries`, `anthology`.
    pub formats: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_country: Option<String>,
    /// Three letters, as `translations` names a language: Sonarr compares
    /// the two as text to find the original title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
    /// The language answered in, as asked.
    pub language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_aired: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_aired: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tv_rage_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tv_maze_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    pub mal_ids: Vec<i64>,
    /// Always empty: the catalogue keeps no AniDB id. Written as the real
    /// service writes it, a list, for a reader that counts on one.
    pub anidb_ids: Vec<i64>,
    pub ani_list_ids: Vec<i64>,
    pub last_updated: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_of_day: Option<TimeOfDay>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_network: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<RatingResource>,
    pub alternative_titles: Vec<AlternativeTitleResource>,
    /// Every language the catalogue holds the series' title in — the one it
    /// is stored in and its own among them — by language code.
    pub translations: Vec<TranslationResource>,
    /// The aired order first, then each other order the catalogue keeps for
    /// the series.
    pub season_types: Vec<SeasonTypeResource>,
    pub actors: Vec<ActorResource>,
    pub images: Vec<ImageResource>,
    pub seasons: Vec<SeasonResourceV2>,
    /// Left out of a search result, as the real service leaves it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episodes: Option<Vec<EpisodeResourceV2>>,
    /// `tvdb`; `tmdb` or `fankai` for an id this server made for a work
    /// TheTVDB does not list (see [`ids`]).
    pub metadata_source: String,
}

/// The series' title and overview in one language. Named apart in the
/// spec: Radarr's surface has a `TranslationResource` of its own, in
/// PascalCase, and utoipa files schemas by their short name.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SkyhookTranslationResource)]
pub struct TranslationResource {
    /// Three letters, ISO 639-2/T, as Sonarr's own table spells them.
    pub language: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
}

/// One order the series' episodes come in, as Sonarr offers it.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SeasonTypeResource {
    /// `Aired Order`, `DVD Order`…: what Sonarr shows.
    pub name: String,
    /// `official`, `dvd`, `absolute`, `alternate`, `regional`, `altdvd`:
    /// what Sonarr asks the series in.
    #[serde(rename = "type")]
    pub kind: String,
    pub season_numbers: Vec<i32>,
    /// The episodes of the regular seasons; the specials are not counted,
    /// as the real service does not count them.
    pub episode_count: i32,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SeasonResourceV2 {
    pub season_number: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The year the season first aired.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    pub images: Vec<ImageResource>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeResourceV2 {
    pub tvdb_show_id: i64,
    pub metadata_show_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_id: Option<i64>,
    pub season_number: i32,
    pub episode_number: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub absolute_episode_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aired_after_season_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aired_before_season_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aired_before_episode_number: Option<i32>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_date_utc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finale_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<RatingResource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

/// The work's own title and overview as the catalogue holds them, in the
/// language of the answers, taken before another language is laid over
/// them: one more language the series is held in, listed with the rest.
#[derive(Clone, Debug)]
pub struct OwnText {
    /// Three letters: the language the work is stored in.
    pub language: String,
    pub title: String,
    pub overview: Option<String>,
}

impl OwnText {
    pub fn of(item: &MediaItem, language: &str) -> Self {
        Self {
            language: language.to_string(),
            title: item.title.clone(),
            overview: item.overview.clone(),
        }
    }
}

/// What an answer is built for.
pub struct Asked<'a> {
    /// The id the client asked by, echoed back: Sonarr has stored it, and a
    /// different one in the answer would orphan the series.
    pub client_id: i64,
    /// Three letters: the language the work's text was laid over in.
    pub language: &'a str,
    /// [`OFFICIAL`], or one of the orders the work has: what its episodes
    /// are numbered by.
    pub season_type: &'a str,
    /// The work as it was fetched (see [`OwnText`]).
    pub own: &'a OwnText,
    /// The other orders the catalogue keeps for the work.
    pub orders: &'a [EpisodeOrder],
    /// With the episodes, for a series asked for by id; without, for a
    /// search result.
    pub with_episodes: bool,
}

/// Render a work as Sonarr v5 expects it.
pub fn from_item(item: &MediaItem, asked: &Asked<'_>) -> ShowResourceV2 {
    let base = sonarr::from_item(item, asked.client_id, asked.language);

    // Sonarr finds the original title by comparing this to each
    // translation's language as text: both spelt the one way, or it finds
    // nothing.
    let original_language = item
        .original_language
        .as_deref()
        .map(language::normalize)
        .filter(|l| !l.is_empty());
    let translations = translations(item, asked.own, original_language.as_deref());
    // A title locked in every language is the work's name in each of them,
    // its own among them, and not the name it was made under: that is the
    // original title it carries, as a source gave it or as it was locked.
    let renamed = crate::domain::fields::text_lock(item, "item", "title", "").is_some()
        || item.locked_fields.iter().any(|f| f == "item/originalTitle");
    let original_title = original_language
        .as_deref()
        .filter(|_| !renamed)
        .and_then(|code| translations.iter().find(|t| t.language == code))
        .map(|t| t.title.clone())
        .or_else(|| item.original_title.clone().filter(|t| !t.trim().is_empty()))
        .unwrap_or_else(|| base.title.clone());

    // An order the work has in name only — its places all name episodes the
    // work no longer holds, as after a refresh without TheTVDB — is no order
    // to answer in: a Sonarr set to it would be sent no episode at all, and
    // would drop its own. The aired order stands in, as for a type unknown.
    let (seasons, episodes) = match order_named(asked.orders, asked.season_type)
        .filter(|order| !placements(item, order).is_empty())
    {
        Some(order) => placed(item, order, asked.client_id),
        None => aired(item, asked.client_id),
    };

    ShowResourceV2 {
        tvdb_id: base.tvdb_id,
        title: base.title,
        original_title,
        overview: base.overview,
        slug: base.slug,
        formats: formats(item),
        original_country: base.original_country,
        original_language,
        language: asked.language.to_string(),
        first_aired: base.first_aired,
        last_aired: base.last_aired,
        tv_rage_id: base.tv_rage_id,
        tv_maze_id: base.tv_maze_id,
        tmdb_id: base.tmdb_id,
        imdb_id: base.imdb_id,
        mal_ids: base.mal_ids,
        anidb_ids: Vec::new(),
        ani_list_ids: base.ani_list_ids,
        last_updated: base.last_updated,
        status: base.status,
        runtime: base.runtime,
        time_of_day: base.time_of_day,
        original_network: base.original_network,
        network: base.network,
        genres: base.genres,
        content_rating: base.content_rating,
        rating: base.rating,
        alternative_titles: base.alternative_titles,
        translations,
        season_types: season_types(item, asked.orders),
        actors: base.actors,
        images: base.images,
        seasons,
        episodes: asked.with_episodes.then_some(episodes),
        metadata_source: metadata_source(asked.client_id).to_string(),
    }
}

/// Every language the work is named in, by code.
///
/// The translations held, each under its three-letter code and only when
/// it names the work: a title in no language is nothing to Sonarr, which
/// drops the entry. The work's own text counts as one, under the language
/// it is stored in, when no translation is held in that language. And the
/// work's original language has an entry whenever a title for it is known
/// — the provider's original title, where no translation is held — because
/// that entry is where Sonarr reads the original title from.
fn translations(
    item: &MediaItem,
    own: &OwnText,
    original_language: Option<&str>,
) -> Vec<TranslationResource> {
    let mut held: BTreeMap<String, TranslationResource> = BTreeMap::new();

    for translation in &item.translations {
        let code = language::normalize(&translation.language);
        let Some(title) = non_empty(translation.title.as_deref()) else {
            continue;
        };
        if code.is_empty() {
            continue;
        }
        held.entry(code.clone()).or_insert(TranslationResource {
            language: code,
            title,
            overview: non_empty(translation.overview.as_deref()),
        });
    }

    if let Some(title) = non_empty(Some(&own.title))
        && !own.language.is_empty()
    {
        held.entry(own.language.clone())
            .or_insert(TranslationResource {
                language: own.language.clone(),
                title,
                overview: non_empty(own.overview.as_deref()),
            });
    }

    if let Some(original) = original_language
        && !held.contains_key(original)
        && let Some(title) = non_empty(item.original_title.as_deref())
    {
        held.insert(
            original.to_string(),
            TranslationResource {
                language: original.to_string(),
                title,
                overview: None,
            },
        );
    }

    held.into_values().collect()
}

/// The orders Sonarr may ask the series in: the aired one, then each other
/// order the catalogue keeps that places an episode the work still has.
fn season_types(item: &MediaItem, orders: &[EpisodeOrder]) -> Vec<SeasonTypeResource> {
    let mut types = vec![SeasonTypeResource {
        name: order_name(OFFICIAL),
        kind: OFFICIAL.to_string(),
        season_numbers: sorted_distinct(
            item.seasons
                .iter()
                .map(|s| s.season_number)
                .chain(item.episodes.iter().map(|e| e.season_number)),
        ),
        episode_count: regular(item.episodes.iter().map(|e| e.season_number)),
    }];

    for order in orders {
        let placements = placements(item, order);
        if placements.is_empty() {
            continue;
        }
        types.push(SeasonTypeResource {
            name: order_name(&order.kind),
            kind: order.kind.clone(),
            season_numbers: sorted_distinct(
                placements.iter().map(|(place, _)| place.season_number),
            ),
            episode_count: regular(placements.iter().map(|(place, _)| place.season_number)),
        });
    }

    types
}

/// What Sonarr shows an order as, in Skyhook's words.
fn order_name(kind: &str) -> String {
    match kind {
        OFFICIAL => "Aired Order".to_string(),
        "dvd" => "DVD Order".to_string(),
        "absolute" => "Absolute Order".to_string(),
        "alternate" => "Alternate Order".to_string(),
        "regional" => "Regional Order".to_string(),
        "altdvd" => "Alternate DVD Order".to_string(),
        other => format!("{} Order", sonarr::capitalize(other)),
    }
}

/// The seasons and episodes as they aired: the work's own. Every season
/// number once, whatever the seasons were merged from — Sonarr threw on a
/// number it met twice — and a season the episodes name that no season row
/// does is listed too, bare.
fn aired(item: &MediaItem, client_id: i64) -> (Vec<SeasonResourceV2>, Vec<EpisodeResourceV2>) {
    let first_years = first_years(
        item.episodes
            .iter()
            .map(|e| (e.season_number, e.air_date.as_deref())),
    );

    let mut seasons: BTreeMap<i32, SeasonResourceV2> = BTreeMap::new();
    for season in &item.seasons {
        seasons
            .entry(season.season_number)
            .or_insert_with(|| SeasonResourceV2 {
                season_number: season.season_number,
                title: own_season_title(season.title.as_deref(), season.season_number),
                year: year_of(season.air_date.as_deref())
                    .or_else(|| first_years.get(&season.season_number).copied()),
                images: season_images(season),
            });
    }
    for episode in &item.episodes {
        seasons
            .entry(episode.season_number)
            .or_insert_with(|| SeasonResourceV2 {
                season_number: episode.season_number,
                title: None,
                year: first_years.get(&episode.season_number).copied(),
                images: Vec::new(),
            });
    }

    let episodes = item
        .episodes
        .iter()
        .map(|e| episode_resource(e, client_id))
        .collect();

    (seasons.into_values().collect(), episodes)
}

/// The seasons and episodes as one of the work's other orders places them:
/// the same episodes, by TVDB id, under that order's seasons and numbers.
///
/// The order's seasons have names and pictures of their own on TheTVDB,
/// which the catalogue does not keep: the aired season of the same number
/// lends its pictures, and no name. The year is that of the first episode
/// placed in the season.
fn placed(
    item: &MediaItem,
    order: &EpisodeOrder,
    client_id: i64,
) -> (Vec<SeasonResourceV2>, Vec<EpisodeResourceV2>) {
    let placements = placements(item, order);
    let first_years = first_years(
        placements
            .iter()
            .map(|(place, episode)| (place.season_number, episode.air_date.as_deref())),
    );
    let mut aired: HashMap<i32, &Season> = HashMap::new();
    for season in &item.seasons {
        aired.entry(season.season_number).or_insert(season);
    }

    let mut seasons: BTreeMap<i32, SeasonResourceV2> = BTreeMap::new();
    let mut episodes = Vec::with_capacity(placements.len());
    for (place, episode) in placements {
        seasons
            .entry(place.season_number)
            .or_insert_with(|| SeasonResourceV2 {
                season_number: place.season_number,
                title: None,
                year: first_years.get(&place.season_number).copied(),
                images: aired
                    .get(&place.season_number)
                    .map(|season| season_images(season))
                    .unwrap_or_default(),
            });

        let mut resource = episode_resource(episode, client_id);
        resource.season_number = place.season_number;
        resource.episode_number = place.episode_number;
        resource.absolute_episode_number = place.absolute_number;
        episodes.push(resource);
    }

    (seasons.into_values().collect(), episodes)
}

fn episode_resource(episode: &Episode, client_id: i64) -> EpisodeResourceV2 {
    let e = sonarr::episode_resource(episode, client_id);
    EpisodeResourceV2 {
        tvdb_show_id: e.tvdb_show_id,
        metadata_show_id: e.tvdb_show_id,
        tvdb_id: e.tvdb_id,
        metadata_id: e.tvdb_id,
        season_number: e.season_number,
        episode_number: e.episode_number,
        absolute_episode_number: e.absolute_episode_number,
        aired_after_season_number: e.aired_after_season_number,
        aired_before_season_number: e.aired_before_season_number,
        aired_before_episode_number: e.aired_before_episode_number,
        title: e.title,
        air_date: e.air_date,
        air_date_utc: e.air_date_utc,
        runtime: e.runtime,
        finale_type: e.finale_type,
        rating: e.rating,
        overview: e.overview,
        image: e.image,
    }
}

/// The poster a person chose for the season, as for the work.
fn season_images(season: &Season) -> Vec<ImageResource> {
    lead_images(&season.images, &season.primary_images)
        .into_iter()
        .map(sonarr::image_resource)
        .collect()
}

/// The year each season first aired, from the dates of its episodes.
fn first_years<'a>(episodes: impl Iterator<Item = (i32, Option<&'a str>)>) -> BTreeMap<i32, i32> {
    let mut years = BTreeMap::new();
    for (season, date) in episodes {
        if let Some(year) = year_of(date) {
            years
                .entry(season)
                .and_modify(|first: &mut i32| *first = (*first).min(year))
                .or_insert(year);
        }
    }
    years
}

/// The year a date names, when it is a date.
fn year_of(date: Option<&str>) -> Option<i32> {
    sonarr::sonarr_day(date)?.get(0..4)?.parse().ok()
}

/// What the real service files the series' kind under. Sonarr v5 reads
/// none of it; what is said is what the catalogue can stand behind.
fn formats(item: &MediaItem) -> Vec<String> {
    let anime = !item.external_ids.mal.is_empty() || !item.external_ids.anilist.is_empty();
    vec![if anime { "anime" } else { "unknown" }.to_string()]
}

/// Where the id Sonarr holds the series under comes from: TheTVDB, or this
/// server's own ranges for a work TheTVDB does not list.
fn metadata_source(client_id: i64) -> &'static str {
    if ids::from_fankai(client_id).is_some() {
        "fankai"
    } else if ids::from_synthetic(client_id).is_some() {
        "tmdb"
    } else {
        "tvdb"
    }
}

/// How many of these seasons' episodes are regular: the specials, season 0,
/// are not counted.
fn regular(seasons: impl Iterator<Item = i32>) -> i32 {
    i32::try_from(seasons.filter(|n| *n > 0).count()).unwrap_or(i32::MAX)
}

fn sorted_distinct(numbers: impl Iterator<Item = i32>) -> Vec<i32> {
    let mut numbers: Vec<i32> = numbers.collect();
    numbers.sort_unstable();
    numbers.dedup();
    numbers
}

fn non_empty(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// A season's own name, or nothing when the name only says its number.
/// TMDB names every season — "Season 3", "Saison 3", "Staffel 3" — and
/// the specials "Specials" or "Épisodes spéciaux"; Sonarr v5 writes a
/// season's title into its folder's name beside the number, which would
/// then be said twice. The real service sends nothing for those, and so
/// does this one: one word and the season's number, in whatever language,
/// or the few words the specials go by, name nothing.
pub(crate) fn own_season_title(title: Option<&str>, number: i32) -> Option<String> {
    let title = non_empty(title)?;
    let said = title.trim_end_matches(|c: char| c.is_ascii_digit());
    let digits = &title[said.len()..];
    let stem = said.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '.' | ':'));
    let one_word = !stem.is_empty() && stem.chars().all(|c| c.is_alphabetic() || c == '\'');
    let numbered = digits.parse::<i32>().is_ok_and(|n| n == number);
    if numbered && (stem.is_empty() || one_word) {
        return None;
    }
    if number == 0 {
        const SPECIALS: [&str; 10] = [
            "specials",
            "special",
            "hors-série",
            "hors série",
            "épisodes spéciaux",
            "episodes speciaux",
            "episodios especiales",
            "specialfolgen",
            "speciali",
            "extras",
        ];
        if SPECIALS.contains(&title.to_lowercase().as_str()) {
            return None;
        }
    }
    Some(title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::repo::child::{blank_episode, blank_season},
        domain::{CoverType, ExternalIds, Image, MediaKind, PlacedEpisode, Translation},
    };

    fn translation(language: &str, title: &str, overview: Option<&str>) -> Translation {
        Translation {
            language: language.into(),
            title: Some(title.into()),
            overview: overview.map(str::to_string),
            is_manual: false,
        }
    }

    fn season(number: i32, title: Option<&str>, air_date: Option<&str>) -> Season {
        let mut season = blank_season(number);
        season.title = title.map(str::to_string);
        season.air_date = air_date.map(str::to_string);
        season
    }

    fn episode(season: i32, number: i32, tvdb_id: i64, air_date: &str) -> Episode {
        let mut episode = blank_episode(season, number);
        episode.title = format!("Episode {tvdb_id}");
        episode.tvdb_id = Some(tvdb_id);
        episode.air_date = Some(air_date.into());
        episode
    }

    fn placed(tvdb_id: i64, season: i32, number: i32, absolute: Option<i32>) -> PlacedEpisode {
        PlacedEpisode {
            tvdb_id,
            season_number: season,
            episode_number: number,
            absolute_number: absolute,
        }
    }

    /// *Rurouni Kenshin* (2023), as a catalogue stored in English holds it:
    /// two arcs, a special, three languages — one of them not a tag.
    fn series() -> MediaItem {
        let mut it = MediaItem::empty(MediaKind::Series);
        it.title = "Rurouni Kenshin".into();
        it.overview = Some("A wanderer with a reverse-blade sword.".into());
        it.original_title = Some("るろうに剣心 －明治剣客浪漫譚－".into());
        it.original_language = Some("ja".into());
        it.slug = "rurouni-kenshin-2023".into();
        it.external_ids = ExternalIds {
            tvdb: Some(416802),
            tmdb: Some(209867),
            mal: vec![49778],
            ..Default::default()
        };
        it.translations = vec![
            translation(
                "fra",
                "Kenshin le vagabond",
                Some("Un vagabond au sabre inversé."),
            ),
            translation("eng", "Rurouni Kenshin", None),
            translation("zhtw", "神劍闖江湖", None),
        ];
        let mut first = season(1, Some("東京編"), Some("2023-07-07"));
        first.images = vec![Image {
            id: "s1".into(),
            season_number: Some(1),
            cover_type: CoverType::Poster,
            url: "https://x/s1.jpg".into(),
            language: None,
            sort_order: 0,
            source: None,
            is_manual: false,
        }];
        it.seasons = vec![
            season(0, None, None),
            first,
            season(2, Some("京都編"), None),
        ];
        it.episodes = vec![
            episode(0, 1, 4, "2023-09-01"),
            episode(1, 1, 1, "2023-07-07"),
            episode(1, 2, 2, "2023-07-14"),
            episode(2, 1, 3, "2024-10-04"),
        ];
        it
    }

    fn orders() -> Vec<EpisodeOrder> {
        vec![
            EpisodeOrder {
                kind: "dvd".into(),
                episodes: vec![
                    placed(4, 0, 1, None),
                    placed(2, 1, 1, None),
                    placed(1, 1, 2, None),
                    placed(3, 2, 1, None),
                    // An episode the work no longer has, and a place given
                    // twice.
                    placed(99, 2, 2, None),
                    placed(1, 1, 2, None),
                ],
            },
            EpisodeOrder {
                kind: "absolute".into(),
                episodes: vec![
                    placed(1, 1, 1, Some(1)),
                    placed(2, 1, 2, Some(2)),
                    placed(3, 1, 3, Some(3)),
                ],
            },
        ]
    }

    fn own(item: &MediaItem) -> OwnText {
        OwnText::of(item, "eng")
    }

    fn asked<'a>(own: &'a OwnText, orders: &'a [EpisodeOrder], season_type: &'a str) -> Asked<'a> {
        Asked {
            client_id: 416802,
            language: "fra",
            season_type,
            own,
            orders,
            with_episodes: true,
        }
    }

    #[test]
    fn every_language_the_series_is_named_in_is_listed_its_own_among_them() {
        let it = series();
        let own = own(&it);
        let show = from_item(&it, &asked(&own, &[], OFFICIAL));

        let languages: Vec<&str> = show
            .translations
            .iter()
            .map(|t| t.language.as_str())
            .collect();
        // The French and English held, and Japanese from the original title;
        // what is no language tag names nothing Sonarr can read.
        assert_eq!(languages, ["eng", "fra", "jpn"]);
        assert_eq!(show.translations[1].title, "Kenshin le vagabond");
        assert_eq!(
            show.translations[1].overview.as_deref(),
            Some("Un vagabond au sabre inversé.")
        );
        assert_eq!(
            show.translations[2].title,
            "るろうに剣心 －明治剣客浪漫譚－"
        );

        // The original language is spelt as the entry is, since Sonarr finds
        // the original title by comparing the two as text.
        assert_eq!(show.original_language.as_deref(), Some("jpn"));
        assert_eq!(show.original_title, "るろうに剣心 －明治剣客浪漫譚－");
    }

    #[test]
    fn the_works_own_text_is_a_translation_when_none_is_held_in_its_language() {
        let mut it = series();
        it.translations.retain(|t| t.language != "eng");
        // Stored in German, say, with nothing held in German besides.
        let own = OwnText::of(&it, "deu");
        let show = from_item(&it, &asked(&own, &[], OFFICIAL));

        let german = show
            .translations
            .iter()
            .find(|t| t.language == "deu")
            .expect("the stored text is listed");
        assert_eq!(german.title, "Rurouni Kenshin");
        assert_eq!(
            german.overview.as_deref(),
            Some("A wanderer with a reverse-blade sword.")
        );
        assert!(show.translations.iter().all(|t| t.language != "eng"));

        // A title in no language is no translation: Sonarr drops the entry,
        // so it is not sent.
        it.translations.push(Translation {
            language: "ita".into(),
            title: Some("  ".into()),
            overview: Some("Un vagabondo.".into()),
            is_manual: false,
        });
        let show = from_item(&it, &asked(&own, &[], OFFICIAL));
        assert!(show.translations.iter().all(|t| t.language != "ita"));
    }

    #[test]
    fn with_no_original_language_the_original_title_still_stands() {
        let mut it = series();
        it.original_language = None;
        let own = own(&it);
        let show = from_item(&it, &asked(&own, &[], OFFICIAL));
        assert_eq!(show.original_language, None);
        assert_eq!(show.original_title, "るろうに剣心 －明治剣客浪漫譚－");
        assert!(show.translations.iter().all(|t| t.language != "jpn"));

        it.original_title = None;
        let show = from_item(&it, &asked(&own, &[], OFFICIAL));
        assert_eq!(show.original_title, "Rurouni Kenshin");
    }

    #[test]
    fn the_orders_the_series_has_are_offered_after_the_aired_one() {
        let it = series();
        let own = own(&it);
        let orders = orders();
        let show = from_item(&it, &asked(&own, &orders, OFFICIAL));

        let offered: Vec<(&str, &str, &[i32], i32)> = show
            .season_types
            .iter()
            .map(|t| {
                (
                    t.name.as_str(),
                    t.kind.as_str(),
                    t.season_numbers.as_slice(),
                    t.episode_count,
                )
            })
            .collect();
        // The specials are not counted; nor the episode the work no longer
        // has, nor a place given twice.
        assert_eq!(
            offered,
            [
                ("Aired Order", "official", &[0, 1, 2][..], 3),
                ("DVD Order", "dvd", &[0, 1, 2][..], 3),
                ("Absolute Order", "absolute", &[1][..], 3),
            ]
        );

        // An order that places nothing the work has is not offered.
        let orphan = vec![EpisodeOrder {
            kind: "regional".into(),
            episodes: vec![placed(99, 1, 1, None)],
        }];
        let show = from_item(&it, &asked(&own, &orphan, OFFICIAL));
        assert_eq!(show.season_types.len(), 1);
    }

    #[test]
    fn asked_in_an_order_it_has_the_series_is_numbered_by_it() {
        let it = series();
        let own = own(&it);
        let orders = orders();

        let show = from_item(&it, &asked(&own, &orders, "dvd"));
        let numbered: Vec<(i32, i32, Option<i64>)> = show
            .episodes
            .as_deref()
            .unwrap()
            .iter()
            .map(|e| (e.season_number, e.episode_number, e.tvdb_id))
            .collect();
        assert_eq!(
            numbered,
            [
                (0, 1, Some(4)),
                (1, 1, Some(2)),
                (1, 2, Some(1)),
                (2, 1, Some(3))
            ]
        );
        // The order's seasons: no name of their own, the year of their first
        // episode, the aired season's pictures.
        let seasons: Vec<(i32, Option<&str>, Option<i32>, usize)> = show
            .seasons
            .iter()
            .map(|s| (s.season_number, s.title.as_deref(), s.year, s.images.len()))
            .collect();
        assert_eq!(
            seasons,
            [
                (0, None, Some(2023), 0),
                (1, None, Some(2023), 1),
                (2, None, Some(2024), 0)
            ]
        );

        // Straight through: one season, the absolute numbers the order gives.
        let show = from_item(&it, &asked(&own, &orders, "Absolute"));
        let numbered: Vec<(i32, i32, Option<i32>)> = show
            .episodes
            .as_deref()
            .unwrap()
            .iter()
            .map(|e| (e.season_number, e.episode_number, e.absolute_episode_number))
            .collect();
        assert_eq!(
            numbered,
            [(1, 1, Some(1)), (1, 2, Some(2)), (1, 3, Some(3))]
        );
        assert_eq!(show.seasons.len(), 1);
    }

    #[test]
    fn an_order_the_series_lacks_is_answered_with_the_aired_one() {
        let it = series();
        let own = own(&it);
        let orders = orders();

        for season_type in [OFFICIAL, "regional", "nonsense", ""] {
            let show = from_item(&it, &asked(&own, &orders, season_type));
            let numbered: Vec<(i32, i32)> = show
                .episodes
                .as_deref()
                .unwrap()
                .iter()
                .map(|e| (e.season_number, e.episode_number))
                .collect();
            assert_eq!(
                numbered,
                [(0, 1), (1, 1), (1, 2), (2, 1)],
                "{season_type:?}"
            );
            assert_eq!(show.seasons[1].title.as_deref(), Some("東京編"));
        }
    }

    #[test]
    fn an_order_whose_places_name_no_held_episode_is_answered_with_the_aired_one() {
        // The DVD order stayed from before a refresh that left the episodes
        // without the TheTVDB ids it places by. A Sonarr set to it keeps
        // asking in it: sent nothing, it would drop every episode it has.
        let it = series();
        let own = own(&it);
        let orders = vec![EpisodeOrder {
            kind: "dvd".into(),
            episodes: vec![placed(98, 1, 1, None), placed(99, 1, 2, None)],
        }];

        let show = from_item(&it, &asked(&own, &orders, "dvd"));
        let numbered: Vec<(i32, i32)> = show
            .episodes
            .as_deref()
            .unwrap()
            .iter()
            .map(|e| (e.season_number, e.episode_number))
            .collect();
        assert_eq!(numbered, [(0, 1), (1, 1), (1, 2), (2, 1)]);
        assert!(
            show.season_types.iter().all(|t| t.kind != "dvd"),
            "an order that places nothing is not offered either"
        );
    }

    #[test]
    fn a_season_named_only_by_its_number_has_no_title() {
        // TMDB's "Season 3" and "Saison 3", TheTVDB's "Specials": the number
        // again, which Sonarr v5 would write into the folder's name beside
        // the number it already carries. A name is a name.
        assert_eq!(own_season_title(Some("Season 3"), 3), None);
        assert_eq!(own_season_title(Some("Saison 3"), 3), None);
        assert_eq!(own_season_title(Some("Staffel 03"), 3), None);
        assert_eq!(own_season_title(Some("3"), 3), None);
        assert_eq!(own_season_title(Some("Specials"), 0), None);
        assert_eq!(own_season_title(Some("Hors-série"), 0), None);
        assert_eq!(own_season_title(Some("Épisodes spéciaux"), 0), None);
        assert_eq!(own_season_title(Some(" "), 1), None);
        assert_eq!(
            own_season_title(Some("Season 2"), 3).as_deref(),
            Some("Season 2")
        );
        assert_eq!(
            own_season_title(Some("The Final Chapter"), 4).as_deref(),
            Some("The Final Chapter")
        );
        assert_eq!(
            own_season_title(Some("Stranger Things 2"), 2).as_deref(),
            Some("Stranger Things 2")
        );
        assert_eq!(
            own_season_title(Some("東京編"), 1).as_deref(),
            Some("東京編")
        );
    }

    #[test]
    fn seasons_are_listed_once_each_with_a_title_and_a_year() {
        let mut it = series();
        // The same season from two providers, and an episode of a season
        // no provider described.
        it.seasons.push(season(1, Some("Tokyo"), None));
        it.episodes.push(episode(3, 1, 5, "2025-01-10"));
        let own = own(&it);
        let show = from_item(&it, &asked(&own, &[], OFFICIAL));

        let seasons: Vec<(i32, Option<&str>, Option<i32>)> = show
            .seasons
            .iter()
            .map(|s| (s.season_number, s.title.as_deref(), s.year))
            .collect();
        assert_eq!(
            seasons,
            [
                // No date of its own: the year of its first episode.
                (0, None, Some(2023)),
                (1, Some("東京編"), Some(2023)),
                (2, Some("京都編"), Some(2024)),
                (3, None, Some(2025)),
            ]
        );
        assert_eq!(show.season_types[0].season_numbers, [0, 1, 2, 3]);
    }

    #[test]
    fn an_episode_carries_both_names_of_its_ids_and_a_search_result_no_episodes() {
        let it = series();
        let own = own(&it);
        let sent = serde_json::to_value(from_item(&it, &asked(&own, &[], OFFICIAL))).unwrap();

        let first = &sent["episodes"][1];
        assert_eq!(first["tvdbShowId"], 416802);
        assert_eq!(first["metadataShowId"], 416802);
        assert_eq!(first["tvdbId"], 1);
        assert_eq!(first["metadataId"], 1);
        assert_eq!(sent["seasonTypes"][0]["type"], "official");
        assert_eq!(sent["metadataSource"], "tvdb");
        assert_eq!(sent["formats"], serde_json::json!(["anime"]));
        assert_eq!(sent["anidbIds"], serde_json::json!([]));
        assert_eq!(sent["language"], "fra");

        let mut search = asked(&own, &[], OFFICIAL);
        search.with_episodes = false;
        let sent = serde_json::to_value(from_item(&it, &search)).unwrap();
        assert!(sent.get("episodes").is_none(), "{sent}");
        assert_eq!(sent["seasons"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn an_id_this_server_made_says_where_the_work_comes_from() {
        let mut it = series();
        it.external_ids.mal.clear();
        let own = own(&it);
        for (client_id, source) in [
            (416802, "tvdb"),
            (100_209_867, "tmdb"),
            (200_000_012, "fankai"),
        ] {
            let mut by = asked(&own, &[], OFFICIAL);
            by.client_id = client_id;
            let show = from_item(&it, &by);
            assert_eq!(show.metadata_source, source);
            assert_eq!(show.tvdb_id, client_id);
            assert_eq!(show.formats, ["unknown"]);
            assert_eq!(
                show.episodes.as_deref().unwrap()[0].metadata_show_id,
                client_id
            );
        }
    }
}
