//! Process-wide shared state, built once at startup and cloned into handlers.

use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};

use crate::{
    auth::{naming::Resolver, ratelimit::Limiter},
    cache::Caches,
    config::Config,
    db::{Db, repo},
    providers::{
        fanart::FanartClient, radarr::RadarrMetadataClient, skyhook::SkyhookClient,
        tmdb::TmdbClient, tvdb::TvdbClient,
    },
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
    pub limiter: Limiter,
    /// Who may call the address-guarded surfaces, as the database holds it.
    ///
    /// Cached because the guard reads it on every request to those surfaces and
    /// it changes about once a year; [`AppState::reload_allowlist`] is the only
    /// way it moves, and every write path calls it.
    allowlist: Arc<RwLock<Vec<ipnet::IpNet>>>,
    /// Puts a container name to an address, for the callers table.
    pub resolver: Resolver,
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

        let state = Self(Arc::new(Inner {
            allowlist: Arc::new(RwLock::new(Vec::new())),
            resolver: Resolver::new(),
            config,
            db,
            caches,
            http,
            tmdb,
            skyhook,
            radarr_metadata,
            fanart,
            tvdb,
            limiter,
            instance,
        }));

        state.bootstrap_admin().await?;
        state.bootstrap_allowlist().await?;

        Ok(state)
    }

    /// The addresses allowed to call the arr surfaces, right now.
    pub fn allowlist(&self) -> Vec<ipnet::IpNet> {
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
