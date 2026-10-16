//! The operator's own Sonarr and Radarr, reached the other way.
//!
//! Everywhere else they ask and this server answers. Here it asks them: it
//! tells them a work changed, so that a correction made here reaches them at
//! once rather than at their next refresh ([`push`]); it reads what they hold
//! to fill the catalogue before they are pointed here ([`seed`]). And of
//! every write of a series it says, in the trail, when TheTVDB moved
//! episodes under them ([`renumber`]) — the one change Sonarr takes in
//! without a word, its files then matched to other episodes.
//!
//! Each is reached at the address and with the key an administrator set on
//! the Sonarr & Radarr page (`arr.sonarrUrl`, `arr.sonarrKey`, `arr.radarrUrl`,
//! `arr.radarrKey`); see [`client`] for which addresses those may be.

pub mod client;
pub mod history;
pub mod push;
pub mod renumber;
pub mod seed;

pub use client::{App, Connection};
