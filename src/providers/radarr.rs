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
use bytes::Bytes;
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

/// As with Skyhook: the client speaks HTTP, the caller decides whether to.
pub struct RadarrMetadataClient {
    http: reqwest::Client,
    base: String,
    instance: String,
    gate: crate::providers::Gate,
}

/// How many requests to Radarr's service are in flight at once: a Radarr bulk
/// refresh of a hundred films asks for all of them at the same moment.
const AT_ONCE: usize = 8;

impl RadarrMetadataClient {
    pub fn new(http: reqwest::Client, cfg: &config::RadarrMetadata, instance: String) -> Self {
        Self {
            http,
            base: cfg.upstream.clone(),
            instance,
            gate: crate::providers::Gate::new("radarr", "Radarr's metadata service", AT_ONCE),
        }
    }

    /// One movie by TMDB id. Returns the raw body alongside the parsed one.
    pub async fn movie(&self, tmdb_id: i64) -> Result<Option<(Value, MovieResource)>> {
        let url = format!("{}/v1/movie/{tmdb_id}", self.base);

        let Some(value) = self.fetch(&url, &[]).await? else {
            return Ok(None);
        };

        // serde's message names the offending field and line, which is the
        // only thing that makes an upstream schema change diagnosable.
        let movie: MovieResource = serde_json::from_value(value.clone()).with_context(|| {
            format!(
                "Radarr's metadata service returned a movie this server could not interpret: {url}"
            )
        })?;

        Ok(Some((value, movie)))
    }

    pub async fn by_imdb_id(&self, imdb_id: &str) -> Result<Option<(Value, MovieResource)>> {
        let url = format!(
            "{}/v1/movie/imdb/{}",
            self.base,
            crate::providers::segment(imdb_id)
        );

        let Some(value) = self.fetch(&url, &[]).await? else {
            return Ok(None);
        };

        // This endpoint answers with an array even for a single hit.
        let movies: Vec<MovieResource> = serde_json::from_value(value.clone()).unwrap_or_default();

        Ok(movies.into_iter().next().map(|m| (value, m)))
    }

    pub async fn search(&self, term: &str, year: Option<i32>) -> Result<Vec<MovieResource>> {
        let url = format!("{}/v1/search", self.base);
        let year = year.map(|y| y.to_string()).unwrap_or_default();
        let query: Vec<(&str, &str)> = vec![("q", term), ("year", &year)];

        let Some(value) = self.fetch(&url, &query).await? else {
            return Ok(Vec::new());
        };

        Ok(serde_json::from_value(value).unwrap_or_default())
    }

    /// One of IMDb's lists as Radarr's metadata service compiles them —
    /// `top250`, `popular`, or a user's ratings by `ur…` id — as the bytes it
    /// came as, for a caller that reads it whole. `None` where the service
    /// has no such list. The Top 250 is thirteen megabytes of whole movie
    /// resources, so it is given longer than a movie to arrive — and read
    /// under the same ceiling as every provider answer, this host being one
    /// a deployment redirects by design.
    pub async fn imdb_list(&self, id: &str) -> Result<Option<Bytes>> {
        let url = format!(
            "{}/v1/list/imdb/{}",
            self.base,
            crate::providers::segment(id)
        );
        let Some((response, _permit)) = self.send(&url, &[], 90).await? else {
            return Ok(None);
        };
        crate::providers::read_body(response, crate::providers::MAX_BODY_BYTES)
            .await
            .map(|body| Some(Bytes::from(body)))
            .with_context(|| format!("Radarr's metadata service cut off its answer: {url}"))
    }

    async fn fetch(&self, url: &str, query: &[(&str, &str)]) -> Result<Option<Value>> {
        let Some((response, _permit)) = self.send(url, query, 20).await? else {
            return Ok(None);
        };

        crate::providers::read_json(response)
            .await
            .map(Some)
            .with_context(|| {
                format!(
                    "Radarr's metadata service returned a body this server could not read: {url}"
                )
            })
    }

    /// One request to the service, answered or not: `None` for a 404, an
    /// error for anything else that is not success. The permit is the
    /// request's place among those in flight, held while its body is read.
    async fn send(
        &self,
        url: &str,
        query: &[(&str, &str)],
        timeout_secs: u64,
    ) -> Result<Option<(reqwest::Response, tokio::sync::SemaphorePermit<'_>)>> {
        let (response, permit) = self
            .gate
            .send(|| {
                self.http
                    .get(url)
                    .query(query)
                    .header(LOOP_HEADER, &self.instance)
                    .timeout(std::time::Duration::from_secs(timeout_secs))
            })
            .await
            .map_err(|e| {
                anyhow::anyhow!("request to Radarr's metadata service failed: {url}: {e}")
            })?;

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
            let reason = crate::providers::error_text(response).await;
            anyhow::bail!("Radarr's metadata service returned {status} for {url}: {reason}");
        }

        Ok(Some((response, permit)))
    }
}
