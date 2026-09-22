//! Upstream metadata providers.
//!
//! Every provider produces two things: a raw payload, stored verbatim as a
//! snapshot, and a canonical [`crate::domain::MediaItem`] mapped from it. Storing
//! both means a mapping bug can be fixed and replayed without re-fetching.

pub mod fanart;
pub mod lang;
pub mod radarr;
pub mod skyhook;
pub mod tmdb;
pub mod tvdb;

/// The most of one provider answer this server will read into memory.
///
/// Generous — TheTVDB's extended document for a long-running anime, with every
/// translation attached, runs to a few megabytes — and finite, which the reads
/// it replaces were not. Every upstream here is a URL an operator can point
/// somewhere else, and a redirected one that answers with a gigabyte was
/// buffered whole and mapped into a row per element.
pub const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

/// Read a provider response as JSON, refusing one that will not fit.
///
/// Streamed rather than trusting `Content-Length`, which a chunked answer does
/// not carry and a careless one can understate.
pub async fn read_json(response: reqwest::Response) -> anyhow::Result<serde_json::Value> {
    use futures::StreamExt as _;

    if response
        .content_length()
        .is_some_and(|n| n > MAX_BODY_BYTES)
    {
        anyhow::bail!("the answer is larger than {MAX_BODY_BYTES} bytes");
    }

    let mut body = Vec::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;

        if body.len() as u64 + chunk.len() as u64 > MAX_BODY_BYTES {
            anyhow::bail!("the answer is larger than {MAX_BODY_BYTES} bytes");
        }

        body.extend_from_slice(&chunk);
    }

    Ok(serde_json::from_slice(&body)?)
}

/// Names used in `media_provider_snapshot.provider` and in merge priority.
pub mod names {
    pub const TMDB: &str = "tmdb";
    pub const SKYHOOK: &str = "skyhook";
    pub const RADARR: &str = "radarr";
    pub const FANART: &str = "fanart";
    pub const TVDB: &str = "tvdb";
}
