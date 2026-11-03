//! GraphQL as AniList's clients send it: a query read, and its selection laid
//! over a document written here (wave P, P1).
//!
//! What a client posts — a document, its variables, the operation to run —
//! is read here as GraphQL reads it ([`Prepared`]): the operation chosen, its
//! variables given the request's values or their defaults, its named and
//! inline fragments followed, its aliases, `@include` and `@skip`. What the
//! operation asks is then laid over objects written here ([`Projection`]):
//! each knows its own fields, as AniList's schema names them, and takes the
//! arguments that change what a field holds — a description as HTML, a
//! relation in its third version, a page of a list. No schema is held here,
//! and nothing is executed beyond that projection.
//!
//! What AniList's schema would refuse is refused in AniList's own words: the
//! message, the status AniList answers with, where in the document, and
//! `data` null ([`Error`]). What an object cannot answer as AniList would —
//! an identifier only AniList gives, what an account holds — leaves the whole
//! document to AniList ([`Failure::Unanswerable`]): an answer written here is
//! whole or it is not written.

mod document;
mod error;
mod execute;

use serde_json::{Map, Value};

pub use document::{Field, OperationKind, Prepared};
pub use error::{body, status};
pub use execute::{Arguments, Failure, FieldError, Object, Projection, Resolved};

/// A GraphQL request, as a client sends it: a document, its variables, and
/// which of its operations to run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub query: String,
    pub variables: Map<String, Value>,
    pub operation_name: Option<String>,
}

impl Request {
    /// A request posted as JSON: `query`, and `variables` and
    /// `operationName` when there are any. Variables may come as an object
    /// or as the JSON text of one, as some clients write them.
    pub fn from_body(body: &[u8]) -> Option<Self> {
        let Value::Object(mut posted) = serde_json::from_slice(body).ok()? else {
            return None;
        };
        let Some(Value::String(query)) = posted.remove("query") else {
            return None;
        };
        Some(Self {
            query,
            variables: variables(posted.remove("variables"))?,
            operation_name: match posted.remove("operationName") {
                Some(Value::String(name)) => Some(name),
                None | Some(Value::Null) => None,
                Some(_) => return None,
            },
        })
    }

    /// A request in an address's query string: `query`, `variables` as the
    /// JSON text of an object, `operationName`.
    pub fn from_query(query: &str) -> Option<Self> {
        let mut request = Self::default();
        let mut found = false;
        for (name, value) in url::form_urlencoded::parse(query.as_bytes()) {
            match name.as_ref() {
                "query" => {
                    request.query = value.into_owned();
                    found = true;
                }
                "variables" => {
                    request.variables = variables(Some(Value::String(value.into_owned())))?;
                }
                "operationName" if !value.is_empty() => {
                    request.operation_name = Some(value.into_owned());
                }
                _ => {}
            }
        }
        found.then_some(request)
    }
}

/// A request's variables: an object, the JSON text of one, or none.
fn variables(given: Option<Value>) -> Option<Map<String, Value>> {
    match given {
        None | Some(Value::Null) => Some(Map::new()),
        Some(Value::Object(variables)) => Some(variables),
        Some(Value::String(text)) if text.trim().is_empty() => Some(Map::new()),
        Some(Value::String(text)) => match serde_json::from_str(&text).ok()? {
            Value::Object(variables) => Some(variables),
            Value::Null => Some(Map::new()),
            _ => None,
        },
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_request_is_read_from_a_posted_body() {
        let posted = br#"{"query":"query ($id: Int) { Media(id: $id) { id } }","variables":{"id":21},"operationName":null}"#;
        let request = Request::from_body(posted).unwrap();
        assert_eq!(request.query, "query ($id: Int) { Media(id: $id) { id } }");
        assert_eq!(request.variables["id"], json!(21));
        assert_eq!(request.operation_name, None);

        // Variables as the text of an object; a name to run.
        let text = br#"{"query":"query A { a }","variables":"{\"id\": 5}","operationName":"A"}"#;
        let request = Request::from_body(text).unwrap();
        assert_eq!(request.variables["id"], json!(5));
        assert_eq!(request.operation_name.as_deref(), Some("A"));

        // None at all, or null.
        let bare = Request::from_body(br#"{"query":"{ a }","variables":null}"#).unwrap();
        assert!(bare.variables.is_empty());

        for unread in [
            &br#"{"variables":{}}"#[..],
            br#"{"query":5}"#,
            br#"[{"query":"{ a }"}]"#,
            br#"{"query":"{ a }","variables":[1]}"#,
            br#"{"query":"{ a }","operationName":3}"#,
            b"query { a }",
        ] {
            assert!(Request::from_body(unread).is_none(), "{unread:?}");
        }
    }

    /// AniList's own refusals of a few documents, asked on 2 November 2026
    /// (`wire/fixtures/anilist/errors.json`), and this server's.
    #[test]
    fn a_document_anilist_refuses_is_refused_in_its_words() {
        let refused: Value =
            serde_json::from_str(include_str!("../wire/fixtures/anilist/errors.json")).unwrap();
        let ours = |name: &str| -> error::Error {
            let asked = &refused[name];
            let variables = asked["variables"].as_object().cloned().unwrap_or_default();
            let query = asked["query"].as_str().unwrap();
            match Prepared::new(query, &variables, None) {
                Err(error) => error,
                Ok(prepared) => {
                    let fields = prepared.root_fields().unwrap();
                    prepared
                        .collect("Media", &fields[0].selections)
                        .err()
                        .expect("refused")
                }
            }
        };
        for name in ["unknown-fragment", "missing-variable"] {
            let error = ours(name);
            assert_eq!(
                body(std::slice::from_ref(&error), None),
                refused[name]["body"],
                "{name}"
            );
            assert_eq!(
                u64::from(status(&[error])),
                refused[name]["status"].as_u64().unwrap()
            );
        }
        // Where AniList says the document breaks, in the parser's words.
        let syntax = ours("syntax");
        assert_eq!(syntax.status, 400);
        assert!(syntax.message.starts_with("Syntax Error: "));
        assert_eq!(
            body(&[syntax], None)["errors"][0]["locations"],
            refused["syntax"]["body"]["errors"][0]["locations"]
        );
    }

    #[test]
    fn a_request_is_read_from_an_address() {
        let request = Request::from_query(
            "query=query%20(%24id%3A%20Int)%20%7B%20Media(id%3A%20%24id)%20%7B%20id%20%7D%20%7D&variables=%7B%22id%22%3A21%7D&operationName=",
        )
        .unwrap();
        assert_eq!(request.query, "query ($id: Int) { Media(id: $id) { id } }");
        assert_eq!(request.variables["id"], json!(21));
        assert_eq!(request.operation_name, None);
        assert!(Request::from_query("variables=%7B%7D").is_none());
        assert!(Request::from_query("query=x&variables=%5B1%5D").is_none());
    }
}
