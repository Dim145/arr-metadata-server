//! Radarr's metadata format.
//!
//! Field names follow `NzbDrone.Core/MetadataSource/SkyHook/Resource/*.cs` in
//! the Radarr source. Radarr's own `MovieResource` is what `api.radarr.video`
//! returns, so matching it exactly is what makes the substitution invisible.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::domain::{Credit, CreditType, Image, MediaItem, Rating};

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MovieResource {
    pub tmdb_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    pub title_slug: String,
    /// Deprecated upstream but still read by older Radarr versions.
    #[serde(default)]
    pub ratings: Vec<RatingItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movie_ratings: Option<RatingResource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<i32>,
    #[serde(default)]
    pub images: Vec<ImageResource>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub year: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub premier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_cinema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical_release: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digital_release: Option<String>,
    #[serde(default)]
    pub alternative_titles: Vec<AlternativeTitleResource>,
    #[serde(default)]
    pub translations: Vec<TranslationResource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<Credits>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub studio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub youtube_trailer_id: Option<String>,
    #[serde(default)]
    pub certifications: Vec<CertificationResource>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<CollectionResource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default)]
    pub recommendations: Vec<RecommendationResource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub popularity: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RatingResource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb: Option<RatingItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb: Option<RatingItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metacritic: Option<RatingItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotten_tomatoes: Option<RatingItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trakt: Option<RatingItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RatingItem {
    pub count: i64,
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub rating_type: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImageResource {
    pub cover_type: String,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlternativeTitleResource {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub title_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TranslationResource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    pub language: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CertificationResource {
    pub country: String,
    pub certification: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Credits {
    #[serde(default)]
    pub cast: Vec<CastResource>,
    #[serde(default)]
    pub crew: Vec<CrewResource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CastResource {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    /// Radarr stores this with a NOT NULL constraint.
    pub credit_id: String,
    #[serde(default)]
    pub images: Vec<ImageResource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CrewResource {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub department: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    /// Radarr stores this with a NOT NULL constraint.
    pub credit_id: String,
    #[serde(default)]
    pub images: Vec<ImageResource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CollectionResource {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    pub tmdb_id: i64,
    #[serde(default)]
    pub images: Vec<ImageResource>,
    /// A movie may belong to a collection, and a collection holds movies. That
    /// loop makes schema collection recurse forever unless it is cut here.
    #[serde(default)]
    #[schema(no_recursion)]
    pub parts: Vec<MovieResource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecommendationResource {
    pub tmdb_id: i64,
    pub title: String,
}

// ─── canonical → wire ────────────────────────────────────────────────────────

pub fn from_item(item: &MediaItem) -> MovieResource {
    // Radarr dereferences `resource.Ratings.FirstOrDefault()` without a null
    // check when `movieRatings.tmdb` is absent (SkyHookProxy.cs:285), so an
    // unrated title takes down the whole search response rather than just
    // itself. Always emit a TMDB entry here — zeroed when there are no votes.
    //
    // The canonical model still records no rating for an unrated title; this is
    // a concession to one client's parser, made where that client is served.
    let tmdb_rating = item.rating("tmdb").map(rating_item).unwrap_or(RatingItem {
        count: 0,
        value: 0.0,
        origin: Some("tmdb".to_string()),
        rating_type: Some("user".to_string()),
    });
    let imdb_rating = item.rating("imdb").map(rating_item);

    MovieResource {
        tmdb_id: item.external_ids.tmdb.unwrap_or(0),
        imdb_id: item.external_ids.imdb.clone(),
        overview: item.overview.clone(),
        title: item.title.clone(),
        original_title: item.original_title.clone(),
        title_slug: item.slug.clone(),
        // Radarr reads `ratings` on older versions and `movieRatings` on newer
        // ones; emitting both keeps either working.
        ratings: vec![tmdb_rating.clone()],
        movie_ratings: Some(RatingResource {
            tmdb: Some(tmdb_rating),
            imdb: imdb_rating,
            metacritic: item.rating("metacritic").map(rating_item),
            rotten_tomatoes: item.rating("rottenTomatoes").map(rating_item),
            trakt: item.rating("trakt").map(rating_item),
        }),
        runtime: item.runtime,
        images: item.images.iter().map(image_resource).collect(),
        genres: item.genres.clone(),
        keywords: item.keywords.clone(),
        year: item.year.unwrap_or(0),
        premier: item.in_cinemas.clone(),
        in_cinema: item.in_cinemas.clone(),
        physical_release: item.physical_release.clone(),
        digital_release: item.digital_release.clone(),
        alternative_titles: item
            .alternative_titles
            .iter()
            .map(|t| AlternativeTitleResource {
                title: t.title.clone(),
                title_type: t.title_type.clone(),
                language: t.language.clone(),
            })
            .collect(),
        translations: item
            .translations
            .iter()
            .map(|t| TranslationResource {
                title: t.title.clone(),
                overview: t.overview.clone(),
                language: t.language.clone(),
            })
            .collect(),
        credits: Some(credits(&item.credits)),
        studio: item.studio.clone(),
        youtube_trailer_id: item.trailer_youtube_id.clone(),
        // Radarr looks this up by country code and compares against its own
        // setting, which is ISO 3166-1 alpha-2 uppercase. The production
        // country is not the certification country, and must not stand in for it.
        certifications: match (&item.content_rating, &item.content_rating_country) {
            (Some(rating), Some(country)) => vec![CertificationResource {
                country: country.clone(),
                certification: rating.clone(),
            }],
            _ => Vec::new(),
        },
        status: radarr_status(item.status.as_deref()),
        collection: None,
        original_language: item.original_language.clone(),
        homepage: item.homepage.clone(),
        recommendations: Vec::new(),
        popularity: item.popularity,
    }
}

/// Radarr writes every credit into a table where `CreditTmdbId` is NOT NULL, so
/// one credit missing that identifier aborts the whole movie refresh inside
/// Radarr's database. Credits we cannot identify are dropped instead: losing one
/// name is better than losing the movie.
fn credits(all: &[Credit]) -> Credits {
    Credits {
        cast: all
            .iter()
            .filter(|c| c.credit_type == CreditType::Actor)
            .filter_map(|c| {
                Some(CastResource {
                    name: c.person_name.clone(),
                    character: c.character_name.clone(),
                    order: Some(c.sort_order),
                    tmdb_id: c.tmdb_person_id,
                    credit_id: c.credit_tmdb_id.clone()?,
                    images: person_image(c.image.as_deref()),
                })
            })
            .collect(),
        crew: all
            .iter()
            .filter(|c| c.credit_type != CreditType::Actor)
            .filter_map(|c| {
                Some(CrewResource {
                    name: c.person_name.clone(),
                    job: Some(capitalize(c.credit_type.as_str())),
                    department: Some(department_for(c.credit_type)),
                    order: Some(c.sort_order),
                    tmdb_id: c.tmdb_person_id,
                    credit_id: c.credit_tmdb_id.clone()?,
                    images: person_image(c.image.as_deref()),
                })
            })
            .collect(),
    }
}

fn person_image(url: Option<&str>) -> Vec<ImageResource> {
    url.map(|u| {
        vec![ImageResource {
            cover_type: "headshot".to_string(),
            url: u.to_string(),
        }]
    })
    .unwrap_or_default()
}

fn department_for(credit_type: CreditType) -> String {
    match credit_type {
        CreditType::Director | CreditType::Producer => "Directing",
        CreditType::Writer => "Writing",
        _ => "Acting",
    }
    .to_string()
}

fn image_resource(image: &Image) -> ImageResource {
    ImageResource {
        cover_type: image.cover_type.as_str().to_string(),
        url: image.url.clone(),
    }
}

fn rating_item(rating: &Rating) -> RatingItem {
    RatingItem {
        count: rating.votes.unwrap_or(0),
        value: rating.value.unwrap_or(0.0),
        origin: Some(rating.source.clone()),
        rating_type: rating.rating_type.clone(),
    }
}

/// Radarr's `MovieStatusType`: `tba`, `announced`, `inCinemas`, `released`, `deleted`.
fn radarr_status(status: Option<&str>) -> String {
    match status.unwrap_or("tba").to_ascii_lowercase().as_str() {
        "released" => "released",
        "incinemas" => "inCinemas",
        "announced" => "announced",
        "deleted" => "deleted",
        _ => "tba",
    }
    .to_string()
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

// ─── wire → canonical ────────────────────────────────────────────────────────

/// Absorb a response from the real `api.radarr.video` into the canonical model.
///
/// Radarr's own metadata service is a well-curated view of TMDB with extra
/// ratings and certifications attached, so it is worth taking as a second
/// opinion rather than only as a fallback.
pub fn to_item(resource: &MovieResource) -> MediaItem {
    use crate::{
        db::{new_id, now},
        domain::{AlternativeTitle, CoverType, ExternalIds, MediaKind, Translation},
    };

    let mut item = MediaItem::empty(MediaKind::Movie);

    item.title = resource.title.clone();
    item.original_title = non_empty(resource.original_title.as_deref());
    item.overview = non_empty(resource.overview.as_deref());
    item.homepage = non_empty(resource.homepage.as_deref());
    item.original_language = non_empty(resource.original_language.as_deref());
    item.runtime = resource.runtime.filter(|r| *r > 0);
    item.year = (resource.year > 0).then_some(resource.year);
    item.studio = non_empty(resource.studio.as_deref());
    item.trailer_youtube_id = non_empty(resource.youtube_trailer_id.as_deref());
    item.popularity = resource.popularity;
    item.genres = resource.genres.clone();
    item.keywords = resource.keywords.clone();
    item.status = Some(resource.status.to_ascii_lowercase());
    item.in_cinemas =
        non_empty(resource.in_cinema.as_deref()).or_else(|| non_empty(resource.premier.as_deref()));
    item.physical_release = non_empty(resource.physical_release.as_deref());
    item.digital_release = non_empty(resource.digital_release.as_deref());
    item.slug = if resource.title_slug.trim().is_empty() {
        crate::domain::make_slug(&resource.title, item.year)
    } else {
        resource.title_slug.clone()
    };

    item.external_ids = ExternalIds {
        tmdb: (resource.tmdb_id > 0).then_some(resource.tmdb_id),
        imdb: resource
            .imdb_id
            .as_deref()
            .and_then(crate::domain::ids::normalize_imdb_id),
        ..Default::default()
    };

    // Radarr's certifications carry the country the rating applies to, which is
    // the part TMDB's own block makes us dig for.
    if let Some(cert) = resource.certifications.first() {
        item.content_rating = non_empty(Some(&cert.certification));
        item.content_rating_country =
            non_empty(Some(&cert.country)).map(|c| c.to_ascii_uppercase());
    }

    item.ratings = collect_ratings(resource);

    item.images = resource
        .images
        .iter()
        .enumerate()
        .map(|(i, image)| Image {
            id: new_id(),
            season_number: None,
            cover_type: image.cover_type.parse().unwrap_or(CoverType::Unknown),
            url: image.url.clone(),
            language: None,
            sort_order: i as i32,
            source: Some(crate::providers::names::RADARR.to_string()),
            is_manual: false,
        })
        .collect();

    item.alternative_titles = resource
        .alternative_titles
        .iter()
        .filter(|t| !t.title.trim().is_empty())
        .map(|t| AlternativeTitle {
            id: new_id(),
            title: t.title.clone(),
            title_type: t.title_type.clone(),
            language: t.language.clone(),
            is_manual: false,
        })
        .collect();

    item.translations = resource
        .translations
        .iter()
        .filter(|t| t.title.is_some() || t.overview.is_some())
        .map(|t| Translation {
            language: t.language.clone(),
            title: non_empty(t.title.as_deref()),
            overview: non_empty(t.overview.as_deref()),
            is_manual: false,
        })
        .collect();

    if let Some(credits) = &resource.credits {
        item.credits = to_credits(credits);
    }

    item.updated_at = now();
    item
}

fn collect_ratings(resource: &MovieResource) -> Vec<Rating> {
    let Some(ratings) = &resource.movie_ratings else {
        return Vec::new();
    };

    [
        ("tmdb", &ratings.tmdb),
        ("imdb", &ratings.imdb),
        ("metacritic", &ratings.metacritic),
        ("rottenTomatoes", &ratings.rotten_tomatoes),
        ("trakt", &ratings.trakt),
    ]
    .into_iter()
    .filter_map(|(source, item)| {
        let item = item.as_ref()?;
        // A zeroed entry is the placeholder this server itself emits for an
        // unrated title; absorbing it back would invent a rating of 0/10.
        (item.count > 0).then(|| Rating {
            source: source.to_string(),
            value: Some(item.value),
            votes: Some(item.count),
            rating_type: item.rating_type.clone(),
        })
    })
    .collect()
}

fn to_credits(credits: &Credits) -> Vec<Credit> {
    use crate::db::new_id;

    let cast = credits.cast.iter().enumerate().map(|(i, member)| Credit {
        id: new_id(),
        credit_type: CreditType::Actor,
        person_name: member.name.clone(),
        character_name: member.character.clone(),
        image: member.images.first().map(|i| i.url.clone()),
        tmdb_person_id: member.tmdb_id,
        credit_tmdb_id: Some(member.credit_id.clone()).filter(|c| !c.is_empty()),
        sort_order: member.order.unwrap_or(i as i32),
        is_manual: false,
    });

    let crew = credits.crew.iter().enumerate().map(|(i, member)| Credit {
        id: new_id(),
        credit_type: member
            .job
            .as_deref()
            .unwrap_or("actor")
            .parse()
            .unwrap_or(CreditType::Actor),
        person_name: member.name.clone(),
        character_name: None,
        image: member.images.first().map(|i| i.url.clone()),
        tmdb_person_id: member.tmdb_id,
        credit_tmdb_id: Some(member.credit_id.clone()).filter(|c| !c.is_empty()),
        sort_order: member.order.unwrap_or(i as i32),
        is_manual: false,
    });

    cast.chain(crew).collect()
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CoverType, ExternalIds, MediaKind};

    fn item() -> MediaItem {
        let mut it = MediaItem::empty(MediaKind::Movie);
        it.title = "Arrival".into();
        it.slug = "arrival-2016".into();
        it.year = Some(2016);
        it.runtime = Some(116);
        it.status = Some("released".into());
        it.in_cinemas = Some("2016-11-11".into());
        it.digital_release = Some("2017-01-31".into());
        it.content_rating = Some("PG-13".into());
        it.original_country = Some("usa".into());
        it.content_rating_country = Some("US".into());
        it.external_ids = ExternalIds {
            tmdb: Some(329865),
            imdb: Some("tt2543164".into()),
            ..Default::default()
        };
        it.ratings = vec![Rating {
            source: "tmdb".into(),
            value: Some(7.6),
            votes: Some(18_000),
            rating_type: Some("user".into()),
        }];
        it.credits = vec![
            Credit {
                id: "1".into(),
                credit_type: CreditType::Actor,
                person_name: "Amy Adams".into(),
                character_name: Some("Louise Banks".into()),
                image: Some("https://example.invalid/a.jpg".into()),
                tmdb_person_id: Some(9273),
                credit_tmdb_id: Some("52fe4726c3a36847f812048b".into()),
                sort_order: 0,
                is_manual: false,
            },
            Credit {
                id: "2".into(),
                credit_type: CreditType::Director,
                person_name: "Denis Villeneuve".into(),
                character_name: None,
                image: None,
                tmdb_person_id: Some(137427),
                credit_tmdb_id: Some("5751b41cc3a3685ba7002c1f".into()),
                sort_order: 0,
                is_manual: false,
            },
        ];
        it.images = vec![Image {
            id: "i".into(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: "https://example.invalid/p.jpg".into(),
            language: None,
            sort_order: 0,
            source: None,
            is_manual: false,
        }];
        it
    }

    #[test]
    fn statuses_use_radarrs_names() {
        assert_eq!(radarr_status(Some("released")), "released");
        assert_eq!(radarr_status(Some("inCinemas")), "inCinemas");
        assert_eq!(radarr_status(Some("announced")), "announced");
        assert_eq!(radarr_status(None), "tba");
        assert_eq!(radarr_status(Some("nonsense")), "tba");
    }

    #[test]
    fn both_rating_shapes_are_emitted() {
        let resource = from_item(&item());

        // Old Radarr reads `ratings`, new Radarr reads `movieRatings`.
        assert_eq!(resource.ratings.len(), 1);
        assert_eq!(resource.ratings[0].value, 7.6);
        assert_eq!(resource.movie_ratings.unwrap().tmdb.unwrap().count, 18_000);
    }

    #[test]
    fn a_credit_with_no_identifier_is_dropped_rather_than_sent() {
        // Radarr's Credits.CreditTmdbId is NOT NULL: sending one without it
        // fails the whole movie refresh, not just that credit.
        let mut it = item();
        it.credits[0].credit_tmdb_id = None;

        let credits = from_item(&it).credits.unwrap();
        assert_eq!(credits.cast.len(), 0);
        assert_eq!(credits.crew.len(), 1, "the crew credit still has its id");
    }

    #[test]
    fn credit_identifiers_are_emitted() {
        let credits = from_item(&item()).credits.unwrap();
        assert_eq!(credits.cast[0].credit_id, "52fe4726c3a36847f812048b");
        assert_eq!(credits.crew[0].credit_id, "5751b41cc3a3685ba7002c1f");
    }

    #[test]
    fn cast_and_crew_are_separated() {
        let credits = from_item(&item()).credits.unwrap();

        assert_eq!(credits.cast.len(), 1);
        assert_eq!(credits.cast[0].name, "Amy Adams");
        assert_eq!(credits.cast[0].images.len(), 1);

        assert_eq!(credits.crew.len(), 1);
        assert_eq!(credits.crew[0].job.as_deref(), Some("Director"));
        assert_eq!(credits.crew[0].department.as_deref(), Some("Directing"));
    }

    #[test]
    fn premier_and_in_cinema_carry_the_same_date() {
        // Radarr reads whichever of the two its version knows about.
        let resource = from_item(&item());
        assert_eq!(resource.premier.as_deref(), Some("2016-11-11"));
        assert_eq!(resource.in_cinema.as_deref(), Some("2016-11-11"));
    }

    #[test]
    fn a_content_rating_becomes_a_certification_with_an_alpha2_country() {
        // Radarr matches on `country == "US"`; the alpha-3 production country
        // would never match and the certification would silently vanish.
        let resource = from_item(&item());
        assert_eq!(resource.certifications.len(), 1);
        assert_eq!(resource.certifications[0].certification, "PG-13");
        assert_eq!(resource.certifications[0].country, "US");
    }

    #[test]
    fn a_rating_with_no_known_country_is_not_guessed_at() {
        let mut it = item();
        it.content_rating_country = None;
        assert!(from_item(&it).certifications.is_empty());
    }

    #[test]
    fn an_unrated_movie_still_carries_a_zeroed_tmdb_rating() {
        // Radarr dereferences this without a null check; leaving it out fails
        // the entire search response, not just this one title.
        let mut it = item();
        it.ratings.clear();

        let resource = from_item(&it);

        assert_eq!(resource.ratings.len(), 1);
        assert_eq!(resource.ratings[0].count, 0);

        let tmdb = resource.movie_ratings.unwrap().tmdb.unwrap();
        assert_eq!(tmdb.count, 0);
        assert_eq!(tmdb.value, 0.0);
        // Radarr parses this into an enum; a null would throw.
        assert_eq!(tmdb.rating_type.as_deref(), Some("user"));
    }

    #[test]
    fn a_radarr_response_round_trips_through_the_canonical_model() {
        let original = from_item(&item());
        let canonical = to_item(&original);

        assert_eq!(canonical.title, "Arrival");
        assert_eq!(canonical.year, Some(2016));
        assert_eq!(canonical.runtime, Some(116));
        assert_eq!(canonical.status.as_deref(), Some("released"));
        assert_eq!(canonical.external_ids.tmdb, Some(329865));
        assert_eq!(canonical.external_ids.imdb.as_deref(), Some("tt2543164"));
        assert_eq!(canonical.content_rating.as_deref(), Some("PG-13"));
        assert_eq!(canonical.content_rating_country.as_deref(), Some("US"));
        assert_eq!(canonical.in_cinemas.as_deref(), Some("2016-11-11"));
        assert_eq!(canonical.credits.len(), 2);
        assert_eq!(canonical.images.len(), 1);
    }

    #[test]
    fn the_zeroed_rating_placeholder_is_not_absorbed_back() {
        // `from_item` emits a zeroed TMDB entry for an unrated title so Radarr
        // does not crash on it. Reading that back as a real rating of 0/10 would
        // be worse than having none.
        let mut it = item();
        it.ratings.clear();

        let canonical = to_item(&from_item(&it));
        assert!(canonical.ratings.is_empty());
    }

    #[test]
    fn a_real_rating_does_survive_the_round_trip() {
        let canonical = to_item(&from_item(&item()));

        assert_eq!(canonical.ratings.len(), 1);
        assert_eq!(canonical.ratings[0].source, "tmdb");
        assert_eq!(canonical.ratings[0].votes, Some(18_000));
    }

    #[test]
    fn the_wire_form_serialises_to_radarrs_field_names() {
        let json = serde_json::to_value(from_item(&item())).unwrap();

        assert_eq!(json["tmdbId"], 329865);
        assert_eq!(json["titleSlug"], "arrival-2016");
        assert_eq!(json["inCinema"], "2016-11-11");
        assert_eq!(json["digitalRelease"], "2017-01-31");
        assert_eq!(json["youtubeTrailerId"], serde_json::Value::Null);
    }
}
