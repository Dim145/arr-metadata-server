//! The people endpoints, against the whole server on a database of its own.

use axum::http::Uri;
use serde_json::Value;

use super::*;
use crate::domain::{Credit, Episode, MediaKind, person::EpisodeCredit};

fn credit(kind: CreditType, name: &str, tmdb: Option<i64>, character: Option<&str>) -> Credit {
    Credit {
        id: crate::db::new_id(),
        credit_type: kind,
        person_name: name.into(),
        character_name: character.map(str::to_string),
        image: None,
        tmdb_person_id: tmdb,
        credit_tmdb_id: None,
        sort_order: 0,
        is_manual: false,
    }
}

async fn stored(
    state: &AppState,
    kind: MediaKind,
    title: &str,
    year: i32,
    adjust: impl FnOnce(&mut MediaItem),
) -> MediaItem {
    let mut item = MediaItem::empty(kind);
    item.title = title.into();
    item.year = Some(year);
    item.slug = crate::domain::make_slug(title, Some(year));
    adjust(&mut item);
    repo::item::upsert(
        &state.db,
        repo::item::ItemWrite {
            item: &item,
            replace_children: true,
        },
    )
    .await
    .expect("stored");
    item
}

/// What `identity` is answered with for the index asked as `asked`.
async fn listed(state: &AppState, identity: Identity, asked: &str) -> AppResult<PeoplePage> {
    let uri: Uri = format!("/people?{asked}").parse().expect("an address");
    let Query(query) = Query::<IndexQuery>::try_from_uri(&uri).expect("a query");
    let response = index(State(state.clone()), Extension(identity), Query(query)).await?;
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("a body");
    Ok(serde_json::from_slice(&body).expect("a page"))
}

async fn read(state: &AppState, identity: Identity, address: &str) -> AppResult<PersonPage> {
    let Query(query) =
        Query::<PersonQuery>::try_from_uri(&"/people/x".parse::<Uri>().expect("an address"))
            .expect("a query");
    page(
        State(state.clone()),
        Extension(identity),
        Path(address.to_string()),
        Query(query),
    )
    .await
    .map(|Json(page)| page)
}

fn names(page: &PeoplePage) -> Vec<&str> {
    page.people.iter().map(|p| p.name.as_str()).collect()
}

#[tokio::test]
async fn the_index_lists_by_letter_and_by_job_the_most_credited_first() {
    let state = crate::api::native::testing::server().await;
    stored(&state, MediaKind::Movie, "Paris, Texas", 1984, |i| {
        i.credits = vec![
            credit(CreditType::Director, "Wim Wenders", Some(2725), None),
            credit(
                CreditType::Actor,
                "Harry Dean Stanton",
                Some(5048),
                Some("Travis"),
            ),
            credit(CreditType::Actor, "Nastassja Kinski", Some(3), Some("Jane")),
        ];
    })
    .await;
    stored(&state, MediaKind::Movie, "Wings of Desire", 1987, |i| {
        i.credits = vec![
            credit(CreditType::Director, "Wim Wenders", Some(2725), None),
            credit(
                CreditType::Writer,
                "Wim Wenders",
                Some(2725),
                Some("Screenplay"),
            ),
            credit(CreditType::Actor, "Élodie Example", None, None),
        ];
    })
    .await;
    stored(&state, MediaKind::Movie, "For Adults", 2001, |i| {
        i.is_adult = true;
        i.credits = vec![credit(CreditType::Actor, "Somebody Adult", Some(77), None)];
    })
    .await;

    let all = listed(&state, Identity::Visitor, "").await.expect("listed");
    assert_eq!(
        names(&all),
        [
            "Wim Wenders",
            "Élodie Example",
            "Harry Dean Stanton",
            "Nastassja Kinski"
        ],
        "the most credited first, then by name; nobody a visitor may not see"
    );
    assert_eq!(all.total, 4);
    assert_eq!(all.letters, ["E", "H", "N", "W"]);
    let wenders = &all.people[0];
    assert_eq!(wenders.works, 2);
    assert_eq!(wenders.jobs, [Job::Directing, Job::Writing]);
    assert_eq!(wenders.tmdb_id, Some(2725));
    assert!(wenders.aliases.is_empty() && wenders.imdb_id.is_none());

    let e = listed(&state, Identity::Visitor, "letter=e")
        .await
        .expect("listed");
    assert_eq!(names(&e), ["Élodie Example"], "its accent aside");
    assert_eq!(e.letters, all.letters, "the letters worth offering, still");

    let acting = listed(&state, Identity::Visitor, "job=acting&sort=name")
        .await
        .expect("listed");
    assert_eq!(
        names(&acting),
        ["Élodie Example", "Harry Dean Stanton", "Nastassja Kinski"]
    );
    let paged = listed(&state, Identity::Visitor, "sort=name&limit=2&offset=1")
        .await
        .expect("listed");
    assert_eq!(names(&paged), ["Harry Dean Stanton", "Nastassja Kinski"]);
    assert_eq!(paged.total, 4);

    for wrong in ["letter=ab", "job=catering", "sort=height"] {
        assert!(
            matches!(
                listed(&state, Identity::Visitor, wrong).await,
                Err(AppError::BadRequest(_))
            ),
            "{wrong}"
        );
    }

    // Whoever keeps the catalogue may ask for the records worth a second
    // look; a visitor asking the same is answered as anybody.
    let without = listed(&state, Identity::Anonymous, "withoutTmdb=true")
        .await
        .expect("listed");
    assert_eq!(names(&without), ["Élodie Example"]);
    let visitor = listed(&state, Identity::Visitor, "withoutTmdb=true")
        .await
        .expect("listed");
    assert_eq!(visitor.total, 4);
    let found = listed(&state, Identity::Anonymous, "term=wend")
        .await
        .expect("listed");
    assert_eq!(names(&found), ["Wim Wenders"]);
}

#[tokio::test]
async fn a_page_is_read_by_the_record_or_by_the_tmdb_id_with_each_episode_under_its_work() {
    let state = crate::api::native::testing::server().await;
    let film = stored(&state, MediaKind::Movie, "Paris, Texas", 1984, |i| {
        i.credits = vec![credit(
            CreditType::Actor,
            "Harry Dean Stanton",
            Some(5048),
            Some("Travis"),
        )];
    })
    .await;
    let series = stored(&state, MediaKind::Series, "Twin Peaks", 2017, |i| {
        i.episodes = (1..=3)
            .map(|n| {
                let mut episode: Episode = crate::db::repo::child::blank_episode(3, n);
                episode.is_manual = false;
                episode.title = format!("Part {n}");
                episode.air_date = Some(format!("2017-05-2{n}"));
                episode
            })
            .collect();
        i.people.episodes = (2..=3)
            .map(|n| EpisodeCredit {
                season_number: 3,
                episode_number: n,
                tmdb_episode_id: None,
                credit: credit(
                    CreditType::Guest,
                    "Harry Dean Stanton",
                    Some(5048),
                    Some("Carl Rodd"),
                ),
            })
            .collect();
    })
    .await;

    let by_tmdb = read(&state, Identity::Visitor, "5048").await.expect("read");
    let by_record = read(&state, Identity::Visitor, &by_tmdb.id)
        .await
        .expect("read");
    assert_eq!(by_record.id, by_tmdb.id);
    assert_eq!(by_record.tmdb_id, Some(5048));
    assert_eq!(by_record.name, "Harry Dean Stanton");
    let works: Vec<&str> = by_record.works.iter().map(|w| w.title.as_str()).collect();
    assert_eq!(works, ["Twin Peaks", "Paris, Texas"], "the newest first");
    let roles: Vec<(&str, Option<i32>, Option<&str>)> = by_record
        .roles
        .iter()
        .map(|r| {
            (
                r.work_id.as_str(),
                r.episode_number,
                r.episode_title.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        roles,
        [
            (series.id.as_str(), Some(2), Some("Part 2")),
            (series.id.as_str(), Some(3), Some("Part 3")),
            (film.id.as_str(), None, None),
        ]
    );
    assert_eq!(by_record.roles[0].credit_type, CreditType::Guest);
    assert_eq!(by_record.roles[0].air_date.as_deref(), Some("2017-05-22"));
    assert!(
        by_record.record.is_none(),
        "the record as it is kept is for whoever may put it right"
    );
    let kept = read(&state, Identity::Anonymous, "5048")
        .await
        .expect("read")
        .record
        .expect("the record, to whoever keeps the catalogue");
    assert_eq!(
        (kept.id.as_str(), kept.tmdb_id, kept.name.as_str()),
        (by_record.id.as_str(), Some(5048), "Harry Dean Stanton")
    );
    assert_eq!(
        kept.aliases
            .iter()
            .map(|a| a.source.as_str())
            .collect::<Vec<_>>(),
        Vec::<&str>::new(),
        "every other name with where it came from: none yet"
    );

    assert!(matches!(
        read(&state, Identity::Visitor, "999999").await,
        Err(AppError::NotFound)
    ));
    assert!(matches!(
        read(&state, Identity::Visitor, "no-such-person").await,
        Err(AppError::NotFound)
    ));
}

#[tokio::test]
async fn records_are_entered_merged_split_and_given_credits_by_whoever_keeps_the_catalogue() {
    let state = crate::api::native::testing::server().await;
    let film = stored(&state, MediaKind::Movie, "Cléo de 5 à 7", 1962, |i| {
        i.credits = vec![
            credit(CreditType::Director, "Agnès Varda", Some(6817), None),
            credit(CreditType::Actor, "Corinne Marchand", None, Some("Cléo")),
            credit(CreditType::Actor, "C. Marchand", None, None),
        ];
    })
    .await;
    let entered = |name: &str, tmdb: Option<i64>| EnteredPerson {
        name: name.into(),
        tmdb_id: tmdb,
        imdb_id: None,
        tvdb_id: None,
        sort_name: None,
        original_name: None,
        gender: None,
        birthday: None,
        deathday: None,
        place_of_birth: None,
        biography: None,
        image: None,
        aliases: Vec::new(),
    };

    // Nobody but whoever keeps the catalogue.
    assert!(matches!(
        create(
            State(state.clone()),
            Extension(Identity::Visitor),
            ClientIp::NONE,
            Json(entered("Anyone", None))
        )
        .await,
        Err(AppError::Forbidden)
    ));
    // A TMDB id another record holds, a name of nothing, a day that is not
    // one.
    assert!(matches!(
        create(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Json(entered("Agnès V.", Some(6817)))
        )
        .await,
        Err(AppError::Conflict(_))
    ));
    assert!(matches!(
        create(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Json(entered(" ", None))
        )
        .await,
        Err(AppError::BadRequest(_))
    ));
    let mut odd = entered("Anyone", None);
    odd.birthday = Some("long ago".into());
    assert!(matches!(
        create(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Json(odd)
        )
        .await,
        Err(AppError::BadRequest(_))
    ));

    let (status, Json(made)) = create(
        State(state.clone()),
        Extension(Identity::Anonymous),
        ClientIp::NONE,
        Json(entered("Michel Legrand", Some(2088))),
    )
    .await
    .expect("created");
    assert_eq!(status, StatusCode::CREATED);
    let mut corrected = entered("Michel Legrand", Some(2088));
    corrected.imdb_id = Some("nm0006166".into());
    update(
        State(state.clone()),
        Extension(Identity::Anonymous),
        ClientIp::NONE,
        Path(made.id.clone()),
        Json(corrected),
    )
    .await
    .expect("put right");

    // The two Marchands are one: merged.
    let page = read(&state, Identity::Anonymous, &film.id).await;
    assert!(
        matches!(page, Err(AppError::NotFound)),
        "a work is no person"
    );
    let marchand = repo::person::find(&state.db, &film.credits[1].id)
        .await
        .unwrap();
    assert!(marchand.is_none(), "a credit's id is no person's either");
    let filed = |credit: &Credit| {
        let state = state.clone();
        let id = credit.id.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                state
                    .db
                    .sql("SELECT person_id FROM media_credit WHERE id = ?"),
            )
            .bind(id)
            .fetch_one(state.db.pool())
            .await
            .expect("filed")
        }
    };
    let (marchand, short) = (filed(&film.credits[1]).await, filed(&film.credits[2]).await);
    let Json(kept) = merge(
        State(state.clone()),
        Extension(Identity::Anonymous),
        ClientIp::NONE,
        Path(short.clone()),
        Json(MergePerson {
            into: marchand.clone(),
        }),
    )
    .await
    .expect("merged");
    assert_eq!(kept.id, marchand);
    let page = read(&state, Identity::Visitor, &short).await.expect("read");
    assert_eq!(page.id, marchand, "the merged one leads to the one kept");
    assert_eq!(page.roles.len(), 2);
    assert_eq!(page.aliases, ["C. Marchand"]);

    // And split again: the short name was somebody else after all.
    let (status, Json(split_off)) = split(
        State(state.clone()),
        Extension(Identity::Anonymous),
        ClientIp::NONE,
        Path(marchand.clone()),
        Json(SplitPerson {
            credits: vec![film.credits[2].id.clone()],
            name: Some("Claire Marchand".into()),
        }),
    )
    .await
    .expect("split");
    assert_eq!(status, StatusCode::CREATED);
    let page = read(&state, Identity::Visitor, &split_off.id)
        .await
        .expect("read");
    assert_eq!(page.name, "Claire Marchand");
    assert!(page.is_manual);
    assert!(matches!(
        split(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path(marchand.clone()),
            Json(SplitPerson {
                credits: vec![film.credits[0].id.clone()],
                name: None,
            }),
        )
        .await,
        Err(AppError::BadRequest(_))
    ));

    // Given to Legrand by hand: his page lists it.
    attach(
        State(state.clone()),
        Extension(Identity::Anonymous),
        ClientIp::NONE,
        Path(made.id.clone()),
        Json(AttachCredits {
            credits: vec![film.credits[2].id.clone()],
        }),
    )
    .await
    .expect("attached");
    let page = read(&state, Identity::Visitor, "2088").await.expect("read");
    assert_eq!(page.id, made.id);
    assert_eq!(page.roles.len(), 1);

    // Each write is on the audit trail, under the record it was made to.
    let actions: Vec<String> = sqlx::query_scalar(
        state
            .db
            .sql("SELECT action FROM audit_log WHERE action LIKE 'person.%' ORDER BY at, action"),
    )
    .fetch_all(state.db.pool())
    .await
    .unwrap();
    for action in [
        "person.created",
        "person.updated",
        "person.merged",
        "person.split",
        "person.credited",
    ] {
        assert!(
            actions.iter().any(|a| a == action),
            "{action} in {actions:?}"
        );
    }
    assert_eq!(page.works[0].id, film.id);

    // Found by the search under every name a record goes by.
    let found =
        repo::search::shown_people(&state.db, std::slice::from_ref(&marchand), false, false)
            .await
            .unwrap();
    assert!(found.contains(&marchand));
}

#[test]
fn somebody_is_read_off_tmdb_s_record_down_to_what_is_kept() {
    let profiles: Vec<Value> = (0..9)
        .map(|n| serde_json::json!({ "file_path": format!("/p{n}.jpg") }))
        .collect();
    let raw = serde_json::json!({
        "id": 17419,
        "name": "Bryan Cranston",
        "biography": "  ",
        "birthday": "1956-03-07",
        "deathday": null,
        "place_of_birth": "Hollywood, Los Angeles, California, USA",
        "known_for_department": "Acting",
        "gender": 2,
        "popularity": 9.5,
        "profile_path": "/portrait.jpg",
        "also_known_as": ["Bryan Lee Cranston", " ", "Lee Stone", "Bryan Lee Cranston"],
        "homepage": "",
        "imdb_id": null,
        "external_ids": { "imdb_id": "nm0186505", "wikidata_id": "Q23547" },
        "images": { "profiles": profiles }
    });
    let details = read_details(&raw);
    assert_eq!(details.name.as_deref(), Some("Bryan Cranston"));
    assert_eq!(details.biography, None, "blank is nothing");
    assert_eq!(details.birthday.as_deref(), Some("1956-03-07"));
    assert_eq!(details.deathday, None);
    assert_eq!(
        details.place_of_birth.as_deref(),
        Some("Hollywood, Los Angeles, California, USA")
    );
    assert_eq!(details.known_for.as_deref(), Some("Acting"));
    assert_eq!(details.gender, Some(2));
    assert_eq!(details.popularity, Some(9.5));
    assert_eq!(details.also_known_as, ["Bryan Lee Cranston", "Lee Stone"]);
    assert_eq!(details.homepage, None);
    assert_eq!(details.imdb_id.as_deref(), Some("nm0186505"));
    assert!(
        details
            .image
            .as_deref()
            .is_some_and(|i| i.ends_with("/portrait.jpg"))
    );
    assert_eq!(details.photos.len(), MOST_PHOTOS);
    assert!(details.photos[0].starts_with("https://"));
    // TMDB's 0 is "not set", which is nothing to agree with; a title's id
    // is no person's.
    let unknown = read_details(&serde_json::json!({ "gender": 0, "imdb_id": "tt0903747" }));
    assert_eq!((unknown.gender, unknown.imdb_id), (None, None));
}
