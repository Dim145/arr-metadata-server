//! The operator's own Sonarr and Radarr, reached through their API.
//!
//! A few calls each are all this needs, and they are the same calls in both:
//! the status a test reads (`/api/v3/system/status`), a work found by the id
//! this server hands it (`/api/v3/series?tvdbId=`, `/api/v3/movie?tmdbId=`),
//! the whole library the seed reads (`/api/v3/series`, `/api/v3/movie`), and a
//! command (`POST /api/v3/command`: `RefreshSeries` with `seriesIds`,
//! `RefreshMovie` with `movieIds`). The key travels in `X-Api-Key`, the header
//! both read it from.
//!
//! The address is an administrator's, typed on their own page, and is reached
//! wherever it is — unlike the address of a picture, which anybody who may
//! write chose. A private network, then; and this machine too: a Sonarr on
//! the same host as this server, installed beside it or sharing the host's
//! network, is the common setup, and refusing loopback would refuse it. What
//! no Sonarr or Radarr is ever at stays refused, whichever way it is written
//! or resolved: the link-local block a cloud parks its metadata service in,
//! the other clouds' metadata addresses, multicast, "this network". A redirect
//! is followed only on the host first asked, where the key was meant to go;
//! to another, it is an error, and the key goes nowhere else. These requests
//! go direct, whatever `HTTP_PROXY` says: a proxy would resolve the name out
//! of this resolver's sight, and a Sonarr on the network does not sit behind
//! the way out to the internet.

use std::{net::IpAddr, sync::OnceLock, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::{domain::MediaKind, state::AppState};

/// Sonarr or Radarr.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum App {
    Sonarr,
    Radarr,
}

impl App {
    pub const ALL: [Self; 2] = [Self::Sonarr, Self::Radarr];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sonarr => "sonarr",
            Self::Radarr => "radarr",
        }
    }

    /// As it calls itself in its status, and as a message names it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sonarr => "Sonarr",
            Self::Radarr => "Radarr",
        }
    }

    /// The one that holds works of this kind.
    pub const fn of(kind: MediaKind) -> Self {
        match kind {
            MediaKind::Series => Self::Sonarr,
            MediaKind::Movie => Self::Radarr,
        }
    }

    /// The setting its address is kept in.
    pub const fn url_setting(self) -> &'static str {
        match self {
            Self::Sonarr => "arr.sonarrUrl",
            Self::Radarr => "arr.radarrUrl",
        }
    }

    /// The setting its key is kept in, never read back.
    pub const fn key_setting(self) -> &'static str {
        match self {
            Self::Sonarr => "arr.sonarrKey",
            Self::Radarr => "arr.radarrKey",
        }
    }

    /// Its library, as its API names it.
    const fn library(self) -> &'static str {
        match self {
            Self::Sonarr => "series",
            Self::Radarr => "movie",
        }
    }

    /// The parameter its library is filtered by to find one work: the id
    /// this server serves the work under.
    const fn id_parameter(self) -> &'static str {
        match self {
            Self::Sonarr => "tvdbId",
            Self::Radarr => "tmdbId",
        }
    }

    /// The command that reads works again, and the list it takes their ids
    /// in.
    const fn refresh_command(self) -> (&'static str, &'static str) {
        match self {
            Self::Sonarr => ("RefreshSeries", "seriesIds"),
            Self::Radarr => ("RefreshMovie", "movieIds"),
        }
    }
}

/// The header both read their key from.
const KEY_HEADER: &str = "X-Api-Key";

/// How long one call is given: a status, a lookup, a command.
const SHORT: Duration = Duration::from_secs(15);

/// How long the whole library is given: a few thousand series, every season
/// and picture of each, from a Sonarr on a small machine.
const LONG: Duration = Duration::from_secs(120);

/// How many redirects one call follows, on its own host.
const MAX_REDIRECTS: usize = 5;

/// One of them, as this server reaches it: where, and with which key.
#[derive(Clone)]
pub struct Connection {
    pub app: App,
    base: url::Url,
    key: String,
}

/// Without the key: a connection printed in a log or a panic shows where it
/// goes, never what it carries.
impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("app", &self.app)
            .field("base", &self.base.as_str())
            .finish_non_exhaustive()
    }
}

/// What a status says of the one that answered.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// `Sonarr` or `Radarr`.
    pub app_name: String,
    pub version: String,
    /// The name it was given, when it was given one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_name: Option<String>,
}

/// A work as Sonarr or Radarr holds it: its own id, and the ids it knows the
/// work by elsewhere. Read leniently — of a series Sonarr sends every season
/// and picture, of which this needs none.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Held {
    pub id: i64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub year: Option<i32>,
    /// Sonarr's: TheTVDB's, or the synthetic id this server handed it.
    #[serde(default)]
    pub tvdb_id: Option<i64>,
    /// Radarr's; Sonarr's too, for a series TMDB knows.
    #[serde(default)]
    pub tmdb_id: Option<i64>,
    #[serde(default)]
    pub imdb_id: Option<String>,
}

impl Connection {
    /// As the settings have it: an address and a key both set. Nothing when
    /// either is missing, or when the address is not one to reach — said in
    /// the log, since only a hand on the table could have put it there.
    pub fn configured(state: &AppState, app: App) -> Option<Self> {
        let url = state.settings.resolve(app.url_setting(), None, None)?;
        let key = state.settings.resolve(app.key_setting(), None, None)?;
        match Self::new(app, &url, &key) {
            Ok(connection) => Some(connection),
            Err(why) => {
                tracing::warn!(app = app.as_str(), %why, "the address set for it cannot be used");
                None
            }
        }
    }

    pub fn new(app: App, url: &str, key: &str) -> Result<Self, String> {
        let base = address(url)?;
        let key = key.trim();
        if key.is_empty() {
            return Err("there is no API key".into());
        }
        Ok(Self {
            app,
            base,
            key: key.to_string(),
        })
    }

    /// A call of its API, under whatever URL base the address carries.
    fn endpoint(&self, path: &str) -> String {
        format!("{}/api/v3/{path}", self.base.as_str().trim_end_matches('/'))
    }

    /// Who answers at the address, and in which version. A Radarr at the
    /// address Sonarr was given answers the same call: said, rather than
    /// commands sent to the wrong one.
    pub async fn status(&self) -> Result<Status> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Answer {
            app_name: Option<String>,
            instance_name: Option<String>,
            version: Option<String>,
        }

        let answer: Answer = serde_json::from_value(self.get("system/status", &[], SHORT).await?)
            .context("its status could not be read")?;
        let app_name = answer
            .app_name
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| self.app.name().to_string());
        if !app_name.eq_ignore_ascii_case(self.app.name()) {
            bail!("this is {app_name}, not {}", self.app.name());
        }
        let version = answer
            .version
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                anyhow!(
                    "it answered without a version: is this {}'s address?",
                    self.app.name()
                )
            })?;

        Ok(Status {
            app_name,
            version,
            instance_name: answer
                .instance_name
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty()),
        })
    }

    /// The work it holds under the id this server serves it as — TheTVDB's
    /// or a synthetic one for a series, TMDB's for a film — if it holds it.
    pub async fn find(&self, client_id: i64) -> Result<Option<Held>> {
        let answer = self
            .get(
                self.app.library(),
                &[(self.app.id_parameter(), client_id.to_string())],
                SHORT,
            )
            .await?;
        Ok(entries(answer)?.into_iter().find(|held| held.id > 0))
    }

    /// Everything it holds.
    pub async fn library(&self) -> Result<Vec<Held>> {
        let answer = self.get(self.app.library(), &[], LONG).await?;
        entries(answer)
    }

    /// Ask it to read these works again, by its own ids: one command for
    /// them all. The command's id, when it says.
    pub async fn refresh(&self, ids: &[i64]) -> Result<Option<i64>> {
        let (name, list) = self.app.refresh_command();
        let mut body = serde_json::Map::new();
        body.insert("name".into(), Value::from(name));
        body.insert(list.into(), Value::from(ids.to_vec()));

        let response = client()?
            .post(self.endpoint("command"))
            .header(KEY_HEADER, &self.key)
            .json(&Value::Object(body))
            .timeout(SHORT)
            .send()
            .await
            .map_err(|e| anyhow!(crate::providers::describe_request_error(&e)))?;
        let answer = self.read(response, "command").await?;
        Ok(answer.get("id").and_then(Value::as_i64))
    }

    async fn get(&self, path: &str, query: &[(&str, String)], timeout: Duration) -> Result<Value> {
        let response = client()?
            .get(self.endpoint(path))
            .query(query)
            .header(KEY_HEADER, &self.key)
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| anyhow!(crate::providers::describe_request_error(&e)))?;
        self.read(response, path).await
    }

    /// An answer read as JSON, or what went wrong in words an operator can
    /// act on — never the address, which may carry a password, nor the key.
    async fn read(&self, response: reqwest::Response, path: &str) -> Result<Value> {
        let status = response.status();
        if status.is_success() {
            return crate::providers::read_json(response)
                .await
                .with_context(|| {
                    format!(
                        "its answer to {path} could not be read: is this {}'s address?",
                        self.app.name()
                    )
                });
        }
        match status {
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                bail!("{} refused the API key", self.app.name())
            }
            reqwest::StatusCode::NOT_FOUND => bail!(
                "nothing answers /api/v3/{path} there: check the address, and its URL base if {} \
                 has one",
                self.app.name()
            ),
            _ => {
                let said = crate::providers::error_text(response).await;
                if said.is_empty() {
                    bail!("{} answered {status}", self.app.name())
                }
                bail!("{} answered {status}: {said}", self.app.name())
            }
        }
    }
}

/// A list of works, each read on its own: one that cannot be read is left
/// out, not the list.
fn entries(answer: Value) -> Result<Vec<Held>> {
    let Value::Array(entries) = answer else {
        bail!("it answered something other than a list");
    };
    Ok(entries
        .into_iter()
        .filter_map(|entry| serde_json::from_value(entry).ok())
        .collect())
}

/// An address as typed, checked and made plain: http or https, a host, no
/// credentials, query or fragment in it — the key has a field of its own —
/// and not an address no Sonarr or Radarr is ever at. The trailing slash goes;
/// a URL base stays.
pub fn address(text: &str) -> Result<url::Url, String> {
    let text = text.trim();
    let url = url::Url::parse(text).map_err(|_| "this is not an address".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("the address is an http or https one".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("the address carries no name or password: the key has its own field".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("the address has no query: the key has its own field".into());
    }
    match url.host() {
        None => return Err("the address names no host".into()),
        Some(url::Host::Ipv4(v4)) if refused(IpAddr::V4(v4)) => return Err(NOWHERE.into()),
        Some(url::Host::Ipv6(v6)) if refused(IpAddr::V6(v6)) => return Err(NOWHERE.into()),
        Some(_) => {}
    }
    let plain = url.as_str().trim_end_matches('/').to_string();
    url::Url::parse(&plain).map_err(|_| "this is not an address".to_string())
}

/// Why an address is refused.
const NOWHERE: &str = "no Sonarr or Radarr is at a link-local, metadata, multicast or unspecified \
                       address";

/// Whether an address is one no Sonarr or Radarr is at: what the media client
/// refuses ([`crate::outbound::is_internal`]), but for this machine's
/// loopback, which is where a Sonarr installed beside this server answers.
fn refused(ip: IpAddr) -> bool {
    crate::outbound::is_internal(ip) && !loopback(ip)
}

/// Loopback, written either way: `127.0.0.0/8`, `::1`, and IPv4's written
/// as IPv6 (`::ffff:127.0.0.1`).
fn loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

/// Resolves a Sonarr's name as any other, refusing a name that resolves to
/// an address no Sonarr is at. Where the connection is made, so that a name
/// cannot resolve one way when it is checked and another when it is used.
#[derive(Clone, Copy, Debug, Default)]
struct Resolver;

impl reqwest::dns::Resolve for Resolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let found: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
                .collect();
            if found.iter().any(|a| refused(a.ip())) {
                return Err(format!("{host} resolves to an address {NOWHERE}").into());
            }
            Ok(Box::new(found.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// The client every call here is made with, built once.
fn client() -> Result<&'static reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let built = builder()
        .build()
        .context("could not build the client for Sonarr and Radarr")?;
    Ok(CLIENT.get_or_init(|| built))
}

fn builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(concat!(
            "arr-metadata-server/",
            env!("CARGO_PKG_VERSION"),
            " (+",
            env!("CARGO_PKG_REPOSITORY"),
            ")"
        ))
        .dns_resolver(std::sync::Arc::new(Resolver))
        .no_proxy()
        .connect_timeout(Duration::from_secs(5))
        .timeout(LONG)
        .redirect(reqwest::redirect::Policy::custom(|attempt| match redirect(
            attempt.previous().first(),
            attempt.url(),
            attempt.previous().len(),
        ) {
            Ok(()) => attempt.follow(),
            Err(why) => attempt.error(why),
        }))
}

/// Whether a redirect to `next` is followed, `hops` requests in: on the host
/// first asked, by http or https, five at most.
fn redirect(first: Option<&url::Url>, next: &url::Url, hops: usize) -> Result<(), &'static str> {
    if hops > MAX_REDIRECTS {
        return Err("too many redirects");
    }
    if !matches!(next.scheme(), "http" | "https") {
        return Err("redirected to something other than http or https");
    }
    match first {
        Some(first) if first.host_str() == next.host_str() => Ok(()),
        _ => Err("redirected to another host, where the API key is not sent"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    /// One request as the stand-in saw it.
    #[derive(Clone, Debug)]
    pub struct Seen {
        pub method: String,
        /// The path and its query.
        pub target: String,
        pub key: Option<String>,
        pub body: String,
    }

    /// What the stand-in answers a request with: its status line and body.
    pub type Answer = Arc<dyn Fn(&Seen) -> (u16, String) + Send + Sync>;

    /// A Sonarr or a Radarr small enough to read: it answers each request
    /// with what `answer` says, and keeps every request it was sent.
    pub async fn stand_in(answer: Answer) -> (String, Arc<Mutex<Vec<Seen>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        tokio::spawn(async move {
            while let Ok((mut connection, _)) = listener.accept().await {
                let answer = Arc::clone(&answer);
                let kept = Arc::clone(&kept);
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut connection).await else {
                        return;
                    };
                    kept.lock().push(request.clone());
                    let (status, body) = answer(&request);
                    let reply = format!(
                        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        if status < 400 { "OK" } else { "Error" },
                        body.len()
                    );
                    let _ = connection.write_all(reply.as_bytes()).await;
                    let _ = connection.shutdown().await;
                });
            }
        });
        (at, seen)
    }

    async fn read_request(connection: &mut tokio::net::TcpStream) -> Option<Seen> {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            let read = connection.read(&mut chunk).await.ok()?;
            if read == 0 {
                return None;
            }
            buffer.extend_from_slice(&chunk[..read]);
            if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
        };
        let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
        let mut lines = head.lines();
        let mut first = lines.next()?.split_whitespace();
        let method = first.next()?.to_string();
        let target = first.next()?.to_string();
        let mut key = None;
        let mut length = 0usize;
        for line in lines {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            match name.trim().to_ascii_lowercase().as_str() {
                "x-api-key" => key = Some(value.trim().to_string()),
                "content-length" => length = value.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
        while buffer.len() < head_end + length {
            let read = connection.read(&mut chunk).await.ok()?;
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
        }
        let body = String::from_utf8_lossy(&buffer[head_end..]).to_string();
        Some(Seen {
            method,
            target,
            key,
            body,
        })
    }

    #[test]
    fn an_address_is_checked_and_made_plain() {
        let plain = |text: &str| address(text).map(|url| url.to_string());
        assert_eq!(
            plain(" http://sonarr:8989/ ").unwrap(),
            "http://sonarr:8989/"
        );
        assert_eq!(
            plain("https://media.example.org/sonarr/").unwrap(),
            "https://media.example.org/sonarr"
        );
        // Beside this server, on its loopback, and on the home network.
        for fine in [
            "http://localhost:8989",
            "http://127.0.0.1:7878",
            "http://[::1]:8989",
            "http://[::ffff:127.0.0.1]:8989",
            "http://192.168.1.20:8989",
            "http://[fd7a:115c:a1e0::5]:7878",
        ] {
            assert!(address(fine).is_ok(), "{fine}");
        }
        for bad in [
            "sonarr:8989",
            "ftp://sonarr/",
            "http://user:secret@sonarr:8989",
            "http://sonarr:8989/?apikey=abc",
            "http://169.254.169.254/",
            "http://[fe80::1]:8989",
            "http://0.0.0.0:8989",
            "http://100.100.100.200/",
            "http://224.0.0.1/",
        ] {
            assert!(address(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_redirect_stays_on_the_host_first_asked() {
        let url = |text: &str| url::Url::parse(text).unwrap();
        let first = url("http://sonarr:8989/api/v3/system/status");
        assert!(redirect(Some(&first), &url("http://sonarr:8989/sonarr/api/v3/x"), 1).is_ok());
        // To https on the same host: the key goes where it was meant to.
        assert!(redirect(Some(&first), &url("https://sonarr/api/v3/x"), 1).is_ok());
        assert!(redirect(Some(&first), &url("http://elsewhere:8989/api/v3/x"), 1).is_err());
        assert!(redirect(Some(&first), &url("file:///etc/passwd"), 1).is_err());
        assert!(
            redirect(
                Some(&first),
                &url("http://sonarr:8989/x"),
                MAX_REDIRECTS + 1
            )
            .is_err()
        );
    }

    #[test]
    fn the_key_is_never_printed() {
        let connection = Connection::new(App::Sonarr, "http://sonarr:8989", "abcdef0123456789")
            .expect("a connection");
        let printed = format!("{connection:?}");
        assert!(printed.contains("sonarr:8989"), "{printed}");
        assert!(!printed.contains("abcdef0123456789"), "{printed}");
    }

    #[tokio::test]
    async fn a_status_says_who_answers_and_a_radarr_is_no_sonarr() {
        let (at, seen) = stand_in(Arc::new(|request: &Seen| {
            if request.key.as_deref() != Some("right-key-0001") {
                return (401, String::new());
            }
            (
                200,
                r#"{"appName":"Radarr","instanceName":"Films","version":"5.26.2.10099"}"#.into(),
            )
        }))
        .await;

        let radarr = Connection::new(App::Radarr, &at, "right-key-0001").unwrap();
        let status = radarr.status().await.expect("an answer");
        assert_eq!(status.version, "5.26.2.10099");
        assert_eq!(status.instance_name.as_deref(), Some("Films"));
        assert_eq!(seen.lock()[0].target, "/api/v3/system/status");

        let as_sonarr = Connection::new(App::Sonarr, &at, "right-key-0001").unwrap();
        let error = as_sonarr.status().await.expect_err("a Radarr");
        assert!(error.to_string().contains("this is Radarr"), "{error:#}");

        let wrong = Connection::new(App::Radarr, &at, "wrong-key-0002").unwrap();
        let error = wrong.status().await.expect_err("refused");
        assert!(
            error.to_string().contains("refused the API key"),
            "{error:#}"
        );
        assert!(!format!("{error:#}").contains("wrong-key-0002"));
    }

    #[tokio::test]
    async fn a_series_is_found_by_its_id_and_a_command_names_sonarrs_own() {
        let (at, seen) = stand_in(Arc::new(|request: &Seen| {
            match (request.method.as_str(), request.target.as_str()) {
                ("GET", "/sonarr/api/v3/series?tvdbId=81189") => (
                    200,
                    r#"[{"id":12,"title":"Breaking Bad","tvdbId":81189,"seasons":[{"seasonNumber":1}]}]"#
                        .into(),
                ),
                ("GET", _) => (200, "[]".into()),
                ("POST", "/sonarr/api/v3/command") => {
                    (201, r#"{"id":4521,"name":"RefreshSeries","status":"queued"}"#.into())
                }
                _ => (404, String::new()),
            }
        }))
        .await;

        let sonarr =
            Connection::new(App::Sonarr, &format!("{at}/sonarr/"), "key-of-sonarr").unwrap();
        let held = sonarr.find(81189).await.unwrap().expect("held");
        assert_eq!(held.id, 12);
        assert!(sonarr.find(100_001_396).await.unwrap().is_none());

        assert_eq!(sonarr.refresh(&[12, 30]).await.unwrap(), Some(4521));
        let sent = seen.lock().last().cloned().unwrap();
        assert_eq!(sent.key.as_deref(), Some("key-of-sonarr"));
        let body: Value = serde_json::from_str(&sent.body).unwrap();
        assert_eq!(body["name"], "RefreshSeries");
        assert_eq!(body["seriesIds"], serde_json::json!([12, 30]));
    }
}
