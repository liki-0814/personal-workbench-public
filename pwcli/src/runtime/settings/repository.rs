use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde_json::{Map, Value};

pub const CONFIG_SCHEMA_VERSION: u32 = 2;
const BACKUP_RETAIN: usize = 20;

#[derive(Debug, Clone)]
pub struct ConfigRepository {
    path: PathBuf,
    fallback_data_dir: PathBuf,
}

impl ConfigRepository {
    pub fn new(path: impl Into<PathBuf>, fallback_data_dir: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            fallback_data_dir: fallback_data_dir.into(),
        }
    }

    pub fn standard() -> Self {
        Self::new(
            super::local_config::config_path(),
            super::local_config::resolve_data_dir_from(None),
        )
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read(&self) -> Result<Value> {
        read_root(&self.path)
    }

    pub fn update<F>(&self, mutator: F) -> Result<()>
    where
        F: FnOnce(&mut Value) -> Result<()>,
    {
        let parent = self
            .path
            .parent()
            .context("config path has no parent directory")?;
        fs::create_dir_all(parent)?;
        let lock_path = parent.join(".config.json.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("open config lock {}", lock_path.display()))?;
        lock.lock_exclusive()?;

        let result = (|| {
            let mut root = read_root(&self.path)?;
            mutator(&mut root)?;
            let object = root
                .as_object_mut()
                .context("config root must be a JSON object")?;
            object.insert(
                "schemaVersion".to_string(),
                Value::from(CONFIG_SCHEMA_VERSION),
            );
            self.backup_existing(&root)?;
            self.atomic_write(parent, &root)
        })();
        let unlock_result = lock.unlock();
        result?;
        unlock_result?;
        Ok(())
    }

    fn atomic_write(&self, parent: &Path, root: &Value) -> Result<()> {
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).context("create unique config temp file")?;
        temporary.write_all(serde_json::to_string_pretty(root)?.as_bytes())?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        set_private(temporary.path())?;
        temporary
            .persist(&self.path)
            .map_err(|error| error.error)
            .with_context(|| format!("replace config {}", self.path.display()))?;
        set_private(&self.path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    }

    fn backup_existing(&self, root: &Value) -> Result<()> {
        if !self.path.exists() {
            return Ok(());
        }
        let configured = root
            .pointer("/server/dataDir")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let data_dir = configured
            .map(|value| PathBuf::from(shellexpand::tilde(value).as_ref()))
            .unwrap_or_else(|| self.fallback_data_dir.clone());
        let directory = data_dir.join("config-backups");
        fs::create_dir_all(&directory)?;
        let filename = format!(
            "config-{}.bak",
            chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S-%6fZ")
        );
        let target = directory.join(filename);
        fs::copy(&self.path, &target)?;
        set_private(&target)?;
        let mut backups = fs::read_dir(&directory)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("config-"))
            .filter_map(|entry| {
                let modified = entry.metadata().ok()?.modified().ok()?;
                Some((modified, entry.path()))
            })
            .collect::<Vec<_>>();
        backups.sort_by(|left, right| right.0.cmp(&left.0));
        for (_, path) in backups.into_iter().skip(BACKUP_RETAIN) {
            if let Err(error) = fs::remove_file(&path) {
                tracing::warn!(path = %path.display(), error = %error, "remove old config backup failed");
            }
        }
        Ok(())
    }
}

fn read_root(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let value: Value = serde_json::from_str(&content)
        .with_context(|| format!("parse config {}", path.display()))?;
    if !value.is_object() {
        anyhow::bail!("config root must be a JSON object");
    }
    Ok(value)
}

fn set_private(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_json_is_never_silently_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "{invalid").unwrap();
        let repository = ConfigRepository::new(&path, dir.path().join("data"));
        assert!(repository.update(|_| Ok(())).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "{invalid");
    }

    #[test]
    fn update_preserves_siblings_and_sets_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"unmanaged":{"keep":true}}"#).unwrap();
        let repository = ConfigRepository::new(&path, dir.path().join("data"));
        repository
            .update(|root| {
                root["permissions"] = serde_json::json!({"agent_mode":"risk"});
                Ok(())
            })
            .unwrap();
        let root = repository.read().unwrap();
        assert_eq!(root["schemaVersion"], CONFIG_SCHEMA_VERSION);
        assert_eq!(root["unmanaged"]["keep"], true);
    }

    #[test]
    fn concurrent_updates_reread_under_the_file_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "{}").unwrap();
        let repository = ConfigRepository::new(&path, dir.path().join("data"));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));

        let handles = ["left", "right"].map(|field| {
            let repository = repository.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                repository
                    .update(|root| {
                        root["concurrent"][field] = Value::Bool(true);
                        Ok(())
                    })
                    .unwrap();
            })
        });
        barrier.wait();
        for handle in handles {
            handle.join().unwrap();
        }

        let root = repository.read().unwrap();
        assert_eq!(root["concurrent"]["left"], true);
        assert_eq!(root["concurrent"]["right"], true);
    }

    #[test]
    fn backups_are_bounded_and_written_beneath_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let data_dir = dir.path().join("data");
        fs::write(&path, "{}").unwrap();
        let repository = ConfigRepository::new(&path, &data_dir);
        for revision in 0..25 {
            repository
                .update(|root| {
                    root["revision"] = Value::from(revision);
                    Ok(())
                })
                .unwrap();
        }
        let backups = fs::read_dir(data_dir.join("config-backups"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .count();
        assert_eq!(backups, BACKUP_RETAIN);
    }
}
