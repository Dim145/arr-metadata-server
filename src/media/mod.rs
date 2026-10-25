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
    db::repo::asset::{Indexed, Kind, Thumb, Wanted},
    domain::{CoverType, MediaItem, Picture},
};

pub mod encode;
pub mod fetch;
pub mod file;
pub mod placeholder;
pub mod serve;
pub mod store;
pub mod variants;
pub mod worker;

pub use file::valid_key;

/// The path the media are served under, on this server.
pub const ROUTE: &str = "/media/";

/// The fields whose locked value is an address: a still put on an episode by
/// hand, a theme put on a work, the poster and background a work or a season
/// is to lead with.
pub const ADDRESS_FIELDS: &[&str] = &["image", "themeMusic", "primaryPoster", "primaryFanart"];

/// The media store, as this process holds it.
pub struct Media {
    store: Option<Arc<store::Store>>,
    index: RwLock<Index>,
    /// The client that follows the addresses providers gave: resolving
    /// through the guard, so a name that turns internal gets nowhere.
    pub http: reqwest::Client,
    /// What that client may reach, for the checks made before it is asked.
    pub guard: crate::outbound::Guard,
    /// Rung by whatever put an address in line.
    pub notify: tokio::sync::Notify,
    /// What the served addresses begin with: the public URL, so that a
    /// client elsewhere can follow them, or nothing, for the interface alone.
    base: String,
    pub config: config::Media,
    /// Among several instances: the server the others are told through of
    /// what this one kept or forgot, and this instance's id, so it can tell
    /// its own word from theirs.
    tell: Option<(crate::cache::RedisSlot, String)>,
}

#[derive(Default)]
struct Index {
    by_origin: HashMap<String, Entry>,
    by_key: HashMap<String, Keyed>,
    /// A copy's key carries the hash and not the picture's extension: the
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

/// A key, as the index knows it. Its content type is its extension's: see
/// [`file::content_type_of`].
#[derive(Clone, Debug)]
pub struct Keyed {
    pub origin: Arc<str>,
    pub thumb: Thumb,
    /// What a page is told of the picture: see [`Look`].
    pub look: Arc<Look>,
}

/// What this server knows of a picture it keeps that a page draws it with:
/// its size, its placeholder, and the AVIF copies made of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub color: Option<[u8; 3]>,
    pub thumbhash: Option<Arc<[u8]>>,
    /// The widths of the AVIF copies made, in order.
    pub avif: Vec<u32>,
}

impl Look {
    /// From the row: the size the bytes were inspected as, and the
    /// placeholder worked out — each only when it reads.
    pub fn of_row(row: &Indexed) -> Self {
        let size = |n: Option<i32>| n.and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0);
        Self {
            width: size(row.width),
            height: size(row.height),
            color: row.color.as_deref().and_then(placeholder::parse_color),
            thumbhash: row
                .thumbhash
                .as_deref()
                .and_then(placeholder::parse_thumbhash)
                .map(Arc::from),
            avif: Vec::new(),
        }
    }

    /// As the native API sends it.
    pub fn as_picture(&self) -> Picture {
        Picture {
            width: self.width,
            height: self.height,
            color: self
                .color
                .map(|[r, g, b]| format!("#{r:02x}{g:02x}{b:02x}")),
            thumbhash: self.thumbhash.as_deref().map(|hash| {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD.encode(hash)
            }),
            avif: self.avif.clone(),
        }
    }
}

impl Media {
    pub fn open(
        config: &config::Media,
        public_url: Option<&str>,
        tell: Option<(crate::cache::RedisSlot, String)>,
    ) -> anyhow::Result<Self> {
        let store = store::Store::open(config)?.map(Arc::new);
        if let Some(store) = &store {
            tracing::info!(backend = ?store.backend, "media are kept");
        }
        let guard = crate::outbound::Guard::from_env()?;
        if !guard.private_networks {
            tracing::info!("media are not fetched from private networks");
        }
        Ok(Self {
            store,
            index: RwLock::new(Index::default()),
            http: crate::outbound::guarded_client(guard)?,
            guard,
            notify: tokio::sync::Notify::new(),
            base: public_url
                .map(|u| u.trim_end_matches('/').to_string())
                .unwrap_or_default(),
            config: config.clone(),
            tell,
        })
    }

    /// Tell the other instances, when there are any.
    fn tell(&self, message: crate::coord::Message) {
        let Some((slot, instance)) = &self.tell else {
            return;
        };
        let Some(redis) = slot.load_full() else {
            return;
        };
        let text = message.encode(instance);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move { redis.publish(&text).await });
        }
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
        let rows = crate::db::repo::asset::indexed(db).await?;
        let mut index = Index::default();
        for row in &rows {
            index.insert(&row.origin, &row.key, row.thumb, Some(Look::of_row(row)));
        }
        for (stem, width) in crate::db::repo::asset::avif_index(db).await? {
            index.add_avif(&stem, width);
        }
        let n = index.by_origin.len();
        *self.index.write().unwrap_or_else(|e| e.into_inner()) = index;
        tracing::info!(stored = n, "media index loaded");
        Ok(())
    }

    /// Say what a picture kept looks like — its size and its placeholder —
    /// once it is worked out. Here alone: another instance learns it when
    /// it next loads its index.
    pub fn describe(&self, key: &str, mut look: Look) {
        let mut index = self.index.write().unwrap_or_else(|e| e.into_inner());
        if let Some(keyed) = index.by_key.get_mut(key) {
            look.avif.clone_from(&keyed.look.avif);
            keyed.look = Arc::new(look);
        }
    }

    /// Note an AVIF copy made of the picture of `stem`, so that a page is
    /// offered it.
    pub fn add_avif(&self, stem: &str, width: u32) {
        self.index
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .add_avif(stem, width);
    }

    /// Forget an AVIF copy whose file is gone, so that no page asks for it
    /// again.
    pub fn drop_avif(&self, stem: &str, width: u32) {
        let mut index = self.index.write().unwrap_or_else(|e| e.into_inner());
        let Some(key) = index.by_sha.get(stem).cloned() else {
            return;
        };
        if let Some(keyed) = index.by_key.get_mut(&key) {
            let mut look = (*keyed.look).clone();
            look.avif.retain(|w| *w != width);
            keyed.look = Arc::new(look);
        }
    }

    /// Note an asset as stored, here and on every other instance.
    pub fn remember(&self, origin: &str, key: &str, thumb: Thumb, content_type: &str) {
        self.remember_quietly(origin, key, thumb, content_type);
        self.tell(crate::coord::Message::MediaKept {
            origin: origin.to_string(),
            key: key.to_string(),
            thumb: thumb.as_i64(),
            content_type: content_type.to_string(),
        });
    }

    /// Note an asset as stored, here alone: what another instance's word
    /// does. That word comes over the cache server, which somebody else may
    /// be able to write to, so it is checked as any other request is: a key
    /// of this server's grammar, a content type its extension stands for, an
    /// origin that is an address or an upload. Anything else is not noted.
    pub fn remember_quietly(&self, origin: &str, key: &str, thumb: Thumb, content_type: &str) {
        if !fits_the_index(origin, key, content_type) {
            tracing::warn!(
                key = ?key.chars().take(80).collect::<String>(),
                "a medium kept was refused: not a key, type or origin this server files"
            );
            return;
        }
        self.index
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(origin, key, thumb, None);
    }

    /// Forget an asset, here and on every other instance: its address is a
    /// provider's again. Its key stays known while another origin holds the
    /// same bytes.
    pub fn forget(&self, origin: &str) {
        self.forget_quietly(origin);
        self.tell(crate::coord::Message::MediaForgotten(origin.to_string()));
    }

    /// Forget an asset here alone.
    pub fn forget_quietly(&self, origin: &str) {
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

    /// Where a key is served from: a path, which the interface follows
    /// whatever name it reached this server by. The public URL is for what
    /// is read elsewhere — see [`Self::for_elsewhere`].
    pub fn url_for(&self, key: &str) -> String {
        format!("{ROUTE}{key}")
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
        // A copy's key carries its picture's hash, not its extension.
        let stem = if file::is_made(key) {
            self.keyed_by_sha(file::stem_of(key))
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
        // What a person hid, as the editor shows it to be given back.
        for image in &mut item.hidden.images {
            self.localize_url(&mut image.url);
        }
        for credit in &mut item.hidden.credits {
            self.localize_optional(&mut credit.image);
        }
        self.describe_leads(item);
    }

    /// Say what the pictures a card or a plate leads with look like: the
    /// poster, the background, the landscape and the logo — the one chosen,
    /// one added by hand, or the first, as a page picks it — and the first
    /// of each besides, which is what a card keeps of a work. Only the
    /// copies kept here: of any other address nothing is known.
    fn describe_leads(&self, item: &mut MediaItem) {
        const LEADING: [CoverType; 4] = [
            CoverType::Poster,
            CoverType::Fanart,
            CoverType::Landscape,
            CoverType::Clearlogo,
        ];
        let own: Vec<&crate::domain::Image> = item
            .images
            .iter()
            .filter(|image| image.season_number.is_none() && LEADING.contains(&image.cover_type))
            .collect();
        let mut urls: Vec<String> =
            crate::domain::lead_images(own.iter().copied(), &item.primary_images)
                .into_iter()
                .map(|image| image.url.clone())
                .collect();
        for kind in LEADING {
            if let Some(first) = own.iter().find(|image| image.cover_type == kind) {
                urls.push(first.url.clone());
            }
        }

        let index = self.index.read().unwrap_or_else(|e| e.into_inner());
        item.pictures.clear();
        for url in urls {
            let Some(keyed) = Self::key_in(&url).and_then(|key| index.by_key.get(key)) else {
                continue;
            };
            if keyed.look.width.is_some() {
                item.pictures.insert(url, keyed.look.as_picture());
            }
        }
    }

    /// The address a reader elsewhere can follow, for one of this server's
    /// own: under the public URL — or, with none set, the provider's, and
    /// nothing at all for an upload, which has no address but the path.
    pub fn for_elsewhere(&self, url: &str) -> Option<String> {
        if !url.starts_with(ROUTE) {
            return Some(url.to_string());
        }
        if !self.base.is_empty() {
            return Some(format!("{}{url}", self.base));
        }
        let origin = self.unlocalize(url);
        (origin.starts_with("https://") || origin.starts_with("http://")).then_some(origin)
    }

    /// A work as a client elsewhere reads it: every copy kept is addressed
    /// by a path only this server's own pages can follow, so each is given
    /// as [`Self::for_elsewhere`] has it.
    pub fn for_clients(&self, item: &mut MediaItem) {
        // What this server's own pages are told of its pictures, by their
        // paths here: nothing a client elsewhere reads.
        item.pictures.clear();
        if !self.is_on() {
            return;
        }
        let back_optional = |url: &mut Option<String>| {
            if let Some(current) = url.as_deref() {
                *url = self.for_elsewhere(current);
            }
        };
        let back_images = |images: &mut Vec<crate::domain::Image>| {
            images.retain_mut(|image| match self.for_elsewhere(&image.url) {
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
            if !fetch::fetchable(url, self.guard)
                || self.lookup(url).is_some()
                || !seen.insert(url.to_string())
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
            Ok(added) if added > 0 => {
                self.notify.notify_one();
                // Whichever instance is idle may fetch them.
                self.tell(crate::coord::Message::MediaWake);
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(id = %item.id, error = %e, "could not put the work's media in line")
            }
        }
    }
}

/// Whether an entry is one this server could have filed: a picture's or a
/// sound's key — never a copy's, which is found by its picture's — served
/// as the type its extension stands for, for an address or an upload.
fn fits_the_index(origin: &str, key: &str, content_type: &str) -> bool {
    valid_key(key)
        && !file::is_made(key)
        && file::content_type_of(key) == Some(content_type)
        && (origin.starts_with("https://")
            || origin.starts_with("http://")
            || origin.starts_with("upload:"))
}

impl Index {
    /// Note a key under an origin. What is known of the picture already —
    /// the same bytes from another address — stays, unless `look` says
    /// more.
    fn insert(&mut self, origin: &str, key: &str, thumb: Thumb, look: Option<Look>) {
        let key: Arc<str> = Arc::from(key);
        let origin_arc: Arc<str> = Arc::from(origin);
        self.by_origin
            .insert(origin.to_string(), Entry { key: key.clone() });
        let holders = self.holders.entry(key.to_string()).or_default();
        if !holders.iter().any(|o| o == origin) {
            holders.push(origin.to_string());
        }
        let look = match (look, self.by_key.get(key.as_ref())) {
            (Some(mut look), Some(known)) => {
                // The same bytes under another address: what either row
                // says of them holds.
                look.color = look.color.or(known.look.color);
                look.thumbhash = look.thumbhash.or_else(|| known.look.thumbhash.clone());
                look.avif.clone_from(&known.look.avif);
                Arc::new(look)
            }
            (Some(look), None) => Arc::new(look),
            (None, Some(known)) => known.look.clone(),
            (None, None) => Arc::default(),
        };
        self.by_key.insert(
            key.to_string(),
            Keyed {
                origin: origin_arc,
                thumb,
                look,
            },
        );
        if let Some(sha) = key.split('.').next() {
            self.by_sha.insert(sha.to_string(), key.to_string());
        }
    }

    fn add_avif(&mut self, stem: &str, width: u32) {
        let Some(key) = self.by_sha.get(stem).cloned() else {
            return;
        };
        if let Some(keyed) = self.by_key.get_mut(&key)
            && !keyed.look.avif.contains(&width)
        {
            let mut look = (*keyed.look).clone();
            look.avif.push(width);
            look.avif.sort_unstable();
            keyed.look = Arc::new(look);
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
            dir: crate::testing::scratch_path("media-test"),
            s3: None,
        };
        Media::open(&config, public_url, None).unwrap()
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
            format!("/media/{KEY}"),
            "a path, whatever name the interface reached the server by"
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

        // A client elsewhere is given the copy under the public URL.
        media.for_clients(&mut item);
        assert_eq!(
            item.images[0].url,
            format!("https://ams.example/media/{KEY}")
        );
        assert_eq!(
            media.unlocalize(&item.images[0].url),
            "https://p/poster.jpg"
        );
        assert_eq!(
            media.unlocalize(&format!(
                "http://other.host/media/{}",
                file::thumb_key(KEY, Thumb::Jpeg).unwrap()
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

        // Under a public URL, an upload has an address like any other copy.
        let public = self::tests::media(Some("https://ams.example"));
        public.remember("upload:0123", UP, Thumb::None, "image/png");
        assert_eq!(
            public.for_elsewhere(&format!("/media/{UP}")).as_deref(),
            Some(&*format!("https://ams.example/media/{UP}"))
        );
        assert_eq!(
            public.for_elsewhere("https://r/c.jpg").as_deref(),
            Some("https://r/c.jpg")
        );
    }

    /// Another instance's word, as the cache server carries it, is checked
    /// before it is believed: anybody able to write there could otherwise
    /// have a picture served as a page.
    #[test]
    fn a_word_from_elsewhere_is_checked_before_it_is_noted() {
        let media = media(None);
        let sha = KEY.split('.').next().unwrap();
        for (origin, key, content_type) in [
            // A type the key's extension does not stand for.
            ("https://p/a.jpg", KEY, "text/html"),
            ("https://p/a.jpg", KEY, "image/png"),
            // Not a key of this server's.
            ("https://p/a.jpg", "../../etc/passwd", "image/jpeg"),
            ("https://p/a.jpg", &*format!("{sha}.svg"), "image/svg+xml"),
            ("https://p/a.jpg", &*format!("{sha}.html"), "text/html"),
            // A thumbnail's key, which is found through its picture's.
            ("https://p/a.jpg", &*format!("{sha}-t.jpg"), "image/jpeg"),
            // Not an address or an upload.
            ("javascript:alert(1)", KEY, "image/jpeg"),
        ] {
            media.remember_quietly(origin, key, Thumb::None, content_type);
            assert!(
                media.lookup(origin).is_none() && media.keyed(key).is_none(),
                "{origin} {key} {content_type}"
            );
        }

        media.remember_quietly("https://p/a.jpg", KEY, Thumb::Jpeg, "image/jpeg");
        assert_eq!(media.lookup("https://p/a.jpg").unwrap().key.as_ref(), KEY);
        const SOUND: &str = "cb7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.m4a";
        media.remember_quietly("upload:1", SOUND, Thumb::None, "audio/mp4");
        assert!(media.keyed(SOUND).is_some());
    }

    /// A page is told what the pictures a card or a plate leads with look
    /// like, by their addresses here — and of a copy kept, nothing else; a
    /// client elsewhere is told nothing of it.
    #[test]
    fn a_work_says_what_its_leading_pictures_look_like() {
        let media = media(None);
        let sha = KEY.split('.').next().unwrap();
        media.remember("https://p/poster.jpg", KEY, Thumb::None, "image/jpeg");
        media.describe(
            KEY,
            Look {
                width: Some(2000),
                height: Some(3000),
                color: Some([20, 40, 90]),
                thumbhash: Some(Arc::from(&[1u8, 2, 3, 4, 5][..])),
                avif: Vec::new(),
            },
        );
        media.add_avif(sha, 960);
        media.add_avif(sha, 480);

        let mut item = MediaItem::empty(MediaKind::Movie);
        for (cover_type, url) in [
            (CoverType::Poster, "https://p/poster.jpg"),
            (CoverType::Poster, "https://p/other.jpg"),
            (CoverType::Banner, "https://p/poster.jpg"),
        ] {
            item.images.push(Image {
                id: crate::db::new_id(),
                season_number: None,
                cover_type,
                url: url.into(),
                language: None,
                sort_order: 0,
                source: Some("tmdb".into()),
                is_manual: false,
            });
        }
        media.localize(&mut item);
        assert_eq!(
            item.pictures.into_iter().collect::<Vec<_>>(),
            [(
                format!("/media/{KEY}"),
                Picture {
                    width: Some(2000),
                    height: Some(3000),
                    color: Some("#14285a".into()),
                    thumbhash: Some("AQIDBAU=".into()),
                    avif: vec![480, 960],
                }
            )]
        );

        let mut item = MediaItem::empty(MediaKind::Movie);
        item.pictures.insert("/media/x".into(), Picture::default());
        media.for_clients(&mut item);
        assert!(item.pictures.is_empty());

        media.drop_avif(sha, 480);
        assert_eq!(media.keyed(KEY).unwrap().look.avif, [960]);
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
