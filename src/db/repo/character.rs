//! Characters, each kept once for every work it is in, what each is to a
//! work, who plays it there and who voices it in each dub.
//!
//! A character is found again by the id a source files it under —
//! TheTVDB's record of the part, AniList's, MyAnimeList's — and one of no
//! source, by its name among the work's own: a character with no id is its
//! work's alone, for two works' *Narrator* are not one. What a later answer
//! says of it replaces what the row said — its name only where it is the
//! answer of the source whose id found it — and an id it adds is kept; one
//! another row holds already says the two rows are one character, and they
//! are made one. A work's own list is written again whole by a refresh that
//! gives one, as its credits are; a row no work lists any more goes, in the
//! housekeeping's time ([`prune`]).
//!
//! Who plays a character on a work is the person a credit of the work is
//! filed under, not the credit: a refresh writes the credits again, under
//! other ids, and a person keeps theirs. A credit's `character_name` stays
//! what it was: these double it, with ids and pictures.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use sqlx::{Any, Transaction};

use crate::{
    db::{Db, RowExt, new_id, now},
    domain::person::{Character, CharacterCredit, CharacterVoice, character_key},
};

const COLUMNS: &str = "id, name, native_name, image, tvdb_id, anilist_id, mal_id";

/// The columns a source's id is kept in, and the source each is.
const IDS: [(&str, &str); 3] = [
    ("tvdb_id", crate::providers::names::TVDB),
    ("anilist_id", crate::providers::names::ANILIST),
    ("mal_id", crate::providers::names::MAL),
];

fn map(row: &sqlx::any::AnyRow) -> Result<Character> {
    Ok(Character {
        id: row.text("id")?,
        name: row.text("name")?,
        native_name: row.opt_text("native_name")?,
        image: row.opt_text("image")?,
        tvdb_id: row.opt_big("tvdb_id")?,
        anilist_id: row.opt_big("anilist_id")?,
        mal_id: row.opt_big("mal_id")?,
    })
}

fn id_of(character: &Character, column: &str) -> Option<i64> {
    match column {
        "tvdb_id" => character.tvdb_id,
        "anilist_id" => character.anilist_id,
        _ => character.mal_id,
    }
}

/// One character, by this server's id.
pub async fn get(db: &Db, id: &str) -> Result<Option<Character>> {
    let row = sqlx::query(db.sql(&format!("SELECT {COLUMNS} FROM character WHERE id = ?")))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;
    row.as_ref().map(map).transpose()
}

/// One character, by TheTVDB's id for its record of the part: what
/// TheTVDB's `characters/{id}` is asked by.
pub async fn by_tvdb(db: &Db, tvdb_id: i64) -> Result<Option<Character>> {
    let row = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM character WHERE tvdb_id = ?"
    )))
    .bind(tvdb_id)
    .fetch_optional(db.pool())
    .await?;
    row.as_ref().map(map).transpose()
}

/// A work a character is in, and what it is there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub work_id: String,
    pub role: Option<crate::domain::person::CharacterRole>,
    /// Who plays it there: the person here.
    pub person_id: Option<String>,
}

/// The works a character is in, switched on, the most recent first and the
/// undated last, on either engine — those for adults only when asked for.
pub async fn works_of(db: &Db, character_id: &str, include_adult: bool) -> Result<Vec<Appearance>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT l.media_id, l.role, l.person_id
           FROM media_character l JOIN media_item m ON m.id = l.media_id
          WHERE l.character_id = ? AND m.is_enabled = 1 AND m.merged_into IS NULL{}
          ORDER BY m.year DESC NULLS LAST, m.title",
        if include_adult {
            ""
        } else {
            " AND m.is_adult = 0"
        }
    )))
    .bind(character_id)
    .fetch_all(db.pool())
    .await?;
    rows.iter()
        .map(|row| {
            Ok(Appearance {
                work_id: row.text("media_id")?,
                role: row.opt_text("role")?.and_then(|r| r.parse().ok()),
                person_id: row.opt_text("person_id")?,
            })
        })
        .collect()
}

/// This server's ids of the characters TheTVDB has no id for: those
/// TheTVDB's dialect knows by a number made of their id here
/// (`wire::tvdb::character_id`), found again by it.
pub async fn ids_without_tvdb(db: &Db) -> Result<Vec<String>> {
    Ok(
        sqlx::query_scalar(db.sql("SELECT id FROM character WHERE tvdb_id IS NULL"))
            .fetch_all(db.pool())
            .await?,
    )
}

/// The characters of one work in their order, each with its voices and the
/// person who plays it there: whose credit plays it is for the reader, who
/// holds the work's credits, to find (`repo::item::load_children`).
pub async fn of_work(db: &Db, media_id: &str) -> Result<Vec<(CharacterCredit, Option<String>)>> {
    let rows = sqlx::query(db.sql(
        "SELECT c.id, c.name, c.native_name, c.image, c.tvdb_id, c.anilist_id, c.mal_id,
                l.role, l.sort_order, l.person_id, l.source
           FROM media_character l JOIN character c ON c.id = l.character_id
          WHERE l.media_id = ?
          ORDER BY l.sort_order, c.name",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let voiced = sqlx::query(db.sql(
        "SELECT character_id, language, name, native_name, image, anilist_id, mal_id, sort_order
           FROM media_character_voice WHERE media_id = ?
          ORDER BY language, sort_order, name",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;
    let mut voices: HashMap<String, Vec<CharacterVoice>> = HashMap::new();
    for row in &voiced {
        voices
            .entry(row.text("character_id")?)
            .or_default()
            .push(CharacterVoice {
                language: row.text("language")?,
                name: row.text("name")?,
                native_name: row.opt_text("native_name")?,
                image: row.opt_text("image")?,
                anilist_id: row.opt_big("anilist_id")?,
                mal_id: row.opt_big("mal_id")?,
                sort_order: row.int("sort_order")?,
            });
    }

    rows.iter()
        .map(|row| {
            let character = map(row)?;
            Ok((
                CharacterCredit {
                    voices: voices.remove(&character.id).unwrap_or_default(),
                    character,
                    role: row.opt_text("role")?.and_then(|r| r.parse().ok()),
                    sort_order: row.int("sort_order")?,
                    credit_id: None,
                    source: row.opt_text("source")?,
                },
                row.opt_text("person_id")?,
            ))
        })
        .collect()
}

/// Write a work's characters in place of those it had, each found again or
/// added (see the module), with who plays each — the person `played_by`
/// files the credit it names under, by the credit's id — and its voices. In
/// the work's own write, so a reader never finds the list half written.
pub async fn replace(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    media_id: &str,
    characters: &[CharacterCredit],
    played_by: &HashMap<String, String>,
) -> Result<()> {
    // The work's characters of no source, by name, before their links go.
    let rows = sqlx::query(db.sql(
        "SELECT c.id, c.name FROM media_character l JOIN character c ON c.id = l.character_id
          WHERE l.media_id = ? AND c.tvdb_id IS NULL AND c.anilist_id IS NULL
            AND c.mal_id IS NULL",
    ))
    .bind(media_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut own: HashMap<Vec<String>, String> = HashMap::new();
    for row in &rows {
        own.insert(character_key(&row.text("name")?), row.text("id")?);
    }

    for table in ["media_character_voice", "media_character"] {
        sqlx::query(db.sql(&format!("DELETE FROM {table} WHERE media_id = ?")))
            .bind(media_id)
            .execute(&mut **tx)
            .await?;
    }

    // Each found again or added — a row found the same character as another
    // taken into it on the way (see [`complete`]) — before any is written.
    let mut folded: HashMap<String, String> = HashMap::new();
    let mut found: Vec<(String, &CharacterCredit)> = Vec::new();
    for credit in characters {
        if let Some(character_id) = resolve(
            db,
            tx,
            &credit.character,
            credit.source.as_deref(),
            &own,
            &mut folded,
        )
        .await?
        {
            found.push((character_id, credit));
        }
    }

    // Two of the list found as one row are one character: the first, its
    // role, who plays it and its voices completed by the second's.
    let mut links: Vec<Link<'_>> = Vec::new();
    for (mut character_id, credit) in found {
        // A row the list named before it was taken into another is that one.
        for _ in 0..=folded.len() {
            match folded.get(&character_id) {
                Some(into) => character_id.clone_from(into),
                None => break,
            }
        }
        let person = credit
            .credit_id
            .as_ref()
            .and_then(|credit| played_by.get(credit));
        match links
            .iter_mut()
            .find(|link| link.character_id == character_id)
        {
            Some(link) => link.complete(credit, person),
            None => links.push(Link::of(character_id, credit, person)),
        }
    }

    for link in &links {
        sqlx::query(db.sql(
            "INSERT INTO media_character
                 (media_id, character_id, role, sort_order, person_id, source)
             VALUES (?, ?, ?, ?, ?, ?)",
        ))
        .bind(media_id)
        .bind(&link.character_id)
        .bind(link.role.map(|r| r.as_str()))
        .bind(link.sort_order)
        .bind(link.person)
        .bind(link.source)
        .execute(&mut **tx)
        .await
        .context("failed to write a work's character")?;

        for voice in &link.voices {
            if voice.name.trim().is_empty() || voice.language.trim().is_empty() {
                continue;
            }
            sqlx::query(db.sql(
                "INSERT INTO media_character_voice
                     (media_id, character_id, language, name, native_name, image,
                      anilist_id, mal_id, sort_order)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT (media_id, character_id, language, name) DO NOTHING",
            ))
            .bind(media_id)
            .bind(&link.character_id)
            .bind(voice.language.trim())
            .bind(voice.name.trim())
            .bind(&voice.native_name)
            .bind(&voice.image)
            .bind(voice.anilist_id)
            .bind(voice.mal_id)
            .bind(voice.sort_order)
            .execute(&mut **tx)
            .await
            .context("failed to write who voices a character")?;
        }
    }
    Ok(())
}

/// A character of a work as it is written: the row it is, and what the
/// list says of it there.
struct Link<'a> {
    character_id: String,
    role: Option<crate::domain::person::CharacterRole>,
    sort_order: i32,
    person: Option<&'a String>,
    source: Option<&'a str>,
    voices: Vec<&'a CharacterVoice>,
}

impl<'a> Link<'a> {
    fn of(character_id: String, credit: &'a CharacterCredit, person: Option<&'a String>) -> Self {
        Self {
            character_id,
            role: credit.role,
            sort_order: credit.sort_order,
            person,
            source: credit.source.as_deref(),
            voices: credit.voices.iter().collect(),
        }
    }

    /// What another of the list found as the same row says that this one
    /// does not: a role, who plays it, the voices of a dub it has none of —
    /// as the merge completes one source's character with another's.
    fn complete(&mut self, other: &'a CharacterCredit, person: Option<&'a String>) {
        self.role = self.role.or(other.role);
        self.person = self.person.or(person);
        let spoken: Vec<&str> = self.voices.iter().map(|v| v.language.as_str()).collect();
        let theirs: Vec<&'a CharacterVoice> = other
            .voices
            .iter()
            .filter(|v| !spoken.contains(&v.language.as_str()))
            .collect();
        self.voices.extend(theirs);
    }
}

/// The row a character is, found again or added; what the answer says of
/// it written into it. `own` holds the work's characters of no source, by
/// name; `folded`, each row taken into another on the way, to that one.
/// `None` for one with no name.
async fn resolve(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    character: &Character,
    source: Option<&str>,
    own: &HashMap<Vec<String>, String>,
    folded: &mut HashMap<String, String>,
) -> Result<Option<String>> {
    let name = character.name.trim();
    if name.is_empty() {
        return Ok(None);
    }

    // By a source's own id: the name that source gives is the one kept.
    for (column, by) in IDS {
        let Some(id) = id_of(character, column) else {
            continue;
        };
        let found: Option<String> =
            sqlx::query_scalar(db.sql(&format!("SELECT id FROM character WHERE {column} = ?")))
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?;
        if let Some(found) = found {
            let renamed = source == Some(by);
            complete(db, tx, &found, character, renamed, folded).await?;
            return Ok(Some(found));
        }
    }

    // Of no source: the work's own of that name.
    if !character.has_an_id()
        && let Some(found) = own.get(&character_key(name))
    {
        complete(db, tx, found, character, true, folded).await?;
        return Ok(Some(found.clone()));
    }

    // New. Two writes adding the same character at once — PostgreSQL's,
    // which do not wait for each other as SQLite's do — meet on an id: the
    // second takes the first's row.
    let id = new_id();
    let at = now();
    let added = sqlx::query(db.sql(
        "INSERT INTO character
             (id, name, native_name, image, tvdb_id, anilist_id, mal_id, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT DO NOTHING",
    ))
    .bind(&id)
    .bind(name)
    .bind(&character.native_name)
    .bind(&character.image)
    .bind(character.tvdb_id)
    .bind(character.anilist_id)
    .bind(character.mal_id)
    .bind(&at)
    .bind(&at)
    .execute(&mut **tx)
    .await
    .context("failed to add a character")?;
    if added.rows_affected() > 0 {
        return Ok(Some(id));
    }
    for (column, _) in IDS {
        let Some(given) = id_of(character, column) else {
            continue;
        };
        let found: Option<String> =
            sqlx::query_scalar(db.sql(&format!("SELECT id FROM character WHERE {column} = ?")))
                .bind(given)
                .fetch_optional(&mut **tx)
                .await?;
        if found.is_some() {
            return Ok(found);
        }
    }
    Ok(None)
}

/// Write into a character's row what an answer says of it: an id of a
/// source it has none of, its name in its own script and its picture, where
/// the answer gives them; and its name, when `renamed`.
///
/// An id another row holds already says the two are one character, kept
/// twice: that row is taken into this one ([`fold`]) — its works, its
/// voices, its ids — and noted in `folded`. Unless the two disagree, each
/// holding an id of one source the other does not, or the answer gives one
/// the other row does not hold: two records of TheTVDB's — a series' part
/// and a film's — of one of AniList's characters are two rows, as only one
/// of them can be filed under TheTVDB's id; the id stays the other row's.
async fn complete(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    id: &str,
    character: &Character,
    renamed: bool,
    folded: &mut HashMap<String, String>,
) -> Result<()> {
    let row = sqlx::query(db.sql(&format!("SELECT {COLUMNS} FROM character WHERE id = ?")))
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    let held = map(&row)?;

    let mut ids = [held.tvdb_id, held.anilist_id, held.mal_id];
    let given = [character.tvdb_id, character.anilist_id, character.mal_id];
    let mut taken_in: Vec<Character> = Vec::new();
    for (at, (column, _)) in IDS.iter().enumerate() {
        let (Some(given_here), None) = (given[at], ids[at]) else {
            continue;
        };
        let taken = sqlx::query(db.sql(&format!(
            "SELECT {COLUMNS} FROM character WHERE {column} = ?"
        )))
        .bind(given_here)
        .fetch_optional(&mut **tx)
        .await?;
        let Some(other) = taken.as_ref().map(map).transpose()? else {
            ids[at] = Some(given_here);
            continue;
        };
        let theirs = [other.tvdb_id, other.anilist_id, other.mal_id];
        let agree = (0..IDS.len()).all(|k| match theirs[k] {
            None => true,
            Some(their) => {
                ids[k].is_none_or(|ours| ours == their) && given[k].is_none_or(|said| said == their)
            }
        });
        if other.id == id || !agree {
            continue;
        }
        fold(db, tx, &other.id, id).await?;
        for k in 0..IDS.len() {
            ids[k] = ids[k].or(theirs[k]);
        }
        folded.insert(other.id.clone(), id.to_string());
        taken_in.push(other);
    }

    let name = match renamed {
        true => character.name.trim().to_string(),
        false => held.name.clone(),
    };
    let native_name = character
        .native_name
        .clone()
        .or_else(|| held.native_name.clone())
        .or_else(|| taken_in.iter().find_map(|other| other.native_name.clone()));
    let image = character
        .image
        .clone()
        .or_else(|| held.image.clone())
        .or_else(|| taken_in.iter().find_map(|other| other.image.clone()));
    if (&name, &native_name, &image, ids)
        == (
            &held.name,
            &held.native_name,
            &held.image,
            [held.tvdb_id, held.anilist_id, held.mal_id],
        )
    {
        return Ok(());
    }

    sqlx::query(db.sql(
        "UPDATE character SET name = ?, native_name = ?, image = ?, tvdb_id = ?, anilist_id = ?,
                mal_id = ?, updated_at = ?
          WHERE id = ?",
    ))
    .bind(&name)
    .bind(&native_name)
    .bind(&image)
    .bind(ids[0])
    .bind(ids[1])
    .bind(ids[2])
    .bind(now())
    .bind(id)
    .execute(&mut **tx)
    .await
    .context("failed to complete a character")?;
    Ok(())
}

/// Take the row `from` into `into`, the same character kept twice. On each
/// work listing `from`, `into` is listed in its place; on one listing both,
/// `into`'s role and who plays it there stand, completed by `from`'s, and
/// `from`'s voices join `into`'s in a dub these have none of. Then `from`
/// goes — or, while the record of a merge of two works names it, stays with
/// no id and listed nowhere, for that merge's undo to put back what it
/// lists; [`prune`] takes it once nothing does. Its ids are `into`'s to
/// take.
async fn fold(db: &Db, tx: &mut Transaction<'_, Any>, from: &str, into: &str) -> Result<()> {
    let spoken: HashSet<(String, String)> = sqlx::query(
        db.sql("SELECT media_id, language FROM media_character_voice WHERE character_id = ?"),
    )
    .bind(into)
    .fetch_all(&mut **tx)
    .await?
    .iter()
    .map(|row| Ok((row.text("media_id")?, row.text("language")?)))
    .collect::<Result<_>>()?;
    let voices = sqlx::query(
        db.sql("SELECT media_id, language, name FROM media_character_voice WHERE character_id = ?"),
    )
    .bind(from)
    .fetch_all(&mut **tx)
    .await?;
    for row in &voices {
        let (media, language, name) = (
            row.text("media_id")?,
            row.text("language")?,
            row.text("name")?,
        );
        if spoken.contains(&(media.clone(), language.clone())) {
            continue;
        }
        sqlx::query(db.sql(
            "UPDATE media_character_voice SET character_id = ?
              WHERE media_id = ? AND character_id = ? AND language = ? AND name = ?",
        ))
        .bind(into)
        .bind(&media)
        .bind(from)
        .bind(&language)
        .bind(&name)
        .execute(&mut **tx)
        .await
        .context("failed to give a character's voices to the one it is")?;
    }

    sqlx::query(db.sql(
        "UPDATE media_character
            SET role = COALESCE(role, (SELECT f.role FROM media_character f
                                        WHERE f.media_id = media_character.media_id
                                          AND f.character_id = ?)),
                person_id = COALESCE(person_id, (SELECT f.person_id FROM media_character f
                                                  WHERE f.media_id = media_character.media_id
                                                    AND f.character_id = ?))
          WHERE character_id = ?
            AND media_id IN (SELECT media_id FROM media_character WHERE character_id = ?)",
    ))
    .bind(from)
    .bind(from)
    .bind(into)
    .bind(from)
    .execute(&mut **tx)
    .await
    .context("failed to complete a work's character with its double")?;
    sqlx::query(db.sql(
        "UPDATE media_character SET character_id = ?
          WHERE character_id = ?
            AND media_id NOT IN (SELECT media_id FROM media_character WHERE character_id = ?)",
    ))
    .bind(into)
    .bind(from)
    .bind(into)
    .execute(&mut **tx)
    .await
    .context("failed to list a character in place of its double")?;

    // What is left of it: on the works listing both.
    for table in ["media_character_voice", "media_character"] {
        sqlx::query(db.sql(&format!("DELETE FROM {table} WHERE character_id = ?")))
            .bind(from)
            .execute(&mut **tx)
            .await?;
    }
    let named = sqlx::query(db.sql(
        "SELECT 1 AS hit FROM media_item
          WHERE merged_into IS NOT NULL AND merge_record LIKE ?
          LIMIT 1",
    ))
    .bind(format!("%{from}%"))
    .fetch_optional(&mut **tx)
    .await?
    .is_some();
    let gone = match named {
        true => sqlx::query(db.sql(
            "UPDATE character SET tvdb_id = NULL, anilist_id = NULL, mal_id = NULL, updated_at = ?
              WHERE id = ?",
        ))
        .bind(now())
        .bind(from),
        false => sqlx::query(db.sql("DELETE FROM character WHERE id = ?")).bind(from),
    };
    gone.execute(&mut **tx)
        .await
        .context("failed to take a character into the one it is")?;
    Ok(())
}

/// Forget the characters no work lists any more — what a refresh leaves
/// behind when a source drops one, what a [`fold`] leaves — and so their
/// pictures, at the media store's next sweep, which nothing names then. One
/// the record of a merge of two works made since `undo_since` names stays,
/// for that merge's undo to put back; `None` where no merge is undone. How
/// many went.
pub async fn prune(db: &Db, undo_since: Option<&str>) -> Result<u64> {
    let mut nobodys = String::from(
        "SELECT h.id FROM character h
          WHERE NOT EXISTS (SELECT 1 FROM media_character l WHERE l.character_id = h.id)
            AND NOT EXISTS (SELECT 1 FROM media_character_voice v WHERE v.character_id = h.id)",
    );
    if undo_since.is_some() {
        nobodys.push_str(
            "
            AND NOT EXISTS (SELECT 1 FROM media_item t
                             WHERE t.merged_into IS NOT NULL AND t.merged_at >= ?
                               AND t.merge_record LIKE '%' || h.id || '%')",
        );
    }
    // Asked first: most passes find none, and a delete would wait on the
    // writers, and hold them, for nothing.
    let mut any = sqlx::query(db.sql(&format!("{nobodys} LIMIT 1")));
    if let Some(since) = undo_since {
        any = any.bind(since);
    }
    if any.fetch_optional(db.pool()).await?.is_none() {
        return Ok(0);
    }
    let mut delete = sqlx::query(db.sql(&format!("DELETE FROM character WHERE id IN ({nobodys})")));
    if let Some(since) = undo_since {
        delete = delete.bind(since);
    }
    Ok(delete
        .execute(db.pool())
        .await
        .context("failed to forget the characters no work lists")?
        .rows_affected())
}

/// Who a person merged into another plays now, on every work: the other.
/// In the merge's own transaction.
pub async fn played_by_merged(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    from: &str,
    into: &str,
) -> Result<()> {
    sqlx::query(db.sql("UPDATE media_character SET person_id = ? WHERE person_id = ?"))
        .bind(into)
        .bind(from)
        .execute(&mut **tx)
        .await
        .context("failed to give a merged person's parts to the other")?;
    Ok(())
}

/// Who plays the characters on these works whose credits were filed under
/// another person: the one `moved` says each work's credit of `from` is
/// filed under now. In the transaction that moved them.
pub async fn played_by_moved(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    moved: &[(String, String, String)],
) -> Result<()> {
    for (media_id, from, to) in moved {
        sqlx::query(
            db.sql("UPDATE media_character SET person_id = ? WHERE media_id = ? AND person_id = ?"),
        )
        .bind(to)
        .bind(media_id)
        .bind(from)
        .execute(&mut **tx)
        .await
        .context("failed to give a part to the person its credit is filed under")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::repo::item::{ItemWrite, get, load_children, upsert},
        domain::{
            Credit, CreditType, MediaItem, MediaKind,
            person::{CharacterRole, CharacterVoice},
        },
    };

    fn credit(name: &str, tmdb: i64) -> Credit {
        Credit {
            id: new_id(),
            credit_type: CreditType::Actor,
            person_name: name.into(),
            character_name: None,
            image: None,
            tmdb_person_id: Some(tmdb),
            credit_tmdb_id: None,
            sort_order: 0,
            is_manual: false,
            department: None,
            job: None,
        }
    }

    fn part(name: &str, tvdb: Option<i64>, anilist: Option<i64>, order: i32) -> CharacterCredit {
        CharacterCredit {
            character: Character {
                id: String::new(),
                name: name.into(),
                native_name: None,
                image: None,
                tvdb_id: tvdb,
                anilist_id: anilist,
                mal_id: None,
            },
            role: Some(CharacterRole::Supporting),
            sort_order: order,
            credit_id: None,
            voices: Vec::new(),
            source: Some("tvdb".into()),
        }
    }

    /// The work as a refresh gives it: its cast under fresh ids, as every
    /// mapping makes them, and its characters played by them.
    fn frieren(title: &str, with_characters: bool) -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = title.into();
        item.slug = crate::domain::make_slug(title, None);
        item.credits = vec![
            credit("Atsumi Tanezaki", 9_300_001),
            credit("Kana Ichinose", 9_300_002),
        ];
        if with_characters {
            let mut lead = part("Frieren", Some(9_310_001), Some(9_320_001), 0);
            lead.role = Some(CharacterRole::Main);
            lead.character.native_name = Some("フリーレン".into());
            lead.credit_id = Some(item.credits[0].id.clone());
            lead.voices = vec![
                CharacterVoice {
                    language: "jpn".into(),
                    name: "Atsumi Tanezaki".into(),
                    native_name: Some("種﨑敦美".into()),
                    image: None,
                    anilist_id: Some(112_215),
                    mal_id: None,
                    sort_order: 0,
                },
                CharacterVoice {
                    language: "eng".into(),
                    name: "Mallorie Rodak".into(),
                    native_name: None,
                    image: None,
                    anilist_id: Some(119_697),
                    mal_id: None,
                    sort_order: 0,
                },
            ];
            let mut fern = part("Fern", Some(9_310_002), None, 1);
            fern.credit_id = Some(item.credits[1].id.clone());
            let narrator = part("Narrator", None, None, 2);
            item.characters = vec![lead, fern, narrator];
        }
        item
    }

    async fn write(db: &Db, item: &MediaItem) {
        upsert(
            db,
            ItemWrite {
                item,
                replace_children: true,
            },
        )
        .await
        .expect("written");
    }

    async fn read(db: &Db, id: &str) -> MediaItem {
        let mut read = get(db, id).await.unwrap().expect("the work");
        load_children(db, &mut read).await.unwrap();
        read
    }

    /// Who plays each character, by name, as the work reads back.
    fn played(work: &MediaItem) -> Vec<(&str, Option<&str>)> {
        work.characters
            .iter()
            .map(|c| {
                let by = c
                    .credit_id
                    .as_ref()
                    .and_then(|id| work.credits.iter().find(|credit| credit.id == *id))
                    .map(|credit| credit.person_name.as_str());
                (c.character.name.as_str(), by)
            })
            .collect()
    }

    #[tokio::test]
    async fn characters_are_kept_played_by_the_cast_however_often_it_is_written() {
        let db = crate::db::testing::db().await;
        let first = frieren("Characters Kept", true);
        write(&db, &first).await;

        let read_back = read(&db, &first.id).await;
        assert_eq!(
            played(&read_back),
            [
                ("Frieren", Some("Atsumi Tanezaki")),
                ("Fern", Some("Kana Ichinose")),
                ("Narrator", None)
            ]
        );
        let lead = &read_back.characters[0];
        assert!(!lead.character.id.is_empty());
        assert_eq!(
            (lead.character.tvdb_id, lead.character.anilist_id),
            (Some(9_310_001), Some(9_320_001))
        );
        assert_eq!(lead.character.native_name.as_deref(), Some("フリーレン"));
        assert_eq!(lead.role, Some(CharacterRole::Main));
        assert_eq!(lead.source.as_deref(), Some("tvdb"));
        assert_eq!(
            lead.voices
                .iter()
                .map(|v| (v.language.as_str(), v.name.as_str()))
                .collect::<Vec<_>>(),
            [("eng", "Mallorie Rodak"), ("jpn", "Atsumi Tanezaki")]
        );
        let ids: Vec<String> = read_back
            .characters
            .iter()
            .map(|c| c.character.id.clone())
            .collect();

        // Refreshed without characters — nobody said — under fresh credits:
        // those held stand, played by the same people.
        let mut again = frieren("Characters Kept", false);
        again.id.clone_from(&first.id);
        write(&db, &again).await;
        let read_back = read(&db, &first.id).await;
        assert_eq!(
            played(&read_back),
            [
                ("Frieren", Some("Atsumi Tanezaki")),
                ("Fern", Some("Kana Ichinose")),
                ("Narrator", None)
            ]
        );

        // Refreshed with them: the same rows, the one of no source found by
        // its name on its work.
        let mut again = frieren("Characters Kept", true);
        again.id.clone_from(&first.id);
        write(&db, &again).await;
        let read_back = read(&db, &first.id).await;
        assert_eq!(
            read_back
                .characters
                .iter()
                .map(|c| c.character.id.clone())
                .collect::<Vec<_>>(),
            ids
        );

        // Another work naming the lead by AniList's id alone shares its row;
        // its own narrator is another.
        let mut film = MediaItem::empty(MediaKind::Movie);
        film.title = "Characters Kept, The Film".into();
        film.slug = "characters-kept-the-film".into();
        film.credits = vec![credit("Somebody Else", 9_300_003)];
        let mut lead = part("Frieren", None, Some(9_320_001), 0);
        lead.source = Some("anilist".into());
        lead.character.image = Some("https://s4.anilist.co/frieren.png".into());
        film.characters = vec![lead, part("Narrator", None, None, 1)];
        write(&db, &film).await;
        let in_film = read(&db, &film.id).await;
        assert_eq!(in_film.characters[0].character.id, ids[0]);
        assert_eq!(
            in_film.characters[0].character.tvdb_id,
            Some(9_310_001),
            "the row's ids"
        );
        assert_eq!(
            in_film.characters[0].character.image.as_deref(),
            Some("https://s4.anilist.co/frieren.png")
        );
        assert_ne!(in_film.characters[1].character.id, ids[2]);
        let shared = works_of(&db, &ids[0], true).await.unwrap();
        assert_eq!(shared.len(), 2);
        assert_eq!(
            by_tvdb(&db, 9_310_002).await.unwrap().map(|c| c.name),
            Some("Fern".to_string())
        );

        // Somebody merged into another plays their parts as the other.
        let person_of = |work: &MediaItem, name: &str| {
            let credit = work.credits.iter().find(|c| c.person_name == name).unwrap();
            work.people
                .of(credit)
                .and_then(|p| p.person_id.clone())
                .unwrap()
        };
        let kana = person_of(&read_back, "Kana Ichinose");
        let other = person_of(&in_film, "Somebody Else");
        crate::db::repo::person::merge(&db, &kana, &other)
            .await
            .unwrap()
            .expect("merged");
        let read_back = read(&db, &first.id).await;
        assert_eq!(played(&read_back)[1], ("Fern", Some("Kana Ichinose")));
    }
    /// Two works merged: a character both list is the survivor's, voices
    /// and all; the merged work's others move over beside them; an undo puts
    /// them back.
    #[tokio::test]
    async fn a_merge_folds_the_characters_and_an_undo_puts_them_back() {
        use crate::db::repo::item::{MergeInput, merge_works, unmerge_work};

        let db = crate::db::testing::db().await;
        let survivor = frieren("Characters Merged", true);
        write(&db, &survivor).await;
        let mut merged = MediaItem::empty(MediaKind::Series);
        merged.title = "Characters Merged, Again".into();
        merged.slug = "characters-merged-again".into();
        let mut lead = part("Frieren", None, Some(9_320_001), 0);
        lead.source = Some("anilist".into());
        lead.voices = vec![CharacterVoice {
            language: "fra".into(),
            name: "Marie Nonnenmacher".into(),
            native_name: None,
            image: None,
            anilist_id: Some(116_893),
            mal_id: None,
            sort_order: 0,
        }];
        let mut himmel = part("Himmel", None, Some(9_320_002), 1);
        himmel.source = Some("anilist".into());
        merged.characters = vec![lead, himmel];
        write(&db, &merged).await;

        let keep = std::collections::BTreeMap::new();
        let taken = std::collections::BTreeMap::new();
        merge_works(
            &db,
            &MergeInput {
                merged: &merged.id,
                into: &survivor.id,
                keep: &keep,
                taken: &taken,
                titles: &[],
                by: None,
            },
        )
        .await
        .expect("merged");
        let read_back = read(&db, &survivor.id).await;
        let names: Vec<&str> = read_back
            .characters
            .iter()
            .map(|c| c.character.name.as_str())
            .collect();
        assert_eq!(names, ["Frieren", "Fern", "Himmel", "Narrator"]);
        assert_eq!(
            read_back.characters[0]
                .voices
                .iter()
                .map(|v| v.language.as_str())
                .collect::<Vec<_>>(),
            ["eng", "fra", "jpn"],
            "a dub the survivor's lists lacked moved over"
        );

        unmerge_work(&db, &merged.id, None).await.expect("undone");
        let back = read(&db, &merged.id).await;
        assert_eq!(
            back.characters
                .iter()
                .map(|c| (c.character.name.as_str(), c.voices.len()))
                .collect::<Vec<_>>(),
            [("Frieren", 1), ("Himmel", 0)]
        );
        assert_eq!(read(&db, &survivor.id).await.characters.len(), 3);
    }

    /// A work of these characters, written.
    async fn work_of(db: &Db, title: &str, characters: Vec<CharacterCredit>) -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = title.into();
        item.slug = crate::domain::make_slug(title, None);
        item.characters = characters;
        write(db, &item).await;
        item
    }

    fn voice(language: &str, name: &str, id: i64) -> CharacterVoice {
        CharacterVoice {
            language: language.into(),
            name: name.into(),
            native_name: None,
            image: None,
            anilist_id: Some(id),
            mal_id: None,
            sort_order: 0,
        }
    }

    /// The characters of a work as it reads back: each one's row, its role
    /// and its voices' languages.
    async fn listed(db: &Db, id: &str) -> Vec<(String, Option<CharacterRole>, Vec<String>)> {
        read(db, id)
            .await
            .characters
            .into_iter()
            .map(|c| {
                (
                    c.character.id,
                    c.role,
                    c.voices.into_iter().map(|v| v.language).collect(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn one_character_kept_twice_is_made_one() {
        let db = crate::db::testing::db().await;

        // Two of a list found as one row — the merge did not pair them, the
        // row did: one character, the second's voices and role completing
        // the first.
        let held = work_of(
            &db,
            "Kept Once",
            vec![part("Kaji", Some(9_350_001), Some(9_350_101), 0)],
        )
        .await;
        let row = read(&db, &held.id).await.characters[0].character.id.clone();
        let mut theirs = part("Eren", None, Some(9_350_101), 1);
        theirs.source = Some("anilist".into());
        theirs.role = Some(CharacterRole::Main);
        theirs.voices = vec![voice("jpn", "Yuuki Kaji", 95_101)];
        let mut ours = part("Eren Jaeger", Some(9_350_001), None, 0);
        ours.role = None;
        let twice = work_of(&db, "Listed Twice", vec![ours, theirs]).await;
        assert_eq!(
            listed(&db, &twice.id).await,
            [(
                row.clone(),
                Some(CharacterRole::Main),
                vec!["jpn".to_string()]
            )],
            "one character, its voices and its role not lost"
        );

        // Two rows a later answer says are one: the second taken into the
        // first, where it was listed and with its voices; it goes.
        let armin = work_of(
            &db,
            "TheTVDB's Armin",
            vec![part("Armin", Some(9_360_001), None, 0)],
        )
        .await;
        let mut anilists = part("Armin Arlert", None, Some(9_360_101), 0);
        anilists.source = Some("anilist".into());
        anilists.role = Some(CharacterRole::Main);
        anilists.character.image = Some("https://s4.anilist.co/armin.png".into());
        anilists.voices = vec![
            voice("jpn", "Marina Inoue", 95_201),
            voice("eng", "Josh Grelle", 95_202),
        ];
        let drawn = work_of(&db, "AniList's Armin", vec![anilists]).await;
        let first = read(&db, &armin.id).await.characters[0]
            .character
            .id
            .clone();
        let second = read(&db, &drawn.id).await.characters[0]
            .character
            .id
            .clone();
        assert_ne!(first, second);

        work_of(
            &db,
            "Both Say Armin",
            vec![part("Armin", Some(9_360_001), Some(9_360_101), 0)],
        )
        .await;
        assert!(
            super::get(&db, &second).await.unwrap().is_none(),
            "taken in"
        );
        let one = by_tvdb(&db, 9_360_001).await.unwrap().expect("the one");
        assert_eq!(
            (one.id.as_str(), one.anilist_id),
            (first.as_str(), Some(9_360_101))
        );
        assert_eq!(
            one.image.as_deref(),
            Some("https://s4.anilist.co/armin.png")
        );
        assert_eq!(
            listed(&db, &drawn.id).await,
            [(
                first.clone(),
                Some(CharacterRole::Main),
                vec!["eng".to_string(), "jpn".to_string()]
            )],
            "listed where the other was, voices and all"
        );
        assert_eq!(works_of(&db, &first, true).await.unwrap().len(), 3);

        // Two that disagree — each filed under a record of TheTVDB's of its
        // own — stay two: the id stays the other's.
        let series = work_of(
            &db,
            "A Series' Mikasa",
            vec![part("Mikasa", Some(9_370_001), None, 0)],
        )
        .await;
        let film = work_of(
            &db,
            "A Film's Mikasa",
            vec![part("Mikasa", Some(9_370_009), Some(9_370_101), 0)],
        )
        .await;
        work_of(
            &db,
            "Both Say Mikasa",
            vec![part("Mikasa", Some(9_370_001), Some(9_370_101), 0)],
        )
        .await;
        let of_series = read(&db, &series.id).await.characters[0].character.clone();
        let of_film = read(&db, &film.id).await.characters[0].character.clone();
        assert_ne!(of_series.id, of_film.id);
        assert_eq!(of_series.anilist_id, None);
        assert_eq!(
            (of_film.tvdb_id, of_film.anilist_id),
            (Some(9_370_009), Some(9_370_101))
        );

        // One the undo of a merge of two works would put back stays, with no
        // id and listed nowhere, until nothing names it.
        let named = work_of(
            &db,
            "Named By A Merge",
            vec![part("Hange", None, Some(9_380_101), 0)],
        )
        .await;
        let hange = read(&db, &named.id).await.characters[0]
            .character
            .id
            .clone();
        let tombstone = work_of(&db, "Merged Away", Vec::new()).await;
        sqlx::query(db.sql(
            "UPDATE media_item SET merged_into = ?, merged_at = ?, merge_record = ? WHERE id = ?",
        ))
        .bind(&named.id)
        .bind(now())
        .bind(format!(
            r#"{{"dropped":{{"media_character":[{{"character_id":"{hange}"}}]}}}}"#
        ))
        .bind(&tombstone.id)
        .execute(db.pool())
        .await
        .unwrap();
        work_of(
            &db,
            "TheTVDB's Hange",
            vec![part("Hange", Some(9_380_001), None, 0)],
        )
        .await;
        work_of(
            &db,
            "Both Say Hange",
            vec![part("Hange", Some(9_380_001), Some(9_380_101), 0)],
        )
        .await;
        let husk = super::get(&db, &hange)
            .await
            .unwrap()
            .expect("kept for the undo");
        assert_eq!((husk.tvdb_id, husk.anilist_id), (None, None));
        assert!(works_of(&db, &hange, true).await.unwrap().is_empty());
        assert_eq!(
            by_tvdb(&db, 9_380_001)
                .await
                .unwrap()
                .and_then(|c| c.anilist_id),
            Some(9_380_101)
        );
    }

    #[tokio::test]
    async fn the_characters_no_work_lists_are_forgotten_but_one_a_merge_can_put_back() {
        let db = crate::db::testing::db().await;
        let mut work = work_of(
            &db,
            "Characters Dropped",
            vec![
                part("Stays", Some(9_390_001), None, 0),
                part("Dropped", Some(9_390_002), None, 1),
                part("Put Back", Some(9_390_003), None, 2),
            ],
        )
        .await;
        let rows: Vec<String> = read(&db, &work.id)
            .await
            .characters
            .into_iter()
            .map(|c| c.character.id)
            .collect();
        work.characters.truncate(1);
        write(&db, &work).await;

        // A merge made now names the third, as its undo would put it back.
        let tombstone = work_of(&db, "Merged Into It", Vec::new()).await;
        sqlx::query(db.sql(
            "UPDATE media_item SET merged_into = ?, merged_at = ?, merge_record = ? WHERE id = ?",
        ))
        .bind(&work.id)
        .bind(now())
        .bind(format!(
            r#"{{"dropped":{{"media_character":[{{"character_id":"{}"}}]}}}}"#,
            rows[2]
        ))
        .bind(&tombstone.id)
        .execute(db.pool())
        .await
        .unwrap();

        let an_hour_ago = crate::db::to_rfc3339(chrono::Utc::now() - chrono::TimeDelta::hours(1));
        assert_eq!(prune(&db, Some(&an_hour_ago)).await.unwrap(), 1);
        assert!(super::get(&db, &rows[0]).await.unwrap().is_some(), "listed");
        assert!(
            super::get(&db, &rows[1]).await.unwrap().is_none(),
            "forgotten"
        );
        assert!(
            super::get(&db, &rows[2]).await.unwrap().is_some(),
            "the undo's"
        );
        assert_eq!(prune(&db, Some(&an_hour_ago)).await.unwrap(), 0);

        // Past the merge's window, or with no undo kept, it goes too.
        let later = crate::db::to_rfc3339(chrono::Utc::now() + chrono::TimeDelta::hours(1));
        assert_eq!(prune(&db, Some(&later)).await.unwrap(), 1);
        assert!(super::get(&db, &rows[2]).await.unwrap().is_none());
        assert_eq!(prune(&db, None).await.unwrap(), 0);
        assert_eq!(read(&db, &work.id).await.characters.len(), 1);
    }
}
