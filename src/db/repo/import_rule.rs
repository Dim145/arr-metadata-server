//! What is brought into the catalogue on a schedule, and into which
//! selection; see `jobs::rules`, which runs them, and `api::native::lists`,
//! where they are written.

use anyhow::{Context, Result};

use crate::db::{Db, RowExt, from_bool, new_id, now};

/// A rule as it is kept: its source's parameters still the JSON they were
/// written as, which `jobs::rules` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub id: String,
    pub name: String,
    /// `tmdb` or `feed`.
    pub source: String,
    pub params: String,
    pub list_id: Option<String>,
    /// `add` or `replace`.
    pub list_mode: String,
    pub cron: String,
    pub language: Option<String>,
    pub enable_works: bool,
    pub is_enabled: bool,
    pub last_run_at: Option<String>,
    pub last_job_id: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// What a rule is made of, on creation and on every change.
pub struct Fields<'a> {
    pub name: &'a str,
    pub source: &'a str,
    pub params: &'a str,
    pub list_id: Option<&'a str>,
    pub list_mode: &'a str,
    pub cron: &'a str,
    pub language: Option<&'a str>,
    pub enable_works: bool,
    pub is_enabled: bool,
}

const COLUMNS: &str = "id, name, source, params, list_id, list_mode, cron, language, \
                       enable_works, is_enabled, last_run_at, last_job_id, created_by, \
                       created_at, updated_at";

fn map(row: &sqlx::any::AnyRow) -> Result<Rule> {
    Ok(Rule {
        id: row.text("id")?,
        name: row.text("name")?,
        source: row.text("source")?,
        params: row.text("params")?,
        list_id: row.opt_text("list_id")?,
        list_mode: row.text("list_mode")?,
        cron: row.text("cron")?,
        language: row.opt_text("language")?,
        enable_works: row.flag("enable_works")?,
        is_enabled: row.flag("is_enabled")?,
        last_run_at: row.opt_text("last_run_at")?,
        last_job_id: row.opt_text("last_job_id")?,
        created_by: row.opt_text("created_by")?,
        created_at: row.text("created_at")?,
        updated_at: row.text("updated_at")?,
    })
}

pub async fn create(db: &Db, fields: Fields<'_>, by: Option<&str>) -> Result<Rule> {
    let id = new_id();
    let at = now();
    sqlx::query(db.sql(
        "INSERT INTO import_rule
             (id, name, source, params, list_id, list_mode, cron, language, enable_works,
              is_enabled, created_by, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(&id)
    .bind(fields.name)
    .bind(fields.source)
    .bind(fields.params)
    .bind(fields.list_id)
    .bind(fields.list_mode)
    .bind(fields.cron)
    .bind(fields.language)
    .bind(from_bool(fields.enable_works))
    .bind(from_bool(fields.is_enabled))
    .bind(by)
    .bind(&at)
    .bind(&at)
    .execute(db.pool())
    .await
    .context("failed to create the import rule")?;

    get(db, &id)
        .await?
        .context("the rule just created is not there")
}

/// Change what a rule is made of. `None` when there is no such rule.
pub async fn update(db: &Db, id: &str, fields: Fields<'_>) -> Result<Option<Rule>> {
    let done = sqlx::query(db.sql(
        "UPDATE import_rule
         SET name = ?, source = ?, params = ?, list_id = ?, list_mode = ?, cron = ?,
             language = ?, enable_works = ?, is_enabled = ?, updated_at = ?
         WHERE id = ?",
    ))
    .bind(fields.name)
    .bind(fields.source)
    .bind(fields.params)
    .bind(fields.list_id)
    .bind(fields.list_mode)
    .bind(fields.cron)
    .bind(fields.language)
    .bind(from_bool(fields.enable_works))
    .bind(from_bool(fields.is_enabled))
    .bind(now())
    .bind(id)
    .execute(db.pool())
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get(db, id).await
}

pub async fn delete(db: &Db, id: &str) -> Result<bool> {
    let done = sqlx::query(db.sql("DELETE FROM import_rule WHERE id = ?"))
        .bind(id)
        .execute(db.pool())
        .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn get(db: &Db, id: &str) -> Result<Option<Rule>> {
    let row = sqlx::query(db.sql(&format!("SELECT {COLUMNS} FROM import_rule WHERE id = ?")))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;
    row.as_ref().map(map).transpose()
}

/// Every rule, by name.
pub async fn list(db: &Db) -> Result<Vec<Rule>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM import_rule ORDER BY name, created_at"
    )))
    .fetch_all(db.pool())
    .await?;
    rows.iter().map(map).collect()
}

/// Say that a rule ran: when it started, and its run in the history.
pub async fn ran(db: &Db, id: &str, at: &str, job_id: Option<&str>) -> Result<()> {
    sqlx::query(db.sql("UPDATE import_rule SET last_run_at = ?, last_job_id = ? WHERE id = ?"))
        .bind(at)
        .bind(job_id)
        .bind(id)
        .execute(db.pool())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::db;

    fn fields<'a>(name: &'a str, list: Option<&'a str>) -> Fields<'a> {
        Fields {
            name,
            source: "tmdb",
            params: r#"{"kind":"movie","days":90,"limit":20}"#,
            list_id: list,
            list_mode: "replace",
            cron: "0 5 * * 1",
            language: None,
            enable_works: true,
            is_enabled: true,
        }
    }

    #[tokio::test]
    async fn a_rule_is_written_changed_run_and_goes_with_its_selection() {
        let db = db().await;
        let selection = crate::db::repo::list::create(
            &db,
            crate::db::repo::list::ListFields {
                slug: "top",
                name: "Top",
                description: None,
                kind: crate::db::repo::list::ListKind::Movie,
                mode: crate::db::repo::list::ListMode::Manual,
                filter: None,
                is_public: true,
                cover: None,
            },
            None,
        )
        .await
        .unwrap();

        let rule = create(
            &db,
            fields("Weekly top", Some(&selection.id)),
            Some("admin:a"),
        )
        .await
        .unwrap();
        assert!(rule.is_enabled && rule.enable_works);
        assert_eq!(rule.list_id.as_deref(), Some(selection.id.as_str()));
        assert_eq!(rule.created_by.as_deref(), Some("admin:a"));
        let alone = create(&db, fields("Into the catalogue", None), None)
            .await
            .unwrap();

        let changed = update(
            &db,
            &rule.id,
            Fields {
                is_enabled: false,
                ..fields("Weekly top of the quarter", Some(&selection.id))
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(changed.name, "Weekly top of the quarter");
        assert!(!changed.is_enabled);
        assert!(
            update(&db, "nobody", fields("x", None))
                .await
                .unwrap()
                .is_none()
        );

        ran(&db, &rule.id, "2026-10-26T05:00:00Z", Some("job-1"))
            .await
            .unwrap();
        let read = get(&db, &rule.id).await.unwrap().unwrap();
        assert_eq!(read.last_run_at.as_deref(), Some("2026-10-26T05:00:00Z"));
        assert_eq!(read.last_job_id.as_deref(), Some("job-1"));
        assert_eq!(list(&db).await.unwrap().len(), 2);

        // The selection deleted, the rule that filled it goes too; the one
        // into the catalogue alone stays.
        crate::db::repo::list::delete(&db, &selection.id)
            .await
            .unwrap();
        assert!(get(&db, &rule.id).await.unwrap().is_none());
        assert!(get(&db, &alone.id).await.unwrap().is_some());
        assert!(delete(&db, &alone.id).await.unwrap());
        assert!(list(&db).await.unwrap().is_empty());
    }
}
