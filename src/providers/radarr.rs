//! Radarr's own metadata service, used as a provider.
//!
//! `api.radarr.video` is a curated view of TMDB with certifications, extra
//! ratings and alternative titles already resolved — the work Radarr's
//! maintainers do so Radarr does not have to. Taking it as a second opinion
//! costs one call and fills gaps TMDB leaves.
//!
//! It is also, of course, one of the hostnames this server impersonates, so
//! calling it carries a risk of calling ourselves. See [`LOOP_HEADER`].

use anyhow::{Context, Result};
use serde_json::Value;

use crate::{config, wire::radarr::MovieResource};

/// Marks an outbound request to a hostname we also answer on.
///
/// Redirecting `api.radarr.video` at this server is the documented way to put it
/// in front of Radarr. If that redirect is done at the resolver rather than per
/// container, this server resolves the same name and calls itself. The arr
/// surface refuses a request carrying this header with its own instance id,
/// which turns an unbounded recursion into one clear error.
pub const LOOP_HEADER: &str = "x-ams-instance";

pub struct RadarrMetadataClient {
    http: reqwest::Client,
    base: String,
    enabled: bool,
    enrich: bool,
    instance: String,
}

impl RadarrMetadataClient {
    pub fn new(http: reqwest::Client, cfg: &config::RadarrMetadata, instance: String) -> Self {
        Self {
            http,
            base: cfg.upstream.clone(),
            enabled: cfg.fallback,
            enrich: cfg.enrich,
            instance,
        }
    }

    /// Whether it may be used at all.
    pub fn is_enabled(&self) -> bool {
        self.enabled || self.enrich
    }

    /// Whether every movie should be enriched with it, not just the ones
    /// nothing else could answer.
    pub fn enriches(&self) -> bool {
        self.enrich
    }

    /// One movie by TMDB id. Returns the raw body alongside the parsed one.
    pub async fn movie(&self, tmdb_id: i64) -> Result<Option<(Value, MovieResource)>> {
        if !self.is_enabled() {
            return Ok(None);
        }

        let url = format!("{}/v1/movie/{tmdb_id}", self.base);

        let Some(value) = self.fetch(&url, &[]).await? else {
            return Ok(None);
        };

        let movie: MovieResource = serde_json::from_value(value.clone()).context(
            "Radarr's metadata service returned a movie this server could not interpret",
        )?;

        Ok(Some((value, movie)))
    }

    pub async fn by_imdb_id(&self, imdb_id: &str) -> Result<Option<(Value, MovieResource)>> {
        if !self.is_enabled() {
            return Ok(None);
        }

        let url = format!("{}/v1/movie/imdb/{imdb_id}", self.base);

        let Some(value) = self.fetch(&url, &[]).await? else {
            return Ok(None);
        };

        // This endpoint answers with an array even for a single hit.
        let movies: Vec<MovieResource> = serde_json::from_value(value.clone()).unwrap_or_default();

        Ok(movies.into_iter().next().map(|m| (value, m)))
    }

    pub async fn search(&self, term: &str, year: Option<i32>) -> Result<Vec<MovieResource>> {
        if !self.is_enabled() {
            return Ok(Vec::new());
        }

        let url = format!("{}/v1/search", self.base);
        let year = year.map(|y| y.to_string()).unwrap_or_default();
        let query: Vec<(&str, &str)> = vec![("q", term), ("year", &year)];

        let Some(value) = self.fetch(&url, &query).await? else {
            return Ok(Vec::new());
        };

        Ok(serde_json::from_value(value).unwrap_or_default())
    }

    async fn fetch(&self, url: &str, query: &[(&str, &str)]) -> Result<Option<Value>> {
        let response = self
            .http
            .get(url)
            .query(query)
            .header(LOOP_HEADER, &self.instance)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
            .with_context(|| format!("request to Radarr's metadata service failed: {url}"))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        // Our own arr surface answers this when a request loops back.
        if response.status() == reqwest::StatusCode::LOOP_DETECTED {
            anyhow::bail!(
                "api.radarr.video resolves to this server — set AMS_RADARR_METADATA_UPSTREAM \
                 to somewhere else, or turn the provider off"
            );
        }

        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("Radarr's metadata service returned {status} for {url}");
        }

        response.json::<Value>().await.map(Some).with_context(|| {
            format!("Radarr's metadata service returned a malformed body for {url}")
        })
    }
}
