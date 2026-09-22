//! Downloading artwork beside the `.nfo` documents that reference it.
//!
//! A `.nfo` names its artwork by URL, and what the consumer does with that URL
//! is up to the consumer: Kodi fetches it, Plex's Personal Media agent often
//! will not, and neither works on a library that is offline or behind a network
//! the media server cannot reach. Local files are read by all of them.
//!
//! So the export writes the pictures next to the documents, under the names
//! Kodi defined and Plex adopted — `poster.jpg`, `fanart.jpg`,
//! `season01-poster.jpg`, `S01E01-thumb.jpg`, and cast headshots in `.actors/`.
//! Nothing here invents a convention; these are the names those programs look
//! for.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::domain::{CoverType, Episode, MediaItem, MediaKind};

/// Refuse anything larger. Artwork is measured in megabytes; a hundred of them
/// is a redirect to something that is not a picture.
const MAX_BYTES: u64 = 25 * 1024 * 1024;

/// One picture to place at one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    /// Relative to the export root, using `/` as the separator.
    pub path: String,
    pub url: String,
}

/// Every picture a work's documents refer to, and where each one belongs.
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
    for image in &item.images {
        let Some(stem) = file_stem(image.cover_type, image.season_number) else {
            continue;
        };

        if !taken.insert(format!("{folder}/{stem}")) {
            continue;
        }

        downloads.push(Download {
            path: format!("{folder}/{stem}.{}", extension(&image.url)),
            url: image.url.clone(),
        });
    }

    for episode in &item.episodes {
        let Some(url) = episode.image.as_deref().filter(|u| !u.trim().is_empty()) else {
            continue;
        };

        let path = format!("{folder}/{}", episode_name(episode, url));

        if taken.insert(path.clone()) {
            downloads.push(Download {
                path,
                url: url.to_string(),
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
            });
        }
    }

    downloads
}

/// The name Kodi looks for, without an extension, or `None` for artwork it has
/// no place for.
fn file_stem(cover: CoverType, season: Option<i32>) -> Option<String> {
    // A season's poster sits in the show's folder, numbered.
    if let Some(season) = season.filter(|s| *s >= 0) {
        return match cover {
            CoverType::Poster => Some(format!("season{season:02}-poster")),
            CoverType::Banner => Some(format!("season{season:02}-banner")),
            CoverType::Fanart => Some(format!("season{season:02}-fanart")),
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

/// Fetch one picture and write it, unless it is already there.
///
/// Returns `false` when the file already existed, so a second export costs
/// nothing rather than refetching a library's worth of artwork.
pub async fn fetch_one(http: &reqwest::Client, root: &Path, download: &Download) -> Result<bool> {
    let destination = resolve(root, &download.path)?;

    if tokio::fs::metadata(&destination)
        .await
        .is_ok_and(|m| m.len() > 0)
    {
        return Ok(false);
    }

    // Again here, and not only where the URL was stored: a name that pointed
    // somewhere ordinary when it was accepted can point at loopback by the time
    // this runs, and rows written before the check existed are still in there.
    if crate::outbound::resolves_internally(&download.url).await {
        anyhow::bail!(
            "{} resolves to an address only this server can reach",
            download.url
        );
    }

    let response = http
        .get(&download.url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .with_context(|| format!("could not fetch {}", download.url))?;

    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("{status} fetching {}", download.url);
    }

    if response.content_length().is_some_and(|n| n > MAX_BYTES) {
        anyhow::bail!("{} is larger than this export will write", download.url);
    }

    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("could not read {}", download.url))?;

    if bytes.len() as u64 > MAX_BYTES {
        anyhow::bail!("{} is larger than this export will write", download.url);
    }

    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("could not create {}", parent.display()))?;
    }

    // Written beside the target and renamed, so a half-downloaded picture is
    // never left behind under a name a media server will read.
    let staged = destination.with_extension("partial");
    tokio::fs::write(&staged, &bytes)
        .await
        .with_context(|| format!("could not write {}", staged.display()))?;
    tokio::fs::rename(&staged, &destination)
        .await
        .with_context(|| format!("could not place {}", destination.display()))?;

    Ok(true)
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
    fn the_extension_follows_the_url() {
        assert_eq!(extension("https://x.invalid/a.png"), "png");
        assert_eq!(extension("https://x.invalid/a.jpg?size=w500"), "jpg");
        assert_eq!(extension("https://x.invalid/nothing"), "jpg");
    }
}
