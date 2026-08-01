//! ratatui client transport for the persistent local daemon.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::llm::ChatMessage;
use crate::tui_app::{StatusUpdate, UiEvent, UserSubmission};

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureWorkItemResponse {
    pub client_request_id: String,
    pub work_item_id: String,
    pub session_id: String,
    pub queued_input_id: String,
    pub runtime_task_id: Option<String>,
    pub created: bool,
}

pub async fn capture_work_item(
    prompt: String,
    cwd: &std::path::Path,
    executor_preference: Option<&str>,
) -> Result<(CaptureWorkItemResponse, String)> {
    crate::config::local_config::init().await;
    crate::daemon::start().await?;
    let daemon_url = crate::daemon::status().await.http_address;
    let response = reqwest::Client::new()
        .post(format!("{daemon_url}/api/agent/work-items/capture"))
        .json(&json!({
            "clientRequestId": format!("capture_{}", uuid::Uuid::now_v7().simple()),
            "prompt": prompt,
            "cwd": cwd.to_string_lossy(),
            "executorPreference": executor_preference,
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<CaptureWorkItemResponse>()
        .await
        .context("decode work-item capture response")?;
    Ok((response, daemon_url))
}

#[derive(Debug, Deserialize)]
struct CreateSessionResponse {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionSnapshot {
    id: String,
    name: String,
    messages: Vec<ChatMessage>,
    estimated_tokens: u32,
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttentionItem {
    id: String,
    task_id: String,
    kind: String,
    version: i64,
    title: String,
    detail: Value,
}

pub async fn fetch_daemon_sessions(daemon_url: &str) -> Result<Vec<(String, String)>> {
    reqwest::Client::new()
        .get(format!("{daemon_url}/api/agent/sessions"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("decode daemon sessions")
}

pub async fn run_daemon_oneshot(prompt: String) -> Result<()> {
    crate::config::local_config::init().await;
    crate::daemon::start().await?;
    let daemon_url = crate::daemon::status().await.http_address;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60 * 60))
        .build()?;
    let mut session = select_or_create_tui_session(&client, &daemon_url).await?;
    session.messages.push(ChatMessage {
        role: "user".into(),
        content: prompt,
        images: Vec::new(),
        generated_images: Vec::new(),
        tool_calls: None,
        tool_call_id: None,
    });
    let response = client
        .post(format!(
            "{daemon_url}/api/agent/sessions/{}/chat",
            session.id
        ))
        .json(&json!({
            "messages": session.messages,
            "system_prompt": null,
            "thinking": false,
            "cwd": std::env::current_dir()?.to_string_lossy(),
            "session_name": session.name,
            "require_permission_approval": false
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    println!("{}", response["content"].as_str().unwrap_or_default());
    Ok(())
}

pub async fn run_daemon_client_task(
    mut input_rx: mpsc::Receiver<UserSubmission>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
    daemon_url: String,
    context_window: u32,
) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(60 * 60))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            let _ = ui_tx.send(UiEvent::Error(error.to_string()));
            return;
        }
    };
    let mut session = match select_or_create_tui_session(&client, &daemon_url).await {
        Ok(session) => session,
        Err(error) => {
            let _ = ui_tx.send(UiEvent::Error(format!("连接 daemon 失败：{error:#}")));
            return;
        }
    };
    let permission_mode = fetch_permission_mode(&client, &daemon_url)
        .await
        .unwrap_or_else(|_| "risk".to_string());
    let _ = ui_tx.send(UiEvent::Status(StatusUpdate {
        model: Some("daemon connected".into()),
        session_name: Some(session.name.clone()),
        permission_mode: Some(permission_mode),
        ..Default::default()
    }));
    let _ = ui_tx.send(UiEvent::UsageUpdate {
        used: session.estimated_tokens,
        window: context_window,
    });

    while let Some(submission) = input_rx.recv().await {
        let UserSubmission::Text(input) = submission else {
            break;
        };
        let trimmed = input.trim();
        if trimmed.is_empty() {
            continue;
        }
        if handle_client_command(
            trimmed,
            &client,
            &daemon_url,
            &mut session,
            &ui_tx,
            context_window,
        )
        .await
        {
            continue;
        }

        let _ = ui_tx.send(UiEvent::UserEcho(trimmed.to_string()));
        let cancellation = CancellationToken::new();
        let _ = ui_tx.send(UiEvent::TurnStart {
            cancel_token: cancellation.clone(),
        });
        let outcome = stream_turn(
            &client,
            &daemon_url,
            &session.id,
            &session.name,
            trimmed,
            &ui_tx,
            cancellation,
            context_window,
        )
        .await;
        if let Err(error) = outcome {
            let _ = ui_tx.send(UiEvent::Error(error.to_string()));
        }
        if let Ok(snapshot) = fetch_snapshot(&client, &daemon_url, &session.id).await {
            session = snapshot;
            let _ = ui_tx.send(UiEvent::UsageUpdate {
                used: session.estimated_tokens,
                window: context_window,
            });
        }
        let _ = ui_tx.send(UiEvent::TurnComplete);
    }
}

async fn select_or_create_tui_session(
    client: &reqwest::Client,
    daemon_url: &str,
) -> Result<SessionSnapshot> {
    let sessions = client
        .get(format!("{daemon_url}/api/agent/sessions"))
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<(String, String)>>()
        .await?;
    let cwd = std::fs::canonicalize(std::env::current_dir()?)?;
    for (id, name) in sessions {
        if !name.starts_with("tui-") {
            continue;
        }
        if let Ok(snapshot) = fetch_snapshot(client, daemon_url, &id).await {
            let matches = snapshot
                .cwd
                .as_deref()
                .and_then(|path| std::fs::canonicalize(path).ok())
                .is_some_and(|path| path == cwd);
            if matches {
                return Ok(snapshot);
            }
        }
    }
    create_tui_session(client, daemon_url).await
}

async fn create_tui_session(client: &reqwest::Client, daemon_url: &str) -> Result<SessionSnapshot> {
    let name = format!("tui-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let cwd = std::env::current_dir()?;
    let created = client
        .post(format!("{daemon_url}/api/agent/sessions"))
        .json(&json!({ "name": name, "cwd": cwd }))
        .send()
        .await?
        .error_for_status()?
        .json::<CreateSessionResponse>()
        .await?;
    fetch_snapshot(client, daemon_url, &created.id)
        .await
        .or_else(|_| {
            Ok(SessionSnapshot {
                id: created.id,
                name: created.name,
                messages: Vec::new(),
                estimated_tokens: 0,
                cwd: Some(cwd.to_string_lossy().into_owned()),
            })
        })
}

async fn fetch_snapshot(
    client: &reqwest::Client,
    daemon_url: &str,
    session_id: &str,
) -> Result<SessionSnapshot> {
    client
        .get(format!(
            "{daemon_url}/api/agent/sessions/{session_id}/snapshot"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("decode daemon session snapshot")
}

async fn handle_client_command(
    input: &str,
    client: &reqwest::Client,
    daemon_url: &str,
    session: &mut SessionSnapshot,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    context_window: u32,
) -> bool {
    let result = match input {
        "/new" => create_tui_session(client, daemon_url).await.map(|created| {
            *session = created;
            format!("已创建 daemon 会话 {}", session.id)
        }),
        "/sessions" => {
            async {
                let sessions = client
                    .get(format!("{daemon_url}/api/agent/sessions"))
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<Vec<(String, String)>>()
                    .await?;
                Ok::<_, anyhow::Error>(
                    sessions
                        .into_iter()
                        .map(|(id, name)| format!("{id}\t{name}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            }
            .await
        }
        "/status" => {
            async {
                let status = client
                    .get(format!("{daemon_url}/api/agent/daemon/status"))
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<Value>()
                    .await?;
                Ok::<_, anyhow::Error>(format!(
                    "daemon pid={} · tasks={} · ACP sessions={} · {}",
                    status["pid"], status["activeTasks"], status["activeAcpSessions"], daemon_url
                ))
            }
            .await
        }
        "/permissions" => fetch_permission_mode(client, daemon_url)
            .await
            .map(|mode| format!("当前权限模式：{mode}\n用法：/permissions <prompt|risk|full>")),
        _ if input.starts_with("/permissions ") => {
            async {
                let mode = input.trim_start_matches("/permissions ").trim();
                if !matches!(mode, "prompt" | "risk" | "full") {
                    anyhow::bail!("usage: /permissions <prompt|risk|full>");
                }
                save_permission_mode(client, daemon_url, mode).await?;
                Ok::<_, anyhow::Error>(format!("权限模式已切换为 {mode}"))
            }
            .await
        }
        "/yolo" => {
            async {
                let current = fetch_permission_mode(client, daemon_url).await?;
                let next = if current == "full" { "risk" } else { "full" };
                save_permission_mode(client, daemon_url, next).await?;
                Ok::<_, anyhow::Error>(format!("权限模式已切换为 {next}"))
            }
            .await
        }
        "/attention" | "/notifications" => {
            async {
                let endpoint = if input == "/attention" {
                    "attention"
                } else {
                    "notifications"
                };
                let items = client
                    .get(format!("{daemon_url}/api/agent/{endpoint}"))
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<Vec<Value>>()
                    .await?;
                Ok::<_, anyhow::Error>(if items.is_empty() {
                    format!("没有待处理的 {endpoint}")
                } else {
                    items
                        .into_iter()
                        .map(|item| {
                            format!(
                                "{}\tv{}\t{}",
                                item["id"].as_str().unwrap_or("unknown"),
                                item["version"].as_i64().unwrap_or_default(),
                                item["title"].as_str().unwrap_or_default()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            }
            .await
        }
        _ if input.starts_with("/resolve ") => {
            async {
                let mut parts = input.split_whitespace();
                let _command = parts.next();
                let id = parts
                    .next()
                    .context("usage: /resolve <id> <version> <action>")?;
                let version = parts
                    .next()
                    .context("usage: /resolve <id> <version> <action>")?
                    .parse::<i64>()?;
                let action = parts
                    .next()
                    .context("usage: /resolve <id> <version> <action>")?;
                client
                    .post(format!("{daemon_url}/api/agent/attention/{id}/resolve"))
                    .json(&json!({
                        "expectedVersion": version,
                        "resolution": { "action": action }
                    }))
                    .send()
                    .await?
                    .error_for_status()?;
                Ok::<_, anyhow::Error>(format!("已处理 attention {id}: {action}"))
            }
            .await
        }
        "/acp-permissions" => {
            async {
                let value = client
                    .get(format!("{daemon_url}/api/agent/acp/permissions"))
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<Value>()
                    .await?;
                Ok::<_, anyhow::Error>(serde_json::to_string_pretty(&value)?)
            }
            .await
        }
        _ if input.starts_with("/acp-resolve ") => {
            async {
                let mut parts = input.split_whitespace();
                let _command = parts.next();
                let id = parts
                    .next()
                    .context("usage: /acp-resolve <permission-id> <option-id>")?;
                let option_id = parts
                    .next()
                    .context("usage: /acp-resolve <permission-id> <option-id>")?;
                client
                    .post(format!(
                        "{daemon_url}/api/agent/acp/permissions/{id}/resolve"
                    ))
                    .json(&json!({ "optionId": option_id }))
                    .send()
                    .await?
                    .error_for_status()?;
                Ok::<_, anyhow::Error>(format!("已处理 ACP permission {id}: {option_id}"))
            }
            .await
        }
        _ if input.starts_with("/resume ") => {
            let id = input.trim_start_matches("/resume ").trim();
            fetch_snapshot(client, daemon_url, id)
                .await
                .map(|snapshot| {
                    *session = snapshot;
                    format!("已切换 daemon 会话 {}", session.id)
                })
        }
        _ => return false,
    };
    match result {
        Ok(message) => {
            let _ = ui_tx.send(UiEvent::SystemLine(message));
            let permission_mode = fetch_permission_mode(client, daemon_url)
                .await
                .unwrap_or_else(|_| "risk".to_string());
            let _ = ui_tx.send(UiEvent::Status(StatusUpdate {
                session_name: Some(session.name.clone()),
                permission_mode: Some(permission_mode),
                ..Default::default()
            }));
            let _ = ui_tx.send(UiEvent::UsageUpdate {
                used: session.estimated_tokens,
                window: context_window,
            });
        }
        Err(error) => {
            let _ = ui_tx.send(UiEvent::Error(error.to_string()));
        }
    }
    let _ = ui_tx.send(UiEvent::TurnComplete);
    true
}

#[allow(clippy::too_many_arguments)]
async fn stream_turn(
    client: &reqwest::Client,
    daemon_url: &str,
    session_id: &str,
    session_name: &str,
    input: &str,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    cancellation: CancellationToken,
    context_window: u32,
) -> Result<()> {
    let mut snapshot = fetch_snapshot(client, daemon_url, session_id).await?;
    snapshot.messages.push(ChatMessage {
        role: "user".into(),
        content: input.into(),
        images: Vec::new(),
        generated_images: Vec::new(),
        tool_calls: None,
        tool_call_id: None,
    });
    let cwd = std::env::current_dir()?.to_string_lossy().into_owned();
    let response = client
        .post(format!(
            "{daemon_url}/api/agent/sessions/{session_id}/stream"
        ))
        .json(&json!({
            "messages": snapshot.messages,
            "system_prompt": null,
            "thinking": false,
            "cwd": cwd,
            "session_name": session_name,
            "require_permission_approval": false
        }))
        .send()
        .await?
        .error_for_status()?;
    let mut bytes = response.bytes_stream();
    let mut buffer = String::new();
    let mut tool_args: HashMap<String, String> = HashMap::new();
    let mut permission_poll = tokio::time::interval(Duration::from_millis(400));
    permission_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let chunk = tokio::select! {
            _ = cancellation.cancelled() => {
                let _ = client.post(format!("{daemon_url}/api/agent/sessions/{session_id}/control"))
                    .json(&json!({ "action": "abort" }))
                    .send().await;
                let _ = ui_tx.send(UiEvent::SystemLine("已请求 daemon 停止当前生成；后台 daemon 保持运行".into()));
                break;
            }
            _ = permission_poll.tick() => {
                if let Ok(Some(item)) = fetch_pending_permission(client, daemon_url, session_id).await {
                    resolve_tui_permission(
                        client,
                        daemon_url,
                        ui_tx,
                        &cancellation,
                        item,
                    ).await?;
                }
                continue;
            }
            chunk = bytes.next() => chunk,
        };
        let Some(chunk) = chunk else { break };
        buffer.push_str(std::str::from_utf8(&chunk?)?);
        while let Some(boundary) = buffer.find("\n\n") {
            let frame = buffer[..boundary].to_string();
            buffer.drain(..boundary + 2);
            handle_sse_frame(&frame, ui_tx, &mut tool_args, context_window)?;
        }
    }
    Ok(())
}

async fn fetch_permission_mode(client: &reqwest::Client, daemon_url: &str) -> Result<String> {
    let response = client
        .get(format!("{daemon_url}/api/local-config"))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    Ok(response
        .pointer("/data/permissions/agent_mode")
        .and_then(Value::as_str)
        .unwrap_or("risk")
        .to_string())
}

async fn save_permission_mode(
    client: &reqwest::Client,
    daemon_url: &str,
    mode: &str,
) -> Result<()> {
    client
        .put(format!("{daemon_url}/api/local-config"))
        .json(&json!({ "permissions": { "agent_mode": mode } }))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

async fn fetch_pending_permission(
    client: &reqwest::Client,
    daemon_url: &str,
    session_id: &str,
) -> Result<Option<AttentionItem>> {
    let items = client
        .get(format!("{daemon_url}/api/agent/attention"))
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<AttentionItem>>()
        .await?;
    Ok(items
        .into_iter()
        .find(|item| item.kind == "permission" && item.task_id == session_id))
}

async fn resolve_tui_permission(
    client: &reqwest::Client,
    daemon_url: &str,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    cancellation: &CancellationToken,
    item: AttentionItem,
) -> Result<()> {
    let tool = item
        .detail
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or(&item.title)
        .to_string();
    let arguments = item
        .detail
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}")
        .to_string();
    let (reply_tx, reply_rx) = oneshot::channel();
    ui_tx
        .send(UiEvent::PermissionRequest {
            name: tool,
            args: arguments,
            reply: reply_tx,
        })
        .map_err(|_| anyhow::anyhow!("TUI 已关闭，无法处理权限请求"))?;
    let allow = tokio::select! {
        reply = reply_rx => reply.unwrap_or(false),
        _ = cancellation.cancelled() => false,
    };
    client
        .post(format!(
            "{daemon_url}/api/agent/attention/{}/resolve",
            item.id
        ))
        .json(&json!({
            "expectedVersion": item.version,
            "resolution": { "action": if allow { "allow" } else { "deny" } }
        }))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

fn handle_sse_frame(
    frame: &str,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    tool_args: &mut HashMap<String, String>,
    context_window: u32,
) -> Result<()> {
    let mut event = "message";
    let mut data = String::new();
    for line in frame.lines() {
        if let Some(value) = line.strip_prefix("event: ") {
            event = value;
        } else if let Some(value) = line.strip_prefix("data: ") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value);
        }
    }
    let value = serde_json::from_str::<Value>(&data).unwrap_or(Value::Null);
    match event {
        "first_token" => {
            let _ = ui_tx.send(UiEvent::Thinking);
        }
        "text_delta" => {
            if let Some(delta) = value["delta"].as_str() {
                let _ = ui_tx.send(UiEvent::AssistantDelta(delta.into()));
            }
        }
        "thinking_delta" => {}
        "tool_call_start" => {
            let id = value["id"].as_str().unwrap_or_default().to_string();
            tool_args.insert(id, String::new());
            let _ = ui_tx.send(UiEvent::ToolCall {
                name: value["name"].as_str().unwrap_or("tool").into(),
                args: String::new(),
            });
        }
        "tool_call_delta" => {
            if let (Some(id), Some(delta)) =
                (value["id"].as_str(), value["arguments_delta"].as_str())
            {
                tool_args.entry(id.into()).or_default().push_str(delta);
            }
        }
        "tool_result" => {
            let _ = ui_tx.send(UiEvent::ToolResult {
                result: value["result"].as_str().unwrap_or_default().into(),
                is_error: value["is_error"].as_bool().unwrap_or(false),
            });
        }
        "tool_progress" => {
            if let Some(line) = value["line"].as_str() {
                let _ = ui_tx.send(UiEvent::SystemLine(line.into()));
            }
        }
        "tool_image" => {
            let _ = ui_tx.send(UiEvent::ToolImage {
                url: value["url"].as_str().unwrap_or_default().into(),
                alt: value["alt"].as_str().unwrap_or("image").into(),
            });
        }
        "context_usage" => {
            let used = value["total_tokens"]
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or_default();
            let _ = ui_tx.send(UiEvent::UsageUpdate {
                used,
                window: context_window,
            });
        }
        "stream_reset" => {
            let _ = ui_tx.send(UiEvent::StreamRetry(
                value["reason"].as_str().unwrap_or("stream reset").into(),
            ));
        }
        "error" => {
            let _ = ui_tx.send(UiEvent::Error(
                value["message"]
                    .as_str()
                    .unwrap_or("daemon stream error")
                    .into(),
            ));
        }
        "done" => {
            let _ = ui_tx.send(UiEvent::AssistantEnd);
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_split_safe_sse_fields() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        handle_sse_frame(
            "event: text_delta\ndata: {\"delta\":\"hello\"}",
            &tx,
            &mut HashMap::new(),
            100,
        )
        .unwrap();
        assert!(matches!(rx.try_recv(), Ok(UiEvent::AssistantDelta(value)) if value == "hello"));
    }
}
