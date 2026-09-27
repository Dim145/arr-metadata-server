//! Repositories. One module per aggregate; every query goes through
//! [`crate::db::Db::sql`] so it works on both engines.

pub mod anime;
pub mod asset;
pub mod audit;
pub mod child;
pub mod client;
pub mod imdb;
pub mod import;
pub mod invitation;
pub mod item;
pub mod job;
pub mod keystore;
pub mod list;
pub mod network;
pub mod order;
pub mod override_field;
pub mod season;
pub mod snapshot;
pub mod translation;
pub mod user;
