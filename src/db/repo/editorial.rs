//! The programmer's note: a few lines somebody wrote about what the front
//! page features, a selection, a genre, a network, a year or a decade, in one
//! language — the catalogue's own voice, beside what its sources say.
//!
//! A note is a restricted Markdown: paragraphs, `*emphasis*`, `**strong
//! emphasis**` and `[links](/genre/drama)` to pages of this site. It is
//! checked as it is saved ([`clean`]) and never read as HTML: the interface
//! draws it from the same few rules (`frontend/src/lib/note.ts`), and a
//! preview reads it as plain text ([`plain`]).

use std::str::FromStr;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::db::{Db, RowExt, now};

/// What a note is about.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "camelCase")]
#[schema(as = EditorialScope)]
pub enum Scope {
    /// A work, shown with the note when the front page features it: keyed
    /// by the work's id.
    Work,
    /// A selection, keyed by its id.
    List,
    /// A genre, keyed by its slug: `drama`.
    Genre,
    /// A network or a studio, keyed by its slug: `hbo`.
    Network,
    /// A year: `1999`.
    Year,
    /// A decade, by its first year: `1990`.
    Decade,
}

impl Scope {
    pub const ALL: [Scope; 6] = [
        Self::Work,
        Self::List,
        Self::Genre,
        Self::Network,
        Self::Year,
        Self::Decade,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::List => "list",
            Self::Genre => "genre",
            Self::Network => "network",
            Self::Year => "year",
            Self::Decade => "decade",
        }
    }
}

impl FromStr for Scope {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|scope| scope.as_str() == raw.trim())
            .ok_or_else(|| {
                format!("a note is about a work, a list, a genre, a network, a year or a decade, not {raw:?}")
            })
    }
}

/// One note, in one language.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = EditorialNote)]
pub struct Note {
    pub scope: Scope,
    /// Which one it is about: a work's or a selection's id, a genre's or a
    /// network's slug, a year, the first year of a decade.
    pub key: String,
    /// The language it is written in: `en`, `fr`.
    pub language: String,
    /// What it says, in the restricted Markdown.
    pub body: String,
    /// Who wrote it last, as the journal names people: `editor:alice`.
    #[schema(required)]
    pub updated_by: Option<String>,
    pub updated_at: String,
}

const COLUMNS: &str = "scope, key, language, body, updated_by, updated_at";

fn map(row: &sqlx::any::AnyRow) -> Result<Note> {
    Ok(Note {
        scope: row.text("scope")?.parse().map_err(anyhow::Error::msg)?,
        key: row.text("key")?,
        language: row.text("language")?,
        body: row.text("body")?,
        updated_by: row.opt_text("updated_by")?,
        updated_at: row.text("updated_at")?,
    })
}

/// The note on one thing a reader of `language` is shown: in their language,
/// else in English, else the one written last in any other.
pub async fn read(db: &Db, scope: Scope, key: &str, language: &str) -> Result<Option<Note>> {
    let row = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM editorial WHERE scope = ? AND key = ?
         ORDER BY CASE WHEN language = ? THEN 0 WHEN language = 'en' THEN 1 ELSE 2 END,
                  updated_at DESC
         LIMIT 1"
    )))
    .bind(scope.as_str())
    .bind(key)
    .bind(language)
    .fetch_optional(db.pool())
    .await?;
    row.as_ref().map(map).transpose()
}

/// What the notes are, at most `limit` of them, the one written last first:
/// of one kind of thing, of one of them, in one language, as asked.
pub async fn list(
    db: &Db,
    scope: Option<Scope>,
    key: Option<&str>,
    language: Option<&str>,
    limit: i64,
) -> Result<Vec<Note>> {
    let mut sql = format!("SELECT {COLUMNS} FROM editorial WHERE 1 = 1");
    if scope.is_some() {
        sql.push_str(" AND scope = ?");
    }
    if key.is_some() {
        sql.push_str(" AND key = ?");
    }
    if language.is_some() {
        sql.push_str(" AND language = ?");
    }
    sql.push_str(" ORDER BY updated_at DESC, scope, key, language LIMIT ?");

    let mut query = sqlx::query(db.sql(&sql));
    if let Some(scope) = scope {
        query = query.bind(scope.as_str());
    }
    if let Some(key) = key {
        query = query.bind(key);
    }
    if let Some(language) = language {
        query = query.bind(language);
    }
    let rows = query.bind(limit).fetch_all(db.pool()).await?;
    rows.iter().map(map).collect()
}

/// Write a note, in place of the one in its language if there was one.
pub async fn put(
    db: &Db,
    scope: Scope,
    key: &str,
    language: &str,
    body: &str,
    updated_by: Option<&str>,
) -> Result<Note> {
    let at = now();
    sqlx::query(db.sql(
        "INSERT INTO editorial (scope, key, language, body, updated_by, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (scope, key, language) DO UPDATE SET
             body = excluded.body,
             updated_by = excluded.updated_by,
             updated_at = excluded.updated_at",
    ))
    .bind(scope.as_str())
    .bind(key)
    .bind(language)
    .bind(body)
    .bind(updated_by)
    .bind(&at)
    .execute(db.pool())
    .await?;

    Ok(Note {
        scope,
        key: key.to_string(),
        language: language.to_string(),
        body: body.to_string(),
        updated_by: updated_by.map(str::to_string),
        updated_at: at,
    })
}

/// Take a note away. Whether there was one.
pub async fn remove(db: &Db, scope: Scope, key: &str, language: &str) -> Result<bool> {
    let done =
        sqlx::query(db.sql("DELETE FROM editorial WHERE scope = ? AND key = ? AND language = ?"))
            .bind(scope.as_str())
            .bind(key)
            .bind(language)
            .execute(db.pool())
            .await?;
    Ok(done.rows_affected() > 0)
}

// ─── the text ────────────────────────────────────────────────────────────────

/// The longest a note may be, in characters: a few paragraphs.
pub const MAX_NOTE: usize = 3000;

/// The longest a link's address may be.
const MAX_LINK: usize = 300;

/// What a backslash makes a plain character of.
const ESCAPABLE: &[char] = &['\\', '*', '[', ']', '(', ')'];

/// A piece of a paragraph, as the interface draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Em(Vec<Inline>),
    Strong(Vec<Inline>),
    Link { text: Vec<Inline>, to: String },
}

/// Where the next unescaped `pattern` starts in `chars`, from `from` on.
fn find(chars: &[char], from: usize, pattern: &[char]) -> Option<usize> {
    let mut at = from;
    while at < chars.len() {
        if chars[at] == '\\' && chars.get(at + 1).is_some_and(|c| ESCAPABLE.contains(c)) {
            at += 2;
            continue;
        }
        if chars[at..].starts_with(pattern) {
            return Some(at);
        }
        at += 1;
    }
    None
}

/// Where the `*` that closes an emphasis opened just before `from` is: the
/// next lone one, after something other than a space. A `**` on the way is
/// strong emphasis inside it, passed over whole.
fn closing_star(chars: &[char], from: usize) -> Option<usize> {
    let mut at = from;
    while at < chars.len() {
        match chars[at] {
            '\\' if chars.get(at + 1).is_some_and(|c| ESCAPABLE.contains(c)) => at += 2,
            '*' if chars.get(at + 1) == Some(&'*') => at += 2,
            '*' if at > from && !chars[at - 1].is_whitespace() => return Some(at),
            _ => at += 1,
        }
    }
    None
}

/// Whether an address leads to a page of this site: a path from its root,
/// `/genre/drama` or `/browse?genre=drama`, with nothing a page's address
/// cannot hold.
pub fn is_inside(to: &str) -> bool {
    to.starts_with('/')
        && !to.starts_with("//")
        && to.chars().count() <= MAX_LINK
        && to
            .chars()
            .all(|c| c.is_ascii_graphic() && !matches!(c, '\\' | '<' | '>' | '"' | '`'))
}

/// A link that starts at `at`, a `[`: its text, its address, and where it ends.
fn link_at(chars: &[char], at: usize) -> Option<(Vec<char>, String, usize)> {
    let close = find(chars, at + 1, &[']'])?;
    // Its text is on one line, with no link inside it.
    let text = &chars[at + 1..close];
    if text.is_empty() || text.contains(&'\n') || find(text, 0, &['[']).is_some() {
        return None;
    }
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = find(chars, close + 2, &[')'])?;
    let to: String = chars[close + 2..end].iter().collect();
    if to.is_empty() || to.chars().any(char::is_whitespace) {
        return None;
    }
    Some((text.to_vec(), to, end))
}

/// A paragraph's pieces: text, emphasis, strong emphasis and links, an
/// escaped character as itself, a marker nothing closes as itself too.
pub fn inline(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    pieces(&chars)
}

fn pieces(chars: &[char]) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let flush = |plain: &mut String, out: &mut Vec<Inline>| {
        if !plain.is_empty() {
            out.push(Inline::Text(std::mem::take(plain)));
        }
    };

    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        if c == '\\' && chars.get(at + 1).is_some_and(|n| ESCAPABLE.contains(n)) {
            plain.push(chars[at + 1]);
            at += 2;
            continue;
        }
        if chars[at..].starts_with(&['*', '*']) {
            match find(chars, at + 2, &['*', '*']) {
                Some(close) if close > at + 2 => {
                    flush(&mut plain, &mut out);
                    out.push(Inline::Strong(pieces(&chars[at + 2..close])));
                    at = close + 2;
                }
                _ => {
                    plain.push_str("**");
                    at += 2;
                }
            }
            continue;
        }
        if c == '*' && chars.get(at + 1).is_some_and(|n| !n.is_whitespace()) {
            match closing_star(chars, at + 1) {
                Some(close) => {
                    flush(&mut plain, &mut out);
                    out.push(Inline::Em(pieces(&chars[at + 1..close])));
                    at = close + 1;
                }
                None => {
                    plain.push('*');
                    at += 1;
                }
            }
            continue;
        }
        if c == '['
            && let Some((text, to, end)) = link_at(chars, at)
        {
            flush(&mut plain, &mut out);
            out.push(Inline::Link {
                text: pieces(&text),
                to,
            });
            at = end + 1;
            continue;
        }
        plain.push(c);
        at += 1;
    }
    flush(&mut plain, &mut out);
    out
}

/// A note's paragraphs: its text cut at each empty line, the lines of one
/// joined by a space.
pub fn paragraphs(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                out.push(current.join(" "));
                current.clear();
            }
        } else {
            current.push(line.trim());
        }
    }
    if !current.is_empty() {
        out.push(current.join(" "));
    }
    out
}

/// The addresses a note's links lead to, every one.
fn links(pieces: &[Inline], into: &mut Vec<String>) {
    for piece in pieces {
        match piece {
            Inline::Text(_) => {}
            Inline::Em(inner) | Inline::Strong(inner) => links(inner, into),
            Inline::Link { text, to } => {
                into.push(to.clone());
                links(text, into);
            }
        }
    }
}

/// A note as it is kept: its line ends made one, its control characters
/// and the spaces around its lines taken out, no more than one empty line
/// between two paragraphs. A refusal, said for whoever wrote it, when it is
/// longer than [`MAX_NOTE`], holds HTML, or links to anywhere but a page of
/// this site. Empty, it is no note at all.
pub fn clean(raw: &str) -> Result<String, String> {
    let unified = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = Vec::new();
    for line in unified.split('\n') {
        let line: String = line
            .chars()
            .map(|c| if c == '\t' { ' ' } else { c })
            .filter(|c| !c.is_control())
            .collect();
        // Indentation means nothing here: every line is taken at its words.
        lines.push(line.trim().to_string());
    }
    let mut body = String::new();
    let mut blank = 0;
    for line in lines {
        if line.trim().is_empty() {
            blank += 1;
            continue;
        }
        if !body.is_empty() {
            body.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        blank = 0;
        body.push_str(&line);
    }

    if body.chars().count() > MAX_NOTE {
        return Err(format!("a note is at most {MAX_NOTE} characters"));
    }
    let chars: Vec<char> = body.chars().collect();
    if chars.windows(2).any(|pair| {
        pair[0] == '<' && (pair[1].is_ascii_alphabetic() || matches!(pair[1], '/' | '!' | '?'))
    }) {
        return Err(
            "a note is not read as HTML: write *emphasis*, **strong emphasis** and \
             [a link](/genre/drama) instead"
                .into(),
        );
    }
    let mut to = Vec::new();
    for paragraph in paragraphs(&body) {
        links(&inline(&paragraph), &mut to);
    }
    if let Some(outside) = to.iter().find(|to| !is_inside(to)) {
        return Err(format!(
            "a link leads to a page of this site, written as its path — /genre/drama, \
             /work/… — not to {outside:?}"
        ));
    }
    Ok(body)
}

/// A note as one line of plain text, for a link preview: its markers taken
/// out, a link as its words.
pub fn plain(body: &str) -> String {
    fn flatten(pieces: &[Inline], out: &mut String) {
        for piece in pieces {
            match piece {
                Inline::Text(text) => out.push_str(text),
                Inline::Em(inner) | Inline::Strong(inner) => flatten(inner, out),
                Inline::Link { text, .. } => flatten(text, out),
            }
        }
    }
    paragraphs(body)
        .iter()
        .map(|paragraph| {
            let mut out = String::new();
            flatten(&inline(paragraph), &mut out);
            out
        })
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Inline {
        Inline::Text(s.into())
    }

    #[test]
    fn a_paragraph_is_read_as_text_emphasis_and_links() {
        assert_eq!(
            inline("A *quiet* film, **very** much [a drama](/genre/drama)."),
            [
                text("A "),
                Inline::Em(vec![text("quiet")]),
                text(" film, "),
                Inline::Strong(vec![text("very")]),
                text(" much "),
                Inline::Link {
                    text: vec![text("a drama")],
                    to: "/genre/drama".into()
                },
                text("."),
            ]
        );
        // Emphasis inside strong emphasis, and the other way round.
        assert_eq!(
            inline("**a *b* c**"),
            [Inline::Strong(vec![
                text("a "),
                Inline::Em(vec![text("b")]),
                text(" c")
            ])]
        );
        assert_eq!(
            inline("*a **b** c*"),
            [Inline::Em(vec![
                text("a "),
                Inline::Strong(vec![text("b")]),
                text(" c")
            ])]
        );
    }

    #[test]
    fn a_marker_nothing_closes_is_itself() {
        assert_eq!(inline("5 * 3 = 15"), [text("5 * 3 = 15")]);
        assert_eq!(inline("a *b"), [text("a *b")]);
        assert_eq!(inline("**"), [text("**")]);
        assert_eq!(inline("[no link]"), [text("[no link]")]);
        assert_eq!(inline("[spaced](/a b)"), [text("[spaced](/a b)")]);
        // Escaped, a marker is a character.
        assert_eq!(inline(r"\*not\* \[a\](/b)"), [text("*not* [a](/b)")]);
    }

    #[test]
    fn a_note_is_kept_tidy() {
        assert_eq!(
            clean("  First line\r\nsame paragraph  \r\n\r\n\r\n\tSecond\u{7}  \n\n").unwrap(),
            "First line\nsame paragraph\n\nSecond"
        );
        assert_eq!(clean(" \n\n ").unwrap(), "");
        assert_eq!(
            paragraphs("One\ntwo\n\nThree"),
            ["One two".to_string(), "Three".to_string()]
        );
    }

    #[test]
    fn html_and_links_elsewhere_are_refused() {
        assert!(clean("<b>bold</b>").unwrap_err().contains("HTML"));
        assert!(clean("<!-- hidden -->").is_err());
        assert!(clean("<script>alert(1)</script>").is_err());
        // A less-than sign is not a tag.
        assert_eq!(clean("a < b, <3").unwrap(), "a < b, <3");

        for outside in [
            "[x](https://example.com)",
            "[x](javascript:alert(1))",
            "[x](//example.com/a)",
            "*[x](mailto:a@b.c)*",
        ] {
            assert!(clean(outside).unwrap_err().contains("link"), "{outside}");
        }
        assert!(clean("[a](/genre/drama) and [b](/browse?genre=drama&kind=movie)").is_ok());
        // An address the reader sees as text is text.
        assert!(clean("See https://example.com for more.").is_ok());

        let long = "a".repeat(MAX_NOTE + 1);
        assert!(clean(&long).unwrap_err().contains("at most"));
    }

    #[test]
    fn a_preview_reads_the_note_as_plain_text() {
        assert_eq!(
            plain("A *quiet* film.\n\nSee [the dramas](/genre/drama), **all** of them."),
            "A quiet film. See the dramas, all of them."
        );
    }

    #[test]
    fn a_scope_is_one_of_six() {
        for scope in Scope::ALL {
            assert_eq!(scope.as_str().parse::<Scope>(), Ok(scope));
        }
        assert!("person".parse::<Scope>().is_err());
    }

    #[tokio::test]
    async fn a_note_is_read_in_the_reader_s_language_or_the_nearest_one() {
        let db = crate::db::testing::db().await;
        assert!(
            read(&db, Scope::Genre, "drama", "fr")
                .await
                .unwrap()
                .is_none()
        );

        put(
            &db,
            Scope::Genre,
            "drama",
            "en",
            "In English.",
            Some("editor:ann"),
        )
        .await
        .unwrap();
        // In English for a French reader while there is nothing in French.
        let note = read(&db, Scope::Genre, "drama", "fr")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (note.language.as_str(), note.body.as_str()),
            ("en", "In English.")
        );
        assert_eq!(note.updated_by.as_deref(), Some("editor:ann"));

        put(&db, Scope::Genre, "drama", "fr", "En français.", None)
            .await
            .unwrap();
        let note = read(&db, Scope::Genre, "drama", "fr")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(note.body, "En français.");
        // A German reader is given English, not the last one written.
        let note = read(&db, Scope::Genre, "drama", "de")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(note.language, "en");

        // Written again, in place.
        put(
            &db,
            Scope::Genre,
            "drama",
            "fr",
            "Encore.",
            Some("editor:bob"),
        )
        .await
        .unwrap();
        let mine = list(&db, Some(Scope::Genre), Some("drama"), Some("fr"), 10)
            .await
            .unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].body, "Encore.");
        assert_eq!(
            list(&db, Some(Scope::Genre), Some("drama"), None, 10)
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            list(&db, Some(Scope::Year), None, None, 10)
                .await
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            list(&db, None, None, Some("fr"), 10).await.unwrap()[0].body,
            "Encore."
        );

        // Another thing's notes are its own.
        assert!(
            read(&db, Scope::Genre, "crime", "en")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            read(&db, Scope::Network, "drama", "en")
                .await
                .unwrap()
                .is_none()
        );

        assert!(remove(&db, Scope::Genre, "drama", "en").await.unwrap());
        assert!(!remove(&db, Scope::Genre, "drama", "en").await.unwrap());
        let note = read(&db, Scope::Genre, "drama", "de")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(note.language, "fr");
    }
}
