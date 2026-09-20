//! On-the-wire formats of the clients this server impersonates.
//!
//! Each submodule owns one client's vocabulary and converts in both directions:
//! canonical → wire to answer a request, wire → canonical to absorb a fallback
//! response from the real upstream. Keeping both directions together means the
//! field mapping is stated once.

pub mod radarr;
pub mod sonarr;
