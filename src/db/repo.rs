//! Repositories. One module per aggregate; every query goes through
//! [`crate::db::Db::sql`] so it works on both engines.

pub mod anime;
pub mod audit;
pub mod child;
pub mod client;
pub mod imdb;
pub mod import;
pub mod item;
pub mod job;
pub mod network;
pub mod override_field;
pub mod snapshot;
pub mod translation;
pub mod user;
