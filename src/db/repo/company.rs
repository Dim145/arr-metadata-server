//! Networks and companies, each kept once for every work it is credited on,
//! and what each did for a work.
//!
//! A company is found again by the id of a source that gives one — TMDB's,
//! which numbers networks and companies apart, or TheTVDB's — else by its
//! name, among those of its kind that no id says are another: TMDB's AMC and
//! TheTVDB's are one network, which carries both ids once both have named
//! it. What a later answer knows of it that the row does not — an id, a
//! logo, a country — is added; what the row knows stands. A work's own list
//! is written again whole by a refresh that gives one, as its credits are.
//!
//! The work's texts — `network`, `studio`, `production_companies` — and
//! their locks are not this: these double them, with ids and logos.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{Any, Transaction};
use utoipa::ToSchema;

use crate::{
    db::{Db, RowExt, new_id, now},
    domain::{Company, CompanyCredit, CompanyRole, company_key},
};

const COLUMNS: &str = "id, kind, name, tmdb_id, tvdb_id, logo, country";

fn map(row: &sqlx::any::AnyRow) -> Result<Company> {
    Ok(Company {
        id: row.text("id")?,
        kind: row.text("kind")?.parse()?,
        name: row.text("name")?,
        tmdb_id: row.opt_big("tmdb_id")?,
        tvdb_id: row.opt_big("tvdb_id")?,
        logo: row.opt_text("logo")?,
        country: row.opt_text("country")?,
    })
}

/// One company, by this server's id.
pub async fn get(db: &Db, id: &str) -> Result<Option<Company>> {
    let row = sqlx::query(db.sql(&format!("SELECT {COLUMNS} FROM company WHERE id = ?")))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;
    row.as_ref().map(map).transpose()
}

/// One company of a kind, by TMDB's id for it — a network's, or a
/// company's: TMDB numbers the two apart.
#[cfg(test)]
pub async fn by_tmdb(
    db: &Db,
    kind: crate::domain::CompanyKind,
    tmdb_id: i64,
) -> Result<Option<Company>> {
    let row = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM company WHERE kind = ? AND tmdb_id = ?"
    )))
    .bind(kind.as_str())
    .bind(tmdb_id)
    .fetch_optional(db.pool())
    .await?;
    row.as_ref().map(map).transpose()
}

/// One company of a kind, by TheTVDB's id for it.
#[cfg(test)]
pub async fn by_tvdb(
    db: &Db,
    kind: crate::domain::CompanyKind,
    tvdb_id: i64,
) -> Result<Option<Company>> {
    let row = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM company WHERE kind = ? AND tvdb_id = ?"
    )))
    .bind(kind.as_str())
    .bind(tvdb_id)
    .fetch_optional(db.pool())
    .await?;
    row.as_ref().map(map).transpose()
}

/// The companies credited on one work, by role in the order roles come —
/// networks, production, studios, distributors — and each role in its
/// source's order.
pub async fn of_work(db: &Db, media_id: &str) -> Result<Vec<CompanyCredit>> {
    let rows = sqlx::query(db.sql(
        "SELECT c.id, c.kind, c.name, c.tmdb_id, c.tvdb_id, c.logo, c.country,
                l.role, l.sort_order, l.source
           FROM media_company l JOIN company c ON c.id = l.company_id
          WHERE l.media_id = ?",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    let mut out = rows
        .iter()
        .map(|row| {
            Ok(CompanyCredit {
                company: map(row)?,
                role: row.text("role")?.parse()?,
                sort_order: row.int("sort_order")?,
                source: row.opt_text("source")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    out.sort_by_key(|c| (c.role, c.sort_order));
    Ok(out)
}

/// A work a company is credited on, and what it did there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = CompanyWork)]
pub struct Credited {
    pub work_id: String,
    pub role: CompanyRole,
}

/// The works a company is credited on, switched on, the most recent first —
/// those for adults only when asked for. What a company's page lists, and
/// what a filter by company finds.
pub async fn works_of(db: &Db, company_id: &str, include_adult: bool) -> Result<Vec<Credited>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT l.media_id, l.role
           FROM media_company l JOIN media_item m ON m.id = l.media_id
          WHERE l.company_id = ? AND m.is_enabled = 1 AND m.merged_into IS NULL{}
          ORDER BY m.year DESC, m.title, l.role",
        if include_adult {
            ""
        } else {
            " AND m.is_adult = 0"
        }
    )))
    .bind(company_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| {
            Ok(Credited {
                work_id: row.text("media_id")?,
                role: row.text("role")?.parse()?,
            })
        })
        .collect()
}

/// Write a work's companies in place of those it had, each found again or
/// added (see the module). In the work's own write, so a reader never finds
/// the list half written.
pub async fn replace(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    media_id: &str,
    companies: &[CompanyCredit],
) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM media_company WHERE media_id = ?"))
        .bind(media_id)
        .execute(&mut **tx)
        .await?;

    for credit in companies {
        let Some(company_id) = resolve(db, tx, &credit.company).await? else {
            continue;
        };
        sqlx::query(db.sql(
            "INSERT INTO media_company (media_id, company_id, role, sort_order, source)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (media_id, company_id, role) DO NOTHING",
        ))
        .bind(media_id)
        .bind(&company_id)
        .bind(credit.role.as_str())
        .bind(credit.sort_order)
        .bind(&credit.source)
        .execute(&mut **tx)
        .await
        .context("failed to write a work's company")?;
    }
    Ok(())
}

/// The row a company is, found again or added; what the answer knows of it
/// that the row does not is written into it. `None` for one with no name.
async fn resolve(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    company: &Company,
) -> Result<Option<String>> {
    let name = company.name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    let key = company_key(name);
    let kind = company.kind.as_str();

    // By a source's own id: the name it gives is the one kept.
    for (column, id) in [("tmdb_id", company.tmdb_id), ("tvdb_id", company.tvdb_id)] {
        let Some(id) = id else { continue };
        let found: Option<String> = sqlx::query_scalar(db.sql(&format!(
            "SELECT id FROM company WHERE kind = ? AND {column} = ?"
        )))
        .bind(kind)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(found) = found {
            complete(db, tx, &found, company, Some(name)).await?;
            return Ok(Some(found));
        }
    }

    // By its name, among those of its kind that no id of the same source
    // says are another: none set against an id this answer gives.
    let mut sql = "SELECT id FROM company WHERE kind = ? AND name_key = ?".to_string();
    if company.tmdb_id.is_some() {
        sql.push_str(" AND tmdb_id IS NULL");
    }
    if company.tvdb_id.is_some() {
        sql.push_str(" AND tvdb_id IS NULL");
    }
    sql.push_str(" ORDER BY created_at LIMIT 1");
    let found: Option<String> = sqlx::query_scalar(db.sql(&sql))
        .bind(kind)
        .bind(&key)
        .fetch_optional(&mut **tx)
        .await?;
    if let Some(found) = found {
        complete(db, tx, &found, company, None).await?;
        return Ok(Some(found));
    }

    // New. Two writes adding the same company at once — PostgreSQL's, which
    // do not wait for each other as SQLite's do — meet on an id: the second
    // takes the first's row.
    let id = new_id();
    let at = now();
    let added = sqlx::query(db.sql(
        "INSERT INTO company
             (id, kind, name, name_key, tmdb_id, tvdb_id, logo, country, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT DO NOTHING",
    ))
    .bind(&id)
    .bind(kind)
    .bind(name)
    .bind(&key)
    .bind(company.tmdb_id)
    .bind(company.tvdb_id)
    .bind(&company.logo)
    .bind(&company.country)
    .bind(&at)
    .bind(&at)
    .execute(&mut **tx)
    .await
    .context("failed to add a company")?;
    if added.rows_affected() > 0 {
        return Ok(Some(id));
    }
    for (column, given) in [("tmdb_id", company.tmdb_id), ("tvdb_id", company.tvdb_id)] {
        let Some(given) = given else { continue };
        let found: Option<String> = sqlx::query_scalar(db.sql(&format!(
            "SELECT id FROM company WHERE kind = ? AND {column} = ?"
        )))
        .bind(kind)
        .bind(given)
        .fetch_optional(&mut **tx)
        .await?;
        if found.is_some() {
            return Ok(found);
        }
    }
    Ok(None)
}

/// Add to a company's row what an answer knows of it and the row does not:
/// an id of a source — unless another row of its kind holds that id already,
/// which stays that one's — a logo, a country; and its name, when the
/// answer found it by its own source's id.
async fn complete(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    id: &str,
    company: &Company,
    renamed: Option<&str>,
) -> Result<()> {
    let row = sqlx::query(db.sql(&format!("SELECT {COLUMNS} FROM company WHERE id = ?")))
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    let held = map(&row)?;

    let mut tmdb_id = held.tmdb_id;
    let mut tvdb_id = held.tvdb_id;
    for (column, given, kept) in [
        ("tmdb_id", company.tmdb_id, held.tmdb_id),
        ("tvdb_id", company.tvdb_id, held.tvdb_id),
    ] {
        let (Some(given), None) = (given, kept) else {
            continue;
        };
        let taken: Option<String> = sqlx::query_scalar(db.sql(&format!(
            "SELECT id FROM company WHERE kind = ? AND {column} = ?"
        )))
        .bind(company.kind.as_str())
        .bind(given)
        .fetch_optional(&mut **tx)
        .await?;
        if taken.is_none() {
            match column {
                "tmdb_id" => tmdb_id = Some(given),
                _ => tvdb_id = Some(given),
            }
        }
    }

    let name = renamed.unwrap_or(&held.name).to_string();
    let logo = held.logo.clone().or_else(|| company.logo.clone());
    let country = held.country.clone().or_else(|| company.country.clone());
    if (tmdb_id, tvdb_id, &name, &logo, &country)
        == (
            held.tmdb_id,
            held.tvdb_id,
            &held.name,
            &held.logo,
            &held.country,
        )
    {
        return Ok(());
    }

    sqlx::query(db.sql(
        "UPDATE company SET name = ?, name_key = ?, tmdb_id = ?, tvdb_id = ?, logo = ?,
                country = ?, updated_at = ?
          WHERE id = ?",
    ))
    .bind(&name)
    .bind(company_key(&name))
    .bind(tmdb_id)
    .bind(tvdb_id)
    .bind(&logo)
    .bind(&country)
    .bind(now())
    .bind(id)
    .execute(&mut **tx)
    .await
    .context("failed to complete a company")?;
    Ok(())
}
