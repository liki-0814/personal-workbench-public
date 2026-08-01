//! 后台任务管理器（中立层，供 Graph 与 service 共同使用）。
//!
//! 耗时工具（code_agent、SSH 长命令等）可通过 `spawn()` 在后台执行，
//! 用户对话不阻塞。完成后通过 `broadcast::Sender<TaskResult>` 广播结果，
//! SSE 端点 / REPL 监听并通知用户。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::{broadcast, RwLock};
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// 后台任务状态
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// 后台任务事件（广播给所有订阅者）
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TaskEvent {
    #[serde(rename_all = "camelCase")]
    Started {
        task_id: String,
        session_id: String,
        tool_name: String,
        description: String,
    },
    Completed(TaskResult),
    Cancelled(TaskResult),
}

/// 后台任务完成后的结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResult {
    pub task_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub success: bool,
    pub summary: String,
    pub log_file: Option<String>,
    pub duration_secs: u64,
}

/// 可序列化的任务信息（不含 JoinHandle）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    pub id: String,
    pub session_id: String,
    /// 前端传入的 session name（即前端 ChatSession.id），用于回调匹配。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    pub tool_name: String,
    pub description: String,
    pub status: TaskStatus,
    pub elapsed_secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_file: Option<String>,
    pub replayable: bool,
}

#[derive(Clone)]
struct ReplaySpec {
    arguments: Value,
}

/// 内部持有的后台任务
struct BackgroundTask {
    id: String,
    session_id: String,
    tool_name: String,
    description: String,
    started_at: Instant,
    status: TaskStatus,
    handle: JoinHandle<()>,
    replay: Option<ReplaySpec>,
}

pub struct BackgroundTaskManager {
    tasks: RwLock<HashMap<String, BackgroundTask>>,
    results: Arc<RwLock<HashMap<String, TaskResult>>>,
    max_concurrent: usize,
    event_tx: broadcast::Sender<TaskEvent>,
}

impl BackgroundTaskManager {
    pub fn new(max: usize) -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            tasks: RwLock::new(HashMap::new()),
            results: Arc::new(RwLock::new(HashMap::new())),
            max_concurrent: max,
            event_tx: tx,
        }
    }

    /// 获取当前运行中的任务数
    pub async fn running_count(&self) -> usize {
        let guard = self.tasks.read().await;
        guard
            .values()
            .filter(|t| t.status == TaskStatus::Running)
            .count()
    }

    /// spawn 一个后台 tokio task，返回 task_id
    pub async fn spawn<F>(
        &self,
        session_id: String,
        tool_name: String,
        description: String,
        future: F,
    ) -> anyhow::Result<String>
    where
        F: std::future::Future<Output = anyhow::Result<String>> + Send + 'static,
    {
        if self.running_count().await >= self.max_concurrent {
            anyhow::bail!(
                "后台任务已满（上限 {}），请等待现有任务完成",
                self.max_concurrent
            );
        }

        let task_id = format!("bg_{}", chrono::Utc::now().timestamp_millis());

        // 广播 Started 事件
        let _ = self.event_tx.send(TaskEvent::Started {
            task_id: task_id.clone(),
            session_id: session_id.clone(),
            tool_name: tool_name.clone(),
            description: description.clone(),
        });

        let id = task_id.clone();
        let sid = session_id.clone();
        let tname = tool_name.clone();
        let tx = self.event_tx.clone();
        let results_map = Arc::clone(&self.results);
        let started_at = Instant::now();
        let log_dir = log_dir();
        let log_file = log_dir.join(format!("{}.log", id));

        let handle = tokio::spawn(async move {
            let result_str = match future.await {
                Ok(output) => {
                    // 完整输出写日志
                    if let Err(e) = write_log(&log_file, &output) {
                        warn!(task_id = %id, error = %e, "写后台任务日志失败");
                    }
                    // 摘要：最后 30 行
                    let summary = make_summary(&output, &id, started_at.elapsed().as_secs());
                    TaskResult {
                        task_id: id.clone(),
                        session_id: sid,
                        tool_name: tname,
                        success: true,
                        summary,
                        log_file: Some(log_file.to_string_lossy().to_string()),
                        duration_secs: started_at.elapsed().as_secs(),
                    }
                }
                Err(e) => {
                    let err_msg = format!("Error: {}", e);
                    let _ = write_log(&log_file, &err_msg);
                    TaskResult {
                        task_id: id.clone(),
                        session_id: sid,
                        tool_name: tname,
                        success: false,
                        summary: err_msg,
                        log_file: Some(log_file.to_string_lossy().to_string()),
                        duration_secs: started_at.elapsed().as_secs(),
                    }
                }
            };
            info!(
                task_id = %result_str.task_id,
                success = result_str.success,
                duration = result_str.duration_secs,
                "后台任务完成"
            );
            results_map
                .write()
                .await
                .insert(result_str.task_id.clone(), result_str.clone());
            let _ = tx.send(TaskEvent::Completed(result_str));
        });

        let task = BackgroundTask {
            id: task_id.clone(),
            session_id,
            tool_name,
            description,
            started_at,
            status: TaskStatus::Running,
            handle,
            replay: None,
        };
        self.tasks.write().await.insert(task_id.clone(), task);
        Ok(task_id)
    }

    pub async fn spawn_tool(
        &self,
        session_id: String,
        tool_name: String,
        description: String,
        arguments: Value,
        registry: Arc<crate::tools::registry::ToolRegistry>,
    ) -> anyhow::Result<String> {
        let execute_name = tool_name.clone();
        let execute_arguments = arguments.clone();
        let id = self
            .spawn(session_id, tool_name, description, async move {
                registry.execute(&execute_name, &execute_arguments).await
            })
            .await?;
        if let Some(task) = self.tasks.write().await.get_mut(&id) {
            task.replay = Some(ReplaySpec { arguments });
        }
        Ok(id)
    }

    pub async fn retry_tool(
        &self,
        task_id: &str,
        registry: Arc<crate::tools::registry::ToolRegistry>,
    ) -> anyhow::Result<String> {
        let (session_id, tool_name, description, replay) = {
            let tasks = self.tasks.read().await;
            let task = tasks
                .get(task_id)
                .ok_or_else(|| anyhow::anyhow!("background task not found"))?;
            let result = self.results.read().await.get(task_id).cloned();
            if result.as_ref().is_none_or(|result| result.success) {
                anyhow::bail!("background task is not a failed task");
            }
            (
                task.session_id.clone(),
                task.tool_name.clone(),
                task.description.clone(),
                task.replay.clone(),
            )
        };
        let replay = replay.ok_or_else(|| anyhow::anyhow!("background task is not replayable"))?;
        self.spawn_tool(
            session_id,
            tool_name,
            description,
            replay.arguments,
            registry,
        )
        .await
    }

    /// 从一个已 spawn 的 JoinHandle 创建后台任务（用于超时自动转后台场景）。
    ///
    /// `result_rx` 接收工具执行结果，内部 spawn 一个 watcher 监听完成并广播 TaskResult。
    pub async fn adopt(
        &self,
        session_id: String,
        tool_name: String,
        description: String,
        handle: JoinHandle<()>,
        result_rx: tokio::sync::oneshot::Receiver<anyhow::Result<String>>,
    ) -> anyhow::Result<String> {
        if self.running_count().await >= self.max_concurrent {
            anyhow::bail!(
                "后台任务已满（上限 {}），请等待现有任务完成",
                self.max_concurrent
            );
        }

        let task_id = format!("bg_{}", chrono::Utc::now().timestamp_millis());

        // 广播 Started 事件
        let _ = self.event_tx.send(TaskEvent::Started {
            task_id: task_id.clone(),
            session_id: session_id.clone(),
            tool_name: tool_name.clone(),
            description: description.clone(),
        });

        let id = task_id.clone();
        let sid = session_id.clone();
        let tname = tool_name.clone();
        let tx = self.event_tx.clone();
        let results_map = Arc::clone(&self.results);
        let started_at = Instant::now();
        let log_dir = log_dir();
        let log_file = log_dir.join(format!("{}.log", id));

        let watcher = tokio::spawn(async move {
            let _ = handle.await;
            let result_str = match result_rx.await {
                Ok(Ok(output)) => {
                    let _ = write_log(&log_file, &output);
                    let summary = make_summary(&output, &id, started_at.elapsed().as_secs());
                    TaskResult {
                        task_id: id.clone(),
                        session_id: sid,
                        tool_name: tname,
                        success: true,
                        summary,
                        log_file: Some(log_file.to_string_lossy().to_string()),
                        duration_secs: started_at.elapsed().as_secs(),
                    }
                }
                Ok(Err(e)) => {
                    let err_msg = format!("Error: {}", e);
                    let _ = write_log(&log_file, &err_msg);
                    TaskResult {
                        task_id: id.clone(),
                        session_id: sid,
                        tool_name: tname,
                        success: false,
                        summary: err_msg,
                        log_file: Some(log_file.to_string_lossy().to_string()),
                        duration_secs: started_at.elapsed().as_secs(),
                    }
                }
                Err(_) => TaskResult {
                    task_id: id.clone(),
                    session_id: sid,
                    tool_name: tname,
                    success: false,
                    summary: "任务被中断（sender dropped）".to_string(),
                    log_file: None,
                    duration_secs: started_at.elapsed().as_secs(),
                },
            };
            info!(
                task_id = %result_str.task_id,
                success = result_str.success,
                duration = result_str.duration_secs,
                "后台任务完成（adopt）"
            );
            results_map
                .write()
                .await
                .insert(result_str.task_id.clone(), result_str.clone());
            let _ = tx.send(TaskEvent::Completed(result_str));
        });

        let task = BackgroundTask {
            id: task_id.clone(),
            session_id,
            tool_name,
            description,
            started_at,
            status: TaskStatus::Running,
            handle: watcher,
            replay: None,
        };
        self.tasks.write().await.insert(task_id.clone(), task);
        Ok(task_id)
    }

    /// 取消某个 session 的所有运行中后台任务
    pub async fn cancel_by_session(&self, session_id: &str) -> usize {
        let mut guard = self.tasks.write().await;
        let mut cancelled = 0;
        for task in guard.values_mut() {
            if task.session_id == session_id && task.status == TaskStatus::Running {
                task.handle.abort();
                task.status = TaskStatus::Cancelled;
                let _ = self.event_tx.send(TaskEvent::Cancelled(TaskResult {
                    task_id: task.id.clone(),
                    session_id: task.session_id.clone(),
                    tool_name: task.tool_name.clone(),
                    success: false,
                    summary: "会话关闭，任务已取消".to_string(),
                    log_file: None,
                    duration_secs: task.started_at.elapsed().as_secs(),
                }));
                cancelled += 1;
            }
        }
        if cancelled > 0 {
            info!(session_id = %session_id, cancelled, "会话关闭，批量取消后台任务");
        }
        cancelled
    }

    /// 取消后台任务
    pub async fn cancel(&self, task_id: &str) -> bool {
        let mut guard = self.tasks.write().await;
        if let Some(task) = guard.get_mut(task_id) {
            if task.status == TaskStatus::Running {
                task.handle.abort();
                task.status = TaskStatus::Cancelled;
                info!(task_id = %task_id, "后台任务已取消");
                let _ = self.event_tx.send(TaskEvent::Cancelled(TaskResult {
                    task_id: task_id.to_string(),
                    session_id: task.session_id.clone(),
                    tool_name: task.tool_name.clone(),
                    success: false,
                    summary: "任务已取消".to_string(),
                    log_file: None,
                    duration_secs: task.started_at.elapsed().as_secs(),
                }));
                return true;
            }
        }
        false
    }

    /// 列出所有任务
    pub async fn list(&self) -> Vec<TaskInfo> {
        let mut guard = self.tasks.write().await;
        let results = self.results.read().await;
        for task in guard.values_mut() {
            if task.status == TaskStatus::Running && task.handle.is_finished() {
                task.status = TaskStatus::Completed;
            }
        }
        guard
            .values()
            .map(|t| {
                let r = results.get(&t.id);
                TaskInfo {
                    id: t.id.clone(),
                    session_id: t.session_id.clone(),
                    session_name: None,
                    tool_name: t.tool_name.clone(),
                    description: t.description.clone(),
                    status: t.status.clone(),
                    elapsed_secs: t.started_at.elapsed().as_secs(),
                    success: r.map(|r| r.success),
                    summary: r.map(|r| r.summary.clone()),
                    log_file: r.and_then(|r| r.log_file.clone()),
                    replayable: t.replay.is_some(),
                }
            })
            .collect()
    }

    /// 订阅任务事件（Started / Completed / Cancelled）
    pub fn subscribe(&self) -> broadcast::Receiver<TaskEvent> {
        self.event_tx.subscribe()
    }

    /// 移除已完成/已取消的任务记录
    pub async fn remove(&self, task_id: &str) -> bool {
        let mut guard = self.tasks.write().await;
        if let Some(task) = guard.get(task_id) {
            if task.status != TaskStatus::Running {
                guard.remove(task_id);
                self.results.write().await.remove(task_id);
                return true;
            }
        }
        false
    }

    /// 清理超过 24 小时的旧日志
    pub fn cleanup_old_logs() {
        let dir = log_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 3600);
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    if modified < cutoff {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}

fn log_dir() -> PathBuf {
    let dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".pwcli/task-logs");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn write_log(path: &PathBuf, content: &str) -> std::io::Result<()> {
    std::fs::write(path, content)
}

fn make_summary(output: &str, task_id: &str, duration_secs: u64) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let last_30: String = if lines.len() > 30 {
        let skipped = lines.len() - 30;
        format!(
            "…（省略前 {} 行）\n{}",
            skipped,
            lines[lines.len() - 30..].join("\n")
        )
    } else {
        output.to_string()
    };
    format!(
        "后台任务 {} 完成（耗时 {}s）\n\n{}",
        task_id, duration_secs, last_30
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn recv_task_result(rx: &mut broadcast::Receiver<TaskEvent>) -> TaskResult {
        loop {
            match rx.recv().await.unwrap() {
                TaskEvent::Started { .. } => continue,
                TaskEvent::Completed(r) | TaskEvent::Cancelled(r) => return r,
            }
        }
    }

    #[tokio::test]
    async fn spawn_and_complete() {
        let mgr = BackgroundTaskManager::new(4);
        let mut rx = mgr.subscribe();

        let id = mgr
            .spawn(
                "sess1".into(),
                "test_tool".into(),
                "测试任务".into(),
                async { Ok("done".to_string()) },
            )
            .await
            .unwrap();
        assert!(id.starts_with("bg_"));

        let result = recv_task_result(&mut rx).await;
        assert_eq!(result.task_id, id);
        assert!(result.success);
    }

    #[tokio::test]
    async fn cancel_running_task() {
        let mgr = BackgroundTaskManager::new(4);
        let mut rx = mgr.subscribe();

        let id = mgr
            .spawn("sess1".into(), "slow_tool".into(), "慢任务".into(), async {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                Ok("done".to_string())
            })
            .await
            .unwrap();

        assert!(mgr.cancel(&id).await);

        let result = recv_task_result(&mut rx).await;
        assert!(!result.success);
        assert!(result.summary.contains("取消"));
    }

    #[tokio::test]
    async fn max_concurrent_limit() {
        let mgr = BackgroundTaskManager::new(1);

        let _id1 = mgr
            .spawn("s".into(), "t".into(), "d".into(), async {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                Ok("".to_string())
            })
            .await
            .unwrap();

        let err = mgr
            .spawn("s".into(), "t".into(), "d".into(), async {
                Ok("".to_string())
            })
            .await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("已满"));

        // 清理
        mgr.cancel(&_id1).await;
    }

    #[tokio::test]
    async fn list_tasks() {
        let mgr = BackgroundTaskManager::new(4);
        let _id = mgr
            .spawn("s".into(), "t".into(), "d".into(), async {
                Ok("ok".to_string())
            })
            .await
            .unwrap();

        // 让 task 完成
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let tasks = mgr.list().await;
        assert_eq!(tasks.len(), 1);
    }
}
