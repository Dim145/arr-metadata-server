//! Process-wide shared state, built once at startup and cloned into handlers.

use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};

use crate::{
    auth::{naming::Resolver, ratelimit::Limiter},
    cache::Caches,
    config::Config,
    db::{Db, repo},
    providers::{
        anilist::AnilistClient, fanart::FanartClient, fankai::FankaiClient,
        fankai_wiki::FankaiWikiClient, mal::MalClient, radarr::RadarrMetadataClient,
        skyhook::SkyhookClient, tmdb::TmdbClient, tvdb::TvdbClient, tvmaze::TvmazeClient,
    },
    settings::{Scope, Store},
};

/// Keys a member or an editor may hold until an administrator says otherwise.
pub const DEFAULT_KEYS_PER_USER: i64 = 5;

/// Who may open an account for themselves. See `api::native::signup`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Registration {
    Closed,
    Invite,
    Approval,
    Open,
}

/// Calls each API answered and refused since this process started, for the
/// access page. Counted in memory: a number an administrator glances at to
/// see whether Sonarr still calls, not a record anybody audits. Among
/// several instances the counts are added up on the cache server every few
/// seconds, so the page shows what every instance answered.
#[derive(Default)]
pub struct Calls {
    counts: [[std::sync::atomic::AtomicU64; 2]; 4],
    /// How much of each count has been told to the server.
    told: [[std::sync::atomic::AtomicU64; 2]; 4],
}

impl Calls {
    pub fn note(&self, api: crate::config::Api, served: bool) {
        self.counts[api.index()][usize::from(served)]
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Served, then refused — this process's own.
    pub fn read(&self, api: crate::config::Api) -> (u64, u64) {
        let [refused, served] = &self.counts[api.index()];
        (
            served.load(std::sync::atomic::Ordering::Relaxed),
            refused.load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// The key a count is added up under on the server.
    pub fn key(prefix: &str, api: crate::config::Api, served: bool) -> String {
        format!(
            "{prefix}calls:{}:{}",
            api.setting().trim_start_matches("api."),
            if served { "served" } else { "refused" }
        )
    }

    /// The key that says since when the server's tally counts.
    pub fn since_key(prefix: &str) -> String {
        format!("{prefix}calls:since")
    }

    /// Add what was counted since last time to the server's tally.
    pub async fn flush_to(&self, redis: &crate::cache::Redis, prefix: &str) {
        use std::sync::atomic::Ordering::Relaxed;
        // Dated once, by whichever instance first has something to tell.
        redis
            .set_nx_text(&Self::since_key(prefix), &crate::db::now())
            .await;
        for api in crate::config::Api::ALL {
            for served in [true, false] {
                let (i, j) = (api.index(), usize::from(served));
                let counted = self.counts[i][j].load(Relaxed);
                let told = self.told[i][j].load(Relaxed);
                if counted > told
                    && redis
                        .incr_by(&Self::key(prefix, api, served), counted - told)
                        .await
                        .is_some()
                {
                    self.told[i][j].store(counted, Relaxed);
                }
            }
        }
    }

    /// Served, then refused, over every instance: the server's tally plus
    /// what this process has not told it yet. This process's own when the
    /// server does not answer.
    pub async fn read_shared(
        &self,
        redis: &crate::cache::Redis,
        prefix: &str,
        api: crate::config::Api,
    ) -> (u64, u64) {
        use std::sync::atomic::Ordering::Relaxed;
        let keys = [Self::key(prefix, api, true), Self::key(prefix, api, false)];
        let Some(shared) = redis.mget_u64(&keys).await else {
            return self.read(api);
        };
        let untold = |served: bool| {
            let (i, j) = (api.index(), usize::from(served));
            self.counts[i][j]
                .load(Relaxed)
                .saturating_sub(self.told[i][j].load(Relaxed))
        };
        (shared[0] + untold(true), shared[1] + untold(false))
    }
}

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
    pub fankai_wiki: FankaiWikiClient,
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
    /// Held by whatever changes accounts or their keys, so that a rule checked
    /// before a write — "another administrator remains", "under the key
    /// limit" — still holds when the write lands. Two administrators demoting
    /// each other at the same moment would otherwise both pass, and leave
    /// nobody.
    accounts: tokio::sync::Mutex<()>,
    /// What each API answered since the start.
    pub calls: Calls,
    /// The media kept: the store, and the index of what is in it.
    pub media: crate::media::Media,
    /// The doors' certificates: the clients' listener and what it shows.
    pub tls: crate::tls::Tls,
    /// This instance among the others: who leads, what is held, what is told.
    pub coord: crate::coord::Coordination,
    /// Seals the cookies a sign-in through the identity provider travels in.
    /// Alone: made at start and never stored, so a restart only costs a
    /// sign-in that was halfway through, which is asked again. Among
    /// several: kept in the database, since the sign-in may come back to
    /// another instance than the one it left from.
    pub cookie_key: axum_extra::extract::cookie::Key,
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
                " (+",
                env!("CARGO_PKG_REPOSITORY"),
                ")"
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

        let slot = crate::cache::RedisSlot::default();
        let caches = Caches::new(&config.cache, slot.clone());
        let coord = crate::coord::Coordination::new(&config, slot.clone());
        let limiter = Limiter::new(
            config.security.rate_limit_per_minute,
            (config.mode == crate::config::Mode::Multi)
                .then(|| (slot.clone(), config.cache.redis_prefix.clone())),
        );
        let instance = coord.instance.id.clone();
        crate::db::repo::job::name_instance(&config.instance_name);
        let cookie_key = match config.mode {
            crate::config::Mode::Single => axum_extra::extract::cookie::Key::generate(),
            crate::config::Mode::Multi => shared_cookie_key(&db).await?,
        };
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
        let fankai_wiki = FankaiWikiClient::new(http.clone(), &config.fankai_wiki);

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
        let tls = crate::tls::Tls::open(&config, &db).await?;
        let media = crate::media::Media::open(
            &config.media,
            config.server.public_url.as_deref(),
            (config.mode == crate::config::Mode::Multi).then(|| (slot.clone(), instance.clone())),
        )?;
        if media.is_on() {
            media.load_index(&db).await?;
            if config.server.public_url.is_none() {
                tracing::warn!(
                    "media are kept, but AMS_PUBLIC_URL is unset: Sonarr, Radarr and the NFO \
                     documents keep the providers' addresses for them"
                );
            }
        }

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
            fankai_wiki,
            limiter,
            instance,
            accounts: tokio::sync::Mutex::new(()),
            calls: Calls::default(),
            cookie_key,
            tls,
            coord,
            media,
        }));

        // One instance at a time: each finds what the one before it made.
        let held = state.db.hold_start(state.coord.is_multi()).await?;
        let started = async {
            state.bootstrap_admin().await?;
            state.bootstrap_allowlist().await?;
            state.bootstrap_settings().await
        }
        .await;
        held.release().await;
        started?;

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
                ("fankai.wiki", cfg.fankai_wiki.enabled.to_string()),
                ("keys.maxPerUser", DEFAULT_KEYS_PER_USER.to_string()),
                // The one variable the site's opening came from, until it was
                // a setting.
                (
                    "site.access",
                    if cfg.security.public_browse {
                        "public"
                    } else {
                        "private"
                    }
                    .to_string(),
                ),
                ("registration.mode", "closed".to_string()),
                ("registration.role", "member".to_string()),
                // The media kept: fetched as works are stored, served by this
                // server, the cast's photographs and the themes included.
                ("media.store", "true".to_string()),
                ("media.serve", "proxy".to_string()),
                ("cache.items", "true".to_string()),
                ("cache.searches", "true".to_string()),
                ("cache.lists", "true".to_string()),
                ("cache.relay", "true".to_string()),
                ("cache.sessions", "true".to_string()),
                ("media.people", "true".to_string()),
                ("media.audio", "true".to_string()),
                ("api.sonarr", "true".to_string()),
                ("api.radarr", "true".to_string()),
                ("api.tmdb", "true".to_string()),
                ("api.native", "true".to_string()),
                // Off: the relay spends the operator's TMDB quota, and with
                // sign-ups open a member is anybody.
                ("api.tmdbMembers", "false".to_string()),
                ("oidc.enabled", "false".to_string()),
                ("oidc.scopes", "openid profile email".to_string()),
                ("oidc.autoRegister", "false".to_string()),
                ("auth.passwordLogin", "true".to_string()),
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

        // The one variable that is still read: said at every start when it
        // and the setting disagree, since one of them is being overruled.
        let stored_public = self
            .settings
            .resolve("site.access", None, None)
            .is_some_and(|access| access == "public");
        match cfg.security.public_browse_env {
            Some(false) if stored_public => tracing::warn!(
                "AMS_PUBLIC_BROWSE=false keeps the site private, although Opening & APIs says \
                 public; remove the variable to let the setting decide"
            ),
            Some(true) if !stored_public => tracing::info!(
                "AMS_PUBLIC_BROWSE=true only gave the site its first opening; it is private \
                 now because Opening & APIs says so"
            ),
            _ => {}
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
        self.adopt_settings().await;

        // The other instances read the table again and tune their own
        // providers — told before the generation moves on, so that nothing
        // they compute under the old settings is filed under the new one.
        self.coord.tell_now(crate::coord::Message::Settings).await;

        // After the tune, not before: a search that started in between reads
        // the old generation, so whatever it caches is filed where nothing
        // will look for it again.
        self.caches.bump_generation().await;

        // A cached search was computed under the old settings. The generation
        // already means none of it will be served; this gives the memory back
        // now rather than at the end of the TTL — on the server too.
        self.caches.searches.invalidate_all().await;

        // A cached work carries what the settings put on it when it was read —
        // IMDb's rating, for one — so it is read again under the new ones.
        self.caches.items.invalidate_all().await;

        // The relay's documents were patched with the old settings too.
        self.caches.relay.invalidate_all().await;
    }

    /// Take the settings as they stand: tune the providers, forget what
    /// this process computed under the old ones, set the switches. What a
    /// change made here does before it moves the generation on, and all a
    /// change made on another instance asks of this one.
    pub async fn adopt_settings(&self) {
        self.tmdb
            .tune(&self.language(None, None), self.adult_visible());
        self.caches.searches.forget_local();
        self.caches.items.forget_local();
        self.caches.relay.forget_local();
        self.caches.sync_switches(|key| self.flag(key, true));
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

    /// Wait for, then hold, the right to change accounts and their keys. See
    /// the field for why; held for a few statements at most. Among several
    /// instances the right is held on the cache server too, so two
    /// administrators on two instances cannot both pass the same check;
    /// with nobody to ask in time, this process's own lock has to do.
    pub async fn accounts_lock(&self) -> AccountsHeld<'_> {
        let local = self.accounts.lock().await;
        let shared = if self.coord.is_multi() {
            let held = self.coord.hold_wait("accounts").await;
            if held.is_none() {
                tracing::warn!(
                    "the accounts could not be held on the cache server in time; going on with \
                     this instance's own lock"
                );
            }
            held
        } else {
            None
        };
        AccountsHeld {
            _local: local,
            _shared: shared,
        }
    }

    /// Whether the catalogue may be read without signing in: the setting
    /// says so, and the environment does not forbid it.
    pub fn public_site(&self) -> bool {
        !self.site_locked()
            && self
                .settings
                .resolve("site.access", None, None)
                .map(|access| access == "public")
                .unwrap_or(self.config.security.public_browse)
    }

    /// Whether `AMS_PUBLIC_BROWSE=false` keeps the site private whatever the
    /// setting says.
    pub fn site_locked(&self) -> bool {
        self.config.security.public_browse_env == Some(false)
    }

    /// A setting's text, when it says something.
    pub fn text(&self, key: &str) -> Option<String> {
        self.settings
            .resolve(key, None, None)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    /// Where the identity provider sends a browser back: this server's public
    /// address, which the provider must know in advance — so without
    /// AMS_PUBLIC_URL there is none, rather than one guessed from a header.
    pub fn oidc_redirect_uri(&self) -> Option<String> {
        self.config
            .server
            .public_url
            .as_deref()
            .map(|base| format!("{base}/api/v1/auth/oidc/callback"))
    }

    /// The identity provider, when signing in through it is on and every
    /// part of it is set.
    pub fn oidc_provider(&self) -> Option<crate::auth::oidc::Provider> {
        self.oidc_provider_from(&|key| self.settings.resolve(key, None, None))
    }

    /// The identity provider the settings `setting` reads would make: what a
    /// change is checked against before it is written.
    pub fn oidc_provider_from(
        &self,
        setting: &dyn Fn(&str) -> Option<String>,
    ) -> Option<crate::auth::oidc::Provider> {
        let text = |key: &str| {
            setting(key)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        if text("oidc.enabled").as_deref() != Some("true") {
            return None;
        }

        let mut scopes: Vec<String> = text("oidc.scopes")
            .unwrap_or_else(|| "openid profile email".into())
            .split_whitespace()
            .map(str::to_string)
            .collect();
        if !scopes.iter().any(|s| s == "openid") {
            scopes.insert(0, "openid".into());
        }

        Some(crate::auth::oidc::Provider {
            issuer: text("oidc.issuer")?,
            client_id: text("oidc.clientId")?,
            client_secret: self
                .config
                .security
                .oidc_client_secret
                .clone()
                .or_else(|| text("oidc.clientSecret")),
            scopes,
            redirect_uri: self.oidc_redirect_uri()?,
        })
    }

    /// Whether `name` is the account the environment names: the one that
    /// keeps a password door, which no sign-up and no provider may claim.
    pub fn is_break_glass_name(&self, name: &str) -> bool {
        self.config
            .security
            .bootstrap_admin
            .as_ref()
            .is_some_and(|(door, _)| door.eq_ignore_ascii_case(name.trim()))
    }

    /// Whether that door opens onto anything: the account exists, is an
    /// active administrator, and has a password.
    pub async fn break_glass_ready(&self) -> anyhow::Result<bool> {
        let Some((name, _)) = self.config.security.bootstrap_admin.as_ref() else {
            return Ok(false);
        };
        Ok(repo::user::find_by_username(&self.db, name)
            .await?
            .is_some_and(|found| {
                found.user.role == repo::user::Role::Admin
                    && found.user.status == repo::user::Status::Active
                    && found.password_hash != repo::user::NO_PASSWORD
            }))
    }

    /// How the provider's claims become a role here.
    pub fn oidc_mapping(&self) -> crate::auth::oidc::RoleMapping {
        let values = |key: &str| {
            self.text(key)
                .map(|v| {
                    v.split([',', ' '])
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };

        crate::auth::oidc::RoleMapping {
            claim: self.text("oidc.roleClaim"),
            admin: values("oidc.adminValues"),
            editor: values("oidc.editorValues"),
        }
    }

    /// What the sign-in page's button says.
    pub fn oidc_button(&self) -> Option<String> {
        self.text("oidc.buttonLabel")
    }

    /// Whether the password form is offered. Off, only the account
    /// AMS_ADMIN_USERNAME names may still sign in with one — the way back in
    /// when the provider is down.
    pub fn password_login(&self) -> bool {
        self.config.security.force_password_login
            || self.flag("auth.passwordLogin", true)
            || self.oidc_provider().is_none()
    }

    /// Whether members, in person or with their keys, may use the TMDB relay.
    /// Editors and administrators may whenever it is on.
    pub fn relay_for_members(&self) -> bool {
        self.flag("api.tmdbMembers", false)
    }

    /// Who may open an account for themselves.
    pub fn registration(&self) -> Registration {
        match self
            .settings
            .resolve("registration.mode", None, None)
            .as_deref()
        {
            Some("invite") => Registration::Invite,
            Some("approval") => Registration::Approval,
            Some("open") => Registration::Open,
            _ => Registration::Closed,
        }
    }

    /// The role of an account opened without an invitation.
    pub fn registration_role(&self) -> repo::user::Role {
        match self
            .settings
            .resolve("registration.role", None, None)
            .as_deref()
        {
            Some("editor") => repo::user::Role::Editor,
            _ => repo::user::Role::Member,
        }
    }

    /// Whether an administrator left this API on.
    pub fn api_on(&self, api: crate::config::Api) -> bool {
        self.flag(api.setting(), true)
    }

    /// How many keys a person who is not an administrator may hold.
    pub fn keys_per_user(&self) -> i64 {
        self.settings
            .int_at("keys.maxPerUser", None, None)
            .unwrap_or(DEFAULT_KEYS_PER_USER)
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

    /// Re-read the rules, here and on every other instance. Called after
    /// every change to them.
    pub async fn reload_allowlist(&self) -> Result<()> {
        self.reload_allowlist_quietly().await?;
        self.coord.tell(crate::coord::Message::Allowlist);
        Ok(())
    }

    /// Re-read the rules here alone: what another instance's change does.
    pub async fn reload_allowlist_quietly(&self) -> Result<()> {
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

        // The variables open the first account, and let an operator back in
        // who has lost every administrator. They are not a standing order:
        // once administrators exist, an account deleted or demoted from the
        // interface stays that way, whatever the environment still says.
        if repo::user::count_active_admins(&self.db, None).await? > 0 {
            tracing::debug!(%username, "administrators exist; AMS_ADMIN_USERNAME is left alone");
            return Ok(());
        }

        let hash = crate::auth::secrets::hash_password(&password)
            .context("AMS_ADMIN_PASSWORD was rejected")?;

        if let Some(found) = repo::user::find_by_username(&self.db, &username).await? {
            // Nobody can administer the server, and the environment names an
            // account: it is given back its rights, with the password the
            // environment holds, since whoever set that one controls the
            // server anyway.
            let id = &found.user.id;
            repo::user::set_role(&self.db, id, repo::user::Role::Admin).await?;
            repo::user::set_status(&self.db, id, repo::user::Status::Active).await?;
            repo::user::set_password(&self.db, id, &hash).await?;
            repo::user::delete_sessions_for_user(&self.db, id).await?;
            // Whoever it was tied to at the identity provider is not who the
            // environment's password is for.
            repo::user::unlink_oidc(&self.db, id).await?;

            tracing::warn!(
                %username,
                "no active administrator was left: AMS_ADMIN_USERNAME was made an active \
                 administrator again, with AMS_ADMIN_PASSWORD"
            );
            return Ok(());
        }

        repo::user::create(
            &self.db,
            repo::user::NewUser {
                username: &username,
                password_hash: &hash,
                role: repo::user::Role::Admin,
                status: repo::user::Status::Active,
                display_name: None,
                email: None,
                invited_by: None,
                oidc: None,
            },
        )
        .await?;

        tracing::info!(%username, "created the bootstrap administrator");
        Ok(())
    }
}

/// The right to change accounts, held: this process's, and — among several
/// instances — every instance's.
pub struct AccountsHeld<'a> {
    _local: tokio::sync::MutexGuard<'a, ()>,
    _shared: Option<crate::coord::Held>,
}

/// The key every instance seals the sign-in's cookies with: made by the
/// first instance to start, kept in the database, taken by the others.
async fn shared_cookie_key(db: &crate::db::Db) -> Result<axum_extra::extract::cookie::Key> {
    use base64::Engine as _;
    let fresh = axum_extra::extract::cookie::Key::generate();
    let encoded = base64::engine::general_purpose::STANDARD.encode(fresh.master());
    let stored = repo::keystore::put_if_absent(db, repo::keystore::names::COOKIE_KEY, &encoded)
        .await
        .context("could not keep the cookie key in the database")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(stored)
        .context("the cookie key in the database is not base64")?;
    axum_extra::extract::cookie::Key::try_from(bytes.as_slice())
        .map_err(|e| anyhow::anyhow!("the cookie key in the database cannot be used: {e}"))
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
