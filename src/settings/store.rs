//! Reading and writing settings, and resolving them across scopes.
//!
//! Every setting is read on a request path, so the whole set is held in memory
//! and refreshed when it changes rather than queried per read: there are a few
//! dozen of them and they move about once a month.

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    db::{Db, RowExt, now},
    settings::registry::{self, Definition, Scope},
};

/// One setting as the interface shows it: what it is, and where the answer
/// currently comes from.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Effective {
    pub key: String,
    pub value: String,
    /// The scope the value was found at — `server` unless something narrower
    /// overrode it.
    pub source: String,
    /// Whether this scope sets it itself, as opposed to inheriting.
    pub overridden: bool,
}

/// `(scope, scope_id, key)` — what a setting is filed under.
type Address = (String, String, String);

/// Why a setting was not written.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// What was asked cannot be: no such setting, a scope it is not set at,
    /// a value it cannot hold. The caller is told, as a 400.
    #[error("{0}")]
    Invalid(String),
    /// The database failed. Nothing the caller asked was wrong: a 500, and
    /// an error in the log, rather than a raw driver message sent back.
    #[error(transparent)]
    Storage(#[from] anyhow::Error),
}

impl From<WriteError> for crate::error::AppError {
    fn from(e: WriteError) -> Self {
        match e {
            WriteError::Invalid(why) => Self::BadRequest(why),
            WriteError::Storage(cause) => Self::from(cause),
        }
    }
}

/// The settings whose values are secrets wherever they turn up in a log:
/// the identity provider's client secret, and the address webhooks are
/// posted to, which for most services carries its token in its path.
const LOGGED_SECRETS: [&str; 2] = ["oidc.clientSecret", "webhooks.url"];

#[derive(Clone)]
pub struct Store {
    db: Db,
    /// The whole table, which is a few dozen rows.
    values: Arc<RwLock<HashMap<Address, String>>>,
    /// Held across write-then-reload, so two writers cannot install their
    /// snapshots in the opposite order to the one they read them in.
    ///
    /// Without it: A writes `hidden` and reads it back, B writes `visible` and
    /// reads it back, B installs, A installs. The table says `visible`, the map
    /// says `hidden`, every decision is made from the map — and the next
    /// restart reloads the table and flips the server's adult policy with
    /// nobody having touched it. Settings are written when somebody clicks a
    /// switch, so serialising them costs nothing.
    writes: Arc<tokio::sync::Mutex<()>>,
}

impl Store {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            values: Arc::new(RwLock::new(HashMap::new())),
            writes: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Read the table into memory. Called at boot and after every write.
    pub async fn reload(&self) -> Result<()> {
        let rows = sqlx::query(
            self.db
                .sql("SELECT scope, scope_id, key, value FROM setting"),
        )
        .fetch_all(self.db.pool())
        .await?;

        let mut fresh = HashMap::with_capacity(rows.len());
        for row in &rows {
            fresh.insert(
                (row.text("scope")?, row.text("scope_id")?, row.text("key")?),
                row.text("value")?,
            );
        }

        for ((_, _, key), value) in &fresh {
            if LOGGED_SECRETS.contains(&key.as_str()) {
                hide_from_logs(value);
            }
        }

        *self.values.write() = fresh;
        Ok(())
    }

    /// The value at one scope exactly, without inheriting.
    pub fn at(&self, scope: Scope, scope_id: &str, key: &str) -> Option<String> {
        self.values
            .read()
            .get(&(
                scope.as_str().to_string(),
                scope_id.to_string(),
                key.to_string(),
            ))
            .cloned()
    }

    /// The value that applies, narrowest scope first.
    ///
    /// `client` and `peer` are the ids of whichever the caller was identified
    /// as, if either. The fallback is the server scope, which the environment
    /// seeded, so there is always an answer.
    pub fn resolve(&self, key: &str, client: Option<&str>, peer: Option<&str>) -> Option<String> {
        if let Some(id) = peer
            && let Some(value) = self.at(Scope::Peer, id, key)
        {
            return Some(value);
        }

        if let Some(id) = client
            && let Some(value) = self.at(Scope::Client, id, key)
        {
            return Some(value);
        }

        self.at(Scope::Server, "", key)
    }

    pub fn bool_at(&self, key: &str, client: Option<&str>, peer: Option<&str>) -> Option<bool> {
        self.resolve(key, client, peer).and_then(|v| v.parse().ok())
    }

    pub fn int_at(&self, key: &str, client: Option<&str>, peer: Option<&str>) -> Option<i64> {
        self.resolve(key, client, peer).and_then(|v| v.parse().ok())
    }

    /// Everything settable at a scope, with where each answer comes from.
    pub fn effective(&self, scope: Scope, scope_id: &str) -> Vec<Effective> {
        registry::REGISTRY
            .iter()
            .filter(|def| def.scopes.contains(&scope))
            .filter_map(|def| self.one(def, scope, scope_id))
            .collect()
    }

    fn one(&self, def: &Definition, scope: Scope, scope_id: &str) -> Option<Effective> {
        if let Some(value) = self.at(scope, scope_id, def.key) {
            return Some(Effective {
                key: def.key.to_string(),
                // Set, and never shown: what it is stays on the server.
                value: if def.kind.is_secret() {
                    registry::MASK.to_string()
                } else {
                    value
                },
                source: scope.as_str().to_string(),
                overridden: true,
            });
        }

        // Not set here: show what would apply, so the interface can say
        // "inherited: French" rather than leaving the field blank.
        let inherited = match scope {
            Scope::Server => None,
            Scope::Client | Scope::Peer => self.at(Scope::Server, "", def.key),
        };

        inherited.map(|value| Effective {
            key: def.key.to_string(),
            value: if def.kind.is_secret() {
                registry::MASK.to_string()
            } else {
                value
            },
            source: Scope::Server.as_str().to_string(),
            overridden: false,
        })
    }

    pub async fn set(
        &self,
        scope: Scope,
        scope_id: &str,
        key: &str,
        value: &str,
        actor: Option<&str>,
    ) -> Result<(), WriteError> {
        let def = registry::find(key)
            .ok_or_else(|| WriteError::Invalid(format!("no setting called {key:?}")))?;

        if !def.scopes.contains(&scope) {
            return Err(WriteError::Invalid(format!(
                "{key} cannot be set at the {} scope",
                scope.as_str(),
            )));
        }

        registry::validate(def, value)
            .map_err(|why| WriteError::Invalid(format!("{key} {why}")))?;

        let _writing = self.writes.lock().await;

        sqlx::query(self.db.sql(
            "INSERT INTO setting (scope, scope_id, key, value, updated_at, updated_by)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (scope, scope_id, key) DO UPDATE SET
                 value = excluded.value,
                 updated_at = excluded.updated_at,
                 updated_by = excluded.updated_by",
        ))
        .bind(scope.as_str())
        .bind(scope_id)
        .bind(key)
        .bind(value)
        .bind(now())
        .bind(actor)
        .execute(self.db.pool())
        .await
        .with_context(|| format!("failed to store {key}"))?;

        Ok(self.reload().await?)
    }

    /// Stop overriding a setting at a scope, so it inherits again.
    pub async fn clear(&self, scope: Scope, scope_id: &str, key: &str) -> Result<bool> {
        let _writing = self.writes.lock().await;

        let done = sqlx::query(
            self.db
                .sql("DELETE FROM setting WHERE scope = ? AND scope_id = ? AND key = ?"),
        )
        .bind(scope.as_str())
        .bind(scope_id)
        .bind(key)
        .execute(self.db.pool())
        .await?;

        self.reload().await?;
        Ok(done.rows_affected() > 0)
    }

    /// Forget everything a scope set, for a key or a rule being deleted.
    pub async fn forget(&self, scope: Scope, scope_id: &str) -> Result<()> {
        let _writing = self.writes.lock().await;

        sqlx::query(
            self.db
                .sql("DELETE FROM setting WHERE scope = ? AND scope_id = ?"),
        )
        .bind(scope.as_str())
        .bind(scope_id)
        .execute(self.db.pool())
        .await?;

        self.reload().await
    }

    /// Give the server scope a value for every key that has none yet.
    ///
    /// Runs on every start, not only against an empty table, because the
    /// registry grows: a setting added in a later version would otherwise never
    /// exist for an already-running deployment, and would be invisible in the
    /// interface even though the code reading it had a default in hand.
    ///
    /// Only absent keys are written. An operator's change must never be undone
    /// by a restart reapplying what the environment happened to say.
    pub async fn seed_missing(&self, defaults: &[(&str, String)]) -> Result<usize> {
        let mut written = 0;

        for (key, value) in defaults {
            if self.at(Scope::Server, "", key).is_none() {
                self.set(Scope::Server, "", key, value, None).await?;
                written += 1;
            }
        }

        Ok(written)
    }
}

/// A setting's value kept out of the log: the whole of it, and — for an
/// address — its path and query too, which is where a webhook's token is.
fn hide_from_logs(value: &str) {
    crate::telemetry::redact_also(value);
    if let Ok(url) = url::Url::parse(value.trim()) {
        let mut rest = url.path().to_string();
        if let Some(query) = url.query() {
            rest.push('?');
            rest.push_str(query);
        }
        // `/` alone, or a short path, is no secret: the redaction leaves
        // values too short to be told from ordinary text alone.
        if rest.len() > 1 {
            crate::telemetry::redact_also(&rest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_value_is_the_callers_and_a_failed_store_is_the_servers() {
        use crate::error::AppError;

        let refused = AppError::from(WriteError::Invalid("adult.mode must be one of".into()));
        assert!(matches!(refused, AppError::BadRequest(_)));

        let failed = AppError::from(WriteError::Storage(anyhow::anyhow!(
            "pool timed out while waiting for an open connection"
        )));
        assert!(matches!(failed, AppError::Internal(_)));
    }

    #[test]
    fn a_webhooks_token_is_kept_out_of_the_log() {
        hide_from_logs("https://discord.example/api/webhooks/42/hookTokenValue99");
        let logged = crate::telemetry::redact(
            "could not post to https://discord.example/api/webhooks/42/hookTokenValue99: 404",
        );
        assert!(!logged.contains("hookTokenValue99"), "{logged}");
        // Reprinted by a client that normalised the address, the path is
        // still caught on its own.
        let logged = crate::telemetry::redact("POST /api/webhooks/42/hookTokenValue99 failed");
        assert!(!logged.contains("hookTokenValue99"), "{logged}");
    }
}
