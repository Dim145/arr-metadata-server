//! Process-wide shared state, built once at startup and cloned into handlers.

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::{cache::Caches, config::Config, db::Db};

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

pub struct Inner {
    pub config: Config,
    pub db: Db,
    pub caches: Caches,
    pub http: reqwest::Client,
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

        Ok(Self(Arc::new(Inner {
            config,
            db,
            caches,
            http,
        })))
    }
}
