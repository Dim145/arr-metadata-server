//! The other orders a series' episodes come in, read from TheTVDB when a
//! series is fetched and kept beside its aired order — never in it.

use crate::{
    db::repo,
    domain::{MediaItem, MediaKind},
    state::AppState,
};

/// Ask TheTVDB how else it numbers this series, and keep the answer. Best
/// effort: a failure is logged, and what was kept before stays.
pub async fn gather(state: &AppState, item: &MediaItem) {
    if item.kind != MediaKind::Series || !state.tvdb.is_enabled() {
        return;
    }
    let Some(tvdb_id) = item.external_ids.tvdb else {
        return;
    };

    let orders = match state.tvdb.orders(tvdb_id).await {
        Ok(orders) => orders,
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "TheTVDB's other episode orders could not be read"
            );
            return;
        }
    };

    if let Err(e) = repo::order::replace(&state.db, &item.id, &orders).await {
        tracing::warn!(id = %item.id, error = %e, "could not keep the episode orders");
    }
}
