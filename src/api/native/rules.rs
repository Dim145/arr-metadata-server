//! What each source gives a work, and which wins.

use axum::{Extension, Json, extract::State};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    auth::Identity,
    error::{AppError, AppResult},
    merge::rules,
    service::gather,
    state::AppState,
};

const TAG: &str = super::items::TAG;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(source_rules))
}

/// One provider, where the merge ranks it, and whether it answers now.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RankedProvider {
    pub id: &'static str,
    /// Its place in the order the merge consults providers, from 1.
    pub rank: usize,
    /// Switched on and able to answer.
    pub on: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SourceRules {
    /// Every provider, in the order the merge consults them.
    pub providers: Vec<RankedProvider>,
    /// A row per thing a work holds: who supplies it, in the order their
    /// values are taken, and by which rule.
    pub rows: Vec<rules::Laid>,
}

/// The merge's rules: who gives what, in which order, and who replaces the
/// rest. For the people who keep the catalogue.
#[utoipa::path(
    get, path = "/sources/rules", tag = TAG,
    responses(
        (status = 200, body = SourceRules),
        (status = 403, description = "The caller may not write"),
    ),
)]
async fn source_rules(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<SourceRules>> {
    if !identity.can_write() {
        return Err(AppError::Forbidden);
    }

    let priority = &state.config.provider_priority;
    let providers = rules::order(priority)
        .into_iter()
        .enumerate()
        .map(|(at, id)| RankedProvider {
            id,
            rank: at + 1,
            on: gather::switched_on(&state, id),
        })
        .collect();

    Ok(Json(SourceRules {
        providers,
        rows: rules::laid_out(priority),
    }))
}
