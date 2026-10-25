//! The import rules, written and run: what is brought into the catalogue on
//! a schedule, and into which selection — TMDB's most popular works of a
//! recent stretch, or a list another server publishes, followed. See
//! [`crate::jobs::rules`], which runs them.
//!
//! An administrator's: a rule fetches from the providers on its own, for as
//! long as it is switched on.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{self, audit::Action, import_rule::Rule, job::Job},
    domain::MediaKind,
    error::{AppError, AppResult},
    jobs::{
        import::{self, Line, Placement, Preview},
        rules::{self, Params, SourceKind},
        schedule::Cron,
    },
    service,
    state::AppState,
};

use super::TAG;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(update, delete))
        .routes(routes!(run))
        .routes(routes!(preview))
}

/// The longest a rule's name and a followed address may be.
const NAME_LIMIT: usize = 120;
const URL_LIMIT: usize = 2048;

/// How many of a source's works a preview sets beside the catalogue.
const PREVIEW_MOST: usize = 200;

/// A rule as the pages show it.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportRule {
    pub id: String,
    pub name: String,
    pub source: SourceKind,
    /// TMDB: films or series. A list: what a bare `tmdb:` line stands for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<MediaKind>,
    /// TMDB: how many days back its stretch reaches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub days: Option<u32>,
    /// TMDB: how many works, the most popular first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// TMDB: works first made in this language only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
    /// A list: its address, a key in it shown as `hidden`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The selection the works go into.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_name: Option<String>,
    pub list_mode: Placement,
    /// When it runs: five cron fields, UTC.
    pub cron: String,
    /// A language the episode texts of each series are fetched in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Whether the works it brings in are switched on.
    pub enable_works: bool,
    pub is_enabled: bool,
    /// When it runs next, while it is switched on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_at: Option<String>,
    /// Its latest run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<Job>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A rule, as it is written. On a change, every field says what the rule is
/// to be.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportRuleRequest {
    pub name: String,
    pub source: SourceKind,
    /// TMDB: films or series, films when absent. A list: what a bare
    /// `tmdb:` line stands for.
    #[serde(default)]
    pub kind: Option<MediaKind>,
    /// TMDB: how many days back its stretch reaches, 1 to 3650; ninety when
    /// absent — a quarter.
    #[serde(default)]
    pub days: Option<u32>,
    /// TMDB: how many works, 1 to 500; twenty when absent.
    #[serde(default)]
    pub limit: Option<u32>,
    /// TMDB: works first made in this language only, ISO 639-1: `ja`.
    #[serde(default)]
    pub original_language: Option<String>,
    /// A list: the address of its text file of ids or its Atom feed. Sent
    /// back as it was shown, a key `hidden` in it, the key kept is kept.
    #[serde(default)]
    pub url: Option<String>,
    /// A hand-made selection the works go into, by id or slug.
    #[serde(default)]
    pub list_id: Option<String>,
    /// `add`: they join it. `replace`: it becomes them. `add` when absent.
    #[serde(default)]
    pub list_mode: Option<Placement>,
    /// When it runs: five cron fields, UTC — `0 5 * * 1`, Monday at five.
    pub cron: String,
    /// A language the episode texts of each series are fetched in at once.
    #[serde(default)]
    pub language: Option<String>,
    /// Whether the works it brings in are switched on; on when absent.
    #[serde(default)]
    pub enable_works: Option<bool>,
    /// Whether it runs; on when absent.
    #[serde(default)]
    pub is_enabled: Option<bool>,
}

/// A rule's request, checked.
struct Checked {
    name: String,
    source: SourceKind,
    params: Params,
    list_id: Option<String>,
    list_mode: Placement,
    cron: String,
    language: Option<String>,
    enable_works: bool,
    is_enabled: bool,
}

async fn check(
    state: &AppState,
    request: ImportRuleRequest,
    current: Option<&Rule>,
) -> AppResult<Checked> {
    let name = request.name.trim().to_string();
    if name.is_empty() || name.chars().count() > NAME_LIMIT {
        return Err(AppError::BadRequest(format!(
            "a rule needs a name of {NAME_LIMIT} characters at the most"
        )));
    }
    let cron = request
        .cron
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    cron.parse::<Cron>().map_err(AppError::BadRequest)?;

    let params = match request.source {
        SourceKind::Tmdb => {
            let days = request.days.unwrap_or(90);
            if !(1..=rules::MOST_DAYS).contains(&days) {
                return Err(AppError::BadRequest(format!(
                    "the stretch reaches back 1 to {} days",
                    rules::MOST_DAYS
                )));
            }
            let limit = request.limit.unwrap_or(20);
            if !(1..=rules::MOST_FROM_TMDB).contains(&limit) {
                return Err(AppError::BadRequest(format!(
                    "a rule takes 1 to {} of TMDB's works",
                    rules::MOST_FROM_TMDB
                )));
            }
            let original_language = match request.original_language.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(code) if code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic()) => {
                    Some(code.to_ascii_lowercase())
                }
                Some(code) => {
                    return Err(AppError::BadRequest(format!(
                        "{code} is not a language as TMDB names them: two letters, ja or fr"
                    )));
                }
            };
            Params {
                kind: Some(request.kind.unwrap_or(MediaKind::Movie)),
                days: Some(days),
                limit: Some(limit),
                original_language,
                url: None,
            }
        }
        SourceKind::Feed => {
            let given = request.url.as_deref().map(str::trim).unwrap_or_default();
            // Sent back as it was shown: the address kept, its key with it.
            let kept = current
                .and_then(|rule| rules::read(rule).ok())
                .and_then(|read| read.params.url)
                .filter(|kept| rules::shown_url(kept) == given);
            let url = match kept {
                Some(kept) => kept,
                None => check_url(state, given)?,
            };
            Params {
                kind: request.kind,
                days: None,
                limit: None,
                original_language: None,
                url: Some(url),
            }
        }
    };

    let list_id = match request.list_id.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(key) => Some(super::super::discover::hand_made(state, key).await?),
    };
    let language = match request.language.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => Some(service::language::tag(raw).ok_or_else(|| {
            AppError::BadRequest(format!("{raw} is not a language: write it fr, de or pt-BR"))
        })?),
    };

    Ok(Checked {
        name,
        source: request.source,
        params,
        list_id,
        list_mode: request.list_mode.unwrap_or(Placement::Add),
        cron,
        language,
        enable_works: request.enable_works.unwrap_or(true),
        is_enabled: request.is_enabled.unwrap_or(true),
    })
}

/// An address to follow: http or https, of a host the server may reach.
fn check_url(state: &AppState, given: &str) -> AppResult<String> {
    if given.is_empty() {
        return Err(AppError::BadRequest(
            "a followed list needs its address".into(),
        ));
    }
    if given.len() > URL_LIMIT {
        return Err(AppError::BadRequest(format!(
            "an address of {URL_LIMIT} characters at the most"
        )));
    }
    let parsed = url::Url::parse(given)
        .map_err(|_| AppError::BadRequest(format!("{given} is not an address")))?;
    if let Some(why) = state.media.guard.refusal(&parsed) {
        return Err(AppError::BadRequest(format!(
            "{} is {why}, which this server does not follow",
            rules::shown_url(given)
        )));
    }
    Ok(parsed.to_string())
}

fn params_text(params: &Params) -> AppResult<String> {
    Ok(serde_json::to_string(params)?)
}

/// A rule as the pages show it: its address's key hidden, when it runs next,
/// its latest run.
async fn shown(state: &AppState, rule: Rule) -> AppResult<ImportRule> {
    let read = rules::read(&rule).ok();
    let next_at = read
        .as_ref()
        .filter(|_| rule.is_enabled)
        .and_then(|read| rules::next_at(&rule, &read.cron))
        .map(crate::db::to_rfc3339);
    let params = read.map(|read| read.params).unwrap_or_default();
    let list_name = match rule.list_id.as_deref() {
        Some(id) => repo::list::get(&state.db, id).await?.map(|list| list.name),
        None => None,
    };
    let last_run = match rule.last_job_id.as_deref() {
        Some(id) => repo::job::get(&state.db, id).await?,
        None => None,
    };
    Ok(ImportRule {
        source: match rule.source.as_str() {
            "feed" => SourceKind::Feed,
            _ => SourceKind::Tmdb,
        },
        kind: params.kind,
        days: params.days,
        limit: params.limit,
        original_language: params.original_language,
        url: params.url.as_deref().map(rules::shown_url),
        list_name,
        list_mode: Placement::parse(&rule.list_mode),
        next_at,
        last_run,
        id: rule.id,
        name: rule.name,
        list_id: rule.list_id,
        cron: rule.cron,
        language: rule.language,
        enable_works: rule.enable_works,
        is_enabled: rule.is_enabled,
        created_by: rule.created_by,
        created_at: rule.created_at,
        updated_at: rule.updated_at,
    })
}

/// Every import rule, by name.
#[utoipa::path(
    get, path = "/import/rules", tag = TAG,
    operation_id = "import_rules_list",
    responses(
        (status = 200, body = Vec<ImportRule>),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Vec<ImportRule>>> {
    identity.require_admin()?;
    let mut out = Vec::new();
    for rule in repo::import_rule::list(&state.db).await? {
        out.push(shown(&state, rule).await?);
    }
    Ok(Json(out))
}

/// Write an import rule. It runs at its expression's next moment, and can be
/// run at once.
#[utoipa::path(
    post, path = "/import/rules", tag = TAG,
    operation_id = "import_rules_create",
    request_body = ImportRuleRequest,
    responses(
        (status = 201, body = ImportRule),
        (status = 400, description = "A name, an expression, a source's parameters, an address, a selection or a language that is none — said which and why"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<ImportRuleRequest>,
) -> AppResult<(StatusCode, Json<ImportRule>)> {
    identity.require_admin()?;
    let checked = check(&state, request, None).await?;
    let params = params_text(&checked.params)?;
    let rule = repo::import_rule::create(
        &state.db,
        repo::import_rule::Fields {
            name: &checked.name,
            source: checked.source.as_str(),
            params: &params,
            list_id: checked.list_id.as_deref(),
            list_mode: checked.list_mode.as_str(),
            cron: &checked.cron,
            language: checked.language.as_deref(),
            enable_works: checked.enable_works,
            is_enabled: checked.is_enabled,
        },
        Some(&identity.label()),
    )
    .await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ImportRuleCreated,
            target: Some(&rule.id),
            detail: Some(&rule.name),
        },
    )
    .await;

    Ok((StatusCode::CREATED, Json(shown(&state, rule).await?)))
}

/// Change an import rule: every field says what it is to be.
#[utoipa::path(
    put, path = "/import/rules/{id}", tag = TAG,
    operation_id = "import_rules_update",
    params(("id" = String, Path, description = "The rule's id")),
    request_body = ImportRuleRequest,
    responses(
        (status = 200, body = ImportRule),
        (status = 400, description = "A name, an expression, a source's parameters, an address, a selection or a language that is none — said which and why"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such rule"),
    ),
)]
async fn update(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<ImportRuleRequest>,
) -> AppResult<Json<ImportRule>> {
    identity.require_admin()?;
    let current = repo::import_rule::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    let checked = check(&state, request, Some(&current)).await?;
    let params = params_text(&checked.params)?;
    let rule = repo::import_rule::update(
        &state.db,
        &id,
        repo::import_rule::Fields {
            name: &checked.name,
            source: checked.source.as_str(),
            params: &params,
            list_id: checked.list_id.as_deref(),
            list_mode: checked.list_mode.as_str(),
            cron: &checked.cron,
            language: checked.language.as_deref(),
            enable_works: checked.enable_works,
            is_enabled: checked.is_enabled,
        },
    )
    .await?
    .ok_or(AppError::NotFound)?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ImportRuleUpdated,
            target: Some(&rule.id),
            detail: Some(&rule.name),
        },
    )
    .await;

    Ok(Json(shown(&state, rule).await?))
}

/// Delete an import rule. What it brought in stays.
#[utoipa::path(
    delete, path = "/import/rules/{id}", tag = TAG,
    operation_id = "import_rules_delete",
    params(("id" = String, Path, description = "The rule's id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such rule"),
    ),
)]
async fn delete(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    identity.require_admin()?;
    let rule = repo::import_rule::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    repo::import_rule::delete(&state.db, &id).await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ImportRuleDeleted,
            target: Some(&rule.id),
            detail: Some(&rule.name),
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

/// What starting a rule came to.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleStarted {
    /// Its run, as the tasks page follows it.
    pub job_id: String,
}

/// Run an import rule now, whatever its schedule says.
#[utoipa::path(
    post, path = "/import/rules/{id}/run", tag = TAG,
    operation_id = "import_rules_run",
    params(("id" = String, Path, description = "The rule's id")),
    responses(
        (status = 202, body = RuleStarted),
        (status = 400, description = "A rule this version cannot read"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such rule"),
        (status = 409, description = "A rule is running already, or a backup is being restored"),
    ),
)]
async fn run(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<(StatusCode, Json<RuleStarted>)> {
    identity.require_admin()?;
    let job_id = rules::start(&state, &id, Some(&identity.label())).await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::TaskStarted,
            target: Some(rules::KIND),
            detail: Some(&format!("rule {id}; run {job_id}")),
        },
    )
    .await;

    Ok((StatusCode::ACCEPTED, Json(RuleStarted { job_id })))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PreviewQuery {
    /// The rule being changed, whose followed address keeps its key when it
    /// is sent back as it was shown.
    pub rule: Option<String>,
}

/// What a rule being written would bring in now: the first two hundred of
/// its source's works, set beside the catalogue as a batch's lines are.
/// Nothing is imported, and nothing kept.
#[utoipa::path(
    post, path = "/import/rules/preview", tag = TAG,
    operation_id = "import_rules_preview",
    params(PreviewQuery),
    request_body = ImportRuleRequest,
    responses(
        (status = 200, body = Preview),
        (status = 400, description = "The rule is not one this server reads, or its source could not be read — said why"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn preview(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<PreviewQuery>,
    Json(request): Json<ImportRuleRequest>,
) -> AppResult<Json<Preview>> {
    identity.require_admin()?;
    let current = match query.rule.as_deref() {
        Some(id) => repo::import_rule::get(&state.db, id).await?,
        None => None,
    };
    let checked = check(&state, request, current.as_ref()).await?;
    let wanted = rules::gather(&state, checked.source, &checked.params)
        .await
        .map_err(|e| AppError::BadRequest(format!("{e:#}")))?;
    let lines: Vec<Line> = wanted
        .iter()
        .take(PREVIEW_MOST)
        .map(|one| Line {
            text: one.label(),
            kind: one.kind,
            title: None,
        })
        .collect();
    Ok(Json(import::preview(&state, &lines, None).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::native::testing::server;

    fn tmdb_rule(name: &str) -> ImportRuleRequest {
        ImportRuleRequest {
            name: name.into(),
            source: SourceKind::Tmdb,
            kind: Some(MediaKind::Movie),
            days: Some(90),
            limit: Some(20),
            original_language: None,
            url: None,
            list_id: None,
            list_mode: None,
            cron: "0 5 * * 1".into(),
            language: None,
            enable_works: None,
            is_enabled: None,
        }
    }

    async fn created(state: &AppState, request: ImportRuleRequest) -> AppResult<ImportRule> {
        create(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Json(request),
        )
        .await
        .map(|(_, Json(rule))| rule)
    }

    #[tokio::test]
    async fn a_rule_is_written_checked_shown_and_deleted() {
        let state = server().await;
        let selection = repo::list::create(
            &state.db,
            repo::list::ListFields {
                slug: "top-of-the-quarter",
                name: "Top of the quarter",
                description: None,
                kind: repo::list::ListKind::Movie,
                mode: repo::list::ListMode::Manual,
                filter: None,
                is_public: true,
                cover: None,
            },
            None,
        )
        .await
        .unwrap();

        // Every week, TMDB's top of the quarter into the selection.
        let rule = created(
            &state,
            ImportRuleRequest {
                list_id: Some("top-of-the-quarter".into()),
                list_mode: Some(Placement::Replace),
                cron: " 0  5 * * 1 ".into(),
                ..tmdb_rule("Weekly top")
            },
        )
        .await
        .expect("created");
        assert_eq!(rule.list_id.as_deref(), Some(selection.id.as_str()));
        assert_eq!(rule.list_name.as_deref(), Some("Top of the quarter"));
        assert_eq!(rule.cron, "0 5 * * 1");
        assert_eq!(rule.days, Some(90));
        assert!(rule.next_at.is_some());
        assert!(rule.is_enabled && rule.enable_works);

        // What is no rule is said why.
        for bad in [
            ImportRuleRequest {
                name: " ".into(),
                ..tmdb_rule("x")
            },
            ImportRuleRequest {
                cron: "every monday".into(),
                ..tmdb_rule("x")
            },
            ImportRuleRequest {
                days: Some(0),
                ..tmdb_rule("x")
            },
            ImportRuleRequest {
                limit: Some(501),
                ..tmdb_rule("x")
            },
            ImportRuleRequest {
                original_language: Some("japanese".into()),
                ..tmdb_rule("x")
            },
            ImportRuleRequest {
                list_id: Some("nowhere".into()),
                ..tmdb_rule("x")
            },
            ImportRuleRequest {
                language: Some("not one".into()),
                ..tmdb_rule("x")
            },
            ImportRuleRequest {
                source: SourceKind::Feed,
                url: None,
                ..tmdb_rule("x")
            },
            ImportRuleRequest {
                source: SourceKind::Feed,
                url: Some("ftp://example.org/list.txt".into()),
                ..tmdb_rule("x")
            },
            // This machine itself, and the cloud's metadata service.
            ImportRuleRequest {
                source: SourceKind::Feed,
                url: Some("http://127.0.0.1:8479/api/v1/lists/x/ids.txt".into()),
                ..tmdb_rule("x")
            },
            ImportRuleRequest {
                source: SourceKind::Feed,
                url: Some("http://169.254.169.254/latest".into()),
                ..tmdb_rule("x")
            },
        ] {
            assert!(
                matches!(
                    created(&state, bad.clone()).await,
                    Err(AppError::BadRequest(_))
                ),
                "{bad:?}"
            );
        }

        // A list followed, with a key: shown hidden, and kept when sent back
        // as it was shown.
        let followed = created(
            &state,
            ImportRuleRequest {
                source: SourceKind::Feed,
                kind: None,
                url: Some("https://other.example/api/v1/lists/top/ids.txt?api_key=s3cret".into()),
                ..tmdb_rule("Their top")
            },
        )
        .await
        .expect("created");
        let shown_url = followed.url.clone().unwrap();
        assert!(!shown_url.contains("s3cret"), "{shown_url}");
        let Json(renamed) = update(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path(followed.id.clone()),
            Json(ImportRuleRequest {
                source: SourceKind::Feed,
                kind: None,
                url: Some(shown_url),
                is_enabled: Some(false),
                ..tmdb_rule("Their top, renamed")
            }),
        )
        .await
        .expect("updated");
        assert_eq!(renamed.name, "Their top, renamed");
        assert!(!renamed.is_enabled);
        assert!(renamed.next_at.is_none(), "switched off: no next run");
        let kept = repo::import_rule::get(&state.db, &followed.id)
            .await
            .unwrap()
            .unwrap();
        assert!(kept.params.contains("s3cret"), "{}", kept.params);

        let Json(all) = list(State(state.clone()), Extension(Identity::Anonymous))
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
        // Not for an editor's eyes, nor a visitor's.
        assert!(
            list(State(state.clone()), Extension(Identity::Visitor))
                .await
                .is_err()
        );

        // A rule's run, now: TMDB is not configured here, so it fails, and
        // says why.
        let (status, Json(started)) = run(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path(rule.id.clone()),
        )
        .await
        .expect("started");
        assert_eq!(status, StatusCode::ACCEPTED);
        let finished = async {
            loop {
                let job = repo::job::get(&state.db, &started.job_id)
                    .await
                    .unwrap()
                    .unwrap();
                if job.status != repo::job::Status::Running {
                    return job;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        };
        let job = tokio::time::timeout(std::time::Duration::from_secs(20), finished)
            .await
            .expect("the run ends");
        assert_eq!(job.status, repo::job::Status::Failed);
        assert_eq!(job.kind, rules::KIND);
        assert_eq!(job.target.as_deref(), Some(rule.id.as_str()));
        assert!(
            job.error.as_deref().unwrap_or_default().contains("TMDB"),
            "{job:?}"
        );
        let Json(all) = list(State(state.clone()), Extension(Identity::Anonymous))
            .await
            .unwrap();
        let ran = all.iter().find(|r| r.id == rule.id).unwrap();
        assert_eq!(
            ran.last_run.as_ref().map(|j| j.id.as_str()),
            Some(started.job_id.as_str())
        );

        // Deleted, and with its selection the other goes too.
        assert_eq!(
            delete(
                State(state.clone()),
                Extension(Identity::Anonymous),
                ClientIp::NONE,
                Path(followed.id.clone()),
            )
            .await
            .unwrap(),
            StatusCode::NO_CONTENT
        );
        repo::list::delete(&state.db, &selection.id).await.unwrap();
        assert!(repo::import_rule::list(&state.db).await.unwrap().is_empty());
    }
}
