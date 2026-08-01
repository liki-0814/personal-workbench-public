use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const EXTENSION_HOST_SOURCE: &str = include_str!("../resources/extension-host.mjs");

#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[serde(default)]
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

pub async fn run_stdio() -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<RpcRequest>(&line) {
            Ok(request) => handle(request).await,
            Err(error) => RpcResponse {
                id: Value::Null,
                result: None,
                error: Some(RpcError {
                    code: -32700,
                    message: error.to_string(),
                }),
            },
        };
        stdout
            .write_all(format!("{}\n", serde_json::to_string(&response)?).as_bytes())
            .await?;
        stdout.flush().await?;
    }
    Ok(())
}

async fn handle(request: RpcRequest) -> RpcResponse {
    let id = request.id;
    match dispatch(&request.method, request.params).await {
        Ok(result) => RpcResponse {
            id,
            result: Some(result),
            error: None,
        },
        Err(error) => RpcResponse {
            id,
            result: None,
            error: Some(RpcError {
                code: -32000,
                message: error.to_string(),
            }),
        },
    }
}

async fn dispatch(method: &str, params: Value) -> Result<Value> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": 1,
            "capabilities": ["memory", "resources", "extensions"]
        })),
        "memory/list" => {
            let store = current_store()?;
            Ok(json!({ "index": store.read_index_raw()? }))
        }
        "memory/search" => {
            let query = params
                .get("query")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("query is required"))?;
            let hits = crate::memory::hybrid_search(
                &current_store()?,
                crate::memory::HybridSearchOptions {
                    query: query.to_string(),
                    max_results: params
                        .get("maxResults")
                        .and_then(Value::as_u64)
                        .unwrap_or(10) as usize,
                    exclude_slugs: Default::default(),
                    candidate_top_n: Some(20),
                    include_archived: true,
                },
            )?;
            Ok(serde_json::to_value(
                hits.into_iter()
                    .map(
                        |hit| json!({"slug": hit.slug, "summary": hit.summary, "score": hit.score}),
                    )
                    .collect::<Vec<_>>(),
            )?)
        }
        "resources/diagnostics" | "resources/reload" => {
            if method.ends_with("reload") {
                crate::skills::init();
            }
            let skills = crate::skills::load_skills();
            Ok(json!({
                "skills": skills.len(),
                "memoryRoot": current_store()?.base_dir()
            }))
        }
        "extensions/list" => extension_host(json!({ "method": "list" })).await,
        "extensions/invokeTool" => {
            extension_host(json!({
                "method": "invokeTool",
                "extension": params.get("extension"),
                "name": params.get("name"),
                "args": params.get("args").cloned().unwrap_or_else(|| json!({}))
            }))
            .await
        }
        "extensions/invokeCommand" => {
            extension_host(json!({
                "method": "invokeCommand",
                "extension": params.get("extension"),
                "name": params.get("name"),
                "args": params.get("args").cloned().unwrap_or_else(|| json!({}))
            }))
            .await
        }
        "extensions/event" => {
            extension_host(json!({
                "method": "event",
                "extension": params.get("extension"),
                "event": params.get("event"),
                "payload": params.get("payload").cloned().unwrap_or(Value::Null)
            }))
            .await
        }
        _ => anyhow::bail!("method not found: {}", method),
    }
}

pub(crate) async fn extension_host(request: Value) -> Result<Value> {
    let script = materialize_extension_host(&crate::config::local_config::data_dir())?;
    extension_host_with_script(request, &script).await
}

fn materialize_extension_host(data_dir: &Path) -> Result<PathBuf> {
    let directory = data_dir.join("runtime");
    std::fs::create_dir_all(&directory)?;
    let script = directory.join("extension-host.mjs");
    let current = std::fs::read_to_string(&script).ok();
    if current.as_deref() != Some(EXTENSION_HOST_SOURCE) {
        std::fs::write(&script, EXTENSION_HOST_SOURCE)?;
    }
    Ok(script)
}

async fn extension_host_with_script(request: Value, script: &Path) -> Result<Value> {
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    let mut child = Command::new("node")
        .arg(script)
        .current_dir(std::env::current_dir()?)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("extension host stdin unavailable"))?
        .write_all(serde_json::to_string(&request)?.as_bytes())
        .await?;
    drop(child.stdin.take());
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!("extension host timed out after 30s"))??;
    let response: Value = serde_json::from_slice(&output.stdout)?;
    if let Some(error) = response.get("error").and_then(Value::as_str) {
        anyhow::bail!("extension host: {}", error);
    }
    Ok(response.get("result").cloned().unwrap_or(Value::Null))
}

fn current_store() -> Result<crate::memory::MemoryStore> {
    let config = crate::config::RuntimeConfig::load();
    let slug = config
        .user
        .and_then(|user| user.slug)
        .unwrap_or_else(|| "local".to_string());
    Ok(crate::memory::MemoryStore::new(&slug)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initialize_advertises_stable_capabilities() {
        let value = dispatch("initialize", Value::Null).await.unwrap();
        assert_eq!(value["protocolVersion"], 1);
        assert!(value["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("resources")));
    }

    #[tokio::test]
    async fn unknown_method_is_rejected() {
        assert!(dispatch("missing", Value::Null).await.is_err());
    }

    #[tokio::test]
    async fn node_extension_host_lists_extensions() {
        let directory = tempfile::tempdir().unwrap();
        let script = materialize_extension_host(directory.path()).unwrap();
        let value = extension_host_with_script(json!({ "method": "list" }), &script)
            .await
            .unwrap();
        assert!(value.is_array());
    }

    #[test]
    fn extension_host_is_materialized_below_data_dir() {
        let directory = tempfile::tempdir().unwrap();
        let script = materialize_extension_host(directory.path()).unwrap();
        assert_eq!(script, directory.path().join("runtime/extension-host.mjs"));
        assert_eq!(
            std::fs::read_to_string(script).unwrap(),
            EXTENSION_HOST_SOURCE
        );
    }
}
