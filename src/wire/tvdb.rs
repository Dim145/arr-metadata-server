//! TheTVDB's shape, written from the catalogue: what the TheTVDB relay
//! answers itself for a work held here, where an administrator chose so
//! (`tvdb.relayMode = local-first`), rather than asking TheTVDB.
//!
//! A document is written from the work as the catalogue holds it — its
//! merged values, its locks, its translations, what a person hid taken out —
//! and, for what the catalogue does not keep, from TheTVDB's own record of
//! the work, as it was last fetched (`media_provider_snapshot`): its
//! characters, its companies, its lists and tags, the ids and scores of its
//! pictures, the seasons of its other orders. What neither holds is written
//! empty or `null`, as TheTVDB writes what it has nothing for: nothing is
//! made up.
//!
//! The shape is TheTVDB's as it answers, which is not always what its
//! documentation says: a page of episodes in a language is the series'
//! record with its episodes in it, not `{series, episodes}`; a season or an
//! episode leaves out a name it has none for; a page in a language, and the
//! episodes a series' record carries, give a still of TheTVDB's as a path
//! under `artworks.thetvdb.com`, where every other document gives its
//! address.
//!
//! A record is in its work's own language, as TheTVDB writes one: the name
//! and the overview in the language the work was made in, and the languages
//! it has text in listed beside them. A document in a language says what
//! the catalogue holds in that language and nothing else: where it holds
//! none, a name is `null`, as TheTVDB writes one, never the text of another
//! language. The catalogue's own language is its works' as they are read;
//! another is what a person locked in that language alone, then what a
//! source gave in it — a translation TMDB listed, an episode's text fetched
//! for a client that asked in it before — then TheTVDB's own translation.
//! A text locked in every language is the catalogue's own, as the relay
//! writes it: in another language the work's own text in it stands.
//!
//! A work is known by the id Skyhook hands Sonarr (`ids::for_client`):
//! TheTVDB's own, or one made of TMDB's, of Fankai's or of the catalogue's
//! own. A season or an episode TheTVDB gave no id is given a negative one
//! made of what names it, the same at every answer.

use std::collections::{BTreeSet, HashMap};

use serde_json::{Map, Value, json};

use crate::{
    db::repo::translation::EpisodeText,
    domain::{
        AlternativeTitle, Company, CompanyCredit, CompanyRole, CoverType, Credit, CreditType,
        Episode, Image, ImageDetails, MediaItem, MediaKind, Season, Video, lead_images,
        person::{Character, CharacterCredit},
    },
    providers::lang::{country_name, iso_3166_2_to_3},
    service::{
        ids, language,
        orders::{self, Placed},
    },
};

/// The aired order, as TheTVDB names its season type.
pub const AIRED: &str = "official";

/// How many episodes TheTVDB lays on one page of a series' episodes.
pub const PAGE_SIZE: usize = 500;

/// Where TheTVDB's pictures are: a still on a page in a language is given
/// as its path under it.
const ARTWORKS: &str = "https://artworks.thetvdb.com";

/// TheTVDB's id of a still's type, where its record does not say: a 16:9
/// episode screencap.
const STILL_TYPE: i64 = 11;

/// TheTVDB's id of a season poster's type.
const SEASON_POSTER_TYPE: i64 = 7;

/// TheTVDB's genres, by id, name and slug (`/v4/genres`): what a genre of
/// the catalogue is written as.
const GENRES: &[(i64, &str, &str)] = &[
    (1, "Soap", "soap"),
    (2, "Science Fiction", "science-fiction"),
    (3, "Reality", "reality"),
    (4, "News", "news"),
    (5, "Mini-Series", "mini-series"),
    (6, "Horror", "horror"),
    (7, "Home and Garden", "home-and-garden"),
    (8, "Game Show", "game-show"),
    (9, "Food", "food"),
    (10, "Fantasy", "fantasy"),
    (11, "Family", "family"),
    (12, "Drama", "drama"),
    (13, "Documentary", "documentary"),
    (14, "Crime", "crime"),
    (15, "Comedy", "comedy"),
    (16, "Children", "children"),
    (17, "Animation", "animation"),
    (18, "Adventure", "adventure"),
    (19, "Action", "action"),
    (21, "Sport", "sport"),
    (22, "Suspense", "suspense"),
    (23, "Talk Show", "talk-show"),
    (24, "Thriller", "thriller"),
    (25, "Travel", "travel"),
    (26, "Western", "western"),
    (27, "Anime", "anime"),
    (28, "Romance", "romance"),
    (29, "Musical", "musical"),
    (30, "Podcast", "podcast"),
    (31, "Mystery", "mystery"),
    (32, "Indie", "indie"),
    (33, "History", "history"),
    (34, "War", "war"),
    (35, "Martial Arts", "martial-arts"),
    (36, "Awards Show", "awards-show"),
];

/// The episodes' texts the catalogue holds in languages other than its own,
/// fetched for a client that asked in one: by language, then by the
/// episode's aired season and number.
pub type Held = HashMap<String, HashMap<(i32, i32), EpisodeText>>;

/// A text TheTVDB gives a work, a season or an episode in each language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Text {
    Name,
    Overview,
}

impl Text {
    /// The field the catalogue keeps it in, and locks it by.
    const fn field(self) -> &'static str {
        match self {
            Self::Name => "title",
            Self::Overview => "overview",
        }
    }

    /// Its key in TheTVDB's documents.
    const fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Overview => "overview",
        }
    }

    /// The list of a record that names the languages it is in.
    const fn list(self) -> &'static str {
        match self {
            Self::Name => "nameTranslations",
            Self::Overview => "overviewTranslations",
        }
    }
}

/// What a series' record carries besides itself, as asked: its
/// translations or its episodes (`meta`), and without its people and its
/// pictures (`short`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extended {
    pub translations: bool,
    pub episodes: bool,
    pub short: bool,
}

/// How the episodes of a document are written: in which language, and the
/// way TheTVDB writes them there.
#[derive(Clone, Copy, Debug, Default)]
pub struct Style<'s> {
    /// The language the path names, a page's; `None` for TheTVDB's own
    /// language of a record, the work's original one.
    pub language: Option<&'s str>,
    /// Whether each lists the languages it has text in, as a record of an
    /// episode does; `null` on a page in a language, and in the episodes a
    /// series' record carries.
    pub lists: bool,
    /// Whether a still of TheTVDB's is given as its path, as a page in a
    /// language and a series' record give it.
    pub paths: bool,
    /// Whether the order is the aired one, where a special says where it
    /// airs among the seasons.
    pub aired: bool,
}

/// What a page of episodes was asked to be narrowed to: a season, an episode
/// of it, the day they aired. TheTVDB reads them on a page with no language
/// alone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub season: Option<i32>,
    pub number: Option<i32>,
    pub aired: Option<String>,
}

/// One page of a series' episodes, as asked.
#[derive(Clone, Copy, Debug)]
pub struct PageAsked<'p> {
    pub style: Style<'p>,
    pub filter: &'p Filter,
    /// The page, from 0: every episode is on the first, as TheTVDB lays
    /// five hundred.
    pub page: u64,
    /// Where the page is (`self`), and the one before it (`prev`).
    pub link: &'p str,
    pub previous: Option<&'p str>,
}

/// A work of the catalogue, as a document in TheTVDB's shape is written
/// from it.
pub struct Work<'a> {
    /// As read in the catalogue's own language — its locks laid, what a
    /// person hid taken out — its pictures addressed for clients.
    pub item: &'a MediaItem,
    /// The id it is known by here.
    pub id: i64,
    /// TheTVDB's own record of it, its `data`, as last fetched: what the
    /// catalogue does not keep.
    pub record: Option<&'a Map<String, Value>>,
    /// The language the catalogue keeps its works in, as a lock is filed
    /// under it: `fra`.
    pub stored: &'a str,
    /// Its episodes' texts held in other languages.
    pub held: &'a Held,
    /// A picture's address as its source gave it, for one addressed here:
    /// what TheTVDB's record names it by.
    pub source: &'a (dyn Fn(&str) -> String + Sync),
    /// The language its records are in: the one it was made in, else the
    /// catalogue's own.
    original: String,
    /// TheTVDB's records of its episodes, by their ids.
    episodes: HashMap<i64, &'a Map<String, Value>>,
    /// Today, `YYYY-MM-DD`: what the next episode to air is told by.
    today: String,
}

impl<'a> Work<'a> {
    pub fn new(
        item: &'a MediaItem,
        id: i64,
        record: Option<&'a Map<String, Value>>,
        stored: &'a str,
        held: &'a Held,
        source: &'a (dyn Fn(&str) -> String + Sync),
    ) -> Self {
        let original = original_language(item, record).unwrap_or_else(|| stored.to_string());
        let episodes = record
            .and_then(|record| record.get("episodes"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .filter_map(|episode| Some((episode.get("id")?.as_i64()?, episode)))
            .collect();
        Self {
            item,
            id,
            record,
            stored,
            held,
            source,
            original,
            episodes,
            today: chrono::Utc::now().format("%Y-%m-%d").to_string(),
        }
    }

    /// A field of TheTVDB's record, where it holds one.
    fn recorded(&self, key: &str) -> Option<&'a Value> {
        self.record?.get(key).filter(|value| !value.is_null())
    }

    /// Whether a field of the work itself is locked in the catalogue's own
    /// language, or in every one: `title`, `overview`, `genres`.
    fn locked(&self, field: &str) -> bool {
        self.item.locked_fields.contains(&format!("item/{field}"))
    }

    // ── texts ──────────────────────────────────────────────────────────────

    /// The work's name or overview in `language`, as far as the catalogue
    /// holds it: its own text in its own language; in another, what a
    /// person locked in that language, else a source's translation — the
    /// original title in the original language — else TheTVDB's.
    pub fn text(&self, text: Text, language: &str) -> Option<String> {
        let item = self.item;
        if language == self.stored {
            return match text {
                Text::Name => non_empty(Some(item.title.as_str())),
                Text::Overview => non_empty(item.overview.as_deref()),
            };
        }
        if let Some(locked) = locked_in(item, "item", text.field(), language) {
            return locked;
        }
        item.translations
            .iter()
            .find(|translation| translation.language == language)
            .and_then(|translation| match text {
                Text::Name => non_empty(translation.title.as_deref()),
                Text::Overview => non_empty(translation.overview.as_deref()),
            })
            .or_else(|| {
                (text == Text::Name && language == self.original)
                    .then(|| non_empty(item.original_title.as_deref()))
                    .flatten()
            })
            .or_else(|| self.translated_there(text, language))
    }

    /// TheTVDB's own translation of the work's name or overview in one
    /// language, from its record: none that is an alias.
    fn translated_there(&self, text: Text, language: &str) -> Option<String> {
        self.recorded("translations")?
            .get(text.list())?
            .as_array()?
            .iter()
            .filter(|entry| entry.get("isAlias").and_then(Value::as_bool) != Some(true))
            .find(|entry| {
                entry
                    .get("language")
                    .and_then(Value::as_str)
                    .is_some_and(|code| language::normalize(code) == language)
            })
            .and_then(|entry| non_empty(entry.get(text.key()).and_then(Value::as_str)))
    }

    /// The name its record goes by: in its own language — a title locked
    /// here first, as the relay writes it into TheTVDB's — else TheTVDB's,
    /// else the catalogue's, which every work has.
    pub fn name(&self) -> String {
        if self.locked("title") {
            return self.item.title.clone();
        }
        self.text(Text::Name, &self.original)
            .or_else(|| non_empty(self.recorded("name").and_then(Value::as_str)))
            .unwrap_or_else(|| self.item.title.clone())
    }

    /// The overview its record carries, in its own language: one locked
    /// here first.
    pub fn overview(&self) -> Option<String> {
        if self.locked("overview") {
            return non_empty(self.item.overview.as_deref());
        }
        self.text(Text::Overview, &self.original)
            .or_else(|| non_empty(self.recorded("overview").and_then(Value::as_str)))
    }

    /// The languages the work has a name, or an overview, in.
    pub fn languages(&self, text: Text) -> Vec<String> {
        let item = self.item;
        let mut candidates = vec![self.stored.to_string(), self.original.clone()];
        candidates.extend(item.translations.iter().map(|t| t.language.clone()));
        candidates.extend(
            item.locked_texts
                .iter()
                .filter(|lock| lock.scope == "item" && lock.field == text.field())
                .map(|lock| lock.language.clone()),
        );
        if let Some(entries) = self
            .recorded("translations")
            .and_then(|translations| translations.get(text.list()))
            .and_then(Value::as_array)
        {
            candidates.extend(
                entries
                    .iter()
                    .filter_map(|entry| entry.get("language")?.as_str())
                    .map(language::normalize),
            );
        }
        held_in(candidates, |language| self.text(text, language).is_some())
    }

    /// The translations a record carries when asked for them
    /// (`meta=translations`): every language the work has a name or an
    /// overview in, the original marked primary.
    pub fn translations(&self) -> Value {
        let entries = |text: Text| -> Vec<Value> {
            self.languages(text)
                .into_iter()
                .filter_map(|language| {
                    let said = self.text(text, &language)?;
                    Some(translation_entry(
                        text,
                        said,
                        &language,
                        language == self.original,
                    ))
                })
                .collect()
        };
        json!({
            "nameTranslations": entries(Text::Name),
            "overviewTranslations": entries(Text::Overview),
            "aliases": self
                .recorded("translations")
                .and_then(|translations| translations.get("aliases"))
                .cloned()
                .unwrap_or(Value::Null),
        })
    }

    /// The work's name and overview in one language
    /// (`/translations/{lang}`), where the catalogue holds either: `None`
    /// where it holds neither, which TheTVDB answers with a 404.
    pub fn translation(&self, language: &str) -> Option<Value> {
        let name = self.text(Text::Name, language);
        let overview = self.text(Text::Overview, language);
        if name.is_none() && overview.is_none() {
            return None;
        }
        let mut data = json!({ "name": name, "overview": overview, "language": language });
        if language == self.original {
            data["isPrimary"] = Value::Bool(true);
        }
        Some(data)
    }

    // ── a series ───────────────────────────────────────────────────────────

    /// TheTVDB's base record of the series (`/series/{id}`).
    pub fn series(&self) -> Map<String, Value> {
        let item = self.item;
        let record = |key: &str| self.recorded(key).cloned();
        object(json!({
            "id": self.id,
            "name": self.name(),
            "slug": self.slug(),
            "image": self.poster(),
            "nameTranslations": self.languages(Text::Name),
            "overviewTranslations": self.languages(Text::Overview),
            "aliases": self.aliases(),
            "firstAired": item.first_aired,
            "lastAired": item.last_aired,
            "nextAired": self.next_aired(),
            "score": record("score").unwrap_or(Value::from(0)),
            "status": series_status(item.status.as_deref()),
            "originalCountry": self.country(),
            "originalLanguage": original_language(item, self.record),
            "defaultSeasonType": record("defaultSeasonType").unwrap_or(Value::from(1)),
            "isOrderRandomized": record("isOrderRandomized").unwrap_or(Value::Bool(false)),
            "lastUpdated": stamp(&item.updated_at),
            "averageRuntime": item.runtime,
            "episodes": null,
            "overview": self.overview(),
            "year": item.year.map(|year| year.to_string()),
        }))
    }

    /// The extended record (`/series/{id}/extended`): its base record and
    /// what it carries besides — its pictures and people unless `short`,
    /// its seasons, its translations or its episodes as asked (`meta`).
    /// The episodes are given: those of the series' default order.
    pub fn series_extended(&self, asked: Extended, episodes: Vec<Value>) -> Map<String, Value> {
        let item = self.item;
        let record = |key: &str| self.recorded(key).cloned();
        let mut data = self.series();
        if asked.episodes {
            data.insert("episodes".into(), Value::Array(episodes));
        }
        let (first_network, latest_network) = self.networks();
        let more = json!({
            "artworks": if asked.short { Value::Null } else { Value::Array(self.artworks(true)) },
            "companies": self.companies(),
            "originalNetwork": first_network,
            "latestNetwork": latest_network,
            "genres": self.genres(),
            "trailers": self.trailers(),
            "lists": record("lists").unwrap_or(json!([])),
            "remoteIds": self.remote_ids(),
            "characters": if asked.short { Value::Null } else { Value::Array(self.characters()) },
            "airsDays": record("airsDays").unwrap_or_else(|| json!({
                "sunday": false, "monday": false, "tuesday": false, "wednesday": false,
                "thursday": false, "friday": false, "saturday": false,
            })),
            "airsTime": item.air_time.clone().map(Value::String).or_else(|| record("airsTime")),
            "seasons": self.seasons(),
            "tags": record("tags"),
            "contentRatings": record("contentRatings").unwrap_or(json!([])),
            "seasonTypes": record("seasonTypes").unwrap_or_else(|| json!([aired_type(None)])),
        });
        data.extend(object(more));
        if asked.translations {
            data.insert("translations".into(), self.translations());
        }
        data
    }

    /// The record with its pictures alone (`/series/{id}/artworks`), those
    /// of a language (`lang`) or of a type (`type`) where asked: the lists
    /// it carries otherwise are `null`, as TheTVDB writes them there.
    pub fn series_artworks(&self, language: Option<&str>, kind: Option<i64>) -> Map<String, Value> {
        let mut data = self.series();
        let artworks: Vec<Value> = self
            .artworks(true)
            .into_iter()
            .filter(|artwork| {
                language.is_none_or(|language| {
                    artwork.get("language").and_then(Value::as_str) == Some(language)
                })
            })
            .filter(|artwork| {
                kind.is_none_or(|kind| artwork.get("type").and_then(Value::as_i64) == Some(kind))
            })
            .collect();
        data.extend(object(json!({
            "artworks": artworks,
            "companies": null, "genres": null, "trailers": null, "lists": null,
            "remoteIds": null, "characters": null,
            "airsDays": self.recorded("airsDays").cloned(),
            "airsTime": self.item.air_time,
            "seasons": null, "tags": null, "contentRatings": null,
        })));
        data
    }

    /// The slug TheTVDB addresses the work by, else the catalogue's.
    fn slug(&self) -> String {
        non_empty(self.recorded("slug").and_then(Value::as_str))
            .unwrap_or_else(|| self.item.slug.clone())
    }

    /// The poster the work leads with: the one a person chose, else one
    /// added by hand, else the first the sources gave.
    fn poster(&self) -> Option<String> {
        let posters =
            self.item.images.iter().filter(|image| {
                image.season_number.is_none() && image.cover_type == CoverType::Poster
            });
        lead_images(posters, &self.item.primary_images)
            .first()
            .map(|image| image.url.clone())
    }

    /// The day its next episode airs, `""` when none is still to come, as
    /// TheTVDB writes it.
    fn next_aired(&self) -> String {
        self.item
            .episodes
            .iter()
            .filter_map(|episode| episode.air_date.as_deref())
            .map(|day| day.get(..10).unwrap_or(day))
            .filter(|day| *day >= self.today.as_str())
            .min()
            .unwrap_or_default()
            .to_string()
    }

    /// The country it was made in, as TheTVDB writes one: three letters, in
    /// lower case.
    fn country(&self) -> Option<String> {
        let code = self.item.original_country.as_deref().map(str::trim);
        match code {
            Some(code) if code.len() == 2 => Some(iso_3166_2_to_3(code)),
            Some(code) if code.len() == 3 => Some(code.to_ascii_lowercase()),
            _ => non_empty(self.recorded("originalCountry").and_then(Value::as_str)),
        }
    }

    /// The other names it goes by: TheTVDB's, and those a person added here;
    /// the catalogue's own where TheTVDB's record is not held. What a person
    /// hid is taken out where the document is answered.
    fn aliases(&self) -> Vec<Value> {
        let alias = |title: &AlternativeTitle| {
            json!({
                "language": title.language.as_deref().map(language::normalize).unwrap_or_default(),
                "name": title.title,
            })
        };
        match self.recorded("aliases").and_then(Value::as_array) {
            Some(theirs) => {
                let mut aliases = theirs.clone();
                aliases.extend(
                    self.item
                        .alternative_titles
                        .iter()
                        .filter(|title| title.is_manual)
                        .map(alias),
                );
                aliases
            }
            None => self.item.alternative_titles.iter().map(alias).collect(),
        }
    }

    /// Its genres, as TheTVDB names them: the catalogue's, each one TheTVDB
    /// has, TMDB's pairs split; TheTVDB's own where none of the catalogue's
    /// is one of TheTVDB's and they are not locked.
    fn genres(&self) -> Vec<Value> {
        let mut ids: Vec<i64> = Vec::new();
        for name in &self.item.genres {
            for id in tvdb_genres(name) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        if ids.is_empty()
            && !self.locked("genres")
            && let Some(theirs) = self.recorded("genres").and_then(Value::as_array)
        {
            return theirs.clone();
        }
        ids.into_iter()
            .filter_map(|id| GENRES.iter().find(|genre| genre.0 == id))
            .map(|(id, name, slug)| json!({ "id": id, "name": name, "slug": slug }))
            .collect()
    }

    /// Its trailers: the catalogue's — its trailers and teasers, the one
    /// chosen here first — then TheTVDB's it does not list; TheTVDB's
    /// alone, the chosen one first, for a work whose videos are not kept
    /// yet.
    fn trailers(&self) -> Vec<Value> {
        let theirs: Vec<Value> = self
            .recorded("trailers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let chosen = non_empty(self.item.trailer_youtube_id.as_deref());
        let mut ours: Vec<&Video> = self
            .item
            .videos
            .iter()
            .filter(|video| {
                video
                    .video_type
                    .as_deref()
                    .is_some_and(|kind| matches!(kind, "Trailer" | "Teaser"))
                    && video_url(video).is_some()
            })
            .collect();
        if !ours.is_empty() {
            // The one chosen here first, then as the sources ordered them.
            ours.sort_by_key(|video| {
                (
                    chosen.as_deref() != Some(video.key.as_str()),
                    video.sort_order,
                )
            });
            let mut trailers: Vec<Value> = ours
                .into_iter()
                .filter_map(|video| {
                    let url = video_url(video)?;
                    let listed = theirs.iter().find(|trailer| {
                        trailer.get("url").and_then(Value::as_str) == Some(url.as_str())
                    });
                    Some(json!({
                        "id": listed
                            .and_then(|trailer| trailer.get("id").cloned())
                            .unwrap_or_else(|| Value::from(ids::own_episode(&format!("trailer:{url}")))),
                        "name": non_empty(video.name.as_deref()).unwrap_or_else(|| "Trailer".into()),
                        "url": url,
                        "language": video
                            .language
                            .as_deref()
                            .map(language::normalize)
                            .filter(|language| !language.is_empty()),
                        "runtime": listed
                            .and_then(|trailer| trailer.get("runtime").cloned())
                            .unwrap_or(Value::from(0)),
                    }))
                })
                .collect();
            // One chosen here that no source lists leads all the same.
            if let Some(key) = chosen.as_deref()
                && !self.item.videos.iter().any(|video| video.key == key)
            {
                trailers.insert(0, chosen_trailer(key));
            }
            let kept: Vec<String> = trailers
                .iter()
                .filter_map(|trailer| trailer.get("url")?.as_str().map(str::to_string))
                .collect();
            trailers.extend(theirs.into_iter().filter(|trailer| {
                trailer
                    .get("url")
                    .and_then(Value::as_str)
                    .is_none_or(|url| !kept.iter().any(|kept| kept == url))
            }));
            return trailers;
        }
        let Some(key) = chosen else {
            return theirs;
        };
        let (chosen, others): (Vec<Value>, Vec<Value>) = theirs.into_iter().partition(|trailer| {
            trailer
                .get("url")
                .and_then(Value::as_str)
                .is_some_and(|url| url.contains(&key))
        });
        let mut trailers = if chosen.is_empty() {
            vec![chosen_trailer(&key)]
        } else {
            chosen
        };
        trailers.extend(others);
        trailers
    }

    /// The ids it has elsewhere, as TheTVDB lists them: the catalogue's
    /// own — IMDb's, TMDB's, TVmaze's, Wikidata's — then TheTVDB's for the
    /// sources the catalogue keeps none of: a site, EIDR, an account.
    fn remote_ids(&self) -> Vec<Value> {
        let ids = &self.item.external_ids;
        let tmdb_type = match self.item.kind {
            MediaKind::Series => 12,
            MediaKind::Movie => 10,
        };
        let mut ours: Vec<(String, i64, &str)> = Vec::new();
        if let Some(imdb) = &ids.imdb {
            ours.push((imdb.clone(), 2, "IMDB"));
        }
        if let Some(tmdb) = ids.tmdb {
            ours.push((tmdb.to_string(), tmdb_type, "TheMovieDB.com"));
        }
        if self.item.kind == MediaKind::Series
            && let Some(tvmaze) = ids.tvmaze
        {
            ours.push((tvmaze.to_string(), 19, "TV Maze"));
        }
        if let Some(wikidata) = &ids.wikidata {
            ours.push((wikidata.clone(), 18, "Wikidata"));
        }
        let kinds: Vec<i64> = ours.iter().map(|(_, kind, _)| *kind).collect();
        let mut remote: Vec<Value> = ours
            .into_iter()
            .map(|(id, kind, source)| json!({ "id": id, "type": kind, "sourceName": source }))
            .collect();
        if let Some(theirs) = self.recorded("remoteIds").and_then(Value::as_array) {
            remote.extend(
                theirs
                    .iter()
                    .filter(|entry| {
                        entry
                            .get("type")
                            .and_then(Value::as_i64)
                            .is_none_or(|kind| !kinds.contains(&kind))
                    })
                    .cloned(),
            );
        }
        remote
    }

    /// Its people: the catalogue's characters, each as TheTVDB's record lists
    /// it where it keeps one of that id ([`Self::part`]), then TheTVDB's
    /// other people — its crew, its guests, the parts the catalogue keeps no
    /// character of — then those added here by hand; TheTVDB's own where the
    /// catalogue keeps no character of the work, and its credits where it
    /// holds neither. A person hid is taken out where the document is
    /// answered.
    fn characters(&self) -> Vec<Value> {
        let manual = || {
            self.item
                .credits
                .iter()
                .filter(|credit| credit.is_manual)
                .map(|credit| self.character(credit))
        };
        let theirs = self.recorded("characters").and_then(Value::as_array);
        if self.item.characters.is_empty() {
            return match theirs {
                Some(theirs) => {
                    let mut people = theirs.clone();
                    people.extend(manual());
                    people
                }
                None => self
                    .item
                    .credits
                    .iter()
                    .map(|credit| self.character(credit))
                    .collect(),
            };
        }
        let mut people: Vec<Value> = self
            .item
            .characters
            .iter()
            .filter(|played| self.shown(played))
            .map(|played| Value::Object(self.part(played)))
            .collect();
        let written: BTreeSet<i64> = self
            .item
            .characters
            .iter()
            .filter_map(|played| played.character.tvdb_id)
            .collect();
        people.extend(
            theirs
                .into_iter()
                .flatten()
                .filter(|entry| {
                    entry
                        .get("id")
                        .and_then(Value::as_i64)
                        .is_none_or(|id| !written.contains(&id))
                })
                .cloned(),
        );
        people.extend(manual());
        people
    }

    /// One of the catalogue's characters as TheTVDB lists a character — its
    /// record of the part, where it keeps one of that id, under what the
    /// catalogue says of it: its name, its picture, whether it is one of the
    /// leads, the person playing it — the credit's, else its voice in the
    /// work's own language.
    pub fn part(&self, played: &CharacterCredit) -> Map<String, Value> {
        let character = &played.character;
        let record = character.tvdb_id.and_then(|id| {
            self.recorded("characters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .find(|entry| entry.get("id").and_then(Value::as_i64) == Some(id))
        });
        let credit = played
            .credit_id
            .as_ref()
            .and_then(|id| self.item.credits.iter().find(|credit| credit.id == *id));
        let mut entry = match record {
            Some(record) => record.clone(),
            None => {
                let series = (self.item.kind == MediaKind::Series).then_some(self.id);
                let movie = (self.item.kind == MediaKind::Movie).then_some(self.id);
                let voice = played
                    .voices
                    .iter()
                    .find(|voice| voice.language == self.original)
                    .or_else(|| played.voices.iter().find(|voice| voice.language == "jpn"));
                let person = credit
                    .map(|credit| credit.person_name.clone())
                    .or_else(|| voice.map(|voice| voice.name.clone()));
                let mut entry = Map::new();
                for (key, value) in [
                    ("id", json!(character_id(character))),
                    ("name", json!(character.name)),
                    (
                        "peopleId",
                        json!(credit.and_then(|c| self.item.people.of(c)?.tvdb_id)),
                    ),
                    ("seriesId", json!(series)),
                    ("series", Value::Null),
                    ("movie", Value::Null),
                    ("movieId", json!(movie)),
                    ("episodeId", Value::Null),
                    ("type", json!(3)),
                    ("image", json!(character.image)),
                    ("sort", json!(played.sort_order)),
                    ("isFeatured", json!(false)),
                    ("url", Value::Null),
                    ("nameTranslations", Value::Null),
                    ("overviewTranslations", Value::Null),
                    ("aliases", Value::Null),
                    ("peopleType", json!("Actor")),
                    ("personName", json!(person)),
                    ("tagOptions", Value::Null),
                    (
                        "personImgURL",
                        json!(
                            credit
                                .and_then(|c| c.image.clone())
                                .or_else(|| voice?.image.clone())
                        ),
                    ),
                ] {
                    entry.insert(key.to_string(), value);
                }
                entry
            }
        };
        entry.insert("name".into(), json!(character.name));
        if let Some(image) = &character.image {
            entry.insert("image".into(), json!(image));
        }
        if let Some(role) = played.role {
            entry.insert(
                "isFeatured".into(),
                json!(role == crate::domain::person::CharacterRole::Main),
            );
        }
        if let Some(credit) = credit {
            entry.insert("personName".into(), json!(credit.person_name));
            if let Some(image) = &credit.image {
                entry.insert("personImgURL".into(), json!(image));
            }
        }
        entry
    }

    /// Whether a character is shown: not played by a credit a person hid,
    /// which the work is read without — as TheTVDB's record of the part is
    /// taken out with the person.
    fn shown(&self, played: &CharacterCredit) -> bool {
        played
            .credit_id
            .as_ref()
            .is_none_or(|id| self.item.credits.iter().any(|credit| credit.id == *id))
    }

    /// The record of one of its characters, by this server's id for it:
    /// what TheTVDB answers `characters/{id}` with — the part as the work
    /// lists it, but written as TheTVDB writes a record asked for alone: the
    /// work it is of (`series` or `movie`: its name, picture and year), and
    /// its lists as lists where a work's own list has them null. None for
    /// one it does not list, or played by a person hid.
    pub fn character_record(&self, character_id: &str) -> Option<Map<String, Value>> {
        let mut record = self
            .item
            .characters
            .iter()
            .find(|played| played.character.id == character_id)
            .filter(|played| self.shown(played))
            .map(|played| self.part(played))?;
        let of = match self.item.kind {
            MediaKind::Movie => "movie",
            _ => "series",
        };
        record.insert(
            of.into(),
            json!({
                "name": self.name(),
                "image": self.poster(),
                "year": self.item.year.map(|year| year.to_string()),
            }),
        );
        for list in [
            "nameTranslations",
            "overviewTranslations",
            "aliases",
            "tagOptions",
        ] {
            if record.get(list).is_none_or(Value::is_null) {
                record.insert(list.into(), json!([]));
            }
        }
        Some(record)
    }

    /// One credit of the catalogue's, as TheTVDB lists a character: the
    /// person's TheTVDB id it has none of.
    fn character(&self, credit: &Credit) -> Value {
        let (kind, name) = people_type(credit.credit_type);
        let series = (self.item.kind == MediaKind::Series).then_some(self.id);
        let movie = (self.item.kind == MediaKind::Movie).then_some(self.id);
        let person = credit
            .tmdb_person_id
            .map_or_else(|| credit.person_name.clone(), |id| id.to_string());
        // A role is an actor's or a guest's alone.
        let role = matches!(credit.credit_type, CreditType::Actor | CreditType::Guest)
            .then(|| credit.character_name.clone())
            .flatten();
        json!({
            "id": ids::own_episode(&format!(
                "character:{}:{person}:{}",
                credit.credit_type.as_str(),
                credit.character_name.as_deref().unwrap_or_default()
            )),
            "name": role,
            "peopleId": null,
            "seriesId": series,
            "series": null,
            "movie": null,
            "movieId": movie,
            "episodeId": null,
            "type": kind,
            "image": null,
            "sort": credit.sort_order,
            "isFeatured": false,
            "url": null,
            "nameTranslations": null,
            "overviewTranslations": null,
            "aliases": null,
            "peopleType": name,
            "personName": credit.person_name,
            "tagOptions": null,
            "personImgURL": credit.image,
        })
    }

    /// Its pictures, as TheTVDB lists a series' or a film's — its seasons'
    /// among them: the catalogue's, each with what TheTVDB's record says of
    /// it where the record names it by its address — its id, its score, its
    /// size — and an id made of its address otherwise. A kind TheTVDB has no
    /// type for is left out. A series' are listed `full`, as TheTVDB lists
    /// them, with their thumbnails' sizes and their status; a film's, and a
    /// season's, without.
    fn artworks(&self, full: bool) -> Vec<Value> {
        let theirs: HashMap<&str, &Map<String, Value>> = self
            .recorded("artworks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .filter_map(|artwork| Some((artwork.get("image")?.as_str()?, artwork)))
            .collect();
        let seasons = self
            .item
            .seasons
            .iter()
            .flat_map(|season| season.images.iter());
        self.item
            .images
            .iter()
            .chain(seasons)
            .filter_map(|image| {
                let kind = artwork_type(image.cover_type, image.season_number.is_some())?;
                let source = (self.source)(&image.url);
                let details = self.details(image, &source);
                Some(match theirs.get(source.as_str()) {
                    Some(record) => {
                        let mut artwork = (*record).clone();
                        artwork.insert("image".into(), Value::String(image.url.clone()));
                        if let Some(details) = details {
                            sized(&mut artwork, details);
                        }
                        Value::Object(artwork)
                    }
                    None => artwork(image, kind, &source, full, details),
                })
            })
            .collect()
    }

    /// What the catalogue keeps of a picture beside its address: its size,
    /// TMDB's votes on it.
    fn details(&self, image: &Image, source: &str) -> Option<&'a ImageDetails> {
        let details = &self.item.image_details;
        details
            .get(&image.url)
            .or_else(|| details.get(source))
            .filter(|details| !details.is_empty())
    }

    // ── companies ──────────────────────────────────────────────────────────

    /// The companies a series' record lists: the catalogue's — its
    /// networks, studios, producers and distributors — each as TheTVDB's
    /// record names it where that one lists it by its TheTVDB id; TheTVDB's
    /// own for a work whose companies are not kept yet.
    fn companies(&self) -> Vec<Value> {
        if self.item.companies.is_empty() {
            return self
                .recorded("companies")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
        }
        self.item
            .companies
            .iter()
            .map(|credited| self.company(credited))
            .collect()
    }

    /// A film's companies in one role, as its record groups them.
    fn companies_as(&self, role: CompanyRole) -> Vec<Value> {
        self.item
            .companies
            .iter()
            .filter(|credited| credited.role == role)
            .map(|credited| self.company(credited))
            .collect()
    }

    /// The network a series aired on first and the one it airs on now:
    /// TheTVDB's own picks where the catalogue holds them among its
    /// networks, else its first and its last; TheTVDB's for a work whose
    /// companies are not kept yet.
    fn networks(&self) -> (Option<Value>, Option<Value>) {
        let mut networks: Vec<&CompanyCredit> = self
            .item
            .companies
            .iter()
            .filter(|credited| credited.role == CompanyRole::Network)
            .collect();
        networks.sort_by_key(|credited| credited.sort_order);
        let (Some(first), Some(last)) = (networks.first(), networks.last()) else {
            return (
                self.recorded("originalNetwork").cloned(),
                self.recorded("latestNetwork").cloned(),
            );
        };
        // TheTVDB's own record of the network it names there, under the
        // catalogue's name.
        let pick = |key: &str, otherwise: &CompanyCredit| {
            let theirs = self.recorded(key).and_then(Value::as_object);
            let id = theirs
                .and_then(|network| network.get("id"))
                .and_then(Value::as_i64);
            match (
                theirs,
                networks
                    .iter()
                    .find(|credited| id.is_some() && credited.company.tvdb_id == id),
            ) {
                (Some(theirs), Some(held)) => {
                    let mut entry = theirs.clone();
                    entry.insert("name".into(), Value::String(held.company.name.clone()));
                    Value::Object(entry)
                }
                _ => self.company(otherwise),
            }
        };
        (
            Some(pick("originalNetwork", first)),
            Some(pick("latestNetwork", last)),
        )
    }

    /// One company of the catalogue's, in its role, as TheTVDB writes a
    /// company: TheTVDB's own record of it, its name the catalogue's, where
    /// TheTVDB's record of the work lists it by its id; else made of what
    /// the catalogue keeps — a slug it does not know as `null`.
    fn company(&self, credited: &CompanyCredit) -> Value {
        let company = &credited.company;
        let (type_id, type_name) = company_type(credited.role);
        let kind = json!({ "companyTypeId": type_id, "companyTypeName": type_name });
        if let Some(id) = company.tvdb_id
            && let Some(theirs) = self.listed_company(id)
        {
            let mut entry = theirs.clone();
            entry.insert("name".into(), Value::String(company.name.clone()));
            entry.insert("companyType".into(), kind);
            return Value::Object(entry);
        }
        json!({
            "id": company_id(company),
            "name": company.name,
            "slug": null,
            "nameTranslations": [],
            "overviewTranslations": [],
            "aliases": [],
            "country": company.country.as_deref().map(iso_3166_2_to_3),
            "primaryCompanyType": type_id,
            "activeDate": null,
            "inactiveDate": null,
            "companyType": kind,
            "parentCompany": { "id": null, "name": null, "relation": { "id": null, "typeName": null } },
            "tagOptions": null,
        })
    }

    /// TheTVDB's record of a company its record of the work names: among
    /// its companies and its networks.
    fn listed_company(&self, id: i64) -> Option<&'a Map<String, Value>> {
        ["companies", "originalNetwork", "latestNetwork"]
            .into_iter()
            .filter_map(|key| self.recorded(key))
            .flat_map(|listed| match listed {
                Value::Array(companies) => companies.iter().collect::<Vec<_>>(),
                company => vec![company],
            })
            .filter_map(Value::as_object)
            .find(|company| company.get("id").and_then(Value::as_i64) == Some(id))
    }

    // ── seasons ────────────────────────────────────────────────────────────

    /// The seasons a series' record lists: the catalogue's, of the aired
    /// order, then TheTVDB's of its other orders, from its record.
    pub fn seasons(&self) -> Vec<Value> {
        let mut seasons: Vec<Value> = self
            .item
            .seasons
            .iter()
            .map(|season| Value::Object(self.season(season)))
            .collect();
        if let Some(theirs) = self.recorded("seasons").and_then(Value::as_array) {
            seasons.extend(
                theirs
                    .iter()
                    .filter(|season| {
                        season.pointer("/type/type").and_then(Value::as_str) != Some(AIRED)
                    })
                    .cloned(),
            );
        }
        seasons
    }

    /// The id a season of the aired order goes by: TheTVDB's, else one made
    /// of what names it.
    pub fn season_id(&self, season: &Season) -> i64 {
        season
            .tvdb_id
            .unwrap_or_else(|| own_season_id(&self.item.id, season))
    }

    /// A season's name or overview in `language`: the catalogue's own in
    /// its own language; in another, what a person locked in that one
    /// alone, else the season's translation a source gave in it.
    pub fn season_text(&self, season: &Season, text: Text, language: &str) -> Option<String> {
        if language == self.stored {
            return match text {
                Text::Name => non_empty(season.title.as_deref()),
                Text::Overview => non_empty(season.overview.as_deref()),
            };
        }
        let scope = format!("season:{}", season.season_number);
        if let Some(locked) = locked_in(self.item, &scope, text.field(), language) {
            return locked;
        }
        season
            .translations
            .iter()
            .find(|translation| translation.language == language)
            .and_then(|translation| match text {
                Text::Name => non_empty(translation.title.as_deref()),
                Text::Overview => non_empty(translation.overview.as_deref()),
            })
    }

    /// A season's name, as its record carries it: a title locked here
    /// first, else the one in the work's own language.
    fn season_name(&self, season: &Season) -> Option<String> {
        let locked = format!("season:{}/title", season.season_number);
        if self.item.locked_fields.contains(&locked) {
            return non_empty(season.title.as_deref());
        }
        self.season_text(season, Text::Name, &self.original)
    }

    /// The languages a season has a name, or an overview, in.
    fn season_languages(&self, season: &Season, text: Text) -> Vec<String> {
        let scope = format!("season:{}", season.season_number);
        let mut candidates = vec![self.stored.to_string(), self.original.clone()];
        candidates.extend(season.translations.iter().map(|t| t.language.clone()));
        candidates.extend(
            self.item
                .locked_texts
                .iter()
                .filter(|lock| lock.scope == scope && lock.field == text.field())
                .map(|lock| lock.language.clone()),
        );
        held_in(candidates, |language| {
            self.season_text(season, text, language).is_some()
        })
    }

    /// A season's base record (`/seasons/{id}`), as TheTVDB writes one of
    /// the aired order: its name and its picture where it has them.
    pub fn season(&self, season: &Season) -> Map<String, Value> {
        let mut data = object(json!({
            "id": self.season_id(season),
            "seriesId": self.id,
            "type": aired_type(self.recorded("seasonTypes")),
            "number": season.season_number,
            "nameTranslations": self.season_languages(season, Text::Name),
            "overviewTranslations": self.season_languages(season, Text::Overview),
            "companies": {
                "studio": null, "network": null, "production": null,
                "distributor": null, "special_effects": null,
            },
            "lastUpdated": stamp(&self.item.updated_at),
        }));
        if let Some(name) = self.season_name(season) {
            data.insert("name".into(), Value::String(name));
        }
        if let Some(poster) = season_poster(season) {
            data.insert("image".into(), Value::String(poster.url.clone()));
            data.insert("imageType".into(), Value::from(SEASON_POSTER_TYPE));
        }
        data
    }

    /// A season's extended record (`/seasons/{id}/extended`): its episodes
    /// — given, those the aired order places in it — and its pictures; its
    /// translations where asked.
    pub fn season_extended(
        &self,
        season: &Season,
        episodes: &[Placed<'_>],
        translations: bool,
    ) -> Map<String, Value> {
        let style = Style {
            lists: true,
            aired: true,
            ..Style::default()
        };
        let mut data = self.season(season);
        let artwork: Vec<Value> = season
            .images
            .iter()
            .filter_map(|image| {
                let kind = artwork_type(image.cover_type, true)?;
                let source = (self.source)(&image.url);
                Some(artwork(
                    image,
                    kind,
                    &source,
                    false,
                    self.details(image, &source),
                ))
            })
            .collect();
        data.extend(object(json!({
            "companies": {
                "studio": [], "network": [], "production": [],
                "distributor": [], "special_effects": [],
            },
            "episodes": episodes
                .iter()
                .map(|placed| Value::Object(self.episode(placed, &style)))
                .collect::<Vec<_>>(),
            "trailers": [],
            "artwork": artwork,
            "tagOptions": null,
        })));
        if translations {
            let entries = |text: Text| -> Value {
                let entries: Vec<Value> = self
                    .season_languages(season, text)
                    .into_iter()
                    .filter_map(|language| {
                        let said = self.season_text(season, text, &language)?;
                        Some(translation_entry(
                            text,
                            said,
                            &language,
                            language == self.original,
                        ))
                    })
                    .collect();
                if entries.is_empty() {
                    Value::Null
                } else {
                    Value::Array(entries)
                }
            };
            data.insert(
                "translations".into(),
                json!({
                    "nameTranslations": entries(Text::Name),
                    "overviewTranslations": entries(Text::Overview),
                    "aliases": null,
                }),
            );
        }
        data
    }

    // ── episodes ───────────────────────────────────────────────────────────

    /// The id an episode goes by: TheTVDB's, else one made of what names it.
    pub fn episode_id(episode: &Episode) -> i64 {
        episode
            .tvdb_id
            .unwrap_or_else(|| orders::own_episode_id(episode))
    }

    /// An episode's name or overview in `language`: the catalogue's own in
    /// its own language; in another, what a person locked in that one alone,
    /// else its text fetched in it for a client that asked. An order's own
    /// record of an episode the work does not hold has its own language's
    /// alone.
    pub fn episode_text(
        &self,
        episode: &Episode,
        own: bool,
        text: Text,
        language: &str,
    ) -> Option<String> {
        if language == self.stored {
            return match text {
                Text::Name => non_empty(Some(episode.title.as_str())),
                Text::Overview => non_empty(episode.overview.as_deref()),
            };
        }
        if own {
            return None;
        }
        let scope = format!(
            "episode:{}x{}",
            episode.season_number, episode.episode_number
        );
        if let Some(locked) = locked_in(self.item, &scope, text.field(), language) {
            return locked;
        }
        let held = self
            .held
            .get(language)?
            .get(&(episode.season_number, episode.episode_number))?;
        match text {
            Text::Name => non_empty(held.title.as_deref()),
            Text::Overview => non_empty(held.overview.as_deref()),
        }
    }

    /// The languages an episode has a name, or an overview, in.
    fn episode_languages(&self, episode: &Episode, own: bool, text: Text) -> Vec<String> {
        let scope = format!(
            "episode:{}x{}",
            episode.season_number, episode.episode_number
        );
        let mut candidates = vec![self.stored.to_string(), self.original.clone()];
        candidates.extend(self.held.keys().cloned());
        candidates.extend(
            self.item
                .locked_texts
                .iter()
                .filter(|lock| lock.scope == scope && lock.field == text.field())
                .map(|lock| lock.language.clone()),
        );
        held_in(candidates, |language| {
            self.episode_text(episode, own, text, language).is_some()
        })
    }

    /// One episode where an order places it, as TheTVDB writes an episode's
    /// base record: in the language of `style`, else in the work's own.
    pub fn episode(&self, placed: &Placed<'_>, style: &Style<'_>) -> Map<String, Value> {
        let episode = placed.episode.as_ref();
        let language = style.language.unwrap_or(self.original.as_str());
        let said = |text: Text| self.episode_text(episode, placed.own, text, language);
        let listed = |text: Text| {
            if style.lists {
                json!(self.episode_languages(episode, placed.own, text))
            } else {
                Value::Null
            }
        };
        let theirs = episode.tvdb_id.and_then(|id| self.episodes.get(&id));
        let image = episode.image.as_deref().map(|image| {
            if style.paths {
                image.strip_prefix(ARTWORKS).unwrap_or(image).to_string()
            } else {
                image.to_string()
            }
        });
        let mut data = object(json!({
            "id": Self::episode_id(episode),
            "seriesId": self.id,
            "name": said(Text::Name),
            "aired": episode.air_date,
            "runtime": episode.runtime,
            "nameTranslations": listed(Text::Name),
            "overview": said(Text::Overview),
            "overviewTranslations": listed(Text::Overview),
            "imageType": image.as_ref().map(|_| {
                theirs
                    .and_then(|record| record.get("imageType"))
                    .and_then(Value::as_i64)
                    .unwrap_or(STILL_TYPE)
            }),
            "image": image,
            "isMovie": theirs
                .and_then(|record| record.get("isMovie"))
                .cloned()
                .unwrap_or(Value::from(0)),
            "seasons": null,
            "number": placed.episode_number,
            "absoluteNumber": placed.absolute_number.unwrap_or(0),
            "seasonNumber": placed.season_number,
            "lastUpdated": stamp(&self.item.updated_at),
            "finaleType": episode.finale_type,
        }));
        if let Some(year) = episode.air_date.as_deref().and_then(|day| day.get(..4)) {
            data.insert("year".into(), Value::String(year.to_string()));
        }
        if style.aired {
            for (key, value) in [
                ("airsAfterSeason", episode.aired_after_season_number),
                ("airsBeforeSeason", episode.aired_before_season_number),
                ("airsBeforeEpisode", episode.aired_before_episode_number),
            ] {
                if let Some(value) = value {
                    data.insert(key.into(), Value::from(value));
                }
            }
        }
        if let Some(language) = style.language
            && let Some(season) = self
                .item
                .seasons
                .iter()
                .find(|season| season.season_number == placed.season_number)
            && let Some(name) = self.season_text(season, Text::Name, language)
        {
            data.insert("seasonName".into(), Value::String(name));
        }
        if let Some(movie) = theirs.and_then(|record| record.get("linkedMovie")) {
            data.insert("linkedMovie".into(), movie.clone());
        }
        data
    }

    /// An episode's own record (`/episodes/{id}`): in the work's own
    /// language, with the season of the aired order it is in.
    pub fn episode_record(&self, placed: &Placed<'_>) -> Map<String, Value> {
        let style = Style {
            lists: true,
            aired: true,
            ..Style::default()
        };
        let mut data = self.episode(placed, &style);
        let seasons: Vec<Value> = self
            .item
            .seasons
            .iter()
            .filter(|season| season.season_number == placed.episode.season_number && !placed.own)
            .map(|season| {
                let mut record = self.season(season);
                record.remove("name");
                record.insert("nameTranslations".into(), Value::Null);
                record.insert("overviewTranslations".into(), Value::Null);
                Value::Object(record)
            })
            .collect();
        data.insert("seasons".into(), Value::Array(seasons));
        data
    }

    /// An episode's extended record (`/episodes/{id}/extended`): what
    /// TheTVDB keeps of one episode besides — its people, its ratings, its
    /// codes — the catalogue does not, and is written empty; its
    /// translations where asked.
    pub fn episode_extended(&self, placed: &Placed<'_>, translations: bool) -> Map<String, Value> {
        let mut data = self.episode_record(placed);
        data.extend(object(json!({
            "productionCode": null,
            "nominations": null,
            "characters": [],
            "contentRatings": [],
            "remoteIds": [],
            "tagOptions": [],
            "trailers": [],
            "networks": null,
            "studios": null,
            "companies": [],
            "awards": [],
        })));
        if translations {
            let episode = placed.episode.as_ref();
            let entries = |text: Text| -> Vec<Value> {
                self.episode_languages(episode, placed.own, text)
                    .into_iter()
                    .filter_map(|language| {
                        let said = self.episode_text(episode, placed.own, text, &language)?;
                        Some(translation_entry(
                            text,
                            said,
                            &language,
                            language == self.original,
                        ))
                    })
                    .collect()
            };
            data.insert(
                "translations".into(),
                json!({
                    "nameTranslations": entries(Text::Name),
                    "overviewTranslations": entries(Text::Overview),
                    "aliases": null,
                }),
            );
        }
        data
    }

    /// An episode's name and overview in one language
    /// (`/episodes/{id}/translations/{lang}`): `None` where the catalogue
    /// holds neither.
    pub fn episode_translation(&self, placed: &Placed<'_>, language: &str) -> Option<Value> {
        let episode = placed.episode.as_ref();
        let name = self.episode_text(episode, placed.own, Text::Name, language);
        let overview = self.episode_text(episode, placed.own, Text::Overview, language);
        if name.is_none() && overview.is_none() {
            return None;
        }
        let mut data = json!({ "name": name, "overview": overview, "language": language });
        if language == self.original {
            data["isPrimary"] = Value::Bool(true);
        }
        Some(data)
    }

    /// A page of the series' episodes in one of its orders
    /// (`/series/{id}/episodes/{season-type}[/{lang}]`), placed as the order
    /// places them, every one on the first page. Without a language, the
    /// page names the series by its base record beside its episodes, and is
    /// narrowed as asked; in one, it is the series' base record with its
    /// episodes in it, and is not narrowed — as TheTVDB answers either.
    pub fn episodes_page(&self, placed: &[Placed<'_>], asked: &PageAsked<'_>) -> Value {
        let filter = asked.filter;
        let narrowed: Vec<&Placed<'_>> = placed
            .iter()
            .filter(|placed| {
                asked.style.language.is_some()
                    || (filter
                        .season
                        .is_none_or(|season| placed.season_number == season)
                        && filter
                            .number
                            .is_none_or(|number| placed.episode_number == number)
                        && filter.aired.as_deref().is_none_or(|day| {
                            placed
                                .episode
                                .air_date
                                .as_deref()
                                .and_then(|aired| aired.get(..10))
                                == Some(day)
                        }))
            })
            .collect();
        // TheTVDB counts the whole order where the page is narrowed to a
        // day alone.
        let total = if asked.style.language.is_none()
            && filter.season.is_none()
            && filter.aired.is_some()
        {
            placed.len()
        } else {
            narrowed.len()
        };
        let episodes: Vec<Value> = if asked.page == 0 {
            narrowed
                .iter()
                .map(|placed| Value::Object(self.episode(placed, &asked.style)))
                .collect()
        } else {
            Vec::new()
        };
        let links = json!({
            "prev": asked.previous,
            "self": asked.link,
            "next": null,
            "total_items": total,
            "page_size": PAGE_SIZE.max(episodes.len()),
        });
        let data = match asked.style.language {
            Some(_) => {
                let mut series = self.series();
                series.insert("episodes".into(), Value::Array(episodes));
                Value::Object(series)
            }
            None => json!({ "series": self.series(), "episodes": episodes }),
        };
        json!({ "status": "success", "data": data, "links": links })
    }

    // ── a film ─────────────────────────────────────────────────────────────

    /// TheTVDB's base record of a film (`/movies/{id}`).
    pub fn movie(&self) -> Map<String, Value> {
        let item = self.item;
        object(json!({
            "id": self.id,
            "name": self.name(),
            "slug": self.slug(),
            "image": self.poster(),
            "nameTranslations": self.languages(Text::Name),
            "overviewTranslations": self.languages(Text::Overview),
            "aliases": self.aliases(),
            "score": self.recorded("score").cloned().unwrap_or(Value::from(0)),
            "runtime": item.runtime,
            "status": movie_status(item.status.as_deref()),
            "lastUpdated": stamp(&item.updated_at),
            "year": item.year.map(|year| year.to_string()),
        }))
    }

    /// A film's extended record (`/movies/{id}/extended`): what TheTVDB
    /// keeps of a film the catalogue does not — its takings, its awards, its
    /// companies by TheTVDB's ids — is written empty; its people, pictures
    /// and trailers are left out where `short`.
    pub fn movie_extended(&self, short: bool, translations: bool) -> Map<String, Value> {
        let item = self.item;
        let mut data = self.movie();
        let releases: Vec<Value> = self.releases();
        let first_release = releases
            .iter()
            .min_by(|a, b| a["date"].as_str().cmp(&b["date"].as_str()))
            .cloned();
        data.extend(object(json!({
            "trailers": if short { Value::Null } else { Value::Array(self.trailers()) },
            "genres": self.genres(),
            "releases": releases,
            "artworks": if short { Value::Null } else { Value::Array(self.artworks(false)) },
            "remoteIds": self.remote_ids(),
            "characters": if short { Value::Null } else { Value::Array(self.characters()) },
            "budget": null,
            "boxOffice": null,
            "boxOfficeUS": null,
            "originalCountry": self.country(),
            "originalLanguage": original_language(item, self.record),
            "audioLanguages": null,
            "subtitleLanguages": null,
            "studios": self
                .item
                .companies
                .iter()
                .filter(|credited| credited.role == CompanyRole::Studio)
                .map(|credited| json!({
                    "id": company_id(&credited.company),
                    "name": credited.company.name,
                    "parentStudio": null,
                }))
                .collect::<Vec<_>>(),
            "awards": [],
            "tagOptions": [],
            "lists": [],
            "contentRatings": [],
            "companies": {
                "studio": self.companies_as(CompanyRole::Studio),
                "network": self.companies_as(CompanyRole::Network),
                "production": self.companies_as(CompanyRole::Production),
                "distributor": self.companies_as(CompanyRole::Distributor),
                "special_effects": [],
            },
            "production_countries": item
                .production_countries
                .iter()
                .map(|code| json!({
                    "id": null,
                    "country": iso_3166_2_to_3(code),
                    "name": country_name(code),
                }))
                .collect::<Vec<_>>(),
            "inspirations": [],
            "spoken_languages": [],
            "first_release": first_release,
        })));
        if translations {
            data.insert("translations".into(), self.translations());
        }
        data
    }

    /// When a film came out in each country, the first day of each, as
    /// TheTVDB lists them.
    fn releases(&self) -> Vec<Value> {
        let mut first: Vec<(String, String)> = Vec::new();
        for release in &self.item.releases {
            let Some(date) = release.date.as_deref().and_then(|date| date.get(..10)) else {
                continue;
            };
            let country = iso_3166_2_to_3(&release.country);
            match first.iter_mut().find(|(held, _)| *held == country) {
                Some((_, day)) if date < day.as_str() => *day = date.to_string(),
                Some(_) => {}
                None => first.push((country, date.to_string())),
            }
        }
        first
            .into_iter()
            .map(|(country, date)| json!({ "country": country, "date": date, "detail": null }))
            .collect()
    }

    // ── search ─────────────────────────────────────────────────────────────

    /// The work as TheTVDB lists a result of `/search`: what it has, and no
    /// key for what it has not.
    pub fn search_hit(&self) -> Value {
        let item = self.item;
        let kind = match item.kind {
            MediaKind::Series => "series",
            MediaKind::Movie => "movie",
        };
        let key = format!("{kind}-{}", self.id);
        let name = self.name();
        let mut hit = Map::new();
        let mut put = |key: &str, value: Value| {
            if !value.is_null() {
                hit.insert(key.to_string(), value);
            }
        };
        put("objectID", Value::String(key.clone()));
        put("country", json!(self.country()));
        if item.kind == MediaKind::Movie {
            let director = item
                .credits
                .iter()
                .find(|credit| credit.credit_type == CreditType::Director)
                .map(|credit| credit.person_name.clone());
            put("director", json!(director));
            put(
                "extended_title",
                json!(match item.year {
                    Some(year) => format!("{name} ({year})"),
                    None => name.clone(),
                }),
            );
            put("genres", json!(self.genre_names()));
            let studios: Vec<&str> = item
                .companies
                .iter()
                .filter(|credited| credited.role == CompanyRole::Studio)
                .map(|credited| credited.company.name.as_str())
                .collect();
            if studios.is_empty() {
                put("studios", json!(item.studio.iter().collect::<Vec<_>>()));
            } else {
                put("studios", json!(studios));
            }
        }
        put("id", Value::String(key));
        put("image_url", json!(self.poster()));
        put("name", Value::String(name));
        let first = match item.kind {
            MediaKind::Series => item.first_aired.clone(),
            MediaKind::Movie => item.in_cinemas.clone().or_else(|| {
                self.releases()
                    .iter()
                    .filter_map(|release| release["date"].as_str().map(str::to_string))
                    .min()
            }),
        };
        put("first_air_time", json!(first));
        put("overview", json!(self.overview()));
        put(
            "primary_language",
            json!(original_language(item, self.record)),
        );
        put("primary_type", Value::String(kind.into()));
        let status = match item.kind {
            MediaKind::Series => series_status(item.status.as_deref()),
            MediaKind::Movie => movie_status(item.status.as_deref()),
        };
        put("status", status.get("name").cloned().unwrap_or(Value::Null));
        put("type", Value::String(kind.into()));
        put("tvdb_id", Value::String(self.id.to_string()));
        put("year", json!(item.year.map(|year| year.to_string())));
        put("slug", Value::String(self.slug()));
        let said = |text: Text| -> Value {
            let texts: Map<String, Value> = self
                .languages(text)
                .into_iter()
                .filter_map(|language| {
                    let said = self.text(text, &language)?;
                    Some((language, Value::String(said)))
                })
                .collect();
            if texts.is_empty() {
                Value::Null
            } else {
                Value::Object(texts)
            }
        };
        put("overviews", said(Text::Overview));
        put("translations", said(Text::Name));
        if item.kind == MediaKind::Series {
            put("network", json!(item.network));
        }
        let remote = self.remote_ids();
        if !remote.is_empty() {
            put("remote_ids", Value::Array(remote));
        }
        put("thumbnail", json!(self.poster()));
        let aliases: Vec<Value> = self
            .aliases()
            .into_iter()
            .filter_map(|alias| alias.get("name").cloned())
            .collect();
        if !aliases.is_empty() {
            put("aliases", Value::Array(aliases));
        }
        Value::Object(hit)
    }

    /// The work as `/search/remoteid/{id}` lists it: its base record, under
    /// its kind.
    pub fn remote_hit(&self) -> Value {
        match self.item.kind {
            MediaKind::Series => json!({ "series": self.series() }),
            MediaKind::Movie => json!({ "movie": self.movie() }),
        }
    }

    /// The names of its genres TheTVDB has, as a search result lists them.
    fn genre_names(&self) -> Vec<Value> {
        self.genres()
            .into_iter()
            .filter_map(|genre| genre.get("name").cloned())
            .collect()
    }
}

/// TheTVDB's answer where it holds nothing by that name or in that
/// language, as it writes it.
pub fn not_found() -> Value {
    json!({
        "status": "failure",
        "message": "NotFoundException: error fetching translation",
        "data": null,
    })
}

/// TheTVDB's answer to a page of episodes narrowed to a number with no
/// season, as it writes it.
pub fn no_season() -> Value {
    json!({ "status": "failure", "message": "empty param season", "data": null })
}

/// A document as TheTVDB answers one.
pub fn success(data: impl Into<Value>) -> Value {
    json!({ "status": "success", "data": data.into() })
}

/// The id a season of the aired order TheTVDB gave none is given: made of
/// TMDB's id for it, else of the work's and its number. Negative, and never
/// one of TheTVDB's.
pub fn own_season_id(work: &str, season: &Season) -> i64 {
    match season.tmdb_id {
        Some(tmdb) => ids::own_episode(&format!("season:tmdb:{tmdb}")),
        None => ids::own_episode(&format!("season:{work}:{}", season.season_number)),
    }
}

/// The aired order's season type, as the series' record names it, else as
/// TheTVDB names it for every series.
fn aired_type(listed: Option<&Value>) -> Value {
    listed
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|kind| kind.get("type").and_then(Value::as_str) == Some(AIRED))
        .cloned()
        .unwrap_or_else(
            || json!({ "id": 1, "name": "Aired Order", "type": AIRED, "alternateName": null }),
        )
}

/// The language a work was made in, as TheTVDB writes it: the catalogue's,
/// else TheTVDB's record's.
fn original_language(item: &MediaItem, record: Option<&Map<String, Value>>) -> Option<String> {
    item.original_language
        .as_deref()
        .map(language::normalize)
        .filter(|language| !language.is_empty())
        .or_else(|| {
            record?
                .get("originalLanguage")?
                .as_str()
                .map(language::normalize)
                .filter(|language| !language.is_empty())
        })
}

/// A text locked in one language alone: `Some` with its text — `Some(None)`
/// where it was locked cleared — or `None` where nothing was locked in it.
fn locked_in(item: &MediaItem, scope: &str, field: &str, language: &str) -> Option<Option<String>> {
    item.locked_texts
        .iter()
        .find(|lock| lock.scope == scope && lock.field == field && lock.language == language)
        .map(|lock| non_empty(lock.value.as_deref()))
}

/// The languages among `candidates` that `holds` a text in, each once, in
/// order of their codes.
fn held_in(candidates: Vec<String>, holds: impl Fn(&str) -> bool) -> Vec<String> {
    candidates
        .into_iter()
        .filter(|language| !language.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|language| holds(language))
        .collect()
}

/// An entry of a record's translations, as TheTVDB lists one: the original
/// language's marked primary.
fn translation_entry(text: Text, said: String, language: &str, primary: bool) -> Value {
    let mut entry = Map::new();
    entry.insert(text.key().into(), Value::String(said));
    entry.insert("language".into(), Value::String(language.to_string()));
    if primary {
        entry.insert("isPrimary".into(), Value::Bool(true));
    }
    Value::Object(entry)
}

/// A picture of the catalogue's TheTVDB's record does not name, as TheTVDB
/// lists a work's picture: an id made of its address; its size and score
/// from what the catalogue keeps of it ([`sized`]); a size, a score and a
/// date it does not know as nought, as TheTVDB writes them. `full` for a
/// series', with its thumbnail's size and its status.
fn artwork(
    image: &Image,
    kind: i64,
    source: &str,
    full: bool,
    details: Option<&ImageDetails>,
) -> Value {
    let language = image
        .language
        .as_deref()
        .map(language::normalize)
        .filter(|language| !language.is_empty());
    let mut artwork = object(json!({
        "id": ids::own_episode(&format!("artwork:{source}")),
        "image": image.url,
        "thumbnail": image.url,
        "includesText": language.is_some(),
        "language": language,
        "type": kind,
        "score": 0,
        "width": 0,
        "height": 0,
    }));
    if full {
        artwork.extend(object(json!({
            "thumbnailWidth": 0,
            "thumbnailHeight": 0,
            "updatedAt": 0,
            "status": { "id": 0, "name": null },
            "tagOptions": null,
        })));
    }
    if let Some(details) = details {
        sized(&mut artwork, details);
    }
    Value::Object(artwork)
}

/// A picture's size, and its score, from what the catalogue keeps of it:
/// TMDB's votes, its average a thousand times over, an integer as TheTVDB
/// writes a score — TheTVDB asks that no meaning be read into it, and a
/// client that sorts pictures by it sorts them as TMDB ranks them.
fn sized(artwork: &mut Map<String, Value>, details: &ImageDetails) {
    if let Some(width) = details.width {
        artwork.insert("width".into(), Value::from(width));
    }
    if let Some(height) = details.height {
        artwork.insert("height".into(), Value::from(height));
    }
    if let Some(average) = details.vote_average.filter(|average| average.is_finite()) {
        artwork.insert(
            "score".into(),
            Value::from((average * 1000.0).round() as i64),
        );
    }
}

/// The id a company TheTVDB gave none goes by: made of TMDB's id for it,
/// else of its name. Negative, never one of TheTVDB's.
fn company_id(company: &Company) -> i64 {
    company.tvdb_id.unwrap_or_else(|| match company.tmdb_id {
        Some(tmdb) => ids::own_episode(&format!("company:tmdb:{tmdb}")),
        None => ids::own_episode(&format!("company:{}", company.name.trim().to_lowercase())),
    })
}

/// TheTVDB's type of a company in a role (`/companies/types`).
fn company_type(role: CompanyRole) -> (i64, &'static str) {
    match role {
        CompanyRole::Network => (1, "Network"),
        CompanyRole::Studio => (2, "Studio"),
        CompanyRole::Production => (3, "Production Company"),
        CompanyRole::Distributor => (4, "Distributor"),
    }
}

/// The trailer chosen here, by its YouTube id, where no source lists it.
fn chosen_trailer(key: &str) -> Value {
    json!({
        "id": ids::own_episode(&format!("trailer:youtube:{key}")),
        "name": "Trailer",
        "url": format!("https://www.youtube.com/watch?v={key}"),
        "language": null,
        "runtime": 0,
    })
}

/// Where a video is watched, for the sites TheTVDB links trailers on.
fn video_url(video: &Video) -> Option<String> {
    let key = video.key.trim();
    if key.is_empty() {
        return None;
    }
    match video.site.trim().to_ascii_lowercase().as_str() {
        "youtube" => Some(format!("https://www.youtube.com/watch?v={key}")),
        "vimeo" => Some(format!("https://vimeo.com/{key}")),
        _ => None,
    }
}

/// TheTVDB's type of a picture of a kind, a season's or the work's; `None`
/// for a kind it has none for.
fn artwork_type(kind: CoverType, of_season: bool) -> Option<i64> {
    match (kind, of_season) {
        (CoverType::Banner, false) => Some(1),
        (CoverType::Poster, false) => Some(2),
        (CoverType::Fanart, false) => Some(3),
        (CoverType::Clearart, false) => Some(22),
        (CoverType::Clearlogo, false) => Some(23),
        (CoverType::Banner, true) => Some(6),
        (CoverType::Poster, true) => Some(SEASON_POSTER_TYPE),
        (CoverType::Fanart, true) => Some(8),
        _ => None,
    }
}

/// The poster a season leads with: the one a person chose, else one added
/// by hand, else the first.
fn season_poster(season: &Season) -> Option<&Image> {
    let posters = season
        .images
        .iter()
        .filter(|image| image.cover_type == CoverType::Poster);
    lead_images(posters, &season.primary_images)
        .first()
        .copied()
}

/// The id TheTVDB's dialect knows a character of the catalogue by:
/// TheTVDB's own id for its record of the part, else one made of this
/// server's id for the character — negative, so never one of TheTVDB's, and
/// the same at every answer.
pub fn character_id(character: &Character) -> i64 {
    character
        .tvdb_id
        .unwrap_or_else(|| ids::own_episode(&format!("character:{}", character.id)))
}

/// TheTVDB's type of a credit of the catalogue's, by number and by name.
fn people_type(kind: CreditType) -> (i64, &'static str) {
    match kind {
        CreditType::Director => (1, "Director"),
        CreditType::Writer => (2, "Writer"),
        CreditType::Actor => (3, "Actor"),
        CreditType::Guest => (4, "Guest Star"),
        CreditType::Crew => (5, "Crew"),
        CreditType::Producer => (7, "Producer"),
    }
}

/// A series' status in TheTVDB's words (`/series/statuses`): ended, as
/// TheTVDB marks it, is kept updated no more.
fn series_status(status: Option<&str>) -> Value {
    let (id, name) = match status {
        Some("continuing") => (1, "Continuing"),
        Some("ended") => (2, "Ended"),
        Some("upcoming") => (3, "Upcoming"),
        _ => {
            return json!({ "id": null, "name": null, "recordType": "series", "keepUpdated": false });
        }
    };
    json!({ "id": id, "name": name, "recordType": "series", "keepUpdated": id != 2 })
}

/// A film's status in TheTVDB's words (`/movies/statuses`).
fn movie_status(status: Option<&str>) -> Value {
    let (id, name) = match status {
        Some("announced" | "tba") => (1, "Announced"),
        Some("inCinemas" | "released") => (5, "Released"),
        _ => {
            return json!({ "id": null, "name": null, "recordType": "movie", "keepUpdated": false });
        }
    };
    json!({ "id": id, "name": name, "recordType": "movie", "keepUpdated": true })
}

/// TheTVDB's genres a genre of the catalogue's is: one of the same name,
/// or each of a pair TMDB names together.
fn tvdb_genres(name: &str) -> Vec<i64> {
    let by_name = |name: &str| {
        GENRES
            .iter()
            .find(|genre| genre.1.eq_ignore_ascii_case(name.trim()))
            .map(|genre| genre.0)
    };
    match name.trim() {
        "Sci-Fi & Fantasy" => vec![2, 10],
        "Action & Adventure" => vec![19, 18],
        "War & Politics" => vec![34],
        "Kids" => vec![16],
        "Talk" => vec![23],
        "Music" => vec![29],
        other => by_name(other).into_iter().collect(),
    }
}

/// An instant as TheTVDB writes one: `2026-07-13 01:23:56`.
fn stamp(at: &str) -> String {
    crate::db::parse_rfc3339(at).map_or_else(
        || at.to_string(),
        |at| at.format("%Y-%m-%d %H:%M:%S").to_string(),
    )
}

fn non_empty(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// The map a `json!` object literal makes.
fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// The documents written here against TheTVDB's own for the same request,
/// one test a request a client makes (notes C, §2.2): fetched from TheTVDB
/// for *Covington Cross* and *The Matrix* on the 1st of November 2026, and
/// cut down (`fixtures/tvdb/`). TheTVDB's record of *Covington Cross* is the
/// one the catalogue kept of it (`snapshot.json`).
#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        db::repo::child::{blank_episode, blank_season},
        domain::{CountryRelease, Image, Translation},
        wire::contract::{GapKind, assert_same_shape, gaps},
    };

    /// A document TheTVDB answered, cut down.
    macro_rules! fixture {
        ($name:literal) => {
            serde_json::from_str::<Value>(include_str!(concat!("fixtures/tvdb/", $name, ".json")))
                .expect("a fixture")
        };
    }

    /// The catalogue keeps its works in French.
    const STORED: &str = "fra";

    /// A picture as its source gave it: the address it is served at.
    fn same(url: &str) -> String {
        url.to_string()
    }

    fn image(cover: &str, url: &str, language: Option<&str>, season: Option<i32>) -> Image {
        serde_json::from_value(json!({
            "id": crate::db::new_id(), "seasonNumber": season, "coverType": cover, "url": url,
            "language": language, "sortOrder": 0, "source": "tvdb", "isManual": false,
        }))
        .expect("an image")
    }

    fn credit(kind: &str, person: &str, role: Option<&str>, tmdb: i64) -> Credit {
        serde_json::from_value(json!({
            "id": crate::db::new_id(), "creditType": kind, "personName": person,
            "characterName": role, "tmdbPersonId": tmdb, "sortOrder": 0, "isManual": false,
        }))
        .expect("a credit")
    }

    /// A company as the catalogue credits it.
    fn company(role: &str, name: &str, tvdb: Option<i64>, tmdb: i64) -> CompanyCredit {
        let kind = if role == "network" {
            "network"
        } else {
            "company"
        };
        serde_json::from_value(json!({
            "company": { "kind": kind, "name": name, "tvdbId": tvdb, "tmdbId": tmdb, "country": "US" },
            "role": role, "sortOrder": 0, "source": "tmdb",
        }))
        .expect("a company")
    }

    fn translation(language: &str, title: Option<&str>, overview: Option<&str>) -> Translation {
        serde_json::from_value(json!({
            "language": language, "title": title, "overview": overview, "isManual": false,
        }))
        .expect("a translation")
    }

    /// *Covington Cross* as the catalogue holds it: in French, TMDB's
    /// English overview beside it.
    fn covington() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.id = "01a0db2c-d484-723f-bc68-3b1494a1d272".into();
        item.title = "Covington Cross".into();
        item.original_title = Some("Covington Cross".into());
        item.overview = Some("Les aventures de Thomas Grey …".into());
        item.original_language = Some("en".into());
        item.original_country = Some("US".into());
        item.status = Some("ended".into());
        item.runtime = Some(60);
        item.year = Some(1992);
        item.first_aired = Some("1992-08-25".into());
        item.last_aired = Some("1992-12-11".into());
        item.network = Some("ABC".into());
        item.genres = vec!["Drama".into(), "Action & Adventure".into()];
        item.slug = "covington-cross-1992".into();
        item.external_ids.tvdb = Some(70753);
        item.external_ids.tmdb = Some(9721);
        item.external_ids.imdb = Some("tt0103391".into());
        item.translations = vec![translation(
            "eng",
            None,
            Some("Covington Cross is a British …"),
        )];
        item.images = vec![
            image(
                "poster",
                "https://artworks.thetvdb.com/banners/posters/70753-2.jpg",
                Some("eng"),
                None,
            ),
            image(
                "fanart",
                "https://artworks.thetvdb.com/banners/fanart/original/70753-1.jpg",
                None,
                None,
            ),
        ];
        let mut specials = blank_season(0);
        specials.is_manual = false;
        specials.title = Some("Épisodes spéciaux".into());
        specials.tvdb_id = Some(18221);
        specials.tmdb_id = Some(19531);
        let mut first = blank_season(1);
        first.is_manual = false;
        first.title = Some("Saison 1".into());
        first.tvdb_id = Some(963);
        first.tmdb_id = Some(19530);
        first.images = vec![image(
            "poster",
            "https://artworks.thetvdb.com/banners/seasons/70753-1.jpg",
            Some("eng"),
            Some(1),
        )];
        item.seasons = vec![specials, first];
        let episode = |season, number, tvdb: i64, title: &str, aired: Option<&str>| {
            let mut episode = blank_episode(season, number);
            episode.is_manual = false;
            episode.tvdb_id = Some(tvdb);
            episode.title = title.into();
            episode.overview = Some(format!("{title}, en français …"));
            episode.air_date = aired.map(str::to_string);
            episode.runtime = Some(45);
            episode.image = Some(format!(
                "https://artworks.thetvdb.com/banners/episodes/70753/{tvdb}.jpg"
            ));
            episode
        };
        item.episodes = vec![
            episode(0, 1, 422_700, "Dossier de presse", None),
            episode(1, 1, 20492, "Le pilote", Some("1992-08-25")),
            episode(1, 2, 20493, "Le retour d'Armus", Some("1992-09-01")),
        ];
        item.credits = vec![credit("actor", "Glenn Quinn", Some("Cedric Grey"), 80292)];
        // ABC, which TheTVDB's record lists by its id; a producer it does
        // not.
        item.companies = vec![
            company("network", "ABC", Some(5), 2),
            company("production", "Columbia Pictures Television", None, 1234),
        ];
        // TMDB's poster, with its size and its votes; TMDB's trailer.
        let tmdb_poster = "https://image.tmdb.org/t/p/original/pD6xJ6z2RWxSXTNNlGJaGe9hkhU.jpg";
        item.images
            .push(image("poster", tmdb_poster, Some("eng"), None));
        item.image_details.insert(
            tmdb_poster.into(),
            serde_json::from_value(json!({
                "width": 2000, "height": 3000, "voteAverage": 5.312, "voteCount": 4,
            }))
            .expect("details"),
        );
        item.videos = vec![
            serde_json::from_value(json!({
                "site": "YouTube", "key": "cvRWgB0dHEk", "name": "Covington Cross Trailer",
                "videoType": "Trailer", "language": "eng", "official": true, "sortOrder": 0,
            }))
            .expect("a video"),
        ];
        item
    }

    /// TheTVDB's record of *Covington Cross*, as the catalogue kept it.
    fn record() -> Map<String, Value> {
        object(fixture!("snapshot")["data"].clone())
    }

    /// The episodes' English text, fetched for a client that asked in
    /// English.
    fn english() -> Held {
        let text = |season, number, title: &str| {
            (
                (season, number),
                EpisodeText {
                    season_number: season,
                    episode_number: number,
                    title: Some(title.into()),
                    overview: Some(format!("{title}, in English …")),
                },
            )
        };
        Held::from([(
            "eng".to_string(),
            HashMap::from([
                text(0, 1, "Video Press Kit"),
                text(1, 1, "Pilot"),
                text(1, 2, "Armus Returns"),
            ]),
        )])
    }

    /// What TheTVDB marks only on the original language's entry, which it
    /// lists among the others, not first.
    const PRIMARY: &[&str] = &[
        "translations.nameTranslations[].isPrimary",
        "translations.overviewTranslations[].isPrimary",
    ];

    /// What TheTVDB writes of a season only where it has it: a name, a
    /// picture; and a company's slug, which is TheTVDB's and is not known
    /// for one its record of the work does not list.
    const SEASONS: &[&str] = &[
        "seasons[].name",
        "seasons[].image",
        "seasons[].imageType",
        "companies[].slug",
    ];

    /// What *Covington Cross*'s documents are written from: the work,
    /// TheTVDB's record of it, its episodes' English text.
    struct Covington {
        item: MediaItem,
        record: Map<String, Value>,
        held: Held,
    }

    impl Covington {
        fn new() -> Self {
            Self {
                item: covington(),
                record: record(),
                held: english(),
            }
        }

        fn work(&self) -> Work<'_> {
            Work::new(
                &self.item,
                70753,
                Some(&self.record),
                STORED,
                &self.held,
                &same,
            )
        }
    }

    /// `series/{id}`: Jellyfin, tinyMediaManager.
    #[test]
    fn the_series_record_jellyfin_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        assert_same_shape(
            &fixture!("series")["data"],
            &Value::Object(covington.work().series()),
            &[],
        );
    }

    /// The characters the catalogue keeps, listed as TheTVDB lists a
    /// series' people: its record of each part under what the catalogue
    /// says of it, one it has no record of under an id made here, then
    /// TheTVDB's other people; and each answered alone (`characters/{id}`).
    #[test]
    fn the_characters_kept_are_listed_as_thetvdb_lists_a_series_people() {
        use crate::domain::person::{CharacterRole, CharacterVoice};

        let mut covington = Covington::new();
        let cedric = credit("actor", "Glenn Quinn", Some("Cedric Grey"), 1);
        covington.item.credits.push(cedric.clone());
        let character = |id: &str, name: &str, tvdb: Option<i64>| Character {
            id: id.into(),
            name: name.into(),
            native_name: None,
            image: Some(format!("https://example.test/{id}.png")),
            tvdb_id: tvdb,
            anilist_id: None,
            mal_id: None,
        };
        covington.item.characters = vec![
            CharacterCredit {
                character: character("c-1", "Cedric Grey, as kept", Some(63_495_072)),
                role: Some(CharacterRole::Main),
                sort_order: 0,
                credit_id: Some(cedric.id.clone()),
                voices: Vec::new(),
                source: Some("tvdb".into()),
            },
            CharacterCredit {
                character: character("c-2", "A Part Of AniList's", None),
                role: Some(CharacterRole::Supporting),
                sort_order: 1,
                credit_id: None,
                voices: vec![CharacterVoice {
                    language: "eng".into(),
                    name: "A Voice".into(),
                    native_name: None,
                    image: Some("https://example.test/voice.png".into()),
                    anilist_id: Some(1),
                    mal_id: None,
                    sort_order: 0,
                }],
                source: Some("anilist".into()),
            },
        ];
        let work = covington.work();
        let people = work.characters();
        let theirs = &covington.record["characters"][0];

        let ids: Vec<i64> = people.iter().filter_map(|p| p["id"].as_i64()).collect();
        let made = character_id(&covington.item.characters[1].character);
        assert!(made < 0, "never one of TheTVDB's");
        assert_eq!(ids, [63_495_072, made, 63_495_077, 63_495_081]);
        // What TheTVDB keeps of a person the catalogue does not, for one it
        // has no record of: the person's id and page.
        for person in &people {
            assert_same_shape(
                theirs,
                person,
                &["image", "personImgURL", "peopleId", "url"],
            );
        }
        // TheTVDB's record of the part, under the catalogue's word.
        assert_eq!(people[0]["name"], json!("Cedric Grey, as kept"));
        assert_eq!(people[0]["url"], theirs["url"]);
        assert_eq!(people[0]["isFeatured"], json!(true));
        assert_eq!(people[0]["image"], json!("https://example.test/c-1.png"));
        // One TheTVDB has no record of: played by its voice in the work's
        // own language.
        assert_eq!(people[1]["personName"], json!("A Voice"));
        assert_eq!(
            people[1]["personImgURL"],
            json!("https://example.test/voice.png")
        );
        assert_eq!(people[1]["seriesId"], json!(70753));
        assert_eq!(people[1]["isFeatured"], json!(false));

        // Asked for alone, as TheTVDB writes such a record (read on 3
        // November 2026): the work it is of, and its lists as lists.
        let alone = work.character_record("c-2").expect("listed");
        let series = work.series();
        assert_eq!(alone["series"]["name"], series["name"]);
        assert_eq!(alone["series"]["image"], series["image"]);
        assert_eq!(alone["series"]["year"], series["year"]);
        assert_eq!(alone["movie"], Value::Null);
        for list in [
            "nameTranslations",
            "overviewTranslations",
            "aliases",
            "tagOptions",
        ] {
            assert_eq!(alone[list], json!([]), "{list}");
        }
        for (key, value) in people[1].as_object().expect("a record") {
            if !matches!(
                key.as_str(),
                "series" | "nameTranslations" | "overviewTranslations" | "aliases" | "tagOptions"
            ) {
                assert_eq!(&alone[key.as_str()], value, "{key}");
            }
        }
        assert!(work.character_record("nobody").is_none());
    }

    /// `series/{id}/extended`: Kodi, Yamtrack.
    #[test]
    fn the_extended_record_kodi_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        assert_same_shape(
            &fixture!("series-extended")["data"],
            &Value::Object(
                covington
                    .work()
                    .series_extended(Extended::default(), Vec::new()),
            ),
            SEASONS,
        );
    }

    /// `series/{id}/extended?meta=translations&short=false`: Jellyfin,
    /// SickGear.
    #[test]
    fn the_extended_record_with_translations_jellyfin_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        let asked = Extended {
            translations: true,
            ..Extended::default()
        };
        assert_same_shape(
            &fixture!("series-extended-translations")["data"],
            &Value::Object(covington.work().series_extended(asked, Vec::new())),
            &[SEASONS, PRIMARY].concat(),
        );
    }

    /// `series/{id}/extended?meta=translations&short=true`: Jellyfin.
    #[test]
    fn the_short_record_with_translations_jellyfin_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        let asked = Extended {
            translations: true,
            short: true,
            ..Extended::default()
        };
        assert_same_shape(
            &fixture!("series-extended-translations-short")["data"],
            &Value::Object(covington.work().series_extended(asked, Vec::new())),
            &[SEASONS, PRIMARY].concat(),
        );
    }

    /// `series/{id}/extended?meta=episodes&short=true`: Seerr. An episode
    /// that never aired has no year.
    #[test]
    fn the_short_record_with_episodes_seerr_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        let work = covington.work();
        let style = Style {
            paths: true,
            aired: true,
            ..Style::default()
        };
        let episodes = orders::aired_placements(&covington.item)
            .iter()
            .map(|placed| Value::Object(work.episode(placed, &style)))
            .collect();
        let asked = Extended {
            episodes: true,
            short: true,
            ..Extended::default()
        };
        assert_same_shape(
            &fixture!("series-extended-episodes-short")["data"],
            &Value::Object(work.series_extended(asked, episodes)),
            &[SEASONS, &["episodes[].year"]].concat(),
        );
    }

    /// `series/{id}/artworks`: Jellyfin.
    #[test]
    fn the_pictures_jellyfin_reads_have_thetvdbs_shape() {
        let covington = Covington::new();
        assert_same_shape(
            &fixture!("series-artworks")["data"],
            &Value::Object(covington.work().series_artworks(None, None)),
            &[],
        );
    }

    /// `series/{id}/translations/{lang}`: Kodi, tinyMediaManager.
    #[test]
    fn the_series_text_kodi_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        assert_same_shape(
            &fixture!("series-translation")["data"],
            &covington
                .work()
                .translation("eng")
                .expect("English is held"),
            &[],
        );
    }

    /// One page of *Covington Cross*'s aired episodes, the first.
    fn first_page(covington: &Covington, language: Option<&str>) -> Value {
        let filter = Filter::default();
        covington.work().episodes_page(
            &orders::aired_placements(&covington.item),
            &PageAsked {
                style: Style {
                    language,
                    lists: language.is_none(),
                    paths: language.is_some(),
                    aired: true,
                },
                filter: &filter,
                page: 0,
                link: "/v4/series/70753/episodes/official?page=0",
                previous: None,
            },
        )
    }

    /// `series/{id}/episodes/{season-type}?page=0`: Jellyfin, Kodi,
    /// SickGear. Every episode on the first page.
    #[test]
    fn the_page_of_episodes_jellyfin_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        let page = first_page(&covington, None);
        assert_same_shape(
            &fixture!("episodes-official"),
            &page,
            &["data.episodes[].year"],
        );
        assert_eq!(page["links"]["total_items"], json!(3));
        assert_eq!(page["links"]["next"], Value::Null);
    }

    /// `series/{id}/episodes/default/{lang}?page=N`: Seerr, which takes a
    /// still that is a path under TheTVDB's pictures, and drops an address.
    #[test]
    fn the_page_in_a_language_seerr_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        let page = first_page(&covington, Some("eng"));
        assert_same_shape(
            &fixture!("episodes-default-eng"),
            &page,
            &["data.episodes[].year"],
        );
        assert_eq!(
            page["data"]["episodes"][1]["image"],
            json!("/banners/episodes/70753/20492.jpg")
        );
    }

    /// The first season of *Covington Cross*, and the episodes the aired
    /// order places in it.
    fn first_season(covington: &Covington) -> (&Season, Vec<Placed<'_>>) {
        let placed = orders::aired_placements(&covington.item)
            .into_iter()
            .filter(|placed| placed.season_number == 1)
            .collect();
        (&covington.item.seasons[1], placed)
    }

    /// `seasons/{id}`: tinyMediaManager.
    #[test]
    fn the_season_record_tmm_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        let (season, _) = first_season(&covington);
        assert_same_shape(
            &fixture!("season")["data"],
            &Value::Object(covington.work().season(season)),
            &[],
        );
    }

    /// `seasons/{id}/extended`: Seerr.
    #[test]
    fn the_extended_season_seerr_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        let (season, placed) = first_season(&covington);
        assert_same_shape(
            &fixture!("season-extended")["data"],
            &Value::Object(covington.work().season_extended(season, &placed, false)),
            &[],
        );
    }

    /// `seasons/{id}/extended?meta=translations`: Jellyfin.
    #[test]
    fn the_extended_season_with_translations_jellyfin_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        let (season, placed) = first_season(&covington);
        assert_same_shape(
            &fixture!("season-extended-translations")["data"],
            &Value::Object(covington.work().season_extended(season, &placed, true)),
            &[],
        );
    }

    /// *Covington Cross*'s pilot, where the aired order places it.
    fn pilot(covington: &Covington) -> Placed<'_> {
        orders::aired_placements(&covington.item)
            .into_iter()
            .find(|placed| placed.episode.tvdb_id == Some(20492))
            .expect("the pilot")
    }

    /// `episodes/{id}`: Yamtrack, tinyMediaManager.
    #[test]
    fn the_episode_record_yamtrack_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        assert_same_shape(
            &fixture!("episode")["data"],
            &Value::Object(covington.work().episode_record(&pilot(&covington))),
            &[],
        );
    }

    /// `episodes/{id}/extended`: Kodi. What TheTVDB keeps of one episode
    /// the catalogue does not — its production code — is null.
    #[test]
    fn the_extended_episode_kodi_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        assert_same_shape(
            &fixture!("episode-extended")["data"],
            &Value::Object(covington.work().episode_extended(&pilot(&covington), false)),
            &["productionCode"],
        );
    }

    /// `episodes/{id}/extended?meta=translations`: Jellyfin.
    #[test]
    fn the_extended_episode_with_translations_jellyfin_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        assert_same_shape(
            &fixture!("episode-extended-translations")["data"],
            &Value::Object(covington.work().episode_extended(&pilot(&covington), true)),
            &[PRIMARY, &["productionCode"]].concat(),
        );
    }

    /// `episodes/{id}/translations/{lang}`: Kodi, tinyMediaManager.
    #[test]
    fn the_episode_text_kodi_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        assert_same_shape(
            &fixture!("episode-translation")["data"],
            &covington
                .work()
                .episode_translation(&pilot(&covington), "eng")
                .expect("English is held"),
            &[],
        );
    }

    /// `search?query=…&type=series&limit=…`: Jellyfin, Kodi.
    #[test]
    fn the_search_result_jellyfin_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        assert_same_shape(
            &fixture!("search-series")["data"][0],
            &covington.work().search_hit(),
            &[],
        );
    }

    /// `search/remoteid/{id}`: Jellyfin, tinyMediaManager, SickGear.
    #[test]
    fn the_result_by_remote_id_jellyfin_reads_has_thetvdbs_shape() {
        let covington = Covington::new();
        assert_same_shape(
            &fixture!("remoteid-series")["data"][0],
            &covington.work().remote_hit(),
            &[],
        );
    }

    /// *The Matrix* as the catalogue holds it, which keeps no record of
    /// TheTVDB's for a film.
    fn matrix() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.title = "Matrix".into();
        item.original_title = Some("The Matrix".into());
        item.overview = Some("Programmeur anonyme …".into());
        item.original_language = Some("en".into());
        item.original_country = Some("US".into());
        item.status = Some("released".into());
        item.runtime = Some(136);
        item.year = Some(1999);
        item.in_cinemas = Some("1999-03-31".into());
        item.genres = vec!["Action".into(), "Science Fiction".into()];
        item.studio = Some("Warner Bros. Pictures".into());
        item.production_countries = vec!["US".into(), "AU".into()];
        item.trailer_youtube_id = Some("vKQi3bBA1y8".into());
        item.external_ids.tvdb = Some(169);
        item.external_ids.tmdb = Some(603);
        item.external_ids.imdb = Some("tt0133093".into());
        item.translations = vec![translation(
            "eng",
            Some("The Matrix"),
            Some("In the 22nd Century …"),
        )];
        item.images = vec![image(
            "poster",
            "https://artworks.thetvdb.com/banners/movies/169/posters/5f274c00c85c1.jpg",
            Some("eng"),
            None,
        )];
        item.credits = vec![
            credit("actor", "Keanu Reeves", Some("Neo"), 6384),
            credit("director", "Lana Wachowski", None, 9339),
        ];
        item.companies = vec![
            company("studio", "Warner Bros. Pictures", Some(2076), 174),
            company("production", "Village Roadshow Pictures", None, 79),
        ];
        item.releases = ["US", "FR"]
            .into_iter()
            .zip(["1999-03-31", "1999-06-23"])
            .map(|(country, date)| {
                serde_json::from_value::<CountryRelease>(
                    json!({ "country": country, "date": date }),
                )
                .expect("a release")
            })
            .collect();
        item
    }

    /// *The Matrix*'s documents, written with no record of TheTVDB's.
    fn the_matrix(item: &MediaItem) -> Work<'_> {
        static NOTHING: std::sync::LazyLock<Held> = std::sync::LazyLock::new(Held::new);
        Work::new(item, 169, None, STORED, &NOTHING, &same)
    }

    /// `movies/{id}`: tinyMediaManager.
    #[test]
    fn the_film_record_tmm_reads_has_thetvdbs_shape() {
        let item = matrix();
        assert_same_shape(
            &fixture!("movie")["data"],
            &Value::Object(the_matrix(&item).movie()),
            &[],
        );
    }

    /// `movies/{id}/extended?meta=translations&short=false`: Jellyfin.
    #[test]
    fn the_extended_film_jellyfin_reads_has_thetvdbs_shape() {
        let item = matrix();
        assert_same_shape(
            &fixture!("movie-extended-translations")["data"],
            &Value::Object(the_matrix(&item).movie_extended(false, true)),
            &[
                PRIMARY,
                // What TheTVDB keeps of a film the catalogue does not: its
                // takings, its people's ids, pages and pictures, the
                // language of its trailers, its countries' ids. A director
                // plays no role.
                &[
                    "budget",
                    "boxOffice",
                    "boxOfficeUS",
                    "characters[].peopleId",
                    "characters[].url",
                    "characters[].image",
                    "characters[].personImgURL",
                    "characters[].name",
                    "trailers[].language",
                    "production_countries[].id",
                    // A company's slug, which is TheTVDB's.
                    "companies.studio[].slug",
                    "companies.production[].slug",
                ],
            ]
            .concat(),
        );
    }

    /// `movies/{id}/translations/{lang}`: tinyMediaManager.
    #[test]
    fn the_film_text_tmm_reads_has_thetvdbs_shape() {
        let item = matrix();
        assert_same_shape(
            &fixture!("movie-translation")["data"],
            &the_matrix(&item)
                .translation("eng")
                .expect("English is held"),
            &[],
        );
    }

    /// `search?query=…&type=movie`: Jellyfin. A result's texts by language:
    /// each holds its own languages.
    #[test]
    fn the_film_jellyfin_finds_by_name_has_thetvdbs_shape() {
        let item = matrix();
        assert_same_shape(
            &fixture!("search-movie")["data"][0],
            &the_matrix(&item).search_hit(),
            &["overviews.*", "translations.*"],
        );
    }

    /// `search/remoteid/{id}`, for a film: Jellyfin.
    #[test]
    fn the_film_jellyfin_finds_by_a_remote_id_has_thetvdbs_shape() {
        let item = matrix();
        assert_same_shape(
            &fixture!("remoteid-movie")["data"][0],
            &the_matrix(&item).remote_hit(),
            &[],
        );
    }

    /// The gaps left where they are only values the catalogue does not have.
    #[track_caller]
    fn only_nulled(theirs: &Value, ours: &Value, allowed: &[&str]) {
        let left: Vec<_> = gaps(theirs, ours, allowed)
            .into_iter()
            .filter(|gap| !matches!(gap.what, GapKind::Nulled { .. }))
            .collect();
        assert!(left.is_empty(), "{left:#?}");
    }

    /// A series entered by hand, of which TheTVDB keeps no record: every
    /// document keeps every key TheTVDB writes, `null` or empty where the
    /// catalogue has nothing.
    #[test]
    fn a_series_entered_by_hand_keeps_every_key_thetvdb_writes() {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.is_manual = true;
        item.title = "Les Gardiens du phare d'Ouessant".into();
        item.overview = Some("Une série saisie à la main.".into());
        let mut season = blank_season(1);
        season.title = Some("Saison 1".into());
        item.seasons = vec![season];
        item.episodes = (1..=2)
            .map(|number| {
                let mut episode = blank_episode(1, number);
                episode.id = format!("by-hand-{number}");
                episode.title = format!("Épisode {number}");
                episode.air_date = Some(format!("2026-01-0{number}"));
                episode
            })
            .collect();
        let held = Held::new();
        let id = ids::own(&item.id);
        let work = Work::new(&item, id, None, STORED, &held, &same);
        let placed = orders::aired_placements(&item);
        let filter = Filter::default();

        only_nulled(
            &fixture!("series")["data"],
            &Value::Object(work.series()),
            &[],
        );
        let translated = [SEASONS, PRIMARY].concat();
        for asked in [
            Extended::default(),
            Extended {
                translations: true,
                ..Extended::default()
            },
        ] {
            only_nulled(
                &fixture!("series-extended-translations")["data"],
                &Value::Object(work.series_extended(asked, Vec::new())),
                &[translated.as_slice(), &["translations"]].concat(),
            );
        }
        only_nulled(
            &fixture!("series-artworks")["data"],
            &Value::Object(work.series_artworks(None, None)),
            &[],
        );
        let page = work.episodes_page(
            &placed,
            &PageAsked {
                style: Style {
                    lists: true,
                    aired: true,
                    ..Style::default()
                },
                filter: &filter,
                page: 0,
                link: "/v4/series/1/episodes/official",
                previous: None,
            },
        );
        only_nulled(
            &fixture!("episodes-official"),
            &page,
            &["data.episodes[].year"],
        );
        let season = &item.seasons[0];
        only_nulled(
            &fixture!("season-extended")["data"],
            &Value::Object(work.season_extended(season, &placed, false)),
            &["name", "image", "imageType"],
        );
        only_nulled(
            &fixture!("episode-extended")["data"],
            &Value::Object(work.episode_extended(&placed[0], false)),
            &["seasons[].image", "seasons[].imageType"],
        );
        // A result of a search is written with what it has alone, as
        // TheTVDB writes one: no key it does not write, none of another
        // type.
        let hit: Vec<_> = gaps(
            &fixture!("search-series")["data"][0],
            &work.search_hit(),
            &["overviews.*", "translations.*"],
        )
        .into_iter()
        .filter(|gap| gap.what != GapKind::Missing)
        .collect();
        assert!(hit.is_empty(), "{hit:#?}");
        only_nulled(
            &fixture!("remoteid-series")["data"][0],
            &work.remote_hit(),
            &[],
        );

        // Ids made of what names each season and episode, never one of
        // TheTVDB's, the same at every answer.
        let season_id = work.season_id(season);
        assert!(season_id < 0);
        assert_eq!(season_id, own_season_id(&item.id, season));
        let episode = &page["data"]["episodes"][0];
        assert!(episode["id"].as_i64().is_some_and(|id| id < 0));
        assert_eq!(
            episode["id"],
            json!(orders::own_episode_id(&item.episodes[0]))
        );
        assert_eq!(page["data"]["series"]["id"], json!(id));
    }

    /// A record is in its work's own language — TheTVDB's own name, where
    /// the catalogue's is in another — unless a title was locked here; a
    /// document in a language holds that language's text and none other.
    #[test]
    fn each_document_says_what_is_held_in_its_language_and_nothing_else() {
        let (mut item, record, held) = (covington(), record(), english());
        // A Czech name, locked in Czech alone.
        item.locked_texts = vec![crate::domain::fields::LockedText {
            scope: "item".into(),
            field: "title".into(),
            language: "ces".into(),
            value: Some("Covingtonský kříž".into()),
        }];
        // The first season's English name, as a source gave it.
        item.seasons[1].translations = vec![translation("eng", Some("Season One"), None)];
        let work = Work::new(&item, 70753, Some(&record), STORED, &held, &same);

        // Its own language: TMDB's original title, TMDB's English overview.
        assert_eq!(work.name(), "Covington Cross");
        assert_eq!(
            work.overview().as_deref(),
            Some("Covington Cross is a British …")
        );
        // In French, the catalogue's; in Czech, the lock; in Spanish,
        // nothing at all, which TheTVDB answers with a 404.
        assert_eq!(
            work.translation("fra").expect("French")["overview"],
            json!("Les aventures de Thomas Grey …")
        );
        assert_eq!(
            work.translation("ces").expect("Czech")["name"],
            json!("Covingtonský kříž")
        );
        assert!(work.translation("spa").is_none());
        assert_eq!(
            work.languages(Text::Name),
            ["ces", "eng", "fra"].map(String::from)
        );

        // A page in German names no episode: nothing is held in German.
        let placed = orders::aired_placements(&item);
        let filter = Filter::default();
        let asked = |language| PageAsked {
            style: Style {
                language: Some(language),
                paths: true,
                aired: true,
                ..Style::default()
            },
            filter: &filter,
            page: 0,
            link: "",
            previous: None,
        };
        let german = work.episodes_page(&placed, &asked("deu"));
        assert_eq!(german["data"]["episodes"][1]["name"], Value::Null);
        assert_eq!(german["data"]["episodes"][1]["overview"], Value::Null);
        let french = work.episodes_page(&placed, &asked("fra"));
        assert_eq!(french["data"]["episodes"][1]["name"], json!("Le pilote"));
        assert_eq!(
            french["data"]["episodes"][1]["seasonName"],
            json!("Saison 1")
        );
        let english = work.episodes_page(&placed, &asked("eng"));
        assert_eq!(english["data"]["episodes"][1]["name"], json!("Pilot"));
        assert_eq!(
            english["data"]["episodes"][1]["seasonName"],
            json!("Season One")
        );
        assert!(german["data"]["episodes"][1].get("seasonName").is_none());
        // A season's record names it in the work's own language.
        assert_eq!(work.season(&item.seasons[1])["name"], json!("Season One"));
        // The series' record on a page in a language stays in its own.
        assert_eq!(german["data"]["name"], json!("Covington Cross"));

        // A title locked here names the record, as the relay writes it.
        let mut locked = covington();
        locked.title = "La Croix de Covington".into();
        locked.locked_fields = vec!["item/title".into()];
        let work = Work::new(&locked, 70753, Some(&record), STORED, &held, &same);
        assert_eq!(work.name(), "La Croix de Covington");
    }

    /// A page is narrowed as TheTVDB narrows one — to a season, an episode
    /// of it, a day — on a page with no language alone; the next page is
    /// empty, and links back.
    #[test]
    fn a_page_is_narrowed_and_paged_as_thetvdb_does_it() {
        let (item, record, held) = (covington(), record(), Held::new());
        let work = Work::new(&item, 70753, Some(&record), STORED, &held, &same);
        let placed = orders::aired_placements(&item);
        let page = |filter: &Filter, language: Option<&str>, number: u64| {
            work.episodes_page(
                &placed,
                &PageAsked {
                    style: Style {
                        language,
                        lists: language.is_none(),
                        paths: language.is_some(),
                        aired: true,
                    },
                    filter,
                    page: number,
                    link: "/v4/series/70753/episodes/official?page=1",
                    previous: (number > 0).then_some("/v4/series/70753/episodes/official?page=0"),
                },
            )
        };
        let ids = |page: &Value| -> Vec<i64> {
            page["data"]["episodes"]
                .as_array()
                .expect("its episodes")
                .iter()
                .map(|episode| episode["id"].as_i64().unwrap())
                .collect()
        };

        let season = Filter {
            season: Some(1),
            ..Filter::default()
        };
        assert_eq!(ids(&page(&season, None, 0)), [20492, 20493]);
        let one = Filter {
            season: Some(1),
            number: Some(2),
            ..Filter::default()
        };
        let narrowed = page(&one, None, 0);
        assert_eq!(ids(&narrowed), [20493]);
        assert_eq!(narrowed["links"]["total_items"], json!(1));
        // A day: the episodes aired on it, counted with the whole order.
        let day = Filter {
            aired: Some("1992-08-25".into()),
            ..Filter::default()
        };
        let aired = page(&day, None, 0);
        assert_eq!(ids(&aired), [20492]);
        assert_eq!(aired["links"]["total_items"], json!(3));
        // In a language, not narrowed.
        assert_eq!(ids(&page(&one, Some("fra"), 0)).len(), 3);
        // The next page: empty, linking back to the first.
        let next = page(&Filter::default(), None, 1);
        assert!(ids(&next).is_empty());
        assert_eq!(
            next["links"]["prev"],
            json!("/v4/series/70753/episodes/official?page=0")
        );
        assert_eq!(next["links"]["total_items"], json!(3));
    }

    /// What the relay refuses as TheTVDB refuses it is written word for
    /// word as TheTVDB writes it: a translation it has not, a page narrowed
    /// to a number with no season.
    #[test]
    fn a_refusal_is_thetvdbs_own() {
        assert_eq!(not_found(), fixture!("not-found"));
        assert_eq!(no_season(), fixture!("no-season"));
    }

    /// Genres are TheTVDB's, TMDB's pairs split; the ids elsewhere are the
    /// catalogue's, then TheTVDB's for the sources the catalogue keeps none
    /// of; the trailer chosen here leads.
    #[test]
    fn what_the_catalogue_names_is_written_in_thetvdbs_words() {
        let (mut item, record, held) = (covington(), record(), Held::new());
        item.genres = vec![
            "Sci-Fi & Fantasy".into(),
            "Drama".into(),
            "Politique".into(),
        ];
        item.external_ids.tmdb = Some(424_242);
        item.trailer_youtube_id = Some("abc123".into());
        let work = Work::new(&item, 70753, Some(&record), STORED, &held, &same);
        let data = work.series_extended(Extended::default(), Vec::new());

        let genres: Vec<i64> = data["genres"]
            .as_array()
            .unwrap()
            .iter()
            .map(|genre| genre["id"].as_i64().unwrap())
            .collect();
        assert_eq!(genres, [2, 10, 12]);
        // The catalogue's TMDB id, corrected here, not TheTVDB's.
        let remote: Vec<(i64, String)> = data["remoteIds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| {
                (
                    id["type"].as_i64().unwrap(),
                    id["id"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert!(remote.contains(&(12, "424242".to_string())));
        assert!(!remote.contains(&(12, "9721".to_string())));
        assert!(remote.contains(&(2, "tt0103391".to_string())));
        assert_eq!(
            data["trailers"][0]["url"],
            json!("https://www.youtube.com/watch?v=abc123")
        );
        // Of a picture TheTVDB's record names by its address, its id there.
        let poster = data["artworks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|artwork| artwork["type"] == json!(2))
            .expect("a poster");
        assert_eq!(poster["id"], json!(1_276_926));
        assert_eq!(poster["width"], json!(680));
        // Of TMDB's, its size and its votes, from what the catalogue keeps.
        let tmdb = data["artworks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|artwork| {
                artwork["image"]
                    .as_str()
                    .is_some_and(|url| url.contains("image.tmdb.org"))
            })
            .expect("TMDB's poster");
        assert_eq!(
            (&tmdb["width"], &tmdb["height"], &tmdb["score"]),
            (&json!(2000), &json!(3000), &json!(5312))
        );
        assert!(tmdb["id"].as_i64().is_some_and(|id| id < 0));

        // The companies: ABC by TheTVDB's id, under the catalogue's name, as
        // TheTVDB's record writes it; the producer it does not list, made of
        // what the catalogue keeps.
        let companies = data["companies"].as_array().unwrap();
        assert_eq!(companies[0]["id"], json!(5));
        assert_eq!(companies[0]["name"], json!("ABC"));
        assert_eq!(companies[0]["slug"], json!("abc-us"));
        assert!(companies[1]["id"].as_i64().is_some_and(|id| id < 0));
        assert_eq!(companies[1]["slug"], Value::Null);
        assert_eq!(companies[1]["country"], json!("usa"));
        assert_eq!(
            companies[1]["companyType"],
            json!({ "companyTypeId": 3, "companyTypeName": "Production Company" })
        );
        assert_eq!(data["originalNetwork"]["id"], json!(5));
        assert_eq!(data["latestNetwork"]["name"], json!("ABC"));

        // The trailer chosen here leads, though no source lists it; TMDB's
        // follows.
        assert_eq!(
            data["trailers"][1]["url"],
            json!("https://www.youtube.com/watch?v=cvRWgB0dHEk")
        );
        assert_eq!(data["trailers"][1]["language"], json!("eng"));

        // A work not refreshed since its companies and videos were kept:
        // TheTVDB's own.
        item.companies.clear();
        item.videos.clear();
        item.trailer_youtube_id = None;
        let work = Work::new(&item, 70753, Some(&record), STORED, &held, &same);
        let data = work.series_extended(Extended::default(), Vec::new());
        assert_eq!(data["companies"], record["companies"]);
        assert_eq!(data["latestNetwork"], record["latestNetwork"]);
        assert_eq!(data["trailers"], record["trailers"]);
    }
}
