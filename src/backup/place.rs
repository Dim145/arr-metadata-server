//! Where the archives are kept: a directory, or — among several instances,
//! whose media are in a bucket already — that bucket, under `backups/`.
//!
//! An archive is only where the others are once it is whole: written under
//! a dotted name beside them and renamed, in a directory; uploaded in parts
//! the bucket shows nobody until the last, in a bucket. A backup cut short
//! by a crash or a full disk leaves no archive behind that a restore would
//! take for one.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use bytes::Bytes;
use futures::{StreamExt as _, TryStreamExt as _, stream::BoxStream};
use object_store::{ObjectStore, ObjectStoreExt as _, buffered::BufWriter, path::Path as Key};
use tokio_util::io::{StreamReader, SyncIoBridge};

use crate::config;

/// The bucket's folder the archives are kept in, under the media's prefix
/// when the bucket is shared with other things.
const FOLDER: &str = "backups";

/// How much of an archive is held before a part of it is sent to the
/// bucket, and how many parts are in flight at once: within the memory a
/// container is given, at a pace a home network keeps.
const PART_BYTES: usize = 8 * 1024 * 1024;
const PARTS_IN_FLIGHT: usize = 2;

pub enum Place {
    Directory(PathBuf),
    Bucket(Bucket),
}

pub struct Bucket {
    store: Arc<dyn ObjectStore>,
    folder: Key,
    /// What the page calls it: the bucket, and the folder in it.
    label: String,
    /// How much of an archive is sent at a time.
    part_bytes: usize,
}

/// An archive as found where they are kept.
#[derive(Clone, Debug)]
pub struct Stored {
    pub name: String,
    pub size: u64,
}

impl Place {
    /// The bucket the media are kept in, opened for the archives: the same
    /// address, region and keys. A client of its own rather than the media
    /// store's, which files every key two levels deep under its hash and
    /// refuses a name that is not one.
    pub fn bucket(s3: &config::S3) -> Result<Self> {
        let mut builder = object_store::aws::AmazonS3Builder::new()
            .with_region(&s3.region)
            .with_bucket_name(&s3.bucket)
            .with_access_key_id(&s3.access_key)
            .with_secret_access_key(&s3.secret_key)
            .with_virtual_hosted_style_request(!s3.path_style)
            .with_client_options(
                object_store::ClientOptions::new()
                    .with_allow_http(
                        s3.endpoint
                            .as_deref()
                            .is_some_and(|e| e.starts_with("http://")),
                    )
                    // A part of an archive is larger than any medium.
                    .with_timeout(Duration::from_secs(600))
                    .with_connect_timeout(Duration::from_secs(10)),
            );
        if let Some(endpoint) = &s3.endpoint {
            builder = builder.with_endpoint(endpoint);
        }
        let store = builder
            .build()
            .context("could not open the bucket for the archives")?;
        let folder = match &s3.prefix {
            Some(prefix) => format!("{prefix}/{FOLDER}"),
            None => FOLDER.to_string(),
        };
        Ok(Self::Bucket(Bucket {
            store: Arc::new(store),
            label: format!("{}/{folder}", s3.bucket),
            folder: Key::from(folder),
            part_bytes: PART_BYTES,
        }))
    }

    /// A store of any kind standing in for the bucket, sending an archive in
    /// parts of `part_bytes`: a test's, in memory.
    #[cfg(test)]
    pub fn bucket_of(store: Arc<dyn ObjectStore>, folder: &str, part_bytes: usize) -> Self {
        Self::Bucket(Bucket {
            store,
            label: format!("a bucket/{folder}"),
            folder: Key::from(folder),
            part_bytes,
        })
    }

    /// `directory` or `bucket`, as a row and the page say it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Directory(_) => "directory",
            Self::Bucket(_) => "bucket",
        }
    }

    /// Where it is, in words: the directory's path, or the bucket and the
    /// folder in it.
    pub fn label(&self) -> String {
        match self {
            Self::Directory(dir) => dir.display().to_string(),
            Self::Bucket(bucket) => bucket.label.clone(),
        }
    }

    /// The directory, when it is one.
    pub fn directory(&self) -> Option<&Path> {
        match self {
            Self::Directory(dir) => Some(dir),
            Self::Bucket(_) => None,
        }
    }

    /// Every archive kept, newest first by name — which is by date.
    pub async fn list(&self) -> Result<Vec<Stored>> {
        let mut found = match self {
            Self::Directory(dir) => list_directory(dir).await?,
            Self::Bucket(bucket) => {
                let mut listed = bucket.store.list(Some(&bucket.folder));
                let mut found = Vec::new();
                while let Some(meta) = listed.next().await {
                    let meta = meta.context("could not list the archives in the bucket")?;
                    let Some(name) = meta.location.filename() else {
                        continue;
                    };
                    // Only the folder's own: not one in a folder below it.
                    if meta
                        .location
                        .prefix_match(&bucket.folder)
                        .map(Iterator::count)
                        != Some(1)
                        || !super::is_archive_name(name)
                    {
                        continue;
                    }
                    found.push(Stored {
                        name: name.to_string(),
                        size: meta.size,
                    });
                }
                found
            }
        };
        found.sort_by(|a, b| b.name.cmp(&a.name));
        Ok(found)
    }

    /// The archive's size, when it is kept.
    pub async fn size(&self, name: &str) -> Result<Option<u64>> {
        match self {
            Self::Directory(dir) => match tokio::fs::metadata(dir.join(name)).await {
                Ok(meta) if meta.is_file() => Ok(Some(meta.len())),
                Ok(_) => Ok(None),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e).with_context(|| format!("could not look for {name}")),
            },
            Self::Bucket(bucket) => match bucket.store.head(&bucket.key(name)).await {
                Ok(meta) => Ok(Some(meta.size)),
                Err(object_store::Error::NotFound { .. }) => Ok(None),
                Err(e) => Err(e).with_context(|| format!("could not look for {name}")),
            },
        }
    }

    /// Take an archive away; one already gone is no error, and says so.
    pub async fn delete(&self, name: &str) -> Result<bool> {
        match self {
            Self::Directory(dir) => match tokio::fs::remove_file(dir.join(name)).await {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(e).with_context(|| format!("could not delete {name}")),
            },
            Self::Bucket(bucket) => {
                if self.size(name).await?.is_none() {
                    return Ok(false);
                }
                match bucket.store.delete(&bucket.key(name)).await {
                    Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(true),
                    Err(e) => Err(e).with_context(|| format!("could not delete {name}")),
                }
            }
        }
    }

    /// The archive's bytes as they come, and how many there are: what a
    /// download is answered with, never held whole.
    pub async fn stream(
        &self,
        name: &str,
    ) -> Result<Option<(BoxStream<'static, std::io::Result<Bytes>>, u64)>> {
        match self {
            Self::Directory(dir) => {
                let file = match tokio::fs::File::open(dir.join(name)).await {
                    Ok(file) => file,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(e) => return Err(e).with_context(|| format!("could not open {name}")),
                };
                let size = file.metadata().await?.len();
                Ok(Some((
                    tokio_util::io::ReaderStream::new(file).boxed(),
                    size,
                )))
            }
            Self::Bucket(bucket) => match bucket.store.get(&bucket.key(name)).await {
                Ok(found) => {
                    let size = found.meta.size;
                    Ok(Some((
                        found
                            .into_stream()
                            .map_err(|e| std::io::Error::other(e.to_string()))
                            .boxed(),
                        size,
                    )))
                }
                Err(object_store::Error::NotFound { .. }) => Ok(None),
                Err(e) => Err(e).with_context(|| format!("could not read {name}")),
            },
        }
    }

    /// The archive, for a blocking reader — tar, zstd and age read
    /// synchronously — to take into a blocking task. Made here, on the
    /// runtime, which the bucket's reader reaches back into.
    pub async fn reader(&self, name: &str) -> Result<Option<Box<dyn Read + Send>>> {
        match self {
            Self::Directory(dir) => match std::fs::File::open(dir.join(name)) {
                Ok(file) => Ok(Some(Box::new(file))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e).with_context(|| format!("could not open {name}")),
            },
            Self::Bucket(_) => {
                let Some((stream, _)) = self.stream(name).await? else {
                    return Ok(None);
                };
                Ok(Some(Box::new(SyncIoBridge::new(StreamReader::new(stream)))))
            }
        }
    }

    /// Somewhere to write the archive `name` to, a blocking writer for the
    /// same reason. It is not listed until [`Writer::finish`].
    pub async fn writer(&self, name: &str) -> Result<Writer> {
        match self {
            Self::Directory(dir) => {
                tokio::fs::create_dir_all(dir).await.with_context(|| {
                    format!("could not create the backups' directory {}", dir.display())
                })?;
                Writer::file(&dir.join(name))
            }
            Self::Bucket(bucket) => {
                let upload = BufWriter::with_capacity(
                    bucket.store.clone(),
                    bucket.key(name),
                    bucket.part_bytes,
                )
                .with_max_concurrency(PARTS_IN_FLIGHT);
                Ok(Writer(Inner::Upload(SyncIoBridge::new(upload))))
            }
        }
    }
}

impl Bucket {
    fn key(&self, name: &str) -> Key {
        self.folder.clone().join(name)
    }
}

/// What an archive is written under in a directory until it is whole: a
/// name no listing takes for an archive.
fn partial_name(name: &str) -> String {
    format!(".{name}.partial")
}

async fn list_directory(dir: &Path) -> Result<Vec<Stored>> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        // None made yet: nothing kept, rather than a failure.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(e).with_context(|| format!("could not list {}", dir.display()));
        }
    };
    let mut found = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        if !super::is_archive_name(&name) {
            continue;
        }
        let meta = entry.metadata().await?;
        if !meta.is_file() {
            continue;
        }
        found.push(Stored {
            name,
            size: meta.len(),
        });
    }
    Ok(found)
}

/// An archive being written: a blocking writer, finished or abandoned.
pub struct Writer(Inner);

enum Inner {
    File {
        file: std::fs::File,
        partial: PathBuf,
        whole: PathBuf,
    },
    /// Written to its own name: the bucket shows an upload in parts to
    /// nobody until its last part is sent and the whole is completed.
    Upload(SyncIoBridge<BufWriter>),
}

impl Writer {
    /// A file at `path`, written under a dotted name beside it until it is
    /// whole: what the command line's `--out` writes to.
    pub fn file(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .context("an archive needs a file name")?
            .to_string_lossy()
            .to_string();
        let partial = path.with_file_name(partial_name(&name));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)
            .with_context(|| format!("could not write {}", partial.display()))?;
        Ok(Self(Inner::File {
            file,
            partial,
            whole: path.to_path_buf(),
        }))
    }

    /// Make the archive whole where it is kept, and listed: the file synced
    /// and renamed to its name; the upload completed. Blocking.
    pub fn finish(self) -> Result<()> {
        match self.0 {
            Inner::File {
                file,
                partial,
                whole,
            } => {
                file.sync_all()
                    .with_context(|| format!("could not write {}", partial.display()))?;
                drop(file);
                if whole.exists() {
                    let _ = std::fs::remove_file(&partial);
                    anyhow::bail!("{} exists already", whole.display());
                }
                std::fs::rename(&partial, &whole)
                    .with_context(|| format!("could not name {}", whole.display()))?;
                // The rename itself is kept only once the directory is.
                if let Some(dir) = whole.parent()
                    && let Ok(dir) = std::fs::File::open(dir)
                {
                    let _ = dir.sync_all();
                }
                Ok(())
            }
            Inner::Upload(mut bridge) => bridge
                .shutdown()
                .context("could not complete the archive's upload"),
        }
    }

    /// Give the archive up: nothing of it is left. Blocking.
    pub fn abandon(self) {
        match self.0 {
            Inner::File { file, partial, .. } => {
                drop(file);
                let _ = std::fs::remove_file(&partial);
            }
            Inner::Upload(bridge) => {
                let mut upload = bridge.into_inner();
                let _ = tokio::runtime::Handle::current().block_on(upload.abort());
            }
        }
    }
}

impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match &mut self.0 {
            Inner::File { file, .. } => file.write(buf),
            Inner::Upload(bridge) => bridge.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match &mut self.0 {
            Inner::File { file, .. } => file.flush(),
            Inner::Upload(bridge) => bridge.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    use object_store::{PutPayload, memory::InMemory};

    use super::*;
    use crate::backup::{Ask, Reason, restore};

    #[tokio::test]
    async fn an_archive_is_kept_in_a_bucket_listed_read_back_and_taken_away() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        // Something else in the bucket, and an archive's name a level below
        // the folder: neither is one of its archives.
        store
            .put(
                &Key::from("media/ab/cd/x.jpg"),
                PutPayload::from_static(b"x"),
            )
            .await
            .unwrap();
        store
            .put(
                &Key::from("media/backups/old/ams-20200101-000000-manual.tar.zst"),
                PutPayload::from_static(b"x"),
            )
            .await
            .unwrap();
        // Sent a kilobyte at a time: the upload goes in parts.
        let place = || Place::bucket_of(store.clone(), "media/backups", 1024);

        let source = crate::db::testing::sqlite().await;
        let mut work = crate::domain::MediaItem::empty(crate::domain::MediaKind::Movie);
        work.title = "Kept in a bucket".into();
        work.slug = crate::domain::make_slug(&work.title, Some(2026));
        crate::db::repo::item::upsert(
            &source,
            crate::db::repo::item::ItemWrite {
                item: &work,
                replace_children: false,
            },
        )
        .await
        .unwrap();
        let backups = backups_of(&source).with_place(place());
        let made = backups.make(Ask::new(Reason::Manual)).await.unwrap();
        assert!(made.size > 1024, "more than one part: {}", made.size);

        let listed = place().list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, made.name);
        assert_eq!(listed[0].size, made.size);
        assert_eq!(place().kind(), "bucket");

        // Streamed as a download, the bytes the archive was made of.
        let (stream, size) = place().stream(&made.name).await.unwrap().unwrap();
        assert_eq!(size, made.size);
        let bytes: Vec<u8> = stream.try_collect::<Vec<Bytes>>().await.unwrap().concat();
        assert_eq!(crate::backup::archive::sha256_of(&bytes), made.sha256);

        // Restored from where it is kept, by its name.
        let target = crate::db::testing::sqlite().await;
        backups_of(&target)
            .with_place(place())
            .restore(restore::Request {
                source: restore::Source::Kept(made.name.clone()),
                replace: false,
                passphrase: None,
                by: None,
                progress: None,
            })
            .await
            .unwrap();
        let titles: Vec<String> = sqlx::query("SELECT title FROM media_item")
            .fetch_all(target.pool())
            .await
            .unwrap()
            .iter()
            .map(|row| sqlx::Row::get::<String, _>(row, "title"))
            .collect();
        assert_eq!(titles, ["Kept in a bucket"]);

        assert!(place().delete(&made.name).await.unwrap());
        assert!(!place().delete(&made.name).await.unwrap());
        assert!(place().list().await.unwrap().is_empty());
        assert!(place().stream(&made.name).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn an_upload_given_up_leaves_nothing_in_the_bucket() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let place = Place::bucket_of(store.clone(), "backups", 1024);
        let name = "ams-20261019-020000-manual.tar.zst";
        let mut writer = place.writer(name).await.unwrap();
        tokio::task::spawn_blocking(move || {
            writer.write_all(&[7; 4096]).unwrap();
            writer.abandon();
        })
        .await
        .unwrap();
        assert!(place.list().await.unwrap().is_empty());
        assert_eq!(place.size(name).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_file_is_listed_only_once_whole() {
        let dir = crate::backup::scratch();
        let place = Place::Directory(dir.clone());
        let name = "ams-20261019-020000-manual.tar.zst";
        let mut writer = place.writer(name).await.unwrap();
        writer.write_all(b"half").unwrap();
        assert!(place.list().await.unwrap().is_empty(), "not yet");
        writer.finish().unwrap();
        let listed = place.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].size, 4);
        // A second one of the same name is refused, and leaves the first.
        let mut again = place.writer(name).await.unwrap();
        again.write_all(b"other").unwrap();
        assert!(again.finish().is_err());
        assert_eq!(place.size(name).await.unwrap(), Some(4));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn backups_of(db: &crate::db::Db) -> crate::backup::Backups {
        let mut config = crate::config::Config::from_env().expect("a configuration");
        config.mode = crate::config::Mode::Single;
        config.media.storage = crate::config::MediaStorage::Off;
        crate::backup::Backups::new(&config, db.clone(), None).unwrap()
    }
}
