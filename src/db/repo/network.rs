//! Who may call the address-guarded surfaces, and who has tried.
//!
//! Sonarr and Radarr cannot present a key, so those surfaces are guarded by
//! address. That makes the list of addresses a credential like any other, and
//! it belongs where the API keys are rather than in an environment variable
//! that needs a restart to change.

use std::net::IpAddr;

use anyhow::{Context, Result};
use ipnet::IpNet;
use serde::Serialize;
use utoipa::ToSchema;

use crate::db::{Db, RowExt, from_bool, new_id, now};

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetworkRule {
    pub id: String,
    /// As the operator wrote it: `172.31.0.0/24`, or a bare address.
    pub cidr: String,
    /// What the client behind this address is called, once somebody says. It
    /// is what per-client settings hang off, because an address changes when a
    /// container restarts and a name does not.
    pub name: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
    pub created_by: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Caller {
    pub ip: String,
    /// What the resolver and the hosts file call this address, if anything.
    pub hostname: Option<String>,
    /// What the caller called itself. Sonarr and Radarr say so plainly.
    pub user_agent: Option<String>,
    pub last_surface: String,
    pub last_path: Option<String>,
    pub last_allowed: bool,
    pub hits: i64,
    pub refusals: i64,
    pub first_seen: String,
    pub last_seen: String,
}

/// Accept a bare address as well as a block, because that is what an operator
/// copies out of a log line.
pub fn parse_rule(text: &str) -> Result<IpNet> {
    let trimmed = text.trim();

    if let Ok(net) = trimmed.parse::<IpNet>() {
        return Ok(net);
    }

    let single: IpAddr = trimmed
        .parse()
        .with_context(|| format!("{trimmed:?} is not an address or a CIDR block"))?;

    Ok(IpNet::from(single))
}

pub async fn list_rules(db: &Db) -> Result<Vec<NetworkRule>> {
    let rows = sqlx::query(db.sql(
        "SELECT id, cidr, name, note, created_at, created_by
         FROM network_rule ORDER BY created_at",
    ))
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| {
            Ok(NetworkRule {
                id: row.text("id")?,
                cidr: row.text("cidr")?,
                name: row.opt_text("name")?,
                note: row.opt_text("note")?,
                created_at: row.text("created_at")?,
                created_by: row.opt_text("created_by")?,
            })
        })
        .collect()
}

/// The rules as the guard needs them, already parsed.
///
/// A row that no longer parses is skipped rather than fatal: it can only get
/// there by hand, and one bad line should not close the door on the rest.
pub async fn effective(db: &Db) -> Result<Vec<(String, IpNet)>> {
    Ok(list_rules(db)
        .await?
        .iter()
        .filter_map(|rule| match parse_rule(&rule.cidr) {
            Ok(net) => Some((rule.id.clone(), net)),
            Err(e) => {
                tracing::warn!(cidr = %rule.cidr, error = %e, "ignoring an unreadable network rule");
                None
            }
        })
        .collect())
}

pub async fn add_rule(
    db: &Db,
    cidr: &str,
    name: Option<&str>,
    note: Option<&str>,
    created_by: Option<&str>,
) -> Result<NetworkRule> {
    // Stored as written, but only once it is known to mean something.
    parse_rule(cidr)?;

    let rule = NetworkRule {
        id: new_id(),
        cidr: cidr.trim().to_string(),
        name: name.map(str::to_string).filter(|n| !n.trim().is_empty()),
        note: note.map(str::to_string).filter(|n| !n.trim().is_empty()),
        created_at: now(),
        created_by: created_by.map(str::to_string),
    };

    sqlx::query(db.sql(
        "INSERT INTO network_rule (id, cidr, name, note, created_at, created_by)
         VALUES (?, ?, ?, ?, ?, ?)",
    ))
    .bind(&rule.id)
    .bind(&rule.cidr)
    .bind(&rule.name)
    .bind(&rule.note)
    .bind(&rule.created_at)
    .bind(&rule.created_by)
    .execute(db.pool())
    .await
    .context("failed to add the network rule")?;

    Ok(rule)
}

pub async fn remove_rule(db: &Db, id: &str) -> Result<bool> {
    let done = sqlx::query(db.sql("DELETE FROM network_rule WHERE id = ?"))
        .bind(id)
        .execute(db.pool())
        .await?;

    Ok(done.rows_affected() > 0)
}

pub async fn count_rules(db: &Db) -> Result<i64> {
    let row = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM network_rule"))
        .fetch_one(db.pool())
        .await?;

    Ok(row.big("n")?)
}

/// Seed the table from a list, for a server starting against an empty one.
pub async fn seed(db: &Db, nets: &[IpNet]) -> Result<usize> {
    let mut added = 0;

    for net in nets {
        add_rule(db, &net.to_string(), None, Some("from AMS_ALLOWLIST"), None).await?;
        added += 1;
    }

    Ok(added)
}

/// Rename the client an address stands for.
pub async fn rename(db: &Db, id: &str, name: Option<&str>) -> Result<bool> {
    let done = sqlx::query(db.sql("UPDATE network_rule SET name = ? WHERE id = ?"))
        .bind(name.map(str::trim).filter(|n| !n.is_empty()))
        .bind(id)
        .execute(db.pool())
        .await?;

    Ok(done.rows_affected() > 0)
}

pub struct Sighting<'a> {
    pub ip: IpAddr,
    pub hostname: Option<&'a str>,
    pub user_agent: Option<&'a str>,
    pub surface: &'a str,
    pub path: &'a str,
    pub allowed: bool,
}

/// Record that somebody called, or that they called again.
///
/// Best effort by design: this is a write on the read path, and a caller being
/// served matters more than the note that they were.
pub async fn saw(db: &Db, sighting: Sighting<'_>) -> Result<()> {
    let ip = sighting.ip.to_string();
    let at = now();
    let refusal = i64::from(!sighting.allowed);

    let sql = "
        INSERT INTO network_caller (
            ip, hostname, user_agent, last_surface, last_path, last_allowed,
            hits, refusals, first_seen, last_seen
        ) VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, ?)
        ON CONFLICT (ip) DO UPDATE SET
            hostname     = COALESCE(excluded.hostname, network_caller.hostname),
            user_agent   = COALESCE(excluded.user_agent, network_caller.user_agent),
            last_surface = excluded.last_surface,
            last_path    = excluded.last_path,
            last_allowed = excluded.last_allowed,
            hits         = network_caller.hits + 1,
            refusals     = network_caller.refusals + excluded.refusals,
            last_seen    = excluded.last_seen
    ";

    sqlx::query(db.sql(sql))
        .bind(&ip)
        .bind(sighting.hostname)
        .bind(sighting.user_agent)
        .bind(sighting.surface)
        .bind(sighting.path)
        .bind(from_bool(sighting.allowed))
        .bind(refusal)
        .bind(&at)
        .bind(&at)
        .execute(db.pool())
        .await
        .context("failed to record the caller")?;

    Ok(())
}

pub async fn callers(db: &Db, limit: i64) -> Result<Vec<Caller>> {
    let rows = sqlx::query(db.sql(
        "SELECT ip, hostname, user_agent, last_surface, last_path, last_allowed,
                hits, refusals, first_seen, last_seen
         FROM network_caller ORDER BY last_seen DESC LIMIT ?",
    ))
    .bind(limit.clamp(1, 500))
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| {
            Ok(Caller {
                ip: row.text("ip")?,
                hostname: row.opt_text("hostname")?,
                user_agent: row.opt_text("user_agent")?,
                last_surface: row.text("last_surface")?,
                last_path: row.opt_text("last_path")?,
                last_allowed: row.flag("last_allowed")?,
                hits: row.big("hits")?,
                refusals: row.big("refusals")?,
                first_seen: row.text("first_seen")?,
                last_seen: row.text("last_seen")?,
            })
        })
        .collect()
}

/// Drop callers nobody has seen for a while, so the table stays a list of what
/// is using this server rather than everything that ever touched it.
pub async fn prune(db: &Db, cutoff: &str) -> Result<u64> {
    let done = sqlx::query(db.sql("DELETE FROM network_caller WHERE last_seen < ?"))
        .bind(cutoff)
        .execute(db.pool())
        .await?;

    Ok(done.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_address_is_a_block_of_one() {
        // What an operator copies out of a log line is an address, not a block.
        let net = parse_rule("172.31.0.7").unwrap();

        assert_eq!(net.prefix_len(), 32);
        assert!(net.contains(&"172.31.0.7".parse::<IpAddr>().unwrap()));
        assert!(!net.contains(&"172.31.0.8".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn a_block_is_taken_as_written() {
        let net = parse_rule(" 172.31.0.0/24 ").unwrap();

        assert!(net.contains(&"172.31.0.200".parse::<IpAddr>().unwrap()));
        assert!(!net.contains(&"172.32.0.1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn ipv6_works_the_same_way() {
        assert_eq!(parse_rule("::1").unwrap().prefix_len(), 128);
        assert!(parse_rule("fd00::/8").is_ok());
    }

    #[test]
    fn nonsense_is_refused_before_it_is_stored() {
        assert!(parse_rule("everyone").is_err());
        assert!(parse_rule("172.31.0.0/99").is_err());
        assert!(parse_rule("").is_err());
    }
}
