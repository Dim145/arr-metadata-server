//! What each source gives a work, and which wins.

use axum::{Extension, Json, extract::State};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    auth::Identity,
    error::AppResult,
    merge::rules,
    providers::{mal, names},
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
    /// MyAnimeList's alone: which of its ways in answers, as this instance
    /// last found them — Jikan passed over while Tenrai serves, or none
    /// answering.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reach: Option<mal::Reach>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SourceRules {
    /// Every provider, in the order the merge consults them: the
    /// `sources.priority` setting, as it stands.
    pub providers: Vec<RankedProvider>,
    /// Every provider in the order the server started with, from
    /// `AMS_PROVIDER_PRIORITY`: what the order can be put back to.
    pub default_order: Vec<&'static str>,
    /// A row per thing a work holds: who supplies it, in the order their
    /// values are taken, and by which rule.
    pub rows: Vec<rules::Laid>,
}

/// The merge's rules: who gives what, in which order, and who replaces the
/// rest. For the people who keep the catalogue.
///
/// The order is the `sources.priority` setting, which every merge reads when
/// it runs: a work takes a new order at its next refresh.
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
    identity.require_write()?;

    let priority = state.provider_priority();
    let providers = rules::order(&priority)
        .into_iter()
        .enumerate()
        .map(|(at, id)| RankedProvider {
            id,
            rank: at + 1,
            on: gather::switched_on(&state, id),
            reach: (id == names::MAL).then(|| state.mal.reach(state.flag("mal.fallback", true))),
        })
        .collect();

    Ok(Json(SourceRules {
        providers,
        default_order: rules::order(&state.config.provider_priority),
        rows: rules::laid_out(&priority),
    }))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{
        domain::{MediaItem, MediaKind},
        error::AppError,
        merge::{self, Contribution},
        settings::registry::Scope,
    };

    /// Authentication switched off: an administrator, as far as a guard asks.
    fn admin() -> Extension<Identity> {
        Extension(Identity::Anonymous)
    }

    async fn set(state: &AppState, key: &str, value: &str) -> bool {
        state
            .settings
            .set(Scope::Server, "", key, value, None)
            .await
            .is_ok()
    }

    /// Two answers for one work, TMDB's and TheTVDB's, each naming it its own way.
    fn answers() -> Vec<Contribution> {
        ["tmdb", "tvdb"]
            .into_iter()
            .map(|provider| {
                let mut item = MediaItem::empty(MediaKind::Series);
                item.title = format!("as {provider} names it");
                Contribution {
                    provider: provider.into(),
                    item,
                }
            })
            .collect()
    }

    #[tokio::test]
    async fn the_order_is_the_setting_and_the_merge_reads_it_when_it_runs() {
        let state = crate::api::native::testing::server().await;
        let started = rules::order(&state.config.provider_priority);

        // Started with the environment's order, every provider named.
        let Json(before) = source_rules(State(state.clone()), admin())
            .await
            .expect("laid out");
        let ids: Vec<&str> = before.providers.iter().map(|p| p.id).collect();
        assert_eq!(ids, started);
        assert_eq!(before.default_order, started);

        // A new order, set while the server runs, is the next merge's.
        assert!(set(&state, "sources.priority", "fanart,tvdb").await);
        assert_eq!(state.provider_priority(), ["fanart", "tvdb"]);
        let merged = merge::combine(answers(), &state.provider_priority()).expect("merged");
        assert_eq!(merged.title, "as tvdb names it");

        let Json(after) = source_rules(State(state.clone()), admin())
            .await
            .expect("laid out");
        assert_eq!(after.providers[0].id, "fanart");
        assert_eq!(after.providers[1].id, "tvdb");
        assert_eq!(after.providers[1].rank, 2);
        assert_eq!(after.providers.len(), rules::PROVIDERS.len());
        let title = after
            .rows
            .iter()
            .find(|r| r.field == "title")
            .expect("a title row");
        assert_eq!(title.suppliers[0], "tvdb");
        // What it can be put back to has not moved.
        assert_eq!(after.default_order, started);

        // Put back, TMDB names it again.
        assert!(set(&state, "sources.priority", &started.join(",")).await);
        let merged = merge::combine(answers(), &state.provider_priority()).expect("merged");
        assert_eq!(merged.title, "as tmdb names it");

        // A provider the merge does not know, or one named twice, is refused,
        // and the order stands.
        assert!(!set(&state, "sources.priority", "tmdb,imdb").await);
        assert!(!set(&state, "sources.priority", "tvdb,tvdb").await);
        assert_eq!(state.provider_priority(), started);

        // For the people who keep the catalogue.
        assert!(matches!(
            source_rules(State(state), Extension(Identity::Visitor)).await,
            Err(AppError::Forbidden)
        ));
    }

    #[tokio::test]
    async fn the_refresh_intervals_are_read_when_a_work_is_scheduled() {
        let state = crate::api::native::testing::server().await;

        // Started with the environment's.
        assert_eq!(
            state.refresh_ttl(false),
            state.config.refresh.continuing_ttl
        );
        assert_eq!(state.refresh_ttl(true), state.config.refresh.ended_ttl);

        assert!(set(&state, "refresh.ttlContinuing", "3600").await);
        assert!(set(&state, "refresh.ttlEnded", "2592000").await);
        assert_eq!(state.refresh_ttl(false), Duration::from_secs(3_600));
        assert_eq!(
            state.refresh_ttl(true),
            Duration::from_secs(30 * 24 * 3_600)
        );

        // Under a minute is a refresh at every sweep; past the bounds, refused.
        assert!(!set(&state, "refresh.ttlContinuing", "30").await);
        assert!(!set(&state, "refresh.ttlEnded", "315360001").await);
        assert_eq!(state.refresh_ttl(false), Duration::from_secs(3_600));
    }
}
