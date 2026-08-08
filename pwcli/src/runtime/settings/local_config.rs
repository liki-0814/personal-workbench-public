//! Nested-schema reader/writer over `~/.pwcli/config.json`.
//!
//! Track H1 — single source of truth for config that *used to* live in `.env`:
//! - `ai.moa` (Harness decision review configuration)
//! - `ai.mineruToken` (was `MINERU_TOKEN`)
//! - `tools.fsBase` (was `PWB_FS_BASE`)
//! - `tools.codeAgent.{enabledBackends,model,contextWindow,effort}` (was `PWCLI_CODE_AGENT_*`)
//! - `tools.delegation` (executor allow-list, role routes, and executor defaults)
//! - `memory.{injectionMode,autoRetrieve,maxResults,maxDigestBytes}` (was `PWCLI_MEMORY_INJECTION_MODE` + `features.memory_injection`)
//! - `features.autoMemoryExtract`
//!
//! Coexistence: the file is also written by `RuntimeConfig` (providers /
//! permissions / user / features). To avoid clobbering each other, this module
//! holds a raw `serde_json::Value` cache and merges only its managed keys on
//! save (preserving any unknown sibling fields). `RuntimeConfig::save()` does
//! the same in reverse — it serializes the full struct, but it only mutates
//! its own keys (so as long as both sides read-modify-write in their own
//! window, neither blows away the other).
//!
//! Read strategy: cache populated via [`init`] at startup and refreshed via
//! [`reload`]; readers call [`get`] which clones the typed view (fast — only
//! the small managed sections deserialize, no providers/sessions touched).
//!
//! Write strategy: read-from-disk → merge managed keys → write. No file lock
//! (best-effort; conflicting writes resolve by last-writer-wins, which matches
//! how `RuntimeConfig::save()` already behaves).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use crate::runtime::decision::config::MoaConfig;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalConfig {
    #[serde(rename = "schemaVersion", default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub user: UserSection,
    #[serde(default)]
    pub ai: AiSection,
    #[serde(default)]
    pub tools: ToolsSection,
    #[serde(default)]
    pub memory: MemorySection,
    #[serde(default)]
    pub features: FeaturesSection,
    #[serde(default)]
    pub server: ServerSection,
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            user: UserSection::default(),
            ai: AiSection::default(),
            tools: ToolsSection::default(),
            memory: MemorySection::default(),
            features: FeaturesSection::default(),
            server: ServerSection::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerSection {
    #[serde(default)]
    pub data_dir: String,
    #[serde(default = "default_backend_port")]
    pub backend_port: u16,
    #[serde(default)]
    pub show_hidden_files: bool,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            data_dir: String::new(),
            backend_port: default_backend_port(),
            show_hidden_files: false,
        }
    }
}

fn default_backend_port() -> u16 {
    3456
}

fn default_schema_version() -> u32 {
    super::CONFIG_SCHEMA_VERSION
}

pub fn resolve_data_dir_from(config_value: Option<&str>) -> PathBuf {
    config_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| PathBuf::from(shellexpand::tilde(value).as_ref()))
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".pwcli/data")
        })
}

pub fn data_dir() -> PathBuf {
    resolve_data_dir_from(Some(&get().server.data_dir))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct UserSection {
    #[serde(default)]
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AiSection {
    /// Named advisor/judge presets used only at Harness decision checkpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moa: Option<MoaConfig>,
    #[serde(default)]
    pub mineru_token: String,
    #[serde(default)]
    pub response_language: ResponseLanguage,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ResponseLanguage {
    #[default]
    #[serde(rename = "zh-CN")]
    Chinese,
    #[serde(rename = "en")]
    English,
    #[serde(rename = "auto")]
    Auto,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolsSection {
    #[serde(default)]
    pub fs_base: String,
    #[serde(default)]
    pub code_agent: CodeAgentSection,
    #[serde(default)]
    pub delegation: DelegationSection,
    #[serde(default)]
    pub any_search: AnySearchSection,
    #[serde(default)]
    pub gen_image: GenImageSection,
    /// User-managed SSH configuration. `config.json` is the only source of
    /// truth; an empty list means no machine is provisioned for a new user.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ssh_servers: Vec<crate::runtime::tools::ssh::config::SshServer>,
}

impl Default for ToolsSection {
    fn default() -> Self {
        Self {
            fs_base: default_fs_base(),
            code_agent: CodeAgentSection::default(),
            delegation: DelegationSection::default(),
            any_search: AnySearchSection::default(),
            gen_image: GenImageSection::default(),
            ssh_servers: Vec::new(),
        }
    }
}

fn default_fs_base() -> String {
    "~/".to_string()
}

impl<'de> Deserialize<'de> for ToolsSection {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize, Default)]
        #[serde(rename_all = "camelCase")]
        struct RawToolsSection {
            #[serde(default = "default_fs_base")]
            fs_base: String,
            #[serde(default)]
            code_agent: CodeAgentSection,
            delegation: Option<DelegationSection>,
            #[serde(default)]
            any_search: AnySearchSection,
            #[serde(default)]
            gen_image: GenImageSection,
            #[serde(default)]
            ssh_servers: Vec<crate::runtime::tools::ssh::config::SshServer>,
        }

        let raw = RawToolsSection::deserialize(deserializer)?;
        let delegation = raw.delegation.unwrap_or_else(|| {
            DelegationSection::from_legacy_backends(&raw.code_agent.enabled_backends)
        });
        Ok(Self {
            fs_base: raw.fs_base,
            code_agent: raw.code_agent,
            delegation,
            any_search: raw.any_search,
            gen_image: raw.gen_image,
            ssh_servers: raw.ssh_servers,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AnySearchSection {
    /// Optional AnySearch API key. Empty means use the anonymous quota.
    #[serde(default)]
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenImageSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub default_model: String,
    #[serde(default)]
    pub models: Vec<GenImageModelSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenImageModelSection {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub protocol: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub edit_url: String,
    #[serde(default)]
    pub capabilities: GenImageCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenImageCapabilities {
    #[serde(default)]
    pub generate: Option<bool>,
    #[serde(default)]
    pub reference: Option<bool>,
    #[serde(default)]
    pub edit: Option<bool>,
    #[serde(default)]
    pub preferred_scenes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeAgentSection {
    /// CLIs the user allows pwcli to use. Empty keeps backward compatibility by
    /// allowing every locally detected backend.
    #[serde(default)]
    pub enabled_backends: Vec<String>,
    /// Legacy per-backend defaults. Kept for existing config files, but no
    /// longer used as backend priority.
    #[serde(default)]
    pub backend: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub context_window: u64,
    #[serde(default)]
    pub effort: String,
    #[serde(default)]
    pub thinking: bool,
    #[serde(default)]
    pub fast: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DelegationSection {
    #[serde(default = "default_enabled_executors")]
    pub enabled_executors: Vec<String>,
    #[serde(default = "default_cli_priority")]
    pub cli_priority: Vec<String>,
    #[serde(default = "default_delegation_roles")]
    pub roles: BTreeMap<String, DelegationRoleSection>,
    #[serde(default = "default_executor_defaults")]
    pub executor_defaults: BTreeMap<String, ExecutorDefaultSection>,
}

impl Default for DelegationSection {
    fn default() -> Self {
        Self {
            enabled_executors: default_enabled_executors(),
            cli_priority: default_cli_priority(),
            roles: default_delegation_roles(),
            executor_defaults: default_executor_defaults(),
        }
    }
}

impl DelegationSection {
    /// Migrate legacy code-agent configuration without changing the user's CLI order.
    pub fn from_legacy_backends(backends: &[String]) -> Self {
        let cli_priority = normalize_legacy_backends(backends);
        let mut enabled_executors = Vec::with_capacity(cli_priority.len() + 1);
        enabled_executors.push("pwcli".to_string());
        enabled_executors.extend(cli_priority.iter().cloned());
        Self {
            enabled_executors,
            cli_priority,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DelegationRoleSection {
    pub label: String,
    #[serde(default)]
    pub routes: Vec<String>,
    #[serde(default)]
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorDefaultSection {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub effort: String,
    #[serde(default = "default_executor_permission_mode")]
    pub permission_mode: String,
}

impl Default for ExecutorDefaultSection {
    fn default() -> Self {
        Self {
            model: String::new(),
            effort: String::new(),
            permission_mode: default_executor_permission_mode(),
        }
    }
}

fn default_enabled_executors() -> Vec<String> {
    ["pwcli", "codex", "qoder", "kimi"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn default_cli_priority() -> Vec<String> {
    ["codex", "qoder", "kimi"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn normalize_legacy_backends(backends: &[String]) -> Vec<String> {
    if backends.is_empty() {
        return default_cli_priority();
    }
    let mut normalized = Vec::new();
    for backend in backends {
        let canonical = match backend.trim().to_ascii_lowercase().as_str() {
            "codex" => "codex",
            "qoder" | "qodercli" => "qoder",
            "kimi" | "kimi-code" => "kimi",
            _ => continue,
        };
        if !normalized.iter().any(|value| value == canonical) {
            normalized.push(canonical.to_string());
        }
    }
    normalized
}

fn default_delegation_roles() -> BTreeMap<String, DelegationRoleSection> {
    [
        (
            "researcher",
            "调研员",
            ["pwcli:researcher", "cli:auto"],
            ["Alex", "Sam", "Tina"],
        ),
        (
            "engineer",
            "工程师",
            ["cli:auto", "pwcli:engineer"],
            ["Leo", "Mia", "Noah"],
        ),
        (
            "reviewer",
            "审阅员",
            ["pwcli:reviewer", "cli:auto"],
            ["Iris", "Evan", "Maya"],
        ),
        (
            "analyst",
            "分析师",
            ["pwcli:analyst", "cli:auto"],
            ["Robin", "Quinn", "Avery"],
        ),
        (
            "operator",
            "执行员",
            ["pwcli:operator", "cli:auto"],
            ["Kai", "Max", "Nora"],
        ),
        (
            "general",
            "协作者",
            ["pwcli:general", "cli:auto"],
            ["Jamie", "Taylor", "Morgan"],
        ),
    ]
    .into_iter()
    .map(|(id, label, routes, names)| {
        (
            id.to_string(),
            DelegationRoleSection {
                label: label.to_string(),
                routes: routes.into_iter().map(str::to_string).collect(),
                names: names.into_iter().map(str::to_string).collect(),
            },
        )
    })
    .collect()
}

fn default_executor_defaults() -> BTreeMap<String, ExecutorDefaultSection> {
    ["codex", "qoder", "kimi"]
        .into_iter()
        .map(|executor| (executor.to_string(), ExecutorDefaultSection::default()))
        .collect()
}

fn default_executor_permission_mode() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemorySection {
    #[serde(default = "default_injection_mode")]
    pub injection_mode: String,
    #[serde(default = "default_true")]
    pub auto_retrieve: bool,
    #[serde(default = "default_max_results")]
    pub max_results: u32,
    #[serde(default = "default_max_digest_bytes")]
    pub max_digest_bytes: u32,
    #[serde(default)]
    pub dream_enabled: bool,
    #[serde(default)]
    pub dream_project: String,
}

impl Default for MemorySection {
    fn default() -> Self {
        Self {
            injection_mode: default_injection_mode(),
            auto_retrieve: true,
            max_results: 5,
            max_digest_bytes: 1024,
            dream_enabled: false,
            dream_project: String::new(),
        }
    }
}

fn default_injection_mode() -> String {
    "retrieval".into()
}
fn default_true() -> bool {
    true
}
fn default_max_results() -> u32 {
    5
}
fn default_max_digest_bytes() -> u32 {
    1024
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FeaturesSection {
    #[serde(default = "default_true")]
    pub auto_memory_extract: bool,
    #[serde(default = "default_true")]
    pub unified_recovery: bool,
}

impl Default for FeaturesSection {
    fn default() -> Self {
        Self {
            auto_memory_extract: true,
            unified_recovery: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------

/// `~/.pwcli/config.json`. Tests can override `$HOME` to redirect.
pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".pwcli")
        .join("config.json")
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

fn cache() -> &'static RwLock<LocalConfig> {
    static CACHE: OnceLock<RwLock<LocalConfig>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(LocalConfig::default()))
}

/// Snapshot of the current cached config.
pub fn get() -> LocalConfig {
    cache().read().map(|g| g.clone()).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Init / reload
// ---------------------------------------------------------------------------

/// Read disk → cache. Call once at startup. Failures are logged but never
/// propagate (so a broken config.json doesn't kill pwcli — caller falls back
/// to defaults).
///
/// Also schedules a lightweight mtime-polling watcher (3s interval) so that
/// when the daemon writes `~/.pwcli/config.json` we pick up
/// changes within ~3s without needing IPC. Polling instead of `notify` to
/// avoid pulling in another C dependency and to handle macOS atomic-saves
/// (rename + new inode) without re-binding watchers.
pub async fn init() {
    if let Err(e) = reload().await {
        tracing::warn!("local_config init failed ({e}); using defaults");
    }
    start_mtime_watcher();
}

fn start_mtime_watcher() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return; // already running
    }
    tokio::spawn(async {
        let path = config_path();
        // Seed the baseline mtime; if file doesn't yet exist we'll just pick
        // it up on first poll where mtime becomes Some.
        let mut last_mtime: Option<std::time::SystemTime> = match tokio::fs::metadata(&path).await {
            Ok(m) => m.modified().ok(),
            Err(_) => None,
        };
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3));
        // First tick fires immediately; skip it to avoid a redundant reload.
        interval.tick().await;
        loop {
            interval.tick().await;
            let mtime = match tokio::fs::metadata(&path).await {
                Ok(m) => m.modified().ok(),
                Err(_) => None,
            };
            if mtime != last_mtime {
                last_mtime = mtime;
                if let Err(e) = reload().await {
                    tracing::warn!("local_config auto-reload failed: {e}");
                } else {
                    tracing::debug!("local_config auto-reloaded from {}", path.display());
                }
            }
        }
    });
}

/// Force a fresh disk read into the cache. Used after the frontend (or
/// `RuntimeConfig::save`) writes the file out-of-band, and we want to pick up
/// the changes without restarting.
pub async fn reload() -> Result<()> {
    let path = config_path();
    let raw: Value = if path.exists() {
        let content = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("read {}", path.display()))?;
        if content.trim().is_empty() {
            Value::Object(Map::new())
        } else {
            serde_json::from_str(&content).with_context(|| format!("parse {}", path.display()))?
        }
    } else {
        // First run: seed file with defaults so the schema is discoverable.
        let default = LocalConfig::default();
        if let Err(e) = save(&default).await {
            tracing::warn!("seeding default {} failed: {e}", path.display());
        }
        return Ok(());
    };
    let typed = extract_typed(&raw);
    *cache().write().unwrap() = typed;
    Ok(())
}

/// Write through: re-read disk, merge our managed sections, write back.
/// Caller-provided `cfg` clobbers any prior values for the keys this module
/// owns (`schemaVersion` / `user.id` / `ai.*` / `tools.*` / `memory.*` /
/// `features.autoMemoryExtract`); other fields (e.g. `providers`,
/// `permissions`) are preserved.
pub async fn save(cfg: &LocalConfig) -> Result<()> {
    let cfg_for_write = cfg.clone();
    tokio::task::spawn_blocking(move || {
        super::ConfigRepository::standard().update(|root| merge_managed(root, &cfg_for_write))
    })
    .await
    .context("join config writer")??;

    *cache().write().unwrap() = cfg.clone();
    Ok(())
}

/// Read-modify-write helper: reload from disk first (defends against frontend
/// writes during the same window), apply mutator, save.
pub async fn update<F>(mutator: F) -> Result<()>
where
    F: FnOnce(&mut LocalConfig),
{
    // Best-effort reload to minimize the read-modify-write window.
    let _ = reload().await;
    let mut cfg = get();
    mutator(&mut cfg);
    save(&cfg).await
}

// ---------------------------------------------------------------------------
// Value <-> typed coercion
// ---------------------------------------------------------------------------

/// Extract just the keys this module owns from a raw value, falling back to
/// defaults when missing/malformed. Unknown sibling keys (providers, etc.)
/// are ignored — they're untouched on save.
fn extract_typed(root: &Value) -> LocalConfig {
    let mut out = LocalConfig::default();

    if let Some(v) = root.get("schemaVersion").and_then(|v| v.as_u64()) {
        out.schema_version = v as u32;
    }

    if let Some(user) = root.get("user") {
        if let Some(id) = user.get("id").and_then(|v| v.as_str()) {
            out.user.id = id.to_string();
        }
    }

    if let Some(ai) = root.get("ai") {
        if let Ok(parsed) = serde_json::from_value::<AiSection>(ai.clone()) {
            out.ai = parsed;
        } else {
            // Partial parse: take what we can.
            if let Some(token) = ai.get("mineruToken").and_then(|v| v.as_str()) {
                out.ai.mineru_token = token.to_string();
            }
            if let Some(moa) = ai.get("moa") {
                if let Ok(parsed) = serde_json::from_value::<MoaConfig>(moa.clone()) {
                    out.ai.moa = Some(parsed);
                }
            }
        }
    }

    if let Some(tools) = root.get("tools") {
        if let Ok(parsed) = serde_json::from_value::<ToolsSection>(tools.clone()) {
            out.tools = parsed;
        }
    }

    if let Some(mem) = root.get("memory") {
        if let Ok(parsed) = serde_json::from_value::<MemorySection>(mem.clone()) {
            out.memory = parsed;
        }
    }

    if let Some(features) = root.get("features") {
        if let Some(b) = features.get("autoMemoryExtract").and_then(|v| v.as_bool()) {
            out.features.auto_memory_extract = b;
        }
        if let Some(b) = features.get("unifiedRecovery").and_then(|v| v.as_bool()) {
            out.features.unified_recovery = b;
        }
    }

    if let Some(server) = root.get("server") {
        if let Ok(parsed) = serde_json::from_value::<ServerSection>(server.clone()) {
            out.server = parsed;
        }
    }

    out
}

/// Merge managed sections of `cfg` into the existing `root` object,
/// preserving unknown sibling keys (e.g. `providers`, `permissions`,
/// `features.streaming`).
fn merge_managed(root: &mut Value, cfg: &LocalConfig) -> Result<()> {
    let map = match root {
        Value::Object(m) => m,
        _ => {
            *root = Value::Object(Map::new());
            root.as_object_mut().unwrap()
        }
    };

    map.insert(
        "schemaVersion".into(),
        Value::Number(cfg.schema_version.into()),
    );

    // user.id (preserve other user.* fields)
    {
        let user_entry = map
            .entry("user")
            .or_insert_with(|| Value::Object(Map::new()));
        if !user_entry.is_object() {
            *user_entry = Value::Object(Map::new());
        }
        let user_obj = user_entry.as_object_mut().unwrap();
        if cfg.user.id.is_empty() {
            user_obj.remove("id");
        } else {
            user_obj.insert("id".into(), Value::String(cfg.user.id.clone()));
        }
    }

    // ai.*
    map.insert("ai".into(), serde_json::to_value(&cfg.ai)?);

    // tools.*
    map.insert("tools".into(), serde_json::to_value(&cfg.tools)?);

    // memory.*
    map.insert("memory".into(), serde_json::to_value(&cfg.memory)?);

    // features.autoMemoryExtract (preserve other features.* — RuntimeConfig
    // also writes here)
    {
        let feats_entry = map
            .entry("features")
            .or_insert_with(|| Value::Object(Map::new()));
        if !feats_entry.is_object() {
            *feats_entry = Value::Object(Map::new());
        }
        let feats_obj = feats_entry.as_object_mut().unwrap();
        feats_obj.insert(
            "autoMemoryExtract".into(),
            Value::Bool(cfg.features.auto_memory_extract),
        );
        feats_obj.insert(
            "unifiedRecovery".into(),
            Value::Bool(cfg.features.unified_recovery),
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::sync::Mutex;

    /// Tests that mutate `$HOME` must serialize — concurrent set_var across
    /// threads is a data race. The async-aware guard intentionally remains
    /// held while file operations await.
    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    async fn lock_env() -> tokio::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().await
    }

    struct HomeGuard {
        _tmp: TempDir,
        prev: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn new() -> Self {
            let tmp = TempDir::new().expect("tempdir");
            let prev = std::env::var_os("HOME");
            unsafe {
                std::env::set_var("HOME", tmp.path());
            }
            // Reset cache so each test starts clean.
            *cache().write().unwrap() = LocalConfig::default();
            Self { _tmp: tmp, prev }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    #[tokio::test]
    async fn default_when_file_missing() {
        let _lock = lock_env().await;
        let _g = HomeGuard::new();

        // File should not exist yet.
        assert!(!config_path().exists());

        reload().await.expect("reload seeds default");

        // First-run reload writes a default file.
        assert!(config_path().exists(), "default file should be seeded");

        let cfg = get();
        assert_eq!(cfg.schema_version, super::super::CONFIG_SCHEMA_VERSION);
        assert_eq!(cfg.memory.injection_mode, "retrieval");
        assert!(cfg.features.auto_memory_extract);
        assert!(cfg.ai.moa.is_none());
        assert_eq!(cfg.ai.response_language, ResponseLanguage::Chinese);
        assert_eq!(cfg.tools.fs_base, "~/");
        let seeded: Value =
            serde_json::from_str(&tokio::fs::read_to_string(config_path()).await.unwrap()).unwrap();
        assert_eq!(seeded["tools"]["fsBase"], "~/");
        assert_eq!(
            cfg.tools.delegation.enabled_executors,
            ["pwcli", "codex", "qoder", "kimi"]
        );
        assert_eq!(
            cfg.tools.delegation.roles["engineer"].routes,
            ["cli:auto", "pwcli:engineer"]
        );
    }

    #[test]
    fn data_dir_uses_config_or_home_default() {
        assert!(resolve_data_dir_from(None).ends_with(".pwcli/data"));
        assert_eq!(
            resolve_data_dir_from(Some("/tmp/pwcli-data")),
            PathBuf::from("/tmp/pwcli-data")
        );
    }

    #[test]
    fn server_show_hidden_files_defaults_false_and_reads_camel_case_value() {
        assert!(!LocalConfig::default().server.show_hidden_files);
        let cfg = extract_typed(&serde_json::json!({
            "server": { "showHiddenFiles": true }
        }));
        assert!(cfg.server.show_hidden_files);
    }

    #[tokio::test]
    async fn roundtrip_save_load() {
        let _lock = lock_env().await;
        let _g = HomeGuard::new();

        let mut cfg = LocalConfig::default();
        cfg.user.id = "alice".into();
        cfg.tools.fs_base = "/tmp/sandbox".into();
        cfg.tools.code_agent.backend = "qoder".into();
        cfg.tools.code_agent.enabled_backends = vec!["codex".into(), "qoder".into()];
        cfg.tools.delegation.cli_priority = vec!["qoder".into(), "codex".into()];
        cfg.tools.code_agent.context_window = 200_000;
        cfg.tools.any_search.api_key = "anysearch-test-key".into();
        cfg.ai.mineru_token = "tok-xyz".into();
        cfg.ai.response_language = ResponseLanguage::English;
        cfg.memory.max_results = 9;
        cfg.features.auto_memory_extract = false;

        save(&cfg).await.expect("save");
        // wipe cache to force disk read
        *cache().write().unwrap() = LocalConfig::default();
        reload().await.expect("reload");

        let loaded = get();
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn response_language_serializes_supported_values() {
        assert_eq!(
            serde_json::to_value(ResponseLanguage::Chinese).unwrap(),
            serde_json::json!("zh-CN")
        );
        assert_eq!(
            serde_json::to_value(ResponseLanguage::English).unwrap(),
            serde_json::json!("en")
        );
        assert_eq!(
            serde_json::to_value(ResponseLanguage::Auto).unwrap(),
            serde_json::json!("auto")
        );
    }

    #[test]
    fn missing_delegation_migrates_legacy_backend_order() {
        let tools: ToolsSection = serde_json::from_value(serde_json::json!({
            "codeAgent": {
                "enabledBackends": ["qodercli", "codex", "qoder", "unknown"]
            }
        }))
        .unwrap();
        assert_eq!(
            tools.delegation.enabled_executors,
            ["pwcli", "qoder", "codex"]
        );
        assert_eq!(tools.delegation.cli_priority, ["qoder", "codex"]);
    }

    #[test]
    fn empty_legacy_backend_list_migrates_to_all_clis() {
        let tools: ToolsSection = serde_json::from_value(serde_json::json!({
            "codeAgent": { "enabledBackends": [] }
        }))
        .unwrap();
        assert_eq!(
            tools.delegation.enabled_executors,
            ["pwcli", "codex", "qoder", "kimi"]
        );
        assert_eq!(tools.delegation.cli_priority, ["codex", "qoder", "kimi"]);
    }

    #[test]
    fn explicit_delegation_is_not_replaced_by_legacy_values() {
        let tools: ToolsSection = serde_json::from_value(serde_json::json!({
            "codeAgent": { "enabledBackends": ["codex"] },
            "delegation": {
                "enabledExecutors": ["pwcli", "kimi"],
                "cliPriority": ["kimi"]
            }
        }))
        .unwrap();
        assert_eq!(tools.delegation.enabled_executors, ["pwcli", "kimi"]);
        assert_eq!(tools.delegation.cli_priority, ["kimi"]);
        assert_eq!(tools.delegation.roles["researcher"].label, "调研员");
        assert_eq!(
            tools.delegation.executor_defaults["codex"].permission_mode,
            "default"
        );
    }

    #[test]
    fn invalid_response_language_falls_back_without_losing_ai_siblings() {
        let cfg = extract_typed(&serde_json::json!({
            "ai": {
                "mineruToken": "keep-token",
                "responseLanguage": "fr"
            }
        }));
        assert_eq!(cfg.ai.response_language, ResponseLanguage::Chinese);
        assert_eq!(cfg.ai.mineru_token, "keep-token");
    }

    #[tokio::test]
    async fn update_does_not_clobber_other_fields() {
        let _lock = lock_env().await;
        let _g = HomeGuard::new();

        // Seed with everything set.
        let mut cfg = LocalConfig::default();
        cfg.tools.fs_base = "/keep/me".into();
        cfg.ai.mineru_token = "keep-token".into();
        save(&cfg).await.expect("save initial");

        // Mutate only one field.
        update(|c| {
            c.tools.code_agent.effort = "high".into();
        })
        .await
        .expect("update");

        let loaded = get();
        assert_eq!(loaded.tools.fs_base, "/keep/me");
        assert_eq!(loaded.ai.mineru_token, "keep-token");
        assert_eq!(loaded.tools.code_agent.effort, "high");
    }

    #[tokio::test]
    async fn partial_json_loads_with_defaults() {
        let _lock = lock_env().await;
        let _g = HomeGuard::new();

        // Hand-write a minimal file with only one section.
        let path = config_path();
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            &path,
            r#"{ "schemaVersion": 1, "tools": { "fsBase": "/x" } }"#,
        )
        .await
        .unwrap();

        reload().await.expect("reload");
        let cfg = get();
        assert_eq!(cfg.tools.fs_base, "/x");
        // missing sections default
        assert_eq!(cfg.memory.injection_mode, "retrieval");
        assert!(cfg.features.auto_memory_extract);
        assert_eq!(cfg.user.id, "");
    }

    #[test]
    fn tools_without_fs_base_use_portable_home_default() {
        let cfg = extract_typed(&serde_json::json!({
            "tools": { "codeAgent": { "model": "test-model" } }
        }));
        assert_eq!(cfg.tools.fs_base, "~/");
        assert_eq!(
            serde_json::to_value(ToolsSection::default()).unwrap()["fsBase"],
            "~/"
        );
    }

    #[tokio::test]
    async fn nested_moa_serializes_harness_schema() {
        let _lock = lock_env().await;
        let _g = HomeGuard::new();

        let mut cfg = LocalConfig::default();
        cfg.ai.moa = Some(MoaConfig {
            active_preset: "review".into(),
            presets: std::collections::BTreeMap::from([(
                "review".into(),
                crate::runtime::decision::MoaPreset {
                    advisors: vec![crate::runtime::decision::MoaModelRef {
                        provider: "p".into(),
                        model: "a".into(),
                    }],
                    judge: crate::runtime::decision::MoaModelRef {
                        provider: "p".into(),
                        model: "j".into(),
                    },
                    ..Default::default()
                },
            )]),
        });
        save(&cfg).await.expect("save");

        let raw = tokio::fs::read_to_string(config_path()).await.unwrap();
        assert!(raw.contains("\"activePreset\""), "raw was: {raw}");
        assert!(raw.contains("\"advisors\""), "raw was: {raw}");
        assert!(raw.contains("\"judge\""), "raw was: {raw}");
        // Top-level managed keys we own in this module.
        assert!(raw.contains("\"schemaVersion\""), "raw was: {raw}");
        assert!(raw.contains("\"autoMemoryExtract\""), "raw was: {raw}");
        // Memory section uses camelCase via our own serde annotation.
        assert!(raw.contains("\"injectionMode\""), "raw was: {raw}");
        assert!(raw.contains("\"maxDigestBytes\""), "raw was: {raw}");
        // Tools section likewise.
        assert!(raw.contains("\"fsBase\""), "raw was: {raw}");
        assert!(raw.contains("\"codeAgent\""), "raw was: {raw}");
        assert!(raw.contains("\"contextWindow\""), "raw was: {raw}");
        assert!(raw.contains("\"anySearch\""), "raw was: {raw}");
        assert!(raw.contains("\"apiKey\""), "raw was: {raw}");
    }

    #[tokio::test]
    async fn preserves_unknown_sibling_fields() {
        // Simulates RuntimeConfig having written `providers` / `permissions`
        // alongside our managed keys; we must not drop them on save.
        let _lock = lock_env().await;
        let _g = HomeGuard::new();

        let path = config_path();
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            &path,
            r#"{
  "providers": [{"name": "openai"}],
  "active_provider": "openai",
  "permissions": {"default_mode": "auto"},
  "tools": {"fsBase": "/old"}
}"#,
        )
        .await
        .unwrap();

        update(|c| {
            c.tools.fs_base = "/new".into();
        })
        .await
        .expect("update");

        let raw = tokio::fs::read_to_string(&path).await.unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        // RuntimeConfig fields preserved
        assert!(
            parsed.get("providers").is_some(),
            "providers dropped: {raw}"
        );
        assert!(
            parsed.get("active_provider").is_some(),
            "active_provider dropped"
        );
        assert!(parsed.get("permissions").is_some(), "permissions dropped");
        // Our key updated
        assert_eq!(parsed["tools"]["fsBase"], "/new");
    }

    #[test]
    fn new_users_do_not_receive_an_ssh_server_preset() {
        let tools = ToolsSection::default();
        assert!(tools.ssh_servers.is_empty());
        let serialized = serde_json::to_value(tools).unwrap();
        assert!(serialized.get("sshServers").is_none());
    }
}
