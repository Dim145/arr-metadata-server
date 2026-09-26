//! Authentication and access control.
//!
//! Policy is set per API surface rather than globally, because the clients
//! differ in what they are able to send. See [`crate::config::SurfacePolicy`].

pub mod ip;
pub mod middleware;
pub mod naming;
pub mod oidc;
pub mod ratelimit;
pub mod secrets;

use crate::{
    db::repo::{
        client::ApiClient,
        user::{Role, User},
    },
    error::{AppError, AppResult},
};

/// Who made a request. Inserted into the request extensions by the guards.
#[derive(Clone, Debug)]
pub enum Identity {
    /// A named API key.
    Client(Box<ApiClient>),
    /// A signed-in person, whatever their role.
    User(Box<User>),
    /// Allowed by network policy, with no credential presented. Carries the
    /// rule that let it through, which is what per-client settings hang off:
    /// Sonarr and Radarr send nothing that tells them apart except the address
    /// they call from.
    Network(Option<String>),
    /// Someone browsing the catalogue with no credential, when public browsing
    /// is switched on. Reads a fixed set of paths and nothing else.
    Visitor,
    /// Authentication is switched off for this surface.
    Anonymous,
}

impl Identity {
    pub fn label(&self) -> String {
        match self {
            // Owned keys read as whose they are: two people may each call
            // theirs "Sonarr".
            Self::Client(c) => match &c.owner_name {
                Some(owner) => format!("client:{owner}/{}", c.name),
                None => format!("client:{}", c.name),
            },
            // The role at the time, so the journal says who could do what:
            // `admin:dim145`, `editor:margaux`, `member:leo`.
            Self::User(u) => format!("{}:{}", u.role.as_str(), u.username),
            Self::Network(Some(rule)) => format!("peer:{rule}"),
            Self::Network(None) => "network".to_string(),
            Self::Visitor => "visitor".to_string(),
            Self::Anonymous => "anonymous".to_string(),
        }
    }

    /// Whether this identity may perform writes through the native API.
    ///
    /// An editor or an administrator may. A key must carry the `write` scope —
    /// and an owned key carries no more than its owner's role grants. A caller
    /// that presented nothing may only when authentication is off entirely.
    pub fn can_write(&self) -> bool {
        match self {
            Self::User(u) => u.role >= Role::Editor,
            Self::Anonymous => true,
            Self::Client(c) => c.scopes.iter().any(|s| s == "write" || s == "admin"),
            Self::Network(_) | Self::Visitor => false,
        }
    }

    /// Whether this identity may manage clients, users and settings.
    pub fn is_admin(&self) -> bool {
        match self {
            Self::User(u) => u.role == Role::Admin,
            Self::Anonymous => true,
            Self::Client(c) => c.scopes.iter().any(|s| s == "admin"),
            Self::Network(_) | Self::Visitor => false,
        }
    }

    /// The API key this is, if it is one. Settings resolve against it.
    pub fn client_id(&self) -> Option<&str> {
        match self {
            Self::Client(c) => Some(&c.id),
            _ => None,
        }
    }

    /// The person whose rights this request carries: the one signed in, or
    /// the owner of the key it came with. An administrator's own key must not
    /// do to them what they could not do to themselves.
    pub fn person_id(&self) -> Option<&str> {
        match self {
            Self::User(u) => Some(&u.id),
            Self::Client(c) => c.owner_id.as_deref(),
            _ => None,
        }
    }

    /// Whether this is a member, in person or through their key. A member
    /// reads what a visitor may, and keeps their own account.
    pub fn is_member(&self) -> bool {
        match self {
            Self::User(u) => u.role == Role::Member,
            Self::Client(c) => c.owner.is_some_and(|(role, _)| role == Role::Member),
            _ => false,
        }
    }

    /// The signed-in person, when there is one.
    pub fn user(&self) -> Option<&User> {
        match self {
            Self::User(u) => Some(u),
            _ => None,
        }
    }

    /// The signed-in person, or a refusal: what a person does for themselves
    /// — their profile, their keys — a key cannot do in their name.
    pub fn require_user(&self) -> AppResult<&User> {
        match self {
            Self::User(u) => Ok(u),
            Self::Visitor | Self::Network(_) => Err(AppError::Unauthorized),
            Self::Client(_) | Self::Anonymous => Err(AppError::Forbidden),
        }
    }

    pub fn require_admin(&self) -> AppResult<()> {
        self.is_admin().then_some(()).ok_or(AppError::Forbidden)
    }

    /// The allowlist rule this came through, if any.
    pub fn peer_id(&self) -> Option<&str> {
        match self {
            Self::Network(rule) => rule.as_deref(),
            _ => None,
        }
    }
}
