//! Two records of one work: told apart from the rest, folded into one, and
//! taken apart again.
//!
//! **Found.** Two works of one kind filed under one identifier elsewhere —
//! TMDB's, TheTVDB's, IMDb's, AniList's, MyAnimeList's, Fankai's — are one
//! work (`certain`). The store keeps an identifier on one work at a time, so
//! "filed under" is held, or last said to be the work's: by its journal,
//! which writes what each refresh found its identifiers to be — a refresh
//! that takes an id from another work leaves the taker alone holding it, and
//! the other's journal still says it — or by a lock on them. Two works of one
//! kind under one title, as the search index folds it, a year apart at the
//! most or with a year unknown, may be one (`probable`); a film and a series
//! under one IMDb id are said (`flag`), and never merged. What is found is
//! written to `media_duplicate`, which the catalogue's `missing=duplicates`
//! lists by; a pair somebody said is not one stays there, dismissed, and is
//! not proposed again.
//!
//! **Folded.** One transaction moves what the merged work held onto the
//! survivor, table by table. A row the survivor holds the same of — a season
//! of the same number, an episode at the same place, a picture at the same
//! address, a translation into the same language, a credit of the same
//! person in the same part, an order under the same key, a selection it is
//! in already — stands, and the merged work's gives way, kept whole in the
//! merge's record; every other row moves over as it is. Two season 1s are the
//! survivor's season 1, and the merged work's season 2, of which the survivor
//! has none, becomes the survivor's. Its locks move over where the survivor
//! locks nothing of the same; where both lock a field, the choice made for it
//! (`keep`) decides, and the survivor's lock stands without one; a field kept
//! from the merged work that it did not lock is locked on the survivor to the
//! value it was served with. Its identifiers become the survivor's where the
//! survivor has none of their source, and each of them, its id and its slug
//! are filed as aliases of the survivor (`repo::alias`). The merged work
//! stays, a tombstone switched off and never refreshed, the record beside it.
//!
//! **Taken apart.** Within `merge.undoDays`, the record puts the tombstone
//! back: what moved over and is still as the merge left it moves back, what
//! gave way is put back, the aliases go. What changed on the survivor since —
//! a lock set again, a refresh that replaced what its sources give — stays
//! the survivor's; the work put back is due a refresh, which gives it its own
//! again.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{
    Any, Arguments as _, Column as _, Row as _, Transaction, ValueRef as _, any::AnyArguments,
};
use utoipa::ToSchema;

use crate::{
    db::{Db, Dialect, RowExt, from_bool, new_id, now},
    domain::{ExternalIds, ExternalSource, MediaKind},
};

// ─── found ──────────────────────────────────────────────────────────────────

/// How sure the server is that two works are one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = DuplicateConfidence)]
pub enum Confidence {
    /// Both of one kind, and filed under one identifier elsewhere.
    Certain,
    /// Both of one kind, under one title, a year apart at the most.
    Probable,
    /// A film and a series under one IMDb id: one of them is filed wrongly.
    /// Said, and never merged.
    Flag,
}

impl Confidence {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Certain => "certain",
            Self::Probable => "probable",
            Self::Flag => "flag",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "certain" => Self::Certain,
            "flag" => Self::Flag,
            _ => Self::Probable,
        }
    }

    /// Which of two findings of one pair is kept: the surer.
    const fn weight(self) -> u8 {
        match self {
            Self::Certain => 3,
            Self::Probable => 2,
            Self::Flag => 1,
        }
    }
}

/// Why two works look like one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = DuplicateReason)]
pub enum DuplicateReason {
    /// Filed under one identifier elsewhere.
    SharedId,
    /// One title, within a year.
    Title,
    /// One IMDb id, on a film and a series.
    ImdbAcrossKinds,
}

impl DuplicateReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SharedId => "sharedId",
            Self::Title => "title",
            Self::ImdbAcrossKinds => "imdbAcrossKinds",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "sharedId" => Self::SharedId,
            "imdbAcrossKinds" => Self::ImdbAcrossKinds,
            _ => Self::Title,
        }
    }
}

/// Two works that look like one, as the duplicates were last looked for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = DuplicatePair)]
pub struct Duplicate {
    /// The one of the two whose id sorts first.
    pub first_id: String,
    pub second_id: String,
    pub confidence: Confidence,
    pub reason: DuplicateReason,
    /// What they share: an identifier as `imdb:tt0903747`, `tmdb:603`; the
    /// title as it was folded.
    #[schema(required)]
    pub detail: Option<String>,
    /// When the pair was first found.
    pub found_at: String,
    /// When somebody said it is not one work; absent while it is proposed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dismissed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dismissed_by: Option<String>,
}

/// A pair as the duplicates are looked for, before it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundPair {
    pub first: String,
    pub second: String,
    pub confidence: Confidence,
    pub reason: DuplicateReason,
    pub detail: String,
}

impl FoundPair {
    fn of(
        a: &str,
        b: &str,
        confidence: Confidence,
        reason: DuplicateReason,
        detail: String,
    ) -> Self {
        let (first, second) = pair_order(a, b);
        Self {
            first: first.to_string(),
            second: second.to_string(),
            confidence,
            reason,
            detail,
        }
    }
}

/// Two ids in the order a pair is filed in.
pub fn pair_order<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if a <= b { (a, b) } else { (b, a) }
}

/// The identifiers two works are one work by, each named as a reader names
/// its source: TMDB's and TheTVDB's for a film or a series alike — the store
/// files them apart — IMDb's, AniList's, MyAnimeList's and Fankai's.
fn family(source: ExternalSource) -> Option<&'static str> {
    match source {
        ExternalSource::TmdbMovie | ExternalSource::TmdbTv => Some("tmdb"),
        ExternalSource::TvdbSeries | ExternalSource::TvdbMovie => Some("tvdb"),
        ExternalSource::Imdb => Some("imdb"),
        ExternalSource::AniList => Some("anilist"),
        ExternalSource::Mal => Some("mal"),
        ExternalSource::Fankai => Some("fankai"),
        _ => None,
    }
}

/// A work of its own, as the duplicates are looked for among.
struct Candidate {
    id: String,
    kind: MediaKind,
    /// The title it is shown by: a lock's in every language, else its own.
    title: String,
    year: Option<i32>,
}

/// Whether two works' years let them be one: a year apart at the most, or
/// one of them not known — a work entered by hand often has none.
fn years_meet(a: Option<i32>, b: Option<i32>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= 1,
        _ => true,
    }
}

/// Every pair of works that look like one, the surer finding of each pair.
/// Works merged into another are not among them.
pub async fn find_duplicates(db: &Db) -> Result<Vec<FoundPair>> {
    let rows = sqlx::query(db.sql(
        "SELECT id, kind, title, year, listed_year FROM media_item WHERE merged_into IS NULL",
    ))
    .fetch_all(db.pool())
    .await?;
    let mut works: HashMap<String, Candidate> = HashMap::with_capacity(rows.len());
    for row in &rows {
        let Ok(kind) = row.text("kind")?.parse::<MediaKind>() else {
            continue;
        };
        let id = row.text("id")?;
        works.insert(
            id.clone(),
            Candidate {
                id,
                kind,
                title: row.text("title")?,
                year: row.opt_int("listed_year")?.or(row.opt_int("year")?),
            },
        );
    }

    // A title locked in every language is the one the work is shown by.
    let locked = sqlx::query(db.sql(
        "SELECT media_id, value FROM media_override
          WHERE scope = 'item' AND field = 'title' AND language = ''",
    ))
    .fetch_all(db.pool())
    .await?;
    for row in &locked {
        let title = row
            .opt_text("value")?
            .and_then(|value| serde_json::from_str::<String>(&value).ok())
            .filter(|title| !title.trim().is_empty());
        if let (Some(work), Some(title)) = (works.get_mut(&row.text("media_id")?), title) {
            work.title = title;
        }
    }

    let mut found: BTreeMap<(String, String), FoundPair> = BTreeMap::new();
    let mut offer = |pair: FoundPair| {
        let key = (pair.first.clone(), pair.second.clone());
        match found.get(&key) {
            Some(held) if held.confidence.weight() >= pair.confidence.weight() => {}
            _ => {
                found.insert(key, pair);
            }
        }
    };

    // One title, folded as the search index folds it, a year apart at most.
    let mut by_title: HashMap<(MediaKind, String), Vec<&Candidate>> = HashMap::new();
    for work in works.values() {
        let folded = crate::search::norm(&work.title);
        if !folded.is_empty() {
            by_title.entry((work.kind, folded)).or_default().push(work);
        }
    }
    for ((_, folded), group) in &by_title {
        for (at, a) in group.iter().enumerate() {
            for b in &group[at + 1..] {
                if years_meet(a.year, b.year) {
                    offer(FoundPair::of(
                        &a.id,
                        &b.id,
                        Confidence::Probable,
                        DuplicateReason::Title,
                        folded.clone(),
                    ));
                }
            }
        }
    }

    // One identifier elsewhere, held or said.
    for ((source, value), claimants) in claimed_ids(db, &works).await? {
        let Ok(parsed) = source.parse::<ExternalSource>() else {
            continue;
        };
        let Some(family) = family(parsed) else {
            continue;
        };
        let claimants: Vec<&Candidate> = claimants.iter().filter_map(|id| works.get(id)).collect();
        for (at, a) in claimants.iter().enumerate() {
            for b in &claimants[at + 1..] {
                let detail = format!("{family}:{value}");
                if a.kind == b.kind {
                    offer(FoundPair::of(
                        &a.id,
                        &b.id,
                        Confidence::Certain,
                        DuplicateReason::SharedId,
                        detail,
                    ));
                } else if parsed == ExternalSource::Imdb {
                    offer(FoundPair::of(
                        &a.id,
                        &b.id,
                        Confidence::Flag,
                        DuplicateReason::ImdbAcrossKinds,
                        detail,
                    ));
                }
            }
        }
    }

    Ok(found.into_values().collect())
}

/// Each identifier some work is filed under or said to have, by its source
/// as the store files it and its value, with every work of its own that is:
/// the one holding it, those whose journal last said it was theirs, those
/// that lock it.
async fn claimed_ids(
    db: &Db,
    works: &HashMap<String, Candidate>,
) -> Result<BTreeMap<(String, String), BTreeSet<String>>> {
    let mut said: HashMap<String, Vec<String>> = HashMap::new();

    // What each work's journal last said its identifiers were: the rows of a
    // work in the order they were written, the last one standing.
    let rows = sqlx::query(db.sql(
        "SELECT media_id, value_after FROM media_change
          WHERE scope = 'item' AND field = 'externalIds'
          ORDER BY media_id, at, id",
    ))
    .fetch_all(db.pool())
    .await?;
    let mut last: HashMap<String, Option<String>> = HashMap::new();
    for row in &rows {
        last.insert(row.text("media_id")?, row.opt_text("value_after")?);
    }
    for (id, value) in last {
        said.entry(id).or_default().extend(value);
    }

    // What a lock says they are.
    let rows = sqlx::query(db.sql(
        "SELECT media_id, value FROM media_override
          WHERE scope = 'item' AND field = 'externalIds' AND language = ''",
    ))
    .fetch_all(db.pool())
    .await?;
    for row in &rows {
        if let Some(value) = row.opt_text("value")? {
            said.entry(row.text("media_id")?).or_default().push(value);
        }
    }

    let mut claims: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for (id, values) in said {
        let Some(work) = works.get(&id) else {
            continue;
        };
        for text in values {
            let Ok(ids) = serde_json::from_str::<ExternalIds>(&text) else {
                continue;
            };
            for (source, value) in ids.rows(work.kind) {
                if family(source).is_some() {
                    claims
                        .entry((source.as_str().to_string(), value))
                        .or_default()
                        .insert(id.clone());
                }
            }
        }
    }

    // Who holds each of them now.
    let mut by_source: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (source, value) in claims.keys() {
        by_source
            .entry(source.clone())
            .or_default()
            .push(value.clone());
    }
    for (source, values) in by_source {
        let Ok(parsed) = source.parse::<ExternalSource>() else {
            continue;
        };
        for (value, holder) in super::held_external_ids(db, parsed, &values).await? {
            if works.contains_key(&holder)
                && let Some(claimants) = claims.get_mut(&(source.clone(), value))
            {
                claimants.insert(holder);
            }
        }
    }

    Ok(claims)
}

/// Look for the duplicates again and write what was found: a pair found
/// anew is proposed, one no longer found is forgotten — unless somebody said
/// it is not one work, which is remembered whatever is found. Tombstones are
/// put straight first (see [`bury_tombstones`]). Says whether a pair was
/// proposed, changed or forgotten: the register narrowed to duplicates is
/// drawn from what is written here.
pub async fn sync_duplicates(db: &Db) -> Result<bool> {
    bury_tombstones(db).await?;
    let found = find_duplicates(db).await?;
    let at = now();

    /// A pair as it is written, set beside what was found.
    struct Written {
        dismissed: bool,
        confidence: String,
        reason: String,
        detail: Option<String>,
    }

    let mut tx = db.begin_write().await?;
    let mut stored: HashMap<(String, String), Written> = HashMap::new();
    for row in sqlx::query(db.sql(
        "SELECT first_id, second_id, confidence, reason, detail, dismissed_at FROM media_duplicate",
    ))
    .fetch_all(&mut *tx)
    .await?
    {
        stored.insert(
            (row.text("first_id")?, row.text("second_id")?),
            Written {
                dismissed: row.opt_text("dismissed_at")?.is_some(),
                confidence: row.text("confidence")?,
                reason: row.text("reason")?,
                detail: row.opt_text("detail")?,
            },
        );
    }

    let mut changed = false;
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for pair in &found {
        let key = (pair.first.clone(), pair.second.clone());
        seen.insert(key.clone());
        let unchanged = stored.get(&key).is_some_and(|written| {
            written.confidence == pair.confidence.as_str()
                && written.reason == pair.reason.as_str()
                && written.detail.as_deref() == Some(pair.detail.as_str())
        });
        if unchanged {
            continue;
        }
        // Both works still there as it is written: one deleted meanwhile is
        // no pair to propose, and its key no row to point at.
        let written = sqlx::query(db.sql(
            "INSERT INTO media_duplicate
                 (first_id, second_id, confidence, reason, detail, found_at)
             SELECT ?, ?, ?, ?, ?, ?
              WHERE EXISTS (SELECT 1 FROM media_item WHERE id = ?)
                AND EXISTS (SELECT 1 FROM media_item WHERE id = ?)
             ON CONFLICT (first_id, second_id) DO UPDATE SET
                 confidence = excluded.confidence,
                 reason = excluded.reason,
                 detail = excluded.detail",
        ))
        .bind(&pair.first)
        .bind(&pair.second)
        .bind(pair.confidence.as_str())
        .bind(pair.reason.as_str())
        .bind(&pair.detail)
        .bind(&at)
        .bind(&pair.first)
        .bind(&pair.second)
        .execute(&mut *tx)
        .await
        .context("failed to write a pair of duplicates")?;
        changed |= written.rows_affected() > 0;
    }

    for ((first, second), written) in &stored {
        if written.dismissed || seen.contains(&(first.clone(), second.clone())) {
            continue;
        }
        let forgotten = sqlx::query(db.sql(
            "DELETE FROM media_duplicate
              WHERE first_id = ? AND second_id = ? AND dismissed_at IS NULL",
        ))
        .bind(first)
        .bind(second)
        .execute(&mut *tx)
        .await?;
        changed |= forgotten.rows_affected() > 0;
    }

    tx.commit()
        .await
        .context("failed to write the duplicates")?;
    Ok(changed)
}

/// The pairs proposed — those somebody dismissed too, when asked — both of
/// their works still works of their own, surest first.
pub async fn duplicates(db: &Db, with_dismissed: bool) -> Result<Vec<Duplicate>> {
    let sql = format!(
        "SELECT d.first_id, d.second_id, d.confidence, d.reason, d.detail, d.found_at,
                d.dismissed_at, d.dismissed_by
           FROM media_duplicate d
           JOIN media_item a ON a.id = d.first_id
           JOIN media_item b ON b.id = d.second_id
          WHERE a.merged_into IS NULL AND b.merged_into IS NULL{}
          ORDER BY d.found_at DESC, d.first_id, d.second_id",
        if with_dismissed {
            ""
        } else {
            " AND d.dismissed_at IS NULL"
        }
    );
    let rows = sqlx::query(db.sql(&sql)).fetch_all(db.pool()).await?;
    let mut pairs: Vec<Duplicate> = rows
        .iter()
        .map(|row| {
            Ok(Duplicate {
                first_id: row.text("first_id")?,
                second_id: row.text("second_id")?,
                confidence: Confidence::parse(&row.text("confidence")?),
                reason: DuplicateReason::parse(&row.text("reason")?),
                detail: row.opt_text("detail")?,
                found_at: row.text("found_at")?,
                dismissed_at: row.opt_text("dismissed_at")?,
                dismissed_by: row.opt_text("dismissed_by")?,
            })
        })
        .collect::<Result<_>>()?;
    pairs.sort_by_key(|pair| std::cmp::Reverse(pair.confidence.weight()));
    Ok(pairs)
}

/// Say two works are not one: the pair is kept, dismissed, and never
/// proposed again — found later or not found at all yet.
pub async fn dismiss_duplicate(db: &Db, a: &str, b: &str, by: Option<&str>) -> Result<()> {
    let mut tx = db.begin_write().await?;
    dismiss_in(db, &mut tx, a, b, by).await?;
    tx.commit().await.context("failed to dismiss a pair")?;
    Ok(())
}

async fn dismiss_in(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    a: &str,
    b: &str,
    by: Option<&str>,
) -> Result<()> {
    let (first, second) = pair_order(a, b);
    let at = now();
    sqlx::query(db.sql(
        "INSERT INTO media_duplicate
             (first_id, second_id, confidence, reason, detail, found_at, dismissed_at, dismissed_by)
         VALUES (?, ?, 'probable', 'title', NULL, ?, ?, ?)
         ON CONFLICT (first_id, second_id) DO UPDATE SET
             dismissed_at = excluded.dismissed_at,
             dismissed_by = excluded.dismissed_by",
    ))
    .bind(first)
    .bind(second)
    .bind(&at)
    .bind(&at)
    .bind(by)
    .execute(&mut **tx)
    .await
    .context("failed to remember a pair as no duplicate")?;
    Ok(())
}

/// Put the tombstones straight: one whose survivor is gone goes too, with
/// what still names it — a survivor deleted takes the works merged into it,
/// and those merged into them — and one a write brought back is switched off
/// again, never refreshed, its identifiers left to its aliases. A refresh
/// that read the work before its merge and wrote it after is how one comes
/// back.
pub async fn bury_tombstones(db: &Db) -> Result<()> {
    let mut tx = db.begin_write().await?;
    for _ in 0..16 {
        let gone = sqlx::query(db.sql(
            "DELETE FROM media_item
              WHERE merged_into IS NOT NULL
                AND NOT EXISTS (SELECT 1 FROM media_item s WHERE s.id = media_item.merged_into)",
        ))
        .execute(&mut *tx)
        .await
        .context("failed to take away the works merged into one that is gone")?;
        if gone.rows_affected() == 0 {
            break;
        }
    }
    sqlx::query(db.sql(
        "UPDATE media_item SET is_enabled = 0, refresh_after = NULL
          WHERE merged_into IS NOT NULL AND (is_enabled = 1 OR refresh_after IS NOT NULL)",
    ))
    .execute(&mut *tx)
    .await?;
    sqlx::query(db.sql(
        "DELETE FROM media_external_id
          WHERE media_id IN (SELECT id FROM media_item WHERE merged_into IS NOT NULL)",
    ))
    .execute(&mut *tx)
    .await?;
    tx.commit()
        .await
        .context("failed to put the tombstones straight")?;
    Ok(())
}

// ─── what a work is, beside a merge ─────────────────────────────────────────

/// A work merged into another, as its row says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    pub into: String,
    pub at: String,
}

/// The merge a work went into, if it did; `None` for a work of its own, and
/// for no work at all.
pub async fn tombstone(db: &Db, id: &str) -> Result<Option<Tombstone>> {
    let row = sqlx::query(db.sql(
        "SELECT merged_into, merged_at FROM media_item WHERE id = ? AND merged_into IS NOT NULL",
    ))
    .bind(id)
    .fetch_optional(db.pool())
    .await?;
    Ok(match row {
        Some(row) => Some(Tombstone {
            into: row.text("merged_into")?,
            at: row
                .opt_text("merged_at")?
                .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".into()),
        }),
        None => None,
    })
}

/// A work merged into another, as the survivor's editor names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MergedWork {
    /// The tombstone's id: what the undo is asked by.
    pub id: String,
    pub title: String,
    pub kind: MediaKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    pub merged_at: String,
}

/// The works merged into `into` at `since` or after, newest first.
pub async fn merged_into(db: &Db, into: &str, since: &str) -> Result<Vec<MergedWork>> {
    let rows = sqlx::query(db.sql(
        "SELECT id, title, kind, year, merged_at FROM media_item
          WHERE merged_into = ? AND merged_at >= ?
          ORDER BY merged_at DESC, id",
    ))
    .bind(into)
    .bind(since)
    .fetch_all(db.pool())
    .await?;
    rows.iter()
        .map(|row| {
            Ok(MergedWork {
                id: row.text("id")?,
                title: row.text("title")?,
                kind: row.text("kind")?.parse()?,
                year: row.opt_int("year")?,
                merged_at: row.text("merged_at")?,
            })
        })
        .collect()
}

/// A work as a pair of duplicates or a merge names it: enough to tell the
/// two apart at a glance, and to choose which of them stays.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = MergeWork)]
pub struct WorkCard {
    pub id: String,
    /// As it is shown: a title locked in every language, else its own.
    pub title: String,
    pub kind: MediaKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    pub slug: String,
    /// The address of its first poster, the work's own; absent when it has
    /// none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    pub external_ids: ExternalIds,
    pub is_manual: bool,
    pub is_enabled: bool,
    pub is_adult: bool,
    pub created_at: String,
    /// Its locks and what was added to it by hand, counted, and one more
    /// for a work entered by hand: what the survivor is chosen by once
    /// whether its sources fill it is weighed.
    pub hand_edits: i64,
}

/// The works these ids name, as cards, by id: one statement a table for
/// them all. An id no work holds is left out.
pub async fn cards(db: &Db, ids: &[String]) -> Result<HashMap<String, WorkCard>> {
    let mut found: HashMap<String, WorkCard> = HashMap::new();
    for chunk in ids.chunks(200) {
        let holes = vec!["?"; chunk.len()].join(", ");
        let read = |sql: String| {
            let mut query = sqlx::query(db.sql(&sql));
            for id in chunk {
                query = query.bind(id);
            }
            query
        };

        for row in read(format!(
            "SELECT id, title, kind, year, listed_year, slug, is_manual, is_enabled, is_adult,
                    created_at
               FROM media_item WHERE id IN ({holes})"
        ))
        .fetch_all(db.pool())
        .await?
        {
            let id = row.text("id")?;
            found.insert(
                id.clone(),
                WorkCard {
                    id,
                    title: row.text("title")?,
                    kind: row.text("kind")?.parse()?,
                    year: row.opt_int("listed_year")?.or(row.opt_int("year")?),
                    slug: row.text("slug")?,
                    poster: None,
                    external_ids: ExternalIds::default(),
                    is_manual: row.flag("is_manual")?,
                    is_enabled: row.flag("is_enabled")?,
                    is_adult: row.flag("is_adult")?,
                    created_at: row.text("created_at")?,
                    hand_edits: 0,
                },
            );
        }

        for row in read(format!(
            "SELECT media_id, value FROM media_override
              WHERE media_id IN ({holes}) AND scope = 'item' AND field = 'title' AND language = ''"
        ))
        .fetch_all(db.pool())
        .await?
        {
            let title = row
                .opt_text("value")?
                .and_then(|value| serde_json::from_str::<String>(&value).ok())
                .filter(|title| !title.trim().is_empty());
            if let (Some(card), Some(title)) = (found.get_mut(&row.text("media_id")?), title) {
                card.title = title;
            }
        }

        for row in read(format!(
            "SELECT media_id, url FROM media_image
              WHERE media_id IN ({holes}) AND cover_type = 'poster' AND season_number IS NULL
              ORDER BY media_id, sort_order DESC"
        ))
        .fetch_all(db.pool())
        .await?
        {
            // The lowest sort order last, so that it is the one kept.
            if let Some(card) = found.get_mut(&row.text("media_id")?) {
                card.poster = Some(row.text("url")?);
            }
        }

        for row in read(format!(
            "SELECT media_id, source, value FROM media_external_id
              WHERE media_id IN ({holes}) ORDER BY source, value"
        ))
        .fetch_all(db.pool())
        .await?
        {
            if let (Some(card), Ok(source)) = (
                found.get_mut(&row.text("media_id")?),
                row.text("source")?.parse::<ExternalSource>(),
            ) {
                card.external_ids.apply(source, &row.text("value")?);
            }
        }

        for table in [
            "media_override",
            "media_season",
            "media_episode",
            "media_image",
            "media_credit",
            "media_alternative_title",
            "media_translation",
        ] {
            let manual = if table == "media_override" {
                ""
            } else {
                " AND is_manual = 1"
            };
            for row in read(format!(
                "SELECT media_id, COUNT(*) AS n FROM {table}
                  WHERE media_id IN ({holes}){manual} GROUP BY media_id"
            ))
            .fetch_all(db.pool())
            .await?
            {
                if let Some(card) = found.get_mut(&row.text("media_id")?) {
                    card.hand_edits += row.big("n")?;
                }
            }
        }
    }
    for card in found.values_mut() {
        card.hand_edits += i64::from(card.is_manual);
    }
    Ok(found)
}

/// How many of each kind of row a work holds, as a merge would move them:
/// what the merge dialog says will happen to the merged work's.
pub async fn child_counts(db: &Db, id: &str) -> Result<BTreeMap<String, i64>> {
    let mut counts = BTreeMap::new();
    for (name, table) in [
        ("seasons", "media_season"),
        ("episodes", "media_episode"),
        ("images", "media_image"),
        ("credits", "media_credit"),
        ("alternativeTitles", "media_alternative_title"),
        ("translations", "media_translation"),
        ("orders", "media_order"),
        ("locks", "media_override"),
        ("lists", "curated_list_item"),
        ("changes", "media_change"),
    ] {
        let row = sqlx::query(db.sql(&format!(
            "SELECT COUNT(*) AS n FROM {table} WHERE media_id = ?"
        )))
        .bind(id)
        .fetch_one(db.pool())
        .await?;
        counts.insert(name.to_string(), row.big("n")?);
    }
    Ok(counts)
}

// ─── folded ─────────────────────────────────────────────────────────────────

/// Which of two works a field's value is kept from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = MergeSide)]
pub enum Side {
    Survivor,
    Merged,
}

/// What a merge is asked to do.
pub struct MergeInput<'a> {
    /// The work that becomes a tombstone.
    pub merged: &'a str,
    /// The work that stays.
    pub into: &'a str,
    /// The side each field of the work is kept from, where one was chosen.
    pub keep: &'a BTreeMap<String, Side>,
    /// The merged work's value of each field kept from it, as it was served:
    /// what the survivor is locked to where the merged work held no lock of
    /// its own. `null` for none.
    pub taken: &'a BTreeMap<String, Value>,
    /// Names of the merged work the survivor goes by none of: kept among its
    /// other titles, so that a search for them finds the work.
    pub titles: &'a [String],
    pub by: Option<&'a str>,
}

/// An order moved over, as it was: whether it was frozen as the aired
/// numbering, and whether it stood for the aired order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderWas {
    pub id: String,
    pub is_frozen: bool,
    pub is_default: bool,
}

/// What a merge did, kept on the tombstone for its undo.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeRecord {
    /// What this record is written as, for a later version to read it by.
    pub version: u32,
    /// The survivor.
    pub into: String,
    pub at: String,
    pub by: Option<String>,
    /// Whether the tombstone was switched on, and when it was due again.
    pub was_enabled: bool,
    pub was_due: Option<String>,
    /// Its identifiers elsewhere, by source and value, as it held them.
    pub ids: Vec<(String, String)>,
    /// Those the survivor took on: it had none of their source.
    pub added_ids: Vec<(String, String)>,
    /// The rows moved over, by table: what finds each again as the merge left
    /// it — its id, or the values it was moved with.
    pub moved: BTreeMap<String, Vec<Value>>,
    /// The rows of the tombstone that gave way to the survivor's, by table,
    /// whole.
    pub dropped: BTreeMap<String, Vec<Map<String, Value>>>,
    /// Every lock the tombstone held, whole.
    pub locks: Vec<Map<String, Value>>,
    /// The survivor's locks a choice replaced, whole.
    pub replaced: Vec<Map<String, Value>>,
    /// The locks the merge wrote on the survivor: scope, field, language.
    pub written: Vec<(String, String, String)>,
    /// The orders moved over, as they were.
    pub orders: Vec<OrderWas>,
    /// Whether the survivor was for adults, where the merged work's flag was
    /// kept.
    pub survivor_adult: Option<bool>,
    /// The alternative titles the survivor was given, by id.
    pub titles: Vec<String>,
    /// The fields chosen, and the side each was kept from.
    pub keep: BTreeMap<String, Side>,
}

/// What came of a merge: its record, and the people its credits name, whose
/// rows of the search index are written once it is committed.
pub struct Merged {
    pub record: MergeRecord,
    pub people: BTreeSet<String>,
}

/// One table of what hangs off a work, as a merge folds it.
struct Child {
    table: &'static str,
    /// What finds one of its rows again: its id, or the columns that tell a
    /// work's rows apart.
    key: &'static [&'static str],
    /// The columns that make one of the merged work's rows the same as one of
    /// the survivor's, which stands; `None` where no two are ever the same.
    same: Option<&'static [&'static str]>,
    /// What a row moved over is found by again, unchanged, for the undo.
    marks: &'static [&'static str],
    /// Whether a row that gave way is kept whole in the record. Not a
    /// provider's answer, which is the size of a document and is given again
    /// at the next refresh.
    kept: bool,
}

const ID: &[&str] = &["id"];

/// The tables folded row by row, in the order they are folded. Orders, the
/// episodes' translations, the locks and the identifiers have rules of their
/// own, below.
const CHILDREN: &[Child] = &[
    Child {
        table: "media_season",
        key: ID,
        same: Some(&["season_number"]),
        marks: ID,
        kept: true,
    },
    Child {
        table: "media_episode",
        key: ID,
        same: Some(&["season_number", "episode_number"]),
        marks: ID,
        kept: true,
    },
    Child {
        table: "media_image",
        key: ID,
        same: Some(&["cover_type", "url"]),
        marks: ID,
        kept: true,
    },
    Child {
        table: "media_credit",
        key: ID,
        same: Some(&["credit_type", "tmdb_person_id", "person_name"]),
        marks: ID,
        kept: true,
    },
    Child {
        table: "media_alternative_title",
        key: ID,
        same: Some(&["title", "language"]),
        marks: ID,
        kept: true,
    },
    Child {
        table: "media_relation",
        key: ID,
        same: Some(&["relation_type", "source", "external_id"]),
        marks: ID,
        kept: true,
    },
    Child {
        table: "media_release",
        key: ID,
        same: Some(&["country", "release_type", "release_date"]),
        marks: ID,
        kept: true,
    },
    Child {
        table: "media_translation",
        key: &["language"],
        same: Some(&["language"]),
        marks: &["language", "title", "overview", "tagline", "is_manual"],
        kept: true,
    },
    Child {
        table: "media_rating",
        key: &["source"],
        same: Some(&["source"]),
        marks: &["source", "value", "votes", "rating_type"],
        kept: true,
    },
    Child {
        table: "media_numbering",
        key: &[],
        same: Some(&[]),
        marks: &["silent", "silent_at", "since", "updated_at"],
        kept: true,
    },
    Child {
        table: "media_provider_snapshot",
        key: &["provider"],
        same: Some(&["provider"]),
        marks: &["provider", "fetched_at"],
        kept: false,
    },
    Child {
        table: "media_change",
        key: ID,
        same: None,
        marks: ID,
        kept: true,
    },
    Child {
        table: "curated_list_item",
        key: &["list_id"],
        same: Some(&["list_id"]),
        marks: &["list_id", "position", "added_at"],
        kept: true,
    },
];

/// A row, as a merge keeps one: each column by its name, integers as
/// integers, reals as reals, text as text — the three kinds of value the
/// schema holds.
type Row = Map<String, Value>;

/// Fold `merged` into `into`, in one transaction. The caller has checked
/// that both are works of their own and of one kind.
pub async fn merge_works(db: &Db, input: &MergeInput<'_>) -> Result<Merged> {
    let (merged, into) = (input.merged, input.into);
    let at = now();
    let mut tx = db.begin_write().await?;
    hold(db, &mut tx, &[merged, into]).await?;

    let row = sqlx::query(db.sql(
        "SELECT id, slug, is_enabled, is_manual, refresh_after, merged_into
           FROM media_item WHERE id IN (?, ?)",
    ))
    .bind(merged)
    .bind(into)
    .fetch_all(&mut *tx)
    .await?;
    let find = |id: &str| {
        row.iter()
            .find(|row| row.text("id").ok().as_deref() == Some(id))
    };
    let (Some(gone), Some(kept)) = (find(merged), find(into)) else {
        bail!("one of the two works is no longer there");
    };
    if gone.opt_text("merged_into")?.is_some() || kept.opt_text("merged_into")?.is_some() {
        bail!("one of the two works was merged into another meanwhile");
    }
    let slug = gone.text("slug")?;
    let survivor_manual = kept.flag("is_manual")?;

    let mut record = MergeRecord {
        version: 1,
        into: into.to_string(),
        at: at.clone(),
        by: input.by.map(str::to_string),
        was_enabled: gone.flag("is_enabled")?,
        was_due: gone.opt_text("refresh_after")?,
        keep: input.keep.clone(),
        ..Default::default()
    };

    // Who the credits of both name, before theirs move: written again once
    // this is committed, for a work fewer each where two credits were one.
    let both = [into.to_string(), merged.to_string()];
    let mut people = crate::db::repo::search::people_of(db, &mut tx, &both).await?;

    fold_ids(db, &mut tx, merged, into, &at, &mut record).await?;

    let mut moved_episodes: Vec<(i64, i64)> = Vec::new();
    for child in CHILDREN {
        let moved = fold(db, &mut tx, child, merged, into, &mut record).await?;
        if child.table == "media_episode" {
            for row in &moved {
                if let (Some(season), Some(episode)) = (
                    row.get("season_number").and_then(Value::as_i64),
                    row.get("episode_number").and_then(Value::as_i64),
                ) {
                    moved_episodes.push((season, episode));
                }
            }
        }
    }
    fold_orders(db, &mut tx, merged, into, &mut record).await?;
    fold_episode_texts(db, &mut tx, merged, into, &moved_episodes).await?;
    fold_locks(db, &mut tx, input, &at, &mut record).await?;

    // The merged work's names, kept among the survivor's other titles.
    for title in input.titles {
        let id = new_id();
        let written = sqlx::query(db.sql(
            "INSERT INTO media_alternative_title
                 (id, media_id, title, title_type, language, is_manual, created_at)
             VALUES (?, ?, ?, NULL, NULL, 1, ?)
             ON CONFLICT DO NOTHING",
        ))
        .bind(&id)
        .bind(into)
        .bind(title)
        .bind(&at)
        .execute(&mut *tx)
        .await
        .context("failed to keep the merged work's title")?;
        if written.rows_affected() > 0 {
            record.titles.push(id);
        }
    }

    // What the server fetched each work's episodes in: the survivor holds
    // episodes now that were never fetched in those languages.
    sqlx::query(db.sql("DELETE FROM media_language_fetch WHERE media_id IN (?, ?)"))
        .bind(merged)
        .bind(into)
        .execute(&mut *tx)
        .await?;
    // A picture or a theme the merged work was the first to want is the
    // survivor's to be refreshed by once it is kept.
    sqlx::query(db.sql("UPDATE media_asset SET wanted_by = ? WHERE wanted_by = ?"))
        .bind(into)
        .bind(merged)
        .execute(&mut *tx)
        .await?;
    // The pairs the tombstone was in: looked for again with the survivor.
    sqlx::query(db.sql("DELETE FROM media_duplicate WHERE first_id = ? OR second_id = ?"))
        .bind(merged)
        .bind(merged)
        .execute(&mut *tx)
        .await?;

    // What it was known by, answered by the survivor.
    let mut names = vec![
        (crate::db::repo::alias::ID.to_string(), merged.to_string()),
        (crate::db::repo::alias::SLUG.to_string(), slug),
    ];
    names.extend(record.ids.iter().cloned());
    crate::db::repo::alias::write(db, &mut tx, merged, into, &names).await?;

    // The tombstone: switched off, never due, the record beside it.
    sqlx::query(db.sql(
        "UPDATE media_item
            SET merged_into = ?, merged_at = ?, merge_record = ?, is_enabled = 0,
                refresh_after = NULL, updated_at = ?
          WHERE id = ?",
    ))
    .bind(into)
    .bind(&at)
    .bind(serde_json::to_string(&record)?)
    .bind(&at)
    .bind(merged)
    .execute(&mut *tx)
    .await
    .context("failed to leave the merged work as a tombstone")?;

    // The survivor: written now, as Radarr's list of what changed reads it,
    // and asked of its sources again soon when it took on identifiers its
    // sources can be asked by — unless it was entered by hand, which is
    // never asked anything.
    match (!record.added_ids.is_empty()) && !survivor_manual {
        true => {
            sqlx::query(
                db.sql("UPDATE media_item SET updated_at = ?, refresh_after = ? WHERE id = ?"),
            )
            .bind(&at)
            .bind(&at)
            .bind(into)
            .execute(&mut *tx)
            .await?;
        }
        false => {
            sqlx::query(db.sql("UPDATE media_item SET updated_at = ? WHERE id = ?"))
                .bind(&at)
                .bind(into)
                .execute(&mut *tx)
                .await?;
        }
    }
    super::mark_changed_in(db, &mut tx, into).await?;

    // Found by what it now holds, in the same transaction. The tombstone
    // first: an episode keeps its row of the index wherever it is, and the
    // tombstone's rows of what moved are taken out before the survivor's are
    // written, or they would stand for the survivor's and still name it.
    for id in [merged, into] {
        people.extend(crate::db::repo::search::index_works(db, &mut tx, &[id.to_string()]).await?);
    }

    tx.commit().await.context("failed to commit the merge")?;
    Ok(Merged { record, people })
}

/// Hold two works' rows for the transaction, in id order, so that two merges
/// crossing each other wait rather than deadlock. SQLite's write transaction
/// holds everything already.
async fn hold(db: &Db, tx: &mut Transaction<'_, Any>, ids: &[&str]) -> Result<()> {
    if db.dialect() != Dialect::Postgres {
        return Ok(());
    }
    let mut ordered: Vec<&str> = ids.to_vec();
    ordered.sort_unstable();
    for id in ordered {
        sqlx::query(db.sql("SELECT id FROM media_item WHERE id = ? FOR UPDATE"))
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    }
    Ok(())
}

/// The identifiers: every one of the merged work's taken off it — its
/// aliases answer for them — and each one whose source the survivor has none
/// of, and each of AniList's and MyAnimeList's, of which a work may have
/// several, made the survivor's.
async fn fold_ids(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    merged: &str,
    into: &str,
    at: &str,
    record: &mut MergeRecord,
) -> Result<()> {
    let read = |id: String| {
        sqlx::query(db.sql(
            "SELECT source, value FROM media_external_id WHERE media_id = ? ORDER BY source, value",
        ))
        .bind(id)
    };
    let mine: Vec<(String, String)> = read(merged.to_string())
        .fetch_all(&mut **tx)
        .await?
        .iter()
        .map(|row| Ok((row.text("source")?, row.text("value")?)))
        .collect::<Result<_>>()?;
    let theirs: Vec<String> = read(into.to_string())
        .fetch_all(&mut **tx)
        .await?
        .iter()
        .map(|row| row.text("source"))
        .collect::<Result<_, _>>()?;

    sqlx::query(db.sql("DELETE FROM media_external_id WHERE media_id = ?"))
        .bind(merged)
        .execute(&mut **tx)
        .await?;

    for (source, value) in &mine {
        let several = matches!(source.as_str(), "mal" | "anilist");
        if !several && theirs.contains(source) {
            continue;
        }
        let written = sqlx::query(db.sql(
            "INSERT INTO media_external_id (media_id, source, value, created_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (source, value) DO NOTHING",
        ))
        .bind(into)
        .bind(source)
        .bind(value)
        .bind(at)
        .execute(&mut **tx)
        .await?;
        if written.rows_affected() > 0 {
            record.added_ids.push((source.clone(), value.clone()));
        }
    }
    record.ids = mine;
    Ok(())
}

/// Fold one table: the merged work's rows the survivor holds the same of
/// give way, the rest move over. Returns the rows moved.
async fn fold(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    child: &Child,
    merged: &str,
    into: &str,
    record: &mut MergeRecord,
) -> Result<Vec<Row>> {
    let rows = capture(
        db,
        tx,
        &format!("SELECT * FROM {} WHERE media_id = ?", child.table),
        merged,
    )
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let theirs: HashSet<String> = match child.same {
        Some(columns) => {
            // A credit is told apart by more than its columns name here — the
            // episode it is of, the person it names — as far as the schema
            // has them.
            let list = if columns.is_empty() {
                "media_id".to_string()
            } else if child.table == "media_credit" {
                "*".to_string()
            } else {
                columns.join(", ")
            };
            capture(
                db,
                tx,
                &format!("SELECT {list} FROM {} WHERE media_id = ?", child.table),
                into,
            )
            .await?
            .iter()
            .map(|row| same_key(child.table, columns, row))
            .collect()
        }
        None => HashSet::new(),
    };
    let (gave_way, moving): (Vec<Row>, Vec<Row>) = rows.into_iter().partition(|row| {
        child
            .same
            .is_some_and(|columns| theirs.contains(&same_key(child.table, columns, row)))
    });

    // What gives way goes first: moved, it would meet the survivor's row
    // under a key the table keeps unique.
    remove(db, tx, child, merged, &gave_way).await?;
    sqlx::query(db.sql(&format!(
        "UPDATE {} SET media_id = ? WHERE media_id = ?",
        child.table
    )))
    .bind(into)
    .bind(merged)
    .execute(&mut **tx)
    .await
    .with_context(|| format!("failed to move the merged work's {}", child.table))?;

    if !moving.is_empty() {
        record
            .moved
            .entry(child.table.to_string())
            .or_default()
            .extend(moving.iter().map(|row| marks(child.marks, row)));
    }
    if child.kept && !gave_way.is_empty() {
        record
            .dropped
            .entry(child.table.to_string())
            .or_default()
            .extend(gave_way);
    }
    Ok(moving)
}

/// What makes two rows of a table the same thing: their values of `columns`,
/// a missing one as an empty one — as the unique indexes over a language
/// read it. A credit is a person in a part, of the work or of one of its
/// episodes: the person by TMDB's id for them where the credit has one,
/// else by the person it names here, else by their name whatever its case.
fn same_key(table: &str, columns: &[&str], row: &Row) -> String {
    if table == "media_credit" {
        let person = match (row.get("tmdb_person_id"), row.get("person_id")) {
            (Some(Value::Number(id)), _) => format!("tmdb:{id}"),
            (_, Some(Value::String(id))) => format!("person:{id}"),
            _ => format!(
                "name:{}",
                text_of(row.get("person_name")).trim().to_lowercase()
            ),
        };
        return format!(
            "{}|{}|{}|{person}",
            text_of(row.get("credit_type")),
            text_of(row.get("season_number")),
            text_of(row.get("episode_number")),
        );
    }
    columns
        .iter()
        .map(|column| text_of(row.get(*column)))
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

fn text_of(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

/// What finds a row again: its id alone, or the values of `columns`.
fn marks(columns: &[&str], row: &Row) -> Value {
    if columns == ID {
        return row.get("id").cloned().unwrap_or(Value::Null);
    }
    Value::Object(
        columns
            .iter()
            .map(|column| {
                (
                    (*column).to_string(),
                    row.get(*column).cloned().unwrap_or(Value::Null),
                )
            })
            .collect(),
    )
}

/// Delete rows of the merged work by their key.
async fn remove(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    child: &Child,
    merged: &str,
    rows: &[Row],
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    if child.key == ID {
        let ids: Vec<String> = rows.iter().map(|row| text_of(row.get("id"))).collect();
        for chunk in ids.chunks(400) {
            let sql = format!(
                "DELETE FROM {} WHERE id IN ({})",
                child.table,
                vec!["?"; chunk.len()].join(", ")
            );
            let mut query = sqlx::query(db.sql(&sql));
            for id in chunk {
                query = query.bind(id);
            }
            query.execute(&mut **tx).await?;
        }
        return Ok(());
    }
    for row in rows {
        let (condition, values) = matching(child.key, row)?;
        let sql = format!("DELETE FROM {} WHERE media_id = ?{condition}", child.table);
        let mut args = AnyArguments::default();
        args.add(merged.to_string())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        for value in values {
            bind(&mut args, value)?;
        }
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// The words `AND column = ?` for each of `columns` a row has a value of,
/// `AND column IS NULL` for one it has none of — a null bound would be a
/// typed one, which PostgreSQL will not compare with a column of another
/// type — and the values to bind, in order.
fn matching<'r>(columns: &[&str], row: &'r Map<String, Value>) -> Result<(String, Vec<&'r Value>)> {
    let mut words = String::new();
    let mut values = Vec::new();
    for column in columns {
        named(column)?;
        match row.get(*column) {
            None | Some(Value::Null) => words.push_str(&format!(" AND {column} IS NULL")),
            Some(value) => {
                words.push_str(&format!(" AND {column} = ?"));
                values.push(value);
            }
        }
    }
    Ok((words, values))
}

/// A column's name, as a statement may be built with: what the record kept
/// is read back from the database, so it is checked before it is written
/// into one.
fn named(column: &str) -> Result<()> {
    let fine = !column.is_empty()
        && column
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if fine {
        Ok(())
    } else {
        bail!("{column:?} is no column of this schema")
    }
}

fn bind(args: &mut AnyArguments, value: &Value) -> Result<()> {
    let added = match value {
        Value::Number(number) => match number.as_i64() {
            Some(int) => args.add(int),
            None => args.add(number.as_f64().unwrap_or_default()),
        },
        Value::String(text) => args.add(text.clone()),
        Value::Bool(flag) => args.add(from_bool(*flag)),
        other => args.add(other.to_string()),
    };
    added.map_err(|e| anyhow::anyhow!("{e}"))
}

/// Read the rows a statement selects, with one value bound, whole.
async fn capture(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    sql: &str,
    bound: &str,
) -> Result<Vec<Row>> {
    let rows = sqlx::query(db.sql(sql))
        .bind(bound)
        .fetch_all(&mut **tx)
        .await?;
    rows.iter().map(row_of).collect()
}

fn row_of(row: &sqlx::any::AnyRow) -> Result<Row> {
    let mut out = Map::new();
    for (index, column) in row.columns().iter().enumerate() {
        out.insert(column.name().to_string(), cell(row, index)?);
    }
    Ok(out)
}

/// One column's value, without asking the engine its type — `Any` has none
/// to give: integers tried first, since an integer reads as a real too.
fn cell(row: &sqlx::any::AnyRow, index: usize) -> Result<Value> {
    if row.try_get_raw(index)?.is_null() {
        return Ok(Value::Null);
    }
    if let Ok(int) = row.try_get::<i64, _>(index) {
        return Ok(Value::from(int));
    }
    if let Ok(real) = row.try_get::<f64, _>(index) {
        return Ok(serde_json::Number::from_f64(real).map_or(Value::Null, Value::Number));
    }
    if let Ok(text) = row.try_get::<String, _>(index) {
        return Ok(Value::String(text));
    }
    bail!(
        "column {} holds a value of no kind this schema uses",
        row.column(index).name()
    )
}

/// Write rows back as they were kept: a row that is there again already is
/// left as it is. A missing value is written as `NULL` itself rather than
/// bound, for the reason [`matching`] gives.
async fn put_back(db: &Db, tx: &mut Transaction<'_, Any>, table: &str, rows: &[Row]) -> Result<()> {
    for row in rows {
        let mut columns = Vec::with_capacity(row.len());
        let mut holes = Vec::with_capacity(row.len());
        let mut args = AnyArguments::default();
        for (column, value) in row {
            named(column)?;
            columns.push(column.as_str());
            if value.is_null() {
                holes.push("NULL");
            } else {
                holes.push("?");
                bind(&mut args, value)?;
            }
        }
        let sql = format!(
            "INSERT INTO {table} ({}) VALUES ({}) ON CONFLICT DO NOTHING",
            columns.join(", "),
            holes.join(", ")
        );
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut **tx)
            .await
            .with_context(|| format!("failed to put a row of {table} back"))?;
    }
    Ok(())
}

/// The orders: one under a key the survivor has an order under gives way,
/// its places with it; the rest move over, none of them frozen as the
/// survivor's aired numbering — that would renumber the survivor for its
/// clients — and none standing for its aired order where one does already.
async fn fold_orders(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    merged: &str,
    into: &str,
    record: &mut MergeRecord,
) -> Result<()> {
    let mine = capture(
        db,
        tx,
        "SELECT * FROM media_order WHERE media_id = ?",
        merged,
    )
    .await?;
    if mine.is_empty() {
        return Ok(());
    }
    let theirs = capture(
        db,
        tx,
        "SELECT key, is_default FROM media_order WHERE media_id = ?",
        into,
    )
    .await?;
    let keys: HashSet<String> = theirs.iter().map(|row| text_of(row.get("key"))).collect();
    let has_default = theirs
        .iter()
        .any(|row| row.get("is_default").and_then(Value::as_i64) == Some(1));

    for order in mine {
        let id = text_of(order.get("id"));
        if keys.contains(&text_of(order.get("key"))) {
            let places = capture(
                db,
                tx,
                "SELECT * FROM media_order_episode WHERE order_id = ?",
                &id,
            )
            .await?;
            sqlx::query(db.sql("DELETE FROM media_order WHERE id = ?"))
                .bind(&id)
                .execute(&mut **tx)
                .await?;
            record
                .dropped
                .entry("media_order".into())
                .or_default()
                .push(order);
            if !places.is_empty() {
                record
                    .dropped
                    .entry("media_order_episode".into())
                    .or_default()
                    .extend(places);
            }
            continue;
        }

        let is_default = order.get("is_default").and_then(Value::as_i64) == Some(1);
        sqlx::query(db.sql(
            "UPDATE media_order SET media_id = ?, is_frozen = 0, is_default = ? WHERE id = ?",
        ))
        .bind(into)
        .bind(from_bool(is_default && !has_default))
        .bind(&id)
        .execute(&mut **tx)
        .await?;
        record.orders.push(OrderWas {
            id: id.clone(),
            is_frozen: order.get("is_frozen").and_then(Value::as_i64) == Some(1),
            is_default,
        });
        record
            .moved
            .entry("media_order".into())
            .or_default()
            .push(Value::String(id));
    }
    Ok(())
}

/// The episodes' text in other languages goes where the episode goes: with
/// those moved over, to the survivor, over whatever it kept at those numbers;
/// with those that gave way, nowhere. It is fetched again, in each language,
/// the next time it is asked for.
async fn fold_episode_texts(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    merged: &str,
    into: &str,
    moved: &[(i64, i64)],
) -> Result<()> {
    for (season, episode) in moved {
        sqlx::query(db.sql(
            "DELETE FROM media_episode_translation
              WHERE media_id = ? AND season_number = ? AND episode_number = ?",
        ))
        .bind(into)
        .bind(season)
        .bind(episode)
        .execute(&mut **tx)
        .await?;
        sqlx::query(db.sql(
            "UPDATE media_episode_translation SET media_id = ?
              WHERE media_id = ? AND season_number = ? AND episode_number = ?",
        ))
        .bind(into)
        .bind(merged)
        .bind(season)
        .bind(episode)
        .execute(&mut **tx)
        .await?;
    }
    sqlx::query(db.sql("DELETE FROM media_episode_translation WHERE media_id = ?"))
        .bind(merged)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// A lock's key: its scope, its field and its language.
fn lock_key(row: &Row) -> (String, String, String) {
    (
        text_of(row.get("scope")),
        text_of(row.get("field")),
        text_of(row.get("language")),
    )
}

/// The locks, as the module's rules say: the merged work's where the
/// survivor locks nothing of the same, a choice where both do, the merged
/// work's value locked on the survivor where it was chosen and not locked.
/// Its slug and its identifiers stay its own: the survivor keeps its own.
async fn fold_locks(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    input: &MergeInput<'_>,
    at: &str,
    record: &mut MergeRecord,
) -> Result<()> {
    let (merged, into) = (input.merged, input.into);
    let mine = capture(
        db,
        tx,
        "SELECT * FROM media_override WHERE media_id = ?",
        merged,
    )
    .await?;
    let theirs: HashMap<(String, String, String), Row> = capture(
        db,
        tx,
        "SELECT * FROM media_override WHERE media_id = ?",
        into,
    )
    .await?
    .into_iter()
    .map(|row| (lock_key(&row), row))
    .collect();
    record.locks = mine.clone();

    let mut written: Vec<(String, String, String)> = Vec::new();
    for lock in &mine {
        let key = lock_key(lock);
        let (scope, field, language) = (&key.0, &key.1, &key.2);
        if scope == "item" && (field == "slug" || field == "externalIds") {
            continue;
        }
        let chosen = (scope == "item" && language.is_empty())
            .then(|| input.keep.get(field.as_str()).copied())
            .flatten();
        let take = match chosen {
            Some(Side::Survivor) => false,
            Some(Side::Merged) => true,
            None => !theirs.contains_key(&key),
        };
        if !take {
            continue;
        }
        if let Some(old) = theirs.get(&key) {
            record.replaced.push(old.clone());
            delete_lock(db, tx, into, &key).await?;
        }
        sqlx::query(db.sql(
            "UPDATE media_override SET media_id = ?, updated_at = ?, updated_by = ?
              WHERE media_id = ? AND scope = ? AND field = ? AND language = ?",
        ))
        .bind(into)
        .bind(at)
        .bind(input.by)
        .bind(merged)
        .bind(scope)
        .bind(field)
        .bind(language)
        .execute(&mut **tx)
        .await
        .context("failed to move a lock onto the survivor")?;
        written.push(key);
    }

    // A field kept from the merged work that it held no lock on: the
    // survivor locked to what the merged work was served with.
    for (field, side) in input.keep {
        let key = ("item".to_string(), field.clone(), String::new());
        if *side != Side::Merged || written.contains(&key) {
            continue;
        }
        let Some(value) = input.taken.get(field) else {
            bail!("no value was taken of the merged work's {field}");
        };
        if let Some(old) = theirs.get(&key) {
            record.replaced.push(old.clone());
        }
        let encoded = (!value.is_null())
            .then(|| serde_json::to_string(value))
            .transpose()?;
        sqlx::query(db.sql(
            "INSERT INTO media_override
                 (media_id, scope, field, language, value, created_at, updated_at, updated_by)
             VALUES (?, 'item', ?, '', ?, ?, ?, ?)
             ON CONFLICT (media_id, scope, field, language) DO UPDATE SET
                 value = excluded.value,
                 updated_at = excluded.updated_at,
                 updated_by = excluded.updated_by",
        ))
        .bind(into)
        .bind(field)
        .bind(encoded)
        .bind(at)
        .bind(at)
        .bind(input.by)
        .execute(&mut **tx)
        .await
        .context("failed to lock the survivor to the merged work's value")?;
        written.push(key);
    }

    // Whether it is for adults is written to the row as well as locked.
    let adult = ("item".to_string(), "isAdult".to_string(), String::new());
    if written.contains(&adult) {
        let row = sqlx::query(db.sql(
            "SELECT value FROM media_override
              WHERE media_id = ? AND scope = 'item' AND field = 'isAdult' AND language = ''",
        ))
        .bind(into)
        .fetch_optional(&mut **tx)
        .await?;
        let locked = row
            .and_then(|row| row.opt_text("value").ok().flatten())
            .and_then(|value| serde_json::from_str::<bool>(&value).ok());
        if let Some(locked) = locked {
            let before: i64 =
                sqlx::query_scalar(db.sql("SELECT is_adult FROM media_item WHERE id = ?"))
                    .bind(into)
                    .fetch_one(&mut **tx)
                    .await?;
            record.survivor_adult = Some(before != 0);
            sqlx::query(db.sql("UPDATE media_item SET is_adult = ? WHERE id = ?"))
                .bind(from_bool(locked))
                .bind(into)
                .execute(&mut **tx)
                .await?;
        }
    }

    // What gave way goes with the tombstone's other children: it is all in
    // the record.
    sqlx::query(db.sql("DELETE FROM media_override WHERE media_id = ?"))
        .bind(merged)
        .execute(&mut **tx)
        .await?;
    record.written = written;
    Ok(())
}

async fn delete_lock(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    media_id: &str,
    (scope, field, language): &(String, String, String),
) -> Result<()> {
    sqlx::query(db.sql(
        "DELETE FROM media_override
          WHERE media_id = ? AND scope = ? AND field = ? AND language = ?",
    ))
    .bind(media_id)
    .bind(scope)
    .bind(field)
    .bind(language)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

// ─── taken apart ────────────────────────────────────────────────────────────

/// What came of an undo: the record it followed, the identifiers another
/// work holds now — by source, value and holder — which the work put back
/// goes without, and the people to write the search index's rows of.
pub struct Unmerged {
    pub record: MergeRecord,
    pub taken: Vec<(String, String, String)>,
    pub people: BTreeSet<String>,
}

/// The order the rows that gave way are put back in: an order before its
/// places, the rest as they come.
const PUT_BACK: &[&str] = &[
    "media_season",
    "media_episode",
    "media_image",
    "media_credit",
    "media_alternative_title",
    "media_relation",
    "media_release",
    "media_translation",
    "media_rating",
    "media_numbering",
    "media_order",
    "media_order_episode",
    "media_change",
    "curated_list_item",
];

/// Undo the merge `merged` went into, in one transaction, as the module says.
/// The caller has checked the window and that the survivor is still a work
/// of its own.
pub async fn unmerge_work(db: &Db, merged: &str, by: Option<&str>) -> Result<Unmerged> {
    let at = now();
    let mut tx = db.begin_write().await?;

    let text: Option<String> = sqlx::query_scalar(
        db.sql("SELECT merge_record FROM media_item WHERE id = ? AND merged_into IS NOT NULL"),
    )
    .bind(merged)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    let Some(text) = text else {
        bail!("the work is no tombstone, or its merge left no record");
    };
    let record: MergeRecord =
        serde_json::from_str(&text).context("the merge's record cannot be read")?;
    let into = record.into.clone();
    hold(db, &mut tx, &[merged, &into]).await?;

    let manual: Option<i64> = sqlx::query_scalar(
        db.sql("SELECT is_manual FROM media_item WHERE id = ? AND merged_into IS NULL"),
    )
    .bind(&into)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(survivor_manual) = manual else {
        bail!("the work it was merged into is gone, or merged in its turn");
    };
    let merged_manual: i64 =
        sqlx::query_scalar(db.sql("SELECT is_manual FROM media_item WHERE id = ?"))
            .bind(merged)
            .fetch_one(&mut *tx)
            .await?;

    let both = [into.clone(), merged.to_string()];
    let mut people = crate::db::repo::search::people_of(db, &mut tx, &both).await?;

    crate::db::repo::alias::remove(db, &mut tx, merged).await?;

    // The identifiers: those the survivor took on given back, then the
    // tombstone's own, but those another work holds now.
    for (source, value) in &record.added_ids {
        sqlx::query(
            db.sql("DELETE FROM media_external_id WHERE media_id = ? AND source = ? AND value = ?"),
        )
        .bind(&into)
        .bind(source)
        .bind(value)
        .execute(&mut *tx)
        .await?;
    }
    let mut taken = Vec::new();
    for (source, value) in &record.ids {
        let written = sqlx::query(db.sql(
            "INSERT INTO media_external_id (media_id, source, value, created_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (source, value) DO NOTHING",
        ))
        .bind(merged)
        .bind(source)
        .bind(value)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        if written.rows_affected() == 0 {
            let holder: Option<String> = sqlx::query_scalar(
                db.sql("SELECT media_id FROM media_external_id WHERE source = ? AND value = ?"),
            )
            .bind(source)
            .bind(value)
            .fetch_optional(&mut *tx)
            .await?;
            taken.push((source.clone(), value.clone(), holder.unwrap_or_default()));
        }
    }

    unmerge_locks(db, &mut tx, &into, &record).await?;

    // What moved over and is still as it was moved, back.
    for (table, marked) in &record.moved {
        let Some(child) = CHILDREN
            .iter()
            .find(|child| child.table == table)
            .map(|child| (child.table, child.marks))
            .or_else(|| (table == "media_order").then_some(("media_order", ID)))
        else {
            continue;
        };
        move_back(db, &mut tx, child, merged, &into, marked).await?;
    }
    for order in &record.orders {
        sqlx::query(db.sql(
            "UPDATE media_order SET is_frozen = ?, is_default = ? WHERE id = ? AND media_id = ?",
        ))
        .bind(from_bool(order.is_frozen))
        .bind(from_bool(order.is_default))
        .bind(&order.id)
        .bind(merged)
        .execute(&mut *tx)
        .await?;
    }

    // What gave way, put back — a selection's membership only while the
    // selection is there.
    for table in PUT_BACK {
        let Some(rows) = record.dropped.get(*table) else {
            continue;
        };
        let rows: Vec<Row> = if *table == "curated_list_item" {
            let mut kept = Vec::new();
            for row in rows {
                let there: Option<String> =
                    sqlx::query_scalar(db.sql("SELECT id FROM curated_list WHERE id = ?"))
                        .bind(text_of(row.get("list_id")))
                        .fetch_optional(&mut *tx)
                        .await?;
                if there.is_some() {
                    kept.push(row.clone());
                }
            }
            kept
        } else {
            rows.clone()
        };
        put_back(db, &mut tx, table, &rows).await?;
    }

    // The names the survivor was given.
    for id in &record.titles {
        sqlx::query(db.sql("DELETE FROM media_alternative_title WHERE id = ? AND media_id = ?"))
            .bind(id)
            .bind(&into)
            .execute(&mut *tx)
            .await?;
    }

    sqlx::query(db.sql("DELETE FROM media_language_fetch WHERE media_id IN (?, ?)"))
        .bind(merged)
        .bind(&into)
        .execute(&mut *tx)
        .await?;

    // The work put back: switched on as it was, and due — but for one
    // entered by hand, which nothing refreshes — so that what its sources
    // give comes back to it. The survivor too, when it gives identifiers
    // back: what it holds of them is the merged work's.
    sqlx::query(db.sql(
        "UPDATE media_item
            SET merged_into = NULL, merged_at = NULL, merge_record = NULL, is_enabled = ?,
                refresh_after = ?, updated_at = ?
          WHERE id = ?",
    ))
    .bind(from_bool(record.was_enabled))
    .bind(if merged_manual == 0 {
        Some(at.clone())
    } else {
        record.was_due.clone()
    })
    .bind(&at)
    .bind(merged)
    .execute(&mut *tx)
    .await
    .context("failed to put the merged work back")?;
    let survivor_due = !record.added_ids.is_empty() && survivor_manual == 0;
    match survivor_due {
        true => {
            sqlx::query(
                db.sql("UPDATE media_item SET updated_at = ?, refresh_after = ? WHERE id = ?"),
            )
            .bind(&at)
            .bind(&at)
            .bind(&into)
            .execute(&mut *tx)
            .await?;
        }
        false => {
            sqlx::query(db.sql("UPDATE media_item SET updated_at = ? WHERE id = ?"))
                .bind(&at)
                .bind(&into)
                .execute(&mut *tx)
                .await?;
        }
    }
    super::mark_changed_in(db, &mut tx, &into).await?;
    super::mark_changed_in(db, &mut tx, merged).await?;

    // Taken apart, they are not one work: not proposed again.
    dismiss_in(db, &mut tx, merged, &into, by).await?;

    // The survivor's rows of what went back first, as the merge took the
    // tombstone's first.
    for id in [into.as_str(), merged] {
        people.extend(crate::db::repo::search::index_works(db, &mut tx, &[id.to_string()]).await?);
    }
    tx.commit().await.context("failed to commit the undo")?;

    Ok(Unmerged {
        record,
        taken,
        people,
    })
}

/// Move rows back to the work put back: by id, or — for a table whose rows
/// have none — only where the row is still the one the merge moved, every
/// value it was moved with unchanged.
async fn move_back(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    (table, columns): (&str, &[&str]),
    merged: &str,
    into: &str,
    marked: &[Value],
) -> Result<()> {
    if columns == ID {
        let ids: Vec<String> = marked
            .iter()
            .filter_map(|mark| mark.as_str().map(str::to_string))
            .collect();
        for chunk in ids.chunks(400) {
            let sql = format!(
                "UPDATE {table} SET media_id = ? WHERE media_id = ? AND id IN ({})",
                vec!["?"; chunk.len()].join(", ")
            );
            let mut query = sqlx::query(db.sql(&sql)).bind(merged).bind(into);
            for id in chunk {
                query = query.bind(id);
            }
            query.execute(&mut **tx).await?;
        }
        return Ok(());
    }
    for mark in marked {
        let Some(values) = mark.as_object() else {
            continue;
        };
        let (condition, binds) = matching(columns, values)?;
        let sql = format!("UPDATE {table} SET media_id = ? WHERE media_id = ?{condition}");
        let mut args = AnyArguments::default();
        args.add(merged.to_string())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        args.add(into.to_string())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        for value in binds {
            bind(&mut args, value)?;
        }
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// The locks, back: those the merge wrote on the survivor and nobody has
/// set again since go, the survivor's own it replaced come back where the
/// survivor holds none now, and the tombstone has every one of its own again.
async fn unmerge_locks(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    into: &str,
    record: &MergeRecord,
) -> Result<()> {
    for key in &record.written {
        sqlx::query(db.sql(
            "DELETE FROM media_override
              WHERE media_id = ? AND scope = ? AND field = ? AND language = ? AND updated_at = ?",
        ))
        .bind(into)
        .bind(&key.0)
        .bind(&key.1)
        .bind(&key.2)
        .bind(&record.at)
        .execute(&mut **tx)
        .await?;
    }
    put_back(db, tx, "media_override", &record.replaced).await?;
    put_back(db, tx, "media_override", &record.locks).await?;

    // Whether the survivor is for adults: as its lock says, now that its
    // own is back, else as its row said before the merge.
    if let Some(before) = record.survivor_adult {
        let row = sqlx::query(db.sql(
            "SELECT value FROM media_override
              WHERE media_id = ? AND scope = 'item' AND field = 'isAdult' AND language = ''",
        ))
        .bind(into)
        .fetch_optional(&mut **tx)
        .await?;
        let locked = row
            .and_then(|row| row.opt_text("value").ok().flatten())
            .and_then(|value| serde_json::from_str::<bool>(&value).ok());
        sqlx::query(db.sql("UPDATE media_item SET is_adult = ? WHERE id = ?"))
            .bind(from_bool(locked.unwrap_or(before)))
            .bind(into)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        db::{
            repo::{
                self,
                change::NewChange,
                child::{blank_episode, blank_season},
                item::{ItemWrite, upsert},
                list::{ListFields, ListKind, ListMode},
            },
            testing::db,
        },
        domain::{
            AlternativeTitle, CoverType, Credit, CreditType, Image, MediaItem, Translation,
            fields::Scope,
        },
    };

    /// A work, stored as its sources gave it; `shape` says what it is.
    async fn work(
        db: &Db,
        kind: MediaKind,
        title: &str,
        shape: impl FnOnce(&mut MediaItem),
    ) -> MediaItem {
        let mut item = MediaItem::empty(kind);
        item.title = title.into();
        item.slug = crate::domain::make_slug(title, None);
        shape(&mut item);
        upsert(
            db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        item
    }

    fn poster(url: &str) -> Image {
        Image {
            id: String::new(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: url.into(),
            language: None,
            sort_order: 0,
            source: Some("tmdb".into()),
            is_manual: false,
        }
    }

    fn credit(kind: CreditType, name: &str, tmdb: i64) -> Credit {
        Credit {
            id: String::new(),
            credit_type: kind,
            person_name: name.into(),
            character_name: None,
            image: None,
            tmdb_person_id: Some(tmdb),
            credit_tmdb_id: None,
            sort_order: 0,
            is_manual: false,
        }
    }

    fn translation(language: &str, title: &str) -> Translation {
        Translation {
            language: language.into(),
            title: Some(title.into()),
            overview: None,
            tagline: None,
            is_manual: false,
        }
    }

    fn other_title(title: &str) -> AlternativeTitle {
        AlternativeTitle {
            id: String::new(),
            title: title.into(),
            title_type: None,
            language: None,
            is_manual: false,
        }
    }

    fn season(number: i32, title: &str) -> crate::domain::Season {
        let mut season = blank_season(number);
        season.is_manual = false;
        season.title = Some(title.into());
        season
    }

    fn episode(season: i32, number: i32, title: &str) -> crate::domain::Episode {
        let mut episode = blank_episode(season, number);
        episode.is_manual = false;
        episode.title = title.into();
        episode
    }

    /// What a refresh found a work's identifiers to be, in its journal.
    async fn said_ids(db: &Db, id: &str, ids: Value) {
        repo::change::record(
            db,
            id,
            None,
            &[NewChange {
                cause: repo::change::Cause::Refresh,
                scope: "item".into(),
                field: "externalIds".into(),
                language: String::new(),
                before: None,
                after: Some(ids),
                provider: Some("tmdb".into()),
                was_locked: false,
            }],
        )
        .await
        .expect("journaled");
    }

    /// What a statement reads of one column, `v`, in byte order: an
    /// engine's collation may set punctuation aside when it sorts.
    async fn texts(db: &Db, sql: &str, bound: &str) -> Vec<String> {
        let mut read: Vec<String> = sqlx::query(db.sql(sql))
            .bind(bound)
            .fetch_all(db.pool())
            .await
            .expect("read")
            .iter()
            .map(|row| row.text("v").expect("a text"))
            .collect();
        read.sort();
        read
    }

    fn found(
        pairs: &[FoundPair],
        a: &str,
        b: &str,
    ) -> Option<(Confidence, DuplicateReason, String)> {
        let (first, second) = pair_order(a, b);
        pairs
            .iter()
            .find(|pair| pair.first == first && pair.second == second)
            .map(|pair| (pair.confidence, pair.reason, pair.detail.clone()))
    }

    #[tokio::test]
    async fn duplicates_are_found_by_an_id_held_or_said_by_a_title_within_a_year_and_across_kinds()
    {
        let db = db().await;
        let office = work(&db, MediaKind::Series, "The Office", |w| {
            w.year = Some(2005);
            w.external_ids.tvdb = Some(73244);
        })
        .await;
        // Typed in by hand, without a year: the title alone.
        let typed = work(&db, MediaKind::Series, "the office!", |w| {
            w.is_manual = true
        })
        .await;
        // Another one under the same title, five years on, and a film.
        let later = work(&db, MediaKind::Series, "The Office", |w| {
            w.year = Some(2010);
            w.slug = "the-office-2010".into();
        })
        .await;
        let film = work(&db, MediaKind::Movie, "The Office", |w| w.year = Some(2005)).await;

        // A film holding an IMDb id, another film whose journal last said it
        // was its own — the refresh that took it left only the taker — and
        // a series that locks it.
        let heat = work(&db, MediaKind::Movie, "Heat", |w| {
            w.year = Some(1995);
            w.external_ids.tmdb = Some(949);
            w.external_ids.imdb = Some("tt0113277".into());
        })
        .await;
        let cut = work(&db, MediaKind::Movie, "Heat, the long cut", |w| {
            w.year = Some(1995);
            w.external_ids.tmdb = Some(950);
        })
        .await;
        said_ids(&db, &cut.id, json!({ "tmdb": 950, "imdb": "tt0113277" })).await;
        let series = work(&db, MediaKind::Series, "Heat: the series", |w| {
            w.year = Some(2001)
        })
        .await;
        repo::override_field::set(
            &db,
            &series.id,
            Scope::Item,
            "externalIds",
            Some(&json!({ "imdb": "tt0113277" })),
            None,
        )
        .await
        .expect("locked");

        let pairs = find_duplicates(&db).await.expect("looked for");
        assert_eq!(
            found(&pairs, &office.id, &typed.id),
            Some((
                Confidence::Probable,
                DuplicateReason::Title,
                "the office".into()
            ))
        );
        assert_eq!(
            found(&pairs, &office.id, &later.id),
            None,
            "five years apart"
        );
        assert!(
            found(&pairs, &typed.id, &later.id).is_some(),
            "a year unknown meets any"
        );
        assert_eq!(
            found(&pairs, &office.id, &film.id),
            None,
            "a film is no series"
        );
        assert_eq!(
            found(&pairs, &heat.id, &cut.id),
            Some((
                Confidence::Certain,
                DuplicateReason::SharedId,
                "imdb:tt0113277".into()
            ))
        );
        assert_eq!(
            found(&pairs, &heat.id, &series.id),
            Some((
                Confidence::Flag,
                DuplicateReason::ImdbAcrossKinds,
                "imdb:tt0113277".into()
            ))
        );
        assert_eq!(
            found(&pairs, &cut.id, &series.id).map(|f| f.0),
            Some(Confidence::Flag)
        );
    }

    #[tokio::test]
    async fn a_pair_somebody_dismissed_is_not_proposed_again_whatever_is_found() {
        let db = db().await;
        let one = work(&db, MediaKind::Movie, "Solaris", |w| w.year = Some(1972)).await;
        let two = work(&db, MediaKind::Movie, "Solaris", |w| {
            w.year = Some(1972);
            w.slug = "solaris-1972".into();
        })
        .await;

        sync_duplicates(&db).await.expect("looked for");
        let proposed = duplicates(&db, false).await.expect("listed");
        assert_eq!(proposed.len(), 1);
        assert_eq!(proposed[0].confidence, Confidence::Probable);

        dismiss_duplicate(&db, &two.id, &one.id, Some("user:admin"))
            .await
            .expect("dismissed");
        sync_duplicates(&db).await.expect("looked for again");
        assert!(duplicates(&db, false).await.expect("listed").is_empty());
        let all = duplicates(&db, true).await.expect("listed");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].dismissed_by.as_deref(), Some("user:admin"));

        // A pair no longer found is forgotten; one dismissed is remembered.
        let three = work(&db, MediaKind::Movie, "Stalker", |w| w.year = Some(1979)).await;
        let four = work(&db, MediaKind::Movie, "Stalker", |w| {
            w.year = Some(1979);
            w.slug = "stalker-1979".into();
        })
        .await;
        sync_duplicates(&db).await.expect("looked for");
        assert_eq!(duplicates(&db, false).await.expect("listed").len(), 1);
        sqlx::query(db.sql("UPDATE media_item SET title = 'Zerkalo', year = 1975 WHERE id = ?"))
            .bind(&four.id)
            .execute(db.pool())
            .await
            .expect("renamed");
        sync_duplicates(&db).await.expect("looked for");
        assert!(duplicates(&db, false).await.expect("listed").is_empty());
        assert_eq!(duplicates(&db, true).await.expect("listed").len(), 1);
        let _ = three;
    }

    /// The survivor and the merged work of the tests below: a season 1 each,
    /// the merged one a season 2 besides; an episode at one place each and
    /// one at its own; a poster at one address each and one of its own; one
    /// actor each and a director besides; translations into French each, and
    /// German besides; a lock of the overview each and one of the status;
    /// and selections.
    struct Pair {
        survivor: MediaItem,
        merged: MediaItem,
        both_in: String,
        merged_in: String,
    }

    async fn pair(db: &Db) -> Pair {
        let survivor = work(db, MediaKind::Series, "Twin Peaks", |w| {
            w.year = Some(1990);
            w.external_ids.tvdb = Some(70533);
            w.seasons = vec![season(1, "Season one, kept")];
            w.episodes = vec![episode(1, 1, "Pilot"), episode(1, 2, "Traces to Nowhere")];
            w.images = vec![poster("https://img.example/shared.jpg")];
            w.credits = vec![credit(CreditType::Actor, "Kyle MacLachlan", 6677)];
            w.alternative_titles = vec![other_title("Les Mystères de Twin Peaks")];
            w.translations = vec![translation("fra", "Twin Peaks, gardé")];
        })
        .await;
        let merged = work(db, MediaKind::Series, "Twin Peaks (typed)", |w| {
            w.year = Some(1990);
            w.slug = "twin-peaks-typed".into();
            w.external_ids.tmdb = Some(1920);
            w.external_ids.imdb = Some("tt0098936".into());
            w.seasons = vec![season(1, "Season one, given way"), season(2, "Season two")];
            w.episodes = vec![
                episode(1, 1, "Northwest Passage"),
                episode(2, 1, "May the Giant Be with You"),
            ];
            w.images = vec![
                poster("https://img.example/shared.jpg"),
                poster("https://img.example/own.jpg"),
            ];
            w.credits = vec![
                credit(CreditType::Actor, "Kyle MacLachlan", 6677),
                credit(CreditType::Director, "David Lynch", 5602),
            ];
            w.alternative_titles = vec![other_title("Twin Peaks: the series")];
            w.translations = vec![
                translation("fra", "Twin Peaks, cédé"),
                translation("deu", "Das Geheimnis von Twin Peaks"),
            ];
        })
        .await;

        // An episode added by hand to the merged work.
        sqlx::query(db.sql(
            "INSERT INTO media_episode (id, media_id, season_number, episode_number, title,
                                        is_manual, created_at, updated_at)
             VALUES (?, ?, 2, 2, 'Added by hand', 1, ?, ?)",
        ))
        .bind(crate::db::new_id())
        .bind(&merged.id)
        .bind(now())
        .bind(now())
        .execute(db.pool())
        .await
        .expect("added");

        for (id, field, value) in [
            (&survivor.id, "overview", json!("Mine.")),
            (&merged.id, "overview", json!("Theirs.")),
            (&merged.id, "status", json!("ended")),
        ] {
            repo::override_field::set(db, id, Scope::Item, field, Some(&value), Some("user:admin"))
                .await
                .expect("locked");
        }
        said_ids(db, &merged.id, json!({ "tmdb": 1920, "imdb": "tt0098936" })).await;

        let list = |slug: &'static str, members: Vec<String>| async move {
            repo::list::create(
                db,
                ListFields {
                    slug,
                    name: slug,
                    description: None,
                    kind: ListKind::Series,
                    mode: ListMode::Manual,
                    filter: None,
                    is_public: true,
                    cover: Some("https://img.example/own.jpg"),
                },
                Some(&members),
            )
            .await
            .expect("a selection")
            .id
        };
        let both_in = list("both", vec![merged.id.clone(), survivor.id.clone()]).await;
        let merged_in = list("merged", vec![merged.id.clone()]).await;

        Pair {
            survivor,
            merged,
            both_in,
            merged_in,
        }
    }

    async fn merge_plain(
        db: &Db,
        pair: &Pair,
        keep: &BTreeMap<String, Side>,
        taken: &BTreeMap<String, Value>,
    ) -> Merged {
        merge_works(
            db,
            &MergeInput {
                merged: &pair.merged.id,
                into: &pair.survivor.id,
                keep,
                taken,
                titles: &["Twin Peaks (typed)".to_string()],
                by: Some("user:admin"),
            },
        )
        .await
        .expect("merged")
    }

    #[tokio::test]
    async fn a_merge_folds_what_both_hold_into_the_survivors_and_moves_the_rest() {
        let db = db().await;
        let pair = pair(&db).await;
        let (s, m) = (&pair.survivor.id, &pair.merged.id);
        let done = merge_plain(&db, &pair, &BTreeMap::new(), &BTreeMap::new()).await;

        // Two season 1s are the survivor's; its season 2 is the merged work's.
        let seasons = texts(
            &db,
            "SELECT season_number || ':' || title AS v FROM media_season WHERE media_id = ? ORDER BY season_number",
            s,
        )
        .await;
        assert_eq!(seasons, ["1:Season one, kept", "2:Season two"]);
        let episodes = texts(
            &db,
            "SELECT season_number || 'x' || episode_number || ' ' || title AS v FROM media_episode
              WHERE media_id = ? ORDER BY season_number, episode_number",
            s,
        )
        .await;
        assert_eq!(
            episodes,
            [
                "1x1 Pilot",
                "1x2 Traces to Nowhere",
                "2x1 May the Giant Be with You",
                "2x2 Added by hand"
            ]
        );
        let images = texts(
            &db,
            "SELECT url AS v FROM media_image WHERE media_id = ? ORDER BY url",
            s,
        )
        .await;
        assert_eq!(
            images,
            [
                "https://img.example/own.jpg",
                "https://img.example/shared.jpg"
            ]
        );
        let credits = texts(
            &db,
            "SELECT credit_type || ' ' || person_name AS v FROM media_credit WHERE media_id = ? ORDER BY person_name",
            s,
        )
        .await;
        assert_eq!(credits, ["actor Kyle MacLachlan", "director David Lynch"]);
        let translations = texts(
            &db,
            "SELECT language || ' ' || title AS v FROM media_translation WHERE media_id = ? ORDER BY language",
            s,
        )
        .await;
        assert_eq!(
            translations,
            ["deu Das Geheimnis von Twin Peaks", "fra Twin Peaks, gardé"]
        );
        let others = texts(
            &db,
            "SELECT title AS v FROM media_alternative_title WHERE media_id = ? ORDER BY title",
            s,
        )
        .await;
        assert_eq!(
            others,
            [
                "Les Mystères de Twin Peaks",
                "Twin Peaks (typed)",
                "Twin Peaks: the series"
            ],
            "the merged work's title among the survivor's other titles"
        );

        // The survivor's lock stands where both locked; the merged work's
        // moves where only it did.
        let locks = texts(
            &db,
            "SELECT field || '=' || value AS v FROM media_override WHERE media_id = ? ORDER BY field",
            s,
        )
        .await;
        assert_eq!(locks, ["overview=\"Mine.\"", "status=\"ended\""]);

        // Its identifiers the survivor had none of are the survivor's; every
        // one of them, its id and its slug answer with the survivor.
        let ids = texts(
            &db,
            "SELECT source || ':' || value AS v FROM media_external_id WHERE media_id = ? ORDER BY source",
            s,
        )
        .await;
        assert_eq!(ids, ["imdb:tt0098936", "tmdb_tv:1920", "tvdb_series:70533"]);
        assert!(
            texts(
                &db,
                "SELECT value AS v FROM media_external_id WHERE media_id = ?",
                m
            )
            .await
            .is_empty()
        );
        for (source, value) in [
            ("id", m.as_str()),
            ("slug", "twin-peaks-typed"),
            ("tmdb_tv", "1920"),
            ("imdb", "tt0098936"),
        ] {
            assert_eq!(
                repo::alias::survivor(&db, source, value)
                    .await
                    .expect("read")
                    .as_deref(),
                Some(s.as_str()),
                "{source} {value}"
            );
        }
        assert_eq!(
            repo::alias::for_address(&db, m)
                .await
                .expect("read")
                .as_deref(),
            Some(s.as_str())
        );
        assert_eq!(
            repo::alias::for_address(&db, "twin-peaks-typed")
                .await
                .expect("read")
                .as_deref(),
            Some(s.as_str())
        );

        // The selections follow: the one it alone was in holds the survivor,
        // the one both were in holds it once; the cover is still a picture
        // the survivor holds.
        assert_eq!(
            repo::list::member_ids(&db, &pair.merged_in).await.unwrap(),
            std::slice::from_ref(s)
        );
        assert_eq!(
            repo::list::member_ids(&db, &pair.both_in).await.unwrap(),
            std::slice::from_ref(s)
        );
        let cover = repo::list::get(&db, &pair.merged_in)
            .await
            .unwrap()
            .unwrap()
            .cover;
        assert!(images.contains(cover.as_ref().expect("a cover")));

        // Its journal is the survivor's.
        let journal = texts(
            &db,
            "SELECT field AS v FROM media_change WHERE media_id = ? AND cause = 'refresh'",
            s,
        )
        .await;
        assert_eq!(journal, ["externalIds"]);

        // A tombstone: switched off, never due, the survivor named.
        let row = sqlx::query(
            db.sql("SELECT merged_into, is_enabled, refresh_after FROM media_item WHERE id = ?"),
        )
        .bind(m)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            row.opt_text("merged_into").unwrap().as_deref(),
            Some(s.as_str())
        );
        assert!(!row.flag("is_enabled").unwrap());
        assert_eq!(row.opt_text("refresh_after").unwrap(), None);
        assert_eq!(
            tombstone(&db, m).await.unwrap().map(|t| t.into),
            Some(s.clone())
        );

        // Found by what it now holds: the episode that came with it is the
        // survivor's in the search index, and the tombstone keeps none.
        let found = texts(
            &db,
            "SELECT work_id AS v FROM search_doc WHERE kind = 'episode' AND text = ?",
            "May the Giant Be with You",
        )
        .await;
        assert_eq!(found, std::slice::from_ref(s));
        let left = texts(
            &db,
            "SELECT ref_id AS v FROM search_doc WHERE kind = 'episode' AND work_id = ?",
            m,
        )
        .await;
        assert!(left.is_empty(), "{left:?}");

        // What gave way is in the record, whole.
        assert_eq!(done.record.dropped["media_season"].len(), 1);
        assert_eq!(done.record.dropped["media_episode"].len(), 1);
        assert_eq!(done.record.moved["media_episode"].len(), 2);
        assert_eq!(
            done.record.added_ids,
            [
                ("imdb".to_string(), "tt0098936".to_string()),
                ("tmdb_tv".to_string(), "1920".to_string())
            ]
        );
    }

    #[tokio::test]
    async fn a_field_kept_from_the_merged_work_is_the_survivors_lock() {
        let db = db().await;
        let pair = pair(&db).await;
        let s = &pair.survivor.id;
        let keep: BTreeMap<String, Side> = [
            ("overview".to_string(), Side::Merged),
            ("status".to_string(), Side::Survivor),
            ("tagline".to_string(), Side::Merged),
        ]
        .into();
        // What the merged work was served with, of a field it held no lock
        // on: the survivor is locked to it.
        let taken: BTreeMap<String, Value> = [
            ("overview".to_string(), json!("Theirs.")),
            (
                "tagline".to_string(),
                json!("She's dead, wrapped in plastic."),
            ),
        ]
        .into();
        let done = merge_plain(&db, &pair, &keep, &taken).await;

        let locks = texts(
            &db,
            "SELECT field || '=' || value AS v FROM media_override WHERE media_id = ? ORDER BY field",
            s,
        )
        .await;
        assert_eq!(
            locks,
            [
                "overview=\"Theirs.\"",
                "tagline=\"She's dead, wrapped in plastic.\""
            ],
            "the merged work's overview over the survivor's, its tagline as served, and its status \
             left behind"
        );
        assert_eq!(
            done.record.replaced.len(),
            1,
            "the survivor's own overview, kept to give back"
        );
    }

    #[tokio::test]
    async fn an_undo_puts_the_merged_work_back_and_leaves_what_changed_since() {
        let db = db().await;
        let pair = pair(&db).await;
        let (s, m) = (&pair.survivor.id, &pair.merged.id);
        merge_plain(&db, &pair, &BTreeMap::new(), &BTreeMap::new()).await;

        // Since the merge, somebody set the status the merge brought again —
        // a moment after, as a person does.
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        repo::override_field::set(
            &db,
            s,
            Scope::Item,
            "status",
            Some(&json!("continuing")),
            Some("user:editor"),
        )
        .await
        .expect("locked again");

        let undone = unmerge_work(&db, m, Some("user:admin"))
            .await
            .expect("undone");
        assert!(undone.taken.is_empty());

        // A work of its own again, and switched on, with everything back.
        assert_eq!(tombstone(&db, m).await.unwrap(), None);
        let seasons = texts(
            &db,
            "SELECT season_number || ':' || title AS v FROM media_season WHERE media_id = ? ORDER BY season_number",
            m,
        )
        .await;
        assert_eq!(seasons, ["1:Season one, given way", "2:Season two"]);
        let episodes = texts(
            &db,
            "SELECT season_number || 'x' || episode_number AS v FROM media_episode WHERE media_id = ?
              ORDER BY season_number, episode_number",
            m,
        )
        .await;
        assert_eq!(episodes, ["1x1", "2x1", "2x2"]);
        let survivor_episodes = texts(
            &db,
            "SELECT season_number || 'x' || episode_number AS v FROM media_episode WHERE media_id = ?
              ORDER BY season_number, episode_number",
            s,
        )
        .await;
        assert_eq!(survivor_episodes, ["1x1", "1x2"]);
        let ids = texts(
            &db,
            "SELECT source || ':' || value AS v FROM media_external_id WHERE media_id = ? ORDER BY source",
            m,
        )
        .await;
        assert_eq!(ids, ["imdb:tt0098936", "tmdb_tv:1920"]);
        let survivor_ids = texts(
            &db,
            "SELECT source || ':' || value AS v FROM media_external_id WHERE media_id = ? ORDER BY source",
            s,
        )
        .await;
        assert_eq!(survivor_ids, ["tvdb_series:70533"]);
        assert_eq!(repo::alias::for_address(&db, m).await.unwrap(), None);
        assert_eq!(
            repo::alias::survivor(&db, "tmdb_tv", "1920").await.unwrap(),
            None
        );

        // Its locks are its own; the survivor keeps the one set since.
        let locks = texts(
            &db,
            "SELECT field || '=' || value AS v FROM media_override WHERE media_id = ? ORDER BY field",
            m,
        )
        .await;
        assert_eq!(locks, ["overview=\"Theirs.\"", "status=\"ended\""]);
        let survivor_locks = texts(
            &db,
            "SELECT field || '=' || value AS v FROM media_override WHERE media_id = ? ORDER BY field",
            s,
        )
        .await;
        assert_eq!(
            survivor_locks,
            ["overview=\"Mine.\"", "status=\"continuing\""]
        );

        // Back in its selections; the title it lent the survivor gone.
        let mut both = repo::list::member_ids(&db, &pair.both_in).await.unwrap();
        both.sort();
        let mut expected = vec![m.clone(), s.clone()];
        expected.sort();
        assert_eq!(both, expected);
        assert_eq!(
            repo::list::member_ids(&db, &pair.merged_in).await.unwrap(),
            std::slice::from_ref(m)
        );
        let others = texts(
            &db,
            "SELECT title AS v FROM media_alternative_title WHERE media_id = ? ORDER BY title",
            s,
        )
        .await;
        assert_eq!(others, ["Les Mystères de Twin Peaks"]);
        let row =
            sqlx::query(db.sql("SELECT is_enabled, refresh_after FROM media_item WHERE id = ?"))
                .bind(m)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert!(row.flag("is_enabled").unwrap());
        assert!(
            row.opt_text("refresh_after").unwrap().is_some(),
            "due, to have its own again"
        );

        // Taken apart, the two are not proposed as one again.
        sync_duplicates(&db).await.unwrap();
        assert!(duplicates(&db, false).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_survivor_merged_in_turn_answers_for_both_and_one_deleted_takes_its_tombstones() {
        let db = db().await;
        let first = work(&db, MediaKind::Movie, "Alien", |w| {
            w.external_ids.tmdb = Some(1)
        })
        .await;
        let second = work(&db, MediaKind::Movie, "Alien (1979)", |w| {
            w.external_ids.tmdb = Some(2)
        })
        .await;
        let third = work(&db, MediaKind::Movie, "Alien, the original", |w| {
            w.external_ids.tmdb = Some(3)
        })
        .await;
        let merge = |merged: &str, into: &str| {
            let (merged, into) = (merged.to_string(), into.to_string());
            let db = db.clone();
            async move {
                merge_works(
                    &db,
                    &MergeInput {
                        merged: &merged,
                        into: &into,
                        keep: &BTreeMap::new(),
                        taken: &BTreeMap::new(),
                        titles: &[],
                        by: None,
                    },
                )
                .await
                .expect("merged");
            }
        };
        merge(&first.id, &second.id).await;
        merge(&second.id, &third.id).await;

        assert_eq!(
            repo::alias::for_address(&db, &first.id)
                .await
                .unwrap()
                .as_deref(),
            Some(third.id.as_str())
        );
        assert_eq!(
            repo::alias::for_external(&db, ExternalSource::TmdbMovie, "1")
                .await
                .unwrap()
                .as_deref(),
            Some(third.id.as_str())
        );
        assert!(
            unmerge_work(&db, &first.id, None).await.is_err(),
            "its survivor was merged in its turn"
        );

        // The survivor deleted, the works merged into it go too.
        repo::item::delete(&db, &third.id).await.unwrap();
        bury_tombstones(&db).await.unwrap();
        for gone in [&first.id, &second.id] {
            assert!(repo::item::get(&db, gone).await.unwrap().is_none());
        }
        assert_eq!(
            repo::alias::for_address(&db, &first.id).await.unwrap(),
            None
        );
    }
}
