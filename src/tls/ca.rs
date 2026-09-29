//! The authority this server keeps, and the certificate it issues itself.
//!
//! Sonarr, Radarr and the TMDB clients have their metadata addresses compiled
//! in — `https://skyhook.sonarr.tv/…`, `https://api.radarr.video/…`,
//! `https://api.themoviedb.org/…` — and reach this server only because those
//! names are resolved to it. No public authority will ever certify those
//! names to anyone else, so the certificate they are shown has to come from
//! an authority the clients are told to trust: this one, made on first start
//! and kept in the data directory.
//!
//! An authority a client trusts can, in principle, vouch for any name at all.
//! This one cannot: it carries *name constraints* (RFC 5280 §4.2.1.10) that
//! limit what it may certify to the names it was created for, so a machine
//! that trusts it has not, by that, trusted this server with the rest of the
//! web. The constraints are set once, when the authority is created: a name
//! added later that they do not cover is left out of the certificate, with a
//! warning that says so.

use std::{
    net::IpAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use rcgen::{
    BasicConstraints, CertificateParams, CidrSubnet, DnType, ExtendedKeyUsagePurpose,
    GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose, NameConstraints,
    PKCS_ECDSA_P256_SHA256,
};
use sha2::{Digest as _, Sha256};
use time::{Duration, OffsetDateTime};
use x509_parser::prelude::{FromDer as _, GeneralName, X509Certificate};

/// The files, under the TLS directory.
pub const CA_CERT: &str = "ca.crt";
pub const CA_KEY: &str = "ca.key";
pub const CERT: &str = "server.crt";
pub const KEY: &str = "server.key";

const CA_NAME: &str = "arr-metadata-server local CA";
/// How long the authority stands.
const CA_DAYS: i64 = 365 * 10;
/// How long a certificate it issues is good for.
const CERT_DAYS: i64 = 365;
/// A certificate with less than this left is issued anew.
pub const RENEW_BEFORE_DAYS: i64 = 30;

/// A name a certificate can be issued for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Name {
    Dns(String),
    Ip(IpAddr),
}

impl Name {
    /// A hostname or an address, as somebody typed it. `None` for what is
    /// neither.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_matches('.').to_ascii_lowercase();
        if text.is_empty() {
            return None;
        }
        if let Ok(ip) = text.parse::<IpAddr>() {
            return Some(Self::Ip(ip));
        }
        let hostname = text.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        }) && text.len() <= 253;
        hostname.then_some(Self::Dns(text))
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dns(name) => f.write_str(name),
            Self::Ip(ip) => write!(f, "{ip}"),
        }
    }
}

/// Where the authority and what it issues are kept: the TLS directory,
/// alone; the database, among several instances, so that every one of them
/// shows the same certificate and a new one takes the authority the others
/// already trust rather than making its own.
#[derive(Clone)]
pub enum Keep {
    Files(PathBuf),
    /// The database — and the directory an authority may already be in
    /// from before, taken into the database the first time.
    Store {
        db: crate::db::Db,
        from: PathBuf,
    },
}

/// A certificate and its key, as one entry: written together or not at
/// all, so two instances starting together cannot leave a certificate with
/// the other's key.
#[derive(serde::Serialize, serde::Deserialize)]
struct Pair {
    cert: String,
    key: String,
}

impl Keep {
    /// The authority's pair, when there is one.
    async fn read_authority(&self) -> Result<Option<Pair>> {
        match self {
            Self::Files(dir) => read_pair_files(&dir.join(CA_CERT), &dir.join(CA_KEY)),
            Self::Store { db, .. } => {
                read_pair_store(db, crate::db::repo::keystore::names::CA).await
            }
        }
    }

    /// Keep the authority's pair, unless one is there already: the one
    /// there afterwards.
    async fn keep_authority(&self, pair: Pair) -> Result<Pair> {
        match self {
            Self::Files(dir) => {
                write_private(&dir.join(CA_KEY), pair.key.as_bytes())?;
                write_atomic(&dir.join(CA_CERT), pair.cert.as_bytes())?;
                Ok(pair)
            }
            Self::Store { db, .. } => {
                keep_pair_store(db, crate::db::repo::keystore::names::CA, &pair).await
            }
        }
    }

    /// The certificate issued last, when there is one.
    async fn read_issued(&self) -> Result<Option<Pair>> {
        Ok(self.read_issued_raw().await?.and_then(|(_, pair)| pair))
    }

    /// The certificate issued last, as kept: its text, and the pair it
    /// reads as — none when it cannot be read, which is said, so that it is
    /// replaced rather than served or refused.
    async fn read_issued_raw(&self) -> Result<Option<(String, Option<Pair>)>> {
        match self {
            Self::Files(dir) => Ok(read_pair_files(&dir.join(CERT), &dir.join(KEY))?
                .map(|pair| (String::new(), Some(pair)))),
            Self::Store { db, .. } => {
                let name = crate::db::repo::keystore::names::CERT;
                match crate::db::repo::keystore::get(db, name).await? {
                    Some(text) => {
                        let pair = serde_json::from_str::<Pair>(&text).ok();
                        if pair.is_none() {
                            tracing::warn!("{name} in the database cannot be read; issuing anew");
                        }
                        Ok(Some((text, pair)))
                    }
                    None => Ok(None),
                }
            }
        }
    }

    /// Keep a certificate issued: the one there afterwards is what is
    /// served. `Over` writes whatever was there — a renewal; `IfAbsent`
    /// writes only where there is none, and `Replacing` only over the pair
    /// it was decided against — so two instances starting together, or
    /// both finding the kept one unusable, settle on one certificate.
    async fn keep_issued(&self, pair: Pair, how: Write) -> Result<Pair> {
        match self {
            Self::Files(dir) => {
                write_private(&dir.join(KEY), pair.key.as_bytes())?;
                write_atomic(&dir.join(CERT), pair.cert.as_bytes())?;
                Ok(pair)
            }
            Self::Store { db, .. } => {
                let name = crate::db::repo::keystore::names::CERT;
                let text = serde_json::to_string(&pair)?;
                match how {
                    Write::Over => {
                        crate::db::repo::keystore::put(db, name, &text).await?;
                        Ok(pair)
                    }
                    Write::IfAbsent => keep_pair_store(db, name, &pair).await,
                    Write::Replacing(old) => {
                        if crate::db::repo::keystore::replace_if(db, name, &old, &text).await? {
                            return Ok(pair);
                        }
                        // Another instance replaced it first: what it
                        // kept, or what is there now, whichever.
                        match read_pair_store(db, name).await? {
                            Some(kept) => Ok(kept),
                            None => keep_pair_store(db, name, &pair).await,
                        }
                    }
                }
            }
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Files(dir) => dir.display().to_string(),
            Self::Store { .. } => "the database".to_string(),
        }
    }

    /// Replace the authority kept, when `asked` is its fingerprint, by one
    /// made for `names`: what is kept afterwards. The one replaced is kept
    /// beside it, named after its fingerprint, so it can be put back.
    async fn replace_authority(&self, current: Pair, asked: &str, names: &[Name]) -> Result<Pair> {
        let parsed = parse(&current.cert).context("the authority's certificate cannot be read")?;
        let old = parsed.fingerprint;
        let bare = |fingerprint: &str| {
            fingerprint
                .chars()
                .filter(char::is_ascii_hexdigit)
                .collect::<String>()
                .to_ascii_uppercase()
        };
        if bare(asked) != bare(&old) {
            tracing::warn!(
                asked,
                kept = %old,
                "AMS_TLS_REPLACE_AUTHORITY does not name the authority kept, which is left as \
                 it is; once the authority it named has been replaced, remove the setting"
            );
            return Ok(current);
        }
        // A key that is not its certificate's is a pair half-restored, or half
        // replaced: nothing is replaced, and nothing kept aside is written over.
        if !key_matches(&current.key, &parsed.spki).context("the authority's key cannot be read")? {
            bail!(
                "the authority's key in {} is not its certificate's; restore the pair kept \
                 aside (ca.crt.replaced-… and ca.key.replaced-…) before replacing it",
                self.describe()
            );
        }

        let (key, cert) = create(names)?;
        let made = Pair { cert, key };
        let aside = format!("replaced-{}", &bare(&old)[..16]);
        let kept = match self {
            Self::Files(dir) => {
                // Kept aside once: what is already there is the authority
                // that was replaced first, and the way back.
                let (key_aside, cert_aside) = (
                    dir.join(format!("{CA_KEY}.{aside}")),
                    dir.join(format!("{CA_CERT}.{aside}")),
                );
                if !key_aside.exists() && !cert_aside.exists() {
                    write_private(&key_aside, current.key.as_bytes())?;
                    write_atomic(&cert_aside, current.cert.as_bytes())?;
                }
                write_private(&dir.join(CA_KEY), made.key.as_bytes())?;
                write_atomic(&dir.join(CA_CERT), made.cert.as_bytes())?;
                made
            }
            Self::Store { db, .. } => {
                use crate::db::repo::keystore;
                let name = keystore::names::CA;
                // Judged again on what is there now, and replaced only over
                // exactly that: another instance may have replaced it since.
                let stored = keystore::get(db, name)
                    .await?
                    .context("the authority is no longer in the database")?;
                let there: Pair = serde_json::from_str(&stored)
                    .with_context(|| format!("{name} in the database cannot be read"))?;
                if bare(&parse(&there.cert)?.fingerprint) != bare(&old) {
                    tracing::info!("another instance replaced the authority first; taking its one");
                    return Ok(there);
                }
                keystore::put_if_absent(db, &format!("{name}.{aside}"), &stored).await?;
                let text = serde_json::to_string(&made)?;
                if keystore::replace_if(db, name, &stored, &text).await? {
                    made
                } else {
                    tracing::info!("another instance replaced the authority first; taking its one");
                    return read_pair_store(db, name)
                        .await?
                        .context("the authority is no longer in the database");
                }
            }
        };

        let new = parse(&kept.cert)?.fingerprint;
        tracing::warn!(
            replaced = %old,
            by = %new,
            names = ?names.iter().map(ToString::to_string).collect::<Vec<_>>(),
            kept_aside_as = %aside,
            "replaced the authority: every client has to trust the new one (restart the \
             containers that run trust-ca.sh, update AMS_CA_FINGERPRINT where it is set), and \
             AMS_TLS_REPLACE_AUTHORITY can be removed"
        );
        Ok(kept)
    }
}

/// How a certificate issued is written where it is kept.
enum Write {
    Over,
    IfAbsent,
    /// Over exactly this text, which was read and found wanting.
    Replacing(String),
}

fn read_pair_files(cert_path: &Path, key_path: &Path) -> Result<Option<Pair>> {
    match (cert_path.exists(), key_path.exists()) {
        (true, true) => Ok(Some(Pair {
            key: std::fs::read_to_string(key_path)
                .with_context(|| format!("could not read {}", key_path.display()))?,
            cert: std::fs::read_to_string(cert_path)
                .with_context(|| format!("could not read {}", cert_path.display()))?,
        })),
        (false, false) => Ok(None),
        _ => bail!(
            "{} holds one of {} and {} but not the other; remove it to create the pair anew \
             (an authority made anew has to be trusted again by every client)",
            cert_path.parent().unwrap_or(cert_path).display(),
            cert_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("cert"),
            key_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("key"),
        ),
    }
}

async fn read_pair_store(db: &crate::db::Db, name: &str) -> Result<Option<Pair>> {
    match crate::db::repo::keystore::get(db, name).await? {
        Some(text) => {
            Ok(Some(serde_json::from_str(&text).with_context(|| {
                format!("{name} in the database cannot be read")
            })?))
        }
        None => Ok(None),
    }
}

async fn keep_pair_store(db: &crate::db::Db, name: &str, pair: &Pair) -> Result<Pair> {
    let text = serde_json::to_string(pair)?;
    let stored = crate::db::repo::keystore::put_if_absent(db, name, &text).await?;
    serde_json::from_str(&stored).with_context(|| format!("{name} in the database cannot be read"))
}

/// The authority, as kept.
pub struct Authority {
    keep: Keep,
    key_pem: String,
    cert_pem: String,
    pub fingerprint: String,
    pub subject: String,
    pub not_after: OffsetDateTime,
    /// What the constraints let it certify.
    permitted_dns: Vec<String>,
    permitted_ips: Vec<IpAddr>,
    /// Whether every address is excluded: the case when it was created for
    /// hostnames alone.
    ips_excluded: bool,
}

/// A certificate the authority issued, with its key: what the listener
/// serves with, and nothing else reads. The key is shown to nobody, the
/// debug output included.
pub struct Issued {
    /// The certificate, then the authority's: the chain a client is shown.
    pub chain_pem: String,
    pub key_pem: String,
    pub info: Info,
}

impl std::fmt::Debug for Issued {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Issued")
            .field("info", &self.info)
            .field("key_pem", &"<private>")
            .finish()
    }
}

/// What is known of the certificate issued: everything but its key.
#[derive(Clone, Debug)]
pub struct Info {
    pub names: Vec<Name>,
    pub not_after: OffsetDateTime,
    pub fingerprint: String,
}

impl Info {
    /// Whether it is time to issue anew.
    pub fn due(&self) -> bool {
        self.not_after - OffsetDateTime::now_utc() < Duration::days(RENEW_BEFORE_DAYS)
    }

    /// Whether it carries every name asked.
    pub fn covers(&self, names: &[Name]) -> bool {
        names.iter().all(|name| self.names.contains(name))
    }
}

impl Authority {
    /// The authority as kept, created for `names` when there is none yet —
    /// or, in the database, taken from the directory an earlier deployment
    /// kept it in, so the clients need not trust a new one.
    ///
    /// With `replace` naming its fingerprint, the authority kept is replaced
    /// by one made for `names`, and kept beside it for a way back; naming any
    /// other, it is left as it is.
    pub async fn open(keep: Keep, names: &[Name], replace: Option<&str>) -> Result<Self> {
        if let Keep::Files(dir) = &keep {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("could not create {}", dir.display()))?;
        }

        let pair = match keep.read_authority().await? {
            Some(pair) => pair,
            None => {
                let from_files = match &keep {
                    Keep::Store { from, .. } => {
                        read_pair_files(&from.join(CA_CERT), &from.join(CA_KEY))?
                    }
                    Keep::Files(_) => None,
                };
                let (pair, made) = match from_files {
                    Some(pair) => (pair, false),
                    None => {
                        let (key, cert) = create(names)?;
                        (Pair { cert, key }, true)
                    }
                };
                let own = pair.cert.clone();
                let kept = keep.keep_authority(pair).await?;
                tracing::info!(
                    kept_in = %keep.describe(),
                    names = ?names.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "{}",
                    if kept.cert != own {
                        "took the authority another instance created"
                    } else if made {
                        "created the authority the clients are to trust"
                    } else {
                        "took the authority from its files into the database"
                    }
                );
                kept
            }
        };
        let pair = match replace {
            Some(asked) => keep.replace_authority(pair, asked, names).await?,
            None => pair,
        };
        let Pair {
            cert: cert_pem,
            key: key_pem,
        } = pair;

        let parsed = parse(&cert_pem).context("the authority's certificate cannot be read")?;
        // The key must be the certificate's: a pair put together from two
        // backups would issue certificates no client can verify, silently.
        if !key_matches(&key_pem, &parsed.spki).context("the authority's key cannot be read")? {
            bail!(
                "the authority's key in {} is not its certificate's; restore the pair together, \
                 or remove both to create the authority anew (every client will have to trust \
                 it again)",
                keep.describe()
            );
        }

        Ok(Self {
            keep,
            key_pem,
            cert_pem,
            fingerprint: parsed.fingerprint,
            subject: parsed.subject,
            not_after: parsed.not_after,
            permitted_dns: parsed.permitted_dns,
            permitted_ips: parsed.permitted_ips,
            ips_excluded: parsed.ips_excluded,
        })
    }

    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// Whether the constraints let the authority certify a name.
    pub fn may_certify(&self, name: &Name) -> bool {
        match name {
            Name::Dns(dns) => {
                self.permitted_dns.is_empty()
                    || self
                        .permitted_dns
                        .iter()
                        .any(|p| dns == p || dns.ends_with(&format!(".{p}")))
            }
            Name::Ip(ip) => {
                !self.ips_excluded
                    && (self.permitted_ips.is_empty() || self.permitted_ips.contains(ip))
            }
        }
    }

    /// The names asked, split into what may be certified and what may not.
    pub fn permitted(&self, names: &[Name]) -> (Vec<Name>, Vec<Name>) {
        names
            .iter()
            .cloned()
            .partition(|name| self.may_certify(name))
    }

    /// Issue a certificate for the names, and keep it beside the authority,
    /// over whatever was there.
    pub async fn issue(&self, names: &[Name]) -> Result<Issued> {
        self.issue_kept(names, Write::Over).await
    }

    /// Issue a certificate for the names, unless the authority already
    /// keeps one that will do — which is then what is served: two
    /// instances starting together settle on one certificate. One that
    /// will not do — run out, due, without a name asked, another
    /// authority's — is replaced, and two replacing it at once settle on
    /// one too.
    pub async fn issue_unless_kept(&self, names: &[Name]) -> Result<Issued> {
        let wanted = self.permitted(names).0;
        let how = match self.keep.read_issued_raw().await? {
            None => Write::IfAbsent,
            Some((text, kept)) => match kept.and_then(|kept| self.judge(&kept, &wanted)) {
                Some(issued) => return Ok(issued),
                // Replaced as it was read, so that two replacing it at once
                // settle on one.
                None => Write::Replacing(text),
            },
        };
        self.issue_kept(names, how).await
    }

    /// The certificate kept, when it will do: this authority's, its key its
    /// own, every name asked on it, and time left.
    fn judge(&self, kept: &Pair, wanted: &[Name]) -> Option<Issued> {
        let parsed = parse(&kept.cert).ok()?;
        if !self.verifies(&kept.cert) || !key_matches(&kept.key, &parsed.spki).unwrap_or(false) {
            return None;
        }
        let info = Info {
            names: parsed.names,
            not_after: parsed.not_after,
            fingerprint: parsed.fingerprint,
        };
        if !info.covers(wanted) || info.due() {
            return None;
        }
        Some(Issued {
            chain_pem: format!("{}\n{}", kept.cert.trim_end(), self.cert_pem.trim_end()),
            key_pem: kept.key.clone(),
            info,
        })
    }

    async fn issue_kept(&self, names: &[Name], how: Write) -> Result<Issued> {
        let (allowed, refused) = self.permitted(names);
        for name in &refused {
            tracing::warn!(
                %name,
                "the authority may not certify this name: its constraints were set when it \
                 was created; AMS_TLS_REPLACE_AUTHORITY, set to its fingerprint, replaces it with one for every name, which every client then has to trust"
            );
        }
        if allowed.is_empty() {
            bail!("the authority may certify none of the names asked");
        }

        let ca_key = KeyPair::from_pem(&self.key_pem)?;
        let issuer = Issuer::from_ca_cert_pem(&self.cert_pem, ca_key)?;
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;

        let mut params =
            CertificateParams::new(allowed.iter().map(ToString::to_string).collect::<Vec<_>>())?;
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;
        // A common name that is one of the names: what an old client reads,
        // and what the constraints allow.
        let common = allowed
            .iter()
            .find_map(|name| match name {
                Name::Dns(dns) => Some(dns.clone()),
                Name::Ip(_) => None,
            })
            .unwrap_or_else(|| allowed[0].to_string());
        params.distinguished_name.push(DnType::CommonName, common);
        let now = OffsetDateTime::now_utc();
        params.not_before = now - Duration::days(1);
        // Never past the authority's own end: a certificate that outlives
        // its issuer verifies for nobody.
        params.not_after = (now + Duration::days(CERT_DAYS)).min(self.not_after);
        if self.not_after - now < Duration::days(CERT_DAYS) {
            tracing::warn!(
                until = %self.not_after.date(),
                "the authority ends within the year: create it anew before then, and trust \
                 it again on every client"
            );
        }

        let cert = params.signed_by(&key, &issuer)?;
        let made = Pair {
            cert: cert.pem(),
            key: key.serialize_pem(),
        };
        let kept = self.keep.keep_issued(made, how).await?;
        let taken = kept.cert != cert.pem();
        let parsed = parse(&kept.cert)?;
        if taken {
            // Another instance issued first: what it kept is what every
            // instance serves — as long as it will do.
            if self.judge(&kept, &allowed).is_none() {
                bail!(
                    "the certificate another instance kept is not this authority's, or will \
                     not do; renew it from the interface"
                );
            }
            tracing::info!(
                until = %parsed.not_after.date(),
                "took the clients' certificate another instance issued"
            );
        } else {
            tracing::info!(
                names = ?allowed.iter().map(ToString::to_string).collect::<Vec<_>>(),
                until = %parsed.not_after.date(),
                "issued the clients' certificate"
            );
        }
        Ok(Issued {
            chain_pem: format!("{}\n{}", kept.cert.trim_end(), self.cert_pem.trim_end()),
            key_pem: kept.key,
            info: Info {
                names: parsed.names,
                not_after: parsed.not_after,
                fingerprint: parsed.fingerprint,
            },
        })
    }

    /// The certificate issued last time, when there is one, it is this
    /// authority's — signed by it, not merely bearing its name, since every
    /// authority made here bears the same — it still verifies, and its key
    /// is its own. Anything else is issued anew.
    pub async fn issued(&self) -> Result<Option<Issued>> {
        let Some(Pair {
            cert: cert_pem,
            key: key_pem,
        }) = self.keep.read_issued().await?
        else {
            return Ok(None);
        };
        let parsed = match parse(&cert_pem) {
            Ok(parsed) => parsed,
            Err(e) => {
                tracing::warn!(error = %e, "the certificate kept cannot be read; issuing anew");
                return Ok(None);
            }
        };
        if !self.verifies(&cert_pem) {
            tracing::warn!(
                "the certificate kept is not this authority's, or no longer verifies; \
                 issuing anew"
            );
            return Ok(None);
        }
        if !key_matches(&key_pem, &parsed.spki).unwrap_or(false) {
            tracing::warn!("the key kept is not the certificate's; issuing anew");
            return Ok(None);
        }
        Ok(Some(Issued {
            chain_pem: format!("{}\n{}", cert_pem.trim_end(), self.cert_pem.trim_end()),
            key_pem,
            info: Info {
                names: parsed.names,
                not_after: parsed.not_after,
                fingerprint: parsed.fingerprint,
            },
        }))
    }

    /// Whether a certificate chains to this authority and is good for a
    /// server today — as rustls would judge it, since that is what serves it.
    fn verifies(&self, cert_pem: &str) -> bool {
        use rustls_pki_types::pem::PemObject as _;
        let (Ok(leaf), Ok(root)) = (
            rustls_pki_types::CertificateDer::from_pem_slice(cert_pem.as_bytes()),
            rustls_pki_types::CertificateDer::from_pem_slice(self.cert_pem.as_bytes()),
        ) else {
            return false;
        };
        let Ok(anchor) = webpki::anchor_from_trusted_cert(&root) else {
            return false;
        };
        let Ok(end) = webpki::EndEntityCert::try_from(&leaf) else {
            return false;
        };
        let now = rustls_pki_types::UnixTime::since_unix_epoch(std::time::Duration::from_secs(
            OffsetDateTime::now_utc().unix_timestamp().max(0) as u64,
        ));
        end.verify_for_usage(
            webpki::ALL_VERIFICATION_ALGS,
            &[anchor],
            &[],
            now,
            webpki::KeyUsage::server_auth(),
            None,
            None,
        )
        .is_ok()
    }
}

/// A new authority, constrained to the names.
fn create(names: &[Name]) -> Result<(String, String)> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.distinguished_name.push(DnType::CommonName, CA_NAME);
    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(CA_DAYS);

    let mut permitted: Vec<GeneralSubtree> = names
        .iter()
        .filter_map(|name| match name {
            Name::Dns(dns) => Some(GeneralSubtree::DnsName(dns.clone())),
            Name::Ip(_) => None,
        })
        .collect();
    let ips: Vec<GeneralSubtree> = names
        .iter()
        .filter_map(|name| match name {
            Name::Ip(ip @ IpAddr::V4(_)) => Some(GeneralSubtree::IpAddress(
                CidrSubnet::from_addr_prefix(*ip, 32),
            )),
            Name::Ip(ip @ IpAddr::V6(_)) => Some(GeneralSubtree::IpAddress(
                CidrSubnet::from_addr_prefix(*ip, 128),
            )),
            Name::Dns(_) => None,
        })
        .collect();
    // A constraint applies to the kind of name it names; with no address
    // permitted, every address must be excluded, or any would do.
    let excluded = if ips.is_empty() {
        vec![
            GeneralSubtree::IpAddress(CidrSubnet::from_addr_prefix(
                IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                0,
            )),
            GeneralSubtree::IpAddress(CidrSubnet::from_addr_prefix(
                IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
                0,
            )),
        ]
    } else {
        permitted.extend(ips);
        Vec::new()
    };
    params.name_constraints = Some(NameConstraints {
        permitted_subtrees: permitted,
        excluded_subtrees: excluded,
    });

    let cert = params.self_signed(&key)?;
    Ok((key.serialize_pem(), cert.pem()))
}

/// Whether a key in PEM is the one a certificate's public key belongs to.
fn key_matches(key_pem: &str, spki: &[u8]) -> Result<bool> {
    use rcgen::PublicKeyData as _;
    let key = KeyPair::from_pem(key_pem)?;
    Ok(key.subject_public_key_info() == spki)
}

/// What a certificate says, read back from its PEM.
struct Parsed {
    fingerprint: String,
    subject: String,
    /// Its public key, whole, as the certificate carries it.
    spki: Vec<u8>,
    not_after: OffsetDateTime,
    names: Vec<Name>,
    permitted_dns: Vec<String>,
    permitted_ips: Vec<IpAddr>,
    ips_excluded: bool,
}

fn parse(pem: &str) -> Result<Parsed> {
    let (_, doc) = x509_parser::pem::parse_x509_pem(pem.as_bytes())
        .map_err(|e| anyhow::anyhow!("not a PEM certificate: {e}"))?;
    let (_, cert) = X509Certificate::from_der(&doc.contents)
        .map_err(|e| anyhow::anyhow!("not an X.509 certificate: {e}"))?;

    let mut names = Vec::new();
    if let Ok(Some(san)) = cert.subject_alternative_name() {
        for general in &san.value.general_names {
            match general {
                GeneralName::DNSName(dns) => names.push(Name::Dns(dns.to_ascii_lowercase())),
                GeneralName::IPAddress(bytes) => {
                    if let Some(ip) = ip_from_bytes(bytes) {
                        names.push(Name::Ip(ip));
                    }
                }
                _ => {}
            }
        }
    }

    let (mut permitted_dns, mut permitted_ips, mut ips_excluded) = (Vec::new(), Vec::new(), false);
    if let Ok(Some(constraints)) = cert.name_constraints() {
        for subtree in constraints.value.permitted_subtrees.iter().flatten() {
            match &subtree.base {
                GeneralName::DNSName(dns) => permitted_dns.push(dns.to_ascii_lowercase()),
                GeneralName::IPAddress(bytes) => {
                    // Address and mask, halved: only the address is kept, the
                    // constraints this server writes being single addresses.
                    if let Some(ip) = ip_from_bytes(&bytes[..bytes.len() / 2]) {
                        permitted_ips.push(ip);
                    }
                }
                _ => {}
            }
        }
        ips_excluded = constraints
            .value
            .excluded_subtrees
            .iter()
            .flatten()
            .any(|subtree| {
                matches!(&subtree.base, GeneralName::IPAddress(bytes)
                    if bytes.iter().all(|b| *b == 0))
            });
    }

    Ok(Parsed {
        fingerprint: fingerprint(&doc.contents),
        subject: cert.subject().to_string(),
        spki: cert.public_key().raw.to_vec(),
        not_after: cert.validity().not_after.to_datetime(),
        names,
        permitted_dns,
        permitted_ips,
        ips_excluded,
    })
}

fn ip_from_bytes(bytes: &[u8]) -> Option<IpAddr> {
    match bytes.len() {
        4 => Some(IpAddr::from(<[u8; 4]>::try_from(bytes).ok()?)),
        16 => Some(IpAddr::from(<[u8; 16]>::try_from(bytes).ok()?)),
        _ => None,
    }
}

/// The SHA-256 of the certificate, as a browser shows it.
fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// A key file, readable by this process alone — whatever mode the file had
/// before, and whole or not at all.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    write_file(path, bytes, Some(0o600))
}

/// A file written whole or not at all: to a sibling first, then renamed
/// over, so a crash mid-way leaves what was there.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_file(path, bytes, None)
}

fn write_file(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<()> {
    use std::io::Write as _;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(mode);
    }
    let mut file = options
        .open(&tmp)
        .with_context(|| format!("could not write {}", tmp.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    std::fs::rename(&tmp, path).with_context(|| format!("could not write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<Name> {
        list.iter().map(|n| Name::parse(n).unwrap()).collect()
    }

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("ams-tls-test-{}", crate::db::new_id()))
    }

    #[test]
    fn a_name_is_a_hostname_or_an_address() {
        assert_eq!(
            Name::parse(" Skyhook.Sonarr.TV. "),
            Some(Name::Dns("skyhook.sonarr.tv".into()))
        );
        assert_eq!(
            Name::parse("192.168.1.7"),
            Some(Name::Ip("192.168.1.7".parse().unwrap()))
        );
        assert_eq!(Name::parse("::1"), Some(Name::Ip("::1".parse().unwrap())));
        for bad in ["", "-a.b", "a_b.c", "a..b", "a b"] {
            assert_eq!(Name::parse(bad), None, "{bad:?}");
        }
    }

    /// The authority certifies what it was made for, and nothing else; the
    /// certificate it issues carries the names, its signature and its end.
    #[tokio::test]
    async fn the_authority_is_constrained_to_the_names_it_was_made_for() {
        let dir = scratch();
        let made_for = names(&["skyhook.sonarr.tv", "api.radarr.video", "ams.lan"]);
        let authority = Authority::open(Keep::Files(dir.clone()), &made_for, None)
            .await
            .unwrap();
        assert_eq!(authority.subject, format!("CN={CA_NAME}"));
        assert!(authority.may_certify(&Name::parse("skyhook.sonarr.tv").unwrap()));
        assert!(authority.may_certify(&Name::parse("deep.ams.lan").unwrap()));
        assert!(!authority.may_certify(&Name::parse("example.com").unwrap()));
        assert!(
            !authority.may_certify(&Name::parse("10.0.0.1").unwrap()),
            "made for hostnames alone, every address is excluded"
        );
        assert_eq!(authority.fingerprint.len(), 32 * 3 - 1);

        let asked = names(&["skyhook.sonarr.tv", "example.com", "ams.lan"]);
        let issued = authority.issue(&asked).await.unwrap();
        assert_eq!(issued.info.names, names(&["skyhook.sonarr.tv", "ams.lan"]));
        assert!(!issued.info.due());
        assert!(issued.info.covers(&names(&["ams.lan"])));
        assert!(!issued.info.covers(&names(&["example.com"])));
        assert!(!format!("{issued:?}").contains("PRIVATE KEY"));
        assert!(issued.chain_pem.matches("BEGIN CERTIFICATE").count() == 2);
        assert!(issued.key_pem.contains("PRIVATE KEY"));

        // Opened again: the same authority, and the certificate it issued.
        let again = Authority::open(Keep::Files(dir.clone()), &names(&["other.example"]), None)
            .await
            .unwrap();
        assert_eq!(again.fingerprint, authority.fingerprint);
        let kept = again
            .issued()
            .await
            .unwrap()
            .expect("the certificate issued");
        assert_eq!(kept.info.fingerprint, issued.info.fingerprint);
        assert_eq!(kept.info.names, issued.info.names);

        // An authority made anew bears the same name; the certificate the old
        // one issued is not its own and is not kept.
        std::fs::remove_file(dir.join(CA_CERT)).unwrap();
        std::fs::remove_file(dir.join(CA_KEY)).unwrap();
        let renewed = Authority::open(Keep::Files(dir.clone()), &made_for, None)
            .await
            .unwrap();
        assert_ne!(renewed.fingerprint, authority.fingerprint);
        assert!(
            renewed.issued().await.unwrap().is_none(),
            "the old authority's certificate must not be served under the new one"
        );

        // A key that is not the certificate's is refused, not used.
        let stray = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        std::fs::write(dir.join(CA_KEY), stray.serialize_pem()).unwrap();
        assert!(
            Authority::open(Keep::Files(dir.clone()), &made_for, None)
                .await
                .is_err()
        );

        // The signature is the authority's: webpki, which rustls uses, says so.
        use rustls_pki_types::pem::PemObject as _;
        let chain: Vec<rustls_pki_types::CertificateDer<'_>> =
            rustls_pki_types::CertificateDer::pem_slice_iter(issued.chain_pem.as_bytes())
                .collect::<std::result::Result<_, _>>()
                .unwrap();
        assert_eq!(chain.len(), 2);
        let anchor = webpki::anchor_from_trusted_cert(&chain[1]).unwrap();
        let leaf = webpki::EndEntityCert::try_from(&chain[0]).unwrap();
        let now = rustls_pki_types::UnixTime::since_unix_epoch(std::time::Duration::from_secs(
            OffsetDateTime::now_utc().unix_timestamp() as u64,
        ));
        leaf.verify_for_usage(
            webpki::ALL_VERIFICATION_ALGS,
            &[anchor],
            &[],
            now,
            webpki::KeyUsage::server_auth(),
            None,
            None,
        )
        .expect("the chain verifies");
        leaf.verify_is_valid_for_subject_name(
            &rustls_pki_types::ServerName::try_from("skyhook.sonarr.tv").unwrap(),
        )
        .expect("issued for skyhook");
        assert!(
            leaf.verify_is_valid_for_subject_name(
                &rustls_pki_types::ServerName::try_from("example.com").unwrap()
            )
            .is_err(),
            "not for a name the authority may not certify"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Made for an address too, the authority certifies that address and no
    /// other.
    #[tokio::test]
    async fn an_address_it_was_made_for_is_certified_and_no_other() {
        let dir = scratch();
        let authority = Authority::open(
            Keep::Files(dir.clone()),
            &names(&["skyhook.sonarr.tv", "192.168.1.7"]),
            None,
        )
        .await
        .unwrap();
        assert!(authority.may_certify(&Name::parse("192.168.1.7").unwrap()));
        assert!(!authority.may_certify(&Name::parse("192.168.1.8").unwrap()));
        let issued = authority
            .issue(&names(&["skyhook.sonarr.tv", "192.168.1.7", "10.0.0.1"]))
            .await
            .unwrap();
        assert_eq!(
            issued.info.names,
            names(&["skyhook.sonarr.tv", "192.168.1.7"])
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Kept in the database, the authority is one for every instance: the
    /// second to open it takes the first's, a certificate issued at start is
    /// the one already there, and a renewal replaces it for everybody. An
    /// authority a directory held from before is taken in, not replaced.
    #[tokio::test]
    async fn in_the_database_every_instance_holds_the_same() {
        let db = crate::db::Db::connect(&crate::config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .unwrap();
        db.migrate().await.unwrap();
        let made_for = names(&["skyhook.sonarr.tv", "ams.lan"]);

        // From before: an authority in files.
        let dir = scratch();
        let earlier = Authority::open(Keep::Files(dir.clone()), &made_for, None)
            .await
            .unwrap();
        let keep = Keep::Store {
            db: db.clone(),
            from: dir.clone(),
        };
        let a = Authority::open(keep.clone(), &made_for, None)
            .await
            .unwrap();
        assert_eq!(a.fingerprint, earlier.fingerprint, "taken from the files");
        let b = Authority::open(keep.clone(), &made_for, None)
            .await
            .unwrap();
        assert_eq!(b.fingerprint, a.fingerprint);
        assert!(a.issued().await.unwrap().is_none());

        // Both start: whichever issues first is served by both.
        let first = a.issue_unless_kept(&made_for).await.unwrap();
        let second = b.issue_unless_kept(&made_for).await.unwrap();
        assert_eq!(second.info.fingerprint, first.info.fingerprint);
        assert_eq!(second.key_pem, first.key_pem);
        assert_eq!(
            b.issued().await.unwrap().unwrap().info.fingerprint,
            first.info.fingerprint
        );

        // A renewal replaces it, for the other instance too.
        let renewed = a.issue(&made_for).await.unwrap();
        assert_ne!(renewed.info.fingerprint, first.info.fingerprint);
        assert_eq!(
            b.issued().await.unwrap().unwrap().info.fingerprint,
            renewed.info.fingerprint
        );

        // A kept certificate without a name now asked will not do: the
        // instance that starts with the new name replaces it, and the
        // other takes the replacement rather than serving the old one.
        let more = names(&["skyhook.sonarr.tv", "ams.lan", "new.ams.lan"]);
        let widened = a.issue_unless_kept(&more).await.unwrap();
        assert_ne!(widened.info.fingerprint, renewed.info.fingerprint);
        assert!(widened.info.covers(&more));
        assert_eq!(
            b.issue_unless_kept(&more).await.unwrap().info.fingerprint,
            widened.info.fingerprint
        );
        // One that cannot be read is replaced too, not served or refused.
        crate::db::repo::keystore::put(&db, crate::db::repo::keystore::names::CERT, "{}")
            .await
            .unwrap();
        assert!(a.issued().await.is_err() || a.issued().await.unwrap().is_none());
        let again = a.issue_unless_kept(&made_for).await.unwrap();
        assert!(again.info.covers(&made_for));

        // Without files to take from, an authority is made.
        let other = crate::db::Db::connect(&crate::config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .unwrap();
        other.migrate().await.unwrap();
        let fresh = Authority::open(
            Keep::Store {
                db: other,
                from: scratch(),
            },
            &made_for,
            None,
        )
        .await
        .unwrap();
        assert_ne!(fresh.fingerprint, a.fingerprint);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// An authority's constraints are set when it is made: a name added
    /// later, `services.sonarr.tv` for one, is covered only by a new one.
    #[tokio::test]
    async fn an_authority_is_replaced_only_when_its_fingerprint_is_named() {
        let dir = scratch();
        let before = names(&["skyhook.sonarr.tv", "api.radarr.video"]);
        let after = names(&[
            "skyhook.sonarr.tv",
            "api.radarr.video",
            "services.sonarr.tv",
        ]);
        let old = Authority::open(Keep::Files(dir.clone()), &before, None)
            .await
            .unwrap();
        let services = Name::parse("services.sonarr.tv").unwrap();
        assert!(!old.may_certify(&services));

        // Another fingerprint named: nothing is replaced.
        let kept = Authority::open(Keep::Files(dir.clone()), &after, Some("AB:CD"))
            .await
            .unwrap();
        assert_eq!(kept.fingerprint, old.fingerprint);

        // Its own, however written: replaced, and the old one kept aside.
        let written = old.fingerprint.replace(':', "").to_lowercase();
        let new = Authority::open(Keep::Files(dir.clone()), &after, Some(&written))
            .await
            .unwrap();
        assert_ne!(new.fingerprint, old.fingerprint);
        assert!(new.may_certify(&services));
        let aside: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.contains(".replaced-"))
            .collect();
        assert_eq!(aside.len(), 2, "{aside:?}");

        // Left set on the next start, it names nothing any more.
        let again = Authority::open(Keep::Files(dir.clone()), &after, Some(&written))
            .await
            .unwrap();
        assert_eq!(again.fingerprint, new.fingerprint);

        // The certificate issued by the old one is issued anew.
        let issued = again.issue_unless_kept(&after).await.unwrap();
        assert!(issued.info.covers(&after));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_pair_half_restored_is_never_replaced() {
        // The key of one authority beside the certificate of another: what a
        // replacement interrupted between its two writes, or a restore of one
        // file of two, would leave. Nothing is replaced, nothing kept aside.
        let (dir, other) = (scratch(), scratch());
        let made_for = names(&["skyhook.sonarr.tv"]);
        let one = Authority::open(Keep::Files(dir.clone()), &made_for, None)
            .await
            .unwrap();
        Authority::open(Keep::Files(other.clone()), &made_for, None)
            .await
            .unwrap();
        std::fs::copy(other.join(CA_KEY), dir.join(CA_KEY)).unwrap();

        let refused =
            Authority::open(Keep::Files(dir.clone()), &made_for, Some(&one.fingerprint)).await;

        assert!(refused.is_err());
        let aside = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .any(|n| n.contains(".replaced-"));
        assert!(
            !aside,
            "nothing is kept aside from a pair that does not hold together"
        );
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&other).ok();
    }

    #[tokio::test]
    async fn an_authority_in_the_database_is_replaced_as_well() {
        let db = crate::db::Db::connect(&crate::config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .unwrap();
        db.migrate().await.unwrap();
        let keep = Keep::Store {
            db: db.clone(),
            from: scratch(),
        };
        let before = names(&["skyhook.sonarr.tv"]);
        let after = names(&["skyhook.sonarr.tv", "services.sonarr.tv"]);
        let old = Authority::open(keep.clone(), &before, None).await.unwrap();

        let new = Authority::open(keep.clone(), &after, Some(&old.fingerprint))
            .await
            .unwrap();
        assert_ne!(new.fingerprint, old.fingerprint);
        assert!(new.may_certify(&Name::parse("services.sonarr.tv").unwrap()));

        // Another instance starting with the same setting takes the new one.
        let other = Authority::open(keep.clone(), &after, Some(&old.fingerprint))
            .await
            .unwrap();
        assert_eq!(other.fingerprint, new.fingerprint);

        // And the old one is still in the database, for a way back.
        let name = format!(
            "{}.replaced-{}",
            crate::db::repo::keystore::names::CA,
            &old.fingerprint.replace(':', "")[..16]
        );
        assert!(
            crate::db::repo::keystore::get(&db, &name)
                .await
                .unwrap()
                .is_some()
        );
    }
}
