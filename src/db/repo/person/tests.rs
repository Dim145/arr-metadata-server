//! The filing of credits under their people, the records kept of them, and
//! what is done to those by hand — on whichever engine the environment picks.

use super::*;
use crate::{
    db::{
        repo::item::{ItemWrite, upsert},
        testing,
    },
    domain::{Episode, MediaItem, MediaKind, person::EpisodeCredit},
};

/// A credit a source gives, under an id of its own.
fn credit(kind: CreditType, name: &str, tmdb: Option<i64>) -> Credit {
    Credit {
        id: new_id(),
        credit_type: kind,
        person_name: name.into(),
        character_name: None,
        image: None,
        tmdb_person_id: tmdb,
        credit_tmdb_id: None,
        sort_order: 0,
        is_manual: false,
    }
}

/// A work as its sources gave it, written as a refresh writes one.
async fn stored(db: &Db, title: &str, adjust: impl FnOnce(&mut MediaItem)) -> MediaItem {
    let mut item = MediaItem::empty(MediaKind::Movie);
    item.title = title.into();
    item.slug = crate::domain::make_slug(title, None);
    adjust(&mut item);
    write(db, &item).await;
    item
}

/// Write a work again, its sources' children replaced.
async fn write(db: &Db, item: &MediaItem) {
    upsert(
        db,
        ItemWrite {
            item,
            replace_children: true,
        },
    )
    .await
    .expect("written");
}

/// Whom a credit, by its id, is filed under.
async fn filed(db: &Db, credit: &str) -> Option<String> {
    sqlx::query_scalar(db.sql("SELECT person_id FROM media_credit WHERE id = ?"))
        .bind(credit)
        .fetch_one(db.pool())
        .await
        .expect("a credit")
}

/// Whom the credit of this name on this work is filed under.
async fn filed_by_name(db: &Db, work: &str, name: &str) -> Option<String> {
    sqlx::query_scalar(db.sql(
        "SELECT person_id FROM media_credit
          WHERE media_id = ? AND person_name = ? AND season_number IS NULL",
    ))
    .bind(work)
    .bind(name)
    .fetch_one(db.pool())
    .await
    .expect("a credit of that name")
}

async fn people(db: &Db) -> i64 {
    sqlx::query_scalar(db.sql("SELECT COUNT(*) FROM person WHERE merged_into IS NULL"))
        .fetch_one(db.pool())
        .await
        .expect("counted")
}

/// A credit as the version before kept it: its id, its work, the name, the
/// TMDB id, the portrait, and whether it was added by hand.
type Held<'a> = (&'a str, &'a str, &'a str, Option<i64>, Option<&'a str>, i64);

#[tokio::test]
async fn the_migration_files_every_credit_held_under_one_person_and_a_name_under_the_one_tmdb_knows()
 {
    // A database as the version before left it, its credits keyed as they
    // were: by a TMDB id, or by a name.
    let config = testing::database().await;
    let db = Db::connect(&config).await.expect("a database");
    db.migrate_to(40).await.expect("migrated to 0040");
    let at = "2026-10-01T00:00:00.000Z";
    for (id, title) in [("w1", "One"), ("w2", "Two"), ("w3", "Three")] {
        sqlx::query(db.sql(
            "INSERT INTO media_item (id, kind, slug, title, created_at, updated_at)
             VALUES (?, 'movie', ?, ?, ?, ?)",
        ))
        .bind(id)
        .bind(title.to_lowercase())
        .bind(title)
        .bind(at)
        .bind(at)
        .execute(db.pool())
        .await
        .expect("a work");
    }
    let credits: &[Held] = &[
        // id, work, name, TMDB id, portrait, by hand — written in this order
        (
            "c1",
            "w1",
            "Bryan Cranston",
            Some(17419),
            Some("https://i/old.jpg"),
            0,
        ),
        ("c2", "w2", "Bryan Cranston", Some(17419), None, 0),
        (
            "c3",
            "w3",
            "Bryan Lee Cranston",
            Some(17419),
            Some("https://i/new.jpg"),
            0,
        ),
        // By name alone, under a name somebody TMDB knows goes by.
        ("c4", "w3", "Bryan Lee Cranston", None, None, 1),
        // Two people TMDB knows by one name, and somebody by it alone.
        ("c5", "w1", "John Smith", Some(1), None, 0),
        ("c6", "w2", "John Smith", Some(2), None, 0),
        ("c7", "w3", "John Smith", None, None, 0),
        // Somebody known by their name alone, on two works.
        ("c8", "w1", "Jane Doe", None, None, 0),
        (
            "c9",
            "w2",
            " Jane Doe ",
            None,
            Some("https://i/jane.jpg"),
            0,
        ),
        // A credit that names nobody.
        ("c10", "w2", "  ", None, None, 0),
    ];
    for (at, (id, work, name, tmdb, image, manual)) in credits.iter().enumerate() {
        sqlx::query(db.sql(
            "INSERT INTO media_credit
                 (id, media_id, credit_type, person_name, image, tmdb_person_id, sort_order,
                  is_manual, created_at)
             VALUES (?, ?, 'actor', ?, ?, ?, 0, ?, ?)",
        ))
        .bind(*id)
        .bind(*work)
        .bind(*name)
        .bind(*image)
        .bind(*tmdb)
        .bind(*manual)
        .bind(format!("2026-10-01T00:00:{at:02}.000Z"))
        .execute(db.pool())
        .await
        .expect("a credit");
    }

    db.migrate().await.expect("migrated to the latest");

    // One record a TMDB id, under the name most of its credits give it, the
    // other spelling kept, the latest portrait.
    let cranston = filed(&db, "c1").await.expect("filed");
    assert_eq!(filed(&db, "c2").await.as_deref(), Some(cranston.as_str()));
    assert_eq!(filed(&db, "c3").await.as_deref(), Some(cranston.as_str()));
    let record = get(&db, &cranston).await.unwrap().expect("a record");
    assert_eq!(record.tmdb_id, Some(17419));
    assert_eq!(record.name, "Bryan Cranston");
    assert_eq!(record.image.as_deref(), Some("https://i/new.jpg"));
    assert_eq!(
        record.aliases,
        [Alias {
            name: "Bryan Lee Cranston".into(),
            source: "credit".into()
        }]
    );
    // The name alone is taken for the one person TMDB knows by it.
    assert_eq!(filed(&db, "c4").await.as_deref(), Some(cranston.as_str()));

    // Two people TMDB knows by one name: somebody by the name alone is
    // neither of them.
    let smiths = [
        filed(&db, "c5").await.expect("filed"),
        filed(&db, "c6").await.expect("filed"),
        filed(&db, "c7").await.expect("filed"),
    ];
    assert_ne!(smiths[0], smiths[1]);
    assert!(!smiths[..2].contains(&smiths[2]));
    let alone = get(&db, &smiths[2]).await.unwrap().expect("a record");
    assert_eq!((alone.tmdb_id, alone.name.as_str()), (None, "John Smith"));

    // One record for a name alone, however its credits space it.
    let jane = filed(&db, "c8").await.expect("filed");
    assert_eq!(filed(&db, "c9").await.as_deref(), Some(jane.as_str()));
    let record = get(&db, &jane).await.unwrap().expect("a record");
    assert_eq!(record.name, "Jane Doe");
    assert_eq!(record.image.as_deref(), Some("https://i/jane.jpg"));
    assert!(uuid::Uuid::parse_str(&record.id).is_ok(), "{}", record.id);

    assert_eq!(filed(&db, "c10").await, None, "nobody named");
    assert_eq!(people(&db).await, 5, "Cranston, three Smiths, Doe");

    // The search index's people were keys; every work is marked for them to
    // be written again under their records.
    let stale: i64 =
        sqlx::query_scalar(db.sql("SELECT COUNT(*) FROM media_item WHERE search_stale = 1"))
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(stale, 3);
    crate::db::repo::search::catch_up(&db, 10)
        .await
        .expect("caught up");
    let rows: Vec<String> = sqlx::query_scalar(db.sql(
        "SELECT ref_id FROM search_doc WHERE kind = 'person' AND norm = 'bryan lee cranston'",
    ))
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(rows, [cranston], "found by the other spelling too");
}

#[tokio::test]
async fn a_refresh_files_each_credit_under_its_person_once_and_keeps_where_it_was_put() {
    let db = testing::db().await;
    let mut item = stored(&db, "Paris, Texas", |i| {
        i.credits = vec![
            credit(CreditType::Director, "Wim Wenders", Some(2725)),
            credit(CreditType::Actor, "Harry Dean Stanton", None),
        ];
    })
    .await;
    let wenders = filed(&db, &item.credits[0].id).await.expect("filed");
    let stanton = filed(&db, &item.credits[1].id).await.expect("filed");
    let record = get(&db, &wenders).await.unwrap().expect("a record");
    assert_eq!(
        (record.tmdb_id, record.name.as_str()),
        (Some(2725), "Wim Wenders")
    );
    assert!(!record.is_manual);

    // Written again under new rows: the same people, nobody new.
    for credit in &mut item.credits {
        credit.id = new_id();
    }
    write(&db, &item).await;
    assert_eq!(filed(&db, &item.credits[0].id).await, Some(wenders.clone()));
    assert_eq!(filed(&db, &item.credits[1].id).await, Some(stanton.clone()));
    assert_eq!(people(&db).await, 2);

    // The actor split off, as a homonym would be: a refresh leaves him
    // where he was put.
    let (split, works) = split(&db, &stanton, &[item.credits[1].id.clone()], None)
        .await
        .unwrap()
        .expect("split");
    assert_eq!(works, [item.id.clone()]);
    for credit in &mut item.credits {
        credit.id = new_id();
    }
    write(&db, &item).await;
    assert_eq!(filed(&db, &item.credits[1].id).await, Some(split.clone()));
    let record = get(&db, &split).await.unwrap().expect("a record");
    assert!(record.is_manual, "made by hand");
    assert_eq!(record.name, "Harry Dean Stanton");
}

#[tokio::test]
async fn a_name_alone_is_the_one_person_tmdb_knows_by_it_and_nobody_when_two_are() {
    let db = testing::db().await;
    stored(&db, "Breaking Bad", |i| {
        i.credits = vec![
            credit(CreditType::Actor, "Bryan Cranston", Some(17419)),
            credit(CreditType::Actor, "John Smith", Some(1)),
        ];
    })
    .await;
    stored(&db, "Another", |i| {
        i.credits = vec![credit(CreditType::Actor, "John Smith", Some(2))];
    })
    .await;
    let named = stored(&db, "Malcolm", |i| {
        i.credits = vec![
            credit(CreditType::Actor, "Bryan Cranston", None),
            credit(CreditType::Actor, "John Smith", None),
            credit(CreditType::Writer, "John Smith", None),
        ];
    })
    .await;

    let cranston = filed_by_name(&db, &named.id, "Bryan Cranston")
        .await
        .expect("filed");
    let record = get(&db, &cranston).await.unwrap().expect("a record");
    assert_eq!(
        record.tmdb_id,
        Some(17419),
        "the one TMDB knows by the name"
    );

    let smith = filed(&db, &named.credits[1].id).await.expect("filed");
    let record = get(&db, &smith).await.unwrap().expect("a record");
    assert_eq!(record.tmdb_id, None, "two are known by it: neither");
    assert_eq!(
        filed(&db, &named.credits[2].id).await,
        Some(smith),
        "one record for the name on one work"
    );
}

#[tokio::test]
async fn thetvdb_s_id_files_its_people_and_completes_a_record_tmdb_gives() {
    let db = testing::db().await;
    // TheTVDB's alone first: a record by its id.
    let tvdb_only = stored(&db, "Anime", |i| {
        let mut actor = credit(CreditType::Actor, "Tanaka Mayumi", None);
        actor.image = Some("https://tvdb/p.jpg".into());
        i.people.by_credit.insert(
            actor.id.clone(),
            PersonRef {
                tvdb_id: Some(269404),
                ..PersonRef::default()
            },
        );
        i.credits = vec![actor];
    })
    .await;
    let tanaka = filed(&db, &tvdb_only.credits[0].id).await.expect("filed");
    let record = get(&db, &tanaka).await.unwrap().expect("a record");
    assert_eq!((record.tvdb_id, record.tmdb_id), (Some(269404), None));

    // TMDB names the same person, matched with TheTVDB's on another work:
    // the record is theirs, and is given TMDB's id and what TMDB says.
    let both = stored(&db, "Anime, again", |i| {
        let actor = credit(CreditType::Actor, "Mayumi Tanaka", Some(77));
        i.people.by_credit.insert(
            actor.id.clone(),
            PersonRef {
                tvdb_id: Some(269404),
                original_name: Some("田中真弓".into()),
                gender: Some(1),
                popularity: Some(12.5),
                ..PersonRef::default()
            },
        );
        i.credits = vec![actor];
    })
    .await;
    assert_eq!(
        filed(&db, &both.credits[0].id).await.as_deref(),
        Some(tanaka.as_str())
    );
    let record = get(&db, &tanaka).await.unwrap().expect("a record");
    assert_eq!(record.tmdb_id, Some(77));
    assert_eq!(record.original_name.as_deref(), Some("田中真弓"));
    assert_eq!(record.gender, Some(1));
    assert_eq!(record.popularity, Some(12.5));
    assert_eq!(record.image.as_deref(), Some("https://tvdb/p.jpg"));
    assert_eq!(
        record
            .aliases
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["Mayumi Tanaka"],
        "the spelling TMDB gives, as a name they go by"
    );

    // And a TMDB credit of somebody TheTVDB also names gives the record
    // TheTVDB's id where it has none and nobody else holds it.
    let named = stored(&db, "Film", |i| {
        let actor = credit(CreditType::Actor, "Bryan Cranston", Some(17419));
        i.people.by_credit.insert(
            actor.id.clone(),
            PersonRef {
                tvdb_id: Some(4242),
                ..PersonRef::default()
            },
        );
        i.credits = vec![actor];
    })
    .await;
    let cranston = filed(&db, &named.credits[0].id).await.expect("filed");
    let record = get(&db, &cranston).await.unwrap().expect("a record");
    assert_eq!((record.tmdb_id, record.tvdb_id), (Some(17419), Some(4242)));
}

/// A series with two episodes, which TMDB numbers one further than its own
/// list does, and a guest of the second.
fn series_with_a_guest() -> MediaItem {
    let mut item = MediaItem::empty(MediaKind::Series);
    item.title = "Guests".into();
    item.slug = "guests".into();
    item.episodes = (1..=2)
        .map(|n| {
            let mut episode = crate::db::repo::child::blank_episode(1, n);
            episode.is_manual = false;
            episode.title = format!("Part {n}");
            episode.air_date = Some(format!("2019-0{n}-01"));
            episode.tmdb_id = Some(100 + i64::from(n));
            episode
        })
        .collect::<Vec<Episode>>();
    item.credits = vec![credit(CreditType::Actor, "A Regular", Some(10))];
    let guest = credit(CreditType::Guest, "A Guest", Some(20));
    let director = credit(CreditType::Director, "A Director", Some(30));
    let nowhere = credit(CreditType::Guest, "Of No Episode Here", Some(40));
    item.people.episodes = vec![
        EpisodeCredit {
            season_number: 1,
            episode_number: 3,
            tmdb_episode_id: Some(102),
            credit: guest,
        },
        EpisodeCredit {
            season_number: 1,
            episode_number: 3,
            tmdb_episode_id: Some(102),
            credit: director,
        },
        EpisodeCredit {
            season_number: 1,
            episode_number: 9,
            tmdb_episode_id: Some(909),
            credit: nowhere,
        },
    ];
    item
}

#[tokio::test]
async fn a_guest_is_credited_on_the_episode_of_the_work_tmdb_names_and_not_on_the_work() {
    let db = testing::db().await;
    let item = series_with_a_guest();
    write(&db, &item).await;

    // The work's cast is what it was.
    let mut read = crate::db::repo::item::get(&db, &item.id)
        .await
        .unwrap()
        .expect("held");
    crate::db::repo::item::load_children(&db, &mut read)
        .await
        .unwrap();
    let cast: Vec<&str> = read
        .credits
        .iter()
        .map(|c| c.person_name.as_str())
        .collect();
    assert_eq!(cast, ["A Regular"]);
    assert!(read.people.episodes.is_empty(), "never read back with it");
    let regular = &read.people.by_credit[&read.credits[0].id];
    assert_eq!(regular.tmdb_id, Some(10));
    assert!(regular.person_id.is_some());

    // The guest is on the work's second episode — TMDB's 1x3 — with its
    // title and its day; the episode the work does not have, nowhere.
    let rows: Vec<(String, Option<i32>, Option<i32>)> = sqlx::query(db.sql(
        "SELECT person_name, season_number, episode_number FROM media_credit
          WHERE media_id = ? AND season_number IS NOT NULL ORDER BY person_name",
    ))
    .bind(&item.id)
    .fetch_all(db.pool())
    .await
    .unwrap()
    .iter()
    .map(|row| {
        (
            row.text("person_name").unwrap(),
            row.opt_int("season_number").unwrap(),
            row.opt_int("episode_number").unwrap(),
        )
    })
    .collect();
    assert_eq!(
        rows,
        [
            ("A Director".to_string(), Some(1), Some(2)),
            ("A Guest".to_string(), Some(1), Some(2)),
        ]
    );
    let guest = find(&db, "20").await.unwrap().expect("a record");
    let credited = credits(&db, &guest.id, false).await.unwrap();
    assert_eq!(credited.len(), 1);
    assert_eq!(credited[0].credit.credit_type, CreditType::Guest);
    assert_eq!(
        (
            credited[0].season_number,
            credited[0].episode_number,
            credited[0].episode_title.as_deref(),
            credited[0].air_date.as_deref()
        ),
        (Some(1), Some(2), Some("Part 2"), Some("2019-02-01"))
    );

    // A refresh no episode credits came back with keeps those held.
    let mut again = item.clone();
    again.people.episodes.clear();
    write(&db, &again).await;
    assert_eq!(credits(&db, &guest.id, false).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_merge_files_every_credit_under_the_record_kept_and_its_ids_lead_there() {
    let db = testing::db().await;
    let one = stored(&db, "One", |i| {
        i.credits = vec![credit(CreditType::Actor, "Jon Doe", Some(1))];
    })
    .await;
    let two = stored(&db, "Two", |i| {
        i.credits = vec![credit(CreditType::Actor, "J. Doe", Some(2))];
    })
    .await;
    let kept = filed(&db, &one.credits[0].id).await.expect("filed");
    let merged = filed(&db, &two.credits[0].id).await.expect("filed");

    assert_eq!(
        merge(&db, &kept, &kept).await.unwrap(),
        Err(Refused::Same),
        "not into itself"
    );
    let works = merge(&db, &merged, &kept).await.unwrap().expect("merged");
    assert_eq!(works, std::slice::from_ref(&two.id));
    assert_eq!(filed(&db, &two.credits[0].id).await, Some(kept.clone()));
    let record = get(&db, &kept).await.unwrap().expect("a record");
    assert_eq!(
        record.aliases,
        [Alias {
            name: "J. Doe".into(),
            source: "merged".into()
        }]
    );
    // The merged record leads to the one kept, by its id or its TMDB id.
    assert_eq!(
        find(&db, &merged).await.unwrap().map(|p| p.id),
        Some(kept.clone())
    );
    assert_eq!(
        find(&db, "2").await.unwrap().map(|p| p.id),
        Some(kept.clone())
    );
    assert_eq!(people(&db).await, 1);
    assert_eq!(
        merge(&db, &merged, &kept).await.unwrap(),
        Err(Refused::Gone),
        "merged already"
    );

    // A work crediting the merged one's TMDB id later files it there too.
    let three = stored(&db, "Three", |i| {
        i.credits = vec![credit(CreditType::Director, "J. Doe", Some(2))];
    })
    .await;
    assert_eq!(filed(&db, &three.credits[0].id).await, Some(kept));
}

#[tokio::test]
async fn a_credit_by_name_alone_is_attached_to_somebody_and_a_refresh_keeps_it() {
    let db = testing::db().await;
    stored(&db, "Known", |i| {
        i.credits = vec![credit(CreditType::Actor, "Mads Mikkelsen", Some(1019))];
    })
    .await;
    let mut item = stored(&db, "Typed", |i| {
        i.credits = vec![credit(CreditType::Actor, "M. Mikkelsen", None)];
    })
    .await;
    let mads = find(&db, "1019").await.unwrap().expect("a record");
    let was = filed(&db, &item.credits[0].id).await.expect("filed");
    assert_ne!(was, mads.id, "nothing said it was him");

    let (before, works) = attach(&db, &mads.id, &[item.credits[0].id.clone()])
        .await
        .unwrap()
        .expect("attached");
    assert_eq!((before, works), (vec![was], vec![item.id.clone()]));
    assert_eq!(filed(&db, &item.credits[0].id).await, Some(mads.id.clone()));

    item.credits[0].id = new_id();
    write(&db, &item).await;
    assert_eq!(filed(&db, &item.credits[0].id).await, Some(mads.id.clone()));

    assert_eq!(
        attach(&db, "nobody", &[item.credits[0].id.clone()])
            .await
            .unwrap(),
        Err(Refused::Gone)
    );
    assert_eq!(
        attach(&db, &mads.id, &["no-such-credit".to_string()])
            .await
            .unwrap(),
        Err(Refused::NotTheirs("no-such-credit".into()))
    );
}

#[tokio::test]
async fn a_credit_added_by_hand_is_filed_when_its_work_is_indexed() {
    let db = testing::db().await;
    let item = stored(&db, "By Hand", |i| {
        i.credits = vec![credit(CreditType::Actor, "Liv Ullmann", Some(101))];
    })
    .await;
    let mut typed = crate::db::repo::child::blank_credit(CreditType::Actor, "Liv Ullmann".into());
    typed.character_name = Some("Elisabet".into());
    let id = crate::db::repo::child::add_credit(&db, &item.id, &typed)
        .await
        .expect("added");
    assert_eq!(filed(&db, &id).await, None, "its own path names nobody");

    crate::db::repo::search::catch_up(&db, 10)
        .await
        .expect("caught up");
    let ullmann = find(&db, "101").await.unwrap().expect("a record");
    assert_eq!(filed(&db, &id).await, Some(ullmann.id));
}

#[tokio::test]
async fn a_hidden_credit_is_not_counted_and_nobody_whose_every_credit_is_hidden_is_listed() {
    let db = testing::db().await;
    let item = stored(&db, "Hidden", |i| {
        i.credits = vec![
            credit(CreditType::Actor, "Shown Somebody", Some(1)),
            credit(CreditType::Actor, "Hidden Somebody", Some(2)),
            credit(CreditType::Director, "Shown Somebody", Some(1)),
        ];
    })
    .await;
    let scope = crate::domain::fields::Child::Credit {
        kind: CreditType::Actor,
        person: crate::domain::fields::Person::Tmdb(2),
    }
    .to_string();
    crate::db::repo::override_field::set_in(
        &db,
        &item.id,
        &scope,
        "hidden",
        "",
        Some(&serde_json::json!(true)),
        None,
    )
    .await
    .expect("hidden");
    // The lock marks the work, and the index's next pass takes the person
    // whose every credit it hides out of the people found.
    crate::db::repo::search::catch_up(&db, 10)
        .await
        .expect("caught up");

    let page = |job: Option<Job>| {
        let db = db.clone();
        async move {
            index(
                &db,
                Asked {
                    job,
                    limit: 10,
                    ..Asked::default()
                },
            )
            .await
            .expect("listed")
        }
    };
    let listed = page(None).await;
    let names: Vec<(&str, i64, &[Job])> = listed
        .people
        .iter()
        .map(|p| (p.name.as_str(), p.works, p.jobs.as_slice()))
        .collect();
    assert_eq!(
        names,
        [("Shown Somebody", 1, &[Job::Acting, Job::Directing][..])]
    );
    assert_eq!((listed.total, listed.letters.as_slice()), (1, &['S'][..]));
    assert_eq!(page(Some(Job::Directing)).await.total, 1);
    assert_eq!(page(Some(Job::Writing)).await.total, 0);

    // Nor on their own page, nor in the search.
    let hidden = find(&db, "2").await.unwrap().expect("a record");
    assert!(credits(&db, &hidden.id, false).await.unwrap().is_empty());
    let shown =
        crate::db::repo::search::shown_people(&db, std::slice::from_ref(&hidden.id), false, false)
            .await
            .unwrap();
    assert!(shown.is_empty());
    let rows: i64 = sqlx::query_scalar(
        db.sql("SELECT COUNT(*) FROM search_doc WHERE kind = 'person' AND ref_id = ?"),
    )
    .bind(&hidden.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn somebody_entered_by_hand_keeps_what_was_typed_over_what_tmdb_says() {
    let db = testing::db().await;
    let entered = Entered {
        name: "Agnès Varda".into(),
        tmdb_id: Some(6817),
        sort_name: Some("Varda, Agnès".into()),
        aliases: vec!["Arlette Varda".into(), " ".into()],
        ..Entered::default()
    };
    let id = create(&db, &entered).await.unwrap().expect("created");
    assert_eq!(
        create(&db, &entered).await.unwrap(),
        Err(Refused::TmdbHeld(id.clone())),
        "one record a TMDB id"
    );

    // Credits TMDB gives under that id are filed under it.
    let item = stored(&db, "Cléo de 5 à 7", |i| {
        i.credits = vec![credit(CreditType::Director, "Agnes Varda", Some(6817))];
    })
    .await;
    assert_eq!(filed(&db, &item.credits[0].id).await, Some(id.clone()));

    write_details(
        &db,
        &id,
        &Details {
            name: Some("Agnès Varda (TMDB)".into()),
            biography: Some("Une cinéaste.".into()),
            birthday: Some("1928-05-30".into()),
            also_known_as: vec!["Arlette Varda".into(), "Аньес Варда".into()],
            ..Details::default()
        },
    )
    .await
    .unwrap();
    let record = get(&db, &id).await.unwrap().expect("a record");
    assert!(record.is_manual);
    assert_eq!(record.name, "Agnès Varda", "typed by hand");
    assert_eq!(record.sort_name.as_deref(), Some("Varda, Agnès"));
    assert_eq!(
        record.biography.as_deref(),
        Some("Une cinéaste."),
        "was empty"
    );
    assert!(record.refreshed_at.is_some());
    let names: Vec<(&str, &str)> = record
        .aliases
        .iter()
        .map(|a| (a.name.as_str(), a.source.as_str()))
        .collect();
    assert!(names.contains(&("Arlette Varda", "manual")), "{names:?}");
    assert!(names.contains(&("Аньес Варда", "tmdb")), "{names:?}");
    assert!(names.contains(&("Agnes Varda", "credit")), "{names:?}");

    // A record TMDB's alone takes its new name, the old one kept.
    let other = stored(&db, "Le Bonheur", |i| {
        i.credits = vec![credit(CreditType::Actor, "J-C Drouot", Some(9))];
    })
    .await;
    let drouot = filed(&db, &other.credits[0].id).await.expect("filed");
    write_details(
        &db,
        &drouot,
        &Details {
            name: Some("Jean-Claude Drouot".into()),
            ..Details::default()
        },
    )
    .await
    .unwrap();
    let record = get(&db, &drouot).await.unwrap().expect("a record");
    assert_eq!(record.name, "Jean-Claude Drouot");
    assert_eq!(record.aliases[0].name, "J-C Drouot");

    // Put right by hand: every field as given, the TMDB id checked.
    assert_eq!(
        update(
            &db,
            &drouot,
            &Entered {
                name: "Jean-Claude Drouot".into(),
                tmdb_id: Some(6817),
                ..Entered::default()
            }
        )
        .await
        .unwrap(),
        Err(Refused::TmdbHeld(id))
    );
    update(
        &db,
        &drouot,
        &Entered {
            name: "Jean-Claude Drouot".into(),
            tmdb_id: Some(9),
            imdb_id: Some("nm0238541".into()),
            ..Entered::default()
        },
    )
    .await
    .unwrap()
    .expect("put right");
    let record = get(&db, &drouot).await.unwrap().expect("a record");
    assert!(record.is_manual);
    assert_eq!(record.imdb_id.as_deref(), Some("nm0238541"));
}

#[test]
fn a_name_is_filed_under_its_first_letter_its_accent_aside() {
    assert_eq!(letter_of("Élodie Bouchez"), 'E');
    assert_eq!(letter_of("wim wenders"), 'W');
    assert_eq!(letter_of("宮崎駿"), '#');
    assert_eq!(letter_of("2Pac"), '#');
    assert_eq!(letter_of(""), '#');
}

#[test]
fn people_sharing_a_name_are_told_whatever_its_accents() {
    let listed = |id: &str, name: &str, aliases: &[&str]| Listed {
        id: id.into(),
        name: name.into(),
        tmdb_id: None,
        imdb_id: None,
        tvdb_id: None,
        image: None,
        is_manual: false,
        letter: letter_of(name),
        works: 1,
        jobs: Vec::new(),
        aliases: aliases.iter().map(|a| a.to_string()).collect(),
    };
    let people = [
        listed("a", "Pedro Almodóvar", &[]),
        listed("b", "Someone", &["pedro almodovar"]),
        listed("c", "Nobody Else", &[]),
        listed("d", "Twice Me", &["Twice Me"]),
    ];
    let sharing = sharing_a_name(&people);
    let mut ids: Vec<&str> = sharing.iter().map(String::as_str).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        ["a", "b"],
        "one record twice under its own name is one"
    );
}

#[tokio::test]
async fn a_credit_kept_by_its_person_still_brings_what_its_source_says_for_certain() {
    let db = testing::db().await;
    let mut item = stored(&db, "Breaking Bad", |i| {
        i.credits = vec![credit(CreditType::Actor, "Bryan Cranston", Some(17419))];
    })
    .await;
    let cranston = filed(&db, &item.credits[0].id).await.expect("filed");

    // Written again with TheTVDB's id of him, matched by the merge, and what
    // TMDB says beyond his name: the record he is kept under takes it.
    item.credits[0].id = new_id();
    item.people.by_credit.insert(
        item.credits[0].id.clone(),
        PersonRef {
            tvdb_id: Some(174190),
            gender: Some(2),
            ..PersonRef::default()
        },
    );
    write(&db, &item).await;
    assert_eq!(
        filed(&db, &item.credits[0].id).await,
        Some(cranston.clone())
    );
    let record = get(&db, &cranston).await.unwrap().expect("a record");
    assert_eq!((record.tvdb_id, record.gender), (Some(174190), Some(2)));

    // A person a credit by name alone was attached to by hand is no person
    // TheTVDB's id is certain of: nothing is filled in.
    let mut typed = stored(&db, "Typed", |i| {
        i.credits = vec![credit(CreditType::Actor, "B. Cranston", None)];
    })
    .await;
    attach(&db, &cranston, &[typed.credits[0].id.clone()])
        .await
        .unwrap()
        .expect("attached");
    typed.credits[0].id = new_id();
    typed.people.by_credit.insert(
        typed.credits[0].id.clone(),
        PersonRef {
            tvdb_id: Some(99),
            ..PersonRef::default()
        },
    );
    write(&db, &typed).await;
    assert_eq!(
        filed(&db, &typed.credits[0].id).await,
        Some(cranston.clone())
    );
    let record = get(&db, &cranston).await.unwrap().expect("a record");
    assert_eq!(record.tvdb_id, Some(174190));
}

#[tokio::test]
async fn a_name_alone_and_a_guest_tmdb_names_so_in_one_write_are_one_person() {
    let db = testing::db().await;
    let mut item = series_with_a_guest();
    // TheTVDB's character, which the merge could match to nobody in the cast.
    let mut character = credit(CreditType::Actor, "A Guest", None);
    character.sort_order = 1;
    item.people.by_credit.insert(
        character.id.clone(),
        PersonRef {
            tvdb_id: Some(555),
            ..PersonRef::default()
        },
    );
    item.credits.push(character.clone());
    write(&db, &item).await;

    let guest = find(&db, "20").await.unwrap().expect("a record");
    assert_eq!(
        filed(&db, &character.id).await,
        Some(guest.id.clone()),
        "the one TMDB names, made first"
    );
    assert_eq!(people(&db).await, 3, "the regular, the guest, the director");
}

#[tokio::test]
async fn the_index_counts_what_anybody_may_see_and_follows_a_work_switched_off() {
    let db = testing::db().await;
    let shown = stored(&db, "Shown", |i| {
        i.credits = vec![
            credit(CreditType::Director, "Agnès Varda", Some(6817)),
            credit(CreditType::Writer, "Agnès Varda", Some(6817)),
        ];
    })
    .await;
    stored(&db, "Also Shown", |i| {
        i.credits = vec![credit(CreditType::Actor, "Agnès Varda", Some(6817))];
    })
    .await;
    stored(&db, "For Adults", |i| {
        i.is_adult = true;
        i.credits = vec![
            credit(CreditType::Actor, "Agnès Varda", Some(6817)),
            credit(CreditType::Actor, "Only Adult", Some(1)),
        ];
    })
    .await;
    let asked = |job: Option<Job>, letter: Option<char>, by_name: bool| Asked {
        job,
        letter,
        by_name,
        limit: 10,
        offset: 0,
    };

    let page = index(&db, asked(None, None, false)).await.unwrap();
    assert_eq!(page.total, 1, "nobody credited on adult works alone");
    let varda = &page.people[0];
    assert_eq!((varda.letter, varda.works), ('A', 2), "its accent aside");
    assert_eq!(varda.jobs, [Job::Acting, Job::Directing, Job::Writing]);
    let writing = index(&db, asked(Some(Job::Writing), Some('A'), true))
        .await
        .unwrap();
    assert_eq!(writing.people[0].works, 1, "counted by the works written");
    assert_eq!(
        index(&db, asked(None, Some('B'), false))
            .await
            .unwrap()
            .total,
        0
    );

    // A work switched off: its credits count for nobody once the index's
    // next pass writes its people again.
    crate::db::repo::item::set_enabled(&db, &shown.id, false)
        .await
        .expect("switched off");
    crate::db::repo::search::catch_up(&db, 10)
        .await
        .expect("caught up");
    let page = index(&db, asked(None, None, false)).await.unwrap();
    assert_eq!(page.people[0].works, 1);
    assert_eq!(page.people[0].jobs, [Job::Acting]);
}
