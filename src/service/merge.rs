//! Two records of one work: the duplicates proposed, the merge of two into
//! one, its undo, and the ids a merge folded answered by the work that
//! survived — the rules around the transactions `repo::item::merge` makes.
//!
//! The survivor is the work its sources fill over one entered by hand, which
//! nothing refreshes: kept, the stub would hold what the sources said once,
//! for good. Then the work a person put something into by hand — a lock, an
//! episode, a picture — else the one filed under more identifiers elsewhere,
//! else the older, as TMDB keeps the oldest of two entries it merges; whoever
//! merges may choose the other. A field whose
//! value differs between the two is kept from the side chosen for it, the
//! survivor's unless said otherwise. Once merged, everything that named the
//! merged work answers with the survivor: its id and its slug on the site and
//! in the API (`service::load`), its identifiers for Sonarr, Radarr and Plex
//! (`series`, `movie`, the Plex surface's keys) — a client keeps the id it
//! was handed, and is answered under it.
//!
//! What a merge did is written to the survivor's journal (cause `merge`), to
//! the audit trail (`item.merged`, with what the survivor was and became),
//! and told to Sonarr or Radarr under both ids when `arr.pushRefresh` is on.
//! It can be undone for `merge.undoDays` days, two weeks unless set; `0`
//! keeps no undo.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use chrono::{TimeDelta, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use utoipa::ToSchema;

use crate::{
    arr::{
        App, Connection,
        history::{self, Outcome as Pushed},
    },
    db::{
        parse_rfc3339,
        repo::{
            self,
            audit::Action,
            change::{Cause, NewChange},
            item::{Confidence, Duplicate, DuplicateReason, MergedWork, Side, WorkCard},
        },
        to_rfc3339,
    },
    domain::{
        ExternalIds, ExternalSource, MediaItem, MediaKind,
        fields::{ITEM_FIELDS, Override, Scope},
    },
    merge::provenance::{self, Provenance, Watched},
    service::webhook,
    state::AppState,
};

/// The setting that says for how many days a merge can be undone.
pub const UNDO_DAYS: &str = "merge.undoDays";

/// Two weeks, unless it is set.
pub const DEFAULT_UNDO_DAYS: i64 = 14;

/// The fields of a work kept from one side or the other: every one a lock
/// may hold, but its slug and its identifiers, which the survivor keeps — the
/// merged work's become its aliases, and its identifiers the survivor's
/// where it has none of their source.
fn choosable(field: &str) -> bool {
    field != "slug" && field != "externalIds" && ITEM_FIELDS.iter().any(|def| def.name == field)
}

/// The fields whose value is an address the server may keep a copy of, and
/// serve under a path of its own: locked as the provider's address, as the
/// locks' own route locks them.
const ADDRESSES: &[&str] = &["themeMusic", "primaryPoster", "primaryFanart"];

/// For how many days a merge can be undone now: as set, two weeks unless it
/// is, never fewer than none.
pub fn undo_days(state: &AppState) -> i64 {
    state
        .settings
        .int_at(UNDO_DAYS, None, None)
        .unwrap_or(DEFAULT_UNDO_DAYS)
        .max(0)
}

/// Until when a merge made at `at` can be undone, with undos kept `days`
/// days; none when none is.
pub fn undo_until(at: &str, days: i64) -> Option<String> {
    if days <= 0 {
        return None;
    }
    parse_rfc3339(at)
        .and_then(|at| at.checked_add_signed(TimeDelta::days(days)))
        .map(to_rfc3339)
}

/// Why the survivor is the one the rule names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = SurvivorRule)]
pub enum Why {
    /// Its sources fill it, and the other was entered by hand, which
    /// nothing refreshes.
    Sourced,
    /// It holds what a person put in by hand, and the other does not.
    HandEdits,
    /// It is filed under more identifiers elsewhere.
    MoreIds,
    /// It is the older: the first of the two the catalogue held.
    Older,
}

/// How many identifiers elsewhere a work is filed under — Wikidata's aside,
/// which nothing is asked by.
fn id_count(card: &WorkCard) -> usize {
    card.external_ids
        .rows(card.kind)
        .iter()
        .filter(|(source, _)| *source != ExternalSource::Wikidata)
        .count()
}

/// What a person put into a work by hand, the work itself aside: a work
/// entered by hand counts as one, which [`Why::Sourced`] weighs first.
fn edits(card: &WorkCard) -> i64 {
    card.hand_edits - i64::from(card.is_manual)
}

/// Which of two works the rule keeps, and why.
pub fn proposed<'a>(a: &'a WorkCard, b: &'a WorkCard) -> (&'a WorkCard, Why) {
    let key = |card: &WorkCard| {
        (
            !card.is_manual,
            edits(card) > 0,
            id_count(card),
            std::cmp::Reverse(card.created_at.clone()),
            std::cmp::Reverse(card.id.clone()),
        )
    };
    let (kept, other) = if key(a) >= key(b) { (a, b) } else { (b, a) };
    let why = if kept.is_manual != other.is_manual {
        Why::Sourced
    } else if edits(kept) > 0 && edits(other) == 0 {
        Why::HandEdits
    } else if id_count(kept) > id_count(other) {
        Why::MoreIds
    } else {
        Why::Older
    };
    (kept, why)
}

/// Why a merge, or its preview, was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    NotFound,
    /// A work merged into itself.
    Itself,
    /// A film and a series: never one work, whatever they share.
    Kinds,
    /// One of the two was merged into another already.
    Merged {
        id: String,
        into: String,
    },
    /// A field that cannot be kept from the side chosen, and why.
    Field(String),
}

/// Why an undo was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Undo {
    /// No work by that id was merged into another.
    NotMerged,
    /// Undos are not kept: `merge.undoDays` is `0`.
    Off,
    /// The window closed at this instant.
    Expired { until: String },
    /// The work it was merged into is gone.
    SurvivorGone,
    /// The work it was merged into was merged in its turn, into this one:
    /// that merge is to be undone first.
    SurvivorMerged { into: String },
}

/// The work an identifier elsewhere answers with when no work holds it but
/// a merge filed it against the work that survived: that work, as it is
/// held, if it is of `kind`. `None` when a work holds the identifier — the
/// usual ladder answers for that one — and when no merge filed it.
pub async fn aliased(
    state: &AppState,
    kind: MediaKind,
    source: ExternalSource,
    value: &str,
) -> Result<Option<MediaItem>> {
    if repo::item::find_id_by_external(&state.db, source, value)
        .await?
        .is_some()
    {
        return Ok(None);
    }
    let Some(id) = repo::alias::for_external(&state.db, source, value).await? else {
        return Ok(None);
    };
    Ok(crate::service::load(state, &id)
        .await?
        .filter(|item| item.kind == kind))
}

// ─── the duplicates ─────────────────────────────────────────────────────────

/// A pair of duplicates as the catalogue shows it.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = DuplicateView)]
pub struct Pair {
    pub first: WorkCard,
    pub second: WorkCard,
    pub confidence: Confidence,
    pub reason: DuplicateReason,
    /// What they share: an identifier as `imdb:tt0903747`, or the title as
    /// it was folded.
    #[schema(required)]
    pub detail: Option<String>,
    pub found_at: String,
    /// The one of the two the rule would keep.
    pub survivor: String,
    pub why: Why,
}

/// Look for the duplicates again. A pair found anew or forgotten moves every
/// list on, as a write does: the register narrowed to duplicates is one of
/// them, and a page of it drawn before would be answered again as it was.
pub async fn look_again(state: &AppState) -> Result<()> {
    if repo::item::sync_duplicates(&state.db).await? {
        state.caches.bump_epoch().await;
    }
    Ok(())
}

/// The pairs proposed, looked for again first: those whose works this
/// reader may be shown — none for adults unless they may see those.
pub async fn duplicates(state: &AppState, include_adult: bool) -> Result<Vec<Pair>> {
    look_again(state).await?;
    let found: Vec<Duplicate> = repo::item::duplicates(&state.db, false).await?;
    let ids: Vec<String> = found
        .iter()
        .flat_map(|pair| [pair.first_id.clone(), pair.second_id.clone()])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut cards = repo::item::cards(&state.db, &ids).await?;
    for card in cards.values_mut() {
        card.poster = card.poster.as_deref().map(|url| state.media.localized(url));
    }

    let mut pairs = Vec::with_capacity(found.len());
    for pair in found {
        let (Some(first), Some(second)) = (cards.get(&pair.first_id), cards.get(&pair.second_id))
        else {
            continue;
        };
        if !include_adult && (first.is_adult || second.is_adult) {
            continue;
        }
        let (kept, why) = proposed(first, second);
        pairs.push(Pair {
            survivor: kept.id.clone(),
            why,
            first: first.clone(),
            second: second.clone(),
            confidence: pair.confidence,
            reason: pair.reason,
            detail: pair.detail,
            found_at: pair.found_at,
        });
    }
    Ok(pairs)
}

/// Say two works are not one.
pub async fn dismiss(
    state: &AppState,
    a: &str,
    b: &str,
    by: Option<&str>,
) -> Result<Result<(), Refusal>> {
    if a == b {
        return Ok(Err(Refusal::Itself));
    }
    let cards = repo::item::cards(&state.db, &[a.to_string(), b.to_string()]).await?;
    if cards.len() < 2 {
        return Ok(Err(Refusal::NotFound));
    }
    repo::item::dismiss_duplicate(&state.db, a, b, by).await?;
    // The two leave the register narrowed to duplicates: the pages of it
    // drawn so far are of the past.
    state.caches.bump_epoch().await;
    Ok(Ok(()))
}

// ─── the preview ────────────────────────────────────────────────────────────

/// One field whose value differs between the two, and whether each locks it.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = MergeField)]
pub struct FieldChoice {
    /// As the field registry names it.
    pub field: String,
    /// What the survivor is served with; `null` for nothing.
    #[schema(required)]
    pub survivor: Option<Value>,
    /// What the merged work is served with; `null` for nothing.
    #[schema(required)]
    pub merged: Option<Value>,
    pub survivor_locked: bool,
    pub merged_locked: bool,
    /// Whether the merged work's value can be kept: a field a work cannot be
    /// read without — its title, the poster it leads with — is never kept
    /// empty.
    pub choosable: bool,
}

/// A name the survivor will answer for besides its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = MergeAlias)]
pub struct AliasName {
    /// `id`, `slug`, or the source of an identifier elsewhere.
    pub source: String,
    pub value: String,
}

/// What a merge of one work into another would do.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = MergePreview)]
pub struct Preview {
    /// The work that would become a tombstone.
    pub merged: WorkCard,
    /// The work that would stay.
    pub survivor: WorkCard,
    /// The one of the two the rule would keep, and why: the dialog offers to
    /// swap when it is not `survivor`.
    pub proposed: String,
    pub why: Why,
    /// Every field whose value differs, as each is served.
    pub fields: Vec<FieldChoice>,
    /// What the merged work holds, by kind of row: moved over, or folded
    /// into the survivor's own.
    pub children: BTreeMap<String, i64>,
    /// What the survivor will answer for besides its own.
    pub aliases: Vec<AliasName>,
    /// For how many days the merge could be undone; `0`: not at all.
    pub undo_days: i64,
}

/// Both works, checked: works of their own, of one kind, two of them.
async fn pair_of(
    state: &AppState,
    merged: &str,
    into: &str,
) -> Result<Result<(WorkCard, WorkCard), Refusal>> {
    if merged == into {
        return Ok(Err(Refusal::Itself));
    }
    for id in [merged, into] {
        if let Some(tombstone) = repo::item::tombstone(&state.db, id).await? {
            return Ok(Err(Refusal::Merged {
                id: id.to_string(),
                into: tombstone.into,
            }));
        }
    }
    let mut cards = repo::item::cards(&state.db, &[merged.to_string(), into.to_string()]).await?;
    let (Some(mut gone), Some(mut kept)) = (cards.remove(merged), cards.remove(into)) else {
        return Ok(Err(Refusal::NotFound));
    };
    if gone.kind != kept.kind {
        return Ok(Err(Refusal::Kinds));
    }
    for card in [&mut gone, &mut kept] {
        card.poster = card.poster.as_deref().map(|url| state.media.localized(url));
    }
    Ok(Ok((gone, kept)))
}

/// A work as it is served, with its locks: what each side of the dialog
/// shows, and what a field kept from it is locked to.
async fn served(state: &AppState, id: &str) -> Result<Option<(MediaItem, Vec<Override>)>> {
    let Some(item) = crate::service::load(state, id).await? else {
        return Ok(None);
    };
    // Loaded by an id that is not its own: merged meanwhile.
    if item.id != id {
        return Ok(None);
    }
    let locks = repo::override_field::list(&state.db, id).await?;
    Ok(Some((item, locks)))
}

fn locked(locks: &[Override], field: &str) -> bool {
    locks
        .iter()
        .any(|lock| lock.scope == "item" && lock.field == field && lock.language.is_none())
}

/// The value a field is locked to when it is kept from the merged work: the
/// one it is served with, a copy kept here named by its provider's address,
/// and checked as a lock is.
fn taken(state: &AppState, field: &str, value: Option<Value>) -> Result<Value, String> {
    let value = match value {
        Some(Value::String(address)) if ADDRESSES.contains(&field) => {
            Some(Value::String(state.media.unlocalize(&address)))
        }
        other => other,
    };
    crate::domain::fields::validate(Scope::Item, field, value.as_ref())?;
    Ok(value.unwrap_or(Value::Null))
}

/// What merging `merged` into `into` would do.
pub async fn preview(
    state: &AppState,
    merged: &str,
    into: &str,
) -> Result<Result<Preview, Refusal>> {
    let (gone, kept) = match pair_of(state, merged, into).await? {
        Ok(pair) => pair,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let (Some((theirs, their_locks)), Some((ours, our_locks))) =
        (served(state, merged).await?, served(state, into).await?)
    else {
        return Ok(Err(Refusal::NotFound));
    };

    let mut fields = Vec::new();
    for def in ITEM_FIELDS {
        if !choosable(def.name) {
            continue;
        }
        let survivor = provenance::value_at(&ours, "item", def.name, "");
        let merged_value = provenance::value_at(&theirs, "item", def.name, "");
        if provenance::same(survivor.as_ref(), merged_value.as_ref()) {
            continue;
        }
        fields.push(FieldChoice {
            field: def.name.to_string(),
            choosable: taken(state, def.name, merged_value.clone()).is_ok(),
            survivor,
            merged: merged_value,
            survivor_locked: locked(&our_locks, def.name),
            merged_locked: locked(&their_locks, def.name),
        });
    }

    let mut aliases = vec![
        AliasName {
            source: repo::alias::ID.into(),
            value: gone.id.clone(),
        },
        AliasName {
            source: repo::alias::SLUG.into(),
            value: gone.slug.clone(),
        },
    ];
    aliases.extend(
        gone.external_ids
            .rows(gone.kind)
            .into_iter()
            .map(|(source, value)| AliasName {
                source: source.as_str().to_string(),
                value,
            }),
    );

    let (rule, why) = proposed(&gone, &kept);
    Ok(Ok(Preview {
        proposed: rule.id.clone(),
        why,
        fields,
        children: repo::item::child_counts(&state.db, merged).await?,
        aliases,
        undo_days: undo_days(state),
        merged: gone,
        survivor: kept,
    }))
}

// ─── the merge ──────────────────────────────────────────────────────────────

/// What a merge came to.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = MergeOutcome)]
pub struct Outcome {
    /// The work that stays.
    pub survivor: String,
    /// The work merged into it, a tombstone now.
    pub merged: String,
    /// Until when it can be undone; `null` when it cannot.
    #[schema(required)]
    pub undo_until: Option<String>,
    /// How many names of the merged work the survivor answers for now.
    pub aliases: usize,
    /// The rows moved over, by table.
    pub moved: BTreeMap<String, usize>,
    /// The rows that gave way to the survivor's, by table.
    pub dropped: BTreeMap<String, usize>,
}

/// Who asked, and from where: the journal, the trail and the clients told
/// name them.
pub struct Asker {
    pub by: Option<String>,
    pub ip: Option<String>,
}

/// The merged work's names the survivor goes by none of: its title, and its
/// original title — kept among the survivor's other titles, so that a search
/// for either still finds the work.
fn names_kept(gone: &MediaItem, kept: &MediaItem) -> Vec<String> {
    let held: BTreeSet<String> = [Some(&kept.title), kept.original_title.as_ref()]
        .into_iter()
        .flatten()
        .chain(kept.sort_title.as_ref())
        .chain(kept.alternative_titles.iter().map(|alt| &alt.title))
        .map(|name| crate::search::norm(name))
        .collect();
    let mut names: Vec<String> = Vec::new();
    for name in [Some(&gone.title), gone.original_title.as_ref()]
        .into_iter()
        .flatten()
    {
        let folded = crate::search::norm(name);
        if folded.is_empty()
            || held.contains(&folded)
            || names.iter().any(|n| crate::search::norm(n) == folded)
        {
            continue;
        }
        names.push(name.trim().to_string());
    }
    names
}

/// Merge `merged` into `into`, the fields of `keep` kept from the side
/// chosen for each.
pub async fn merge(
    state: &AppState,
    asker: &Asker,
    merged: &str,
    into: &str,
    keep: BTreeMap<String, Side>,
) -> Result<Result<Outcome, Refusal>> {
    let (gone_card, kept_card) = match pair_of(state, merged, into).await? {
        Ok(pair) => pair,
        Err(refusal) => return Ok(Err(refusal)),
    };
    if let Some(field) = keep.keys().find(|field| !choosable(field)) {
        return Ok(Err(Refusal::Field(format!(
            "{field:?} is not kept from either side: the survivor keeps its own"
        ))));
    }
    let (Some((gone, _)), Some((before, _))) =
        (served(state, merged).await?, served(state, into).await?)
    else {
        return Ok(Err(Refusal::NotFound));
    };

    let mut values: BTreeMap<String, Value> = BTreeMap::new();
    for (field, side) in &keep {
        if *side != Side::Merged {
            continue;
        }
        match taken(state, field, provenance::value_at(&gone, "item", field, "")) {
            Ok(value) => {
                values.insert(field.clone(), value);
            }
            Err(why) => {
                return Ok(Err(Refusal::Field(format!(
                    "the merged work's {field} cannot be kept: {why}"
                ))));
            }
        }
    }
    let titles = names_kept(&gone, &before);

    let done = repo::item::merge_works(
        &state.db,
        &repo::item::MergeInput {
            merged,
            into,
            keep: &keep,
            taken: &values,
            titles: &titles,
            by: asker.by.as_deref(),
        },
    )
    .await?;
    let record = done.record;

    for id in [into, merged] {
        state.caches.touched(id).await;
    }
    crate::service::listing::after_write(state, into).await;
    if let Err(e) = repo::search::index_people(&state.db, &done.people).await {
        tracing::warn!(
            error = format_args!("{e:#}"),
            "could not write the people of a merged work into the search index"
        );
    }

    let after = crate::service::load(state, into).await?;
    if let Some(after) = &after {
        journal(
            state,
            asker,
            &before,
            after,
            Cause::Merge,
            merged_row(&gone, true),
        )
        .await;
    }

    let detail = json!({
        "merged": {
            "id": gone.id,
            "title": gone.title,
            "kind": gone.kind,
            "year": gone.year,
            "slug": gone.slug,
            "externalIds": gone.external_ids,
        },
        "into": { "id": before.id, "title": before.title },
        "before": {
            "externalIds": before.external_ids,
            "fields": kept_values(&before, &keep),
        },
        "after": {
            "externalIds": after.as_ref().map(|after| &after.external_ids),
            "fields": after.as_ref().map(|after| kept_values(after, &keep)),
        },
        "keep": keep,
        "moved": counted(&record.moved),
        "dropped": counted(&record.dropped),
    });
    trail(
        state,
        asker,
        Action::ItemMerged,
        merged,
        &detail.to_string(),
        &format!("into \"{}\"", before.title),
    )
    .await;

    // The survivor under its ids as they stand now, the merged work under
    // the one it was handed before.
    let survivor_ids = after.as_ref().map_or_else(
        || kept_card.external_ids.clone(),
        |after| after.external_ids.clone(),
    );
    tell_arr(
        state,
        before.kind,
        vec![
            (
                into.to_string(),
                before.title.clone(),
                client_id(before.kind, into, kept_card.is_manual, &survivor_ids),
            ),
            (
                merged.to_string(),
                gone.title.clone(),
                client_id(
                    gone_card.kind,
                    merged,
                    gone_card.is_manual,
                    &gone_card.external_ids,
                ),
            ),
        ],
    );

    let days = undo_days(state);
    Ok(Ok(Outcome {
        survivor: into.to_string(),
        merged: merged.to_string(),
        undo_until: undo_until(&record.at, days),
        aliases: 2 + record.ids.len(),
        moved: counted(&record.moved),
        dropped: counted(&record.dropped),
    }))
}

/// How many rows of each table a record names.
fn counted<T>(by_table: &BTreeMap<String, Vec<T>>) -> BTreeMap<String, usize> {
    by_table
        .iter()
        .map(|(table, rows)| (table.clone(), rows.len()))
        .collect()
}

/// The values a work is served with of the fields chosen: what the trail
/// says the survivor was, and became.
fn kept_values(item: &MediaItem, keep: &BTreeMap<String, Side>) -> BTreeMap<String, Value> {
    keep.keys()
        .map(|field| {
            (
                field.clone(),
                provenance::value_at(item, "item", field, "").unwrap_or(Value::Null),
            )
        })
        .collect()
}

/// The id Sonarr or Radarr keeps a work under: TheTVDB's, or the one made
/// for a series it does not list; TMDB's for a film; for a work entered by
/// hand that nothing elsewhere names, the number made of its catalogue id.
fn client_id(kind: MediaKind, id: &str, is_manual: bool, ids: &ExternalIds) -> Option<i64> {
    crate::service::ids::for_client(kind, id, is_manual, ids)
}

/// The journal's line for the record merged in, or taken out again: its
/// title and year, said where nothing else of the journal would say it.
fn merged_row(gone: &MediaItem, into: bool) -> NewChange {
    let named = match gone.year {
        Some(year) => format!("{} ({year})", gone.title),
        None => gone.title.clone(),
    };
    let named = Some(Value::String(named));
    NewChange {
        cause: if into { Cause::Merge } else { Cause::Revert },
        scope: "item".into(),
        field: "merged".into(),
        language: String::new(),
        before: if into { None } else { named.clone() },
        after: if into { named } else { None },
        provider: None,
        was_locked: false,
    }
}

/// Write to a work's journal what a merge, or its undo, changed in it as it
/// is served: `first` before the rest. Best effort, as the trail is: the
/// work is written already.
async fn journal(
    state: &AppState,
    asker: &Asker,
    before: &MediaItem,
    after: &MediaItem,
    cause: Cause,
    first: NewChange,
) {
    let locks = repo::override_field::list(&state.db, &after.id)
        .await
        .unwrap_or_default();
    let mut changes = vec![first];
    changes.extend(
        provenance::changes(
            &Watched::of(before),
            &Watched::of(after),
            None,
            &Provenance::default(),
        )
        .into_iter()
        .map(|change| {
            let held = locks.iter().any(|lock| {
                lock.scope == change.scope && lock.field == change.field && lock.language.is_none()
            });
            NewChange::found(change, cause, held)
        }),
    );
    if let Err(e) = repo::change::record(&state.db, &after.id, asker.by.as_deref(), &changes).await
    {
        tracing::warn!(id = %after.id, error = format_args!("{e:#}"), "could not write a merge to the work's journal");
    }
}

/// Write the trail's entry — `detail` whole, the before and the after of
/// the survivor in it — and tell the notifications that want it, shortly.
async fn trail(
    state: &AppState,
    asker: &Asker,
    action: Action,
    target: &str,
    detail: &str,
    said: &str,
) {
    let written = repo::audit::record(
        &state.db,
        repo::audit::Record {
            actor: asker.by.as_deref(),
            action,
            target: Some(target),
            detail: Some(detail),
            ip: asker.ip.as_deref(),
        },
    )
    .await;
    if let Err(e) = written {
        tracing::warn!(action = %action, error = %e, "could not write the audit entry");
    }
    webhook::notify(
        state,
        webhook::Happening {
            action,
            actor: asker.by.clone(),
            target: Some(target.to_string()),
            detail: Some(said.chars().take(200).collect()),
        },
    )
    .await;
}

/// Tell Sonarr or Radarr, when telling is on, to read both works again under
/// the ids it keeps them by: the survivor's own, and the one the merged work
/// was handed, which it answers under now. Done in the background: a client
/// slow to answer holds up nothing, and what came of it is written down for
/// the Sonarr & Radarr page.
fn tell_arr(state: &AppState, kind: MediaKind, works: Vec<(String, String, Option<i64>)>) {
    if !state.flag(crate::arr::push::SETTING, false) {
        return;
    }
    let Some(connection) = Connection::configured(state, App::of(kind)) else {
        return;
    };
    /// What came of telling one of the two, as the page lists it.
    struct Told {
        id: String,
        title: String,
        client: Option<i64>,
        arr: Option<i64>,
        outcome: Pushed,
        detail: Option<String>,
    }

    let state = state.clone();
    tokio::spawn(async move {
        let reasons = vec!["merge".to_string()];
        let mut rows: Vec<Told> = Vec::new();
        let mut found: Vec<Told> = Vec::new();
        for (id, title, client) in works {
            let mut told = Told {
                id,
                title,
                client,
                arr: None,
                outcome: Pushed::Absent,
                detail: None,
            };
            let Some(client) = client else {
                rows.push(told);
                continue;
            };
            match connection.find(client).await {
                Ok(Some(held)) => {
                    told.arr = Some(held.id);
                    found.push(told);
                }
                Ok(None) => rows.push(told),
                Err(e) => {
                    told.outcome = Pushed::Failed;
                    told.detail = Some(crate::providers::clip(&format!("{e:#}"), 300));
                    rows.push(told);
                }
            }
        }
        if !found.is_empty() {
            let ids: Vec<i64> = found.iter().filter_map(|told| told.arr).collect();
            let (outcome, said) = match connection.refresh(&ids).await {
                Ok(command) => (Pushed::Sent, command.map(|c| format!("command {c}"))),
                Err(e) => (
                    Pushed::Failed,
                    Some(crate::providers::clip(&format!("{e:#}"), 300)),
                ),
            };
            for mut told in found {
                told.outcome = outcome;
                told.detail.clone_from(&said);
                rows.push(told);
            }
        }
        let told: Vec<history::Told<'_>> = rows
            .iter()
            .map(|told| history::Told {
                app: connection.app,
                media_id: &told.id,
                title: &told.title,
                client_id: told.client,
                arr_id: told.arr,
                outcome: told.outcome,
                reasons: &reasons,
                detail: told.detail.as_deref(),
            })
            .collect();
        if let Err(e) = history::add(&state.db, &told).await {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "could not write down what was told of a merge"
            );
        }
    });
}

// ─── the undo ───────────────────────────────────────────────────────────────

/// An identifier the work put back went without: another work holds it now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = UnmergeTaken)]
pub struct TakenId {
    pub source: String,
    pub value: String,
    /// The work that holds it.
    pub holder: String,
}

/// What an undo came to.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = UnmergeOutcome)]
pub struct Unmerge {
    /// The work put back, a work of its own again.
    pub restored: String,
    /// The work it had been merged into.
    pub survivor: String,
    /// The identifiers it went without.
    pub taken: Vec<TakenId>,
}

/// Undo the merge `merged` went into, while it may be.
pub async fn unmerge(
    state: &AppState,
    asker: &Asker,
    merged: &str,
) -> Result<Result<Unmerge, Undo>> {
    let Some(tombstone) = repo::item::tombstone(&state.db, merged).await? else {
        return Ok(Err(Undo::NotMerged));
    };
    let days = undo_days(state);
    let Some(until) = undo_until(&tombstone.at, days) else {
        return Ok(Err(Undo::Off));
    };
    if parse_rfc3339(&until).is_none_or(|until| until < Utc::now()) {
        return Ok(Err(Undo::Expired { until }));
    }
    if let Some(further) = repo::item::tombstone(&state.db, &tombstone.into).await? {
        return Ok(Err(Undo::SurvivorMerged { into: further.into }));
    }
    let Some((before, _)) = served(state, &tombstone.into).await? else {
        return Ok(Err(Undo::SurvivorGone));
    };

    let done = repo::item::unmerge_work(&state.db, merged, asker.by.as_deref()).await?;

    for id in [tombstone.into.as_str(), merged] {
        state.caches.touched(id).await;
        crate::service::listing::after_write(state, id).await;
    }
    if let Err(e) = repo::search::index_people(&state.db, &done.people).await {
        tracing::warn!(
            error = format_args!("{e:#}"),
            "could not write the people of a work taken apart into the search index"
        );
    }

    let restored = crate::service::load(state, merged).await?;
    let after = crate::service::load(state, &tombstone.into).await?;
    if let (Some(restored), Some(after)) = (&restored, &after) {
        journal(
            state,
            asker,
            &before,
            after,
            Cause::Revert,
            merged_row(restored, false),
        )
        .await;
        if let Err(e) = repo::change::record(
            &state.db,
            merged,
            asker.by.as_deref(),
            &[merged_row(after, false)],
        )
        .await
        {
            tracing::warn!(id = %merged, error = format_args!("{e:#}"), "could not write an undo to the work's journal");
        }
    }

    let taken: Vec<TakenId> = done
        .taken
        .into_iter()
        .map(|(source, value, holder)| TakenId {
            source,
            value,
            holder,
        })
        .collect();
    let detail = json!({
        "restored": merged,
        "from": tombstone.into,
        "mergedAt": tombstone.at,
        "taken": taken,
    });
    trail(
        state,
        asker,
        Action::ItemUnmerged,
        merged,
        &detail.to_string(),
        &format!("out of \"{}\"", before.title),
    )
    .await;

    if let Some(restored) = &restored {
        let mut ids = ExternalIds::default();
        for (source, value) in &done.record.ids {
            if let Ok(source) = source.parse::<ExternalSource>() {
                ids.apply(source, value);
            }
        }
        tell_arr(
            state,
            restored.kind,
            vec![
                (
                    tombstone.into.clone(),
                    before.title.clone(),
                    after.as_ref().and_then(|after| {
                        client_id(after.kind, &after.id, after.is_manual, &after.external_ids)
                    }),
                ),
                (
                    merged.to_string(),
                    restored.title.clone(),
                    client_id(restored.kind, merged, restored.is_manual, &ids),
                ),
            ],
        );
    }

    Ok(Ok(Unmerge {
        restored: merged.to_string(),
        survivor: tombstone.into,
        taken,
    }))
}

/// A merge into a work that can still be undone, as its editor offers it.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = MergeUndoable)]
pub struct Undoable {
    pub work: MergedWork,
    pub undo_until: String,
}

/// The merges into `into` that can still be undone, newest first: none
/// while undos are not kept.
pub async fn undoable(state: &AppState, into: &str) -> Result<Vec<Undoable>> {
    let days = undo_days(state);
    if days == 0 {
        return Ok(Vec::new());
    }
    let since = to_rfc3339(Utc::now() - TimeDelta::days(days));
    Ok(repo::item::merged_into(&state.db, into, &since)
        .await?
        .into_iter()
        .filter_map(|work| {
            Some(Undoable {
                undo_until: undo_until(&work.merged_at, days)?,
                work,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, created_at: &str, shape: impl FnOnce(&mut WorkCard)) -> WorkCard {
        let mut card = WorkCard {
            id: id.into(),
            title: "Heat".into(),
            kind: MediaKind::Movie,
            year: Some(1995),
            slug: id.into(),
            poster: None,
            external_ids: ExternalIds::default(),
            is_manual: false,
            is_enabled: true,
            is_adult: false,
            created_at: created_at.into(),
            hand_edits: 0,
        };
        shape(&mut card);
        card
    }

    /// A stub typed in by hand gives way to a record its sources fill,
    /// whatever was put into it; between two of a kind, hand edits, then
    /// identifiers, then age decide.
    #[test]
    fn the_rule_keeps_the_sourced_record_then_the_edited_then_the_better_filed_then_the_older() {
        let stub = card("stub", "2026-01-01T00:00:00Z", |c| {
            c.is_manual = true;
            c.hand_edits = 3;
        });
        let filed = card("filed", "2026-06-01T00:00:00Z", |c| {
            c.external_ids.tmdb = Some(949);
        });
        for (a, b) in [(&stub, &filed), (&filed, &stub)] {
            let (kept, why) = proposed(a, b);
            assert_eq!((kept.id.as_str(), why), ("filed", Why::Sourced));
        }

        let edited = card("edited", "2026-06-02T00:00:00Z", |c| {
            c.external_ids.tmdb = Some(950);
            c.hand_edits = 1;
        });
        let (kept, why) = proposed(&filed, &edited);
        assert_eq!((kept.id.as_str(), why), ("edited", Why::HandEdits));

        let better = card("better", "2026-06-03T00:00:00Z", |c| {
            c.external_ids.tmdb = Some(951);
            c.external_ids.imdb = Some("tt0113277".into());
        });
        let (kept, why) = proposed(&filed, &better);
        assert_eq!((kept.id.as_str(), why), ("better", Why::MoreIds));

        let twin = card("twin", "2026-07-01T00:00:00Z", |c| {
            c.external_ids.tmdb = Some(952);
        });
        let (kept, why) = proposed(&twin, &filed);
        assert_eq!((kept.id.as_str(), why), ("filed", Why::Older));

        // Two typed in by hand: what was put into them, then age.
        let bare = card("bare", "2026-02-01T00:00:00Z", |c| {
            c.is_manual = true;
            c.hand_edits = 1;
        });
        let (kept, why) = proposed(&bare, &stub);
        assert_eq!((kept.id.as_str(), why), ("stub", Why::HandEdits));
    }
}
