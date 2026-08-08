mod backups;
mod config;
pub(crate) use config::ProviderEndpoint;
mod data;
mod documents;
mod fs;
mod illustration;
pub(crate) mod images;
mod jobs;
mod providers;
mod proxy;

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;

use super::state::AppState;

pub use config::ConfigStore;
pub use data::{DataStore, KvEventJournal};

pub struct WebState {
    pub data: DataStore,
    pub config: ConfigStore,
    pub events: KvEventJournal,
    pub jobs: Arc<jobs::JobManager>,
}

impl WebState {
    pub fn new(data_dir: &Path) -> Result<Arc<Self>> {
        crate::runtime::documents::init(data_dir)?;
        let config = ConfigStore::new(data_dir.to_path_buf());
        let data = DataStore::open(data_dir.join("data.db"))?;
        let state = Arc::new(Self {
            data,
            config,
            events: KvEventJournal::new(256),
            jobs: jobs::JobManager::new()?,
        });
        state.migrate_legacy_ssh_servers()?;
        state.seed_config_cache()?;
        state.start_config_watcher();
        state.start_periodic_backup();
        Ok(state)
    }

    fn start_config_watcher(self: &Arc<Self>) {
        let state = Arc::clone(self);
        tokio::spawn(async move {
            let mut revision = state.config.revision();
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
            interval.tick().await;
            loop {
                interval.tick().await;
                let current = state.config.revision();
                if current == revision {
                    continue;
                }
                revision = current;
                if let Err(error) = crate::app::config::local_config::reload()
                    .await
                    .and_then(|()| state.seed_config_cache())
                {
                    tracing::warn!(%error, "failed to reload externally edited config");
                    continue;
                }
                state.events.publish(vec![
                    "local_config".to_string(),
                    "ai_providers".to_string(),
                    "app_config".to_string(),
                ]);
            }
        });
    }

    fn seed_config_cache(&self) -> Result<()> {
        self.data.set_raw(
            "ai_providers",
            &self.config.frontend_providers(true)?.to_string(),
        )?;
        self.data.set_raw(
            "app_config",
            &self.config.frontend_app_config()?.to_string(),
        )?;
        Ok(())
    }

    fn migrate_legacy_ssh_servers(&self) -> Result<()> {
        let legacy = self.data.get_raw("ssh_servers")?;
        if self.config.ssh_servers()?.is_none() {
            if let Some(raw) = legacy.as_deref() {
                match serde_json::from_str::<Vec<crate::runtime::tools::ssh::config::SshServer>>(
                    raw,
                ) {
                    Ok(servers) => {
                        self.config
                            .write_ssh_servers(serde_json::to_value(servers)?)?;
                    }
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            "ignored invalid legacy ssh_servers while migrating to config.json"
                        );
                    }
                }
            }
        }
        if legacy.is_some() {
            self.data.delete("ssh_servers")?;
        }
        Ok(())
    }

    fn start_periodic_backup(self: &Arc<Self>) {
        let state = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30 * 60));
            interval.tick().await;
            loop {
                interval.tick().await;
                let directory = state
                    .data
                    .path()
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join("backups");
                if let Err(error) = std::fs::create_dir_all(&directory)
                    .map_err(anyhow::Error::from)
                    .and_then(|()| state.data.backup_to(&directory.join("latest.db")))
                {
                    tracing::warn!(%error, "periodic frontend database backup failed");
                }
            }
        });
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(backups::routes())
        .merge(data::routes())
        .merge(documents::routes())
        .merge(config::routes())
        .merge(fs::routes())
        .merge(images::routes())
        .merge(illustration::routes())
        .merge(jobs::routes())
        .merge(proxy::routes())
        .merge(providers::routes())
}
