//! The tools: what each one takes, what it asks the API, and what an agent
//! is given back.
//!
//! An answer is the API's, cut down to what an agent reads a work by: no
//! pictures' addresses, no internal ids it cannot use, a long overview cut,
//! a long list shortened — and every tool's answer kept well under what a
//! client takes in at once (see [`super::server::MOST`]).

use chrono::Datelike as _;
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::facade::{Facade, Refusal, segment};
use crate::auth::Identity;

// ─── what each tool takes ────────────────────────────────────────────────────

/// What `search` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "camelCase")]
pub(super) struct SearchArgs {
    /// What to look for, as a person would type it: a title, somebody's
    /// name, an episode's title. A year after a title, `dune 2021`, puts that
    /// year's work first.
    pub query: String,
    /// Which kinds of thing to find: `work`, `person`, `episode`. All three
    /// when left out.
    #[serde(default)]
    pub kinds: Option<Vec<HitKind>>,
    /// The most found of each kind, from 1 to 20; 8 when left out.
    #[serde(default)]
    pub limit: Option<u8>,
    /// The language to read titles in, as a tag: `fr`, `ja`, `pt-BR`. The
    /// one this key is answered in when left out.
    #[serde(default)]
    pub language: Option<String>,
}

/// A kind of thing `search` finds.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "camelCase")]
pub(super) enum HitKind {
    Work,
    Person,
    Episode,
}

impl HitKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Person => "person",
            Self::Episode => "episode",
        }
    }
}

/// What `item` and `provenance` take.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "camelCase")]
pub(super) struct ItemArgs {
    /// The work's id, as `search`, `calendar` and `lists` give it.
    pub id: String,
    /// The language to read the work in, as a tag: `fr`, `ja`. The one this
    /// key is answered in when left out.
    #[serde(default)]
    pub language: Option<String>,
}

/// What `episodes` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "camelCase")]
pub(super) struct EpisodesArgs {
    /// The series' id, as `search` gives it.
    pub id: String,
    /// One season, by its number: 0 holds the specials. Every season when
    /// left out.
    #[serde(default)]
    pub season: Option<i32>,
    /// How many episodes to pass over: the `nextOffset` of the answer
    /// before, for the next page.
    #[serde(default)]
    pub offset: Option<u32>,
    /// The most episodes in one answer, from 1 to 100; 50 when left out.
    #[serde(default)]
    pub limit: Option<u8>,
    /// The language to read the episodes in, as a tag. The one this key is
    /// answered in when left out.
    #[serde(default)]
    pub language: Option<String>,
}

/// What `calendar` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "camelCase")]
pub(super) struct CalendarArgs {
    /// The first day, `YYYY-MM-DD`, counted in UTC. The Monday of the
    /// current week when left out.
    #[serde(default)]
    pub start: Option<String>,
    /// How many days from it, from 1 to 31; 7 when left out — a week.
    #[serde(default)]
    pub days: Option<u8>,
    /// The country a film's release dates are read in, ISO 3166-1 alpha-2:
    /// `FR`, `US`. The one this key is answered for when left out.
    #[serde(default)]
    pub region: Option<String>,
    /// The language to read titles in, as a tag. The one this key is
    /// answered in when left out.
    #[serde(default)]
    pub language: Option<String>,
}

/// What `lists` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "camelCase")]
pub(super) struct ListsArgs {
    /// One list, by the slug or the id `lists` gives: its works. Every list,
    /// without their works, when left out.
    #[serde(default)]
    pub list: Option<String>,
    /// The most works of one list in the answer, from 1 to 100; 50 when
    /// left out.
    #[serde(default)]
    pub limit: Option<u8>,
}

/// What `todo` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "camelCase")]
pub(super) struct TodoArgs {
    /// One errand, whose works to list. Every errand, counted, when left
    /// out.
    #[serde(default)]
    pub gap: Option<Gap>,
    /// The most works in the answer, from 1 to 100; 25 when left out.
    #[serde(default)]
    pub limit: Option<u8>,
}

/// An errand of the catalogue's: the works it lists lack something, or need
/// a look.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "camelCase")]
pub(super) enum Gap {
    /// No poster, of their own or chosen by hand.
    Poster,
    /// No overview in the language the catalogue is kept in.
    Overview,
    /// No IMDb identifier.
    Imdb,
    /// Never refreshed, though they could be.
    Refresh,
    /// Their last refresh failed.
    RefreshFailed,
    /// A field locked by hand that a source has moved under since.
    DivergingLock,
    /// Another work looks like the same one.
    Duplicates,
    /// A picture or a theme the media store gave up fetching.
    MediaFailed,
}

impl Gap {
    /// The filter of the catalogue's list it is: `missing=…`, or the
    /// failed refreshes'.
    fn filter(self) -> (&'static str, &'static str) {
        match self {
            Self::Poster => ("missing", "poster"),
            Self::Overview => ("missing", "overview"),
            Self::Imdb => ("missing", "imdb"),
            Self::Refresh => ("missing", "refresh"),
            Self::DivergingLock => ("missing", "divergingLock"),
            Self::Duplicates => ("missing", "duplicates"),
            Self::MediaFailed => ("missing", "mediaFailed"),
            Self::RefreshFailed => ("refreshFailed", "true"),
        }
    }
}

// ─── the tools ───────────────────────────────────────────────────────────────

/// What a tool hands back: an answer, or why there is none.
pub(super) type Answer = Result<Value, Refusal>;

/// Search the catalogue.
pub(super) async fn search(api: &Facade, reader: &Identity, args: SearchArgs) -> Answer {
    let query = args.query.trim();
    if query.is_empty() {
        return Err(Refusal::Asked(
            "query is empty: say what to look for".into(),
        ));
    }
    let mut asked = vec![
        ("term", query.to_string()),
        ("limit", within(args.limit, 8, 20).to_string()),
    ];
    if let Some(kinds) = &args.kinds
        && !kinds.is_empty()
    {
        let named: Vec<&str> = kinds.iter().map(|kind| kind.as_str()).collect();
        asked.push(("kinds", named.join(",")));
    }
    language(&mut asked, args.language);

    let found = api.get(reader, "/search", &asked).await?;
    Ok(json!({
        "works": each(&found["works"], &["id", "kind", "title", "year", "matched"]),
        "people": each(&found["people"], &["id", "name", "matched", "jobs", "works", "tmdbId"]),
        "episodes": each(
            &found["episodes"],
            &["workId", "workTitle", "seasonNumber", "episodeNumber", "title", "airDate"],
        ),
    }))
}

/// One work, without its episodes.
pub(super) async fn item(api: &Facade, reader: &Identity, args: ItemArgs) -> Answer {
    let work = work(api, reader, &args.id, args.language).await?;

    let mut shown = pick(
        &work,
        &[
            "id",
            "kind",
            "title",
            "originalTitle",
            "year",
            "status",
            "tagline",
            "overview",
            "genres",
            "runtime",
            "firstAired",
            "lastAired",
            "airTime",
            "inCinemas",
            "digitalRelease",
            "physicalRelease",
            "network",
            "studio",
            "originalLanguage",
            "originalCountry",
            "contentRating",
            "contentRatingCountry",
            "homepage",
            "externalIds",
            "lockedFields",
            "isEnabled",
            "isAdult",
            "isManual",
            "refreshedAt",
            "refreshError",
        ],
    );
    let object = shown.as_object_mut().expect("picked is an object");
    if let Some(keywords) = work["keywords"].as_array() {
        object.insert("keywords".into(), Value::Array(first(keywords, 20)));
    }
    if let Some(ratings) = work["ratings"].as_array() {
        object.insert(
            "ratings".into(),
            each_of(ratings, &["source", "value", "votes", "ratingType"]),
        );
    }
    if let Some(credits) = work["credits"].as_array() {
        object.insert(
            "credits".into(),
            each_of(
                &first(credits, 20),
                &["creditType", "personName", "characterName"],
            ),
        );
        object.insert("creditCount".into(), json!(credits.len()));
    }
    if let Some(titles) = work["alternativeTitles"].as_array() {
        object.insert(
            "alternativeTitles".into(),
            each_of(&first(titles, 15), &["title", "titleType", "language"]),
        );
    }
    if let Some(relations) = work["relations"].as_array() {
        object.insert(
            "relations".into(),
            each_of(
                &first(relations, 15),
                &[
                    "relationType",
                    "title",
                    "medium",
                    "format",
                    "year",
                    "workId",
                ],
            ),
        );
    }
    if work["kind"] == "series" {
        object.insert("seasons".into(), seasons(&work));
    }
    Ok(shown)
}

/// A series' episodes, a season or a page at a time.
pub(super) async fn episodes(api: &Facade, reader: &Identity, args: EpisodesArgs) -> Answer {
    let work = work(api, reader, &args.id, args.language).await?;
    if work["kind"] != "series" {
        return Err(Refusal::Asked(
            "that work is a film: it has no episodes; `item` reads it whole".into(),
        ));
    }
    let all = work["episodes"].as_array().cloned().unwrap_or_default();
    let chosen: Vec<&Value> = all
        .iter()
        .filter(|episode| {
            args.season
                .is_none_or(|season| episode["seasonNumber"].as_i64() == Some(i64::from(season)))
        })
        .collect();
    let offset = args.offset.unwrap_or(0) as usize;
    let limit = within(args.limit, 50, 100) as usize;
    let page: Vec<Value> = chosen
        .iter()
        .skip(offset)
        .take(limit)
        .map(|episode| {
            let mut shown = pick(
                episode,
                &[
                    "seasonNumber",
                    "episodeNumber",
                    "absoluteEpisodeNumber",
                    "title",
                    "airDate",
                    "airDateUtc",
                    "runtime",
                    "overview",
                    "imdbId",
                ],
            );
            if let Some(rating) = episode.get("rating").filter(|r| !r.is_null()) {
                shown["rating"] = rating.clone();
            }
            shown
        })
        .collect();
    let next = offset + page.len();

    let mut answer = json!({
        "work": pick(&work, &["id", "title", "year"]),
        "total": chosen.len(),
        "episodes": page,
    });
    if next < chosen.len() {
        answer["nextOffset"] = json!(next);
    }
    Ok(answer)
}

/// What airs and what comes out, day by day.
pub(super) async fn calendar(api: &Facade, reader: &Identity, args: CalendarArgs) -> Answer {
    let start = match args
        .start
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(day) => chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
            .map_err(|_| Refusal::Asked(format!("start is a day, as 2026-10-19, not {day:?}")))?,
        None => {
            let today = chrono::Utc::now().date_naive();
            today - chrono::Duration::days(i64::from(today.weekday().num_days_from_monday()))
        }
    };
    let days = within(args.days, 7, 31);
    let end = start + chrono::Duration::days(i64::from(days));
    let instant = |day: chrono::NaiveDate| format!("{day}T00:00:00Z");

    let mut asked = vec![("from", instant(start)), ("to", instant(end))];
    if let Some(region) = args.region.filter(|r| !r.trim().is_empty()) {
        asked.push(("region", region.trim().to_string()));
    }
    language(&mut asked, args.language);
    let window = api.get(reader, "/calendar", &asked).await?;

    // Each work once, by its id: what an entry belongs to.
    let works: Map<String, Value> = window["works"]
        .as_array()
        .map(|works| {
            works
                .iter()
                .filter_map(|work| {
                    Some((
                        work["id"].as_str()?.to_string(),
                        pick(work, &["title", "year", "kind", "network"]),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let entries: Vec<Value> = window["entries"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    let work_id = entry["workId"].as_str().unwrap_or_default();
                    let work = works.get(work_id).cloned().unwrap_or(Value::Null);
                    match entry["kind"].as_str() {
                        Some("episode") => {
                            let episode = &entry["episode"];
                            json!({
                                "kind": "episode",
                                "workId": work_id,
                                "work": work,
                                "seasonNumber": episode["seasonNumber"],
                                "episodeNumber": episode["episodeNumber"],
                                "title": episode["title"],
                                "airDateUtc": episode["airDateUtc"],
                                "airDate": episode["airDate"],
                            })
                        }
                        _ => json!({
                            "kind": "release",
                            "workId": work_id,
                            "work": work,
                            "date": entry["date"],
                            "releaseTypes": entry["releaseTypes"],
                        }),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(json!({
        "from": start.to_string(),
        "to": (end - chrono::Duration::days(1)).to_string(),
        "entries": entries,
        "truncated": window["truncated"],
    }))
}

/// The curated lists, or one list's works.
pub(super) async fn lists(api: &Facade, reader: &Identity, args: ListsArgs) -> Answer {
    let Some(key) = args
        .list
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    else {
        let all = api.get(reader, "/lists", &[]).await?;
        return Ok(json!({
            "lists": each(
                &all["lists"],
                &["id", "slug", "name", "description", "kind", "mode", "isPublic", "itemCount"],
            ),
        }));
    };
    let one = api
        .get(reader, &format!("/lists/{}", segment(key)), &[])
        .await?;
    let works = one["items"].as_array().cloned().unwrap_or_default();
    let limit = within(args.limit, 50, 100) as usize;
    Ok(json!({
        "list": pick(&one["list"], &["id", "slug", "name", "description", "kind", "mode", "isPublic"]),
        "total": one["total"],
        "works": each_of(&first(&works, limit), &["id", "kind", "title", "year", "status"]),
    }))
}

/// Where a work's values came from.
pub(super) async fn provenance(api: &Facade, reader: &Identity, args: ItemArgs) -> Answer {
    let id = known_id(&args.id)?;
    let report = api
        .get(reader, &format!("/items/{}/provenance", segment(id)), &[])
        .await?;
    let given = &report["provenance"];

    // Translations and alternative titles are many: counted by who gave
    // them, which is what the question is about.
    let by_source = |map: &Value| -> Value {
        let mut counted: Map<String, Value> = Map::new();
        if let Some(map) = map.as_object() {
            for source in map.values().filter_map(Value::as_str) {
                let n = counted.get(source).and_then(Value::as_u64).unwrap_or(0);
                counted.insert(source.to_string(), json!(n + 1));
            }
        }
        Value::Object(counted)
    };

    Ok(json!({
        "fields": given["fields"],
        "images": given["images"],
        "episodes": given["episodes"],
        "ratings": given["ratings"],
        "translationsBySource": by_source(&given["translations"]),
        "alternativeTitlesBySource": by_source(&given["alternativeTitles"]),
        "sources": each(&report["sources"], &["provider", "fetchedAt", "unavailable", "brings"]),
        "divergingLocks": report["diverging"],
    }))
}

/// How the server is doing.
pub(super) async fn health(api: &Facade, reader: &Identity) -> Answer {
    let health = api.get(reader, "/admin/health", &[]).await?;
    let mut shown = pick(
        &health,
        &[
            "version",
            "uptimeSeconds",
            "database",
            "mode",
            "instance",
            "instances",
            "leader",
            "works",
            "episodes",
            "refreshFailed",
            "updateAvailable",
            "lastBackupAt",
        ],
    );
    shown["sources"] = health["sources"].clone();
    if let Some(jobs) = health["jobs"].as_array() {
        shown["recentJobs"] = each_of(
            &first(jobs, 12),
            &[
                "kind",
                "target",
                "status",
                "startedAt",
                "finishedAt",
                "error",
                "detail",
            ],
        );
    }
    Ok(shown)
}

/// What there is to do.
pub(super) async fn todo(api: &Facade, reader: &Identity, args: TodoArgs) -> Answer {
    let Some(gap) = args.gap else {
        return api.get(reader, "/admin/todo", &[]).await;
    };
    let (filter, value) = gap.filter();
    let listed = api
        .get(
            reader,
            "/items",
            &[
                (filter, value.to_string()),
                // As the administration's register lists them: the works
                // switched off among them.
                ("includeDisabled", "true".into()),
                ("sort", "title".into()),
                ("limit", within(args.limit, 25, 100).to_string()),
            ],
        )
        .await?;
    Ok(json!({
        "total": listed["total"],
        "works": each(&listed["items"], &["id", "kind", "title", "year", "isEnabled"]),
    }))
}

// ─── shaping ─────────────────────────────────────────────────────────────────

/// A work, read whole, or why it is not there.
async fn work(
    api: &Facade,
    reader: &Identity,
    id: &str,
    language_asked: Option<String>,
) -> Result<Value, Refusal> {
    let id = known_id(id)?;
    let mut asked = Vec::new();
    language(&mut asked, language_asked);
    api.get(reader, &format!("/items/{}", segment(id)), &asked)
        .await
        .map_err(|refusal| match refusal {
            Refusal::NotFound(_) => Refusal::NotFound(format!(
                "no work has the id {id:?}: `search` finds a work's id by its title"
            )),
            other => other,
        })
}

/// An id as a work is known by, or a refusal that says how to find one.
fn known_id(id: &str) -> Result<&str, Refusal> {
    let id = id.trim();
    if id.is_empty() || id.len() > 100 {
        return Err(Refusal::Asked(
            "id is a work's id, as `search` gives it".into(),
        ));
    }
    Ok(id)
}

/// The language asked for, when one was: the API checks it.
fn language(asked: &mut Vec<(&str, String)>, language: Option<String>) {
    if let Some(language) = language.filter(|l| !l.trim().is_empty()) {
        asked.push(("language", language.trim().to_string()));
    }
}

/// A count asked for, kept to its bounds: from 1 to `most`, `default` when
/// left out.
fn within(asked: Option<u8>, default: u8, most: u8) -> u8 {
    asked.unwrap_or(default).clamp(1, most)
}

/// Each season of a series: its number, its name, when it began and how
/// many episodes it holds — counted from the episodes, which the answer
/// leaves to `episodes`. A season is one the series lists, or one its
/// episodes are numbered in.
fn seasons(work: &Value) -> Value {
    let episodes = work["episodes"].as_array().cloned().unwrap_or_default();
    let listed = work["seasons"].as_array().cloned().unwrap_or_default();
    let numbers: std::collections::BTreeSet<i64> = listed
        .iter()
        .chain(&episodes)
        .filter_map(|value| value["seasonNumber"].as_i64())
        .collect();
    Value::Array(
        numbers
            .into_iter()
            .map(|number| {
                let held: Vec<&Value> = episodes
                    .iter()
                    .filter(|episode| episode["seasonNumber"].as_i64() == Some(number))
                    .collect();
                let aired: Vec<&str> = held
                    .iter()
                    .filter_map(|episode| episode["airDate"].as_str())
                    .collect();
                let mut shown = listed
                    .iter()
                    .find(|season| season["seasonNumber"].as_i64() == Some(number))
                    .map(|season| pick(season, &["seasonNumber", "title", "airDate"]))
                    .unwrap_or_else(|| json!({"seasonNumber": number}));
                if shown.get("airDate").is_none()
                    && let Some(first) = aired.iter().min()
                {
                    shown["airDate"] = json!(first);
                }
                shown["episodes"] = json!(held.len());
                if let Some(last) = aired.iter().max() {
                    shown["lastAired"] = json!(last);
                }
                shown
            })
            .collect(),
    )
}

/// The fields of an object worth an agent's reading, in the order named:
/// those it has, and has a value for.
fn pick(from: &Value, fields: &[&str]) -> Value {
    let mut shown = Map::new();
    for field in fields {
        if let Some(value) = from.get(*field).filter(|v| !v.is_null()) {
            shown.insert((*field).to_string(), value.clone());
        }
    }
    Value::Object(shown)
}

/// [`pick`], over each object of a list the answer holds; none for none.
fn each(list: &Value, fields: &[&str]) -> Value {
    each_of(
        list.as_array().map(Vec::as_slice).unwrap_or_default(),
        fields,
    )
}

/// [`pick`], over each object of a list.
fn each_of(list: &[Value], fields: &[&str]) -> Value {
    Value::Array(list.iter().map(|value| pick(value, fields)).collect())
}

/// The first `n` of a list.
fn first(list: &[Value], n: usize) -> Vec<Value> {
    list.iter().take(n).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_work_is_cut_to_what_an_agent_reads_it_by() {
        let work = json!({
            "id": "w", "kind": "series", "title": "Show", "year": 2020,
            "images": [{"url": "https://image.example/x.jpg"}],
            "popularity": 12.5, "overview": null,
        });
        assert_eq!(
            pick(&work, &["id", "title", "overview", "images", "absent"]),
            json!({"id": "w", "title": "Show", "images": [{"url": "https://image.example/x.jpg"}]})
        );
        assert_eq!(each(&json!(null), &["id"]), json!([]));
        assert_eq!(
            each(&json!([{"id": 1, "x": 2}, {"y": 3}]), &["id"]),
            json!([{"id": 1}, {}])
        );
    }

    #[test]
    fn a_count_asked_for_keeps_to_its_bounds() {
        assert_eq!(within(None, 8, 20), 8);
        assert_eq!(within(Some(0), 8, 20), 1);
        assert_eq!(within(Some(200), 8, 20), 20);
        assert_eq!(within(Some(5), 8, 20), 5);
    }

    #[test]
    fn a_series_seasons_are_counted_from_its_episodes() {
        let work = json!({
            "seasons": [
                {"seasonNumber": 0, "title": "Specials", "images": []},
                {"seasonNumber": 1, "title": "Season 1", "airDate": "2020-01-01"},
            ],
            "episodes": [
                {"seasonNumber": 1, "episodeNumber": 1, "airDate": "2020-01-01"},
                {"seasonNumber": 1, "episodeNumber": 2, "airDate": "2020-01-08"},
                {"seasonNumber": 0, "episodeNumber": 1},
            ],
        });
        assert_eq!(
            seasons(&work),
            json!([
                {"seasonNumber": 0, "title": "Specials", "episodes": 1},
                {"seasonNumber": 1, "title": "Season 1", "airDate": "2020-01-01",
                 "episodes": 2, "lastAired": "2020-01-08"},
            ])
        );
        // A season its episodes are numbered in, which the series does not
        // list, is one all the same.
        let unlisted = json!({
            "episodes": [
                {"seasonNumber": 2, "episodeNumber": 1, "airDate": "2021-03-01"},
                {"seasonNumber": 2, "episodeNumber": 2, "airDate": "2021-03-08"},
            ],
        });
        assert_eq!(
            seasons(&unlisted),
            json!([{"seasonNumber": 2, "airDate": "2021-03-01", "episodes": 2,
                    "lastAired": "2021-03-08"}])
        );
    }

    #[test]
    fn every_errand_is_a_filter_of_the_catalogues_list() {
        for gap in [
            Gap::Poster,
            Gap::Overview,
            Gap::Imdb,
            Gap::Refresh,
            Gap::DivergingLock,
            Gap::Duplicates,
            Gap::MediaFailed,
        ] {
            let (filter, value) = gap.filter();
            assert_eq!(filter, "missing");
            assert!(
                value.parse::<crate::db::repo::item::Missing>().is_ok(),
                "{value}"
            );
        }
        assert_eq!(Gap::RefreshFailed.filter(), ("refreshFailed", "true"));
    }
}
