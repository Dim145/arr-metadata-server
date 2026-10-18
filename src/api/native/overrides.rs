//! Manual edits and the locks they create.
//!
//! A lock names a field of the work, of a season or of an episode — a text
//! in one language alone, if asked — or a child the sources gave the work,
//! by what it is: a credit, an image, an alternative title, hidden or
//! corrected (see [`fields::Child`]). The editor sets the latter through the
//! child's own routes (`children.rs`), which name it by its row.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{self, audit::Action},
    domain::{
        MediaKind,
        fields::{self, Child, Override, Scope},
    },
    error::{AppError, AppResult},
    service,
    state::AppState,
};

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Locks";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, set, clear))
        .routes(routes!(unset))
}

/// Every manual edit stored against a work.
///
/// Of a work this caller may see, as its own page decides it. Who made each
/// edit is said to whoever maintains the catalogue: to anybody else it would
/// name accounts and keys.
#[utoipa::path(
    get, path = "/items/{id}/overrides", tag = TAG,
    operation_id = "overrides_list",
    params(("id" = String, Path, description = "The work's identifier")),
    responses(
        (status = 200, body = Vec<Override>),
        (status = 404, description = "No such work, or none this caller may see"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<Override>>> {
    super::visible_work(&state, &identity, &id).await?;

    let mut overrides = repo::override_field::list(&state.db, &id).await?;
    if !identity.can_write() {
        for edit in &mut overrides {
            edit.updated_by = None;
        }
    }
    Ok(Json(overrides))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetRequest {
    /// `item`, `season:3`, `episode:3x7` — or a child the sources gave:
    /// `credit:actor:tmdb:17419`, `image:https://…`,
    /// `alternativeTitle:fr:Les Experts`. Defaults to the work itself.
    #[serde(default = "default_scope")]
    pub scope: String,
    pub field: String,
    /// A text — a title, an overview, a tagline — locked in this language
    /// alone: `fr`, `fr-FR` and `fra` are one. Absent for every language.
    #[serde(default)]
    pub language: Option<String>,
    /// Absent or `null` stores an explicit "cleared" value, which still locks
    /// the field. To hand it back to the provider, delete the override instead.
    pub value: Option<Value>,
}

fn default_scope() -> String {
    "item".to_string()
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetResponse {
    pub locked_fields: Vec<String>,
}

/// Record an edit, locking the field.
///
/// From this point every refresh leaves the field alone. Sending a `null` value
/// stores an explicit "cleared" state, which still locks it; to hand the field
/// back to the provider, delete the override instead.
///
/// With a `language`, a text — a title, an overview, a tagline — is locked in
/// that language alone: a work served in it is given that text, over a lock
/// in every language and over any translation; served in another, it is not.
/// A scope naming a child the sources gave hides it (`hidden`, `true`) or
/// corrects a credit (`characterName`, `sortOrder`), whatever row a refresh
/// writes it in next.
#[utoipa::path(
    put, path = "/items/{id}/overrides", tag = TAG,
    operation_id = "overrides_set",
    params(("id" = String, Path, description = "The work's identifier")),
    request_body = SetRequest,
    responses(
        (status = 200, description = "Every field now locked on this work", body = SetResponse),
        (status = 400, description = "Unknown field, wrong type, unparsable scope, or a language for what has none"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work, season or episode"),
    ),
)]
async fn set(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(mut request): Json<SetRequest>,
) -> AppResult<Json<SetResponse>> {
    identity.require_write()?;

    // A child the sources gave, by what it is.
    if let Ok(child) = request.scope.parse::<Child>() {
        if request.language.is_some() {
            return Err(AppError::BadRequest(
                "a credit, an image or a title is locked in every language".into(),
            ));
        }
        let child = own_address(&state, child);
        fields::validate_child(&child, &request.field, request.value.as_ref())
            .map_err(AppError::BadRequest)?;
        let item = service::load(&state, &id)
            .await?
            .ok_or(AppError::NotFound)?;
        lock_child(
            &state,
            &item.id,
            &child,
            &request.field,
            request.value.as_ref(),
            &identity.label(),
        )
        .await?;
        let target = format!("{id}#{child}/{}", request.field);
        return answer(
            &state,
            &identity,
            &ip,
            &id,
            &target,
            request.value.is_some(),
        )
        .await;
    }

    let scope: Scope = request
        .scope
        .parse()
        .map_err(|e: anyhow::Error| AppError::BadRequest(e.to_string()))?;
    let language = lock_language(request.language.as_deref())?;

    // An address of this server's own — the editor shows the copies kept —
    // is locked as the provider's: what the sweep looks for, and what stays
    // right if the copy is ever forgotten.
    if matches!(
        request.field.as_str(),
        "image" | "themeMusic" | "primaryPoster" | "primaryFanart"
    ) && let Some(serde_json::Value::String(url)) = &request.value
    {
        request.value = Some(serde_json::Value::String(state.media.unlocalize(url)));
    }

    fields::validate(scope, &request.field, request.value.as_ref())
        .map_err(AppError::BadRequest)?;
    if language.is_some() {
        fields::validate_language(scope, &request.field).map_err(AppError::BadRequest)?;
    }

    // Refuse to attach an override to something that is not there: it would
    // silently do nothing on read.
    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    match scope {
        Scope::Season(n) if !item.seasons.iter().any(|s| s.season_number == n) => {
            return Err(AppError::NotFound);
        }
        Scope::Episode { season, episode }
            if !item
                .episodes
                .iter()
                .any(|e| e.season_number == season && e.episode_number == episode) =>
        {
            return Err(AppError::NotFound);
        }
        _ => {}
    }

    match language.as_deref() {
        Some(language) => {
            lock_in_language(
                &state,
                &item.id,
                scope,
                &request.field,
                language,
                request.value.as_ref(),
                &identity.label(),
            )
            .await?;
        }
        None => {
            lock(
                &state,
                (&item.id, item.kind),
                scope,
                &request.field,
                request.value.as_ref(),
                &identity.label(),
            )
            .await?;
        }
    }

    let target = match language.as_deref() {
        Some(language) => format!("{id}#{scope}/{}@{language}", request.field),
        None => format!("{id}#{scope}/{}", request.field),
    };
    answer(
        &state,
        &identity,
        &ip,
        &id,
        &target,
        request.value.is_some(),
    )
    .await
}

/// What a lock set is answered with, once it is listed again and recorded:
/// every field now locked on the work.
async fn answer(
    state: &AppState,
    identity: &Identity,
    ip: &ClientIp,
    id: &str,
    target: &str,
    given: bool,
) -> AppResult<Json<SetResponse>> {
    service::listing::after_write(state, id).await;

    tracing::info!(
        %id, %target, actor = %identity.label(),
        "locked a field with a manual edit"
    );

    audit::record(
        state,
        Event {
            identity: Some(identity),
            ip,
            action: Action::OverrideSet,
            target: Some(target),
            // The value itself is not recorded: an overview runs to paragraphs,
            // and the current value is one read away.
            detail: Some(if given { "set" } else { "cleared" }),
        },
    )
    .await;

    let updated = service::load(state, id).await?.ok_or(AppError::NotFound)?;

    Ok(Json(SetResponse {
        locked_fields: updated.locked_fields,
    }))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UnsetQuery {
    /// The language a text was locked in alone: `fr`, `fra`. Left out, the
    /// lock in every language is lifted.
    pub language: Option<String>,
}

/// Unlock one field, handing it back to provider data on the next refresh.
///
/// The scope may name a child the sources gave, as the lock does
/// (`credit:actor:tmdb:17419`), percent-encoded; an image's, whose address
/// holds slashes, is easier lifted from the image's own route.
#[utoipa::path(
    delete, path = "/items/{id}/overrides/{scope}/{field}", tag = TAG,
    operation_id = "overrides_unset",
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("scope" = String, Path, description = "`item`, `season:3` or `episode:3x7`, or a child's"),
        ("field" = String, Path, description = "The field name, as the registry reports it"),
        UnsetQuery,
    ),
    responses(
        (status = 204, description = "Unlocked"),
        (status = 400, description = "Unparsable scope, or no language"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such override"),
    ),
)]
async fn unset(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, scope, field)): Path<(String, String, String)>,
    Query(query): Query<UnsetQuery>,
) -> AppResult<StatusCode> {
    identity.require_write()?;

    let language = lock_language(query.language.as_deref())?;
    let (scope, uploads) = match scope.parse::<Child>() {
        Ok(child) => (own_address(&state, child).to_string(), Vec::new()),
        Err(_) => {
            let scope: Scope = scope
                .parse()
                .map_err(|e: anyhow::Error| AppError::BadRequest(e.to_string()))?;
            // An upload locked into the field goes with the lock.
            let uploads = match language {
                Some(_) => Vec::new(),
                None => uploads_locked(&state, &id, Some((scope, field.as_str()))).await?,
            };
            (scope.to_string(), uploads)
        }
    };

    let removed = repo::override_field::unset_in(
        &state.db,
        &id,
        &scope,
        &field,
        language.as_deref().unwrap_or_default(),
    )
    .await?;
    if !removed {
        return Err(AppError::NotFound);
    }
    super::media::forget_uploads(&state, uploads.iter().map(String::as_str)).await;

    state.caches.touched(&id).await;
    service::listing::after_write(&state, &id).await;
    tracing::info!(%id, %scope, %field, actor = %identity.label(), "unlocked a field");

    let target = match language.as_deref() {
        Some(language) => format!("{id}#{scope}/{field}@{language}"),
        None => format!("{id}#{scope}/{field}"),
    };
    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::OverrideRemoved,
            target: Some(&target),
            detail: None,
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

/// The key a text is locked in a language under, as translations are filed
/// — `fra` for `fr`, `fr-FR` and `fra` — or a 400 for what names no language
/// a source has text in: a lock no answer would ever be served in.
pub(super) fn lock_language(asked: Option<&str>) -> AppResult<Option<String>> {
    let Some(tag) = crate::api::extract::language(asked)? else {
        return Ok(None);
    };
    let key = crate::service::language::normalize(&tag);
    if key.is_empty() || !crate::service::language::is_known(&key) {
        return Err(AppError::BadRequest(
            "that is no language a work is served in".into(),
        ));
    }
    Ok(Some(key))
}

/// An image named by this server's own copy of it, named instead as its
/// source gives it: the address a lock on it is kept under, as for the
/// fields that hold an address.
fn own_address(state: &AppState, child: Child) -> Child {
    match child {
        Child::Image(address) => Child::Image(state.media.unlocalize(&address)),
        other => other,
    }
}

/// Lock a text of a work, a season or an episode in one language alone, by
/// its key (`fra`): what an edit in that language and an imported lock come
/// to. Nothing of the work's row moves with it — no text is its identity —
/// and the work is served afresh from here on. The value has been checked
/// against the registry and [`fields::validate_language`].
pub(super) async fn lock_in_language(
    state: &AppState,
    id: &str,
    scope: Scope,
    field: &str,
    language: &str,
    value: Option<&Value>,
    by: &str,
) -> AppResult<()> {
    repo::override_field::set_in(
        &state.db,
        id,
        &scope.to_string(),
        field,
        language,
        value,
        Some(by),
    )
    .await?;
    state.caches.touched(id).await;
    Ok(())
}

/// Lock something of a child the sources gave — hide it, correct it — in
/// every language: what the child's own routes and an import of locks come
/// to. The value has been checked against [`fields::validate_child`].
pub(super) async fn lock_child(
    state: &AppState,
    id: &str,
    child: &Child,
    field: &str,
    value: Option<&Value>,
    by: &str,
) -> AppResult<()> {
    repo::override_field::set_in(
        &state.db,
        id,
        &child.to_string(),
        field,
        "",
        value,
        Some(by),
    )
    .await?;
    state.caches.touched(id).await;
    Ok(())
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClearResponse {
    pub removed: u64,
}

/// Unlock every field of a work at once.
#[utoipa::path(
    delete, path = "/items/{id}/overrides", tag = TAG,
    operation_id = "overrides_clear",
    params(("id" = String, Path, description = "The work's identifier")),
    responses(
        (status = 200, body = ClearResponse),
        (status = 403, description = "The caller may not write"),
    ),
)]
async fn clear(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<Json<ClearResponse>> {
    identity.require_write()?;

    let uploads = uploads_locked(&state, &id, None).await?;
    let removed = repo::override_field::clear(&state.db, &id).await?;
    super::media::forget_uploads(&state, uploads.iter().map(String::as_str)).await;

    state.caches.touched(&id).await;
    service::listing::after_write(&state, &id).await;
    tracing::info!(%id, removed, actor = %identity.label(), "unlocked every field");

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::OverridesCleared,
            target: Some(&id),
            detail: Some(&format!("{removed} fields unlocked")),
        },
    )
    .await;

    Ok(Json(ClearResponse { removed }))
}

/// Lock a field of a work, or of one of its seasons or episodes: what an edit
/// by hand and an imported lock both come to.
///
/// A lock on the work's identity — adult, its slug, its identifiers — is
/// written to its row as well, in the lock's own transaction: the lists, the
/// calendar, the feeds, the addresses and the clients' lookups read the row,
/// not the lock, and an adult work locked so is kept from all of them at
/// once. A slug or an identifier another work holds refuses the lock, with
/// nothing written (409). The value has been checked against the registry.
/// The work is served afresh from here on; listing it again is the caller's.
pub(super) async fn lock(
    state: &AppState,
    (id, kind): (&str, MediaKind),
    scope: Scope,
    field: &str,
    value: Option<&Value>,
    by: &str,
) -> AppResult<()> {
    match scope {
        Scope::Item => {
            repo::override_field::set_on_work(&state.db, id, kind, field, value, Some(by))
                .await?
                .map_err(|taken| AppError::Conflict(taken.0))?;
        }
        _ => repo::override_field::set(&state.db, id, scope, field, value, Some(by)).await?,
    }

    state.caches.touched(id).await;
    Ok(())
}

/// The uploads a work's locks point at — one lock's, or every lock's — for
/// deleting when the lock goes.
async fn uploads_locked(
    state: &AppState,
    id: &str,
    only: Option<(Scope, &str)>,
) -> AppResult<Vec<String>> {
    let locked = repo::override_field::list(&state.db, id).await?;
    Ok(locked
        .into_iter()
        .filter(|lock| {
            only.is_none_or(|(scope, field)| {
                lock.field == field
                    && !lock.in_one_language()
                    && lock.scope.parse::<Scope>().ok() == Some(scope)
            })
        })
        .filter_map(|lock| match lock.value {
            Some(serde_json::Value::String(url)) if url.starts_with("upload:") => Some(url),
            _ => None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config,
        domain::{MediaItem, Translation},
        service::language,
    };
    use serde_json::json;

    /// The whole server on a database in memory, its works stored in
    /// English: what the languages asked for here are told apart from.
    async fn server() -> AppState {
        let mut config = config::Config::from_env().expect("a configuration");
        config.mode = config::Mode::Single;
        config.database = config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        };
        config.security.bootstrap_admin = None;
        config.cache.redis_url = None;
        config.media.storage = config::MediaStorage::Off;
        config.clients = None;
        config.tmdb.api_key = None;
        config.tmdb.language = "en-US".into();
        config.tvdb.api_key = None;
        config.tvdb.enabled = false;

        AppState::bootstrap(config).await.expect("a server")
    }

    /// *Heat*, as TMDB gave it: named in French and in German besides.
    async fn heat(state: &AppState) -> MediaItem {
        let mut work = MediaItem::empty(MediaKind::Movie);
        work.title = "Heat".into();
        work.slug = "heat-1995".into();
        work.overview = Some("A group of professional bank robbers.".into());
        work.external_ids.tmdb = Some(949);
        work.translations = vec![
            Translation {
                language: "fra".into(),
                title: Some("Heat (TMDB)".into()),
                overview: Some("Un groupe de braqueurs.".into()),
                is_manual: false,
            },
            Translation {
                language: "deu".into(),
                title: Some("Heat (Deutsch)".into()),
                overview: None,
                is_manual: false,
            },
        ];
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

    async fn put(
        state: &AppState,
        id: &str,
        field: &str,
        language: Option<&str>,
        value: Value,
    ) -> AppResult<Json<SetResponse>> {
        set(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path(id.to_string()),
            Json(SetRequest {
                scope: "item".into(),
                field: field.into(),
                language: language.map(str::to_string),
                value: Some(value),
            }),
        )
        .await
    }

    /// The work as a request in a language is answered with it — the
    /// native API's, Radarr's — or as it is read, with none.
    async fn in_language(state: &AppState, id: &str, language: &str) -> MediaItem {
        let mut item = service::load(state, id).await.expect("read").expect("held");
        language::apply(state, &mut item, language)
            .await
            .expect("laid");
        item
    }

    /// For a language, a lock in it, else a lock in every language, else
    /// the translation, else the text the work is stored with: in French
    /// alone; in every language; both at once.
    #[tokio::test]
    async fn a_language_is_given_its_own_lock_then_every_languages_then_its_translation() {
        let state = server().await;
        let film = heat(&state).await;

        // In French alone, written as a reader writes it and kept as
        // translations are filed.
        let locked = put(
            &state,
            &film.id,
            "title",
            Some("fr-FR"),
            json!("Heat, en français"),
        )
        .await
        .expect("locked")
        .0
        .locked_fields;
        assert_eq!(locked, ["item/title@fra"]);
        let stored = repo::override_field::list(&state.db, &film.id)
            .await
            .unwrap();
        assert_eq!(stored[0].language.as_deref(), Some("fra"));

        assert_eq!(
            in_language(&state, &film.id, "fr").await.title,
            "Heat, en français"
        );
        assert_eq!(
            in_language(&state, &film.id, "de").await.title,
            "Heat (Deutsch)",
            "German is given its translation"
        );
        assert_eq!(in_language(&state, &film.id, "").await.title, "Heat");

        // And in every language: French keeps its own; German's translation
        // is beaten; English is given it too.
        let _ = put(&state, &film.id, "title", None, json!("Heat (1995)"))
            .await
            .expect("locked");
        assert_eq!(
            in_language(&state, &film.id, "fr").await.title,
            "Heat, en français"
        );
        assert_eq!(
            in_language(&state, &film.id, "de").await.title,
            "Heat (1995)"
        );
        assert_eq!(
            in_language(&state, &film.id, "en").await.title,
            "Heat (1995)"
        );

        // An overview locked in every language beats French's translation.
        let _ = put(
            &state,
            &film.id,
            "overview",
            None,
            json!("Two men, one city."),
        )
        .await
        .expect("locked");
        let french = in_language(&state, &film.id, "fr").await;
        assert_eq!(french.overview.as_deref(), Some("Two men, one city."));
        assert_eq!(
            french
                .translations
                .iter()
                .find(|t| t.language == "fra")
                .and_then(|t| t.overview.as_deref()),
            Some("Two men, one city."),
            "the translations Radarr reads say the same"
        );

        // Lifted in French: French is given the one of every language.
        unset(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path((film.id.clone(), "item".into(), "title".into())),
            Query(UnsetQuery {
                language: Some("fra".into()),
            }),
        )
        .await
        .expect("unlocked");
        assert_eq!(
            in_language(&state, &film.id, "fr").await.title,
            "Heat (1995)"
        );
    }

    /// Only a text is locked in one language, and only in a language a work
    /// is served in.
    #[tokio::test]
    async fn a_lock_in_one_language_is_a_texts_in_a_real_language() {
        let state = server().await;
        let film = heat(&state).await;

        let not_a_text = put(&state, &film.id, "runtime", Some("fr"), json!(170)).await;
        assert!(matches!(not_a_text, Err(AppError::BadRequest(_))));
        let no_language = put(&state, &film.id, "title", Some("qqq"), json!("x")).await;
        assert!(matches!(no_language, Err(AppError::BadRequest(_))));
        let not_a_tag = put(&state, &film.id, "title", Some("français"), json!("x")).await;
        assert!(matches!(not_a_tag, Err(AppError::BadRequest(_))));
        assert!(
            repo::override_field::list(&state.db, &film.id)
                .await
                .unwrap()
                .is_empty()
        );

        // A credit is hidden in every language, not in one.
        let refused = set(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path(film.id.clone()),
            Json(SetRequest {
                scope: "credit:actor:tmdb:1158".into(),
                field: "hidden".into(),
                language: Some("fr".into()),
                value: Some(json!(true)),
            }),
        )
        .await;
        assert!(matches!(refused, Err(AppError::BadRequest(_))));
    }
}
