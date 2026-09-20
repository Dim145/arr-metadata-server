//! Repositories. One module per aggregate; every query goes through
//! [`crate::db::Db::sql`] so it works on both engines.

pub mod audit;
pub mod client;
pub mod item;
pub mod override_field;
pub mod snapshot;
pub mod user;
