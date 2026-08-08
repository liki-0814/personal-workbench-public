use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::agent_core::runner::PermissionDecision;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPermission {
    pub id: String,
    pub session_id: String,
    pub tool_name: String,
    pub arguments: String,
    pub cwd: String,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

struct PendingEntry {
    request: PendingPermission,
    reply: oneshot::Sender<PermissionDecision>,
}

pub struct PermissionBroker {
    pending: Mutex<HashMap<String, PendingEntry>>,
    resolved: Mutex<HashMap<String, PermissionDecision>>,
    audit_path: PathBuf,
    pending_path: PathBuf,
}

impl PermissionBroker {
    pub fn new(data_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let broker = Self {
            pending: Mutex::new(HashMap::new()),
            resolved: Mutex::new(HashMap::new()),
            audit_path: data_dir.join("permission-audit.log"),
            pending_path: data_dir.join("permission-pending.json"),
        };
        if let Ok(bytes) = std::fs::read(&broker.pending_path) {
            if let Ok(requests) = serde_json::from_slice::<Vec<PendingPermission>>(&bytes) {
                for request in requests {
                    broker.audit(
                        "crash_expired",
                        &request,
                        Some(&PermissionDecision::DenyWithReason(
                            "服务异常退出，旧权限请求已过期".into(),
                        )),
                    );
                }
            }
            let _ = std::fs::remove_file(&broker.pending_path);
        }
        Ok(broker)
    }

    pub async fn request(
        &self,
        session_id: &str,
        tool_name: &str,
        arguments: &str,
        cwd: &str,
        timeout: Duration,
    ) -> PermissionDecision {
        let id = format!("permission_{}", uuid::Uuid::now_v7().simple());
        let created_at = Utc::now();
        let request = PendingPermission {
            id: id.clone(),
            session_id: session_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments: arguments.to_string(),
            cwd: cwd.to_string(),
            version: 1,
            created_at,
            expires_at: created_at
                + chrono::Duration::from_std(timeout)
                    .unwrap_or_else(|_| chrono::Duration::minutes(5)),
        };
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                id.clone(),
                PendingEntry {
                    request: request.clone(),
                    reply: tx,
                },
            );
        self.persist_pending();
        self.audit("requested", &request, None);

        let decision = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(decision)) => decision,
            Ok(Err(_)) => PermissionDecision::DenyWithReason("权限请求已关闭".into()),
            Err(_) => PermissionDecision::DenyWithReason("权限请求超时，默认拒绝".into()),
        };
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&id);
        self.persist_pending();
        self.resolved
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id, decision.clone());
        self.audit("completed", &request, Some(&decision));
        decision
    }

    pub fn pending(&self) -> Vec<PendingPermission> {
        let mut requests: Vec<_> = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .map(|entry| entry.request.clone())
            .collect();
        requests.sort_by_key(|request| request.created_at);
        requests
    }

    pub fn resolve(
        &self,
        id: &str,
        expected_version: i64,
        resolution: &Value,
    ) -> anyhow::Result<()> {
        if self
            .resolved
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(id)
        {
            return Ok(());
        }
        let current_version = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(id)
            .map(|entry| entry.request.version)
            .ok_or_else(|| {
                anyhow::anyhow!("stale_attention: permission request is no longer pending")
            })?;
        if current_version != expected_version {
            return Err(anyhow::anyhow!(
                "stale_attention: permission version changed"
            ));
        }
        let entry = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(id)
            .ok_or_else(|| {
                anyhow::anyhow!("stale_attention: permission request is no longer pending")
            })?;
        self.persist_pending();
        let allow = resolution
            .get("allow")
            .and_then(Value::as_bool)
            .or_else(|| {
                resolution
                    .get("action")
                    .and_then(Value::as_str)
                    .map(|action| action.eq_ignore_ascii_case("allow"))
            })
            .unwrap_or(false);
        let decision = if allow {
            PermissionDecision::Allow
        } else {
            PermissionDecision::DenyWithReason("用户拒绝了工具执行".into())
        };
        let _ = entry.reply.send(decision.clone());
        self.resolved
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id.to_string(), decision.clone());
        self.audit("resolved", &entry.request, Some(&decision));
        Ok(())
    }

    pub fn deny_all(&self, reason: &str) {
        let entries: Vec<_> = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .drain()
            .map(|(_, entry)| entry)
            .collect();
        self.persist_pending();
        for entry in entries {
            let decision = PermissionDecision::DenyWithReason(reason.to_string());
            let _ = entry.reply.send(decision.clone());
            self.audit("shutdown", &entry.request, Some(&decision));
        }
    }

    pub fn deny_session(&self, session_id: &str, reason: &str) {
        let entries: Vec<_> = {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let request_ids: Vec<_> = pending
                .iter()
                .filter(|(_, entry)| entry.request.session_id == session_id)
                .map(|(id, _)| id.clone())
                .collect();
            request_ids
                .into_iter()
                .filter_map(|id| pending.remove(&id))
                .collect()
        };
        self.persist_pending();
        for entry in entries {
            let decision = PermissionDecision::DenyWithReason(reason.to_string());
            let _ = entry.reply.send(decision.clone());
            self.audit("session_cancelled", &entry.request, Some(&decision));
        }
    }

    fn audit(
        &self,
        event: &str,
        request: &PendingPermission,
        decision: Option<&PermissionDecision>,
    ) {
        let decision = decision.map(|value| match value {
            PermissionDecision::Allow => "allow",
            PermissionDecision::Deny | PermissionDecision::DenyWithReason(_) => "deny",
        });
        let record = json!({
            "timestamp": Utc::now(),
            "event": event,
            "permissionRequestId": request.id,
            "sessionId": request.session_id,
            "tool": request.tool_name,
            "cwd": request.cwd,
            "decision": decision,
        });
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.audit_path)
        {
            let _ = writeln!(file, "{record}");
        }
    }

    fn persist_pending(&self) {
        let requests: Vec<_> = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .map(|entry| entry.request.clone())
            .collect();
        if requests.is_empty() {
            let _ = std::fs::remove_file(&self.pending_path);
            return;
        }
        let temp = self.pending_path.with_extension("json.tmp");
        if let Ok(bytes) = serde_json::to_vec_pretty(&requests) {
            if std::fs::write(&temp, bytes).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
                }
                let _ = std::fs::rename(temp, &self.pending_path);
            }
        }
    }
}

impl Drop for PermissionBroker {
    fn drop(&mut self) {
        self.deny_all("服务正常关闭，权限请求已拒绝");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn request_resolves_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let broker = Arc::new(PermissionBroker::new(dir.path()).unwrap());
        let waiting = Arc::clone(&broker);
        let task = tokio::spawn(async move {
            waiting
                .request("s", "write_file", "{}", "/tmp", Duration::from_secs(2))
                .await
        });
        tokio::task::yield_now().await;
        let request = broker.pending().pop().unwrap();
        broker
            .resolve(&request.id, 1, &json!({"allow": true}))
            .unwrap();
        broker
            .resolve(&request.id, 1, &json!({"allow": true}))
            .unwrap();
        assert_eq!(task.await.unwrap(), PermissionDecision::Allow);
    }

    #[tokio::test]
    async fn timeout_denies() {
        let dir = tempfile::tempdir().unwrap();
        let broker = PermissionBroker::new(dir.path()).unwrap();
        let decision = broker
            .request("s", "write", "{}", "/tmp", Duration::from_millis(1))
            .await;
        assert!(matches!(decision, PermissionDecision::DenyWithReason(_)));
    }

    #[tokio::test]
    async fn cancelling_session_denies_only_its_pending_requests() {
        let dir = tempfile::tempdir().unwrap();
        let broker = Arc::new(PermissionBroker::new(dir.path()).unwrap());
        let waiting = Arc::clone(&broker);
        let task = tokio::spawn(async move {
            waiting
                .request(
                    "cancelled",
                    "bash",
                    "{}",
                    "/tmp",
                    Duration::from_secs(2),
                )
                .await
        });
        tokio::task::yield_now().await;

        broker.deny_session("cancelled", "任务已取消，权限请求已拒绝");

        assert!(matches!(
            task.await.unwrap(),
            PermissionDecision::DenyWithReason(reason) if reason.contains("任务已取消")
        ));
        assert!(broker.pending().is_empty());
    }
}
