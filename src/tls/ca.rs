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

/// The authority, as kept on disk.
pub struct Authority {
    dir: PathBuf,
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
    /// The authority in `dir`, created for `names` when there is none yet.
    pub fn open(dir: &Path, names: &[Name]) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
        let cert_path = dir.join(CA_CERT);
        let key_path = dir.join(CA_KEY);

        let (key_pem, cert_pem) = match (cert_path.exists(), key_path.exists()) {
            (true, true) => (
                std::fs::read_to_string(&key_path)
                    .with_context(|| format!("could not read {}", key_path.display()))?,
                std::fs::read_to_string(&cert_path)
                    .with_context(|| format!("could not read {}", cert_path.display()))?,
            ),
            (false, false) => {
                let (key_pem, cert_pem) = create(names)?;
                write_private(&key_path, key_pem.as_bytes())?;
                write_atomic(&cert_path, cert_pem.as_bytes())?;
                tracing::info!(
                    dir = %dir.display(),
                    names = ?names.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "created the authority the clients are to trust"
                );
                (key_pem, cert_pem)
            }
            _ => bail!(
                "{} holds one of {CA_CERT} and {CA_KEY} but not the other; remove it to \
                 create the authority anew (every client will have to trust it again)",
                dir.display()
            ),
        };

        let parsed = parse(&cert_pem).context("the authority's certificate cannot be read")?;
        // The key must be the certificate's: a pair put together from two
        // backups would issue certificates no client can verify, silently.
        if !key_matches(&key_pem, &parsed.spki).context("the authority's key cannot be read")? {
            bail!(
                "{} is not the key of {}; restore the pair together, or remove both to create                  the authority anew (every client will have to trust it again)",
                key_path.display(),
                cert_path.display()
            );
        }

        Ok(Self {
            dir: dir.to_path_buf(),
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

    /// Issue a certificate for the names, and keep it beside the authority.
    pub fn issue(&self, names: &[Name]) -> Result<Issued> {
        let (allowed, refused) = self.permitted(names);
        for name in &refused {
            tracing::warn!(
                %name,
                "the authority may not certify this name: its constraints were set when it \
                 was created; remove ca.crt and ca.key to create it anew, and trust it again"
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
        let cert_pem = cert.pem();
        let key_pem = key.serialize_pem();
        write_private(&self.dir.join(KEY), key_pem.as_bytes())?;
        write_atomic(&self.dir.join(CERT), cert_pem.as_bytes())?;

        let parsed = parse(&cert_pem)?;
        tracing::info!(
            names = ?allowed.iter().map(ToString::to_string).collect::<Vec<_>>(),
            until = %parsed.not_after.date(),
            "issued the clients' certificate"
        );
        Ok(Issued {
            chain_pem: format!("{}\n{}", cert_pem.trim_end(), self.cert_pem.trim_end()),
            key_pem,
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
    pub fn issued(&self) -> Result<Option<Issued>> {
        let cert_path = self.dir.join(CERT);
        let key_path = self.dir.join(KEY);
        if !cert_path.exists() || !key_path.exists() {
            return Ok(None);
        }
        let cert_pem = std::fs::read_to_string(&cert_path)?;
        let key_pem = std::fs::read_to_string(&key_path)?;
        let parsed = match parse(&cert_pem) {
            Ok(parsed) => parsed,
            Err(e) => {
                tracing::warn!(error = %e, "the certificate on disk cannot be read; issuing anew");
                return Ok(None);
            }
        };
        if !self.verifies(&cert_pem) {
            tracing::warn!(
                "the certificate on disk is not this authority's, or no longer verifies; \
                 issuing anew"
            );
            return Ok(None);
        }
        if !key_matches(&key_pem, &parsed.spki).unwrap_or(false) {
            tracing::warn!("the key on disk is not the certificate's; issuing anew");
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
    #[test]
    fn the_authority_is_constrained_to_the_names_it_was_made_for() {
        let dir = scratch();
        let made_for = names(&["skyhook.sonarr.tv", "api.radarr.video", "ams.lan"]);
        let authority = Authority::open(&dir, &made_for).unwrap();
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
        let issued = authority.issue(&asked).unwrap();
        assert_eq!(issued.info.names, names(&["skyhook.sonarr.tv", "ams.lan"]));
        assert!(!issued.info.due());
        assert!(issued.info.covers(&names(&["ams.lan"])));
        assert!(!issued.info.covers(&names(&["example.com"])));
        assert!(!format!("{issued:?}").contains("PRIVATE KEY"));
        assert!(issued.chain_pem.matches("BEGIN CERTIFICATE").count() == 2);
        assert!(issued.key_pem.contains("PRIVATE KEY"));

        // Opened again: the same authority, and the certificate it issued.
        let again = Authority::open(&dir, &names(&["other.example"])).unwrap();
        assert_eq!(again.fingerprint, authority.fingerprint);
        let kept = again.issued().unwrap().expect("the certificate issued");
        assert_eq!(kept.info.fingerprint, issued.info.fingerprint);
        assert_eq!(kept.info.names, issued.info.names);

        // An authority made anew bears the same name; the certificate the old
        // one issued is not its own and is not kept.
        std::fs::remove_file(dir.join(CA_CERT)).unwrap();
        std::fs::remove_file(dir.join(CA_KEY)).unwrap();
        let renewed = Authority::open(&dir, &made_for).unwrap();
        assert_ne!(renewed.fingerprint, authority.fingerprint);
        assert!(
            renewed.issued().unwrap().is_none(),
            "the old authority's certificate must not be served under the new one"
        );

        // A key that is not the certificate's is refused, not used.
        let stray = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        std::fs::write(dir.join(CA_KEY), stray.serialize_pem()).unwrap();
        assert!(Authority::open(&dir, &made_for).is_err());

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
    #[test]
    fn an_address_it_was_made_for_is_certified_and_no_other() {
        let dir = scratch();
        let authority =
            Authority::open(&dir, &names(&["skyhook.sonarr.tv", "192.168.1.7"])).unwrap();
        assert!(authority.may_certify(&Name::parse("192.168.1.7").unwrap()));
        assert!(!authority.may_certify(&Name::parse("192.168.1.8").unwrap()));
        let issued = authority
            .issue(&names(&["skyhook.sonarr.tv", "192.168.1.7", "10.0.0.1"]))
            .unwrap();
        assert_eq!(
            issued.info.names,
            names(&["skyhook.sonarr.tv", "192.168.1.7"])
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
