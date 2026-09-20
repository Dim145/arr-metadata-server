//! Fallback to the real Skyhook.
//!
//! Used when this server cannot answer from its own store or from TMDB — most
//! often for a TVDB-only series, or one whose TVDB↔TMDB link is missing. The
//! response is absorbed into the canonical model, so the next request is served
//! locally and the entry becomes editable like any other.

use anyhow::{Context, Result};
use serde_json::Value;

use crate::{config, wire::sonarr::ShowResource};

pub struct SkyhookClient {
    http: reqwest::Client,
    base: String,
    enabled: bool,
}

impl SkyhookClient {
    pub fn new(http: reqwest::Client, cfg: &config::Skyhook) -> Self {
        Self {
            http,
            base: cfg.upstream.clone(),
            enabled: cfg.fallback,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// One show by TVDB id. Returns the raw body alongside the parsed one so the
    /// caller can snapshot it.
    pub async fn show(&self, language: &str, tvdb_id: i64) -> Result<Option<(Value, ShowResource)>> {
        if !self.enabled {
            return Ok(None);
        }

        let url = format!("{}/v1/tvdb/shows/{language}/{tvdb_id}", self.base);

        let Some(value) = self.fetch(&url, &[]).await? else {
            return Ok(None);
        };

        let show: ShowResource = serde_json::from_value(value.clone())
            .context("Skyhook returned a show this server could not interpret")?;

        Ok(Some((value, show)))
    }

    pub async fn search(&self, language: &str, term: &str) -> Result<Vec<ShowResource>> {
        if !self.enabled {
            return Ok(Vec::new());
        }

        let url = format!("{}/v1/tvdb/search/{language}", self.base);

        let Some(value) = self.fetch(&url, &[("term", term)]).await? else {
            return Ok(Vec::new());
        };

        // A single 404-ish payload or an unexpected shape is an empty result,
        // not an error: the caller has already tried everything else.
        Ok(serde_json::from_value(value).unwrap_or_default())
    }

    async fn fetch(&self, url: &str, query: &[(&str, &str)]) -> Result<Option<Value>> {
        let response = self
            .http
            .get(url)
            .query(query)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
            .with_context(|| format!("Skyhook request failed: {url}"))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("Skyhook returned {status} for {url}");
        }

        response
            .json::<Value>()
            .await
            .map(Some)
            .with_context(|| format!("Skyhook returned a malformed body for {url}"))
    }
}
