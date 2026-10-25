//! Downloading artwork beside the `.nfo` documents that reference it.
//!
//! A `.nfo` names its artwork by URL, and what the consumer does with that URL
//! is up to the consumer: Kodi fetches it, Plex's Personal Media agent often
//! will not, and neither works on a library that is offline or behind a network
//! the media server cannot reach. Local files are read by all of them.
//!
//! So the export writes the pictures next to the documents, under the names
//! those programs look for — and where the two do not look for the same, under
//! both:
//!
//! | | Kodi | Plex (local assets, which its NFO agent reads) |
//! |---|---|---|
//! | poster, background, logo | `poster.jpg`, `fanart.jpg`, `clearlogo.png` | the same: `poster`, `fanart` (among `art`, `backdrop`, `background`), `clearlogo` (or `logo`) |
//! | a season's poster | `season01-poster.jpg` | `season01.jpg` |
//! | the specials' | `season-specials-poster.jpg` | the same |
//! | an episode's still | `S01E01-thumb.jpg` | `S01E01.jpg` |
//! | banner, clear art, landscape | `banner.jpg`, `clearart.png`, `landscape.jpg` | not read |
//! | key art, disc art, character art | `keyart.jpg`, `discart.png`, `characterart.png` | not read |
//!
//! Where the names differ the picture is written once, under Kodi's, and the
//! other is a hard link to it: the same bytes on the disk, which a second
//! copy would double — a library's stills are thousands of files. A disk that
//! cannot link (a network share, FAT) is given a copy instead, which is the
//! one case that costs the space. The disc art goes by Emby's name as well,
//! `disc.png`, which Jellyfin reads too; tinyMediaManager and Kodi read
//! `discart.png`.
//!
//! Key art is a file and nothing else: the documents never name it, since
//! Jellyfin reads any `<thumb>` it does not know as the work's poster.
//!
//! Cast headshots go in `.actors/`, and the theme beside the show as
//! `theme.mp3`, where both programs and Jellyfin look for them.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::{
    db::repo::asset::Kind,
    domain::{CoverType, Episode, MediaItem, MediaKind},
    media::file,
};

/// One file to place at one path: a picture, or the theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    /// Relative to the export root, using `/` as the separator.
    pub path: String,
    pub url: String,
    /// The other names the same file goes by, for the program that does not
    /// look for the first: each a link to it, or a copy. See the table above.
    pub aliases: Vec<String>,
}

/// Every picture a work's documents refer to, and its theme music, and where
/// each one belongs.
///
/// Pure, so the layout can be checked without a network or a filesystem.
pub fn plan(item: &MediaItem) -> Vec<Download> {
    let folder = match item.kind {
        MediaKind::Series => format!("series/{}", item.slug),
        MediaKind::Movie => format!("movies/{}", item.slug),
    };

    let mut downloads = Vec::new();
    let mut taken: HashSet<String> = HashSet::new();

    // The first of each kind wins: the images are already sorted by the merge,
    // most-wanted first, and a folder holds one poster. The claim is staked on
    // the stem, not the file name, or a PNG logo and a JPEG one would both land
    // and leave the consumer to pick.
    //
    // A season's own pictures are kept on the season, not among the work's: a
    // work read from the store has none of them in `images`, so they are taken
    // from the seasons — in their order — after the work's own.
    let season_images = item.seasons.iter().flat_map(|season| {
        season
            .images
            .iter()
            .map(move |image| (image, Some(season.season_number)))
    });
    for (image, season) in item
        .images
        .iter()
        .map(|image| (image, image.season_number))
        .chain(season_images)
    {
        let Some(stem) = file_stem(image.cover_type, season) else {
            continue;
        };

        if !taken.insert(format!("{folder}/{stem}")) {
            continue;
        }

        let extension = extension(&image.url);
        // Plex reads a season's poster as `seasonNN.jpg`, where Kodi reads
        // `seasonNN-poster.jpg`; the specials' is the same for both. Emby
        // reads a disc's art as `disc`, where Kodi reads `discart`.
        let aliases = match (image.cover_type, season) {
            (CoverType::Poster, Some(number)) if number > 0 => {
                vec![format!("{folder}/season{number:02}.{extension}")]
            }
            (CoverType::Discart, None) => vec![format!("{folder}/disc.{extension}")],
            _ => Vec::new(),
        };

        downloads.push(Download {
            path: format!("{folder}/{stem}.{extension}"),
            url: image.url.clone(),
            aliases,
        });
    }

    for episode in &item.episodes {
        let Some(url) = episode.image.as_deref().filter(|u| !u.trim().is_empty()) else {
            continue;
        };

        let path = format!("{folder}/{}", episode_name(episode, url));

        if taken.insert(path.clone()) {
            // Plex reads the still as the episode's file name with a picture's
            // extension, where Kodi reads it with `-thumb` before it.
            let alias = format!(
                "{folder}/Season {:02}/S{:02}E{:02}.{}",
                episode.season_number,
                episode.season_number,
                episode.episode_number,
                extension(url)
            );
            downloads.push(Download {
                path,
                url: url.to_string(),
                aliases: vec![alias],
            });
        }
    }

    // Kodi reads cast headshots from `.actors`, named after the `<name>` in the
    // document. A person appearing twice is one file.
    for credit in &item.credits {
        let Some(url) = credit.image.as_deref().filter(|u| !u.trim().is_empty()) else {
            continue;
        };

        let Some(name) = actor_file_name(&credit.person_name) else {
            continue;
        };

        let path = format!("{folder}/.actors/{name}");

        if taken.insert(path.clone()) {
            downloads.push(Download {
                path,
                url: url.to_string(),
                aliases: Vec::new(),
            });
        }
    }

    // The theme, where one is known. `theme.mp3` beside the show's documents is
    // what Plex's local assets, Jellyfin and Kodi's theme add-ons play.
    if item.kind == MediaKind::Series
        && let Some(url) = item.theme_music.as_deref().filter(|u| !u.trim().is_empty())
    {
        let path = format!("{folder}/theme.mp3");
        if taken.insert(path.clone()) {
            downloads.push(Download {
                path,
                url: url.to_string(),
                aliases: Vec::new(),
            });
        }
    }

    downloads
}

/// The name Kodi looks for, without an extension, or `None` for artwork it has
/// no place for.
fn file_stem(cover: CoverType, season: Option<i32>) -> Option<String> {
    // A season's poster sits in the show's folder, numbered — and the
    // specials', season nought, have a name of their own, which Plex reads
    // the same: `season00-poster` is read by neither.
    if let Some(season) = season.filter(|s| *s >= 0) {
        let season = match season {
            0 => "season-specials".to_string(),
            number => format!("season{number:02}"),
        };
        return match cover {
            CoverType::Poster => Some(format!("{season}-poster")),
            CoverType::Banner => Some(format!("{season}-banner")),
            CoverType::Fanart => Some(format!("{season}-fanart")),
            _ => None,
        };
    }

    let stem = match cover {
        CoverType::Poster => "poster",
        CoverType::Fanart => "fanart",
        CoverType::Banner => "banner",
        CoverType::Clearlogo => "clearlogo",
        CoverType::Clearart => "clearart",
        CoverType::Landscape => "landscape",
        CoverType::Keyart => "keyart",
        CoverType::Discart => "discart",
        CoverType::Characterart => "characterart",
        // A headshot outside a credit, and a still outside an episode, belong to
        // nothing a consumer would look for.
        CoverType::Screenshot | CoverType::Headshot | CoverType::Unknown => return None,
    };

    Some(stem.to_string())
}

/// `Season 01/S01E01-thumb.jpg`, beside that episode's document.
fn episode_name(episode: &Episode, url: &str) -> String {
    format!(
        "Season {:02}/S{:02}E{:02}-thumb.{}",
        episode.season_number,
        episode.season_number,
        episode.episode_number,
        extension(url)
    )
}

/// A person's name as a file name, or `None` if nothing usable is left.
///
/// The name comes from a provider, so it is not trusted to be a path: anything
/// that could climb out of `.actors` or confuse a filesystem is dropped rather
/// than escaped, since a headshot is not worth a surprising write.
fn actor_file_name(person: &str) -> Option<String> {
    let cleaned: String = person
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect();

    // A word of nothing but dots is how a path climbs, and is not part of
    // anyone's name. Dropping those also collapses the gaps left above.
    let name = cleaned
        .split_whitespace()
        .filter(|word| !word.chars().all(|c| c == '.'))
        .collect::<Vec<_>>()
        .join(" ");

    if name.is_empty() || name.len() > 120 {
        return None;
    }

    Some(format!("{name}.jpg"))
}

/// The file extension to use, from the URL, defaulting to `jpg`.
fn extension(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url);

    match path.rsplit('.').next().map(str::to_ascii_lowercase) {
        Some(ext) if ext == "png" => "png",
        Some(ext) if ext == "webp" => "webp",
        _ => "jpg",
    }
}

/// Fetch one picture and write it, unless it is already there — and give it
/// the other names it goes by, which are links to it.
///
/// Returns `false` when the file already existed, so a second export costs
/// nothing rather than refetching a library's worth of artwork.
pub async fn fetch_one(
    state: &crate::state::AppState,
    root: &Path,
    download: &Download,
) -> Result<bool> {
    let written = fetch_primary(state, root, download).await?;

    // Also where the file was there already: an export from before a name
    // was added is given it by the next one.
    let original = resolve(root, &download.path)?;
    for alias in &download.aliases {
        link_or_copy(&original, &resolve(root, alias)?).await?;
    }

    Ok(written)
}

/// Another name for a file already written: a hard link, which is the same
/// bytes on the disk and costs none, or — where the disk cannot link — a
/// copy. Left alone where the name is taken: a file somebody put there is
/// not ours to replace.
async fn link_or_copy(original: &Path, name: &Path) -> Result<()> {
    if tokio::fs::metadata(name).await.is_ok_and(|m| m.len() > 0) {
        return Ok(());
    }
    // An empty file left by a write that did not finish is no name taken.
    let _ = tokio::fs::remove_file(name).await;

    if tokio::fs::hard_link(original, name).await.is_ok() {
        return Ok(());
    }
    tokio::fs::copy(original, name)
        .await
        .with_context(|| format!("could not place {}", name.display()))?;

    Ok(())
}

/// Fetch one picture and write it, unless it is already there.
///
/// Fetched as the media store fetches: through its guarded client, every
/// redirect judged, no larger than a picture — or a theme — may be, and
/// written only when the bytes are the picture or the sound wanted. An
/// address that answers with anything else is somebody's page, or worse,
/// and is not left beside the documents under a picture's name.
async fn fetch_primary(
    state: &crate::state::AppState,
    root: &Path,
    download: &Download,
) -> Result<bool> {
    let destination = resolve(root, &download.path)?;

    if tokio::fs::metadata(&destination)
        .await
        .is_ok_and(|m| m.len() > 0)
    {
        return Ok(false);
    }

    // A copy this server keeps is read from its store, not fetched from
    // itself over the network.
    if let (Some(key), Some(store)) = (
        crate::media::Media::key_in(&download.url),
        state.media.store(),
    ) {
        let bytes = store
            .read(key)
            .await?
            .with_context(|| format!("{key} is not in the media store"))?;
        return write(&destination, &bytes).await.map(|()| true);
    }
    let http = &state.media.http;

    // Again here, and not only where the URL was stored: a name that pointed
    // somewhere ordinary when it was accepted can point at loopback by the time
    // this runs, and rows written before the check existed are still in there.
    if state.media.guard.refuses_url(&download.url).await {
        anyhow::bail!(
            "{} is not an address this server fetches from here",
            download.url
        );
    }

    let response = http
        .get(&download.url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .with_context(|| format!("could not fetch {}", download.url))?;

    // An answer that is not the file is not read: its body is nothing
    // wanted, and is bounded by nothing.
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("{status} fetching {}", download.url);
    }

    let kind = kind_of(download);
    let limit = match kind {
        Kind::Image => file::MAX_IMAGE_BYTES,
        Kind::Audio => file::MAX_AUDIO_BYTES,
    };
    let bytes = crate::providers::read_body(response, limit)
        .await
        .with_context(|| format!("could not read {}", download.url))?;

    file::inspect(&bytes, kind).with_context(|| format!("{} was not kept", download.url))?;

    write(&destination, &bytes).await.map(|()| true)
}

/// What a planned file is: the theme a sound, everything else a picture.
fn kind_of(download: &Download) -> Kind {
    if download.path.ends_with("/theme.mp3") {
        Kind::Audio
    } else {
        Kind::Image
    }
}

/// Put a picture in its place: written beside the target and renamed, so
/// a half-written one is never left behind under a name a media server
/// will read.
async fn write(destination: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("could not create {}", parent.display()))?;
    }

    let staged = destination.with_extension("partial");
    tokio::fs::write(&staged, bytes)
        .await
        .with_context(|| format!("could not write {}", staged.display()))?;
    tokio::fs::rename(&staged, destination)
        .await
        .with_context(|| format!("could not place {}", destination.display()))?;

    Ok(())
}

/// Join a planned path onto the root, refusing anything that leaves it.
fn resolve(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut destination = root.to_path_buf();

    for segment in relative.split('/') {
        anyhow::ensure!(
            !segment.is_empty() && segment != "." && segment != "..",
            "refusing to write outside the export root: {relative}"
        );

        destination.push(segment);
    }

    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Credit, CreditType, Image};

    fn image(cover: CoverType, season: Option<i32>, url: &str) -> Image {
        Image {
            id: String::new(),
            season_number: season,
            cover_type: cover,
            url: url.to_string(),
            language: None,
            sort_order: 0,
            source: None,
            is_manual: false,
        }
    }

    fn series() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.slug = "breaking-bad-2008".into();
        item
    }

    #[test]
    fn a_theme_lands_as_theme_mp3_beside_the_show() {
        let mut item = series();
        item.theme_music = Some("https://metadata.fankai.fr/series/12/theme?t=1".into());

        let downloads = plan(&item);
        assert_eq!(
            downloads,
            vec![Download {
                path: "series/breaking-bad-2008/theme.mp3".into(),
                url: "https://metadata.fankai.fr/series/12/theme?t=1".into(),
                aliases: Vec::new(),
            }]
        );

        // A film's folder has no theme to play.
        let mut film = MediaItem::empty(MediaKind::Movie);
        film.slug = "arrival-2016".into();
        film.theme_music = item.theme_music.clone();
        assert!(plan(&film).is_empty());
    }

    #[test]
    fn artwork_lands_under_the_names_kodi_reads() {
        let mut item = series();
        item.images = vec![
            image(CoverType::Poster, None, "https://x.invalid/p.jpg"),
            image(CoverType::Fanart, None, "https://x.invalid/b.jpg"),
            image(CoverType::Clearlogo, None, "https://x.invalid/l.png"),
            image(CoverType::Poster, Some(1), "https://x.invalid/s1.jpg"),
        ];

        let paths: Vec<String> = plan(&item).into_iter().map(|d| d.path).collect();

        assert_eq!(
            paths,
            vec![
                "series/breaking-bad-2008/poster.jpg",
                "series/breaking-bad-2008/fanart.jpg",
                "series/breaking-bad-2008/clearlogo.png",
                "series/breaking-bad-2008/season01-poster.jpg",
            ]
        );
    }

    #[test]
    fn one_kind_lands_once_whatever_the_source_extension() {
        // Two clearlogos, one PNG and one JPEG, are still one clearlogo.
        let mut item = series();
        item.images = vec![
            image(CoverType::Clearlogo, None, "https://x.invalid/a.png"),
            image(CoverType::Clearlogo, None, "https://x.invalid/b.jpg"),
        ];

        let downloads = plan(&item);

        assert_eq!(downloads.len(), 1);
        assert_eq!(downloads[0].path, "series/breaking-bad-2008/clearlogo.png");
    }

    #[test]
    fn only_the_first_of_each_kind_is_written() {
        // A work carries dozens of posters; a folder holds one.
        let mut item = series();
        item.images = vec![
            image(CoverType::Poster, None, "https://x.invalid/first.jpg"),
            image(CoverType::Poster, None, "https://x.invalid/second.jpg"),
        ];

        let downloads = plan(&item);

        assert_eq!(downloads.len(), 1);
        assert_eq!(downloads[0].url, "https://x.invalid/first.jpg");
    }

    #[test]
    fn an_episode_still_sits_beside_its_document() {
        let mut item = series();
        let mut episode = crate::db::repo::child::blank_episode(1, 3);
        episode.image = Some("https://x.invalid/still.jpg".into());
        item.episodes = vec![episode];

        assert_eq!(
            plan(&item)[0].path,
            "series/breaking-bad-2008/Season 01/S01E03-thumb.jpg"
        );
    }

    #[test]
    fn cast_headshots_go_into_the_actors_folder() {
        let mut item = series();
        item.credits = vec![Credit {
            id: String::new(),
            credit_type: CreditType::Actor,
            person_name: "Bryan Cranston".into(),
            character_name: None,
            image: Some("https://x.invalid/bc.jpg".into()),
            tmdb_person_id: None,
            credit_tmdb_id: None,
            sort_order: 0,
            is_manual: false,
        }];

        assert_eq!(
            plan(&item)[0].path,
            "series/breaking-bad-2008/.actors/Bryan Cranston.jpg"
        );
    }

    #[test]
    fn a_name_that_would_climb_out_of_the_folder_is_refused() {
        assert_eq!(
            actor_file_name("../../etc/passwd"),
            Some("etc passwd.jpg".into())
        );
        assert_eq!(actor_file_name(".."), None);
        assert_eq!(actor_file_name("   "), None);
        assert_eq!(
            actor_file_name("Ana de Armas"),
            Some("Ana de Armas.jpg".into()),
            "an ordinary name passes through untouched"
        );
    }

    #[test]
    fn a_planned_path_cannot_leave_the_root() {
        let root = Path::new("/tmp/export");

        assert!(resolve(root, "series/x/poster.jpg").is_ok());
        assert!(resolve(root, "series/../../etc/passwd").is_err());
        assert!(resolve(root, "series//poster.jpg").is_err());
    }

    #[test]
    fn a_theme_is_a_sound_and_the_rest_are_pictures() {
        let mut item = series();
        item.theme_music = Some("https://x.invalid/theme".into());
        item.images = vec![image(CoverType::Poster, None, "https://x.invalid/p.jpg")];
        let kinds: Vec<Kind> = plan(&item).iter().map(kind_of).collect();
        assert_eq!(kinds, [Kind::Image, Kind::Audio]);
    }

    #[test]
    fn the_extension_follows_the_url() {
        assert_eq!(extension("https://x.invalid/a.png"), "png");
        assert_eq!(extension("https://x.invalid/a.jpg?size=w500"), "jpg");
        assert_eq!(extension("https://x.invalid/nothing"), "jpg");
    }

    /// A season, with the posters it was given.
    fn season(number: i32, posters: &[&str]) -> crate::domain::Season {
        crate::domain::Season {
            id: format!("s{number}"),
            season_number: number,
            title: None,
            overview: None,
            air_date: None,
            tmdb_id: None,
            tvdb_id: None,
            is_manual: false,
            images: posters
                .iter()
                .map(|url| image(CoverType::Poster, Some(number), url))
                .collect(),
            primary_images: Default::default(),
        }
    }

    #[test]
    fn a_seasons_poster_is_taken_from_the_season_and_goes_by_both_names() {
        // A work read from the store has its seasons' pictures on the
        // seasons, not among its own: the plan must look there.
        let mut item = series();
        item.images = vec![image(CoverType::Poster, None, "https://x.invalid/p.jpg")];
        item.seasons = vec![
            season(
                1,
                &["https://x.invalid/s1a.png", "https://x.invalid/s1b.jpg"],
            ),
            season(2, &["https://x.invalid/s2.jpg"]),
        ];

        let downloads = plan(&item);
        let named: Vec<(&str, Vec<&str>)> = downloads
            .iter()
            .map(|d| {
                (
                    d.path.as_str(),
                    d.aliases.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            named,
            [
                ("series/breaking-bad-2008/poster.jpg", vec![]),
                (
                    "series/breaking-bad-2008/season01-poster.png",
                    vec!["series/breaking-bad-2008/season01.png"]
                ),
                (
                    "series/breaking-bad-2008/season02-poster.jpg",
                    vec!["series/breaking-bad-2008/season02.jpg"]
                ),
            ],
            "the first poster of each season, once"
        );
        assert_eq!(downloads[1].url, "https://x.invalid/s1a.png");
    }

    #[test]
    fn the_specials_are_named_as_both_programs_name_them() {
        let mut item = series();
        item.seasons = vec![season(0, &["https://x.invalid/sp.jpg"])];

        let downloads = plan(&item);
        assert_eq!(downloads.len(), 1);
        assert_eq!(
            downloads[0].path,
            "series/breaking-bad-2008/season-specials-poster.jpg"
        );
        assert!(
            downloads[0].aliases.is_empty(),
            "Kodi and Plex read the same name for the specials"
        );
        assert_eq!(
            file_stem(CoverType::Banner, Some(0)).as_deref(),
            Some("season-specials-banner")
        );
        assert_eq!(
            file_stem(CoverType::Fanart, Some(3)).as_deref(),
            Some("season03-fanart")
        );
    }

    /// Key art lands as `keyart.jpg` and disc art as `discart.png`, which
    /// Emby reads as `disc.png`; neither is named in the document, where
    /// Jellyfin would take key art for the poster.
    #[test]
    fn key_art_and_disc_art_land_under_their_names_and_nowhere_else() {
        let mut film = MediaItem::empty(MediaKind::Movie);
        film.slug = "the-matrix-1999".into();
        film.title = "The Matrix".into();
        film.images = vec![
            image(CoverType::Poster, None, "https://x.invalid/p.jpg"),
            image(CoverType::Keyart, None, "https://x.invalid/k.jpg"),
            image(CoverType::Keyart, None, "https://x.invalid/k2.jpg"),
            image(CoverType::Discart, None, "https://x.invalid/d.png"),
        ];
        let named: Vec<(String, Vec<String>)> = plan(&film)
            .into_iter()
            .map(|d| (d.path, d.aliases))
            .collect();
        assert_eq!(
            named,
            [
                ("movies/the-matrix-1999/poster.jpg".to_string(), vec![]),
                ("movies/the-matrix-1999/keyart.jpg".to_string(), vec![]),
                (
                    "movies/the-matrix-1999/discart.png".to_string(),
                    vec!["movies/the-matrix-1999/disc.png".to_string()]
                ),
            ]
        );

        let document = crate::export::nfo::for_item(&film);
        assert!(document.contains("<thumb aspect=\"poster\">https://x.invalid/p.jpg</thumb>"));
        for absent in ["keyart", "discart", "k.jpg", "d.png"] {
            assert!(!document.contains(absent), "{absent} in {document}");
        }
    }

    /// A series' character art is `characterart.png` beside its clear art,
    /// and, like it, named by no document.
    #[test]
    fn character_art_lands_beside_the_clear_art() {
        let mut item = series();
        item.images = vec![
            image(CoverType::Clearart, None, "https://x.invalid/clear.png"),
            image(
                CoverType::Characterart,
                None,
                "https://x.invalid/walter.png",
            ),
        ];
        let paths: Vec<String> = plan(&item).into_iter().map(|d| d.path).collect();
        assert_eq!(
            paths,
            [
                "series/breaking-bad-2008/clearart.png",
                "series/breaking-bad-2008/characterart.png"
            ]
        );
        assert!(!crate::export::nfo::for_item(&item).contains("walter.png"));
    }

    /// One document per episode, as Kodi 22 reads them: never two
    /// `<episodedetails>` in one file.
    #[test]
    fn each_episode_has_a_document_of_its_own() {
        let mut item = series();
        item.episodes = vec![
            crate::db::repo::child::blank_episode(1, 1),
            crate::db::repo::child::blank_episode(1, 2),
        ];
        let paths: Vec<String> = item
            .episodes
            .iter()
            .map(|episode| crate::export::nfo::episode_relative_path(&item, episode))
            .collect();
        assert_eq!(
            paths,
            [
                "series/breaking-bad-2008/Season 01/S01E01.nfo",
                "series/breaking-bad-2008/Season 01/S01E02.nfo"
            ]
        );
        for episode in &item.episodes {
            let document = crate::export::nfo::for_episode(&item, episode, &[], None);
            assert_eq!(document.matches("<episodedetails>").count(), 1);
        }
    }

    #[test]
    fn what_both_programs_read_under_one_name_is_written_once() {
        let mut item = series();
        item.images = vec![
            image(CoverType::Poster, None, "https://x.invalid/p.jpg"),
            image(CoverType::Fanart, None, "https://x.invalid/b.jpg"),
            image(CoverType::Clearlogo, None, "https://x.invalid/l.png"),
            image(CoverType::Banner, None, "https://x.invalid/ba.jpg"),
        ];
        for download in plan(&item) {
            assert!(
                download.aliases.is_empty(),
                "{} needs no second name",
                download.path
            );
        }
    }

    #[test]
    fn an_episodes_still_goes_by_the_name_plex_reads_too() {
        let mut item = series();
        let mut episode = crate::db::repo::child::blank_episode(2, 3);
        episode.image = Some("https://x.invalid/still.png".into());
        item.episodes = vec![episode];

        let downloads = plan(&item);
        assert_eq!(
            downloads[0].path,
            "series/breaking-bad-2008/Season 02/S02E03-thumb.png"
        );
        assert_eq!(
            downloads[0].aliases,
            ["series/breaking-bad-2008/Season 02/S02E03.png"]
        );
    }

    /// A directory for a test to write in.
    fn scratch(name: &str) -> PathBuf {
        crate::testing::scratch_path(name)
    }

    #[tokio::test]
    async fn another_name_costs_no_second_copy_and_never_replaces_a_file() {
        let root = scratch("art");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let original = root.join("season01-poster.jpg");
        tokio::fs::write(&original, b"the poster").await.unwrap();

        let alias = root.join("season01.jpg");
        link_or_copy(&original, &alias).await.expect("linked");
        assert_eq!(tokio::fs::read(&alias).await.unwrap(), b"the poster");
        // On a disk that links, the same file: one set of bytes, two names.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let (a, b) = (
                std::fs::metadata(&original).unwrap(),
                std::fs::metadata(&alias).unwrap(),
            );
            assert_eq!((a.dev(), a.ino()), (b.dev(), b.ino()));
        }

        // Again, and over a name somebody put there: left alone.
        link_or_copy(&original, &alias).await.expect("again");
        let theirs = root.join("theirs.jpg");
        tokio::fs::write(&theirs, b"their poster").await.unwrap();
        link_or_copy(&original, &theirs).await.expect("kept");
        assert_eq!(tokio::fs::read(&theirs).await.unwrap(), b"their poster");

        // A name left empty by a write that did not finish is replaced.
        let empty = root.join("empty.jpg");
        tokio::fs::write(&empty, b"").await.unwrap();
        link_or_copy(&original, &empty).await.expect("replaced");
        assert_eq!(tokio::fs::read(&empty).await.unwrap(), b"the poster");

        // Nothing to link to: said, not skipped.
        assert!(
            link_or_copy(&root.join("missing.jpg"), &root.join("other.jpg"))
                .await
                .is_err()
        );

        let _ = tokio::fs::remove_dir_all(&root).await;
    }
}
