//! Server metadata: what is editable, what is stored, how it is configured.

use axum::{
    Extension, Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use serde::Serialize;

use crate::{
    auth::Identity,
    config::{Surface, SurfacePolicy},
    db::repo,
    domain::{MediaKind, fields},
    error::AppResult,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/fields", get(field_registry))
        .route("/stats", get(stats))
        .route("/settings", get(settings))
        .route("/cache/clear", post(clear_cache))
}

#[derive(Serialize)]
pub struct FieldRegistry {
    pub item: &'static [fields::FieldDef],
    pub season: &'static [fields::FieldDef],
    pub episode: &'static [fields::FieldDef],
}

/// What the web UI renders its edit form from.
async fn field_registry() -> Json<FieldRegistry> {
    Json(FieldRegistry {
        item: fields::ITEM_FIELDS,
        season: fields::SEASON_FIELDS,
        episode: fields::EPISODE_FIELDS,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub series: i64,
    pub movies: i64,
    pub total: i64,
    pub overrides: i64,
    pub clients: i64,
    pub cached_items: u64,
    pub cached_searches: u64,
}

async fn stats(State(state): State<AppState>) -> AppResult<Json<Stats>> {
    let series = repo::item::count(&state.db, Some(MediaKind::Series)).await?;
    let movies = repo::item::count(&state.db, Some(MediaKind::Movie)).await?;

    Ok(Json(Stats {
        series,
        movies,
        total: series + movies,
        overrides: repo::override_field::count(&state.db).await?,
        clients: repo::client::count(&state.db).await?,
        cached_items: state.caches.items.entry_count(),
        cached_searches: state.caches.searches.entry_count(),
    }))
}

#[derive(Serialize)]
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

/// Effective configuration. Deliberately carries no secrets: the TMDB key is
/// reported as a boolean, never echoed.
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
async fn clear_cache(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<StatusCode> {
    if !identity.can_write() {
        return Err(crate::error::AppError::Forbidden);
    }

    state.caches.invalidate_all().await;
    tracing::info!(actor = %identity.label(), "cleared the in-process cache");

    Ok(StatusCode::NO_CONTENT)
}

fn policy_name(policy: SurfacePolicy) -> &'static str {
    match policy {
        SurfacePolicy::ApiKey => "apikey",
        SurfacePolicy::Allowlist => "allowlist",
        SurfacePolicy::Open => "open",
    }
}
