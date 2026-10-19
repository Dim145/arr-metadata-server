//! What the index is asked, on each engine: the rows a term may name, a few
//! hundred at most, for [`super::rank`] to put in order.
//!
//! Two reads by the term's letters, then a third only when they found too
//! little. First the names that are the term or start with it, read in order
//! off `(kind, norm)`; then, for a term of three letters or more, the names
//! that hold it — SQLite's FTS5 trigram table matched with the term as one
//! phrase, PostgreSQL's trigram index with `LIKE '%term%'`. Each read takes the
//! best-known rows first, so that a term found in thousands of names still
//! brings the works a reader most likely means. The third read forgives a
//! misspelling: the names sharing most of the term's trigrams — counted on
//! SQLite, one trigram's rows after another, and `word_similarity` on
//! PostgreSQL, under a threshold set for the one transaction.

use anyhow::Result;
use sqlx::{Arguments, any::AnyArguments};

use super::{Kind, norm::indexed_trigrams};
use crate::{
    db::{Db, Dialect, RowExt},
    domain::MediaKind,
};

/// A row the index answered with.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub id: i64,
    pub kind: Kind,
    pub ref_id: String,
    pub work_id: Option<String>,
    pub lang: Option<String>,
    pub norm: String,
    pub text: String,
    pub popularity: Option<f64>,
    pub year: Option<i32>,
}

/// What a read may return, for whom.
#[derive(Clone, Copy, Debug)]
pub struct Scope {
    pub kind: Kind,
    /// Works of this kind only: series, or films. An episode's work is a
    /// series whatever this says.
    pub media_kind: Option<MediaKind>,
    pub include_disabled: bool,
    pub include_adult: bool,
    /// The most rows one read returns.
    pub limit: i64,
    /// Whether a term whose letters find little is looked for as a
    /// misspelling too: not for the clients' lookups, which TMDB's own search
    /// follows, and where a work held here that only resembles the title
    /// would come first in the list a person picks from.
    pub forgive: bool,
}

/// Which rows a read by the letters takes, when there are more than it may.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taking {
    /// The best-known first: the pool the ranking draws from. A term found
    /// in thousands of names pays for reading them all, to sort them.
    BestKnown,
    /// Whichever the index comes to first, read no further than the limit:
    /// enough to tell what the letters find at all.
    First,
}

/// The least a misspelt term is worth asking about: under four letters, a
/// misspelling of it is any other short word.
const RESEMBLES_FROM: usize = 4;

/// The most trigrams the third read asks for: a whole sentence typed in is
/// read by its first twelve.
const MOST_TRIGRAMS: usize = 12;

/// The share of a term's trigrams PostgreSQL asks a name to hold before it is
/// read, a little under what the ranking takes for a misspelling
/// ([`super::rank::RESEMBLES`]): the ranking decides, the index only has to
/// bring it.
const POSTGRES_THRESHOLD: &str = "0.4";

const COLUMNS: &str = "d.id AS id, d.kind AS kind, d.ref_id AS ref_id, d.work_id AS work_id, \
                       d.lang AS lang, d.norm AS norm, d.text AS text, \
                       d.popularity AS popularity, d.year AS year";

/// How the tables of a read are joined, in the order they are written.
///
/// On SQLite a `CROSS JOIN`: its own way of saying "in this order". Left to
/// itself, its planner — not knowing what the bound range of row numbers
/// holds — read the works of one kind first and ran the trigram match again
/// for each of them, twenty thousand series twenty thousand times: a search
/// of series took fifteen seconds where the match first takes sixty
/// milliseconds. PostgreSQL knows what its tables hold, and takes no `ON` after
/// a `CROSS JOIN` anyway.
fn join(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Sqlite => "CROSS JOIN",
        Dialect::Postgres => "JOIN",
    }
}

/// What joins a row to the work it belongs to, and what that work must be.
/// Nothing for a person, who is checked once found: a person belongs to
/// every work that credits them.
fn shown(dialect: Dialect, scope: &Scope) -> (String, String) {
    if scope.kind == Kind::Person {
        return (String::new(), String::new());
    }
    let mut conditions = String::new();
    if !scope.include_disabled {
        conditions.push_str(" AND m.is_enabled = 1");
    }
    if !scope.include_adult {
        conditions.push_str(" AND m.is_adult = 0");
    }
    // Literals of this crate, never a caller's text.
    let kind = match scope.kind {
        Kind::Episode => Some(MediaKind::Series),
        _ => scope.media_kind,
    };
    match kind {
        Some(MediaKind::Series) => conditions.push_str(" AND m.kind = 'series'"),
        Some(MediaKind::Movie) => conditions.push_str(" AND m.kind = 'movie'"),
        None => {}
    }
    (
        format!(" {} media_item m ON m.id = d.work_id", join(dialect)),
        conditions,
    )
}

fn found(row: &sqlx::any::AnyRow) -> Result<Found> {
    Ok(Found {
        id: row.big("id")?,
        kind: row.text("kind")?.parse()?,
        ref_id: row.text("ref_id")?,
        work_id: row.opt_text("work_id")?,
        lang: row.opt_text("lang")?,
        norm: row.text("norm")?,
        text: row.text("text")?,
        popularity: row.opt_real("popularity")?,
        year: row.opt_int("year")?,
    })
}

/// The upper bound of the names that start with `term`, for a range read in
/// byte order: no character sorts after U+10FFFF.
fn after_prefix(term: &str) -> String {
    format!("{term}\u{10FFFF}")
}

/// Text as a `LIKE` pattern matches it, its wildcards escaped. A folded term
/// holds none — punctuation is a space by then — but the escape costs
/// nothing, and a pattern is never trusted to have been folded.
fn like_escaped(text: &str) -> String {
    text.replace('\\', r"\\")
        .replace('%', r"\%")
        .replace('_', r"\_")
}

/// A folded term as one FTS5 phrase: quoted, so that nothing in it is read as
/// the query syntax.
fn phrase(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

/// How a read by the letters orders its rows before its limit.
fn taken(taking: Taking) -> &'static str {
    match taking {
        Taking::BestKnown => " ORDER BY d.popularity DESC NULLS LAST, d.id",
        Taking::First => "",
    }
}

/// The read of the names that hold a term, for a scope: on SQLite the
/// trigram match first, then its rows, then their works.
fn inside_read(dialect: Dialect, scope: &Scope, taking: Taking) -> String {
    let kind = scope.kind.as_str();
    let (joined, conditions) = shown(dialect, scope);
    let cross = join(dialect);
    let order = taken(taking);
    match dialect {
        Dialect::Sqlite => format!(
            "SELECT {COLUMNS} FROM search_fts f {cross} search_doc d ON d.id = f.rowid{joined}
             WHERE search_fts MATCH ? AND f.rowid >= ? AND f.rowid < ?{conditions}{order} LIMIT ?"
        ),
        Dialect::Postgres => format!(
            "SELECT {COLUMNS} FROM search_doc d{joined}
             WHERE d.kind = '{kind}' AND d.norm LIKE ? ESCAPE '\\'{conditions}{order} LIMIT ?"
        ),
    }
}

/// The rows whose names are `term`, start with it, or — for a term of three
/// letters or more — hold it: a scope's limit apiece, taken as `taking` says.
/// And whether that is every one of them: none of the reads stopped at its
/// limit.
pub async fn matching(
    db: &Db,
    term: &str,
    scope: &Scope,
    taking: Taking,
) -> Result<(Vec<Found>, bool)> {
    if term.is_empty() {
        return Ok((Vec::new(), true));
    }
    let kind = scope.kind.as_str();
    let (joined, conditions) = shown(db.dialect(), scope);
    let order = taken(taking);

    let mut rows = Vec::new();

    let starting = match db.dialect() {
        Dialect::Sqlite => format!(
            "SELECT {COLUMNS} FROM search_doc d{joined}
             WHERE d.kind = '{kind}' AND d.norm >= ? AND d.norm < ?{conditions}{order} LIMIT ?"
        ),
        Dialect::Postgres => format!(
            "SELECT {COLUMNS} FROM search_doc d{joined}
             WHERE d.kind = '{kind}' AND d.norm LIKE ? ESCAPE '\\'{conditions}{order} LIMIT ?"
        ),
    };
    let mut args = AnyArguments::default();
    match db.dialect() {
        Dialect::Sqlite => {
            args.add(term.to_string())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            args.add(after_prefix(term))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        Dialect::Postgres => {
            args.add(format!("{}%", like_escaped(term)))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
    }
    args.add(scope.limit).map_err(|e| anyhow::anyhow!("{e}"))?;
    let read = sqlx::query_with(db.sql(&starting), args)
        .fetch_all(db.pool())
        .await?;
    let mut complete = (read.len() as i64) < scope.limit;
    rows.extend(read);

    // A trigram needs three letters: anything shorter is looked for at the
    // start of a name alone, which is the read above.
    if term.chars().count() >= 3 {
        let (from, to) = scope.kind.ids();
        let inside = inside_read(db.dialect(), scope, taking);
        let mut args = AnyArguments::default();
        match db.dialect() {
            Dialect::Sqlite => {
                args.add(phrase(term)).map_err(|e| anyhow::anyhow!("{e}"))?;
                args.add(from).map_err(|e| anyhow::anyhow!("{e}"))?;
                args.add(to).map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            Dialect::Postgres => {
                args.add(format!("%{}%", like_escaped(term)))
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
        }
        args.add(scope.limit).map_err(|e| anyhow::anyhow!("{e}"))?;
        let read = sqlx::query_with(db.sql(&inside), args)
            .fetch_all(db.pool())
            .await?;
        complete &= (read.len() as i64) < scope.limit;
        rows.extend(read);
    }

    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Found> = Vec::with_capacity(rows.len());
    for row in &rows {
        let row = found(row)?;
        if seen.insert(row.id) {
            out.push(row);
        }
    }
    Ok((out, complete))
}

/// The rows whose names share most of a misspelt term's trigrams, the closest
/// first: nothing for a term too short to be misspelt, and nothing on a
/// PostgreSQL without `pg_trgm`.
pub async fn resembling(db: &Db, term: &str, scope: &Scope) -> Result<Vec<Found>> {
    if term.chars().count() < RESEMBLES_FROM {
        return Ok(Vec::new());
    }
    let (joined, conditions) = shown(db.dialect(), scope);
    let cross = join(db.dialect());
    let rows =
        match db.dialect() {
            Dialect::Sqlite => {
                let grams: Vec<String> = indexed_trigrams(term)
                    .into_iter()
                    .take(MOST_TRIGRAMS)
                    .collect();
                if grams.is_empty() {
                    return Ok(Vec::new());
                }
                let (from, to) = scope.kind.ids();
                let sql = format!(
                    "SELECT {COLUMNS} FROM ({}) f {cross} search_doc d ON d.id = f.id{joined}
                     WHERE 1 = 1{conditions}
                     ORDER BY f.shared DESC, d.popularity DESC NULLS LAST, d.id LIMIT ?",
                    most_shared(grams.len())
                );
                let mut query = sqlx::query(db.sql(&sql));
                for gram in &grams {
                    query = query.bind(phrase(gram)).bind(from).bind(to);
                }
                query
                    .bind(scope.limit * SHARED_OVER)
                    .bind(scope.limit)
                    .fetch_all(db.pool())
                    .await?
            }
            Dialect::Postgres => {
                if !has_trigrams(db).await? {
                    return Ok(Vec::new());
                }
                let kind = scope.kind.as_str();
                let sql = format!(
                    "SELECT {COLUMNS} FROM search_doc d{joined}
                 WHERE d.kind = '{kind}' AND ? <% d.norm{conditions}
                 ORDER BY word_similarity(?, d.norm) DESC, d.popularity DESC NULLS LAST, d.id
                 LIMIT ?"
                );
                // The threshold is the operator's, so it is set for this
                // transaction alone rather than for whoever has the connection
                // next.
                let mut tx = db.pool().begin().await?;
                sqlx::query(db.sql(
                    "SELECT set_config('pg_trgm.word_similarity_threshold', ?, true) AS applied",
                ))
                .bind(POSTGRES_THRESHOLD)
                .fetch_one(&mut *tx)
                .await?;
                let rows = sqlx::query(db.sql(&sql))
                    .bind(term)
                    .bind(term)
                    .bind(scope.limit)
                    .fetch_all(&mut *tx)
                    .await?;
                tx.commit().await?;
                rows
            }
        };
    rows.iter().map(found).collect()
}

/// On SQLite, the rows holding most of `grams` trigrams of a term — each
/// trigram's own match, its rows counted together — as `(id, shared)`, the
/// most shared first: `?` for each trigram's phrase and the kind's range of
/// rows, then the most rows.
///
/// Not an OR of the trigrams ordered by FTS5's `rank`: bm25 is worked out
/// for every row an OR matches — tens of thousands, for the trigrams of a
/// common word — and it favours the shortest names, not those sharing the
/// most. Measured over fifty thousand works, three hundred titles each
/// misspelt by a letter: either read brought the title back among its 200
/// rows nineteen times in twenty, the count in a fifth of the time (17 ms
/// on average against 89, 62 at worst against 265); where one word made up
/// most titles, the OR brought it back a third of the time, the count nine
/// times in ten.
fn most_shared(grams: usize) -> String {
    let each = "SELECT rowid AS id FROM search_fts
                WHERE search_fts MATCH ? AND rowid >= ? AND rowid < ?";
    format!(
        "SELECT id, COUNT(*) AS shared FROM ({}) GROUP BY id ORDER BY shared DESC LIMIT ?",
        vec![each; grams].join(" UNION ALL ")
    )
}

/// How many more rows the trigrams' count keeps than the read returns: the
/// works the reader may not see are taken out after it, and a few among the
/// rows sharing as much as the last kept are kept too.
const SHARED_OVER: i64 = 2;

/// Whether this PostgreSQL has `pg_trgm`, without which nothing is forgiven.
pub async fn has_trigrams(db: &Db) -> Result<bool> {
    let row =
        sqlx::query(db.sql("SELECT COUNT(*) AS n FROM pg_extension WHERE extname = 'pg_trgm'"))
            .fetch_one(db.pool())
            .await?;
    Ok(row.big("n")? > 0)
}

/// Append to a statement over `media_item` the works one of `terms` is found
/// in by its letters, as [`matching`] finds them — `id IN (…)` — so that a
/// count, a page in another order and the facets say what the search says.
pub fn push_works(
    dialect: Dialect,
    terms: &[&str],
    sql: &mut String,
    args: &mut AnyArguments,
) -> Result<()> {
    let (from, to) = Kind::Work.ids();
    let mut first = true;
    sql.push('(');
    for term in terms.iter().filter(|t| !t.is_empty()) {
        if !first {
            sql.push_str(" OR ");
        }
        first = false;
        match (dialect, term.chars().count() >= 3) {
            (Dialect::Sqlite, true) => {
                sql.push_str(
                    "id IN (SELECT d.work_id FROM search_fts f CROSS JOIN search_doc d ON d.id = f.rowid
                            WHERE search_fts MATCH ? AND f.rowid >= ? AND f.rowid < ?)",
                );
                args.add(phrase(term)).map_err(|e| anyhow::anyhow!("{e}"))?;
                args.add(from).map_err(|e| anyhow::anyhow!("{e}"))?;
                args.add(to).map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            (Dialect::Sqlite, false) => {
                sql.push_str(
                    "id IN (SELECT work_id FROM search_doc
                            WHERE kind = 'work' AND norm >= ? AND norm < ?)",
                );
                args.add(term.to_string())
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                args.add(after_prefix(term))
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            (Dialect::Postgres, inside) => {
                sql.push_str(
                    r"id IN (SELECT work_id FROM search_doc
                             WHERE kind = 'work' AND norm LIKE ? ESCAPE '\')",
                );
                let pattern = if inside {
                    format!("%{}%", like_escaped(term))
                } else {
                    format!("{}%", like_escaped(term))
                };
                args.add(pattern).map_err(|e| anyhow::anyhow!("{e}"))?;
            }
        }
    }
    // A term of punctuation alone: nothing, rather than everything.
    if first {
        sql.push_str("1 = 0");
    }
    sql.push(')');
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_matches_its_text_as_typed() {
        assert_eq!(like_escaped("100%"), r"100\%");
        assert_eq!(like_escaped("a_c"), r"a\_c");
        assert_eq!(like_escaped(r"back\slash"), r"back\\slash");
        assert_eq!(phrase("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[tokio::test]
    async fn the_trigram_match_is_read_before_the_works_it_names() {
        // What took a search of series fifteen seconds: the works of one
        // kind read first, and the match run again for each. SQLite's own
        // plan, whatever engine the other tests run on.
        let db = crate::db::testing::sqlite().await;
        let scope = Scope {
            kind: Kind::Work,
            media_kind: Some(MediaKind::Series),
            include_disabled: false,
            include_adult: false,
            limit: 200,
            forgive: true,
        };
        let (from, to) = Kind::Work.ids();
        let plan: Vec<String> = sqlx::query(db.sql(&format!(
            "EXPLAIN QUERY PLAN {}",
            inside_read(Dialect::Sqlite, &scope, Taking::BestKnown)
        )))
        .bind(phrase("love"))
        .bind(from)
        .bind(to)
        .bind(200_i64)
        .fetch_all(db.pool())
        .await
        .expect("explained")
        .iter()
        .map(|row| row.text("detail").expect("a plan line"))
        .collect();
        assert!(
            plan.first()
                .is_some_and(|line| line.contains("VIRTUAL TABLE")),
            "{plan:?}"
        );
    }

    #[tokio::test]
    async fn a_misspelling_reads_first_the_names_sharing_most_of_its_trigrams() {
        // SQLite's count; PostgreSQL asks pg_trgm, which the other tests
        // run when they run there.
        let db = crate::db::testing::sqlite().await;
        let (first, _) = Kind::Person.ids();
        for (n, (name, popularity)) in [
            ("ender wiggin", 90.0),
            ("wanda wender", 50.0),
            ("wim wendt", 1.0),
            ("wim wenders", 5.0),
            ("nobody else", 99.0),
        ]
        .into_iter()
        .enumerate()
        {
            sqlx::query(db.sql(
                "INSERT INTO search_doc (id, kind, ref_id, work_id, lang, norm, text, popularity, year)
                 VALUES (?, 'person', ?, NULL, NULL, ?, ?, ?, NULL)",
            ))
            .bind(first + n as i64)
            .bind(format!("name:{name}"))
            .bind(name)
            .bind(name)
            .bind(popularity)
            .execute(db.pool())
            .await
            .expect("indexed");
        }
        let scope = Scope {
            kind: Kind::Person,
            media_kind: None,
            include_disabled: false,
            include_adult: false,
            limit: 10,
            forgive: true,
        };
        // wim, wen, end, ndr, drs: three shared by both Wims, the better known
        // first; two by Wanda Wender; one by Ender Wiggin, however well known.
        let found: Vec<String> = resembling(&db, "wim wendrs", &scope)
            .await
            .expect("read")
            .into_iter()
            .map(|row| row.norm)
            .collect();
        assert_eq!(
            found,
            ["wim wenders", "wim wendt", "wanda wender", "ender wiggin"]
        );
    }

    #[test]
    fn every_name_that_starts_with_a_term_sorts_before_its_bound() {
        let bound = after_prefix("ab");
        for name in ["ab", "abc", "ab z", "ab\u{FFFF}", "abé"] {
            assert!(name >= "ab" && name < bound.as_str(), "{name:?}");
        }
        assert!("ac" > bound.as_str());
        assert!("aa" < "ab");
    }
}
