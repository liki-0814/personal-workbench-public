use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Credential {
    ApiKey { key: String },
    OAuth(OAuthCredential),
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKey { .. } => formatter.write_str("ApiKey(REDACTED)"),
            Self::OAuth(value) => formatter
                .debug_struct("OAuth")
                .field("expires_at", &value.expires_at)
                .field("account_id", &value.account_id.as_ref().map(|_| "REDACTED"))
                .field("account_label", &value.account_label)
                .finish(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthCredential {
    pub access: String,
    pub refresh: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
    /// Cloud Code Assist project discovered during Google Antigravity login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct CredentialFile {
    #[serde(default)]
    credentials: BTreeMap<String, Credential>,
}

#[derive(Debug, Clone)]
pub struct CredentialStore {
    path: PathBuf,
}

impl Default for CredentialStore {
    fn default() -> Self {
        Self::standard()
    }
}

impl CredentialStore {
    pub fn standard() -> Self {
        Self::new(crate::config::RuntimeConfig::config_dir().join("credentials.json"))
    }

    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read(&self, reference: &str) -> Result<Option<Credential>> {
        Ok(self.read_file()?.credentials.get(reference).cloned())
    }

    pub fn list(&self) -> Result<Vec<(String, Credential)>> {
        Ok(self.read_file()?.credentials.into_iter().collect())
    }

    pub fn write(&self, reference: &str, credential: Credential) -> Result<()> {
        self.modify(reference, |_| Ok(Some(credential))).map(|_| ())
    }

    pub fn remove(&self, reference: &str) -> Result<()> {
        self.modify(reference, |_| Ok(None)).map(|_| ())
    }

    pub fn modify<F>(&self, reference: &str, update: F) -> Result<Option<Credential>>
    where
        F: FnOnce(Option<Credential>) -> Result<Option<Credential>>,
    {
        let parent = self
            .path
            .parent()
            .context("credential path has no parent")?;
        fs::create_dir_all(parent)?;
        let lock_path = parent.join(".credentials.json.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        lock.lock_exclusive()?;
        let result = (|| {
            let mut file = self.read_file()?;
            let next = update(file.credentials.get(reference).cloned())?;
            match &next {
                Some(value) => {
                    file.credentials
                        .insert(reference.to_string(), value.clone());
                }
                None => {
                    file.credentials.remove(reference);
                }
            }
            self.atomic_write(parent, &file)?;
            Ok(next)
        })();
        let unlock = lock.unlock();
        result.and_then(|value| {
            unlock?;
            Ok(value)
        })
    }

    fn read_file(&self) -> Result<CredentialFile> {
        if !self.path.exists() {
            return Ok(CredentialFile::default());
        }
        let raw = fs::read_to_string(&self.path)
            .with_context(|| format!("read credential store {}", self.path.display()))?;
        if raw.trim().is_empty() {
            return Ok(CredentialFile::default());
        }
        serde_json::from_str(&raw)
            .with_context(|| format!("parse credential store {}", self.path.display()))
    }

    fn atomic_write(&self, parent: &Path, value: &CredentialFile) -> Result<()> {
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        set_private(temporary.path())?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        set_private(&self.path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    }
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
    fn credentials_round_trip_without_debug_leakage() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path().join("credentials.json"));
        store
            .write(
                "provider-a",
                Credential::ApiKey {
                    key: "secret".into(),
                },
            )
            .unwrap();
        assert!(
            matches!(store.read("provider-a").unwrap(), Some(Credential::ApiKey { key }) if key == "secret")
        );
        assert!(!format!("{:?}", store.read("provider-a").unwrap()).contains("secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(store.path()).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn modify_serializes_updates() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path().join("credentials.json"));
        store
            .modify("p", |_| Ok(Some(Credential::ApiKey { key: "one".into() })))
            .unwrap();
        store
            .modify("p", |_| Ok(Some(Credential::ApiKey { key: "two".into() })))
            .unwrap();
        assert!(
            matches!(store.read("p").unwrap(), Some(Credential::ApiKey { key }) if key == "two")
        );
    }
}
