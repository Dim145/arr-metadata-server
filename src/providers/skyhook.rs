//! Fallback to the real Skyhook.
//!
//! Used when this server cannot answer from its own store or from TMDB — most
//! often for a TVDB-only series, or one whose TVDB↔TMDB link is missing. The
//! response is absorbed into the canonical model, so the next request is served
//! locally and the entry becomes editable like any other.

use anyhow::{Context, Result};
use serde_json::Value;

use crate::{config, wire::sonarr::ShowResource};

/// The only language Skyhook answers in.
///
/// Not a simplification — it is all it accepts. `en-US`, `fr` and `es` are each
/// a 400 or a 404 on both its endpoints, so passing the caller's language
/// through meant Skyhook silently contributing nothing to every request that
/// was not made in exactly `en`. What it is here for is structural anyway:
/// absolute numbering, air-order hints, ids. The prose is translated by the
/// language overlay, from providers that do speak other languages.
const LANGUAGE: &str = "en";

/// The client only speaks HTTP. Whether it is spoken to at all is a setting,
/// read by the caller — keeping the switch in one place rather than half here
/// and half there, which is how `skyhook.fallback` came to be a setting that
/// changed nothing.
pub struct SkyhookClient {
    http: reqwest::Client,
    base: String,
    instance: String,
    gate: crate::providers::Gate,
}

/// How many requests to Skyhook are in flight at once: a Sonarr library
/// refresh asks for every series at the same moment.
const AT_ONCE: usize = 8;

impl SkyhookClient {
    pub fn new(http: reqwest::Client, cfg: &config::Skyhook, instance: String) -> Self {
        Self {
            http,
            base: cfg.upstream.clone(),
            instance,
            gate: crate::providers::Gate::new("skyhook", "Skyhook", AT_ONCE),
        }
    }

    /// One show by TVDB id. Returns the raw body alongside the parsed one so the
    /// caller can snapshot it.
    pub async fn show(&self, tvdb_id: i64) -> Result<Option<(Value, ShowResource)>> {
        let url = format!("{}/v1/tvdb/shows/{LANGUAGE}/{tvdb_id}", self.base);

        let Some(value) = self.fetch(&url, &[]).await? else {
            return Ok(None);
        };

        let show: ShowResource = serde_json::from_value(value.clone())
            .context("Skyhook returned a show this server could not interpret")?;

        Ok(Some((value, show)))
    }

    pub async fn search(&self, term: &str) -> Result<Vec<ShowResource>> {
        let url = format!("{}/v1/tvdb/search/{LANGUAGE}", self.base);

        let Some(value) = self.fetch(&url, &[("term", term)]).await? else {
            return Ok(Vec::new());
        };

        // A single 404-ish payload or an unexpected shape is an empty result,
        // not an error: the caller has already tried everything else. A show
        // it names nothing would be a result with nothing to pick it by.
        let shows: Vec<ShowResource> = serde_json::from_value(value).unwrap_or_default();
        Ok(shows
            .into_iter()
            .filter(|show| !show.title.trim().is_empty())
            .collect())
    }

    async fn fetch(&self, url: &str, query: &[(&str, &str)]) -> Result<Option<Value>> {
        let (response, _permit) = self
            .gate
            .send(|| {
                self.http
                    .get(url)
                    .query(query)
                    // See `providers::radarr::LOOP_HEADER`: this hostname is one
                    // we also answer on, so a resolver-level redirect would have
                    // us call ourselves.
                    .header(crate::providers::radarr::LOOP_HEADER, &self.instance)
                    .timeout(std::time::Duration::from_secs(20))
            })
            .await
            .map_err(|e| anyhow::anyhow!("Skyhook request failed: {url}: {e}"))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if response.status() == reqwest::StatusCode::LOOP_DETECTED {
            anyhow::bail!(
                "skyhook.sonarr.tv resolves to this server — set AMS_SKYHOOK_UPSTREAM to \
                 somewhere else, or turn the provider off"
            );
        }

        let status = response.status();
        if !status.is_success() {
            let reason = crate::providers::error_text(response).await;
            anyhow::bail!("Skyhook returned {status} for {url}: {reason}");
        }

        crate::providers::read_json(response)
            .await
            .map(Some)
            .with_context(|| format!("Skyhook returned a body this server could not read: {url}"))
    }
}
