//! Backups: the database — and, when asked, the media kept on disk — as one
//! archive, which restores on either engine.
//!
//! An archive is a tar compressed with zstd, named for when and why it was
//! made — `ams-20261019-020000-scheduled.tar.zst` — that holds, in order:
//!
//! * `manifest.json`: the archive's layout, the version and the last
//!   migration of the server that made it, when, from which engine, why,
//!   each file's size and SHA-256, and what it holds of the media;
//! * `database.sqlite`: the whole database as one SQLite file, whatever the
//!   engine it was read from — a `VACUUM INTO` of an SQLite, a `transfer`
//!   of a PostgreSQL — which a restore copies into whichever engine it is
//!   restored to;
//! * `media/<key>`: when asked for, and when they are kept on disk, the
//!   media, each under its key, which is the SHA-256 of its bytes. Media in
//!   a bucket are not: the bucket is backed up as a bucket is, and the
//!   manifest says so.
//!
//! Given a passphrase — `AMS_BACKUP_PASSPHRASE`, or one given to the command
//! line — the whole archive is encrypted as age writes it, and named
//! `….tar.zst.age`. It is the reason encryption is offered: what the
//! database holds — the accounts' password hashes, the keys' hashes, the
//! webhooks' signing secrets and tokens, the identity provider's settings,
//! and among several instances the authority the clients trust — is what
//! lets somebody into this server and the services it calls.
//!
//! The archives are kept in a directory, `AMS_BACKUP_DIR` — `backups` beside
//! the SQLite database by default — or, among several instances whose media
//! are in a bucket, in that bucket, under `backups/`, where every instance
//! finds them.

pub mod archive;
pub mod lock;
pub mod maintenance;
pub mod manifest;
pub mod place;
pub mod restore;
pub mod snapshot;

mod index;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use age::secrecy::SecretString;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub use maintenance::Maintenance;
pub use manifest::Manifest;
pub use place::Place;

use crate::{
    config::{self, Config},
    db::{Db, Dialect},
    media::store::Store,
};

/// Where the archives are kept, when it is a directory: read by the backups
/// alone, so here rather than with the rest of the configuration.
pub const DIR_VARIABLE: &str = "AMS_BACKUP_DIR";

/// The passphrase every archive the server makes is encrypted with, when
/// it is set.
pub const PASSPHRASE_VARIABLE: &str = "AMS_BACKUP_PASSPHRASE";

/// Whether a start that applies migrations makes an archive first: unless
/// this says `false`, which is how a start stopped by an archive it could
/// not make goes on without one.
pub const BEFORE_UPGRADE_VARIABLE: &str = "AMS_BACKUP_BEFORE_UPGRADE";

/// The setting that says whether a PostgreSQL is archived before its
/// migrations; an SQLite file always is.
pub const BEFORE_UPGRADE_SETTING: &str = "backup.beforeUpgrade";

/// How many archives made before upgrades are kept.
pub const KEEP_UPGRADES_SETTING: &str = "backup.keepUpgrades";

/// The archives made before upgrades kept when nothing says how many: the
/// way back from this upgrade and the one before.
pub const DEFAULT_KEEP_UPGRADES: i64 = 2;

/// What every archive's name begins with.
const PREFIX: &str = "ams-";

/// What a plain archive's name ends with, and an encrypted one's.
const PLAIN: &str = ".tar.zst";
const SEALED: &str = ".tar.zst.age";

/// Why an archive was made: said in its name, and in its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = BackupReason)]
pub enum Reason {
    /// The schedule's: the only ones the retention takes away.
    Scheduled,
    /// Asked for from the Backups page.
    Manual,
    /// Made before a restore, so that the restore can be undone.
    Safety,
    /// Made by `arr-metadata-server backup`.
    Command,
    /// Made by a start, before it applied the migrations a new version
    /// brought: what going back to the version before restores. Kept to a
    /// retention of their own (`backup.keepUpgrades`).
    Upgrade,
}

impl Reason {
    pub const ALL: [Reason; 5] = [
        Self::Scheduled,
        Self::Manual,
        Self::Safety,
        Self::Command,
        Self::Upgrade,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Manual => "manual",
            Self::Safety => "safety",
            Self::Command => "command",
            Self::Upgrade => "upgrade",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.as_str() == text)
    }
}

/// What an archive's name says of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Named {
    pub at: chrono::DateTime<chrono::Utc>,
    pub reason: Reason,
    /// For an archive made before an upgrade, the migration the start went
    /// on to apply.
    pub upgrade_to: Option<i64>,
    pub encrypted: bool,
}

/// The name an archive made at `at` for `reason` is kept under: the
/// migration an upgrade went on to after the reason, `-m0037`, and `-2`,
/// `-3` after that when two are made in the same second.
pub fn name_for(
    at: chrono::DateTime<chrono::Utc>,
    reason: Reason,
    upgrade_to: Option<i64>,
    encrypted: bool,
    nth: u32,
) -> String {
    let to = upgrade_to
        .map(|version| format!("-m{version:04}"))
        .unwrap_or_default();
    let again = if nth > 1 {
        format!("-{nth}")
    } else {
        String::new()
    };
    format!(
        "{PREFIX}{}-{}{to}{again}{}",
        at.format("%Y%m%d-%H%M%S"),
        reason.as_str(),
        if encrypted { SEALED } else { PLAIN }
    )
}

/// Read an archive's name back: when and why it was made, and whether it is
/// encrypted. `None` for any other name.
pub fn parse_name(name: &str) -> Option<Named> {
    let (stem, encrypted) = match name.strip_suffix(SEALED) {
        Some(stem) => (stem, true),
        None => (name.strip_suffix(PLAIN)?, false),
    };
    let rest = stem.strip_prefix(PREFIX)?;
    // `20261019-020000-scheduled`, with `-2` after it the second time.
    let (date, rest) = rest.split_at_checked(8)?;
    let rest = rest.strip_prefix('-')?;
    let (time, rest) = rest.split_at_checked(6)?;
    let rest = rest.strip_prefix('-')?;
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    // The reason, then an upgrade's `-m0037`, then a second one's `-2`.
    let mut parts = rest.split('-');
    let reason = Reason::parse(parts.next()?)?;
    let mut upgrade_to = None;
    let mut counted = false;
    for part in parts {
        match part.strip_prefix('m') {
            Some(version)
                if reason == Reason::Upgrade
                    && upgrade_to.is_none()
                    && !counted
                    && digits(version) =>
            {
                upgrade_to = Some(version.parse().ok()?);
            }
            _ if digits(part) && !counted => counted = true,
            _ => return None,
        }
    }
    let at = chrono::NaiveDateTime::parse_from_str(&format!("{date}{time}"), "%Y%m%d%H%M%S")
        .ok()?
        .and_utc();
    Some(Named {
        at,
        reason,
        upgrade_to,
        encrypted,
    })
}

/// Whether a name is an archive's: what a listing takes, and the only names
/// a download, a deletion or a restore is asked for by.
pub fn is_archive_name(name: &str) -> bool {
    !name.contains(['/', '\\']) && parse_name(name).is_some()
}

/// An archive, as the Backups page lists it.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Backup)]
pub struct Archive {
    pub name: String,
    pub created_at: String,
    pub reason: Reason,
    /// Its size where it is kept, in bytes.
    pub size_bytes: u64,
    pub encrypted: bool,
    /// The engine its database was read from — `sqlite` or `postgres` —
    /// when this database describes it.
    #[schema(required)]
    pub engine: Option<String>,
    /// The version of the server that made it, the same way.
    #[schema(required)]
    pub server_version: Option<String>,
    /// The last migration its database had, the same way.
    #[schema(required)]
    pub migration: Option<i64>,
    pub includes_media: bool,
    pub media_files: i64,
    /// Who asked for it, as the journal names them; `null` for the schedule
    /// and for one this database does not describe.
    #[schema(required)]
    pub created_by: Option<String>,
    /// The SHA-256 of the archive as kept, when this database describes it:
    /// what a copy taken elsewhere is checked against.
    #[schema(required)]
    pub sha256: Option<String>,
    /// Whether it is where the archives are kept. One removed by hand is
    /// still listed while this database describes it, and can only be
    /// forgotten.
    pub present: bool,
    /// Whether this database describes it. One put where the archives are
    /// kept by hand — or made before a restore brought back an older list —
    /// is listed from its name alone; its manifest says the rest.
    pub described: bool,
    /// For an archive made before an upgrade, the migration the start went
    /// on to apply: the database in it is at `migration`, the one before.
    #[schema(required)]
    pub upgrade_to: Option<i64>,
}

/// What a backup is asked to be.
pub struct Ask {
    pub reason: Reason,
    /// The media kept on disk with it. Nothing for media in a bucket, or
    /// none kept.
    pub include_media: bool,
    /// Who asked, as the journal names them; nothing for the schedule.
    pub by: Option<String>,
    /// The run it is made in.
    pub job: Option<String>,
    /// Written there rather than where the archives are kept, and not
    /// listed: a file, or a directory to name it in. The command line's
    /// `--out`.
    pub out: Option<PathBuf>,
    /// Encrypted with this rather than with `AMS_BACKUP_PASSPHRASE`.
    pub passphrase: Option<SecretString>,
    /// Made before an upgrade's migrations: the last of them.
    pub upgrade_to: Option<i64>,
}

impl Ask {
    pub fn new(reason: Reason) -> Self {
        Self {
            reason,
            include_media: false,
            by: None,
            job: None,
            out: None,
            passphrase: None,
            upgrade_to: None,
        }
    }
}

/// An archive made before an upgrade's migrations, listed once they have
/// made the table it is listed in: see [`Backups::before_upgrade`].
pub struct BeforeUpgrade {
    pub made: Made,
    row: index::Row,
}

/// An archive made.
#[derive(Clone, Debug)]
pub struct Made {
    pub name: String,
    /// Where it was written: the place's own words, or the file's path.
    pub at: String,
    pub size: u64,
    pub sha256: String,
    pub encrypted: bool,
    pub manifest: Manifest,
}

/// What making and restoring archives needs, wherever it is done from:
/// the server, or the command line beside it.
pub struct Backups {
    db: Db,
    place: Place,
    /// Where an archive is put together and taken apart: never a
    /// container's `/tmp` by default, which is often a small tmpfs that a
    /// copy of the database outgrows.
    work: PathBuf,
    /// The media kept on disk, when they are: what an archive may carry.
    media_dir: Option<PathBuf>,
    media_backend: config::MediaStorage,
    /// Where the media go back to on a restore, whatever the store is.
    media_store: Option<Arc<Store>>,
    passphrase: Option<SecretString>,
    /// Whether the database lives in memory: nothing of it outlives the
    /// process, and nothing is archived before migrating it.
    in_memory: bool,
}

impl Backups {
    /// The backups of the database `db`, as the configuration says: kept
    /// in the bucket among several instances whose media are in one, in a
    /// directory otherwise. Nothing is touched yet: a directory that cannot
    /// be written is found out by the first backup, not at start.
    pub fn new(config: &Config, db: Db, media_store: Option<Arc<Store>>) -> Result<Self> {
        #[cfg(not(test))]
        let named = std::env::var(DIR_VARIABLE)
            .ok()
            .map(|dir| dir.trim().to_string())
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from);
        // A test's server keeps its archives in a directory of its own,
        // whatever the environment says — never the repository's `data` —
        // made only if an archive is.
        #[cfg(test)]
        let named = Some(std::env::temp_dir().join(format!("ams-backup-{}", crate::db::new_id())));
        let dir = named
            .clone()
            .unwrap_or_else(|| default_dir(&config.database.url));
        let place = match (&config.mode, &config.media.s3) {
            (config::Mode::Multi, Some(s3)) => Place::bucket(s3)?,
            _ => Place::Directory(dir.clone()),
        };
        // Beside the archives, whose room a copy of the database fits in
        // the way the archives do — even when they go to a bucket, which
        // keeps none locally: a read-only container sends it to the
        // system's directory instead (see `Work::new`).
        let work = dir.join(".work");
        #[cfg(not(test))]
        let passphrase = std::env::var(PASSPHRASE_VARIABLE)
            .ok()
            .filter(|p| !p.trim().is_empty());
        // Nor the environment's passphrase: a test says which it means.
        #[cfg(test)]
        let passphrase: Option<String> = None;
        if let Some(passphrase) = &passphrase {
            crate::telemetry::redact_also(passphrase);
        }
        Ok(Self {
            db,
            place,
            work,
            media_dir: (config.media.storage == config::MediaStorage::Filesystem)
                .then(|| config.media.dir.clone()),
            media_backend: config.media.storage,
            media_store,
            passphrase: passphrase.map(SecretString::from),
            in_memory: config.database.url.starts_with("sqlite:")
                && crate::db::sqlite_path(&config.database.url).is_none(),
        })
    }

    /// The same, putting the media back into `store` on a restore: the
    /// store the server opens once the database is migrated.
    pub fn with_media(mut self, store: Option<Arc<Store>>) -> Self {
        self.media_store = store;
        self
    }

    pub fn place(&self) -> &Place {
        &self.place
    }

    /// The same, encrypting with `passphrase` as `AMS_BACKUP_PASSPHRASE`
    /// would have it.
    #[cfg(test)]
    pub fn with_passphrase(mut self, passphrase: &str) -> Self {
        self.passphrase = Some(SecretString::from(passphrase.to_string()));
        self
    }

    /// The same, keeping the archives in `place`.
    #[cfg(test)]
    pub fn with_place(mut self, place: Place) -> Self {
        self.place = place;
        self
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    /// Whether the archives the server makes are encrypted.
    pub fn encrypts(&self) -> bool {
        self.passphrase.is_some()
    }

    /// The passphrase the server's archives are encrypted with, which also
    /// opens one when none is given.
    pub fn passphrase(&self) -> Option<&SecretString> {
        self.passphrase.as_ref()
    }

    /// Whether an archive can carry the media: they are kept on disk.
    pub fn media_on_disk(&self) -> bool {
        self.media_dir.is_some()
    }

    /// `off`, `filesystem` or `s3`.
    pub fn media_store_kind(&self) -> &'static str {
        match self.media_backend {
            config::MediaStorage::Off => "off",
            config::MediaStorage::Filesystem => "filesystem",
            config::MediaStorage::S3 => "s3",
        }
    }

    /// When the newest archive this database describes was made.
    pub async fn newest_at(&self) -> Result<Option<String>> {
        index::newest_at(&self.db).await
    }

    /// Make an archive, and list it.
    pub async fn make(&self, ask: Ask) -> Result<Made> {
        let (made, row) = self.write(ask).await?;
        if let Some(row) = row {
            index::insert(&self.db, &row)
                .await
                .context("the archive was made, but could not be listed")?;
        }
        Ok(made)
    }

    /// Make an archive: what it is, and the row that lists it unless it was
    /// written elsewhere.
    async fn write(&self, ask: Ask) -> Result<(Made, Option<index::Row>)> {
        let work = match &ask.out {
            // Beside the file asked for, in a directory of its own: the
            // room a copy of the database needs is where the archive goes.
            Some(out) => Work::new(&out_dir(out).join(".ams-work"))?,
            None => Work::new(&self.work)?,
        };
        let database = work.path().join(manifest::DATABASE);
        let taken = snapshot::take(&self.db, &database).await?;
        let hashed = database.clone();
        let (database_size, database_sha) =
            tokio::task::spawn_blocking(move || archive::hash_file(&hashed)).await??;

        let media = match (&self.media_dir, ask.include_media) {
            (Some(dir), true) => {
                let dir = dir.clone();
                tokio::task::spawn_blocking(move || media_on_disk(&dir)).await??
            }
            _ => Vec::new(),
        };
        let created = chrono::Utc::now();
        let manifest = Manifest {
            format: manifest::FORMAT,
            server: env!("CARGO_PKG_VERSION").to_string(),
            migration: taken.migration,
            created_at: crate::db::to_rfc3339(created),
            engine: engine_name(taken.engine).to_string(),
            reason: ask.reason.as_str().to_string(),
            upgrade_to: ask.upgrade_to,
            files: vec![manifest::File {
                path: manifest::DATABASE.to_string(),
                size: database_size,
                sha256: database_sha,
            }],
            media: manifest::Media {
                included: ask.include_media && self.media_dir.is_some(),
                store: self.media_store_kind().to_string(),
                files: media.len() as u64,
                bytes: media.iter().map(|(_, _, size)| size).sum(),
                note: self.media_note(ask.include_media),
            },
        };

        let passphrase = ask.passphrase.clone().or_else(|| self.passphrase.clone());
        let encrypted = passphrase.is_some();
        let (name, mut writer, at) = match &ask.out {
            Some(out) => {
                let path = if out.is_dir() {
                    out.join(name_for(created, ask.reason, ask.upgrade_to, encrypted, 1))
                } else {
                    out.clone()
                };
                let name = path
                    .file_name()
                    .context("an archive needs a file name")?
                    .to_string_lossy()
                    .to_string();
                (
                    name,
                    place::Writer::file(&path)?,
                    path.display().to_string(),
                )
            }
            None => {
                let name = self
                    .free_name(created, ask.reason, ask.upgrade_to, encrypted)
                    .await?;
                let writer = self.place.writer(&name).await?;
                (name, writer, self.place.label())
            }
        };

        let written = {
            let manifest = manifest.clone();
            tokio::task::spawn_blocking(move || {
                let media: Vec<archive::Medium<'_>> = media
                    .iter()
                    .map(|(key, path, size)| archive::Medium {
                        key,
                        path,
                        size: *size,
                    })
                    .collect();
                match archive::write(
                    &mut writer,
                    passphrase.as_ref(),
                    &manifest,
                    &database,
                    &media,
                ) {
                    Ok(written) => writer.finish().map(|()| written),
                    Err(e) => {
                        writer.abandon();
                        Err(e)
                    }
                }
            })
            .await??
        };
        drop(work);

        let row = ask.out.is_none().then(|| index::Row {
            id: crate::db::new_id(),
            name: name.clone(),
            created_at: manifest.created_at.clone(),
            reason: ask.reason.as_str().to_string(),
            place: self.place.kind().to_string(),
            size_bytes: i64::try_from(written.size).unwrap_or(i64::MAX),
            sha256: written.sha256.clone(),
            engine: manifest.engine.clone(),
            server_version: manifest.server.clone(),
            migration: manifest.migration,
            includes_media: manifest.media.included,
            media_files: i64::try_from(manifest.media.files).unwrap_or(i64::MAX),
            encrypted,
            created_by: ask.by.clone(),
            job_id: ask.job.clone(),
        });

        tracing::info!(
            archive = %name,
            bytes = written.size,
            encrypted,
            media = manifest.media.files,
            "made a backup"
        );
        Ok((
            Made {
                name,
                at,
                size: written.size,
                sha256: written.sha256,
                encrypted,
                manifest,
            },
            row,
        ))
    }

    /// What the manifest says of media an archive does not carry.
    fn media_note(&self, asked: bool) -> Option<String> {
        match self.media_backend {
            config::MediaStorage::S3 => Some(
                "the media are kept in a bucket, which an archive does not hold: back the \
                 bucket up on its own; a restore says which media its rows name that the \
                 bucket lacks"
                    .to_string(),
            ),
            config::MediaStorage::Filesystem if !asked => Some(
                "the media kept on disk were left out: back up the media directory on its \
                 own, or ask for them in the archive"
                    .to_string(),
            ),
            _ => None,
        }
    }

    /// A name no archive kept has. Where they are kept says so; the list
    /// is asked too where it can be — not before the migrations that make
    /// it, when an archive is made before an upgrade.
    async fn free_name(
        &self,
        at: chrono::DateTime<chrono::Utc>,
        reason: Reason,
        upgrade_to: Option<i64>,
        encrypted: bool,
    ) -> Result<String> {
        for nth in 1..100 {
            let name = name_for(at, reason, upgrade_to, encrypted, nth);
            if self.place.size(&name).await?.is_none()
                && index::by_name(&self.db, &name)
                    .await
                    .ok()
                    .flatten()
                    .is_none()
            {
                return Ok(name);
            }
        }
        bail!("a hundred archives were made in the same second")
    }

    /// Every archive: those this database describes, where they are kept or
    /// not, and those found where they are kept that it does not, newest
    /// first.
    pub async fn list(&self) -> Result<Vec<Archive>> {
        let rows = index::all(&self.db).await?;
        let stored = self.place.list().await?;
        let mut listed: Vec<Archive> = rows
            .iter()
            .map(|row| {
                let kept = stored.iter().find(|s| s.name == row.name);
                Archive {
                    name: row.name.clone(),
                    created_at: row.created_at.clone(),
                    reason: Reason::parse(&row.reason).unwrap_or(Reason::Manual),
                    size_bytes: kept
                        .map(|s| s.size)
                        .unwrap_or_else(|| u64::try_from(row.size_bytes).unwrap_or(0)),
                    encrypted: row.encrypted,
                    engine: Some(row.engine.clone()),
                    server_version: Some(row.server_version.clone()),
                    migration: Some(row.migration),
                    includes_media: row.includes_media,
                    media_files: row.media_files,
                    created_by: row.created_by.clone(),
                    sha256: Some(row.sha256.clone()),
                    present: kept.is_some(),
                    described: true,
                    upgrade_to: parse_name(&row.name).and_then(|named| named.upgrade_to),
                }
            })
            .collect();
        for found in stored {
            if rows.iter().any(|row| row.name == found.name) {
                continue;
            }
            let Some(named) = parse_name(&found.name) else {
                continue;
            };
            listed.push(Archive {
                name: found.name,
                created_at: crate::db::to_rfc3339(named.at),
                reason: named.reason,
                size_bytes: found.size,
                encrypted: named.encrypted,
                engine: None,
                server_version: None,
                migration: None,
                includes_media: false,
                media_files: 0,
                created_by: None,
                sha256: None,
                present: true,
                described: false,
                upgrade_to: named.upgrade_to,
            });
        }
        listed.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.name.cmp(&a.name))
        });
        Ok(listed)
    }

    /// Take an archive away, and forget it. Whether there was anything to
    /// take away or forget.
    pub async fn delete(&self, name: &str) -> Result<bool> {
        if !is_archive_name(name) {
            return Ok(false);
        }
        let removed = self.place.delete(name).await?;
        let forgotten = index::remove(&self.db, name).await?;
        Ok(removed || forgotten)
    }

    /// The schedule's archives past the newest `keep`, taken away: their
    /// names, oldest last. Those made by hand, by the command line or before
    /// a restore are deliberate, and stay until somebody deletes them.
    pub async fn prune(&self, keep: usize) -> Result<Vec<String>> {
        self.prune_reason(Reason::Scheduled, keep).await
    }

    /// The archives made for `reason` past the newest `keep`, taken away:
    /// each reason's retention counts its own, so that the archives made
    /// before upgrades never take the schedule's place, nor it theirs.
    pub async fn prune_reason(&self, reason: Reason, keep: usize) -> Result<Vec<String>> {
        let mut made: Vec<(chrono::DateTime<chrono::Utc>, String)> = self
            .list()
            .await?
            .into_iter()
            .filter(|archive| archive.reason == reason)
            .filter_map(|archive| {
                crate::db::parse_rfc3339(&archive.created_at).map(|at| (at, archive.name))
            })
            .collect();
        made.sort_by(|a, b| b.cmp(a));
        let mut removed = Vec::new();
        for (_, name) in made.into_iter().skip(keep) {
            self.delete(&name).await?;
            removed.push(name);
        }
        Ok(removed)
    }

    /// Before a start applies the migrations a new version brings, an
    /// archive of the database as it stands: the way back to the version
    /// before. Made when some of this binary's migrations are pending on a
    /// database that has had others — a new one has nothing to keep — and
    /// unless `AMS_BACKUP_BEFORE_UPGRADE=false`: always for an SQLite file,
    /// a copy of a file; for a PostgreSQL unless `backup.beforeUpgrade` is
    /// off, since every row of it goes through SQLite. Its row is written
    /// once the migrations have made the table it goes in:
    /// [`Backups::after_upgrade`]. An archive that cannot be made stops the
    /// start, saying how to go on without one.
    pub async fn before_upgrade(&self) -> Result<Option<BeforeUpgrade>> {
        let pending = self.db.pending_migrations().await?;
        let (Some(first), Some(last)) = (pending.iter().min(), pending.iter().max()) else {
            return Ok(None);
        };
        let wanted = upgrade_wanted(
            std::env::var(BEFORE_UPGRADE_VARIABLE).ok().as_deref(),
            server_setting(&self.db, BEFORE_UPGRADE_SETTING)
                .await
                .as_deref(),
            self.db.dialect(),
            self.in_memory,
        );
        if !wanted {
            tracing::warn!(
                from = first,
                to = last,
                "applying migrations without an archive of the database first"
            );
            return Ok(None);
        }
        let from = self.db.last_migration().await?;
        tracing::info!(
            from,
            to = last,
            "making an archive of the database before applying this version's migrations"
        );
        let (made, row) = self
            .write(Ask {
                upgrade_to: Some(*last),
                ..Ask::new(Reason::Upgrade)
            })
            .await
            .with_context(|| {
                format!(
                    "could not make an archive of the database before applying migrations \
                     {first:04} to {last:04}, so none was applied: give {} room and the \
                     right to write — or the bucket the archives go to — or start once with \
                     {BEFORE_UPGRADE_VARIABLE}=false to migrate without one",
                    self.place.label()
                )
            })?;
        let row = row.context("an archive made before an upgrade is always listed")?;
        tracing::info!(archive = %made.name, at = %made.at, "the database's archive is made: migrating");
        Ok(Some(BeforeUpgrade { made, row }))
    }

    /// Once the migrations are applied: the archive made before them listed,
    /// and those made before earlier upgrades past `backup.keepUpgrades`
    /// taken away. Neither stops the start: the archive is made, and listed
    /// from its name when its row could not be written.
    pub async fn after_upgrade(&self, before: BeforeUpgrade) -> Made {
        if let Err(e) = index::insert(&self.db, &before.row).await {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "could not list the archive made before the migrations"
            );
        }
        let keep = server_setting(&self.db, KEEP_UPGRADES_SETTING)
            .await
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_KEEP_UPGRADES as usize)
            .max(1);
        if let Err(e) = self.prune_reason(Reason::Upgrade, keep).await {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "could not take away the archives made before earlier upgrades"
            );
        }
        before.made
    }

    /// An archive's bytes as they come, its size, and its SHA-256 when this
    /// database describes it: what a download is answered with.
    pub async fn download(&self, name: &str) -> Result<Option<Download>> {
        if !is_archive_name(name) {
            return Ok(None);
        }
        let Some((stream, size)) = self.place.stream(name).await? else {
            return Ok(None);
        };
        let sha256 = index::by_name(&self.db, name).await?.map(|row| row.sha256);
        Ok(Some(Download {
            stream,
            size,
            sha256,
        }))
    }
}

/// An archive being downloaded.
pub struct Download {
    pub stream: futures::stream::BoxStream<'static, std::io::Result<bytes::Bytes>>,
    pub size: u64,
    pub sha256: Option<String>,
}

/// Whether a start that applies migrations archives the database first: not
/// one in memory, nor when `AMS_BACKUP_BEFORE_UPGRADE` says `false`; an
/// SQLite file always; a PostgreSQL unless `backup.beforeUpgrade` is off.
fn upgrade_wanted(
    variable: Option<&str>,
    setting: Option<&str>,
    dialect: Dialect,
    in_memory: bool,
) -> bool {
    let off = |value: Option<&str>| {
        value.is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "false" | "0" | "no" | "off"
            )
        })
    };
    if in_memory || off(variable) {
        return false;
    }
    match dialect {
        Dialect::Sqlite => true,
        Dialect::Postgres => !off(setting),
    }
}

/// A server setting as the table holds it, read before the settings are —
/// before the migrations, even: nothing when it is unset, or when the
/// database is too old to hold settings this way.
async fn server_setting(db: &Db, key: &str) -> Option<String> {
    sqlx::query(
        db.sql("SELECT value FROM setting WHERE scope = 'server' AND scope_id = '' AND key = ?"),
    )
    .bind(key)
    .fetch_optional(db.pool())
    .await
    .ok()
    .flatten()
    .and_then(|row| sqlx::Row::try_get::<String, _>(&row, "value").ok())
}

/// The directory a file asked for goes in.
fn out_dir(out: &Path) -> PathBuf {
    if out.is_dir() {
        return out.to_path_buf();
    }
    out.parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `sqlite` or `postgres`.
pub fn engine_name(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Sqlite => "sqlite",
        Dialect::Postgres => "postgres",
    }
}

/// Where the archives are kept by default: `backups` beside an SQLite
/// database, `data/backups` with PostgreSQL.
fn default_dir(database_url: &str) -> PathBuf {
    crate::db::sqlite_path(database_url)
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .map(|dir| dir.join("backups"))
        .unwrap_or_else(|| PathBuf::from("data/backups"))
}

/// Every medium kept on disk: its key, its path, its size. Blocking.
fn media_on_disk(dir: &Path) -> Result<Vec<(String, PathBuf, u64)>> {
    let mut found = Vec::new();
    let mut unread = vec![dir.to_path_buf()];
    while let Some(dir) = unread.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("could not list {}", dir.display())),
        };
        for entry in entries {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                unread.push(path);
            } else if kind.is_file() {
                let name = entry.file_name().to_string_lossy().to_string();
                // A key the store filed, and nothing else that may lie there.
                if crate::media::file::valid_key(&name) {
                    found.push((name, path, entry.metadata()?.len()));
                }
            }
        }
    }
    found.sort();
    Ok(found)
}

/// A directory of its own to put an archive together in or take one apart,
/// taken away with whatever is left in it once done.
struct Work(PathBuf);

/// How old what a crash left behind has to be before the next backup takes
/// it away: older than any backup takes.
const LEFTOVER_AFTER: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

impl Work {
    /// A directory under `under`, or under the system's own when `under`
    /// cannot be written — a container with a read-only root and no volume
    /// for the backups — said, since that one is often a small tmpfs.
    fn new(under: &Path) -> Result<Self> {
        sweep_leftovers(under);
        let dir = under.join(crate::db::new_id());
        match std::fs::create_dir_all(&dir) {
            Ok(()) => Ok(Self(dir)),
            Err(e) => {
                let fallback = std::env::temp_dir().join("ams-backups");
                if under.starts_with(&fallback) {
                    return Err(e).with_context(|| format!("could not create {}", dir.display()));
                }
                tracing::warn!(
                    dir = %under.display(),
                    error = %e,
                    instead = %fallback.display(),
                    "the backups' working directory cannot be written; working in the system's \
                     temporary directory instead, which a copy of the database has to fit in"
                );
                sweep_leftovers(&fallback);
                let dir = fallback.join(crate::db::new_id());
                std::fs::create_dir_all(&dir)
                    .with_context(|| format!("could not create {}", dir.display()))?;
                Ok(Self(dir))
            }
        }
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(dir = %self.0.display(), error = %e, "could not remove a backup's working directory");
        }
    }
}

/// What a backup killed halfway left behind — its working directory under
/// `work`, the archive it was writing beside the others — taken away once
/// it is a day old. Best effort: a backup does not fail for want of it.
fn sweep_leftovers(work: &Path) {
    let old = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age > LEFTOVER_AFTER)
    };
    if let Ok(entries) = std::fs::read_dir(work) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && old(&path) {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
    let Some(kept) = work.parent() else {
        return;
    };
    if let Ok(entries) = std::fs::read_dir(kept) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            if name.starts_with(&format!(".{PREFIX}")) && name.ends_with(".partial") && old(&path) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// A directory of its own for a test.
#[cfg(test)]
pub(crate) fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ams-backup-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> chrono::DateTime<chrono::Utc> {
        crate::db::parse_rfc3339(text).unwrap()
    }

    #[test]
    fn an_archives_name_says_when_and_why_it_was_made() {
        let made = at("2026-10-19T02:00:00Z");
        let name = name_for(made, Reason::Scheduled, None, false, 1);
        assert_eq!(name, "ams-20261019-020000-scheduled.tar.zst");
        assert_eq!(
            parse_name(&name),
            Some(Named {
                at: made,
                reason: Reason::Scheduled,
                upgrade_to: None,
                encrypted: false
            })
        );

        let again = name_for(made, Reason::Safety, None, true, 2);
        assert_eq!(again, "ams-20261019-020000-safety-2.tar.zst.age");
        assert_eq!(
            parse_name(&again),
            Some(Named {
                at: made,
                reason: Reason::Safety,
                upgrade_to: None,
                encrypted: true
            })
        );

        // Before an upgrade: the migration it went on to, and a second one
        // in the same second after that.
        for (nth, expected) in [
            (1, "ams-20261019-020000-upgrade-m0037.tar.zst"),
            (2, "ams-20261019-020000-upgrade-m0037-2.tar.zst"),
        ] {
            let upgrade = name_for(made, Reason::Upgrade, Some(37), false, nth);
            assert_eq!(upgrade, expected);
            assert_eq!(
                parse_name(&upgrade),
                Some(Named {
                    at: made,
                    reason: Reason::Upgrade,
                    upgrade_to: Some(37),
                    encrypted: false
                })
            );
        }

        for other in [
            "ams-20261019-020000-weekly.tar.zst",
            "ams-2026101-020000-manual.tar.zst",
            "notes.txt",
            ".ams-20261019-020000-manual.tar.zst.partial",
            "ams-20261019-020000-manual.tar.gz",
            "../ams-20261019-020000-manual.tar.zst",
            // A migration is an upgrade's alone, said once, before the count.
            "ams-20261019-020000-manual-m0037.tar.zst",
            "ams-20261019-020000-upgrade-m0037-m0038.tar.zst",
            "ams-20261019-020000-upgrade-2-m0037.tar.zst",
            "ams-20261019-020000-upgrade-2-3.tar.zst",
        ] {
            assert!(!is_archive_name(other), "{other}");
        }
    }

    #[test]
    fn an_upgrade_archives_a_file_always_and_a_postgresql_unless_asked_not_to() {
        // An SQLite file: always, but for the variable.
        assert!(upgrade_wanted(None, None, Dialect::Sqlite, false));
        assert!(upgrade_wanted(None, Some("false"), Dialect::Sqlite, false));
        assert!(!upgrade_wanted(Some("false"), None, Dialect::Sqlite, false));
        assert!(!upgrade_wanted(Some(" OFF "), None, Dialect::Sqlite, false));
        assert!(upgrade_wanted(Some("true"), None, Dialect::Sqlite, false));
        // A PostgreSQL: unless the setting or the variable says no.
        assert!(upgrade_wanted(None, None, Dialect::Postgres, false));
        assert!(upgrade_wanted(None, Some("true"), Dialect::Postgres, false));
        assert!(!upgrade_wanted(
            None,
            Some("false"),
            Dialect::Postgres,
            false
        ));
        assert!(!upgrade_wanted(
            Some("0"),
            Some("true"),
            Dialect::Postgres,
            false
        ));
        // A database in memory: never, nothing of it outlives the process.
        assert!(!upgrade_wanted(None, None, Dialect::Sqlite, true));
    }

    #[test]
    fn the_archives_are_kept_beside_an_sqlite_database_by_default() {
        assert_eq!(
            default_dir("sqlite:///data/ams.db?mode=rwc"),
            PathBuf::from("/data/backups")
        );
        assert_eq!(
            default_dir("sqlite://data/ams.db"),
            PathBuf::from("data/backups")
        );
        assert_eq!(
            default_dir("postgres://ams@db/ams"),
            PathBuf::from("data/backups")
        );
    }

    /// A database an earlier version left: every migration of this
    /// binary's but the last, and a setting written, as a start leaves one.
    async fn left_by_the_version_before(db: &Db) -> i64 {
        let before = crate::db::latest_migration() - 1;
        db.migrate_to(before).await.unwrap();
        sqlx::query(db.sql(
            "INSERT INTO setting (scope, scope_id, key, value, updated_at) \
             VALUES ('server', '', 'tmdb.language', 'fr-FR', '2026-10-01T00:00:00.000Z')",
        ))
        .execute(db.pool())
        .await
        .unwrap();
        before
    }

    /// The tables of an SQLite file, its own and the search index's aside.
    async fn tables_in(path: &Path) -> Vec<String> {
        let copy = snapshot::open(path, false).await.unwrap();
        let mut tables: Vec<String> = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        )
        .fetch_all(copy.pool())
        .await
        .unwrap()
        .iter()
        .map(|row| sqlx::Row::get::<String, _>(row, "name"))
        .collect();
        copy.close().await;
        tables.sort();
        tables
    }

    /// The database an archive holds, taken out into `into`.
    fn database_of(archive: &Path, into: &Path) {
        let mut opened =
            archive::open(Box::new(std::fs::File::open(archive).unwrap()), None).unwrap();
        for entry in opened.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path().unwrap().to_string_lossy() == manifest::DATABASE {
                let mut file = std::fs::File::create(into).unwrap();
                std::io::copy(&mut entry, &mut file).unwrap();
                return;
            }
        }
        panic!("no database in {}", archive.display());
    }

    /// The tables a database at `version` has, as this binary's migrations
    /// make them.
    async fn tables_at(version: i64) -> Vec<String> {
        let dir = scratch();
        let path = dir.join("at.sqlite");
        let db = snapshot::open(&path, true).await.unwrap();
        db.migrate_to(version).await.unwrap();
        db.close().await;
        let tables = tables_in(&path).await;
        let _ = std::fs::remove_dir_all(&dir);
        tables
    }

    fn config_for(url: String) -> Config {
        let mut config = Config::from_env().expect("a configuration");
        config.mode = config::Mode::Single;
        config.database = config::Database {
            // A database in memory is its connection's: a second would be a
            // second, empty, database.
            max_connections: if url.contains(":memory:") { 1 } else { 4 },
            url,
            acquire_timeout: std::time::Duration::from_secs(10),
        };
        config.security.bootstrap_admin = None;
        config.cache.redis_url = None;
        config.media.storage = config::MediaStorage::Off;
        config.clients = None;
        config.tmdb.api_key = None;
        config.tvdb.api_key = None;
        config.tvdb.enabled = false;
        config
    }

    #[tokio::test]
    async fn a_start_that_migrates_a_file_archives_it_first_as_the_version_before_left_it() {
        let dir = scratch();
        let file = dir.join("ams.db");
        let older = snapshot::open(&file, true).await.unwrap();
        let before = left_by_the_version_before(&older).await;
        older.close().await;
        let latest = crate::db::latest_migration();

        let state =
            crate::state::AppState::bootstrap(config_for(crate::db::sqlite_url(&file, false)))
                .await
                .unwrap();
        assert_eq!(state.db.last_migration().await.unwrap(), latest);

        let listed = state.backups.list().await.unwrap();
        assert_eq!(listed.len(), 1, "{listed:?}");
        let archive = &listed[0];
        assert_eq!(archive.reason, Reason::Upgrade);
        assert_eq!(archive.upgrade_to, Some(latest));
        assert_eq!(archive.migration, Some(before));
        assert!(archive.described && archive.present);
        assert_eq!(
            archive.name,
            name_for(
                parse_name(&archive.name).unwrap().at,
                Reason::Upgrade,
                Some(latest),
                false,
                1
            )
        );

        // What it holds is the database as the version before left it: its
        // schema, its tables — those the last migration reshapes among them.
        let kept = state
            .backups
            .place()
            .directory()
            .unwrap()
            .join(&archive.name);
        let taken = dir.join("taken.sqlite");
        database_of(&kept, &taken);
        assert_eq!(tables_in(&taken).await, tables_at(before).await);
        let manifest = state
            .backups
            .manifest_of(&restore::Source::File(kept), None)
            .await
            .unwrap();
        assert_eq!(
            (
                manifest.reason.as_str(),
                manifest.migration,
                manifest.upgrade_to
            ),
            ("upgrade", before, Some(latest))
        );

        // A start with nothing to migrate makes none.
        drop(state);
        let again =
            crate::state::AppState::bootstrap(config_for(crate::db::sqlite_url(&file, false)))
                .await
                .unwrap();
        assert_eq!(index::all(&again.db).await.unwrap().len(), 1);
        again.db.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_new_database_or_one_in_memory_is_not_archived_before_its_migrations() {
        let fresh = crate::state::AppState::bootstrap(config_for("sqlite::memory:".into()))
            .await
            .unwrap();
        assert!(fresh.backups.list().await.unwrap().is_empty());

        let in_memory = Db::connect(&config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .unwrap();
        left_by_the_version_before(&in_memory).await;
        let backups = Backups::new(&config_for("sqlite::memory:".into()), in_memory, None).unwrap();
        assert!(backups.before_upgrade().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn an_archive_that_cannot_be_made_stops_the_migrations_and_says_how_to_go_on() {
        let dir = scratch();
        let file = dir.join("ams.db");
        let db = snapshot::open(&file, true).await.unwrap();
        let before = left_by_the_version_before(&db).await;

        // Where the archives go is a file: nothing can be written there.
        let blocked = dir.join("a file");
        std::fs::write(&blocked, b"not a directory").unwrap();
        let backups = Backups::new(
            &config_for(crate::db::sqlite_url(&file, false)),
            db.clone(),
            None,
        )
        .unwrap()
        .with_place(Place::Directory(blocked));
        let refused = format!(
            "{:#}",
            backups.before_upgrade().await.err().expect("refused")
        );
        assert!(refused.contains("none was applied"), "{refused}");
        assert!(
            refused.contains("AMS_BACKUP_BEFORE_UPGRADE=false"),
            "{refused}"
        );
        assert_eq!(db.last_migration().await.unwrap(), before);
        db.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_postgresql_is_archived_at_its_own_schema_before_it_migrates_unless_told_not_to() {
        if crate::db::testing::server().is_none() {
            return;
        }
        let database = crate::db::testing::database().await;
        let db = Db::connect(&database).await.unwrap();
        let before = left_by_the_version_before(&db).await;
        let latest = crate::db::latest_migration();
        let backups = Backups::new(&config_for(database.url.clone()), db.clone(), None).unwrap();

        let made = backups.before_upgrade().await.unwrap().expect("an archive");
        assert_eq!(made.made.manifest.engine, "postgres");
        assert_eq!(made.made.manifest.migration, before);
        assert_eq!(made.made.manifest.upgrade_to, Some(latest));
        db.migrate().await.unwrap();
        let made = backups.after_upgrade(made).await;
        assert_eq!(backups.list().await.unwrap().len(), 1);

        // Copied through SQLite at the schema it had, every table of it.
        let dir = scratch();
        let kept = backups.place().directory().unwrap().join(&made.name);
        let taken = dir.join("taken.sqlite");
        database_of(&kept, &taken);
        assert_eq!(tables_in(&taken).await, tables_at(before).await);

        // And restored where this version runs, brought up to it first.
        let target = crate::db::testing::sqlite().await;
        Backups::new(&config_for("sqlite::memory:".into()), target.clone(), None)
            .unwrap()
            .restore(restore::Request {
                source: restore::Source::File(kept),
                replace: false,
                passphrase: None,
                by: None,
                progress: None,
            })
            .await
            .unwrap();
        assert_eq!(
            server_setting(&target, "tmdb.language").await.as_deref(),
            Some("fr-FR")
        );

        // Told not to, a PostgreSQL migrates without one; nothing is due
        // either once it has.
        let other = crate::db::testing::database().await;
        let other_db = Db::connect(&other).await.unwrap();
        left_by_the_version_before(&other_db).await;
        sqlx::query(other_db.sql(
            "INSERT INTO setting (scope, scope_id, key, value, updated_at) \
             VALUES ('server', '', 'backup.beforeUpgrade', 'false', '2026-10-01T00:00:00.000Z')",
        ))
        .execute(other_db.pool())
        .await
        .unwrap();
        let told = Backups::new(&config_for(other.url.clone()), other_db.clone(), None).unwrap();
        assert!(told.before_upgrade().await.unwrap().is_none());
        assert!(backups.before_upgrade().await.unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_archives_made_before_upgrades_keep_to_their_own_retention() {
        let db = crate::db::testing::sqlite().await;
        let backups =
            Backups::new(&config_for("sqlite::memory:".into()), db.clone(), None).unwrap();
        let scheduled = backups.make(Ask::new(Reason::Scheduled)).await.unwrap();
        let mut upgrades = Vec::new();
        for to in [35, 36, 37] {
            upgrades.push(
                backups
                    .make(Ask {
                        upgrade_to: Some(to),
                        ..Ask::new(Reason::Upgrade)
                    })
                    .await
                    .unwrap(),
            );
        }
        let removed = backups.prune_reason(Reason::Upgrade, 2).await.unwrap();
        assert_eq!(removed, [upgrades[0].name.clone()]);
        // The schedule's own keeping takes none of theirs, nor they its.
        assert!(backups.prune(1).await.unwrap().is_empty());
        let left: Vec<String> = backups
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|archive| archive.name)
            .collect();
        for kept in [&scheduled, &upgrades[1], &upgrades[2]] {
            assert!(left.contains(&kept.name), "{} in {left:?}", kept.name);
        }
    }
}
