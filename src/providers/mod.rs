//! Upstream metadata providers.
//!
//! Every provider produces two things: a raw payload, stored verbatim as a
//! snapshot, and a canonical [`crate::domain::MediaItem`] mapped from it. Storing
//! both means a mapping bug can be fixed and replayed without re-fetching.

pub mod lang;
pub mod radarr;
pub mod skyhook;
pub mod tmdb;

/// Names used in `media_provider_snapshot.provider` and in merge priority.
pub mod names {
    pub const TMDB: &str = "tmdb";
    pub const SKYHOOK: &str = "skyhook";
    pub const RADARR: &str = "radarr";
}
