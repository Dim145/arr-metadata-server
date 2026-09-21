//! Authentication and access control.
//!
//! Policy is set per API surface rather than globally, because the clients
//! differ in what they are able to send. See [`crate::config::SurfacePolicy`].

pub mod ip;
pub mod middleware;
pub mod naming;
pub mod ratelimit;
pub mod secrets;

use crate::db::repo::{client::ApiClient, user::AdminUser};

/// Who made a request. Inserted into the request extensions by the guards.
#[derive(Clone, Debug)]
pub enum Identity {
    /// A named API key.
    Client(Box<ApiClient>),
    /// A signed-in administrator.
    Admin(Box<AdminUser>),
    /// Allowed by network policy, with no credential presented.
    Network,
    /// Someone browsing the catalogue with no credential, when public browsing
    /// is switched on. Reads a fixed set of paths and nothing else.
    Visitor,
    /// Authentication is switched off for this surface.
    Anonymous,
}

impl Identity {
    pub fn label(&self) -> String {
        match self {
            Self::Client(c) => format!("client:{}", c.name),
            Self::Admin(u) => format!("admin:{}", u.username),
            Self::Network => "network".to_string(),
            Self::Visitor => "visitor".to_string(),
            Self::Anonymous => "anonymous".to_string(),
        }
    }

    /// Whether this identity may perform writes through the native API.
    ///
    /// An administrator always may. A key must carry the `write` scope. A caller
    /// that presented nothing may only when authentication is off entirely.
    pub fn can_write(&self) -> bool {
        match self {
            Self::Admin(_) | Self::Anonymous => true,
            Self::Client(c) => c.scopes.iter().any(|s| s == "write" || s == "admin"),
            Self::Network | Self::Visitor => false,
        }
    }

    /// Whether this identity may manage clients, users and settings.
    pub fn is_admin(&self) -> bool {
        match self {
            Self::Admin(u) => u.is_admin,
            Self::Anonymous => true,
            Self::Client(c) => c.scopes.iter().any(|s| s == "admin"),
            Self::Network | Self::Visitor => false,
        }
    }
}
