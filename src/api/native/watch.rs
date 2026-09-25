//! Where a work can be watched: the streaming, rental and purchase services
//! that carry it in a country, as TMDB lists them from JustWatch.
//!
//! Asked of TMDB once a day per work and kept for every country at once, so
//! a reader abroad costs nothing more. TMDB's terms ask that JustWatch be
//! named beside the data, and the answer carries the name for the page to
//! show.

use axum::{
    Extension, Json,
    body::Bytes,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    auth::Identity,
    error::{AppError, AppResult},
    providers::tmdb,
    service,
    state::AppState,
};

const TAG: &str = super::items::TAG;

/// Who the data comes from, to be shown beside it.
const ATTRIBUTION: &str = "JustWatch";

/// The country to answer for when neither the caller nor a setting names one.
const FALLBACK_REGION: &str = "US";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(where_to_watch))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WatchQuery {
    /// A country, ISO 3166-1 alpha-2: `FR`, `US`. The caller's own setting
    /// when absent, then the country of the language they are answered in.
    pub region: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WatchProvider {
    pub id: i64,
    pub name: String,
    /// The service's mark, as TMDB serves it.
    pub logo: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WhereToWatch {
    /// The country answered for.
    pub region: String,
    /// TMDB's page for the work in that country, which carries JustWatch's
    /// own links to each service.
    pub link: Option<String>,
    /// Included with a subscription.
    pub flatrate: Vec<WatchProvider>,
    pub rent: Vec<WatchProvider>,
    pub buy: Vec<WatchProvider>,
    /// Free to watch, and free with advertising.
    pub free: Vec<WatchProvider>,
    pub ads: Vec<WatchProvider>,
    /// Every country TMDB lists anything for, so a reader may ask for another.
    pub regions: Vec<String>,
    /// Who the data comes from; shown beside it, as TMDB's terms ask.
    pub attribution: &'static str,
}

/// Where a work can be watched in a country.
#[utoipa::path(
    get, path = "/items/{id}/watch", tag = TAG,
    params(("id" = String, Path, description = "The work's id"), WatchQuery),
    responses(
        (status = 200, body = WhereToWatch),
        (status = 400, description = "The region is not a country code"),
        (status = 404, description = "No such work, or none this caller may see"),
        (status = 503, description = "No TMDB key is configured"),
    ),
)]
async fn where_to_watch(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Query(query): Query<WatchQuery>,
) -> AppResult<Json<WhereToWatch>> {
    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    // As the work's own page decides it.
    let hidden = !item.is_enabled
        || (item.is_adult
            && !state.adult_for(identity.client_id(), identity.peer_id(), Some(true)));
    if hidden && !identity.can_write() {
        return Err(AppError::NotFound);
    }

    let region = match query
        .region
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        Some(asked) => region_code(asked).ok_or_else(|| {
            AppError::BadRequest("region is a country code such as FR or US".into())
        })?,
        None => state
            .watch_region(identity.client_id(), identity.peer_id())
            .unwrap_or_else(|| FALLBACK_REGION.to_string()),
    };

    let Some(tmdb_id) = item.external_ids.tmdb else {
        return Ok(Json(empty(region)));
    };
    if !state.tmdb.is_configured() {
        return Err(AppError::ProviderNotConfigured);
    }

    // Every country at once, kept a day: the page asks for one, a reader
    // abroad for another, and TMDB is asked once.
    let key = format!("watch:{}:{tmdb_id}", item.kind.as_str());
    let results: Value = match state.caches.lists.get(&key).await {
        Some(bytes) => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        None => {
            let fetched = state
                .tmdb
                .watch_providers(item.kind, tmdb_id)
                .await
                .map_err(AppError::UpstreamUnavailable)?
                .and_then(|v| v.get("results").cloned())
                .unwrap_or(Value::Object(Default::default()));
            let bytes = Bytes::from(serde_json::to_vec(&fetched).unwrap_or_default());
            state.caches.lists.insert(key, bytes).await;
            fetched
        }
    };

    Ok(Json(shape(&results, region)))
}

/// A country code as TMDB keys its answers: two letters, upper case.
fn region_code(raw: &str) -> Option<String> {
    let code = raw.trim().to_ascii_uppercase();
    (code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase())).then_some(code)
}

fn empty(region: String) -> WhereToWatch {
    WhereToWatch {
        region,
        link: None,
        flatrate: Vec::new(),
        rent: Vec::new(),
        buy: Vec::new(),
        free: Vec::new(),
        ads: Vec::new(),
        regions: Vec::new(),
        attribution: ATTRIBUTION,
    }
}

/// TMDB's `results` — one object per country — read for one of them.
fn shape(results: &Value, region: String) -> WhereToWatch {
    let mut regions: Vec<String> = results
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    regions.sort();
    let country = results.get(&region);
    let list = |name: &str| -> Vec<WatchProvider> {
        let mut providers: Vec<(i64, WatchProvider)> = country
            .and_then(|c| c.get(name))
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|e| {
                        Some((
                            e.get("display_priority")
                                .and_then(Value::as_i64)
                                .unwrap_or(i64::MAX),
                            WatchProvider {
                                id: e.get("provider_id").and_then(Value::as_i64)?,
                                name: e.get("provider_name").and_then(Value::as_str)?.to_string(),
                                logo: e
                                    .get("logo_path")
                                    .and_then(Value::as_str)
                                    .map(tmdb::image_url),
                            },
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        providers.sort_by_key(|(priority, _)| *priority);
        providers.into_iter().map(|(_, p)| p).collect()
    };
    WhereToWatch {
        link: country
            .and_then(|c| c.get("link"))
            .and_then(Value::as_str)
            .map(str::to_string),
        flatrate: list("flatrate"),
        rent: list("rent"),
        buy: list("buy"),
        free: list("free"),
        ads: list("ads"),
        regions,
        region,
        attribution: ATTRIBUTION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_countrys_services_are_read_in_their_order_and_the_countries_listed() {
        let results = json!({
            "FR": {
                "link": "https://www.themoviedb.org/movie/238/watch?locale=FR",
                "flatrate": [
                    { "provider_id": 8, "provider_name": "Netflix", "logo_path": "/n.jpg", "display_priority": 2 },
                    { "provider_id": 119, "provider_name": "Amazon Prime Video", "logo_path": "/a.jpg", "display_priority": 1 }
                ],
                "buy": [ { "provider_id": 2, "provider_name": "Apple TV", "logo_path": "/t.jpg", "display_priority": 4 } ]
            },
            "US": { "link": "https://www.themoviedb.org/movie/238/watch?locale=US" }
        });
        let fr = shape(&results, "FR".into());
        assert_eq!(fr.regions, vec!["FR", "US"]);
        assert_eq!(
            fr.link.as_deref(),
            Some("https://www.themoviedb.org/movie/238/watch?locale=FR")
        );
        assert_eq!(
            fr.flatrate
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["Amazon Prime Video", "Netflix"]
        );
        assert!(
            fr.flatrate[0]
                .logo
                .as_deref()
                .is_some_and(|l| l.ends_with("/a.jpg"))
        );
        assert_eq!(fr.buy.len(), 1);
        assert!(fr.rent.is_empty() && fr.free.is_empty());
        assert_eq!(fr.attribution, "JustWatch");

        let de = shape(&results, "DE".into());
        assert!(de.flatrate.is_empty() && de.link.is_none());
        assert_eq!(de.regions.len(), 2);
    }

    #[test]
    fn a_region_is_two_letters() {
        assert_eq!(region_code(" fr ").as_deref(), Some("FR"));
        assert!(region_code("FRA").is_none());
        assert!(region_code("f1").is_none());
        assert!(region_code("").is_none());
    }
}
