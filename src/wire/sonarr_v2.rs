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

use std::collections::{BTreeMap, HashMap, HashSet};

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
        orders::{self, Placed},
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
    /// the series: TheTVDB's, TMDB's episode groups, those made here.
    pub season_types: Vec<SeasonTypeResource>,
    pub actors: Vec<ActorResource>,
    pub images: Vec<ImageResource>,
    pub seasons: Vec<SeasonResourceV2>,
    /// Left out of a search result, as the real service leaves it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episodes: Option<Vec<EpisodeResourceV2>>,
    /// Who numbers the episodes: `tvdb`; `tmdb` for a series TMDB numbers —
    /// one TheTVDB does not list, under an id this server made for it (see
    /// [`ids`]), or one TheTVDB stopped answering for; `fankai` for a
    /// Fan-Kai; `local` for a series entered by hand that nothing elsewhere
    /// names.
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
    /// `Aired Order`, `DVD Order`, `Superfan Order`…: what Sonarr shows.
    pub name: String,
    /// What Sonarr asks the series in, and writes into the address it asks
    /// for it at: `official`; TheTVDB's `dvd`, `absolute`, `alternate`,
    /// `regional`, `altdvd`, `alttwo`; `tmdb-` and a TMDB episode group's
    /// id; `ams-` and a name for an order made here.
    #[serde(rename = "type")]
    pub kind: String,
    /// Empty for a TMDB group not read yet, which is read when the series is
    /// first asked for in it.
    pub season_numbers: Vec<i32>,
    /// The episodes of the regular seasons; the specials are not counted,
    /// as the real service does not count them. For a TMDB group not read
    /// yet, the count TMDB lists it with.
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
    /// different one in the answer would orphan the series. A TheTVDB id, or
    /// one this server made, for the TheTVDB edition; for the TMDB edition,
    /// TMDB's id, or the one a work no TMDB id names is known by
    /// ([`from_item_by_tmdb`]).
    pub client_id: i64,
    /// Three letters: the language the work's text was laid over in.
    pub language: &'a str,
    /// What the episodes are numbered by: [`OFFICIAL`] or [`TMDB_DEFAULT`],
    /// as the edition asked names its default, or one of the orders the
    /// work has.
    pub season_type: &'a str,
    /// The work as it was fetched (see [`OwnText`]).
    pub own: &'a OwnText,
    /// The orders the catalogue keeps for the work, the aired one first:
    /// `repo::order::list`.
    pub orders: &'a [EpisodeOrder],
    /// With the episodes, for a series asked for by id; without, for a
    /// search result.
    pub with_episodes: bool,
}

/// What both editions say of a series besides how its episodes are
/// numbered: the first edition's document, and the work's names in every
/// language it is held in.
struct Head {
    base: sonarr::ShowResource,
    original_title: String,
    original_language: Option<String>,
    translations: Vec<TranslationResource>,
}

fn head(item: &MediaItem, asked: &Asked<'_>) -> Head {
    let base = sonarr::from_item(item, asked.client_id, asked.language);

    // Sonarr finds the original title by comparing this to each
    // translation's language as text: both spelt the one way, or it finds
    // nothing.
    let original_language = item
        .original_language
        .as_deref()
        .map(language::normalize)
        .filter(|l| !l.is_empty());
    let translations = translations(
        item,
        asked.own,
        asked.language,
        original_language.as_deref(),
    );
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

    Head {
        base,
        original_title,
        original_language,
        translations,
    }
}

/// Render a work as Sonarr v5 expects it.
pub fn from_item(item: &MediaItem, asked: &Asked<'_>) -> ShowResourceV2 {
    let Head {
        base,
        original_title,
        original_language,
        translations,
    } = head(item, asked);

    // An order the work has in name only — its places all name episodes the
    // work no longer holds, as after a refresh without TheTVDB — is no order
    // to answer in: a Sonarr set to it would be sent no episode at all, and
    // would drop its own. The aired order stands in, as for a type unknown,
    // and the aired order answers with the order frozen as it, when one is.
    let (seasons, episodes) = match orders::in_use(item, asked.orders, asked.season_type) {
        Some(order) => placed(item, &orders::served(item, order), asked.client_id),
        None => match orders::as_official(item, asked.orders) {
            Some(frozen) => placed(item, &frozen, asked.client_id),
            None => aired(item, asked.client_id),
        },
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
        metadata_source: metadata_source(asked.client_id, asked.orders).to_string(),
    }
}

// ─── the TMDB edition ────────────────────────────────────────────────────────

/// What Skyhook's TMDB edition calls TMDB's own numbering: the season type
/// every series has there, and what a type the series has not is answered
/// in.
pub const TMDB_DEFAULT: &str = "default";

/// The name the TMDB edition gives TMDB's own numbering among its season
/// types.
const TMDB_DEFAULT_NAME: &str = "Default";

/// A series as Skyhook's TMDB edition serves it, `/v2/tmdb/…`: what the
/// TheTVDB edition says of it, asked by TMDB's id and numbered as TMDB
/// numbers it — its episodes known by their TMDB ids, TMDB's episode groups
/// its other orders under their bare ids — and without what the real service
/// leaves out of this edition: alternative titles, a time of day, an
/// episode's TheTVDB ids, its absolute number, where a special airs and its
/// rating.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SkyhookTmdbShowResource)]
pub struct ShowResourceTmdb {
    pub title: String,
    /// As the TheTVDB edition writes it: see [`ShowResourceV2`].
    pub original_title: String,
    /// The id the TheTVDB edition knows the series by: TheTVDB's, or the
    /// one this server made for a series TheTVDB does not list (see
    /// [`ids::for_client`]), so that a series Sonarr holds under it is
    /// known as the same. Left out for none, as the real service leaves it
    /// out of a series TheTVDB does not list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    /// The id asked by, echoed back: TMDB's, or for a work no TMDB id names
    /// — a Fan-Kai, a series entered by hand — the one Sonarr knows it by.
    pub tmdb_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tv_rage_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tv_maze_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    pub slug: String,
    pub formats: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
    pub language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_aired: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_aired: Option<String>,
    pub mal_ids: Vec<i64>,
    pub anidb_ids: Vec<i64>,
    pub ani_list_ids: Vec<i64>,
    pub last_updated: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_network: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<RatingResource>,
    pub translations: Vec<TranslationResource>,
    /// TMDB's own numbering first, `default`; then TMDB's episode groups,
    /// each under its id on TMDB, and the orders made here.
    pub season_types: Vec<SeasonTypeResource>,
    pub actors: Vec<ActorResource>,
    pub images: Vec<ImageResource>,
    pub seasons: Vec<SeasonResourceV2>,
    /// Left out of a search result, as the real service leaves it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episodes: Option<Vec<EpisodeResourceTmdb>>,
    /// `tmdb`; `fankai` for a Fan-Kai, `local` for a series entered by hand
    /// that nothing elsewhere names.
    pub metadata_source: String,
}

/// An episode as Skyhook's TMDB edition serves it: known by its TMDB id.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SkyhookTmdbEpisodeResource)]
pub struct EpisodeResourceTmdb {
    /// The id the series was asked by.
    pub tmdb_show_id: i64,
    /// TMDB's id for the episode; for one TMDB gave none — an episode
    /// TheTVDB alone lists, a record written here — a negative one made of
    /// what names it, the same at every answer.
    pub tmdb_id: i64,
    pub season_number: i32,
    pub episode_number: i32,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_date_utc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finale_type: Option<String>,
}

/// Render a work as Skyhook's TMDB edition serves it (see
/// [`ShowResourceTmdb`]), numbered by the season type asked: `default`,
/// TMDB's own numbering; a TMDB group by its bare id; an order made here by
/// its key; any other order by its key, as the TheTVDB edition answers one
/// it does not offer. A type the series has not is answered in TMDB's own
/// numbering — where the real service answers no episode at all, which
/// would have Sonarr drop every one it holds.
///
/// TMDB's own numbering is the order kept of it (`tmdb-aired`), read at
/// every refresh that reads TMDB's seasons. While none is kept that places
/// an episode, the work's numbering stands in, as the aired order answers:
/// TMDB's for a series TMDB numbers in TheTVDB's place, the one written here
/// for a series entered by hand — TheTVDB's only where nothing better is to
/// be had, which the edition's handler sees to (`api::sonarr`).
pub fn from_item_by_tmdb(item: &MediaItem, asked: &Asked<'_>) -> ShowResourceTmdb {
    let Head {
        base,
        original_title,
        original_language,
        translations,
    } = head(item, asked);

    let (numbered, as_aired) = by_tmdb_numbering(item, asked.orders, asked.season_type);
    let (seasons, episodes) = tmdb_placed(item, &numbered, asked.client_id, as_aired);

    ShowResourceTmdb {
        title: base.title,
        original_title,
        tvdb_id: ids::for_client(
            crate::domain::MediaKind::Series,
            &item.id,
            item.is_manual,
            &item.external_ids,
        ),
        tmdb_id: asked.client_id,
        tv_rage_id: base.tv_rage_id,
        tv_maze_id: base.tv_maze_id,
        imdb_id: base.imdb_id,
        overview: base.overview,
        slug: base.slug,
        formats: formats(item),
        original_country: base.original_country,
        original_language,
        language: asked.language.to_string(),
        first_aired: base.first_aired,
        last_aired: base.last_aired,
        mal_ids: base.mal_ids,
        anidb_ids: Vec::new(),
        ani_list_ids: base.ani_list_ids,
        last_updated: base.last_updated,
        status: base.status,
        // With the episodes alone, as the real service writes it: a search's
        // result has none.
        runtime: base.runtime.filter(|_| asked.with_episodes),
        original_network: base.original_network,
        network: base.network,
        genres: base.genres,
        content_rating: base.content_rating,
        rating: base.rating,
        translations,
        season_types: tmdb_season_types(item, asked.orders),
        actors: base.actors,
        images: base.images,
        seasons,
        episodes: asked.with_episodes.then_some(episodes),
        metadata_source: tmdb_metadata_source(asked.client_id, asked.orders).to_string(),
    }
}

/// The order a season type of the TMDB edition names, other than TMDB's own
/// numbering, when the series has it and it places an episode: a TMDB group
/// by its bare id, or by this server's key for it; any other order by its
/// key. None for `default`, and for a type the series has not.
pub fn tmdb_edition_order<'a>(
    item: &'a MediaItem,
    orders: &'a [EpisodeOrder],
    season_type: &str,
) -> Option<&'a EpisodeOrder> {
    let asked = season_type.trim();
    if asked.eq_ignore_ascii_case(TMDB_DEFAULT) {
        return None;
    }
    let group = format!("{}{asked}", orders::TMDB_GROUP);
    orders::in_use(item, orders, asked).or_else(|| orders::in_use(item, orders, &group))
}

/// The places a series is answered with by TMDB's id, for the season type
/// asked, and whether they are the work's own numbering as it aired.
fn by_tmdb_numbering<'a>(
    item: &'a MediaItem,
    orders: &'a [EpisodeOrder],
    season_type: &str,
) -> (Vec<Placed<'a>>, bool) {
    match tmdb_edition_order(item, orders, season_type) {
        Some(order) => (orders::served(item, order), false),
        None => tmdbs_own(item, orders),
    }
}

/// TMDB's own numbering of a series, as clients are given it: the order
/// kept of it, or while none is kept that places an episode, the work's
/// numbering — the order frozen as it, when one is — and whether that is the
/// work's own numbering as it aired.
fn tmdbs_own<'a>(item: &'a MediaItem, orders: &'a [EpisodeOrder]) -> (Vec<Placed<'a>>, bool) {
    match orders::in_use(item, orders, orders::TMDB_AIRED) {
        Some(order) => (orders::served(item, order), false),
        None => match orders::as_official(item, orders) {
            Some(frozen) => (frozen, false),
            None => (orders::aired_placements(item), true),
        },
    }
}

/// The TMDB edition's season types: TMDB's own numbering, then each TMDB
/// group that places or announces an episode, by its bare id as the real
/// service names it — whatever TheTVDB keeps beside it, which this edition
/// does not list — then the orders made here, each under its own name, told
/// apart from another of the same name by its source.
fn tmdb_season_types(item: &MediaItem, orders: &[EpisodeOrder]) -> Vec<SeasonTypeResource> {
    let (own, as_aired) = tmdbs_own(item, orders);
    let mut default = counted(TMDB_DEFAULT, TMDB_DEFAULT_NAME, &own);
    if as_aired {
        // As the aired order is offered: its seasons with no episode too.
        default.season_numbers = sorted_distinct(
            item.seasons
                .iter()
                .map(|s| s.season_number)
                .chain(own.iter().map(|p| p.season_number)),
        );
    }

    let listed: Vec<&EpisodeOrder> = orders
        .iter()
        .filter(|order| !order.is_default && order.key != orders::TMDB_AIRED)
        .filter(|order| {
            order.source == crate::providers::names::TMDB
                || order.source == crate::db::repo::order::LOCAL
        })
        .filter(|order| orders::size(item, order) > 0)
        .collect();
    let names = orders::client_names(item, orders, &listed, TMDB_DEFAULT_NAME);

    let mut types = vec![default];
    for (order, name) in listed.into_iter().zip(names) {
        let kind = match order.source.as_str() {
            crate::providers::names::TMDB => order
                .key
                .strip_prefix(orders::TMDB_GROUP)
                .unwrap_or(&order.key),
            _ => order.key.as_str(),
        };
        types.push(if order.episodes.is_empty() {
            // A TMDB group not read yet: what TMDB lists it with.
            SeasonTypeResource {
                name,
                kind: kind.to_string(),
                season_numbers: Vec::new(),
                episode_count: order.announced.unwrap_or(0),
            }
        } else {
            counted(kind, &name, &orders::served(item, order))
        });
    }
    types
}

/// The seasons and episodes of the TMDB edition, as `placements` number
/// them. A season lends the aired season of its number its pictures, and
/// its name — in the language asked, where one is held in it — where it is
/// that season: the numbering is the work's own as it aired (`as_aired`),
/// or it places that season's episodes there and nowhere else
/// ([`seasons_as_held`]), as TMDB numbers most series. A season TMDB cuts
/// otherwise is another one, its name another's.
fn tmdb_placed(
    item: &MediaItem,
    placements: &[Placed<'_>],
    client_id: i64,
    as_aired: bool,
) -> (Vec<SeasonResourceV2>, Vec<EpisodeResourceTmdb>) {
    let mut seasons = placed_seasons(item, placements);
    let held = seasons_as_held(placements);
    for season in &mut seasons {
        if as_aired || held.contains(&season.season_number) {
            season.title = item
                .seasons
                .iter()
                .find(|s| s.season_number == season.season_number)
                .and_then(|s| own_season_title(s.title.as_deref(), s.season_number));
        }
    }
    let episodes = placements
        .iter()
        .map(|placed| {
            let e = sonarr::episode_resource(&placed.episode, client_id);
            EpisodeResourceTmdb {
                tmdb_show_id: client_id,
                tmdb_id: tmdb_episode_id(placed),
                season_number: placed.season_number,
                episode_number: placed.episode_number,
                title: e.title,
                air_date: e.air_date,
                air_date_utc: e.air_date_utc,
                runtime: e.runtime,
                overview: e.overview,
                image: e.image,
                finale_type: e.finale_type,
            }
        })
        .collect();
    (seasons, episodes)
}

/// The id an episode is known by in the TMDB edition: TMDB's, as the order
/// names it; else one made of its TheTVDB id, or of what names a record of
/// an order's own — negative, never one of TMDB's, the same at every answer.
fn tmdb_episode_id(placed: &Placed<'_>) -> i64 {
    match (placed.tmdb_id, placed.episode.tvdb_id) {
        (Some(tmdb), _) => tmdb,
        (None, Some(tvdb)) => ids::own_episode(&format!("tvdb:{tvdb}")),
        (None, None) => orders::own_episode_id(&placed.episode),
    }
}

/// Who numbers the episodes in the TMDB edition: TMDB, but for a Fan-Kai,
/// numbered by Fankai, and a series entered by hand that nothing elsewhere
/// names, numbered here.
fn tmdb_metadata_source(client_id: i64, orders: &[EpisodeOrder]) -> &'static str {
    match orders::aired(orders).map(|order| order.source.as_str()) {
        Some("fankai") => return "fankai",
        Some(crate::db::repo::order::LOCAL) => return crate::db::repo::order::LOCAL,
        _ => {}
    }
    if ids::from_fankai(client_id).is_some() {
        "fankai"
    } else if ids::is_own(client_id) {
        crate::db::repo::order::LOCAL
    } else {
        "tmdb"
    }
}

/// Every language the work is named in, by code.
///
/// The translations held, each under its three-letter code and only when
/// it names the work: a title in no language is nothing to Sonarr, which
/// drops the entry. One in the language asked that has an overview and no
/// title — TMDB leaves the name out where it is the one the work already
/// goes by, as Breaking Bad's in French — is listed under the title the
/// answer gives, which is the work's name in that language. The work's own
/// text counts as one, under the language it is stored in, when no
/// translation is held in that language. And the work's original language
/// has an entry whenever a title for it is known — the provider's original
/// title, where no translation is held — because that entry is where Sonarr
/// reads the original title from.
fn translations(
    item: &MediaItem,
    own: &OwnText,
    asked_language: &str,
    original_language: Option<&str>,
) -> Vec<TranslationResource> {
    let mut held: BTreeMap<String, TranslationResource> = BTreeMap::new();

    for translation in &item.translations {
        let code = language::normalize(&translation.language);
        if code.is_empty() {
            continue;
        }
        let overview = non_empty(translation.overview.as_deref());
        let Some(title) = non_empty(translation.title.as_deref()).or_else(|| {
            (code == asked_language && overview.is_some())
                .then(|| non_empty(Some(item.title.as_str())))
                .flatten()
        }) else {
            continue;
        };
        held.entry(code.clone()).or_insert(TranslationResource {
            language: code,
            title,
            overview,
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

/// The orders Sonarr may ask the series in: the aired one — numbered by the
/// order frozen as it, when one is — then each other order the catalogue
/// offers for it (`orders::offered`), under its own name, told apart from
/// another of the same name by its source (`orders::client_names`). Not
/// TMDB's own numbering, which TheTVDB's documents do not have: it is what
/// Skyhook's TMDB edition answers by ([`from_item_by_tmdb`]). Asked for by
/// its key, it is answered all the same.
fn season_types(item: &MediaItem, orders: &[EpisodeOrder]) -> Vec<SeasonTypeResource> {
    let official = match orders::as_official(item, orders) {
        Some(frozen) => counted(OFFICIAL, orders::AIRED_LABEL, &frozen),
        None => SeasonTypeResource {
            name: orders::AIRED_LABEL.to_string(),
            kind: OFFICIAL.to_string(),
            season_numbers: sorted_distinct(
                item.seasons
                    .iter()
                    .map(|s| s.season_number)
                    .chain(item.episodes.iter().map(|e| e.season_number)),
            ),
            episode_count: regular(item.episodes.iter().map(|e| e.season_number)),
        },
    };

    let mut types = vec![official];
    let offered: Vec<&EpisodeOrder> = orders::offered(item, orders)
        .into_iter()
        .filter(|order| order.key != orders::TMDB_AIRED)
        .collect();
    let names = orders::client_names(item, orders, &offered, orders::AIRED_LABEL);
    for (order, name) in offered.into_iter().zip(names) {
        types.push(if order.episodes.is_empty() {
            // A TMDB group not read yet: what TMDB lists it with.
            SeasonTypeResource {
                name,
                kind: order.key.clone(),
                season_numbers: Vec::new(),
                episode_count: order.announced.unwrap_or(0),
            }
        } else {
            counted(&order.key, &name, &orders::served(item, order))
        });
    }
    types
}

/// An order as Sonarr is offered it: its seasons and how many regular
/// episodes it places.
fn counted(kind: &str, name: &str, placed: &[Placed<'_>]) -> SeasonTypeResource {
    SeasonTypeResource {
        name: name.to_string(),
        kind: kind.to_string(),
        season_numbers: sorted_distinct(placed.iter().map(|p| p.season_number)),
        episode_count: regular(placed.iter().map(|p| p.season_number)),
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

/// The seasons and episodes as one of the work's orders places them: the
/// same episodes, under that order's seasons and numbers, and the records it
/// carries of episodes the work does not hold, as episodes of their own.
///
/// The order's seasons have names and pictures of their own on TheTVDB,
/// which the catalogue does not keep: the aired season of the same number
/// lends its pictures, and no name. The year is that of the first episode
/// placed in the season.
fn placed(
    item: &MediaItem,
    placements: &[Placed<'_>],
    client_id: i64,
) -> (Vec<SeasonResourceV2>, Vec<EpisodeResourceV2>) {
    let seasons = placed_seasons(item, placements);
    let mut episodes = Vec::with_capacity(placements.len());
    for placed in placements {
        let mut resource = episode_resource(&placed.episode, client_id);
        resource.season_number = placed.season_number;
        resource.episode_number = placed.episode_number;
        resource.absolute_episode_number = placed.absolute_number;
        // Where a special airs among the aired seasons says nothing of where
        // the order puts it.
        resource.aired_after_season_number = None;
        resource.aired_before_season_number = None;
        resource.aired_before_episode_number = None;
        if placed.own && resource.tvdb_id.is_none() {
            // An episode of the order's own that TheTVDB gave no id: one
            // made of what names it, the same at every answer.
            let id = Some(orders::own_episode_id(&placed.episode));
            resource.tvdb_id = id;
            resource.metadata_id = id;
        }
        episodes.push(resource);
    }

    (seasons, episodes)
}

/// The seasons a numbering places as the work holds them: each episode of
/// the work's it places in one is of the work's season of that number, and
/// none of that season's goes into another. The order's own records of
/// episodes the work does not hold say nothing either way.
fn seasons_as_held(placements: &[Placed<'_>]) -> HashSet<i32> {
    let mut held = HashSet::new();
    let mut strayed = HashSet::new();
    for placed in placements.iter().filter(|placed| !placed.own) {
        let aired = placed.episode.season_number;
        if aired == placed.season_number {
            held.insert(aired);
        } else {
            strayed.insert(aired);
            strayed.insert(placed.season_number);
        }
    }
    held.retain(|season| !strayed.contains(season));
    held
}

/// The seasons an order's places fall in, each once: no name of their own,
/// the year of the first episode placed in each, the aired season of the
/// same number's pictures.
fn placed_seasons(item: &MediaItem, placements: &[Placed<'_>]) -> Vec<SeasonResourceV2> {
    let first_years = first_years(
        placements
            .iter()
            .map(|placed| (placed.season_number, placed.episode.air_date.as_deref())),
    );
    let mut aired: HashMap<i32, &Season> = HashMap::new();
    for season in &item.seasons {
        aired.entry(season.season_number).or_insert(season);
    }

    let mut seasons: BTreeMap<i32, SeasonResourceV2> = BTreeMap::new();
    for placed in placements {
        seasons
            .entry(placed.season_number)
            .or_insert_with(|| SeasonResourceV2 {
                season_number: placed.season_number,
                title: None,
                year: first_years.get(&placed.season_number).copied(),
                images: aired
                    .get(&placed.season_number)
                    .map(|season| season_images(season))
                    .unwrap_or_default(),
            });
    }
    seasons.into_values().collect()
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

/// Who numbers the episodes: whoever numbers the aired order, as the orders
/// say — TMDB for a series it numbers while TheTVDB does not answer for it —
/// and, where they say nothing, where the id Sonarr holds the series under
/// comes from: TheTVDB, or this server's own ranges for a work TheTVDB does
/// not list — the catalogue itself, `local`, for one entered by hand.
fn metadata_source(client_id: i64, orders: &[EpisodeOrder]) -> &'static str {
    match orders::aired(orders).map(|order| order.source.as_str()) {
        Some("tvdb") => return "tvdb",
        Some("tmdb") => return "tmdb",
        Some("fankai") => return "fankai",
        _ => {}
    }
    if ids::from_fankai(client_id).is_some() {
        "fankai"
    } else if ids::from_synthetic(client_id).is_some() {
        "tmdb"
    } else if ids::is_own(client_id) {
        crate::db::repo::order::LOCAL
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
        domain::{
            CoverType, ExternalIds, Image, MediaKind, OrderKind, OrderRecord, PlacedEpisode,
            Translation,
        },
    };

    fn translation(language: &str, title: &str, overview: Option<&str>) -> Translation {
        Translation {
            language: language.into(),
            title: Some(title.into()),
            overview: overview.map(str::to_string),
            tagline: None,
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
            absolute_number: absolute,
            ..PlacedEpisode::of_tvdb(tvdb_id, season, number)
        }
    }

    /// An order of the catalogue's, as `repo::order::list` gives it.
    fn order(key: &str, label: &str, episodes: Vec<PlacedEpisode>) -> EpisodeOrder {
        EpisodeOrder {
            key: key.into(),
            kind: OrderKind::of_tvdb(key),
            source: "tvdb".into(),
            source_ref: None,
            label: label.into(),
            is_default: false,
            is_frozen: false,
            announced: None,
            updated_at: "2026-10-19T00:00:00.000Z".into(),
            episodes,
        }
    }

    /// The aired order, numbered by `source`.
    fn aired_order(source: &str) -> EpisodeOrder {
        EpisodeOrder {
            kind: OrderKind::Aired,
            source: source.into(),
            is_default: true,
            ..order(OFFICIAL, "Aired Order", Vec::new())
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
            aired_order("tvdb"),
            order(
                "dvd",
                "DVD Order",
                vec![
                    placed(4, 0, 1, None),
                    placed(2, 1, 1, None),
                    placed(1, 1, 2, None),
                    placed(3, 2, 1, None),
                    // An episode the work no longer has, and a place given
                    // twice.
                    placed(99, 2, 2, None),
                    placed(1, 1, 2, None),
                ],
            ),
            order(
                "absolute",
                "Absolute Order",
                vec![
                    placed(1, 1, 1, Some(1)),
                    placed(2, 1, 2, Some(2)),
                    placed(3, 1, 3, Some(3)),
                ],
            ),
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
            tagline: None,
            is_manual: false,
        });
        let show = from_item(&it, &asked(&own, &[], OFFICIAL));
        assert!(show.translations.iter().all(|t| t.language != "ita"));
    }

    #[test]
    fn the_language_asked_is_listed_when_only_its_overview_is_held() {
        // TMDB leaves a translation's name out where it is the name the
        // series already goes by: an overview in French, and no title.
        let mut it = series();
        for french in it.translations.iter_mut().filter(|t| t.language == "fra") {
            french.title = None;
        }
        let own = own(&it);
        let show = from_item(&it, &asked(&own, &[], OFFICIAL));

        // Asked in French, the series is named in French by the title the
        // answer gives, with the overview held in French.
        let french = show
            .translations
            .iter()
            .find(|t| t.language == "fra")
            .expect("the language asked is listed");
        assert_eq!(french.title, show.title);
        assert_eq!(
            french.overview.as_deref(),
            Some("Un vagabond au sabre inversé.")
        );

        // In another language than the one asked, an overview names nothing.
        it.translations.push(Translation {
            language: "ita".into(),
            title: None,
            overview: Some("Un vagabondo.".into()),
            tagline: None,
            is_manual: false,
        });
        let show = from_item(&it, &asked(&own, &[], OFFICIAL));
        assert!(show.translations.iter().all(|t| t.language != "ita"));

        // Nor does a translation with neither, in the language asked.
        for french in it.translations.iter_mut().filter(|t| t.language == "fra") {
            french.overview = None;
        }
        let show = from_item(&it, &asked(&own, &[], OFFICIAL));
        assert!(show.translations.iter().all(|t| t.language != "fra"));
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
        let orphan = vec![order(
            "regional",
            "Netflix Order",
            vec![placed(99, 1, 1, None)],
        )];
        let show = from_item(&it, &asked(&own, &orphan, OFFICIAL));
        assert_eq!(show.season_types.len(), 1);
    }

    #[test]
    fn a_tmdb_group_named_as_one_of_thetvdbs_orders_says_its_source_in_sonarrs_list() {
        let it = series();
        let own = own(&it);
        let mut orders = orders();
        orders.push(EpisodeOrder {
            kind: OrderKind::Digital,
            source: "tmdb".into(),
            source_ref: Some("a4".into()),
            announced: Some(186),
            ..order("tmdb-a4", "DVD Order", Vec::new())
        });
        let show = from_item(&it, &asked(&own, &orders, OFFICIAL));
        let names: Vec<(&str, &str)> = show
            .season_types
            .iter()
            .map(|t| (t.kind.as_str(), t.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("official", "Aired Order"),
                ("dvd", "DVD Order"),
                ("absolute", "Absolute Order"),
                ("tmdb-a4", "DVD Order (TMDB)"),
            ]
        );
        // The order itself keeps its name.
        assert_eq!(orders[3].label, "DVD Order");
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
        let orders = vec![order(
            "dvd",
            "DVD Order",
            vec![placed(98, 1, 1, None), placed(99, 1, 2, None)],
        )];

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
    fn an_order_of_records_the_aired_list_lacks_is_offered_and_served_whole() {
        // The Office's Superfan order: extended cuts, each an episode of its
        // own on TheTVDB; and an order made here with a record somebody
        // wrote, which TheTVDB gave no id.
        let it = series();
        let own_text = own(&it);
        let record = |title: &str| OrderRecord {
            title: title.into(),
            overview: None,
            aired: Some("2023-07-07".into()),
            runtime: Some(31),
            image: None,
            finale_type: None,
        };
        let superfan = order(
            "alttwo",
            "Superfan Order",
            vec![
                PlacedEpisode {
                    record: Some(record("Pilot (Extended Cut)")),
                    ..placed(10718410, 1, 1, None)
                },
                PlacedEpisode {
                    record: Some(record("Diversity Day (Extended Cut)")),
                    ..placed(10718411, 1, 2, None)
                },
            ],
        );
        let mut written = PlacedEpisode {
            record: Some(record("A night at the cinema")),
            tvdb_id: None,
            ..placed(0, 0, 9, None)
        };
        written.id = "place-1".into();
        let mine = EpisodeOrder {
            kind: OrderKind::Custom,
            source: "local".into(),
            ..order("ams-mine", "Mine", vec![placed(2, 1, 1, None), written])
        };
        let orders = vec![aired_order("tvdb"), superfan, mine];

        let show = from_item(&it, &asked(&own_text, &orders, OFFICIAL));
        let offered: Vec<(&str, &str, i32)> = show
            .season_types
            .iter()
            .map(|t| (t.kind.as_str(), t.name.as_str(), t.episode_count))
            .collect();
        assert_eq!(
            offered,
            [
                ("official", "Aired Order", 3),
                ("alttwo", "Superfan Order", 2),
                // Its own record, a special, is not counted.
                ("ams-mine", "Mine", 1),
            ]
        );

        let show = from_item(&it, &asked(&own_text, &orders, "alttwo"));
        let served: Vec<(i32, i32, Option<i64>, &str)> = show
            .episodes
            .as_deref()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e.season_number,
                    e.episode_number,
                    e.tvdb_id,
                    e.title.as_str(),
                )
            })
            .collect();
        assert_eq!(
            served,
            [
                (1, 1, Some(10718410), "Pilot (Extended Cut)"),
                (1, 2, Some(10718411), "Diversity Day (Extended Cut)")
            ]
        );

        // A record TheTVDB gave no id is served under one of its own:
        // negative, never one of TheTVDB's, and the same at every answer.
        let show = from_item(&it, &asked(&own_text, &orders, "ams-mine"));
        let night = show
            .episodes
            .as_deref()
            .unwrap()
            .iter()
            .find(|e| e.title == "A night at the cinema")
            .expect("served");
        let id = night.tvdb_id.expect("an id");
        assert!(id < 0, "{id}");
        assert_eq!(night.metadata_id, Some(id));
        let again = from_item(&it, &asked(&own_text, &orders, "ams-mine"));
        assert!(
            again
                .episodes
                .unwrap()
                .iter()
                .any(|e| e.tvdb_id == Some(id))
        );
    }

    #[test]
    fn a_frozen_order_is_what_the_aired_order_answers() {
        let it = series();
        let own = own(&it);
        let mut orders = orders();
        orders[1].is_frozen = true;

        for season_type in [OFFICIAL, "nonsense"] {
            let show = from_item(&it, &asked(&own, &orders, season_type));
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
                ],
                "{season_type:?}"
            );
            assert_eq!(show.season_types[0].kind, OFFICIAL);
            assert_eq!(show.season_types[0].episode_count, 3);
        }
        // The other orders are still offered, and answered, as themselves.
        let show = from_item(&it, &asked(&own, &orders, "absolute"));
        assert_eq!(show.episodes.as_deref().unwrap().len(), 3);
    }

    #[test]
    fn the_source_that_numbers_the_series_is_said() {
        let it = series();
        let own = own(&it);
        let tvdb = vec![aired_order("tvdb")];
        let tmdb = vec![aired_order("tmdb")];
        assert_eq!(
            from_item(&it, &asked(&own, &tvdb, OFFICIAL)).metadata_source,
            "tvdb"
        );
        // TMDB numbers it while TheTVDB does not answer for it.
        assert_eq!(
            from_item(&it, &asked(&own, &tmdb, OFFICIAL)).metadata_source,
            "tmdb"
        );
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

    // ─── the TMDB edition ────────────────────────────────────────────────────

    /// A TMDB place: TMDB's id and record, and the episode the series holds
    /// for it by TheTVDB's id, where it holds one — as `orders` links it.
    fn tmdb_place(tmdb_id: i64, tvdb_id: Option<i64>, season: i32, number: i32) -> PlacedEpisode {
        PlacedEpisode {
            tmdb_id: Some(tmdb_id),
            tvdb_id,
            record: Some(OrderRecord {
                title: format!("TMDB's {tmdb_id}"),
                overview: Some("As TMDB tells it.".into()),
                aired: Some("2024-10-04".into()),
                runtime: Some(24),
                image: Some(format!("https://image.tmdb.org/t/p/original/{tmdb_id}.jpg")),
                finale_type: None,
            }),
            ..PlacedEpisode::of_tvdb(0, season, number)
        }
    }

    /// TMDB's own numbering of *Rurouni Kenshin*, as a refresh keeps it:
    /// the Kyoto arc's first episode at the end of a first season, and one
    /// episode the series does not hold.
    fn tmdb_aired(places: Vec<PlacedEpisode>) -> EpisodeOrder {
        EpisodeOrder {
            kind: OrderKind::Aired,
            source: "tmdb".into(),
            ..order(
                orders::TMDB_AIRED,
                crate::domain::orders::TMDB_AIRED_LABEL,
                places,
            )
        }
    }

    fn kenshins_tmdb_orders() -> Vec<EpisodeOrder> {
        let mut all = orders();
        all.push(tmdb_aired(vec![
            tmdb_place(904, Some(4), 0, 1),
            tmdb_place(901, Some(1), 1, 1),
            tmdb_place(902, Some(2), 1, 2),
            tmdb_place(903, Some(3), 1, 3),
            tmdb_place(905, None, 1, 4),
        ]));
        all.push(EpisodeOrder {
            kind: OrderKind::Dvd,
            source: "tmdb".into(),
            source_ref: Some("69f50757054263b7bc87e32a".into()),
            ..order(
                "tmdb-69f50757054263b7bc87e32a",
                "DVD / PVOD",
                vec![
                    tmdb_place(901, Some(1), 1, 1),
                    tmdb_place(902, Some(2), 2, 1),
                ],
            )
        });
        all.push(EpisodeOrder {
            kind: OrderKind::Custom,
            source: "local".into(),
            ..order("ams-mine", "Mine", vec![placed(2, 1, 1, None)])
        });
        all
    }

    fn asked_by_tmdb<'a>(
        own: &'a OwnText,
        orders: &'a [EpisodeOrder],
        season_type: &'a str,
    ) -> Asked<'a> {
        Asked {
            client_id: 209867,
            ..asked(own, orders, season_type)
        }
    }

    fn tmdb_numbers(show: &ShowResourceTmdb) -> Vec<(i32, i32, i64)> {
        show.episodes
            .as_deref()
            .unwrap()
            .iter()
            .map(|e| (e.season_number, e.episode_number, e.tmdb_id))
            .collect()
    }

    #[test]
    fn tmdbs_own_numbering_is_no_season_type_of_thetvdbs_and_answered_by_its_key() {
        let it = series();
        let own = own(&it);
        let orders = kenshins_tmdb_orders();

        let show = from_item(&it, &asked(&own, &orders, OFFICIAL));
        let kinds: Vec<&str> = show.season_types.iter().map(|t| t.kind.as_str()).collect();
        // TheTVDB's documents have no such order; the orders made here, as
        // before — and TMDB's DVD group not, which holds fewer episodes than
        // TheTVDB's DVD order.
        assert_eq!(kinds, ["official", "dvd", "absolute", "ams-mine"]);

        // Asked for by its key, it numbers the series all the same.
        let show = from_item(&it, &asked(&own, &orders, orders::TMDB_AIRED));
        let numbered: Vec<(i32, i32, Option<i64>)> = show
            .episodes
            .as_deref()
            .unwrap()
            .iter()
            .map(|e| (e.season_number, e.episode_number, e.tvdb_id))
            .collect();
        assert_eq!(numbered[3], (1, 3, Some(3)));
    }

    #[test]
    fn the_tmdb_edition_numbers_a_series_as_tmdb_does_each_episode_by_its_tmdb_id() {
        let it = series();
        let own = own(&it);
        let orders = kenshins_tmdb_orders();

        let show = from_item_by_tmdb(&it, &asked_by_tmdb(&own, &orders, TMDB_DEFAULT));
        assert_eq!(
            tmdb_numbers(&show),
            [
                (0, 1, 904),
                (1, 1, 901),
                (1, 2, 902),
                (1, 3, 903),
                (1, 4, 905)
            ]
        );
        let episodes = show.episodes.as_deref().unwrap();
        // The episodes the series holds, as it holds them; TMDB's record of
        // the one it does not.
        assert_eq!(episodes[3].title, "Episode 3");
        assert_eq!(episodes[4].title, "TMDB's 905");
        assert!(episodes.iter().all(|e| e.tmdb_show_id == 209867));
        assert_eq!(show.tmdb_id, 209867);
        assert_eq!(show.tvdb_id, Some(416802));
        assert_eq!(show.metadata_source, "tmdb");

        // TMDB's own numbering, then TMDB's group by its bare id, then the
        // order made here; none of TheTVDB's.
        let types: Vec<(&str, &str, &[i32], i32)> = show
            .season_types
            .iter()
            .map(|t| {
                (
                    t.kind.as_str(),
                    t.name.as_str(),
                    t.season_numbers.as_slice(),
                    t.episode_count,
                )
            })
            .collect();
        assert_eq!(
            types,
            [
                ("default", "Default", &[0, 1][..], 4),
                ("69f50757054263b7bc87e32a", "DVD / PVOD", &[1, 2][..], 2),
                ("ams-mine", "Mine", &[1][..], 1),
            ]
        );

        // A group asked for by its bare id, as the real service names it.
        let show = from_item_by_tmdb(
            &it,
            &asked_by_tmdb(&own, &orders, "69f50757054263b7bc87e32a"),
        );
        assert_eq!(tmdb_numbers(&show), [(1, 1, 901), (2, 1, 902)]);

        // A type the series has not, `official` among them: TMDB's own
        // numbering, where the real service answers no episode at all.
        for season_type in [OFFICIAL, "nonsense", "Default"] {
            let show = from_item_by_tmdb(&it, &asked_by_tmdb(&own, &orders, season_type));
            assert_eq!(tmdb_numbers(&show).len(), 5, "{season_type:?}");
        }

        // What the TMDB edition leaves out.
        let show = from_item_by_tmdb(&it, &asked_by_tmdb(&own, &orders, TMDB_DEFAULT));
        let sent = serde_json::to_value(&show).unwrap();
        for key in ["alternativeTitles", "timeOfDay"] {
            assert!(sent.get(key).is_none(), "{key}");
        }
        for key in [
            "tvdbId",
            "tvdbShowId",
            "metadataId",
            "absoluteEpisodeNumber",
        ] {
            assert!(sent["episodes"][0].get(key).is_none(), "{key}");
        }
        assert_eq!(sent["episodes"][0]["tmdbId"], 904);
    }

    #[test]
    fn the_tmdb_edition_names_a_season_tmdb_cuts_as_the_series_does_and_no_other() {
        let it = series();
        let own = own(&it);
        let titles = |show: &ShowResourceTmdb| -> Vec<(i32, Option<String>)> {
            show.seasons
                .iter()
                .map(|s| (s.season_number, s.title.clone()))
                .collect()
        };

        // TMDB's first season ends with the Kyoto arc's first episode: it is
        // neither of the series' seasons, and is given no name of theirs.
        let kenshin = kenshins_tmdb_orders();
        let show = from_item_by_tmdb(&it, &asked_by_tmdb(&own, &kenshin, TMDB_DEFAULT));
        assert_eq!(titles(&show), [(0, None), (1, None)]);

        // Cut as the series is: each season its own name.
        let mut alike = orders();
        alike.push(tmdb_aired(vec![
            tmdb_place(904, Some(4), 0, 1),
            tmdb_place(901, Some(1), 1, 1),
            tmdb_place(902, Some(2), 1, 2),
            tmdb_place(903, Some(3), 2, 1),
        ]));
        let show = from_item_by_tmdb(&it, &asked_by_tmdb(&own, &alike, TMDB_DEFAULT));
        assert_eq!(
            titles(&show),
            [
                (0, None),
                (1, Some("東京編".into())),
                (2, Some("京都編".into()))
            ]
        );
    }

    #[test]
    fn without_tmdbs_own_numbering_the_tmdb_edition_answers_as_the_series_is_numbered() {
        let mut it = series();
        it.episodes[1].tmdb_id = Some(901);
        let own = own(&it);
        let orders = vec![aired_order("tvdb")];

        let show = from_item_by_tmdb(&it, &asked_by_tmdb(&own, &orders, TMDB_DEFAULT));
        let numbered = tmdb_numbers(&show);
        assert_eq!(
            numbered
                .iter()
                .map(|(s, n, _)| (*s, *n))
                .collect::<Vec<_>>(),
            [(0, 1), (1, 1), (1, 2), (2, 1)]
        );
        // TMDB's id where the series holds one; else one made of TheTVDB's,
        // negative, the same at every answer.
        assert_eq!(numbered[1].2, 901);
        assert!(numbered[0].2 < 0);
        let again = from_item_by_tmdb(&it, &asked_by_tmdb(&own, &orders, TMDB_DEFAULT));
        assert_eq!(tmdb_numbers(&again), numbered);
        // The work's own seasons, named as they aired.
        assert_eq!(show.seasons[1].title.as_deref(), Some("東京編"));
        assert_eq!(show.season_types[0].season_numbers, [0, 1, 2]);
    }

    #[test]
    fn a_work_no_tmdb_id_names_says_who_numbers_it() {
        let mut it = series();
        it.external_ids = ExternalIds::default();
        it.external_ids.fankai = Some(12);
        let own = own(&it);
        let fankai = vec![aired_order("fankai")];
        let mut by = asked_by_tmdb(&own, &fankai, TMDB_DEFAULT);
        by.client_id = 200_000_012;
        let show = from_item_by_tmdb(&it, &by);
        assert_eq!(show.metadata_source, "fankai");
        assert_eq!(show.tmdb_id, 200_000_012);
        assert_eq!(show.tvdb_id, Some(200_000_012));

        it.external_ids = ExternalIds::default();
        it.is_manual = true;
        let own_id = ids::own(&it.id);
        by.orders = &[];
        by.client_id = own_id;
        let show = from_item_by_tmdb(&it, &by);
        assert_eq!(show.metadata_source, "local");
        assert_eq!(show.tvdb_id, Some(own_id));
    }

    /// Skyhook's TMDB edition for *Breaking Bad*, `/v2/tmdb/shows/eng/1396/
    /// default`, as the real service answered it on 2026-11-01: reduced to
    /// three entries a list, its free texts cut to a few words.
    const REAL_TMDB_DEFAULT: &str = r#"{
  "title": "Breaking Bad",
  "originalTitle": "Breaking Bad",
  "tvdbId": 81189,
  "tmdbId": 1396,
  "tvRageId": 18164,
  "tvMazeId": 169,
  "imdbId": "tt0903747",
  "overview": "Walter White, a New…",
  "slug": "breaking-bad",
  "formats": ["scripted"],
  "originalCountry": "usa",
  "originalLanguage": "eng",
  "language": "eng",
  "firstAired": "2008-01-20",
  "lastAired": "2013-09-29",
  "malIds": [],
  "anidbIds": [],
  "aniListIds": [],
  "lastUpdated": "2026-11-01T11:26:20.3737133Z",
  "status": "Ended",
  "runtime": 48,
  "originalNetwork": "AMC",
  "network": "AMC",
  "genres": ["Crime", "Drama"],
  "contentRating": "16",
  "rating": {"count": 19012, "value": "9.0"},
  "translations": [
    {"language": "eng", "overview": "Walter White, a New…"},
    {"language": "deu", "overview": "Ein krebskranker Chemielehrer tut…"},
    {"language": "fra", "title": "Breaking Bad : Le…", "overview": "En phase terminale d'un…"}
  ],
  "seasonTypes": [
    {"name": "Default", "type": "default", "seasonNumbers": [0, 1, 2], "episodeCount": 62},
    {"name": "DVD / PVOD", "type": "69f50757054263b7bc87e32a", "seasonNumbers": [1, 2, 3], "episodeCount": 62}
  ],
  "actors": [
    {"name": "Bryan Cranston", "character": "Walter White", "image": "https://image.tmdb.org/t/p/original/7Jahy5LZX2Fo8fGJltMreAI49hC.jpg"},
    {"name": "Aaron Paul", "character": "Jesse Pinkman", "image": "https://image.tmdb.org/t/p/original/8Ac9uuoYwZoYVAIJfRLzzLsGGJn.jpg"},
    {"name": "Anna Gunn", "character": "Skyler White", "image": "https://image.tmdb.org/t/p/original/adppyeu1a4REN3khtgmXusrapFi.jpg"}
  ],
  "images": [
    {"coverType": "Poster", "url": "https://image.tmdb.org/t/p/original/ztkUQFLlC19CCMYHW9o1zWhJRNq.jpg"},
    {"coverType": "Fanart", "url": "https://image.tmdb.org/t/p/original/tsRy63Mu5cu8etL1X7ZLyf7UP1M.jpg"},
    {"coverType": "Clearlogo", "url": "https://image.tmdb.org/t/p/original/chw44B2VnLha8iiTdyZcIW0ZELC.png"}
  ],
  "seasons": [
    {"seasonNumber": 0, "year": 2009, "title": "Specials", "images": [{"coverType": "Poster", "url": "https://image.tmdb.org/t/p/original/40dT79mDEZwXkQiZNBgSaydQFDP.jpg"}]},
    {"seasonNumber": 1, "year": 2008, "images": [{"coverType": "Poster", "url": "https://image.tmdb.org/t/p/original/1BP4xYv9ZG4ZVHkL7ocOziBbSYH.jpg"}]},
    {"seasonNumber": 2, "year": 2009, "images": [{"coverType": "Poster", "url": "https://image.tmdb.org/t/p/original/e3oGYpoTUhOFK0BJfloru5ZmGV.jpg"}]}
  ],
  "episodes": [
    {"tmdbShowId": 1396, "tmdbId": 62131, "seasonNumber": 0, "episodeNumber": 1, "title": "Good Cop / Bad…", "airDate": "2009-02-17", "airDateUtc": "2009-02-18T02:00:00Z", "runtime": 3, "overview": "Season 1 Minisode 1:…", "image": "https://image.tmdb.org/t/p/original/t729tFVXPetnJlJ2VsUZQz0rX6v.jpg"},
    {"tmdbShowId": 1396, "tmdbId": 62133, "seasonNumber": 0, "episodeNumber": 2, "title": "Wedding Day", "airDate": "2009-02-17", "airDateUtc": "2009-02-18T02:00:00Z", "runtime": 5, "overview": "Season 1 Minisode 2:…", "image": "https://image.tmdb.org/t/p/original/wT62P6ZnjKgZXk0M5hHl4e5zSjB.jpg"},
    {"tmdbShowId": 1396, "tmdbId": 62132, "seasonNumber": 0, "episodeNumber": 3, "title": "TwaüghtHammër", "airDate": "2009-02-17", "airDateUtc": "2009-02-18T02:00:00Z", "runtime": 5, "overview": "Season 1 Minisode 3:…", "image": "https://image.tmdb.org/t/p/original/gUHAqFw3Ptzya96JFFE9xVfMjze.jpg"}
  ],
  "metadataSource": "tmdb"
}"#;

    /// What this edition writes besides, or leaves out, against the real
    /// service's first entry of a list:
    /// - a translation's title: a language is listed here only under the
    ///   name the series goes by in it, which Sonarr names it by; the real
    ///   service lists one with an overview alone, as its first;
    /// - a season's title: the real service names the specials, its first
    ///   season, and no other here; an order's seasons are named none here;
    /// - an episode's finale type: written for a finale, which the real
    ///   service's first episode, a special, is not.
    const TMDB_EDITION_ALLOWED: &[&str] = &[
        "translations[].title",
        "seasons[].title",
        "episodes[].finaleType",
    ];

    /// *Breaking Bad* as the catalogue holds it, TheTVDB numbering it, with
    /// TMDB's own numbering and its DVD group kept beside.
    fn breaking_bad() -> (MediaItem, Vec<EpisodeOrder>) {
        let picture = |kind: &str, url: &str, season: Option<i32>| -> Image {
            serde_json::from_value(serde_json::json!({
                "id": url, "seasonNumber": season, "coverType": kind, "url": url,
                "sortOrder": 0, "isManual": false
            }))
            .expect("an image")
        };
        let mut it = MediaItem::empty(MediaKind::Series);
        it.title = "Breaking Bad".into();
        it.original_title = Some("Breaking Bad".into());
        it.original_language = Some("en".into());
        it.original_country = Some("usa".into());
        it.overview = Some("A chemistry teacher.".into());
        it.slug = "breaking-bad".into();
        it.first_aired = Some("2008-01-20".into());
        it.last_aired = Some("2013-09-29".into());
        it.status = Some("ended".into());
        it.runtime = Some(48);
        it.network = Some("AMC".into());
        it.genres = vec!["Crime".into(), "Drama".into()];
        it.content_rating = Some("TV-MA".into());
        it.ratings = vec![crate::domain::Rating {
            source: "tmdb".into(),
            value: Some(8.9),
            votes: Some(19012),
            rating_type: Some("user".into()),
        }];
        it.external_ids = ExternalIds {
            tvdb: Some(81189),
            tmdb: Some(1396),
            imdb: Some("tt0903747".into()),
            tvrage: Some(18164),
            tvmaze: Some(169),
            ..Default::default()
        };
        it.translations = vec![translation(
            "fra",
            "Breaking Bad : Le chimiste",
            Some("En phase terminale."),
        )];
        it.credits = vec![
            serde_json::from_value(serde_json::json!({
                "id": "c1", "creditType": "actor", "personName": "Bryan Cranston",
                "characterName": "Walter White", "image": "https://x/bc.jpg",
                "sortOrder": 0, "isManual": false
            }))
            .expect("a credit"),
        ];
        it.images = vec![
            picture("poster", "https://x/poster.jpg", None),
            picture("fanart", "https://x/fanart.jpg", None),
        ];
        it.seasons = (0..=2)
            .map(|number| {
                let mut s = season(number, None, None);
                s.images = vec![picture(
                    "poster",
                    &format!("https://x/s{number}.jpg"),
                    Some(number),
                )];
                s
            })
            .collect();
        it.episodes = [
            (0, 1, 1_000_001, 62131, "2009-02-17"),
            (1, 1, 349232, 62085, "2008-01-20"),
            (1, 2, 349235, 62086, "2008-01-27"),
            (2, 1, 438906, 62094, "2009-03-08"),
        ]
        .into_iter()
        .map(|(season, number, tvdb_id, tmdb_id, aired)| {
            let mut e = episode(season, number, tvdb_id, aired);
            e.tmdb_id = Some(tmdb_id);
            e.runtime = Some(47);
            e.overview = Some("What happens.".into());
            e.image = Some(format!("https://x/{tvdb_id}.jpg"));
            e
        })
        .collect();
        it.episodes[2].finale_type = Some("season".into());

        let orders = vec![
            aired_order("tvdb"),
            tmdb_aired(vec![
                tmdb_place(62131, Some(1_000_001), 0, 1),
                tmdb_place(62085, Some(349232), 1, 1),
                tmdb_place(62086, Some(349235), 1, 2),
                tmdb_place(62094, Some(438906), 2, 1),
            ]),
            EpisodeOrder {
                kind: OrderKind::Dvd,
                source: "tmdb".into(),
                source_ref: Some("69f50757054263b7bc87e32a".into()),
                ..order(
                    "tmdb-69f50757054263b7bc87e32a",
                    "DVD / PVOD",
                    vec![tmdb_place(62085, Some(349232), 1, 1)],
                )
            },
        ];
        (it, orders)
    }

    #[test]
    fn the_tmdb_edition_is_shaped_as_the_real_services() {
        let real: serde_json::Value = serde_json::from_str(REAL_TMDB_DEFAULT).expect("JSON");
        let (it, orders) = breaking_bad();
        let own = OwnText::of(&it, "eng");
        let asked = Asked {
            client_id: 1396,
            language: "eng",
            season_type: TMDB_DEFAULT,
            own: &own,
            orders: &orders,
            with_episodes: true,
        };
        let ours = serde_json::to_value(from_item_by_tmdb(&it, &asked)).unwrap();
        crate::wire::contract::assert_same_shape(&real, &ours, TMDB_EDITION_ALLOWED);
        assert_eq!(ours["tvdbId"], 81189);
        assert_eq!(ours["tmdbId"], 1396);
        assert_eq!(ours["seasonTypes"][1]["type"], "69f50757054263b7bc87e32a");

        // A search's result, as the real service answers `tmdb:1396` — the
        // same document, it checked the same day, without the episodes and
        // the runtime they are counted from.
        let mut found = real;
        let fields = found.as_object_mut().expect("an object");
        fields.remove("episodes");
        fields.remove("runtime");
        let searched = Asked {
            with_episodes: false,
            ..asked
        };
        let ours = serde_json::to_value(vec![from_item_by_tmdb(&it, &searched)]).unwrap();
        let in_a_list: Vec<String> = TMDB_EDITION_ALLOWED
            .iter()
            .map(|path| format!("[].{path}"))
            .collect();
        crate::wire::contract::assert_same_shape(
            &serde_json::json!([found]),
            &ours,
            &in_a_list.iter().map(String::as_str).collect::<Vec<_>>(),
        );
    }
}
