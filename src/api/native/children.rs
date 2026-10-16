//! Adding and removing a work's children by hand — and hiding or correcting
//! the ones a source gave.
//!
//! Editing a child's *fields* is a different operation: that goes through the
//! override endpoints, which work the same whether the row came from a provider
//! or from a person. What lives here is creation and removal, which overrides
//! cannot express, and the edits a credit, an image or an alternative title a
//! source gave takes: hidden, and a credit's character and place. Each of
//! those is a lock that names the child by what it is (see [`fields::Child`]),
//! set and lifted here by the row it is read in, and outlasting the row a
//! refresh writes again.
//!
//! Only manual rows can be removed. Deleting a provider row would bring it back
//! on the next refresh, which reads as the delete having silently failed; it is
//! hidden instead.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Deserializer, Serialize};
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
    domain::{
        CoverType, CreditType, MediaKind,
        fields::{self, Child, Scope},
    },
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
        .routes(routes!(remove_image, edit_image))
        .routes(routes!(add_credit))
        .routes(routes!(remove_credit, edit_credit))
        .routes(routes!(add_alternative_title))
        .routes(routes!(remove_alternative_title, edit_alternative_title))
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
    registry_checks(
        Scope::Season(request.season_number),
        &[("airDate", request.air_date.as_deref())],
    )?;

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
    registry_checks(
        Scope::Episode {
            season: request.season_number,
            episode: request.episode_number,
        },
        &[
            ("airDate", request.air_date.as_deref()),
            ("image", request.image.as_deref()),
        ],
    )?;
    // Counts, not offsets: Sonarr reads a negative runtime or absolute
    // number as the nonsense it is.
    if request.runtime.is_some_and(|r| r < 0)
        || request.absolute_episode_number.is_some_and(|n| n < 0)
    {
        return Err(AppError::BadRequest(
            "a runtime and an absolute episode number are not negative".into(),
        ));
    }

    let mut episode = repo::child::blank_episode(request.season_number, request.episode_number);
    episode.title = request.title.unwrap_or_default();
    episode.overview = request.overview;
    episode.runtime = request.runtime;
    episode.image = request.image;
    episode.absolute_episode_number = request.absolute_episode_number;

    // A date is what was entered, so a date is what is stored. The UTC form a
    // client expects is derived on the way out when nothing precise is known;
    // storing an invented midnight here would outrank a real time later.
    if let Some(date) = request.air_date {
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
    let item = writable(&state, &identity, &id).await?;
    check_url(&request.url)?;

    // A season's picture is of a season the series has: filed under another
    // number, it was kept and shown nowhere.
    if let Some(number) = request.season_number
        && (item.kind != MediaKind::Series
            || number < 0
            || !item.seasons.iter().any(|s| s.season_number == number))
    {
        return Err(AppError::BadRequest(format!(
            "season {number} is not a season of this work"
        )));
    }
    let language = crate::api::extract::filed_language(request.language)?;

    let cover_type: CoverType = request.cover_type.parse().unwrap_or(CoverType::Unknown);

    let mut image = repo::child::blank_image(cover_type, request.url.clone());
    image.season_number = request.season_number;
    image.language = language;
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

    // An upload's file goes with its row.
    let url = repo::child::image_url(&state.db, &id, &image_id).await?;
    if !repo::child::remove_image(&state.db, &id, &image_id).await? {
        return Err(AppError::NotFound);
    }
    super::media::forget_uploads(&state, url.as_deref()).await;

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
    let language = crate::api::extract::filed_language(request.language)?;

    // The database has a unique index over (media_id, title, language); catching
    // it here gives a usable message instead of a 500.
    if item
        .alternative_titles
        .iter()
        .any(|t| t.title == title && t.language == language)
    {
        return Err(AppError::Conflict(format!("{title:?} is already listed")));
    }

    let alternative = crate::domain::AlternativeTitle {
        id: String::new(),
        title: title.to_string(),
        title_type: request.title_type,
        language,
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

// ─── what the sources gave, hidden or corrected ──────────────────────────────

/// Whether an image or an alternative title a source gave is hidden.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HiddenEdit {
    /// `true` keeps it out of every answer; `false` gives it back.
    pub hidden: bool,
}

/// What may be changed of a credit. Each key left out stands as it is.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreditEdit {
    /// `true` keeps a credit a source gave out of every answer; `false`
    /// gives it back. A credit added by hand is removed instead.
    #[serde(default)]
    pub hidden: Option<bool>,
    /// The character played: a string corrects it — an empty one says there
    /// is none — and `null` lifts the correction.
    #[serde(default, deserialize_with = "given")]
    #[schema(value_type = Option<String>)]
    pub character_name: Option<Option<String>>,
    /// Its place among the credits of its kind, lower first: a number moves
    /// it, `null` lifts the correction.
    #[serde(default, deserialize_with = "given")]
    #[schema(value_type = Option<i32>)]
    pub sort_order: Option<Option<i32>>,
}

/// Absent is `None`; `null` is `Some(None)`; a value is `Some(Some(_))`.
fn given<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// Hide a credit a source gave, give it back, or correct it: the character
/// played, its place among the credits of its kind.
///
/// Each is a lock on the credit by who the person is and what they did —
/// their TMDB id, or their name where the source gave none — so it holds
/// through every refresh, is counted and lifted like any lock, and reaches
/// every answer: Sonarr's, Radarr's, the `.nfo`, the person's own page. A
/// credit added by hand is corrected in its row, and removed rather than
/// hidden.
#[utoipa::path(
    patch, path = "/items/{id}/credits/{credit_id}", tag = TAG,
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("credit_id" = String, Path, description = "The credit's identifier, as the work is read"),
    ),
    request_body = CreditEdit,
    responses(
        (status = 204, description = "Changed"),
        (status = 400, description = "Nothing to change, a credit added by hand hidden, or an order that is no number"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work or credit"),
    ),
)]
async fn edit_credit(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, credit_id)): Path<(String, String)>,
    Json(edit): Json<CreditEdit>,
) -> AppResult<StatusCode> {
    let item = writable(&state, &identity, &id).await?;
    let credit = item
        .credits
        .iter()
        .chain(&item.hidden.credits)
        .find(|c| c.id == credit_id)
        .ok_or(AppError::NotFound)?;

    if credit.is_manual {
        if edit.hidden.is_some() {
            return Err(AppError::BadRequest(
                "a credit added by hand is removed, not hidden".into(),
            ));
        }
        let order = match edit.sort_order {
            Some(None) => {
                return Err(AppError::BadRequest(
                    "an order is a number: a credit added by hand keeps one".into(),
                ));
            }
            Some(Some(order)) => Some(order),
            None => None,
        };
        let character = edit
            .character_name
            .as_ref()
            .map(|c| c.as_deref().map(str::trim).filter(|c| !c.is_empty()));
        if character.is_none() && order.is_none() {
            return Err(AppError::BadRequest(
                "nothing to change: characterName or sortOrder".into(),
            ));
        }
        if !repo::child::edit_credit(&state.db, &id, &credit_id, character, order).await? {
            return Err(AppError::NotFound);
        }
        finish(
            &state,
            &identity,
            &ip,
            &id,
            &format!("corrected credit {:?}", credit.person_name),
        )
        .await;
        return Ok(StatusCode::NO_CONTENT);
    }

    let mut changes: Vec<(&str, Option<Value>)> = Vec::new();
    if let Some(hidden) = edit.hidden {
        changes.push(("hidden", hidden.then_some(Value::Bool(true))));
    }
    if let Some(character) = &edit.character_name {
        changes.push((
            "characterName",
            character
                .as_ref()
                .map(|c| Value::String(c.trim().to_string())),
        ));
    }
    if let Some(order) = edit.sort_order {
        changes.push(("sortOrder", order.map(Value::from)));
    }
    if changes.is_empty() {
        return Err(AppError::BadRequest(
            "nothing to change: hidden, characterName or sortOrder".into(),
        ));
    }

    let child = Child::of_credit(credit);
    edit_child(&state, &identity, &ip, &id, &child, &changes).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Hide an image a source gave — the work's or a season's — or give it
/// back.
///
/// A lock on the image by its address as the source gives it, holding
/// through every refresh: Sonarr, Radarr, the `.nfo` and the site are given
/// the work without it. An image added by hand is removed instead; the one
/// chosen to lead with is shown whatever is hidden.
#[utoipa::path(
    patch, path = "/items/{id}/images/{image_id}", tag = TAG,
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("image_id" = String, Path, description = "The image's identifier, as the work is read"),
    ),
    request_body = HiddenEdit,
    responses(
        (status = 204, description = "Changed"),
        (status = 400, description = "An image added by hand"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work or image"),
    ),
)]
async fn edit_image(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, image_id)): Path<(String, String)>,
    Json(edit): Json<HiddenEdit>,
) -> AppResult<StatusCode> {
    let item = writable(&state, &identity, &id).await?;
    let image = item
        .images
        .iter()
        .chain(item.seasons.iter().flat_map(|s| &s.images))
        .chain(&item.hidden.images)
        .find(|i| i.id == image_id)
        .ok_or(AppError::NotFound)?;
    if image.is_manual {
        return Err(AppError::BadRequest(
            "an image added by hand is removed, not hidden".into(),
        ));
    }

    // By the source's address, which the copy kept here is not.
    let child = Child::Image(state.media.unlocalize(&image.url));
    let change = [("hidden", edit.hidden.then_some(Value::Bool(true)))];
    edit_child(&state, &identity, &ip, &id, &child, &change).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Hide an alternative title a source gave, or give it back.
///
/// A lock on the title by its text and the language it is filed under,
/// holding through every refresh: Sonarr and Radarr match release names
/// against these, and a wrong one matches the wrong releases. A title added
/// by hand is removed instead.
#[utoipa::path(
    patch, path = "/items/{id}/alternative-titles/{title_id}", tag = TAG,
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("title_id" = String, Path, description = "The title's identifier, as the work is read"),
    ),
    request_body = HiddenEdit,
    responses(
        (status = 204, description = "Changed"),
        (status = 400, description = "A title added by hand"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work or title"),
    ),
)]
async fn edit_alternative_title(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, title_id)): Path<(String, String)>,
    Json(edit): Json<HiddenEdit>,
) -> AppResult<StatusCode> {
    let item = writable(&state, &identity, &id).await?;
    let title = item
        .alternative_titles
        .iter()
        .chain(&item.hidden.alternative_titles)
        .find(|t| t.id == title_id)
        .ok_or(AppError::NotFound)?;
    if title.is_manual {
        return Err(AppError::BadRequest(
            "a title added by hand is removed, not hidden".into(),
        ));
    }

    let child = Child::of_title(title);
    let change = [("hidden", edit.hidden.then_some(Value::Bool(true)))];
    edit_child(&state, &identity, &ip, &id, &child, &change).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Set or lift the locks an edit of a child comes to — a value sets one,
/// `None` lifts it — each recorded as an edit of a field is. Every value is
/// checked before any is written.
async fn edit_child(
    state: &AppState,
    identity: &Identity,
    ip: &ClientIp,
    id: &str,
    child: &Child,
    changes: &[(&str, Option<Value>)],
) -> AppResult<()> {
    for (field, value) in changes {
        if let Some(value) = value {
            fields::validate_child(child, field, Some(value)).map_err(AppError::BadRequest)?;
        }
    }

    let scope = child.to_string();
    for (field, value) in changes {
        let target = format!("{id}#{scope}/{field}");
        let action = match value {
            Some(value) => {
                super::overrides::lock_child(
                    state,
                    id,
                    child,
                    field,
                    Some(value),
                    &identity.label(),
                )
                .await?;
                Action::OverrideSet
            }
            None => {
                if !repo::override_field::unset_in(&state.db, id, &scope, field, "").await? {
                    continue;
                }
                state.caches.touched(id).await;
                Action::OverrideRemoved
            }
        };
        tracing::info!(%id, %target, actor = %identity.label(), "changed what a source gave");
        audit::record(
            state,
            Event {
                identity: Some(identity),
                ip,
                action,
                target: Some(&target),
                detail: None,
            },
        )
        .await;
    }

    service::listing::after_write(state, id).await;
    Ok(())
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

/// The field registry's own checks, for the fields a child is entered with
/// as for the same fields locked by hand: a day as a day — one typed in as
/// "TBA" was stored, served, and failed in Sonarr — an address a client can
/// follow. Each `(field, value)` given is checked; one left out is not.
fn registry_checks(scope: Scope, given: &[(&str, Option<&str>)]) -> AppResult<()> {
    for (field, value) in given {
        if let Some(value) = value {
            fields::validate(
                scope,
                field,
                Some(&serde_json::Value::String((*value).to_string())),
            )
            .map_err(AppError::BadRequest)?;
        }
    }
    Ok(())
}

/// An image URL has to be something a client can actually fetch.
///
/// And somewhere this server is willing to go. The NFO export downloads every
/// stored image, so an image URL is a request this server makes on somebody
/// else's instruction — which is a fine way to read a cloud instance's
/// credentials, or this server's own administration API on loopback, from a
/// credential that should only be able to name a picture.
fn check_url(url: &str) -> AppResult<()> {
    let trimmed = url.trim();

    // Not one of this server's own: an address that only means something
    // while the copy is kept.
    if crate::media::Media::key_in(trimmed).is_some() {
        return Err(AppError::BadRequest(
            "that is this server's own copy; give the provider's address".into(),
        ));
    }

    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return Err(AppError::BadRequest(
            "image URLs must start with http:// or https://".into(),
        ));
    }

    if crate::outbound::names_internal_host(trimmed) {
        return Err(AppError::BadRequest(
            "that address is not reachable from anywhere but this server".into(),
        ));
    }

    Ok(())
}

/// Drop the cached copy and record what happened.
async fn finish(state: &AppState, identity: &Identity, ip: &ClientIp, id: &str, detail: &str) {
    state.caches.touched(id).await;

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

    use crate::domain::{AlternativeTitle, Credit, Image, MediaItem};

    /// A work as its sources gave it: two posters and a backdrop, two of its
    /// cast, two other titles.
    async fn as_given(state: &AppState, kind: MediaKind, title: &str) -> MediaItem {
        let mut work = MediaItem::empty(kind);
        work.title = title.into();
        work.slug = crate::domain::make_slug(title, Some(1995));
        work.external_ids.tmdb = Some(949);
        work.external_ids.tvdb = Some(81189);
        work.images = [
            ("p1", CoverType::Poster, "https://images.example/p1.jpg", 0),
            ("p2", CoverType::Poster, "https://images.example/p2.jpg", 1),
            ("f1", CoverType::Fanart, "https://images.example/f1.jpg", 0),
        ]
        .into_iter()
        .map(|(id, cover_type, url, sort_order)| Image {
            id: id.into(),
            season_number: None,
            cover_type,
            url: url.into(),
            language: None,
            sort_order,
            source: Some("tmdb".into()),
            is_manual: false,
        })
        .collect();
        work.credits = [
            ("Al Pacino", 1158, "Vincent Hanna", 0),
            ("Robert De Niro", 380, "McCauley", 1),
        ]
        .into_iter()
        .map(|(name, person, character, order)| Credit {
            id: format!("credit-{person}"),
            credit_type: CreditType::Actor,
            person_name: name.into(),
            character_name: Some(character.into()),
            image: None,
            tmdb_person_id: Some(person),
            credit_tmdb_id: Some(format!("52fe4{person}")),
            sort_order: order,
            is_manual: false,
        })
        .collect();
        work.alternative_titles = ["Heat: The Movie", "L.A. Takedown"]
            .into_iter()
            .map(|title| AlternativeTitle {
                id: String::new(),
                title: title.into(),
                title_type: None,
                language: Some("us".into()),
                is_manual: false,
            })
            .collect();
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

    async fn read(state: &AppState, id: &str) -> MediaItem {
        service::load(state, id).await.expect("read").expect("held")
    }

    fn credit_edit(
        hidden: Option<bool>,
        character: Option<Option<&str>>,
        order: Option<Option<i32>>,
    ) -> Json<CreditEdit> {
        Json(CreditEdit {
            hidden,
            character_name: character.map(|c| c.map(str::to_string)),
            sort_order: order,
        })
    }

    /// Hidden, a credit a source gave is in no answer — Radarr's, the `.nfo`,
    /// the person's own page — and kept apart for whoever maintains the
    /// catalogue; corrected, it is played as corrected everywhere; given
    /// back, it is the source's again. Each is a lock, counted as one.
    #[tokio::test]
    async fn a_credit_a_source_gave_is_hidden_and_corrected_in_every_answer() {
        let state = crate::api::native::testing::server().await;
        let film = as_given(&state, MediaKind::Movie, "Heat").await;
        let held = read(&state, &film.id).await;
        let id_of = |name: &str| {
            held.credits
                .iter()
                .find(|c| c.person_name == name)
                .map(|c| c.id.clone())
                .expect("credited")
        };
        let (pacino, de_niro) = (id_of("Al Pacino"), id_of("Robert De Niro"));
        let edit = |credit: String, body: Json<CreditEdit>| {
            edit_credit(
                State(state.clone()),
                Extension(Identity::Anonymous),
                ClientIp::NONE,
                Path((film.id.clone(), credit)),
                body,
            )
        };

        edit(pacino.clone(), credit_edit(Some(true), None, None))
            .await
            .expect("hidden");
        edit(
            de_niro.clone(),
            credit_edit(None, Some(Some("Neil McCauley")), None),
        )
        .await
        .expect("corrected");

        let served = read(&state, &film.id).await;
        let cast = crate::wire::radarr::from_item(&served)
            .credits
            .expect("its credits")
            .cast;
        let played: Vec<(&str, Option<&str>)> = cast
            .iter()
            .map(|c| (c.name.as_str(), c.character.as_deref()))
            .collect();
        assert_eq!(played, [("Robert De Niro", Some("Neil McCauley"))]);
        let nfo = crate::export::nfo::for_item(&served);
        assert!(!nfo.contains("Al Pacino"), "{nfo}");
        assert!(nfo.contains("<role>Neil McCauley</role>"), "{nfo}");
        assert!(
            repo::item::person_credits(&state.db, 1158, false)
                .await
                .expect("read")
                .is_empty(),
            "not one of his on his own page"
        );
        let roles = repo::item::person_credits(&state.db, 380, false)
            .await
            .expect("read");
        assert_eq!(
            roles[0].credit.character_name.as_deref(),
            Some("Neil McCauley")
        );
        assert_eq!(served.hidden.credits.len(), 1, "kept apart for the editor");
        assert_eq!(
            repo::override_field::list(&state.db, &film.id)
                .await
                .expect("read")
                .len(),
            2
        );

        // Given back, and the correction lifted: the source's again.
        edit(pacino, credit_edit(Some(false), None, None))
            .await
            .expect("shown");
        edit(de_niro, credit_edit(None, Some(None), None))
            .await
            .expect("lifted");
        let served = read(&state, &film.id).await;
        let cast = crate::wire::radarr::from_item(&served)
            .credits
            .expect("its credits")
            .cast;
        assert_eq!(cast.len(), 2);
        assert!(
            cast.iter()
                .any(|c| c.character.as_deref() == Some("McCauley"))
        );
        assert!(
            repo::override_field::list(&state.db, &film.id)
                .await
                .expect("read")
                .is_empty()
        );

        // Nothing to change, an order that is no number: refused.
        let nothing = edit(id_of("Al Pacino"), credit_edit(None, None, None)).await;
        assert!(matches!(nothing, Err(AppError::BadRequest(_))));
    }

    /// Hidden, an image a source gave is in no answer — Sonarr's, the
    /// `.nfo`, the catalogue's cards — and kept apart for the editor.
    #[tokio::test]
    async fn an_image_a_source_gave_is_hidden_from_sonarr_and_the_nfo() {
        let state = crate::api::native::testing::server().await;
        let series = as_given(&state, MediaKind::Series, "Breaking Bad").await;
        let held = read(&state, &series.id).await;
        let first = held
            .images
            .iter()
            .find(|i| i.url.ends_with("p1.jpg"))
            .map(|i| i.id.clone())
            .expect("pictured");

        edit_image(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path((series.id.clone(), first.clone())),
            Json(HiddenEdit { hidden: true }),
        )
        .await
        .expect("hidden");

        let served = read(&state, &series.id).await;
        let sonarr = crate::wire::sonarr::from_item(&served, 81189, "en");
        let urls: Vec<&str> = sonarr.images.iter().map(|i| i.url.as_str()).collect();
        assert!(urls.contains(&"https://images.example/p2.jpg"), "{urls:?}");
        assert!(!urls.iter().any(|u| u.ends_with("p1.jpg")), "{urls:?}");
        let nfo = crate::export::nfo::for_item(&served);
        assert!(!nfo.contains("p1.jpg"), "{nfo}");
        assert!(nfo.contains("p2.jpg"), "{nfo}");
        assert_eq!(served.hidden.images.len(), 1);

        // A card of the catalogue, read as every list reads them.
        let mut cards = repo::item::by_ids(&state.db, std::slice::from_ref(&series.id))
            .await
            .expect("listed");
        service::apply_overrides(&state, &mut cards)
            .await
            .expect("locked");
        repo::item::load_artwork(&state.db, &mut cards)
            .await
            .expect("pictured");
        assert!(
            !cards[0].images.iter().any(|i| i.url.ends_with("p1.jpg")),
            "not on its card either"
        );

        // An image added by hand is removed, not hidden.
        let mine = repo::child::add_image(
            &state.db,
            &series.id,
            &repo::child::blank_image(CoverType::Poster, "https://mine.example/p.jpg".into()),
        )
        .await
        .expect("added");
        state.caches.touched(&series.id).await;
        let refused = edit_image(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path((series.id.clone(), mine)),
            Json(HiddenEdit { hidden: true }),
        )
        .await;
        assert!(matches!(refused, Err(AppError::BadRequest(_))));

        // Given back.
        edit_image(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path((series.id.clone(), first)),
            Json(HiddenEdit { hidden: false }),
        )
        .await
        .expect("shown");
        let served = read(&state, &series.id).await;
        assert!(served.images.iter().any(|i| i.url.ends_with("p1.jpg")));
    }

    /// Hidden, an alternative title a source gave is not one Sonarr or
    /// Radarr is given to match releases by.
    #[tokio::test]
    async fn an_alternative_title_a_source_gave_is_hidden() {
        let state = crate::api::native::testing::server().await;
        let film = as_given(&state, MediaKind::Movie, "Heat").await;
        let held = read(&state, &film.id).await;
        let takedown = held
            .alternative_titles
            .iter()
            .find(|t| t.title == "L.A. Takedown")
            .map(|t| t.id.clone())
            .expect("titled");

        edit_alternative_title(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path((film.id.clone(), takedown)),
            Json(HiddenEdit { hidden: true }),
        )
        .await
        .expect("hidden");

        let served = read(&state, &film.id).await;
        let titles: Vec<String> = crate::wire::radarr::from_item(&served)
            .alternative_titles
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(titles, ["Heat: The Movie"]);
        assert_eq!(
            repo::override_field::list(&state.db, &film.id)
                .await
                .expect("read")[0]
                .scope,
            "alternativeTitle:us:L.A. Takedown"
        );
    }
}
