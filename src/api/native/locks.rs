//! The locks — every manual edit — as a document that travels.
//!
//! Exported from one catalogue and imported into another, or kept as the one
//! part of a catalogue nobody can fetch again: a lock names its work by the
//! ids the work shares with the world, so it finds the same work wherever the
//! same series or film is held, whatever id that catalogue gave it.

use std::collections::{HashMap, HashSet};

use axum::{Extension, Json, extract::State};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{self, audit::Action},
    domain::{ExternalSource, MediaKind, fields, ids::normalize_imdb_id},
    error::{AppError, AppResult},
    service,
    state::AppState,
};

const TAG: &str = super::overrides::TAG;

/// The shape of the document; bumped when it changes in a way an older
/// server could not read.
const VERSION: u32 = 1;

/// The most locks one request takes. A request body is a megabyte at most,
/// so a long document is sent in parts — the interface does that on its own.
pub const MOST_LOCKS: usize = 2_000;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(export))
        .routes(routes!(import))
}

/// The work a lock is on, by every id it can be found by.
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LockedWork {
    /// The id here, which another catalogue will not share.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub kind: MediaKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tvdb: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imdb: Option<String>,
}

/// One lock: a field of a work, or of one of its seasons or episodes, and the
/// value it was set to.
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Lock {
    pub work: LockedWork,
    /// `item`, `season:3` or `episode:3x7`.
    pub scope: String,
    pub field: String,
    /// The value the field is locked to, or `null` where it was cleared and
    /// locked so. Left out altogether, the lock is refused: nothing is
    /// guessed on a document's behalf.
    #[serde(default, deserialize_with = "given", serialize_with = "given_or_null")]
    #[schema(value_type = Option<Object>)]
    pub value: Option<Option<Value>>,
}

/// Absent is `None`; `null` is `Some(None)`; a value is `Some(Some(_))`.
fn given<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<Value>>, D::Error> {
    Option::<Value>::deserialize(deserializer).map(Some)
}

fn given_or_null<S: Serializer>(
    value: &Option<Option<Value>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(Some(value)) => value.serialize(serializer),
        _ => serializer.serialize_none(),
    }
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Locks {
    pub version: u32,
    #[serde(default)]
    pub exported_at: String,
    pub locks: Vec<Lock>,
}

/// What an import did: how many locks it set, how many were already so,
/// on how many works — and which it could not set: on a work this catalogue
/// does not hold, or on a field, season or episode it does not know.
#[derive(Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = LocksImported)]
pub struct Imported {
    pub applied: usize,
    pub unchanged: usize,
    pub works: usize,
    /// The works named that are not held here, by title.
    pub unmatched: Vec<String>,
    /// The locks refused, each with why.
    pub refused: Vec<String>,
}

/// Every lock in the catalogue, as a document to keep or carry elsewhere.
#[utoipa::path(
    get, path = "/admin/locks", tag = TAG,
    responses(
        (status = 200, body = Locks),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn export(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Locks>> {
    require_admin(&identity)?;

    let overrides = repo::override_field::all(&state.db).await?;
    let mut ids: Vec<String> = overrides.iter().map(|(id, _)| id.clone()).collect();
    ids.sort_unstable();
    ids.dedup();

    let mut works = HashMap::new();
    for chunk in ids.chunks(500) {
        for work in repo::item::by_ids(&state.db, chunk).await? {
            works.insert(work.id.clone(), work);
        }
    }

    let locks = overrides
        .into_iter()
        .filter_map(|(id, o)| {
            let work = works.get(&id)?;
            Some(Lock {
                work: LockedWork {
                    id: Some(work.id.clone()),
                    kind: work.kind,
                    title: work.title.clone(),
                    year: work.year,
                    tmdb: work.external_ids.tmdb,
                    tvdb: work.external_ids.tvdb,
                    imdb: work.external_ids.imdb.clone(),
                },
                scope: o.scope,
                field: o.field,
                value: Some(o.value),
            })
        })
        .collect();

    Ok(Json(Locks {
        version: VERSION,
        exported_at: crate::db::now(),
        locks,
    }))
}

/// Set the locks of a document on the works held here. A work is found by
/// the id it was exported with when that is held, or failing that by its
/// TMDB, TheTVDB or IMDb id for a work of the same kind; a lock on a work
/// not held, or on a field, season or episode not known, is left out and
/// named in the answer, and one already set to the same value is left as it
/// is. What is set is set as an edit by hand is: it stays through every
/// refresh. At most two thousand locks a request; a longer document is sent
/// in parts.
#[utoipa::path(
    post, path = "/admin/locks", tag = TAG,
    request_body = Locks,
    responses(
        (status = 200, body = Imported),
        (status = 400, description = "Not a document this server reads, or too long for one request"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn import(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(document): Json<Locks>,
) -> AppResult<Json<Imported>> {
    require_admin(&identity)?;

    if document.version != VERSION {
        return Err(AppError::BadRequest(format!(
            "this server reads version {VERSION} of the locks document, not {}",
            document.version
        )));
    }
    if document.locks.len() > MOST_LOCKS {
        return Err(AppError::BadRequest(format!(
            "at most {MOST_LOCKS} locks in one request; send a longer document in parts"
        )));
    }

    let mut outcome = Outcome::default();
    let result = apply(&state, &identity, &document, &mut outcome).await;

    // Whatever happened, what was written is listed afresh and told of: a
    // request that stopped early leaves nothing half-done behind it.
    for id in &outcome.touched {
        service::listing::after_write(&state, id).await;
    }
    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::LocksImported,
            target: None,
            detail: Some(&format!(
                "{} set and {} already so on {} works; {} not held, {} refused{}",
                outcome.applied,
                outcome.unchanged,
                outcome.touched.len(),
                outcome.unmatched.len(),
                outcome.refused.len(),
                if result.is_err() {
                    " · stopped early"
                } else {
                    ""
                }
            )),
        },
    )
    .await;
    result?;

    Ok(Json(Imported {
        applied: outcome.applied,
        unchanged: outcome.unchanged,
        works: outcome.touched.len(),
        unmatched: outcome.unmatched,
        refused: outcome.refused,
    }))
}

#[derive(Default)]
struct Outcome {
    applied: usize,
    unchanged: usize,
    /// The works written to, each once, in the order they were met.
    touched: Vec<String>,
    unmatched: Vec<String>,
    refused: Vec<String>,
}

impl Outcome {
    fn touch(&mut self, id: &str) {
        if !self.touched.iter().any(|t| t == id) {
            self.touched.push(id.to_string());
        }
    }
}

/// The work a document names, as much of it as setting its locks needs.
struct Known {
    id: String,
    kind: MediaKind,
    title: String,
    seasons: HashSet<i32>,
    episodes: HashSet<(i32, i32)>,
    /// What is locked already, by scope and field, to tell a lock already
    /// set from one to set.
    current: HashMap<(String, String), Option<Value>>,
}

/// One key per work named, whatever it was named by.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct WorkKey {
    id: Option<String>,
    kind: MediaKind,
    tmdb: Option<i64>,
    tvdb: Option<i64>,
    imdb: Option<String>,
}

impl WorkKey {
    fn of(work: &LockedWork) -> Self {
        Self {
            id: work.id.clone().filter(|id| !id.is_empty()),
            kind: work.kind,
            tmdb: work.tmdb,
            tvdb: work.tvdb,
            imdb: work.imdb.as_deref().and_then(normalize_imdb_id),
        }
    }
}

async fn apply(
    state: &AppState,
    identity: &Identity,
    document: &Locks,
    outcome: &mut Outcome,
) -> AppResult<()> {
    // Each work is found once and read once, however many locks it has.
    let mut found: HashMap<WorkKey, Option<Known>> = HashMap::new();

    for lock in &document.locks {
        let key = WorkKey::of(&lock.work);
        if !found.contains_key(&key) {
            let known = know(state, &lock.work).await?;
            found.insert(key.clone(), known);
        }
        let Some(known) = found.get_mut(&key).and_then(Option::as_mut) else {
            let name = match lock.work.year {
                Some(year) => format!("{} ({year})", lock.work.title),
                None => lock.work.title.clone(),
            };
            if !outcome.unmatched.contains(&name) {
                outcome.unmatched.push(name);
            }
            continue;
        };

        let refuse = |outcome: &mut Outcome, why: &str| {
            outcome.refused.push(format!(
                "{} · {}/{}: {why}",
                known.title, lock.scope, lock.field
            ));
        };
        let Some(value) = &lock.value else {
            refuse(outcome, "no value; give one, or null to clear the field");
            continue;
        };
        let scope: fields::Scope = match lock.scope.parse() {
            Ok(scope) => scope,
            Err(e) => {
                refuse(outcome, &e.to_string());
                continue;
            }
        };
        if let Err(e) = fields::validate(scope, &lock.field, value.as_ref()) {
            refuse(outcome, &e);
            continue;
        }
        let present = match scope {
            fields::Scope::Item => true,
            fields::Scope::Season(n) => known.seasons.contains(&n),
            fields::Scope::Episode { season, episode } => {
                known.episodes.contains(&(season, episode))
            }
        };
        if !present {
            refuse(outcome, "not held here");
            continue;
        }

        let slot = (scope.to_string(), lock.field.clone());
        let unchanged = known
            .current
            .get(&slot)
            .is_some_and(|stored| stored == value);
        // The work's identity is written through to its row as an edit by
        // hand writes it — an adult work kept from every list at once — and
        // so even where the lock is already so: imported before, its row may
        // never have followed it.
        let of_identity =
            scope == fields::Scope::Item && fields::IDENTITY.contains(&lock.field.as_str());
        if unchanged && !of_identity {
            outcome.unchanged += 1;
            continue;
        }

        // Served afresh from here on, and the lists drawn before it dropped,
        // whether or not the request gets to the end of the document.
        match super::overrides::lock(
            state,
            (&known.id, known.kind),
            scope,
            &lock.field,
            value.as_ref(),
            &identity.label(),
        )
        .await
        {
            Ok(()) => {}
            // A slug or an identifier another work holds: this lock, not the
            // document.
            Err(AppError::Conflict(why)) => {
                refuse(outcome, &why);
                continue;
            }
            Err(e) => return Err(e),
        }
        known.current.insert(slot, value.clone());
        if unchanged {
            outcome.unchanged += 1;
        } else {
            outcome.applied += 1;
        }
        outcome.touch(&known.id);
    }
    Ok(())
}

/// The work held here that a document names, if it is held: by the id it
/// was exported with, then by the ids the world files it under — for a work
/// of the same kind, since a series and a film can share a number.
async fn know(state: &AppState, work: &LockedWork) -> AppResult<Option<Known>> {
    let Some(id) = resolve(state, work).await? else {
        return Ok(None);
    };
    let Some(item) = service::load(state, &id).await? else {
        return Ok(None);
    };
    if item.kind != work.kind {
        return Ok(None);
    }
    let current = repo::override_field::list(&state.db, &id)
        .await?
        .into_iter()
        .map(|o| ((o.scope, o.field), o.value))
        .collect();
    Ok(Some(Known {
        id: item.id,
        kind: item.kind,
        title: item.title,
        seasons: item.seasons.iter().map(|s| s.season_number).collect(),
        episodes: item
            .episodes
            .iter()
            .map(|e| (e.season_number, e.episode_number))
            .collect(),
        current,
    }))
}

async fn resolve(state: &AppState, work: &LockedWork) -> AppResult<Option<String>> {
    if let Some(id) = work.id.as_deref().filter(|id| !id.is_empty())
        && repo::item::get(&state.db, id).await?.is_some()
    {
        return Ok(Some(id.to_string()));
    }

    let mut candidates: Vec<(ExternalSource, String)> = Vec::new();
    if let Some(tmdb) = work.tmdb {
        candidates.push((ExternalSource::tmdb_for(work.kind), tmdb.to_string()));
    }
    if let Some(tvdb) = work.tvdb {
        candidates.push((ExternalSource::tvdb_for(work.kind), tvdb.to_string()));
    }
    if let Some(imdb) = work.imdb.as_deref().and_then(normalize_imdb_id) {
        candidates.push((ExternalSource::Imdb, imdb));
    }

    for (source, value) in candidates {
        let held =
            repo::item::held_external_ids(&state.db, source, std::slice::from_ref(&value)).await?;
        if let Some(id) = held.get(&value) {
            return Ok(Some(id.clone()));
        }
    }
    Ok(None)
}

fn require_admin(identity: &Identity) -> AppResult<()> {
    identity.is_admin().then_some(()).ok_or(AppError::Forbidden)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_round_trips_and_tells_a_cleared_lock_from_an_absent_value() {
        let document = Locks {
            version: VERSION,
            exported_at: "2026-09-25T21:00:00.000Z".into(),
            locks: vec![
                Lock {
                    work: LockedWork {
                        id: Some("01a0cff7-d8dc-70bc-a6b1-38379ee39bd0".into()),
                        kind: MediaKind::Series,
                        title: "Breaking Bad".into(),
                        year: Some(2008),
                        tmdb: Some(1396),
                        tvdb: Some(81189),
                        imdb: Some("tt0903747".into()),
                    },
                    scope: "item".into(),
                    field: "title".into(),
                    value: Some(Some(Value::String("Breaking Bad".into()))),
                },
                Lock {
                    work: LockedWork {
                        id: None,
                        kind: MediaKind::Movie,
                        title: "Heat".into(),
                        year: None,
                        tmdb: Some(949),
                        tvdb: None,
                        imdb: None,
                    },
                    scope: "item".into(),
                    field: "tagline".into(),
                    value: Some(None),
                },
            ],
        };
        let text = serde_json::to_string(&document).unwrap();
        // A cleared lock is written out as null, never left out.
        assert!(
            text.contains("\"field\":\"tagline\",\"value\":null"),
            "{text}"
        );

        let back: Locks = serde_json::from_str(&text).unwrap();
        assert_eq!(back.locks[0].work.tmdb, Some(1396));
        assert_eq!(
            back.locks[0].value,
            Some(Some(Value::String("Breaking Bad".into())))
        );
        assert_eq!(back.locks[1].value, Some(None), "null is a cleared lock");

        // Written by hand without a value: nothing to set, and not a clearing.
        let bare: Locks = serde_json::from_str(
            r#"{"version":1,"locks":[{"work":{"kind":"movie","title":"Heat","tmdb":949},"scope":"item","field":"overview"}]}"#,
        )
        .unwrap();
        assert_eq!(bare.locks[0].value, None);
        assert_eq!(bare.locks[0].work.id, None);
    }

    #[test]
    fn a_work_is_keyed_by_what_names_it_with_its_imdb_id_made_regular() {
        let named = |id: Option<&str>, imdb: Option<&str>| {
            WorkKey::of(&LockedWork {
                id: id.map(String::from),
                kind: MediaKind::Movie,
                title: "Heat".into(),
                year: None,
                tmdb: Some(949),
                tvdb: None,
                imdb: imdb.map(String::from),
            })
        };
        assert_eq!(
            named(None, Some("tt0113277")),
            named(Some(""), Some("0113277"))
        );
        assert_ne!(named(Some("a"), None), named(Some("b"), None));
    }

    /// A film, stored, by its title and TMDB id.
    async fn film(state: &AppState, title: &str, tmdb: i64) -> crate::domain::MediaItem {
        let mut work = crate::domain::MediaItem::empty(MediaKind::Movie);
        work.title = title.into();
        work.slug = crate::domain::make_slug(title, None);
        work.external_ids.tmdb = Some(tmdb);
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &work,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        work
    }

    /// What a reader the adult policy keeps from adult titles is listed.
    async fn listed(state: &AppState) -> Vec<String> {
        repo::item::search(
            &state.db,
            &repo::item::Query {
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .expect("listed")
        .into_iter()
        .map(|w| w.title)
        .collect()
    }

    #[tokio::test]
    async fn an_imported_identity_lock_is_written_to_the_row_every_list_reads() {
        let state = super::super::testing::server().await;
        let heat = film(&state, "Heat", 949).await;
        film(&state, "Ronin", 8195).await;
        let heat_lock = |field: &str, value: Value| {
            serde_json::json!({
                "work": { "kind": "movie", "title": "Heat", "tmdb": 949 },
                "scope": "item",
                "field": field,
                "value": value,
            })
        };
        let document = |locks: Vec<Value>| -> Locks {
            serde_json::from_value(serde_json::json!({ "version": 1, "locks": locks }))
                .expect("a document")
        };

        // Adult, and a slug another film has.
        let mut outcome = Outcome::default();
        apply(
            &state,
            &Identity::Anonymous,
            &document(vec![
                heat_lock("isAdult", Value::Bool(true)),
                heat_lock("slug", Value::String("ronin".into())),
            ]),
            &mut outcome,
        )
        .await
        .expect("imported");

        assert_eq!((outcome.applied, outcome.unchanged), (1, 0));
        assert_eq!(outcome.refused.len(), 1, "{:?}", outcome.refused);
        let row = repo::item::get(&state.db, &heat.id).await.unwrap().unwrap();
        // The row every list, the calendar, the feeds and the clients'
        // lookups read: the film is for adults there, at once.
        assert!(row.is_adult);
        assert_eq!(listed(&state).await, ["Ronin"]);
        // The slug stays its own, and the lock refused is not kept.
        assert_eq!(row.slug, "heat");
        let locks = repo::override_field::list(&state.db, &heat.id)
            .await
            .unwrap();
        assert_eq!(
            locks.iter().map(|l| l.field.as_str()).collect::<Vec<_>>(),
            ["isAdult"]
        );

        // Imported again over a row that never followed the lock — as an
        // import before this left it: written through all the same, and
        // counted as already so.
        sqlx::query(
            state
                .db
                .sql("UPDATE media_item SET is_adult = 0 WHERE id = ?"),
        )
        .bind(&heat.id)
        .execute(state.db.pool())
        .await
        .unwrap();
        let mut outcome = Outcome::default();
        apply(
            &state,
            &Identity::Anonymous,
            &document(vec![heat_lock("isAdult", Value::Bool(true))]),
            &mut outcome,
        )
        .await
        .expect("imported");
        assert_eq!((outcome.applied, outcome.unchanged), (0, 1));
        assert!(
            repo::item::get(&state.db, &heat.id)
                .await
                .unwrap()
                .unwrap()
                .is_adult
        );
        assert_eq!(listed(&state).await, ["Ronin"]);
    }
}
