//! Radarr's metadata format.
//!
//! Field names follow `NzbDrone.Core/MetadataSource/SkyHook/Resource/*.cs` in
//! the Radarr source. Radarr's own `MovieResource` is what `api.radarr.video`
//! returns, so matching it exactly is what makes the substitution invisible.

use serde::{Deserialize, Serialize};

use crate::domain::{Credit, CreditType, Image, MediaItem, Rating};

#[derive(Clone, Debug, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatingItem {
    pub count: i64,
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub rating_type: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageResource {
    pub cover_type: String,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlternativeTitleResource {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub title_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationResource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    pub language: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificationResource {
    pub country: String,
    pub certification: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credits {
    #[serde(default)]
    pub cast: Vec<CastResource>,
    #[serde(default)]
    pub crew: Vec<CrewResource>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CastResource {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(default)]
    pub images: Vec<ImageResource>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrewResource {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub department: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(default)]
    pub images: Vec<ImageResource>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionResource {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    pub tmdb_id: i64,
    #[serde(default)]
    pub images: Vec<ImageResource>,
    #[serde(default)]
    pub parts: Vec<MovieResource>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecommendationResource {
    pub tmdb_id: i64,
    pub title: String,
}

// ─── canonical → wire ────────────────────────────────────────────────────────

pub fn from_item(item: &MediaItem) -> MovieResource {
    let tmdb_rating = item.rating("tmdb").map(rating_item);
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
        ratings: tmdb_rating.clone().into_iter().collect(),
        movie_ratings: (tmdb_rating.is_some() || imdb_rating.is_some()).then(|| RatingResource {
            tmdb: tmdb_rating,
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
        certifications: item
            .content_rating
            .as_ref()
            .map(|c| {
                vec![CertificationResource {
                    country: item
                        .original_country
                        .clone()
                        .unwrap_or_else(|| "usa".to_string()),
                    certification: c.clone(),
                }]
            })
            .unwrap_or_default(),
        status: radarr_status(item.status.as_deref()),
        collection: None,
        original_language: item.original_language.clone(),
        homepage: item.homepage.clone(),
        recommendations: Vec::new(),
        popularity: item.popularity,
    }
}

fn credits(all: &[Credit]) -> Credits {
    Credits {
        cast: all
            .iter()
            .filter(|c| c.credit_type == CreditType::Actor)
            .map(|c| CastResource {
                name: c.person_name.clone(),
                character: c.character_name.clone(),
                order: Some(c.sort_order),
                tmdb_id: c.tmdb_person_id,
                images: person_image(c.image.as_deref()),
            })
            .collect(),
        crew: all
            .iter()
            .filter(|c| c.credit_type != CreditType::Actor)
            .map(|c| CrewResource {
                name: c.person_name.clone(),
                job: Some(capitalize(c.credit_type.as_str())),
                department: Some(department_for(c.credit_type)),
                tmdb_id: c.tmdb_person_id,
                images: person_image(c.image.as_deref()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CoverType, ExternalIds, MediaKind};

    fn item() -> MediaItem {
        let mut it = MediaItem::empty(MediaKind::Movie);
        it.title = "Arrival".into();
        it.slug = "arrival-2016".into();
        it.year = Some(2016);
        it.status = Some("released".into());
        it.in_cinemas = Some("2016-11-11".into());
        it.digital_release = Some("2017-01-31".into());
        it.content_rating = Some("PG-13".into());
        it.original_country = Some("usa".into());
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
    fn a_content_rating_becomes_a_certification() {
        let resource = from_item(&item());
        assert_eq!(resource.certifications.len(), 1);
        assert_eq!(resource.certifications[0].certification, "PG-13");
        assert_eq!(resource.certifications[0].country, "usa");
    }

    #[test]
    fn a_movie_with_no_ratings_omits_both_shapes() {
        let mut it = item();
        it.ratings.clear();

        let resource = from_item(&it);
        assert!(resource.ratings.is_empty());
        assert!(resource.movie_ratings.is_none());
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
