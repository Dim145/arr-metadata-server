//! Restoring an archive: what it holds put back into the database, whatever
//! its engine, and its media into the store.
//!
//! In order: the manifest is read, and an archive this server could not
//! read whole — a later layout, a later migration, a later version — is
//! refused before anything is touched. A database that holds rows is only
//! restored over when asked to (`--replace`, or the Backups page, which
//! always does), and then an archive of it is made first: the safety copy,
//! which is how a restore is undone. The archive is read once: its database
//! taken out and checked against the manifest, its media put back into the
//! store as they come — a medium is named by its bytes, so nothing there is
//! overwritten. The copy of the database is brought up to this server's
//! migrations, then copied into the database in one transaction, every
//! table emptied first: a restore that fails part of the way leaves the
//! database as it was. The list of archives is the one table kept as it
//! stands — the archives did not move.

use std::{io::Read as _, path::PathBuf, sync::Arc};

use age::secrecy::SecretString;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use utoipa::ToSchema;

use super::{Ask, Backups, Made, Reason, archive, manifest, manifest::Manifest, snapshot};
use crate::{
    db::{self, transfer},
    media::store::Store,
};

/// The largest medium a restore puts back: larger than any the store keeps.
const MOST_MEDIUM_BYTES: u64 = 256 * 1024 * 1024;

/// The tables a restore leaves as they are.
const KEPT: &[&str] = &["backup"];

/// What a run still running when the archive was made says once restored.
const STILL_RUNNING: &str =
    "still running when the archive restored was made: what it did after is not in it";

/// The archive to restore.
#[derive(Clone, Debug)]
pub enum Source {
    /// One where the archives are kept, by its name.
    Kept(String),
    /// A file anywhere: what the command line is given.
    File(PathBuf),
}

impl Source {
    /// The name the journal and the page say it by.
    pub fn label(&self) -> String {
        match self {
            Self::Kept(name) => name.clone(),
            Self::File(path) => path.display().to_string(),
        }
    }
}

/// A step of a restore, as it is told to whoever follows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// The safety copy is being made.
    Safety,
    /// The archive is being read: its database taken out, its media put
    /// back.
    Reading,
    /// The rows are being copied, a table at a time.
    Copying {
        table: String,
        step: usize,
        steps: usize,
    },
}

/// What a restore is asked to do.
pub struct Request<'a> {
    pub source: Source,
    /// Restore over a database that holds rows, after a safety copy of it.
    pub replace: bool,
    /// What opens the archive, when it is encrypted. The server's own
    /// passphrase is tried when none is given.
    pub passphrase: Option<SecretString>,
    /// Who asked, as the journal names them.
    pub by: Option<String>,
    /// Told of each step.
    pub progress: Option<&'a (dyn Fn(Step) + Send + Sync)>,
}

/// What a restore did.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BackupRestored)]
pub struct Restored {
    /// The archive restored, by its name or its path.
    pub archive: String,
    /// What it said of itself.
    pub manifest: Manifest,
    /// The archive made of the database before it was replaced: what
    /// undoes this restore. `null` when the database was empty.
    #[schema(required)]
    pub safety: Option<String>,
    /// The rows copied into the database.
    pub rows: u64,
    /// The media the archive held that were put back into the store.
    pub media_restored: u64,
    /// Those it held that were not: the store already had them, or there
    /// is no store to put them in.
    pub media_skipped: u64,
    /// How the media the restored rows name stand in the store, when there
    /// is one.
    #[schema(required)]
    pub media: Option<MediaCheck>,
}

/// The media the database names, against the media the store holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BackupMediaCheck)]
pub struct MediaCheck {
    /// Named by a row, not in the store: served from its provider's
    /// address until it is fetched again.
    pub missing: u64,
    /// In the store, named by no row: the sweep takes these away.
    pub unreferenced: u64,
}

impl Backups {
    /// Restore an archive into this database.
    pub async fn restore(&self, request: Request<'_>) -> Result<Restored> {
        let tell = |step: Step| {
            if let Some(progress) = request.progress {
                progress(step);
            }
        };
        let passphrase = request
            .passphrase
            .clone()
            .or_else(|| self.passphrase().cloned());

        let reader = self.open(&request.source).await?;
        let opened_with = passphrase.clone();
        let manifest = tokio::task::spawn_blocking(move || {
            archive::read_manifest(reader, opened_with.as_ref())
        })
        .await??;
        if let Some(refusal) = manifest.refusal(db::latest_migration()) {
            bail!(refusal);
        }
        let expected = manifest
            .database()
            .cloned()
            .context("this archive's manifest names no database")?;

        if !request.replace && holds_rows(self.db()).await? {
            bail!(
                "this database holds rows already — a server that started on it once has its \
                 settings — so a restore would add to them: restore with --replace, which makes \
                 an archive of it first"
            );
        }

        let safety = if request.replace {
            tell(Step::Safety);
            Some(
                self.make(Ask {
                    by: request.by.clone(),
                    ..Ask::new(Reason::Safety)
                })
                .await
                .context("the safety copy could not be made, and nothing was restored")?,
            )
        } else {
            None
        };

        tell(Step::Reading);
        let work = super::Work::new(&self.work)?;
        let database = work.path().join(manifest::DATABASE);
        let reader = self.open(&request.source).await?;
        let taken = {
            let database = database.clone();
            let store = self.media_store.clone();
            tokio::task::spawn_blocking(move || {
                take_apart(reader, passphrase.as_ref(), &database, store)
            })
            .await??
        };
        if taken.database != (expected.size, expected.sha256.clone()) {
            bail!(
                "the archive's database is not the one its manifest describes: the archive is \
                 damaged, and nothing was restored"
            );
        }

        let copy = snapshot::open(&database, false).await?;
        let copied = async {
            let had = copy.last_migration().await?;
            if had > db::latest_migration() {
                bail!(
                    "this archive's database has migration {had}, and this server knows them up \
                     to {}",
                    db::latest_migration()
                );
            }
            copy.migrate()
                .await
                .context("the archive's database could not be brought up to this version")?;
            // What was running when the archive was made — the backup that
            // made it, to begin with — ended where the archive did, as far
            // as the database restored can tell: closed, rather than left
            // running for ever in the history.
            sqlx::query(copy.sql(
                "UPDATE job_run SET status = 'failed', finished_at = ?, error = ? \
                 WHERE status = 'running'",
            ))
            .bind(db::now())
            .bind(STILL_RUNNING)
            .execute(copy.pool())
            .await?;
            let progress = |table: &str, step: usize, steps: usize| {
                tell(Step::Copying {
                    table: table.to_string(),
                    step,
                    steps,
                });
            };
            transfer::run_with(
                &copy,
                self.db(),
                transfer::Options {
                    replace: request.replace,
                    keep: KEPT,
                    progress: Some(&progress),
                    ..transfer::Options::default()
                },
            )
            .await
        }
        .await;
        copy.close().await;
        drop(work);
        let report = copied.context("the rows could not be copied, and nothing was restored")?;

        let media = match &self.media_store {
            Some(store) => Some(check_media(self.db(), store).await?),
            None => None,
        };

        tracing::info!(
            archive = %request.source.label(),
            rows = report.total(),
            media = taken.media_restored,
            "restored a backup"
        );
        Ok(Restored {
            archive: request.source.label(),
            manifest,
            safety: safety.map(|made: Made| made.name),
            rows: report.total(),
            media_restored: taken.media_restored,
            media_skipped: taken.media_skipped,
            media,
        })
    }

    /// Only the manifest of an archive.
    pub async fn manifest_of(
        &self,
        source: &Source,
        passphrase: Option<SecretString>,
    ) -> Result<Manifest> {
        let reader = self.open(source).await?;
        let passphrase = passphrase.or_else(|| self.passphrase().cloned());
        tokio::task::spawn_blocking(move || archive::read_manifest(reader, passphrase.as_ref()))
            .await?
    }

    async fn open(&self, source: &Source) -> Result<Box<dyn std::io::Read + Send>> {
        match source {
            Source::Kept(name) => {
                if !super::is_archive_name(name) {
                    bail!("{name:?} is not the name of an archive");
                }
                self.place()
                    .reader(name)
                    .await?
                    .with_context(|| format!("no archive named {name} is kept"))
            }
            Source::File(path) => {
                let file = std::fs::File::open(path)
                    .with_context(|| format!("could not open {}", path.display()))?;
                Ok(Box::new(file))
            }
        }
    }
}

/// Whether any table a restore replaces holds a row.
async fn holds_rows(db: &db::Db) -> Result<bool> {
    for table in transfer::tables() {
        if KEPT.contains(table) {
            continue;
        }
        let found = sqlx::query(db.sql(&format!("SELECT 1 AS hit FROM {table} LIMIT 1")))
            .fetch_optional(db.pool())
            .await?;
        if found.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// What reading the archive through came to.
struct TakenApart {
    /// The database's size and SHA-256, as taken out.
    database: (u64, String),
    media_restored: u64,
    media_skipped: u64,
}

/// Read the archive once: the database into `database`, the media into the
/// store. A path in the archive never names one on disk: the database goes
/// where it is told, a medium is put by its key, and a key that is not one
/// of the store's is refused. Blocking.
fn take_apart(
    reader: Box<dyn std::io::Read + Send>,
    passphrase: Option<&SecretString>,
    database: &std::path::Path,
    store: Option<Arc<Store>>,
) -> Result<TakenApart> {
    let mut opened = archive::open(reader, passphrase)?;
    let mut entries = opened.entries().context("could not read the archive")?;
    let mut first = entries
        .next()
        .context("this archive is empty")?
        .context("could not read the archive")?;
    archive::manifest_of(&mut first)?;
    drop(first);

    let handle = tokio::runtime::Handle::current();
    let mut taken: Option<(u64, String)> = None;
    let (mut restored, mut skipped) = (0u64, 0u64);
    for entry in entries {
        let mut entry = entry.context("could not read the archive")?;
        let path = entry.path()?.to_string_lossy().to_string();
        if path == manifest::DATABASE {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(database)
                .with_context(|| format!("could not write {}", database.display()))?;
            let hashed = archive::copy_hashed(&mut entry, &mut file)
                .context("could not take the database out of the archive")?;
            file.sync_all()?;
            taken = Some(hashed);
        } else if let Some(key) = path.strip_prefix(manifest::MEDIA) {
            if !crate::media::file::valid_key(key) {
                bail!("the archive holds {path:?}, which is not a medium of this server's");
            }
            let Some(store) = &store else {
                skipped += 1;
                continue;
            };
            if handle.block_on(store.exists(key))? {
                skipped += 1;
                continue;
            }
            if entry.size() > MOST_MEDIUM_BYTES {
                bail!("the archive's medium {key} is larger than any this server keeps");
            }
            let mut bytes = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
            entry
                .by_ref()
                .take(MOST_MEDIUM_BYTES)
                .read_to_end(&mut bytes)?;
            // A medium is named by its bytes: one whose bytes are not those
            // is not put back. A thumbnail is named by its picture's.
            if !crate::media::file::is_thumb(key)
                && archive::sha256_of(&bytes) != crate::media::file::stem_of(key)
            {
                bail!("the archive's medium {key} is damaged");
            }
            let content_type =
                crate::media::file::content_type_of(key).unwrap_or("application/octet-stream");
            handle.block_on(store.put(key, bytes.into(), content_type))?;
            restored += 1;
        }
        // Anything else a later version wrote is passed by: the manifest's
        // layout says what this one must find, and it found it.
    }
    let database = taken.context("this archive holds no database")?;
    Ok(TakenApart {
        database,
        media_restored: restored,
        media_skipped: skipped,
    })
}

/// The media the database names against those the store holds.
pub async fn check_media(db: &db::Db, store: &Store) -> Result<MediaCheck> {
    let mut named = std::collections::HashSet::new();
    for (_, key, thumb) in db::repo::asset::stored_index(db).await? {
        if let Some(small) = crate::media::file::thumb_key(&key, thumb) {
            named.insert(small);
        }
        named.insert(key);
    }
    let held: std::collections::HashSet<String> = store
        .list()
        .await?
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    Ok(MediaCheck {
        missing: named.difference(&held).count() as u64,
        unreferenced: held.difference(&named).count() as u64,
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use serde_json::json;
    use sqlx::{Column as _, Row as _, ValueRef as _};

    use super::*;
    use crate::{
        backup::{Backups, Reason, archive::Medium, index, manifest, scratch},
        config,
        db::{
            Db,
            repo::{self, item::ItemWrite},
        },
        domain::{ExternalIds, MediaItem, MediaKind, fields::Scope},
    };

    /// The archives of `db`, kept in a directory of their own, with the
    /// media in `media` when it is given.
    fn backups_of(db: &Db, media: Option<&Path>) -> Backups {
        let mut config = config::Config::from_env().expect("a configuration");
        config.mode = config::Mode::Single;
        config.media = config::Media {
            storage: if media.is_some() {
                config::MediaStorage::Filesystem
            } else {
                config::MediaStorage::Off
            },
            dir: media.map(Path::to_path_buf).unwrap_or_default(),
            s3: None,
        };
        let store = Store::open(&config.media).unwrap().map(Arc::new);
        Backups::new(&config, db.clone(), store).unwrap()
    }

    /// What an archive is to carry: a work with its ids, a lock on it, an
    /// account, a key, what every instance holds the same of.
    async fn populate(db: &Db, title: &str) -> String {
        let mut work = MediaItem::empty(MediaKind::Series);
        work.title = title.into();
        work.slug = crate::domain::make_slug(title, Some(2026));
        work.year = Some(2026);
        work.popularity = Some(7.25);
        work.overview = Some("An overview with a 'quote' & an ampersand.".into());
        work.external_ids = ExternalIds {
            tmdb: Some(4_242),
            tvdb: Some(81_189),
            imdb: Some("tt0903747".into()),
            ..Default::default()
        };
        repo::item::upsert(
            db,
            ItemWrite {
                item: &work,
                replace_children: false,
            },
        )
        .await
        .unwrap();
        repo::override_field::set(
            db,
            &work.id,
            Scope::Item,
            "title",
            Some(&json!(format!("{title}, by hand"))),
            Some("admin:alice"),
        )
        .await
        .unwrap();
        repo::user::create(
            db,
            repo::user::NewUser {
                username: "alice",
                password_hash: "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA",
                role: repo::user::Role::Admin,
                status: repo::user::Status::Active,
                display_name: Some("Alice"),
                email: None,
                invited_by: None,
                oidc: None,
            },
        )
        .await
        .unwrap();
        repo::client::create(
            db,
            repo::client::NewClient {
                name: "Sonarr",
                key_prefix: "ams_abcdefgh",
                key_hash: &"9".repeat(64),
                scopes: &["read".to_string()],
                expires_at: None,
                note: Some("the living room's"),
                owner_id: None,
            },
        )
        .await
        .unwrap();
        repo::keystore::put(db, repo::keystore::names::COOKIE_KEY, "a key, in base64")
            .await
            .unwrap();
        work.id
    }

    /// A table's rows, each as its values read back, in a stable order —
    /// its columns by name: the engines put a column added later in
    /// different places.
    async fn rows(db: &Db, table: &str) -> Vec<String> {
        let mut found: Vec<String> = sqlx::query(db.sql(&format!("SELECT * FROM {table}")))
            .fetch_all(db.pool())
            .await
            .unwrap()
            .iter()
            .map(|row| {
                let mut cells: Vec<String> = (0..row.columns().len())
                    .map(|i| {
                        let name = row.column(i).name().to_string();
                        let value = if row.try_get_raw(i).unwrap().is_null() {
                            "null".to_string()
                        } else if let Ok(v) = row.try_get::<i64, _>(i) {
                            v.to_string()
                        } else if let Ok(v) = row.try_get::<f64, _>(i) {
                            v.to_string()
                        } else {
                            row.try_get::<String, _>(i).unwrap()
                        };
                        format!("{name}={value}")
                    })
                    .collect();
                cells.sort();
                cells.join(" ")
            })
            .collect();
        found.sort();
        found
    }

    /// The tables whose rows a restore must bring back as they were: the
    /// works, their locks, the accounts and the keys.
    const CARRIED: [&str; 6] = [
        "media_item",
        "media_external_id",
        "media_override",
        "admin_user",
        "api_client",
        "keystore",
    ];

    async fn same_rows(left: &Db, right: &Db) {
        for table in CARRIED {
            let found = rows(right, table).await;
            assert!(!found.is_empty(), "{table} came back empty");
            assert_eq!(rows(left, table).await, found, "{table}");
        }
    }

    fn path_of(backups: &Backups, made: &Made) -> PathBuf {
        backups.place().directory().unwrap().join(&made.name)
    }

    fn request(source: Source) -> Request<'static> {
        Request {
            source,
            replace: false,
            passphrase: None,
            by: None,
            progress: None,
        }
    }

    #[tokio::test]
    async fn an_archive_restores_into_an_empty_database_with_its_locks_and_keys() {
        let source = crate::db::testing::sqlite().await;
        populate(&source, "Breaking Bad").await;
        let backups = backups_of(&source, None);
        let made = backups.make(Ask::new(Reason::Manual)).await.unwrap();
        assert_eq!(made.manifest.engine, "sqlite");
        assert_eq!(made.manifest.migration, db::latest_migration());
        assert!(!made.encrypted);

        let target = crate::db::testing::sqlite().await;
        let steps = std::sync::Mutex::new(Vec::new());
        let told = |step: Step| steps.lock().unwrap().push(step);
        let restored = backups_of(&target, None)
            .restore(Request {
                progress: Some(&told),
                ..request(Source::File(path_of(&backups, &made)))
            })
            .await
            .unwrap();
        assert!(restored.rows >= 6, "{}", restored.rows);
        assert_eq!(restored.safety, None, "an empty database needs none");
        same_rows(&source, &target).await;

        let steps = steps.into_inner().unwrap();
        assert_eq!(steps.first(), Some(&Step::Reading));
        assert!(steps.iter().any(|step| matches!(
            step,
            Step::Copying { table, .. } if table == "media_override"
        )));
    }

    #[tokio::test]
    async fn a_database_that_holds_rows_is_restored_over_only_when_asked_and_after_a_safety_copy() {
        let source = crate::db::testing::sqlite().await;
        populate(&source, "The Wire").await;
        let backups = backups_of(&source, None);
        let made = backups.make(Ask::new(Reason::Manual)).await.unwrap();
        let archive = path_of(&backups, &made);

        // A database of its own, already holding a work, an archive of its
        // own listed.
        let target = crate::db::testing::sqlite().await;
        populate(&target, "Treme").await;
        let restoring = backups_of(&target, None);
        let before = restoring.make(Ask::new(Reason::Scheduled)).await.unwrap();

        let refused = restoring
            .restore(request(Source::File(archive.clone())))
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("--replace"), "{refused}");
        assert_eq!(rows(&target, "media_item").await.len(), 1);

        let restored = restoring
            .restore(Request {
                replace: true,
                ..request(Source::File(archive))
            })
            .await
            .unwrap();
        same_rows(&source, &target).await;

        // What undoes it was made first, and is listed — with the archive the
        // target had: the list of archives is the target's own.
        let safety = restored.safety.expect("a safety copy");
        assert!(safety.contains("-safety"), "{safety}");
        let listed: Vec<String> = index::all(&target)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.name)
            .collect();
        assert!(listed.contains(&safety), "{listed:?}");
        assert!(listed.contains(&before.name), "{listed:?}");

        // And it undoes it.
        restoring
            .restore(Request {
                replace: true,
                ..request(Source::Kept(safety))
            })
            .await
            .unwrap();
        let titles = rows(&target, "media_item").await;
        assert!(
            titles.iter().all(|row| row.contains("title=Treme")),
            "{titles:?}"
        );
    }

    #[tokio::test]
    async fn an_encrypted_archive_opens_with_its_passphrase_or_the_servers_own() {
        let source = crate::db::testing::sqlite().await;
        populate(&source, "Severance").await;
        let backups = backups_of(&source, None).with_passphrase("a passphrase of the server's");
        let made = backups.make(Ask::new(Reason::Manual)).await.unwrap();
        assert!(made.encrypted);
        assert!(made.name.ends_with(".tar.zst.age"), "{}", made.name);
        let archive = path_of(&backups, &made);

        let target = crate::db::testing::sqlite().await;
        let refused = backups_of(&target, None)
            .restore(request(Source::File(archive.clone())))
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("encrypted"), "{refused}");

        let wrong = backups_of(&target, None)
            .restore(Request {
                passphrase: Some(SecretString::from("another".to_string())),
                ..request(Source::File(archive.clone()))
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(wrong.contains("does not open"), "{wrong}");

        backups_of(&target, None)
            .with_passphrase("a passphrase of the server's")
            .restore(request(Source::File(archive)))
            .await
            .unwrap();
        same_rows(&source, &target).await;
    }

    #[tokio::test]
    async fn a_damaged_archive_leaves_the_database_as_it_was() {
        let source = crate::db::testing::sqlite().await;
        populate(&source, "Dark").await;
        let backups = backups_of(&source, None);
        let made = backups.make(Ask::new(Reason::Manual)).await.unwrap();
        let archive = path_of(&backups, &made);

        // A byte changed past the manifest, in the database's part.
        let mut bytes = std::fs::read(&archive).unwrap();
        let at = bytes.len() * 3 / 4;
        bytes[at] ^= 0x5a;
        std::fs::write(&archive, &bytes).unwrap();

        let target = crate::db::testing::sqlite().await;
        populate(&target, "Kept as it was").await;
        let restoring = backups_of(&target, None);
        let before = rows(&target, "media_item").await;
        assert!(
            restoring
                .restore(Request {
                    replace: true,
                    ..request(Source::File(archive))
                })
                .await
                .is_err()
        );
        assert_eq!(rows(&target, "media_item").await, before);
    }

    #[tokio::test]
    async fn an_archive_of_a_later_migration_is_refused_before_anything_is_touched() {
        let dir = scratch();
        let database = dir.join("database.sqlite");
        let copy = crate::backup::snapshot::open(&database, true)
            .await
            .unwrap();
        copy.migrate().await.unwrap();
        copy.close().await;
        let (size, sha256) = crate::backup::archive::hash_file(&database).unwrap();
        let later = Manifest {
            format: manifest::FORMAT,
            server: env!("CARGO_PKG_VERSION").into(),
            migration: db::latest_migration() + 1,
            created_at: crate::db::now(),
            engine: "postgres".into(),
            reason: "manual".into(),
            upgrade_to: None,
            files: vec![manifest::File {
                path: manifest::DATABASE.into(),
                size,
                sha256,
            }],
            media: manifest::Media {
                included: false,
                store: "off".into(),
                files: 0,
                bytes: 0,
                note: None,
            },
        };
        let archive = dir.join("ams-20261019-020000-manual.tar.zst");
        let mut file = std::fs::File::create(&archive).unwrap();
        crate::backup::archive::write(&mut file, None, &later, &database, &[]).unwrap();
        drop(file);

        let target = crate::db::testing::sqlite().await;
        populate(&target, "Untouched").await;
        let restoring = backups_of(&target, None);
        let refused = restoring
            .restore(Request {
                replace: true,
                ..request(Source::File(archive))
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("migration"), "{refused}");
        // Refused before even the safety copy.
        assert!(index::all(&target).await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_media_kept_on_disk_travel_with_the_archive_when_asked() {
        let from = scratch();
        let picture = b"a picture's bytes".to_vec();
        let key = format!("{}.jpg", archive::sha256_of(&picture));
        let filed = from.join(&key[0..2]).join(&key[2..4]);
        std::fs::create_dir_all(&filed).unwrap();
        std::fs::write(filed.join(&key), &picture).unwrap();

        let source = crate::db::testing::sqlite().await;
        populate(&source, "Fan-Kai").await;
        let backups = backups_of(&source, Some(&from));
        let made = backups
            .make(Ask {
                include_media: true,
                ..Ask::new(Reason::Manual)
            })
            .await
            .unwrap();
        assert!(made.manifest.media.included);
        assert_eq!(made.manifest.media.files, 1);
        assert_eq!(made.manifest.media.store, "filesystem");

        let without = backups.make(Ask::new(Reason::Manual)).await.unwrap();
        assert!(!without.manifest.media.included);
        assert!(
            without.manifest.media.note.is_some(),
            "it says where they are"
        );

        let to = scratch();
        let target = crate::db::testing::sqlite().await;
        let restored = backups_of(&target, Some(&to))
            .restore(request(Source::File(path_of(&backups, &made))))
            .await
            .unwrap();
        assert_eq!(restored.media_restored, 1);
        assert_eq!(
            std::fs::read(to.join(&key[0..2]).join(&key[2..4]).join(&key)).unwrap(),
            picture
        );
        // No row names it: the sweep's to take away.
        assert_eq!(
            restored.media,
            Some(MediaCheck {
                missing: 0,
                unreferenced: 1
            })
        );
        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    #[tokio::test]
    async fn the_schedule_keeps_its_newest_archives_and_nobody_elses() {
        let db = crate::db::testing::sqlite().await;
        let backups = backups_of(&db, None);
        let made: Vec<Made> = {
            let mut made = Vec::new();
            for reason in [
                Reason::Scheduled,
                Reason::Manual,
                Reason::Scheduled,
                Reason::Safety,
                Reason::Scheduled,
            ] {
                made.push(backups.make(Ask::new(reason)).await.unwrap());
            }
            made
        };
        // One put there by hand, from another server, older than them all.
        let dir = backups.place().directory().unwrap();
        std::fs::copy(
            dir.join(&made[0].name),
            dir.join("ams-20200101-000000-scheduled.tar.zst"),
        )
        .unwrap();

        let removed = backups.prune(2).await.unwrap();
        assert_eq!(
            removed,
            [
                made[0].name.clone(),
                "ams-20200101-000000-scheduled.tar.zst".to_string()
            ]
        );
        let left: Vec<String> = backups
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|archive| archive.name)
            .collect();
        for kept in [&made[1], &made[2], &made[3], &made[4]] {
            assert!(left.contains(&kept.name), "{} in {left:?}", kept.name);
        }
        assert_eq!(left.len(), 4);
    }

    #[tokio::test]
    async fn a_postgresql_database_is_archived_restored_into_sqlite_and_back() {
        let Some(source) = crate::db::testing::postgres().await else {
            return;
        };
        populate(&source, "Twin Peaks").await;
        let backups = backups_of(&source, None);
        let made = backups.make(Ask::new(Reason::Manual)).await.unwrap();
        assert_eq!(made.manifest.engine, "postgres");
        let archive = path_of(&backups, &made);

        let sqlite = crate::db::testing::sqlite().await;
        backups_of(&sqlite, None)
            .restore(request(Source::File(archive.clone())))
            .await
            .unwrap();
        same_rows(&source, &sqlite).await;

        // And back, from an archive the SQLite made, into another
        // PostgreSQL: the same rows again.
        let from_sqlite = backups_of(&sqlite, None);
        let again = from_sqlite.make(Ask::new(Reason::Manual)).await.unwrap();
        assert_eq!(again.manifest.engine, "sqlite");
        let postgres = crate::db::testing::postgres().await.unwrap();
        backups_of(&postgres, None)
            .restore(request(Source::File(path_of(&from_sqlite, &again))))
            .await
            .unwrap();
        same_rows(&source, &postgres).await;
    }

    #[test]
    fn a_medium_whose_bytes_are_not_its_name_is_not_put_back() {
        let dir = scratch();
        let database = dir.join("database.sqlite");
        std::fs::write(&database, b"not read").unwrap();
        let forged = dir.join("forged");
        std::fs::write(&forged, b"other bytes").unwrap();
        let key = format!("{}.jpg", "b".repeat(64));
        let (size, sha256) = archive::hash_file(&database).unwrap();
        let mut bytes = Vec::new();
        archive::write(
            &mut bytes,
            None,
            &Manifest {
                format: manifest::FORMAT,
                server: env!("CARGO_PKG_VERSION").into(),
                migration: 1,
                created_at: crate::db::now(),
                engine: "sqlite".into(),
                reason: "manual".into(),
                upgrade_to: None,
                files: vec![manifest::File {
                    path: manifest::DATABASE.into(),
                    size,
                    sha256,
                }],
                media: manifest::Media {
                    included: true,
                    store: "filesystem".into(),
                    files: 1,
                    bytes: 11,
                    note: None,
                },
            },
            &database,
            &[Medium {
                key: &key,
                path: &forged,
                size: 11,
            }],
        )
        .unwrap();

        let media = scratch();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let store = Store::open(&config::Media {
            storage: config::MediaStorage::Filesystem,
            dir: media.clone(),
            s3: None,
        })
        .unwrap()
        .map(Arc::new);
        let refused = runtime
            .block_on(async {
                tokio::task::spawn_blocking(move || {
                    take_apart(
                        Box::new(std::io::Cursor::new(bytes)),
                        None,
                        &dir.join("taken.sqlite"),
                        store,
                    )
                })
                .await
                .unwrap()
            })
            .err()
            .expect("refused")
            .to_string();
        assert!(refused.contains("damaged"), "{refused}");
        let _ = std::fs::remove_dir_all(&media);
    }
}
