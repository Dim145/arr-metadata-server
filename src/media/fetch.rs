//! Fetching a provider's file, carefully: it is an address somebody else
//! chose, and it is followed from inside the network this server is on.

use anyhow::Context;
use bytes::Bytes;

use crate::{db::repo::asset::Kind, state::AppState};

use super::file::{self, Inspected};

/// What came back, once looked at.
pub struct Fetched {
    pub bytes: Bytes,
    pub inspected: Inspected,
}

/// Why a fetch did not end in a file kept.
#[derive(Debug)]
pub enum Failure {
    /// The address is not one to try again: not one this server fetches,
    /// or one that answers with something it does not keep.
    Unfit(String),
    /// This time: a provider down, a timeout, a 5xx.
    Passing(anyhow::Error),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unfit(why) => f.write_str(why),
            Self::Passing(e) => write!(f, "{e:#}"),
        }
    }
}

/// Whether an address is one this server would fetch at all: a web address,
/// not one of its own, and not a kind of file it never keeps. TMDB files
/// some networks' logos as SVG, which this server does not serve.
pub fn fetchable(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && super::Media::key_in(url).is_none()
        && !crate::outbound::names_internal_host(url)
        && !url
            .split(['?', '#'])
            .next()
            .unwrap_or(url)
            .to_ascii_lowercase()
            .ends_with(".svg")
}

/// Fetch a file and make sure it is what was wanted.
pub async fn download(state: &AppState, url: &str, kind: Kind) -> Result<Fetched, Failure> {
    if !fetchable(url) {
        return Err(Failure::Unfit("not an address this server fetches".into()));
    }
    if crate::outbound::resolves_internally(url).await {
        return Err(Failure::Unfit(
            "that address is not reachable from anywhere but this server".into(),
        ));
    }

    let accept = match kind {
        Kind::Image => "image/*;q=1.0, */*;q=0.1",
        Kind::Audio => "audio/*;q=1.0, */*;q=0.1",
    };
    let response = state
        .media
        .http
        .get(url)
        .header(reqwest::header::ACCEPT, accept)
        .send()
        .await
        .context("could not reach the address")
        .map_err(Failure::Passing)?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
        return Err(Failure::Unfit(format!("the address answered {status}")));
    }
    if !status.is_success() {
        return Err(Failure::Passing(anyhow::anyhow!(
            "the address answered {status}"
        )));
    }

    let limit = match kind {
        Kind::Image => file::MAX_IMAGE_BYTES,
        Kind::Audio => file::MAX_AUDIO_BYTES,
    };
    let body = crate::providers::read_body(response, limit)
        .await
        .map_err(Failure::Passing)?;
    if body.is_empty() {
        return Err(Failure::Passing(anyhow::anyhow!(
            "the address answered nothing"
        )));
    }

    let inspected = file::inspect(&body, kind).map_err(|e| Failure::Unfit(e.to_string()))?;
    Ok(Fetched {
        bytes: Bytes::from(body),
        inspected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_web_address_that_is_not_ours_is_fetched() {
        assert!(fetchable("https://image.tmdb.org/t/p/original/a.jpg"));
        assert!(fetchable(
            "http://metadata.fankai.fr/series/33/image/poster"
        ));
        assert!(!fetchable("upload:0123"));
        assert!(!fetchable("/media/x.jpg"));
        assert!(!fetchable(
            "https://ams.example/media/ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.jpg"
        ));
        assert!(!fetchable("http://127.0.0.1:8479/x.jpg"));
        assert!(!fetchable("ftp://a/b.jpg"));
        assert!(!fetchable("https://image.tmdb.org/t/p/original/logo.svg"));
        assert!(!fetchable(
            "https://image.tmdb.org/t/p/original/logo.SVG?x=1"
        ));
    }
}
