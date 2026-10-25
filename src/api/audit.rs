//! Recording and reading the audit trail.

use axum::{
    Extension, Json,
    extract::{Query as AxumQuery, State},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::extract::ClientIp,
    auth::Identity,
    db::repo::{
        self,
        audit::{Action, Entry},
    },
    error::{AppError, AppResult},
    state::AppState,
};

/// One thing that happened, ready to record.
pub struct Event<'a> {
    pub identity: Option<&'a Identity>,
    pub ip: &'a ClientIp,
    pub action: Action,
    pub target: Option<&'a str>,
    pub detail: Option<&'a str>,
}

/// Record an event, best effort.
///
/// A failure here is logged and swallowed: the action already happened, and
/// refusing it after the fact because the trail could not be written would be
/// worse than an incomplete trail.
pub async fn record(state: &AppState, event: Event<'_>) {
    let actor = event.identity.map(Identity::label);
    let ip = event.ip.as_text();

    let target = event.target.map(clip);
    let detail = event.detail.map(clip);

    let result = repo::audit::record(
        &state.db,
        repo::audit::Record {
            actor: actor.as_deref(),
            action: event.action,
            target: target.as_deref(),
            detail: detail.as_deref(),
            ip: ip.as_deref(),
        },
    )
    .await;

    if let Err(e) = result {
        tracing::warn!(action = %event.action, error = %e, "could not write the audit entry");
    }

    // A lock, an edit, a sync: told to the operator's own Sonarr or Radarr
    // too, when that is asked for.
    crate::arr::push::heard(state, event.action, target.as_deref());

    // Owed to the destinations that want it, written before this returns:
    // the sending is the webhook worker's, and a restart loses none of it.
    crate::service::webhook::notify(
        state,
        crate::service::webhook::Happening {
            action: event.action,
            actor,
            target,
            detail,
        },
    )
    .await;
}

/// Who the trail says did what this server does of its own accord.
const SYSTEM: &str = "system";

/// Record what this server did of its own accord — a refresh that found
/// TheTVDB had moved episodes — as [`record`] does a person's doing: in the
/// trail as `system`'s, from no address, and owed to the webhooks that want it.
pub async fn record_system(state: &AppState, action: Action, target: &str, detail: &str) {
    let target = clip(target);
    let detail = clip(detail);

    let result = repo::audit::record(
        &state.db,
        repo::audit::Record {
            actor: Some(SYSTEM),
            action,
            target: Some(&target),
            detail: Some(&detail),
            ip: None,
        },
    )
    .await;

    if let Err(e) = result {
        tracing::warn!(action = %action, error = %e, "could not write the audit entry");
    }

    crate::service::webhook::notify(
        state,
        crate::service::webhook::Happening {
            action,
            actor: Some(SYSTEM.to_string()),
            target: Some(target),
            detail: Some(detail),
        },
    )
    .await;
}

/// The most of any one field this trail will hold.
///
/// Some of what is recorded comes from whoever is calling — the username of a
/// failed sign-in, most of all, and that one needs no credential at all. Without
/// a ceiling, a megabyte of it is a megabyte of row, six hundred times a minute,
/// kept for ninety days.
const FIELD_LIMIT: usize = 200;

/// Cut a field to [`FIELD_LIMIT`], on a character boundary.
fn clip(text: &str) -> String {
    match text.char_indices().nth(FIELD_LIMIT) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

// ─── endpoint ────────────────────────────────────────────────────────────────

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Audit";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(list))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub action: Option<String>,
    pub actor: Option<String>,
    pub target: Option<String>,
    pub since: Option<String>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub limit: Option<i64>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub offset: Option<i64>,
    /// Where the page starts: the `nextCursor` the page before was answered
    /// with, under the same filters. It goes on past the last entry read, so
    /// what is recorded meanwhile neither repeats an entry nor skips one, as
    /// an offset would. Given instead of `offset`, never with one.
    pub cursor: Option<String>,
    /// `csv` or `json`: every entry the filters match, newest first, as a
    /// file to download — the page asked for, `limit`, `offset` and
    /// `cursor`, set aside.
    // Inlined: a format is no schema of its own, and a reference to one
    // only a parameter names would lead nowhere.
    #[param(inline)]
    pub format: Option<Format>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: several modules declare a type with this name, and
// utoipa keys schemas on the leaf name alone — a collision silently
// drops one of them from the spec.
#[schema(as = AuditListResponse)]
pub struct ListResponse {
    pub entries: Vec<Entry>,
    pub total: i64,
    /// Every action name, so a UI can offer them as filters without hard-coding.
    pub actions: Vec<&'static str>,
    /// Where the next page begins: give it back as `cursor`. Absent after
    /// the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Where the next page of the trail begins: past the entry read last, by
/// the instant it was recorded and its id, which order the trail.
#[derive(Debug, Serialize, Deserialize)]
struct Place {
    at: String,
    id: String,
}

impl Place {
    fn of(entry: &Entry) -> String {
        crate::api::cursor::encode(&Place {
            at: entry.at.clone(),
            id: entry.id.clone(),
        })
    }
}

/// Read the audit trail, newest first.
///
/// Administrative: it names who did what, so it requires an administrator or a
/// key carrying the `admin` scope. A page is read from `offset`, or from a
/// `cursor`: the `nextCursor` the page before was answered with, which goes
/// on past the last entry it held. With `format`, every entry the filters
/// match is handed over as a file: a CSV for a spreadsheet — a column for
/// the instant, who, the action, the target, the work it names, the note and
/// the address — or a JSON array of the entries.
#[utoipa::path(
    get, path = "/audit", tag = TAG,
    operation_id = "audit_list",
    params(ListQuery),
    responses(
        (status = 200, description = "A page of the trail; with `format`, the whole of what the filters match, as a file", content(
            (ListResponse = "application/json"),
            (String = "text/csv"),
        )),
        (status = 400, description = "A cursor is not one the trail gave, or was given beside an offset"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    AxumQuery(query): AxumQuery<ListQuery>,
) -> AppResult<Response> {
    if !identity.is_admin() {
        return Err(AppError::Forbidden);
    }

    let cursor = crate::api::cursor::given(query.cursor.as_deref(), query.offset)?
        .map(crate::api::cursor::decode::<Place>)
        .transpose()?;
    let filter = repo::audit::Query {
        action: query.action,
        actor: query.actor,
        target: query.target,
        since: query.since,
        limit: query.limit.unwrap_or(100),
        offset: query.offset.unwrap_or(0),
    };

    if let Some(format) = query.format {
        return Ok(export(state, filter, format));
    }

    let limit = filter.limit.clamp(1, 500);
    let (mut entries, next_cursor) = match &cursor {
        // Past the entry read last: one more than the page, to know whether
        // another follows.
        Some(after) => {
            let mut entries = repo::audit::page_after(
                &state.db,
                &filter,
                Some((after.at.as_str(), after.id.as_str())),
                limit + 1,
            )
            .await?;
            let more = entries.len() as i64 > limit;
            entries.truncate(limit as usize);
            let next = more.then(|| entries.last().map(Place::of)).flatten();
            (entries, next)
        }
        None => (repo::audit::list(&state.db, &filter).await?, None),
    };
    // As many as the filters match, so the pages end where the entries do.
    let total = repo::audit::count_matching(&state.db, &filter).await?;
    // A page read by offset says where the next begins too, so a reader can
    // go on by cursor from any page.
    let next_cursor = match cursor {
        Some(_) => next_cursor,
        None => (filter.offset.max(0) + (entries.len() as i64) < total)
            .then(|| entries.last().map(Place::of))
            .flatten(),
    };
    name_works(&state, &mut entries).await?;

    Ok(Json(ListResponse {
        entries,
        total,
        actions: Action::ALL.iter().map(|a| a.as_str()).collect(),
        next_cursor,
    })
    .into_response())
}

/// A work is named by its id, and read by its title.
async fn name_works(state: &AppState, entries: &mut [Entry]) -> anyhow::Result<()> {
    let ids: Vec<String> = entries
        .iter()
        .filter_map(named_work)
        .map(str::to_string)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let works = repo::item::titles(&state.db, &ids).await?;
    for entry in entries {
        entry.work = named_work(entry).and_then(|id| works.get(id)).cloned();
    }
    Ok(())
}

/// How many entries of the trail an export reads at a time.
const EXPORTED_PAGE: i64 = 500;

/// Every entry `filter` matches, newest first, as a file: read a page at a
/// time after the last one read, so that what is written meanwhile neither
/// repeats an entry nor skips one.
fn export(state: AppState, filter: repo::audit::Query, format: Format) -> Response {
    let pages = futures::stream::try_unfold(
        Some(None::<(String, String)>),
        move |after: Option<Option<(String, String)>>| {
            let state = state.clone();
            let filter = filter.clone();
            async move {
                let Some(after) = after else {
                    return Ok(None);
                };
                let mut page = repo::audit::page_after(
                    &state.db,
                    &filter,
                    after.as_ref().map(|(at, id)| (at.as_str(), id.as_str())),
                    EXPORTED_PAGE,
                )
                .await?;
                let Some(last) = page.last() else {
                    return Ok(None);
                };
                // A short page is the last: no need to ask for another.
                let next = (page.len() as i64 == EXPORTED_PAGE)
                    .then(|| (last.at.clone(), last.id.clone()));
                name_works(&state, &mut page).await?;
                Ok(Some((page, next.map(Some))))
            }
        },
    );

    Download {
        format,
        name: format!("audit-{}", chrono::Utc::now().format("%Y-%m-%d")),
        columns: &["at", "actor", "action", "target", "work", "detail", "ip"],
        cells: |entry: &Entry| {
            vec![
                entry.at.clone(),
                entry.actor.clone().unwrap_or_default(),
                entry.action.clone(),
                entry.target.clone().unwrap_or_default(),
                entry
                    .work
                    .as_ref()
                    .map(|work| work.title.clone())
                    .unwrap_or_default(),
                entry.detail.clone().unwrap_or_default(),
                entry.ip.clone().unwrap_or_default(),
            ]
        },
    }
    .respond(pages)
}

/// The work an entry acted on, where its action acts on one: a work's own
/// actions and its locks name it by the id their target opens with — `{id}`,
/// or `{id}#{scope}/{field}` — and an import by the id its note ends with. A
/// refused sign-in's target is whatever name was typed, and names no work
/// however it is shaped.
fn named_work(entry: &Entry) -> Option<&str> {
    let action = entry.action.as_str();
    if action == Action::ItemImported.as_str() {
        return entry
            .detail
            .as_deref()
            .and_then(|d| d.rsplit(' ').next())
            .and_then(work_id);
    }
    if action.starts_with("item.") || action.starts_with("override.") {
        return entry.target.as_deref().and_then(work_id);
    }
    None
}

/// The work id a target opens with, if it opens with one.
fn work_id(target: &str) -> Option<&str> {
    let id = target.get(..36)?;
    let shaped = id.bytes().enumerate().all(|(i, b)| match i {
        8 | 13 | 18 | 23 => b == b'-',
        _ => b.is_ascii_hexdigit(),
    });
    let whole = matches!(target.as_bytes().get(36), None | Some(b'#'));
    (shaped && whole).then_some(id)
}

// ─── downloads ───────────────────────────────────────────────────────────────

/// The forms a list is downloaded in: `csv` for a spreadsheet, `json` for a
/// program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
// Named explicitly: a format is many things in this document.
#[schema(as = DownloadFormat)]
pub enum Format {
    Csv,
    Json,
}

/// A list handed over as a file — the audit trail, a work's journal — written
/// out a page at a time as it is read from the database, however long it has
/// grown, and never held whole.
pub(crate) struct Download<T> {
    pub format: Format,
    /// The file's name, without its extension.
    pub name: String,
    /// A CSV's columns, and how one entry fills them.
    pub columns: &'static [&'static str],
    pub cells: fn(&T) -> Vec<String>,
}

impl<T: Serialize + Send + 'static> Download<T> {
    /// The file, from `pages`: what the list holds, a page at a time. A page
    /// that cannot be read ends the file there, cut short, and is logged —
    /// the answer was begun, and cannot be refused any more.
    pub(crate) fn respond<S>(self, pages: S) -> axum::response::Response
    where
        S: futures::Stream<Item = anyhow::Result<Vec<T>>> + Send + 'static,
    {
        use axum::{
            body::{Body, Bytes},
            http::{HeaderValue, header},
            response::IntoResponse,
        };
        use futures::StreamExt as _;

        let Download {
            format,
            name,
            columns,
            cells,
        } = self;
        // A spreadsheet takes a file for UTF-8 by the mark it opens with, and
        // a title in Japanese for mojibake without it.
        let head = match format {
            Format::Csv => format!("\u{feff}{}", csv_row(columns.iter().map(|c| c.to_string()))),
            Format::Json => "[".to_string(),
        };
        let tail = match format {
            Format::Csv => String::new(),
            Format::Json => "\n]\n".to_string(),
        };

        let mut first = true;
        let body = pages.map(move |page| -> Result<Bytes, std::io::Error> {
            let page = page.map_err(|e| {
                tracing::warn!(error = format_args!("{e:#}"), "a download was cut short");
                std::io::Error::other(e.to_string())
            })?;
            let mut chunk = String::new();
            for entry in &page {
                match format {
                    Format::Csv => chunk.push_str(&csv_row(cells(entry))),
                    Format::Json => {
                        if !first {
                            chunk.push(',');
                        }
                        first = false;
                        chunk.push('\n');
                        chunk.push_str(
                            &serde_json::to_string(entry).map_err(std::io::Error::other)?,
                        );
                    }
                }
            }
            Ok(Bytes::from(chunk))
        });
        let whole = futures::stream::once(async move { Ok(Bytes::from(head)) })
            .chain(body)
            .chain(futures::stream::once(async move { Ok(Bytes::from(tail)) }));

        let (kind, extension) = match format {
            Format::Csv => ("text/csv; charset=utf-8", "csv"),
            Format::Json => ("application/json", "json"),
        };
        let mut response = Body::from_stream(whole).into_response();
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
        // The name is made here, of letters, digits, dashes and dots.
        if let Ok(disposition) =
            HeaderValue::from_str(&format!("attachment; filename=\"{name}.{extension}\""))
        {
            headers.insert(header::CONTENT_DISPOSITION, disposition);
        }
        // Who did what is not for a cache to keep.
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

/// A row of a CSV file, as RFC 4180 has it: its cells between commas, ended
/// by a carriage return and a line feed.
pub(crate) fn csv_row(cells: impl IntoIterator<Item = String>) -> String {
    let mut row = cells
        .into_iter()
        .map(|cell| csv_cell(&cell))
        .collect::<Vec<_>>()
        .join(",");
    row.push_str("\r\n");
    row
}

/// One cell of a CSV file: quoted when it holds what would end it — a comma,
/// a quote, a line break — and kept from being taken for a formula by a
/// spreadsheet, which reads one in a cell that opens with `=`, `+`, `-`, `@`
/// or a tab: a sign-in refused under a name typed as `=HYPERLINK(…)` would
/// otherwise be a link in it.
fn csv_cell(text: &str) -> String {
    let guarded = if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{text}")
    } else {
        text.to_string()
    };
    if guarded.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

/// A value of a list as a cell says it: a text as it reads, nothing as
/// nothing, anything else — a number, a list — as its JSON.
pub(crate) fn cell_of(value: Option<&serde_json::Value>) -> String {
    match value {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_distinct_name() {
        let mut names: Vec<&str> = Action::ALL.iter().map(|a| a.as_str()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();

        assert_eq!(names.len(), count, "two actions share a name");
    }

    #[test]
    fn a_target_names_a_work_by_the_id_it_opens_with() {
        let id = "01a0cff7-d8dc-70bc-a6b1-38379ee39bd0";
        assert_eq!(work_id(id), Some(id));
        assert_eq!(work_id(&format!("{id}#item/genres")), Some(id));
        assert_eq!(work_id(&format!("{id}#episode:5x25/title")), Some(id));
        for other in [
            "203.0.113.31",
            "peer:tmdb.language",
            "admin",
            &format!("{id}x"),
        ] {
            assert_eq!(work_id(other), None, "{other}");
        }
    }

    #[test]
    fn only_an_action_on_a_work_names_one() {
        let id = "01a0cff7-d8dc-70bc-a6b1-38379ee39bd0";
        let entry = |action: Action, target: &str, detail: Option<&str>| Entry {
            id: String::new(),
            at: String::new(),
            actor: None,
            action: action.as_str().into(),
            target: Some(target.into()),
            detail: detail.map(String::from),
            ip: None,
            work: None,
        };

        assert_eq!(
            named_work(&entry(
                Action::OverrideSet,
                &format!("{id}#item/genres"),
                None
            )),
            Some(id)
        );
        assert_eq!(named_work(&entry(Action::ItemDeleted, id, None)), Some(id));
        assert_eq!(
            named_work(&entry(
                Action::ItemImported,
                "Blade Runner 2099",
                Some(&format!("series {id}"))
            )),
            Some(id)
        );
        // Typed into the sign-in form, shaped like an id or not.
        assert_eq!(named_work(&entry(Action::SignInFailed, id, None)), None);
    }

    #[test]
    fn a_cell_is_quoted_where_it_must_be_and_never_taken_for_a_formula() {
        let row = csv_row(
            [
                "plain",
                "a, b",
                "say \"hi\"",
                "=HYPERLINK(\"x\")",
                "-3",
                "@home",
                "two\nlines",
                "",
            ]
            .map(String::from),
        );
        assert_eq!(
            row,
            "plain,\"a, b\",\"say \"\"hi\"\"\",\"'=HYPERLINK(\"\"x\"\")\",'-3,'@home,\"two\nlines\",\r\n"
        );
        assert_eq!(cell_of(None), "");
        assert_eq!(cell_of(Some(&serde_json::json!("text"))), "text");
        assert_eq!(
            cell_of(Some(&serde_json::json!(["a", "b"]))),
            "[\"a\",\"b\"]"
        );
    }

    #[tokio::test]
    async fn the_trail_is_handed_over_whole_with_the_same_filters_to_an_administrator_alone() {
        use axum::body::to_bytes;

        let state = crate::api::native::testing::server().await;
        for (actor, action) in [
            ("admin:alice", Action::OverrideSet),
            ("admin:alice", Action::OverrideRemoved),
            ("editor:bob", Action::OverrideSet),
            ("admin:alice", Action::SignedIn),
        ] {
            repo::audit::record(
                &state.db,
                repo::audit::Record {
                    actor: Some(actor),
                    action,
                    target: Some("=cmd|' /C calc'!A0"),
                    detail: Some("set"),
                    ip: Some("203.0.113.7"),
                },
            )
            .await
            .unwrap();
        }
        let file = |identity: Identity, query: &'static str| {
            let state = state.clone();
            async move {
                list(
                    State(state),
                    Extension(identity),
                    AxumQuery(serde_urlencoded::from_str(query).unwrap()),
                )
                .await
            }
        };

        let csv = file(
            Identity::Anonymous,
            "format=csv&actor=admin%3Aalice&limit=1",
        )
        .await
        .expect("exported");
        assert_eq!(csv.headers()["content-type"], "text/csv; charset=utf-8");
        let body = to_bytes(csv.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        let lines: Vec<&str> = text.trim_start_matches('\u{feff}').split("\r\n").collect();
        assert_eq!(lines[0], "at,actor,action,target,work,detail,ip");
        // Every entry of the actor's, whatever page was asked for, and the
        // target kept from being taken for a formula.
        assert_eq!(lines.len(), 5, "{text}");
        assert!(
            lines[1].contains(",admin:alice,auth.signed_in,'=cmd|' /C calc'!A0,,set,203.0.113.7"),
            "{text}"
        );

        let json = file(Identity::Anonymous, "format=json&action=override.set")
            .await
            .expect("exported");
        let body = to_bytes(json.into_body(), usize::MAX).await.unwrap();
        let read: Vec<serde_json::Value> = serde_json::from_slice(&body).expect("an array");
        assert_eq!(read.len(), 2);
        assert!(read.iter().all(|entry| entry["action"] == "override.set"));

        assert!(matches!(
            file(Identity::Visitor, "format=csv").await,
            Err(AppError::Forbidden)
        ));
    }

    #[tokio::test]
    async fn the_trail_pages_by_cursor_past_what_is_recorded_meanwhile() {
        use axum::body::to_bytes;

        let state = crate::api::native::testing::server().await;
        let record = |target: &'static str| {
            let state = state.clone();
            async move {
                repo::audit::record(
                    &state.db,
                    repo::audit::Record {
                        actor: Some("admin:alice"),
                        action: Action::SettingChanged,
                        target: Some(target),
                        detail: None,
                        ip: None,
                    },
                )
                .await
                .unwrap();
            }
        };
        for target in ["one", "two", "three", "four", "five"] {
            record(target).await;
        }
        let page = |query: String| {
            let state = state.clone();
            async move {
                let response = list(
                    State(state),
                    Extension(Identity::Anonymous),
                    AxumQuery(serde_urlencoded::from_str(&query).unwrap()),
                )
                .await?;
                let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
                Ok::<_, AppError>(serde_json::from_slice::<serde_json::Value>(&body).unwrap())
            }
        };
        let targets = |page: &serde_json::Value| -> Vec<String> {
            page["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["target"].as_str().unwrap().to_string())
                .collect()
        };

        // Newest first, two at a time; the first page read by offset.
        let first = page("limit=2".into()).await.unwrap();
        assert_eq!(targets(&first), ["five", "four"]);
        let cursor = first["nextCursor"].as_str().expect("more").to_string();

        // Recorded while the reader reads: no entry repeats, none is skipped.
        record("six").await;
        let second = page(format!("limit=2&cursor={}", urlencoding::encode(&cursor)))
            .await
            .unwrap();
        assert_eq!(targets(&second), ["three", "two"]);
        assert_eq!(second["total"], 6);
        let last = page(format!(
            "limit=2&cursor={}",
            urlencoding::encode(second["nextCursor"].as_str().expect("more"))
        ))
        .await
        .unwrap();
        assert_eq!(targets(&last), ["one"]);
        assert!(last.get("nextCursor").is_none());

        // The filters still narrow it.
        let narrowed = page(format!(
            "limit=2&target=one&cursor={}",
            urlencoding::encode(&cursor)
        ))
        .await
        .unwrap();
        assert_eq!(targets(&narrowed), ["one"]);

        for refused in [
            format!("offset=2&cursor={}", urlencoding::encode(&cursor)),
            "cursor=nonsense".to_string(),
        ] {
            assert!(
                matches!(page(refused.clone()).await, Err(AppError::BadRequest(_))),
                "{refused}"
            );
        }
    }

    #[test]
    fn action_names_are_namespaced() {
        // A log shipper filters on the prefix, so every name must carry one.
        for action in Action::ALL {
            assert!(
                action.as_str().contains('.'),
                "{} is not namespaced",
                action.as_str()
            );
        }
    }
}
