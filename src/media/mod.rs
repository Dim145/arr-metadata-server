//! The media kept here: every picture and theme a work points at, fetched
//! once and served from this server or its bucket, so the catalogue reads
//! without its providers.
//!
//! A work's rows keep the provider's addresses. Those are what a merge
//! compares, a refresh puts back and a provenance names, and they are the
//! key to what is kept: an index in memory says which addresses have their
//! bytes here, and a work is rewritten as it is read — never as it is
//! written — to point at them. Something that needs the address a provider
//! gave can always ask for it back.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

use crate::{
    config,
    db::repo::asset::{Kind, Thumb, Wanted},
    domain::MediaItem,
};

pub mod fetch;
pub mod file;
pub mod serve;
pub mod store;
pub mod worker;

pub use file::{thumb_key, valid_key};

/// The path the media are served under, on this server.
pub const ROUTE: &str = "/media/";

/// The media store, as this process holds it.
pub struct Media {
    store: Option<Arc<store::Store>>,
    index: RwLock<Index>,
    /// The client that follows the addresses providers gave: resolving
    /// through the guard, so a name that turns internal gets nowhere.
    pub http: reqwest::Client,
    /// Rung by whatever put an address in line.
    pub notify: tokio::sync::Notify,
    /// What the served addresses begin with: the public URL, so that a
    /// client elsewhere can follow them, or nothing, for the interface alone.
    base: String,
    pub config: config::Media,
}

#[derive(Default)]
struct Index {
    by_origin: HashMap<String, Entry>,
    by_key: HashMap<String, Keyed>,
    /// A thumbnail's key carries the hash and not the extension: the
    /// picture's key, by its hash.
    by_sha: HashMap<String, String>,
    /// Every origin that holds a key: the same bytes from two providers are
    /// one file, which stays until the last of them goes.
    holders: HashMap<String, Vec<String>>,
}

/// One asset stored, as the index knows it.
#[derive(Clone, Debug)]
pub struct Entry {
    pub key: Arc<str>,
}

/// A key, as the index knows it.
#[derive(Clone, Debug)]
pub struct Keyed {
    pub origin: Arc<str>,
    pub thumb: Thumb,
    pub content_type: Arc<str>,
}

impl Media {
    pub fn open(config: &config::Media, public_url: Option<&str>) -> anyhow::Result<Self> {
        let store = store::Store::open(config)?.map(Arc::new);
        if let Some(store) = &store {
            tracing::info!(backend = ?store.backend, "media are kept");
        }
        Ok(Self {
            store,
            index: RwLock::new(Index::default()),
            http: crate::outbound::guarded_client()?,
            notify: tokio::sync::Notify::new(),
            base: public_url
                .map(|u| u.trim_end_matches('/').to_string())
                .unwrap_or_default(),
            config: config.clone(),
        })
    }

    /// Whether anything is kept at all.
    pub fn is_on(&self) -> bool {
        self.store.is_some()
    }

    pub fn store(&self) -> Option<&Arc<store::Store>> {
        self.store.as_ref()
    }

    /// Fill the index from the database, at start.
    pub async fn load_index(&self, db: &crate::db::Db) -> anyhow::Result<()> {
        let rows = crate::db::repo::asset::stored_index(db).await?;
        let mut index = Index::default();
        for (origin, key, thumb, content_type) in rows {
            index.insert(&origin, &key, thumb, &content_type);
        }
        let n = index.by_origin.len();
        *self.index.write().unwrap_or_else(|e| e.into_inner()) = index;
        tracing::info!(stored = n, "media index loaded");
        Ok(())
    }

    /// Note an asset as stored.
    pub fn remember(&self, origin: &str, key: &str, thumb: Thumb, content_type: &str) {
        self.index
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(origin, key, thumb, content_type);
    }

    /// Forget an asset: its address is a provider's again. Its key stays
    /// known while another origin holds the same bytes.
    pub fn forget(&self, origin: &str) {
        let mut index = self.index.write().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = index.by_origin.remove(origin) else {
            return;
        };
        let key = entry.key.to_string();
        let survivor = index.holders.get_mut(&key).and_then(|holders| {
            holders.retain(|o| o != origin);
            holders.first().cloned()
        });
        match survivor {
            Some(survivor) => {
                if let Some(keyed) = index.by_key.get_mut(&key) {
                    keyed.origin = Arc::from(survivor.as_str());
                }
            }
            None => {
                index.holders.remove(&key);
                index.by_key.remove(&key);
                if let Some(sha) = key.split('.').next() {
                    index.by_sha.remove(sha);
                }
            }
        }
    }

    pub fn lookup(&self, origin: &str) -> Option<Entry> {
        self.index
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .by_origin
            .get(origin)
            .cloned()
    }

    pub fn keyed(&self, key: &str) -> Option<Keyed> {
        self.index
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .by_key
            .get(key)
            .cloned()
    }

    /// The key whose hash this is.
    pub fn keyed_by_sha(&self, sha: &str) -> Option<String> {
        self.index
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .by_sha
            .get(sha)
            .cloned()
    }

    /// Where a key is served from.
    pub fn url_for(&self, key: &str) -> String {
        format!("{}{ROUTE}{key}", self.base)
    }

    /// The key an address of this server's names, whether it begins with the
    /// public URL, with nothing, or with any origin at all.
    pub fn key_in(url: &str) -> Option<&str> {
        let (_, rest) = url.split_once(ROUTE)?;
        let key = rest.split(['?', '#']).next().unwrap_or(rest);
        valid_key(key).then_some(key)
    }

    /// The address a provider gave, for one of this server's own — or the
    /// address itself, when it is not one.
    pub fn unlocalize(&self, url: &str) -> String {
        let Some(key) = Self::key_in(url) else {
            return url.to_string();
        };
        // A thumbnail's key carries its picture's hash, not its extension.
        let stem = if file::is_thumb(key) {
            key.split('-').next().and_then(|sha| self.keyed_by_sha(sha))
        } else {
            Some(key.to_string())
        };
        match stem.and_then(|stem| self.keyed(&stem)) {
            Some(keyed) => keyed.origin.to_string(),
            None => url.to_string(),
        }
    }

    /// Point at the copy kept, when there is one.
    fn localize_url(&self, url: &mut String) {
        if let Some(entry) = self.lookup(url) {
            *url = self.url_for(&entry.key);
        }
    }

    fn localize_optional(&self, url: &mut Option<String>) {
        if let Some(url) = url.as_mut() {
            self.localize_url(url);
        }
    }

    /// Rewrite a work to point at every copy kept: its pictures, its
    /// seasons', its episodes' stills, its cast's photographs, its
    /// relations' posters and its theme.
    pub fn localize(&self, item: &mut MediaItem) {
        if !self.is_on() {
            return;
        }
        for image in &mut item.images {
            self.localize_url(&mut image.url);
        }
        for season in &mut item.seasons {
            for image in &mut season.images {
                self.localize_url(&mut image.url);
            }
        }
        for episode in &mut item.episodes {
            self.localize_optional(&mut episode.image);
        }
        for credit in &mut item.credits {
            self.localize_optional(&mut credit.image);
        }
        for relation in &mut item.relations {
            self.localize_optional(&mut relation.image);
        }
        self.localize_optional(&mut item.theme_music);
    }

    /// A work as a client elsewhere reads it. With no public URL, the copies
    /// kept are addressed by paths only this server's own pages can follow:
    /// such a client is given the providers' addresses back — and nothing
    /// at all for an upload, which has no address but the path.
    pub fn for_clients(&self, item: &mut MediaItem) {
        if !self.base.is_empty() || !self.is_on() {
            return;
        }
        // The provider's address, or none to give.
        let back = |url: &str| -> Option<String> {
            if !url.starts_with(ROUTE) {
                return Some(url.to_string());
            }
            let origin = self.unlocalize(url);
            (origin.starts_with("https://") || origin.starts_with("http://")).then_some(origin)
        };
        let back_optional = |url: &mut Option<String>| {
            if let Some(current) = url.as_deref() {
                *url = back(current);
            }
        };
        let back_images = |images: &mut Vec<crate::domain::Image>| {
            images.retain_mut(|image| match back(&image.url) {
                Some(url) => {
                    image.url = url;
                    true
                }
                None => false,
            });
        };
        back_images(&mut item.images);
        for season in &mut item.seasons {
            back_images(&mut season.images);
        }
        for episode in &mut item.episodes {
            back_optional(&mut episode.image);
        }
        for credit in &mut item.credits {
            back_optional(&mut credit.image);
        }
        for relation in &mut item.relations {
            back_optional(&mut relation.image);
        }
        back_optional(&mut item.theme_music);
    }

    /// Rewrite an address that stands alone — a card's poster, a
    /// calendar's still — when a copy is kept.
    pub fn localized(&self, url: &str) -> String {
        match self.lookup(url) {
            Some(entry) => self.url_for(&entry.key),
            None => url.to_string(),
        }
    }

    /// The addresses a work points at that are not kept yet, for the line.
    pub fn wanted_of(&self, item: &MediaItem, people: bool, audio: bool) -> Vec<(String, Kind)> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let mut want = |url: &str, kind: Kind| {
            if !fetch::fetchable(url) || self.lookup(url).is_some() || !seen.insert(url.to_string())
            {
                return;
            }
            out.push((url.to_string(), kind));
        };

        for image in &item.images {
            want(&image.url, Kind::Image);
        }
        for season in &item.seasons {
            for image in &season.images {
                want(&image.url, Kind::Image);
            }
        }
        for episode in &item.episodes {
            if let Some(url) = &episode.image {
                want(url, Kind::Image);
            }
        }
        if people {
            for credit in &item.credits {
                if let Some(url) = &credit.image {
                    want(url, Kind::Image);
                }
            }
        }
        for relation in &item.relations {
            if let Some(url) = &relation.image {
                want(url, Kind::Image);
            }
        }
        if audio && let Some(url) = &item.theme_music {
            want(url, Kind::Audio);
        }
        out
    }

    /// Put a work's media in line to be fetched, as the settings allow, and
    /// wake whatever fetches them. Quiet: a work is stored whether or not
    /// its pictures can be.
    pub async fn enqueue_for(&self, state: &crate::state::AppState, item: &MediaItem) {
        if !self.is_on() || !state.flag("media.store", true) {
            return;
        }
        let wanted = self.wanted_of(
            item,
            state.flag("media.people", true),
            state.flag("media.audio", true),
        );
        if wanted.is_empty() {
            return;
        }
        let rows: Vec<Wanted<'_>> = wanted
            .iter()
            .map(|(origin, kind)| Wanted {
                origin,
                kind: *kind,
                wanted_by: Some(&item.id),
            })
            .collect();
        match crate::db::repo::asset::enqueue(&state.db, &rows).await {
            Ok(added) if added > 0 => self.notify.notify_one(),
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(id = %item.id, error = %e, "could not put the work's media in line")
            }
        }
    }
}

impl Index {
    fn insert(&mut self, origin: &str, key: &str, thumb: Thumb, content_type: &str) {
        let key: Arc<str> = Arc::from(key);
        let content_type: Arc<str> = Arc::from(content_type);
        let origin_arc: Arc<str> = Arc::from(origin);
        self.by_origin
            .insert(origin.to_string(), Entry { key: key.clone() });
        let holders = self.holders.entry(key.to_string()).or_default();
        if !holders.iter().any(|o| o == origin) {
            holders.push(origin.to_string());
        }
        self.by_key.insert(
            key.to_string(),
            Keyed {
                origin: origin_arc,
                thumb,
                content_type,
            },
        );
        if let Some(sha) = key.split('.').next() {
            self.by_sha.insert(sha.to_string(), key.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CoverType, Image, MediaKind};

    fn media(public_url: Option<&str>) -> Media {
        let config = config::Media {
            storage: config::MediaStorage::Filesystem,
            dir: std::env::temp_dir().join(format!("ams-media-test-{}", crate::db::new_id())),
            s3: None,
        };
        Media::open(&config, public_url).unwrap()
    }

    const KEY: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.jpg";

    #[test]
    fn a_work_is_rewritten_to_what_is_kept_and_back() {
        let media = media(Some("https://ams.example/"));
        media.remember("https://p/poster.jpg", KEY, Thumb::Jpeg, "image/jpeg");

        let mut item = MediaItem::empty(MediaKind::Movie);
        item.images.push(Image {
            id: crate::db::new_id(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: "https://p/poster.jpg".into(),
            language: None,
            sort_order: 0,
            source: Some("tmdb".into()),
            is_manual: false,
        });
        item.theme_music = Some("https://p/theme.mp3".into());

        media.localize(&mut item);
        assert_eq!(
            item.images[0].url,
            format!("https://ams.example/media/{KEY}")
        );
        assert_eq!(
            item.theme_music.as_deref(),
            Some("https://p/theme.mp3"),
            "not kept: as it was"
        );

        assert_eq!(
            media.unlocalize(&item.images[0].url),
            "https://p/poster.jpg"
        );
        assert_eq!(
            media.unlocalize(&format!("/media/{KEY}")),
            "https://p/poster.jpg"
        );
        assert_eq!(
            media.unlocalize(&format!(
                "http://other.host/media/{}",
                thumb_key(KEY, Thumb::Jpeg).unwrap()
            )),
            "https://p/poster.jpg",
            "a thumbnail's address is its picture's"
        );
        assert_eq!(
            media.unlocalize("https://p/other.jpg"),
            "https://p/other.jpg"
        );

        // Wanted: what is not kept yet, and only what can be fetched.
        let wanted = media.wanted_of(&item, true, true);
        assert_eq!(
            wanted,
            vec![("https://p/theme.mp3".to_string(), Kind::Audio)]
        );
        assert!(media.wanted_of(&item, true, false).is_empty());

        media.forget("https://p/poster.jpg");
        assert!(media.lookup("https://p/poster.jpg").is_none());
        assert!(media.keyed(KEY).is_none());
    }

    /// The same bytes from two places are one file, known until the last
    /// of them goes.
    #[test]
    fn a_key_two_origins_hold_outlives_the_first() {
        let media = media(None);
        media.remember("https://p/a.jpg", KEY, Thumb::None, "image/jpeg");
        media.remember("https://q/b.jpg", KEY, Thumb::None, "image/jpeg");
        media.forget("https://p/a.jpg");
        assert_eq!(
            media.keyed(KEY).map(|k| k.origin.to_string()).as_deref(),
            Some("https://q/b.jpg")
        );
        assert_eq!(
            media.unlocalize(&format!("/media/{KEY}")),
            "https://q/b.jpg"
        );
        media.forget("https://q/b.jpg");
        assert!(media.keyed(KEY).is_none());
        assert!(media.keyed_by_sha(KEY.split('.').next().unwrap()).is_none());
    }

    /// A client elsewhere gets the providers' addresses back, and nothing
    /// for an upload, which has none.
    #[test]
    fn a_client_elsewhere_is_given_what_it_can_fetch() {
        let media = media(None);
        media.remember("https://p/a.jpg", KEY, Thumb::None, "image/jpeg");
        const UP: &str = "ab7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.png";
        media.remember("upload:0123", UP, Thumb::None, "image/png");

        let mut item = MediaItem::empty(MediaKind::Movie);
        for url in [
            format!("/media/{KEY}"),
            format!("/media/{UP}"),
            "https://r/c.jpg".into(),
        ] {
            item.images.push(Image {
                id: crate::db::new_id(),
                season_number: None,
                cover_type: CoverType::Poster,
                url,
                language: None,
                sort_order: 0,
                source: None,
                is_manual: false,
            });
        }
        item.theme_music = Some(format!("/media/{UP}"));
        media.for_clients(&mut item);
        let urls: Vec<&str> = item.images.iter().map(|i| i.url.as_str()).collect();
        assert_eq!(urls, ["https://p/a.jpg", "https://r/c.jpg"]);
        assert_eq!(item.theme_music, None);
    }

    #[test]
    fn without_a_public_url_the_addresses_are_relative() {
        let media = media(None);
        media.remember("https://p/a.jpg", KEY, Thumb::None, "image/jpeg");
        assert_eq!(media.localized("https://p/a.jpg"), format!("/media/{KEY}"));
        assert_eq!(Media::key_in(&format!("/media/{KEY}?x=1")), Some(KEY));
        assert_eq!(Media::key_in("/media/../etc"), None);
        assert_eq!(Media::key_in("https://p/a.jpg"), None);
    }
}
