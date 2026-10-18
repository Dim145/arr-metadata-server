//! Authentication and access control.
//!
//! Policy is set per API surface rather than globally, because the clients
//! differ in what they are able to send. See [`crate::config::SurfacePolicy`].

pub mod csrf;
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
            // `admin:alice`, `editor:bob`, `member:carol`.
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

    /// Whether this identity may read what is kept from readers — a selection
    /// left private, the corrections locked on the works — as against change
    /// it: whoever may write, and a key limited to the `read` scope that
    /// belongs to an editor or an administrator. A tool that only reads —
    /// Kometa, an import list — is given its key in the address it fetches,
    /// and is not to be handed one that could edit the catalogue. A key that
    /// belongs to nobody has no role to read by, and needs the `write` scope.
    pub fn can_read_private(&self) -> bool {
        self.can_write()
            || matches!(
                self,
                Self::Client(c)
                    if c.scopes.iter().any(|s| s == "read")
                        && c.owner.is_some_and(|(role, _)| role >= Role::Editor)
            )
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

    /// A refusal, unless this identity may write ([`Self::can_write`]). What
    /// every handler that writes opens with, rather than a check of its own,
    /// so that all of them refuse alike.
    pub fn require_write(&self) -> AppResult<()> {
        self.can_write().then_some(()).ok_or(AppError::Forbidden)
    }

    /// A refusal, unless this identity may manage clients, users and settings
    /// ([`Self::is_admin`]); see [`Self::require_write`].
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

/// What the tests of the guards, and of the routes behind them, build an
/// identity from.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::db::repo::user::Status;

    /// A key with these scopes — as they stand once its owner's role has
    /// capped them — and, when it is a person's, that person's role.
    pub fn key(scopes: &[&str], owner: Option<Role>) -> Identity {
        Identity::Client(Box::new(ApiClient {
            id: "key".into(),
            name: "Kometa".into(),
            key_prefix: "ams_test".into(),
            scopes: scopes.iter().map(|s| (*s).to_string()).collect(),
            is_enabled: true,
            expires_at: None,
            created_at: "2026-10-16T00:00:00.000Z".into(),
            last_used_at: None,
            last_used_ip: None,
            note: None,
            owner_id: owner.map(|_| "owner".to_string()),
            owner_name: owner.map(|_| "alice".to_string()),
            owner: owner.map(|role| (role, Status::Active)),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::key, *};

    #[test]
    fn a_key_that_only_reads_reads_what_its_editor_or_administrator_does() {
        // An editor's or an administrator's key limited to reading: what a
        // tool that fetches an address is given, and it reads what is kept
        // from readers without being able to change any of it.
        for role in [Role::Editor, Role::Admin] {
            let reads = key(&["read"], Some(role));
            assert!(reads.can_read_private(), "{role}");
            assert!(!reads.can_write(), "{role}");
        }

        // A member's has no such reading to lend; nor has one that belongs to
        // nobody, with no role to read by.
        assert!(!key(&["read"], Some(Role::Member)).can_read_private());
        assert!(!key(&["read"], None).can_read_private());
        // A key whose owner's role took every scope from it is none either.
        assert!(!key(&[], Some(Role::Editor)).can_read_private());

        // As before, a key that may write reads, owned or not.
        assert!(key(&["read", "write"], None).can_read_private());
        assert!(key(&["read", "write"], Some(Role::Editor)).can_read_private());
        assert!(key(&["write"], Some(Role::Editor)).can_read_private());

        // Nobody who presented nothing reads it, unless authentication is off.
        assert!(!Identity::Visitor.can_read_private());
        assert!(!Identity::Network(Some("lan".into())).can_read_private());
        assert!(Identity::Anonymous.can_read_private());
    }
}
