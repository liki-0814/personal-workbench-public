use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::IllustrationRun;

pub struct IllustrationRunStore {
    root: PathBuf,
}

impl IllustrationRunStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("illustration/runs"),
        }
    }

    pub async fn save(&self, run: &IllustrationRun) -> Result<()> {
        tokio::fs::create_dir_all(&self.root).await?;
        let destination = self.root.join(format!("{}.json", run.id));
        let temporary = self.root.join(format!(".{}.tmp", run.id));
        tokio::fs::write(&temporary, serde_json::to_vec_pretty(run)?).await?;
        tokio::fs::rename(&temporary, &destination)
            .await
            .with_context(|| format!("persist illustration run {}", run.id))?;
        Ok(())
    }

    pub async fn load(&self, id: &str) -> Result<IllustrationRun> {
        anyhow::ensure!(
            id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "invalid run id"
        );
        Ok(serde_json::from_slice(
            &tokio::fs::read(self.root.join(format!("{id}.json"))).await?,
        )?)
    }
}
