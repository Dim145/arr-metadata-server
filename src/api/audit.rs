//! Recording and reading the audit trail.

use axum::{
    Extension, Json,
    extract::{Query as AxumQuery, State},
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
}

/// Read the audit trail, newest first.
///
/// Administrative: it names who did what, so it requires an administrator or a
/// key carrying the `admin` scope.
#[utoipa::path(
    get, path = "/audit", tag = TAG,
    params(ListQuery),
    responses(
        (status = 200, body = ListResponse),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    AxumQuery(query): AxumQuery<ListQuery>,
) -> AppResult<Json<ListResponse>> {
    if !identity.is_admin() {
        return Err(AppError::Forbidden);
    }

    let filter = repo::audit::Query {
        action: query.action,
        actor: query.actor,
        target: query.target,
        since: query.since,
        limit: query.limit.unwrap_or(100),
        offset: query.offset.unwrap_or(0),
    };
    let mut entries = repo::audit::list(&state.db, &filter).await?;

    // A work is named by its id, and read by its title.
    let ids: Vec<String> = entries
        .iter()
        .filter_map(named_work)
        .map(str::to_string)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let works = repo::item::titles(&state.db, &ids).await?;
    for entry in &mut entries {
        entry.work = named_work(entry).and_then(|id| works.get(id)).cloned();
    }

    Ok(Json(ListResponse {
        entries,
        // As many as the filters match, so the pages end where the entries do.
        total: repo::audit::count_matching(&state.db, &filter).await?,
        actions: Action::ALL.iter().map(|a| a.as_str()).collect(),
    }))
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
