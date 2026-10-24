//! Reads and writes for works and their children.
//!
//! Writes distinguish **provider-sourced** rows from **manual** ones: a refresh
//! deletes and re-inserts only `is_manual = 0` children, so anything a human
//! added by hand survives every refresh. Manual *edits* to provider rows live in
//! `media_override` and are applied on read, never written back here.
//!
//! In six parts, each re-exported here whole, so that a caller names
//! `repo::item::search` and never the part it lives in: `read`, one work and
//! what hangs off it; `listed`, what a work is listed by; `search`, the
//! predicate every list and count is narrowed by; `across`, reads across
//! works; `write`; and `refresh`, the refresh's bookkeeping.

mod across;
mod listed;
mod merge;
mod read;
mod refresh;
mod search;
#[cfg(test)]
mod testing;
mod write;

pub use across::*;
pub use listed::*;
pub use merge::*;
pub use read::*;
pub use refresh::*;
pub use search::*;
pub use write::*;
