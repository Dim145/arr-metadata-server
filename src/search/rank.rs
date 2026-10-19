//! How well a name answers a term, and the order the answers come in.
//!
//! What the index finds is a few hundred names at most, and they are put in
//! order here rather than by either engine: a name that is the term, then
//! one that starts with it, then one with a word that does, then one that
//! holds it somewhere, then one that shares most of a misspelt term's
//! trigrams — and within each, the closer the name is to the term's length
//! the better. Over that, how well known what it names is (the logarithm of
//! its popularity, so that the most popular work does not leap over every
//! better-named one), a year the term ends with, and whether the name is the
//! work's own title rather than one of its others.

use super::norm::{TermTrigrams, norm};

/// The least share of a misspelt term's trigrams a name must hold to be
/// taken for it. Half: `mistery` holds five eighths of `mystery`'s, `lost`
/// two fifths of `last`'s.
pub const RESEMBLES: f64 = 0.5;

/// A term as it is looked for: folded, and — when it ends with a year and
/// has something before it, `dune 2021` — read a second way, as the title
/// before the year and the year the work came out.
#[derive(Clone, Debug)]
pub struct Term {
    pub whole: Form,
    pub dated: Option<(Form, i32)>,
}

/// One way of reading a term: its folded text, and that text's trigrams.
#[derive(Clone, Debug)]
pub struct Form {
    pub norm: String,
    trigrams: TermTrigrams,
}

impl Form {
    fn of(norm: String) -> Self {
        let trigrams = TermTrigrams::of(&norm);
        Self { norm, trigrams }
    }
}

/// The years a term may end with: none before the cinema, none so far ahead
/// that it can only be a number in the title.
const YEARS: std::ops::RangeInclusive<i32> = 1870..=2100;

impl Term {
    pub fn parse(text: &str) -> Self {
        let whole = norm(text);
        let dated = whole.rsplit_once(' ').and_then(|(before, last)| {
            let year: i32 = last.parse().ok().filter(|y| YEARS.contains(y))?;
            (last.len() == 4 && !before.is_empty()).then(|| (Form::of(before.to_string()), year))
        });
        Self {
            whole: Form::of(whole),
            dated,
        }
    }

    /// Whether there is anything to look for: a term of punctuation alone
    /// folds to nothing, and finds nothing rather than everything.
    pub fn is_empty(&self) -> bool {
        self.whole.norm.is_empty()
    }

    /// The texts to look the term up by: the whole of it, then the title
    /// before its year.
    pub fn forms(&self) -> Vec<&str> {
        let mut forms = vec![self.whole.norm.as_str()];
        if let Some((form, _)) = &self.dated {
            forms.push(form.norm.as_str());
        }
        forms
    }
}

/// How a name stands to a term, worst first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// It shares most of the term's trigrams: the term misspelt, or its words
    /// in another order.
    Resembles,
    /// It holds the term, inside a word.
    Holds,
    /// A word of it starts with the term.
    WordStart,
    /// It starts with the term.
    Start,
    /// It is the term.
    Exact,
}

impl Class {
    /// What the class is worth before the rest is weighed: twenty points apart,
    /// more than any popularity adds, so that a better name is not passed by
    /// a better-known work except when that one is very much better known.
    const fn base(self) -> f64 {
        match self {
            Self::Exact => 100.0,
            Self::Start => 80.0,
            Self::WordStart => 62.0,
            Self::Holds => 48.0,
            Self::Resembles => 18.0,
        }
    }
}

/// How well a name answers one reading of a term, or nothing when it does
/// not: its class, and within it a little for being close to the term's
/// length, or for sharing more of its trigrams.
fn closeness(form: &Form, name: &str) -> Option<(Class, f64)> {
    let term = form.norm.as_str();
    if term.is_empty() || name.is_empty() {
        return None;
    }
    // Up to ten points within a class, for a name that is mostly the term.
    let near = 10.0 * term.chars().count() as f64 / name.chars().count().max(1) as f64;
    let class = if name == term {
        return Some((Class::Exact, Class::Exact.base()));
    } else if name.starts_with(term) {
        Class::Start
    } else if name.contains(&format!(" {term}")) {
        Class::WordStart
    } else if name.contains(term) {
        Class::Holds
    } else {
        let held = form.trigrams.held_by(name);
        if held < RESEMBLES {
            return None;
        }
        // Up to thirty points, from none for a name that holds just enough
        // of the term's trigrams to all of them: spread over that range, so
        // that how close a name is weighs more than how well known it is —
        // `breking bad` is Breaking Bad before The Breaking Point, however
        // popular that — and a misspelling found whole still comes after a
        // name that holds the term.
        let closer = (held - RESEMBLES) / (1.0 - RESEMBLES);
        return Some((Class::Resembles, Class::Resembles.base() + 30.0 * closer));
    };
    Some((class, class.base() + near.min(10.0)))
}

/// Whether a name is the term, or starts with it, or has a word that does —
/// read whole or without the year it ends with: a name the term was typed
/// to find, rather than one it happens to be written inside.
pub fn begins(term: &Term, name: &str) -> bool {
    std::iter::once(&term.whole)
        .chain(term.dated.as_ref().map(|(form, _)| form))
        .filter_map(|form| closeness(form, name))
        .any(|(class, _)| class >= Class::WordStart)
}

/// What one name of something is worth for a term, or nothing when the name
/// does not answer it.
///
/// `year` and `popularity` are what the name names — a work's, or for an
/// episode the series' popularity and the episode's year; a person's
/// popularity is how many works credit them. `own_title` says whether the
/// name is the work's title as it is shown, which comes before one of its
/// other names when both answer alike.
pub fn score(
    term: &Term,
    name: &str,
    year: Option<i32>,
    popularity: Option<f64>,
    own_title: bool,
) -> Option<f64> {
    let whole = closeness(&term.whole, name).map(|(_, points)| points);
    // Read as a title and a year, the term counts for less than read whole,
    // unless the year is the work's: `blade runner 2049` is the sequel's
    // name before it is the first film in 2049, and `dune 2021` is the 2021
    // film before the 1984 one.
    let dated = term.dated.as_ref().and_then(|(form, wanted)| {
        let (_, points) = closeness(form, name)?;
        let year_points = match year {
            Some(year) if year == *wanted => 20.0,
            Some(year) if (year - wanted).abs() == 1 => 8.0,
            _ => 0.0,
        };
        Some(points - 15.0 + year_points)
    });
    let text = match (whole, dated) {
        (Some(a), Some(b)) => a.max(b),
        (a, b) => a.or(b)?,
    };

    let known = 3.0 * (1.0 + popularity.unwrap_or(0.0).max(0.0)).ln();
    let own = if own_title { 4.0 } else { 0.0 };
    Some(text + known + own)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order<'a>(term: &str, names: &[(&'a str, Option<i32>, f64)]) -> Vec<&'a str> {
        let term = Term::parse(term);
        let mut scored: Vec<(f64, &str)> = names
            .iter()
            .filter_map(|(name, year, popularity)| {
                score(&term, &norm(name), *year, Some(*popularity), true).map(|s| (s, *name))
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.into_iter().map(|(_, name)| name).collect()
    }

    #[test]
    fn a_name_that_is_the_term_comes_before_one_that_starts_with_it() {
        assert_eq!(
            order(
                "dark",
                &[
                    ("The Dark Knight", None, 80.0),
                    ("Dark", None, 40.0),
                    ("Darkness Falls", None, 5.0),
                    ("Park Life", None, 500.0),
                ]
            ),
            ["Dark", "Darkness Falls", "The Dark Knight"],
            "and a word that starts with it after that; a name it is not in is not found"
        );
    }

    #[test]
    fn a_misspelt_term_finds_the_name_it_misspells() {
        let found = order(
            "breking bad",
            &[
                ("Bad Boys", None, 90.0),
                ("Breaking Dawn", None, 120.0),
                ("Breaking Bad", None, 300.0),
            ],
        );
        assert_eq!(found.first(), Some(&"Breaking Bad"), "{found:?}");
        assert!(
            !found.contains(&"Bad Boys"),
            "one word in common is not a misspelling"
        );
        assert_eq!(
            order("wendres", &[("Wim Wenders", None, 12.0)]),
            ["Wim Wenders"]
        );
    }

    #[test]
    fn popularity_orders_names_that_answer_alike() {
        assert_eq!(
            order(
                "the office",
                &[
                    ("The Office", Some(2001), 30.0),
                    ("The Office", Some(2005), 400.0)
                ]
            ),
            ["The Office", "The Office"]
        );
        let term = Term::parse("the office");
        let known = score(&term, "the office", None, Some(400.0), true).unwrap();
        let obscure = score(&term, "the office", None, Some(30.0), true).unwrap();
        assert!(known > obscure);
    }

    #[test]
    fn a_year_at_the_end_is_the_works_year_unless_it_is_part_of_a_title() {
        assert_eq!(
            order(
                "dune 2021",
                &[("Dune", Some(1984), 60.0), ("Dune", Some(2021), 50.0)]
            )
            .len(),
            2
        );
        let term = Term::parse("dune 2021");
        let new = score(&term, "dune", Some(2021), Some(50.0), true).unwrap();
        let old = score(&term, "dune", Some(1984), Some(60.0), true).unwrap();
        assert!(new > old, "{new} against {old}");

        let term = Term::parse("blade runner 2049");
        let sequel = score(&term, "blade runner 2049", Some(2017), Some(80.0), true).unwrap();
        let first = score(&term, "blade runner", Some(1982), Some(90.0), true).unwrap();
        assert!(sequel > first, "{sequel} against {first}");
    }

    #[test]
    fn a_term_of_punctuation_alone_is_nothing_to_look_for() {
        assert!(Term::parse("%").is_empty());
        assert!(Term::parse(" !? ").is_empty());
        assert!(!Term::parse("50%").is_empty());
        assert!(
            Term::parse("2049").dated.is_none(),
            "a year alone is a title"
        );
    }
}
