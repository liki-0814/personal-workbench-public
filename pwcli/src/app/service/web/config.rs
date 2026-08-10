use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Map, Value};

use super::super::state::AppState;
use super::WebState;

const MASK: &str = "******";
#[derive(Clone)]
pub struct ProviderEndpoint {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub protocol: String,
    pub models: Vec<Value>,
    pub use_proxy: Option<bool>,
    pub user_agent: Option<String>,
    pub compat_profile: Option<String>,
}

#[derive(Clone)]
pub struct ConfigStore {
    repository: crate::app::config::ConfigRepository,
}

impl ConfigStore {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            repository: crate::app::config::ConfigRepository::new(
                crate::app::config::RuntimeConfig::config_file(),
                data_dir,
            ),
        }
    }

    pub fn read(&self) -> Result<Value> {
        self.repository.read()
    }

    /// Atomically update the shared configuration while preserving unrelated
    /// sections. Dedicated settings APIs use this instead of round-tripping the
    /// entire config through a frontend-owned DTO.
    pub(crate) fn update<F>(&self, update: F) -> Result<()>
    where
        F: FnOnce(&mut Value) -> Result<()>,
    {
        self.repository.update(update)
    }

    pub fn revision(&self) -> Option<(std::time::SystemTime, u64)> {
        let metadata = fs::metadata(self.repository.path()).ok()?;
        Some((metadata.modified().ok()?, metadata.len()))
    }

    pub fn managed_snapshot(&self) -> Result<Value> {
        let mut snapshot = pick_managed(&self.read()?);
        mask_path(&mut snapshot, &["ai", "mineruToken"]);
        mask_path(&mut snapshot, &["tools", "anySearch", "apiKey"]);
        mask_gen_image_keys(&mut snapshot);
        mask_ssh_server_secrets(&mut snapshot);
        Ok(snapshot)
    }

    pub fn write_managed_patch(&self, incoming: Value) -> Result<()> {
        let Value::Object(_) = incoming else {
            anyhow::bail!("body must be a JSON object");
        };
        validate_managed_patch(&incoming)?;
        self.repository.update(move |existing| {
            let mut cleaned = pick_managed(&incoming);
            restore_masked_path(&mut cleaned, existing, &["ai", "mineruToken"]);
            restore_masked_path(&mut cleaned, existing, &["tools", "anySearch", "apiKey"]);
            restore_gen_image_keys(&mut cleaned, existing);
            restore_ssh_server_secrets(&mut cleaned, existing);
            remove_path(&mut cleaned, &["tools", "wikiRoot"]);
            let replace_moa = get_path(&cleaned, &["ai", "moa"]).cloned();
            let current = std::mem::take(existing);
            *existing = deep_merge(current, cleaned);
            remove_path(existing, &["tools", "wikiRoot"]);
            if let Some(moa) = replace_moa {
                set_path(existing, &["ai", "moa"], moa);
            }
            Ok(())
        })
    }

    pub fn ssh_servers(&self) -> Result<Option<Value>> {
        Ok(self
            .read()?
            .pointer("/tools/sshServers")
            .filter(|value| value.is_array())
            .cloned())
    }

    pub fn write_ssh_servers(&self, incoming: Value) -> Result<Value> {
        serde_json::from_value::<Vec<crate::runtime::tools::ssh::config::SshServer>>(
            incoming.clone(),
        )
        .context("tools.sshServers must be a valid SSH server array")?;
        let stored = incoming.clone();
        self.repository.update(move |root| {
            set_path(root, &["tools", "sshServers"], stored);
            Ok(())
        })?;
        Ok(incoming)
    }

    pub fn frontend_providers(&self, masked: bool) -> Result<Value> {
        let root = self.read()?;
        let providers = root
            .get("providers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|provider| {
                let name = provider.get("name")?.as_str()?.to_string();
                let base_url = provider
                    .get("base_url")
                    .or_else(|| provider.get("baseUrl"))?
                    .as_str()?
                    .to_string();
                let key = provider
                    .get("api_key")
                    .or_else(|| provider.get("apiKey"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let api_key = if masked { mask_secret(key) } else { key.to_string() };
                Some(json!({
                    "id": provider.get("id").and_then(Value::as_str).unwrap_or_default(),
                    "name": name,
                    "baseUrl": base_url,
                    "apiKey": api_key,
                    "protocol": provider.get("protocol").and_then(Value::as_str).unwrap_or("openai"),
                    "models": provider.get("models").and_then(Value::as_array).into_iter().flatten()
                        .filter(|model| !crate::runtime::image_generation::is_image_model(model))
                        .cloned().collect::<Vec<_>>(),
                }))
            })
            .collect::<Vec<_>>();
        Ok(Value::Array(providers))
    }

    pub fn provider(&self, index: usize) -> Result<ProviderEndpoint> {
        let root = self.read()?;
        let provider = root
            .get("providers")
            .and_then(Value::as_array)
            .and_then(|providers| providers.get(index))
            .context("Provider not configured. Please set up AI Provider in settings.")?;
        provider_endpoint(provider)
    }

    pub fn provider_by_id(&self, id: &str) -> Result<ProviderEndpoint> {
        let root = self.read()?;
        let provider = root
            .get("providers")
            .and_then(Value::as_array)
            .and_then(|providers| {
                providers.iter().find(|provider| {
                    provider.get("id").and_then(Value::as_str) == Some(id)
                        || provider.get("name").and_then(Value::as_str) == Some(id)
                })
            })
            .context("Provider not configured. Please set up AI Provider in settings.")?;
        provider_endpoint(provider)
    }

    pub fn providers(&self) -> Result<Vec<ProviderEndpoint>> {
        let count = self
            .read()?
            .get("providers")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        (0..count).map(|index| self.provider(index)).collect()
    }
}

fn provider_endpoint(provider: &Value) -> Result<ProviderEndpoint> {
    Ok(ProviderEndpoint {
        id: provider
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| text(provider, "name"))
            .to_string(),
        name: text(provider, "name").to_string(),
        base_url: provider
            .get("base_url")
            .or_else(|| provider.get("baseUrl"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string(),
        api_key: provider
            .get("api_key")
            .or_else(|| provider.get("apiKey"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        protocol: text(provider, "protocol").to_string(),
        models: provider
            .get("models")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        use_proxy: provider
            .get("useProxy")
            .or_else(|| provider.get("use_proxy"))
            .and_then(Value::as_bool),
        user_agent: provider
            .get("userAgent")
            .or_else(|| provider.get("user_agent"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        compat_profile: provider
            .get("compatProfile")
            .or_else(|| provider.get("compat_profile"))
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

impl ConfigStore {
    pub fn write_frontend_providers(&self, incoming: Value) -> Result<Value> {
        incoming.as_array().context("providers must be an array")?;
        self.repository.update(move |root| {
            let submitted = incoming
                .as_array()
                .expect("providers validated before repository update");
            let previous = root
                .get("providers")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut providers = Vec::new();
            let mut seen_ids = std::collections::HashSet::new();
            for provider in submitted {
                let name = text(provider, "name").trim().to_string();
                let base_url = text(provider, "baseUrl").trim().to_string();
                if name.is_empty() || base_url.is_empty() {
                    continue;
                }
                let submitted_id = text(provider, "id").trim();
                let prior = find_previous_provider(&previous, submitted_id, &name, &base_url)?;
                let provider_id = prior
                    .and_then(|value| value.get("id"))
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .or_else(|| (!submitted_id.is_empty()).then_some(submitted_id))
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("provider_{}", uuid::Uuid::now_v7().simple()));
                if !seen_ids.insert(provider_id.clone()) {
                    anyhow::bail!("provider id must be unique");
                }
                let submitted_key = text(provider, "apiKey");
                let prior_key = prior
                    .and_then(|value| {
                        value
                            .get("api_key")
                            .or_else(|| value.get("apiKey"))
                    })
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let api_key = if looks_masked(submitted_key) {
                    if prior_key.is_empty() {
                        anyhow::bail!(
                            "masked apiKey cannot be restored for provider `{name}`; enter the key again"
                        );
                    }
                    prior_key.to_string()
                } else {
                    submitted_key.to_string()
                };
                let models = normalize_models(provider.get("models"))
                    .into_iter()
                    .filter(|model| !crate::runtime::image_generation::is_image_model(model))
                    .collect::<Vec<_>>();
                let prior_model = prior
                    .and_then(|value| value.get("model"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let model = if models.iter().any(|entry| entry["id"] == prior_model) {
                    prior_model.to_string()
                } else {
                    models
                        .first()
                        .and_then(|entry| entry.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                providers.push(json!({
                    "id": provider_id,
                    "name": name,
                    "base_url": base_url,
                    "api_key": api_key,
                    "protocol": normalize_provider_protocol(text(provider, "protocol")),
                    "model": model,
                    "models": models,
                }));
            }
            let active = root
                .get("active_provider")
                .and_then(Value::as_str)
                .filter(|active| providers.iter().any(|provider| provider["name"] == *active))
                .map(str::to_string)
                .or_else(|| {
                    providers
                        .first()
                        .and_then(|provider| provider["name"].as_str())
                        .map(str::to_string)
                });
            root["providers"] = Value::Array(providers);
            root["active_provider"] = active.map(Value::String).unwrap_or(Value::Null);
            Ok(())
        })?;
        self.frontend_providers(true)
    }

    pub fn frontend_app_config(&self) -> Result<Value> {
        let root = self.read()?;
        Ok(json!({
            "showHiddenFiles": bool_text(get_path(&root, &["server", "showHiddenFiles"])),
            "codeAgentThinking": bool_text(get_path(&root, &["tools", "codeAgent", "thinking"])),
            "codeAgentFast": bool_text(get_path(&root, &["tools", "codeAgent", "fast"])),
        }))
    }

    pub fn write_frontend_app_config(&self, incoming: Value) -> Result<Value> {
        let submitted = incoming
            .as_object()
            .context("app_config must be an object")?;
        let submitted = submitted.clone();
        self.repository.update(move |root| {
            if let Some(value) = submitted.get("showHiddenFiles") {
                set_path(
                    root,
                    &["server", "showHiddenFiles"],
                    json!(value_to_text(value) == "true"),
                );
            }
            if let Some(value) = submitted.get("codeAgentThinking") {
                set_path(
                    root,
                    &["tools", "codeAgent", "thinking"],
                    json!(value_to_text(value) == "true"),
                );
            }
            if let Some(value) = submitted.get("codeAgentFast") {
                set_path(
                    root,
                    &["tools", "codeAgent", "fast"],
                    json!(value_to_text(value) == "true"),
                );
            }
            Ok(())
        })?;
        self.frontend_app_config()
    }
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/local-config", get(get_config).put(put_config))
}

async fn get_config(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let web = web(&state)?;
    let data = web.config.managed_snapshot().map_err(internal)?;
    Ok(Json(json!({ "success": true, "data": data })))
}

async fn put_config(
    State(state): State<AppState>,
    Json(incoming): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let web = web(&state)?;
    if let Err(cause) = validate_managed_patch(&incoming) {
        return Err(error(StatusCode::BAD_REQUEST, &cause.to_string()));
    }
    web.config.write_managed_patch(incoming).map_err(internal)?;
    crate::app::config::local_config::reload()
        .await
        .map_err(internal)?;
    web.events.publish(vec!["local_config".to_string()]);
    Ok(Json(json!({ "success": true })))
}

fn validate_managed_patch(incoming: &Value) -> Result<()> {
    if let Some(value) = incoming.pointer("/tools/fsBase") {
        let configured = value
            .as_str()
            .context("tools.fsBase must be a directory path")?;
        crate::runtime::tools::fs_local::FsSandbox::from_root_setting(configured)
            .context("tools.fsBase must be an accessible directory")?;
    }
    if let Some(value) = incoming.pointer("/ai/responseLanguage") {
        match value.as_str() {
            Some("zh-CN" | "en" | "auto") => {}
            _ => anyhow::bail!("ai.responseLanguage must be one of: zh-CN, en, auto"),
        }
    }
    if let Some(value) = incoming.pointer("/ai/responseVerbosity") {
        match value.as_str() {
            Some("low" | "medium" | "high") => {}
            _ => anyhow::bail!("ai.responseVerbosity must be one of: low, medium, high"),
        }
    }
    if let Some(value) = incoming.pointer("/permissions/agent_mode") {
        match value.as_str() {
            Some("prompt" | "risk" | "full") => {}
            _ => anyhow::bail!("permissions.agent_mode must be one of: prompt, risk, full"),
        }
    }
    if let Some(value) = incoming.pointer("/tools/sshServers") {
        serde_json::from_value::<Vec<crate::runtime::tools::ssh::config::SshServer>>(value.clone())
            .context("tools.sshServers must be a valid SSH server array")?;
    }
    if let Some(value) = incoming.pointer("/tools/delegation") {
        serde_json::from_value::<crate::app::config::local_config::DelegationSection>(
            value.clone(),
        )
        .context("tools.delegation must be a valid delegation configuration")?;
    }
    Ok(())
}

pub fn mask_frontend_providers(value: Value) -> Value {
    let Value::Array(providers) = value else {
        return value;
    };
    Value::Array(
        providers
            .into_iter()
            .map(|mut provider| {
                if let Some(key) = provider.get("apiKey").and_then(Value::as_str) {
                    let masked = mask_secret(key);
                    provider["apiKey"] = Value::String(masked);
                }
                provider
            })
            .collect(),
    )
}

fn pick_managed(root: &Value) -> Value {
    let mut output = Map::new();
    output.insert(
        "schemaVersion".to_string(),
        root.get("schemaVersion")
            .cloned()
            .filter(Value::is_number)
            .unwrap_or_else(|| json!(crate::app::config::CONFIG_SCHEMA_VERSION)),
    );
    copy_section(
        root,
        &mut output,
        "ai",
        &[
            "moa",
            "mineruToken",
            "responseLanguage",
            "responseVerbosity",
        ],
    );
    copy_section(
        root,
        &mut output,
        "tools",
        &[
            "fsBase",
            "codeAgent",
            "delegation",
            "anySearch",
            "genImage",
            "sshServers",
        ],
    );
    copy_section(
        root,
        &mut output,
        "memory",
        &[
            "injectionMode",
            "autoRetrieve",
            "maxResults",
            "maxDigestBytes",
            "dreamEnabled",
            "dreamProject",
        ],
    );
    copy_section(
        root,
        &mut output,
        "server",
        &[
            "backendPort",
            "dataDir",
            "showHiddenFiles",
            "backupIntervalMs",
        ],
    );
    copy_section(
        root,
        &mut output,
        "frontend",
        &["backendUrl", "faviconServiceUrl"],
    );
    if let Some(mode) = get_path(root, &["permissions", "agent_mode"]) {
        output
            .entry("permissions".to_string())
            .or_insert_with(|| json!({}))["agent_mode"] =
            Value::String(mode.as_str().unwrap_or("risk").to_string());
    }
    if let Some(rules) = get_path(root, &["permissions", "allow_rules"]) {
        output
            .entry("permissions".to_string())
            .or_insert_with(|| json!({}))["allow_rules"] = sanitize_rules(rules);
    }
    if let Some(value) = get_path(root, &["features", "autoMemoryExtract"]) {
        output.insert(
            "features".to_string(),
            json!({ "autoMemoryExtract": value.as_bool().unwrap_or(false) }),
        );
    }
    Value::Object(output)
}

fn copy_section(root: &Value, output: &mut Map<String, Value>, name: &str, keys: &[&str]) {
    let Some(source) = root.get(name).and_then(Value::as_object) else {
        return;
    };
    let section = keys
        .iter()
        .filter_map(|key| {
            source
                .get(*key)
                .cloned()
                .map(|value| ((*key).to_string(), value))
        })
        .collect::<Map<_, _>>();
    if !section.is_empty() {
        output.insert(name.to_string(), Value::Object(section));
    }
}

fn sanitize_rules(value: &Value) -> Value {
    Value::Array(
        value
            .as_array()
            .into_iter()
            .flatten()
            .take(500)
            .filter(|rule| {
                text(rule, "id").len() <= 256
                    && !text(rule, "id").is_empty()
                    && !text(rule, "tool").is_empty()
                    && !text(rule, "cwd").is_empty()
                    && text(rule, "arguments").len() <= 100_000
            })
            .cloned()
            .collect(),
    )
}

fn normalize_models(value: Option<&Value>) -> Vec<Value> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = text(model, "id").trim();
            if id.is_empty() {
                return None;
            }
            let mut normalized = Map::new();
            normalized.insert("id".to_string(), json!(id));
            let name = text(model, "name").trim();
            normalized.insert(
                "name".to_string(),
                Value::String(if name.is_empty() { id } else { name }.to_string()),
            );
            for key in [
                "enabled",
                "maxOutput",
                "contextWindow",
                "capabilities",
                "requestParams",
                "thinkingParams",
                "deferredToolsMode",
            ] {
                if let Some(value) = model.get(key) {
                    normalized.insert(key.to_string(), value.clone());
                }
            }
            Some(Value::Object(normalized))
        })
        .collect()
}

fn normalize_provider_protocol(value: &str) -> String {
    let key = value.trim().to_ascii_lowercase().replace('-', "_");
    match key.as_str() {
        "openai" | "openai_chat" | "openai_compatible" => "openai_chat".to_string(),
        "openai_responses" | "responses" => "openai_responses".to_string(),
        "anthropic" | "anthropic_messages" => "anthropic_messages".to_string(),
        "google" | "gemini" | "google_generative" | "generative_language" => {
            "google_generative".to_string()
        }
        other if !other.is_empty() => other.to_string(),
        _ => "openai_chat".to_string(),
    }
}

fn find_previous_provider<'a>(
    previous: &'a [Value],
    submitted_id: &str,
    name: &str,
    base_url: &str,
) -> Result<Option<&'a Value>> {
    let matches = previous
        .iter()
        .filter(|candidate| {
            if !submitted_id.is_empty() {
                candidate.get("id").and_then(Value::as_str) == Some(submitted_id)
            } else {
                text(candidate, "name").trim() == name
                    && candidate
                        .get("base_url")
                        .or_else(|| candidate.get("baseUrl"))
                        .and_then(Value::as_str)
                        .is_some_and(|value| value.trim() == base_url)
            }
        })
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        anyhow::bail!("provider identity must be unique");
    }
    Ok(matches.into_iter().next())
}

fn deep_merge(target: Value, source: Value) -> Value {
    match (target, source) {
        (Value::Object(mut target), Value::Object(source)) => {
            for (key, value) in source {
                let current = target.remove(&key).unwrap_or(Value::Null);
                target.insert(key, deep_merge(current, value));
            }
            Value::Object(target)
        }
        (_, source) => source,
    }
}

fn get_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
}

fn set_path(root: &mut Value, path: &[&str], value: Value) {
    if path.is_empty() {
        *root = value;
        return;
    }
    if !root.is_object() {
        *root = json!({});
    }
    let mut current = root;
    for key in &path[..path.len() - 1] {
        if current.get(*key).is_none_or(|value| !value.is_object()) {
            current[*key] = json!({});
        }
        current = &mut current[*key];
    }
    current[path[path.len() - 1]] = value;
}

fn remove_path(root: &mut Value, path: &[&str]) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut current = root;
    for key in parents {
        let Some(next) = current.get_mut(*key) else {
            return;
        };
        current = next;
    }
    if let Some(object) = current.as_object_mut() {
        object.remove(*last);
    }
}

fn mask_path(root: &mut Value, path: &[&str]) {
    if get_path(root, path)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
    {
        set_path(root, path, Value::String(MASK.to_string()));
    }
}

fn restore_masked_path(patch: &mut Value, existing: &Value, path: &[&str]) {
    if get_path(patch, path).and_then(Value::as_str) != Some(MASK) {
        return;
    }
    match get_path(existing, path).and_then(Value::as_str) {
        Some(value) if !value.is_empty() && value != MASK => {
            set_path(patch, path, Value::String(value.to_string()));
        }
        _ => remove_path(patch, path),
    }
}

fn mask_gen_image_keys(root: &mut Value) {
    let Some(models) = root
        .pointer_mut("/tools/genImage/models")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for model in models {
        if model
            .get("apiKey")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        {
            model["apiKey"] = Value::String(MASK.to_string());
        }
    }
}

fn restore_gen_image_keys(patch: &mut Value, existing: &Value) {
    let previous = existing
        .pointer("/tools/genImage/models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(models) = patch
        .pointer_mut("/tools/genImage/models")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for (index, model) in models.iter_mut().enumerate() {
        if model.get("apiKey").and_then(Value::as_str) != Some(MASK) {
            continue;
        }
        let id = model.get("id").and_then(Value::as_str);
        if let Some(api_key) = previous
            .iter()
            .find(|candidate| candidate.get("id").and_then(Value::as_str) == id)
            .or_else(|| previous.get(index))
            .and_then(|candidate| candidate.get("apiKey"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && *value != MASK)
        {
            model["apiKey"] = Value::String(api_key.to_string());
        } else if let Some(object) = model.as_object_mut() {
            object.remove("apiKey");
        }
    }
}

fn mask_ssh_server_secrets(root: &mut Value) {
    let Some(servers) = root
        .pointer_mut("/tools/sshServers")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for server in servers {
        let Some(auth) = server.get_mut("auth") else {
            continue;
        };
        for key in ["value", "passphrase"] {
            if auth
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
            {
                auth[key] = Value::String(MASK.to_string());
            }
        }
    }
}

fn restore_ssh_server_secrets(patch: &mut Value, existing: &Value) {
    let previous = existing
        .pointer("/tools/sshServers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(servers) = patch
        .pointer_mut("/tools/sshServers")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for (index, server) in servers.iter_mut().enumerate() {
        let id = server.get("id").and_then(Value::as_str).map(str::to_string);
        let prior = previous
            .iter()
            .find(|candidate| candidate.get("id").and_then(Value::as_str) == id.as_deref())
            .or_else(|| previous.get(index));
        let Some(auth) = server.get_mut("auth") else {
            continue;
        };
        for key in ["value", "passphrase"] {
            if auth.get(key).and_then(Value::as_str) != Some(MASK) {
                continue;
            }
            if let Some(secret) = prior
                .and_then(|candidate| candidate.get("auth"))
                .and_then(|candidate| candidate.get(key))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty() && *value != MASK)
            {
                auth[key] = Value::String(secret.to_string());
            } else if let Some(object) = auth.as_object_mut() {
                object.remove(key);
            }
        }
    }
}

fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else if value.len() <= 8 {
        MASK.to_string()
    } else {
        format!("{}****{}", &value[..4], &value[value.len() - 4..])
    }
}

fn looks_masked(value: &str) -> bool {
    value.is_empty() || value.contains("****") || value == MASK
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn value_to_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::trim)
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn bool_text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_bool)
        .map(|value| value.to_string())
        .unwrap_or_default()
}

fn web(state: &AppState) -> Result<&Arc<WebState>, (StatusCode, Json<Value>)> {
    state
        .web
        .as_ref()
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "Web service is disabled"))
}

fn internal(cause: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    error(StatusCode::INTERNAL_SERVER_ERROR, &cause.to_string())
}

fn error(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "success": false, "error": message })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_fs_base_before_persisting_config() {
        let directory = tempfile::tempdir().unwrap();
        assert!(validate_managed_patch(&json!({
            "tools": { "fsBase": directory.path() }
        }))
        .is_ok());
        assert!(validate_managed_patch(&json!({
            "tools": { "fsBase": directory.path().join("missing") }
        }))
        .is_err());
        assert!(validate_managed_patch(&json!({
            "tools": { "fsBase": 42 }
        }))
        .is_err());
    }

    #[test]
    fn managed_snapshot_excludes_provider_secrets_and_masks_tokens() {
        let root = json!({
            "providers": [{ "api_key": "secret" }],
            "ai": { "mineruToken": "secret", "responseLanguage": "en", "ignored": true },
            "tools": {
                "fsBase": "~/",
                "delegation": {
                    "enabledExecutors": ["pwcli", "qoder"],
                    "cliPriority": ["qoder"]
                },
                "anySearch": { "apiKey": "secret" },
                "genImage": { "models": [{ "id": "qwen", "apiKey": "qwen-secret" }] },
                "sshServers": [
                    {
                        "id": "password-host",
                        "auth": { "type": "password", "value": "ssh-password" }
                    },
                    {
                        "id": "key-host",
                        "auth": { "type": "key", "path": "~/.ssh/id_rsa", "passphrase": "ssh-passphrase" }
                    }
                ]
            },
        });
        let mut snapshot = pick_managed(&root);
        mask_path(&mut snapshot, &["ai", "mineruToken"]);
        mask_path(&mut snapshot, &["tools", "anySearch", "apiKey"]);
        mask_gen_image_keys(&mut snapshot);
        mask_ssh_server_secrets(&mut snapshot);
        assert!(snapshot.get("providers").is_none());
        assert_eq!(snapshot["ai"]["mineruToken"], MASK);
        assert_eq!(snapshot["ai"]["responseLanguage"], "en");
        assert_eq!(
            snapshot["tools"]["delegation"]["enabledExecutors"],
            json!(["pwcli", "qoder"])
        );
        assert_eq!(snapshot["tools"]["anySearch"]["apiKey"], MASK);
        assert_eq!(snapshot["tools"]["genImage"]["models"][0]["apiKey"], MASK);
        assert_eq!(snapshot["tools"]["sshServers"][0]["auth"]["value"], MASK);
        assert_eq!(
            snapshot["tools"]["sshServers"][1]["auth"]["passphrase"],
            MASK
        );
    }

    #[test]
    fn deep_merge_preserves_unknown_siblings() {
        let merged = deep_merge(
            json!({ "tools": { "fsBase": "old", "unknown": 1 } }),
            json!({ "tools": { "fsBase": "new" } }),
        );
        assert_eq!(merged["tools"]["fsBase"], "new");
        assert_eq!(merged["tools"]["unknown"], 1);
    }

    #[test]
    fn restores_masked_gen_image_keys_by_stable_id() {
        let existing = json!({
            "tools": { "genImage": { "models": [
                { "id": "qwen", "apiKey": "qwen-secret" },
                { "id": "gemini", "apiKey": "gemini-secret" }
            ] } }
        });
        let mut patch = json!({
            "tools": { "genImage": { "models": [
                { "id": "gemini", "apiKey": MASK },
                { "id": "qwen", "apiKey": MASK }
            ] } }
        });
        restore_gen_image_keys(&mut patch, &existing);
        assert_eq!(
            patch["tools"]["genImage"]["models"][0]["apiKey"],
            "gemini-secret"
        );
        assert_eq!(
            patch["tools"]["genImage"]["models"][1]["apiKey"],
            "qwen-secret"
        );
    }

    #[test]
    fn restores_masked_ssh_secrets_by_stable_id() {
        let existing = json!({
            "tools": { "sshServers": [
                {
                    "id": "key-host",
                    "auth": { "type": "key", "path": "~/.ssh/id_ed25519", "passphrase": "key-secret" }
                },
                {
                    "id": "password-host",
                    "auth": { "type": "password", "value": "password-secret" }
                }
            ] }
        });
        let mut patch = json!({
            "tools": { "sshServers": [
                {
                    "id": "password-host",
                    "auth": { "type": "password", "value": MASK }
                },
                {
                    "id": "key-host",
                    "auth": { "type": "key", "path": "~/.ssh/id_ed25519", "passphrase": MASK }
                }
            ] }
        });
        restore_ssh_server_secrets(&mut patch, &existing);
        assert_eq!(
            patch["tools"]["sshServers"][0]["auth"]["value"],
            "password-secret"
        );
        assert_eq!(
            patch["tools"]["sshServers"][1]["auth"]["passphrase"],
            "key-secret"
        );
    }

    #[test]
    fn rejects_invalid_ssh_server_config() {
        assert!(validate_managed_patch(&json!({
            "tools": {
                "sshServers": [{
                    "id": "missing-host",
                    "alias": "dev",
                    "port": 22,
                    "user": "alice",
                    "auth": { "type": "key", "path": "~/.ssh/id_ed25519" }
                }]
            }
        }))
        .is_err());
    }

    #[test]
    fn validates_partial_delegation_patch_with_defaults() {
        assert!(validate_managed_patch(&json!({
            "tools": {
                "delegation": {
                    "enabledExecutors": ["pwcli", "kimi"],
                    "cliPriority": ["kimi"]
                }
            }
        }))
        .is_ok());
        assert!(validate_managed_patch(&json!({
            "tools": { "delegation": { "cliPriority": "kimi" } }
        }))
        .is_err());
    }

    #[test]
    fn provider_reordering_preserves_keys_by_stable_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "providers": [
                    {
                        "id": "provider-a",
                        "name": "A",
                        "base_url": "https://a.example",
                        "api_key": "secret-a",
                        "models": []
                    },
                    {
                        "id": "provider-b",
                        "name": "B",
                        "base_url": "https://b.example",
                        "api_key": "secret-b",
                        "models": []
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let store = ConfigStore {
            repository: crate::app::config::ConfigRepository::new(&path, dir.path().join("data")),
        };

        store
            .write_frontend_providers(json!([
                {
                    "id": "provider-b",
                    "name": "B",
                    "baseUrl": "https://b.example",
                    "apiKey": MASK,
                    "models": []
                },
                {
                    "id": "provider-a",
                    "name": "A",
                    "baseUrl": "https://a.example",
                    "apiKey": MASK,
                    "models": []
                }
            ]))
            .unwrap();

        let providers = store.read().unwrap()["providers"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(providers[0]["id"], "provider-b");
        assert_eq!(providers[0]["api_key"], "secret-b");
        assert_eq!(providers[1]["id"], "provider-a");
        assert_eq!(providers[1]["api_key"], "secret-a");
    }

    #[test]
    fn provider_write_preserves_canonical_protocols() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let store = ConfigStore {
            repository: crate::app::config::ConfigRepository::new(&path, dir.path().join("data")),
        };

        store
            .write_frontend_providers(json!([{
                "id": "responses",
                "name": "Responses",
                "baseUrl": "https://api.example.test/v1",
                "apiKey": "secret",
                "protocol": "openai_responses",
                "models": [{ "id": "model", "name": "Model" }]
            }]))
            .unwrap();

        assert_eq!(
            store.read().unwrap()["providers"][0]["protocol"],
            "openai_responses"
        );
    }

    #[test]
    fn legacy_provider_matching_is_identity_based_and_fails_closed() {
        let previous = vec![
            json!({
                "name": "A",
                "base_url": "https://a.example",
                "api_key": "secret-a"
            }),
            json!({
                "name": "B",
                "base_url": "https://b.example",
                "api_key": "secret-b"
            }),
        ];
        assert_eq!(
            find_previous_provider(&previous, "", "B", "https://b.example")
                .unwrap()
                .unwrap()["api_key"],
            "secret-b"
        );
        assert!(
            find_previous_provider(&previous, "", "Renamed", "https://b.example")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unmatched_masked_provider_is_rejected_without_overwriting_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(
            &path,
            r#"{"providers":[{"name":"A","base_url":"https://a.example","api_key":"secret-a"}]}"#,
        )
        .unwrap();
        let store = ConfigStore {
            repository: crate::app::config::ConfigRepository::new(&path, dir.path().join("data")),
        };

        assert!(store
            .write_frontend_providers(json!([{
                "name": "B",
                "baseUrl": "https://b.example",
                "apiKey": MASK,
                "models": []
            }]))
            .is_err());
        assert_eq!(store.read().unwrap()["providers"][0]["api_key"], "secret-a");
    }

    #[test]
    fn validates_response_language() {
        assert!(validate_managed_patch(&json!({
            "ai": { "responseLanguage": "zh-CN" }
        }))
        .is_ok());
        assert!(validate_managed_patch(&json!({
            "ai": { "responseLanguage": "en" }
        }))
        .is_ok());
        assert!(validate_managed_patch(&json!({
            "ai": { "responseLanguage": "auto" }
        }))
        .is_ok());
        assert!(validate_managed_patch(&json!({
            "ai": { "responseLanguage": "fr" }
        }))
        .is_err());
    }

    #[test]
    fn validates_response_verbosity() {
        for verbosity in ["low", "medium", "high"] {
            assert!(validate_managed_patch(&json!({
                "ai": { "responseVerbosity": verbosity }
            }))
            .is_ok());
        }
        assert!(validate_managed_patch(&json!({
            "ai": { "responseVerbosity": "verbose" }
        }))
        .is_err());
    }

    #[test]
    fn validates_agent_permission_mode() {
        for mode in ["prompt", "risk", "full"] {
            assert!(validate_managed_patch(&json!({
                "permissions": { "agent_mode": mode }
            }))
            .is_ok());
        }
        assert!(validate_managed_patch(&json!({
            "permissions": { "agent_mode": "unsafe" }
        }))
        .is_err());
    }
}
