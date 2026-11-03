//! A GraphQL document as a client sends one: parsed, its operation chosen,
//! its variables given the values the request carries or their defaults,
//! and its selections read field by field — through its named and inline
//! fragments, its aliases and its `@include` and `@skip`.
//!
//! No schema is read here: what a field is, and what it takes, is for the
//! object it is asked of ([`super::Object`]). What is checked is what the
//! document says of itself, as AniList checks it before it runs anything —
//! an operation to run, its variables, its fragments, its directives — each
//! refusal in AniList's own words ([`Error`]).

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use graphql_parser::query::{
    self as q, Definition, Directive, OperationDefinition, Selection, TypeCondition,
};
use serde_json::{Map, Number, Value};

use super::{
    error::{Error, Location},
    execute::Arguments,
};

/// A document's selection set: the AST's, its names borrowed from the text.
pub(crate) type SelectionSet<'a> = q::SelectionSet<'a, &'a str>;

/// The deepest a document may nest its brackets — selections, argument
/// lists, lists and objects — and still be read. The parser goes one call
/// deeper for each, and a request of a megabyte of braces would go past the
/// stack; AniList's own clients nest a dozen at the most.
pub const MAX_DEPTH: usize = 32;

/// The most definitions — operations and fragments — a document may hold,
/// the most of them fragments, and the most variables its operation may
/// declare, to be read here. AniList's clients send a handful of each; past
/// these, a document is AniList's to weigh.
pub const MOST_DEFINITIONS: usize = 64;
pub const MOST_FRAGMENTS: usize = 32;
pub const MOST_VARIABLES: usize = 64;

/// The most selections — fields, fragments spread, inline fragments — a
/// document may hold, everywhere in it.
pub const MOST_SELECTIONS: usize = 4_000;

/// How deep fragments may be spread one within another, inline ones
/// included, in one selection: each is one call deeper on the stack.
pub const MOST_SPREADS_DEEP: usize = 16;

/// The most values the arguments of a document may hold — a list's every
/// entry, a variable's every value each time it is used — each field's
/// counted once.
pub const MOST_ARGUMENT_VALUES: usize = 2_000;

/// What a document's operation does: a query reads, a mutation changes
/// something, a subscription waits on something.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationKind {
    Query,
    Mutation,
    Subscription,
}

/// A document read and made ready to run: its operation, and the values of
/// its variables.
#[derive(Debug)]
pub struct Prepared<'a> {
    document: q::Document<'a, &'a str>,
    /// Which of its definitions is the operation run.
    operation: usize,
    /// The variables the operation declares, by name.
    declared: HashSet<&'a str>,
    /// Their values: the request's, else their defaults. One the request
    /// left out, with no default, has none — an argument it is given to is
    /// left out.
    variables: Map<String, Value>,
    /// The fragments it defines, by name: their place among its definitions.
    fragments: HashMap<&'a str, usize>,
    /// Each field's arguments, read once, by where the field is in the
    /// document: a selection is gathered again for every object it is laid
    /// over.
    read: Mutex<HashMap<usize, Arc<Arguments>>>,
    /// How many values the arguments read so far hold: see
    /// [`MOST_ARGUMENT_VALUES`].
    values_read: AtomicUsize,
}

/// One field a selection asks for, as it is gathered from the selection, its
/// fragments and the directives that keep or drop each part: every place it
/// is asked under one name in the answer, together.
pub struct Field<'p, 'a> {
    /// Where its value goes in the answer: its alias, else its name.
    pub key: &'a str,
    pub name: &'a str,
    pub arguments: Arc<Arguments>,
    /// Where it is first asked.
    pub at: Location,
    /// What is asked of its value, from every place it is asked.
    pub(crate) selections: Vec<&'p SelectionSet<'a>>,
}

impl Field<'_, '_> {
    /// Whether anything is asked of its value: an object's fields.
    pub fn has_selection(&self) -> bool {
        self.selections.iter().any(|set| !set.items.is_empty())
    }
}

/// An operation's head: what it is, its name, its variables, its directives
/// and its selection.
struct Head<'d, 'a> {
    kind: OperationKind,
    name: Option<&'a str>,
    variables: &'d [q::VariableDefinition<'a, &'a str>],
    directives: &'d [Directive<'a, &'a str>],
    selection_set: &'d SelectionSet<'a>,
    at: Location,
}

fn head<'d, 'a>(operation: &'d OperationDefinition<'a, &'a str>) -> Head<'d, 'a> {
    match operation {
        OperationDefinition::SelectionSet(set) => Head {
            kind: OperationKind::Query,
            name: None,
            variables: &[],
            directives: &[],
            selection_set: set,
            at: set.span.0.into(),
        },
        OperationDefinition::Query(query) => Head {
            kind: OperationKind::Query,
            name: query.name,
            variables: &query.variable_definitions,
            directives: &query.directives,
            selection_set: &query.selection_set,
            at: query.position.into(),
        },
        OperationDefinition::Mutation(mutation) => Head {
            kind: OperationKind::Mutation,
            name: mutation.name,
            variables: &mutation.variable_definitions,
            directives: &mutation.directives,
            selection_set: &mutation.selection_set,
            at: mutation.position.into(),
        },
        OperationDefinition::Subscription(subscription) => Head {
            kind: OperationKind::Subscription,
            name: subscription.name,
            variables: &subscription.variable_definitions,
            directives: &subscription.directives,
            selection_set: &subscription.selection_set,
            at: subscription.position.into(),
        },
    }
}

impl<'a> Prepared<'a> {
    /// Read `query`, choose the operation to run — the one named
    /// `operation_name`, or the only one — and give its variables their
    /// values from `variables`.
    pub fn new(
        query: &'a str,
        variables: &Map<String, Value>,
        operation_name: Option<&str>,
    ) -> Result<Self, Error> {
        if depth(query) > MAX_DEPTH {
            return Err(Error::beyond(
                format!("The query is nested deeper than {MAX_DEPTH} levels."),
                [],
            ));
        }
        let document = q::parse_query::<&str>(query).map_err(|e| syntax(&e.to_string()))?;
        let too_many = |what: &str, most: usize| {
            Error::beyond(format!("The query holds more than {most} {what}."), [])
        };
        if document.definitions.len() > MOST_DEFINITIONS {
            return Err(too_many("operations and fragments", MOST_DEFINITIONS));
        }
        let mut selections = 0;
        let mut fragment_count = 0;
        for definition in &document.definitions {
            let set = match definition {
                Definition::Fragment(fragment) => {
                    fragment_count += 1;
                    &fragment.selection_set
                }
                Definition::Operation(operation) => head(operation).selection_set,
            };
            selections += selections_in(set);
        }
        if fragment_count > MOST_FRAGMENTS {
            return Err(too_many("fragments", MOST_FRAGMENTS));
        }
        if selections > MOST_SELECTIONS {
            return Err(too_many("selections", MOST_SELECTIONS));
        }

        let mut fragments = HashMap::new();
        let mut named: HashSet<&'a str> = HashSet::new();
        let mut operations: Vec<(usize, Option<&'a str>, Location)> = Vec::new();
        for (place, definition) in document.definitions.iter().enumerate() {
            match definition {
                Definition::Fragment(fragment) => {
                    if fragments.insert(fragment.name, place).is_some() {
                        return Err(Error::invalid(
                            format!(
                                "There can be only one fragment named \"{}\".",
                                fragment.name
                            ),
                            [fragment.position.into()],
                        ));
                    }
                    if let Some(directive) = fragment.directives.first() {
                        return Err(misplaced(directive, "FRAGMENT_DEFINITION"));
                    }
                }
                Definition::Operation(operation) => {
                    let head = head(operation);
                    if let Some(name) = head.name
                        && !named.insert(name)
                    {
                        return Err(Error::invalid(
                            format!("There can be only one operation named \"{name}\"."),
                            [head.at],
                        ));
                    }
                    operations.push((place, head.name, head.at));
                }
            }
        }
        if operations.len() > 1
            && let Some((_, _, at)) = operations.iter().find(|(_, name, _)| name.is_none())
        {
            return Err(Error::invalid(
                "This anonymous operation must be the only defined operation.",
                [*at],
            ));
        }
        let operation = match operation_name.filter(|name| !name.is_empty()) {
            Some(wanted) => operations
                .iter()
                .find(|(_, name, _)| *name == Some(wanted))
                .map(|(place, _, _)| *place)
                .ok_or_else(|| {
                    Error::invalid(format!("Unknown operation named \"{wanted}\"."), [])
                })?,
            None => match operations.as_slice() {
                [(place, _, _)] => *place,
                [] => return Err(Error::invalid("Must provide an operation.", [])),
                _ => {
                    return Err(Error::invalid(
                        "Must provide operation name if query contains multiple operations.",
                        [],
                    ));
                }
            },
        };

        let mut prepared = Self {
            document,
            operation,
            declared: HashSet::new(),
            variables: Map::new(),
            fragments,
            read: Mutex::new(HashMap::new()),
            values_read: AtomicUsize::new(0),
        };
        prepared.refuse_cycles()?;

        let head = prepared.head();
        if let Some(directive) = head.directives.first() {
            return Err(misplaced(directive, "QUERY"));
        }
        if head.variables.len() > MOST_VARIABLES {
            return Err(too_many("variables", MOST_VARIABLES));
        }
        let mut declared = HashSet::new();
        for definition in head.variables {
            if !declared.insert(definition.name) {
                return Err(Error::invalid(
                    format!(
                        "There can be only one variable named \"${}\".",
                        definition.name
                    ),
                    [definition.position.into()],
                ));
            }
        }
        let values = coerce(head.variables, variables)?;
        prepared.declared = declared;
        prepared.variables = values;
        Ok(prepared)
    }

    fn head(&self) -> Head<'_, 'a> {
        match &self.document.definitions[self.operation] {
            Definition::Operation(operation) => head(operation),
            Definition::Fragment(_) => unreachable!("the operation chosen is an operation"),
        }
    }

    /// What the operation does.
    pub fn kind(&self) -> OperationKind {
        self.head().kind
    }

    /// The variables, as given to the operation.
    #[cfg(test)]
    pub fn variables(&self) -> &Map<String, Value> {
        &self.variables
    }

    /// The fields the operation asks for at its root, of AniList's `Query`.
    pub fn root_fields(&self) -> Result<Vec<Field<'_, 'a>>, Error> {
        let set = self.head().selection_set;
        self.collect("Query", &[set])
    }

    fn fragment(&self, name: &str) -> Option<&q::FragmentDefinition<'a, &'a str>> {
        match self.document.definitions.get(*self.fragments.get(name)?)? {
            Definition::Fragment(fragment) => Some(fragment),
            Definition::Operation(_) => None,
        }
    }

    /// The fields `sets` ask of an object of the type `on`, gathered: each
    /// fragment that applies to the type followed once, each field kept or
    /// dropped by its directives, and those asked under one name in the
    /// answer made one, in the order they first come.
    pub fn collect<'p>(
        &'p self,
        on: &str,
        sets: &[&'p SelectionSet<'a>],
    ) -> Result<Vec<Field<'p, 'a>>, Error> {
        let mut gathered = Gathered {
            fields: Vec::new(),
            by_key: HashMap::new(),
            spread: HashSet::new(),
        };
        for set in sets {
            self.collect_into(on, set, &mut gathered, 0)?;
        }
        Ok(gathered.fields)
    }

    fn collect_into<'p>(
        &'p self,
        on: &str,
        set: &'p SelectionSet<'a>,
        gathered: &mut Gathered<'p, 'a>,
        deep: usize,
    ) -> Result<(), Error> {
        let deeper = || {
            if deep >= MOST_SPREADS_DEEP {
                Err(Error::beyond(
                    format!("Fragments are spread more than {MOST_SPREADS_DEEP} deep."),
                    [],
                ))
            } else {
                Ok(deep + 1)
            }
        };
        for selection in &set.items {
            match selection {
                Selection::Field(field) => {
                    if !self.kept(&field.directives)? {
                        continue;
                    }
                    let key = field.alias.unwrap_or(field.name);
                    let at = Location::from(field.position);
                    let arguments = self.arguments(field, at)?;
                    match gathered
                        .by_key
                        .get(key)
                        .and_then(|at| gathered.fields.get_mut(*at))
                    {
                        Some(known) => {
                            if known.name != field.name {
                                return Err(Error::invalid(
                                    format!(
                                        "Fields \"{key}\" conflict because \"{}\" and \"{}\" are \
                                         different fields. Use different aliases on the fields \
                                         to fetch both if this was intentional.",
                                        known.name, field.name
                                    ),
                                    [known.at, at],
                                ));
                            }
                            if known.arguments != arguments {
                                return Err(Error::invalid(
                                    format!(
                                        "Fields \"{key}\" conflict because they have differing \
                                         arguments. Use different aliases on the fields to fetch \
                                         both if this was intentional."
                                    ),
                                    [known.at, at],
                                ));
                            }
                            known.selections.push(&field.selection_set);
                        }
                        None => {
                            gathered.by_key.insert(key, gathered.fields.len());
                            gathered.fields.push(Field {
                                key,
                                name: field.name,
                                arguments,
                                at,
                                selections: vec![&field.selection_set],
                            });
                        }
                    }
                }
                Selection::FragmentSpread(spread_here) => {
                    if !self.kept(&spread_here.directives)? {
                        continue;
                    }
                    let name = spread_here.fragment_name;
                    let Some(fragment) = self.fragment(name) else {
                        return Err(Error::invalid(
                            format!("Unknown fragment \"{name}\"."),
                            [spread_here.position.into()],
                        ));
                    };
                    // Followed once in a selection, as GraphQL gathers fields.
                    if !gathered.spread.insert(name) {
                        continue;
                    }
                    let TypeCondition::On(condition) = &fragment.type_condition;
                    if *condition == on {
                        self.collect_into(on, &fragment.selection_set, gathered, deeper()?)?;
                    }
                }
                Selection::InlineFragment(inline) => {
                    if !self.kept(&inline.directives)? {
                        continue;
                    }
                    let applies = match &inline.type_condition {
                        None => true,
                        Some(TypeCondition::On(condition)) => *condition == on,
                    };
                    if applies {
                        self.collect_into(on, &inline.selection_set, gathered, deeper()?)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether what `directives` stand on is kept: dropped by `@skip` whose
    /// `if` is true, or `@include` whose `if` is false. AniList knows no
    /// other directive a document may use.
    fn kept(&self, directives: &[Directive<'a, &'a str>]) -> Result<bool, Error> {
        let mut kept = true;
        for directive in directives {
            let at = Location::from(directive.position);
            let condition = match directive.arguments.as_slice() {
                [("if", value)] => match self.flag(value, at)? {
                    Some(condition) => condition,
                    None => {
                        return Err(Error::invalid(
                            format!(
                                "Argument \"if\" of directive \"@{}\" has an invalid value: a \
                                 Boolean is expected.",
                                directive.name
                            ),
                            [at],
                        ));
                    }
                },
                _ => {
                    return Err(Error::invalid(
                        format!(
                            "Directive \"@{}\" takes one argument, \"if\", of type \"Boolean!\".",
                            directive.name
                        ),
                        [at],
                    ));
                }
            };
            match directive.name {
                "skip" => kept &= !condition,
                "include" => kept &= condition,
                other => {
                    return Err(Error::invalid(
                        format!("Unknown directive \"@{other}\"."),
                        [at],
                    ));
                }
            }
        }
        Ok(kept)
    }

    /// A `Boolean` a directive is given — written, or a variable's — without
    /// copying anything: none for another value.
    fn flag(&self, value: &q::Value<'a, &'a str>, at: Location) -> Result<Option<bool>, Error> {
        Ok(match value {
            q::Value::Boolean(flag) => Some(*flag),
            q::Value::Variable(name) => {
                if !self.declared.contains(name) {
                    return Err(Error::invalid(
                        format!("Variable \"${name}\" is not defined."),
                        [at],
                    ));
                }
                self.variables.get(*name).and_then(Value::as_bool)
            }
            _ => None,
        })
    }

    /// A field's arguments, their variables given their values — read once
    /// for each field of the document, however many objects its selection
    /// is laid over, and within [`MOST_ARGUMENT_VALUES`]. One given a
    /// variable the request left out, with no default, is left out.
    fn arguments(
        &self,
        field: &q::Field<'a, &'a str>,
        at: Location,
    ) -> Result<Arc<Arguments>, Error> {
        let place = std::ptr::from_ref(field) as usize;
        if let Some(known) = self
            .read
            .lock()
            .ok()
            .and_then(|read| read.get(&place).cloned())
        {
            return Ok(known);
        }
        let mut arguments = Map::new();
        for (name, value) in &field.arguments {
            if arguments.contains_key(*name) {
                return Err(Error::invalid(
                    format!("There can be only one argument named \"{name}\"."),
                    [at],
                ));
            }
            if let Some(value) = self.value(value, at)? {
                arguments.insert((*name).to_string(), value);
            }
        }
        let arguments = Arc::new(Arguments::new(arguments));
        if let Ok(mut read) = self.read.lock() {
            read.insert(place, Arc::clone(&arguments));
        }
        Ok(arguments)
    }

    /// Count `values` more against [`MOST_ARGUMENT_VALUES`].
    fn spend(&self, values: usize, at: Location) -> Result<(), Error> {
        let read = self.values_read.fetch_add(values, Ordering::Relaxed) + values;
        if read > MOST_ARGUMENT_VALUES {
            return Err(Error::beyond(
                format!("The query's arguments hold more than {MOST_ARGUMENT_VALUES} values."),
                [at],
            ));
        }
        Ok(())
    }

    /// A value the document writes, as JSON: its variables replaced by
    /// theirs, an enum's name as a string. `None` for a variable the request
    /// left out, with no default; null for one inside a list or an object.
    fn value(&self, value: &q::Value<'a, &'a str>, at: Location) -> Result<Option<Value>, Error> {
        Ok(Some(match value {
            q::Value::Variable(name) => {
                if !self.declared.contains(name) {
                    return Err(Error::invalid(
                        format!("Variable \"${name}\" is not defined."),
                        [at],
                    ));
                }
                match self.variables.get(*name) {
                    // Counted before it is copied: a variable may be large,
                    // and used again and again.
                    Some(value) => {
                        self.spend(values_in(value), at)?;
                        value.clone()
                    }
                    None => return Ok(None),
                }
            }
            q::Value::List(items) => Value::Array(
                items
                    .iter()
                    .map(|item| Ok(self.value(item, at)?.unwrap_or(Value::Null)))
                    .collect::<Result<_, Error>>()?,
            ),
            q::Value::Object(fields) => {
                let mut object = Map::new();
                for (name, value) in fields {
                    if let Some(value) = self.value(value, at)? {
                        object.insert((*name).to_string(), value);
                    }
                }
                Value::Object(object)
            }
            other => {
                self.spend(1, at)?;
                literal(other).ok_or_else(|| {
                    Error::invalid("A value of the document could not be read.", [at])
                })?
            }
        }))
    }

    /// Refuse a fragment that, through the fragments it spreads, spreads
    /// itself: following it would never end.
    fn refuse_cycles(&self) -> Result<(), Error> {
        // Each fragment's spreads, anywhere within it.
        let mut edges: HashMap<&'a str, Vec<(&'a str, Location)>> = HashMap::new();
        for definition in &self.document.definitions {
            if let Definition::Fragment(fragment) = definition {
                let mut found = Vec::new();
                spreads(&fragment.selection_set, &mut found);
                edges.insert(fragment.name, found);
            }
        }
        // Depth first, each fragment once: those finished lead to no cycle.
        let mut finished: HashSet<&'a str> = HashSet::new();
        for start in edges.keys().copied() {
            if finished.contains(start) {
                continue;
            }
            let mut path: Vec<&'a str> = vec![start];
            let mut on_path: HashSet<&'a str> = HashSet::from([start]);
            let mut stack: Vec<std::vec::IntoIter<(&'a str, Location)>> =
                vec![edges.get(start).cloned().unwrap_or_default().into_iter()];
            while let Some(next) = stack.last_mut().map(Iterator::next) {
                match next {
                    Some((name, at)) => {
                        if on_path.contains(name) {
                            return Err(Error::invalid(
                                format!("Cannot spread fragment \"{name}\" within itself."),
                                [at],
                            ));
                        }
                        if finished.contains(name) || !edges.contains_key(name) {
                            continue;
                        }
                        path.push(name);
                        on_path.insert(name);
                        stack.push(edges.get(name).cloned().unwrap_or_default().into_iter());
                    }
                    None => {
                        stack.pop();
                        if let Some(done) = path.pop() {
                            on_path.remove(done);
                            finished.insert(done);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// The fragments spread anywhere within `set`, below it included.
fn spreads<'a>(set: &SelectionSet<'a>, found: &mut Vec<(&'a str, Location)>) {
    for selection in &set.items {
        match selection {
            Selection::Field(field) => spreads(&field.selection_set, found),
            Selection::FragmentSpread(spread) => {
                found.push((spread.fragment_name, spread.position.into()));
            }
            Selection::InlineFragment(inline) => spreads(&inline.selection_set, found),
        }
    }
}

/// The fields gathered from a selection, and where each answer's name is
/// among them; the fragments spread so far.
struct Gathered<'p, 'a> {
    fields: Vec<Field<'p, 'a>>,
    by_key: HashMap<&'a str, usize>,
    spread: HashSet<&'a str>,
}

/// How many selections `set` holds, below it included.
fn selections_in(set: &SelectionSet<'_>) -> usize {
    set.items
        .iter()
        .map(|selection| {
            1 + match selection {
                Selection::Field(field) => selections_in(&field.selection_set),
                Selection::FragmentSpread(_) => 0,
                Selection::InlineFragment(inline) => selections_in(&inline.selection_set),
            }
        })
        .sum()
}

/// How many values a variable's value holds: itself, and every one within.
fn values_in(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(values_in).sum::<usize>(),
        Value::Object(fields) => 1 + fields.values().map(values_in).sum::<usize>(),
        _ => 1,
    }
}

/// A directive written where AniList takes none.
fn misplaced<'a>(directive: &Directive<'a, &'a str>, place: &str) -> Error {
    Error::invalid(
        format!(
            "Directive \"@{}\" may not be used on {place}.",
            directive.name
        ),
        [directive.position.into()],
    )
}

/// A value written in the document with no variable in it, as JSON.
fn literal<'a>(value: &q::Value<'a, &'a str>) -> Option<Value> {
    Some(match value {
        q::Value::Variable(_) => return None,
        q::Value::Int(number) => Value::from(number.as_i64()?),
        q::Value::Float(float) => Value::Number(Number::from_f64(*float)?),
        q::Value::String(text) => Value::String(text.clone()),
        q::Value::Boolean(flag) => Value::Bool(*flag),
        q::Value::Null => Value::Null,
        q::Value::Enum(name) => Value::String((*name).to_string()),
        q::Value::List(items) => Value::Array(items.iter().map(literal).collect::<Option<_>>()?),
        q::Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(name, value)| Some(((*name).to_string(), literal(value)?)))
                .collect::<Option<_>>()?,
        ),
    })
}

/// A variable's type, as the document writes it: `Int!`, `[MediaSort]`.
fn type_name<'a>(written: &q::Type<'a, &'a str>) -> String {
    match written {
        q::Type::NamedType(name) => (*name).to_string(),
        q::Type::ListType(inner) => format!("[{}]", type_name(inner)),
        q::Type::NonNullType(inner) => format!("{}!", type_name(inner)),
    }
}

/// The operation's variables, each given the value the request gives it,
/// else its default; one required and given neither is refused, as one
/// given a value its type cannot hold.
fn coerce<'a>(
    definitions: &[q::VariableDefinition<'a, &'a str>],
    given: &Map<String, Value>,
) -> Result<Map<String, Value>, Error> {
    let mut values = Map::new();
    for definition in definitions {
        let name = definition.name;
        let at = Location::from(definition.position);
        let shown = type_name(&definition.var_type);
        let required = matches!(definition.var_type, q::Type::NonNullType(_));
        let value = match given.get(name) {
            Some(Value::Null) if required => {
                return Err(Error::invalid(
                    format!("Variable \"${name}\" of non-null type \"{shown}\" must not be null."),
                    [at],
                ));
            }
            Some(value) => value.clone(),
            None => match &definition.default_value {
                Some(default) => literal(default).ok_or_else(|| {
                    Error::invalid(
                        format!("Variable \"${name}\" has a default that is not a constant."),
                        [at],
                    )
                })?,
                // AniList says it nowhere in the document.
                None if required => {
                    return Err(Error::invalid(
                        format!(
                            "Variable \"${name}\" of required type \"{shown}\" was not provided."
                        ),
                        [],
                    ));
                }
                None => continue,
            },
        };
        let value = input(&definition.var_type, value.clone()).map_err(|expected| {
            Error::invalid(
                format!(
                    "Variable \"${name}\" got invalid value {value}; Expected type {expected}."
                ),
                [at],
            )
        })?;
        values.insert(name.to_string(), value);
    }
    Ok(values)
}

/// A variable's value, as its type takes it: GraphQL's own scalars checked,
/// a single value standing for a list of one; AniList's enums, scalars and
/// inputs as they come. The type expected when it cannot hold it.
fn input<'a>(written: &q::Type<'a, &'a str>, value: Value) -> Result<Value, String> {
    match written {
        q::Type::NonNullType(inner) => {
            if value.is_null() {
                return Err(type_name(written));
            }
            input(inner, value)
        }
        q::Type::ListType(inner) => match value {
            Value::Null => Ok(Value::Null),
            Value::Array(items) => items
                .into_iter()
                .map(|item| input(inner, item))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            single => Ok(Value::Array(vec![input(inner, single)?])),
        },
        q::Type::NamedType(name) => {
            let wrong = || Err((*name).to_string());
            match (*name, value) {
                (_, Value::Null) => Ok(Value::Null),
                ("Int", Value::Number(number)) => {
                    // A whole number written with a fraction is one still.
                    let whole = number.as_i64().or_else(|| {
                        number
                            .as_f64()
                            .filter(|n| n.fract() == 0.0 && n.abs() < 1e15)
                            .map(|n| n as i64)
                    });
                    match whole.filter(|n| i32::try_from(*n).is_ok()) {
                        Some(whole) => Ok(Value::from(whole)),
                        None => wrong(),
                    }
                }
                ("Float", value @ Value::Number(_))
                | ("String", value @ Value::String(_))
                | ("Boolean", value @ Value::Bool(_))
                | ("ID", value @ Value::String(_)) => Ok(value),
                ("ID", Value::Number(number)) if number.is_i64() => {
                    Ok(Value::String(number.to_string()))
                }
                ("Int" | "Float" | "String" | "Boolean" | "ID", _) => wrong(),
                (_, value) => Ok(value),
            }
        }
    }
}

/// A document that does not parse, in AniList's words as near as the
/// parser's allow: `Syntax Error:` and what it found, where it found it.
fn syntax(said: &str) -> Error {
    // "query parse error: Parse error at 1:23\nUnexpected `<EOF>`\n…"
    let at = said.split_once("Parse error at ").and_then(|(_, rest)| {
        let (place, _) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let (line, column) = place.split_once(':')?;
        Some(Location {
            line: line.parse().ok()?,
            column: column.parse().ok()?,
        })
    });
    let what = said
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("Unexpected") || line.starts_with("Expected"))
        .unwrap_or("Unexpected input")
        .replace('`', "");
    Error::invalid(format!("Syntax Error: {what}"), at)
}

/// How deep `document` nests its brackets, its strings and comments aside.
fn depth(document: &str) -> usize {
    let bytes = document.as_bytes();
    let (mut depth, mut deepest, mut i) = (0usize, 0usize, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            // A comment ends at a line feed or a carriage return, as the
            // parser ends it: past one alone, the rest is the document.
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                    i += 1;
                }
            }
            b'"' if bytes[i..].starts_with(b"\"\"\"") => {
                i += 3;
                while i < bytes.len() && !bytes[i..].starts_with(b"\"\"\"") {
                    i += if bytes[i..].starts_with(b"\\\"\"\"") {
                        4
                    } else {
                        1
                    };
                }
                i += 2;
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' && bytes[i] != b'\n' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'{' | b'(' | b'[' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b'}' | b')' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    deepest
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn prepared<'a>(query: &'a str, variables: Value) -> Result<Prepared<'a>, Error> {
        let Value::Object(variables) = variables else {
            panic!("variables are an object")
        };
        Prepared::new(query, &variables, None)
    }

    /// The root fields, each as `key=name(arguments)`.
    fn roots(prepared: &Prepared<'_>) -> Vec<String> {
        prepared
            .root_fields()
            .unwrap()
            .iter()
            .map(|field| {
                format!(
                    "{}={}({})",
                    field.key,
                    field.name,
                    Value::Object(field.arguments.as_map().clone())
                )
            })
            .collect()
    }

    #[test]
    fn the_operation_run_is_the_one_named_or_the_only_one() {
        let two = "query A { Media(id: 1) { id } } query B { Page { pageInfo { total } } }";
        let variables = Map::new();
        let a = Prepared::new(two, &variables, Some("A")).unwrap();
        assert_eq!(roots(&a), ["Media=Media({\"id\":1})"]);
        let b = Prepared::new(two, &variables, Some("B")).unwrap();
        assert_eq!(roots(&b), ["Page=Page({})"]);

        let unnamed = Prepared::new(two, &variables, None).unwrap_err();
        assert_eq!(
            unnamed.message,
            "Must provide operation name if query contains multiple operations."
        );
        let unknown = Prepared::new(two, &variables, Some("C")).unwrap_err();
        assert_eq!(unknown.message, "Unknown operation named \"C\".");
        // An empty name is none, as clients send it.
        assert!(Prepared::new("{ Media(id: 1) { id } }", &variables, Some("")).is_ok());

        let anonymous = Prepared::new("{ a } query B { b }", &variables, Some("B")).unwrap_err();
        assert_eq!(
            anonymous.message,
            "This anonymous operation must be the only defined operation."
        );
        let twice = Prepared::new("query A { a } query A { b }", &variables, None).unwrap_err();
        assert_eq!(
            twice.message,
            "There can be only one operation named \"A\"."
        );

        let mutation = prepared(
            "mutation { SaveMediaListEntry(mediaId: 1) { id } }",
            json!({}),
        );
        assert_eq!(mutation.unwrap().kind(), OperationKind::Mutation);
        assert_eq!(
            prepared("subscription { x }", json!({})).unwrap().kind(),
            OperationKind::Subscription
        );
        assert_eq!(
            prepared("{ x }", json!({})).unwrap().kind(),
            OperationKind::Query
        );
    }

    #[test]
    fn a_variable_takes_the_requests_value_else_its_default() {
        let query = "query ($id: Int = 21, $type: MediaType = ANIME, $sort: [MediaSort] = [ID]) \
                     { Media(id: $id, type: $type) { id } Page { media(sort: $sort) { id } } }";
        let defaults = prepared(query, json!({})).unwrap();
        assert_eq!(
            roots(&defaults),
            [
                "Media=Media({\"id\":21,\"type\":\"ANIME\"})",
                "Page=Page({})"
            ]
        );
        let given = prepared(query, json!({ "id": 5114, "type": "MANGA" })).unwrap();
        assert_eq!(
            roots(&given),
            [
                "Media=Media({\"id\":5114,\"type\":\"MANGA\"})",
                "Page=Page({})"
            ]
        );
        // Null given is null, not the default.
        let null = prepared(query, json!({ "id": null })).unwrap();
        assert_eq!(null.variables()["id"], Value::Null);
        // A list type takes a single value as a list of one.
        let single = prepared(query, json!({ "sort": "SCORE_DESC" })).unwrap();
        assert_eq!(single.variables()["sort"], json!(["SCORE_DESC"]));

        // Left out, with no default: the argument is left out.
        let left_out = prepared(
            "query ($id: Int) { Media(id: $id, type: ANIME) { id } }",
            json!({}),
        )
        .unwrap();
        assert_eq!(roots(&left_out), ["Media=Media({\"type\":\"ANIME\"})"]);
    }

    #[test]
    fn a_variable_required_and_not_given_or_of_the_wrong_type_is_refused_as_anilist_refuses_it() {
        // AniList's own words, on 2 November 2026: and no locations.
        let missing =
            prepared("query ($id: Int!) {\n  Media(id: $id) { id }\n}", json!({})).unwrap_err();
        assert_eq!(
            missing,
            Error::invalid(
                "Variable \"$id\" of required type \"Int!\" was not provided.",
                []
            )
        );
        let null = prepared(
            "query ($id: Int!) { Media(id: $id) { id } }",
            json!({ "id": null }),
        )
        .unwrap_err();
        assert_eq!(
            null.message,
            "Variable \"$id\" of non-null type \"Int!\" must not be null."
        );
        for wrong in [
            json!("21"),
            json!(1.5),
            json!(true),
            json!(4_000_000_000_i64),
        ] {
            let refused = prepared(
                "query ($id: Int) { Media(id: $id) { id } }",
                json!({ "id": wrong }),
            );
            assert!(refused.is_err(), "{wrong}");
        }
        // A whole number written with a fraction is an Int.
        let whole = prepared(
            "query ($id: Int) { Media(id: $id) { id } }",
            json!({ "id": 21.0 }),
        )
        .unwrap();
        assert_eq!(whole.variables()["id"], json!(21));
        // AniList's own types are taken as they come.
        assert!(
            prepared(
                "query ($s: MediaSeason) { Page { media(season: $s) { id } } }",
                json!({ "s": "FALL" })
            )
            .is_ok()
        );

        let undefined = prepared("{ Media(id: $id) { id } }", json!({})).unwrap();
        let error = undefined.root_fields().err().unwrap();
        assert_eq!(error.message, "Variable \"$id\" is not defined.");
        assert_eq!(error.locations, [Location { line: 1, column: 3 }]);
    }

    #[test]
    fn fragments_aliases_and_directives_are_followed_as_graphql_gathers_fields() {
        let query = "query ($all: Boolean = false) {
            first: Media(id: 1) { ...Basics @include(if: true) }
            ... on Query { second: Media(id: 2) { id } }
            ... on Page { third: Media(id: 3) { id } }
            skipped: Media(id: 4) @skip(if: true) { id }
            maybe: Media(id: 5) @include(if: $all) { id }
            __typename
          }
          fragment Basics on Media { id title { romaji } }";
        let document = prepared(query, json!({})).unwrap();
        assert_eq!(
            roots(&document),
            [
                "first=Media({\"id\":1})",
                "second=Media({\"id\":2})",
                "__typename=__typename({})"
            ]
        );
        let all = prepared(query, json!({ "all": true })).unwrap();
        assert_eq!(roots(&all).len(), 4);

        // Below a field: the fragment's fields with the field's own.
        let fields = document.root_fields().unwrap();
        let below = document.collect("Media", &fields[0].selections).unwrap();
        let names: Vec<&str> = below.iter().map(|f| f.key).collect();
        assert_eq!(names, ["id", "title"]);
        // A fragment for another type does not apply.
        let other = document
            .collect("Character", &fields[0].selections)
            .unwrap();
        assert!(other.is_empty());
    }

    #[test]
    fn a_field_asked_twice_under_one_name_is_one_with_both_selections() {
        let document = prepared(
            "{ Media(id: 1) { id title { romaji } } Media(id: 1) { title { english } } }",
            json!({}),
        )
        .unwrap();
        let fields = document.root_fields().unwrap();
        assert_eq!(fields.len(), 1);
        let below = document.collect("Media", &fields[0].selections).unwrap();
        assert_eq!(below.len(), 2);
        let title = document
            .collect("MediaTitle", &below[1].selections)
            .unwrap();
        let names: Vec<&str> = title.iter().map(|f| f.name).collect();
        assert_eq!(names, ["romaji", "english"]);

        // Under one name, two fields or two sets of arguments conflict.
        for conflicting in [
            "{ a: Media(id: 1) { id } a: Page { pageInfo { total } } }",
            "{ Media(id: 1) { id } Media(id: 2) { id } }",
        ] {
            let error = prepared(conflicting, json!({}))
                .unwrap()
                .root_fields()
                .err();
            assert!(
                error.is_some_and(|e| e.message.starts_with("Fields \"")),
                "{conflicting}"
            );
        }
    }

    #[test]
    fn a_fragment_unknown_or_spreading_itself_and_a_directive_unknown_are_refused() {
        let unknown = prepared("{ Media(id: 21) { ...Missing } }", json!({})).unwrap();
        let fields = unknown.root_fields().unwrap();
        let error = unknown
            .collect("Media", &fields[0].selections)
            .err()
            .unwrap();
        // AniList's own words and place, on 2 November 2026.
        assert_eq!(
            error,
            Error::invalid(
                "Unknown fragment \"Missing\".",
                [Location {
                    line: 1,
                    column: 22
                }]
            )
        );

        let cycle = prepared(
            "{ Media(id: 1) { ...A } } fragment A on Media { relations { nodes { ...B } } } \
             fragment B on Media { ...A }",
            json!({}),
        );
        assert!(
            cycle
                .err()
                .is_some_and(|e| e.message.starts_with("Cannot spread fragment")),
        );
        let itself = prepared("{ ...Q } fragment Q on Query { ...Q }", json!({}));
        assert!(itself.is_err());

        for refused in [
            "{ Media(id: 1) @defer { id } }",
            "{ Media(id: 1) @include { id } }",
            "{ Media(id: 1) @skip(if: \"yes\") { id } }",
            "query @include(if: true) { Media(id: 1) { id } }",
        ] {
            let error = prepared(refused, json!({})).and_then(|d| d.root_fields().map(|_| ()));
            assert!(error.is_err(), "{refused}");
        }
        let twice = prepared(
            "{ ...A } fragment A on Query { a } fragment A on Query { b }",
            json!({}),
        );
        assert!(twice.is_err());
    }

    #[test]
    fn a_document_that_does_not_parse_is_refused_where_it_breaks() {
        // AniList: "Syntax Error: Expected Name, found <EOF>" at 1:23.
        let error = prepared("{ Media(id: 21) { id }", json!({})).err().unwrap();
        assert!(
            error.message.starts_with("Syntax Error: "),
            "{}",
            error.message
        );
        assert_eq!(error.status, 400);
        assert_eq!(error.locations.len(), 1);
        assert_eq!(error.locations[0].line, 1);
        assert!(prepared("", json!({})).is_err());
        assert!(prepared("not graphql at all", json!({})).is_err());
    }

    #[test]
    fn a_document_past_what_is_read_here_is_left_to_anilist_unread() {
        let beyond = |query: &str, variables: Value| -> Error {
            let error = match prepared(query, variables) {
                Ok(prepared) => prepared.root_fields().err().expect("refused"),
                Err(error) => error,
            };
            assert!(error.beyond, "{}", error.message);
            error
        };
        // Deep: fragments spread within fragments, each one short.
        let chain = |links: usize| {
            let mut query = String::from("{ ...F0 }");
            for at in 0..links {
                query += &format!(" fragment F{at} on Query {{ ...F{} }}", at + 1);
            }
            query + &format!(" fragment F{links} on Query {{ a }}")
        };
        let short = chain(MOST_SPREADS_DEEP - 1);
        assert!(prepared(&short, json!({})).unwrap().root_fields().is_ok());
        let error = beyond(&chain(MOST_SPREADS_DEEP), json!({}));
        assert!(error.message.contains("spread more"), "{}", error.message);
        // More fragments than a client defines.
        let error = beyond(&chain(MOST_FRAGMENTS), json!({}));
        assert!(error.message.contains("fragments"), "{}", error.message);
        // A comment ends at a carriage return, as the parser ends it: what
        // follows it is counted.
        let behind = format!(
            "#\r{}{}",
            "{a".repeat(MAX_DEPTH + 1),
            "}".repeat(MAX_DEPTH + 1)
        );
        let error = beyond(&behind, json!({}));
        assert!(error.message.contains("nested deeper"), "{}", error.message);
        assert_eq!(depth("#\r{ a }"), 1);
        assert_eq!(depth("# {\n{ a }"), 1);
        // Wide: operations, variables, fields.
        let operations: String = (0..=MOST_DEFINITIONS)
            .map(|at| format!("query o{at} {{ a }} "))
            .collect();
        beyond(&operations, json!({}));
        let variables = format!(
            "query ({}) {{ a }}",
            (0..=MOST_VARIABLES)
                .map(|at| format!("$v{at}: Int "))
                .collect::<String>()
        );
        beyond(&variables, json!({}));
        let fields = format!(
            "{{ {} }}",
            (0..=MOST_SELECTIONS)
                .map(|at| format!("f{at} "))
                .collect::<String>()
        );
        beyond(&fields, json!({}));

        // A variable used again and again weighs each time it is read, and
        // nothing more is read once the budget is spent.
        let mut reused = String::from("query ($v: [Int]) {");
        for at in 0..30 {
            reused += &format!(" a{at}: Media(id_in: $v)");
        }
        reused += " }";
        let values = json!({ "v": vec![1; 100] });
        let Value::Object(given) = &values else {
            unreachable!()
        };
        let document = Prepared::new(&reused, given, None).unwrap();
        let error = document.root_fields().err().expect("refused");
        assert!(error.beyond, "{}", error.message);
        assert!(error.message.contains("values"), "{}", error.message);
        assert!(document.values_read.load(Ordering::Relaxed) <= MOST_ARGUMENT_VALUES + 100);
        let once = "query ($v: [Int]) { Media(id_in: $v) }";
        assert!(
            prepared(once, values.clone())
                .unwrap()
                .root_fields()
                .is_ok()
        );
        // What AniList refuses is refused here still.
        let unknown = prepared("{ ...Nowhere }", json!({}))
            .and_then(|prepared| prepared.root_fields().map(|_| ()))
            .unwrap_err();
        assert!(!unknown.beyond, "{}", unknown.message);
    }

    #[test]
    fn a_document_nested_too_deep_is_refused_unread() {
        let deep = format!(
            "{}{}",
            "{ a ".repeat(MAX_DEPTH + 1),
            "}".repeat(MAX_DEPTH + 1)
        );
        let error = prepared(&deep, json!({})).err().unwrap();
        assert!(error.message.contains("nested deeper"), "{}", error.message);
        // A megabyte of braces does not reach the parser.
        let huge = "{".repeat(1024 * 1024);
        assert!(prepared(&huge, json!({})).is_err());
        // Brackets in strings and comments are not counted.
        let quoted = format!(
            "{{ Media(search: \"{}\") {{ id }} }} # {}",
            "{".repeat(100),
            "[".repeat(100)
        );
        assert!(prepared(&quoted, json!({})).is_ok());
        assert_eq!(depth("{ a { b(c: [1]) } }"), 4);
        assert_eq!(depth("\"\"\" { { \"\"\" { a }"), 1);
    }
}
