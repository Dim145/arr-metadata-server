//! The lists downloaded whole, as the Sources page shows them, and what
//! IMDb's episodes say of one work's.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::native::meta::ListImport,
    auth::Identity,
    db::repo::{self, job::Status},
    error::{AppError, AppResult},
    jobs::{
        datasets,
        tasks::{self, TaskId},
    },
    state::AppState,
};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(imdb_episodes))
}

/// The lists in the order the Sources page shows them: IMDb's three, then
/// the anime identifier list.
const SHOWN: [&str; 4] = [
    datasets::IMDB,
    datasets::IMDB_AKAS,
    datasets::IMDB_EPISODES,
    datasets::ANIME,
];

/// A list downloaded whole, as the Sources page says of it.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Dataset {
    /// `imdb-ratings`, `imdb-akas` — IMDb's titles by region —,
    /// `imdb-episodes` or `anime-lists`.
    pub id: &'static str,
    /// The task that downloads it, on the Jobs page.
    pub task: TaskId,
    /// Whether a switch that needs it is on.
    pub on: bool,
    /// Seconds between two downloads, by its own cadence.
    pub every_seconds: i64,
    /// When it last landed, and how many of its rows were kept; absent
    /// until it has landed once.
    #[schema(required)]
    pub imported: Option<ListImport>,
    /// How its last run ended, whether it landed or not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<Status>,
    /// When its last run ended, or began while it runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    /// When the schedule downloads it next, while it is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_at: Option<String>,
}

/// The lists the further sources work from: whether each is on, when it
/// last landed and how much of it was kept, how its last run went, and
/// when it is downloaded next. For whoever keeps the catalogue, as the
/// sources' rules are; a run's error, which can name hosts, is on the Jobs
/// page, for an administrator.
#[utoipa::path(
    get, path = "/sources/datasets", tag = super::items::TAG,
    operation_id = "datasets_list",
    responses(
        (status = 200, body = Vec<Dataset>),
        (status = 403, description = "The caller may not write"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Vec<Dataset>>> {
    identity.require_write()?;

    let mut out = Vec::with_capacity(SHOWN.len());
    for list in SHOWN {
        let task = TaskId::named(datasets::kind_of(list))
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("{list} has no task")))?;
        let standing = tasks::state_named(&state, task).await?;
        let last = standing.last.as_ref();
        out.push(Dataset {
            id: list,
            task,
            on: datasets::wanted(&state, list),
            every_seconds: datasets::every(list).num_seconds(),
            imported: repo::import::get(&state.db, list)
                .await?
                .map(|import| ListImport {
                    imported_at: import.imported_at,
                    rows: import.row_count,
                }),
            last_status: last.map(|run| run.status),
            last_run_at: last.and_then(|run| run.finished_at.clone().or(run.started_at.clone())),
            next_at: standing.next_at,
        });
    }

    Ok(Json(out))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EpisodesQuery {
    /// One season's episodes alone.
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub season: Option<i32>,
}

/// What IMDb's episodes say of a work's.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImdbEpisodes {
    /// Whether IMDb's episodes are read (`imdb.episodes`): while they are
    /// not, the list is empty.
    pub on: bool,
    /// Each episode IMDb numbers as the work does, in their order.
    pub episodes: Vec<ImdbEpisode>,
}

/// One episode of a work, and the one of IMDb's it is matched to.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImdbEpisode {
    /// The episode's id in the catalogue.
    pub episode_id: String,
    pub season_number: i32,
    pub episode_number: i32,
    /// IMDb's id of the episode: `tt0959621`.
    pub imdb_id: String,
    /// IMDb's rating of it, out of ten, while IMDb's ratings are read
    /// (`imdb.enabled`) and it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    /// How many votes the rating is of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub votes: Option<i64>,
    /// How it was matched to IMDb's.
    pub matched_by: MatchedBy,
}

/// How an episode was matched to one of IMDb's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = ImdbMatch)]
pub enum MatchedBy {
    /// By the season and the number the episode has in the official order,
    /// against those IMDb gives its own: right as long as IMDb counts the
    /// episodes the same way, and less sure than an id a source gives the
    /// episode itself — which none of TMDB's and TheTVDB's answers read
    /// here does.
    Numbers,
}

/// Which of IMDb's episodes each of a work's is, with IMDb's rating of it.
///
/// Matched by the season and the number the episode has in the official
/// order, against the ones IMDb gives its own — which IMDb may count
/// otherwise: the page says the match is the less sure for it. Read from
/// IMDb's episodes list (`imdb.episodes`) and its ratings (`imdb.enabled`),
/// both downloaded here, never asked of IMDb as the page is read.
#[utoipa::path(
    get, path = "/items/{id}/imdb/episodes", tag = super::items::TAG,
    operation_id = "datasets_imdb_episodes",
    params(
        ("id" = String, Path, description = "The work's identifier"),
        EpisodesQuery,
    ),
    responses(
        (status = 200, body = ImdbEpisodes),
        (status = 404, description = "No such work, or none this caller may see"),
    ),
)]
async fn imdb_episodes(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Query(query): Query<EpisodesQuery>,
) -> AppResult<Json<ImdbEpisodes>> {
    let item = super::visible_work(&state, &identity, &id).await?;

    let episodes = crate::service::imdb::episodes(&state, &item.id, query.season)
        .await?
        .into_iter()
        .map(|found| ImdbEpisode {
            episode_id: found.episode_id,
            season_number: found.season_number,
            episode_number: found.episode_number,
            imdb_id: found.tconst,
            rating: found.rating,
            votes: found.votes,
            matched_by: MatchedBy::Numbers,
        })
        .collect();

    Ok(Json(ImdbEpisodes {
        on: state.flag("imdb.episodes", false),
        episodes,
    }))
}
