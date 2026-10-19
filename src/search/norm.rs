//! A name as the index holds it, and the trigrams two names are compared by.
//!
//! Folded by the server, not by the database: SQLite's `LOWER` folds ASCII
//! alone and its trigram tokenizer's `remove_diacritics` turns off the indexed
//! `LIKE`, PostgreSQL's `unaccent` is `STABLE` and wants a wrapper before an
//! index will take it, and neither folds what the other does. Folded here,
//! both engines are given the same text, and a term is folded the same way
//! before it is looked for in it.

use unicode_normalization::UnicodeNormalization as _;
use unicode_properties::general_category::{GeneralCategoryGroup, UnicodeGeneralCategory as _};

/// A name, folded: compatibility-decomposed (`ﬁ` is `fi`, a full-width `Ａ`
/// an `A`), its combining accents taken off — those of U+0300 to U+036F
/// alone, so that `é` is `e` while `が` keeps the mark that tells it from
/// `か` — composed again, in lower case, `ß` as `ss`. An apostrophe joins
/// what it stands between, so that *Grey's* is found by `greys` as by
/// `grey's`; every other character that is neither a letter, a digit nor a
/// mark is a space, and the spaces are one between words, none at the ends.
pub fn norm(text: &str) -> String {
    let composed: String = text
        .nfkd()
        .filter(|c| !('\u{0300}'..='\u{036F}').contains(c))
        .nfc()
        .collect();

    let mut out = String::with_capacity(composed.len());
    let mut gap = false;
    for c in composed.chars() {
        if is_apostrophe(c) {
            continue;
        }
        match c.general_category_group() {
            GeneralCategoryGroup::Letter
            | GeneralCategoryGroup::Number
            | GeneralCategoryGroup::Mark => {
                if gap && !out.is_empty() {
                    out.push(' ');
                }
                gap = false;
                fold(c, &mut out);
            }
            _ => gap = true,
        }
    }
    out
}

/// One character in lower case, with the two letters whose full case folding
/// lower case leaves alone: `ß`, which is `ss`, and the final sigma `ς`, which
/// is `σ` wherever it stands.
fn fold(c: char, out: &mut String) {
    for lower in c.to_lowercase() {
        match lower {
            'ß' => out.push_str("ss"),
            'ς' => out.push('σ'),
            other => out.push(other),
        }
    }
}

/// The apostrophes a title is written with: the typewriter's, the
/// typographer's, the letter one and the full-width one (which the
/// decomposition has already made a typewriter's).
fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '\u{2018}' | '\u{02BC}')
}

/// Three characters of a folded name.
pub type Trigram = [char; 3];

/// The trigrams of one word, as `pg_trgm` takes them: with two spaces before
/// it and one after, so that a word's start weighs more than its middle and
/// `ark` is not mistaken for `park`.
fn word_trigrams(word: &str) -> Vec<Trigram> {
    let padded: Vec<char> = "  ".chars().chain(word.chars()).chain([' ']).collect();
    padded
        .windows(3)
        .map(|window| [window[0], window[1], window[2]])
        .collect()
}

/// The most of a term's trigrams one measure keeps count of: a term longer
/// than a sentence is measured by its first hundred and twenty-eight.
const MOST_TRIGRAMS: usize = 128;

/// A term's trigrams, each once, for names to be measured against it.
#[derive(Clone, Debug, Default)]
pub struct TermTrigrams {
    sorted: Vec<Trigram>,
    words: usize,
}

impl TermTrigrams {
    pub fn of(norm: &str) -> Self {
        let words: Vec<&str> = norm.split(' ').filter(|w| !w.is_empty()).collect();
        let mut sorted: Vec<Trigram> = words.iter().flat_map(|w| word_trigrams(w)).collect();
        sorted.sort_unstable();
        sorted.dedup();
        sorted.truncate(MOST_TRIGRAMS);
        Self {
            sorted,
            words: words.len(),
        }
    }

    /// How much of the term a name holds, from 0 to 1: the share of the
    /// term's trigrams found in the run of the name's words that holds the
    /// most of them — a run as long as the term, or a word longer or shorter.
    /// `pg_trgm`'s word similarity, taken by whole words: a name that holds
    /// the term whole, inside a longer one, holds all of it, and a long name
    /// is not taken for a short term because its words are spread over it —
    /// *One Piece Log: Fish-Man Island Saga* holds `man`, `and` and an `o`
    /// to start a word, half of `ozymand`, in three different words.
    pub fn held_by(&self, name: &str) -> f64 {
        if self.sorted.is_empty() {
            return 0.0;
        }
        // Which of the term's trigrams each word of the name holds, as bits.
        let masks: Vec<u128> = name
            .split(' ')
            .filter(|w| !w.is_empty())
            .map(|word| {
                word_trigrams(word)
                    .into_iter()
                    .filter_map(|gram| self.sorted.binary_search(&gram).ok())
                    .fold(0u128, |mask, at| mask | (1u128 << at))
            })
            .collect();
        let mut best = 0u32;
        for size in self.words.saturating_sub(1).max(1)..=self.words + 1 {
            for run in masks.windows(size.min(masks.len()).max(1)) {
                best = best.max(run.iter().fold(0u128, |all, mask| all | mask).count_ones());
            }
        }
        f64::from(best) / self.sorted.len() as f64
    }
}

/// The trigrams an FTS5 trigram index files a folded term under — every run
/// of three characters, the spaces in it included — less those that hold a
/// space, which every other name shares, each once and in order.
pub fn indexed_trigrams(norm: &str) -> Vec<String> {
    let chars: Vec<char> = norm.chars().collect();
    let mut found: Vec<String> = Vec::new();
    for window in chars.windows(3) {
        if window.contains(&' ') {
            continue;
        }
        let gram: String = window.iter().collect();
        if !found.contains(&gram) {
            found.push(gram);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accents_and_case_fall_away_and_kana_keep_their_marks() {
        assert_eq!(norm("Été 85"), "ete 85");
        assert_eq!(norm("ÉTÉ"), norm("été"));
        assert_eq!(norm("Amélie"), "amelie");
        assert_eq!(norm("Straße"), "strasse");
        assert_eq!(norm("ΟΔΥΣΣΕΙΑΣ"), norm("οδυσσειας"));
        // A voiced kana is another letter, not an accented one: が is not か.
        assert_eq!(norm("が"), "が");
        assert_ne!(norm("がっこう"), norm("かっこう"));
        // Half-width katakana are the full-width ones, mark and all.
        assert_eq!(norm("ｶﾞｯｺｳ"), norm("ガッコウ"));
        // The other scripts' marks are part of their letters.
        assert_eq!(norm("हिन्दी"), "हिन्दी");
    }

    #[test]
    fn punctuation_is_a_space_and_an_apostrophe_joins() {
        assert_eq!(norm("Spider-Man: No Way Home"), "spider man no way home");
        assert_eq!(norm("  Law & Order  "), "law order");
        assert_eq!(norm("Grey's Anatomy"), "greys anatomy");
        assert_eq!(norm("Grey’s Anatomy"), "greys anatomy");
        assert_eq!(norm("L'Été"), "lete");
        assert_eq!(norm("snake_case"), "snake case");
        assert_eq!(norm("50% Off"), "50 off");
        assert_eq!(norm("%"), "");
        assert_eq!(norm("…"), "");
    }

    #[test]
    fn compatibility_forms_are_their_plain_letters() {
        assert_eq!(norm("ＷＡＬＬ·Ｅ"), "wall e");
        assert_eq!(norm("ﬁnding"), "finding");
        assert_eq!(norm("Rocky Ⅱ"), "rocky ii");
    }

    #[test]
    fn a_misspelt_term_shares_most_of_its_trigrams_with_the_name() {
        let held = |term: &str, name: &str| TermTrigrams::of(term).held_by(name);
        assert!(held("breking bad", "breaking bad") > 0.8);
        assert!(held("mistery", "mystery") >= 0.6);
        assert!(held("spiderman", "spider man") >= 0.75);
        assert!(held("lvoe story", "love story") >= 0.6);
        // A name holding the term whole holds all of it.
        assert_eq!(held("wenders", "wim wenders"), 1.0);
        assert!(held("wendres", "wim wenders") >= 0.5);
        // One letter in four is another word.
        assert!(held("lost", "last") < 0.5);
        assert!(held("dark", "park") < 0.5);
        // Nor is a long name the term because its trigrams are spread over
        // its words.
        assert!(held("ozymand", "one piece log fish man island saga") < 0.5);
        assert!(held("casa de papel", "el club de la pelea") < 0.5);
    }

    #[test]
    fn the_index_is_asked_for_the_trigrams_that_hold_no_space() {
        assert_eq!(indexed_trigrams("blod moon"), ["blo", "lod", "moo", "oon"]);
        assert_eq!(indexed_trigrams("aaaa"), ["aaa"]);
        assert!(indexed_trigrams("ab").is_empty());
    }
}
