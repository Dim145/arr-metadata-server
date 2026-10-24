//! The search on a small catalogue, on whichever engine the tests run on:
//! what is found, in what order, and that the index follows the writes.

use super::*;
use crate::{
    db::{
        repo::{
            self,
            item::{ItemWrite, Query, Sort, upsert},
        },
        testing::db,
    },
    domain::{AlternativeTitle, Credit, CreditType, Episode, MediaItem, MediaKind},
};

/// A work, stored with its children.
async fn store(
    db: &Db,
    kind: MediaKind,
    title: &str,
    adjust: impl FnOnce(&mut MediaItem),
) -> MediaItem {
    let mut item = MediaItem::empty(kind);
    item.id = crate::db::new_id();
    item.title = title.to_string();
    adjust(&mut item);
    item.slug = crate::domain::make_slug(&item.title, item.year);
    upsert(
        db,
        ItemWrite {
            item: &item,
            replace_children: true,
        },
    )
    .await
    .expect("stored");
    item
}

fn credit(name: &str, tmdb_id: Option<i64>, kind: CreditType) -> Credit {
    Credit {
        id: String::new(),
        credit_type: kind,
        person_name: name.into(),
        character_name: None,
        image: None,
        tmdb_person_id: tmdb_id,
        credit_tmdb_id: None,
        sort_order: 0,
        is_manual: false,
    }
}

fn episode(season: i32, number: i32, title: &str) -> Episode {
    Episode {
        id: String::new(),
        season_number: season,
        episode_number: number,
        absolute_episode_number: None,
        aired_after_season_number: None,
        aired_before_season_number: None,
        aired_before_episode_number: None,
        title: title.into(),
        overview: None,
        air_date: Some("2013-09-15".into()),
        air_date_utc: None,
        runtime: Some(47),
        finale_type: None,
        image: None,
        tvdb_id: None,
        tmdb_id: None,
        rating: None,
        is_manual: false,
    }
}

/// The catalogue the ranking is pinned on: a few names that share words.
async fn catalogue(db: &Db) {
    store(db, MediaKind::Series, "Breaking Bad", |i| {
        i.year = Some(2008);
        i.popularity = Some(300.0);
        i.runtime = Some(47);
        i.production_countries = vec!["US".into()];
        i.credits = vec![credit("Bryan Cranston", Some(17419), CreditType::Actor)];
        i.episodes = vec![
            episode(5, 14, "Ozymandias"),
            episode(5, 16, "Felina"),
            episode(1, 1, "Episode 1"),
        ];
    })
    .await;
    store(db, MediaKind::Movie, "Breaking Dawn", |i| {
        i.year = Some(2011);
        i.popularity = Some(120.0);
        i.runtime = Some(117);
    })
    .await;
    store(db, MediaKind::Series, "Bad Sisters", |i| {
        i.year = Some(2022);
        i.popularity = Some(80.0);
    })
    .await;
    store(db, MediaKind::Movie, "Paris, Texas", |i| {
        i.year = Some(1984);
        i.popularity = Some(15.0);
        i.runtime = Some(147);
        i.production_countries = vec!["DE".into(), "FR".into()];
        i.credits = vec![credit("Wim Wenders", Some(2725), CreditType::Director)];
    })
    .await;
    store(db, MediaKind::Movie, "Wings of Desire", |i| {
        i.year = Some(1987);
        i.popularity = Some(14.0);
        i.runtime = Some(128);
        i.original_title = Some("Der Himmel über Berlin".into());
        i.original_language = Some("de".into());
        i.production_countries = vec!["DE".into()];
        i.credits = vec![credit("Wim Wenders", Some(2725), CreditType::Director)];
        i.alternative_titles = vec![AlternativeTitle {
            id: String::new(),
            title: "Les Ailes du désir".into(),
            title_type: None,
            language: Some("fr".into()),
            is_manual: false,
        }];
    })
    .await;
    store(db, MediaKind::Series, "The Wenders Family", |i| {
        i.year = Some(2001);
        i.popularity = Some(2.0);
    })
    .await;
}

/// The titles a list search answers with, in its order.
async fn listed(db: &Db, term: &str) -> Vec<String> {
    repo::item::search(
        db,
        &Query {
            term: Some(term.into()),
            sort: Sort::Relevance,
            limit: 20,
            ..Default::default()
        },
    )
    .await
    .expect("searched")
    .into_iter()
    .map(|item| item.title)
    .collect()
}

fn scope(kind: Kind) -> Scope {
    Scope {
        kind,
        media_kind: None,
        include_disabled: false,
        include_adult: false,
        limit: 50,
        forgive: true,
    }
}

#[tokio::test]
async fn a_misspelt_title_finds_the_work_it_misspells_first() {
    let db = db().await;
    catalogue(&db).await;

    let found = listed(&db, "breking bad").await;
    assert_eq!(
        found.first().map(String::as_str),
        Some("Breaking Bad"),
        "{found:?}"
    );
    assert!(!found.contains(&"Bad Sisters".to_string()), "{found:?}");

    // Spelt right, it is first by its letters, before the better-known
    // work it shares a word with.
    let found = listed(&db, "breaking").await;
    assert_eq!(found, ["Breaking Bad", "Breaking Dawn"]);
}

#[tokio::test]
async fn a_glance_at_the_letters_forgives_what_the_whole_look_forgives() {
    let db = db().await;
    catalogue(&db).await;
    let works = scope(Kind::Work);
    let ids = |rows: Vec<Found>| rows.into_iter().map(|row| row.id).collect::<Vec<_>>();

    for typed in ["breking bad", "wendrs", "paris texsa", "aris", "breaking"] {
        let term = Term::parse(typed);
        let looked = ids(find(&db, &term, &works).await.expect("looked").resembling);
        let glanced = ids(forgiven(&db, &term, &works).await.expect("glanced"));
        assert_eq!(glanced, looked, "{typed}");
        // A glance of one row stops short of every name: the whole look
        // settles it then, and says the same.
        let short = ids(glancing(&db, &term, &works, 1).await.expect("glanced"));
        assert_eq!(short, looked, "{typed}, glancing at one row");
    }
    assert!(
        !ids(forgiven(&db, &Term::parse("breking bad"), &works)
            .await
            .expect("glanced"))
        .is_empty()
    );

    // Nothing for whoever forgives nothing, nor for an episode's title.
    let strict = Scope {
        forgive: false,
        ..works
    };
    for (typed, scope) in [("breking bad", strict), ("ozymandas", scope(Kind::Episode))] {
        let found = forgiven(&db, &Term::parse(typed), &scope)
            .await
            .expect("glanced");
        assert!(found.is_empty(), "{typed}: {found:?}");
    }
}

#[tokio::test]
async fn a_name_finds_the_person_and_a_work_by_their_name_after_them() {
    let db = db().await;
    catalogue(&db).await;

    let term = Term::parse("Wenders");
    let people = find(&db, &term, &scope(Kind::Person)).await.expect("found");
    let names: Vec<&str> = people.all().map(|row| row.text.as_str()).collect();
    assert_eq!(names, ["Wim Wenders"]);
    let row = people.all().next().expect("a row");
    let wenders = crate::db::repo::person::find(&db, "2725")
        .await
        .expect("read")
        .expect("a record");
    assert_eq!(row.ref_id, wenders.id, "the person's record");
    assert_eq!(row.popularity, Some(2.0), "credited on two works");

    // The works are found by their titles, not by who made them.
    assert_eq!(listed(&db, "wenders").await, ["The Wenders Family"]);

    // And misspelt, the person still.
    let people = find(&db, &Term::parse("wendres"), &scope(Kind::Person))
        .await
        .expect("found");
    assert_eq!(
        people
            .all()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>(),
        ["Wim Wenders"]
    );
}

#[tokio::test]
async fn a_work_is_found_by_its_other_titles_whatever_their_accents() {
    let db = db().await;
    catalogue(&db).await;

    for term in [
        "der himmel uber berlin",
        "Der Himmel über Berlin",
        "ailes du desir",
        "LES AILES DU DÉSIR",
    ] {
        assert_eq!(listed(&db, term).await, ["Wings of Desire"], "{term:?}");
    }
}

#[tokio::test]
async fn an_episode_is_found_by_its_title_and_not_by_its_number() {
    let db = db().await;
    catalogue(&db).await;

    let found = find(&db, &Term::parse("ozymandias"), &scope(Kind::Episode))
        .await
        .expect("found");
    let rows: Vec<&Found> = found.all().collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].text, "Ozymandias");
    assert_eq!(rows[0].year, Some(2013));

    let numbered = find(&db, &Term::parse("episode 1"), &scope(Kind::Episode))
        .await
        .expect("found");
    assert_eq!(
        numbered.all().count(),
        0,
        "a title that says nothing is not indexed"
    );
}

#[tokio::test]
async fn a_work_switched_off_is_found_by_nobody_but_those_who_ask() {
    let db = db().await;
    catalogue(&db).await;
    let hidden = store(&db, MediaKind::Movie, "Breaking Glass", |i| {
        i.popularity = Some(999.0);
    })
    .await;
    repo::item::set_enabled(&db, &hidden.id, false)
        .await
        .expect("switched off");

    assert!(
        !listed(&db, "breaking")
            .await
            .contains(&"Breaking Glass".to_string())
    );
    let all = find(
        &db,
        &Term::parse("breaking glass"),
        &Scope {
            include_disabled: true,
            ..scope(Kind::Work)
        },
    )
    .await
    .expect("found");
    assert!(
        all.by_letters
            .iter()
            .any(|row| row.text == "Breaking Glass"),
        "{all:?}"
    );
}

#[tokio::test]
async fn the_index_follows_a_work_renamed_deleted_and_its_credits_changed() {
    let db = db().await;
    let mut item = store(&db, MediaKind::Movie, "First Name", |i| {
        i.credits = vec![credit("Jane Doe", None, CreditType::Actor)];
    })
    .await;
    assert_eq!(listed(&db, "first name").await, ["First Name"]);

    // A refresh that renames it and changes its cast.
    item.title = "Second Name".into();
    item.credits = vec![credit("John Roe", Some(55), CreditType::Actor)];
    upsert(
        &db,
        ItemWrite {
            item: &item,
            replace_children: true,
        },
    )
    .await
    .expect("refreshed");
    assert!(listed(&db, "first name").await.is_empty());
    assert_eq!(listed(&db, "second name").await, ["Second Name"]);
    let people = |term: &str| {
        let db = db.clone();
        let term = Term::parse(term);
        async move {
            find(&db, &term, &scope(Kind::Person))
                .await
                .expect("found")
                .all()
                .map(|row| row.text.clone())
                .collect::<Vec<_>>()
        }
    };
    assert!(
        people("jane doe").await.is_empty(),
        "nobody credits her any more"
    );
    assert_eq!(people("john roe").await, ["John Roe"]);

    // An alternative title added by hand marks the work, and the next pass
    // of the catch-up writes it.
    repo::child::add_alternative_title(
        &db,
        &item.id,
        &AlternativeTitle {
            id: String::new(),
            title: "Третье имя".into(),
            title_type: None,
            language: Some("ru".into()),
            is_manual: true,
        },
    )
    .await
    .expect("added");
    assert_eq!(
        repo::search::stale(&db, 10).await.expect("read"),
        [item.id.clone()]
    );
    repo::search::catch_up(&db, 10).await.expect("caught up");
    assert_eq!(listed(&db, "третье имя").await, ["Second Name"]);
    assert!(repo::search::stale(&db, 10).await.expect("read").is_empty());

    repo::item::delete(&db, &item.id).await.expect("deleted");
    assert!(listed(&db, "second name").await.is_empty());
    assert!(people("john roe").await.is_empty());
    let standing = repo::search::standings(&db).await.expect("read");
    assert_eq!(
        (standing.works, standing.people, standing.episodes),
        (0, 0, 0),
        "nothing of it is left in the index"
    );
}

#[tokio::test]
async fn the_list_and_its_count_agree_on_a_misspelt_term() {
    let db = db().await;
    catalogue(&db).await;

    let (items, total) = repo::item::search_counted(
        &db,
        &Query {
            term: Some("breking bad".into()),
            sort: Sort::Relevance,
            limit: 10,
            ..Default::default()
        },
    )
    .await
    .expect("searched");
    assert_eq!(
        items.first().map(|i| i.title.as_str()),
        Some("Breaking Bad")
    );
    assert_eq!(total, items.len() as i64);

    // In another order, the same works.
    let (by_title, titled_total) = repo::item::search_counted(
        &db,
        &Query {
            term: Some("breking bad".into()),
            sort: Sort::Title,
            limit: 10,
            ..Default::default()
        },
    )
    .await
    .expect("searched");
    assert_eq!(titled_total, total);
    let mut ranked: Vec<String> = items.into_iter().map(|i| i.id).collect();
    let mut titled: Vec<String> = by_title.into_iter().map(|i| i.id).collect();
    ranked.sort();
    titled.sort();
    assert_eq!(ranked, titled);
}

#[tokio::test]
async fn a_page_past_the_ranked_works_goes_on_in_popularity_order() {
    let db = db().await;
    for n in 0..12 {
        store(&db, MediaKind::Movie, &format!("Star {n:02}"), |i| {
            i.popularity = Some(f64::from(n));
        })
        .await;
    }
    let page = |offset: i64| {
        let db = db.clone();
        async move {
            repo::item::search(
                &db,
                &Query {
                    term: Some("star".into()),
                    sort: Sort::Relevance,
                    limit: 5,
                    offset,
                    ..Default::default()
                },
            )
            .await
            .expect("searched")
            .into_iter()
            .map(|i| i.title)
            .collect::<Vec<_>>()
        }
    };
    let mut seen: Vec<String> = Vec::new();
    for offset in [0, 5, 10] {
        seen.extend(page(offset).await);
    }
    assert_eq!(seen.len(), 12, "{seen:?}");
    let mut each = seen.clone();
    each.sort();
    each.dedup();
    assert_eq!(each.len(), 12, "no work twice: {seen:?}");
    assert_eq!(
        seen.first().map(String::as_str),
        Some("Star 11"),
        "the best known first"
    );
}

#[tokio::test]
async fn the_further_filters_narrow_the_list() {
    let db = db().await;
    catalogue(&db).await;
    let titles = |q: Query| {
        let db = db.clone();
        async move {
            let mut found: Vec<String> = repo::item::search(&db, &Query { limit: 50, ..q })
                .await
                .expect("searched")
                .into_iter()
                .map(|i| i.title)
                .collect();
            found.sort();
            found
        }
    };

    assert_eq!(
        titles(Query {
            person: Some("2725".into()),
            ..Default::default()
        })
        .await,
        ["Paris, Texas", "Wings of Desire"]
    );
    assert_eq!(
        titles(Query {
            person: Some("WIM WENDERS".into()),
            ..Default::default()
        })
        .await,
        ["Paris, Texas", "Wings of Desire"],
        "a name whatever its case"
    );
    assert_eq!(
        titles(Query {
            runtime_min: Some(120),
            runtime_max: Some(140),
            ..Default::default()
        })
        .await,
        ["Wings of Desire"]
    );
    assert_eq!(
        titles(Query {
            country: Some("fr".into()),
            ..Default::default()
        })
        .await,
        ["Paris, Texas"]
    );

    let sorted = |sort: Sort| {
        let db = db.clone();
        async move {
            repo::item::search(
                &db,
                &Query {
                    sort,
                    limit: 3,
                    ..Default::default()
                },
            )
            .await
            .expect("searched")
            .into_iter()
            .map(|i| i.title)
            .collect::<Vec<_>>()
        }
    };
    assert_eq!(
        sorted(Sort::Runtime).await,
        ["Paris, Texas", "Wings of Desire", "Breaking Dawn"]
    );
    assert_eq!(
        sorted(Sort::Episodes).await.first().map(String::as_str),
        Some("Breaking Bad"),
        "the series with episodes first"
    );
}

#[tokio::test]
async fn a_rating_is_read_in_the_readers_country() {
    let db = db().await;
    let rated = store(&db, MediaKind::Movie, "Rated", |i| {
        i.content_rating = Some("R".into());
        i.content_rating_country = Some("US".into());
        i.releases = vec![crate::domain::CountryRelease {
            country: "FR".into(),
            release_type: None,
            date: None,
            certification: Some("12".into()),
            note: None,
            source: Some("tmdb".into()),
        }];
    })
    .await;
    store(&db, MediaKind::Movie, "Unrated", |_| {}).await;

    let rated_in = |region: Option<&str>, rating: &str| {
        let db = db.clone();
        let (region, rating) = (region.map(str::to_string), rating.to_string());
        async move {
            repo::item::search(
                &db,
                &Query {
                    certification: Some(rating),
                    region,
                    limit: 10,
                    ..Default::default()
                },
            )
            .await
            .expect("searched")
            .into_iter()
            .map(|i| i.id)
            .collect::<Vec<_>>()
        }
    };
    assert_eq!(
        rated_in(Some("FR"), "12").await,
        std::slice::from_ref(&rated.id)
    );
    assert_eq!(
        rated_in(Some("US"), "R").await,
        std::slice::from_ref(&rated.id)
    );
    assert!(rated_in(Some("FR"), "R").await.is_empty());
    assert_eq!(rated_in(None, "R").await, std::slice::from_ref(&rated.id));

    let facets = repo::item::certification_facets(
        &db,
        &Query {
            region: Some("FR".into()),
            ..Default::default()
        },
    )
    .await
    .expect("counted");
    assert_eq!(
        facets
            .iter()
            .map(|f| (f.value.as_str(), f.count))
            .collect::<Vec<_>>(),
        [("12", 1)]
    );
}

#[tokio::test]
async fn a_work_merged_into_another_is_found_by_nobody() {
    let db = db().await;
    let kept = store(&db, MediaKind::Series, "Twin Peaks", |i| {
        i.year = Some(1990);
        i.episodes = vec![episode(1, 1, "Pilot")];
    })
    .await;
    let merged = store(&db, MediaKind::Series, "Twin Peaks (copy)", |i| {
        i.year = Some(1990);
        i.episodes = vec![episode(1, 1, "Northwest Passage")];
    })
    .await;
    let rows = |id: String| {
        let db = db.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                db.sql("SELECT COUNT(*) FROM search_doc WHERE work_id = ?"),
            )
            .bind(id)
            .fetch_one(db.pool())
            .await
            .expect("counted")
        }
    };
    assert!(rows(merged.id.clone()).await > 0);

    // As a merge leaves it: switched off, naming the work it went into.
    sqlx::query(db.sql(
        "UPDATE media_item SET merged_into = ?, is_enabled = 0, search_stale = 1 WHERE id = ?",
    ))
    .bind(&kept.id)
    .bind(&merged.id)
    .execute(db.pool())
    .await
    .expect("merged");
    repo::search::catch_up(&db, 10).await.expect("caught up");

    assert_eq!(
        rows(merged.id.clone()).await,
        0,
        "its titles and its episodes'"
    );
    assert!(rows(kept.id.clone()).await > 0);
    let found = find(
        &db,
        &Term::parse("northwest passage"),
        &Scope {
            include_disabled: true,
            ..scope(Kind::Episode)
        },
    )
    .await
    .expect("found");
    assert_eq!(
        found.all().count(),
        0,
        "not even for those who see what is off"
    );
}
