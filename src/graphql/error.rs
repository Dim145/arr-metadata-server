//! What a document that cannot be answered is answered with, in AniList's
//! words: every error with its message, the HTTP status AniList answers it
//! with, and where in the document it is — and `data` null, or what the
//! fields could be given.
//!
//! Read off AniList's own answers on 2 November 2026: a document that does
//! not parse, a field its schema does not have, a fragment never defined, a
//! variable required and not given — `400` each, `data: null` — and an entry
//! that does not exist, `404`, its field null in `data`. A variable not given
//! is the one error AniList says nowhere, without `locations`.

use serde_json::{Map, Value, json};

/// A place in the document, as AniList counts it: from one, the line first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Location {
    pub line: usize,
    pub column: usize,
}

impl From<graphql_parser::Pos> for Location {
    fn from(at: graphql_parser::Pos) -> Self {
        Self {
            line: at.line,
            column: at.column,
        }
    }
}

/// One error, as AniList writes it among `errors`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    /// The HTTP status AniList answers with, which it writes beside the
    /// message too.
    pub status: u16,
    /// Where in the document; none for what is nowhere in it.
    pub locations: Vec<Location>,
    /// Whether the document is past what this server reads, rather than
    /// wrong: AniList's to answer, never refused here.
    pub beyond: bool,
}

impl Error {
    /// A document AniList would not run: one that does not parse, or asks
    /// in a way its schema does not allow.
    pub fn invalid(message: impl Into<String>, at: impl IntoIterator<Item = Location>) -> Self {
        Self {
            message: message.into(),
            status: 400,
            locations: at.into_iter().collect(),
            beyond: false,
        }
    }

    /// A document longer, deeper or wider than this server reads, which
    /// AniList may well run: left to it.
    pub fn beyond(message: impl Into<String>, at: impl IntoIterator<Item = Location>) -> Self {
        Self {
            beyond: true,
            ..Self::invalid(message, at)
        }
    }

    /// The error as AniList writes it: its message, its status, and its
    /// locations when it has any.
    pub fn to_json(&self) -> Value {
        let mut error = Map::new();
        error.insert("message".into(), Value::String(self.message.clone()));
        error.insert("status".into(), Value::from(self.status));
        if !self.locations.is_empty() {
            error.insert(
                "locations".into(),
                Value::Array(
                    self.locations
                        .iter()
                        .map(|at| json!({ "line": at.line, "column": at.column }))
                        .collect(),
                ),
            );
        }
        Value::Object(error)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// The body of an answer with errors, as AniList writes it: `errors` first,
/// then `data` — null when the document could not be run at all, or each
/// field asked with what it could be given, null where it could not.
pub fn body(errors: &[Error], data: Option<Map<String, Value>>) -> Value {
    json!({
        "errors": errors.iter().map(Error::to_json).collect::<Vec<_>>(),
        "data": data.map_or(Value::Null, Value::Object),
    })
}

/// The HTTP status an answer with errors goes out with: its first error's,
/// as AniList's do.
pub fn status(errors: &[Error]) -> u16 {
    errors.first().map_or(200, |error| error.status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_is_written_as_anilist_writes_it() {
        // A field AniList's schema does not have, on 2 November 2026.
        let unknown = Error::invalid(
            "Cannot query field \"idd\" on type \"Media\".",
            [Location { line: 3, column: 5 }],
        );
        assert_eq!(
            body(std::slice::from_ref(&unknown), None),
            json!({
                "errors": [{
                    "message": "Cannot query field \"idd\" on type \"Media\".",
                    "status": 400,
                    "locations": [{ "line": 3, "column": 5 }]
                }],
                "data": null
            })
        );
        assert_eq!(status(&[unknown]), 400);

        // A variable not given is nowhere in the document: no locations.
        let missing = Error::invalid(
            "Variable \"$id\" of required type \"Int!\" was not provided.",
            [],
        );
        assert_eq!(
            missing.to_json(),
            json!({
                "message": "Variable \"$id\" of required type \"Int!\" was not provided.",
                "status": 400
            })
        );
        // The keys in AniList's order: message, status, locations.
        let keys: Vec<String> = unknown_keys(&not_found());
        assert_eq!(keys, ["message", "status", "locations"]);
    }

    fn unknown_keys(error: &Error) -> Vec<String> {
        error
            .to_json()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    /// AniList's answer for an entry that does not exist, on 2 November
    /// 2026: what a mode answering without AniList would give.
    fn not_found() -> Error {
        Error {
            message: "Not Found.".into(),
            status: 404,
            locations: vec![Location { line: 1, column: 3 }],
            beyond: false,
        }
    }

    #[test]
    fn an_entry_that_does_not_exist_is_a_404_with_its_field_null() {
        let mut data = Map::new();
        data.insert("Media".into(), Value::Null);
        let errors = [not_found()];
        assert_eq!(
            body(&errors, Some(data)),
            json!({
                "errors": [{ "message": "Not Found.", "status": 404, "locations": [{ "line": 1, "column": 3 }] }],
                "data": { "Media": null }
            })
        );
        assert_eq!(status(&errors), 404);
        assert_eq!(status(&[]), 200);
    }
}
