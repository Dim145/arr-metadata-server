//! TMDB client.
//!
//! Responses are fetched as `serde_json::Value` first and then deserialized into
//! the typed models. The untyped copy is what gets stored as a snapshot, so a
//! later mapping fix can be replayed without spending another API call.

pub mod map;
pub mod models;

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures::future::join_all;
use serde_json::Value;

use crate::{config, domain::MediaKind, providers::Gate};

pub const IMAGE_BASE: &str = "https://image.tmdb.org/t/p/original";

/// TMDB's documented ceiling is around 50 requests/second. Series fetches fan
/// out one call per season, so bound concurrency rather than discovering the
/// limit through 429s.
const MAX_CONCURRENT: usize = 16;

pub struct TmdbClient {
    http: reqwest::Client,
    api_key: Option<String>,
    base: String,
    /// The language and adult flag sent to TMDB. Both are settings, so both can
    /// change while the server runs; holding them behind a lock rather than
    /// threading them through eight call sites keeps the change where it
    /// belongs, which is one method.
    tuning: parking_lot::RwLock<Tuning>,
    /// At most [`MAX_CONCURRENT`] in flight, and none while TMDB asked to be
    /// left alone.
    gate: Gate,
}

#[derive(Clone)]
struct Tuning {
    language: String,
    include_adult: bool,
}

impl TmdbClient {
    pub fn new(http: reqwest::Client, cfg: &config::Tmdb) -> Self {
        Self {
            http,
            api_key: cfg.api_key.clone(),
            base: format!("{}/3", cfg.upstream),
            tuning: parking_lot::RwLock::new(Tuning {
                language: cfg.language.clone(),
                include_adult: cfg.include_adult,
            }),
            gate: Gate::new("tmdb", "TMDB", MAX_CONCURRENT),
        }
    }

    /// Follow the settings. Called at boot and whenever one of them changes.
    pub fn tune(&self, language: &str, include_adult: bool) {
        let mut tuning = self.tuning.write();
        tuning.language = language.to_string();
        tuning.include_adult = include_adult;
    }

    fn language(&self) -> String {
        self.tuning.read().language.clone()
    }

    fn include_adult(&self) -> bool {
        self.tuning.read().include_adult
    }

    pub fn is_configured(&self) -> bool {
        self.api_key.is_some()
    }

    /// Build an authenticated request.
    ///
    /// A v4 token is a JWT and goes in `Authorization`; a v3 key goes in the
    /// query string. Accepting both means an existing `.env` from either
    /// predecessor project keeps working.
    fn get(&self, url: &str, key: &str) -> reqwest::RequestBuilder {
        if key.starts_with("eyJ") {
            self.http.get(url).bearer_auth(key)
        } else {
            self.http.get(url).query(&[("api_key", key)])
        }
    }

    /// Perform a GET and return the parsed body, or `None` on 404.
    async fn fetch(&self, url: &str, params: &[(&str, String)]) -> Result<Option<Value>> {
        let key = self
            .api_key
            .as_deref()
            .ok_or_else(|| anyhow!("no TMDB API key configured"))?;

        // A request error names the address it was sent to, key and all, and
        // the error is logged: the gate keeps its kind and what lay under it
        // — `url` here is the path alone.
        let (response, _permit) = self
            .gate
            .send(|| {
                self.get(url, key)
                    .query(params)
                    .timeout(Duration::from_secs(20))
            })
            .await
            .map_err(|e| anyhow!("TMDB request failed: {url}: {e}"))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            anyhow::bail!("TMDB rate limit reached");
        }

        let status = response.status();
        if !status.is_success() {
            // TMDB puts a human-readable reason in the body; include it, since
            // "401" alone does not distinguish a bad key from a revoked one —
            // the start of it, read no further than that.
            let reason = crate::providers::error_text(response).await;
            anyhow::bail!("TMDB returned {status} for {url}: {reason}");
        }

        let value = crate::providers::read_json(response)
            .await
            .with_context(|| format!("TMDB returned a body this server could not read: {url}"))?;

        Ok(Some(value))
    }

    fn typed<T: serde::de::DeserializeOwned>(value: &Value, what: &str) -> Result<T> {
        serde_json::from_value(value.clone())
            .with_context(|| format!("could not interpret the TMDB {what} response"))
    }

    // ─── lookup ──────────────────────────────────────────────────────────────

    /// Resolve an id from another provider, e.g. `tvdb_id` or `imdb_id`.
    pub async fn find(
        &self,
        external_source: &str,
        external_id: &str,
    ) -> Result<models::FindResponse> {
        let url = format!("{}/find/{external_id}", self.base);
        let params = [
            ("external_source", external_source.to_string()),
            ("language", self.language()),
        ];

        match self.fetch(&url, &params).await? {
            Some(value) => Self::typed(&value, "find"),
            None => Ok(models::FindResponse::default()),
        }
    }

    // ─── series ──────────────────────────────────────────────────────────────

    /// Full series document, with the sub-resources the mapper needs appended.
    ///
    /// Returns the raw body alongside the typed one so the caller can snapshot it.
    pub async fn tv(&self, id: i64) -> Result<Option<(Value, models::Tv)>> {
        let Some(raw) = self.fetch_tv(id, &self.language()).await? else {
            return Ok(None);
        };

        let mut tv: models::Tv = Self::typed(&raw, "series")?;

        // TMDB returns an empty overview rather than falling back when a title
        // has no translation in the requested language. Backfill from en-US so
        // a French-configured server still gets a description.
        if self.language() != "en-US"
            && tv.overview.as_deref().unwrap_or_default().is_empty()
            && let Ok(Some(fallback_raw)) = self.fetch_tv(id, "en-US").await
            && let Ok(fallback) = Self::typed::<models::Tv>(&fallback_raw, "series")
            && tv.overview.as_deref().unwrap_or_default().is_empty()
        {
            tv.overview = fallback.overview;
        }

        Ok(Some((raw, tv)))
    }

    async fn fetch_tv(&self, id: i64, language: &str) -> Result<Option<Value>> {
        let url = format!("{}/tv/{id}", self.base);
        let params = [
            ("language", language.to_string()),
            (
                "append_to_response",
                "external_ids,credits,content_ratings,alternative_titles,keywords,videos,images,translations"
                    .to_string(),
            ),
            // Posters and logos in the configured language plus language-neutral art.
            (
                "include_image_language",
                format!("{},null", crate::providers::lang::base_language(language)),
            ),
        ];

        self.fetch(&url, &params).await
    }

    /// Every season's episode list, fetched concurrently, and the numbers of
    /// those that went unanswered.
    ///
    /// A season that fails is left out rather than failing the series — and
    /// named, so that what is stored of it stands: taken for a season with no
    /// episodes, it was written as one, and Sonarr deleted every episode of
    /// it until the next refresh. A season the series lists and its own
    /// address does not know is the same failure.
    pub async fn tv_seasons(&self, id: i64, numbers: &[i32]) -> (Vec<models::Season>, Vec<i32>) {
        let results = join_all(numbers.iter().map(|&n| self.tv_season(id, n))).await;

        let mut unanswered = Vec::new();
        let seasons = results
            .into_iter()
            .zip(numbers)
            .filter_map(|(result, &number)| match result {
                Ok(Some(season)) => Some(season),
                Ok(None) => {
                    tracing::warn!(
                        tmdb_id = id,
                        season = number,
                        "a season the series lists was not found"
                    );
                    unanswered.push(number);
                    None
                }
                Err(e) => {
                    tracing::warn!(
                        tmdb_id = id,
                        season = number,
                        error = format_args!("{e:#}"),
                        "season fetch failed"
                    );
                    unanswered.push(number);
                    None
                }
            })
            .collect();

        (seasons, unanswered)
    }

    async fn tv_season(&self, id: i64, number: i32) -> Result<Option<models::Season>> {
        let Some(raw) = self.fetch_season(id, number, &self.language()).await? else {
            return Ok(None);
        };

        let mut season: models::Season = Self::typed(&raw, "season")?;

        let missing_titles = season
            .episodes
            .iter()
            .any(|e| e.name.as_deref().unwrap_or_default().is_empty());

        if self.language() != "en-US"
            && missing_titles
            && let Ok(Some(fallback_raw)) = self.fetch_season(id, number, "en-US").await
            && let Ok(fallback) = Self::typed::<models::Season>(&fallback_raw, "season")
        {
            backfill_episodes(&mut season, &fallback);
        }

        Ok(Some(season))
    }

    async fn fetch_season(&self, id: i64, number: i32, language: &str) -> Result<Option<Value>> {
        let url = format!("{}/tv/{id}/season/{number}", self.base);
        self.fetch(&url, &[("language", language.to_string())])
            .await
    }

    /// Every season's episodes in one specific language, and whether one of
    /// them went unanswered.
    ///
    /// Used to fill in a language somebody asked for. Unlike [`Self::tv_seasons`]
    /// this does not fall back to en-US: the caller wants this language or
    /// nothing, and a silent English fallback would look like a translation that
    /// exists when it does not. A season TMDB does not have is an answer; one
    /// whose request failed, or whose answer could not be read, is not, and the
    /// caller is told so rather than left to take the seasons it got for all
    /// there is.
    pub async fn tv_seasons_in(
        &self,
        id: i64,
        numbers: &[i32],
        language: &str,
    ) -> (Vec<models::Season>, bool) {
        let results = join_all(numbers.iter().map(|&n| self.fetch_season(id, n, language))).await;

        let mut unanswered = false;
        let seasons = results
            .into_iter()
            .zip(numbers)
            .filter_map(|(result, number)| match result {
                Ok(Some(raw)) => match Self::typed::<models::Season>(&raw, "season") {
                    Ok(season) => Some(season),
                    Err(e) => {
                        tracing::warn!(tmdb_id = id, season = number, error = %e, "season parse failed");
                        unanswered = true;
                        None
                    }
                },
                Ok(None) => None,
                Err(e) => {
                    tracing::warn!(tmdb_id = id, season = number, %language, error = %e, "season fetch failed");
                    unanswered = true;
                    None
                }
            })
            .collect();

        (seasons, unanswered)
    }

    pub async fn tv_external_ids(&self, id: i64) -> Result<models::ExternalIds> {
        let url = format!("{}/tv/{id}/external_ids", self.base);

        match self.fetch(&url, &[]).await? {
            Some(value) => Self::typed(&value, "external ids"),
            None => Ok(models::ExternalIds::default()),
        }
    }

    pub async fn search_tv(&self, query: &str, limit: usize) -> Result<Vec<models::TvSummary>> {
        let url = format!("{}/search/tv", self.base);
        let params = [
            ("query", query.to_string()),
            ("language", self.language()),
            ("include_adult", self.include_adult().to_string()),
            ("page", "1".to_string()),
        ];

        let Some(value) = self.fetch(&url, &params).await? else {
            return Ok(Vec::new());
        };

        let parsed: models::SearchResponse<models::TvSummary> = Self::typed(&value, "search")?;
        Ok(parsed.results.into_iter().take(limit).collect())
    }

    // ─── movie ───────────────────────────────────────────────────────────────

    pub async fn movie(&self, id: i64) -> Result<Option<(Value, models::Movie)>> {
        let Some(raw) = self.fetch_movie(id, &self.language()).await? else {
            return Ok(None);
        };

        let mut movie: models::Movie = Self::typed(&raw, "movie")?;

        if self.language() != "en-US"
            && movie.overview.as_deref().unwrap_or_default().is_empty()
            && let Ok(Some(fallback_raw)) = self.fetch_movie(id, "en-US").await
            && let Ok(fallback) = Self::typed::<models::Movie>(&fallback_raw, "movie")
        {
            movie.overview = fallback.overview;
        }

        Ok(Some((raw, movie)))
    }

    /// Where a work can be watched, by country, as TMDB lists it from
    /// JustWatch: `results.{REGION}.{flatrate,rent,buy,free,ads}`, each a
    /// list of services. `None` where TMDB knows nothing of the work.
    pub async fn watch_providers(&self, kind: MediaKind, id: i64) -> Result<Option<Value>> {
        let path = match kind {
            MediaKind::Series => "tv",
            MediaKind::Movie => "movie",
        };
        let url = format!("{}/{path}/{id}/watch/providers", self.base);
        self.fetch(&url, &[]).await
    }

    /// What TMDB recommends beside a work, in the language the answers are
    /// in: the `results` of `/{tv|movie}/{id}/recommendations`, whole.
    pub async fn recommendations(&self, kind: MediaKind, id: i64) -> Result<Option<Value>> {
        let path = match kind {
            MediaKind::Series => "tv",
            MediaKind::Movie => "movie",
        };
        let url = format!("{}/{path}/{id}/recommendations", self.base);
        let params = [("language", self.language())];
        Ok(self
            .fetch(&url, &params)
            .await?
            .and_then(|v| v.get("results").cloned()))
    }

    /// A collection as TMDB has it — name, overview, pictures, parts — in a
    /// language, and untyped, for a reader that wants the record rather
    /// than the model.
    pub async fn collection_raw(&self, id: i64, language: &str) -> Result<Option<Value>> {
        let url = format!("{}/collection/{id}", self.base);
        let params = [("language", language.to_string())];
        self.fetch(&url, &params).await
    }

    /// Somebody as TMDB has them — biography, dates, the names they go by,
    /// pictures and the ids other sites file them under — in a language, and
    /// untyped, for a reader that wants the record rather than the model.
    pub async fn person(&self, id: i64, language: &str) -> Result<Option<Value>> {
        let url = format!("{}/person/{id}", self.base);
        let params = [
            ("language", language.to_string()),
            ("append_to_response", "images,external_ids".to_string()),
        ];
        self.fetch(&url, &params).await
    }

    async fn fetch_movie(&self, id: i64, language: &str) -> Result<Option<Value>> {
        let url = format!("{}/movie/{id}", self.base);
        let params = [
            ("language", language.to_string()),
            (
                "append_to_response",
                "external_ids,credits,release_dates,alternative_titles,keywords,videos,images,translations"
                    .to_string(),
            ),
            ("include_image_language", format!("{},null", crate::providers::lang::base_language(language))),
        ];

        self.fetch(&url, &params).await
    }

    pub async fn search_movie(
        &self,
        query: &str,
        year: Option<i32>,
        limit: usize,
    ) -> Result<Vec<models::MovieSummary>> {
        let url = format!("{}/search/movie", self.base);
        let mut params = vec![
            ("query", query.to_string()),
            ("language", self.language()),
            ("include_adult", self.include_adult().to_string()),
            ("page", "1".to_string()),
        ];

        if let Some(year) = year {
            params.push(("year", year.to_string()));
        }

        let Some(value) = self.fetch(&url, &params).await? else {
            return Ok(Vec::new());
        };

        let parsed: models::SearchResponse<models::MovieSummary> = Self::typed(&value, "search")?;
        Ok(parsed.results.into_iter().take(limit).collect())
    }

    pub async fn collection(&self, id: i64) -> Result<Option<models::Collection>> {
        let url = format!("{}/collection/{id}", self.base);
        let params = [("language", self.language())];

        match self.fetch(&url, &params).await? {
            Some(value) => Ok(Some(Self::typed(&value, "collection")?)),
            None => Ok(None),
        }
    }

    // ─── lists ───────────────────────────────────────────────────────────────

    pub async fn popular_movies(&self, page: i32) -> Result<Vec<models::MovieSummary>> {
        self.movie_list(&format!("{}/movie/popular", self.base), page)
            .await
    }

    pub async fn trending_movies(&self) -> Result<Vec<models::MovieSummary>> {
        self.movie_list(&format!("{}/trending/movie/week", self.base), 1)
            .await
    }

    async fn movie_list(&self, url: &str, page: i32) -> Result<Vec<models::MovieSummary>> {
        let params = [
            ("language", self.language()),
            ("page", page.max(1).to_string()),
        ];

        let Some(value) = self.fetch(url, &params).await? else {
            return Ok(Vec::new());
        };

        let parsed: models::SearchResponse<models::MovieSummary> = Self::typed(&value, "list")?;
        Ok(parsed.results)
    }

    /// Series that first aired between two dates, most popular first: what a
    /// season brings that nobody has asked this server for yet.
    ///
    /// `language` is the language titles and synopses come back in, TMDB's
    /// own setting when absent; `original` narrows to works made in one.
    pub async fn discover_tv(
        &self,
        from: &str,
        to: &str,
        original: Option<&str>,
        language: Option<&str>,
        page: u32,
    ) -> Result<(Vec<models::TvSummary>, bool)> {
        let url = format!("{}/discover/tv", self.base);
        let mut params = vec![
            ("first_air_date.gte", from.to_string()),
            ("first_air_date.lte", to.to_string()),
            ("sort_by", "popularity.desc".to_string()),
            ("include_adult", self.include_adult().to_string()),
            ("include_null_first_air_dates", "false".to_string()),
            (
                "language",
                language.map_or_else(|| self.language(), String::from),
            ),
            ("page", page.to_string()),
        ];
        if let Some(original) = original {
            params.push(("with_original_language", original.to_string()));
        }

        let Some(value) = self.fetch(&url, &params).await? else {
            return Ok((Vec::new(), false));
        };

        let parsed: models::SearchResponse<models::TvSummary> = Self::typed(&value, "discover")?;
        Ok((parsed.results, i64::from(page) < parsed.total_pages))
    }

    /// Films first released between two dates, most popular first.
    pub async fn discover_movies(
        &self,
        from: &str,
        to: &str,
        original: Option<&str>,
        language: Option<&str>,
        page: u32,
    ) -> Result<(Vec<models::MovieSummary>, bool)> {
        let url = format!("{}/discover/movie", self.base);
        let mut params = vec![
            ("primary_release_date.gte", from.to_string()),
            ("primary_release_date.lte", to.to_string()),
            ("sort_by", "popularity.desc".to_string()),
            ("include_adult", self.include_adult().to_string()),
            (
                "language",
                language.map_or_else(|| self.language(), String::from),
            ),
            ("page", page.to_string()),
        ];
        if let Some(original) = original {
            params.push(("with_original_language", original.to_string()));
        }

        let Some(value) = self.fetch(&url, &params).await? else {
            return Ok((Vec::new(), false));
        };

        let parsed: models::SearchResponse<models::MovieSummary> = Self::typed(&value, "discover")?;
        Ok((parsed.results, i64::from(page) < parsed.total_pages))
    }

    /// Ids changed since `start_date` (`YYYY-MM-DD`), used to drive refreshes.
    pub async fn changed_ids(&self, kind: MediaKind, start_date: &str) -> Result<Vec<i64>> {
        let segment = match kind {
            MediaKind::Series => "tv",
            MediaKind::Movie => "movie",
        };

        let url = format!("{}/{segment}/changes", self.base);
        let mut ids = Vec::new();
        let mut page = 1;

        loop {
            let params = [
                ("start_date", start_date.to_string()),
                ("page", page.to_string()),
            ];

            let Some(value) = self.fetch(&url, &params).await? else {
                break;
            };

            let parsed: models::ChangesResponse = Self::typed(&value, "changes")?;
            ids.extend(parsed.results.iter().map(|c| c.id));

            // TMDB caps this endpoint at 100 pages; stop well before that to
            // keep a single refresh tick bounded.
            if page >= parsed.total_pages || page >= 10 {
                break;
            }
            page += 1;
        }

        Ok(ids)
    }
}

/// Fill empty episode titles and overviews from an en-US copy of the season.
fn backfill_episodes(target: &mut models::Season, fallback: &models::Season) {
    use std::collections::HashMap;

    let by_number: HashMap<(i32, i32), &models::Episode> = fallback
        .episodes
        .iter()
        .map(|e| ((e.season_number, e.episode_number), e))
        .collect();

    for episode in &mut target.episodes {
        let Some(source) = by_number.get(&(episode.season_number, episode.episode_number)) else {
            continue;
        };

        if episode.name.as_deref().unwrap_or_default().is_empty() {
            episode.name = source.name.clone();
        }
        if episode.overview.as_deref().unwrap_or_default().is_empty() {
            episode.overview = source.overview.clone();
        }
    }
}

/// Absolute URL for a TMDB image path.
pub fn image_url(path: &str) -> String {
    format!("{IMAGE_BASE}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn season(episodes: Vec<(i32, i32, Option<&str>, Option<&str>)>) -> models::Season {
        models::Season {
            season_number: 1,
            episodes: episodes
                .into_iter()
                .map(|(s, e, name, overview)| models::Episode {
                    id: None,
                    season_number: s,
                    episode_number: e,
                    name: name.map(String::from),
                    overview: overview.map(String::from),
                    air_date: None,
                    runtime: None,
                    still_path: None,
                    vote_average: None,
                    vote_count: None,
                    episode_type: None,
                })
                .collect(),
        }
    }

    #[test]
    fn backfill_only_fills_empty_fields() {
        let mut target = season(vec![
            (1, 1, Some("Titre"), Some("")),
            (1, 2, Some(""), Some("Résumé")),
        ]);
        let fallback = season(vec![
            (1, 1, Some("Title"), Some("Summary")),
            (1, 2, Some("Title 2"), Some("Summary 2")),
        ]);

        backfill_episodes(&mut target, &fallback);

        assert_eq!(target.episodes[0].name.as_deref(), Some("Titre"));
        assert_eq!(target.episodes[0].overview.as_deref(), Some("Summary"));
        assert_eq!(target.episodes[1].name.as_deref(), Some("Title 2"));
        assert_eq!(target.episodes[1].overview.as_deref(), Some("Résumé"));
    }

    #[test]
    fn backfill_ignores_episodes_absent_from_the_fallback() {
        let mut target = season(vec![(1, 9, Some(""), None)]);
        let fallback = season(vec![(1, 1, Some("Title"), None)]);

        backfill_episodes(&mut target, &fallback);

        assert_eq!(target.episodes[0].name.as_deref(), Some(""));
    }
}
