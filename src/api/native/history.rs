//! A work's journal: what changed in it, value by value, and the ways back.
//!
//! Every change a refresh, a sync or a person made to a work is kept in its
//! journal (`repo::change`): what the value held, what it took, who and why.
//! It is read here a page at a time, or whole as a file; and a person's last
//! change of a value is undone here, as it was — the lock it replaced set
//! again, or none. Going back to a value the journal shows needs no route of
//! its own: it is a lock set to that value through the locks' own route,
//! with `cause: revert`, which a snapshot of a source never is.
//!
//! What a refresh would change, before it is made, is the refresh's own
//! route with `dryRun`, answered as a [`Preview`]; and a lock whose sources
//! say something else now is told beside the work's provenance
//! ([`diverging`]).

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{Download, Format, cell_of},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{
        self,
        change::{Cause, Entry, Filter, NewChange},
    },
    domain::fields::{CHOSEN_IMAGES, Child, IDENTITY, Scope},
    error::{AppError, AppResult},
    merge::provenance::{self, FieldChange, Provenance},
    state::AppState,
};

/// Filed with the audit trail: who changed what, a work at a time.
const TAG: &str = crate::api::audit::TAG;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(changes))
        .routes(routes!(undo))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ChangesQuery {
    /// One field, as the field registry names it — `overview`, `genres` —
    /// wherever it is; `number` for the episodes that moved.
    pub field: Option<String>,
    /// A person, a key or a source: who made the change, or whose answer it
    /// was — `admin`, `tmdb`.
    pub actor: Option<String>,
    /// Only the changes made at or after this instant (RFC 3339), or this
    /// day (`2026-10-01`, from its midnight UTC).
    pub since: Option<String>,
    /// Only the changes made before this instant, or this day.
    pub before: Option<String>,
    /// Where the page before ended: the `next` it was answered with.
    pub cursor: Option<String>,
    /// How many changes a page holds: fifty unless asked, five hundred at the
    /// most.
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub limit: Option<i64>,
    /// `csv` or `json`: every change the filters match, newest first, as a
    /// file to download — `cursor` and `limit` set aside.
    // Inlined: a format is no schema of its own, and a reference to one
    // only a parameter names would lead nowhere.
    #[param(inline)]
    pub format: Option<Format>,
}

/// A page of a work's journal.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: a list is many things in this document.
#[schema(as = ChangeList)]
pub struct ChangeList {
    /// Newest first; the changes one write made, in the order the work
    /// lists them.
    pub changes: Vec<Entry>,
    /// What to ask for the next page with, as `cursor`; absent after the
    /// last.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// Every field the work's journal names, for a filter to offer.
    pub fields: Vec<String>,
    /// Everybody it names — people, keys, sources — for a filter to offer.
    pub who: Vec<String>,
}

/// Read a work's journal, newest first.
///
/// Every value a refresh, a sync or a person changed — of the work, of its
/// seasons and episodes, of a child a lock names — with what it held, what it
/// took, who and why: `refresh`, `sync` and `fallback` are a source's, `lock`,
/// `unlock`, `revert` and `edit` a person's. Kept `history.keepDays` days.
/// For the people who keep the catalogue: it names who locked what. With
/// `format`, every change the filters match is handed over as a file: a CSV
/// for a spreadsheet, or a JSON array of the changes.
#[utoipa::path(
    get, path = "/items/{id}/changes", tag = TAG,
    operation_id = "history_changes",
    params(("id" = String, Path, description = "The work's identifier"), ChangesQuery),
    responses(
        (status = 200, description = "A page of the journal; with `format`, the whole of what the filters match, as a file", content(
            (ChangeList = "application/json"),
            (String = "text/csv"),
        )),
        (status = 400, description = "A period that is neither an instant nor a day"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
    ),
)]
async fn changes(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Query(query): Query<ChangesQuery>,
) -> AppResult<Response> {
    identity.require_write()?;

    let work = repo::item::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    let filter = Filter {
        field: given(query.field),
        actor: given(query.actor),
        since: instant(query.since.as_deref(), "since")?,
        before: instant(query.before.as_deref(), "before")?,
    };

    if let Some(format) = query.format {
        let name = format!(
            "journal-{}-{}",
            work.slug,
            chrono::Utc::now().format("%Y-%m-%d")
        );
        return Ok(export(state, id, filter, format, name));
    }

    let cursor = query.cursor.as_deref().filter(|c| !c.trim().is_empty());
    let page =
        repo::change::list(&state.db, &id, &filter, cursor, query.limit.unwrap_or(50)).await?;
    let (fields, who) = repo::change::named(&state.db, &id).await?;

    Ok(Json(ChangeList {
        changes: page.entries,
        next: page.next,
        fields,
        who,
    })
    .into_response())
}

/// A filter's text, when it says something.
fn given(text: Option<String>) -> Option<String> {
    text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// A bound of the period asked for, as the journal keeps its instants — in
/// UTC, to the millisecond, which sort as time does — or a 400 for what is
/// neither an instant nor a day.
fn instant(text: Option<&str>, name: &str) -> AppResult<Option<String>> {
    let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    if let Some(at) = crate::db::parse_rfc3339(text) {
        return Ok(Some(crate::db::to_rfc3339(at)));
    }
    if let Ok(day) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        let midnight = day.and_hms_opt(0, 0, 0).map(|at| at.and_utc());
        return Ok(midnight.map(crate::db::to_rfc3339));
    }
    Err(AppError::BadRequest(format!(
        "{name} is neither an instant, like 2026-10-01T09:00:00Z, nor a day, like 2026-10-01"
    )))
}

/// How many changes an export reads at a time.
const EXPORTED_PAGE: i64 = 500;

/// Every change `filter` matches in a work's journal, newest first, as a
/// file: read a page at a time after the last one read.
fn export(state: AppState, id: String, filter: Filter, format: Format, name: String) -> Response {
    let pages = futures::stream::try_unfold(
        Some(None::<String>),
        move |cursor: Option<Option<String>>| {
            let state = state.clone();
            let id = id.clone();
            let filter = filter.clone();
            async move {
                let Some(cursor) = cursor else {
                    return Ok(None);
                };
                let page =
                    repo::change::list(&state.db, &id, &filter, cursor.as_deref(), EXPORTED_PAGE)
                        .await?;
                if page.entries.is_empty() {
                    return Ok(None);
                }
                Ok(Some((page.entries, page.next.map(Some))))
            }
        },
    );

    Download {
        format,
        name,
        columns: &[
            "at",
            "cause",
            "actor",
            "provider",
            "scope",
            "field",
            "language",
            "before",
            "after",
            "wasLocked",
        ],
        cells: |entry: &Entry| {
            vec![
                entry.at.clone(),
                entry.cause.as_str().to_string(),
                entry.actor.clone().unwrap_or_default(),
                entry.provider.clone().unwrap_or_default(),
                entry.scope.clone(),
                entry.field.clone(),
                entry.language.clone().unwrap_or_default(),
                cell_of(entry.before.as_ref()),
                cell_of(entry.after.as_ref()),
                entry.was_locked.to_string(),
            ]
        },
    }
    .respond(pages)
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: what undoing a change asks for.
#[schema(as = UndoChangeRequest)]
pub struct UndoRequest {
    /// `item`, `season:3`, `episode:3x7`, or a child a lock names. Defaults
    /// to the work itself.
    #[serde(default = "the_work")]
    pub scope: String,
    pub field: String,
    /// The language the value was locked in alone: `fr`, `fra`. Absent for
    /// every language.
    #[serde(default)]
    pub language: Option<String>,
}

fn the_work() -> String {
    "item".to_string()
}

/// Undo the caller's last change of one value of a work.
///
/// The lock it set, changed or lifted is put back as it was — the lock it
/// replaced, with the value that held, or no lock at all — and the journal
/// and the audit trail call that a `revert`. What the catalogue's "Undo"
/// does for each work an action over a selection changed. Refused when
/// something else changed the value since — a refresh, or somebody else —
/// and for a row added or removed by hand, which is removed or added again.
#[utoipa::path(
    post, path = "/items/{id}/changes/undo", tag = TAG,
    operation_id = "history_undo",
    params(("id" = String, Path, description = "The work's identifier")),
    request_body = UndoRequest,
    responses(
        (status = 204, description = "Put back as it was"),
        (status = 400, description = "Unparsable scope, a value the field cannot hold, or no language"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No change of that value to undo"),
        (status = 409, description = "Something else changed the value since, or the change added or removed a row by hand"),
    ),
)]
async fn undo(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<UndoRequest>,
) -> AppResult<StatusCode> {
    identity.require_write()?;

    let language = super::overrides::lock_language(request.language.as_deref())?;
    let last = repo::change::latest(
        &state.db,
        &id,
        &request.scope,
        &request.field,
        language.as_deref().unwrap_or_default(),
    )
    .await?
    .ok_or(AppError::NotFound)?;

    // A lock a person set, lifted, put back or imported is undone as a
    // lock; what a source changed, or a merge, is no lock to put back.
    let a_lock = !last.cause.is_a_source() && last.cause != Cause::Merge;
    if !a_lock || last.actor.as_deref() != Some(identity.label().as_str()) {
        return Err(AppError::Conflict(
            "something else changed this value since; open the work's journal to put back what \
             it held"
                .into(),
        ));
    }
    // Nor is a row added or removed by hand: it is added or removed again.
    if came_or_went(&last.scope, &last.field) {
        return Err(AppError::Conflict(
            "this added or removed a row by hand; add or remove it again rather than undo it"
                .into(),
        ));
    }

    super::overrides::put_back(&state, &identity, &ip, &id, &last).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ─── writing it ──────────────────────────────────────────────────────────────

/// Write what a person changed to the work's journal, as `by` did it. Best
/// effort, as the audit trail is: the change is made already, and refusing it
/// now would leave it made and said to have failed.
pub(crate) async fn write(state: &AppState, id: &str, by: &str, changes: &[NewChange]) {
    if let Err(e) = repo::change::record(&state.db, id, Some(by), changes).await {
        tracing::warn!(%id, error = format_args!("{e:#}"), "could not write a change to the work's journal");
    }
}

/// The kinds of child a person adds or removes by hand, as the journal names
/// the field of the change that adds or removes one: a season, an episode, a
/// picture, a credit, an alternative title. Each change is of the child's own
/// scope — `season:2`, `episode:2x5`, `image:…`, `credit:…`,
/// `alternativeTitle:…` — what it held `null` when it came, and what it holds
/// `null` when it went.
pub(crate) const SEASON: &str = "season";
pub(crate) const EPISODE: &str = "episode";
pub(crate) const IMAGE: &str = "image";
pub(crate) const CREDIT: &str = "credit";
pub(crate) const ALTERNATIVE_TITLE: &str = "alternativeTitle";

/// Whether a change is a child that came or went, rather than a value of one:
/// an episode's `image` is its still, and a picture's is the picture itself.
pub(crate) fn came_or_went(scope: &str, field: &str) -> bool {
    match field {
        SEASON => scope.starts_with("season:"),
        EPISODE => scope.starts_with("episode:"),
        IMAGE | CREDIT | ALTERNATIVE_TITLE => scope.parse::<Child>().is_ok(),
        _ => false,
    }
}

/// A child a person added by hand, as the journal writes it.
pub(crate) fn came(scope: String, kind: &str, summary: Value) -> NewChange {
    NewChange {
        cause: Cause::Edit,
        scope,
        field: kind.to_string(),
        language: String::new(),
        before: None,
        after: Some(summary),
        provider: None,
        was_locked: false,
    }
}

/// A child a person removed by hand, as the journal writes it.
pub(crate) fn went(scope: String, kind: &str, summary: Value) -> NewChange {
    NewChange {
        before: Some(summary),
        after: None,
        ..came(scope, kind, Value::Null)
    }
}

/// What the journal writes of a child that came or went: a few words of it,
/// as an object — never nothing, which would read as no change.
pub(crate) mod summary {
    use serde_json::{Map, Value};

    use crate::domain::{AlternativeTitle, Credit, Episode, Image, Season};

    fn of(pairs: &[(&str, Option<Value>)]) -> Value {
        Value::Object(
            pairs
                .iter()
                .filter_map(|(key, value)| Some(((*key).to_string(), value.clone()?)))
                .collect::<Map<String, Value>>(),
        )
    }

    fn text(value: Option<&str>) -> Option<Value> {
        value
            .filter(|text| !text.trim().is_empty())
            .map(|text| Value::String(text.to_string()))
    }

    /// Its title and its day, as a refresh writes a season that came.
    pub(crate) fn season(season: &Season) -> Value {
        of(&[
            ("title", text(season.title.as_deref())),
            ("airDate", text(season.air_date.as_deref())),
        ])
    }

    /// Its title and its day, as a refresh writes an episode that came.
    pub(crate) fn episode(episode: &Episode) -> Value {
        of(&[
            ("title", text(Some(&episode.title))),
            ("airDate", text(episode.air_date.as_deref())),
        ])
    }

    /// Its kind, and the season it is of.
    pub(crate) fn image(image: &Image) -> Value {
        of(&[
            (
                "coverType",
                Some(Value::String(image.cover_type.as_str().to_string())),
            ),
            ("seasonNumber", image.season_number.map(Value::from)),
        ])
    }

    /// Who, and what they did or played.
    pub(crate) fn credit(credit: &Credit) -> Value {
        of(&[
            ("personName", text(Some(&credit.person_name))),
            ("characterName", text(credit.character_name.as_deref())),
            (
                "creditType",
                Some(Value::String(credit.credit_type.as_str().to_string())),
            ),
        ])
    }

    /// The title, and the language it is filed under.
    pub(crate) fn alternative_title(title: &AlternativeTitle) -> Value {
        of(&[
            ("title", text(Some(&title.title))),
            ("language", text(title.language.as_deref())),
        ])
    }
}

/// A yes or a no in a query — `dryRun=1` — for a route that takes one:
/// `1`, `true`, `yes` or `on` for a yes, `0`, `false`, `no`, `off` or nothing
/// for a no, and anything else refused rather than taken for either.
pub(crate) fn flag<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let raw = Option::<String>::deserialize(deserializer)?;
    let said = raw.map(|text| text.trim().to_ascii_lowercase());
    match said.as_deref() {
        None | Some("" | "0" | "false" | "no" | "off") => Ok(false),
        Some("1" | "true" | "yes" | "on") => Ok(true),
        Some(other) => Err(serde::de::Error::custom(format!(
            "{other:?} is neither a yes nor a no"
        ))),
    }
}

/// What a refresh or a sync would write over, had it been asked for in
/// earnest.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: what a dry run answers.
#[schema(as = RefreshPreview)]
pub struct Preview {
    /// The values it would write over, none of them locked — the work's own
    /// first, then its seasons and its episodes — each with what it holds
    /// and what it would hold.
    pub changes: Vec<FieldChange>,
    /// For a sync, the sources asked that answered.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub answered: Vec<String>,
    /// For a sync, the sources asked that did not.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub silent: Vec<String>,
}

impl Preview {
    /// What a preview gathered, as a dry run answers it.
    pub(crate) fn of(found: Vec<NewChange>) -> Self {
        Self {
            changes: found
                .into_iter()
                .map(|change| FieldChange {
                    scope: change.scope,
                    field: change.field,
                    before: change.before,
                    after: change.after,
                    provider: change.provider,
                })
                .collect(),
            answered: Vec::new(),
            silent: Vec::new(),
        }
    }
}

/// A lock whose sources say something else now.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: a lock and what its sources say, apart.
#[schema(as = LockDivergence)]
pub struct Divergence {
    /// Where the lock is, as it names it: `item`, `season:2`, `episode:2x5`.
    pub scope: String,
    pub field: String,
    /// What the sources give the work now, merged as a refresh merges them;
    /// `null` for nothing.
    #[schema(required)]
    pub value: Option<Value>,
    /// The source whose value that is, where one is known: of the work's own
    /// fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Whether a source changed the value since the lock was set: one the
    /// dashboard counts among the locks to look at again.
    pub moved: bool,
}

/// The locks of a work whose sources say something else now, beside what
/// they say: its locks in every language on a field of the work, a season
/// or an episode — not its identity, which is written to the work as it is
/// locked, nor the pictures it leads with — set beside the value the sources
/// gave the work as stored, where the two differ. What the editor shows under
/// a locked field, with the way to take the sources' value instead.
pub(crate) async fn diverging(
    state: &AppState,
    id: &str,
    provenance: Option<&Provenance>,
) -> AppResult<Vec<Divergence>> {
    let locks: Vec<_> = repo::override_field::list(&state.db, id)
        .await?
        .into_iter()
        .filter(|lock| {
            !lock.in_one_language()
                && lock.scope.parse::<Scope>().is_ok()
                && !IDENTITY.contains(&lock.field.as_str())
                && !CHOSEN_IMAGES.contains(&lock.field.as_str())
        })
        .collect();
    if locks.is_empty() {
        return Ok(Vec::new());
    }

    let Some(mut stored) = repo::item::get(&state.db, id).await? else {
        return Ok(Vec::new());
    };
    if locks.iter().any(|lock| lock.scope != "item") {
        repo::item::load_children(&state.db, &mut stored).await?;
    }
    let moved = repo::change::last_by_a_source(&state.db, id).await?;

    Ok(locks
        .into_iter()
        .filter_map(|lock| {
            let value = provenance::value_at(&stored, &lock.scope, &lock.field, "");
            let locked = lock.value.as_ref().filter(|value| !value.is_null());
            if provenance::same(value.as_ref(), locked) {
                return None;
            }
            let moved = moved
                .get(&(lock.scope.clone(), lock.field.clone()))
                .is_some_and(|at| at.as_str() > lock.updated_at.as_str());
            let provider = match lock.scope.as_str() {
                "item" => provenance::given_by(provenance, &lock.field),
                _ => None,
            };
            Some(Divergence {
                scope: lock.scope,
                field: lock.field,
                value,
                provider,
                moved,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use serde_json::json;

    use super::*;
    use crate::{
        api::native::overrides,
        domain::{MediaItem, MediaKind},
        merge::provenance::Source,
    };

    /// A series as its sources gave it, TMDB naming its status and its
    /// overview, with one episode.
    async fn held(state: &AppState) -> MediaItem {
        let mut work = MediaItem::empty(MediaKind::Series);
        work.title = "The Journal".into();
        work.slug = "the-journal".into();
        work.overview = Some("As TMDB tells it.".into());
        work.status = Some("ended".into());
        work.external_ids.tmdb = Some(4242);
        let mut episode = repo::child::blank_episode(1, 1);
        episode.is_manual = false;
        episode.title = "Pilot".into();
        work.episodes = vec![episode];
        let mut season = repo::child::blank_season(1);
        season.is_manual = false;
        work.seasons = vec![season];
        let from = |provider: &str| Source {
            from: provider.into(),
            agreed: Vec::new(),
            differed: Vec::new(),
            passed: Vec::new(),
        };
        let provenance = Provenance {
            fields: [
                ("overview".to_string(), from("tmdb")),
                ("status".to_string(), from("tmdb")),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        repo::item::upsert_traced(
            &state.db,
            repo::item::ItemWrite {
                item: &work,
                replace_children: true,
            },
            &provenance,
        )
        .await
        .expect("stored");
        work
    }

    /// A page of the journal, as the route answers it.
    async fn page(state: &AppState, id: &str, query: &str) -> Value {
        let response = changes(
            State(state.clone()),
            Extension(Identity::Anonymous),
            Path(id.to_string()),
            Query(serde_urlencoded::from_str(query).expect("a query")),
        )
        .await
        .expect("read");
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).expect("JSON")
    }

    fn listed(page: &Value) -> &Vec<Value> {
        page["changes"].as_array().expect("the changes")
    }

    /// A lock set on the work as the locks' route sets it — or, with
    /// `scope`, on a season or an episode.
    async fn lock(state: &AppState, id: &str, (scope, field): (&str, &str), value: Value) {
        let _ = overrides::testing::set(
            state,
            id,
            serde_json::from_value(json!({ "scope": scope, "field": field, "value": value }))
                .unwrap(),
        )
        .await
        .expect("locked");
    }

    #[test]
    fn a_dry_run_is_asked_for_with_a_yes_and_a_word_that_is_neither_is_refused() {
        #[derive(Deserialize)]
        struct Asked {
            #[serde(default, deserialize_with = "flag")]
            dry_run: bool,
        }
        let read = |query: &str| serde_urlencoded::from_str::<Asked>(query).map(|a| a.dry_run);
        for (query, said) in [
            ("", false),
            ("dry_run=", false),
            ("dry_run=0", false),
            ("dry_run=false", false),
            ("dry_run=1", true),
            ("dry_run=true", true),
            ("dry_run=Yes", true),
        ] {
            assert_eq!(read(query).ok(), Some(said), "{query}");
        }
        assert!(read("dry_run=maybe").is_err());
    }

    #[tokio::test]
    async fn a_lock_set_changed_and_lifted_is_journaled_with_what_it_replaced() {
        let state = crate::api::native::testing::server().await;
        let work = held(&state).await;

        // A few milliseconds apart, as two requests are: one write's rows
        // share an instant, and are read in the order they were written.
        let pause = || tokio::time::sleep(std::time::Duration::from_millis(3));
        lock(&state, &work.id, ("item", "overview"), json!("Mine.")).await;
        pause().await;
        lock(
            &state,
            &work.id,
            ("item", "overview"),
            json!("Mine, again."),
        )
        .await;
        pause().await;
        overrides::testing::unset(&state, &work.id, "item", "overview")
            .await
            .expect("lifted");

        let journal = page(&state, &work.id, "").await;
        let read: Vec<(&str, &Value, &Value, bool)> = listed(&journal)
            .iter()
            .map(|c| {
                (
                    c["cause"].as_str().unwrap(),
                    &c["before"],
                    &c["after"],
                    c["wasLocked"].as_bool().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            read,
            [
                // Lifted: the lock's value, then the sources' again.
                (
                    "unlock",
                    &json!("Mine, again."),
                    &json!("As TMDB tells it."),
                    true
                ),
                ("lock", &json!("Mine."), &json!("Mine, again."), true),
                // Set over what the sources gave, which no lock held.
                ("lock", &json!("As TMDB tells it."), &json!("Mine."), false),
            ]
        );
        assert!(listed(&journal).iter().all(|c| c["actor"] == "anonymous"));
        assert_eq!(journal["fields"], json!(["overview"]));
        assert_eq!(journal["who"], json!(["anonymous"]));
        assert!(journal.get("next").is_none());
    }

    #[tokio::test]
    async fn the_journal_filters_by_field_by_who_and_by_period_and_pages_with_a_cursor() {
        let state = crate::api::native::testing::server().await;
        let work = held(&state).await;
        for n in 0..3 {
            lock(
                &state,
                &work.id,
                ("item", "overview"),
                json!(format!("Take {n}.")),
            )
            .await;
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        }
        lock(&state, &work.id, ("item", "status"), json!("canceled")).await;

        let first = page(&state, &work.id, "limit=2").await;
        assert_eq!(listed(&first).len(), 2);
        assert_eq!(listed(&first)[0]["field"], "status", "newest first");
        let cursor = first["next"].as_str().expect("more");
        let rest = page(
            &state,
            &work.id,
            &serde_urlencoded::to_string([("limit", "2"), ("cursor", cursor)]).unwrap(),
        )
        .await;
        assert_eq!(listed(&rest).len(), 2);
        assert!(rest.get("next").is_none());

        let count = |query: &'static str| {
            let state = state.clone();
            let id = work.id.clone();
            async move { listed(&page(&state, &id, query).await).len() }
        };
        assert_eq!(count("field=status").await, 1);
        assert_eq!(count("actor=anonymous").await, 4);
        assert_eq!(count("actor=tmdb").await, 0);
        assert_eq!(count("since=2999-01-01").await, 0);
        assert_eq!(count("before=2999-01-01").await, 4);
        // An instant in any zone, read as the one in UTC it is.
        assert_eq!(count("since=2000-01-01T02%3A00%3A00%2B02%3A00").await, 4);

        let asked = |identity: Identity, id: &str, query: &str| {
            let state = state.clone();
            let id = id.to_string();
            let query = query.to_string();
            async move {
                changes(
                    State(state),
                    Extension(identity),
                    Path(id),
                    Query(serde_urlencoded::from_str(&query).unwrap()),
                )
                .await
            }
        };
        assert!(matches!(
            asked(Identity::Anonymous, &work.id, "since=yesterday").await,
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            asked(
                Identity::Anonymous,
                "01a0cff7-d8dc-70bc-a6b1-38379ee39bd0",
                ""
            )
            .await,
            Err(AppError::NotFound)
        ));
        // Who locked what is not a reader's to know.
        assert!(matches!(
            asked(Identity::Visitor, &work.id, "").await,
            Err(AppError::Forbidden)
        ));
    }

    #[tokio::test]
    async fn the_journal_is_handed_over_whole_as_a_spreadsheet_or_as_json() {
        let state = crate::api::native::testing::server().await;
        let work = held(&state).await;
        lock(
            &state,
            &work.id,
            ("item", "overview"),
            json!("=1+1, said \"they\""),
        )
        .await;
        lock(
            &state,
            &work.id,
            ("item", "genres"),
            json!(["Drama", "Crime"]),
        )
        .await;

        let file = |format: &'static str| {
            let state = state.clone();
            let id = work.id.clone();
            async move {
                let response = changes(
                    State(state),
                    Extension(Identity::Anonymous),
                    Path(id),
                    Query(serde_urlencoded::from_str(&format!("format={format}&limit=1")).unwrap()),
                )
                .await
                .expect("exported");
                let headers = response.headers().clone();
                let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
                (headers, String::from_utf8(bytes.to_vec()).unwrap())
            }
        };

        let (headers, csv) = file("csv").await;
        assert_eq!(headers["content-type"], "text/csv; charset=utf-8");
        let disposition = headers["content-disposition"].to_str().unwrap();
        assert!(
            disposition.starts_with("attachment; filename=\"journal-the-journal-")
                && disposition.ends_with(".csv\""),
            "{disposition}"
        );
        let lines: Vec<&str> = csv.trim_start_matches('\u{feff}').split("\r\n").collect();
        assert_eq!(
            lines[0],
            "at,cause,actor,provider,scope,field,language,before,after,wasLocked"
        );
        // Every change, whatever the page asked for, newest first, and the
        // line the last row ends with.
        assert_eq!(lines.len(), 4, "{csv}");
        assert_eq!(lines[3], "");
        assert!(
            lines[1].ends_with(",item,genres,,,\"[\"\"Drama\"\",\"\"Crime\"\"]\",false"),
            "{csv}"
        );
        // Quoted where it holds a comma, and kept from being taken for a
        // formula.
        assert!(
            lines[2]
                .ends_with(",item,overview,,As TMDB tells it.,\"'=1+1, said \"\"they\"\"\",false"),
            "{csv}"
        );

        let (headers, json) = file("json").await;
        assert_eq!(headers["content-type"], "application/json");
        let read: Vec<Value> = serde_json::from_str(&json).expect("an array of changes");
        assert_eq!(read.len(), 2);
        assert_eq!(read[0]["field"], "genres");
        assert_eq!(read[1]["after"], "=1+1, said \"they\"");
    }

    #[tokio::test]
    async fn an_undo_puts_back_the_lock_as_it_was_and_only_the_callers_own_last_change() {
        let state = crate::api::native::testing::server().await;
        let work = held(&state).await;
        let undone = |field: &'static str| {
            let state = state.clone();
            let id = work.id.clone();
            async move {
                undo(
                    State(state),
                    Extension(Identity::Anonymous),
                    ClientIp::NONE,
                    Path(id),
                    Json(UndoRequest {
                        scope: "item".into(),
                        field: field.into(),
                        language: None,
                    }),
                )
                .await
            }
        };

        // Nothing to undo.
        assert!(matches!(undone("status").await, Err(AppError::NotFound)));

        // A lock where there was none: undone, there is none again.
        lock(&state, &work.id, ("item", "status"), json!("canceled")).await;
        undone("status").await.expect("undone");
        assert_eq!(
            repo::change::lock_held(&state.db, &work.id, "item", "status", "")
                .await
                .unwrap(),
            None
        );

        // A lock changed: undone, the lock it replaced is back with its value.
        lock(&state, &work.id, ("item", "overview"), json!("First.")).await;
        lock(&state, &work.id, ("item", "overview"), json!("Second.")).await;
        undone("overview").await.expect("undone");
        assert_eq!(
            repo::change::lock_held(&state.db, &work.id, "item", "overview", "")
                .await
                .unwrap(),
            Some(Some(json!("First.")))
        );
        let last = repo::change::latest(&state.db, &work.id, "item", "overview", "")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(last.cause, Cause::Revert);
        assert_eq!(
            (last.before, last.after),
            (Some(json!("Second.")), Some(json!("First.")))
        );

        // Somebody else's change since is not undone over their head.
        repo::change::record(
            &state.db,
            &work.id,
            Some("editor:someone"),
            &[NewChange {
                cause: Cause::Lock,
                scope: "item".into(),
                field: "overview".into(),
                language: String::new(),
                before: Some(json!("First.")),
                after: Some(json!("Theirs.")),
                provider: None,
                was_locked: true,
            }],
        )
        .await
        .unwrap();
        assert!(matches!(
            undone("overview").await,
            Err(AppError::Conflict(_))
        ));

        // A row added by hand is no lock to put back: it is removed instead.
        repo::change::record(
            &state.db,
            &work.id,
            Some("anonymous"),
            &[came(
                "season:9".into(),
                SEASON,
                json!({ "title": "Extras" }),
            )],
        )
        .await
        .unwrap();
        let refused = undo(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path(work.id.clone()),
            Json(UndoRequest {
                scope: "season:9".into(),
                field: SEASON.into(),
                language: None,
            }),
        )
        .await;
        assert!(matches!(refused, Err(AppError::Conflict(_))));
    }

    #[test]
    fn a_child_that_came_or_went_is_told_from_a_value_of_one() {
        assert!(came_or_went("season:2", SEASON));
        assert!(came_or_went("episode:2x5", EPISODE));
        assert!(came_or_went("image:https://images.example/p.jpg", IMAGE));
        assert!(came_or_went("credit:actor:tmdb:17419", CREDIT));
        assert!(came_or_went(
            "alternativeTitle:fra:Les Experts",
            ALTERNATIVE_TITLE
        ));
        // An episode's still is a value of the episode, and hiding a credit
        // is a lock on it.
        assert!(!came_or_went("episode:2x5", IMAGE));
        assert!(!came_or_went("item", "title"));
        assert!(!came_or_went("credit:actor:tmdb:17419", "hidden"));
        assert!(!came_or_went("item", SEASON));

        let added = came("season:9".into(), SEASON, serde_json::json!({}));
        let removed = went("season:9".into(), SEASON, serde_json::json!({}));
        assert_eq!(
            (added.cause, added.before, added.after.is_some()),
            (Cause::Edit, None, true)
        );
        assert_eq!(
            (removed.cause, removed.before.is_some(), removed.after),
            (Cause::Edit, true, None)
        );
    }

    #[tokio::test]
    async fn a_lock_its_sources_moved_under_is_told_beside_their_value() {
        let state = crate::api::native::testing::server().await;
        let work = held(&state).await;
        let provenance = repo::item::provenance(&state.db, &work.id).await.unwrap();
        let apart = || {
            let state = state.clone();
            let id = work.id.clone();
            let provenance = provenance.clone();
            async move { diverging(&state, &id, provenance.as_ref()).await.unwrap() }
        };

        // Locked to what the sources say: nothing apart.
        lock(&state, &work.id, ("item", "status"), json!("ended")).await;
        assert!(apart().await.is_empty());

        // Locked to something else: apart, though the sources did not move.
        lock(&state, &work.id, ("item", "overview"), json!("Mine.")).await;
        assert_eq!(
            apart().await,
            [Divergence {
                scope: "item".into(),
                field: "overview".into(),
                value: Some(json!("As TMDB tells it.")),
                provider: Some("tmdb".into()),
                moved: false,
            }]
        );
        assert_eq!(
            repo::change::count_diverging(&state.db, true)
                .await
                .unwrap(),
            0
        );

        // TMDB changes it under the lock: moved, and counted.
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        sqlx::query(
            state
                .db
                .sql("UPDATE media_item SET overview = ? WHERE id = ?"),
        )
        .bind("As TMDB tells it now.")
        .bind(&work.id)
        .execute(state.db.pool())
        .await
        .unwrap();
        repo::change::record(
            &state.db,
            &work.id,
            None,
            &[NewChange {
                cause: Cause::Refresh,
                scope: "item".into(),
                field: "overview".into(),
                language: String::new(),
                before: Some(json!("As TMDB tells it.")),
                after: Some(json!("As TMDB tells it now.")),
                provider: Some("tmdb".into()),
                was_locked: true,
            }],
        )
        .await
        .unwrap();
        let moved = apart().await;
        assert_eq!(moved.len(), 1);
        assert!(moved[0].moved);
        assert_eq!(moved[0].value, Some(json!("As TMDB tells it now.")));
        assert_eq!(
            repo::change::count_diverging(&state.db, true)
                .await
                .unwrap(),
            1
        );

        // Taken, by lifting the lock: nothing apart, nothing to count.
        overrides::testing::unset(&state, &work.id, "item", "overview")
            .await
            .expect("lifted");
        assert!(apart().await.is_empty());
        assert_eq!(
            repo::change::count_diverging(&state.db, true)
                .await
                .unwrap(),
            0
        );

        // An episode's lock is set beside the episode as stored.
        lock(
            &state,
            &work.id,
            ("episode:1x1", "title"),
            json!("Pilot, as aired"),
        )
        .await;
        let episode = apart().await;
        assert_eq!(episode.len(), 1);
        assert_eq!(episode[0].scope, "episode:1x1");
        assert_eq!(episode[0].value, Some(json!("Pilot")));
        assert_eq!(episode[0].provider, None);
    }
}
