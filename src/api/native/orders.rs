//! The other orders a series' episodes come in, for a reader who knows it
//! that way.

use axum::{
    Extension, Json,
    extract::{Path, State},
};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    auth::Identity,
    db::repo,
    domain::EpisodeOrder,
    error::{AppError, AppResult},
    state::AppState,
};

const TAG: &str = super::items::TAG;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(orders))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Orders {
    /// Each order TheTVDB keeps besides the aired one — `dvd`, `absolute`,
    /// `alternate`, `regional` — with the work's episodes placed in it by
    /// their TVDB id. Empty for a film, or a series numbered one way only.
    pub orders: Vec<EpisodeOrder>,
}

/// How else a series' episodes are numbered, as TheTVDB keeps them. The aired
/// order is the work's own and the one every client is served; these are for
/// a reader who knows the series by its DVDs, or straight through.
#[utoipa::path(
    get, path = "/items/{id}/orders", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier")),
    responses(
        (status = 200, body = Orders),
        (status = 404, description = "No such work"),
    ),
)]
async fn orders(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<Orders>> {
    let item = repo::item::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    // As the work's own page decides: switched off, or kept from this caller
    // by the adult policy, it is not here either.
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), Some(true));
    let hidden = !item.is_enabled || (item.is_adult && !adult);
    if hidden && !identity.can_write() {
        return Err(AppError::NotFound);
    }

    Ok(Json(Orders {
        orders: repo::order::list(&state.db, &id).await?,
    }))
}
