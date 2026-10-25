//! Cursors: where the next page of a list begins.
//!
//! A list that grows while somebody reads it — the catalogue as works are
//! imported, the trail as things are done, a destination's log as it is told
//! things — is read better a page at a time after the last row read than at a
//! count of rows from its start: an offset shows a row twice when one is added
//! in front of it, and skips one when one goes. A cursor says where the page
//! before ended, in the list's own order: the value the list is sorted by and
//! the id of the last row, so the next page begins just past it whatever
//! changed meanwhile.
//!
//! To a caller it is opaque — base64url of a small JSON document — and it is
//! no secret: a cursor written by hand reads as a place in the list, or is
//! refused. Each list says what its own holds; this is how one is written and
//! read back.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Serialize, de::DeserializeOwned};

use crate::error::{AppError, AppResult};

/// The longest cursor read: a list's is a hundred characters or two, and one
/// much longer was not written by this server.
const LONGEST: usize = 2048;

/// A place in a list, written as a caller is given it.
pub fn encode<T: Serialize>(place: &T) -> String {
    // A place is a few strings and numbers: it always serializes.
    let json = serde_json::to_vec(place).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(json)
}

/// A place a caller gave back, or a refusal that says what to do instead.
pub fn decode<T: DeserializeOwned>(cursor: &str) -> AppResult<T> {
    let cursor = cursor.trim();
    if cursor.len() > LONGEST {
        return Err(not_ours());
    }
    let bytes = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| not_ours())?;
    serde_json::from_slice(&bytes).map_err(|_| not_ours())
}

/// The cursor a request gave, when it gave one that says something — and a
/// refusal when it gave an offset beside it, which would say where to start
/// twice.
pub fn given(cursor: Option<&str>, offset: Option<i64>) -> AppResult<Option<&str>> {
    let cursor = cursor.map(str::trim).filter(|c| !c.is_empty());
    if cursor.is_some() && offset.is_some_and(|o| o != 0) {
        return Err(AppError::BadRequest(
            "give a cursor or an offset, not both: a cursor already says where the page starts"
                .into(),
        ));
    }
    Ok(cursor)
}

/// The refusal of a cursor this server did not write, or wrote for a list in
/// another order.
pub fn not_ours() -> AppError {
    AppError::BadRequest(
        "cursor is not one this list gave: ask again without it, then follow nextCursor".into(),
    )
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Place {
        at: String,
        id: String,
    }

    #[test]
    fn a_place_comes_back_as_it_was_written() {
        let place = Place {
            at: "2026-10-25T06:00:00.000Z".into(),
            id: "01a17182-dc6a-7647-8b05-869160d8b0a8".into(),
        };
        let cursor = encode(&place);
        // Safe in an address as it is: nothing to escape.
        assert!(
            cursor
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "{cursor}"
        );
        assert_eq!(decode::<Place>(&cursor).unwrap(), place);
        assert_eq!(decode::<Place>(&format!(" {cursor} ")).unwrap(), place);
    }

    #[test]
    fn a_cursor_this_server_did_not_write_is_refused() {
        for written in [
            "nonsense!",
            "e30",                         // `{}`: no place in it
            &URL_SAFE_NO_PAD.encode("[]"), // not a place either
            &"A".repeat(LONGEST + 1),
        ] {
            assert!(
                matches!(decode::<Place>(written), Err(AppError::BadRequest(_))),
                "{written}"
            );
        }
    }

    #[test]
    fn a_cursor_and_an_offset_are_not_given_together() {
        assert_eq!(given(None, None).unwrap(), None);
        assert_eq!(given(Some("  "), Some(40)).unwrap(), None);
        assert_eq!(given(Some("abc"), None).unwrap(), Some("abc"));
        assert_eq!(given(Some("abc"), Some(0)).unwrap(), Some("abc"));
        assert!(matches!(
            given(Some("abc"), Some(20)),
            Err(AppError::BadRequest(_))
        ));
    }
}
