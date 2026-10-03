//! Sonarr's Skyhook format.
//!
//! Field names and types follow
//! `NzbDrone.Core/MetadataSource/SkyHook/Resource/ShowResource.cs` in the Sonarr
//! source. Sonarr deserializes with a case-insensitive JSON reader, but it emits
//! camelCase, so that is what we emit too.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Ceilings on what one upstream answer may turn into.
///
/// Every element here becomes a database row and a line in the cached document,
/// and the upstream is a URL an operator can point elsewhere — the README's own
/// deployment pattern is to redirect `skyhook.sonarr.tv` at a mirror. A mirror
/// answering with two million episodes should cost a truncated series, not a
/// transaction that inserts two million rows. Both are far above anything real:
/// the longest thing television has produced is in the low thousands.
const MAX_EPISODES: usize = 10_000;
const MAX_IMAGES: usize = 200;

use super::{readable_entries, readable_or_none, string_or_null};
use crate::{
    db::{new_id, now},
    domain::{
        AlternativeTitle, CoverType, Credit, CreditType, Episode, ExternalIds, Image, MediaItem,
        MediaKind, Rating, RatingValue, Season, make_slug,
    },
};

/// A series as Skyhook describes one, and as this server answers Sonarr.
///
/// Read leniently, written in full. Skyhook leaves out whatever it has nothing
/// for — an episode nobody has named has no `title` — and a field read as
/// required that one answer lacked failed the whole series: everything Skyhook
/// adds to the other providers was lost with it. So a text it leaves out is
/// read as empty, a list as none, an entry or a value that cannot be read is
/// left out; only the series' own id is required. What this server sends
/// Sonarr still carries every field Sonarr reads without a null check.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShowResource {
    pub tvdb_id: i64,
    /// Empty when Skyhook names nothing: another provider names the work then.
    #[serde(default, deserialize_with = "string_or_null")]
    #[schema(required = true)]
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(default, deserialize_with = "string_or_null")]
    #[schema(required = true)]
    pub slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
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
    #[serde(default, deserialize_with = "readable_entries")]
    pub mal_ids: Vec<i64>,
    #[serde(default, deserialize_with = "readable_entries")]
    pub ani_list_ids: Vec<i64>,
    #[serde(default, deserialize_with = "string_or_null")]
    #[schema(required = true)]
    pub last_updated: String,
    /// Empty when Skyhook says nothing: no status, rather than one made up.
    #[serde(default, deserialize_with = "string_or_null")]
    #[schema(required = true)]
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<i32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "readable_or_none"
    )]
    pub time_of_day: Option<TimeOfDay>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_network: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(default, deserialize_with = "readable_entries")]
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_rating: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "readable_or_none"
    )]
    pub rating: Option<RatingResource>,
    #[serde(default, deserialize_with = "readable_entries")]
    pub alternative_titles: Vec<AlternativeTitleResource>,
    #[serde(default, deserialize_with = "readable_entries")]
    pub actors: Vec<ActorResource>,
    #[serde(default, deserialize_with = "readable_entries")]
    pub images: Vec<ImageResource>,
    #[serde(default, deserialize_with = "readable_entries")]
    pub seasons: Vec<SeasonResource>,
    #[serde(default, deserialize_with = "readable_entries")]
    pub episodes: Vec<EpisodeResource>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, ToSchema)]
pub struct TimeOfDay {
    pub hours: i32,
    pub minutes: i32,
}

/// Sonarr reads `value` as a string and parses it itself.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct RatingResource {
    pub count: i64,
    pub value: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AlternativeTitleResource {
    pub title: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ActorResource {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImageResource {
    pub cover_type: String,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SeasonResource {
    pub season_number: i32,
    #[serde(default, deserialize_with = "readable_entries")]
    pub images: Vec<ImageResource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeResource {
    /// Always written; never read, so not needed from the upstream either.
    #[serde(default)]
    #[schema(required = true)]
    pub tvdb_show_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
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
    /// Always written, `TBA` for an episode nobody has named yet (see
    /// `episode_title`). Skyhook leaves it out then, and that is read as no
    /// name: the episode, and the series, are kept.
    #[serde(default, deserialize_with = "string_or_null")]
    #[schema(required = true)]
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_date_utc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finale_type: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "readable_or_none"
    )]
    pub rating: Option<RatingResource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

// ─── canonical → wire ────────────────────────────────────────────────────────

/// Render a work as Sonarr expects it.
///
/// `tvdb_id` is the identifier the client asked for. It is passed in rather than
/// read off the item because a TMDB-only work is addressed by a synthesised id
/// (see `api::compat::synthetic`), and the response must echo back whatever
/// Sonarr will store.
pub fn from_item(item: &MediaItem, tvdb_id: i64, language: &str) -> ShowResource {
    ShowResource {
        tvdb_id,
        title: item.title.clone(),
        overview: item.overview.clone(),
        slug: item.slug.clone(),
        original_country: item.original_country.clone(),
        original_language: item.original_language.clone(),
        language: Some(language.to_string()),
        first_aired: sonarr_day(item.first_aired.as_deref()),
        last_aired: sonarr_day(item.last_aired.as_deref()),
        tv_rage_id: item.external_ids.tvrage,
        tv_maze_id: item.external_ids.tvmaze,
        tmdb_id: item.external_ids.tmdb,
        imdb_id: item.external_ids.imdb.clone(),
        mal_ids: item.external_ids.mal.clone(),
        ani_list_ids: item.external_ids.anilist.clone(),
        last_updated: item.updated_at.clone(),
        status: sonarr_status(item.status.as_deref()),
        runtime: item.runtime,
        time_of_day: item.air_time.as_deref().and_then(parse_time_of_day),
        original_network: item.network.clone(),
        network: item.network.clone(),
        genres: item.genres.clone(),
        content_rating: item.content_rating.clone(),
        rating: item.headline_rating().map(rating_resource),
        alternative_titles: item
            .alternative_titles
            .iter()
            .map(|t| AlternativeTitleResource {
                title: t.title.clone(),
            })
            .collect(),
        actors: item
            .credits
            .iter()
            .filter(|c| c.credit_type == CreditType::Actor)
            .map(|c| ActorResource {
                name: c.person_name.clone(),
                character: c.character_name.clone(),
                image: c.image.clone(),
            })
            .collect(),
        // One a kind: Sonarr writes every image of a kind to one file.
        images: crate::domain::lead_images(
            item.images.iter().filter(|i| i.season_number.is_none()),
            &item.primary_images,
        )
        .into_iter()
        .map(image_resource)
        .collect(),
        seasons: item
            .seasons
            .iter()
            .map(|s| SeasonResource {
                season_number: s.season_number,
                images: crate::domain::lead_images(&s.images, &Default::default())
                    .into_iter()
                    .map(image_resource)
                    .collect(),
            })
            .collect(),
        episodes: item
            .episodes
            .iter()
            .map(|e| episode_resource(e, tvdb_id))
            .collect(),
    }
}

fn image_resource(image: &Image) -> ImageResource {
    ImageResource {
        // Sonarr compares these case-insensitively but displays them as given.
        cover_type: capitalize(image.cover_type.as_str()),
        url: image.url.clone(),
    }
}

fn episode_resource(episode: &Episode, tvdb_show_id: i64) -> EpisodeResource {
    EpisodeResource {
        tvdb_show_id,
        tvdb_id: episode.tvdb_id,
        season_number: episode.season_number,
        episode_number: episode.episode_number,
        absolute_episode_number: episode.absolute_episode_number,
        aired_after_season_number: episode.aired_after_season_number,
        aired_before_season_number: episode.aired_before_season_number,
        aired_before_episode_number: episode.aired_before_episode_number,
        title: episode_title(&episode.title),
        air_date: sonarr_day(episode.air_date.as_deref()),
        // The real moment when a provider knew it (Skyhook, TVmaze); midnight
        // UTC of the broadcast date otherwise, which is what Sonarr needs to
        // consider an episode aired at all — without it, nothing is searched.
        air_date_utc: episode.air_date_utc.clone().or_else(|| {
            episode
                .air_date
                .as_deref()
                .and_then(crate::domain::midnight_utc)
        }),
        runtime: episode.runtime,
        finale_type: episode.finale_type.clone(),
        rating: episode.rating.map(|r| RatingResource {
            count: r.votes,
            value: format!("{:.1}", r.value),
        }),
        overview: episode.overview.clone(),
        image: episode.image.clone(),
    }
}

/// What Skyhook calls an episode nobody has named yet.
pub const UNTITLED: &str = "TBA";

/// An episode's title as Skyhook sends it: `TBA` when there is none.
///
/// Sonarr turns a missing title into `TBA` itself, but keeps an empty one as
/// it is, and lists the episode as a row with nothing in it to click.
fn episode_title(title: &str) -> String {
    if title.trim().is_empty() {
        UNTITLED.to_string()
    } else {
        title.to_string()
    }
}

/// A day as Sonarr reads one: `yyyy-MM-dd`, exactly.
///
/// Sonarr parses a series' first and last air dates with
/// `DateTime.ParseExact(value, "yyyy-MM-dd")` (`SkyHookProxy.MapSeries`), with
/// nothing around it to catch a failure: the date-time a date field here also
/// accepts failed the series' refresh there, and every search that found it.
/// An episode's air date is matched as text against the day a daily release
/// names, and parsed the same strict way when a daily series is searched.
///
/// A date-time gives the day it names in its own zone, the local broadcast
/// date, which is what Skyhook sends. What is not a day at all is left out.
fn sonarr_day(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    let day = match chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        Ok(day) => day,
        Err(_) => chrono::DateTime::parse_from_rfc3339(value)
            .ok()?
            .date_naive(),
    };

    Some(day.format("%Y-%m-%d").to_string())
}

fn rating_resource(rating: &Rating) -> RatingResource {
    RatingResource {
        count: rating.votes.unwrap_or(0),
        value: format!("{:.1}", rating.value.unwrap_or(0.0)),
    }
}

/// Sonarr's `SeriesStatusType` parses these names case-insensitively.
fn sonarr_status(status: Option<&str>) -> String {
    match status.unwrap_or("continuing").to_ascii_lowercase().as_str() {
        "ended" => "Ended",
        "upcoming" => "Upcoming",
        "deleted" => "Deleted",
        _ => "Continuing",
    }
    .to_string()
}

fn parse_time_of_day(air_time: &str) -> Option<TimeOfDay> {
    let (h, m) = air_time.trim().split_once(':')?;
    Some(TimeOfDay {
        hours: h.trim().parse().ok()?,
        minutes: m.trim().parse().ok()?,
    })
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

// ─── wire → canonical ────────────────────────────────────────────────────────

/// Absorb a response from the real Skyhook into the canonical model, so a
/// fallback answer can be cached and later edited like any other entry.
pub fn to_item(show: &ShowResource) -> MediaItem {
    let mut item = MediaItem::empty(MediaKind::Series);

    item.title = show.title.clone();
    item.overview = show.overview.clone();
    item.original_country = show.original_country.clone();
    item.original_language = show.original_language.clone();
    item.first_aired = show.first_aired.clone();
    item.last_aired = show.last_aired.clone();
    item.year = show
        .first_aired
        .as_deref()
        .and_then(|d| d.get(0..4)?.parse().ok());
    item.status = Some(show.status.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase);
    item.runtime = show.runtime;
    item.network = show
        .network
        .clone()
        .or_else(|| show.original_network.clone());
    item.content_rating = show.content_rating.clone();
    // Skyhook sends a bare rating with no country; Sonarr never uses one.
    item.content_rating_country = None;
    item.genres = show.genres.clone();
    item.air_time = show
        .time_of_day
        .map(|t| format!("{:02}:{:02}", t.hours, t.minutes));
    // Skyhook's slug is authoritative for a TVDB-sourced show; keep it so the
    // entry stays addressable by the same name it had upstream.
    item.slug = if show.slug.is_empty() {
        make_slug(&show.title, item.year)
    } else {
        show.slug.clone()
    };

    item.external_ids = ExternalIds {
        tvdb: Some(show.tvdb_id),
        tmdb: show.tmdb_id,
        imdb: show
            .imdb_id
            .as_deref()
            .and_then(crate::domain::ids::normalize_imdb_id),
        tvmaze: show.tv_maze_id,
        tvrage: show.tv_rage_id,
        mal: show.mal_ids.clone(),
        anilist: show.ani_list_ids.clone(),
        trakt: None,
        fankai: None,
    };

    // IMDb's rating, republished: Breaking Bad's is 9.5 from 2 679 821 votes
    // here and 9.5 from 2 679 470 in IMDb's own dataset a day older. Filed
    // under the source it comes from, so the IMDb list can keep it current.
    if let Some(rating) = &show.rating
        && rating.count > 0
    {
        item.ratings = vec![Rating {
            source: "imdb".to_string(),
            value: rating.value.parse().ok(),
            votes: Some(rating.count),
            rating_type: Some("user".to_string()),
        }];
    }

    item.images = show
        .images
        .iter()
        .take(MAX_IMAGES)
        .enumerate()
        .map(|(i, img)| to_image(img, None, i as i32))
        .collect();

    item.credits = show
        .actors
        .iter()
        .enumerate()
        .map(|(i, actor)| Credit {
            id: new_id(),
            credit_type: CreditType::Actor,
            person_name: actor.name.clone(),
            character_name: actor.character.clone(),
            image: actor.image.clone(),
            tmdb_person_id: None,
            // Skyhook carries no credit identifier.
            credit_tmdb_id: None,
            sort_order: i as i32,
            is_manual: false,
        })
        .collect();

    item.alternative_titles = show
        .alternative_titles
        .iter()
        .map(|t| AlternativeTitle {
            id: new_id(),
            title: t.title.clone(),
            title_type: None,
            language: None,
            is_manual: false,
        })
        .collect();

    item.seasons = show
        .seasons
        .iter()
        .map(|s| Season {
            id: new_id(),
            season_number: s.season_number,
            title: None,
            overview: None,
            air_date: None,
            tmdb_id: None,
            tvdb_id: None,
            is_manual: false,
            images: s
                .images
                .iter()
                .take(MAX_IMAGES)
                .enumerate()
                .map(|(i, img)| to_image(img, Some(s.season_number), i as i32))
                .collect(),
        })
        .collect();

    item.episodes = show
        .episodes
        .iter()
        .take(MAX_EPISODES)
        .map(|e| Episode {
            id: new_id(),
            season_number: e.season_number,
            episode_number: e.episode_number,
            absolute_episode_number: e.absolute_episode_number,
            aired_after_season_number: e.aired_after_season_number,
            aired_before_season_number: e.aired_before_season_number,
            aired_before_episode_number: e.aired_before_episode_number,
            title: e.title.clone(),
            overview: e.overview.clone(),
            air_date: e.air_date.clone(),
            air_date_utc: e.air_date_utc.clone(),
            runtime: e.runtime,
            finale_type: e.finale_type.clone(),
            image: e.image.clone(),
            tvdb_id: e.tvdb_id,
            tmdb_id: None,
            rating: e.rating.as_ref().and_then(|r| {
                (r.count > 0).then(|| RatingValue {
                    value: r.value.parse().unwrap_or(0.0),
                    votes: r.count,
                })
            }),
            is_manual: false,
        })
        .collect();

    item.updated_at = now();
    item
}

fn to_image(resource: &ImageResource, season_number: Option<i32>, sort_order: i32) -> Image {
    Image {
        id: new_id(),
        season_number,
        cover_type: resource.cover_type.parse().unwrap_or(CoverType::Unknown),
        url: resource.url.clone(),
        language: None,
        sort_order,
        source: Some(crate::providers::names::SKYHOOK.to_string()),
        is_manual: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> MediaItem {
        let mut it = MediaItem::empty(MediaKind::Series);
        it.title = "Breaking Bad".into();
        it.slug = "breaking-bad-2008".into();
        it.status = Some("ended".into());
        it.air_time = Some("21:00".into());
        it.external_ids = ExternalIds {
            tvdb: Some(81189),
            tmdb: Some(1396),
            imdb: Some("tt0903747".into()),
            ..Default::default()
        };
        it.ratings = vec![Rating {
            source: "tmdb".into(),
            value: Some(8.88),
            votes: Some(1234),
            rating_type: Some("user".into()),
        }];
        it
    }

    #[test]
    fn statuses_use_sonarrs_names() {
        assert_eq!(sonarr_status(Some("ended")), "Ended");
        assert_eq!(sonarr_status(Some("continuing")), "Continuing");
        assert_eq!(sonarr_status(Some("upcoming")), "Upcoming");
        assert_eq!(sonarr_status(None), "Continuing");
        assert_eq!(sonarr_status(Some("nonsense")), "Continuing");
    }

    #[test]
    fn the_requested_id_is_echoed_back() {
        // Sonarr stores whatever id it asked for; the response must agree.
        let resource = from_item(&item(), 100_001_396, "en");
        assert_eq!(resource.tvdb_id, 100_001_396);
        assert_eq!(resource.tmdb_id, Some(1396));
    }

    #[test]
    fn ratings_are_serialised_as_sonarr_expects() {
        let resource = from_item(&item(), 81189, "en");
        let rating = resource.rating.unwrap();
        assert_eq!(rating.count, 1234);
        assert_eq!(rating.value, "8.9");
    }

    #[test]
    fn an_episode_reaches_sonarr_with_the_real_moment_or_the_date_at_midnight() {
        // Sonarr reads `airDateUtc` as the moment an episode aired and does not
        // search before it. The real one when a provider knew it; midnight UTC
        // of the broadcast date when none did — never nothing, or the episode
        // would never be considered aired at all.
        let mut item = item();
        item.episodes = vec![crate::db::repo::child::blank_episode(1, 1)];
        item.episodes[0].air_date = Some("2008-01-20".into());

        item.episodes[0].air_date_utc = Some("2008-01-21T02:00:00Z".into());
        let known = from_item(&item, 81189, "en");
        assert_eq!(
            known.episodes[0].air_date_utc.as_deref(),
            Some("2008-01-21T02:00:00Z")
        );

        item.episodes[0].air_date_utc = None;
        let dated = from_item(&item, 81189, "en");
        assert_eq!(
            dated.episodes[0].air_date_utc.as_deref(),
            Some("2008-01-20T00:00:00Z")
        );
    }

    #[test]
    fn air_time_becomes_a_time_of_day() {
        let resource = from_item(&item(), 81189, "en");
        let tod = resource.time_of_day.unwrap();
        assert_eq!((tod.hours, tod.minutes), (21, 0));
    }

    #[test]
    fn a_malformed_air_time_is_dropped_not_fatal() {
        let mut it = item();
        it.air_time = Some("evening".into());
        assert!(from_item(&it, 1, "en").time_of_day.is_none());
    }

    #[test]
    fn a_skyhook_response_round_trips_through_the_canonical_model() {
        let original = from_item(&item(), 81189, "en");
        let canonical = to_item(&original);

        assert_eq!(canonical.title, "Breaking Bad");
        assert_eq!(canonical.slug, "breaking-bad-2008");
        assert_eq!(canonical.status.as_deref(), Some("ended"));
        assert_eq!(canonical.external_ids.tvdb, Some(81189));
        assert_eq!(canonical.external_ids.imdb.as_deref(), Some("tt0903747"));
        assert_eq!(canonical.air_time.as_deref(), Some("21:00"));

        let again = from_item(&canonical, 81189, "en");
        assert_eq!(again.status, original.status);
        assert_eq!(again.title, original.title);
    }

    fn rating(source: &str, value: f64, votes: i64) -> Rating {
        Rating {
            source: source.into(),
            value: Some(value),
            votes: Some(votes),
            rating_type: Some("user".into()),
        }
    }

    #[test]
    fn sonarr_is_given_imdbs_rating_when_there_is_one() {
        // What Skyhook gives it, so what it has always shown — even where
        // another source has more votes behind it.
        let mut it = item();
        it.ratings = vec![
            rating("tmdb", 8.7, 7_706),
            rating("mal", 8.57, 3_089_461),
            rating("imdb", 9.1, 748_283),
        ];

        let resource = from_item(&it, 267440, "en").rating.unwrap();
        assert_eq!((resource.value.as_str(), resource.count), ("9.1", 748_283));
    }

    #[test]
    fn otherwise_sonarr_is_given_the_rating_with_the_most_votes() {
        let mut it = item();
        it.ratings = vec![
            rating("tmdb", 8.7, 7_706),
            rating("anilist", 8.5, 609_257),
            // A value of nothing is not a rating, however many voted.
            Rating {
                source: "trakt".into(),
                value: None,
                votes: Some(9_000_000),
                rating_type: None,
            },
        ];

        let resource = from_item(&it, 267440, "en").rating.unwrap();
        assert_eq!((resource.value.as_str(), resource.count), ("8.5", 609_257));
    }

    #[test]
    fn a_figure_that_is_not_a_mark_out_of_ten_is_never_the_rating() {
        // TheTVDB's popularity figure, stored as a rating before this server
        // stopped filing it as one.
        let mut it = item();
        it.ratings = vec![Rating {
            source: "tvdb".into(),
            value: Some(3_776_757.0),
            votes: None,
            rating_type: Some("user".into()),
        }];

        assert!(from_item(&it, 81189, "en").rating.is_none());
    }

    #[test]
    fn of_ratings_tied_on_votes_the_first_listed_leads() {
        let mut it = item();
        it.ratings = vec![rating("tmdb", 7.4, 0), rating("mal", 8.1, 0)];

        let resource = from_item(&it, 81189, "en").rating.unwrap();
        assert_eq!(resource.value, "7.4");
    }

    #[test]
    fn skyhooks_rating_is_filed_as_the_imdb_rating_it_is() {
        let mut it = item();
        it.ratings = vec![rating("imdb", 9.5, 2_679_821)];

        let canonical = to_item(&from_item(&it, 81189, "en"));

        assert_eq!(canonical.ratings.len(), 1);
        assert_eq!(canonical.ratings[0].source, "imdb");
        assert_eq!(canonical.ratings[0].votes, Some(2_679_821));
    }

    /// Sonarr writes every image of a kind to one file: it is given one a
    /// kind, the chosen one when there is one, and no season's in the show's.
    #[test]
    fn sonarr_is_given_one_image_a_kind_the_chosen_first() {
        let mut it = item();
        let image = |id: &str, kind: CoverType, season: Option<i32>| Image {
            id: id.into(),
            season_number: season,
            cover_type: kind,
            url: format!("https://x/{id}.jpg"),
            language: None,
            sort_order: 0,
            source: None,
            is_manual: false,
        };
        it.images = vec![
            image("p1", CoverType::Poster, None),
            image("p2", CoverType::Poster, None),
            image("s1", CoverType::Poster, Some(1)),
            image("f1", CoverType::Fanart, None),
        ];
        it.primary_images.poster = Some("p2".into());
        let sent = from_item(&it, 1, "en").images;
        let urls: Vec<(&str, &str)> = sent
            .iter()
            .map(|i| (i.cover_type.as_str(), i.url.as_str()))
            .collect();
        assert_eq!(
            urls,
            [
                ("Poster", "https://x/p2.jpg"),
                ("Fanart", "https://x/f1.jpg")
            ]
        );
    }

    #[test]
    fn cover_types_are_capitalised_for_sonarr() {
        let mut it = item();
        it.images = vec![Image {
            id: "x".into(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: "https://example.invalid/p.jpg".into(),
            language: None,
            sort_order: 0,
            source: None,
            is_manual: false,
        }];

        assert_eq!(from_item(&it, 1, "en").images[0].cover_type, "Poster");
    }

    #[test]
    fn an_episode_nobody_has_named_is_tba_as_on_skyhook() {
        let mut it = item();
        it.episodes = vec![
            crate::db::repo::child::blank_episode(1, 1),
            crate::db::repo::child::blank_episode(1, 2),
            crate::db::repo::child::blank_episode(1, 3),
        ];
        it.episodes[1].title = "  ".into();
        it.episodes[2].title = "Pilot".into();

        let titles: Vec<String> = from_item(&it, 81189, "en")
            .episodes
            .into_iter()
            .map(|e| e.title)
            .collect();
        assert_eq!(titles, ["TBA", "TBA", "Pilot"]);
    }

    #[test]
    fn dates_reach_sonarr_as_days() {
        // Sonarr parses a series' air dates with ParseExact("yyyy-MM-dd") and
        // nothing around it: a date-time, which a date field here accepts,
        // failed the series' refresh there, and every search that found it.
        let mut it = item();
        it.first_aired = Some("2008-01-20T21:00:00-05:00".into());
        it.last_aired = Some("2013-09-29".into());
        it.episodes = vec![crate::db::repo::child::blank_episode(1, 1)];
        it.episodes[0].air_date = Some("2008-01-20T21:00:00-05:00".into());

        let show = from_item(&it, 81189, "en");
        // The day it names in its own zone, as Skyhook sends: not the UTC one.
        assert_eq!(show.first_aired.as_deref(), Some("2008-01-20"));
        assert_eq!(show.last_aired.as_deref(), Some("2013-09-29"));
        assert_eq!(show.episodes[0].air_date.as_deref(), Some("2008-01-20"));
        // The moment itself still goes where Sonarr reads one.
        assert_eq!(
            show.episodes[0].air_date_utc.as_deref(),
            Some("2008-01-20T21:00:00-05:00")
        );

        // What is no day at all is left out rather than sent.
        it.first_aired = Some("2008".into());
        assert_eq!(from_item(&it, 81189, "en").first_aired, None);
    }

    /// What Skyhook answered for *Magical Explorer* on the evening it
    /// premiered, cut down to two episodes: from the third on, nobody had
    /// named them yet, and Skyhook leaves `title` out rather than sending one.
    fn magical_explorer() -> serde_json::Value {
        serde_json::json!({
            "tvdbId": 440056,
            "title": "Magical Explorer",
            "slug": "magical-explorer",
            "originalCountry": "jpn",
            "originalLanguage": "jpn",
            "language": "eng",
            "firstAired": "2026-10-04",
            "lastAired": "2026-12-27",
            "tvMazeId": 93848,
            "tmdbId": 335194,
            "imdbId": "tt41296506",
            "malIds": [56733],
            "anidbIds": [18208],
            "aniListIds": [169581],
            "lastUpdated": "2026-10-03T17:48:39Z",
            "status": "Continuing",
            "runtime": 24,
            "timeOfDay": { "hours": 0, "minutes": 0 },
            "originalNetwork": "Tokyo MX",
            "network": "Tokyo MX",
            "genres": ["Action", "Anime"],
            "seasons": [{ "seasonNumber": 1 }],
            "episodes": [
                {
                    "tvdbShowId": 440056,
                    "tvdbId": 10050355,
                    "seasonNumber": 1,
                    "episodeNumber": 1,
                    "absoluteEpisodeNumber": 1,
                    "title": "Reincarnated as a Sidekick Character in an Eroge, but I’ll Use My Game Knowledge to Do What I Want",
                    "airDate": "2026-10-04",
                    "airDateUtc": "2026-10-03T15:00:00Z",
                    "runtime": 24
                },
                {
                    "tvdbShowId": 440056,
                    "tvdbId": 12017080,
                    "seasonNumber": 1,
                    "episodeNumber": 3,
                    "absoluteEpisodeNumber": 3,
                    "airDate": "2026-10-18",
                    "airDateUtc": "2026-10-17T15:00:00Z",
                    "runtime": 24
                }
            ]
        })
    }

    #[test]
    fn an_episode_skyhook_has_not_named_costs_nothing_of_the_series() {
        // Read as required, the missing title failed the whole answer, and
        // with it what only Skyhook gave: the instant the first episode aired
        // — midnight UTC of its Japanese day went to Sonarr instead, nine
        // hours late — and the ids it files the series under elsewhere.
        let show: ShowResource =
            serde_json::from_value(magical_explorer()).expect("Skyhook's answer is read");
        let item = to_item(&show);

        assert_eq!(item.episodes.len(), 2);
        assert_eq!(
            item.episodes[0].air_date_utc.as_deref(),
            Some("2026-10-03T15:00:00Z")
        );
        assert_eq!(item.episodes[1].title, "");
        assert_eq!(item.external_ids.mal, [56733]);
        assert_eq!(item.external_ids.anilist, [169581]);
        assert_eq!(item.external_ids.tvmaze, Some(93848));
        assert_eq!(item.status.as_deref(), Some("continuing"));

        // Sonarr is still sent a name for it, as Skyhook's own answer reads.
        let sent = serde_json::to_value(from_item(&item, 440056, "en")).unwrap();
        assert_eq!(sent["episodes"][1]["title"], "TBA");
        assert_eq!(sent["episodes"][0]["airDateUtc"], "2026-10-03T15:00:00Z");
    }

    #[test]
    fn what_the_upstream_leaves_out_or_sends_as_null_is_read_as_nothing() {
        // Skyhook leaves out what it has nothing for; a mirror an operator
        // points this server at may write null instead. Neither costs the
        // series: whatever else the answer says is kept.
        let show: ShowResource = serde_json::from_value(serde_json::json!({
            "tvdbId": 440056,
            "title": null,
            "status": null,
            "genres": null,
            "malIds": null,
            "rating": null,
            "timeOfDay": null,
            "seasons": [{ "seasonNumber": 1, "images": null }],
            "episodes": [{
                "tvdbShowId": 440056,
                "seasonNumber": 1,
                "episodeNumber": 1,
                "title": null,
                "airDateUtc": "2026-10-03T15:00:00Z",
                "rating": null
            }]
        }))
        .expect("the answer is read");
        let item = to_item(&show);

        // No title and no status: another provider gives them, and a work
        // nobody names is not stored.
        assert_eq!(item.title, "");
        assert_eq!(item.status, None);
        assert!(item.genres.is_empty() && item.external_ids.mal.is_empty());
        assert_eq!(item.seasons.len(), 1);
        assert_eq!(
            item.episodes[0].air_date_utc.as_deref(),
            Some("2026-10-03T15:00:00Z")
        );
    }

    #[test]
    fn an_entry_that_cannot_be_read_is_left_out_not_the_series() {
        // An episode with no number to file it under, an actor with no name,
        // a rating with no value: each is left out, and nothing else.
        let mut answer = magical_explorer();
        answer["episodes"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "tvdbShowId": 440056, "seasonNumber": 1, "title": "?" }));
        answer["actors"] = serde_json::json!([
            { "character": "瀧音幸助 / Kosuke Taktoto" },
            { "name": "Nobunaga Shimazaki" }
        ]);
        answer["rating"] = serde_json::json!({ "count": 12 });

        let item = to_item(&serde_json::from_value(answer).expect("the answer is read"));

        assert_eq!(item.episodes.len(), 2);
        assert_eq!(item.credits.len(), 1);
        assert_eq!(item.credits[0].person_name, "Nobunaga Shimazaki");
        assert!(item.ratings.is_empty());
        assert_eq!(item.external_ids.anilist, [169581]);
    }
}
