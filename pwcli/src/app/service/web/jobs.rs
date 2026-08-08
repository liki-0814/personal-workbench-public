use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, SecondsFormat, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::super::state::AppState;

const MAX_LOGS: usize = 20;
const JOB_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const CONFIG_FIELDS: [&str; 9] = [
    "cron",
    "type",
    "command",
    "prompt",
    "model",
    "cwd",
    "enabled",
    "on_missed",
    "group",
];
const FIELDS: [&str; 11] = [
    "cron",
    "type",
    "command",
    "prompt",
    "model",
    "cwd",
    "enabled",
    "on_missed",
    "group",
    "verified_at",
    "spec_hash",
];

#[derive(Clone)]
pub struct JobManager {
    root: PathBuf,
    running: Arc<Mutex<HashMap<String, RunningJob>>>,
}

#[derive(Clone)]
struct RunningJob {
    pid: u32,
    started_at: DateTime<Utc>,
    cancel: CancellationToken,
}

#[derive(Clone)]
struct JobRecord {
    name: String,
    directory: PathBuf,
    file: PathBuf,
    config: Map<String, Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProposalRequest {
    name: String,
    operation: Option<String>,
    test_command: Option<String>,
    approved_test: Option<bool>,
    #[serde(flatten)]
    fields: Map<String, Value>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobProposal {
    id: String,
    operation: String,
    name: String,
    spec_hash: String,
    status: String,
    fields: Map<String, Value>,
    test_command: Option<String>,
    test_exit_code: Option<i32>,
    test_summary: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl JobManager {
    pub fn new() -> Result<Arc<Self>> {
        let root = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".pwcli/jobs");
        let manager = Arc::new(Self {
            root,
            running: Arc::new(Mutex::new(HashMap::new())),
        });
        manager.ensure_dirs()?;
        manager.recover_interrupted_proposals()?;
        manager.start_scheduler();
        Ok(manager)
    }

    fn ensure_dirs(&self) -> Result<()> {
        for path in [
            self.root.clone(),
            self.logs_root(),
            self.backup_root(),
            self.proposals_root(),
        ] {
            fs::create_dir_all(path)?;
        }
        Ok(())
    }

    fn scan(&self) -> Result<Vec<JobRecord>> {
        self.ensure_dirs()?;
        let mut jobs = Vec::new();
        for entry in fs::read_dir(&self.root)?.filter_map(|entry| entry.ok()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !entry.file_type().is_ok_and(|kind| kind.is_dir())
                || name.starts_with('.')
                || name == "logs"
            {
                continue;
            }
            let directory = entry.path();
            let Some(file) = fs::read_dir(&directory)?
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .find(|path| path.extension().and_then(|value| value.to_str()) == Some("job"))
            else {
                continue;
            };
            let config = parse_job(&fs::read_to_string(&file)?);
            if validate_fields(&config).is_ok() {
                jobs.push(JobRecord {
                    name,
                    directory,
                    file,
                    config,
                });
            }
        }
        jobs.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(jobs)
    }

    fn find(&self, name: &str) -> Result<JobRecord> {
        self.scan()?
            .into_iter()
            .find(|job| job.name == name)
            .with_context(|| format!("Job \"{name}\" not found"))
    }

    fn list(&self) -> Result<Vec<Value>> {
        let state = self.read_state();
        let running = self
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        Ok(self
            .scan()?
            .into_iter()
            .map(|job| {
                let enabled = text(&job.config, "enabled") != "false";
                let previous = state.get(&job.name);
                let current = running.get(&job.name);
                let exit_code = previous
                    .and_then(|value| value.get("exitCode"))
                    .and_then(Value::as_i64);
                let status = if current.is_some() {
                    "running"
                } else if !enabled {
                    "disabled"
                } else if exit_code.is_some_and(|code| code != 0) {
                    "error"
                } else {
                    "idle"
                };
                let cron = text(&job.config, "cron");
                json!({
                    "name": job.name,
                    "type": text_or(&job.config, "type", "command"),
                    "enabled": enabled,
                    "cron": cron,
                    "cronHuman": cron_human(cron),
                    "command": optional_text(&job.config, "command"),
                    "prompt": optional_text(&job.config, "prompt"),
                    "model": optional_text(&job.config, "model"),
                    "cwd": optional_text(&job.config, "cwd"),
                    "group": optional_text(&job.config, "group"),
                    "onMissed": text_or(&job.config, "on_missed", "run_once"),
                    "status": status,
                    "lastRun": previous.and_then(|value| value.get("lastRun")).map(|time| json!({
                        "time": time,
                        "exitCode": exit_code.unwrap_or(-1),
                        "duration": previous.and_then(|value| value.get("duration")).and_then(Value::as_u64).unwrap_or(0),
                    })),
                    "nextRun": next_run(cron),
                    "running": current.map(|value| json!({ "startedAt": value.started_at, "pid": value.pid })),
                    "verification": if text(&job.config, "spec_hash").is_empty() { "legacy" } else { "verified" },
                    "verifiedAt": optional_text(&job.config, "verified_at"),
                    "specHash": optional_text(&job.config, "spec_hash"),
                })
            })
            .collect())
    }

    fn create(&self, name: &str, mut fields: Map<String, Value>) -> Result<()> {
        validate_name(name)?;
        fields.entry("type").or_insert_with(|| json!("command"));
        fields.entry("enabled").or_insert_with(|| json!("true"));
        validate_fields(&fields)?;
        let directory = self.root.join(name);
        if directory.exists() {
            anyhow::bail!("Job \"{name}\" already exists");
        }
        fs::create_dir_all(&directory)?;
        write_atomic(
            &directory.join(format!("{name}.job")),
            serialize_job(&fields).as_bytes(),
        )?;
        Ok(())
    }

    fn update(&self, name: &str, updates: Map<String, Value>) -> Result<()> {
        let mut job = self.find(name)?;
        for field in CONFIG_FIELDS {
            if let Some(value) = updates.get(field) {
                job.config.insert(field.into(), value.clone());
            }
        }
        validate_fields(&job.config)?;
        write_atomic(&job.file, serialize_job(&job.config).as_bytes())?;
        Ok(())
    }

    fn proposals_root(&self) -> PathBuf {
        self.root.join(".proposals")
    }

    fn proposal_path(&self, id: &str) -> PathBuf {
        self.proposals_root().join(format!("{id}.json"))
    }

    fn write_proposal(&self, proposal: &JobProposal) -> Result<()> {
        write_atomic(
            &self.proposal_path(&proposal.id),
            &serde_json::to_vec_pretty(proposal)?,
        )
    }

    fn recover_interrupted_proposals(&self) -> Result<()> {
        for entry in fs::read_dir(self.proposals_root())?.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Ok(mut proposal) = fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<JobProposal>(&bytes).ok())
                .ok_or_else(|| anyhow::anyhow!("invalid proposal"))
            else {
                continue;
            };
            if proposal.status == "testing" {
                proposal.status = "test_failed".into();
                proposal.test_summary =
                    Some("daemon 在测试期间重启；正式任务未变更，请由 AI 重新测试".into());
                proposal.updated_at = Utc::now();
                self.write_proposal(&proposal)?;
            } else if proposal.status == "verified" {
                proposal.updated_at = Utc::now();
                match self.apply_verified(&proposal) {
                    Ok(()) => proposal.status = "applied".into(),
                    Err(error) => {
                        proposal.status = "test_failed".into();
                        proposal.test_summary =
                            Some(format!("验证已完成，但恢复应用失败：{error}"));
                    }
                }
                self.write_proposal(&proposal)?;
            }
        }
        Ok(())
    }

    async fn test_proposal(&self, proposal: &JobProposal) -> Result<(i32, String)> {
        let mut config = proposal.fields.clone();
        if let Some(test_command) = proposal.test_command.as_deref() {
            config.insert("command".into(), json!(test_command));
        }
        let job = JobRecord {
            name: format!("proposal-{}", proposal.id),
            directory: self.proposals_root().join(&proposal.id),
            file: self.proposal_path(&proposal.id),
            config,
        };
        fs::create_dir_all(&job.directory)?;
        let cancel = CancellationToken::new();
        if text(&job.config, "type") == "agent" {
            self.execute_agent(&job, cancel).await
        } else {
            self.execute_command(&job, cancel).await
        }
    }

    fn apply_verified(&self, proposal: &JobProposal) -> Result<()> {
        let mut fields = proposal.fields.clone();
        fields.insert("verified_at".into(), json!(Utc::now().to_rfc3339()));
        fields.insert("spec_hash".into(), json!(proposal.spec_hash));
        if proposal.operation == "update" {
            let mut job = self.find(&proposal.name)?;
            let enabled = job.config.get("enabled").cloned();
            for field in CONFIG_FIELDS {
                if let Some(value) = fields.get(field) {
                    job.config.insert(field.into(), value.clone());
                }
            }
            if !proposal.fields.contains_key("enabled") {
                if let Some(value) = enabled {
                    job.config.insert("enabled".into(), value);
                }
            }
            job.config
                .insert("verified_at".into(), fields["verified_at"].clone());
            job.config
                .insert("spec_hash".into(), fields["spec_hash"].clone());
            validate_fields(&job.config)?;
            write_atomic(&job.file, serialize_job(&job.config).as_bytes())
        } else {
            if let Ok(existing) = self.find(&proposal.name) {
                if text(&existing.config, "spec_hash") == proposal.spec_hash.as_str() {
                    return Ok(());
                }
                anyhow::bail!(
                    "Job \"{}\" already exists with a different definition",
                    proposal.name
                );
            }
            self.create(&proposal.name, fields)
        }
    }

    fn toggle(&self, name: &str) -> Result<bool> {
        let job = self.find(name)?;
        let enabled = text(&job.config, "enabled") == "false";
        self.update(name, Map::from_iter([("enabled".into(), json!(enabled))]))?;
        Ok(enabled)
    }

    fn delete(&self, name: &str) -> Result<()> {
        let job = self.find(name)?;
        if let Some(running) = self
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(name)
        {
            running.cancel.cancel();
        }
        let backup = self.backup_root().join(format!("{name}.1"));
        if backup.exists() {
            fs::remove_dir_all(&backup)?;
        }
        fs::rename(job.directory, backup)?;
        let mut state = self.read_state();
        state.remove(name);
        self.write_state(&state)
    }

    fn trigger(self: &Arc<Self>, name: &str) -> Result<()> {
        let job = self.find(name)?;
        let mut running = self
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if running.contains_key(name) {
            return Ok(());
        }
        let cancel = CancellationToken::new();
        let started_at = Utc::now();
        running.insert(
            name.into(),
            RunningJob {
                pid: 0,
                started_at,
                cancel: cancel.clone(),
            },
        );
        drop(running);
        let manager = Arc::clone(self);
        tokio::spawn(async move { manager.execute(job, started_at, cancel).await });
        Ok(())
    }

    async fn execute(
        self: Arc<Self>,
        job: JobRecord,
        started_at: DateTime<Utc>,
        cancel: CancellationToken,
    ) {
        let timer = Instant::now();
        let result = if text(&job.config, "type") == "agent" {
            self.execute_agent(&job, cancel).await
        } else {
            self.execute_command(&job, cancel).await
        };
        let (code, output) = result.unwrap_or_else(|error| (-1, format!("[ERROR] {error}\n")));
        let log = self.log_path(&job.name, started_at);
        if let Some(parent) = log.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(
            log,
            format!(
                "[{}] Starting {} job: {}\n\n{}\n[{}] Exited with code {} ({}ms)\n",
                started_at.to_rfc3339(),
                text_or(&job.config, "type", "command"),
                job.name,
                output,
                Utc::now().to_rfc3339(),
                code,
                timer.elapsed().as_millis()
            ),
        );
        self.running
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&job.name);
        let mut state = self.read_state();
        state.insert(
            job.name.clone(),
            json!({ "lastRun": started_at, "exitCode": code, "duration": timer.elapsed().as_millis() as u64 }),
        );
        let _ = self.write_state(&state);
        let _ = self.trim_logs(&job.name);
    }

    async fn execute_command(
        &self,
        job: &JobRecord,
        cancel: CancellationToken,
    ) -> Result<(i32, String)> {
        let cwd = optional_text(&job.config, "cwd")
            .map(PathBuf::from)
            .unwrap_or_else(|| job.directory.clone());
        let child = Command::new("/bin/sh")
            .args(["-lc", text(&job.config, "command")])
            .current_dir(cwd)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        if let Some(value) = self
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(&job.name)
        {
            value.pid = child.id().unwrap_or_default();
        }
        let output = tokio::select! {
            result = tokio::time::timeout(JOB_TIMEOUT, child.wait_with_output()) => result.context("job timed out")??,
            () = cancel.cancelled() => anyhow::bail!("job cancelled"),
        };
        let mut content = String::from_utf8_lossy(&output.stdout).into_owned();
        content.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok((output.status.code().unwrap_or(-1), content))
    }

    async fn execute_agent(
        &self,
        job: &JobRecord,
        cancel: CancellationToken,
    ) -> Result<(i32, String)> {
        let port = crate::app::config::local_config::get().server.backend_port;
        let session = format!("job-{}-{}", job.name, Utc::now().timestamp_millis());
        let request = reqwest::Client::new()
            .post(format!(
                "http://127.0.0.1:{port}/api/agent/sessions/{session}/stream"
            ))
            .json(&json!({
                "messages": [{ "role": "user", "content": text(&job.config, "prompt") }],
                "cwd": optional_text(&job.config, "cwd"),
            }))
            .send();
        let response = tokio::select! {
            result = tokio::time::timeout(JOB_TIMEOUT, request) => result.context("agent job timed out")??,
            () = cancel.cancelled() => anyhow::bail!("job cancelled"),
        };
        let status = response.status();
        Ok((
            if status.is_success() { 0 } else { 1 },
            response.text().await?,
        ))
    }

    fn start_scheduler(self: &Arc<Self>) {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            interval.tick().await;
            loop {
                interval.tick().await;
                if let Err(error) = manager.tick() {
                    tracing::warn!(%error, "job scheduler tick failed");
                }
            }
        });
    }

    fn tick(self: &Arc<Self>) -> Result<()> {
        let state = self.read_state();
        let now = Utc::now();
        for job in self.scan()? {
            if text(&job.config, "enabled") == "false" {
                continue;
            }
            let last = state
                .get(&job.name)
                .and_then(|value| value.get("lastRun"))
                .and_then(Value::as_str)
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc));
            let minutes = if text(&job.config, "on_missed") == "skip" {
                2
            } else {
                48 * 60
            };
            let after = last.unwrap_or_else(|| now - chrono::Duration::minutes(minutes));
            if parse_schedule(text(&job.config, "cron"))
                .and_then(|schedule| schedule.after(&after).next())
                .is_some_and(|due| due <= now)
            {
                let _ = self.trigger(&job.name);
            }
        }
        Ok(())
    }

    fn log_names(&self, name: &str) -> Vec<String> {
        let mut files = fs::read_dir(self.logs_root().join(name))
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect::<Vec<_>>();
        files.sort_by(|left, right| right.cmp(left));
        files
    }

    fn log_file(&self, name: &str, file: &str) -> Result<PathBuf> {
        validate_name(name)?;
        if file.is_empty() || file.contains("..") || file.contains('/') || file.contains('\\') {
            anyhow::bail!("Invalid log file path");
        }
        Ok(self.logs_root().join(name).join(file))
    }

    fn log_path(&self, name: &str, time: DateTime<Utc>) -> PathBuf {
        self.logs_root()
            .join(name)
            .join(format!("{}.log", time.format("%Y%m%d-%H%M%S")))
    }

    fn trim_logs(&self, name: &str) -> Result<()> {
        for file in self.log_names(name).into_iter().skip(MAX_LOGS) {
            fs::remove_file(self.log_file(name, &file)?)?;
        }
        Ok(())
    }

    fn logs_root(&self) -> PathBuf {
        self.root.join("logs")
    }

    fn backup_root(&self) -> PathBuf {
        self.root.join(".backup")
    }

    fn state_path(&self) -> PathBuf {
        self.root.join(".state.json")
    }

    fn read_state(&self) -> Map<String, Value> {
        fs::read_to_string(self.state_path())
            .ok()
            .and_then(|value| serde_json::from_str::<Value>(&value).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
    }

    fn write_state(&self, state: &Map<String, Value>) -> Result<()> {
        let path = self.state_path();
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(state)?)?;
        fs::rename(temporary, path)?;
        Ok(())
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/jobs", get(list))
        .route("/api/jobs/upload", axum::routing::any(upload_removed))
        .route("/api/jobs/proposals", post(propose))
        .route("/api/jobs/refresh", post(list))
        .route("/api/jobs/{name}", axum::routing::delete(remove))
        .route("/api/jobs/{name}/run", post(run))
        .route("/api/jobs/{name}/toggle", put(toggle))
        .route("/api/jobs/{name}/logs", get(logs))
        .route(
            "/api/jobs/{name}/logs/{file}",
            get(log_content).delete(delete_log),
        )
}

async fn list(State(state): State<AppState>) -> ApiResult {
    ok(json!({ "jobs": manager(&state)?.list().map_err(internal)? }))
}

async fn upload_removed() -> ApiResult {
    Err(api_error(
        StatusCode::NOT_FOUND,
        "ZIP job upload was removed; ask AI to create a tested job",
    ))
}

async fn propose(State(state): State<AppState>, Json(request): Json<ProposalRequest>) -> ApiResult {
    let manager = manager(&state)?;
    validate_name(&request.name)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, &error.to_string()))?;
    let operation = request.operation.as_deref().unwrap_or("create");
    if !matches!(operation, "create" | "update") {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "operation must be create or update",
        ));
    }
    if operation == "create" && manager.find(&request.name).is_ok() {
        return Err(api_error(
            StatusCode::CONFLICT,
            "job already exists; use operation=update",
        ));
    }
    if operation == "update" && manager.find(&request.name).is_err() {
        return Err(api_error(StatusCode::NOT_FOUND, "job not found"));
    }
    let mut fields = normalized_fields(&request.fields);
    if operation == "update" {
        let existing = manager.find(&request.name).map_err(not_found)?;
        let mut merged = Map::new();
        for key in CONFIG_FIELDS {
            if let Some(value) = existing.config.get(key) {
                merged.insert(key.into(), value.clone());
            }
        }
        merged.extend(fields);
        fields = merged;
    } else {
        fields.entry("type").or_insert_with(|| json!("command"));
        fields.entry("enabled").or_insert_with(|| json!("true"));
        fields
            .entry("on_missed")
            .or_insert_with(|| json!("run_once"));
    }
    validate_fields(&fields)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, &error.to_string()))?;
    let now = Utc::now();
    let mut proposal = JobProposal {
        id: format!("job_{}", uuid::Uuid::now_v7().simple()),
        operation: operation.into(),
        name: request.name,
        spec_hash: spec_hash(&fields),
        status: "proposed".into(),
        fields,
        test_command: request
            .test_command
            .filter(|value| !value.trim().is_empty()),
        test_exit_code: None,
        test_summary: None,
        created_at: now,
        updated_at: now,
    };
    if text(&proposal.fields, "type") == "command"
        && proposal.test_command.is_none()
        && request.approved_test != Some(true)
    {
        proposal.status = "awaiting_approval".into();
        proposal.test_summary =
            Some("该测试将真实执行 shell 命令，需确认可能的文件或外部副作用".into());
        manager.write_proposal(&proposal).map_err(internal)?;
        return Err((
            StatusCode::CONFLICT,
            Json(json!({
                "success": false,
                "error": "job_test_approval_required",
                "proposal": proposal,
                "reason": "测试会真实执行 shell 命令",
                "risk": "命令可能写文件或调用外部服务",
                "recommendedAction": "确认测试，或提供无副作用的 testCommand",
                "actions": ["approve_test", "revise"]
            })),
        ));
    }
    proposal.status = "testing".into();
    proposal.updated_at = Utc::now();
    manager.write_proposal(&proposal).map_err(internal)?;
    let (exit_code, output) = match manager.test_proposal(&proposal).await {
        Ok(result) => result,
        Err(error) => {
            proposal.status = "test_failed".into();
            proposal.test_exit_code = Some(-1);
            proposal.test_summary = Some(error.to_string());
            proposal.updated_at = Utc::now();
            manager.write_proposal(&proposal).map_err(internal)?;
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "success": false, "error": "job_test_failed", "proposal": proposal })),
            ));
        }
    };
    proposal.test_exit_code = Some(exit_code);
    proposal.test_summary = Some(truncate_output(&output));
    proposal.updated_at = Utc::now();
    if exit_code != 0 {
        proposal.status = "test_failed".into();
        manager.write_proposal(&proposal).map_err(internal)?;
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "success": false, "error": "job_test_failed", "proposal": proposal })),
        ));
    }
    proposal.status = "verified".into();
    manager.write_proposal(&proposal).map_err(internal)?;
    manager.apply_verified(&proposal).map_err(internal)?;
    proposal.status = "applied".into();
    proposal.updated_at = Utc::now();
    manager.write_proposal(&proposal).map_err(internal)?;
    ok(json!({
        "success": true,
        "proposal": proposal,
        "jobs": manager.list().map_err(internal)?,
    }))
}

async fn run(State(state): State<AppState>, Path(name): Path<String>) -> ApiResult {
    manager(&state)?.trigger(&name).map_err(not_found)?;
    ok(json!({ "success": true, "message": format!("Job \"{name}\" triggered") }))
}

async fn toggle(State(state): State<AppState>, Path(name): Path<String>) -> ApiResult {
    let enabled = manager(&state)?.toggle(&name).map_err(not_found)?;
    ok(json!({ "success": true, "enabled": enabled }))
}

async fn remove(State(state): State<AppState>, Path(name): Path<String>) -> ApiResult {
    manager(&state)?.delete(&name).map_err(not_found)?;
    ok(json!({ "success": true }))
}

async fn logs(State(state): State<AppState>, Path(name): Path<String>) -> ApiResult {
    ok(json!({ "logs": manager(&state)?.log_names(&name) }))
}

async fn log_content(
    State(state): State<AppState>,
    Path((name, file)): Path<(String, String)>,
) -> ApiResult {
    let path = manager(&state)?.log_file(&name, &file).map_err(not_found)?;
    ok(json!({ "content": fs::read_to_string(path).map_err(not_found)? }))
}

async fn delete_log(
    State(state): State<AppState>,
    Path((name, file)): Path<(String, String)>,
) -> ApiResult {
    let path = manager(&state)?.log_file(&name, &file).map_err(not_found)?;
    if path.exists() {
        fs::remove_file(path).map_err(internal)?;
    }
    ok(json!({ "success": true }))
}

type ApiError = (StatusCode, Json<Value>);
type ApiResult = Result<Json<Value>, ApiError>;

fn manager(state: &AppState) -> Result<&Arc<JobManager>, ApiError> {
    state
        .web
        .as_ref()
        .map(|web| &web.jobs)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Web service is disabled"))
}

fn ok(value: Value) -> ApiResult {
    Ok(Json(value))
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    api_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
}

fn not_found(error: impl std::fmt::Display) -> ApiError {
    api_error(StatusCode::NOT_FOUND, &error.to_string())
}

fn api_error(status: StatusCode, message: &str) -> ApiError {
    (status, Json(json!({ "success": false, "error": message })))
}

fn parse_job(content: &str) -> Map<String, Value> {
    let lines = content.lines().collect::<Vec<_>>();
    let mut fields = Map::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        index += 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        let mut value = raw.trim().to_string();
        if let Some(first) = value.strip_prefix("\"\"\"") {
            let mut parts = vec![first.to_string()];
            while index < lines.len() {
                let current = lines[index];
                index += 1;
                if let Some(last) = current.trim_end().strip_suffix("\"\"\"") {
                    parts.push(last.to_string());
                    break;
                }
                parts.push(current.to_string());
            }
            value = parts.join("\n").trim().to_string();
        }
        fields.insert(key.trim().into(), Value::String(value));
    }
    for (key, value) in [
        ("enabled", "true"),
        ("on_missed", "run_once"),
        ("type", "command"),
    ] {
        fields.entry(key).or_insert_with(|| json!(value));
    }
    fields
}

fn serialize_job(fields: &Map<String, Value>) -> String {
    let mut output = String::new();
    for key in FIELDS {
        let value = fields.get(key).map(value_text).unwrap_or_default();
        if value.is_empty() {
            continue;
        }
        if value.contains('\n') {
            output.push_str(&format!("{key} = \"\"\"\n{value}\n\"\"\"\n"));
        } else {
            output.push_str(&format!("{key} = {value}\n"));
        }
    }
    output
}

fn normalized_fields(fields: &Map<String, Value>) -> Map<String, Value> {
    let mut normalized = Map::new();
    for key in CONFIG_FIELDS {
        let Some(value) = fields.get(key) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        normalized.insert(key.into(), Value::String(value_text(value)));
    }
    normalized
}

fn spec_hash(fields: &Map<String, Value>) -> String {
    let mut canonical = String::new();
    for key in CONFIG_FIELDS {
        canonical.push_str(key);
        canonical.push('=');
        canonical.push_str(text(fields, key));
        canonical.push('\n');
    }
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

fn truncate_output(output: &str) -> String {
    const MAX: usize = 4_000;
    if output.chars().count() <= MAX {
        return output.trim().to_string();
    }
    let mut value = output.chars().take(MAX).collect::<String>();
    value.push_str("\n…输出已截断");
    value
}

fn write_atomic(path: &PathBuf, content: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::now_v7().simple()));
    fs::write(&temporary, content)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || !name.chars().all(name_char) {
        anyhow::bail!("Invalid job name (alphanumeric, dash, underscore, dot only)");
    }
    Ok(())
}

fn name_char(value: char) -> bool {
    value.is_ascii_alphanumeric() || "_-.".contains(value)
}

fn validate_fields(fields: &Map<String, Value>) -> Result<()> {
    if text(fields, "cron").is_empty() || parse_schedule(text(fields, "cron")).is_none() {
        anyhow::bail!("cron is required and must be valid");
    }
    match text_or(fields, "type", "command") {
        "command" if text(fields, "command").is_empty() => {
            anyhow::bail!("command is required for type=command")
        }
        "agent" if text(fields, "prompt").is_empty() => {
            anyhow::bail!("prompt is required for type=agent")
        }
        "command" | "agent" => Ok(()),
        _ => anyhow::bail!("type must be one of: command, agent"),
    }
}

fn parse_schedule(value: &str) -> Option<Schedule> {
    let normalized = if value.split_whitespace().count() == 5 {
        format!("0 {value}")
    } else {
        value.to_string()
    };
    Schedule::from_str(&normalized).ok()
}

fn next_run(value: &str) -> Option<String> {
    parse_schedule(value)
        .and_then(|schedule| schedule.upcoming(Utc).next())
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn cron_human(value: &str) -> String {
    let fields = value.split_whitespace().collect::<Vec<_>>();
    if fields.len() == 5 {
        if let Some(minutes) = fields[0].strip_prefix("*/") {
            return format!("每{minutes}分钟");
        }
        if fields[0] == "0" {
            if let Some(hours) = fields[1].strip_prefix("*/") {
                return format!("每{hours}小时");
            }
        }
        if fields[0].chars().all(|value| value.is_ascii_digit())
            && fields[1].chars().all(|value| value.is_ascii_digit())
            && fields[2..] == ["*", "*", "*"]
        {
            return format!("每天 {:0>2}:{:0>2}", fields[1], fields[0]);
        }
    }
    value.to_string()
}

fn text<'a>(fields: &'a Map<String, Value>, key: &str) -> &'a str {
    fields.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn text_or<'a>(fields: &'a Map<String, Value>, key: &str, fallback: &'a str) -> &'a str {
    let value = text(fields, key);
    if value.is_empty() {
        fallback
    } else {
        value
    }
}

fn optional_text(fields: &Map<String, Value>, key: &str) -> Option<String> {
    let value = text(fields, key);
    (!value.is_empty()).then(|| value.to_string())
}

fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiline_jobs() {
        let parsed =
            parse_job("cron = */5 * * * *\ntype = agent\nprompt = \"\"\"\nhello\nworld\n\"\"\"\n");
        assert_eq!(text(&parsed, "prompt"), "hello\nworld");
        assert!(serialize_job(&parsed).contains("prompt = \"\"\""));
    }

    #[test]
    fn validates_names_and_cron() {
        assert!(validate_name("sync-data.1").is_ok());
        assert!(validate_name("../escape").is_err());
        assert!(parse_schedule("*/5 * * * *").is_some());
    }

    #[test]
    fn spec_hash_is_stable_and_bound_to_definition() {
        let first = normalized_fields(&Map::from_iter([
            ("cron".into(), json!("0 9 * * *")),
            ("type".into(), json!("command")),
            ("command".into(), json!("echo ok")),
        ]));
        let mut changed = first.clone();
        changed.insert("command".into(), json!("echo changed"));
        assert_eq!(spec_hash(&first), spec_hash(&first.clone()));
        assert_ne!(spec_hash(&first), spec_hash(&changed));
    }
}
