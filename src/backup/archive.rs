//! An archive's bytes: a tar of its files, compressed with zstd, and — given
//! a passphrase — encrypted as age writes it. Written and read in one pass,
//! never whole in memory, and synchronously: called from a blocking task.
//!
//! What is written can be read without this server:
//!
//! ```text
//! zstd -d < ams-….tar.zst | tar -x
//! age -d ams-….tar.zst.age | zstd -d | tar -x
//! ```

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    path::Path,
};

use age::secrecy::SecretString;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use super::manifest::{self, Manifest};

/// How hard zstd works: its default, quick enough for a backup made while
/// the server answers, and a third of a database's size or less.
const LEVEL: i32 = 3;

/// What an age file begins with.
const AGE_MAGIC: &[u8] = b"age-encryption.org/v1";

/// What a zstd frame begins with.
const ZSTD_MAGIC: &[u8] = &[0x28, 0xb5, 0x2f, 0xfd];

/// The most work a passphrase is asked to take, as scrypt's log₂ of it: an
/// archive encrypted on a quick server — age measures the machine it runs
/// on, and asks a second's work of it — still opens on a slow one, and one
/// that asks more than a minute of work is refused.
const MOST_WORK: u8 = 22;

/// A medium to put in the archive: its key, and where its bytes are.
pub struct Medium<'a> {
    pub key: &'a str,
    pub path: &'a Path,
    pub size: u64,
}

/// What was written: how many bytes, and their SHA-256 — of the archive as
/// kept, encrypted or not, which is what a download is checked against.
pub struct Written {
    pub size: u64,
    pub sha256: String,
}

/// Write the archive to `sink`: the manifest, the database, the media.
pub fn write<W: Write>(
    sink: &mut W,
    passphrase: Option<&SecretString>,
    manifest: &Manifest,
    database: &Path,
    media: &[Medium<'_>],
) -> Result<Written> {
    let mut counted = Counted::new(sink);
    let sealed = match passphrase {
        Some(passphrase) => Sealed::Age(
            age::Encryptor::with_user_passphrase(passphrase.clone())
                .wrap_output(&mut counted)
                .context("could not begin encrypting the archive")?,
        ),
        None => Sealed::Plain(&mut counted),
    };
    let compressed = zstd::stream::Encoder::new(sealed, LEVEL)
        .context("could not begin compressing the archive")?;
    let mut tar = tar::Builder::new(compressed);
    let at = chrono::DateTime::parse_from_rfc3339(&manifest.created_at)
        .map(|at| u64::try_from(at.timestamp()).unwrap_or(0))
        .unwrap_or(0);

    let text = serde_json::to_vec_pretty(manifest)?;
    append(&mut tar, manifest::NAME, text.len() as u64, at, &text[..])?;

    let mut file = std::fs::File::open(database)
        .with_context(|| format!("could not read {}", database.display()))?;
    let size = file.metadata()?.len();
    append(&mut tar, manifest::DATABASE, size, at, &mut file)?;

    for medium in media {
        // Taken away by the sweep since it was listed: nobody pointed at it.
        let mut file = match std::fs::File::open(medium.path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(e).with_context(|| format!("could not read the medium {}", medium.key));
            }
        };
        let name = format!("{}{}", manifest::MEDIA, medium.key);
        append(&mut tar, &name, medium.size, at, &mut file)?;
    }

    let compressed = tar
        .into_inner()
        .context("could not end the archive's listing")?;
    let sealed = compressed
        .finish()
        .context("could not end the archive's compression")?;
    sealed.finish()?;
    counted.flush()?;

    Ok(Written {
        size: counted.count,
        sha256: hex(&counted.hasher.finalize()),
    })
}

/// One file of the archive: readable by its owner only, once taken out.
fn append<W: Write, R: Read>(
    tar: &mut tar::Builder<W>,
    path: &str,
    size: u64,
    at: u64,
    data: R,
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(size);
    header.set_mode(0o600);
    header.set_mtime(at);
    header.set_entry_type(tar::EntryType::Regular);
    // Exactly as many bytes as the header says: a file that grew since it
    // was measured would run into the next entry, and one that shrank would
    // leave the next entry where none is read.
    tar.append_data(
        &mut header,
        path,
        Exactly {
            inner: data,
            left: size,
        },
    )
    .with_context(|| format!("could not put {path} in the archive"))
}

/// A reader of exactly `left` bytes: what follows them is not read, and an
/// end before them is an error rather than a short entry.
struct Exactly<R: Read> {
    inner: R,
    left: u64,
}

impl<R: Read> Read for Exactly<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Ok(0);
        }
        let most = usize::try_from(self.left)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let n = self.inner.read(&mut buf[..most])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "a file of the archive ended before the size it was measured at",
            ));
        }
        self.left -= n as u64;
        Ok(n)
    }
}

/// An archive opened for reading, its files in the order they were written.
pub type Opened = tar::Archive<Box<dyn Read + Send>>;

/// Open an archive: decrypted with `passphrase` when it is encrypted, and
/// refused, saying so, when it is encrypted and no passphrase opens it.
pub fn open(source: Box<dyn Read + Send>, passphrase: Option<&SecretString>) -> Result<Opened> {
    let mut source = BufReader::new(source);
    let start = source
        .fill_buf()
        .context("could not read the archive")?
        .to_vec();

    let plain: Box<dyn Read + Send> = if start.starts_with(AGE_MAGIC) {
        let Some(passphrase) = passphrase else {
            bail!("this archive is encrypted: give the passphrase it was made with");
        };
        let decryptor = age::Decryptor::new_buffered(source)
            .context("this archive's encryption could not be read")?;
        if !decryptor.is_scrypt() {
            bail!("this archive was encrypted for a key rather than with a passphrase");
        }
        let mut identity = age::scrypt::Identity::new(passphrase.clone());
        identity.set_max_work_factor(MOST_WORK);
        let opened = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .map_err(|e| match e {
                age::DecryptError::DecryptionFailed
                | age::DecryptError::KeyDecryptionFailed
                | age::DecryptError::NoMatchingKeys => {
                    anyhow::anyhow!("the passphrase given does not open this archive")
                }
                other => anyhow::anyhow!("this archive could not be decrypted: {other}"),
            })?;
        Box::new(opened)
    } else if start.starts_with(ZSTD_MAGIC) {
        Box::new(source)
    } else {
        bail!("this is not an archive of this server: neither compressed nor encrypted as one");
    };

    let decompressed =
        zstd::stream::Decoder::new(plain).context("could not begin reading the archive")?;
    let boxed: Box<dyn Read + Send> = Box::new(decompressed);
    Ok(tar::Archive::new(boxed))
}

/// The manifest, which an archive of this server begins with.
pub fn manifest_of(entry: &mut tar::Entry<'_, Box<dyn Read + Send>>) -> Result<Manifest> {
    let path = entry.path()?.to_string_lossy().to_string();
    if path != manifest::NAME {
        bail!("this archive does not begin with its manifest: it is not one of this server's");
    }
    if entry.size() > manifest::MOST_BYTES {
        bail!("this archive's manifest is too long to be one of this server's");
    }
    let mut text = Vec::new();
    entry.take(manifest::MOST_BYTES).read_to_end(&mut text)?;
    serde_json::from_slice(&text).context("this archive's manifest could not be read")
}

/// Read only the manifest of an archive.
pub fn read_manifest(
    source: Box<dyn Read + Send>,
    passphrase: Option<&SecretString>,
) -> Result<Manifest> {
    let mut archive = open(source, passphrase)?;
    let mut entries = archive.entries().context("could not read the archive")?;
    let mut first = entries
        .next()
        .context("this archive is empty")?
        .context("could not read the archive")?;
    manifest_of(&mut first)
}

/// Copy an entry to `into`, measuring and hashing what it holds as it goes.
pub fn copy_hashed<R: Read, W: Write>(from: &mut R, into: &mut W) -> io::Result<(u64, String)> {
    let mut hashed = Counted::new(into);
    let size = io::copy(from, &mut hashed)?;
    hashed.flush()?;
    Ok((size, hex(&hashed.hasher.finalize())))
}

/// The SHA-256 of a file, and its size.
pub fn hash_file(path: &Path) -> Result<(u64, String)> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("could not read {}", path.display()))?;
    Ok(copy_hashed(&mut file, &mut io::sink())?)
}

/// The SHA-256 of some bytes, as hex.
pub fn sha256_of(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Encrypted or not: one writer either way, finished either way.
enum Sealed<W: Write> {
    Plain(W),
    Age(age::stream::StreamWriter<W>),
}

impl<W: Write> Sealed<W> {
    fn finish(self) -> Result<()> {
        match self {
            Self::Plain(mut w) => w.flush()?,
            Self::Age(w) => {
                w.finish()
                    .context("could not end the archive's encryption")?
                    .flush()?;
            }
        }
        Ok(())
    }
}

impl<W: Write> Write for Sealed<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(w) => w.write(buf),
            Self::Age(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(w) => w.flush(),
            Self::Age(w) => w.flush(),
        }
    }
}

/// A writer that counts and hashes what goes through it.
struct Counted<W: Write> {
    inner: W,
    count: u64,
    hasher: Sha256,
}

impl<W: Write> Counted<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            count: 0,
            hasher: Sha256::new(),
        }
    }
}

impl<W: Write> Write for Counted<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.count += n as u64;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::manifest::{File, Media};

    fn manifest(database: &Path) -> Manifest {
        let (size, sha256) = hash_file(database).unwrap();
        Manifest {
            format: manifest::FORMAT,
            server: env!("CARGO_PKG_VERSION").into(),
            migration: 36,
            created_at: "2026-10-19T02:00:00.000Z".into(),
            engine: "sqlite".into(),
            reason: "manual".into(),
            upgrade_to: None,
            files: vec![File {
                path: manifest::DATABASE.into(),
                size,
                sha256,
            }],
            media: Media {
                included: true,
                store: "filesystem".into(),
                files: 1,
                bytes: 5,
                note: None,
            },
        }
    }

    /// Write an archive of a small file and a medium, then read it back.
    fn round_trip(
        passphrase: Option<&SecretString>,
        opened_with: Option<&SecretString>,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        let dir = tempdir();
        let database = dir.join("database.sqlite");
        std::fs::write(&database, b"SQLite format 3\0 and the rest").unwrap();
        let medium = dir.join("medium");
        std::fs::write(&medium, b"bytes").unwrap();
        let key = format!("{}.jpg", "a".repeat(64));

        let mut written = Vec::new();
        let made = write(
            &mut written,
            passphrase,
            &manifest(&database),
            &database,
            &[Medium {
                key: &key,
                path: &medium,
                size: 5,
            }],
        )
        .unwrap();
        assert_eq!(made.size, written.len() as u64);
        assert_eq!(made.sha256, sha256_of(&written));
        assert_eq!(written.starts_with(AGE_MAGIC), passphrase.is_some());

        let mut archive = open(Box::new(io::Cursor::new(written)), opened_with)?;
        let mut files = Vec::new();
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_string_lossy().to_string();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            files.push((path, bytes));
        }
        let _ = std::fs::remove_dir_all(&dir);
        Ok(files)
    }

    fn tempdir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ams-archive-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_archive_holds_its_manifest_first_then_the_database_then_the_media() {
        let files = round_trip(None, None).unwrap();
        let paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "manifest.json",
                "database.sqlite",
                &format!("media/{}.jpg", "a".repeat(64))
            ]
        );
        assert_eq!(files[1].1, b"SQLite format 3\0 and the rest");
        assert_eq!(files[2].1, b"bytes");
        let manifest: Manifest = serde_json::from_slice(&files[0].1).unwrap();
        assert_eq!(manifest.database().unwrap().size, 29);
    }

    #[test]
    fn an_encrypted_archive_opens_with_its_passphrase_and_no_other() {
        let passphrase = SecretString::from("correct horse battery staple".to_string());
        let files = round_trip(Some(&passphrase), Some(&passphrase)).unwrap();
        assert_eq!(files.len(), 3);

        let wrong = SecretString::from("incorrect horse".to_string());
        let refused = round_trip(Some(&passphrase), Some(&wrong))
            .unwrap_err()
            .to_string();
        assert!(refused.contains("does not open"), "{refused}");

        let without = round_trip(Some(&passphrase), None).unwrap_err().to_string();
        assert!(without.contains("encrypted"), "{without}");
    }

    #[test]
    fn what_is_not_an_archive_is_said_to_be_so() {
        let refused = open(
            Box::new(io::Cursor::new(b"PK\x03\x04 a zip".to_vec())),
            None,
        )
        .err()
        .expect("refused")
        .to_string();
        assert!(refused.contains("not an archive"), "{refused}");
    }
}
