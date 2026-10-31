//! The search index's table: the rows of a work written again from what the
//! database holds of it, the rows of the people its credits name, and the
//! works marked as behind.
//!
//! A work's rows are worked out whole and compared with those it has: a row
//! whose name went is deleted, one whose name came is inserted, and one that
//! stayed is left alone but for its popularity and its year. Their numbers
//! say what they are (see `search::index::entry_id`), so a refresh that
//! changed nothing a work is found by writes nothing but its popularity.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{Context, Result};
use sqlx::{Any, Arguments, Transaction, any::AnyArguments};

use crate::{
    db::{Db, Dialect, Double, RowExt},
    domain::fields::Child,
    search::{
        Kind,
        index::{self, Entry, EpisodeName, PersonNames, WorkNames},
    },
};

/// How many works, or people, one statement reads: few enough for every
/// engine's ceiling on bound values.
const CHUNK: usize = 200;

/// How many rows one insert writes, at nine values each.
const ROWS_PER_INSERT: usize = 100;

/// How many works one transaction writes the rows of, while catching up: a
/// few milliseconds of the write lock at a time, so that a refresh is never
/// kept waiting for long behind the index.
const WORKS_PER_WRITE: usize = 25;

/// Bind one more value to a statement being built.
fn bind<'t>(
    args: &mut AnyArguments,
    value: impl sqlx::Encode<'t, Any> + sqlx::Type<Any>,
) -> Result<()> {
    args.add(value).map_err(|e| anyhow::anyhow!("{e}"))
}

/// As many placeholders as there are values, for an `IN (…)`.
fn holes(n: usize) -> String {
    vec!["?"; n].join(", ")
}

/// Write the rows of these works again from what the database holds of them
/// now, in `tx`, and mark them current. Returns the people their credits
/// name, by their records' ids, whose own rows the caller writes once `tx`
/// is committed: a person is credited on many works, and two works'
/// transactions holding the same person's row is how two refreshes deadlock.
///
/// A credit added by hand names nobody until it is filed under its person,
/// which is done here first: adding it marked its work, and every marked
/// work passes this way within seconds.
///
/// On PostgreSQL the works are held for update while they are read, so that
/// a change to one made meanwhile is either read or marks it again after.
pub async fn index_works(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    ids: &[String],
) -> Result<BTreeSet<String>> {
    let mut people = BTreeSet::new();
    for chunk in ids.chunks(CHUNK) {
        let mut ids: Vec<String> = chunk.to_vec();
        ids.sort();
        ids.dedup();
        people.extend(crate::db::repo::person::link_unlinked(db, tx, &ids).await?);
        let works = read_names(db, tx, &ids, &mut people).await?;
        write_entries(db, tx, &ids, &works).await?;

        let sql = format!(
            "UPDATE media_item SET search_stale = 0 WHERE id IN ({})",
            holes(ids.len())
        );
        let mut args = AnyArguments::default();
        for id in &ids {
            bind(&mut args, id.clone())?;
        }
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut **tx)
            .await
            .context("failed to mark works as indexed")?;
    }
    Ok(people)
}

/// The people the credits of these works are filed under, as they stand in
/// `tx`: what a write reads before it replaces the credits, so that somebody
/// it takes off a work is written again too.
pub async fn people_of(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    ids: &[String],
) -> Result<BTreeSet<String>> {
    let mut people = BTreeSet::new();
    for chunk in ids.chunks(CHUNK) {
        let sql = format!(
            "SELECT DISTINCT person_id FROM media_credit
              WHERE media_id IN ({}) AND person_id IS NOT NULL",
            holes(chunk.len())
        );
        let mut args = AnyArguments::default();
        for id in chunk {
            bind(&mut args, id.clone())?;
        }
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(&mut **tx)
            .await?
        {
            people.insert(row.text("person_id")?);
        }
    }
    Ok(people)
}

/// What these works are found by, read in a few statements for all of them.
async fn read_names(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    ids: &[String],
    people: &mut BTreeSet<String>,
) -> Result<Vec<WorkNames>> {
    let marks = holes(ids.len());
    let args = || -> Result<AnyArguments> {
        let mut args = AnyArguments::default();
        for id in ids {
            bind(&mut args, id.clone())?;
        }
        Ok(args)
    };

    // In id order, the order every writer of several works takes them in.
    let lock = match db.dialect() {
        Dialect::Postgres => " ORDER BY id FOR UPDATE",
        Dialect::Sqlite => " ORDER BY id",
    };
    let sql = format!(
        "SELECT id, title, sort_title, original_title, original_language, year, popularity,
                merged_into
         FROM media_item WHERE id IN ({marks}){lock}"
    );
    let mut works: BTreeMap<String, WorkNames> = BTreeMap::new();
    for row in sqlx::query_with(db.sql(&sql), args()?)
        .fetch_all(&mut **tx)
        .await?
    {
        // A work merged into another is found by nobody: its names are its
        // survivor's now, and it keeps none of its own — those it had go.
        if row.opt_text("merged_into")?.is_some() {
            continue;
        }
        let id = row.text("id")?;
        works.insert(
            id.clone(),
            WorkNames {
                id,
                title: row.text("title")?,
                sort_title: row.opt_text("sort_title")?,
                original_title: row.opt_text("original_title")?,
                original_language: row.opt_text("original_language")?,
                year: row.opt_int("year")?,
                popularity: row.opt_real("popularity")?,
                ..Default::default()
            },
        );
    }

    let sql = format!(
        "SELECT media_id, title, language FROM media_alternative_title
         WHERE media_id IN ({marks}) ORDER BY media_id, title"
    );
    for row in sqlx::query_with(db.sql(&sql), args()?)
        .fetch_all(&mut **tx)
        .await?
    {
        if let Some(work) = works.get_mut(&row.text("media_id")?) {
            work.alternatives
                .push((row.text("title")?, row.opt_text("language")?));
        }
    }

    let sql = format!(
        "SELECT media_id, language, title FROM media_translation
         WHERE media_id IN ({marks}) AND title IS NOT NULL ORDER BY media_id, language"
    );
    for row in sqlx::query_with(db.sql(&sql), args()?)
        .fetch_all(&mut **tx)
        .await?
    {
        if let Some(work) = works.get_mut(&row.text("media_id")?) {
            work.translations
                .push((row.text("title")?, row.text("language")?));
        }
    }

    let sql = format!(
        "SELECT media_id, id, season_number, episode_number, title, air_date FROM media_episode
         WHERE media_id IN ({marks}) ORDER BY media_id, season_number, episode_number"
    );
    let mut places: HashMap<(String, i32, i32), usize> = HashMap::new();
    for row in sqlx::query_with(db.sql(&sql), args()?)
        .fetch_all(&mut **tx)
        .await?
    {
        let work_id = row.text("media_id")?;
        if let Some(work) = works.get_mut(&work_id) {
            places.insert(
                (
                    work_id,
                    row.int("season_number")?,
                    row.int("episode_number")?,
                ),
                work.episodes.len(),
            );
            work.episodes.push(EpisodeName {
                id: row.text("id")?,
                title: row.text("title")?,
                locked: None,
                air_date: row.opt_text("air_date")?,
            });
        }
    }

    // The locks a work is found by: its titles, an episode's, and the
    // alternative titles somebody hid.
    let sql = format!(
        "SELECT media_id, scope, field, language, value FROM media_override
         WHERE media_id IN ({marks})
           AND ((scope = 'item' AND field IN ('title', 'originalTitle', 'sortTitle'))
                OR (field = 'title' AND scope LIKE 'episode:%')
                OR (field = 'hidden' AND scope LIKE 'alternativeTitle:%'))
         ORDER BY media_id, scope, field, language"
    );
    for row in sqlx::query_with(db.sql(&sql), args()?)
        .fetch_all(&mut **tx)
        .await?
    {
        let work_id = row.text("media_id")?;
        let Some(work) = works.get_mut(&work_id) else {
            continue;
        };
        let scope = row.text("scope")?;
        let field = row.text("field")?;
        let value = row.opt_text("value")?;
        if field == "hidden" {
            if value.as_deref() == Some("true")
                && let Ok(Child::AlternativeTitle { language, title }) = scope.parse::<Child>()
            {
                work.hidden.insert((language.unwrap_or_default(), title));
            }
            continue;
        }
        // A lock holds JSON; a title cleared by hand holds nothing to find.
        let Some(text) = value
            .as_deref()
            .and_then(|v| serde_json::from_str::<String>(v).ok())
            .filter(|t| !t.trim().is_empty())
        else {
            continue;
        };
        if scope == "item" {
            work.locked.push((text, row.text("language")?));
        } else if let Some((season, episode)) = scope
            .strip_prefix("episode:")
            .and_then(|place| place.split_once('x'))
            .and_then(|(s, e)| Some((s.parse::<i32>().ok()?, e.parse::<i32>().ok()?)))
            && let Some(at) = places.get(&(work_id, season, episode))
        {
            work.episodes[*at].locked = Some(text);
        }
    }

    let sql = format!(
        "SELECT DISTINCT person_id FROM media_credit
          WHERE media_id IN ({marks}) AND person_id IS NOT NULL"
    );
    for row in sqlx::query_with(db.sql(&sql), args()?)
        .fetch_all(&mut **tx)
        .await?
    {
        people.insert(row.text("person_id")?);
    }

    Ok(works.into_values().collect())
}

/// A row as it stands in the table, for comparing with what it should be.
struct Standing {
    work_id: Option<String>,
    ref_id: String,
    popularity: Option<f64>,
    year: Option<i32>,
}

/// Whether two popularities are the same figure, a missing one being none.
fn same(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() < 1e-9,
        (None, None) => true,
        _ => false,
    }
}

/// Make the rows of these works what [`index::work_entries`] says they are.
async fn write_entries(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    ids: &[String],
    works: &[WorkNames],
) -> Result<()> {
    let sql = format!(
        "SELECT id, work_id, ref_id, popularity, year FROM search_doc WHERE work_id IN ({})",
        holes(ids.len())
    );
    let mut args = AnyArguments::default();
    for id in ids {
        bind(&mut args, id.clone())?;
    }
    let mut standing: HashMap<i64, Standing> = HashMap::new();
    for row in sqlx::query_with(db.sql(&sql), args)
        .fetch_all(&mut **tx)
        .await?
    {
        standing.insert(
            row.big("id")?,
            Standing {
                work_id: row.opt_text("work_id")?,
                ref_id: row.text("ref_id")?,
                popularity: row.opt_real("popularity")?,
                year: row.opt_int("year")?,
            },
        );
    }

    let wanted: Vec<Entry> = works.iter().flat_map(index::work_entries).collect();
    let wanted_ids: HashSet<i64> = wanted.iter().map(|e| e.id).collect();

    let gone: Vec<i64> = standing
        .keys()
        .filter(|id| !wanted_ids.contains(id))
        .copied()
        .collect();
    delete(db, tx, &gone).await?;

    let new: Vec<&Entry> = wanted
        .iter()
        .filter(|e| !standing.contains_key(&e.id))
        .collect();
    insert(db, tx, &new).await?;

    // A work's popularity is every one of its rows', and moves with every
    // refresh: one statement a work, not one a row.
    for work in works {
        let behind = standing.iter().any(|(id, row)| {
            wanted_ids.contains(id)
                && row.work_id.as_deref() == Some(work.id.as_str())
                && !same(row.popularity, work.popularity)
        });
        if behind {
            sqlx::query(db.sql("UPDATE search_doc SET popularity = ? WHERE work_id = ?"))
                .bind(Double(work.popularity))
                .bind(&work.id)
                .execute(&mut **tx)
                .await
                .context("failed to write a work's popularity into the index")?;
        }
    }
    for entry in &wanted {
        if let Some(row) = standing.get(&entry.id)
            && row.year != entry.year
            && row.ref_id == entry.ref_id
        {
            sqlx::query(db.sql("UPDATE search_doc SET year = ? WHERE id = ?"))
                .bind(entry.year)
                .bind(entry.id)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(())
}

async fn delete(db: &Db, tx: &mut Transaction<'_, Any>, ids: &[i64]) -> Result<()> {
    for chunk in ids.chunks(CHUNK) {
        let sql = format!(
            "DELETE FROM search_doc WHERE id IN ({})",
            holes(chunk.len())
        );
        let mut args = AnyArguments::default();
        for id in chunk {
            bind(&mut args, *id)?;
        }
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut **tx)
            .await
            .context("failed to take rows out of the index")?;
    }
    Ok(())
}

/// Write rows, several to a statement. A number already taken is left as it
/// is: the same row, or — one time in a million — another that hashed alike.
async fn insert(db: &Db, tx: &mut Transaction<'_, Any>, entries: &[&Entry]) -> Result<()> {
    for chunk in entries.chunks(ROWS_PER_INSERT) {
        let sql = format!(
            "INSERT INTO search_doc (id, kind, ref_id, work_id, lang, norm, text, popularity, year)
             VALUES {}
             ON CONFLICT (id) DO NOTHING",
            vec!["(?, ?, ?, ?, ?, ?, ?, ?, ?)"; chunk.len()].join(", ")
        );
        let mut args = AnyArguments::default();
        for entry in chunk {
            bind(&mut args, entry.id)?;
            bind(&mut args, entry.kind.as_str().to_string())?;
            bind(&mut args, entry.ref_id.clone())?;
            bind(&mut args, entry.work_id.clone())?;
            bind(&mut args, entry.lang.clone())?;
            bind(&mut args, entry.norm.clone())?;
            bind(&mut args, entry.text.clone())?;
            bind(&mut args, Double(entry.popularity))?;
            bind(&mut args, entry.year)?;
        }
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut **tx)
            .await
            .context("failed to write rows into the index")?;
    }
    Ok(())
}

/// Write again the rows of these people — by their records' ids — from
/// their records: every name they go by, worth as many works as credit them
/// by a credit no lock hides. Somebody no such credit names, or whose record
/// went into another's, has no row. What the index of people counts them by
/// is written beside (`repo::person::recount`). One transaction a few hundred
/// people, in the order of their ids.
pub async fn index_people(db: &Db, ids: &BTreeSet<String>) -> Result<()> {
    let ids: Vec<&String> = ids.iter().collect();
    for chunk in ids.chunks(CHUNK) {
        let mut tx = db.begin_write().await?;
        write_people(db, &mut tx, chunk).await?;
        tx.commit()
            .await
            .context("failed to commit the people's rows of the index")?;
    }
    Ok(())
}

async fn write_people(db: &Db, tx: &mut Transaction<'_, Any>, ids: &[&String]) -> Result<()> {
    let marks = holes(ids.len());
    let args = || -> Result<AnyArguments> {
        let mut args = AnyArguments::default();
        for id in ids {
            bind(&mut args, (*id).clone())?;
        }
        Ok(args)
    };

    let mut people: BTreeMap<String, PersonNames> = BTreeMap::new();
    let sql = format!(
        "SELECT id, name, original_name FROM person
          WHERE id IN ({marks}) AND merged_into IS NULL"
    );
    for row in sqlx::query_with(db.sql(&sql), args()?)
        .fetch_all(&mut **tx)
        .await?
    {
        let id = row.text("id")?;
        let mut names = vec![row.text("name")?];
        names.extend(row.opt_text("original_name")?);
        people.insert(
            id.clone(),
            PersonNames {
                id,
                names,
                works: 0,
            },
        );
    }
    let sql = format!(
        "SELECT person_id, name FROM person_alias
          WHERE person_id IN ({marks}) ORDER BY person_id, source, name"
    );
    for row in sqlx::query_with(db.sql(&sql), args()?)
        .fetch_all(&mut **tx)
        .await?
    {
        if let Some(person) = people.get_mut(&row.text("person_id")?) {
            person.names.push(row.text("name")?);
        }
    }
    let sql = format!(
        "SELECT c.person_id, COUNT(DISTINCT c.media_id) AS works FROM media_credit c
          WHERE c.person_id IN ({marks}) AND NOT {}
          GROUP BY c.person_id",
        crate::db::repo::person::HIDDEN
    );
    for row in sqlx::query_with(db.sql(&sql), args()?)
        .fetch_all(&mut **tx)
        .await?
    {
        if let Some(person) = people.get_mut(&row.text("person_id")?) {
            person.works = row.big("works")?;
        }
    }

    let wanted: Vec<Entry> = people.values().flat_map(index::person_entries).collect();
    // And what the index of people counts them by, from the same credits.
    crate::db::repo::person::recount(db, tx, ids).await?;

    let sql = format!(
        "SELECT id, popularity FROM search_doc WHERE kind = 'person' AND ref_id IN ({marks})"
    );
    let mut standing: HashMap<i64, Option<f64>> = HashMap::new();
    for row in sqlx::query_with(db.sql(&sql), args()?)
        .fetch_all(&mut **tx)
        .await?
    {
        standing.insert(row.big("id")?, row.opt_real("popularity")?);
    }

    let wanted_ids: HashSet<i64> = wanted.iter().map(|e| e.id).collect();
    let gone: Vec<i64> = standing
        .keys()
        .filter(|id| !wanted_ids.contains(id))
        .copied()
        .collect();
    delete(db, tx, &gone).await?;
    let new: Vec<&Entry> = wanted
        .iter()
        .filter(|e| !standing.contains_key(&e.id))
        .collect();
    insert(db, tx, &new).await?;
    for entry in &wanted {
        if let Some(popularity) = standing.get(&entry.id)
            && !same(*popularity, entry.popularity)
        {
            sqlx::query(db.sql("UPDATE search_doc SET popularity = ? WHERE id = ?"))
                .bind(Double(entry.popularity))
                .bind(entry.id)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(())
}

/// The works the index is behind on, in id order: at most `limit`.
pub async fn stale(db: &Db, limit: i64) -> Result<Vec<String>> {
    let rows =
        sqlx::query(db.sql("SELECT id FROM media_item WHERE search_stale = 1 ORDER BY id LIMIT ?"))
            .bind(limit)
            .fetch_all(db.pool())
            .await?;
    rows.iter().map(|row| Ok(row.text("id")?)).collect()
}

/// Write the rows of up to `most` works the index is behind on, a few works
/// a transaction, then the people their credits name. Returns how many were
/// written, and whether more are behind.
pub async fn catch_up(db: &Db, most: usize) -> Result<(usize, bool)> {
    let ids = stale(db, most as i64 + 1).await?;
    let more = ids.len() > most;
    let ids = &ids[..ids.len().min(most)];

    let mut people = BTreeSet::new();
    for chunk in ids.chunks(WORKS_PER_WRITE) {
        let mut tx = db.begin_write().await?;
        people.extend(index_works(db, &mut tx, chunk).await?);
        tx.commit()
            .await
            .context("failed to commit works' rows of the index")?;
    }
    index_people(db, &people).await?;
    Ok((ids.len(), more))
}

/// Mark every work as behind, for the index to be written again from them all.
pub async fn mark_all_stale(db: &Db) -> Result<u64> {
    Ok(
        sqlx::query(db.sql("UPDATE media_item SET search_stale = 1 WHERE search_stale = 0"))
            .execute(db.pool())
            .await?
            .rows_affected(),
    )
}

/// Write again the rows of everybody the catalogue keeps a record of, and
/// take out the rows of anybody it does not — a record merged into another,
/// a row nothing names. Returns how many people there are.
pub async fn index_everybody(db: &Db) -> Result<usize> {
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for row in sqlx::query(db.sql("SELECT id FROM person WHERE merged_into IS NULL"))
        .fetch_all(db.pool())
        .await?
    {
        ids.insert(row.text("id")?);
    }
    index_people(db, &ids).await?;

    let rows = sqlx::query(db.sql("SELECT id, ref_id FROM search_doc WHERE kind = 'person'"))
        .fetch_all(db.pool())
        .await?;
    let mut gone: Vec<i64> = Vec::new();
    for row in &rows {
        if !ids.contains(&row.text("ref_id")?) {
            gone.push(row.big("id")?);
        }
    }
    for chunk in gone.chunks(CHUNK) {
        let mut tx = db.begin_write().await?;
        delete(db, &mut tx, chunk).await?;
        tx.commit().await?;
    }
    Ok(ids.len())
}

/// How the index stands: its rows of each kind, and the works it is behind on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Standings {
    pub works: i64,
    pub people: i64,
    pub episodes: i64,
    pub behind: i64,
    /// The works the catalogue holds.
    pub held: i64,
}

pub async fn standings(db: &Db) -> Result<Standings> {
    let mut found = Standings::default();
    for row in sqlx::query(db.sql("SELECT kind, COUNT(*) AS n FROM search_doc GROUP BY kind"))
        .fetch_all(db.pool())
        .await?
    {
        let n = row.big("n")?;
        match row.text("kind")?.parse::<Kind>() {
            Ok(Kind::Work) => found.works = n,
            Ok(Kind::Person) => found.people = n,
            Ok(Kind::Episode) => found.episodes = n,
            Err(_) => {}
        }
    }
    let row = sqlx::query(db.sql(
        "SELECT COUNT(*) AS held,
                COALESCE(SUM(CASE WHEN search_stale = 1 THEN 1 ELSE 0 END), 0) AS behind
         FROM media_item",
    ))
    .fetch_one(db.pool())
    .await?;
    found.held = row.big("held")?;
    found.behind = row.big("behind")?;
    Ok(found)
}

/// Of these people, by their records' ids, those credited on a work a
/// reader may be shown — switched on, and not for adults unless they may see
/// those — by a credit no lock hides. A person's row is everybody's, and
/// only their works say whether a reader may be told of them.
pub async fn shown_people(
    db: &Db,
    ids: &[String],
    include_adult: bool,
    include_disabled: bool,
) -> Result<HashSet<String>> {
    let mut shown = HashSet::new();
    let mut works = String::new();
    if !include_disabled {
        works.push_str(" AND m.is_enabled = 1");
    }
    if !include_adult {
        works.push_str(" AND m.is_adult = 0");
    }
    for chunk in ids.chunks(CHUNK) {
        let sql = format!(
            "SELECT DISTINCT c.person_id FROM media_credit c
               JOIN media_item m ON m.id = c.media_id
              WHERE c.person_id IN ({}){works} AND NOT {}",
            holes(chunk.len()),
            crate::db::repo::person::HIDDEN
        );
        let mut args = AnyArguments::default();
        for id in chunk {
            bind(&mut args, id.clone())?;
        }
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?
        {
            shown.insert(row.text("person_id")?);
        }
    }
    Ok(shown)
}

/// An episode, as a search names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Episode {
    pub id: String,
    pub work_id: String,
    pub season_number: i32,
    pub episode_number: i32,
    pub title: String,
    pub air_date: Option<String>,
}

/// These episodes, by id, in no particular order.
pub async fn episodes(db: &Db, ids: &[String]) -> Result<Vec<Episode>> {
    let mut found = Vec::new();
    for chunk in ids.chunks(CHUNK) {
        let sql = format!(
            "SELECT id, media_id, season_number, episode_number, title, air_date
               FROM media_episode WHERE id IN ({})",
            holes(chunk.len())
        );
        let mut args = AnyArguments::default();
        for id in chunk {
            bind(&mut args, id.clone())?;
        }
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?
        {
            found.push(Episode {
                id: row.text("id")?,
                work_id: row.text("media_id")?,
                season_number: row.int("season_number")?,
                episode_number: row.int("episode_number")?,
                title: row.text("title")?,
                air_date: row.opt_text("air_date")?,
            });
        }
    }
    Ok(found)
}
