//! People: their records, the credits filed under them, and how a credit
//! finds the person it is of.
//!
//! A credit is filed under its person when it is written. The rules, in the
//! order they are tried:
//!
//! 1. **Kept.** A credit a work's sources give again — the same kind, on the
//!    same episode or on none, naming the same TMDB id or else the same name —
//!    keeps the person it had. So whatever was decided by hand outlives the
//!    next refresh: a credit split off a homonym, attached to somebody, or
//!    merged with its person into another stays where it was put.
//! 2. **TMDB's id.** The person with that TMDB id, or the one a person with it
//!    was merged into; one TheTVDB's id names and TMDB's does not yet, given
//!    it; else a new record.
//! 3. **TheTVDB's id**, for a credit TMDB gives no id for.
//! 4. **The name**, the person's own or one they go by: the one person known
//!    by a TMDB id who goes by it — the rule migration 0041 joined the credits
//!    held then by — else the first person known by that name alone, else a
//!    new record.
//!
//! What a credit's source says beyond the name completes a record found by
//! an id it gives — TheTVDB's id, the name in its own script, a portrait —
//! and never one found by its name, which is a guess.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{Context, Result};
use sqlx::{Any, Arguments, Transaction, any::AnyArguments};

use crate::{
    db::{Db, RowExt, from_bool, new_id, now},
    domain::{
        Credit, CreditType,
        person::{Alias, Job, Person, PersonRef},
    },
};

/// How many values one statement binds of a list: few enough for every
/// engine's ceiling on bound values.
const CHUNK: usize = 200;

/// Whether a credit is one a lock hides, as `fields::edit_credit` reads the
/// locks, in SQL for the counts: a lock on the work, in every language,
/// setting `hidden` on the credit named as `fields::Child::of_credit` names
/// it — `credit:actor:tmdb:17419`, `credit:director:name:Jane Doe`. A credit
/// added by hand is never hidden, only removed. Reads the credit as `c`.
pub const HIDDEN: &str = "(c.is_manual = 0 AND EXISTS (
        SELECT 1 FROM media_override o
         WHERE o.media_id = c.media_id AND o.field = 'hidden' AND o.language = ''
           AND o.value = 'true'
           AND o.scope = 'credit:' || c.credit_type || ':'
                         || (CASE WHEN c.tmdb_person_id IS NULL THEN 'name:' || trim(c.person_name)
                                  ELSE 'tmdb:' || CAST(c.tmdb_person_id AS TEXT) END)))";

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

/// What a credit names its person by: `tmdb:<id>`, or `name:<name>` where it
/// gives no TMDB id — as a lock on a credit names one.
pub fn key(tmdb_id: Option<i64>, name: &str) -> String {
    match tmdb_id {
        Some(id) => format!("tmdb:{id}"),
        None => format!("name:{}", name.trim()),
    }
}

// ─── records ─────────────────────────────────────────────────────────────────

const COLUMNS: &str = "id, tmdb_id, imdb_id, tvdb_id, name, sort_name, original_name, gender,
    birthday, deathday, place_of_birth, biography, image, photos, known_for, homepage,
    popularity, refreshed_at, is_manual, merged_into, created_at, updated_at";

fn map(row: &sqlx::any::AnyRow) -> Result<Person> {
    Ok(Person {
        id: row.text("id")?,
        tmdb_id: row.opt_big("tmdb_id")?,
        imdb_id: row.opt_text("imdb_id")?,
        tvdb_id: row.opt_big("tvdb_id")?,
        name: row.text("name")?,
        sort_name: row.opt_text("sort_name")?,
        original_name: row.opt_text("original_name")?,
        gender: row
            .opt_int("gender")?
            .and_then(|g| u8::try_from(g).ok())
            .filter(|g| (1..=3).contains(g)),
        birthday: row.opt_text("birthday")?,
        deathday: row.opt_text("deathday")?,
        place_of_birth: row.opt_text("place_of_birth")?,
        biography: row.opt_text("biography")?,
        image: row.opt_text("image")?,
        photos: row.text_list("photos")?,
        known_for: row.opt_text("known_for")?,
        homepage: row.opt_text("homepage")?,
        popularity: row.opt_real("popularity")?,
        refreshed_at: row.opt_text("refreshed_at")?,
        is_manual: row.flag("is_manual")?,
        merged_into: row.opt_text("merged_into")?,
        created_at: row.text("created_at")?,
        updated_at: row.text("updated_at")?,
        aliases: Vec::new(),
    })
}

/// One record by its id, with the names it goes by — a merged one as it was
/// kept, `merged_into` saying where it went.
pub async fn get(db: &Db, id: &str) -> Result<Option<Person>> {
    let row = sqlx::query(db.sql(&format!("SELECT {COLUMNS} FROM person WHERE id = ?")))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mut person = map(&row)?;
    person.aliases = aliases(db, &person.id).await?;
    Ok(Some(person))
}

/// These records, as the index lists them — name, TMDB id, portrait, jobs —
/// each that was not merged into another, by its id.
pub async fn named(db: &Db, ids: &[String]) -> Result<HashMap<String, Listed>> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(CHUNK) {
        let mut args = AnyArguments::default();
        for id in chunk {
            bind(&mut args, id.clone())?;
        }
        let sql = format!(
            "SELECT {LISTED} FROM person WHERE id IN ({}) AND merged_into IS NULL",
            holes(chunk.len())
        );
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?
        {
            let person = listed(&row, None)?;
            out.insert(person.id.clone(), person);
        }
    }
    Ok(out)
}

/// The other names somebody goes by, in order.
pub async fn aliases(db: &Db, id: &str) -> Result<Vec<Alias>> {
    let rows = sqlx::query(
        db.sql("SELECT name, source FROM person_alias WHERE person_id = ? ORDER BY source, name"),
    )
    .bind(id)
    .fetch_all(db.pool())
    .await?;
    rows.iter()
        .map(|row| {
            Ok(Alias {
                name: row.text("name")?,
                source: row.text("source")?,
            })
        })
        .collect()
}

/// Somebody by an address their page is reached at: their record's id, or
/// their TMDB id — every digit — as the person pages were filed before the
/// records. A record merged into another is the one it went into.
pub async fn find(db: &Db, address: &str) -> Result<Option<Person>> {
    let address = address.trim();
    let id = if !address.is_empty() && address.bytes().all(|b| b.is_ascii_digit()) {
        let Ok(tmdb_id) = address.parse::<i64>() else {
            return Ok(None);
        };
        let id: Option<String> =
            sqlx::query_scalar(db.sql("SELECT id FROM person WHERE tmdb_id = ?"))
                .bind(tmdb_id)
                .fetch_optional(db.pool())
                .await?;
        match id {
            Some(id) => id,
            None => return Ok(None),
        }
    } else {
        address.to_string()
    };
    let Some(person) = get(db, &id).await? else {
        return Ok(None);
    };
    match person.merged_into.as_deref() {
        Some(into) => get(db, into).await,
        None => Ok(Some(person)),
    }
}

// ─── filing a credit under its person ────────────────────────────────────────

/// A credit about to be filed, as its person is found by.
#[derive(Clone, Copy, Debug)]
pub struct Linkable<'a> {
    pub kind: CreditType,
    pub name: &'a str,
    pub tmdb_id: Option<i64>,
    pub image: Option<&'a str>,
    /// The episode it is on, where it is on one.
    pub season: Option<i32>,
    pub episode: Option<i32>,
    /// What its source says of the person beyond the name.
    pub known: Option<&'a PersonRef>,
}

impl<'a> Linkable<'a> {
    pub fn of(credit: &'a Credit, place: Option<(i32, i32)>, known: Option<&'a PersonRef>) -> Self {
        Self {
            kind: credit.credit_type,
            name: &credit.person_name,
            tmdb_id: credit.tmdb_person_id,
            image: credit.image.as_deref(),
            season: place.map(|(season, _)| season),
            episode: place.map(|(_, episode)| episode),
            known,
        }
    }
}

/// Where a credit stands on a work: its kind, its episode, and whom it names.
type Standing = (&'static str, Option<i32>, Option<i32>, String);

/// The person each credit of a work was filed under before the work was
/// written again (rule 1).
#[derive(Debug, Default)]
pub struct Kept {
    exact: HashMap<Standing, String>,
    /// Whom a key names on the work, when its every credit says the same.
    by_key: HashMap<String, Option<String>>,
}

/// The persons a work's credits are filed under now, read before the write
/// replaces them.
pub async fn kept(db: &Db, tx: &mut Transaction<'_, Any>, media_id: &str) -> Result<Kept> {
    let rows = sqlx::query(db.sql(
        "SELECT credit_type, season_number, episode_number, tmdb_person_id, person_name,
                person_id
           FROM media_credit WHERE media_id = ? AND person_id IS NOT NULL",
    ))
    .bind(media_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut kept = Kept::default();
    for row in &rows {
        let kind = row
            .text("credit_type")?
            .parse::<CreditType>()
            .unwrap_or(CreditType::Actor)
            .as_str();
        let key = key(row.opt_big("tmdb_person_id")?, &row.text("person_name")?);
        let person = row.text("person_id")?;
        kept.exact
            .entry((
                kind,
                row.opt_int("season_number")?,
                row.opt_int("episode_number")?,
                key.clone(),
            ))
            .or_insert_with(|| person.clone());
        kept.by_key
            .entry(key)
            .and_modify(|held| {
                if held.as_deref() != Some(person.as_str()) {
                    *held = None;
                }
            })
            .or_insert(Some(person));
    }
    Ok(kept)
}

/// A record as the filing reads it.
#[derive(Clone, Debug)]
struct Found {
    id: String,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    name: String,
    original_name: Option<String>,
    image: Option<String>,
    gender: Option<i32>,
    popularity: Option<f64>,
    is_manual: bool,
    created_at: String,
}

const FOUND: &str = "id, tmdb_id, tvdb_id, name, original_name, image, gender, popularity,
    is_manual, merged_into, created_at";

fn found(row: &sqlx::any::AnyRow) -> Result<(Found, Option<String>)> {
    Ok((
        Found {
            id: row.text("id")?,
            tmdb_id: row.opt_big("tmdb_id")?,
            tvdb_id: row.opt_big("tvdb_id")?,
            name: row.text("name")?,
            original_name: row.opt_text("original_name")?,
            image: row.opt_text("image")?,
            gender: row.opt_int("gender")?,
            popularity: row.opt_real("popularity")?,
            is_manual: row.flag("is_manual")?,
            created_at: row.text("created_at")?,
        },
        row.opt_text("merged_into")?,
    ))
}

impl Found {
    fn of(person: &Person) -> Self {
        Self {
            id: person.id.clone(),
            tmdb_id: person.tmdb_id,
            tvdb_id: person.tvdb_id,
            name: person.name.clone(),
            original_name: person.original_name.clone(),
            image: person.image.clone(),
            gender: person.gender.map(i32::from),
            popularity: person.popularity,
            is_manual: person.is_manual,
            created_at: person.created_at.clone(),
        }
    }
}

/// The records an id column names, by that id: one merged into another is
/// the one it went into.
async fn by_id_column(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    column: &str,
    ids: &[i64],
) -> Result<HashMap<i64, Found>> {
    let mut out: HashMap<i64, Found> = HashMap::new();
    let mut merged: Vec<(i64, String)> = Vec::new();
    for chunk in ids.chunks(CHUNK) {
        let mut args = AnyArguments::default();
        for id in chunk {
            bind(&mut args, *id)?;
        }
        // The oldest first: of two records TheTVDB's id names, the first
        // made is the one taken.
        let sql = format!(
            "SELECT {FOUND} FROM person WHERE {column} IN ({}) ORDER BY created_at, id",
            holes(chunk.len())
        );
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(&mut **tx)
            .await?
        {
            let Some(id) = row.opt_big(column)? else {
                continue;
            };
            if out.contains_key(&id) || merged.iter().any(|(seen, _)| *seen == id) {
                continue;
            }
            match found(&row)? {
                (_, Some(into)) => merged.push((id, into)),
                (person, None) => {
                    out.insert(id, person);
                }
            }
        }
    }
    if !merged.is_empty() {
        let targets: Vec<String> = merged.iter().map(|(_, into)| into.clone()).collect();
        let live = by_ids(db, tx, &targets).await?;
        for (id, into) in merged {
            if let Some(person) = live.get(&into) {
                out.entry(id).or_insert_with(|| person.clone());
            }
        }
    }
    Ok(out)
}

/// Live records by their own ids.
async fn by_ids(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    ids: &[String],
) -> Result<HashMap<String, Found>> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(CHUNK) {
        let mut args = AnyArguments::default();
        for id in chunk {
            bind(&mut args, id.clone())?;
        }
        let sql = format!(
            "SELECT {FOUND} FROM person WHERE id IN ({}) AND merged_into IS NULL",
            holes(chunk.len())
        );
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(&mut **tx)
            .await?
        {
            let (person, _) = found(&row)?;
            out.insert(person.id.clone(), person);
        }
    }
    Ok(out)
}

/// The live records each of these names is theirs, or one they go by.
async fn by_names(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    names: &[String],
) -> Result<HashMap<String, Vec<Found>>> {
    let mut out: HashMap<String, Vec<Found>> = HashMap::new();
    for chunk in names.chunks(CHUNK) {
        let mut args = AnyArguments::default();
        for _ in 0..2 {
            for name in chunk {
                bind(&mut args, name.clone())?;
            }
        }
        let marks = holes(chunk.len());
        let sql = format!(
            "SELECT p.id, p.tmdb_id, p.tvdb_id, p.name, p.original_name, p.image, p.gender,
                    p.popularity, p.is_manual, p.merged_into, p.created_at, p.name AS matched
               FROM person p WHERE p.merged_into IS NULL AND p.name IN ({marks})
             UNION ALL
             SELECT p.id, p.tmdb_id, p.tvdb_id, p.name, p.original_name, p.image, p.gender,
                    p.popularity, p.is_manual, p.merged_into, p.created_at, a.name AS matched
               FROM person_alias a JOIN person p ON p.id = a.person_id
              WHERE p.merged_into IS NULL AND a.name IN ({marks})"
        );
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(&mut **tx)
            .await?
        {
            let matched = row.text("matched")?;
            let (person, _) = found(&row)?;
            let candidates = out.entry(matched).or_default();
            if !candidates.iter().any(|c| c.id == person.id) {
                candidates.push(person);
            }
        }
    }
    Ok(out)
}

/// The names each of these records goes by besides its own.
async fn alias_names(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    ids: &[String],
) -> Result<HashMap<String, HashSet<String>>> {
    let mut out: HashMap<String, HashSet<String>> = HashMap::new();
    for chunk in ids.chunks(CHUNK) {
        let mut args = AnyArguments::default();
        for id in chunk {
            bind(&mut args, id.clone())?;
        }
        let sql = format!(
            "SELECT person_id, name FROM person_alias WHERE person_id IN ({})",
            holes(chunk.len())
        );
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(&mut **tx)
            .await?
        {
            out.entry(row.text("person_id")?)
                .or_default()
                .insert(row.text("name")?);
        }
    }
    Ok(out)
}

/// Of the records a name names, the one a credit giving that name alone is
/// of (rule 4): the one person known by a TMDB id among them, else the first
/// known by the name alone. None when two known by TMDB ids share it and
/// nobody else does: which of them it is, nothing says.
fn chosen(candidates: &[Found]) -> Option<&Found> {
    let mut keyed = candidates.iter().filter(|c| c.tmdb_id.is_some());
    if let (Some(only), None) = (keyed.next(), keyed.next()) {
        return Some(only);
    }
    candidates
        .iter()
        .filter(|c| c.tmdb_id.is_none())
        .min_by(|a, b| (&a.created_at, &a.id).cmp(&(&b.created_at, &b.id)))
}

/// What a record found by an id is completed with.
#[derive(Debug, Default)]
struct Fill {
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    original_name: Option<String>,
    gender: Option<i32>,
    popularity: Option<f64>,
    image: Option<String>,
    aliases: Vec<String>,
}

impl Fill {
    fn is_empty(&self) -> bool {
        self.tmdb_id.is_none()
            && self.tvdb_id.is_none()
            && self.original_name.is_none()
            && self.gender.is_none()
            && self.popularity.is_none()
            && self.image.is_none()
            && self.aliases.is_empty()
    }
}

/// What a credit's source says that the record it was found by an id of
/// lacks: TheTVDB's id where nobody else holds it, the name in its own
/// script, a gender, a figure, a portrait, the spelling it gives. A record
/// put right by hand takes only what it has none of.
fn complete(
    fill: &mut Fill,
    person: &Found,
    credit: &Linkable<'_>,
    aliases: Option<&HashSet<String>>,
    tvdb_free: bool,
) {
    let known = credit.known;
    if person.tvdb_id.is_none()
        && fill.tvdb_id.is_none()
        && tvdb_free
        && let Some(tvdb) = known.and_then(|k| k.tvdb_id)
    {
        fill.tvdb_id = Some(tvdb);
    }
    if let Some(original) = known
        .and_then(|k| k.original_name.as_deref())
        .map(str::trim)
        .filter(|o| !o.is_empty() && *o != person.name)
        && person.original_name.as_deref() != Some(original)
        && (person.original_name.is_none() || !person.is_manual)
        && fill.original_name.is_none()
    {
        fill.original_name = Some(original.to_string());
    }
    if person.gender.is_none() && fill.gender.is_none() {
        fill.gender = known
            .and_then(|k| k.gender)
            .filter(|g| (1..=3).contains(g))
            .map(i32::from);
    }
    if person.popularity.is_none() && fill.popularity.is_none() {
        fill.popularity = known.and_then(|k| k.popularity);
    }
    if person.image.is_none() && fill.image.is_none() {
        fill.image = credit.image.map(str::to_string);
    }
    let name = credit.name.trim();
    if name != person.name
        && !aliases.is_some_and(|names| names.contains(name))
        && !fill.aliases.iter().any(|a| a == name)
    {
        fill.aliases.push(name.to_string());
    }
}

/// A new record for the person a credit names.
fn made(credit: &Linkable<'_>, tmdb_id: Option<i64>, tvdb_id: Option<i64>) -> Person {
    let mut person = Person::named(credit.name);
    person.tmdb_id = tmdb_id;
    person.tvdb_id = tvdb_id;
    person.image = credit.image.map(str::to_string);
    if let Some(known) = credit.known {
        person.original_name = known
            .original_name
            .as_deref()
            .map(str::trim)
            .filter(|o| !o.is_empty() && *o != person.name)
            .map(str::to_string);
        person.gender = known.gender.filter(|g| (1..=3).contains(g));
        person.popularity = known.popularity;
    }
    person
}

/// Find, or make, the person each of these credits of one work is of, in
/// `tx`, by the rules this module opens with; `kept` is how the work's
/// credits were filed before. One person's id for each credit, in their
/// order — none for a credit that names nobody, its name blank.
pub async fn resolve(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    kept: &Kept,
    credits: &[Linkable<'_>],
) -> Result<Vec<Option<String>>> {
    let mut out: Vec<Option<String>> = vec![None; credits.len()];
    let mut pending: Vec<usize> = Vec::new();
    let mut held: Vec<usize> = Vec::new();
    for (at, credit) in credits.iter().enumerate() {
        let name = credit.name.trim();
        if name.is_empty() {
            continue;
        }
        let key = key(credit.tmdb_id, name);
        let standing = (credit.kind.as_str(), credit.season, credit.episode, key);
        if let Some(person) = kept.exact.get(&standing) {
            out[at] = Some(person.clone());
            held.push(at);
        } else if let Some(Some(person)) = kept.by_key.get(&standing.3) {
            out[at] = Some(person.clone());
            held.push(at);
        } else {
            pending.push(at);
        }
    }
    // A credit kept is completed too, where its source says more: worth
    // reading its person for only then.
    held.retain(|&at| credits[at].known.is_some() || credits[at].image.is_some());
    if pending.is_empty() && held.is_empty() {
        return Ok(out);
    }
    // TMDB's credits first: a credit by name alone is then taken for
    // somebody TMDB names in the same write — a guest of an episode — rather
    // than made a record of its own.
    pending.sort_by_key(|&at| credits[at].tmdb_id.is_none());

    let mut tmdb_ids: BTreeSet<i64> = BTreeSet::new();
    let mut tvdb_ids: BTreeSet<i64> = BTreeSet::new();
    let mut names: BTreeSet<String> = BTreeSet::new();
    for &at in &pending {
        let credit = &credits[at];
        match credit.tmdb_id {
            Some(id) => {
                tmdb_ids.insert(id);
            }
            None => {
                names.insert(credit.name.trim().to_string());
            }
        }
    }
    for &at in pending.iter().chain(&held) {
        if let Some(tvdb) = credits[at].known.and_then(|k| k.tvdb_id) {
            tvdb_ids.insert(tvdb);
        }
    }
    let tmdb_ids: Vec<i64> = tmdb_ids.into_iter().collect();
    let tvdb_ids: Vec<i64> = tvdb_ids.into_iter().collect();
    let names: Vec<String> = names.into_iter().collect();
    let mut by_tmdb = by_id_column(db, tx, "tmdb_id", &tmdb_ids).await?;
    let mut by_tvdb = by_id_column(db, tx, "tvdb_id", &tvdb_ids).await?;
    let mut by_name = by_names(db, tx, &names).await?;
    let mut kept_ids: Vec<String> = held.iter().filter_map(|&at| out[at].clone()).collect();
    kept_ids.sort();
    kept_ids.dedup();
    let kept_people = by_ids(db, tx, &kept_ids).await?;
    let found_ids: Vec<String> = by_tmdb
        .values()
        .chain(by_tvdb.values())
        .chain(kept_people.values())
        .map(|p| p.id.clone())
        .collect();
    let aliases = alias_names(db, tx, &found_ids).await?;

    // What the sources say completes the person a kept credit is filed
    // under where the credit names them by an id they are known by: what
    // was decided by hand is kept, and what is certain still comes in.
    let mut fills: BTreeMap<String, Fill> = BTreeMap::new();
    for &at in &held {
        let credit = &credits[at];
        let Some(person) = out[at].as_ref().and_then(|id| kept_people.get(id)) else {
            continue;
        };
        let tvdb = credit.known.and_then(|k| k.tvdb_id);
        let certain = credit.tmdb_id.is_some_and(|id| person.tmdb_id == Some(id))
            || tvdb.is_some_and(|id| person.tvdb_id == Some(id));
        if certain {
            let free = tvdb.is_some_and(|t| !by_tvdb.contains_key(&t));
            let fill = fills.entry(person.id.clone()).or_default();
            complete(fill, person, credit, aliases.get(&person.id), free);
        }
    }

    let mut new: Vec<Person> = Vec::new();
    for at in pending {
        let credit = &credits[at];
        let name = credit.name.trim();
        let tvdb = credit.known.and_then(|k| k.tvdb_id);
        let person = if let Some(tmdb) = credit.tmdb_id {
            let by_tvdb_alone = tvdb
                .and_then(|t| by_tvdb.get(&t))
                .filter(|p| p.tmdb_id.is_none())
                .cloned();
            if let Some(person) = by_tmdb.get(&tmdb) {
                let free = tvdb.is_some_and(|t| !by_tvdb.contains_key(&t));
                let fill = fills.entry(person.id.clone()).or_default();
                complete(fill, person, credit, aliases.get(&person.id), free);
                person.id.clone()
            } else if let Some(mut person) = by_tvdb_alone {
                // TheTVDB's person, whom TMDB now names too.
                let fill = fills.entry(person.id.clone()).or_default();
                fill.tmdb_id = Some(tmdb);
                complete(fill, &person, credit, aliases.get(&person.id), false);
                person.tmdb_id = Some(tmdb);
                let id = person.id.clone();
                by_tmdb.insert(tmdb, person);
                id
            } else {
                let tvdb = tvdb.filter(|t| !by_tvdb.contains_key(t));
                let person = made(credit, Some(tmdb), tvdb);
                let found = Found::of(&person);
                by_tmdb.insert(tmdb, found.clone());
                if let Some(t) = tvdb {
                    by_tvdb.insert(t, found.clone());
                }
                by_name.entry(person.name.clone()).or_default().push(found);
                let id = person.id.clone();
                new.push(person);
                id
            }
        } else if let Some(person) = tvdb.and_then(|t| by_tvdb.get(&t)) {
            let fill = fills.entry(person.id.clone()).or_default();
            complete(fill, person, credit, aliases.get(&person.id), false);
            person.id.clone()
        } else if let Some(person) = by_name.get(name).and_then(|c| chosen(c)) {
            person.id.clone()
        } else {
            let tvdb = tvdb.filter(|t| !by_tvdb.contains_key(t));
            let person = made(credit, None, tvdb);
            let found = Found::of(&person);
            if let Some(t) = tvdb {
                by_tvdb.insert(t, found.clone());
            }
            by_name.entry(person.name.clone()).or_default().push(found);
            let id = person.id.clone();
            new.push(person);
            id
        };
        out[at] = Some(person);
    }

    // The new records first, in the order of their TMDB ids, so that two
    // writes making the same ones on PostgreSQL take them in one order; the
    // one that lost a TMDB id to the other takes the record that won it.
    new.sort_by(|a, b| {
        (a.tmdb_id.is_none(), a.tmdb_id, &a.id).cmp(&(b.tmdb_id.is_none(), b.tmdb_id, &b.id))
    });
    insert(db, tx, &new).await?;
    let raced: Vec<i64> = new.iter().filter_map(|p| p.tmdb_id).collect();
    if !raced.is_empty() {
        let held = held_tmdb(db, tx, &raced).await?;
        for person in new.iter().filter(|p| p.tmdb_id.is_some()) {
            let Some(winner) = person.tmdb_id.and_then(|id| held.get(&id)) else {
                continue;
            };
            if *winner != person.id {
                for slot in out.iter_mut() {
                    if slot.as_deref() == Some(person.id.as_str()) {
                        *slot = Some(winner.clone());
                    }
                }
            }
        }
    }

    // Then what the records found by an id are completed with, in the order
    // of their ids.
    for (id, fill) in fills {
        if !fill.is_empty() {
            apply(db, tx, &id, &fill).await?;
        }
    }
    Ok(out)
}

/// The record each of these TMDB ids is held by.
async fn held_tmdb(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    ids: &[i64],
) -> Result<HashMap<i64, String>> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(CHUNK) {
        let mut args = AnyArguments::default();
        for id in chunk {
            bind(&mut args, *id)?;
        }
        let sql = format!(
            "SELECT id, tmdb_id FROM person WHERE tmdb_id IN ({})",
            holes(chunk.len())
        );
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(&mut **tx)
            .await?
        {
            if let Some(tmdb) = row.opt_big("tmdb_id")? {
                out.insert(tmdb, row.text("id")?);
            }
        }
    }
    Ok(out)
}

/// Write new records; one whose TMDB id another took meanwhile is not.
async fn insert(db: &Db, tx: &mut Transaction<'_, Any>, people: &[Person]) -> Result<()> {
    for person in people {
        sqlx::query(db.sql(
            "INSERT INTO person
                 (id, tmdb_id, tvdb_id, name, original_name, gender, image, popularity,
                  is_manual, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (tmdb_id) DO NOTHING",
        ))
        .bind(&person.id)
        .bind(person.tmdb_id)
        .bind(person.tvdb_id)
        .bind(&person.name)
        .bind(&person.original_name)
        .bind(person.gender.map(i32::from))
        .bind(&person.image)
        .bind(person.popularity)
        .bind(from_bool(person.is_manual))
        .bind(&person.created_at)
        .bind(&person.updated_at)
        .execute(&mut **tx)
        .await
        .context("failed to write a person")?;
    }
    Ok(())
}

/// Complete a record: only what it has none of, but the name in its own
/// script, which TMDB may spell anew.
async fn apply(db: &Db, tx: &mut Transaction<'_, Any>, id: &str, fill: &Fill) -> Result<()> {
    let mut sets: Vec<&str> = Vec::new();
    let mut args = AnyArguments::default();
    if let Some(tmdb) = fill.tmdb_id {
        sets.push("tmdb_id = COALESCE(tmdb_id, ?)");
        bind(&mut args, tmdb)?;
    }
    if let Some(tvdb) = fill.tvdb_id {
        sets.push("tvdb_id = COALESCE(tvdb_id, ?)");
        bind(&mut args, tvdb)?;
    }
    if let Some(original) = &fill.original_name {
        sets.push("original_name = ?");
        bind(&mut args, original.clone())?;
    }
    if let Some(gender) = fill.gender {
        sets.push("gender = COALESCE(gender, ?)");
        bind(&mut args, gender)?;
    }
    if let Some(popularity) = fill.popularity {
        sets.push("popularity = COALESCE(popularity, ?)");
        bind(&mut args, popularity)?;
    }
    if let Some(image) = &fill.image {
        sets.push("image = COALESCE(image, ?)");
        bind(&mut args, image.clone())?;
    }
    if !sets.is_empty() {
        sets.push("updated_at = ?");
        bind(&mut args, now())?;
        bind(&mut args, id.to_string())?;
        let sql = format!("UPDATE person SET {} WHERE id = ?", sets.join(", "));
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut **tx)
            .await
            .context("failed to complete a person")?;
    }
    for name in &fill.aliases {
        add_alias(db, tx, id, name, "credit").await?;
    }
    Ok(())
}

async fn add_alias(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    id: &str,
    name: &str,
    source: &str,
) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(());
    }
    sqlx::query(db.sql(
        "INSERT INTO person_alias (person_id, name, source) VALUES (?, ?, ?)
         ON CONFLICT (person_id, name) DO NOTHING",
    ))
    .bind(id)
    .bind(name)
    .bind(source)
    .execute(&mut **tx)
    .await
    .context("failed to write a name somebody goes by")?;
    Ok(())
}

/// A credit filed under nobody yet: its id, what it is, and the episode it
/// is on, where it is on one.
type Unfiled = (String, Credit, Option<(i32, i32)>);

/// File the credits of these works that name nobody yet — those added by
/// hand, which their own path writes without a person — and return whom they
/// were filed under. Read by the search index's writer, which every such
/// credit marks its work for.
pub async fn link_unlinked(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    media_ids: &[String],
) -> Result<BTreeSet<String>> {
    let mut linked = BTreeSet::new();
    for chunk in media_ids.chunks(CHUNK) {
        let mut args = AnyArguments::default();
        for id in chunk {
            bind(&mut args, id.clone())?;
        }
        let sql = format!(
            "SELECT id, media_id, credit_type, person_name, image, tmdb_person_id,
                    season_number, episode_number
               FROM media_credit
              WHERE media_id IN ({}) AND person_id IS NULL AND trim(person_name) <> ''
              ORDER BY media_id, id",
            holes(chunk.len())
        );
        let rows = sqlx::query_with(db.sql(&sql), args)
            .fetch_all(&mut **tx)
            .await?;
        let mut by_work: BTreeMap<String, Vec<Unfiled>> = BTreeMap::new();
        for row in &rows {
            let mut credit = crate::db::repo::child::blank_credit(
                row.text("credit_type")?
                    .parse::<CreditType>()
                    .unwrap_or(CreditType::Actor),
                row.text("person_name")?,
            );
            credit.image = row.opt_text("image")?;
            credit.tmdb_person_id = row.opt_big("tmdb_person_id")?;
            let place = row
                .opt_int("season_number")?
                .zip(row.opt_int("episode_number")?);
            by_work.entry(row.text("media_id")?).or_default().push((
                row.text("id")?,
                credit,
                place,
            ));
        }
        for (media_id, credits) in by_work {
            let kept = kept(db, tx, &media_id).await?;
            let linkables: Vec<Linkable<'_>> = credits
                .iter()
                .map(|(_, credit, place)| Linkable::of(credit, *place, None))
                .collect();
            let people = resolve(db, tx, &kept, &linkables).await?;
            for ((id, _, _), person) in credits.iter().zip(people) {
                let Some(person) = person else { continue };
                sqlx::query(db.sql("UPDATE media_credit SET person_id = ? WHERE id = ?"))
                    .bind(&person)
                    .bind(id)
                    .execute(&mut **tx)
                    .await?;
                linked.insert(person);
            }
        }
    }
    Ok(linked)
}

// ─── what a person did ───────────────────────────────────────────────────────

/// One of a person's credits, on a work the reader may be shown.
#[derive(Clone, Debug)]
pub struct Credited {
    pub media_id: String,
    pub credit: Credit,
    /// The episode it is on, with its title and the day it aired.
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub episode_title: Option<String>,
    pub air_date: Option<String>,
}

/// The most credits a person's page reads: a guest of a thousand episodes is
/// a guest of the first thousand.
const MOST_CREDITS: i64 = 2000;

/// Every credit filed under a person, on works switched on — for adults only
/// where `include_adult` — the newest work first, as each work's locks have
/// them: hidden there, not one of theirs here; corrected, as corrected.
pub async fn credits(db: &Db, person_id: &str, include_adult: bool) -> Result<Vec<Credited>> {
    let adult = if include_adult {
        ""
    } else {
        " AND m.is_adult = 0"
    };
    let sql = format!(
        "SELECT c.media_id, c.id, c.credit_type, c.person_name, c.character_name, c.image,
                c.tmdb_person_id, c.credit_tmdb_id, c.sort_order, c.is_manual,
                c.season_number, c.episode_number,
                e.title AS episode_title, e.air_date AS episode_air_date
           FROM media_credit c
           JOIN media_item m ON m.id = c.media_id
           LEFT JOIN media_episode e
                  ON e.media_id = c.media_id AND e.season_number = c.season_number
                 AND e.episode_number = c.episode_number
          WHERE c.person_id = ? AND m.is_enabled = 1{adult}
          ORDER BY m.year DESC NULLS LAST, m.title, m.id, c.season_number, c.episode_number,
                   c.sort_order
          LIMIT {MOST_CREDITS}"
    );
    let rows = sqlx::query(db.sql(&sql))
        .bind(person_id)
        .fetch_all(db.pool())
        .await?;

    let mut found = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut credit = crate::db::repo::child::blank_credit(
            row.text("credit_type")?
                .parse::<CreditType>()
                .unwrap_or(CreditType::Actor),
            row.text("person_name")?,
        );
        credit.id = row.text("id")?;
        credit.character_name = row.opt_text("character_name")?;
        credit.image = row.opt_text("image")?;
        credit.tmdb_person_id = row.opt_big("tmdb_person_id")?;
        credit.credit_tmdb_id = row.opt_text("credit_tmdb_id")?;
        credit.sort_order = row.int("sort_order")?;
        credit.is_manual = row.flag("is_manual")?;
        found.push(Credited {
            media_id: row.text("media_id")?,
            credit,
            season_number: row.opt_int("season_number")?,
            episode_number: row.opt_int("episode_number")?,
            episode_title: row
                .opt_text("episode_title")?
                .filter(|t| !t.trim().is_empty()),
            air_date: row.opt_text("episode_air_date")?,
        });
    }

    let works: Vec<String> = {
        let mut ids: Vec<String> = found.iter().map(|c| c.media_id.clone()).collect();
        ids.sort();
        ids.dedup();
        ids
    };
    let locks = crate::db::repo::override_field::list_for_many(db, &works).await?;
    Ok(found
        .into_iter()
        .filter_map(|mut credited| {
            let locked = locks
                .get(&credited.media_id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            credited.credit = crate::domain::fields::edit_credit(credited.credit, locked)?;
            Some(credited)
        })
        .collect())
}

/// The languages the works credited to somebody were made in, one for each
/// work: what tells which language a name in their own script is in.
pub async fn languages(db: &Db, person_id: &str) -> Result<Vec<String>> {
    let rows = sqlx::query(db.sql(
        "SELECT DISTINCT m.id, m.original_language FROM media_credit c
           JOIN media_item m ON m.id = c.media_id
          WHERE c.person_id = ? AND m.original_language IS NOT NULL",
    ))
    .bind(person_id)
    .fetch_all(db.pool())
    .await?;
    rows.iter()
        .map(|row| Ok(row.text("original_language")?))
        .collect()
}

// ─── the index of people ─────────────────────────────────────────────────────

/// Somebody as the index lists them.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub id: String,
    pub name: String,
    pub tmdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub tvdb_id: Option<i64>,
    pub image: Option<String>,
    pub is_manual: bool,
    /// The letter the index files them under.
    pub letter: char,
    /// How many works anybody may see credit them by a credit no lock hides
    /// — with the job asked for, when one was.
    pub works: i64,
    /// What they did on those works.
    pub jobs: Vec<Job>,
    /// The other names they go by, read only for those who keep the
    /// catalogue.
    pub aliases: Vec<String>,
}

/// The letter a name is filed under: its first, folded, among A to Z; `#`
/// for anything else.
pub fn letter_of(name: &str) -> char {
    match crate::search::norm(name).chars().next() {
        Some(c) if c.is_ascii_lowercase() => c.to_ascii_uppercase(),
        _ => '#',
    }
}

/// The column a job's count is kept in, or the count of every work.
const fn counted(job: Option<Job>) -> &'static str {
    match job {
        None => "works",
        Some(Job::Acting) => "acting",
        Some(Job::Directing) => "directing",
        Some(Job::Writing) => "writing",
        Some(Job::Producing) => "producing",
    }
}

const LISTED: &str = "id, name, tmdb_id, imdb_id, tvdb_id, image, is_manual, letter, works,
    acting, directing, writing, producing";

fn listed(row: &sqlx::any::AnyRow, job: Option<Job>) -> Result<Listed> {
    let mut jobs = Vec::new();
    for (column, job) in [
        ("acting", Job::Acting),
        ("directing", Job::Directing),
        ("writing", Job::Writing),
        ("producing", Job::Producing),
    ] {
        if row.big(column)? > 0 {
            jobs.push(job);
        }
    }
    Ok(Listed {
        id: row.text("id")?,
        name: row.text("name")?,
        tmdb_id: row.opt_big("tmdb_id")?,
        imdb_id: row.opt_text("imdb_id")?,
        tvdb_id: row.opt_big("tvdb_id")?,
        image: row.opt_text("image")?,
        is_manual: row.flag("is_manual")?,
        letter: row.text("letter")?.chars().next().unwrap_or('#'),
        works: row.big(counted(job))?,
        jobs,
        aliases: Vec::new(),
    })
}

/// What a page of the index is asked with.
#[derive(Clone, Copy, Debug, Default)]
pub struct Asked {
    /// Those who did it, counted by the works they did it on.
    pub job: Option<Job>,
    /// `A` to `Z`, or `#`.
    pub letter: Option<char>,
    /// By name rather than the most credited first.
    pub by_name: bool,
    pub limit: i64,
    pub offset: i64,
}

/// A page of the index, how many there are, and the letters somebody is
/// filed under before the one asked for narrows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Indexed {
    pub people: Vec<Listed>,
    pub total: i64,
    pub letters: Vec<char>,
}

/// A page of everybody credited on a work anybody may see by a credit no
/// lock hides, as their records keep the counts (`repo::search` writes them
/// whenever a work's credits change): the most credited first, or by name.
pub async fn index(db: &Db, asked: Asked) -> Result<Indexed> {
    let column = counted(asked.job);
    let mut filter = format!("merged_into IS NULL AND {column} > 0");

    let mut letters: Vec<char> = Vec::new();
    for row in sqlx::query(db.sql(&format!(
        "SELECT DISTINCT letter FROM person WHERE {filter} ORDER BY letter"
    )))
    .fetch_all(db.pool())
    .await?
    {
        if let Some(letter) = row.text("letter")?.chars().next() {
            letters.push(letter);
        }
    }

    let mut args = AnyArguments::default();
    if let Some(letter) = asked.letter {
        filter.push_str(" AND letter = ?");
        bind(&mut args, letter.to_string())?;
    }
    let total: i64 = sqlx::query_scalar_with(
        db.sql(&format!("SELECT COUNT(*) FROM person WHERE {filter}")),
        args,
    )
    .fetch_one(db.pool())
    .await?;

    let order = match asked.by_name {
        true => "sort_key, id".to_string(),
        false => format!("{column} DESC, sort_key, id"),
    };
    let mut args = AnyArguments::default();
    if let Some(letter) = asked.letter {
        bind(&mut args, letter.to_string())?;
    }
    bind(&mut args, asked.limit)?;
    bind(&mut args, asked.offset)?;
    let rows = sqlx::query_with(
        db.sql(&format!(
            "SELECT {LISTED} FROM person WHERE {filter} ORDER BY {order} LIMIT ? OFFSET ?"
        )),
        args,
    )
    .fetch_all(db.pool())
    .await?;
    Ok(Indexed {
        people: rows
            .iter()
            .map(|row| listed(row, asked.job))
            .collect::<Result<_>>()?,
        total,
        letters,
    })
}

/// Every record, credited or not, as whoever keeps the catalogue lists them:
/// with how many works anybody may see credit them, what they did, and the
/// other names they go by. By name.
pub async fn everybody(db: &Db) -> Result<Vec<Listed>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {LISTED} FROM person WHERE merged_into IS NULL ORDER BY sort_key, id"
    )))
    .fetch_all(db.pool())
    .await?;
    let mut out = rows
        .iter()
        .map(|row| listed(row, None))
        .collect::<Result<Vec<_>>>()?;

    let mut names: HashMap<String, Vec<String>> = HashMap::new();
    for row in sqlx::query(db.sql(
        "SELECT a.person_id, a.name FROM person_alias a
           JOIN person p ON p.id = a.person_id
          WHERE p.merged_into IS NULL
          ORDER BY a.person_id, a.name",
    ))
    .fetch_all(db.pool())
    .await?
    {
        names
            .entry(row.text("person_id")?)
            .or_default()
            .push(row.text("name")?);
    }
    for person in &mut out {
        if let Some(aliases) = names.remove(&person.id) {
            person.aliases = aliases;
        }
    }
    Ok(out)
}

/// Of these records, those who share a name — their own or one they go by,
/// accents, case and punctuation aside — with another: the likely homonyms
/// and the likely duplicates.
pub fn sharing_a_name(people: &[Listed]) -> HashSet<String> {
    let mut by_name: HashMap<String, BTreeSet<&str>> = HashMap::new();
    for person in people {
        let mut folded: HashSet<String> = HashSet::new();
        for name in std::iter::once(&person.name).chain(&person.aliases) {
            let name = crate::search::norm(name);
            if !name.is_empty() && folded.insert(name.clone()) {
                by_name.entry(name).or_default().insert(&person.id);
            }
        }
    }
    by_name
        .into_values()
        .filter(|ids| ids.len() > 1)
        .flatten()
        .map(str::to_string)
        .collect()
}

/// How the index counts somebody, as `repo::search` works it out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counted {
    pub sort_key: String,
    pub letter: char,
    pub works: i64,
    pub acting: i64,
    pub directing: i64,
    pub writing: i64,
    pub producing: i64,
}

/// The counts of these records, as they stand and as their credits make
/// them: the works anybody may see crediting them by a credit no lock
/// hides, in all and by job, and the name they sort by. Written where they
/// moved; read and written in `tx`.
pub async fn recount(db: &Db, tx: &mut Transaction<'_, Any>, ids: &[&String]) -> Result<()> {
    let marks = holes(ids.len());
    let args = || -> Result<AnyArguments> {
        let mut args = AnyArguments::default();
        for id in ids {
            bind(&mut args, (*id).clone())?;
        }
        Ok(args)
    };

    let mut standing: BTreeMap<String, (Counted, Counted)> = BTreeMap::new();
    for row in sqlx::query_with(
        db.sql(&format!(
            "SELECT id, name, sort_name, sort_key, letter, works, acting, directing, writing,
                    producing
               FROM person WHERE id IN ({marks}) AND merged_into IS NULL ORDER BY id"
        )),
        args()?,
    )
    .fetch_all(&mut **tx)
    .await?
    {
        let filed = row
            .opt_text("sort_name")?
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(row.text("name")?);
        let was = Counted {
            sort_key: row.text("sort_key")?,
            letter: row.text("letter")?.chars().next().unwrap_or('#'),
            works: row.big("works")?,
            acting: row.big("acting")?,
            directing: row.big("directing")?,
            writing: row.big("writing")?,
            producing: row.big("producing")?,
        };
        let now = Counted {
            sort_key: crate::search::norm(&filed),
            letter: letter_of(&filed),
            ..Counted::default()
        };
        standing.insert(row.text("id")?, (was, now));
    }

    for row in sqlx::query_with(
        db.sql(&format!(
            "SELECT c.person_id,
                    COUNT(DISTINCT c.media_id) AS works,
                    COUNT(DISTINCT CASE WHEN c.credit_type IN ('actor', 'guest')
                                        THEN c.media_id END) AS acting,
                    COUNT(DISTINCT CASE WHEN c.credit_type = 'director' THEN c.media_id END)
                        AS directing,
                    COUNT(DISTINCT CASE WHEN c.credit_type = 'writer' THEN c.media_id END)
                        AS writing,
                    COUNT(DISTINCT CASE WHEN c.credit_type = 'producer' THEN c.media_id END)
                        AS producing
               FROM media_credit c JOIN media_item m ON m.id = c.media_id
              WHERE c.person_id IN ({marks}) AND m.is_enabled = 1 AND m.is_adult = 0
                AND NOT {HIDDEN}
              GROUP BY c.person_id"
        )),
        args()?,
    )
    .fetch_all(&mut **tx)
    .await?
    {
        if let Some((_, now)) = standing.get_mut(&row.text("person_id")?) {
            now.works = row.big("works")?;
            now.acting = row.big("acting")?;
            now.directing = row.big("directing")?;
            now.writing = row.big("writing")?;
            now.producing = row.big("producing")?;
        }
    }

    for (id, (was, now)) in standing {
        if was == now {
            continue;
        }
        sqlx::query(db.sql(
            "UPDATE person SET sort_key = ?, letter = ?, works = ?, acting = ?, directing = ?,
                    writing = ?, producing = ?
              WHERE id = ?",
        ))
        .bind(&now.sort_key)
        .bind(now.letter.to_string())
        .bind(now.works)
        .bind(now.acting)
        .bind(now.directing)
        .bind(now.writing)
        .bind(now.producing)
        .bind(&id)
        .execute(&mut **tx)
        .await
        .context("failed to count somebody's works")?;
    }
    Ok(())
}

// ─── entered and put right by hand ───────────────────────────────────────────

/// What somebody is entered or put right with, by hand.
#[derive(Clone, Debug, Default)]
pub struct Entered {
    pub name: String,
    pub tmdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub tvdb_id: Option<i64>,
    pub sort_name: Option<String>,
    pub original_name: Option<String>,
    pub gender: Option<u8>,
    pub birthday: Option<String>,
    pub deathday: Option<String>,
    pub place_of_birth: Option<String>,
    pub biography: Option<String>,
    pub image: Option<String>,
    /// The other names they go by, typed by hand; those any other way are
    /// kept.
    pub aliases: Vec<String>,
}

/// Why an entry was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Somebody else holds the TMDB id, by this record.
    TmdbHeld(String),
    /// No such record, or one merged into another.
    Gone,
    /// The two records are one.
    Same,
    /// A credit named is not filed under the record it is taken from.
    NotTheirs(String),
}

/// Who holds a TMDB id, but `except`.
async fn tmdb_holder(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    tmdb_id: i64,
    except: Option<&str>,
) -> Result<Option<String>> {
    let holder: Option<String> =
        sqlx::query_scalar(db.sql("SELECT id FROM person WHERE tmdb_id = ?"))
            .bind(tmdb_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(holder.filter(|h| Some(h.as_str()) != except))
}

async fn write_manual_aliases(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    id: &str,
    name: &str,
    aliases: &[String],
) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM person_alias WHERE person_id = ? AND source = 'manual'"))
        .bind(id)
        .execute(&mut **tx)
        .await?;
    for alias in aliases {
        let alias = alias.trim();
        if alias.is_empty() || alias == name.trim() {
            continue;
        }
        add_alias(db, tx, id, alias, "manual").await?;
    }
    Ok(())
}

/// Enter somebody by hand: a record of its own, whatever TMDB says of them
/// later. Its id, or why it was refused.
pub async fn create(db: &Db, entered: &Entered) -> Result<Result<String, Refused>> {
    let mut tx = db.begin_write().await?;
    if let Some(tmdb) = entered.tmdb_id
        && let Some(holder) = tmdb_holder(db, &mut tx, tmdb, None).await?
    {
        return Ok(Err(Refused::TmdbHeld(holder)));
    }
    let id = new_id();
    let at = now();
    sqlx::query(db.sql(
        "INSERT INTO person
             (id, tmdb_id, imdb_id, tvdb_id, name, sort_name, original_name, gender, birthday,
              deathday, place_of_birth, biography, image, is_manual, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
    ))
    .bind(&id)
    .bind(entered.tmdb_id)
    .bind(&entered.imdb_id)
    .bind(entered.tvdb_id)
    .bind(entered.name.trim())
    .bind(&entered.sort_name)
    .bind(&entered.original_name)
    .bind(entered.gender.map(i32::from))
    .bind(&entered.birthday)
    .bind(&entered.deathday)
    .bind(&entered.place_of_birth)
    .bind(&entered.biography)
    .bind(&entered.image)
    .bind(&at)
    .bind(&at)
    .execute(&mut *tx)
    .await
    .context("failed to write a person entered by hand")?;
    write_manual_aliases(db, &mut tx, &id, &entered.name, &entered.aliases).await?;
    tx.commit().await?;
    Ok(Ok(id))
}

/// Put somebody right by hand: every field as given, the record marked as
/// put right, so that what TMDB says fills only what it leaves empty.
pub async fn update(db: &Db, id: &str, entered: &Entered) -> Result<Result<(), Refused>> {
    let mut tx = db.begin_write().await?;
    let live: Option<String> =
        sqlx::query_scalar(db.sql("SELECT id FROM person WHERE id = ? AND merged_into IS NULL"))
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    if live.is_none() {
        return Ok(Err(Refused::Gone));
    }
    if let Some(tmdb) = entered.tmdb_id
        && let Some(holder) = tmdb_holder(db, &mut tx, tmdb, Some(id)).await?
    {
        return Ok(Err(Refused::TmdbHeld(holder)));
    }
    sqlx::query(db.sql(
        "UPDATE person SET tmdb_id = ?, imdb_id = ?, tvdb_id = ?, name = ?, sort_name = ?,
                original_name = ?, gender = ?, birthday = ?, deathday = ?, place_of_birth = ?,
                biography = ?, image = ?, is_manual = 1, updated_at = ?
          WHERE id = ?",
    ))
    .bind(entered.tmdb_id)
    .bind(&entered.imdb_id)
    .bind(entered.tvdb_id)
    .bind(entered.name.trim())
    .bind(&entered.sort_name)
    .bind(&entered.original_name)
    .bind(entered.gender.map(i32::from))
    .bind(&entered.birthday)
    .bind(&entered.deathday)
    .bind(&entered.place_of_birth)
    .bind(&entered.biography)
    .bind(&entered.image)
    .bind(now())
    .bind(id)
    .execute(&mut *tx)
    .await
    .context("failed to put a person right")?;
    write_manual_aliases(db, &mut tx, id, &entered.name, &entered.aliases).await?;
    tx.commit().await?;
    Ok(Ok(()))
}

/// A record that was not merged into another, read in `tx`.
async fn live(db: &Db, tx: &mut Transaction<'_, Any>, id: &str) -> Result<Option<Person>> {
    let row = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM person WHERE id = ? AND merged_into IS NULL"
    )))
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| map(&row)).transpose()
}

/// The works whose credits name these records.
async fn works_of(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    people: &[&str],
    credits: Option<&[String]>,
) -> Result<Vec<String>> {
    let mut args = AnyArguments::default();
    for id in people {
        bind(&mut args, (*id).to_string())?;
    }
    let mut sql = format!(
        "SELECT DISTINCT media_id FROM media_credit WHERE person_id IN ({})",
        holes(people.len())
    );
    if let Some(credits) = credits {
        sql.push_str(&format!(" AND id IN ({})", holes(credits.len())));
        for id in credits {
            bind(&mut args, id.clone())?;
        }
    }
    let rows = sqlx::query_with(db.sql(&sql), args)
        .fetch_all(&mut **tx)
        .await?;
    rows.iter().map(|row| Ok(row.text("media_id")?)).collect()
}

/// Merge one record into another: every credit filed under it re-filed under
/// the other, which goes by its names from then on and takes whatever ids
/// and facts it lacks; the merged record is kept as a pointer to the other,
/// so that its ids and its address lead there. The works whose credits moved,
/// or why it was refused.
pub async fn merge(db: &Db, from: &str, into: &str) -> Result<Result<Vec<String>, Refused>> {
    if from == into {
        return Ok(Err(Refused::Same));
    }
    let mut tx = db.begin_write().await?;
    let (Some(merged), Some(kept)) = (
        live(db, &mut tx, from).await?,
        live(db, &mut tx, into).await?,
    ) else {
        return Ok(Err(Refused::Gone));
    };

    let works = works_of(db, &mut tx, &[from], None).await?;
    sqlx::query(db.sql("UPDATE media_credit SET person_id = ? WHERE person_id = ?"))
        .bind(into)
        .bind(from)
        .execute(&mut *tx)
        .await
        .context("failed to file a merged person's credits again")?;

    // Its names, as names the other goes by.
    let mut names: Vec<String> = vec![merged.name.clone()];
    names.extend(merged.original_name.clone());
    for row in sqlx::query(db.sql("SELECT name FROM person_alias WHERE person_id = ?"))
        .bind(from)
        .fetch_all(&mut *tx)
        .await?
    {
        names.push(row.text("name")?);
    }
    for name in names {
        if name.trim() != kept.name && Some(name.trim()) != kept.original_name.as_deref() {
            add_alias(db, &mut tx, into, &name, "merged").await?;
        }
    }

    // Its TMDB id, where the other has none: moved, since an id is one
    // record's.
    if kept.tmdb_id.is_none() && merged.tmdb_id.is_some() {
        sqlx::query(db.sql("UPDATE person SET tmdb_id = NULL WHERE id = ?"))
            .bind(from)
            .execute(&mut *tx)
            .await?;
        sqlx::query(db.sql("UPDATE person SET tmdb_id = ? WHERE id = ?"))
            .bind(merged.tmdb_id)
            .bind(into)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(db.sql(
        "UPDATE person SET
             imdb_id = COALESCE(imdb_id, ?), tvdb_id = COALESCE(tvdb_id, ?),
             original_name = COALESCE(original_name, ?), gender = COALESCE(gender, ?),
             birthday = COALESCE(birthday, ?), deathday = COALESCE(deathday, ?),
             place_of_birth = COALESCE(place_of_birth, ?), biography = COALESCE(biography, ?),
             image = COALESCE(image, ?), updated_at = ?
         WHERE id = ?",
    ))
    .bind(&merged.imdb_id)
    .bind(merged.tvdb_id)
    .bind(&merged.original_name)
    .bind(merged.gender.map(i32::from))
    .bind(&merged.birthday)
    .bind(&merged.deathday)
    .bind(&merged.place_of_birth)
    .bind(&merged.biography)
    .bind(&merged.image)
    .bind(now())
    .bind(into)
    .execute(&mut *tx)
    .await
    .context("failed to complete the person merged into")?;

    // What was merged into it before goes where it goes; then it does.
    sqlx::query(db.sql("UPDATE person SET merged_into = ? WHERE merged_into = ?"))
        .bind(into)
        .bind(from)
        .execute(&mut *tx)
        .await?;
    sqlx::query(db.sql("UPDATE person SET merged_into = ?, updated_at = ? WHERE id = ?"))
        .bind(into)
        .bind(now())
        .bind(from)
        .execute(&mut *tx)
        .await?;
    sqlx::query(db.sql("DELETE FROM person_alias WHERE person_id = ?"))
        .bind(from)
        .execute(&mut *tx)
        .await?;

    tx.commit()
        .await
        .context("failed to commit a merge of people")?;
    Ok(Ok(works))
}

/// Move these credits to a record of their own, made by hand under `name` —
/// the person's own when none is given: how a homonym is told apart from
/// the person it was taken for. The new record's id and the works whose
/// credits moved, or why it was refused.
pub async fn split(
    db: &Db,
    from: &str,
    credits: &[String],
    name: Option<&str>,
) -> Result<Result<(String, Vec<String>), Refused>> {
    let mut tx = db.begin_write().await?;
    let person: Option<String> =
        sqlx::query_scalar(db.sql("SELECT name FROM person WHERE id = ? AND merged_into IS NULL"))
            .bind(from)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(own) = person else {
        return Ok(Err(Refused::Gone));
    };
    if let Some(stray) = not_theirs(db, &mut tx, from, credits).await? {
        return Ok(Err(Refused::NotTheirs(stray)));
    }
    let works = works_of(db, &mut tx, &[from], Some(credits)).await?;

    let name = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(&own);
    let mut person = Person::named(name);
    person.is_manual = true;
    insert(db, &mut tx, std::slice::from_ref(&person)).await?;
    move_credits(db, &mut tx, &person.id, credits).await?;
    tx.commit().await.context("failed to commit a split")?;
    Ok(Ok((person.id, works)))
}

/// File these credits under a record, whomever they were filed under: the
/// way a credit by name alone is given the person it is of. The records they
/// were filed under and the works whose credits moved, or why it was refused.
pub async fn attach(
    db: &Db,
    to: &str,
    credits: &[String],
) -> Result<Result<(Vec<String>, Vec<String>), Refused>> {
    let mut tx = db.begin_write().await?;
    let live: Option<String> =
        sqlx::query_scalar(db.sql("SELECT id FROM person WHERE id = ? AND merged_into IS NULL"))
            .bind(to)
            .fetch_optional(&mut *tx)
            .await?;
    if live.is_none() {
        return Ok(Err(Refused::Gone));
    }
    let mut was: BTreeSet<String> = BTreeSet::new();
    let mut works: BTreeSet<String> = BTreeSet::new();
    for chunk in credits.chunks(CHUNK) {
        let mut args = AnyArguments::default();
        for id in chunk {
            bind(&mut args, id.clone())?;
        }
        let sql = format!(
            "SELECT id, media_id, person_id FROM media_credit WHERE id IN ({})",
            holes(chunk.len())
        );
        let rows = sqlx::query_with(db.sql(&sql), args)
            .fetch_all(&mut *tx)
            .await?;
        for id in chunk {
            let Some(row) = rows
                .iter()
                .find(|row| row.text("id").ok().as_deref() == Some(id))
            else {
                return Ok(Err(Refused::NotTheirs(id.clone())));
            };
            works.insert(row.text("media_id")?);
            if let Some(person) = row.opt_text("person_id")? {
                was.insert(person);
            }
        }
    }
    move_credits(db, &mut tx, to, credits).await?;
    tx.commit()
        .await
        .context("failed to commit credits filed again")?;
    Ok(Ok((was.into_iter().collect(), works.into_iter().collect())))
}

/// The first of these credits that is not filed under `person`.
async fn not_theirs(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    person: &str,
    credits: &[String],
) -> Result<Option<String>> {
    if credits.is_empty() {
        return Ok(Some(String::new()));
    }
    let mut theirs: HashSet<String> = HashSet::new();
    for chunk in credits.chunks(CHUNK) {
        let mut args = AnyArguments::default();
        bind(&mut args, person.to_string())?;
        for id in chunk {
            bind(&mut args, id.clone())?;
        }
        let sql = format!(
            "SELECT id FROM media_credit WHERE person_id = ? AND id IN ({})",
            holes(chunk.len())
        );
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(&mut **tx)
            .await?
        {
            theirs.insert(row.text("id")?);
        }
    }
    Ok(credits.iter().find(|id| !theirs.contains(*id)).cloned())
}

async fn move_credits(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    to: &str,
    credits: &[String],
) -> Result<()> {
    for chunk in credits.chunks(CHUNK) {
        let mut args = AnyArguments::default();
        bind(&mut args, to.to_string())?;
        for id in chunk {
            bind(&mut args, id.clone())?;
        }
        let sql = format!(
            "UPDATE media_credit SET person_id = ? WHERE id IN ({})",
            holes(chunk.len())
        );
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut **tx)
            .await
            .context("failed to file credits under a person")?;
    }
    Ok(())
}

// ─── what TMDB says of them ──────────────────────────────────────────────────

/// What TMDB says of somebody, as their record keeps it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    pub name: Option<String>,
    pub imdb_id: Option<String>,
    pub gender: Option<u8>,
    pub birthday: Option<String>,
    pub deathday: Option<String>,
    pub place_of_birth: Option<String>,
    pub biography: Option<String>,
    pub image: Option<String>,
    pub photos: Vec<String>,
    pub known_for: Option<String>,
    pub homepage: Option<String>,
    pub popularity: Option<f64>,
    /// The other names TMDB lists them under.
    pub also_known_as: Vec<String>,
    /// Their name in their own script, where one of those is.
    pub original_name: Option<String>,
}

/// Keep what TMDB says of somebody, and when it was asked. On a record put
/// right by hand, only what it has none of; on any other, all of it — a new
/// name with the old one kept among those they go by. TMDB's other names
/// replace those it gave before.
pub async fn write_details(db: &Db, id: &str, details: &Details) -> Result<()> {
    let mut tx = db.begin_write().await?;
    let row = sqlx::query(
        db.sql("SELECT name, is_manual FROM person WHERE id = ? AND merged_into IS NULL"),
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(());
    };
    let name = row.text("name")?;
    let manual = row.flag("is_manual")?;
    let photos = crate::db::text_list(&details.photos);

    if manual {
        sqlx::query(db.sql(
            "UPDATE person SET
                 imdb_id = COALESCE(imdb_id, ?), gender = COALESCE(gender, ?),
                 birthday = COALESCE(birthday, ?), deathday = COALESCE(deathday, ?),
                 place_of_birth = COALESCE(place_of_birth, ?),
                 biography = COALESCE(biography, ?), image = COALESCE(image, ?),
                 original_name = COALESCE(original_name, ?),
                 photos = ?, known_for = ?, homepage = ?, popularity = ?,
                 refreshed_at = ?, updated_at = ?
             WHERE id = ?",
        ))
        .bind(&details.imdb_id)
        .bind(details.gender.map(i32::from))
        .bind(&details.birthday)
        .bind(&details.deathday)
        .bind(&details.place_of_birth)
        .bind(&details.biography)
        .bind(&details.image)
        .bind(&details.original_name)
        .bind(&photos)
        .bind(&details.known_for)
        .bind(&details.homepage)
        .bind(details.popularity)
        .bind(now())
        .bind(now())
        .bind(id)
        .execute(&mut *tx)
        .await
        .context("failed to keep what TMDB says of somebody")?;
    } else {
        let renamed = details
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty() && *n != name);
        if renamed.is_some() {
            add_alias(db, &mut tx, id, &name, "credit").await?;
            sqlx::query(db.sql("DELETE FROM person_alias WHERE person_id = ? AND name = ?"))
                .bind(id)
                .bind(renamed)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(db.sql(
            "UPDATE person SET
                 name = ?, imdb_id = COALESCE(?, imdb_id), gender = COALESCE(?, gender),
                 birthday = ?, deathday = ?, place_of_birth = ?, biography = ?,
                 image = COALESCE(?, image), original_name = COALESCE(original_name, ?),
                 photos = ?, known_for = ?, homepage = ?, popularity = ?,
                 refreshed_at = ?, updated_at = ?
             WHERE id = ?",
        ))
        .bind(renamed.unwrap_or(&name))
        .bind(&details.imdb_id)
        .bind(details.gender.map(i32::from))
        .bind(&details.birthday)
        .bind(&details.deathday)
        .bind(&details.place_of_birth)
        .bind(&details.biography)
        .bind(&details.image)
        .bind(&details.original_name)
        .bind(&photos)
        .bind(&details.known_for)
        .bind(&details.homepage)
        .bind(details.popularity)
        .bind(now())
        .bind(now())
        .bind(id)
        .execute(&mut *tx)
        .await
        .context("failed to keep what TMDB says of somebody")?;
    }

    sqlx::query(db.sql("DELETE FROM person_alias WHERE person_id = ? AND source = 'tmdb'"))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let own = details.name.as_deref().map(str::trim).unwrap_or(&name);
    for alias in &details.also_known_as {
        if alias.trim() != own {
            add_alias(db, &mut tx, id, alias, "tmdb").await?;
        }
    }
    tx.commit()
        .await
        .context("failed to commit what TMDB says of somebody")?;
    Ok(())
}

#[cfg(test)]
mod tests;
