//! Wikidata's links between the sites' ids, read into the database
//! (`wikidata.enabled`), weekly (wave P, P5).
//!
//! Each site's ids are asked of Wikidata's query service in turn
//! ([`crate::providers::wikidata`]), with a pause between two queries. Once
//! every query has been answered, what the reading changes of the links kept
//! is written ([`repo::wikidata::apply`]): in one transaction, but on SQLite
//! for a change as large as the first reading's. A reading that fails — a
//! query refused, timed out or cut short, or a site listing far fewer ids
//! than the links kept hold of it — writes nothing: the links kept stay as
//! they were, until a reading goes through.

use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result, bail};

use crate::{
    db::{
        now,
        repo::{self, wikidata::Link},
    },
    jobs::datasets::WIKIDATA_IDS as LIST,
    providers::wikidata::{self, Site},
    state::AppState,
};

/// How long between two sites' readings: Wikidata's service wants a client's
/// queries one at a time, and a moment between them. A site is read in eleven
/// parts, with a pause of their own between them
/// ([`wikidata::BETWEEN_PARTS`]): a few minutes of queries in all, in
/// November 2026.
#[cfg(not(test))]
const BETWEEN: Duration = Duration::from_secs(2);
#[cfg(test)]
const BETWEEN: Duration = Duration::from_millis(25);

/// A site with fewer ids kept than this is not weighed against: a few ids
/// more or less there say nothing of the reading.
const WEIGHED_FROM: usize = 100;

/// Read Wikidata's links through, and keep them: what the reading did, as the
/// history of the task says it.
pub async fn import(state: &AppState) -> Result<String> {
    // As of when it began, as the other lists are recorded.
    let started = now();
    let endpoint = &state.config.wikidata.endpoint;

    let mut links: Vec<Link> = Vec::new();
    let mut skipped = 0;
    for (n, site) in Site::ANCHORS.into_iter().enumerate() {
        if n > 0 {
            tokio::time::sleep(BETWEEN).await;
        }
        let asked = std::time::Instant::now();
        let read = wikidata::read(&state.http, endpoint, site)
            .await
            .with_context(|| format!("{} could not be read; the links kept stay", site.label()))?;
        tracing::debug!(
            site = site.label(),
            links = read.links.len(),
            skipped = read.skipped,
            seconds = asked.elapsed().as_secs_f32(),
            "Wikidata answered"
        );
        skipped += read.skipped;
        links.reserve_exact(read.links.len());
        links.extend(read.links);
    }
    // An item's IMDb id comes with each of its sites' answers.
    links.sort_unstable();
    links.dedup();
    links.shrink_to_fit();

    if links.is_empty() {
        bail!("Wikidata listed no id of any site; the links kept stay");
    }

    let change = repo::wikidata::compare(&state.db, &links).await?;
    weigh(&change.before, &links)?;

    let total = links.len();
    let items = {
        let mut items: Vec<u32> = links.iter().map(|link| link.item).collect();
        drop(links);
        items.sort_unstable();
        items.dedup();
        items.len()
    };

    repo::wikidata::apply(&state.db, &change).await?;
    repo::import::record(&state.db, LIST, total as i64, &started).await?;

    if skipped > 0 {
        tracing::info!(
            skipped,
            "Wikidata's values that were not ids of their site's were skipped"
        );
    }
    Ok(format!(
        "{total} links on {items} items: {} new, {} gone",
        change.added.len(),
        change.gone.len()
    ))
}

/// Refuse a reading that lists fewer than half the ids of a site the links
/// kept hold: a property Wikidata renamed, an answer the service cut short
/// in a way that still reads. Taken, it would forget most of what the links
/// knew until the next reading; refused, they are kept, and the history
/// says why.
fn weigh(before: &BTreeMap<u16, usize>, links: &[Link]) -> Result<()> {
    let mut listed: BTreeMap<u16, usize> = BTreeMap::new();
    for link in links {
        *listed.entry(link.property).or_default() += 1;
    }
    // The sites read: the links of one this server no longer asks for go,
    // whatever their number.
    for site in Site::ALL {
        let kept = before.get(&site.property()).copied().unwrap_or(0);
        let now = listed.get(&site.property()).copied().unwrap_or(0);
        if kept >= WEIGHED_FROM && now < kept / 2 {
            bail!(
                "Wikidata listed {now} of {} against {kept} kept; the links kept stay",
                site.label()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, VecDeque},
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Instant,
    };

    use axum::{
        http::{StatusCode, header},
        response::IntoResponse as _,
    };

    use super::*;
    use crate::db::repo::wikidata as kept;

    /// An answer the stand-in gives once, before the table: a status, the
    /// `Retry-After` it carries, and a body.
    type Once = (StatusCode, Option<&'static str>, String);

    /// One request the stand-in was asked.
    #[derive(Clone, Debug)]
    struct Asked {
        property: u16,
        agent: String,
        accept: String,
        began: Instant,
        ended: Instant,
    }

    #[derive(Default)]
    struct Script {
        /// The table each property is answered with: a header alone when
        /// none is set.
        tables: HashMap<u16, String>,
        /// What each property is answered first, in turn.
        once: HashMap<u16, VecDeque<Once>>,
        asked: Vec<Asked>,
    }

    /// Wikidata's query service, as a stand-in: each query answered with the
    /// table of the property it asks for, what it was asked kept, and whether
    /// two queries were ever under way at once.
    struct StandIn {
        endpoint: String,
        script: Arc<Mutex<Script>>,
        overlapped: Arc<AtomicUsize>,
    }

    impl StandIn {
        async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("a port");
            let endpoint = format!(
                "http://{}/sparql",
                listener.local_addr().expect("its address")
            );
            let script = Arc::new(Mutex::new(Script::default()));
            let overlapped = Arc::new(AtomicUsize::new(0));
            let under_way = Arc::new(AtomicUsize::new(0));
            let app = {
                let script = Arc::clone(&script);
                let overlapped = Arc::clone(&overlapped);
                axum::Router::new().route(
                    "/sparql",
                    axum::routing::get(
                        move |headers: axum::http::HeaderMap,
                              query: axum::extract::Query<HashMap<String, String>>| {
                            let script = Arc::clone(&script);
                            let overlapped = Arc::clone(&overlapped);
                            let under_way = Arc::clone(&under_way);
                            async move {
                                let began = Instant::now();
                                if under_way.fetch_add(1, Ordering::SeqCst) > 0 {
                                    overlapped.fetch_add(1, Ordering::SeqCst);
                                }
                                let asked = query.get("query").map(String::as_str).unwrap_or_default();
                                let property = asked_property(asked);
                                let part = asked_part(asked);
                                // As long as a query takes, a little.
                                tokio::time::sleep(Duration::from_millis(5)).await;
                                let reply = {
                                    let mut script = script.lock().expect("the script");
                                    let once = script
                                        .once
                                        .get_mut(&property)
                                        .and_then(VecDeque::pop_front);
                                    let reply = once.unwrap_or_else(|| {
                                        (
                                            StatusCode::OK,
                                            None,
                                            script
                                                .tables
                                                .get(&property)
                                                .map(|table| only(table, part))
                                                .unwrap_or_else(|| format!("{}\n", wikidata::HEADER)),
                                        )
                                    });
                                    let field = |name: header::HeaderName| {
                                        headers
                                            .get(name)
                                            .and_then(|v| v.to_str().ok())
                                            .unwrap_or_default()
                                            .to_string()
                                    };
                                    script.asked.push(Asked {
                                        property,
                                        agent: field(header::USER_AGENT),
                                        accept: field(header::ACCEPT),
                                        began,
                                        ended: Instant::now(),
                                    });
                                    reply
                                };
                                under_way.fetch_sub(1, Ordering::SeqCst);
                                let (status, retry_after, body) = reply;
                                let mut response = (status, body).into_response();
                                response.headers_mut().insert(
                                    header::CONTENT_TYPE,
                                    header::HeaderValue::from_static(
                                        "text/tab-separated-values;charset=utf-8",
                                    ),
                                );
                                if let Some(seconds) = retry_after {
                                    response.headers_mut().insert(
                                        header::RETRY_AFTER,
                                        header::HeaderValue::from_static(seconds),
                                    );
                                }
                                response
                            }
                        },
                    ),
                )
            };
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            Self {
                endpoint,
                script,
                overlapped,
            }
        }

        /// Answer a property's queries with a table of these rows: an
        /// item's number, the id, and the item's IMDb id when it has one.
        fn table(&self, site: Site, rows: &[(u32, &str, Option<&str>)]) {
            let mut table = format!("{}\n", wikidata::HEADER);
            for (item, value, imdb) in rows {
                let imdb = imdb.map(|imdb| format!("\"{imdb}\"")).unwrap_or_default();
                table.push_str(&format!(
                    "<http://www.wikidata.org/entity/Q{item}>\t\"{value}\"\t{imdb}\n"
                ));
            }
            self.script
                .lock()
                .expect("the script")
                .tables
                .insert(site.property(), table);
        }

        /// Answer a property's next query so, before its table.
        fn once(&self, site: Site, answer: Once) {
            self.script
                .lock()
                .expect("the script")
                .once
                .entry(site.property())
                .or_default()
                .push_back(answer);
        }

        fn asked(&self) -> Vec<Asked> {
            self.script.lock().expect("the script").asked.clone()
        }
    }

    /// The property a query asks for: `?item wdt:P4947 ?value`.
    fn asked_property(query: &str) -> u16 {
        query
            .split("?item wdt:P")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .and_then(|number| number.parse().ok())
            .unwrap_or(0)
    }

    /// The part a query asks for, as its filter says it.
    fn asked_part(query: &str) -> wikidata::Part {
        query
            .split("STRENDS(STR(?value), \"")
            .nth(1)
            .and_then(|rest| rest.bytes().next())
            .filter(u8::is_ascii_digit)
            .map_or(wikidata::Part::Rest, |digit| {
                wikidata::Part::Ending(digit - b'0')
            })
    }
    /// A table's header, and its rows whose value falls in `part`: what the
    /// service answers the part's filter with.
    fn only(table: &str, part: wikidata::Part) -> String {
        let mut lines = table.lines();
        let mut out = format!("{}\n", lines.next().unwrap_or_default());
        for line in lines {
            let value = line
                .split('\t')
                .nth(1)
                .unwrap_or_default()
                .trim_matches('"');
            if wikidata::Part::of(value) == part {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }
    /// The server, Wikidata's query service at `endpoint`.
    async fn server(endpoint: &str) -> AppState {
        let mut config = crate::config::Config::from_env().expect("a configuration");
        config.mode = crate::config::Mode::Single;
        config.database = crate::db::testing::database().await;
        config.security.bootstrap_admin = None;
        config.cache.redis_url = None;
        config.media.storage = crate::config::MediaStorage::Off;
        config.clients = None;
        config.tmdb.api_key = None;
        config.tvdb.api_key = None;
        config.tvdb.enabled = false;
        config.wikidata.endpoint = endpoint.to_string();
        AppState::bootstrap(config).await.expect("a server")
    }

    /// *Breaking Bad*, *Inception* and *One Piece*, as Wikidata files them.
    fn tables(stand_in: &StandIn) {
        stand_in.table(Site::TmdbMovie, &[(25188, "27205", Some("tt1375666"))]);
        stand_in.table(
            Site::TmdbTv,
            &[
                (1079, "1396", Some("tt0903747")),
                (2, "37854", Some("tt0388629")),
            ],
        );
        stand_in.table(
            Site::TvdbSeries,
            &[
                (1079, "81189", Some("tt0903747")),
                (2, "81797", Some("tt0388629")),
                // A slug where an id goes: skipped.
                (3, "lists/breaking-bad", None),
            ],
        );
        stand_in.table(Site::TvdbMovie, &[(25188, "113", Some("tt1375666"))]);
        stand_in.table(Site::AniList, &[(2, "21", Some("tt0388629"))]);
        stand_in.table(Site::Mal, &[(2, "21", Some("tt0388629"))]);
        stand_in.table(Site::AniDb, &[(2, "69", Some("tt0388629"))]);
        stand_in.table(Site::TvMaze, &[(1079, "169", Some("tt0903747"))]);
    }

    fn link(site: Site, value: u32, item: u32) -> Link {
        Link {
            property: site.property(),
            value,
            item,
        }
    }

    async fn all_kept(state: &AppState) -> Vec<Link> {
        let mut all = Vec::new();
        for item in [2, 3, 1079, 25188, 550] {
            all.extend(kept::of_item(&state.db, item).await.expect("read"));
        }
        all.sort_unstable();
        all
    }

    #[tokio::test]
    async fn every_site_is_asked_in_turn_politely_and_what_changed_is_kept() {
        let stand_in = StandIn::start().await;
        tables(&stand_in);
        let state = server(&stand_in.endpoint).await;

        let summary = import(&state).await.expect("read");
        assert_eq!(summary, "13 links on 3 items: 13 new, 0 gone");

        // Each site in turn, in its eleven parts, one query at a time, a
        // pause between two, with who asks and what is wanted said.
        let asked = stand_in.asked();
        assert_eq!(
            asked.iter().map(|a| a.property).collect::<Vec<_>>(),
            Site::ANCHORS
                .iter()
                .flat_map(|site| [site.property(); wikidata::Part::ALL.len()])
                .collect::<Vec<_>>()
        );
        assert_eq!(stand_in.overlapped.load(Ordering::SeqCst), 0);
        for pair in asked.windows(2) {
            let pause = if pair[0].property == pair[1].property {
                wikidata::BETWEEN_PARTS
            } else {
                BETWEEN
            };
            assert!(pair[1].began >= pair[0].ended + pause, "{pair:?}");
        }
        for one in &asked {
            assert_eq!(
                one.agent,
                format!(
                    "arr-metadata-server/{} ({})",
                    env!("CARGO_PKG_VERSION"),
                    env!("CARGO_PKG_REPOSITORY")
                )
            );
            assert_eq!(one.accept, "text/tab-separated-values");
        }

        // TMDB's series leads to the whole item; the reading is recorded.
        assert_eq!(
            kept::of_id(&state.db, Site::TmdbTv.property(), 1396)
                .await
                .expect("read"),
            vec![
                link(Site::Imdb, 903_747, 1079),
                link(Site::TvdbSeries, 81189, 1079),
                link(Site::TmdbTv, 1396, 1079),
                link(Site::TvMaze, 169, 1079),
            ]
        );
        let recorded = repo::import::get(&state.db, LIST)
            .await
            .expect("read")
            .expect("recorded");
        assert_eq!(recorded.row_count, 13);

        // A week on: TVmaze's id of the series is gone, a film has come.
        stand_in.table(Site::TvMaze, &[]);
        stand_in.table(
            Site::TmdbMovie,
            &[
                (25188, "27205", Some("tt1375666")),
                (550, "550", Some("tt0137523")),
            ],
        );
        let summary = import(&state).await.expect("read");
        assert_eq!(summary, "14 links on 4 items: 2 new, 1 gone");
        let all = all_kept(&state).await;
        assert_eq!(all.len(), 14);
        assert!(all.contains(&link(Site::TmdbMovie, 550, 550)));
        assert!(!all.contains(&link(Site::TvMaze, 169, 1079)));
    }

    #[tokio::test]
    async fn a_reading_that_fails_anywhere_leaves_the_links_as_they_were() {
        let stand_in = StandIn::start().await;
        tables(&stand_in);
        let state = server(&stand_in.endpoint).await;
        import(&state).await.expect("read");
        let before = all_kept(&state).await;
        let recorded = repo::import::get(&state.db, LIST)
            .await
            .expect("read")
            .expect("recorded");

        // The service failing on the fifth site: nothing is written,
        // nothing is asked after it.
        stand_in.table(Site::TmdbMovie, &[]);
        stand_in.once(
            Site::AniList,
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                None,
                "java.util.concurrent.TimeoutException".into(),
            ),
        );
        let asked = stand_in.asked().len();
        let failed = import(&state).await.expect_err("refused");
        assert!(
            format!("{failed:#}").contains("AniList's entries could not be read"),
            "{failed:#}"
        );
        // The four sites before it, each in its parts, then its first part.
        assert_eq!(
            stand_in.asked().len(),
            asked + 4 * wikidata::Part::ALL.len() + 1
        );
        assert_eq!(all_kept(&state).await, before);

        // An answer begun with a success and cut short.
        stand_in.once(
            Site::TvMaze,
            (
                StatusCode::OK,
                None,
                format!(
                    "{}\n<http://www.wikidata.org/entity/Q1079>\t\"16",
                    wikidata::HEADER
                ),
            ),
        );
        let failed = import(&state).await.expect_err("refused");
        assert!(format!("{failed:#}").contains("cut short"), "{failed:#}");
        assert_eq!(all_kept(&state).await, before);
        let still = repo::import::get(&state.db, LIST)
            .await
            .expect("read")
            .expect("recorded");
        assert_eq!(still.imported_at, recorded.imported_at);
    }

    #[tokio::test]
    async fn a_reading_listing_far_fewer_ids_of_a_site_is_refused() {
        let stand_in = StandIn::start().await;
        tables(&stand_in);
        let films: Vec<(u32, String)> = (1..=200).map(|n| (10_000 + n, n.to_string())).collect();
        let rows = |count: usize| -> Vec<(u32, &str, Option<&str>)> {
            films
                .iter()
                .take(count)
                .map(|(item, id)| (*item, id.as_str(), None))
                .collect()
        };
        stand_in.table(Site::TmdbMovie, &rows(200));
        let state = server(&stand_in.endpoint).await;
        import(&state).await.expect("read");

        // Ninety films of two hundred: under half.
        stand_in.table(Site::TmdbMovie, &rows(90));
        let failed = import(&state).await.expect_err("refused");
        assert!(
            format!("{failed:#}").contains("Wikidata listed 90 of TMDB's films against 200 kept"),
            "{failed:#}"
        );
        assert_eq!(
            kept::of_id(&state.db, Site::TmdbMovie.property(), 150)
                .await
                .expect("read")
                .len(),
            1
        );

        // A hundred and ten: a site losing ids, not a reading gone wrong.
        stand_in.table(Site::TmdbMovie, &rows(110));
        let summary = import(&state).await.expect("read");
        assert!(summary.ends_with("0 new, 90 gone"), "{summary}");
    }

    #[tokio::test]
    async fn a_429_is_waited_out_as_its_retry_after_says_and_not_past_it() {
        let stand_in = StandIn::start().await;
        tables(&stand_in);
        let state = server(&stand_in.endpoint).await;

        // A second asked: waited, and asked again.
        stand_in.once(
            Site::TmdbTv,
            (StatusCode::TOO_MANY_REQUESTS, Some("1"), String::new()),
        );
        import(&state).await.expect("read");
        let asked = stand_in.asked();
        let tv: Vec<&Asked> = asked
            .iter()
            .filter(|a| a.property == Site::TmdbTv.property())
            .collect();
        // Its first part asked twice, then the others.
        assert_eq!(tv.len(), wikidata::Part::ALL.len() + 1);
        assert!(tv[1].began >= tv[0].ended + Duration::from_secs(1));

        // An hour asked: the reading fails at once, and the next one does
        // not ask before the hour is out.
        stand_in.once(
            Site::TmdbMovie,
            (StatusCode::TOO_MANY_REQUESTS, Some("3600"), String::new()),
        );
        let failed = import(&state).await.expect_err("refused");
        assert!(format!("{failed:#}").contains("3600 s"), "{failed:#}");
        let asked = stand_in.asked().len();
        let failed = import(&state).await.expect_err("refused");
        assert!(
            format!("{failed:#}").contains("asked to be left alone"),
            "{failed:#}"
        );
        assert_eq!(stand_in.asked().len(), asked);
    }
}
