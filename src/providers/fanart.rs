//! Fanart.tv — artwork, and nothing else.
//!
//! It exists for the kinds of image no metadata provider supplies: clear logos
//! on a transparent background, character art, wide landscape thumbs. Kodi,
//! Jellyfin and Emby all use them; Sonarr and Radarr map what they do not
//! recognise to `Unknown` and ignore it, so contributing them costs nothing.
//!
//! Every response is a set of arrays keyed by artwork kind, each entry carrying
//! a language and a like count. Those decide the order, since Fanart.tv is
//! community-uploaded and quality varies.
//!
//! A poster filed under no language — `"00"` — is one without its lettering:
//! it is a poster, and Kodi's key art too. A film's disc is its disc art, a
//! series' character art a kind of its own beside its clear art, and the 4K
//! backgrounds are backgrounds after the HD ones, which every client but
//! this server's own interface is given whole.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    config,
    db::new_id,
    domain::{CoverType, Image, MediaItem, MediaKind},
    providers::lang::base_language,
};

/// How many of each kind to keep.
///
/// A popular series can have thirty posters. Past the first few they are
/// variations nobody will look at, and every one is a row in the database.
const PER_KIND: usize = 5;

pub struct FanartClient {
    http: reqwest::Client,
    base: String,
    api_key: Option<String>,
    enabled: bool,
    language: String,
    gate: crate::providers::Gate,
}

/// How many requests to Fanart.tv are in flight at once: a Radarr bulk
/// refresh of a hundred films asks for all of them at the same moment.
const AT_ONCE: usize = 8;

impl FanartClient {
    pub fn new(http: reqwest::Client, cfg: &config::Fanart, language: &str) -> Self {
        Self {
            http,
            base: cfg.upstream.clone(),
            api_key: cfg.api_key.clone(),
            enabled: cfg.enabled,
            language: base_language(language).to_string(),
            gate: crate::providers::Gate::new("fanart", "Fanart.tv", AT_ONCE),
        }
    }

    pub fn is_configured(&self) -> bool {
        self.api_key.is_some()
    }

    /// Switched on and holding a key.
    pub fn is_enabled(&self) -> bool {
        self.enabled && self.is_configured()
    }

    /// Artwork for a series, by TVDB id — the only key Fanart.tv indexes TV on.
    pub async fn series(&self, tvdb_id: i64) -> Result<Option<(Value, MediaItem)>> {
        let Some(raw) = self.fetch(&format!("tv/{tvdb_id}")).await? else {
            return Ok(None);
        };

        let artwork: Artwork =
            serde_json::from_value(raw.clone()).context("Fanart.tv returned an unreadable body")?;

        Ok(Some((raw, self.to_item(&artwork, MediaKind::Series))))
    }

    /// Artwork for a movie, by TMDB or IMDb id — it accepts either.
    pub async fn movie(&self, id: &str) -> Result<Option<(Value, MediaItem)>> {
        let path = format!("movies/{}", crate::providers::segment(id));
        let Some(raw) = self.fetch(&path).await? else {
            return Ok(None);
        };

        let artwork: Artwork =
            serde_json::from_value(raw.clone()).context("Fanart.tv returned an unreadable body")?;

        Ok(Some((raw, self.to_item(&artwork, MediaKind::Movie))))
    }

    async fn fetch(&self, path: &str) -> Result<Option<Value>> {
        if !self.is_enabled() {
            return Ok(None);
        }

        let Some(key) = self.api_key.as_deref() else {
            return Ok(None);
        };

        let url = format!("{}/{path}", self.base);

        // The error would name the address, key and all, and it is logged:
        // the gate keeps its kind and what lay under it instead.
        let (response, _permit) = self
            .gate
            .send(|| {
                self.http
                    .get(&url)
                    .query(&[("api_key", key)])
                    .timeout(std::time::Duration::from_secs(20))
            })
            .await
            .map_err(|e| anyhow::anyhow!("Fanart.tv request failed: {url}: {e}"))?;

        // Fanart.tv answers 404 for anything it has no artwork for, which is
        // most of the long tail rather than an error.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let status = response.status();
        if !status.is_success() {
            let reason = crate::providers::error_text(response).await;
            anyhow::bail!("Fanart.tv returned {status} for {url}: {reason}");
        }

        crate::providers::read_json(response)
            .await
            .map(Some)
            .with_context(|| format!("Fanart.tv returned a body this server could not read: {url}"))
    }

    /// An item carrying only artwork; the merge layer folds it into the rest.
    fn to_item(&self, artwork: &Artwork, kind: MediaKind) -> MediaItem {
        let mut item = MediaItem::empty(kind);
        let mut images = Vec::new();

        // Pairs are (source list, what it is). Where Fanart.tv offers an HD and
        // a standard variant of the same thing, the HD one comes first.
        let sets: &[(&Vec<Entry>, CoverType)] = &[
            (&artwork.tvposter, CoverType::Poster),
            (&artwork.movieposter, CoverType::Poster),
            (&artwork.showbackground, CoverType::Fanart),
            (&artwork.moviebackground, CoverType::Fanart),
            (&artwork.show4kbackground, CoverType::Fanart),
            (&artwork.movie4kbackground, CoverType::Fanart),
            (&artwork.moviedisc, CoverType::Discart),
            (&artwork.tvbanner, CoverType::Banner),
            (&artwork.moviebanner, CoverType::Banner),
            (&artwork.hdtvlogo, CoverType::Clearlogo),
            (&artwork.clearlogo, CoverType::Clearlogo),
            (&artwork.hdmovielogo, CoverType::Clearlogo),
            (&artwork.movielogo, CoverType::Clearlogo),
            (&artwork.hdclearart, CoverType::Clearart),
            (&artwork.clearart, CoverType::Clearart),
            (&artwork.hdmovieclearart, CoverType::Clearart),
            (&artwork.movieart, CoverType::Clearart),
            (&artwork.tvthumb, CoverType::Landscape),
            (&artwork.moviethumb, CoverType::Landscape),
            (&artwork.characterart, CoverType::Characterart),
        ];

        // Numbered on from one set to the next of the same kind, so that
        // the HD set — and the HD background before the 4K one — keeps its
        // place however the images are sorted later.
        let mut next: std::collections::HashMap<CoverType, i32> = std::collections::HashMap::new();
        for (entries, cover_type) in sets {
            let taken = self.rank(entries);
            let first = next.entry(*cover_type).or_default();

            for (order, entry) in taken.iter().take(PER_KIND).enumerate() {
                images.push(self.image(entry, *cover_type, None, *first + order as i32));
            }
            *first += taken.len().min(PER_KIND) as i32;
        }

        // A poster with no lettering is key art too, by the same address.
        for entries in [&artwork.tvposter, &artwork.movieposter] {
            let textless = self
                .rank(entries)
                .into_iter()
                .filter(|entry| entry.lang.as_deref() == Some("00"));
            for (order, entry) in textless.take(PER_KIND).enumerate() {
                images.push(self.image(entry, CoverType::Keyart, None, order as i32));
            }
        }

        // Season artwork carries the season it belongs to.
        // Per season, not per kind: `PER_KIND` of each shape for each season,
        // the same bargain the series-level loop above makes. Uncapped, a
        // heavily-uploaded anime with eight hundred season posters was eight
        // hundred database rows, rewritten on every refresh, inside one cache
        // entry weighed by its own length.
        let mut per_season: std::collections::HashMap<(CoverType, i32), usize> =
            std::collections::HashMap::new();

        for (entries, cover_type) in [
            (&artwork.seasonposter, CoverType::Poster),
            (&artwork.seasonbanner, CoverType::Banner),
            (&artwork.seasonthumb, CoverType::Landscape),
        ] {
            for entry in self.rank(entries) {
                // `season` is a string, and "all" means it applies to the whole
                // series rather than to any one season.
                let Some(number) = entry.season.as_deref().and_then(|s| s.parse::<i32>().ok())
                else {
                    continue;
                };

                let slot = per_season.entry((cover_type, number)).or_default();
                if *slot >= PER_KIND {
                    continue;
                }

                // And the rank is kept, rather than every one being filed at 0
                // and the ordering that was just computed thrown away.
                images.push(self.image(entry, cover_type, Some(number), *slot as i32));
                *slot += 1;
            }
        }

        item.images = images;
        item
    }

    /// Order by usefulness: this language first, then language-neutral, then
    /// everything else, each group by how many people liked it.
    fn rank<'a>(&self, entries: &'a [Entry]) -> Vec<&'a Entry> {
        let mut ranked: Vec<&Entry> = entries.iter().filter(|e| !e.url.is_empty()).collect();

        ranked.sort_by_key(|entry| {
            let language = entry.lang.as_deref().unwrap_or_default();

            let group = if language == self.language {
                0
            } else if language.is_empty() || language == "00" {
                1
            } else {
                2
            };

            // Likes arrive as a string; unparsable means nobody voted.
            let likes: i64 = entry
                .likes
                .as_deref()
                .and_then(|l| l.parse().ok())
                .unwrap_or(0);

            // `Reverse`, not `-likes`: the count is parsed from a string a
            // stranger uploaded, and negating `i64::MIN` overflows.
            (group, std::cmp::Reverse(likes))
        });

        ranked
    }

    fn image(
        &self,
        entry: &Entry,
        cover_type: CoverType,
        season_number: Option<i32>,
        sort_order: i32,
    ) -> Image {
        Image {
            id: new_id(),
            season_number,
            cover_type,
            url: entry.url.clone(),
            language: entry.lang.clone().filter(|l| !l.is_empty() && l != "00"),
            sort_order,
            source: Some(crate::providers::names::FANART.to_string()),
            is_manual: false,
        }
    }
}

// ─── response shape ──────────────────────────────────────────────────────────

/// Every artwork kind Fanart.tv serves, for TV and for film in one struct.
///
/// A response only ever carries one medium's fields; the other lists come back
/// empty, which is simpler than two near-identical types.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Artwork {
    // Television
    tvposter: Vec<Entry>,
    tvbanner: Vec<Entry>,
    tvthumb: Vec<Entry>,
    showbackground: Vec<Entry>,
    show4kbackground: Vec<Entry>,
    hdtvlogo: Vec<Entry>,
    clearlogo: Vec<Entry>,
    hdclearart: Vec<Entry>,
    clearart: Vec<Entry>,
    characterart: Vec<Entry>,
    seasonposter: Vec<Entry>,
    seasonbanner: Vec<Entry>,
    seasonthumb: Vec<Entry>,

    // Film
    movieposter: Vec<Entry>,
    moviebanner: Vec<Entry>,
    moviethumb: Vec<Entry>,
    moviebackground: Vec<Entry>,
    movie4kbackground: Vec<Entry>,
    hdmovielogo: Vec<Entry>,
    movielogo: Vec<Entry>,
    hdmovieclearart: Vec<Entry>,
    movieart: Vec<Entry>,
    moviedisc: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    #[serde(default)]
    url: String,
    lang: Option<String>,
    /// A count, sent as a string.
    likes: Option<String>,
    /// Present on season artwork only; `"all"` for series-wide.
    season: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(language: &str) -> FanartClient {
        FanartClient::new(
            reqwest::Client::new(),
            &config::Fanart {
                upstream: "https://webservice.fanart.tv/v3".into(),
                api_key: Some("test".into()),
                enabled: true,
            },
            language,
        )
    }

    fn entry(url: &str, lang: Option<&str>, likes: &str) -> Entry {
        Entry {
            url: url.into(),
            lang: lang.map(String::from),
            likes: Some(likes.into()),
            season: None,
        }
    }

    #[test]
    fn the_requested_language_outranks_everything() {
        let entries = vec![
            entry("https://x/en-many-likes.png", Some("en"), "50"),
            entry("https://x/fr.png", Some("fr"), "1"),
            entry("https://x/neutral.png", Some(""), "30"),
        ];

        let ranked = client("fr").rank(&entries);

        assert_eq!(ranked[0].url, "https://x/fr.png");
        assert_eq!(
            ranked[1].url, "https://x/neutral.png",
            "then language-neutral"
        );
        assert_eq!(ranked[2].url, "https://x/en-many-likes.png");
    }

    #[test]
    fn within_a_group_the_most_liked_comes_first() {
        let entries = vec![
            entry("https://x/few.png", Some("en"), "2"),
            entry("https://x/many.png", Some("en"), "40"),
            entry("https://x/none.png", Some("en"), "not a number"),
        ];

        let ranked = client("en").rank(&entries);

        assert_eq!(ranked[0].url, "https://x/many.png");
        assert_eq!(ranked[1].url, "https://x/few.png");
        assert_eq!(ranked[2].url, "https://x/none.png");
    }

    #[test]
    fn entries_with_no_url_are_dropped() {
        let entries = vec![
            entry("", Some("en"), "10"),
            entry("https://x/real.png", None, "1"),
        ];
        assert_eq!(client("en").rank(&entries).len(), 1);
    }

    #[test]
    fn a_response_maps_to_artwork_and_nothing_else() {
        let body = serde_json::json!({
            "name": "Breaking Bad",
            "thetvdb_id": "81189",
            "tvposter": [{ "id": "1", "url": "https://f/p.jpg", "lang": "en", "likes": "9" }],
            "hdtvlogo": [{ "id": "2", "url": "https://f/logo.png", "lang": "en", "likes": "4" }],
            "hdclearart": [{ "id": "3", "url": "https://f/art.png", "lang": "en", "likes": "2" }],
            "tvthumb": [{ "id": "4", "url": "https://f/thumb.jpg", "lang": "en", "likes": "1" }],
            "seasonposter": [
                { "id": "5", "url": "https://f/s1.jpg", "lang": "en", "likes": "3", "season": "1" },
                { "id": "6", "url": "https://f/all.jpg", "lang": "en", "likes": "3", "season": "all" }
            ]
        });

        let artwork: Artwork = serde_json::from_value(body).unwrap();
        let item = client("en").to_item(&artwork, MediaKind::Series);

        let kinds: Vec<&str> = item.images.iter().map(|i| i.cover_type.as_str()).collect();
        assert!(kinds.contains(&"poster"));
        assert!(kinds.contains(&"clearlogo"));
        assert!(kinds.contains(&"clearart"));
        assert!(kinds.contains(&"landscape"));

        // "all" is not a season number and must not become one.
        let season_images: Vec<_> = item
            .images
            .iter()
            .filter(|i| i.season_number.is_some())
            .collect();
        assert_eq!(season_images.len(), 1);
        assert_eq!(season_images[0].season_number, Some(1));

        // It contributes artwork only — the merge layer supplies the rest.
        assert!(item.title.is_empty());
        assert!(item.episodes.is_empty());
        assert!(item.credits.is_empty());
    }

    /// A film's textless poster is key art too, its disc is disc art, and
    /// its 4K backgrounds come after the HD ones.
    #[test]
    fn a_films_key_art_disc_and_4k_backgrounds_are_read() {
        let body = serde_json::json!({
            "name": "The Matrix",
            "movieposter": [
                { "id": "1", "url": "https://f/p-en.jpg", "lang": "en", "likes": "9" },
                { "id": "2", "url": "https://f/p-none.jpg", "lang": "00", "likes": "3" }
            ],
            "moviebackground": [
                { "id": "3", "url": "https://f/b-hd.jpg", "lang": "", "likes": "1" }
            ],
            "movie4kbackground": [
                { "id": "4", "url": "https://f/b-4k.jpg", "lang": "", "likes": "40" }
            ],
            "moviedisc": [
                { "id": "5", "url": "https://f/disc.png", "lang": "en", "likes": "2", "disc": "1", "disc_type": "bluray" }
            ]
        });
        let artwork: Artwork = serde_json::from_value(body).unwrap();
        let item = client("en").to_item(&artwork, MediaKind::Movie);
        let of = |kind: CoverType| -> Vec<(&str, i32)> {
            item.images
                .iter()
                .filter(|i| i.cover_type == kind)
                .map(|i| (i.url.as_str(), i.sort_order))
                .collect()
        };

        assert_eq!(
            of(CoverType::Poster),
            [("https://f/p-en.jpg", 0), ("https://f/p-none.jpg", 1)]
        );
        assert_eq!(
            of(CoverType::Keyart),
            [("https://f/p-none.jpg", 0)],
            "only the poster with no lettering"
        );
        assert_eq!(
            of(CoverType::Fanart),
            [("https://f/b-hd.jpg", 0), ("https://f/b-4k.jpg", 1)],
            "the HD background first, however the 4K one is liked"
        );
        assert_eq!(of(CoverType::Discart), [("https://f/disc.png", 0)]);
    }

    /// A series' character art is a kind of its own, not more clear art:
    /// a character alone has no title on it, and Kodi files it apart.
    #[test]
    fn a_series_character_art_is_kept_apart_from_its_clear_art() {
        let body = serde_json::json!({
            "name": "Breaking Bad",
            "hdclearart": [
                { "id": "1", "url": "https://f/clear.png", "lang": "en", "likes": "5" }
            ],
            "characterart": [
                { "id": "2", "url": "https://f/walter.png", "lang": "", "likes": "7" }
            ]
        });
        let artwork: Artwork = serde_json::from_value(body).unwrap();
        let item = client("en").to_item(&artwork, MediaKind::Series);
        let of = |kind: CoverType| -> Vec<&str> {
            item.images
                .iter()
                .filter(|i| i.cover_type == kind)
                .map(|i| i.url.as_str())
                .collect()
        };

        assert_eq!(of(CoverType::Clearart), ["https://f/clear.png"]);
        assert_eq!(of(CoverType::Characterart), ["https://f/walter.png"]);
    }

    #[test]
    fn an_empty_response_is_not_an_error() {
        let artwork: Artwork = serde_json::from_value(serde_json::json!({ "name": "X" })).unwrap();
        assert!(
            client("en")
                .to_item(&artwork, MediaKind::Movie)
                .images
                .is_empty()
        );
    }

    #[test]
    fn no_more_than_a_handful_of_each_kind_is_kept() {
        let many: Vec<serde_json::Value> = (0..20)
            .map(|i| serde_json::json!({ "url": format!("https://f/{i}.jpg"), "lang": "en", "likes": "1" }))
            .collect();

        let artwork: Artwork =
            serde_json::from_value(serde_json::json!({ "movieposter": many })).unwrap();

        let item = client("en").to_item(&artwork, MediaKind::Movie);
        assert_eq!(item.images.len(), PER_KIND);
    }
}
