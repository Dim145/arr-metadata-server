//! Settings an operator can change, at the scope they apply to.
//!
//! The environment configures the deployment — addresses, keys, where the
//! database is — and belongs beside the compose file. This configures the
//! behaviour, and belongs in the interface: which language to answer in, how
//! often to refresh, whether adult titles are served. Restarting a metadata
//! server to change the refresh interval is not a reasonable thing to ask.
//!
//! Three scopes, resolved narrowest first:
//!
//! ```text
//!   peer      an allowlist rule — how a client that sends no credential is known
//!    ↳ client an API key
//!       ↳ server
//!          ↳ the environment, which seeded the server scope once
//! ```
//!
//! A peer rather than an address, because a container takes a new address when
//! it restarts and an operator's intent does not. The allowlist rule is already
//! the thing they curate; naming it names the client.

pub mod registry;
pub mod store;

pub use registry::{Definition, REGISTRY, Scope};
pub use store::{Effective, Store};
