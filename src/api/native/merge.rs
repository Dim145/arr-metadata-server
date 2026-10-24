//! Duplicates, and the merge of two records of one work into one.
//!
//! The pairs that look like one work are listed for whoever keeps the
//! catalogue, each with why and how sure, and the one of the two the rule
//! would keep; a pair that is not one is dismissed, and not proposed again.
//! A merge is previewed before it is made — every field whose value differs,
//! what the merged work holds, the names the survivor will answer for — then
//! made, and undone while `merge.undoDays` allows. See `service::merge`.

use std::collections::BTreeMap;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{self, alias::Alias, audit::Action, item::Side},
    error::{AppError, AppResult},
    service::merge::{self, Asker, Outcome, Pair, Preview, Refusal, Undo, Undoable, Unmerge},
    state::AppState,
};

/// Filed with the rest of the catalogue's routes.
const TAG: &str = super::items::TAG;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(duplicates))
        .routes(routes!(dismiss))
        .routes(routes!(preview, merge_into))
        .routes(routes!(unmerge))
        .routes(routes!(merges))
}

/// What a refusal is told as.
fn refused(refusal: Refusal) -> AppError {
    match refusal {
        Refusal::NotFound => AppError::NotFound,
        Refusal::Itself => AppError::BadRequest("a work is not merged into itself".into()),
        Refusal::Kinds => AppError::Conflict(
            "a film and a series are never one work: one of them is filed under an identifier \
             that is not its own"
                .into(),
        ),
        Refusal::Merged { id, into } => {
            AppError::Conflict(format!("{id} was merged into {into} already"))
        }
        Refusal::Field(why) => AppError::BadRequest(why),
    }
}

/// Who asks, as the journal and the trail name them.
fn asker(identity: &Identity, ip: &ClientIp) -> Asker {
    Asker {
        by: Some(identity.label()),
        ip: ip.as_text(),
    }
}

/// The pairs proposed.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = DuplicateList)]
pub struct DuplicateList {
    /// Surest first: an identifier shared, then a title, then an IMDb id on
    /// a film and a series.
    pub pairs: Vec<Pair>,
}

/// The works that look like one, looked for again first.
///
/// `certain`: two works of one kind filed under one identifier elsewhere —
/// TMDB's, TheTVDB's, IMDb's, AniList's, MyAnimeList's or Fankai's — held by
/// one, or last said by the other's journal or lock to be its own.
/// `probable`: two works of one kind under one title, as the search index
/// folds it, a year apart at the most or with a year unknown. `flag`: a film
/// and a series under one IMDb id, which are never merged. A pair somebody
/// dismissed is not listed. For those who may edit the catalogue; works for
/// adults as this caller may see them.
#[utoipa::path(
    get, path = "/admin/duplicates", tag = TAG,
    operation_id = "merge_duplicates",
    responses(
        (status = 200, body = DuplicateList),
        (status = 403, description = "The caller may not write"),
    ),
)]
async fn duplicates(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<DuplicateList>> {
    identity.require_write()?;
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
    Ok(Json(DuplicateList {
        pairs: merge::duplicates(&state, adult).await?,
    }))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = DismissDuplicateRequest)]
pub struct DismissRequest {
    /// One work of the pair, by its id.
    pub first: String,
    /// The other.
    pub second: String,
}

/// Say two works are not one: the pair is not proposed again, whatever is
/// found of it later. The audit trail says so (`duplicate.dismissed`).
#[utoipa::path(
    post, path = "/admin/duplicates/dismiss", tag = TAG,
    operation_id = "merge_dismiss",
    request_body = DismissRequest,
    responses(
        (status = 204, description = "Not proposed again"),
        (status = 400, description = "The two are one work"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
    ),
)]
async fn dismiss(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<DismissRequest>,
) -> AppResult<StatusCode> {
    identity.require_write()?;
    let by = identity.label();
    merge::dismiss(&state, &request.first, &request.second, Some(&by))
        .await?
        .map_err(refused)?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::DuplicateDismissed,
            target: Some(&request.first),
            detail: Some(&format!("not one work with {}", request.second)),
        },
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct PreviewQuery {
    /// The work it would be merged into: the one that stays.
    pub into: String,
}

/// What merging a work into another would do, before it is done.
///
/// Both works as they would be told apart; every field whose value differs,
/// as each is served, whether each locks it, and whether the merged work's
/// value can be kept; what the merged work holds, by kind of row; the names
/// the survivor would answer for; and which of the two the rule keeps — the
/// one with hand edits, else the one filed under more identifiers, else the
/// older — and why. Nothing is written.
#[utoipa::path(
    get, path = "/items/{id}/merge", tag = TAG,
    operation_id = "merge_preview",
    params(("id" = String, Path, description = "The work that would be merged into the other"), PreviewQuery),
    responses(
        (status = 200, body = Preview),
        (status = 400, description = "The two are one work"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
        (status = 409, description = "A film and a series, or one of the two merged already"),
    ),
)]
async fn preview(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Query(query): Query<PreviewQuery>,
) -> AppResult<Json<Preview>> {
    identity.require_write()?;
    Ok(Json(
        merge::preview(&state, &id, &query.into)
            .await?
            .map_err(refused)?,
    ))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = MergeRequest)]
pub struct MergeRequest {
    /// The work it is merged into: the one that stays.
    pub into: String,
    /// The side each field is kept from, by its name in the field registry,
    /// where the two differ: `survivor`, the default, or `merged`. Every
    /// field the work may lock but its slug and its identifiers.
    #[serde(default)]
    pub keep: BTreeMap<String, Side>,
}

/// Merge a work into another, in one transaction.
///
/// The merged work's seasons, episodes, pictures, credits, other titles,
/// translations, orders, releases, the copies its sources gave, its locks,
/// its journal and its places in selections move onto the survivor, but
/// where the survivor holds the same — a season of the same number, an
/// episode at the same place, a picture at the same address — whose own
/// stands. Its locks move where the survivor locks nothing of the same; a
/// field kept from it (`keep`) is the survivor's lock, set to its value
/// where it held none. Its identifiers become the survivor's where it has
/// none of their source, and its id, its slug and each identifier answer
/// with the survivor from now on, for the site, the API, Sonarr, Radarr and
/// Plex alike. The merged work stays as a tombstone, hidden from every list,
/// until the merge is undone (`merge.undoDays`). Written to the survivor's
/// journal (`merge`) and the audit trail (`item.merged`, with what the
/// survivor was and became); told to Sonarr or Radarr under both ids when
/// `arr.pushRefresh` is on.
#[utoipa::path(
    post, path = "/items/{id}/merge", tag = TAG,
    operation_id = "merge_merge_into",
    params(("id" = String, Path, description = "The work merged into the other: the one that becomes a tombstone")),
    request_body = MergeRequest,
    responses(
        (status = 200, body = Outcome),
        (status = 400, description = "The two are one work, or a field is not kept from either side, or the merged work's value of one cannot be kept"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
        (status = 409, description = "A film and a series, or one of the two merged already"),
    ),
)]
async fn merge_into(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<MergeRequest>,
) -> AppResult<Json<Outcome>> {
    identity.require_write()?;
    let outcome = merge::merge(
        &state,
        &asker(&identity, &ip),
        &id,
        &request.into,
        request.keep,
    )
    .await?
    .map_err(refused)?;
    tracing::info!(merged = %id, into = %request.into, actor = %identity.label(), "merged a work into another");
    Ok(Json(outcome))
}

/// What a closed door says: a `410`, with the code the interface reads.
fn gone(code: &'static str, message: String) -> Response {
    (
        StatusCode::GONE,
        Json(json!({ "error": code, "message": message })),
    )
        .into_response()
}

/// Undo a merge, by the merged work's id.
///
/// The merged work comes back as a work of its own: what moved onto the
/// survivor and is still as the merge left it goes back — what a refresh of
/// the survivor replaced since does not, and comes back to the work at its
/// own refresh, which is due at once — what gave way to the survivor's rows
/// is put back, its locks and its identifiers are its own again but those
/// another work holds now, and its names stop answering with the survivor.
/// The pair is not proposed as duplicates again. Within `merge.undoDays` of
/// the merge; a `410` after, or when undos are not kept.
#[utoipa::path(
    post, path = "/items/{id}/unmerge", tag = TAG,
    operation_id = "merge_unmerge",
    params(("id" = String, Path, description = "The merged work: the tombstone")),
    responses(
        (status = 200, body = Unmerge),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No work by that id was merged into another"),
        (status = 409, description = "The work it was merged into was merged in its turn: that merge is undone first"),
        (status = 410, description = "The window to undo it is closed, undos are not kept, or the work it was merged into is gone"),
    ),
)]
async fn unmerge(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<Response> {
    identity.require_write()?;
    match merge::unmerge(&state, &asker(&identity, &ip), &id).await? {
        Ok(done) => {
            tracing::info!(restored = %id, actor = %identity.label(), "took a merge apart");
            Ok(Json(done).into_response())
        }
        Err(Undo::NotMerged) => Err(AppError::NotFound),
        Err(Undo::Off) => Ok(gone(
            "merge_undo_off",
            "merges are not kept to be undone: merge.undoDays is 0".into(),
        )),
        Err(Undo::Expired { until }) => Ok(gone(
            "merge_undo_expired",
            format!("this merge could be undone until {until}"),
        )),
        Err(Undo::SurvivorGone) => Ok(gone(
            "merge_survivor_gone",
            "the work it was merged into is gone".into(),
        )),
        Err(Undo::SurvivorMerged { into }) => Err(AppError::Conflict(format!(
            "the work it was merged into was merged into {into} since: undo that merge first"
        ))),
    }
}

/// The merges into a work that can still be undone, and every name of the
/// works merged into it that it answers for.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = MergeList)]
pub struct MergeList {
    /// Newest first; none while undos are not kept.
    pub merges: Vec<Undoable>,
    /// For how many days a merge can be undone; `0`: not at all.
    pub undo_days: i64,
    /// Newest merge first.
    pub aliases: Vec<Alias>,
}

/// What was merged into a work: what its editor offers to undo.
#[utoipa::path(
    get, path = "/items/{id}/merges", tag = TAG,
    operation_id = "merge_merges",
    params(("id" = String, Path, description = "The survivor")),
    responses(
        (status = 200, body = MergeList),
        (status = 403, description = "The caller may not write"),
    ),
)]
async fn merges(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<MergeList>> {
    identity.require_write()?;
    Ok(Json(MergeList {
        merges: merge::undoable(&state, &id).await?,
        undo_days: merge::undo_days(&state),
        aliases: repo::alias::of_work(&state.db, &id).await?,
    }))
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        db::{
            RowExt,
            repo::item::{ItemWrite, upsert},
        },
        domain::{ExternalIds, MediaItem, MediaKind, fields::Scope},
        service,
        settings::registry::Scope as SettingScope,
    };

    /// A work, stored as its sources gave it; `shape` says what it is.
    async fn stored(
        state: &AppState,
        kind: MediaKind,
        title: &str,
        shape: impl FnOnce(&mut MediaItem),
    ) -> MediaItem {
        let mut item = MediaItem::empty(kind);
        item.title = title.into();
        item.slug = crate::domain::make_slug(title, None);
        shape(&mut item);
        upsert(
            &state.db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        item
    }

    async fn listed(state: &AppState) -> Vec<Pair> {
        let Json(list) = duplicates(State(state.clone()), Extension(Identity::Anonymous))
            .await
            .expect("listed");
        list.pairs
    }

    /// How many works the catalogue's register lists under `missing`.
    async fn register(state: &AppState, missing: &str) -> i64 {
        let query = super::super::items::to_query(
            state,
            &Identity::Anonymous,
            serde_urlencoded::from_str(&format!("includeDisabled=true&missing={missing}"))
                .expect("a query"),
        )
        .expect("checked");
        repo::item::count_matching(&state.db, &query)
            .await
            .expect("counted")
    }

    #[tokio::test]
    async fn the_duplicates_are_listed_with_the_survivor_the_rule_keeps_and_dismissed() {
        let state = super::super::testing::server().await;
        let older = stored(&state, MediaKind::Movie, "Solaris", |w| w.year = Some(1972)).await;
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        let filed = stored(&state, MediaKind::Movie, "Solaris", |w| {
            w.year = Some(1973);
            w.slug = "solaris-again".into();
            w.external_ids = ExternalIds {
                tmdb: Some(593),
                ..Default::default()
            };
        })
        .await;
        // A series of the same name is no duplicate of a film.
        stored(&state, MediaKind::Series, "Solaris", |w| {
            w.year = Some(1972)
        })
        .await;

        let pairs = listed(&state).await;
        assert_eq!(pairs.len(), 1, "{pairs:?}");
        assert_eq!(pairs[0].confidence, repo::item::Confidence::Probable);
        assert_eq!(pairs[0].survivor, filed.id, "the one filed under more ids");
        assert_eq!(pairs[0].why, merge::Why::MoreIds);
        // The register lists both, as the dashboard counts them.
        assert_eq!(register(&state, "duplicates").await, 2);

        // A hand edit outweighs an identifier.
        repo::override_field::set(
            &state.db,
            &older.id,
            Scope::Item,
            "overview",
            Some(&json!("Written here.")),
            None,
        )
        .await
        .unwrap();
        let pairs = listed(&state).await;
        assert_eq!(pairs[0].survivor, older.id);
        assert_eq!(pairs[0].why, merge::Why::HandEdits);

        let answered = dismiss(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Json(DismissRequest {
                first: filed.id.clone(),
                second: older.id.clone(),
            }),
        )
        .await
        .expect("dismissed");
        assert_eq!(answered, StatusCode::NO_CONTENT);
        assert!(listed(&state).await.is_empty());
        assert_eq!(register(&state, "duplicates").await, 0);

        // Only for those who may edit the catalogue.
        let refused = duplicates(State(state.clone()), Extension(Identity::Visitor)).await;
        assert!(matches!(refused, Err(AppError::Forbidden)));
    }

    #[tokio::test]
    async fn a_pair_found_forgotten_or_dismissed_moves_every_list_on() {
        let state = super::super::testing::server().await;
        let one = stored(&state, MediaKind::Movie, "Stalker", |w| w.year = Some(1979)).await;
        let mut two = stored(&state, MediaKind::Movie, "Stalker", |w| {
            w.year = Some(1979);
            w.slug = "stalker-again".into();
        })
        .await;

        // Found: a page of the register drawn before it is of the past.
        let drawn = state.caches.stamp();
        merge::look_again(&state).await.expect("looked for");
        assert_ne!(state.caches.stamp(), drawn, "a pair found anew");

        // Looked for again with nothing new: the pages drawn stay good.
        let drawn = state.caches.stamp();
        merge::look_again(&state).await.expect("looked for again");
        assert_eq!(state.caches.stamp(), drawn, "nothing found anew");

        let answered = dismiss(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Json(DismissRequest {
                first: one.id.clone(),
                second: two.id.clone(),
            }),
        )
        .await
        .expect("dismissed");
        assert_eq!(answered, StatusCode::NO_CONTENT);
        assert_ne!(state.caches.stamp(), drawn, "a pair dismissed");

        // A pair no longer found is forgotten, and that moves them on too.
        let three = stored(&state, MediaKind::Movie, "Mirror", |w| w.year = Some(1975)).await;
        two.title = "Mirror".into();
        two.year = Some(1975);
        upsert(
            &state.db,
            ItemWrite {
                item: &two,
                replace_children: true,
            },
        )
        .await
        .expect("renamed");
        // Listed again, as a write through the editor or a refresh lists it.
        service::listing::after_write(&state, &two.id).await;
        merge::look_again(&state).await.expect("looked for");
        assert_eq!(listed(&state).await.len(), 1, "two and three, now alike");
        two.title = "Nostalghia".into();
        upsert(
            &state.db,
            ItemWrite {
                item: &two,
                replace_children: true,
            },
        )
        .await
        .expect("renamed again");
        service::listing::after_write(&state, &two.id).await;
        let drawn = state.caches.stamp();
        merge::look_again(&state).await.expect("looked for");
        assert_ne!(state.caches.stamp(), drawn, "a pair forgotten");
        assert!(
            listed(&state).await.is_empty(),
            "{} is alone again",
            three.title
        );
    }

    async fn merged(state: &AppState, merged: &str, into: &str, keep: Value) -> AppResult<Outcome> {
        let Json(outcome) = merge_into(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path(merged.to_string()),
            Json(MergeRequest {
                into: into.to_string(),
                keep: serde_json::from_value(keep).expect("a choice"),
            }),
        )
        .await?;
        Ok(outcome)
    }

    #[tokio::test]
    async fn a_merge_is_previewed_made_and_answered_by_every_name_of_the_merged_work() {
        let state = super::super::testing::server().await;
        let heat = stored(&state, MediaKind::Movie, "Heat", |w| {
            w.year = Some(1995);
            w.overview = Some("A thief and a detective.".into());
            w.external_ids = ExternalIds {
                tmdb: Some(949),
                ..Default::default()
            };
        })
        .await;
        let copy = stored(&state, MediaKind::Movie, "Heat (1995)", |w| {
            w.year = Some(1995);
            w.slug = "heat-copy".into();
            w.overview = Some("Another synopsis.".into());
            w.tagline = Some("A Los Angeles crime saga.".into());
            w.external_ids = ExternalIds {
                tmdb: Some(950),
                imdb: Some("tt0113277".into()),
                ..Default::default()
            };
        })
        .await;
        repo::override_field::set(
            &state.db,
            &heat.id,
            Scope::Item,
            "overview",
            Some(&json!("Locked here.")),
            None,
        )
        .await
        .unwrap();

        let Json(preview) = preview(
            State(state.clone()),
            Extension(Identity::Anonymous),
            Path(copy.id.clone()),
            Query(PreviewQuery {
                into: heat.id.clone(),
            }),
        )
        .await
        .expect("previewed");
        assert_eq!(preview.proposed, heat.id);
        assert_eq!(preview.why, merge::Why::HandEdits);
        let fields: Vec<&str> = preview.fields.iter().map(|f| f.field.as_str()).collect();
        for field in ["title", "overview", "tagline"] {
            assert!(fields.contains(&field), "{field} in {fields:?}");
        }
        let overview = preview
            .fields
            .iter()
            .find(|f| f.field == "overview")
            .unwrap();
        assert!(overview.survivor_locked && !overview.merged_locked);
        assert_eq!(overview.survivor, Some(json!("Locked here.")));
        assert!(
            preview
                .aliases
                .iter()
                .any(|a| a.source == "tmdb_movie" && a.value == "950")
        );
        assert_eq!(preview.undo_days, merge::DEFAULT_UNDO_DAYS);

        let outcome = merged(&state, &copy.id, &heat.id, json!({ "tagline": "merged" }))
            .await
            .expect("merged");
        assert!(outcome.undo_until.is_some());
        assert_eq!(outcome.aliases, 4, "its id, its slug, two identifiers");

        // Every name of the merged work answers with the survivor.
        for address in [copy.id.as_str(), "heat-copy"] {
            let found = service::load(&state, address)
                .await
                .unwrap()
                .expect("found");
            assert_eq!(found.id, heat.id, "{address}");
            assert_eq!(found.tagline.as_deref(), Some("A Los Angeles crime saga."));
        }
        let by_tmdb = service::movie::by_tmdb_id(&state, 950)
            .await
            .unwrap()
            .expect("found");
        assert_eq!(by_tmdb.id, heat.id);
        // Its IMDb id the survivor had none of is the survivor's own now.
        assert_eq!(by_tmdb.external_ids.imdb.as_deref(), Some("tt0113277"));

        // The survivor's journal says so, and the trail has the before and
        // the after.
        let journal = repo::change::list(
            &state.db,
            &heat.id,
            &repo::change::Filter::default(),
            None,
            50,
        )
        .await
        .unwrap();
        let said: Vec<(&str, repo::change::Cause)> = journal
            .entries
            .iter()
            .map(|e| (e.field.as_str(), e.cause))
            .collect();
        assert!(
            said.contains(&("merged", repo::change::Cause::Merge)),
            "{said:?}"
        );
        assert!(
            said.contains(&("tagline", repo::change::Cause::Merge)),
            "{said:?}"
        );
        let detail: String = sqlx::query_scalar(
            state
                .db
                .sql("SELECT detail FROM audit_log WHERE action = 'item.merged' AND target = ?"),
        )
        .bind(&copy.id)
        .fetch_one(state.db.pool())
        .await
        .unwrap();
        let detail: Value = serde_json::from_str(&detail).expect("JSON");
        assert_eq!(detail["into"]["id"], heat.id);
        assert_eq!(detail["before"]["fields"]["tagline"], Value::Null);
        assert_eq!(
            detail["after"]["fields"]["tagline"],
            "A Los Angeles crime saga."
        );

        // The survivor's editor offers the undo, and names what it answers for.
        let Json(merges) = merges(
            State(state.clone()),
            Extension(Identity::Anonymous),
            Path(heat.id.clone()),
        )
        .await
        .unwrap();
        assert_eq!(merges.merges.len(), 1);
        assert_eq!(merges.merges[0].work.id, copy.id);
        assert!(
            merges
                .aliases
                .iter()
                .any(|a| a.source == "slug" && a.value == "heat-copy")
        );
    }

    async fn undone(state: &AppState, id: &str) -> (StatusCode, Value) {
        let response = unmerge(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path(id.to_string()),
        )
        .await
        .unwrap_or_else(IntoResponse::into_response);
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn a_merge_is_undone_within_its_window_and_gone_after_it() {
        let state = super::super::testing::server().await;
        let kept = stored(&state, MediaKind::Series, "Dark", |w| {
            w.external_ids.tvdb = Some(334824);
        })
        .await;
        let typed = stored(&state, MediaKind::Series, "Dark (typed)", |w| {
            w.external_ids.tmdb = Some(70523);
        })
        .await;

        merged(&state, &typed.id, &kept.id, json!({}))
            .await
            .expect("merged");
        // Sonarr's names leave the tombstone out: it would be the survivor's
        // homonym.
        let titles = repo::scene::series_titles(&state.db).await.unwrap();
        assert_eq!(
            titles.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(),
            ["Dark"]
        );
        let (status, body) = undone(&state, &typed.id).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["restored"], typed.id);
        let back = service::load(&state, &typed.id)
            .await
            .unwrap()
            .expect("back");
        assert_eq!(back.id, typed.id, "a work of its own again");
        assert_eq!(back.external_ids.tmdb, Some(70523));
        // Not a tombstone any more: there is nothing to undo.
        assert_eq!(undone(&state, &typed.id).await.0, StatusCode::NOT_FOUND);

        // Merged again, and three weeks went by.
        merged(&state, &typed.id, &kept.id, json!({}))
            .await
            .expect("merged");
        sqlx::query(
            state
                .db
                .sql("UPDATE media_item SET merged_at = ? WHERE id = ?"),
        )
        .bind(crate::db::to_rfc3339(
            chrono::Utc::now() - chrono::TimeDelta::days(21),
        ))
        .bind(&typed.id)
        .execute(state.db.pool())
        .await
        .unwrap();
        let (status, body) = undone(&state, &typed.id).await;
        assert_eq!(status, StatusCode::GONE);
        assert_eq!(body["error"], "merge_undo_expired");
        // A longer window reaches it again; none closes every one.
        state
            .settings
            .set(SettingScope::Server, "", merge::UNDO_DAYS, "30", None)
            .await
            .unwrap();
        let Json(open) = merges(
            State(state.clone()),
            Extension(Identity::Anonymous),
            Path(kept.id.clone()),
        )
        .await
        .unwrap();
        assert_eq!(open.merges.len(), 1);
        state
            .settings
            .set(SettingScope::Server, "", merge::UNDO_DAYS, "0", None)
            .await
            .unwrap();
        let (status, body) = undone(&state, &typed.id).await;
        assert_eq!(status, StatusCode::GONE);
        assert_eq!(body["error"], "merge_undo_off");
        let Json(none) = merges(
            State(state.clone()),
            Extension(Identity::Anonymous),
            Path(kept.id.clone()),
        )
        .await
        .unwrap();
        assert!(none.merges.is_empty());
    }

    #[tokio::test]
    async fn a_merge_that_cannot_be_is_refused_and_says_why() {
        let state = super::super::testing::server().await;
        let film = stored(&state, MediaKind::Movie, "Cube", |w| w.year = Some(1997)).await;
        let series = stored(&state, MediaKind::Series, "Cube", |w| w.year = Some(1997)).await;
        let other = stored(&state, MediaKind::Movie, "Cube²", |w| w.year = Some(2002)).await;

        assert!(matches!(
            merged(&state, &film.id, &film.id, json!({})).await,
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            merged(&state, &film.id, &series.id, json!({})).await,
            Err(AppError::Conflict(_))
        ));
        assert!(matches!(
            merged(&state, &film.id, "no-such-work", json!({})).await,
            Err(AppError::NotFound)
        ));
        assert!(matches!(
            merged(&state, &other.id, &film.id, json!({ "slug": "merged" })).await,
            Err(AppError::BadRequest(_))
        ));
        // The title is never kept empty: the merged work has one, so it may
        // be kept; a poster it has none of may not.
        assert!(matches!(
            merged(
                &state,
                &other.id,
                &film.id,
                json!({ "primaryPoster": "merged" })
            )
            .await,
            Err(AppError::BadRequest(_))
        ));

        merged(&state, &other.id, &film.id, json!({ "title": "merged" }))
            .await
            .expect("merged");
        let row = sqlx::query(
            state
                .db
                .sql("SELECT value FROM media_override WHERE media_id = ? AND field = 'title'"),
        )
        .bind(&film.id)
        .fetch_one(state.db.pool())
        .await
        .unwrap();
        assert_eq!(row.text("value").unwrap(), "\"Cube²\"");
        // A tombstone is merged into nothing again.
        assert!(matches!(
            merged(&state, &other.id, &series.id, json!({})).await,
            Err(AppError::Conflict(_))
        ));
    }

    /// A write under the merged work's id is refused and names the survivor;
    /// one under the survivor's, and the undo under the tombstone's, go on.
    #[tokio::test]
    async fn a_write_under_a_merged_id_is_refused_with_the_survivors() {
        use axum::{
            Router,
            body::Body,
            http::{Request, header},
            middleware::from_fn_with_state,
            routing::{post, put},
        };
        use tower::ServiceExt as _;

        let state = super::super::testing::server().await;
        let kept = stored(&state, MediaKind::Movie, "Ronin", |w| {
            w.year = Some(1998);
            w.external_ids.tmdb = Some(8195);
        })
        .await;
        let gone = stored(&state, MediaKind::Movie, "Ronin (1998)", |w| {
            w.year = Some(1998);
            w.slug = "ronin-copy".into();
        })
        .await;
        merged(&state, &gone.id, &kept.id, json!({}))
            .await
            .expect("merged");

        let app = Router::new()
            .route("/api/v1/items/{id}/locks", put(|| async { "written" }))
            .route("/api/v1/items/{id}/unmerge", post(|| async { "undone" }))
            .layer(from_fn_with_state(
                state.clone(),
                crate::api::merged::refuse_writes,
            ))
            .with_state(state.clone());
        let send = |method: &'static str, path: String| {
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };

        let refused = send("PUT", format!("/api/v1/items/{}/locks", gone.id)).await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(
            refused.headers()[header::LOCATION],
            format!("/api/v1/items/{}", kept.id).as_str()
        );
        let body = to_bytes(refused.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], crate::api::merged::CODE);
        assert_eq!(body["survivor"], kept.id.as_str());

        let written = send("PUT", format!("/api/v1/items/{}/locks", kept.id)).await;
        assert_eq!(written.status(), StatusCode::OK);
        let undone = send("POST", format!("/api/v1/items/{}/unmerge", gone.id)).await;
        assert_eq!(undone.status(), StatusCode::OK);
    }
}
