//! What there is to do: the gaps in the catalogue, the keys about to lapse, the
//! media given up on.
//!
//! The dashboard's list of errands. Each figure over the works is how many the
//! catalogue's own filter lists — `?missing=poster|overview|imdb|refresh`, and
//! `?refreshFailed=true` — counted through that filter's own definition and
//! over the same works, those switched off among them, so that following a
//! line of the list lands on exactly that many, and a figure of nought is one
//! the dashboard says nothing about. The locks a source moved under are
//! counted by the condition `?missing=divergingLock` is to list them by
//! (`repo::change::DIVERGING_LOCK`), and the works that look like another
//! one by `?missing=duplicates`'s, once the duplicates are looked for again.

use axum::{Extension, Json, extract::State};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    auth::Identity,
    db::{
        Db, RowExt,
        repo::{self, item::Missing},
        to_rfc3339,
    },
    error::AppResult,
    service::language,
    state::AppState,
};

const TAG: &str = super::meta::TAG;

/// How far ahead a key's end is worth an errand: long enough to find out who
/// still uses it and to issue its successor before a client starts being
/// refused.
const KEY_HORIZON_DAYS: i64 = 14;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(todo))
}

/// What needs somebody's attention, counted.
#[derive(Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Todo {
    /// Works whose last refresh failed.
    pub refresh_failed: i64,
    /// Works with no poster: no image of that kind of their own, and none
    /// chosen by hand to lead.
    pub no_poster: i64,
    /// Works with no overview in the language they are stored in — none given,
    /// or one cleared by hand; one written by hand, in that language or in
    /// every one, is an overview.
    pub no_overview: i64,
    /// Works with no IMDb identifier.
    pub no_imdb: i64,
    /// Works never refreshed that have an identifier elsewhere to refresh by.
    /// A work entered by hand is never refreshed, and is not one of these.
    pub never_refreshed: i64,
    /// Keys, still switched on, that stop working within `keyHorizonDays`.
    pub expiring_keys: i64,
    /// How many days ahead a key's end is counted.
    pub key_horizon_days: i64,
    /// Pictures and themes every try at fetching failed for.
    pub failed_media: i64,
    /// Whether the newest archive of the database is older than twice the
    /// schedule's interval, or none was ever made — while the schedule is
    /// on.
    pub backup_overdue: bool,
    /// When the newest archive was made; `null` when none was.
    #[schema(required)]
    pub last_backup_at: Option<String>,
    /// Works with a lock a source moved under since it was set, and which
    /// says something else than the lock now: worth a look, to keep the
    /// lock or to take what the source says.
    pub diverging_locks: i64,
    /// Works that look like another one — under one identifier elsewhere,
    /// under one title within a year, or a film and a series under one IMDb
    /// id — that nobody said are not: each pair is two of them.
    pub duplicates: i64,
}

/// How many works the catalogue lists under one gap: the filter's own count,
/// through the filter's own definition — a lock in another language than the
/// stored one does not count, a work entered by hand is not asked to be
/// refreshed — so that a figure and the list it leads to cannot disagree.
/// Works switched off are counted, as the administration's list shows them;
/// adult titles as the reader's policy has them.
async fn lacking(state: &AppState, include_adult: bool, gap: Missing) -> anyhow::Result<i64> {
    repo::item::count_matching(
        &state.db,
        &repo::item::Query {
            missing: vec![gap],
            stored_language: Some(language::stored(state)),
            include_disabled: true,
            include_adult,
            ..Default::default()
        },
    )
    .await
}

/// Keys still switched on that stop within the horizon, and have not yet.
/// Their expiry is stored in RFC 3339 and UTC, which sorts as time does.
async fn expiring_keys(db: &Db, horizon_days: i64) -> anyhow::Result<i64> {
    let now = chrono::Utc::now();
    let row = sqlx::query(db.sql(
        "SELECT COUNT(*) AS n FROM api_client
         WHERE is_enabled = 1 AND expires_at IS NOT NULL AND expires_at > ? AND expires_at <= ?",
    ))
    .bind(to_rfc3339(now))
    .bind(to_rfc3339(now + chrono::Duration::days(horizon_days)))
    .fetch_one(db.pool())
    .await?;

    Ok(row.big("n")?)
}

/// What there is to do, for the administrator's dashboard.
///
/// Each figure over the works is what the catalogue lists under the matching
/// filter — switched-off works counted, as the administration's list shows
/// them, adult titles as the server's policy has them — and the dashboard
/// shows only the ones above nought.
#[utoipa::path(
    get, path = "/admin/todo", tag = TAG,
    operation_id = "todo_todo",
    responses(
        (status = 200, body = Todo),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn todo(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Todo>> {
    identity.require_admin()?;

    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
    Ok(Json(counted(&state, adult).await?))
}

async fn counted(state: &AppState, include_adult: bool) -> anyhow::Result<Todo> {
    let db = &state.db;
    // Looked for again first: the figure, and the list it leads to, are of
    // the catalogue as it stands now.
    crate::service::merge::look_again(state).await?;

    Ok(Todo {
        // The list's own count, rather than another way of reaching it.
        refresh_failed: repo::item::count_matching(
            db,
            &repo::item::Query {
                refresh_failed: true,
                include_disabled: true,
                include_adult,
                ..Default::default()
            },
        )
        .await?,
        no_poster: lacking(state, include_adult, Missing::Poster).await?,
        no_overview: lacking(state, include_adult, Missing::Overview).await?,
        no_imdb: lacking(state, include_adult, Missing::Imdb).await?,
        never_refreshed: lacking(state, include_adult, Missing::Refresh).await?,
        expiring_keys: expiring_keys(db, KEY_HORIZON_DAYS).await?,
        key_horizon_days: KEY_HORIZON_DAYS,
        failed_media: repo::asset::counts(db).await?.failed,
        backup_overdue: crate::jobs::backup::overdue(state).await?,
        last_backup_at: state.backups.newest_at().await?,
        diverging_locks: repo::change::count_diverging(db, include_adult).await?,
        duplicates: lacking(state, include_adult, Missing::Duplicates).await?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{CoverType, Image, MediaItem, MediaKind, fields::Scope},
        error::AppError,
    };
    use serde_json::json;

    /// A work, stored; `shape` says what to leave out of it.
    async fn store(state: &AppState, title: &str, shape: impl FnOnce(&mut MediaItem)) -> String {
        let mut work = MediaItem::empty(MediaKind::Series);
        work.title = title.into();
        work.slug = crate::domain::make_slug(title, None);
        // Whole, to begin with: a poster, a synopsis, an IMDb identifier —
        // one of its own, since two works cannot go by the same.
        static NEXT_IMDB: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        let imdb = NEXT_IMDB.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        work.overview = Some("A synopsis.".into());
        work.external_ids.imdb = Some(format!("tt{imdb:07}"));
        work.images.push(poster("https://img.example/p.jpg"));
        shape(&mut work);
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &work,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        // Asked of its sources once, as a work that came from them has been.
        repo::item::mark_refreshed(&state.db, &work.id, None, None)
            .await
            .expect("marked");
        work.id
    }

    fn poster(url: &str) -> Image {
        Image {
            id: crate::db::new_id(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: url.into(),
            language: None,
            sort_order: 0,
            source: Some("tmdb".into()),
            is_manual: false,
        }
    }

    async fn asked(state: &AppState, adult: bool) -> Todo {
        counted(state, adult).await.expect("counted")
    }

    #[tokio::test]
    async fn a_complete_catalogue_has_nothing_to_do() {
        let state = super::super::testing::server().await;
        store(&state, "Whole", |_| {}).await;

        assert_eq!(
            asked(&state, false).await,
            Todo {
                refresh_failed: 0,
                no_poster: 0,
                no_overview: 0,
                no_imdb: 0,
                never_refreshed: 0,
                expiring_keys: 0,
                key_horizon_days: KEY_HORIZON_DAYS,
                failed_media: 0,
                // Never backed up, the schedule on: one thing the catalogue
                // does not settle.
                backup_overdue: true,
                last_backup_at: None,
                diverging_locks: 0,
                duplicates: 0,
            }
        );
    }

    #[tokio::test]
    async fn each_gap_is_counted_by_the_definition_its_filter_lists_by() {
        let state = super::super::testing::server().await;
        store(&state, "Whole", |_| {}).await;
        store(&state, "No poster", |w| w.images.clear()).await;
        // A season's poster is not the work's.
        store(&state, "Only a season's", |w| {
            w.images = vec![Image {
                season_number: Some(1),
                ..poster("https://img.example/s1.jpg")
            }];
        })
        .await;
        store(&state, "Empty synopsis", |w| w.overview = Some("  ".into())).await;
        store(&state, "No synopsis", |w| w.overview = None).await;
        store(&state, "No IMDb", |w| w.external_ids.imdb = None).await;
        // Nothing to refresh by, so nothing to say of it.
        let by_hand = store(&state, "By hand", |w| {
            w.external_ids.imdb = None;
            w.is_manual = true;
        })
        .await;
        // Entered by hand, and given an identifier: still never refreshed, and
        // still not asked to be.
        let kept = store(&state, "By hand, with an id", |w| w.is_manual = true).await;
        let never = store(&state, "Never refreshed", |_| {}).await;
        sqlx::query(
            state
                .db
                .sql("UPDATE media_item SET refreshed_at = NULL WHERE id IN (?, ?, ?)"),
        )
        .bind(&never)
        .bind(&by_hand)
        .bind(&kept)
        .execute(state.db.pool())
        .await
        .unwrap();
        let failing = store(&state, "Failing", |_| {}).await;
        repo::item::mark_refreshed(&state.db, &failing, None, Some("no provider answered"))
            .await
            .unwrap();

        let todo = asked(&state, false).await;
        assert_eq!(todo.no_poster, 2, "{todo:?}");
        assert_eq!(todo.no_overview, 2, "{todo:?}");
        // "No IMDb" and "By hand", which has none either.
        assert_eq!(todo.no_imdb, 2, "{todo:?}");
        // "By hand" has no identifier at all, and "By hand, with an id" is never
        // refreshed: only the one that came from its sources is asked for.
        assert_eq!(todo.never_refreshed, 1, "{todo:?}");
        assert_eq!(todo.refresh_failed, 1, "{todo:?}");
    }

    #[tokio::test]
    async fn a_poster_or_a_synopsis_put_in_by_hand_is_no_longer_missing() {
        let state = super::super::testing::server().await;
        let bare = store(&state, "Bare", |w| {
            w.images.clear();
            w.overview = None;
        })
        .await;
        assert_eq!(asked(&state, false).await.no_poster, 1);
        assert_eq!(asked(&state, false).await.no_overview, 1);

        // The poster a person chose to lead with, and a synopsis they wrote:
        // locks the refresh never undoes, which the row does not show.
        repo::override_field::set(
            &state.db,
            &bare,
            Scope::Item,
            "primaryPoster",
            Some(&json!("https://img.example/chosen.jpg")),
            None,
        )
        .await
        .unwrap();
        repo::override_field::set(
            &state.db,
            &bare,
            Scope::Item,
            "overview",
            Some(&json!("Written here.")),
            None,
        )
        .await
        .unwrap();
        let todo = asked(&state, false).await;
        assert_eq!((todo.no_poster, todo.no_overview), (0, 0), "{todo:?}");

        // A synopsis cleared by hand is an empty one, still.
        repo::override_field::set(
            &state.db,
            &bare,
            Scope::Item,
            "overview",
            Some(&json!("")),
            None,
        )
        .await
        .unwrap();
        assert_eq!(asked(&state, false).await.no_overview, 1);
    }

    #[tokio::test]
    async fn a_work_switched_off_or_for_adults_is_counted_as_the_list_counts_it() {
        let state = super::super::testing::server().await;
        store(&state, "Shown", |w| w.images.clear()).await;
        store(&state, "Switched off", |w| {
            w.images.clear();
            w.is_enabled = false;
        })
        .await;
        store(&state, "For adults", |w| {
            w.images.clear();
            w.is_adult = true;
        })
        .await;

        // The work switched off is counted, as the administration's list —
        // which the line leads to — shows it.
        assert_eq!(asked(&state, false).await.no_poster, 2);
        // Adult titles shown to this reader: theirs are counted too.
        assert_eq!(asked(&state, true).await.no_poster, 3);
    }

    #[tokio::test]
    async fn every_figure_counts_the_works_the_list_it_leads_to_shows() {
        let state = super::super::testing::server().await;
        let off = |w: &mut MediaItem| w.is_enabled = false;
        store(&state, "Off, no poster", |w| {
            off(w);
            w.images.clear();
        })
        .await;
        store(&state, "Off, no synopsis", |w| {
            off(w);
            w.overview = None;
        })
        .await;
        store(&state, "Off, no IMDb", |w| {
            off(w);
            w.external_ids.imdb = None;
        })
        .await;
        let never = store(&state, "Off, never refreshed", off).await;
        sqlx::query(
            state
                .db
                .sql("UPDATE media_item SET refreshed_at = NULL WHERE id = ?"),
        )
        .bind(&never)
        .execute(state.db.pool())
        .await
        .unwrap();
        let failing = store(&state, "Off, failing", off).await;
        repo::item::mark_refreshed(&state.db, &failing, None, Some("no provider answered"))
            .await
            .unwrap();

        let todo = asked(&state, false).await;
        // The register the dashboard leads to, as the page asks for it.
        let listed = |missing: &str, failed: bool| {
            let state = state.clone();
            let missing = missing.to_string();
            async move {
                let query = super::super::items::to_query(
                    &state,
                    &Identity::Anonymous,
                    serde_urlencoded::from_str(&format!(
                        "includeDisabled=true&missing={missing}&refreshFailed={failed}"
                    ))
                    .expect("a query"),
                )
                .expect("checked");
                repo::item::count_matching(&state.db, &query)
                    .await
                    .expect("counted")
            }
        };
        assert_eq!(todo.no_poster, listed("poster", false).await, "{todo:?}");
        assert_eq!(
            todo.no_overview,
            listed("overview", false).await,
            "{todo:?}"
        );
        assert_eq!(todo.no_imdb, listed("imdb", false).await, "{todo:?}");
        assert_eq!(
            todo.never_refreshed,
            listed("refresh", false).await,
            "{todo:?}"
        );
        assert_eq!(todo.refresh_failed, listed("", true).await, "{todo:?}");
        assert_eq!(
            (
                todo.no_poster,
                todo.no_overview,
                todo.no_imdb,
                todo.never_refreshed,
                todo.refresh_failed
            ),
            (1, 1, 1, 1, 1),
            "{todo:?}"
        );
    }

    #[tokio::test]
    async fn a_synopsis_locked_in_the_stored_language_is_no_longer_missing() {
        let state = super::super::testing::server().await;
        let stored = language::stored(&state);
        let bare = store(&state, "Bare", |w| w.overview = None).await;
        assert_eq!(asked(&state, false).await.no_overview, 1);

        // Written for the readers of another language alone: still missing
        // for everybody else.
        let other = if stored == "deu" { "fra" } else { "deu" };
        repo::override_field::set_in(
            &state.db,
            &bare,
            "item",
            "overview",
            other,
            Some(&json!("Nur für sie.")),
            None,
        )
        .await
        .unwrap();
        assert_eq!(asked(&state, false).await.no_overview, 1);

        // Written in the language the works are kept in: the overview they
        // are read with.
        repo::override_field::set_in(
            &state.db,
            &bare,
            "item",
            "overview",
            &stored,
            Some(&json!("Written here, in the stored language.")),
            None,
        )
        .await
        .unwrap();
        assert_eq!(asked(&state, false).await.no_overview, 0);

        // Cleared in that language, it is an empty one again — whatever was
        // locked in every language, which it is laid over.
        repo::override_field::set_in(
            &state.db,
            &bare,
            "item",
            "overview",
            "",
            Some(&json!("In every language.")),
            None,
        )
        .await
        .unwrap();
        assert_eq!(asked(&state, false).await.no_overview, 0);
        repo::override_field::set_in(
            &state.db,
            &bare,
            "item",
            "overview",
            &stored,
            Some(&json!("")),
            None,
        )
        .await
        .unwrap();
        assert_eq!(asked(&state, false).await.no_overview, 1);
    }

    #[tokio::test]
    async fn a_lock_its_source_moved_under_is_counted_once_a_work_as_the_reader_sees_them() {
        use crate::db::repo::change::{Cause, NewChange};

        let state = super::super::testing::server().await;
        let moved_under = |title: &'static str, adult: bool| {
            let state = state.clone();
            async move {
                let id = store(&state, title, |w| w.is_adult = adult).await;
                for field in ["overview", "status"] {
                    repo::override_field::set(
                        &state.db,
                        &id,
                        Scope::Item,
                        field,
                        Some(&json!("Mine.")),
                        None,
                    )
                    .await
                    .unwrap();
                }
                tokio::time::sleep(std::time::Duration::from_millis(3)).await;
                let changes: Vec<NewChange> = ["overview", "status"]
                    .into_iter()
                    .map(|field| NewChange {
                        cause: Cause::Refresh,
                        scope: "item".into(),
                        field: field.into(),
                        language: String::new(),
                        before: Some(json!("Old.")),
                        after: Some(json!("New.")),
                        provider: Some("tmdb".into()),
                        was_locked: true,
                    })
                    .collect();
                repo::change::record(&state.db, &id, None, &changes)
                    .await
                    .unwrap();
            }
        };

        // Two locks of one work: one work to look at.
        moved_under("Moved under", false).await;
        moved_under("For adults", true).await;
        assert_eq!(asked(&state, false).await.diverging_locks, 1);
        assert_eq!(asked(&state, true).await.diverging_locks, 2);
    }

    #[tokio::test]
    async fn the_works_that_look_like_another_are_counted_as_the_register_lists_them() {
        let state = super::super::testing::server().await;
        store(&state, "Solaris", |w| w.year = Some(1972)).await;
        store(&state, "Solaris", |w| {
            w.year = Some(1972);
            w.slug = "solaris-again".into();
        })
        .await;
        store(&state, "Stalker", |w| w.year = Some(1979)).await;
        // A pair for adults is counted for whoever is shown adult works.
        store(&state, "Nymph", |w| {
            w.year = Some(2013);
            w.is_adult = true;
        })
        .await;
        store(&state, "Nymph", |w| {
            w.year = Some(2014);
            w.slug = "nymph-2014".into();
            w.is_adult = true;
        })
        .await;

        // Looked for as the figure is read: nothing was looked for before.
        let todo = asked(&state, false).await;
        assert_eq!(todo.duplicates, 2, "{todo:?}");
        assert_eq!(asked(&state, true).await.duplicates, 4);

        let query = super::super::items::to_query(
            &state,
            &Identity::Anonymous,
            serde_urlencoded::from_str("includeDisabled=true&missing=duplicates").expect("a query"),
        )
        .expect("checked");
        assert_eq!(
            repo::item::count_matching(&state.db, &query)
                .await
                .expect("counted"),
            todo.duplicates
        );
    }

    #[tokio::test]
    async fn a_key_is_counted_while_it_is_enabled_and_about_to_lapse() {
        let state = super::super::testing::server().await;
        let in_days = |days: i64| to_rfc3339(chrono::Utc::now() + chrono::Duration::days(days));
        let make = |name: &'static str, expires: Option<String>| {
            let state = state.clone();
            async move {
                repo::client::create(
                    &state.db,
                    repo::client::NewClient {
                        name,
                        key_prefix: "ams_test",
                        key_hash: name,
                        scopes: &["read".to_string()],
                        expires_at: expires.as_deref(),
                        note: None,
                        owner_id: None,
                    },
                )
                .await
                .expect("a key")
            }
        };

        make("Soon", Some(in_days(3))).await;
        make("The day", Some(in_days(13))).await;
        let paused = make("Paused", Some(in_days(5))).await;
        repo::client::set_enabled(&state.db, &paused.id, false)
            .await
            .unwrap();
        make("Later", Some(in_days(40))).await;
        make("Already over", Some(in_days(-2))).await;
        make("Never", None).await;

        assert_eq!(asked(&state, false).await.expiring_keys, 2);
    }

    #[tokio::test]
    async fn the_media_given_up_on_are_counted_and_those_still_to_come_are_not() {
        let state = super::super::testing::server().await;
        let wanted = |origin| repo::asset::Wanted {
            origin,
            kind: repo::asset::Kind::Image,
            wanted_by: None,
        };
        repo::asset::enqueue(
            &state.db,
            &[
                wanted("https://img.example/gone.jpg"),
                wanted("https://img.example/later.jpg"),
                wanted("https://img.example/fine.jpg"),
            ],
        )
        .await
        .unwrap();
        let gone = repo::asset::by_origin(&state.db, "https://img.example/gone.jpg")
            .await
            .unwrap()
            .expect("known");
        // No next try: given up on.
        repo::asset::mark_failed(&state.db, &gone.id, "404", None, false)
            .await
            .unwrap();
        let later = repo::asset::by_origin(&state.db, "https://img.example/later.jpg")
            .await
            .unwrap()
            .expect("known");
        // Put off, to be tried again: not an errand yet.
        repo::asset::mark_failed(
            &state.db,
            &later.id,
            "429",
            Some("2999-01-01T00:00:00Z"),
            false,
        )
        .await
        .unwrap();

        assert_eq!(asked(&state, false).await.failed_media, 1);
    }

    #[tokio::test]
    async fn only_an_administrator_is_told() {
        let state = super::super::testing::server().await;

        for refused in [Identity::Visitor, Identity::Network(None)] {
            let answer = todo(State(state.clone()), Extension(refused)).await;
            assert!(matches!(answer, Err(AppError::Forbidden)), "{answer:?}");
        }
        let Json(told) = todo(State(state), Extension(Identity::Anonymous))
            .await
            .expect("answered");
        assert_eq!(told.refresh_failed, 0);
        // Said as the interface reads it, in camel case.
        let shape = serde_json::to_value(&told).unwrap();
        assert!(
            shape.get("noPoster").is_some() && shape.get("keyHorizonDays").is_some(),
            "{shape}"
        );
    }
}
