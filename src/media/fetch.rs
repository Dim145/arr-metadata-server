//! Fetching a provider's file, carefully: it is an address somebody else
//! chose, and it is followed from inside the network this server is on.

use anyhow::Context;
use bytes::Bytes;

use crate::{db::repo::asset::Kind, outbound::Guard, state::AppState};

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

/// How each failure begins, as it is written on the row: what an
/// administrator reads whole, and what [`reason_for_writers`] reads the
/// kind of trouble from.
const REFUSED: &str = "not an address this server fetches";
const REFUSED_NOW: &str = "that address is not reachable from anywhere but this server";
const UNREACHED: &str = "could not reach the address";
const ANSWERED_NOTHING: &str = "the address answered nothing";
const ANSWERED: &str = "the address answered";
const UNREAD: &str = "could not read the answer";
const NOT_KEPT: &str = "not kept:";

/// Whether an address is one this server would fetch at all: a web address,
/// not one of its own, not one the guard refuses on sight, and not a kind of
/// file it never keeps. TMDB files some networks' logos as SVG, which this
/// server does not serve.
pub fn fetchable(url: &str, guard: Guard) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && super::Media::key_in(url).is_none()
        && url::Url::parse(url).is_ok_and(|parsed| guard.refusal(&parsed).is_none())
        && !url
            .split(['?', '#'])
            .next()
            .unwrap_or(url)
            .to_ascii_lowercase()
            .ends_with(".svg")
}

/// Fetch a file and make sure it is what was wanted.
pub async fn download(state: &AppState, url: &str, kind: Kind) -> Result<Fetched, Failure> {
    let guard = state.media.guard;
    if !fetchable(url, guard) {
        return Err(Failure::Unfit(REFUSED.into()));
    }
    if guard.refuses_url(url).await {
        return Err(Failure::Unfit(REFUSED_NOW.into()));
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
        .context(UNREACHED)
        .map_err(Failure::Passing)?;
    // The body of an answer that is not the file is never read: nothing in
    // it is wanted, and it is not bounded the way the file is.
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
        return Err(Failure::Unfit(format!("{ANSWERED} {status}")));
    }
    if !status.is_success() {
        return Err(Failure::Passing(anyhow::anyhow!("{ANSWERED} {status}")));
    }

    let limit = match kind {
        Kind::Image => file::MAX_IMAGE_BYTES,
        Kind::Audio => file::MAX_AUDIO_BYTES,
    };
    let body = crate::providers::read_body(response, limit)
        .await
        .context(UNREAD)
        .map_err(Failure::Passing)?;
    if body.is_empty() {
        return Err(Failure::Passing(anyhow::anyhow!("{ANSWERED_NOTHING}")));
    }

    let inspected =
        file::inspect(&body, kind).map_err(|e| Failure::Unfit(format!("{NOT_KEPT} {e}")))?;
    Ok(Fetched {
        bytes: Bytes::from(body),
        inspected,
    })
}

/// What a writer who is not an administrator is told of a failure: the
/// kind of trouble, and nothing of what the address answered. A status, a
/// content type or a connection refused, read back for any address one
/// cares to type in, is a map of whatever network that address is in.
pub fn reason_for_writers(error: &str) -> &'static str {
    const REASONS: &[(&str, &str)] = &[
        (REFUSED, REFUSED),
        (REFUSED_NOW, REFUSED),
        (UNREACHED, "the address could not be reached"),
        (ANSWERED_NOTHING, ANSWERED_NOTHING),
        (ANSWERED, "the address answered with an error"),
        (UNREAD, "the answer could not be read whole"),
        (NOT_KEPT, "not a picture or a sound this server keeps"),
        (
            crate::db::repo::asset::CUT_SHORT,
            crate::db::repo::asset::CUT_SHORT,
        ),
        (
            crate::db::repo::asset::GIVEN_UP,
            crate::db::repo::asset::GIVEN_UP,
        ),
    ];
    REASONS
        .iter()
        .find(|(start, _)| error.starts_with(start))
        .map_or("the file could not be kept", |(_, shown)| shown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_web_address_that_is_not_ours_is_fetched() {
        let guard = Guard::default();
        assert!(fetchable(
            "https://image.tmdb.org/t/p/original/a.jpg",
            guard
        ));
        assert!(fetchable(
            "http://metadata.fankai.fr/series/33/image/poster",
            guard
        ));
        assert!(fetchable("http://192.168.1.10/posters/a.jpg", guard));
        assert!(!fetchable("upload:0123", guard));
        assert!(!fetchable("/media/x.jpg", guard));
        assert!(!fetchable(
            "https://ams.example/media/ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.jpg",
            guard
        ));
        assert!(!fetchable("http://127.0.0.1:8479/x.jpg", guard));
        assert!(!fetchable("http://[::ffff:169.254.169.254]/x.jpg", guard));
        assert!(!fetchable("http://100.100.100.200/x.jpg", guard));
        assert!(!fetchable("ftp://a/b.jpg", guard));
        assert!(!fetchable(
            "https://image.tmdb.org/t/p/original/logo.svg",
            guard
        ));
        assert!(!fetchable(
            "https://image.tmdb.org/t/p/original/logo.SVG?x=1",
            guard
        ));

        // A home network, when the operator keeps the server out of them.
        let walled = Guard {
            private_networks: false,
        };
        assert!(!fetchable("http://192.168.1.10/posters/a.jpg", walled));
        assert!(fetchable(
            "https://image.tmdb.org/t/p/original/a.jpg",
            walled
        ));
    }

    #[test]
    fn a_writer_is_told_the_kind_of_trouble_and_nothing_it_answered() {
        for (written, shown) in [
            (
                "could not reach the address: error sending request for url \
                 (http://192.168.1.7:8080/x.jpg): tcp connect error: Connection refused",
                "the address could not be reached",
            ),
            (
                "the address answered 401 Unauthorized",
                "the address answered with an error",
            ),
            (
                "the address answered 404 Not Found",
                "the address answered with an error",
            ),
            (
                "the address answered nothing",
                "the address answered nothing",
            ),
            (
                "not kept: text/html is not a picture or a sound this server keeps",
                "not a picture or a sound this server keeps",
            ),
            (
                "that address is not reachable from anywhere but this server",
                "not an address this server fetches",
            ),
            (
                "could not read the answer: the answer is larger than 26214400 bytes",
                "the answer could not be read whole",
            ),
            (
                crate::db::repo::asset::CUT_SHORT,
                crate::db::repo::asset::CUT_SHORT,
            ),
            // Written before the kinds were: nothing of it is repeated.
            (
                "text/html is not a picture or a sound this server keeps",
                "the file could not be kept",
            ),
            (
                "could not store abc.jpg: Generic S3 error: bucket ams-media",
                "the file could not be kept",
            ),
        ] {
            assert_eq!(reason_for_writers(written), shown, "{written}");
        }
    }
}
