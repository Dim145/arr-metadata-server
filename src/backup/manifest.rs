//! What an archive says of itself: the first file in it, read without
//! reading the rest.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The archive's layout. Raised when a server could not read what this one
/// writes; an archive of a later layout is refused rather than half read.
pub const FORMAT: u32 = 1;

/// The archive's first file.
pub const NAME: &str = "manifest.json";

/// The database, as one SQLite file.
pub const DATABASE: &str = "database.sqlite";

/// Where the media are filed in the archive, each under its key.
pub const MEDIA: &str = "media/";

/// The most a manifest may weigh: a few hundred bytes are written, and an
/// archive that says more is not one of this server's.
pub const MOST_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BackupManifest)]
pub struct Manifest {
    /// The archive's layout: [`FORMAT`] when this server wrote it.
    pub format: u32,
    /// The version of the server that made it.
    pub server: String,
    /// The last migration its database had.
    pub migration: i64,
    pub created_at: String,
    /// The engine the database was read from: `sqlite` or `postgres`.
    /// Restored onto either.
    pub engine: String,
    /// Why it was made: `scheduled`, `manual`, `safety`, `command`, or
    /// `upgrade` — by a start, before the migrations it went on to apply.
    pub reason: String,
    /// For an archive made before an upgrade, the last migration the start
    /// went on to apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrade_to: Option<i64>,
    /// Each file but the media, with its size and SHA-256.
    pub files: Vec<File>,
    pub media: Media,
}

/// A file of the archive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BackupFile)]
pub struct File {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// What the archive holds of the media.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BackupMedia)]
pub struct Media {
    /// Whether the media kept on disk are in it, under `media/`.
    pub included: bool,
    /// Where the server it was made of kept them: `off`, `filesystem` or
    /// `s3`.
    pub store: String,
    pub files: u64,
    pub bytes: u64,
    /// What a restore has to know: where the media are when they are not
    /// here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Manifest {
    /// What stands in the way of restoring it with this server, when
    /// something does: a later layout, a later migration, a later version.
    pub fn refusal(&self, latest_migration: i64) -> Option<String> {
        if self.format > FORMAT {
            return Some(format!(
                "this archive is of layout {}, and this server reads layouts up to {FORMAT}: \
                 restore it with the version that made it, {}, or a later one",
                self.format, self.server
            ));
        }
        if self.migration > latest_migration {
            return Some(format!(
                "this archive's database has migration {}, and this server knows them up to \
                 {latest_migration}: restore it with {} or a later version",
                self.migration, self.server
            ));
        }
        // An archive made before an upgrade holds the database as the
        // version before had it, whichever version wrote it: its migration
        // says whether this one can read it — the way back from a new
        // version is to restore it with the old one.
        if self.reason != "upgrade" && newer(&self.server, env!("CARGO_PKG_VERSION")) {
            return Some(format!(
                "this archive was made by version {}, later than this one, {}: restore it \
                 with {} or a later version",
                self.server,
                env!("CARGO_PKG_VERSION"),
                self.server
            ));
        }
        None
    }

    /// The database's entry, as the manifest lists it.
    pub fn database(&self) -> Option<&File> {
        self.files.iter().find(|file| file.path == DATABASE)
    }
}

/// Whether version `made` comes after `running`, by their numbers: a
/// version that does not read as three numbers is taken for no later than
/// any. What follows a `-` or a `+` is no part of the comparison: a
/// pre-release of the version running restores like the version itself.
fn newer(made: &str, running: &str) -> bool {
    fn numbers(version: &str) -> Option<(u64, u64, u64)> {
        let core = version
            .trim()
            .trim_start_matches(['v', 'V'])
            .split(['-', '+'])
            .next()?;
        let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
        let found = (parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(found)
    }
    match (numbers(made), numbers(running)) {
        (Some(made), Some(running)) => made > running,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(server: &str, migration: i64, format: u32) -> Manifest {
        Manifest {
            format,
            server: server.into(),
            migration,
            created_at: "2026-10-19T02:00:00.000Z".into(),
            engine: "sqlite".into(),
            reason: "scheduled".into(),
            upgrade_to: None,
            files: vec![File {
                path: DATABASE.into(),
                size: 4096,
                sha256: "0".repeat(64),
            }],
            media: Media {
                included: false,
                store: "off".into(),
                files: 0,
                bytes: 0,
                note: None,
            },
        }
    }

    #[test]
    fn an_archive_of_a_later_server_layout_or_migration_is_refused() {
        let running = env!("CARGO_PKG_VERSION");
        assert_eq!(manifest(running, 36, FORMAT).refusal(36), None);
        assert_eq!(manifest("0.1.0", 3, FORMAT).refusal(36), None);

        let later = manifest(running, 37, FORMAT).refusal(36).expect("refused");
        assert!(later.contains("migration 37"), "{later}");

        let layout = manifest(running, 36, FORMAT + 1)
            .refusal(36)
            .expect("refused");
        assert!(layout.contains("layout"), "{layout}");

        let version = manifest("99.0.0", 36, FORMAT).refusal(36).expect("refused");
        assert!(version.contains("99.0.0"), "{version}");

        // Made by a later version before its migrations: what the version
        // before restores, as long as it knows the migration it is at.
        let before = Manifest {
            reason: "upgrade".into(),
            upgrade_to: Some(37),
            ..manifest("99.0.0", 36, FORMAT)
        };
        assert_eq!(before.refusal(36), None);
        assert!(before.refusal(35).is_some());
    }

    #[test]
    fn a_version_is_later_by_its_numbers_alone() {
        assert!(newer("0.9.0", "0.8.3"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(!newer("0.8.3", "0.8.3"));
        assert!(!newer("0.8.3-rc.1", "0.8.3"));
        assert!(!newer("0.8.2", "0.8.3"));
        assert!(!newer("nightly", "0.8.3"));
    }

    #[test]
    fn a_manifest_reads_back_as_written() {
        let written = manifest("0.9.0", 36, FORMAT);
        let text = serde_json::to_string(&written).unwrap();
        assert!(text.contains("\"createdAt\""), "{text}");
        assert!(!text.contains("\"note\""), "{text}");
        assert_eq!(serde_json::from_str::<Manifest>(&text).unwrap(), written);
    }
}
