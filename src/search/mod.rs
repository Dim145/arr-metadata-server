//! Search: what a term finds in the catalogue, and in what order.
//!
//! One table, `search_doc`, holds a row for every name something is found by
//! — each title a work goes by, each person its credits name, each episode —
//! with its text folded by [`norm`] the same way on both engines. A term is
//! folded the same way and looked for in it ([`candidates`]): by the start of
//! a name, inside a name, and — when that finds too little — by the trigrams
//! a misspelling still shares. What comes back is a few hundred rows at most,
//! and [`rank`] puts them in order: exact before prefix before a word's start
//! before inside before a misspelling, then how well known what each names
//! is, the year a term ends with, and a work's own title before its others.
//! A work found by several of its names is found once, by its best.
//!
//! The rows are written by the server ([`index`] says which), in the
//! transaction that writes a work; whatever else changes a work's names marks
//! the work, and the index is written again in the background
//! (`jobs::search`).

pub mod candidates;
pub mod index;
pub mod norm;
pub mod rank;

use std::collections::HashSet;

use anyhow::Result;

pub use candidates::{Found, Scope, Taking};
pub use norm::norm;
pub use rank::Term;

use crate::db::Db;

/// What a row of the index names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Work,
    Person,
    Episode,
}

impl Kind {
    /// Where the kind sits in a row's number: its top bits, above the hash.
    const SHIFT: u32 = 59;

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Person => "person",
            Self::Episode => "episode",
        }
    }

    const fn code(self) -> i64 {
        match self {
            Self::Work => 1,
            Self::Person => 2,
            Self::Episode => 3,
        }
    }

    /// The numbers the kind's rows are filed under, from the first to the one
    /// after the last: a search of works reads the FTS5 index between them,
    /// and never an episode's row.
    pub const fn ids(self) -> (i64, i64) {
        (self.code() << Self::SHIFT, (self.code() + 1) << Self::SHIFT)
    }
}

impl std::str::FromStr for Kind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "work" => Ok(Self::Work),
            "person" => Ok(Self::Person),
            "episode" => Ok(Self::Episode),
            other => anyhow::bail!("{other:?} is not a kind of search row"),
        }
    }
}

/// Under how many things found by its letters a term is looked for as a
/// misspelling too: a term that names three works by its letters is no
/// misspelling of a fourth. Nor is one that a name is, or starts with, or
/// has a word starting with, however few it finds: it was typed to find
/// that name.
pub const FEW: usize = 3;

/// How many rows of each read a glance at the letters takes ([`forgiven`]):
/// enough to see [`FEW`] things among the several names each goes by.
const GLANCE: i64 = 50;

/// What a term finds of one kind.
#[derive(Clone, Debug, Default)]
pub struct Finds {
    /// The rows found by the term's letters.
    pub by_letters: Vec<Found>,
    /// The rows a misspelt term resembles, when its letters found fewer than
    /// [`FEW`] things — only those the ranking takes for one.
    pub resembling: Vec<Found>,
    /// Whether the rows found by the letters are every row they match, no
    /// read having stopped at the scope's limit: then nothing past them is
    /// left for a page further down to find.
    pub complete: bool,
}

impl Finds {
    /// Every row found, each once.
    pub fn all(&self) -> impl Iterator<Item = &Found> {
        self.by_letters.iter().chain(self.resembling.iter())
    }
}

/// What a row names: its work for a work's row, else the row's own subject —
/// an episode, a person.
pub fn subject(found: &Found) -> &str {
    match found.kind {
        Kind::Work => found.work_id.as_deref().unwrap_or(&found.ref_id),
        _ => &found.ref_id,
    }
}

/// Look a term up in one kind of row.
pub async fn find(db: &Db, term: &Term, scope: &Scope) -> Result<Finds> {
    let mut finds = Finds {
        complete: true,
        ..Finds::default()
    };
    if term.is_empty() {
        return Ok(finds);
    }

    let mut seen: HashSet<i64> = HashSet::new();
    let (rows, complete) = by_letters(db, term, scope, Taking::BestKnown, &mut seen).await?;
    finds.by_letters = rows;
    finds.complete = complete;
    if unmeant(term, scope, &finds.by_letters) {
        finds.resembling = resembled(db, term, scope, &seen).await?;
    }
    Ok(finds)
}

/// What a misspelt term is taken for, and that alone: the rows [`find`]
/// would add to those of its letters, for a caller with no use for the
/// letters' pool — a count, the facets, a list in another order.
///
/// The letters are glanced at first, [`GLANCE`] rows of each read in
/// whatever order the index holds them. Once those name [`FEW`] things, or
/// one the term was typed to find, nothing is forgiven, and the reads by
/// popularity — which cost a term found in thousands of names its thousands
/// of rows, to sort them — are never made.
pub async fn forgiven(db: &Db, term: &Term, scope: &Scope) -> Result<Vec<Found>> {
    glancing(db, term, scope, GLANCE).await
}

/// [`forgiven`], glancing at `most` rows of each read.
async fn glancing(db: &Db, term: &Term, scope: &Scope, most: i64) -> Result<Vec<Found>> {
    if term.is_empty() || !scope.forgive || scope.kind == Kind::Episode {
        return Ok(Vec::new());
    }
    let glance = Scope {
        limit: most,
        ..*scope
    };
    let mut seen: HashSet<i64> = HashSet::new();
    let (rows, complete) = by_letters(db, term, &glance, Taking::First, &mut seen).await?;
    if !unmeant(term, scope, &rows) {
        return Ok(Vec::new());
    }
    if !complete {
        // Names enough to fill a glance, yet fewer than FEW things: which
        // of them the term meant, only the whole look can say.
        return Ok(find(db, term, scope).await?.resembling);
    }
    resembled(db, term, scope, &seen).await
}

/// The rows a term's letters find under each of its readings, each once,
/// and whether they are all the rows there are.
async fn by_letters(
    db: &Db,
    term: &Term,
    scope: &Scope,
    taking: Taking,
    seen: &mut HashSet<i64>,
) -> Result<(Vec<Found>, bool)> {
    let mut rows = Vec::new();
    let mut complete = true;
    for form in term.forms() {
        let (found, whole) = candidates::matching(db, form, scope, taking).await?;
        complete &= whole;
        for row in found {
            if seen.insert(row.id) {
                rows.push(row);
            }
        }
    }
    Ok((rows, complete))
}

/// Whether what the letters found leaves a misspelling to look for: fewer
/// than [`FEW`] things, none by a name the term is, starts, or starts a word
/// of. Never for an episode's title — a misspelling of one would bring a
/// thousand episodes that share a word — nor for a scope that forgives
/// nothing.
fn unmeant(term: &Term, scope: &Scope, rows: &[Found]) -> bool {
    if !scope.forgive || scope.kind == Kind::Episode {
        return false;
    }
    let named: HashSet<&str> = rows.iter().map(subject).collect();
    named.len() < FEW && !rows.iter().any(|row| rank::begins(term, &row.norm))
}

/// The rows a misspelt term resembles that the ranking takes for one, past
/// those its letters found.
async fn resembled(db: &Db, term: &Term, scope: &Scope, seen: &HashSet<i64>) -> Result<Vec<Found>> {
    Ok(candidates::resembling(db, &term.whole.norm, scope)
        .await?
        .into_iter()
        .filter(|row| {
            !seen.contains(&row.id) && rank::score(term, &row.norm, None, None, false).is_some()
        })
        .collect())
}

/// What each thing found is worth for the term: by the best of its rows, the
/// best first. `own_title` says, of a row, whether its name is the title the
/// thing is shown under; `year` and `popularity` are what it is known by,
/// where the caller has better than the rows say.
pub fn best_first<'a>(
    term: &Term,
    rows: impl IntoIterator<Item = &'a Found>,
    weigh: impl Fn(&Found) -> (Option<i32>, Option<f64>, bool),
) -> Vec<(String, f64)> {
    let mut best: Vec<(String, f64)> = Vec::new();
    for row in rows {
        let (year, popularity, own_title) = weigh(row);
        let Some(score) = rank::score(term, &row.norm, year, popularity, own_title) else {
            continue;
        };
        let key = subject(row);
        match best.iter_mut().find(|(seen, _)| seen == key) {
            Some((_, kept)) => *kept = kept.max(score),
            None => best.push((key.to_string(), score)),
        }
    }
    best.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    best
}

#[cfg(test)]
mod tests;
