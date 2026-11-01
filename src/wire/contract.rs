//! Whether a document written here in another service's shape is shaped as
//! that service's own: every key the service writes is there, holding a
//! value of the same JSON type, and no key the service never sends. The
//! clients read strictly — Jellyfin's TMDbLib, Kodi's scrapers — and one
//! key missing, or of another type, breaks the whole document for them.
//!
//! A list is told by its entries: each of ours against the service's first.
//! An empty list on either side says nothing about them. An integer stands
//! where the service writes a number with a fraction, never the other way:
//! a reader that wants a whole number refuses `7.5`.

use serde_json::Value;

/// One way a document differs from the service's own, and where: a path
/// such as `credits.cast[].character`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gap {
    pub at: String,
    pub what: GapKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GapKind {
    /// The service writes this key; ours does not.
    Missing,
    /// Ours writes a key the service never sends.
    Extra,
    /// Ours writes `null` where the service wrote a value: right only where
    /// the service writes `null` too when it has nothing, which one sample
    /// cannot tell — a test allows it path by path.
    Nulled { theirs: &'static str },
    /// Both write it, as different JSON types.
    Type {
        theirs: &'static str,
        ours: &'static str,
    },
}

/// How `ours` differs in shape from `theirs`, the service's own document
/// for the same request, leaving out the paths `allowed` names: a path as
/// [`Gap::at`] writes it, or a prefix of one ending in `*` (`images.*`).
pub fn gaps(theirs: &Value, ours: &Value, allowed: &[&str]) -> Vec<Gap> {
    let mut found = Vec::new();
    walk(theirs, ours, "", &mut found);
    found.retain(|gap| !allowed.iter().any(|pattern| matches(pattern, &gap.at)));
    found
}

/// Fails the test with every gap listed, when there is any.
#[track_caller]
pub fn assert_same_shape(theirs: &Value, ours: &Value, allowed: &[&str]) {
    let found = gaps(theirs, ours, allowed);
    assert!(
        found.is_empty(),
        "{} gap(s) from the service's shape:\n{}",
        found.len(),
        found
            .iter()
            .map(|gap| format!("  {}: {:?}", gap.at, gap.what))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn matches(pattern: &str, at: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => at.starts_with(prefix),
        None => pattern == at,
    }
}

fn type_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn joined(at: &str, key: &str) -> String {
    if at.is_empty() {
        key.to_string()
    } else {
        format!("{at}.{key}")
    }
}

fn walk(theirs: &Value, ours: &Value, at: &str, found: &mut Vec<Gap>) {
    match (theirs, ours) {
        // Nothing to tell a value by, when the service wrote none.
        (Value::Null, _) => {}
        (_, Value::Null) => found.push(Gap {
            at: at.to_string(),
            what: GapKind::Nulled {
                theirs: type_of(theirs),
            },
        }),
        (Value::Object(theirs), Value::Object(ours)) => {
            for (key, value) in theirs {
                match ours.get(key) {
                    Some(mine) => walk(value, mine, &joined(at, key), found),
                    None => found.push(Gap {
                        at: joined(at, key),
                        what: GapKind::Missing,
                    }),
                }
            }
            for key in ours.keys().filter(|key| !theirs.contains_key(*key)) {
                found.push(Gap {
                    at: joined(at, key),
                    what: GapKind::Extra,
                });
            }
        }
        (Value::Array(theirs), Value::Array(ours)) => {
            if let Some(first) = theirs.first() {
                let at = format!("{at}[]");
                for mine in ours {
                    walk(first, mine, &at, found);
                }
            }
        }
        // An integer is a number too; a fraction is no integer.
        (Value::Number(_), Value::Number(_))
            if !(type_of(theirs) == "integer" && type_of(ours) == "number") => {}
        _ if type_of(theirs) == type_of(ours) => {}
        _ => found.push(Gap {
            at: at.to_string(),
            what: GapKind::Type {
                theirs: type_of(theirs),
                ours: type_of(ours),
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_same_shape_has_no_gap() {
        let theirs = json!({ "id": 603, "title": "The Matrix", "vote_average": 8.2,
            "genres": [{ "id": 28, "name": "Action" }], "belongs_to_collection": null });
        let ours = json!({ "id": 1_000_000_123, "title": "Matrix", "vote_average": 8,
            "genres": [{ "id": 28, "name": "Action" }, { "id": 878, "name": "Science" }],
            "belongs_to_collection": { "id": 2344 } });
        assert_same_shape(&theirs, &ours, &[]);
    }

    #[test]
    fn a_key_missing_extra_nulled_or_of_another_type_is_told_where() {
        let theirs = json!({ "id": 603, "credits": { "cast": [{ "order": 0, "character": "Neo" }] },
            "runtime": 136, "status": "Released" });
        let ours = json!({ "id": 603.5, "credits": { "cast": [{ "character": 1 }] },
            "runtime": null, "tagline": "" });
        let found = gaps(&theirs, &ours, &[]);
        let at = |path: &str| found.iter().find(|gap| gap.at == path).map(|gap| &gap.what);
        assert_eq!(
            at("id"),
            Some(&GapKind::Type {
                theirs: "integer",
                ours: "number"
            })
        );
        assert_eq!(at("credits.cast[].order"), Some(&GapKind::Missing));
        assert_eq!(
            at("credits.cast[].character"),
            Some(&GapKind::Type {
                theirs: "string",
                ours: "integer"
            })
        );
        assert_eq!(at("runtime"), Some(&GapKind::Nulled { theirs: "integer" }));
        assert_eq!(at("status"), Some(&GapKind::Missing));
        assert_eq!(at("tagline"), Some(&GapKind::Extra));
        assert_eq!(found.len(), 6);

        let left = gaps(
            &theirs,
            &ours,
            &["runtime", "credits.*", "tagline", "status", "id"],
        );
        assert!(left.is_empty(), "{left:?}");
    }
}
