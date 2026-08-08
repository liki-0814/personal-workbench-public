//! Shared ACP stdio client used by every native ACP CLI backend.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use agent_client_protocol::schema::{
    CancelNotification, ContentBlock, InitializeRequest, LoadSessionRequest, NewSessionRequest,
    PermissionOptionKind, PromptRequest, ProtocolVersion, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionConfigOption, SessionModeState, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionModeRequest, TextContent,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Error as AcpError};
use anyhow::{Context, Result};
use tokio::io::AsyncReadExt;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::runtime::tools::progress;

use super::NativeSessionListener;

#[derive(Debug)]
pub struct AcpTurnOutput {
    pub session_id: String,
    pub output: String,
    pub usage: Option<crate::runtime::tools::code_agent::CodeAgentUsage>,
}

pub struct AcpTurnRequest<'a> {
    pub program: &'a str,
    pub program_args: &'a [String],
    pub cwd: &'a Path,
    pub task: &'a str,
    pub resume_session_id: Option<&'a str>,
    pub mode: &'a str,
    pub permission_mode: Option<&'a str>,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub context_window: Option<u32>,
    pub timeout: Duration,
    pub native_session_listener: Option<NativeSessionListener>,
}

struct CollectedOutput {
    answer: String,
    reasoning: String,
    activity: tokio::sync::mpsc::UnboundedSender<()>,
    last_answer_snapshot: Option<Instant>,
    last_reasoning_snapshot: Option<Instant>,
}

// Some providers do not stream while doing repository-wide analysis. The outer
// turn timeout remains the hard deadline; this guard only detects a truly
// silent/stalled transport.
const ACP_INACTIVITY_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const ACP_ANSWER_SNAPSHOT_INTERVAL: Duration = Duration::from_millis(300);
const ACP_REASONING_SNAPSHOT_INTERVAL: Duration = Duration::from_millis(750);
const ACP_STREAM_SNAPSHOT_CHARS: usize = 64_000;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionInfo {
    pub session_id: String,
    pub implementation: String,
    pub cwd: PathBuf,
    pub status: String,
    pub model: Option<String>,
    pub permission_mode: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub context_tokens: u64,
    pub context_window: u64,
    pub available_modes: serde_json::Value,
    pub config_options: serde_json::Value,
    pub resume_metadata: serde_json::Value,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpEvent {
    pub id: u64,
    pub session_id: String,
    pub kind: String,
    pub detail: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingAcpPermission {
    pub id: String,
    pub session_id: String,
    pub options: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone)]
struct SessionHandle {
    sender: tokio::sync::mpsc::Sender<SessionCommand>,
    info: Arc<Mutex<AcpSessionInfo>>,
    closed: tokio::sync::watch::Receiver<bool>,
    force_shutdown: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    abort: tokio::task::AbortHandle,
}

struct PromptSettings {
    task: String,
    mode: String,
    model: Option<String>,
    effort: Option<String>,
    context_window: Option<u32>,
    permission_mode: String,
}

enum SessionCommand {
    Prompt {
        settings: PromptSettings,
        reply: tokio::sync::oneshot::Sender<Result<AcpTurnOutput>>,
    },
    Cancel,
    Shutdown,
}

struct PermissionWaiter {
    pending: PendingAcpPermission,
    reply: tokio::sync::oneshot::Sender<String>,
}

struct AcpRuntime {
    sessions: Mutex<HashMap<String, SessionHandle>>,
    permissions: Mutex<HashMap<String, PermissionWaiter>>,
    events: Mutex<VecDeque<AcpEvent>>,
    next_event_id: std::sync::atomic::AtomicU64,
    broadcaster: tokio::sync::broadcast::Sender<AcpEvent>,
}

fn runtime() -> &'static AcpRuntime {
    static RUNTIME: OnceLock<AcpRuntime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        let (broadcaster, _) = tokio::sync::broadcast::channel(1024);
        AcpRuntime {
            sessions: Mutex::new(HashMap::new()),
            permissions: Mutex::new(HashMap::new()),
            events: Mutex::new(VecDeque::new()),
            next_event_id: std::sync::atomic::AtomicU64::new(1),
            broadcaster,
        }
    })
}

pub fn list_sessions() -> Vec<AcpSessionInfo> {
    runtime()
        .sessions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .values()
        .map(|handle| {
            handle
                .info
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
        })
        .collect()
}

pub fn active_session_count() -> usize {
    list_sessions()
        .iter()
        .filter(|session| is_active_session_status(&session.status))
        .count()
}

fn is_active_session_status(status: &str) -> bool {
    status == "running"
}

pub fn events_after(session_id: Option<&str>, after: u64) -> Vec<AcpEvent> {
    runtime()
        .events
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .filter(|event| event.id > after && session_id.is_none_or(|id| event.session_id == id))
        .cloned()
        .collect()
}

pub fn subscribe_events() -> tokio::sync::broadcast::Receiver<AcpEvent> {
    runtime().broadcaster.subscribe()
}

pub fn pending_permissions() -> Vec<PendingAcpPermission> {
    runtime()
        .permissions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .values()
        .map(|waiter| waiter.pending.clone())
        .collect()
}

pub fn resolve_permission(id: &str, option_id: &str) -> Result<()> {
    let waiter = runtime()
        .permissions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(id)
        .context("ACP permission request not found or already resolved")?;
    let session_id = waiter.pending.session_id.clone();
    waiter
        .reply
        .send(option_id.to_string())
        .map_err(|_| anyhow::anyhow!("ACP permission request is no longer active"))?;
    if let Some(handle) = session_handle(&session_id) {
        let mut info = handle
            .info
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        info.status = "running".to_string();
        info.updated_at = chrono::Utc::now();
    }
    emit_event(
        &session_id,
        "permission_resolved",
        serde_json::json!({ "permissionId": id, "optionId": option_id }),
    );
    Ok(())
}

pub async fn cancel_session(session_id: &str) -> Result<()> {
    let handle = session_handle(session_id).context("ACP session not active")?;
    handle
        .sender
        .send(SessionCommand::Cancel)
        .await
        .context("ACP session process exited")
}

pub async fn close_session(session_id: &str) -> Result<()> {
    let handle = runtime()
        .sessions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(session_id)
        .context("ACP session not active")?;
    handle
        .sender
        .send(SessionCommand::Shutdown)
        .await
        .context("ACP session process exited")?;
    let mut closed = handle.closed.clone();
    if !*closed.borrow()
        && tokio::time::timeout(std::time::Duration::from_millis(300), closed.changed())
            .await
            .is_err()
    {
        // Some ACP implementations (notably QoderCLI) keep their transport
        // alive after the client side has ended the session. Aborting the
        // owner task drops the Child with kill_on_drop enabled before the
        // daemon runtime exits, so the process cannot be reparented as an
        // orphan.
        if let Some(force_shutdown) = handle
            .force_shutdown
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = force_shutdown.send(());
        }
        if tokio::time::timeout(std::time::Duration::from_secs(1), closed.changed())
            .await
            .is_err()
        {
            handle.abort.abort();
            let _ = closed.changed().await;
        }
        emit_event(
            session_id,
            "session_force_killed",
            serde_json::json!({ "reason": "shutdown_timeout" }),
        );
        emit_event(session_id, "session_closed", serde_json::json!({}));
    }
    Ok(())
}

pub async fn shutdown_all_sessions() {
    let session_ids = list_sessions()
        .into_iter()
        .map(|session| session.session_id)
        .collect::<Vec<_>>();
    let results = futures::future::join_all(
        session_ids
            .iter()
            .map(|session_id| close_session(session_id)),
    )
    .await;
    for (session_id, result) in session_ids.into_iter().zip(results) {
        if let Err(error) = result {
            tracing::warn!(%error, %session_id, "failed to close ACP session during shutdown");
        }
    }
}

pub async fn run_acp_turn(request: AcpTurnRequest<'_>) -> Result<AcpTurnOutput> {
    tokio::time::timeout(request.timeout, run_acp_turn_inner(request))
        .await
        .context("ACP turn timed out")?
}

async fn run_acp_turn_inner(request: AcpTurnRequest<'_>) -> Result<AcpTurnOutput> {
    if let Some(session_id) = request.resume_session_id {
        if let Some(handle) = session_handle(session_id) {
            if let Some(listener) = &request.native_session_listener {
                listener(session_id.to_string());
            }
            return send_prompt(&handle, &request).await;
        }
    }
    let handle = tokio::time::timeout(std::time::Duration::from_secs(30), spawn_session(&request))
        .await
        .context("ACP handshake timed out")??;
    if let Some(listener) = &request.native_session_listener {
        listener(
            handle
                .info
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .session_id
                .clone(),
        );
    }
    send_prompt(&handle, &request).await
}

fn session_handle(session_id: &str) -> Option<SessionHandle> {
    runtime()
        .sessions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(session_id)
        .cloned()
}

async fn send_prompt(
    handle: &SessionHandle,
    request: &AcpTurnRequest<'_>,
) -> Result<AcpTurnOutput> {
    let (reply, response) = tokio::sync::oneshot::channel();
    handle
        .sender
        .send(SessionCommand::Prompt {
            settings: PromptSettings {
                task: request.task.to_string(),
                mode: request.mode.to_string(),
                model: request.model.map(str::to_string),
                effort: request.effort.map(str::to_string),
                context_window: request.context_window,
                permission_mode: request.permission_mode.unwrap_or("default").to_string(),
            },
            reply,
        })
        .await
        .context("ACP session process exited")?;
    response.await.context("ACP prompt response dropped")?
}

async fn spawn_session(request: &AcpTurnRequest<'_>) -> Result<SessionHandle> {
    let program = request.program.to_string();
    let program_args = request.program_args.to_vec();
    let cwd = request.cwd.to_path_buf();
    let resume = request.resume_session_id.map(str::to_string);
    let implementation = implementation_name(&program);
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<SessionCommand>(32);
    let (ready_sender, ready_receiver) = tokio::sync::oneshot::channel::<Result<AcpSessionInfo>>();
    let (closed_sender, closed_receiver) = tokio::sync::watch::channel(false);
    let (force_shutdown_sender, force_shutdown_receiver) = tokio::sync::oneshot::channel();
    let force_shutdown = Arc::new(Mutex::new(Some(force_shutdown_sender)));
    let info_slot: Arc<Mutex<Option<Arc<Mutex<AcpSessionInfo>>>>> = Arc::new(Mutex::new(None));
    let info_for_task = Arc::clone(&info_slot);
    let implementation_for_task = implementation.clone();
    let progress_emitter = progress::current();
    let image_emitter = progress::current_image();
    let text_delta_emitter = progress::current_text_delta();
    let allow_fresh_resume = request.mode == "research";

    let session_task = tokio::spawn(async move {
        let result = progress::with_all_emitters(
            progress_emitter,
            image_emitter,
            text_delta_emitter,
            run_session_process(
                &program,
                &program_args,
                &cwd,
                resume.as_deref(),
                allow_fresh_resume,
                &implementation_for_task,
                &mut receiver,
                ready_sender,
                info_for_task,
                force_shutdown_receiver,
            ),
        )
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, implementation = %implementation_for_task, "ACP session process stopped");
        }
        let _ = closed_sender.send(true);
    });
    let abort = session_task.abort_handle();

    let info = ready_receiver
        .await
        .context("ACP process exited before initialization")??;
    let info = info_slot
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .unwrap_or_else(|| Arc::new(Mutex::new(info.clone())));
    let session_id = info
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .session_id
        .clone();
    let handle = SessionHandle {
        sender,
        info,
        closed: closed_receiver,
        force_shutdown,
        abort,
    };
    runtime()
        .sessions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(session_id.clone(), handle.clone());
    let snapshot = handle
        .info
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    emit_event(
        &session_id,
        "session_ready",
        serde_json::json!({
            "implementation": implementation,
            "cwd": snapshot.cwd,
            "availableModes": snapshot.available_modes,
            "configOptions": snapshot.config_options,
            "resumed": snapshot.resume_metadata.get("loaded").cloned().unwrap_or_default()
        }),
    );
    Ok(handle)
}

#[allow(clippy::too_many_arguments)]
async fn run_session_process(
    program: &str,
    program_args: &[String],
    cwd: &Path,
    resume_session_id: Option<&str>,
    allow_fresh_resume: bool,
    implementation: &str,
    receiver: &mut tokio::sync::mpsc::Receiver<SessionCommand>,
    ready: tokio::sync::oneshot::Sender<Result<AcpSessionInfo>>,
    info_slot: Arc<Mutex<Option<Arc<Mutex<AcpSessionInfo>>>>>,
    mut force_shutdown: tokio::sync::oneshot::Receiver<()>,
) -> Result<()> {
    let mut command = crate::runtime::tools::fs_local::worker_subprocess_command(
        program,
        program_args,
        crate::runtime::tools::fs_local::worker_execution_policy(),
        false,
    )?;
    command
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().context("spawn ACP agent")?;
    let stdin = child.stdin.take().context("open ACP stdin")?;
    let stdout = child.stdout.take().context("open ACP stdout")?;
    let stderr = child.stderr.take().context("open ACP stderr")?;
    let stderr_task = tokio::spawn(async move {
        let mut output = Vec::new();
        let _ = stderr.take(32 * 1024).read_to_end(&mut output).await;
        String::from_utf8_lossy(&output).trim().to_string()
    });
    let transport = agent_client_protocol::ByteStreams::new(stdin.compat_write(), stdout.compat());
    let active_output: Arc<Mutex<Option<Arc<Mutex<CollectedOutput>>>>> = Arc::new(Mutex::new(None));
    let notification_output = Arc::clone(&active_output);
    let permission_mode = Arc::new(Mutex::new("default".to_string()));
    let permission_mode_for_request = Arc::clone(&permission_mode);
    let cwd = cwd.to_path_buf();
    let resume_session_id = resume_session_id.map(str::to_string);
    let implementation = implementation.to_string();
    let ready = Arc::new(Mutex::new(Some(ready)));
    let ready_for_connection = Arc::clone(&ready);
    let info_for_notification = Arc::clone(&info_slot);
    let info_for_connection = Arc::clone(&info_slot);

    let connection = Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                collect_update(
                    &notification_output,
                    &info_for_notification,
                    &notification.session_id.to_string(),
                    notification.update,
                );
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                let mode = permission_mode_for_request
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                let selected = await_permission(&request, &mode).await;
                if let Some(option_id) = selected {
                    progress::emit(&format!("ACP permission: selected {option_id}"));
                    responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                            option_id,
                        )),
                    ))
                } else {
                    progress::emit("ACP permission: cancelled");
                    responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Cancelled,
                    ))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, |connection: ConnectionTo<Agent>| async move {
            let initialized = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await
                .map_err(|error| acp_error("initialize ACP agent", error))?;
            if let Some(info) = initialized.agent_info {
                progress::emit(&format!("ACP connected: {} {}", info.name, info.version));
            }

            let (session_id, modes, config_options, resumed) = open_session(
                &connection,
                resume_session_id.as_deref(),
                &cwd,
                allow_fresh_resume,
            )
            .await
            .map_err(|error| acp_error("open ACP session", error))?;
            let now = chrono::Utc::now();
            let available_modes = serde_json::to_value(&modes).unwrap_or(serde_json::Value::Null);
            let discovered_config_options =
                serde_json::to_value(&config_options).unwrap_or_else(|_| serde_json::json!([]));
            let info = Arc::new(Mutex::new(AcpSessionInfo {
                session_id: session_id.to_string(),
                implementation,
                cwd,
                status: "idle".to_string(),
                model: None,
                permission_mode: "default".to_string(),
                created_at: now,
                updated_at: now,
                input_tokens: 0,
                output_tokens: 0,
                context_tokens: 0,
                context_window: 0,
                available_modes,
                config_options: discovered_config_options,
                resume_metadata: serde_json::json!({ "loaded": resumed }),
            }));
            *info_for_connection
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(Arc::clone(&info));
            if let Some(ready) = ready_for_connection
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                let snapshot = info
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                let _ = ready.send(Ok(snapshot));
            }

            while let Some(command) = receiver.recv().await {
                match command {
                    SessionCommand::Prompt { settings, reply } => {
                        *permission_mode
                            .lock()
                            .unwrap_or_else(|error| error.into_inner()) =
                            settings.permission_mode.clone();
                        {
                            let mut info = info.lock().unwrap_or_else(|error| error.into_inner());
                            info.status = "running".to_string();
                            info.model = settings.model.clone();
                            info.permission_mode = settings.permission_mode.clone();
                            info.updated_at = chrono::Utc::now();
                        }
                        let result = run_prompt(
                            &connection,
                            &session_id,
                            modes.as_ref(),
                            config_options.as_deref(),
                            settings,
                            &active_output,
                            &info,
                        )
                        .await;
                        let _ = reply.send(result.map_err(anyhow::Error::from));
                    }
                    SessionCommand::Cancel => {
                        connection
                            .send_notification(CancelNotification::new(session_id.clone()))?;
                        emit_event(
                            &session_id.to_string(),
                            "cancel_requested",
                            serde_json::json!({}),
                        );
                    }
                    SessionCommand::Shutdown => break,
                }
            }
            Ok::<_, AcpError>(())
        });
    tokio::pin!(connection);
    let result = tokio::select! {
        result = &mut connection => result,
        _ = &mut force_shutdown => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Ok(());
        }
    };
    let _ = child.kill().await;
    let stderr = stderr_task.await.unwrap_or_default();
    if let Err(error) = result {
        if stderr.is_empty() {
            return Err(error).context("run ACP connection");
        }
        anyhow::bail!(
            "run ACP connection: {error}; ACP stderr: {}",
            bounded_stderr(&stderr)
        );
    }
    if let Some(info) = info_slot
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
    {
        let session_id = info
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .session_id
            .clone();
        runtime()
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
        emit_event(&session_id, "session_closed", serde_json::json!({}));
    }
    Ok(())
}

async fn run_prompt(
    connection: &ConnectionTo<Agent>,
    session_id: &agent_client_protocol::schema::SessionId,
    modes: Option<&SessionModeState>,
    config_options: Option<&[SessionConfigOption]>,
    settings: PromptSettings,
    active_output: &Arc<Mutex<Option<Arc<Mutex<CollectedOutput>>>>>,
    info: &Arc<Mutex<AcpSessionInfo>>,
) -> Result<AcpTurnOutput, AcpError> {
    apply_session_mode(
        connection,
        session_id,
        modes,
        config_options,
        &settings.mode,
    )
    .await
    .map_err(|error| acp_error("set ACP session mode", error))?;
    apply_config(
        connection,
        session_id,
        config_options,
        "model",
        settings.model.as_deref(),
        true,
    )
    .await
    .map_err(|error| acp_error("set ACP model", error))?;
    apply_config(
        connection,
        session_id,
        config_options,
        "effort",
        settings.effort.as_deref(),
        false,
    )
    .await
    .map_err(|error| acp_error("set ACP effort", error))?;
    let context = settings.context_window.map(|value| value.to_string());
    apply_config(
        connection,
        session_id,
        config_options,
        "context",
        context.as_deref(),
        false,
    )
    .await
    .map_err(|error| acp_error("set ACP context", error))?;
    apply_config(
        connection,
        session_id,
        config_options,
        "permission",
        Some(&settings.permission_mode),
        false,
    )
    .await
    .map_err(|error| acp_error("set ACP permission mode", error))?;
    let (activity, mut activity_rx) = tokio::sync::mpsc::unbounded_channel();
    let collected = Arc::new(Mutex::new(CollectedOutput {
        answer: String::new(),
        reasoning: String::new(),
        activity,
        last_answer_snapshot: None,
        last_reasoning_snapshot: None,
    }));
    *active_output
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(Arc::clone(&collected));
    emit_event(
        &session_id.to_string(),
        "user_message",
        serde_json::json!({ "text": settings.task }),
    );
    let response_request = connection
        .send_request(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new(settings.task))],
        ))
        .block_task();
    tokio::pin!(response_request);
    let response = loop {
        tokio::select! {
            response = &mut response_request => {
                break response.map_err(|error| acp_error("send ACP prompt", error))?;
            }
            activity = activity_rx.recv() => {
                if activity.is_none() {
                    return Err(acp_error("send ACP prompt", "activity channel closed"));
                }
            }
            _ = tokio::time::sleep(ACP_INACTIVITY_TIMEOUT) => {
                let _ = connection.send_notification(CancelNotification::new(session_id.clone()));
                progress::emit("ACP inactivity timeout: cancelled stalled prompt");
                return Err(acp_error(
                    "send ACP prompt",
                    format!("inactivity timeout after {}s", ACP_INACTIVITY_TIMEOUT.as_secs()),
                ));
            }
        }
    };
    emit_final_snapshots(&collected);
    *active_output
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    let output = collected
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .answer
        .clone();
    let usage = response
        .usage
        .map(|usage| crate::runtime::tools::code_agent::CodeAgentUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_tokens: usage.cached_read_tokens.unwrap_or(0)
                + usage.cached_write_tokens.unwrap_or(0),
        });
    {
        let mut info = info.lock().unwrap_or_else(|error| error.into_inner());
        info.status = format!("{:?}", response.stop_reason).to_ascii_lowercase();
        info.updated_at = chrono::Utc::now();
        if let Some(usage) = usage {
            info.input_tokens = usage.input_tokens;
            info.output_tokens = usage.output_tokens;
        }
    }
    emit_event(
        &session_id.to_string(),
        "turn_completed",
        serde_json::json!({ "stopReason": response.stop_reason, "usage": usage }),
    );
    Ok(AcpTurnOutput {
        session_id: session_id.to_string(),
        output,
        usage,
    })
}

async fn open_session(
    connection: &ConnectionTo<Agent>,
    resume_session_id: Option<&str>,
    cwd: &Path,
    allow_fresh_resume: bool,
) -> Result<(
    agent_client_protocol::schema::SessionId,
    Option<SessionModeState>,
    Option<Vec<SessionConfigOption>>,
    bool,
)> {
    if let Some(session_id) = resume_session_id {
        let loaded = connection
            .send_request(LoadSessionRequest::new(session_id.to_string(), cwd))
            .block_task()
            .await;
        match loaded {
            Ok(loaded) => {
                return Ok((
                    session_id.to_string().into(),
                    loaded.modes,
                    loaded.config_options,
                    true,
                ));
            }
            Err(error) if allow_fresh_resume => {
                progress::emit(&format!(
                    "ACP session resume unavailable; starting a fresh read-only session: {error}"
                ));
            }
            Err(error) => return Err(error).context("load ACP session"),
        }
    }
    let created = connection
        .send_request(NewSessionRequest::new(PathBuf::from(cwd)))
        .block_task()
        .await
        .context("create ACP session")?;
    Ok((
        created.session_id,
        created.modes,
        created.config_options,
        false,
    ))
}

async fn apply_session_mode(
    connection: &ConnectionTo<Agent>,
    session_id: &agent_client_protocol::schema::SessionId,
    modes: Option<&SessionModeState>,
    config_options: Option<&[SessionConfigOption]>,
    requested_mode: &str,
) -> Result<()> {
    let candidates = if requested_mode == "research" {
        &["plan", "read-only", "readonly"][..]
    } else {
        &["default", "accept-edits", "acceptedits"][..]
    };
    if let Some(mode_id) =
        modes.and_then(|modes| candidates.iter().find_map(|name| find_mode(modes, name)))
    {
        connection
            .send_request(SetSessionModeRequest::new(session_id.clone(), mode_id))
            .block_task()
            .await
            .context("set ACP session mode")?;
        return Ok(());
    }
    if let Some(value) = supported_config_value(config_options, "mode", candidates) {
        apply_config(
            connection,
            session_id,
            config_options,
            "mode",
            Some(&value),
            true,
        )
        .await?;
    }
    Ok(())
}

fn find_mode(
    modes: &SessionModeState,
    desired: &str,
) -> Option<agent_client_protocol::schema::SessionModeId> {
    let desired = normalize(desired);
    modes
        .available_modes
        .iter()
        .find(|mode| normalize(&mode.id.to_string()) == desired || normalize(&mode.name) == desired)
        .map(|mode| mode.id.clone())
}

async fn apply_config(
    connection: &ConnectionTo<Agent>,
    session_id: &agent_client_protocol::schema::SessionId,
    options: Option<&[SessionConfigOption]>,
    category: &str,
    requested_value: Option<&str>,
    required: bool,
) -> Result<()> {
    let Some(requested_value) = requested_value.filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    let Some(option) = requested_config_option(options, category, required)? else {
        progress::emit(&format!(
            "ACP does not expose optional {category} configuration; using its default"
        ));
        return Ok(());
    };
    let requested_value = canonical_config_value(option, requested_value);
    connection
        .send_request(SetSessionConfigOptionRequest::new(
            session_id.clone(),
            option.id.clone(),
            requested_value.as_str(),
        ))
        .block_task()
        .await
        .with_context(|| format!("set ACP {category} option"))?;
    Ok(())
}

fn requested_config_option<'a>(
    options: Option<&'a [SessionConfigOption]>,
    category: &str,
    required: bool,
) -> Result<Option<&'a SessionConfigOption>> {
    let option = find_config_option(options, category);
    if required && option.is_none() {
        anyhow::bail!("ACP agent does not expose a {category} configuration option");
    }
    Ok(option)
}

fn canonical_config_value(option: &SessionConfigOption, requested_value: &str) -> String {
    let serialized = serde_json::to_value(option).unwrap_or_default();
    serialized
        .get("options")
        .and_then(serde_json::Value::as_array)
        .and_then(|options| {
            options.iter().find_map(|candidate| {
                let value = candidate.get("value")?.as_str()?;
                let name = candidate
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                (normalize(value) == normalize(requested_value)
                    || normalize(name) == normalize(requested_value))
                .then(|| value.to_string())
            })
        })
        .unwrap_or_else(|| requested_value.to_string())
}

fn find_config_option<'a>(
    options: Option<&'a [SessionConfigOption]>,
    category: &str,
) -> Option<&'a SessionConfigOption> {
    options.and_then(|items| {
        items.iter().find(|option| {
            let serialized = serde_json::to_value(option).unwrap_or_default();
            let category_value = serialized
                .get("category")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            normalize(&option.id.to_string()).contains(category)
                || normalize(&option.name).contains(category)
                || normalize(category_value).contains(category)
        })
    })
}

fn supported_config_value(
    options: Option<&[SessionConfigOption]>,
    category: &str,
    candidates: &[&str],
) -> Option<String> {
    let serialized = serde_json::to_value(find_config_option(options, category)?).ok()?;
    serialized
        .get("options")?
        .as_array()?
        .iter()
        .filter_map(|option| option.get("value").and_then(serde_json::Value::as_str))
        .find(|value| {
            candidates
                .iter()
                .any(|candidate| normalize(value) == normalize(candidate))
        })
        .map(str::to_string)
}

fn collect_update(
    output: &Arc<Mutex<Option<Arc<Mutex<CollectedOutput>>>>>,
    info: &Arc<Mutex<Option<Arc<Mutex<AcpSessionInfo>>>>>,
    session_id: &str,
    update: SessionUpdate,
) {
    let resets_inactivity = update_resets_inactivity(&update);
    let kind = match &update {
        SessionUpdate::UserMessageChunk(_) => "user_message_chunk",
        SessionUpdate::AgentMessageChunk(_) => "assistant_message_chunk",
        SessionUpdate::AgentThoughtChunk(_) => "reasoning_chunk",
        SessionUpdate::ToolCall(_) => "tool_call",
        SessionUpdate::ToolCallUpdate(_) => "tool_call_update",
        SessionUpdate::Plan(_) => "plan",
        SessionUpdate::AvailableCommandsUpdate(_) => "available_commands",
        SessionUpdate::CurrentModeUpdate(_) => "mode_update",
        SessionUpdate::ConfigOptionUpdate(_) => "config_update",
        SessionUpdate::SessionInfoUpdate(_) => "session_info",
        SessionUpdate::UsageUpdate(_) => "usage_update",
        _ => "session_update",
    };
    let detail = bounded_detail(&update);
    emit_event(session_id, kind, detail.clone());
    if resets_inactivity {
        if let Some(output) = output
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .cloned()
        {
            let _ = output
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .activity
                .send(());
        }
    }
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            if let ContentBlock::Text(text) = chunk.content {
                if let Some(output) = output
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .as_ref()
                    .cloned()
                {
                    let mut output = output.lock().unwrap_or_else(|error| error.into_inner());
                    output.answer.push_str(&text.text);
                    emit_snapshot_if_due(&mut output, SnapshotKind::Answer, false);
                }
            }
        }
        SessionUpdate::AgentThoughtChunk(chunk) => {
            // Reasoning remains secondary UI state and deliberately does not reset the
            // meaningful-progress deadline. It is aggregated into one replaceable snapshot
            // rather than persisted as one timeline row per transport chunk.
            if let ContentBlock::Text(text) = chunk.content {
                if let Some(output) = output
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .as_ref()
                    .cloned()
                {
                    let mut output = output.lock().unwrap_or_else(|error| error.into_inner());
                    output.reasoning.push_str(&text.text);
                    trim_snapshot_buffer(&mut output.reasoning);
                    emit_snapshot_if_due(&mut output, SnapshotKind::Reasoning, false);
                }
            }
        }
        // Tool and plan updates are already emitted as structured ACP events. Mirroring them as
        // free-form progress creates duplicate history rows and loses their stable identifiers.
        SessionUpdate::ToolCall(_)
        | SessionUpdate::ToolCallUpdate(_)
        | SessionUpdate::Plan(_)
        | SessionUpdate::AvailableCommandsUpdate(_) => {}
        SessionUpdate::UsageUpdate(usage) => {
            if let Some(info) = info
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .cloned()
            {
                let mut info = info.lock().unwrap_or_else(|error| error.into_inner());
                info.context_tokens = usage.used;
                info.context_window = usage.size;
                info.updated_at = chrono::Utc::now();
            }
        }
        SessionUpdate::ConfigOptionUpdate(_) => {
            if let Some(info) = info
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .cloned()
            {
                let mut info = info.lock().unwrap_or_else(|error| error.into_inner());
                info.config_options = detail;
                info.updated_at = chrono::Utc::now();
            }
        }
        _ => {}
    }
}

#[derive(Clone, Copy)]
enum SnapshotKind {
    Answer,
    Reasoning,
}

fn emit_snapshot_if_due(output: &mut CollectedOutput, kind: SnapshotKind, force: bool) {
    let (content, last_emit, interval, prefix) = match kind {
        SnapshotKind::Answer => (
            &output.answer,
            &mut output.last_answer_snapshot,
            ACP_ANSWER_SNAPSHOT_INTERVAL,
            "ACP_ASSISTANT_SNAPSHOT ",
        ),
        SnapshotKind::Reasoning => (
            &output.reasoning,
            &mut output.last_reasoning_snapshot,
            ACP_REASONING_SNAPSHOT_INTERVAL,
            "ACP_REASONING_SNAPSHOT ",
        ),
    };
    if content.is_empty()
        || (!force && last_emit.is_some_and(|last_emit| last_emit.elapsed() < interval))
    {
        return;
    }
    *last_emit = Some(Instant::now());
    let snapshot = bounded_snapshot(content);
    progress::emit(&format!(
        "{prefix}{}",
        serde_json::to_string(&snapshot).unwrap_or_else(|_| "\"\"".into())
    ));
}

fn emit_final_snapshots(collected: &Arc<Mutex<CollectedOutput>>) {
    let mut output = collected.lock().unwrap_or_else(|error| error.into_inner());
    emit_snapshot_if_due(&mut output, SnapshotKind::Reasoning, true);
    emit_snapshot_if_due(&mut output, SnapshotKind::Answer, true);
}

fn trim_snapshot_buffer(content: &mut String) {
    if content.chars().count() <= ACP_STREAM_SNAPSHOT_CHARS {
        return;
    }
    *content = content
        .chars()
        .rev()
        .take(ACP_STREAM_SNAPSHOT_CHARS)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
}

fn bounded_snapshot(content: &str) -> String {
    if content.chars().count() <= ACP_STREAM_SNAPSHOT_CHARS {
        return content.to_string();
    }
    format!(
        "…\n{}",
        content
            .chars()
            .rev()
            .take(ACP_STREAM_SNAPSHOT_CHARS)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
    )
}

fn update_resets_inactivity(update: &SessionUpdate) -> bool {
    matches!(
        update,
        SessionUpdate::AgentMessageChunk(_)
            | SessionUpdate::ToolCall(_)
            | SessionUpdate::ToolCallUpdate(_)
            | SessionUpdate::Plan(_)
    )
}

fn bounded_detail(update: &SessionUpdate) -> serde_json::Value {
    let value = serde_json::to_value(update).unwrap_or_else(|_| serde_json::json!({}));
    let serialized = value.to_string();
    if serialized.chars().count() <= 64_000 {
        return value;
    }
    match crate::runtime::tools::artifacts::store_tool_output(&serialized) {
        Ok(artifact) => serde_json::json!({
            "artifactId": artifact.id,
            "chars": artifact.chars,
            "sha256": artifact.sha256,
            "preview": serialized.chars().take(2_000).collect::<String>(),
        }),
        Err(_) => serde_json::json!({
            "truncated": true,
            "chars": serialized.chars().count(),
            "preview": serialized.chars().take(2_000).collect::<String>(),
        }),
    }
}

async fn await_permission(
    request: &RequestPermissionRequest,
    permission_mode: &str,
) -> Option<agent_client_protocol::schema::PermissionOptionId> {
    // RuntimeTask workers have no attached UI. A native default that asks the
    // user must therefore deny immediately; an explicitly snapshotted
    // non-interactive mode is handled deterministically below.
    if crate::runtime::tools::fs_local::worker_execution_policy().is_some() {
        return select_permission_option(request, unattended_permission_mode(permission_mode));
    }
    if permission_mode != "default" {
        return select_permission_option(request, permission_mode);
    }
    let id = format!("acp_permission_{}", uuid::Uuid::now_v7().simple());
    let session_id = request.session_id.to_string();
    let (reply, receiver) = tokio::sync::oneshot::channel();
    let pending = PendingAcpPermission {
        id: id.clone(),
        session_id: session_id.clone(),
        options: serde_json::to_value(&request.options).unwrap_or_else(|_| serde_json::json!([])),
        created_at: chrono::Utc::now(),
    };
    runtime()
        .permissions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(id.clone(), PermissionWaiter { pending, reply });
    emit_event(
        &session_id,
        "permission_request",
        serde_json::json!({ "permissionId": id, "options": request.options }),
    );
    let selected = tokio::time::timeout(Duration::from_secs(5 * 60), receiver)
        .await
        .ok()
        .and_then(|result| result.ok());
    runtime()
        .permissions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&id);
    selected
        .and_then(|selected| {
            request
                .options
                .iter()
                .find(|option| option.option_id.to_string() == selected)
                .map(|option| option.option_id.clone())
        })
        .or_else(|| select_permission_option(request, "dont_ask"))
}

fn unattended_permission_mode(permission_mode: &str) -> &str {
    if permission_mode == "default" {
        "dont_ask"
    } else {
        permission_mode
    }
}

fn emit_event(session_id: &str, kind: &str, detail: serde_json::Value) -> AcpEvent {
    let id = runtime()
        .next_event_id
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let event = AcpEvent {
        id,
        session_id: session_id.to_string(),
        kind: kind.to_string(),
        detail,
        created_at: chrono::Utc::now(),
    };
    {
        let mut events = runtime()
            .events
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        events.push_back(event.clone());
        while events.len() > 2_000 {
            events.pop_front();
        }
    }
    let _ = runtime().broadcaster.send(event.clone());
    progress::emit(&format!(
        "ACP_EVENT {}",
        serde_json::to_string(&event).unwrap_or_default()
    ));
    event
}

fn implementation_name(program: &str) -> String {
    let name = Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    if name.contains("qoder") {
        "qoder".to_string()
    } else if name.contains("kimi") {
        "kimi".to_string()
    } else if name.contains("codex") {
        "codex".to_string()
    } else {
        name
    }
}

fn select_permission_option(
    request: &RequestPermissionRequest,
    permission_mode: &str,
) -> Option<agent_client_protocol::schema::PermissionOptionId> {
    let wants_deny = matches!(permission_mode, "dont_ask" | "plan" | "read-only");
    let wants_bypass = matches!(permission_mode, "bypass" | "bypass_permissions" | "yolo");
    let named = |option: &&agent_client_protocol::schema::PermissionOption, needles: &[&str]| {
        let value = normalize(&format!("{} {}", option.option_id, option.name));
        needles.iter().any(|needle| value.contains(needle))
    };
    if wants_deny {
        return request
            .options
            .iter()
            .find(|option| {
                matches!(
                    option.kind,
                    PermissionOptionKind::RejectOnce | PermissionOptionKind::RejectAlways
                ) || named(option, &["cancel", "reject", "deny"])
            })
            .map(|option| option.option_id.clone());
    }
    if wants_bypass {
        if let Some(option) = request
            .options
            .iter()
            .find(|option| option.kind == PermissionOptionKind::AllowAlways)
            .or_else(|| {
                request
                    .options
                    .iter()
                    .find(|option| named(option, &["bypass", "always", "session"]))
            })
        {
            return Some(option.option_id.clone());
        }
    }
    request
        .options
        .iter()
        .find(|option| option.kind == PermissionOptionKind::AllowOnce)
        .map(|option| option.option_id.clone())
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn acp_error(context: &str, error: impl std::fmt::Display) -> AcpError {
    AcpError::new(-32603, format!("{context}: {error}"))
}

fn bounded_stderr(value: &str) -> String {
    const LIMIT: usize = 4_000;
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= LIMIT {
        value.to_string()
    } else {
        chars[chars.len() - LIMIT..].iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unattended_default_permission_is_fail_closed() {
        assert_eq!(unattended_permission_mode("default"), "dont_ask");
        assert_eq!(unattended_permission_mode("bypass"), "bypass");
    }

    #[test]
    fn unattended_permission_selection_never_uses_option_order() {
        use agent_client_protocol::schema::{
            PermissionOption, ToolCallUpdate, ToolCallUpdateFields,
        };

        let request = RequestPermissionRequest::new(
            "session",
            ToolCallUpdate::new("tool-call", ToolCallUpdateFields::default()),
            vec![
                PermissionOption::new("always", "Always", PermissionOptionKind::AllowAlways),
                PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce),
                PermissionOption::new("once", "Once", PermissionOptionKind::AllowOnce),
            ],
        );
        assert_eq!(
            select_permission_option(&request, "dont_ask")
                .unwrap()
                .to_string(),
            "deny"
        );
        assert_eq!(
            select_permission_option(&request, "bypass")
                .unwrap()
                .to_string(),
            "always"
        );
        assert_eq!(
            select_permission_option(&request, "auto")
                .unwrap()
                .to_string(),
            "once"
        );
    }

    #[test]
    fn event_journal_uses_monotonic_strict_cursors_and_session_filters() {
        let suffix = uuid::Uuid::now_v7().simple().to_string();
        let first_session = format!("session-a-{suffix}");
        let second_session = format!("session-b-{suffix}");
        let first = emit_event(&first_session, "plan", serde_json::json!({ "step": 1 }));
        let second = emit_event(
            &first_session,
            "usage_update",
            serde_json::json!({ "used": 10, "size": 100 }),
        );
        let other = emit_event(&second_session, "plan", serde_json::json!({ "step": 2 }));

        assert!(first.id < second.id && second.id < other.id);
        let replay = events_after(Some(&first_session), first.id);
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].id, second.id);
        assert_eq!(replay[0].kind, "usage_update");
    }

    #[test]
    fn acp_events_round_trip_for_supervisor_progress_transport() {
        let event = emit_event(
            "native-session",
            "tool_call",
            serde_json::json!({ "title": "read_file" }),
        );
        let serialized = serde_json::to_string(&event).unwrap();
        let decoded: AcpEvent = serde_json::from_str(&serialized).unwrap();
        assert_eq!(decoded.id, event.id);
        assert_eq!(decoded.session_id, "native-session");
        assert_eq!(decoded.detail["title"], "read_file");
    }

    #[test]
    fn kimi_mode_config_maps_research_to_plan() {
        let option: SessionConfigOption = serde_json::from_value(serde_json::json!({
            "type": "select",
            "id": "mode",
            "name": "Mode",
            "category": "mode",
            "currentValue": "default",
            "options": [
                { "value": "default", "name": "Default" },
                { "value": "plan", "name": "Plan" }
            ]
        }))
        .unwrap();
        assert_eq!(
            supported_config_value(Some(&[option]), "mode", &["plan", "read-only"]).as_deref(),
            Some("plan")
        );
    }

    #[test]
    fn qoder_model_display_name_maps_to_the_protocol_value() {
        let option: SessionConfigOption = serde_json::from_value(serde_json::json!({
            "type": "select",
            "id": "model",
            "name": "Model",
            "category": "model",
            "currentValue": "ultimate",
            "options": [
                { "value": "ultimate", "name": "Ultimate" },
                { "value": "performance", "name": "Performance" }
            ]
        }))
        .unwrap();
        assert_eq!(
            canonical_config_value(&option, "Performance"),
            "performance"
        );
        assert_eq!(
            canonical_config_value(&option, "performance"),
            "performance"
        );
    }

    #[test]
    fn unsupported_optional_context_uses_the_agent_default() {
        assert!(requested_config_option(None, "context", false)
            .unwrap()
            .is_none());
        assert!(requested_config_option(None, "model", true).is_err());
    }

    #[test]
    fn thought_chunks_do_not_keep_a_stalled_turn_alive() {
        let thought =
            SessionUpdate::AgentThoughtChunk(agent_client_protocol::schema::ContentChunk::new(
                ContentBlock::Text(TextContent::new("still thinking")),
            ));
        let answer =
            SessionUpdate::AgentMessageChunk(agent_client_protocol::schema::ContentChunk::new(
                ContentBlock::Text(TextContent::new("done")),
            ));

        assert!(!update_resets_inactivity(&thought));
        assert!(update_resets_inactivity(&answer));
    }

    #[test]
    fn ended_turns_are_reusable_but_not_reported_as_active() {
        assert!(is_active_session_status("running"));
        for status in ["idle", "endturn", "cancelled", "expired"] {
            assert!(!is_active_session_status(status), "{status}");
        }
    }
}
