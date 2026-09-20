//! Server metadata: what is editable, what is stored, how it is configured.

use axum::{Extension, Json, extract::State, http::StatusCode};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    config::{Surface, SurfacePolicy},
    db::repo::{self, audit::Action},
    domain::{MediaKind, fields},
    error::AppResult,
    state::AppState,
};

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Server";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(field_registry))
        .routes(routes!(stats))
        .routes(routes!(settings))
        .routes(routes!(clear_cache))
}

#[derive(Serialize, ToSchema)]
pub struct FieldRegistry {
    pub item: &'static [fields::FieldDef],
    pub season: &'static [fields::FieldDef],
    pub episode: &'static [fields::FieldDef],
}

/// Which fields are editable, and what type each holds.
///
/// This is the authority: an override naming a field absent from here is
/// refused. The web UI builds its edit form from it.
#[utoipa::path(
    get, path = "/fields", tag = TAG,
    responses((status = 200, body = FieldRegistry)),
)]
async fn field_registry() -> Json<FieldRegistry> {
    Json(FieldRegistry {
        item: fields::ITEM_FIELDS,
        season: fields::SEASON_FIELDS,
        episode: fields::EPISODE_FIELDS,
    })
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub series: i64,
    pub movies: i64,
    pub total: i64,
    pub overrides: i64,
    pub clients: i64,
    pub audit_entries: i64,
    pub cached_items: u64,
    pub cached_searches: u64,
}

/// How much this server is holding.
#[utoipa::path(get, path = "/stats", tag = TAG, responses((status = 200, body = Stats)))]
async fn stats(State(state): State<AppState>) -> AppResult<Json<Stats>> {
    let series = repo::item::count(&state.db, Some(MediaKind::Series)).await?;
    let movies = repo::item::count(&state.db, Some(MediaKind::Movie)).await?;

    Ok(Json(Stats {
        series,
        movies,
        total: series + movies,
        overrides: repo::override_field::count(&state.db).await?,
        clients: repo::client::count(&state.db).await?,
        audit_entries: repo::audit::count(&state.db).await?,
        cached_items: state.caches.items.entry_count(),
        cached_searches: state.caches.searches.entry_count(),
    }))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub version: &'static str,
    pub public_url: Option<String>,
    pub database: &'static str,
    pub tmdb_configured: bool,
    pub tmdb_language: String,
    pub skyhook_fallback: bool,
    pub refresh_enabled: bool,
    pub auth_disabled: bool,
    pub native_policy: &'static str,
    pub tmdb_policy: &'static str,
    pub arr_policy: &'static str,
}

/// Effective configuration.
///
/// Deliberately carries no secrets: the TMDB key is reported as a boolean,
/// never echoed.
#[utoipa::path(get, path = "/settings", tag = TAG, responses((status = 200, body = Settings)))]
async fn settings(State(state): State<AppState>) -> Json<Settings> {
    Json(Settings {
        version: env!("CARGO_PKG_VERSION"),
        public_url: state.config.server.public_url.clone(),
        database: match state.db.dialect() {
            crate::db::Dialect::Sqlite => "sqlite",
            crate::db::Dialect::Postgres => "postgres",
        },
        tmdb_configured: state.tmdb.is_configured(),
        tmdb_language: state.config.tmdb.language.clone(),
        skyhook_fallback: state.skyhook.is_enabled(),
        refresh_enabled: state.config.refresh.enabled,
        auth_disabled: state.config.security.auth_disabled,
        native_policy: policy_name(state.config.policy_for(Surface::Native)),
        tmdb_policy: policy_name(state.config.policy_for(Surface::Tmdb)),
        arr_policy: policy_name(state.config.policy_for(Surface::Arr)),
    })
}

/// Drop every cached entity and search result.
///
/// The database is untouched — this only discards the in-process layer in front
/// of it, so the next request re-reads and re-applies overrides.
#[utoipa::path(
    post, path = "/cache/clear", tag = TAG,
    responses(
        (status = 204, description = "Cleared"),
        (status = 403, description = "The caller may not write"),
    ),
)]
async fn clear_cache(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
) -> AppResult<StatusCode> {
    if !identity.can_write() {
        return Err(crate::error::AppError::Forbidden);
    }

    state.caches.invalidate_all().await;
    tracing::info!(actor = %identity.label(), "cleared the in-process cache");

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::CacheCleared,
            target: None,
            detail: None,
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

fn policy_name(policy: SurfacePolicy) -> &'static str {
    match policy {
        SurfacePolicy::ApiKey => "apikey",
        SurfacePolicy::Allowlist => "allowlist",
        SurfacePolicy::Open => "open",
    }
}
