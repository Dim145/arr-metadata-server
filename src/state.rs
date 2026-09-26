//! Process-wide shared state, built once at startup and cloned into handlers.

use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};

use crate::{
    auth::{naming::Resolver, ratelimit::Limiter},
    cache::Caches,
    config::Config,
    db::{Db, repo},
    providers::{
        anilist::AnilistClient, fanart::FanartClient, fankai::FankaiClient, mal::MalClient,
        radarr::RadarrMetadataClient, skyhook::SkyhookClient, tmdb::TmdbClient, tvdb::TvdbClient,
        tvmaze::TvmazeClient,
    },
    settings::{Scope, Store},
};

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

pub struct Inner {
    pub config: Config,
    pub db: Db,
    pub caches: Caches,
    pub http: reqwest::Client,
    pub tmdb: TmdbClient,
    pub skyhook: SkyhookClient,
    pub radarr_metadata: RadarrMetadataClient,
    pub fanart: FanartClient,
    pub tvdb: TvdbClient,
    pub tvmaze: TvmazeClient,
    pub anilist: AnilistClient,
    pub mal: MalClient,
    pub fankai: FankaiClient,
    pub limiter: Limiter,
    /// Who may call the address-guarded surfaces, as the database holds it.
    ///
    /// Cached because the guard reads it on every request to those surfaces and
    /// it changes about once a year; [`AppState::reload_allowlist`] is the only
    /// way it moves, and every write path calls it.
    allowlist: Arc<RwLock<Vec<(String, ipnet::IpNet)>>>,
    /// Puts a container name to an address, for the callers table.
    pub resolver: Resolver,
    /// Behaviour an operator can change without restarting.
    pub settings: Store,
    /// Identifies this process on outbound calls to hostnames it also answers
    /// on, so a request that loops back can be recognised and refused.
    pub instance: String,
}

impl std::ops::Deref for AppState {
    type Target = Inner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AppState {
    pub async fn bootstrap(config: Config) -> Result<Self> {
        let db = Db::connect(&config.database).await?;
        db.migrate().await?;

        let http = reqwest::Client::builder()
            .user_agent(concat!(
                "arr-metadata-server/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/Dim145/arr-metadata-server)"
            ))
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .pool_max_idle_per_host(16)
            // A redirect is a URL somebody else chose, which is the same
            // problem as an image URL somebody else stored — and it arrives
            // after every check has already passed.
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if crate::outbound::names_internal_host(attempt.url().as_str()) {
                    return attempt.stop();
                }
                if attempt.previous().len() >= 5 {
                    return attempt.error("too many redirects");
                }
                attempt.follow()
            }))
            .build()
            .context("failed to build the outbound HTTP client")?;

        let caches = Caches::new(&config.cache);
        let limiter = Limiter::new(config.security.rate_limit_per_minute);
        let instance = crate::db::new_id();
        let tmdb = TmdbClient::new(http.clone(), &config.tmdb);
        let skyhook = SkyhookClient::new(http.clone(), &config.skyhook, instance.clone());
        let radarr_metadata =
            RadarrMetadataClient::new(http.clone(), &config.radarr_metadata, instance.clone());
        let fanart = FanartClient::new(http.clone(), &config.fanart, &config.tmdb.language);
        let tvdb = TvdbClient::new(http.clone(), &config.tvdb, &config.tmdb.language);
        let tvmaze = TvmazeClient::new(http.clone(), &config.tvmaze);
        let anilist = AnilistClient::new(http.clone(), &config.anilist);
        let mal = MalClient::new(http.clone(), &config.mal);
        let fankai = FankaiClient::new(http.clone(), &config.fankai, &config.tmdb.language);

        if config.fanart.enabled && !fanart.is_configured() {
            tracing::info!("no Fanart.tv key configured; artwork enrichment is off");
        }

        if config.tvdb.enabled && !tvdb.is_configured() {
            tracing::info!(
                "no TheTVDB key configured; absolute episode numbering will only come \
                 from Skyhook where it has it"
            );
        }

        if !tmdb.is_configured() {
            tracing::warn!(
                "no TMDB API key configured: only manual entries and already-cached \
                 data will be served"
            );
        }

        let settings = Store::new(db.clone());

        let state = Self(Arc::new(Inner {
            allowlist: Arc::new(RwLock::new(Vec::new())),
            resolver: Resolver::new(),
            settings,
            config,
            db,
            caches,
            http,
            tmdb,
            skyhook,
            radarr_metadata,
            fanart,
            tvdb,
            tvmaze,
            anilist,
            mal,
            fankai,
            limiter,
            instance,
        }));

        state.bootstrap_admin().await?;
        state.bootstrap_allowlist().await?;
        state.bootstrap_settings().await?;

        Ok(state)
    }

    /// Put the environment's values into the settings table, once.
    ///
    /// Same bargain as the allowlist: a variable gives a setting its starting
    /// value so an existing deployment keeps its behaviour, and is ignored from
    /// then on. Every key is checked on every start rather than only an empty
    /// table, so a setting added in a later version exists for a deployment that
    /// was already running. Anything still read from `config` is about the
    /// deployment rather than the behaviour — a port, a key, a database URL —
    /// and stays there.
    async fn bootstrap_settings(&self) -> Result<()> {
        self.settings.reload().await?;

        let cfg = &self.config;

        let written = self
            .settings
            .seed_missing(&[
                ("tmdb.language", cfg.tmdb.language.clone()),
                // The country of the language's own tag, or TMDB's default: a value
                // from the start, so a key or an address can take it over.
                (
                    "tmdb.watchRegion",
                    cfg.tmdb
                        .language
                        .split(['-', '_'])
                        .nth(1)
                        .map(str::to_ascii_uppercase)
                        .filter(|r| r.len() == 2 && r.bytes().all(|b| b.is_ascii_uppercase()))
                        .unwrap_or_else(|| "US".to_string()),
                ),
                ("tmdb.searchLimit", cfg.tmdb.search_limit.to_string()),
                ("tvdb.searchFallback", cfg.tvdb.enabled.to_string()),
                ("skyhook.fallback", cfg.skyhook.fallback.to_string()),
                ("skyhook.enrich", cfg.skyhook.enrich.to_string()),
                ("radarr.fallback", cfg.radarr_metadata.fallback.to_string()),
                ("radarr.enrich", cfg.radarr_metadata.enrich.to_string()),
                ("tvmaze.enabled", cfg.tvmaze.enabled.to_string()),
                ("anilist.enabled", cfg.anilist.enabled.to_string()),
                ("mal.enabled", cfg.mal.enabled.to_string()),
                ("imdb.enabled", cfg.imdb.enabled.to_string()),
                ("fankai.enabled", cfg.fankai.enabled.to_string()),
                (
                    "webhooks.events",
                    crate::service::webhook::DEFAULT_EVENTS.to_string(),
                ),
                ("refresh.enabled", cfg.refresh.enabled.to_string()),
                (
                    "refresh.intervalSeconds",
                    cfg.refresh.interval.as_secs().to_string(),
                ),
                ("refresh.batchSize", cfg.refresh.batch_size.to_string()),
                // Off unless somebody turns it on. A server that started
                // serving adult titles because it was upgraded would be a
                // surprise of the worst kind.
                (
                    "adult.mode",
                    if cfg.tmdb.include_adult {
                        "visible".to_string()
                    } else {
                        "hidden".to_string()
                    },
                ),
                ("adult.force", "false".to_string()),
            ])
            .await?;

        if written > 0 {
            tracing::info!(
                settings = written,
                "gave the settings a starting value from the environment; they are \
                 editable now, and the variables are not read again"
            );
        }

        self.sync_providers().await;
        Ok(())
    }

    /// Push the settings that providers hold onto them.
    ///
    /// The TMDB client sends a language and an adult flag with every call, and
    /// both are settings now. It keeps them rather than being handed them on
    /// each call, so something has to tell it when they move — and forgetting
    /// to is invisible: the interface would show the new value while the
    /// provider was still asked with the old one.
    pub async fn sync_providers(&self) {
        self.tmdb
            .tune(&self.language(None, None), self.adult_visible());

        // After the tune, not before: a search that started in between reads
        // the old generation, so whatever it caches is filed where nothing
        // will look for it again.
        self.caches
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // A cached search was computed under the old settings. The generation
        // already means none of it will be served; this gives the memory back
        // now rather than at the end of the TTL.
        self.caches.searches.invalidate_all();

        // A cached work carries what the settings put on it when it was read —
        // IMDb's rating, for one — so it is read again under the new ones.
        self.caches.items.invalidate_all();
    }

    /// The language to answer a caller in.
    pub fn language(&self, client: Option<&str>, peer: Option<&str>) -> String {
        self.settings
            .resolve("tmdb.language", client, peer)
            .unwrap_or_else(|| self.config.tmdb.language.clone())
    }

    /// The country a caller watches in: set as such, or the one named by the
    /// language they are answered in (`fr-FR` → `FR`), or none.
    pub fn watch_region(&self, client: Option<&str>, peer: Option<&str>) -> Option<String> {
        let two_letters = |code: &str| {
            let code = code.trim().to_ascii_uppercase();
            (code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase())).then_some(code)
        };
        self.settings
            .resolve("tmdb.watchRegion", client, peer)
            .and_then(|r| two_letters(&r))
            .or_else(|| {
                self.language(client, peer)
                    .split(['-', '_'])
                    .nth(1)
                    .and_then(two_letters)
            })
    }

    pub fn search_limit(&self) -> usize {
        self.settings
            .int_at("tmdb.searchLimit", None, None)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(self.config.tmdb.search_limit)
    }

    pub fn flag(&self, key: &str, fallback: bool) -> bool {
        self.settings.bool_at(key, None, None).unwrap_or(fallback)
    }

    /// Whether adult titles exist at all, as far as this server is concerned.
    pub fn adult_visible(&self) -> bool {
        self.settings
            .resolve("adult.mode", None, None)
            .map(|mode| mode == "visible")
            .unwrap_or(self.config.tmdb.include_adult)
    }

    /// What a caller may see, and whether their own request is allowed to say.
    ///
    /// Server first: with adult titles hidden, nothing else can turn them on.
    /// Then the client's own policy, then — unless the answer is forced — what
    /// the request asked for.
    pub fn adult_for(&self, client: Option<&str>, peer: Option<&str>, asked: Option<bool>) -> bool {
        decide_adult(
            self.adult_visible(),
            self.settings
                .resolve("adult.clientPolicy", client, peer)
                .as_deref()
                .unwrap_or("inherit"),
            self.settings
                .bool_at("adult.force", client, peer)
                .unwrap_or(false),
            asked,
        )
    }

    /// Forget every setting a key or a rule had, when it is deleted.
    pub async fn forget_settings(&self, scope: Scope, scope_id: &str) -> Result<()> {
        self.settings.forget(scope, scope_id).await
    }

    /// The addresses allowed to call the arr surfaces, right now.
    pub fn allowlist(&self) -> Vec<(String, ipnet::IpNet)> {
        match self.0.allowlist.read() {
            Ok(list) => list.clone(),
            // A poisoned lock would otherwise let everyone through; an empty
            // list refuses everyone instead, which is the safe direction.
            Err(_) => Vec::new(),
        }
    }

    /// Re-read the rules. Called after every change to them.
    pub async fn reload_allowlist(&self) -> Result<()> {
        let nets = repo::network::effective(&self.db).await?;

        if let Ok(mut list) = self.0.allowlist.write() {
            *list = nets;
        }

        Ok(())
    }

    /// Put the environment's list into the table, once.
    ///
    /// `AMS_ALLOWLIST` seeds an empty table so an existing deployment keeps
    /// working across the upgrade. After that the table is the truth and the
    /// variable is ignored: two sources for one decision is how a server ends
    /// up refusing a client nobody can explain.
    async fn bootstrap_allowlist(&self) -> Result<()> {
        if repo::network::count_rules(&self.db).await? == 0 {
            let seeded = repo::network::seed(&self.db, &self.config.security.allowlist).await?;

            if seeded > 0 {
                tracing::info!(
                    rules = seeded,
                    "seeded the network allowlist from AMS_ALLOWLIST; it is editable \
                     from the interface now and the variable is no longer read"
                );
            }
        }

        self.reload_allowlist().await
    }

    /// Create the administrator named in the environment, if there is none yet.
    ///
    /// Only ever creates; it never resets an existing password, so leaving the
    /// variables set in a compose file is harmless rather than a standing
    /// credential reset.
    async fn bootstrap_admin(&self) -> Result<()> {
        let Some((username, password)) = self.config.security.bootstrap_admin.clone() else {
            if repo::user::count(&self.db).await? == 0 && !self.config.security.auth_disabled {
                tracing::warn!(
                    "no administrator exists and AMS_ADMIN_USERNAME / AMS_ADMIN_PASSWORD are \
                     unset: the web UI cannot be signed into"
                );
            }
            return Ok(());
        };

        if repo::user::find_by_username(&self.db, &username)
            .await?
            .is_some()
        {
            tracing::debug!(%username, "administrator already exists");
            return Ok(());
        }

        let hash = crate::auth::secrets::hash_password(&password)
            .context("AMS_ADMIN_PASSWORD was rejected")?;

        repo::user::create(&self.db, &username, &hash, true).await?;

        tracing::info!(%username, "created the bootstrap administrator");
        Ok(())
    }
}

/// Whether this caller sees adult titles.
///
/// Four inputs, in the order they win:
///
/// * `visible` — the server. With adult titles hidden nothing below can show
///   them; that is the point of having a server-wide answer at all.
/// * `policy` — this client's own standing: `allow`, `deny`, or `inherit`.
/// * `forced` — whether the request gets a say. Sonarr and Jellyseerr send
///   `include_adult` themselves; forcing means ignoring it and using this
///   server's answer, both towards the providers and when filtering the reply.
/// * `asked` — what the request said, when it is allowed to say anything.
fn decide_adult(visible: bool, policy: &str, forced: bool, asked: Option<bool>) -> bool {
    if !visible {
        return false;
    }

    match policy {
        "deny" => false,
        "allow" if forced => true,
        // Allowed, and the request may still narrow it — a client that asks for
        // none should get none. Silence means yes, since somebody said allow.
        "allow" => asked.unwrap_or(true),
        // `inherit`: the server permits them, so the request decides, unless
        // the operator has said the request does not get to.
        _ if forced => true,
        // Silence means no. A client that never mentions adult titles is not
        // asking for them.
        _ => asked.unwrap_or(false),
    }
}

#[cfg(test)]
mod adult_tests {
    use super::decide_adult;

    #[test]
    fn a_server_that_hides_them_hides_them_from_everyone() {
        for policy in ["inherit", "allow", "deny"] {
            for forced in [true, false] {
                for asked in [None, Some(true), Some(false)] {
                    assert!(
                        !decide_adult(false, policy, forced, asked),
                        "{policy}/{forced}/{asked:?} got through a server that hides them",
                    );
                }
            }
        }
    }

    #[test]
    fn a_client_told_no_is_told_no_whatever_it_asks() {
        assert!(!decide_adult(true, "deny", false, Some(true)));
        assert!(!decide_adult(true, "deny", true, Some(true)));
    }

    #[test]
    fn silence_means_no_unless_somebody_said_otherwise() {
        // A client that never mentions adult titles is not asking for them.
        assert!(!decide_adult(true, "inherit", false, None));
        // But one the operator has allowed is.
        assert!(decide_adult(true, "allow", false, None));
    }

    #[test]
    fn the_request_decides_until_it_is_overruled() {
        assert!(decide_adult(true, "inherit", false, Some(true)));
        assert!(!decide_adult(true, "allow", false, Some(false)));

        // Forced: what the client sent stops mattering.
        assert!(decide_adult(true, "inherit", true, Some(false)));
        assert!(decide_adult(true, "allow", true, Some(false)));
    }
}
