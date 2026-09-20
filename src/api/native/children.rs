//! Adding and removing a work's children by hand.
//!
//! Editing a child's *fields* is a different operation: that goes through the
//! override endpoints, which work the same whether the row came from a provider
//! or from a person. What lives here is creation and removal, which overrides
//! cannot express.
//!
//! Only manual rows can be removed. Deleting a provider row would bring it back
//! on the next refresh, which reads as the delete having silently failed.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{self, audit::Action},
    domain::{CoverType, CreditType, MediaKind},
    error::{AppError, AppResult},
    service,
    state::AppState,
};

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Catalogue";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(add_season))
        .routes(routes!(remove_season))
        .routes(routes!(add_episode))
        .routes(routes!(remove_episode))
        .routes(routes!(add_image))
        .routes(routes!(remove_image))
        .routes(routes!(add_credit))
        .routes(routes!(remove_credit))
        .routes(routes!(add_alternative_title))
        .routes(routes!(remove_alternative_title))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    pub id: String,
}

// ─── seasons ─────────────────────────────────────────────────────────────────

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewSeason {
    pub season_number: i32,
    pub title: Option<String>,
    pub overview: Option<String>,
    pub air_date: Option<String>,
}

/// Add a season by hand.
///
/// It is flagged manual, so no refresh will replace or remove it.
#[utoipa::path(
    post, path = "/items/{id}/seasons", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier")),
    request_body = NewSeason,
    responses(
        (status = 201, body = Created),
        (status = 400, description = "A negative season number, or the work is not a series"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
        (status = 409, description = "That season already exists"),
    ),
)]
async fn add_season(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<NewSeason>,
) -> AppResult<(StatusCode, Json<Created>)> {
    let item = writable(&state, &identity, &id).await?;

    if item.kind != MediaKind::Series {
        return Err(AppError::BadRequest("only a series has seasons".into()));
    }
    if request.season_number < 0 {
        return Err(AppError::BadRequest(
            "season number must not be negative".into(),
        ));
    }
    if item
        .seasons
        .iter()
        .any(|s| s.season_number == request.season_number)
    {
        return Err(AppError::Conflict(format!(
            "season {} already exists",
            request.season_number
        )));
    }

    let mut season = repo::child::blank_season(request.season_number);
    season.title = request.title;
    season.overview = request.overview;
    season.air_date = request.air_date;

    let created = repo::child::add_season(&state.db, &id, &season).await?;

    finish(
        &state,
        &identity,
        &ip,
        &id,
        &format!("added season {}", request.season_number),
    )
    .await;

    Ok((StatusCode::CREATED, Json(Created { id: created })))
}

/// Remove a season that was added by hand.
#[utoipa::path(
    delete, path = "/items/{id}/seasons/{season}", tag = TAG,
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("season" = i32, Path, description = "Season number"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such season, or it came from a provider"),
    ),
)]
async fn remove_season(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, season)): Path<(String, i32)>,
) -> AppResult<StatusCode> {
    writable(&state, &identity, &id).await?;

    if !repo::child::remove_season(&state.db, &id, season).await? {
        return Err(AppError::NotFound);
    }

    finish(
        &state,
        &identity,
        &ip,
        &id,
        &format!("removed season {season}"),
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

// ─── episodes ────────────────────────────────────────────────────────────────

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewEpisode {
    pub season_number: i32,
    pub episode_number: i32,
    pub title: Option<String>,
    pub overview: Option<String>,
    pub air_date: Option<String>,
    pub runtime: Option<i32>,
    pub image: Option<String>,
    pub absolute_episode_number: Option<i32>,
}

/// Add an episode by hand.
#[utoipa::path(
    post, path = "/items/{id}/episodes", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier")),
    request_body = NewEpisode,
    responses(
        (status = 201, body = Created),
        (status = 400, description = "Bad numbering, a bad image URL, or the work is not a series"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
        (status = 409, description = "That episode already exists"),
    ),
)]
async fn add_episode(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<NewEpisode>,
) -> AppResult<(StatusCode, Json<Created>)> {
    let item = writable(&state, &identity, &id).await?;

    if item.kind != MediaKind::Series {
        return Err(AppError::BadRequest("only a series has episodes".into()));
    }
    if request.season_number < 0 || request.episode_number < 1 {
        return Err(AppError::BadRequest(
            "season number must not be negative and episode number must be at least 1".into(),
        ));
    }
    if item.episodes.iter().any(|e| {
        e.season_number == request.season_number && e.episode_number == request.episode_number
    }) {
        return Err(AppError::Conflict(format!(
            "episode {}x{} already exists",
            request.season_number, request.episode_number
        )));
    }

    if let Some(url) = &request.image {
        check_url(url)?;
    }

    let mut episode = repo::child::blank_episode(request.season_number, request.episode_number);
    episode.title = request.title.unwrap_or_default();
    episode.overview = request.overview;
    episode.runtime = request.runtime;
    episode.image = request.image;
    episode.absolute_episode_number = request.absolute_episode_number;

    if let Some(date) = request.air_date {
        // Clients expect both forms; deriving the UTC one keeps them agreeing.
        episode.air_date_utc = Some(format!("{date}T00:00:00Z"));
        episode.air_date = Some(date);
    }

    let created = repo::child::add_episode(&state.db, &id, &episode).await?;

    finish(
        &state,
        &identity,
        &ip,
        &id,
        &format!(
            "added episode {}x{}",
            request.season_number, request.episode_number
        ),
    )
    .await;

    Ok((StatusCode::CREATED, Json(Created { id: created })))
}

/// Remove an episode that was added by hand.
#[utoipa::path(
    delete, path = "/items/{id}/episodes/{season}/{episode}", tag = TAG,
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("season" = i32, Path, description = "Season number"),
        ("episode" = i32, Path, description = "Episode number"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such episode, or it came from a provider"),
    ),
)]
async fn remove_episode(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, season, episode)): Path<(String, i32, i32)>,
) -> AppResult<StatusCode> {
    writable(&state, &identity, &id).await?;

    if !repo::child::remove_episode(&state.db, &id, season, episode).await? {
        return Err(AppError::NotFound);
    }

    finish(
        &state,
        &identity,
        &ip,
        &id,
        &format!("removed episode {season}x{episode}"),
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

// ─── images ──────────────────────────────────────────────────────────────────

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewImage {
    /// `poster`, `fanart`, `banner`, `clearlogo`, `screenshot`, `headshot`.
    pub cover_type: String,
    pub url: String,
    /// Set to attach the image to one season rather than to the work.
    pub season_number: Option<i32>,
    pub language: Option<String>,
    /// Lower sorts first. Use 0 to make this the one clients pick.
    pub sort_order: Option<i32>,
}

/// Add an image by hand.
///
/// Use `sortOrder: 0` to put it ahead of the provider's own artwork.
#[utoipa::path(
    post, path = "/items/{id}/images", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier")),
    request_body = NewImage,
    responses(
        (status = 201, body = Created),
        (status = 400, description = "An unusable URL"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
    ),
)]
async fn add_image(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<NewImage>,
) -> AppResult<(StatusCode, Json<Created>)> {
    writable(&state, &identity, &id).await?;
    check_url(&request.url)?;

    let cover_type: CoverType = request.cover_type.parse().unwrap_or(CoverType::Unknown);

    let mut image = repo::child::blank_image(cover_type, request.url.clone());
    image.season_number = request.season_number;
    image.language = request.language;
    image.sort_order = request.sort_order.unwrap_or(0);

    let created = repo::child::add_image(&state.db, &id, &image).await?;

    finish(
        &state,
        &identity,
        &ip,
        &id,
        &format!("added a {} image", cover_type.as_str()),
    )
    .await;

    Ok((StatusCode::CREATED, Json(Created { id: created })))
}

/// Remove an image that was added by hand.
#[utoipa::path(
    delete, path = "/items/{id}/images/{image_id}", tag = TAG,
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("image_id" = String, Path, description = "The image's identifier"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such image, or it came from a provider"),
    ),
)]
async fn remove_image(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, image_id)): Path<(String, String)>,
) -> AppResult<StatusCode> {
    writable(&state, &identity, &id).await?;

    if !repo::child::remove_image(&state.db, &id, &image_id).await? {
        return Err(AppError::NotFound);
    }

    finish(&state, &identity, &ip, &id, "removed an image").await;

    Ok(StatusCode::NO_CONTENT)
}

// ─── credits ─────────────────────────────────────────────────────────────────

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewCredit {
    /// `actor`, `director`, `writer`, `producer`, `guest`.
    pub credit_type: Option<String>,
    pub person_name: String,
    pub character_name: Option<String>,
    pub image: Option<String>,
    pub sort_order: Option<i32>,
}

/// Add a credit by hand.
#[utoipa::path(
    post, path = "/items/{id}/credits", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier")),
    request_body = NewCredit,
    responses(
        (status = 201, body = Created),
        (status = 400, description = "An empty name or an unusable image URL"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
    ),
)]
async fn add_credit(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<NewCredit>,
) -> AppResult<(StatusCode, Json<Created>)> {
    writable(&state, &identity, &id).await?;

    let name = request.person_name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("personName must not be empty".into()));
    }
    if let Some(url) = &request.image {
        check_url(url)?;
    }

    let credit_type: CreditType = request
        .credit_type
        .as_deref()
        .unwrap_or("actor")
        .parse()
        .unwrap_or(CreditType::Actor);

    let mut credit = repo::child::blank_credit(credit_type, name.to_string());
    credit.character_name = request.character_name;
    credit.image = request.image;
    credit.sort_order = request.sort_order.unwrap_or(0);

    let created = repo::child::add_credit(&state.db, &id, &credit).await?;

    finish(
        &state,
        &identity,
        &ip,
        &id,
        &format!("added credit {name:?}"),
    )
    .await;

    Ok((StatusCode::CREATED, Json(Created { id: created })))
}

/// Remove a credit that was added by hand.
#[utoipa::path(
    delete, path = "/items/{id}/credits/{credit_id}", tag = TAG,
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("credit_id" = String, Path, description = "The credit's identifier"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such credit, or it came from a provider"),
    ),
)]
async fn remove_credit(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, credit_id)): Path<(String, String)>,
) -> AppResult<StatusCode> {
    writable(&state, &identity, &id).await?;

    if !repo::child::remove_credit(&state.db, &id, &credit_id).await? {
        return Err(AppError::NotFound);
    }

    finish(&state, &identity, &ip, &id, "removed a credit").await;

    Ok(StatusCode::NO_CONTENT)
}

// ─── alternative titles ──────────────────────────────────────────────────────

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewAlternativeTitle {
    pub title: String,
    /// Free text, e.g. `working`, `original`, `translated`.
    pub title_type: Option<String>,
    pub language: Option<String>,
}

/// Add an alternative title by hand.
///
/// Sonarr and Radarr match release names against these, so adding the spelling a
/// release group actually uses is often what makes a download get recognised.
#[utoipa::path(
    post, path = "/items/{id}/alternative-titles", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier")),
    request_body = NewAlternativeTitle,
    responses(
        (status = 201, body = Created),
        (status = 400, description = "An empty title"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
        (status = 409, description = "That title is already there"),
    ),
)]
async fn add_alternative_title(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<NewAlternativeTitle>,
) -> AppResult<(StatusCode, Json<Created>)> {
    let item = writable(&state, &identity, &id).await?;

    let title = request.title.trim();
    if title.is_empty() {
        return Err(AppError::BadRequest("title must not be empty".into()));
    }

    // The database has a unique index over (media_id, title, language); catching
    // it here gives a usable message instead of a 500.
    if item
        .alternative_titles
        .iter()
        .any(|t| t.title == title && t.language == request.language)
    {
        return Err(AppError::Conflict(format!("{title:?} is already listed")));
    }

    let alternative = crate::domain::AlternativeTitle {
        id: String::new(),
        title: title.to_string(),
        title_type: request.title_type,
        language: request.language,
        is_manual: true,
    };

    let created = repo::child::add_alternative_title(&state.db, &id, &alternative).await?;

    finish(
        &state,
        &identity,
        &ip,
        &id,
        &format!("added alternative title {title:?}"),
    )
    .await;

    Ok((StatusCode::CREATED, Json(Created { id: created })))
}

/// Remove an alternative title that was added by hand.
#[utoipa::path(
    delete, path = "/items/{id}/alternative-titles/{title_id}", tag = TAG,
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("title_id" = String, Path, description = "The title's identifier"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such title, or it came from a provider"),
    ),
)]
async fn remove_alternative_title(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, title_id)): Path<(String, String)>,
) -> AppResult<StatusCode> {
    writable(&state, &identity, &id).await?;

    if !repo::child::remove_alternative_title(&state.db, &id, &title_id).await? {
        return Err(AppError::NotFound);
    }

    finish(&state, &identity, &ip, &id, "removed an alternative title").await;

    Ok(StatusCode::NO_CONTENT)
}

// ─── shared ──────────────────────────────────────────────────────────────────

/// Check the caller may write, and that the work exists. Returns it loaded.
async fn writable(
    state: &AppState,
    identity: &Identity,
    id: &str,
) -> AppResult<crate::domain::MediaItem> {
    if !identity.can_write() {
        return Err(AppError::Forbidden);
    }

    service::load(state, id).await?.ok_or(AppError::NotFound)
}

/// An image URL has to be something a client can actually fetch.
fn check_url(url: &str) -> AppResult<()> {
    let trimmed = url.trim();

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "image URLs must start with http:// or https://".into(),
        ))
    }
}

/// Drop the cached copy and record what happened.
async fn finish(state: &AppState, identity: &Identity, ip: &ClientIp, id: &str, detail: &str) {
    state.caches.items.invalidate(&format!("item:{id}")).await;

    audit::record(
        state,
        Event {
            identity: Some(identity),
            ip,
            action: Action::ItemUpdated,
            target: Some(id),
            detail: Some(detail),
        },
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_fetchable_image_urls_are_accepted() {
        assert!(check_url("https://example.invalid/p.jpg").is_ok());
        assert!(check_url("http://example.invalid/p.jpg").is_ok());
        assert!(check_url("  https://example.invalid/p.jpg  ").is_ok());

        // A path, a data URI or a file URL would render as a broken image in
        // every client that consumes this.
        assert!(check_url("/local/poster.jpg").is_err());
        assert!(check_url("data:image/png;base64,AAAA").is_err());
        assert!(check_url("file:///etc/passwd").is_err());
        assert!(check_url("").is_err());
    }
}
