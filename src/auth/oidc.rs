//! Signing in through an OpenID Connect provider: Authentik, Keycloak,
//! Authelia, Kanidm, Google — anything that publishes a discovery document.
//!
//! The authorization code flow, with PKCE, a nonce and a state, all three
//! checked; the ID token's signature, issuer, audience and expiry are checked
//! by the `openidconnect` crate against the keys the provider publishes — and
//! only asymmetric signatures are taken, so the client secret can never sign a
//! token — and the access token against the ID token's `at_hash`. Nothing that
//! comes back through the browser is trusted but the code, and the code is
//! worth nothing without the verifier this server sealed into a cookie.
//!
//! What this module does not decide is who the person is here: it hands back
//! what the provider vouched for, and `api::native::oidc` finds, ties or
//! opens the account.

use std::{
    sync::LazyLock,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use openidconnect::{
    AccessTokenHash, AdditionalClaims, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    EmptyExtraTokenFields, EndpointMaybeSet, EndpointNotSet, EndpointSet, HttpRequest,
    HttpResponse, IdTokenFields, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, StandardErrorResponse, StandardTokenResponse,
    TokenResponse, UserInfoClaims,
    core::{
        CoreAuthDisplay, CoreAuthPrompt, CoreAuthenticationFlow, CoreErrorResponseType,
        CoreGenderClaim, CoreJsonWebKey, CoreJweContentEncryptionAlgorithm,
        CoreJwsSigningAlgorithm, CoreProviderMetadata, CoreRevocableToken,
        CoreRevocationErrorResponse, CoreTokenIntrospectionResponse, CoreTokenType,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::db::repo::user::Role;

// ─── what the provider is asked, and how ─────────────────────────────────────

/// Everything this server needs to talk to one provider, as the settings and
/// the environment say it.
#[derive(Clone, PartialEq, Eq)]
pub struct Provider {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    /// Where the provider sends the browser back: this server's public
    /// address and the callback's path.
    pub redirect_uri: String,
}

/// Written by hand so that no log line, however it came to print a provider,
/// can carry the secret.
impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| "…"))
            .field("scopes", &self.scopes)
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

/// How claims become a role here. With no claim named, nobody's role is
/// decided by the provider.
#[derive(Clone, Debug, Default)]
pub struct RoleMapping {
    /// `groups`, a path into nested claims — `realm_access.roles` — or a
    /// namespaced claim, `https://example.org/roles`.
    pub claim: Option<String>,
    pub admin: Vec<String>,
    pub editor: Vec<String>,
}

impl RoleMapping {
    /// The role these claims map to: an administrator's if a value names one,
    /// an editor's if a value names one, a member's if the claim is there and
    /// names neither. Nothing when no claim is named or the claims do not
    /// carry it at all: a provider that did not say is not a provider that
    /// said "member", and must not demote anyone.
    pub fn role(&self, claims: &serde_json::Map<String, serde_json::Value>) -> Option<Role> {
        let path = self.claim.as_deref()?.trim();
        if path.is_empty() {
            return None;
        }

        let values = values(claim_at(claims, path)?);
        let named = |wanted: &[String]| {
            values
                .iter()
                .any(|value| wanted.iter().any(|w| w.eq_ignore_ascii_case(value)))
        };

        Some(if named(&self.admin) {
            Role::Admin
        } else if named(&self.editor) {
            Role::Editor
        } else {
            Role::Member
        })
    }
}

/// The claim a path names: the whole path as one key first, for namespaced
/// claims with dots in them, then the path walked through nested objects.
fn claim_at<'a>(
    claims: &'a serde_json::Map<String, serde_json::Value>,
    path: &str,
) -> Option<&'a serde_json::Value> {
    if let Some(found) = claims.get(path) {
        return Some(found);
    }

    let mut parts = path.split('.');
    let mut here = claims.get(parts.next()?)?;
    for part in parts {
        here = here.get(part)?;
    }
    Some(here)
}

/// A claim's values: a string, an array of them, or a string of several
/// separated by spaces or commas, as providers variously send groups.
fn values(claim: &serde_json::Value) -> Vec<String> {
    match claim {
        serde_json::Value::String(s) => s
            .split([' ', ','])
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .collect(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Every claim the standard ones leave, so the role claim can be read from
/// whatever the provider calls it.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Extra {
    #[serde(flatten)]
    pub rest: serde_json::Map<String, serde_json::Value>,
}

impl AdditionalClaims for Extra {}

type Tokens = StandardTokenResponse<
    IdTokenFields<
        Extra,
        EmptyExtraTokenFields,
        CoreGenderClaim,
        CoreJweContentEncryptionAlgorithm,
        CoreJwsSigningAlgorithm,
    >,
    CoreTokenType,
>;

/// A client after discovery: an authorization endpoint for certain, a token
/// and a user-info endpoint if the provider published them.
type Client = openidconnect::Client<
    Extra,
    CoreAuthDisplay,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJsonWebKey,
    CoreAuthPrompt,
    StandardErrorResponse<CoreErrorResponseType>,
    Tokens,
    CoreTokenIntrospectionResponse,
    CoreRevocableToken,
    CoreRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// The signatures an ID token may carry. Asymmetric only: an HMAC one is
/// signed with the client secret, which this server holds too, so a token
/// signed that way proves nothing about who made it.
const SIGNATURES: [CoreJwsSigningAlgorithm; 10] = [
    CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
    CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha384,
    CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha512,
    CoreJwsSigningAlgorithm::RsaSsaPssSha256,
    CoreJwsSigningAlgorithm::RsaSsaPssSha384,
    CoreJwsSigningAlgorithm::RsaSsaPssSha512,
    CoreJwsSigningAlgorithm::EcdsaP256Sha256,
    CoreJwsSigningAlgorithm::EcdsaP384Sha384,
    CoreJwsSigningAlgorithm::EcdsaP521Sha512,
    CoreJwsSigningAlgorithm::EdDsa,
];

// ─── HTTP ────────────────────────────────────────────────────────────────────

/// The largest answer a provider is read to: a discovery document, a key set
/// and a token response are a few kilobytes each.
const MAX_RESPONSE: usize = 1 << 20;

/// Its own client, which never follows a redirect: a redirect is a URL the
/// provider's answer chose, and the crate's advice is not to go there.
static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(concat!("arr-metadata-server/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("the OIDC HTTP client builds")
});

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("the provider could not be reached: {0}")]
    Request(#[from] reqwest::Error),
    #[error("the provider's answer could not be read: {0}")]
    Response(#[from] openidconnect::http::Error),
    #[error("the provider's answer is larger than {MAX_RESPONSE} bytes")]
    TooLarge,
}

/// One request to the provider, as the crate asks for it.
async fn send(request: HttpRequest) -> Result<HttpResponse, HttpError> {
    let (parts, body) = request.into_parts();

    let mut response = HTTP
        .request(parts.method, parts.uri.to_string())
        .headers(parts.headers)
        .body(body)
        .send()
        .await?;

    let mut answer = openidconnect::http::Response::builder().status(response.status());
    for (name, value) in response.headers() {
        answer = answer.header(name, value);
    }

    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > MAX_RESPONSE {
            return Err(HttpError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }

    Ok(answer.body(bytes)?)
}

/// Whether plain HTTP is acceptable for this address: this machine, or an
/// address that only a home or office network routes — never the internet,
/// where anyone on the way could swap the provider's keys for their own.
pub fn http_allowed(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Some(url::Host::Ipv6(ip)) => {
            ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
        }
        Some(url::Host::Domain(name)) => {
            let name = name.to_ascii_lowercase();
            name == "localhost"
                || [".localhost", ".local", ".lan", ".internal", ".home.arpa"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
                || !name.contains('.')
        }
        None => false,
    }
}

/// Every endpoint the provider published is as safe as its issuer: over
/// HTTPS, unless plain HTTP is acceptable for that address.
fn check_endpoints(metadata: &CoreProviderMetadata) -> Result<()> {
    let mut urls = vec![
        metadata.authorization_endpoint().url().clone(),
        metadata.jwks_uri().url().clone(),
    ];
    if let Some(token) = metadata.token_endpoint() {
        urls.push(token.url().clone());
    }
    if let Some(info) = metadata.userinfo_endpoint() {
        urls.push(info.url().clone());
    }

    for url in urls {
        if url.scheme() != "https" && !http_allowed(&url) {
            bail!("the provider publishes {url} over plain HTTP; only a local address may be");
        }
    }
    Ok(())
}

// ─── discovery, kept for an hour ─────────────────────────────────────────────

/// How long a provider's discovery document and keys are trusted before they
/// are read again. A key the provider rotated in the meantime is found by
/// reading them again at once when a token fails to verify.
const DISCOVERY_TTL: Duration = Duration::from_secs(3600);

/// How long a failed discovery is remembered: a provider that is down is not
/// asked again by every sign-in behind the first.
const FAILURE_TTL: Duration = Duration::from_secs(30);

enum Discovered {
    Found {
        provider: Provider,
        metadata: Box<CoreProviderMetadata>,
        at: Instant,
    },
    Failed {
        provider: Provider,
        why: String,
        at: Instant,
    },
}

static DISCOVERED: LazyLock<tokio::sync::Mutex<Option<Discovered>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));

/// The provider's metadata, from the cache when it is fresh and for the same
/// configuration, from the provider otherwise.
async fn metadata(provider: &Provider, fresh: bool) -> Result<CoreProviderMetadata> {
    let mut cached = DISCOVERED.lock().await;

    match cached.as_ref() {
        Some(Discovered::Found {
            provider: same,
            metadata,
            at,
        }) if !fresh && same == provider && at.elapsed() < DISCOVERY_TTL => {
            return Ok((**metadata).clone());
        }
        Some(Discovered::Failed {
            provider: same,
            why,
            at,
        }) if same == provider && at.elapsed() < FAILURE_TTL => {
            bail!("{why}");
        }
        _ => {}
    }

    let found = async {
        let issuer = IssuerUrl::new(provider.issuer.clone()).context("the issuer is not a URL")?;
        let metadata = CoreProviderMetadata::discover_async(issuer, &send)
            .await
            .map_err(|e| anyhow!("discovery failed: {e}"))?;
        check_endpoints(&metadata)?;
        anyhow::Ok(metadata)
    }
    .await;

    *cached = Some(match &found {
        Ok(metadata) => Discovered::Found {
            provider: provider.clone(),
            metadata: Box::new(metadata.clone()),
            at: Instant::now(),
        },
        Err(e) => Discovered::Failed {
            provider: provider.clone(),
            why: format!("{e:#}"),
            at: Instant::now(),
        },
    });

    found
}

/// Forget what was discovered, when the configuration changes.
pub async fn forget() {
    *DISCOVERED.lock().await = None;
}

fn client(provider: &Provider, metadata: CoreProviderMetadata) -> Result<Client> {
    let redirect = RedirectUrl::new(provider.redirect_uri.clone())
        .context("the return address is not a URL")?;

    Ok(Client::from_provider_metadata(
        metadata,
        ClientId::new(provider.client_id.clone()),
        provider.client_secret.clone().map(ClientSecret::new),
    )
    .set_redirect_uri(redirect))
}

// ─── the flow, sealed into the browser ───────────────────────────────────────

/// How long a person has at the provider before coming back.
pub const FLOW_TTL: Duration = Duration::from_secs(600);

/// What a sign-in carries to the provider and back: kept in the browser, in a
/// cookie sealed with a key only this process holds — nothing is stored on
/// the server, so nobody can fill a table and shut the door for everyone.
#[derive(Clone, Serialize, Deserialize)]
pub struct Flow {
    state: String,
    nonce: String,
    verifier: String,
    /// Where the person was going.
    pub next: String,
    /// Seconds since the epoch, when it began.
    at: i64,
    /// The account to tie the provider's person to: set when a signed-in
    /// person asked for it from their account page.
    pub link: Option<String>,
}

/// The cookie a flow travels in, named after its state so that two sign-ins
/// in two tabs each keep theirs. `__Host-` when the site is served over
/// HTTPS: a cookie of that name cannot be planted from a neighbouring domain.
pub fn cookie_name(state: &str, secure: bool) -> String {
    let digest = Sha256::digest(state.as_bytes());
    let short: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    if secure {
        format!("__Host-ams_oidc_{short}")
    } else {
        format!("ams_oidc_{short}")
    }
}

/// Where to send the browser, and the flow to seal into its cookie.
pub struct Started {
    pub url: String,
    pub state: String,
    pub flow: Flow,
}

/// Begin: a state, a nonce and a PKCE pair, and the address at the provider.
pub async fn start(provider: &Provider, next: String, link: Option<String>) -> Result<Started> {
    let client = client(provider, metadata(provider, false).await?)?;

    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let mut request = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .set_pkce_challenge(challenge);
    for scope in provider.scopes.iter().filter(|s| s.as_str() != "openid") {
        request = request.add_scope(Scope::new(scope.clone()));
    }
    let (url, csrf, nonce) = request.url();

    Ok(Started {
        url: url.to_string(),
        state: csrf.secret().clone(),
        flow: Flow {
            state: csrf.secret().clone(),
            nonce: nonce.secret().clone(),
            verifier: verifier.secret().clone(),
            next,
            at: chrono::Utc::now().timestamp(),
            link,
        },
    })
}

/// Who the provider says came back.
#[derive(Clone, Debug)]
pub struct Vouched {
    pub issuer: String,
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub preferred_username: Option<String>,
    pub name: Option<String>,
    /// What the role mapping made of the claims — nothing when there is no
    /// mapping, or the claims did not carry the role claim at all.
    pub role: Option<Role>,
    /// Where the person was going.
    pub next: String,
    /// The account a signed-in person asked to tie to this one.
    pub link: Option<String>,
}

/// Why a return from the provider was not accepted, in words the sign-in
/// page can choose between.
#[derive(Debug, thiserror::Error)]
pub enum Refusal {
    #[error("this sign-in was not started here, or took too long")]
    UnknownFlow,
    #[error("the provider answered: {0}")]
    Provider(String),
    #[error("{0}")]
    Invalid(String),
}

/// Come back: the flow the cookie carried, its state compared with the one
/// returned and its age checked; the code exchanged with the verifier; the ID
/// token verified; the access token checked against it; the user-info
/// endpoint asked for what the ID token left out.
pub async fn finish(
    provider: &Provider,
    mapping: &RoleMapping,
    flow: Flow,
    state: &str,
    code: &str,
) -> Result<Vouched, Refusal> {
    let age = chrono::Utc::now().timestamp() - flow.at;
    if !(0..=FLOW_TTL.as_secs() as i64).contains(&age)
        || !bool::from(flow.state.as_bytes().ct_eq(state.as_bytes()))
    {
        return Err(Refusal::UnknownFlow);
    }

    let invalid = |what: &str, e: &dyn std::fmt::Display| Refusal::Invalid(format!("{what}: {e}"));

    let discovered = metadata(provider, false)
        .await
        .map_err(|e| invalid("discovery", &e))?;
    let client = client(provider, discovered).map_err(|e| invalid("configuration", &e))?;

    let tokens = client
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .map_err(|e| invalid("the provider has no token endpoint", &e))?
        .set_pkce_verifier(PkceCodeVerifier::new(flow.verifier))
        .request_async(&send)
        .await
        .map_err(|e| Refusal::Provider(e.to_string()))?;

    let id_token = tokens
        .id_token()
        .ok_or_else(|| Refusal::Invalid("the provider sent no ID token".into()))?;
    let nonce = Nonce::new(flow.nonce);

    // A key the provider rotated since discovery: read its keys again, once.
    let (client, claims) = match id_token.claims(&verifier_of(&client), &nonce) {
        Ok(claims) => (client.clone(), claims.clone()),
        Err(first) => {
            let rediscovered = metadata(provider, true)
                .await
                .map_err(|e| invalid("discovery", &e))?;
            let again =
                self::client(provider, rediscovered).map_err(|e| invalid("configuration", &e))?;
            let claims = id_token
                .claims(&verifier_of(&again), &nonce)
                .map_err(|_| invalid("the ID token did not verify", &first))?
                .clone();
            (again, claims)
        }
    };

    if let Some(expected) = claims.access_token_hash() {
        let verifier = verifier_of(&client);
        let actual = AccessTokenHash::from_token(
            tokens.access_token(),
            id_token
                .signing_alg()
                .map_err(|e| invalid("the ID token's algorithm", &e))?,
            id_token
                .signing_key(&verifier)
                .map_err(|e| invalid("the ID token's key", &e))?,
        )
        .map_err(|e| invalid("the access token's hash", &e))?;
        if actual != *expected {
            return Err(Refusal::Invalid(
                "the access token does not belong to this ID token".into(),
            ));
        }
    }

    let mut vouched = Vouched {
        issuer: claims.issuer().as_str().to_string(),
        subject: claims.subject().as_str().to_string(),
        email: claims.email().map(|e| e.as_str().to_string()),
        email_verified: claims.email_verified().unwrap_or(false),
        preferred_username: claims.preferred_username().map(|u| u.as_str().to_string()),
        name: claims
            .name()
            .and_then(|n| n.get(None))
            .map(|n| n.as_str().to_string()),
        role: mapping.role(&claims.additional_claims().rest),
        next: flow.next,
        link: flow.link,
    };

    // What the ID token did not carry — providers differ on whether the
    // e-mail and the groups ride in it or only in user-info. A user-info that
    // fails leaves the role unsaid, never "member".
    let wants_role = mapping.claim.is_some() && vouched.role.is_none();
    if (vouched.email.is_none() || wants_role)
        && let Ok(request) = client.user_info(
            tokens.access_token().clone(),
            Some(claims.subject().clone()),
        )
    {
        match request
            .request_async::<Extra, _, CoreGenderClaim>(&send)
            .await
        {
            Ok(info) => merge(&mut vouched, &info, mapping),
            Err(e) => tracing::info!(error = %e, "the provider's user-info could not be read"),
        }
    }

    Ok(vouched)
}

/// The ID token's verifier, taking only the asymmetric signatures.
fn verifier_of(client: &Client) -> openidconnect::IdTokenVerifier<'_, CoreJsonWebKey> {
    client.id_token_verifier().set_allowed_algs(SIGNATURES)
}

fn merge(
    vouched: &mut Vouched,
    info: &UserInfoClaims<Extra, CoreGenderClaim>,
    mapping: &RoleMapping,
) {
    if vouched.email.is_none() {
        vouched.email = info.email().map(|e| e.as_str().to_string());
        vouched.email_verified = info.email_verified().unwrap_or(false);
    }
    if vouched.preferred_username.is_none() {
        vouched.preferred_username = info.preferred_username().map(|u| u.as_str().to_string());
    }
    if vouched.name.is_none() {
        vouched.name = info
            .name()
            .and_then(|n| n.get(None))
            .map(|n| n.as_str().to_string());
    }
    if vouched.role.is_none() {
        vouched.role = mapping.role(&info.additional_claims().rest);
    }
}

// ─── the test button ─────────────────────────────────────────────────────────

/// What discovery found, for the administrator checking the configuration.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: Option<String>,
    pub userinfo_endpoint: Option<String>,
    /// Keys in the provider's key set.
    pub keys: usize,
    pub signing_algorithms: Vec<String>,
    pub scopes: Vec<String>,
}

/// Read the provider's discovery document afresh.
pub async fn discover(provider: &Provider) -> Result<Discovery> {
    let metadata = metadata(provider, true).await?;

    Ok(Discovery {
        issuer: metadata.issuer().as_str().to_string(),
        authorization_endpoint: metadata.authorization_endpoint().as_str().to_string(),
        token_endpoint: metadata.token_endpoint().map(|u| u.as_str().to_string()),
        userinfo_endpoint: metadata.userinfo_endpoint().map(|u| u.as_str().to_string()),
        keys: metadata.jwks().keys().len(),
        signing_algorithms: metadata
            .id_token_signing_alg_values_supported()
            .iter()
            .map(|alg| {
                serde_json::to_value(alg)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default()
            })
            .collect(),
        scopes: metadata
            .scopes_supported()
            .map(|s| s.iter().map(|s| s.as_str().to_string()).collect())
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(json: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        json.as_object().unwrap().clone()
    }

    fn mapping(claim: &str) -> RoleMapping {
        RoleMapping {
            claim: Some(claim.into()),
            admin: vec!["cine-admins".into()],
            editor: vec!["cine-editors".into()],
        }
    }

    #[test]
    fn groups_map_to_the_highest_role_they_name() {
        let m = mapping("groups");
        assert_eq!(
            m.role(&claims(
                serde_json::json!({ "groups": ["family", "cine-editors"] })
            )),
            Some(Role::Editor)
        );
        assert_eq!(
            m.role(&claims(
                serde_json::json!({ "groups": ["cine-editors", "CINE-ADMINS"] })
            )),
            Some(Role::Admin)
        );
        assert_eq!(
            m.role(&claims(serde_json::json!({ "groups": ["family"] }))),
            Some(Role::Member)
        );
        // An empty list is the provider saying "no groups".
        assert_eq!(
            m.role(&claims(serde_json::json!({ "groups": [] }))),
            Some(Role::Member)
        );
    }

    #[test]
    fn a_claim_that_is_not_there_decides_nothing() {
        // Not "member": a provider that put its groups only in user-info, and
        // a user-info that failed, must not demote anyone.
        assert_eq!(mapping("groups").role(&claims(serde_json::json!({}))), None);
    }

    #[test]
    fn a_nested_a_namespaced_claim_and_a_spaced_string_are_read_too() {
        let keycloak = claims(serde_json::json!({ "realm_access": { "roles": ["cine-admins"] } }));
        assert_eq!(
            mapping("realm_access.roles").role(&keycloak),
            Some(Role::Admin)
        );

        let auth0 = claims(serde_json::json!({ "https://example.org/roles": ["cine-editors"] }));
        assert_eq!(
            mapping("https://example.org/roles").role(&auth0),
            Some(Role::Editor)
        );

        let spaced = claims(serde_json::json!({ "roles": "viewer cine-editors" }));
        assert_eq!(mapping("roles").role(&spaced), Some(Role::Editor));
    }

    #[test]
    fn without_a_claim_the_provider_decides_no_role() {
        let none = RoleMapping::default();
        assert_eq!(
            none.role(&claims(serde_json::json!({ "groups": ["cine-admins"] }))),
            None
        );
    }

    #[test]
    fn plain_http_only_for_a_local_address() {
        let ok = |u: &str| http_allowed(&url::Url::parse(u).unwrap());
        assert!(ok("http://127.0.0.1:9000/"));
        assert!(ok("http://192.168.1.10:9000/application/o/x/"));
        assert!(ok("http://authentik:9000/"));
        assert!(ok("http://auth.home.arpa/"));
        assert!(ok("http://[fd00::1]/"));
        assert!(!ok("http://auth.example.org/"));
        assert!(!ok("http://8.8.8.8/"));
    }

    #[test]
    fn two_sign_ins_keep_two_cookies() {
        assert_ne!(
            cookie_name("state-one", false),
            cookie_name("state-two", false)
        );
        assert!(cookie_name("s", true).starts_with("__Host-"));
        assert!(!cookie_name("s", false).starts_with("__Host-"));
    }

    #[test]
    fn a_flow_is_refused_for_another_state_or_when_too_old() {
        let provider = Provider {
            issuer: "https://idp.invalid".into(),
            client_id: "c".into(),
            client_secret: Some("secret".into()),
            scopes: vec![],
            redirect_uri: "https://meta.invalid/api/v1/auth/oidc/callback".into(),
        };
        let flow = |at: i64| Flow {
            state: "the-state".into(),
            nonce: "n".into(),
            verifier: "v".into(),
            next: "/".into(),
            at,
            link: None,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let now = chrono::Utc::now().timestamp();

        let forged = runtime.block_on(finish(
            &provider,
            &RoleMapping::default(),
            flow(now),
            "forged",
            "c",
        ));
        assert!(matches!(forged, Err(Refusal::UnknownFlow)));

        let stale = runtime.block_on(finish(
            &provider,
            &RoleMapping::default(),
            flow(now - 3600),
            "the-state",
            "c",
        ));
        assert!(matches!(stale, Err(Refusal::UnknownFlow)));

        // And nothing of the secret in a debug print.
        assert!(!format!("{provider:?}").contains("secret\""));
        assert!(!format!("{provider:?}").contains("\"secret"));
    }
}
