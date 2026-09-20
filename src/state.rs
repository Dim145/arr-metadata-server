//! Process-wide shared state, built once at startup and cloned into handlers.

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::{
    auth::ratelimit::Limiter,
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

        Ok(state)
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
