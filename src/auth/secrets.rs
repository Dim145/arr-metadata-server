//! Credential material: API keys and admin passwords.
//!
//! The two use different primitives on purpose.
//!
//! * **API keys** are 256 bits from the OS CSPRNG. There is no guessing attack
//!   to slow down, so they are stored as a plain SHA-256 and compared in
//!   constant time. That keeps authentication to one indexed lookup, which
//!   matters on a path Sonarr hits in bursts.
//! * **Admin passwords** are human-chosen and therefore low-entropy, so they go
//!   through argon2id with its default OWASP-aligned parameters.

use anyhow::{Context, Result, bail};
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Prefix on every issued key, so one can be recognised in a log or a config file.
const KEY_PREFIX: &str = "ams_";

/// Bytes of entropy per key.
const KEY_BYTES: usize = 32;

/// How much of the key is stored in the clear for display.
const DISPLAY_PREFIX_LEN: usize = KEY_PREFIX.len() + 8;

/// Whether a credential is shaped like a key issued here: what a relay goes
/// by to tell this server's key, which it stands in for, from a client's own
/// token for the service relayed, which it hands on.
pub fn is_issued_here(credential: &str) -> bool {
    credential.starts_with(KEY_PREFIX)
}

/// A freshly minted key. `plaintext` is shown to the user exactly once.
pub struct GeneratedKey {
    pub plaintext: String,
    pub prefix: String,
    pub hash: String,
}

/// Mint a new API key.
pub fn generate_api_key() -> Result<GeneratedKey> {
    let mut bytes = [0u8; KEY_BYTES];
    getrandom::fill(&mut bytes).context("failed to read from the system CSPRNG")?;

    let plaintext = format!("{KEY_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    let prefix = plaintext.chars().take(DISPLAY_PREFIX_LEN).collect();
    let hash = hash_api_key(&plaintext);

    Ok(GeneratedKey {
        plaintext,
        prefix,
        hash,
    })
}

/// Hex SHA-256 of a key, which is what the database stores and indexes.
pub fn hash_api_key(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    hex(&digest)
}

/// Constant-time comparison of two key hashes.
///
/// The lookup is by hash so a mismatch is already unlikely, but comparing with
/// `==` would still leak where two hashes diverge.
pub fn api_key_hash_matches(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// A random opaque token for admin sessions, plus the hash stored for it.
pub fn generate_session_token() -> Result<(String, String)> {
    let mut bytes = [0u8; KEY_BYTES];
    getrandom::fill(&mut bytes).context("failed to read from the system CSPRNG")?;

    let token = URL_SAFE_NO_PAD.encode(bytes);
    let hash = hash_api_key(&token);

    Ok((token, hash))
}

/// The letters an invitation code is written in: Crockford's base 32, which
/// leaves out I, L, O and U, so a code read aloud or copied by hand survives.
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Characters in an invitation code, not counting the dashes: 80 bits.
const CODE_LEN: usize = 16;

/// A new invitation code, `K7QM-2XRP-9DHT-4WCN`: four groups a person can
/// type, the first of which is shown in lists to tell invitations apart.
///
/// Not a key: a key is pasted into a program, a code may be dictated over the
/// phone. Eighty bits is still far beyond guessing through a rate limit.
pub fn generate_invitation_code() -> Result<GeneratedKey> {
    let mut bytes = [0u8; CODE_LEN];
    getrandom::fill(&mut bytes).context("failed to read from the system CSPRNG")?;

    // 256 is a multiple of 32, so the low five bits of a byte are uniform.
    let letters: Vec<u8> = bytes
        .iter()
        .map(|b| CODE_ALPHABET[usize::from(b & 31)])
        .collect();

    let plaintext = letters
        .chunks(4)
        .map(|group| String::from_utf8_lossy(group).into_owned())
        .collect::<Vec<_>>()
        .join("-");
    let prefix = plaintext[..4].to_string();
    let hash = hash_api_key(&String::from_utf8_lossy(&letters));

    Ok(GeneratedKey {
        plaintext,
        prefix,
        hash,
    })
}

/// The hash an invitation code is kept under, however it was typed: case,
/// spaces and dashes aside, and the letters people take for digits read as
/// those digits. Nothing when it cannot be a code at all.
pub fn hash_invitation_code(typed: &str) -> Option<String> {
    let normal: String = typed
        .chars()
        // Spaces of every kind, and every dash a keyboard or a mail client
        // might put between the groups: the interface strips the same.
        .filter(|c| !(c.is_whitespace() || matches!(c, '-' | '\u{2010}'..='\u{2015}' | '\u{2212}')))
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect();

    let valid = normal.len() == CODE_LEN && normal.bytes().all(|b| CODE_ALPHABET.contains(&b));
    valid.then(|| hash_api_key(&normal))
}

/// Hash an admin password with argon2id, returning a PHC string.
pub fn hash_password(password: &str) -> Result<String> {
    if password.chars().count() < 12 {
        bail!("password must be at least 12 characters");
    }

    // argon2 0.6 draws its own salt from the system CSPRNG.
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("failed to hash password: {e}"))
}

/// Verify a password against a stored PHC string.
///
/// A malformed stored hash verifies as `false` rather than erroring: a corrupt
/// row should lock the account, not return a 500 that reveals it exists.
pub fn verify_password(password: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(e) => {
            tracing::error!(error = %e, "stored password hash is malformed");
            false
        }
    }
}

/// Why a password could not be hashed or checked just now.
#[derive(Debug, thiserror::Error)]
pub enum HashError {
    /// The password itself was refused: too short.
    #[error("{0}")]
    Rejected(String),
    /// Every hashing slot stayed taken for [`HASH_WAIT`]: a burst of sign-ins
    /// is being worked through. Nothing was checked; the caller may try again.
    #[error("too many passwords are being checked at once; try again in a moment")]
    Busy,
    #[error("the hashing task failed: {0}")]
    Failed(String),
}

impl From<HashError> for crate::error::AppError {
    fn from(e: HashError) -> Self {
        match e {
            HashError::Rejected(why) => Self::BadRequest(why),
            HashError::Busy => Self::RateLimited,
            HashError::Failed(why) => Self::Internal(anyhow::anyhow!(why)),
        }
    }
}

/// How long a password waits for a hashing slot before the request is told
/// to come back: long enough for a family signing in at once, short enough
/// that a burst cannot queue up a minute of work.
const HASH_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Hashes in flight at once: as many as the machine has cores, from two to
/// eight. Each one holds 19 MiB for its duration, so this is what bounds the
/// memory a burst of sign-ins can take — a few hundred at once used to be a
/// few gigabytes, and an OOM kill of everything this server answers for.
static HASHING: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(hashing_slots()));

fn hashing_slots() -> usize {
    std::thread::available_parallelism()
        .map_or(2, std::num::NonZeroUsize::get)
        .clamp(2, 8)
}

/// A slot to hash in, or [`HashError::Busy`] after [`HASH_WAIT`].
async fn hashing_slot() -> Result<tokio::sync::SemaphorePermit<'static>, HashError> {
    match tokio::time::timeout(HASH_WAIT, HASHING.acquire()).await {
        Ok(Ok(permit)) => Ok(permit),
        // The semaphore is never closed; a closed one would be a bug.
        Ok(Err(e)) => Err(HashError::Failed(e.to_string())),
        Err(_) => Err(HashError::Busy),
    }
}

/// [`hash_password`], off the async executor.
///
/// argon2's whole point is that it is slow and memory-hungry — around 20 ms of
/// CPU and 19 MiB per call at the defaults. Run on a tokio worker that is the
/// worker not running anything else for 20 ms, and a handful of sign-in attempts
/// stalls every surface this server has, Sonarr's included. Bounded by the
/// same slots as [`verify_password_async`].
pub async fn hash_password_async(password: String) -> Result<String, HashError> {
    let _slot = hashing_slot().await?;
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|e| HashError::Failed(e.to_string()))?
        .map_err(|e| HashError::Rejected(e.to_string()))
}

/// [`verify_password`], off the async executor and within the hashing slots.
/// See [`hash_password_async`]. `Ok(false)` for a wrong password; an error
/// only when nothing was checked.
pub async fn verify_password_async(password: String, phc: String) -> Result<bool, HashError> {
    let _slot = hashing_slot().await?;
    Ok(
        tokio::task::spawn_blocking(move || verify_password(&password, &phc))
            .await
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "password verification task failed");
                false
            }),
    )
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut acc, b| {
            let _ = write!(acc, "{b:02x}");
            acc
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invitation_code_is_four_groups_and_survives_being_retyped() {
        let code = generate_invitation_code().unwrap();

        assert_eq!(code.plaintext.len(), CODE_LEN + 3);
        assert_eq!(code.plaintext.matches('-').count(), 3);
        assert!(code.plaintext.starts_with(&code.prefix));
        assert_eq!(
            hash_invitation_code(&code.plaintext).as_deref(),
            Some(code.hash.as_str())
        );

        // Lower case, no dashes, spaces: the same code.
        let retyped = code.plaintext.to_lowercase().replace('-', " ");
        assert_eq!(
            hash_invitation_code(&retyped).as_deref(),
            Some(code.hash.as_str())
        );

        assert_ne!(generate_invitation_code().unwrap().hash, code.hash);
    }

    #[test]
    fn letters_taken_for_digits_are_read_as_digits() {
        assert_eq!(
            hash_invitation_code("O0IL-1111-0000-2222"),
            hash_invitation_code("0011-1111-0000-2222")
        );
        // En dashes and a non-breaking space, as a mail client may leave them.
        assert_eq!(
            hash_invitation_code("0011\u{2013}1111\u{00a0}0000\u{2014}2222"),
            hash_invitation_code("0011-1111-0000-2222")
        );
        assert_eq!(hash_invitation_code("too short"), None);
        assert_eq!(hash_invitation_code("UUUU-UUUU-UUUU-UUUU"), None);
        assert_eq!(hash_invitation_code(""), None);
    }

    #[test]
    fn generated_keys_are_prefixed_and_unique() {
        let a = generate_api_key().unwrap();
        let b = generate_api_key().unwrap();

        assert!(a.plaintext.starts_with(KEY_PREFIX));
        assert_ne!(a.plaintext, b.plaintext);
        assert_eq!(a.prefix.len(), DISPLAY_PREFIX_LEN);
        assert!(a.plaintext.starts_with(&a.prefix));
    }

    #[test]
    fn a_key_hashes_to_its_stored_value() {
        let key = generate_api_key().unwrap();
        assert_eq!(hash_api_key(&key.plaintext), key.hash);
        assert!(api_key_hash_matches(
            &hash_api_key(&key.plaintext),
            &key.hash
        ));
    }

    #[test]
    fn a_different_key_does_not_match() {
        let a = generate_api_key().unwrap();
        let b = generate_api_key().unwrap();
        assert!(!api_key_hash_matches(&a.hash, &b.hash));
    }

    #[test]
    fn hashes_are_64_hex_characters() {
        let h = hash_api_key("anything");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// A password for a test, drawn at random: the tests need none in
    /// particular, only one long enough and another unlike it, and a password
    /// written in the code reads as a secret kept there to whoever scans it.
    fn fresh_password() -> String {
        generate_session_token().unwrap().0
    }

    /// Too short by one for [`hash_password`].
    fn short_password() -> String {
        fresh_password().chars().take(11).collect()
    }

    #[test]
    fn passwords_round_trip_through_argon2() {
        let password = fresh_password();
        let phc = hash_password(&password).unwrap();
        assert!(phc.starts_with("$argon2id$"));
        assert!(verify_password(&password, &phc));
        assert!(!verify_password(&fresh_password(), &phc));
    }

    #[test]
    fn short_passwords_are_rejected() {
        assert!(hash_password(&short_password()).is_err());
    }

    #[test]
    fn a_corrupt_hash_fails_closed() {
        assert!(!verify_password(&fresh_password(), "not-a-phc-string"));
    }

    /// However many ask at once, no more hash than there are slots; the
    /// others wait their turn, and each is answered.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn hashing_is_bounded_and_everyone_is_answered() {
        let right = fresh_password();
        let wrong = fresh_password();
        let phc = hash_password(&right).unwrap();
        assert!((2..=8).contains(&hashing_slots()));

        let checks: Vec<_> = (0..12)
            .map(|i| {
                let phc = phc.clone();
                let password = if i % 2 == 0 {
                    right.clone()
                } else {
                    wrong.clone()
                };
                tokio::spawn(verify_password_async(password, phc))
            })
            .collect();
        let mut answers = Vec::new();
        for check in checks {
            answers.push(check.await.unwrap().unwrap());
        }
        assert_eq!(answers.iter().filter(|ok| **ok).count(), 6);

        assert!(matches!(
            hash_password_async(short_password()).await,
            Err(HashError::Rejected(_))
        ));
    }

    #[test]
    fn session_tokens_are_distinct_from_their_hashes() {
        let (token, hash) = generate_session_token().unwrap();
        assert_ne!(token, hash);
        assert_eq!(hash_api_key(&token), hash);
    }
}
