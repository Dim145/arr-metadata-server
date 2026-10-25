//! The programmer's note: read beside what it is about, by whoever may read
//! that, and written by whoever maintains the catalogue — from the
//! administration or in place, on the page itself. See `repo::editorial`.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
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
    db::repo::{
        self,
        audit::Action,
        editorial::{Note, Scope},
    },
    error::{AppError, AppResult},
    state::AppState,
};

/// Filed with the rest of the catalogue in the documentation.
const TAG: &str = super::items::TAG;

/// The most notes one listing answers with.
const MOST: i64 = 500;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(read))
        .routes(routes!(write, remove))
}

/// What a thing a note is about is called in an address: its scope, then
/// its key — `genre/drama`, `year/1999`, `work/<id>`.
fn scope_of(raw: &str) -> AppResult<Scope> {
    raw.parse().map_err(AppError::BadRequest)
}

/// The language a note is filed under: the first part of a language tag, in
/// lower case — `fr` for `fr-FR`.
fn language_of(raw: &str) -> AppResult<String> {
    let tag = crate::api::extract::language(Some(raw))?
        .ok_or_else(|| AppError::BadRequest("a note is written in a language: en, fr".into()))?;
    Ok(repo::genre::language_of(&tag))
}

/// The key a note is filed under, from the one an address gives, checked
/// for the scope: a genre's slug from any name it goes by, a network's slug,
/// a year as four digits, a decade by its first year, a work's or a
/// selection's id — the work or the selection one this caller may see.
pub(super) async fn key_of(
    state: &AppState,
    identity: &Identity,
    scope: Scope,
    raw: &str,
) -> AppResult<String> {
    let raw = raw.trim();
    let bad = |what: &str| AppError::BadRequest(what.to_string());
    match scope {
        Scope::Work => {
            if uuid::Uuid::parse_str(raw).is_err() {
                return Err(AppError::NotFound);
            }
            Ok(super::visible_work(state, identity, raw).await?.id)
        }
        Scope::List => {
            if raw.is_empty() || raw.len() > 128 {
                return Err(AppError::NotFound);
            }
            Ok(super::lists::visible(state, identity, raw).await?.id)
        }
        Scope::Genre => repo::genre::slug_of(raw)
            .filter(|slug| slug.chars().count() <= 128)
            .ok_or_else(|| bad("a genre is named by its slug: drama")),
        Scope::Network => Some(slug::slugify(raw))
            .filter(|slug| !slug.is_empty() && slug.len() <= 128)
            .ok_or_else(|| bad("a network is named by its slug: hbo")),
        Scope::Year | Scope::Decade => {
            let year = raw
                .parse::<i32>()
                .ok()
                .filter(|year| (1800..=2200).contains(year))
                .ok_or_else(|| bad("a year is four digits, from 1800 to 2200"))?;
            Ok(if scope == Scope::Decade {
                year - year.rem_euclid(10)
            } else {
                year
            }
            .to_string())
        }
    }
}

// ─── reading ─────────────────────────────────────────────────────────────────

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReadQuery {
    /// The reader's language: `fr`. The note in it, else in English, else the
    /// one written last. Absent, English.
    pub language: Option<String>,
}

/// The note on one thing, for a reader.
///
/// In the reader's language where there is one, else in English, else in
/// whichever language one was written in last: `language` in the answer says
/// which it is. A work's note is read by whoever may see the work, a
/// selection's by whoever may read the selection.
#[utoipa::path(
    get, path = "/editorial/{scope}/{key}", tag = TAG,
    operation_id = "editorial_read",
    params(
        ("scope" = String, Path, description = "work, list, genre, network, year or decade"),
        ("key" = String, Path, description = "A work's or a selection's id, a genre's or a network's slug, a year, a decade's first year"),
        ReadQuery,
    ),
    responses(
        (status = 200, body = Note),
        (status = 400, description = "Not a scope, a key or a language"),
        (status = 404, description = "Nothing is written about it, or the reader may not see it"),
    ),
)]
async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path((scope, key)): Path<(String, String)>,
    Query(query): Query<ReadQuery>,
) -> AppResult<Json<Note>> {
    let scope = scope_of(&scope)?;
    let key = key_of(&state, &identity, scope, &key).await?;
    let language = match query.language.as_deref().map(str::trim) {
        Some(raw) if !raw.is_empty() => language_of(raw)?,
        _ => "en".to_string(),
    };
    repo::editorial::read(&state.db, scope, &key, &language)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// Only the notes on works, lists, genres, networks, years or decades.
    pub scope: Option<String>,
    /// Only those on this one, with `scope`: every language it is written in.
    pub key: Option<String>,
    /// Only those written in this language: `fr`.
    pub language: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[schema(as = EditorialNotes)]
pub struct NoteList {
    /// The one written last first.
    pub notes: Vec<Note>,
}

/// Every note, for whoever maintains the catalogue: the one written last
/// first, at most five hundred.
#[utoipa::path(
    get, path = "/editorial", tag = TAG,
    operation_id = "editorial_list",
    params(ListQuery),
    responses(
        (status = 200, body = NoteList),
        (status = 400, description = "Not a scope or a language, or a key without its scope"),
        (status = 403, description = "The caller may not edit the catalogue"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<NoteList>> {
    identity.require_write()?;
    let scope = query
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(scope_of)
        .transpose()?;
    let key = match (query.key.as_deref().map(str::trim), scope) {
        (Some(key), Some(scope)) if !key.is_empty() => {
            Some(key_of(&state, &identity, scope, key).await?)
        }
        (Some(key), None) if !key.is_empty() => {
            return Err(AppError::BadRequest(
                "a key is read with its scope: scope=genre&key=drama".into(),
            ));
        }
        _ => None,
    };
    let language = query
        .language
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(language_of)
        .transpose()?;
    Ok(Json(NoteList {
        notes: repo::editorial::list(&state.db, scope, key.as_deref(), language.as_deref(), MOST)
            .await?,
    }))
}

// ─── writing ─────────────────────────────────────────────────────────────────

#[derive(Deserialize, ToSchema)]
#[schema(as = EditorialNoteRequest)]
pub struct NoteRequest {
    /// What it says: paragraphs, `*emphasis*`, `**strong emphasis**` and
    /// `[links](/genre/drama)` to pages of this site, at most three thousand
    /// characters. Empty, the note in this language is taken away.
    pub body: String,
}

/// Write the note on one thing in one language, in place of the one there was.
///
/// Its text is kept tidy — its line ends made one, its control characters
/// taken out, one empty line at most between two paragraphs — and refused
/// when it holds HTML or a link to anywhere but a page of this site. An empty
/// text takes the note away. Recorded in the journal as `editorial.updated`.
#[utoipa::path(
    put, path = "/editorial/{scope}/{key}/{language}", tag = TAG,
    operation_id = "editorial_write",
    params(
        ("scope" = String, Path, description = "work, list, genre, network, year or decade"),
        ("key" = String, Path, description = "A work's or a selection's id, a genre's or a network's slug, a year, a decade's first year"),
        ("language" = String, Path, description = "The language it is written in: en, fr"),
    ),
    request_body = NoteRequest,
    responses(
        (status = 200, body = Note),
        (status = 204, description = "The text was empty: the note in this language is taken away"),
        (status = 400, description = "Not a scope, a key or a language, or a text with HTML, a link elsewhere or too many characters"),
        (status = 403, description = "The caller may not edit the catalogue"),
        (status = 404, description = "No such work or selection"),
    ),
)]
async fn write(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((scope, key, language)): Path<(String, String, String)>,
    Json(request): Json<NoteRequest>,
) -> AppResult<Response> {
    identity.require_write()?;
    let scope = scope_of(&scope)?;
    let key = key_of(&state, &identity, scope, &key).await?;
    let language = language_of(&language)?;
    let body = repo::editorial::clean(&request.body).map_err(AppError::BadRequest)?;
    let target = format!("{}:{key}@{language}", scope.as_str());

    if body.is_empty() {
        let removed = repo::editorial::remove(&state.db, scope, &key, &language).await?;
        if removed {
            record(&state, &identity, &ip, &target, "taken away").await;
        }
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    let label = identity.label();
    let note = repo::editorial::put(&state.db, scope, &key, &language, &body, Some(&label)).await?;
    let excerpt: String = repo::editorial::plain(&note.body)
        .chars()
        .take(120)
        .collect();
    record(&state, &identity, &ip, &target, &excerpt).await;
    tracing::info!(note = %target, actor = %label, "wrote a programmer's note");
    Ok(Json(note).into_response())
}

/// Take the note on one thing in one language away.
#[utoipa::path(
    delete, path = "/editorial/{scope}/{key}/{language}", tag = TAG,
    operation_id = "editorial_remove",
    params(
        ("scope" = String, Path, description = "work, list, genre, network, year or decade"),
        ("key" = String, Path, description = "A work's or a selection's id, a genre's or a network's slug, a year, a decade's first year"),
        ("language" = String, Path, description = "The language it is written in: en, fr"),
    ),
    responses(
        (status = 204, description = "Taken away"),
        (status = 400, description = "Not a scope, a key or a language"),
        (status = 403, description = "The caller may not edit the catalogue"),
        (status = 404, description = "Nothing was written about it in that language"),
    ),
)]
async fn remove(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((scope, key, language)): Path<(String, String, String)>,
) -> AppResult<StatusCode> {
    identity.require_write()?;
    let scope = scope_of(&scope)?;
    let language = language_of(&language)?;
    // A note on a work or a selection gone since is still taken away: its
    // key is the one it was filed under.
    let key = match key_of(&state, &identity, scope, &key).await {
        Ok(key) => key,
        Err(AppError::NotFound) if matches!(scope, Scope::Work | Scope::List) => {
            key.trim().to_string()
        }
        Err(e) => return Err(e),
    };
    if !repo::editorial::remove(&state.db, scope, &key, &language).await? {
        return Err(AppError::NotFound);
    }
    let target = format!("{}:{key}@{language}", scope.as_str());
    record(&state, &identity, &ip, &target, "taken away").await;
    Ok(StatusCode::NO_CONTENT)
}

async fn record(state: &AppState, identity: &Identity, ip: &ClientIp, target: &str, detail: &str) {
    audit::record(
        state,
        Event {
            identity: Some(identity),
            ip,
            action: Action::EditorialUpdated,
            target: Some(target),
            detail: Some(detail),
        },
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn written(
        state: &AppState,
        identity: &Identity,
        path: (&str, &str, &str),
        body: &str,
    ) -> AppResult<Response> {
        write(
            State(state.clone()),
            Extension(identity.clone()),
            ClientIp::NONE,
            Path((path.0.into(), path.1.into(), path.2.into())),
            Json(NoteRequest { body: body.into() }),
        )
        .await
    }

    async fn read_in(
        state: &AppState,
        identity: &Identity,
        scope: &str,
        key: &str,
        language: &str,
    ) -> AppResult<Json<Note>> {
        read(
            State(state.clone()),
            Extension(identity.clone()),
            Path((scope.into(), key.into())),
            Query(ReadQuery {
                language: Some(language.into()),
            }),
        )
        .await
    }

    #[tokio::test]
    async fn a_note_is_written_by_an_editor_and_read_by_anybody_in_their_language() {
        let state = super::super::testing::server().await;
        let editor = Identity::Anonymous;

        // Any name of the genre files it under the slug.
        let answer = written(
            &state,
            &editor,
            ("genre", "Drame", "fr-FR"),
            "Un *drame*.\r\n\r\n\r\nVoir [les films](/genre/drama).",
        )
        .await
        .expect("written");
        assert_eq!(answer.status(), StatusCode::OK);
        written(&state, &editor, ("genre", "drama", "en"), "A **drama**.")
            .await
            .expect("written");

        let french = read_in(&state, &Identity::Visitor, "genre", "Drama", "fr")
            .await
            .expect("read")
            .0;
        assert_eq!(french.key, "drama");
        assert_eq!(french.language, "fr");
        assert_eq!(
            french.body,
            "Un *drame*.\n\nVoir [les films](/genre/drama)."
        );
        assert_eq!(french.updated_by.as_deref(), Some("anonymous"));
        // A German reader is given the English one.
        let german = read_in(&state, &Identity::Visitor, "genre", "drama", "de")
            .await
            .expect("read")
            .0;
        assert_eq!(german.body, "A **drama**.");

        // Recorded in the journal.
        let entries = repo::audit::list(
            &state.db,
            &repo::audit::Query {
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            entries.iter().any(|e| e.action == "editorial.updated"
                && e.target.as_deref() == Some("genre:drama@fr")),
            "{entries:?}"
        );

        // Nobody else writes one.
        assert!(matches!(
            written(
                &state,
                &Identity::Visitor,
                ("genre", "drama", "en"),
                "Mine."
            )
            .await,
            Err(AppError::Forbidden)
        ));

        // Empty, it is taken away; the French reader is given English then.
        let answer = written(&state, &editor, ("genre", "drama", "fr"), "  \n ")
            .await
            .expect("taken away");
        assert_eq!(answer.status(), StatusCode::NO_CONTENT);
        let french = read_in(&state, &Identity::Visitor, "genre", "drama", "fr")
            .await
            .expect("read")
            .0;
        assert_eq!(french.language, "en");
    }

    #[tokio::test]
    async fn html_a_link_elsewhere_and_a_key_that_is_not_one_are_refused() {
        let state = super::super::testing::server().await;
        let editor = Identity::Anonymous;
        for (path, body) in [
            (("genre", "drama", "en"), "<b>bold</b>"),
            (("genre", "drama", "en"), "[out](https://example.com)"),
            (("year", "19999", "en"), "A year."),
            (("year", "nineteen", "en"), "A year."),
            (("person", "x", "en"), "Nobody."),
            (("genre", "drama", "not a language"), "A drama."),
        ] {
            assert!(
                matches!(
                    written(&state, &editor, path, body).await,
                    Err(AppError::BadRequest(_))
                ),
                "{path:?} {body}"
            );
        }
        // A work that is not in the catalogue is not written about.
        assert!(matches!(
            written(
                &state,
                &editor,
                ("work", "0192f3a1-0000-7000-8000-000000000000", "en"),
                "Gone."
            )
            .await,
            Err(AppError::NotFound)
        ));
        // A decade by any of its years.
        written(&state, &editor, ("decade", "1994", "en"), "The nineties.")
            .await
            .expect("written");
        let note = read_in(&state, &Identity::Visitor, "decade", "1990", "en")
            .await
            .expect("read")
            .0;
        assert_eq!(
            (note.key.as_str(), note.body.as_str()),
            ("1990", "The nineties.")
        );
    }

    #[tokio::test]
    async fn a_work_s_note_is_kept_from_a_reader_the_work_is_kept_from() {
        let state = super::super::testing::server().await;
        let mut work = crate::domain::MediaItem::empty(crate::domain::MediaKind::Movie);
        work.title = "Switched off".into();
        work.slug = "switched-off".into();
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &work,
                replace_children: true,
            },
        )
        .await
        .unwrap();
        written(
            &state,
            &Identity::Anonymous,
            ("work", &work.id, "en"),
            "Today's pick.",
        )
        .await
        .expect("written");
        assert_eq!(
            read_in(&state, &Identity::Visitor, "work", &work.id, "en")
                .await
                .expect("read")
                .0
                .body,
            "Today's pick."
        );

        repo::item::set_enabled(&state.db, &work.id, false)
            .await
            .unwrap();
        state
            .caches
            .items
            .invalidate(&format!("item:{}", work.id))
            .await;
        assert!(matches!(
            read_in(&state, &Identity::Visitor, "work", &work.id, "en").await,
            Err(AppError::NotFound)
        ));
    }
}
