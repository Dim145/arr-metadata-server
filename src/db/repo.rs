//! Repositories. One module per aggregate; every query goes through
//! [`crate::db::Db::sql`] so it works on both engines.

pub mod alias;
pub mod anime;
pub mod asset;
pub mod audit;
pub mod change;
pub mod child;
pub mod client;
pub mod editorial;
pub mod genre;
pub mod imdb;
pub mod import;
pub mod import_rule;
pub mod invitation;
pub mod item;
pub mod job;
pub mod keystore;
pub mod list;
pub mod network;
pub mod order;
pub mod override_field;
pub mod person;
pub mod preference;
pub mod release;
pub mod scene;
pub mod search;
pub mod season;
pub mod snapshot;
pub mod task_schedule;
pub mod translation;
pub mod user;
pub mod webhook;
