//! A selection laid over a document written here: each field asked is
//! looked up on the object it is asked of, with its arguments, and what it
//! holds is laid out as the selection below it says — under the names the
//! client chose, in the order it asked.
//!
//! The objects know their own fields ([`Object`]); nothing here knows a
//! schema. An object that cannot answer a field as the service would says
//! so ([`FieldError::Unanswerable`]), and the whole document is the
//! service's to answer: an answer written here is whole or not at all.

use std::cell::Cell;

use serde_json::{Map, Value};

use super::{
    document::{Field, Prepared},
    error::Error,
};

/// The most values one answer written here may hold. AniList refuses a
/// query past a cost of its own; past this one, a selection that names an
/// entry from within it again and again is the service's to answer.
pub const MOST_VALUES: usize = 20_000;

/// The most an answer written here may weigh, written out: past it, the
/// selection is the service's to answer — a text asked again and again under
/// as many aliases weighs a hundred megabytes in a few.
pub const MOST_BYTES: usize = 2 * 1024 * 1024;

/// How deep an answer written here may nest its objects and lists. Each is
/// one call deeper on the stack, and an entry names itself from within —
/// `nextAiringEpisode { media { … } }` — as deep as fragments let a small
/// document go.
pub const MOST_DEEP: usize = 64;

/// A field's arguments, as the document gives them: its variables given
/// their values, an enum's name as a string. One left out is not here; one
/// given null is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arguments(Map<String, Value>);

impl Arguments {
    pub fn new(arguments: Map<String, Value>) -> Self {
        Self(arguments)
    }

    #[cfg(test)]
    pub fn as_map(&self) -> &Map<String, Value> {
        &self.0
    }

    /// Refuse an argument the field does not take.
    pub fn only(&self, taken: &[&str]) -> Result<(), FieldError> {
        match self.0.keys().find(|name| !taken.contains(&name.as_str())) {
            Some(name) => Err(FieldError::UnknownArgument(name.clone())),
            None => Ok(()),
        }
    }

    /// An `Int` argument: none when left out or null.
    pub fn int(&self, name: &str) -> Result<Option<i64>, FieldError> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(number)) => number
                .as_i64()
                .filter(|n| i32::try_from(*n).is_ok())
                .map(Some)
                .ok_or_else(|| wrong(name, "Int")),
            Some(_) => Err(wrong(name, "Int")),
        }
    }

    /// A list of `Int` — one alone stands for a list of it: none when left
    /// out or null; a null among them is no number, and left out.
    pub fn ints(&self, name: &str) -> Result<Option<Vec<i64>>, FieldError> {
        let int = |value: &Value| match value {
            Value::Number(number) => number
                .as_i64()
                .filter(|n| i32::try_from(*n).is_ok())
                .map(Some)
                .ok_or_else(|| wrong(name, "a list of Int")),
            Value::Null => Ok(None),
            _ => Err(wrong(name, "a list of Int")),
        };
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(values)) => values
                .iter()
                .filter_map(|value| int(value).transpose())
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            Some(single) => Ok(Some(int(single)?.into_iter().collect())),
        }
    }

    /// The arguments given, by name.
    pub fn names_given(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// A `Boolean` argument: none when left out or null.
    pub fn boolean(&self, name: &str) -> Result<Option<bool>, FieldError> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(flag)) => Ok(Some(*flag)),
            Some(_) => Err(wrong(name, "Boolean")),
        }
    }

    /// An enum argument, by its value's name: none when left out or null.
    pub fn name(&self, name: &str) -> Result<Option<&str>, FieldError> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value)) => Ok(Some(value)),
            Some(_) => Err(wrong(name, "an enum value")),
        }
    }

    /// A list of enum values — one alone stands for a list of it: none
    /// when left out or null.
    pub fn names(&self, name: &str) -> Result<Option<Vec<&str>>, FieldError> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value)) => Ok(Some(vec![value.as_str()])),
            Some(Value::Array(values)) => values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .ok_or_else(|| wrong(name, "a list of enum values"))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            Some(_) => Err(wrong(name, "a list of enum values")),
        }
    }
}

fn wrong(name: &str, expected: &str) -> FieldError {
    FieldError::Invalid(format!(
        "Argument \"{name}\" has an invalid value: {expected} is expected."
    ))
}

/// Something a document written here is made of: an entry, its title, a
/// date. It answers the fields asked of it by their names in the service's
/// schema.
pub trait Object {
    /// Its type's name in the service's schema: what `__typename` answers,
    /// and what a fragment's type condition is held against.
    fn typename(&self) -> &'static str;

    /// The field `name`, asked with `arguments`.
    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError>;
}

/// What a field holds.
pub enum Resolved<'o> {
    Null,
    /// A scalar or an enum — or a list of them: anything nothing is asked
    /// of below.
    Value(Value),
    Object(Box<dyn Object + 'o>),
    List(Vec<Resolved<'o>>),
}

impl<'o> Resolved<'o> {
    pub fn object(object: impl Object + 'o) -> Self {
        Self::Object(Box::new(object))
    }

    /// A scalar, or null.
    pub fn some<T: Into<Value>>(value: Option<T>) -> Self {
        value.map_or(Self::Null, |value| Self::Value(value.into()))
    }

    /// A list of objects.
    pub fn objects<O: Object + 'o>(objects: impl IntoIterator<Item = O>) -> Self {
        Self::List(objects.into_iter().map(Self::object).collect())
    }
}

/// Why an object could not give a field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldError {
    /// The service's schema has no such field on the type.
    Unknown,
    /// An argument the field does not take.
    UnknownArgument(String),
    /// An argument of a value the field does not take: why.
    Invalid(String),
    /// A field the service would answer, which this server cannot answer
    /// as the service would: an identifier only the service gives, what an
    /// account holds.
    Unanswerable,
}

/// Why a selection could not be laid over a document written here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The document asks in a way the service would refuse: its error.
    Invalid(Error),
    /// The document asks for something this server cannot answer as the
    /// service would, or for too much.
    Unanswerable,
}

impl From<Error> for Failure {
    /// Refused where AniList would refuse the document; left to it where
    /// the document is past what this server reads.
    fn from(error: Error) -> Self {
        if error.beyond {
            Self::Unanswerable
        } else {
            Self::Invalid(error)
        }
    }
}

/// The laying out of one answer: its document, and how much it holds so far.
pub struct Projection<'p, 'a> {
    prepared: &'p Prepared<'a>,
    spent: Cell<usize>,
    /// What the answer weighs so far, written out.
    weight: Cell<usize>,
}

impl<'p, 'a> Projection<'p, 'a> {
    pub fn new(prepared: &'p Prepared<'a>) -> Self {
        Self {
            prepared,
            spent: Cell::new(0),
            weight: Cell::new(0),
        }
    }

    /// What `field` holds, laid out as its selection asks: an object's
    /// fields, each list's entries one by one, a scalar as it is — within
    /// [`MOST_VALUES`], [`MOST_BYTES`] and [`MOST_DEEP`].
    pub fn complete(
        &self,
        field: &Field<'p, 'a>,
        resolved: Resolved<'_>,
    ) -> Result<Value, Failure> {
        self.lay(field, resolved, 0)
    }

    /// Count `bytes` more of the answer.
    fn weigh(&self, bytes: usize) -> Result<(), Failure> {
        let weight = self.weight.get().saturating_add(bytes);
        if weight > MOST_BYTES {
            return Err(Failure::Unanswerable);
        }
        self.weight.set(weight);
        Ok(())
    }

    fn lay(
        &self,
        field: &Field<'p, 'a>,
        resolved: Resolved<'_>,
        deep: usize,
    ) -> Result<Value, Failure> {
        let spent = self.spent.get() + 1;
        if spent > MOST_VALUES || deep > MOST_DEEP {
            return Err(Failure::Unanswerable);
        }
        self.spent.set(spent);

        match resolved {
            Resolved::Null => Ok(Value::Null),
            Resolved::Value(value) => {
                if field.has_selection() {
                    return Err(Failure::Invalid(Error::invalid(
                        format!(
                            "Field \"{}\" must not have a selection since its type has no \
                             subfields.",
                            field.name
                        ),
                        [field.at],
                    )));
                }
                self.weigh(written(&value))?;
                Ok(value)
            }
            Resolved::List(items) => items
                .into_iter()
                .map(|item| self.lay(field, item, deep + 1))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            Resolved::Object(object) => {
                let typename = object.typename();
                if !field.has_selection() {
                    return Err(Failure::Invalid(Error::invalid(
                        format!(
                            "Field \"{}\" of type \"{typename}\" must have a selection of \
                             subfields. Did you mean \"{} {{ ... }}\"?",
                            field.name, field.name
                        ),
                        [field.at],
                    )));
                }
                let mut laid = Map::new();
                for asked in self.prepared.collect(typename, &field.selections)? {
                    self.weigh(asked.key.len() + 4)?;
                    let value = if asked.name == "__typename" {
                        self.weigh(typename.len() + 2)?;
                        Value::String(typename.to_string())
                    } else {
                        match object.field(asked.name, &asked.arguments) {
                            Ok(resolved) => self.lay(&asked, resolved, deep + 1)?,
                            Err(FieldError::Unknown) => {
                                return Err(Failure::Invalid(Error::invalid(
                                    format!(
                                        "Cannot query field \"{}\" on type \"{typename}\".",
                                        asked.name
                                    ),
                                    [asked.at],
                                )));
                            }
                            Err(FieldError::UnknownArgument(argument)) => {
                                return Err(Failure::Invalid(Error::invalid(
                                    format!(
                                        "Unknown argument \"{argument}\" on field \"{}\" of \
                                         type \"{typename}\".",
                                        asked.name
                                    ),
                                    [asked.at],
                                )));
                            }
                            Err(FieldError::Invalid(why)) => {
                                return Err(Failure::Invalid(Error::invalid(
                                    format!(
                                        "Field \"{}\" of type \"{typename}\": {why}",
                                        asked.name
                                    ),
                                    [asked.at],
                                )));
                            }
                            Err(FieldError::Unanswerable) => return Err(Failure::Unanswerable),
                        }
                    };
                    laid.insert(asked.key.to_string(), value);
                }
                Ok(Value::Object(laid))
            }
        }
    }
}

/// What a value weighs written out as JSON, near enough.
fn written(value: &Value) -> usize {
    match value {
        Value::Null | Value::Bool(_) => 5,
        Value::Number(_) => 12,
        Value::String(text) => text.len() + 2,
        Value::Array(items) => 2 + items.iter().map(|item| written(item) + 1).sum::<usize>(),
        Value::Object(fields) => {
            2 + fields
                .iter()
                .map(|(key, value)| key.len() + 4 + written(value))
                .sum::<usize>()
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::graphql::error::Location;

    /// A made-up entry: a title, an id, its dates, its next entries.
    struct Show {
        id: i64,
        depth: u32,
    }

    struct Title;

    impl Object for Title {
        fn typename(&self) -> &'static str {
            "MediaTitle"
        }

        fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
            arguments.only(&["stylised"])?;
            arguments.boolean("stylised")?;
            match name {
                "romaji" => Ok(Resolved::Value(json!("Sousou no Frieren"))),
                "english" => Ok(Resolved::Null),
                _ => Err(FieldError::Unknown),
            }
        }
    }

    impl Object for Show {
        fn typename(&self) -> &'static str {
            "Media"
        }

        fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
            match name {
                "id" => Ok(Resolved::Value(json!(self.id))),
                "title" => Ok(Resolved::object(Title)),
                "genres" => Ok(Resolved::Value(json!(["Adventure", "Drama"]))),
                "description" => {
                    arguments.only(&["asHtml"])?;
                    let html = arguments.boolean("asHtml")?.unwrap_or(false);
                    Ok(Resolved::Value(json!(if html {
                        "<p>An elf.</p>"
                    } else {
                        "An elf."
                    })))
                }
                "next" => Ok(Resolved::objects((1..=3).map(|n| Show {
                    id: self.id + n,
                    depth: self.depth + 1,
                }))),
                "isFavourite" => Err(FieldError::Unanswerable),
                _ => Err(FieldError::Unknown),
            }
        }
    }

    fn laid(query: &str) -> Result<Value, Failure> {
        let prepared = Prepared::new(query, &Map::new(), None).map_err(Failure::Invalid)?;
        let projection = Projection::new(&prepared);
        let fields = prepared.root_fields().map_err(Failure::Invalid)?;
        let mut data = Map::new();
        for field in &fields {
            let value = if field.name == "__typename" {
                json!("Query")
            } else {
                projection.complete(field, Resolved::object(Show { id: 1, depth: 0 }))?
            };
            data.insert(field.key.to_string(), value);
        }
        Ok(Value::Object(data))
    }

    #[test]
    fn a_selection_is_laid_out_under_the_names_asked_in_the_order_asked() {
        let answer = laid(
            "query { Media(id: 1) { title { english romaji } __typename id
               html: description(asHtml: true) plain: description genres
               ... on Media { id } ...More } __typename }
             fragment More on Media { next { id } }",
        )
        .unwrap();
        assert_eq!(
            answer,
            json!({
                "Media": {
                    "title": { "english": null, "romaji": "Sousou no Frieren" },
                    "__typename": "Media",
                    "id": 1,
                    "html": "<p>An elf.</p>",
                    "plain": "An elf.",
                    "genres": ["Adventure", "Drama"],
                    "next": [{ "id": 2 }, { "id": 3 }, { "id": 4 }]
                },
                "__typename": "Query"
            })
        );
        // In the order asked, the alias's place too.
        let keys: Vec<&String> = answer["Media"].as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "title",
                "__typename",
                "id",
                "html",
                "plain",
                "genres",
                "next"
            ]
        );
    }

    #[test]
    fn what_the_schema_would_refuse_is_said_and_what_cannot_be_answered_here_is_not() {
        let unknown = laid("query {\n  Media(id: 21) {\n    idd\n  }\n}").unwrap_err();
        // AniList's own words and place for it, on 2 November 2026, less
        // its suggestion.
        assert_eq!(
            unknown,
            Failure::Invalid(Error::invalid(
                "Cannot query field \"idd\" on type \"Media\".",
                [Location { line: 3, column: 5 }]
            ))
        );
        for invalid in [
            "{ Media { id { x } } }",
            "{ Media { title } }",
            "{ Media { description(asHtml: \"yes\") } }",
            "{ Media { description(asText: true) } }",
            "{ Media { title { romaji(stylised: 1) } } }",
        ] {
            assert!(
                matches!(laid(invalid), Err(Failure::Invalid(_))),
                "{invalid}"
            );
        }
        // AniList's words for an argument a field does not take.
        assert_eq!(
            laid("{ Media { description(asText: true) } }"),
            Err(Failure::Invalid(Error::invalid(
                "Unknown argument \"asText\" on field \"description\" of type \"Media\".",
                [Location {
                    line: 1,
                    column: 11
                }]
            )))
        );
        assert_eq!(
            laid("{ Media { id isFavourite } }"),
            Err(Failure::Unanswerable)
        );
        // Found in a list as much as anywhere.
        assert_eq!(
            laid("{ Media { next { isFavourite } } }"),
            Err(Failure::Unanswerable)
        );
    }

    #[test]
    fn a_selection_that_grows_past_what_one_answer_holds_is_not_answered_here() {
        // Three entries each, nine levels down: 3⁹ entries.
        let mut query = String::from("{ Media { ");
        for _ in 0..9 {
            query.push_str("next { id ");
        }
        query.push_str(&"} ".repeat(9));
        query.push_str("} }");
        assert_eq!(laid(&query), Err(Failure::Unanswerable));
        // Two levels are a dozen values.
        assert!(laid("{ Media { next { next { id } } } }").is_ok());
    }

    /// An entry that names another from within, forever, and has a long
    /// text to tell.
    struct Endless;

    impl Object for Endless {
        fn typename(&self) -> &'static str {
            "Media"
        }

        fn field(&self, name: &str, _: &Arguments) -> Result<Resolved<'_>, FieldError> {
            match name {
                "id" => Ok(Resolved::Value(json!(1))),
                "description" => Ok(Resolved::Value(json!("x".repeat(5_000)))),
                "next" => Ok(Resolved::object(Endless)),
                _ => Err(FieldError::Unknown),
            }
        }
    }

    /// The first root field of `query`, laid out of an endless entry.
    fn endless(query: &str) -> Result<Value, Failure> {
        let prepared = Prepared::new(query, &Map::new(), None)?;
        let projection = Projection::new(&prepared);
        let fields = prepared.root_fields()?;
        projection.complete(&fields[0], Resolved::object(Endless))
    }

    #[test]
    fn an_answer_deeper_or_heavier_than_one_written_here_is_left_to_anilist() {
        // Deep: fragments each nesting thirty levels, spread one in another,
        // a short document for a deep answer.
        let deep = |fragments: usize| {
            let mut query = String::from("{ Media { ...F0 } }");
            for at in 0..fragments {
                query += &format!(
                    " fragment F{at} on Media {{ {}...F{}{} }}",
                    "next { ".repeat(30),
                    at + 1,
                    " }".repeat(30)
                );
            }
            query + &format!(" fragment F{fragments} on Media {{ id }}")
        };
        assert!(endless(&deep(1)).is_ok());
        assert_eq!(endless(&deep(3)), Err(Failure::Unanswerable));
        // Fragments spread deeper than one is read, within an entry.
        let mut spread = String::from("{ Media { ...F0 } }");
        for at in 0..20 {
            spread += &format!(" fragment F{at} on Media {{ ...F{} }}", at + 1);
        }
        spread += " fragment F20 on Media { id }";
        assert_eq!(endless(&spread), Err(Failure::Unanswerable));

        // Heavy: a long text asked again and again under other names.
        let texts = |times: usize| {
            let asked: String = (0..times)
                .map(|at| format!(" d{at}: description"))
                .collect();
            format!("{{ Media {{{asked} }} }}")
        };
        assert!(endless(&texts(300)).is_ok());
        assert_eq!(endless(&texts(500)), Err(Failure::Unanswerable));
        // The names an answer is written under weigh too, its types' among
        // them.
        let named = |levels: usize| {
            let names: String = (0..100)
                .map(|at| format!(" {}{at}: __typename", "n".repeat(1_000)))
                .collect();
            format!(
                "{{ Media {{ ...T {}id{} }} }} fragment T on Media {{{names} }}",
                "next { ...T ".repeat(levels),
                " }".repeat(levels)
            )
        };
        assert!(endless(&named(5)).is_ok());
        assert_eq!(endless(&named(21)), Err(Failure::Unanswerable));
    }

    #[test]
    fn arguments_are_read_as_the_types_they_are() {
        let arguments = Arguments::new(
            json!({ "page": 2, "asHtml": true, "type": "ANIME", "sort": ["ROLE", "ID"], "one": "ROLE", "none": null })
                .as_object()
                .unwrap()
                .clone(),
        );
        assert_eq!(arguments.int("page"), Ok(Some(2)));
        assert_eq!(arguments.int("absent"), Ok(None));
        assert_eq!(arguments.int("none"), Ok(None));
        assert!(arguments.int("type").is_err());
        assert_eq!(arguments.boolean("asHtml"), Ok(Some(true)));
        assert_eq!(arguments.name("type"), Ok(Some("ANIME")));
        assert_eq!(arguments.names("sort"), Ok(Some(vec!["ROLE", "ID"])));
        assert_eq!(arguments.names("one"), Ok(Some(vec!["ROLE"])));
        assert!(
            arguments
                .only(&["page", "asHtml", "type", "sort", "one", "none"])
                .is_ok()
        );
        assert!(arguments.only(&["page"]).is_err());
    }
}
