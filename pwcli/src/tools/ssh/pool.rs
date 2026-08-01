use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::{Mutex, RwLock};

use crate::backend::BackendClient;

use super::client::{self, SshHandle};
use super::config;

pub struct SshPool {
    entries: RwLock<HashMap<String, Arc<Mutex<SshHandle>>>>,
    backend: Arc<BackendClient>,
}

impl SshPool {
    pub fn new(backend: Arc<BackendClient>) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            backend,
        }
    }

    pub async fn get_or_connect(&self, alias: &str) -> Result<Arc<Mutex<SshHandle>>> {
        {
            let entries = self.entries.read().await;
            if let Some(handle) = entries.get(alias) {
                let h = handle.lock().await;
                if !h.is_closed() {
                    drop(h);
                    return Ok(Arc::clone(handle));
                }
            }
        }

        let server = config::find_server(&self.backend, alias).await?;
        let handle = client::connect(&server)
            .await
            .with_context(|| format!("连接 {} 失败", alias))?;

        let shared = Arc::new(Mutex::new(handle));
        {
            let mut entries = self.entries.write().await;
            entries.insert(alias.to_string(), Arc::clone(&shared));
        }

        Ok(shared)
    }

    pub async fn disconnect(&self, alias: &str) {
        let mut entries = self.entries.write().await;
        if let Some(handle) = entries.remove(alias) {
            let h = handle.lock().await;
            let _ = client::disconnect(&h).await;
        }
    }

    pub async fn list_active(&self) -> Vec<String> {
        let entries = self.entries.read().await;
        let mut active = Vec::new();
        for (alias, handle) in entries.iter() {
            let h = handle.lock().await;
            if !h.is_closed() {
                active.push(alias.clone());
            }
        }
        active
    }
}
