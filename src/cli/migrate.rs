//! `migrate status` and `migrate run`: the database's migrations, read and
//! applied by hand.
//!
//! A server applies its own when it starts, and that is the usual way. By
//! hand is for an upgrade one wants to watch, or to try against a copy: so
//! `run` has the database to itself, and is refused while a server holds it
//! — sqlx's own lock is nothing on SQLite, and a migration under a running
//! server is a schema changing under its queries.

use anyhow::{Result, bail};

use crate::db::{self, Applied, Db};

/// The migrations this binary has, by version and description: those that
/// go up, which is every one of them here.
fn known(db: &Db) -> Vec<(i64, String)> {
    db::migrations(db.dialect())
        .iter()
        .filter(|m| !m.migration_type.is_down_migration())
        .map(|m| (m.version, m.description.to_string()))
        .collect()
}

/// How a database's migrations stand against this binary's.
struct Standing {
    /// This binary's, applied.
    applied: Vec<i64>,
    /// This binary's, not applied yet.
    pending: Vec<(i64, String)>,
    /// Applied, and unknown to this binary: a later version's.
    unknown: Vec<Applied>,
    /// Recorded as not having gone through.
    failed: Vec<Applied>,
}

async fn standing(db: &Db) -> Result<Standing> {
    let applied = db.applied_migrations().await?;
    let known = known(db);
    Ok(Standing {
        applied: known
            .iter()
            .filter(|(v, _)| applied.iter().any(|a| a.version == *v && a.success))
            .map(|(v, _)| *v)
            .collect(),
        pending: known
            .iter()
            .filter(|(v, _)| !applied.iter().any(|a| a.version == *v))
            .cloned()
            .collect(),
        unknown: applied
            .iter()
            .filter(|a| !known.iter().any(|(v, _)| *v == a.version))
            .cloned()
            .collect(),
        failed: applied.iter().filter(|a| !a.success).cloned().collect(),
    })
}

/// Refuse a database whose migrations are not exactly this binary's: a
/// command that builds the whole server would otherwise apply what is
/// missing, under a server still running the version before.
pub async fn ensure_current(db: &Db) -> Result<()> {
    let standing = standing(db).await?;
    if !standing.unknown.is_empty() {
        bail!(
            "the database has migrations this binary does not know — {} — so a later version \
             migrated it: use that version's binary",
            versions(standing.unknown.iter().map(|a| a.version))
        );
    }
    if !standing.failed.is_empty() {
        bail!(
            "the database records migrations that did not go through — {} — see `migrate status`",
            versions(standing.failed.iter().map(|a| a.version))
        );
    }
    if !standing.pending.is_empty() {
        bail!(
            "the database lacks this binary's migrations {}: start the server once, or run \
             `migrate run` while it is stopped",
            versions(standing.pending.iter().map(|(v, _)| *v))
        );
    }
    Ok(())
}

fn versions(versions: impl Iterator<Item = i64>) -> String {
    versions
        .map(|v| format!("{v:04}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The migrations the database has had, and those this binary would add.
pub async fn status() -> Result<()> {
    let config = super::configuration()?;
    let db = super::open_database(&config).await?;
    let applied = db.applied_migrations().await?;
    let standing = standing(&db).await?;

    for (version, description) in known(&db) {
        let state = match applied.iter().find(|a| a.version == version) {
            Some(a) if a.success => "applied",
            Some(_) => "failed",
            None => "pending",
        };
        println!("  {version:04}  {state:<8}  {description}");
    }
    for later in &standing.unknown {
        println!(
            "  {:04}  {:<8}  {} — a later version's",
            later.version, "unknown", later.description
        );
    }
    println!(
        "\n{} database: {} of this binary's {} migrations applied, {} to apply{}",
        crate::backup::engine_name(db.dialect()),
        standing.applied.len(),
        known(&db).len(),
        standing.pending.len(),
        if standing.unknown.is_empty() {
            String::new()
        } else {
            format!(", {} it does not know", standing.unknown.len())
        }
    );
    db.close().await;
    Ok(())
}

/// Apply this binary's migrations, the database held alone.
pub async fn run() -> Result<()> {
    let config = super::configuration()?;
    let _alone = crate::backup::lock::hold_alone(
        &config.database,
        ": it applies its migrations itself as it starts",
    )
    .await?;
    if let Some(url) = super::server_answers(&config).await {
        bail!("a server answers at {url}: stop it first; it applies its migrations as it starts");
    }
    let db = super::open_database(&config).await?;
    let before = standing(&db).await?;
    if !before.unknown.is_empty() {
        bail!(
            "the database has migrations this binary does not know — {} — so a later version \
             migrated it: nothing to apply with this one",
            versions(before.unknown.iter().map(|a| a.version))
        );
    }
    // What a start would make first, made first here too.
    let backups = crate::backup::Backups::new(&config, db.clone(), None)?;
    let archived = backups.before_upgrade().await?;
    db.migrate().await?;
    if let Some(archived) = archived {
        let made = backups.after_upgrade(archived).await;
        println!("  archived first: {} in {}", made.name, made.at);
    }
    for (version, description) in &before.pending {
        println!("  {version:04}  applied   {description}");
    }
    println!(
        "\n{} migrations applied; the database is at {:04}",
        before.pending.len(),
        db.last_migration().await?
    );
    db.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_database_migrated_by_this_binary_is_current_and_an_empty_one_is_not() {
        let migrated = crate::db::testing::db().await;
        ensure_current(&migrated).await.unwrap();
        let standing = standing(&migrated).await.unwrap();
        assert!(standing.pending.is_empty() && standing.unknown.is_empty());
        assert_eq!(
            standing.applied.last().copied(),
            Some(db::latest_migration())
        );

        let empty = Db::connect(&crate::config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .unwrap();
        let refused = ensure_current(&empty).await.unwrap_err().to_string();
        assert!(
            refused.contains("lacks this binary's migrations"),
            "{refused}"
        );
    }

    #[tokio::test]
    async fn a_database_a_later_version_migrated_is_refused() {
        let db = crate::db::testing::sqlite().await;
        sqlx::query(
            "INSERT INTO _sqlx_migrations \
             (version, description, installed_on, success, checksum, execution_time) \
             VALUES (9999, 'from the future', CURRENT_TIMESTAMP, 1, x'00', 0)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let refused = ensure_current(&db).await.unwrap_err().to_string();
        assert!(refused.contains("9999"), "{refused}");
        assert!(refused.contains("later version"), "{refused}");
    }
}
