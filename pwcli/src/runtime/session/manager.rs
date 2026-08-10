use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use crate::agent_core::harness::{
    HarnessAuditSink, HarnessFingerprint, HarnessSpec, MiddlewareIntervention,
};
use crate::ai::llm::ChatMessage;
use crate::runtime::session::{
    JournalEntryData, QueuedInput, QueuedInputDelivery, QueuedInputPriority, QueuedInputSource,
    QueuedInputStatus, Session, SessionJournal, SessionRuntimeSnapshot, SessionState,
    WorkspaceBinding,
};
use async_trait::async_trait;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchEntryInfo {
    pub id: uuid::Uuid,
    pub parent_id: Option<uuid::Uuid>,
    pub kind: String,
    pub preview: String,
    pub current: bool,
}

/// 多租户会话管理器
///
/// 使用 `Arc<RwLock<HashMap>>` 实现线程安全的会话存储，
/// 支持并发读、独占写。
#[derive(Debug, Clone)]
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    runtimes: Arc<RwLock<HashMap<String, SessionRuntimeSnapshot>>>,
    journal_dir: Arc<PathBuf>,
    tombstone_dir: Arc<PathBuf>,
    harness_spec_lock: Arc<Mutex<()>>,
    runtime_tx: tokio::sync::broadcast::Sender<SessionRuntimeSnapshot>,
}

pub struct SessionJournalAuditSink {
    manager: SessionManager,
    session_id: String,
}

struct QueuedInputOptions {
    source: QueuedInputSource,
    priority: QueuedInputPriority,
    image_urls: Vec<String>,
    file_references: Vec<serde_json::Value>,
}

impl SessionJournalAuditSink {
    pub fn new(manager: SessionManager, session_id: impl Into<String>) -> Self {
        Self {
            manager,
            session_id: session_id.into(),
        }
    }
}

#[async_trait]
impl HarnessAuditSink for SessionJournalAuditSink {
    async fn record_spec(
        &self,
        spec: &HarnessSpec,
        fingerprint: &HarnessFingerprint,
    ) -> anyhow::Result<()> {
        self.manager
            .append_harness_spec_once(&self.session_id, spec, fingerprint)
    }

    async fn record_intervention(
        &self,
        intervention: &MiddlewareIntervention,
    ) -> anyhow::Result<()> {
        self.manager.append_custom(
            &self.session_id,
            "middleware_intervention_v1",
            serde_json::to_value(intervention)?,
        )
    }
}

impl SessionManager {
    /// 创建一个新的空 SessionManager
    pub fn new() -> Self {
        let base_dir = dirs::home_dir()
            .expect("home dir not found")
            .join(".pwcli")
            .join("sessions");
        Self::new_in(base_dir)
    }

    pub fn new_in(base_dir: impl Into<PathBuf>) -> Self {
        let base_dir = base_dir.into();
        cleanup_legacy_session_json(&base_dir);
        let (runtime_tx, _) = tokio::sync::broadcast::channel(512);
        let manager = Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            runtimes: Arc::new(RwLock::new(HashMap::new())),
            journal_dir: Arc::new(base_dir.join("journal")),
            tombstone_dir: Arc::new(base_dir.join("tombstones")),
            harness_spec_lock: Arc::new(Mutex::new(())),
            runtime_tx,
        };
        manager.restore_journals();
        manager
    }

    /// 创建新会话，存入管理器，返回会话 ID
    pub fn create(&self, name: impl Into<String>) -> String {
        let session = Session::new(name);
        let id = session.id.clone();
        if let Err(error) = self.sync_journal(&session) {
            tracing::warn!(session_id = %id, error = %error, "create session journal failed");
        }
        {
            let mut guard = self.sessions.write().unwrap();
            guard.insert(id.clone(), session);
        }
        self.runtimes
            .write()
            .unwrap()
            .insert(id.clone(), SessionRuntimeSnapshot::idle(&id));
        id
    }

    pub fn create_in_workspace(
        &self,
        name: impl Into<String>,
        canonical_path: PathBuf,
        display_path: PathBuf,
    ) -> anyhow::Result<String> {
        let session = Session::new_in_workspace(name, canonical_path, display_path);
        let id = session.id.clone();
        self.sync_journal(&session)?;
        self.sessions.write().unwrap().insert(id.clone(), session);
        self.runtimes
            .write()
            .unwrap()
            .insert(id.clone(), SessionRuntimeSnapshot::idle(&id));
        Ok(id)
    }

    pub fn create_in_workspace_with_id(
        &self,
        id: &str,
        name: impl Into<String>,
        canonical_path: PathBuf,
        display_path: PathBuf,
    ) -> anyhow::Result<String> {
        if let Some(existing) = self.get(id) {
            let existing_path = existing
                .workspace
                .as_ref()
                .map(|workspace| &workspace.canonical_path);
            if existing_path != Some(&canonical_path) {
                anyhow::bail!("capture session workspace does not match the original request");
            }
            return Ok(id.to_string());
        }
        let mut session = Session::new_in_workspace(name, canonical_path, display_path);
        session.id = id.to_string();
        self.sync_journal(&session)?;
        self.sessions
            .write()
            .unwrap()
            .insert(id.to_string(), session);
        self.runtimes
            .write()
            .unwrap()
            .entry(id.to_string())
            .or_insert_with(|| SessionRuntimeSnapshot::idle(id));
        Ok(id.to_string())
    }

    /// 获取会话的克隆副本
    pub fn get(&self, id: &str) -> Option<Session> {
        let guard = self.sessions.read().unwrap();
        let mut session = guard.get(id).cloned()?;
        drop(guard);
        if let Ok(journal) = SessionJournal::open(self.journal_path(id)) {
            if let Ok(messages) = journal.active_messages() {
                session.messages = messages;
            }
        }
        Some(session)
    }

    pub fn runtime(&self, id: &str) -> Option<SessionRuntimeSnapshot> {
        self.runtimes.read().unwrap().get(id).cloned()
    }

    pub fn subscribe_runtime(&self) -> tokio::sync::broadcast::Receiver<SessionRuntimeSnapshot> {
        self.runtime_tx.subscribe()
    }

    pub fn mark_turn_started(&self, id: &str) -> anyhow::Result<String> {
        let turn_id = format!("turn_{}", uuid::Uuid::now_v7().simple());
        {
            let mut sessions = self.sessions.write().unwrap();
            let session = sessions
                .get_mut(id)
                .ok_or_else(|| anyhow::anyhow!("session not found"))?;
            session.lock_workspace();
            session.state = SessionState::Ready;
        }
        let snapshot = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .entry(id.to_string())
                .or_insert_with(|| SessionRuntimeSnapshot::idle(id));
            if runtime.phase != "idle" {
                anyhow::bail!("session already has an active turn");
            }
            runtime.phase = "running".to_string();
            runtime.active_turn_id = Some(turn_id.clone());
            runtime.last_event_sequence += 1;
            runtime.clone()
        };
        self.persist_runtime(&snapshot)?;
        self.persist_session_workspace(id)?;
        Ok(turn_id)
    }

    pub fn mark_turn_settled(&self, id: &str, failed: bool) -> anyhow::Result<()> {
        let snapshot = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            runtime.active_turn_id = None;
            if failed {
                runtime.paused = true;
                for item in &mut runtime.queue {
                    if matches!(
                        item.status,
                        QueuedInputStatus::Queued | QueuedInputStatus::WaitingSafePoint
                    ) {
                        item.status = QueuedInputStatus::Paused;
                    }
                }
            }
            runtime.phase = if runtime.paused { "paused" } else { "idle" }.to_string();
            runtime.last_event_sequence += 1;
            runtime.clone()
        };
        self.persist_runtime(&snapshot)
    }

    pub fn enqueue_input(
        &self,
        id: &str,
        client_message_id: String,
        delivery: QueuedInputDelivery,
        message: ChatMessage,
        image_urls: Vec<String>,
        file_references: Vec<serde_json::Value>,
    ) -> anyhow::Result<QueuedInput> {
        self.enqueue_input_with_metadata(
            id,
            client_message_id,
            delivery,
            message,
            QueuedInputOptions {
                source: QueuedInputSource::User,
                priority: QueuedInputPriority::Normal,
                image_urls,
                file_references,
            },
        )
    }

    pub fn enqueue_runtime_callback(
        &self,
        id: &str,
        dedupe_key: String,
        message: ChatMessage,
    ) -> anyhow::Result<Option<QueuedInput>> {
        if self.runtime_callback_consumed(id, &dedupe_key)? {
            return Ok(None);
        }
        self.enqueue_input_with_metadata(
            id,
            dedupe_key,
            QueuedInputDelivery::NextTurn,
            message,
            QueuedInputOptions {
                source: QueuedInputSource::RuntimeCallback,
                priority: QueuedInputPriority::High,
                image_urls: Vec::new(),
                file_references: Vec::new(),
            },
        )
        .map(Some)
    }

    fn enqueue_input_with_metadata(
        &self,
        id: &str,
        client_message_id: String,
        mut delivery: QueuedInputDelivery,
        message: ChatMessage,
        options: QueuedInputOptions,
    ) -> anyhow::Result<QueuedInput> {
        let mut session = self
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        if session
            .workspace
            .as_ref()
            .is_some_and(|workspace| workspace.locked_at.is_none())
        {
            session.lock_workspace();
            self.update(session);
        }
        let (item, snapshot) = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .entry(id.to_string())
                .or_insert_with(|| SessionRuntimeSnapshot::idle(id));
            if let Some(existing) = runtime
                .queue
                .iter()
                .find(|item| item.client_message_id == client_message_id)
            {
                return Ok(existing.clone());
            }
            if delivery == QueuedInputDelivery::Guidance && runtime.active_turn_id.is_none() {
                delivery = QueuedInputDelivery::NextTurn;
            }
            let status = if runtime.paused {
                QueuedInputStatus::Paused
            } else if delivery == QueuedInputDelivery::Guidance {
                QueuedInputStatus::WaitingSafePoint
            } else {
                QueuedInputStatus::Queued
            };
            let item = QueuedInput {
                id: format!("queue_{}", uuid::Uuid::now_v7().simple()),
                client_message_id,
                sequence: 0,
                delivery,
                status,
                source: options.source,
                priority: options.priority,
                message,
                image_urls: options.image_urls,
                file_references: options.file_references,
                target_turn_id: runtime.active_turn_id.clone(),
                claimed_by_turn_id: None,
                created_at: chrono::Utc::now(),
            };
            let claimed_boundary = runtime
                .queue
                .iter()
                .rposition(|queued| queued.status == QueuedInputStatus::Claimed)
                .map(|index| index + 1)
                .unwrap_or(0);
            let insertion = if options.priority == QueuedInputPriority::High {
                runtime.queue[claimed_boundary..]
                    .iter()
                    .position(|queued| queued.priority == QueuedInputPriority::Normal)
                    .map(|index| index + claimed_boundary)
                    .unwrap_or(runtime.queue.len())
            } else {
                runtime.queue.len()
            };
            runtime.queue.insert(insertion, item);
            for (index, queued) in runtime.queue.iter_mut().enumerate() {
                queued.sequence = index as u64 + 1;
            }
            let item = runtime.queue[insertion].clone();
            runtime.last_event_sequence += 1;
            (item, runtime.clone())
        };
        self.persist_runtime(&snapshot)?;
        Ok(item)
    }

    pub fn replace_queue_item(
        &self,
        session_id: &str,
        item_id: &str,
        content: Option<String>,
        position: Option<usize>,
        delivery: Option<QueuedInputDelivery>,
    ) -> anyhow::Result<QueuedInput> {
        let (updated, snapshot) = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            let index = runtime
                .queue
                .iter()
                .position(|item| item.id == item_id)
                .ok_or_else(|| anyhow::anyhow!("queue item not found"))?;
            if !matches!(
                runtime.queue[index].status,
                QueuedInputStatus::Queued
                    | QueuedInputStatus::Paused
                    | QueuedInputStatus::WaitingSafePoint
                    | QueuedInputStatus::Failed
            ) {
                anyhow::bail!("queue item can no longer be edited");
            }
            if runtime.queue[index].source == QueuedInputSource::RuntimeCallback {
                anyhow::bail!("runtime callback queue items cannot be edited");
            }
            if let Some(content) = content {
                if content.trim().is_empty() {
                    anyhow::bail!("queue item content cannot be empty");
                }
                runtime.queue[index].message.content = content;
            }
            if let Some(delivery) = delivery {
                runtime.queue[index].delivery = delivery;
                runtime.queue[index].target_turn_id = if delivery == QueuedInputDelivery::Guidance {
                    runtime.active_turn_id.clone()
                } else {
                    None
                };
                runtime.queue[index].status = if runtime.paused {
                    QueuedInputStatus::Paused
                } else if delivery == QueuedInputDelivery::Guidance {
                    QueuedInputStatus::WaitingSafePoint
                } else {
                    QueuedInputStatus::Queued
                };
            }
            if let Some(position) = position {
                let item = runtime.queue.remove(index);
                let protected_prefix = runtime
                    .queue
                    .iter()
                    .enumerate()
                    .filter(|(_, queued)| {
                        queued.status == QueuedInputStatus::Claimed
                            || queued.priority == QueuedInputPriority::High
                    })
                    .map(|(index, _)| index + 1)
                    .max()
                    .unwrap_or(0);
                let target = position.min(runtime.queue.len()).max(protected_prefix);
                runtime.queue.insert(target, item);
                for (sequence, item) in runtime.queue.iter_mut().enumerate() {
                    item.sequence = sequence as u64 + 1;
                }
            }
            let updated = runtime
                .queue
                .iter()
                .find(|item| item.id == item_id)
                .cloned()
                .expect("updated queue item disappeared");
            runtime.last_event_sequence += 1;
            (updated, runtime.clone())
        };
        self.persist_runtime(&snapshot)?;
        Ok(updated)
    }

    pub fn delete_queue_item(&self, session_id: &str, item_id: &str) -> anyhow::Result<()> {
        let snapshot = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            let index = runtime
                .queue
                .iter()
                .position(|item| item.id == item_id)
                .ok_or_else(|| anyhow::anyhow!("queue item not found"))?;
            if runtime.queue[index].status == QueuedInputStatus::Claimed {
                anyhow::bail!("message has already started sending");
            }
            if runtime.queue[index].source == QueuedInputSource::RuntimeCallback {
                anyhow::bail!("runtime callback queue items cannot be deleted");
            }
            runtime.queue.remove(index);
            for (sequence, item) in runtime.queue.iter_mut().enumerate() {
                item.sequence = sequence as u64 + 1;
            }
            runtime.last_event_sequence += 1;
            runtime.clone()
        };
        self.persist_runtime(&snapshot)
    }

    /// Atomically remove the next executable durable input. Queue editing,
    /// deletion, ordering and execution now contend on this single runtime
    /// lock instead of racing an independent Harness copy.
    pub fn take_next_queued_input(&self, session_id: &str) -> anyhow::Result<Option<QueuedInput>> {
        let (taken, snapshot) = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            if runtime.paused {
                return Ok(None);
            }
            let Some(index) = runtime.queue.iter().position(|item| {
                item.delivery == QueuedInputDelivery::NextTurn
                    && item.status == QueuedInputStatus::Queued
            }) else {
                return Ok(None);
            };
            let taken = runtime.queue.remove(index);
            for (sequence, item) in runtime.queue.iter_mut().enumerate() {
                item.sequence = sequence as u64 + 1;
            }
            runtime.last_event_sequence += 1;
            (taken, runtime.clone())
        };
        if taken.source == QueuedInputSource::RuntimeCallback {
            self.append_custom(
                session_id,
                "runtime_callback_consumed_v1",
                serde_json::json!({ "dedupeKey": taken.client_message_id }),
            )?;
        }
        self.persist_runtime(&snapshot)?;
        Ok(Some(taken))
    }

    /// Reserve the next durable input without removing it. The caller must
    /// acknowledge it with `consume_queued_input_by_id` after a successful
    /// turn, or mark it failed so it remains visible and retryable.
    pub fn claim_next_queued_input(&self, session_id: &str) -> anyhow::Result<Option<QueuedInput>> {
        self.claim_next_queued_input_inner(session_id, false)
    }

    /// Final-boundary variant of `claim_next_queued_input`. When no input is
    /// available it atomically settles the session, closing the race where an
    /// item arrives between the last queue check and the idle transition.
    pub fn claim_next_queued_input_or_settle(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<QueuedInput>> {
        self.claim_next_queued_input_inner(session_id, true)
    }

    fn claim_next_queued_input_inner(
        &self,
        session_id: &str,
        settle_when_empty: bool,
    ) -> anyhow::Result<Option<QueuedInput>> {
        let (claimed, snapshot) = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            let claimed = if runtime.paused {
                None
            } else {
                runtime.queue.iter_mut().find_map(|item| {
                    (item.delivery == QueuedInputDelivery::NextTurn
                        && item.status == QueuedInputStatus::Queued)
                        .then(|| {
                            item.status = QueuedInputStatus::Claimed;
                            item.claimed_by_turn_id = runtime.active_turn_id.clone();
                            item.clone()
                        })
                })
            };
            if claimed.is_none() && !settle_when_empty {
                return Ok(None);
            }
            if claimed.is_none() && settle_when_empty {
                runtime.active_turn_id = None;
                runtime.phase = if runtime.paused { "paused" } else { "idle" }.to_string();
            }
            runtime.last_event_sequence += 1;
            (claimed, runtime.clone())
        };
        self.persist_runtime(&snapshot)?;
        Ok(claimed)
    }

    pub fn mark_claimed_input_failed(&self, session_id: &str, item_id: &str) -> anyhow::Result<()> {
        let snapshot = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            let item = runtime
                .queue
                .iter_mut()
                .find(|item| item.id == item_id)
                .ok_or_else(|| anyhow::anyhow!("queue item not found"))?;
            if item.status != QueuedInputStatus::Claimed {
                anyhow::bail!("queue item is not claimed");
            }
            item.status = QueuedInputStatus::Failed;
            item.claimed_by_turn_id = None;
            runtime.last_event_sequence += 1;
            runtime.clone()
        };
        self.persist_runtime(&snapshot)
    }

    /// Atomically re-check the durable queue at the final turn boundary. If a
    /// message arrived after the runner's previous empty check, keep the turn
    /// alive and return it. Otherwise release the session to `idle` (or
    /// `paused`) in the same critical section.
    pub fn take_next_queued_input_or_settle(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<QueuedInput>> {
        let (taken, snapshot) = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            let index = (!runtime.paused).then(|| {
                runtime.queue.iter().position(|item| {
                    item.delivery == QueuedInputDelivery::NextTurn
                        && item.status == QueuedInputStatus::Queued
                })
            });
            let taken = index.flatten().map(|index| runtime.queue.remove(index));
            if taken.is_some() {
                for (sequence, item) in runtime.queue.iter_mut().enumerate() {
                    item.sequence = sequence as u64 + 1;
                }
            } else {
                runtime.active_turn_id = None;
                runtime.phase = if runtime.paused { "paused" } else { "idle" }.to_string();
            }
            runtime.last_event_sequence += 1;
            (taken, runtime.clone())
        };
        if let Some(item) = taken
            .as_ref()
            .filter(|item| item.source == QueuedInputSource::RuntimeCallback)
        {
            self.append_custom(
                session_id,
                "runtime_callback_consumed_v1",
                serde_json::json!({ "dedupeKey": item.client_message_id }),
            )?;
        }
        self.persist_runtime(&snapshot)?;
        Ok(taken)
    }

    pub fn claim_queued_input_for_activation(
        &self,
        session_id: &str,
        item_id: &str,
    ) -> anyhow::Result<bool> {
        self.claim_queued_input_for_activation_from(session_id, item_id, None)
    }

    fn claim_queued_input_for_activation_from(
        &self,
        session_id: &str,
        item_id: &str,
        required_source: Option<QueuedInputSource>,
    ) -> anyhow::Result<bool> {
        let snapshot = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            if runtime.paused || runtime.phase != "idle" || runtime.active_turn_id.is_some() {
                return Ok(false);
            }
            let Some(item) = runtime.queue.iter_mut().find(|item| item.id == item_id) else {
                return Ok(false);
            };
            if item.delivery != QueuedInputDelivery::NextTurn
                || item.status != QueuedInputStatus::Queued
                || required_source.is_some_and(|source| item.source != source)
            {
                return Ok(false);
            }
            item.status = QueuedInputStatus::Claimed;
            item.claimed_by_turn_id = None;
            runtime.last_event_sequence += 1;
            runtime.clone()
        };
        self.persist_runtime(&snapshot)?;
        Ok(true)
    }

    /// Fence an idle callback activation before its message is passed as the
    /// first input to `chat`. A claimed callback is invisible to the active
    /// runner's normal durable-queue take, preventing the same callback from
    /// being executed a second time.
    pub fn claim_runtime_callback_for_activation(
        &self,
        session_id: &str,
        item_id: &str,
    ) -> anyhow::Result<bool> {
        self.claim_queued_input_for_activation_from(
            session_id,
            item_id,
            Some(QueuedInputSource::RuntimeCallback),
        )
    }

    /// Restore a callback claim when activation did not start. A user turn
    /// winning the idle-to-running race uses `pause = false`; terminal launch
    /// failures use `pause = true` so the callback remains durable but awaits
    /// explicit recovery.
    pub fn restore_claimed_runtime_callback(
        &self,
        session_id: &str,
        item_id: &str,
        pause: bool,
    ) -> anyhow::Result<()> {
        self.restore_claimed_input_for_activation_from(
            session_id,
            item_id,
            pause,
            Some(QueuedInputSource::RuntimeCallback),
        )
    }

    pub fn restore_claimed_input_for_activation(
        &self,
        session_id: &str,
        item_id: &str,
        pause: bool,
    ) -> anyhow::Result<()> {
        self.restore_claimed_input_for_activation_from(session_id, item_id, pause, None)
    }

    fn restore_claimed_input_for_activation_from(
        &self,
        session_id: &str,
        item_id: &str,
        pause: bool,
        required_source: Option<QueuedInputSource>,
    ) -> anyhow::Result<()> {
        let snapshot = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            let item = runtime
                .queue
                .iter_mut()
                .find(|item| item.id == item_id)
                .ok_or_else(|| anyhow::anyhow!("queue item not found"))?;
            if item.status != QueuedInputStatus::Claimed
                || required_source.is_some_and(|source| item.source != source)
            {
                anyhow::bail!("queued input is not claimed");
            }
            item.status = if pause {
                QueuedInputStatus::Paused
            } else {
                QueuedInputStatus::Queued
            };
            item.claimed_by_turn_id = None;
            if pause {
                runtime.paused = true;
                if runtime.active_turn_id.is_none() {
                    runtime.phase = "paused".to_string();
                }
            }
            runtime.last_event_sequence += 1;
            runtime.clone()
        };
        self.persist_runtime(&snapshot)
    }

    pub fn set_queue_paused(&self, session_id: &str, paused: bool) -> anyhow::Result<()> {
        let snapshot = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            runtime.paused = paused;
            for item in &mut runtime.queue {
                if item.status == QueuedInputStatus::WaitingSafePoint && paused {
                    item.delivery = QueuedInputDelivery::NextTurn;
                    item.target_turn_id = None;
                }
                if matches!(
                    item.status,
                    QueuedInputStatus::Queued
                        | QueuedInputStatus::Paused
                        | QueuedInputStatus::WaitingSafePoint
                ) {
                    item.status = if paused {
                        QueuedInputStatus::Paused
                    } else {
                        QueuedInputStatus::Queued
                    };
                }
            }
            if runtime.active_turn_id.is_none() {
                runtime.phase = if paused { "paused" } else { "idle" }.to_string();
            }
            runtime.last_event_sequence += 1;
            runtime.clone()
        };
        self.persist_runtime(&snapshot)
    }

    pub fn consume_queued_input(
        &self,
        session_id: &str,
        delivery: QueuedInputDelivery,
        content: &str,
    ) -> anyhow::Result<()> {
        let snapshot = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            if let Some(index) = runtime
                .queue
                .iter()
                .position(|item| item.delivery == delivery && item.message.content == content)
            {
                runtime.queue.remove(index);
                runtime.last_event_sequence += 1;
            }
            runtime.clone()
        };
        self.persist_runtime(&snapshot)
    }

    pub fn consume_queued_input_by_id(
        &self,
        session_id: &str,
        item_id: &str,
    ) -> anyhow::Result<Option<QueuedInput>> {
        let (consumed, snapshot) = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            let consumed = runtime
                .queue
                .iter()
                .position(|item| item.id == item_id)
                .map(|index| runtime.queue.remove(index));
            if consumed.is_some() {
                for (index, queued) in runtime.queue.iter_mut().enumerate() {
                    queued.sequence = index as u64 + 1;
                }
                runtime.last_event_sequence += 1;
            }
            (consumed, runtime.clone())
        };
        if let Some(item) = consumed
            .as_ref()
            .filter(|item| item.source == QueuedInputSource::RuntimeCallback)
        {
            self.append_custom(
                session_id,
                "runtime_callback_consumed_v1",
                serde_json::json!({ "dedupeKey": item.client_message_id }),
            )?;
        }
        self.persist_runtime(&snapshot)?;
        Ok(consumed)
    }

    pub fn requeue_claimed_input(&self, session_id: &str, item_id: &str) -> anyhow::Result<()> {
        let snapshot = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            let item = runtime
                .queue
                .iter_mut()
                .find(|item| item.id == item_id)
                .ok_or_else(|| anyhow::anyhow!("queue item not found"))?;
            if item.status != QueuedInputStatus::Claimed {
                anyhow::bail!("queue item is not claimed");
            }
            item.status = QueuedInputStatus::Paused;
            item.claimed_by_turn_id = None;
            runtime.paused = true;
            runtime.phase = "paused".to_string();
            runtime.last_event_sequence += 1;
            runtime.clone()
        };
        self.persist_runtime(&snapshot)
    }

    pub fn reconcile_guidance(
        &self,
        session_id: &str,
        remaining_queue_item_ids: &[String],
    ) -> anyhow::Result<()> {
        let snapshot = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            runtime.queue.retain_mut(|item| {
                if item.delivery != QueuedInputDelivery::Guidance {
                    return true;
                }
                if remaining_queue_item_ids.contains(&item.id) {
                    item.delivery = QueuedInputDelivery::NextTurn;
                    item.target_turn_id = None;
                    item.status = if runtime.paused {
                        QueuedInputStatus::Paused
                    } else {
                        QueuedInputStatus::Queued
                    };
                    true
                } else {
                    false
                }
            });
            for (sequence, item) in runtime.queue.iter_mut().enumerate() {
                item.sequence = sequence as u64 + 1;
            }
            runtime.last_event_sequence += 1;
            runtime.clone()
        };
        self.persist_runtime(&snapshot)
    }

    pub fn clear_queue(&self, session_id: &str) -> anyhow::Result<()> {
        let snapshot = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            runtime.queue.retain(|item| {
                item.status == QueuedInputStatus::Claimed
                    || item.source == QueuedInputSource::RuntimeCallback
            });
            runtime.last_event_sequence += 1;
            runtime.clone()
        };
        self.persist_runtime(&snapshot)
    }

    pub fn release_next_queued(
        &self,
        session_id: &str,
        resume_all: bool,
    ) -> anyhow::Result<Option<QueuedInput>> {
        let (released, snapshot) = {
            let mut runtimes = self.runtimes.write().unwrap();
            let runtime = runtimes
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?;
            let index = runtime.queue.iter().position(|item| {
                item.delivery == QueuedInputDelivery::NextTurn
                    && item.status != QueuedInputStatus::Claimed
            });
            let released = index.map(|index| {
                if runtime.queue[index].source == QueuedInputSource::RuntimeCallback {
                    runtime.queue[index].status = QueuedInputStatus::Claimed;
                    runtime.queue[index].claimed_by_turn_id = runtime.active_turn_id.clone();
                    runtime.queue[index].clone()
                } else {
                    runtime.queue.remove(index)
                }
            });
            runtime.paused = !resume_all;
            for item in &mut runtime.queue {
                if item.status == QueuedInputStatus::Paused
                    && item.source == QueuedInputSource::User
                    && resume_all
                {
                    item.status = QueuedInputStatus::Queued;
                }
            }
            runtime.phase = if released
                .as_ref()
                .is_some_and(|item| item.source == QueuedInputSource::RuntimeCallback)
            {
                "idle"
            } else if runtime.paused {
                "paused"
            } else {
                "idle"
            }
            .to_string();
            runtime.last_event_sequence += 1;
            (released, runtime.clone())
        };
        self.persist_runtime(&snapshot)?;
        Ok(released)
    }

    pub fn audit_sink(&self, id: impl Into<String>) -> SessionJournalAuditSink {
        SessionJournalAuditSink::new(self.clone(), id)
    }

    fn append_harness_spec_once(
        &self,
        id: &str,
        spec: &HarnessSpec,
        fingerprint: &HarnessFingerprint,
    ) -> anyhow::Result<()> {
        let _guard = self
            .harness_spec_lock
            .lock()
            .expect("harness spec journal lock poisoned");
        let path = self.journal_path(id);
        let mut journal = if path.exists() {
            SessionJournal::open(&path)?
        } else {
            SessionJournal::create(&path, std::env::current_dir()?, None)?
        };
        let already_recorded = journal.entries().iter().any(|entry| {
            matches!(
                &entry.data,
                JournalEntryData::Custom {
                    custom_type,
                    data: Some(data),
                } if custom_type == "harness_spec_v1"
                    && data.get("fingerprint").and_then(serde_json::Value::as_str)
                        == Some(fingerprint.0.as_str())
            )
        });
        if already_recorded {
            return Ok(());
        }
        journal.append(JournalEntryData::Custom {
            custom_type: "harness_spec_v1".to_string(),
            data: Some(serde_json::json!({
                "fingerprint": fingerprint,
                "spec": spec,
            })),
        })?;
        Ok(())
    }

    fn append_custom(
        &self,
        id: &str,
        custom_type: &str,
        data: serde_json::Value,
    ) -> anyhow::Result<()> {
        let path = self.journal_path(id);
        let mut journal = if path.exists() {
            SessionJournal::open(&path)?
        } else {
            SessionJournal::create(&path, std::env::current_dir()?, None)?
        };
        journal.append(JournalEntryData::Custom {
            custom_type: custom_type.to_string(),
            data: Some(data),
        })?;
        Ok(())
    }

    pub fn append_runtime_callback(
        &self,
        id: &str,
        payload: serde_json::Value,
    ) -> anyhow::Result<()> {
        if self.get(id).is_none() {
            anyhow::bail!("session not found");
        }
        let path = self.journal_path(id);
        let mut journal = if path.exists() {
            SessionJournal::open(&path)?
        } else {
            SessionJournal::create(&path, std::env::current_dir()?, None)?
        };
        let dedupe_key = payload.get("dedupeKey").and_then(serde_json::Value::as_str);
        let already_recorded = dedupe_key.is_some_and(|dedupe_key| {
            journal.entries().iter().any(|entry| {
                matches!(
                    &entry.data,
                    JournalEntryData::Custom {
                        custom_type,
                        data: Some(existing),
                    } if custom_type == "runtime_callback_v1"
                        && existing.get("dedupeKey").and_then(serde_json::Value::as_str)
                            == Some(dedupe_key)
                )
            })
        });
        if already_recorded {
            return Ok(());
        }
        journal.append(JournalEntryData::Custom {
            custom_type: "runtime_callback_v1".to_string(),
            data: Some(payload),
        })?;
        Ok(())
    }

    pub fn runtime_callback_task_ids(
        &self,
        id: &str,
        dedupe_key: &str,
    ) -> anyhow::Result<Vec<String>> {
        let journal = SessionJournal::open(self.journal_path(id))?;
        Ok(journal
            .entries()
            .iter()
            .rev()
            .find_map(|entry| match &entry.data {
                JournalEntryData::Custom {
                    custom_type,
                    data: Some(data),
                } if custom_type == "runtime_callback_v1"
                    && data.get("dedupeKey").and_then(serde_json::Value::as_str)
                        == Some(dedupe_key) =>
                {
                    data.get("taskIds").and_then(|task_ids| {
                        task_ids.as_array().map(|task_ids| {
                            task_ids
                                .iter()
                                .filter_map(serde_json::Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                    })
                }
                _ => None,
            })
            .unwrap_or_default())
    }

    fn runtime_callback_consumed(&self, id: &str, dedupe_key: &str) -> anyhow::Result<bool> {
        let journal = SessionJournal::open(self.journal_path(id))?;
        Ok(journal.entries().iter().any(|entry| {
            matches!(
                &entry.data,
                JournalEntryData::Custom {
                    custom_type,
                    data: Some(data),
                } if custom_type == "runtime_callback_consumed_v1"
                    && data.get("dedupeKey").and_then(serde_json::Value::as_str)
                        == Some(dedupe_key)
            )
        }))
    }

    pub fn append_work_item_binding(
        &self,
        session_id: &str,
        work_item_id: &str,
        client_chat_session_id: &str,
    ) -> anyhow::Result<()> {
        if self.get(session_id).is_none() {
            anyhow::bail!("session not found");
        }
        let path = self.journal_path(session_id);
        let journal = SessionJournal::open(&path)?;
        let already_recorded = journal.entries().iter().any(|entry| {
            matches!(
                &entry.data,
                JournalEntryData::Custom {
                    custom_type,
                    data: Some(data),
                } if custom_type == "work_item_binding_v1"
                    && data.get("work_item_id").and_then(serde_json::Value::as_str)
                        == Some(work_item_id)
                    && data.get("client_chat_session_id").and_then(serde_json::Value::as_str)
                        == Some(client_chat_session_id)
            )
        });
        if already_recorded {
            return Ok(());
        }
        self.append_custom(
            session_id,
            "work_item_binding_v1",
            serde_json::json!({
                "work_item_id": work_item_id,
                "client_chat_session_id": client_chat_session_id,
            }),
        )
    }

    fn persist_runtime(&self, snapshot: &SessionRuntimeSnapshot) -> anyhow::Result<()> {
        self.append_custom(
            &snapshot.session_id,
            "session_runtime_v1",
            serde_json::to_value(snapshot)?,
        )?;
        let _ = self.runtime_tx.send(snapshot.clone());
        Ok(())
    }

    fn persist_session_workspace(&self, id: &str) -> anyhow::Result<()> {
        let session = self
            .sessions
            .read()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        self.append_custom(
            id,
            "workspace_binding_v1",
            serde_json::json!({
                "workspace": session.workspace,
                "state": session.state,
                "generation": session.generation,
            }),
        )
    }

    /// 列出所有会话的 (id, name)
    pub fn list(&self) -> Vec<(String, String)> {
        let guard = self.sessions.read().unwrap();
        let mut sessions = guard
            .values()
            .map(|session| (session.updated_at, session.id.clone(), session.name.clone()))
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        sessions
            .into_iter()
            .map(|(_, id, name)| (id, name))
            .collect()
    }

    pub fn branches(&self, id: &str) -> anyhow::Result<Vec<BranchEntryInfo>> {
        let journal = SessionJournal::open(self.journal_path(id))?;
        Ok(journal
            .entries()
            .iter()
            .filter_map(|entry| {
                let (kind, preview) = match &entry.data {
                    JournalEntryData::Message { message } => (
                        "message",
                        message.text_content().chars().take(120).collect(),
                    ),
                    JournalEntryData::Compaction { summary, .. } => {
                        ("compaction", summary.chars().take(120).collect())
                    }
                    JournalEntryData::BranchSummary { summary, .. } => {
                        ("branch_summary", summary.chars().take(120).collect())
                    }
                    _ => return None,
                };
                Some(BranchEntryInfo {
                    id: entry.id,
                    parent_id: entry.parent_id,
                    kind: kind.to_string(),
                    preview,
                    current: journal.leaf_id() == Some(entry.id),
                })
            })
            .collect())
    }

    pub fn switch_branch(
        &self,
        id: &str,
        target: uuid::Uuid,
        summary: Option<String>,
    ) -> anyhow::Result<Session> {
        let mut journal = SessionJournal::open(self.journal_path(id))?;
        let from = journal.leaf_id();
        journal.move_to(Some(target))?;
        if let (Some(from), Some(summary)) =
            (from, summary.filter(|value| !value.trim().is_empty()))
        {
            journal.append_branch_summary(summary, from, None)?;
        }
        let messages = journal.active_messages()?;
        let mut session = self
            .sessions
            .read()
            .expect("session manager lock poisoned")
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("session not found: {id}"))?;
        session.messages = messages;
        self.sessions
            .write()
            .expect("session manager lock poisoned")
            .insert(id.to_string(), session.clone());
        Ok(session)
    }

    pub fn divergent_branch_text(&self, id: &str, target: uuid::Uuid) -> anyhow::Result<String> {
        let journal = SessionJournal::open(self.journal_path(id))?;
        let current = journal.path_to_root(None)?;
        let target_path = journal.path_to_root(Some(target))?;
        let common = current
            .iter()
            .zip(&target_path)
            .take_while(|(left, right)| left.id == right.id)
            .count();
        Ok(current[common..]
            .iter()
            .filter_map(|entry| match &entry.data {
                JournalEntryData::Message { message } => {
                    Some(format!("[{}] {}", message.role, message.text_content()))
                }
                JournalEntryData::Compaction { summary, .. }
                | JournalEntryData::BranchSummary { summary, .. } => Some(summary.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// 删除会话，返回是否成功删除
    pub fn delete(&self, id: &str) -> bool {
        let mut guard = self.sessions.write().unwrap();
        let removed_session = guard.remove(id);
        let removed = removed_session.is_some();
        drop(guard);
        if removed {
            self.runtimes.write().unwrap().remove(id);
            let _ = std::fs::create_dir_all(self.tombstone_dir.as_ref());
            let generation = removed_session
                .as_ref()
                .map(|session| session.generation + 1)
                .unwrap_or(1);
            let tombstone = serde_json::json!({
                "sessionId": id,
                "generation": generation,
                "deletedAt": chrono::Utc::now(),
            });
            let _ = std::fs::write(
                self.tombstone_dir
                    .join(format!("{}.json", encoded_session_id(id))),
                tombstone.to_string(),
            );
            let _ = std::fs::remove_file(self.journal_path(id));
        }
        removed
    }

    /// 更新（替换）会话
    pub fn update(&self, session: Session) {
        let workspace_changed =
            self.sessions
                .read()
                .unwrap()
                .get(&session.id)
                .is_none_or(|previous| {
                    previous.workspace != session.workspace
                        || previous.state != session.state
                        || previous.generation != session.generation
                });
        if let Err(error) = self.sync_journal(&session) {
            tracing::warn!(session_id = %session.id, error = %error, "sync session journal failed");
        }
        let id = session.id.clone();
        let mut guard = self.sessions.write().unwrap();
        guard.insert(id.clone(), session);
        drop(guard);
        if workspace_changed {
            if let Err(error) = self.persist_session_workspace(&id) {
                tracing::warn!(session_id = %id, error = %error, "persist workspace binding failed");
            }
        }
    }

    /// 用指定 id 获取或创建会话。前端持久化了 session_id，pwcli 重启后内存 map 清空，
    /// 此函数让 pwcli 认领前端给的 id 自创建一个 session 而不是 404，避免重启把
    /// 老对话冻死。如果未来加了 id 格式校验（如 `sess_` 前缀），在这里加。
    pub fn get_or_create(&self, id: &str, fallback_name: &str) -> Session {
        if let Some(s) = self.get(id) {
            return s;
        }
        let mut session = Session::new(fallback_name);
        session.id = id.to_string();
        let journal_path = self.journal_path(id);
        if journal_path.exists() {
            if let Ok(journal) = SessionJournal::open(&journal_path) {
                if let Ok(messages) = journal.active_messages() {
                    session.messages = messages;
                }
            }
        } else {
            if let Err(error) = self.sync_journal(&session) {
                tracing::warn!(session_id = %id, error = %error, "create session journal failed");
            }
        }
        let cloned = session.clone();
        self.update(session);
        cloned
    }

    fn journal_path(&self, id: &str) -> PathBuf {
        self.journal_dir
            .join(format!("{}.jsonl", encoded_session_id(id)))
    }

    fn restore_journals(&self) {
        let Ok(entries) = std::fs::read_dir(self.journal_dir.as_ref()) else {
            return;
        };
        let mut restored = self.sessions.write().unwrap();
        let mut restored_runtimes = self.runtimes.write().unwrap();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(encoded) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            let Some(id) = decode_session_id(encoded) else {
                continue;
            };
            let Ok(journal) = SessionJournal::open(&path) else {
                tracing::warn!(path = %path.display(), "skipping unreadable session journal");
                continue;
            };
            let Ok(messages) = journal.active_messages() else {
                continue;
            };
            let name = journal
                .entries()
                .iter()
                .rev()
                .find_map(|entry| match &entry.data {
                    JournalEntryData::SessionInfo { name } => name.clone(),
                    _ => None,
                })
                .unwrap_or_else(|| id.clone());
            let mut session = Session::new(name);
            session.id = id.clone();
            session.messages = messages;
            let mut runtime = SessionRuntimeSnapshot::idle(&id);
            for journal_entry in journal.entries() {
                let JournalEntryData::Custom {
                    custom_type,
                    data: Some(data),
                } = &journal_entry.data
                else {
                    continue;
                };
                match custom_type.as_str() {
                    "workspace_binding_v1" => {
                        session.workspace = data.get("workspace").cloned().and_then(|value| {
                            serde_json::from_value::<WorkspaceBinding>(value).ok()
                        });
                        session.state = data
                            .get("state")
                            .cloned()
                            .and_then(|value| serde_json::from_value::<SessionState>(value).ok())
                            .unwrap_or_else(|| {
                                if session.workspace.is_some() {
                                    SessionState::Ready
                                } else {
                                    SessionState::AwaitingBinding
                                }
                            });
                        session.generation = data
                            .get("generation")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(1);
                    }
                    "session_runtime_v1" => {
                        if let Ok(snapshot) =
                            serde_json::from_value::<SessionRuntimeSnapshot>(data.clone())
                        {
                            runtime = snapshot;
                        }
                    }
                    "runtime_callback_consumed_v1" => {
                        if let Some(dedupe_key) =
                            data.get("dedupeKey").and_then(serde_json::Value::as_str)
                        {
                            runtime.queue.retain(|item| {
                                item.source != QueuedInputSource::RuntimeCallback
                                    || item.client_message_id != dedupe_key
                            });
                        }
                    }
                    _ => {}
                }
            }
            if let Ok(metadata) = entry.metadata() {
                if let Ok(modified) = metadata.modified() {
                    session.updated_at = modified.into();
                }
            }
            if runtime.phase == "running" {
                runtime.phase = "paused".to_string();
                runtime.active_turn_id = None;
                runtime.paused = true;
                for item in &mut runtime.queue {
                    item.status = if item.status == QueuedInputStatus::Claimed {
                        QueuedInputStatus::Failed
                    } else {
                        QueuedInputStatus::Paused
                    };
                    item.claimed_by_turn_id = None;
                }
            }
            restored_runtimes.insert(id.clone(), runtime);
            restored.insert(id, session);
        }
    }

    fn sync_journal(&self, session: &Session) -> anyhow::Result<()> {
        let path = self.journal_path(&session.id);
        let mut journal = if path.exists() {
            SessionJournal::open(&path)?
        } else {
            let cwd = session
                .workspace
                .as_ref()
                .map(|workspace| workspace.canonical_path.clone())
                .unwrap_or(std::env::current_dir()?);
            SessionJournal::create(&path, cwd, None)?
        };
        if let Some(workspace) = &session.workspace {
            let has_binding = journal.entries().iter().any(|entry| {
                matches!(
                    &entry.data,
                    JournalEntryData::Custom { custom_type, .. }
                        if custom_type == "workspace_binding_v1"
                )
            });
            if !has_binding {
                journal.append(JournalEntryData::Custom {
                    custom_type: "workspace_binding_v1".to_string(),
                    data: Some(serde_json::json!({
                        "workspace": workspace,
                        "state": session.state,
                        "generation": session.generation,
                    })),
                })?;
            }
        }
        let existing = journal.active_messages()?;
        let common_prefix = existing
            .iter()
            .zip(&session.messages)
            .take_while(|(left, right)| left.id == right.id)
            .count();
        if common_prefix != existing.len()
            && self.try_append_compaction(&mut journal, &existing, session)?
        {
            return Ok(());
        }
        if common_prefix != existing.len() {
            journal.move_to(None)?;
        }
        let start = if common_prefix == existing.len() {
            common_prefix
        } else {
            0
        };
        for message in session.messages.iter().skip(start) {
            journal.append_message(message.clone())?;
        }
        if journal
            .entries()
            .iter()
            .all(|entry| !matches!(entry.data, JournalEntryData::SessionInfo { .. }))
        {
            journal.append(JournalEntryData::SessionInfo {
                name: Some(session.name.clone()),
            })?;
        }
        Ok(())
    }

    fn try_append_compaction(
        &self,
        journal: &mut SessionJournal,
        existing: &[crate::runtime::session::ConversationMessage],
        session: &Session,
    ) -> anyhow::Result<bool> {
        let Some(summary_message) = session.messages.first() else {
            return Ok(false);
        };
        if summary_message.role != crate::runtime::session::MessageRole::System
            || !summary_message.id.starts_with("msg_compact_")
        {
            return Ok(false);
        }
        let kept = &session.messages[1..];
        if kept.len() > existing.len()
            || existing[existing.len().saturating_sub(kept.len())..]
                .iter()
                .zip(kept)
                .any(|(left, right)| left.id != right.id)
        {
            return Ok(false);
        }

        let first_kept_message_id = kept.first().map(|message| message.id.as_str());
        let first_kept_entry_id = journal
            .path_to_root(None)?
            .into_iter()
            .find_map(|entry| match &entry.data {
                JournalEntryData::Message { message }
                    if Some(message.id.as_str()) == first_kept_message_id =>
                {
                    Some(entry.id)
                }
                _ => None,
            })
            .or_else(|| journal.leaf_id())
            .ok_or_else(|| anyhow::anyhow!("cannot compact an empty journal"))?;
        let encoded_summary = summary_message.text_content();
        let (summary, traces) =
            crate::agent_core::middleware::summarization::decode_compaction_summary(
                &encoded_summary,
            );
        let tokens_before = existing
            .iter()
            .map(crate::runtime::session::estimate_message_tokens)
            .map(u64::from)
            .sum::<u64>();
        journal.append(JournalEntryData::Compaction {
            summary: summary.to_string(),
            first_kept_entry_id,
            tokens_before,
            details: Some(serde_json::json!({
                "summaryMessage": summary_message,
                "readFiles": traces.as_ref().map(|value| &value.read_files).cloned().unwrap_or_default(),
                "modifiedFiles": traces.as_ref().map(|value| &value.modified_files).cloned().unwrap_or_default()
            })),
        })?;
        Ok(true)
    }
}

fn encoded_session_id(id: &str) -> String {
    id.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode_session_id(encoded: &str) -> Option<String> {
    if encoded.is_empty() || !encoded.len().is_multiple_of(2) {
        return None;
    }
    let bytes = (0..encoded.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&encoded[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

fn cleanup_legacy_session_json(base_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(base_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if !file_type.is_file()
            || file_type.is_symlink()
            || path.extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }
        if let Err(error) = std::fs::remove_file(&path) {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "failed to remove legacy session JSON"
            );
        }
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::session::ConversationMessage;

    fn test_manager() -> (tempfile::TempDir, SessionManager) {
        let dir = tempfile::tempdir().unwrap();
        let manager = SessionManager::new_in(dir.path());
        (dir, manager)
    }

    #[test]
    fn test_create_and_get() {
        let (_dir, mgr) = test_manager();
        let id = mgr.create("test_session");
        assert!(!id.is_empty());

        let session = mgr.get(&id);
        assert!(session.is_some());
        assert_eq!(session.unwrap().name, "test_session");
    }

    #[test]
    fn test_get_missing() {
        let (_dir, mgr) = test_manager();
        assert!(mgr.get("nonexistent").is_none());
    }

    #[test]
    fn test_list() {
        let (_dir, mgr) = test_manager();
        mgr.create("session_a");
        mgr.create("session_b");

        let list = mgr.list();
        assert_eq!(list.len(), 2);

        let names: Vec<String> = list.into_iter().map(|(_, name)| name).collect();
        assert!(names.contains(&"session_a".to_string()));
        assert!(names.contains(&"session_b".to_string()));
    }

    #[test]
    fn test_list_empty() {
        let (_dir, mgr) = test_manager();
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn test_delete_existing() {
        let (_dir, mgr) = test_manager();
        let id = mgr.create("to_delete");
        assert!(mgr.delete(&id));
        assert!(mgr.get(&id).is_none());
    }

    #[test]
    fn test_delete_nonexistent() {
        let (_dir, mgr) = test_manager();
        assert!(!mgr.delete("nonexistent"));
    }

    #[test]
    fn test_get_returns_clone() {
        let (_dir, mgr) = test_manager();
        let id = mgr.create("clone_test");

        // 获取克隆并修改
        let mut session = mgr.get(&id).unwrap();
        session.add_message(ConversationMessage::new_user("hello"));
        assert_eq!(session.messages.len(), 1);

        // 原始存储中的会话不应被修改
        let original = mgr.get(&id).unwrap();
        assert_eq!(original.messages.len(), 0);
    }

    #[test]
    fn test_default() {
        let (_dir, mgr) = test_manager();
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn journal_survives_manager_restart_and_is_source_of_truth() {
        let dir = tempfile::tempdir().unwrap();
        let first = SessionManager::new_in(dir.path());
        let id = first.create("durable");
        let mut session = first.get(&id).unwrap();
        session.add_message(ConversationMessage::new_user("persist me"));
        first.update(session);

        let restarted = SessionManager::new_in(dir.path());
        let recovered = restarted.get_or_create(&id, "durable");
        assert_eq!(recovered.messages.len(), 1);
        assert_eq!(recovered.messages[0].text_content(), "persist me");
    }

    #[test]
    fn startup_removes_only_top_level_legacy_json_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("legacy.json"), "{}").unwrap();
        std::fs::write(dir.path().join("keep.txt"), "{}").unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("nested").join("keep.json"), "{}").unwrap();

        let _manager = SessionManager::new_in(dir.path());
        assert!(!dir.path().join("legacy.json").exists());
        assert!(dir.path().join("keep.txt").exists());
        assert!(dir.path().join("nested").join("keep.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn startup_does_not_follow_or_delete_legacy_json_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("outside.txt");
        let link = dir.path().join("legacy.json");
        std::fs::write(&target, "keep").unwrap();
        symlink(&target, &link).unwrap();

        let _manager = SessionManager::new_in(dir.path());
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "keep");
    }

    #[test]
    fn restart_rebuilds_session_index_from_journals() {
        let dir = tempfile::tempdir().unwrap();
        let manager = SessionManager::new_in(dir.path());
        let id = manager.create("durable TUI session");
        let mut session = manager.get(&id).unwrap();
        session.add_message(ConversationMessage::new_user("persist me"));
        manager.update(session);

        let reopened = SessionManager::new_in(dir.path());
        assert_eq!(
            reopened.list(),
            vec![(id.clone(), "durable TUI session".into())]
        );
        assert_eq!(
            reopened.get(&id).unwrap().messages[0].text_content(),
            "persist me"
        );
    }

    #[tokio::test]
    async fn harness_audit_is_durable_deduplicated_and_context_neutral() {
        let dir = tempfile::tempdir().unwrap();
        let manager = SessionManager::new_in(dir.path());
        let id = manager.create("audited");
        let sink = manager.audit_sink(id.clone());
        let spec = crate::agent_core::harness::HarnessSpec::resolve(
            crate::agent_core::harness::HarnessInputs {
                profile: crate::agent_core::harness::HarnessProfile::Main,
                max_rounds: 100,
                context_window: 32_000,
                thinking_level: crate::agent_core::contracts::ThinkingLevel::Off,
                yolo_mode: false,
                externalize_tool_outputs: true,
                unified_recovery: true,
                reviewer_enabled: false,
                background_enabled: true,
                steer_queue_mode: crate::agent_core::harness::spec::QueueModeSpec::OneAtATime,
                follow_up_queue_mode: crate::agent_core::harness::spec::QueueModeSpec::OneAtATime,
                tool_schemas: &[],
            },
        )
        .unwrap();
        let fingerprint = spec.fingerprint().unwrap();
        sink.record_spec(&spec, &fingerprint).await.unwrap();
        sink.record_spec(&spec, &fingerprint).await.unwrap();
        sink.record_intervention(&crate::agent_core::harness::MiddlewareIntervention::new(
            fingerprint,
            "loop_detection",
            1,
            crate::agent_core::harness::MiddlewareHook::AfterLlm,
            crate::agent_core::harness::InterventionKind::ControlFlow,
            "test_intervention",
            3,
            None,
            std::collections::BTreeMap::new(),
        ))
        .await
        .unwrap();

        let journal = SessionJournal::open(manager.journal_path(&id)).unwrap();
        assert_eq!(
            journal
                .entries()
                .iter()
                .filter(|entry| matches!(
                    &entry.data,
                    JournalEntryData::Custom { custom_type, .. }
                        if custom_type == "harness_spec_v1"
                ))
                .count(),
            1
        );
        assert!(journal.entries().iter().any(|entry| matches!(
            &entry.data,
            JournalEntryData::Custom { custom_type, .. }
                if custom_type == "middleware_intervention_v1"
        )));
        assert!(journal.active_messages().unwrap().is_empty());

        let reopened = SessionJournal::open(manager.journal_path(&id)).unwrap();
        assert_eq!(reopened.entries().len(), journal.entries().len());
    }

    #[test]
    fn compaction_is_lossless_in_journal_and_materializes_compact_context() {
        let dir = tempfile::tempdir().unwrap();
        let manager = SessionManager::new_in(dir.path());
        let id = manager.create("compact");
        let mut session = manager.get(&id).unwrap();
        for index in 0..6 {
            session.add_message(ConversationMessage::new_user(format!("message {index}")));
        }
        manager.update(session.clone());

        let kept = session.messages[4..].to_vec();
        let mut summary = ConversationMessage::new_user("history summary");
        summary.id = "msg_compact_test".to_string();
        summary.role = crate::runtime::session::MessageRole::System;
        session.messages = vec![summary.clone()];
        session.messages.extend(kept.clone());
        manager.update(session);

        let journal = SessionJournal::open(manager.journal_path(&id)).unwrap();
        assert!(journal
            .entries()
            .iter()
            .any(|entry| matches!(entry.data, JournalEntryData::Compaction { .. })));
        let materialized = journal.active_messages().unwrap();
        assert_eq!(materialized.len(), 3);
        assert_eq!(materialized[0].id, summary.id);
        assert_eq!(materialized[1].id, kept[0].id);
        assert_eq!(materialized[2].id, kept[1].id);

        // The six original messages remain in the append-only history.
        assert_eq!(
            journal
                .entries()
                .iter()
                .filter(|entry| matches!(entry.data, JournalEntryData::Message { .. }))
                .count(),
            6
        );
    }

    #[test]
    fn switching_branch_materializes_summary_on_target_path() {
        let dir = tempfile::tempdir().unwrap();
        let manager = SessionManager::new_in(dir.path());
        let id = manager.create("branches");
        let mut session = manager.get(&id).unwrap();
        session.add_message(ConversationMessage::new_user("root"));
        manager.update(session.clone());
        let journal = SessionJournal::open(manager.journal_path(&id)).unwrap();
        let root = journal.leaf_id().unwrap();

        session.add_message(ConversationMessage::new_assistant("old branch"));
        manager.update(session);
        let switched = manager
            .switch_branch(&id, root, Some("old branch summary".to_string()))
            .unwrap();

        assert_eq!(switched.messages.len(), 2);
        assert_eq!(switched.messages[0].text_content(), "root");
        assert!(switched.messages[1]
            .text_content()
            .contains("old branch summary"));
        let journal = SessionJournal::open(manager.journal_path(&id)).unwrap();
        assert!(journal.entries().iter().any(|entry| matches!(
            &entry.data,
            JournalEntryData::BranchSummary { summary, .. } if summary == "old branch summary"
        )));
    }

    fn queued_user(content: &str) -> crate::ai::llm::ChatMessage {
        crate::ai::llm::ChatMessage {
            role: "user".into(),
            content: content.into(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn idle_guidance_atomically_becomes_next_turn() {
        let (_dir, manager) = test_manager();
        let id = manager.create("queue");
        let item = manager
            .enqueue_input(
                &id,
                "client-1".into(),
                crate::runtime::session::QueuedInputDelivery::Guidance,
                queued_user("continue"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
        assert_eq!(
            item.delivery,
            crate::runtime::session::QueuedInputDelivery::NextTurn
        );
        assert_eq!(manager.runtime(&id).unwrap().queue.len(), 1);
    }

    #[test]
    fn runtime_callback_precedes_user_queue_without_crossing_claimed_input() {
        let (_dir, manager) = test_manager();
        let id = manager.create("callback-priority");
        let claimed = manager
            .enqueue_input(
                &id,
                "claimed".into(),
                crate::runtime::session::QueuedInputDelivery::NextTurn,
                queued_user("current"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
        {
            let mut runtimes = manager.runtimes.write().unwrap();
            let runtime = runtimes.get_mut(&id).unwrap();
            runtime.queue[0].status = crate::runtime::session::QueuedInputStatus::Claimed;
        }
        manager
            .enqueue_input(
                &id,
                "ordinary".into(),
                crate::runtime::session::QueuedInputDelivery::NextTurn,
                queued_user("ordinary"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
        let callback = manager
            .enqueue_runtime_callback(&id, "callback".into(), queued_user("collaborator result"))
            .unwrap()
            .unwrap();

        let runtime = manager.runtime(&id).unwrap();
        assert_eq!(runtime.queue[0].id, claimed.id);
        assert_eq!(runtime.queue[1].id, callback.id);
        assert_eq!(runtime.queue[2].client_message_id, "ordinary");
        assert_eq!(
            callback.source,
            crate::runtime::session::QueuedInputSource::RuntimeCallback
        );
        assert_eq!(
            callback.priority,
            crate::runtime::session::QueuedInputPriority::High
        );
    }

    #[test]
    fn paused_runtime_callback_is_immutable_and_consumed_once() {
        let (dir, manager) = test_manager();
        let id = manager.create("paused-callback");
        manager.set_queue_paused(&id, true).unwrap();
        manager
            .append_runtime_callback(
                &id,
                serde_json::json!({
                    "dedupeKey": "callback-once",
                    "taskIds": ["task-one", "task-two"]
                }),
            )
            .unwrap();
        assert_eq!(
            manager
                .runtime_callback_task_ids(&id, "callback-once")
                .unwrap(),
            vec!["task-one", "task-two"]
        );
        let callback = manager
            .enqueue_runtime_callback(&id, "callback-once".into(), queued_user("done"))
            .unwrap()
            .unwrap();

        assert_eq!(
            callback.status,
            crate::runtime::session::QueuedInputStatus::Paused
        );
        assert!(manager
            .replace_queue_item(&id, &callback.id, Some("changed".into()), None, None)
            .is_err());
        assert!(manager.delete_queue_item(&id, &callback.id).is_err());
        manager.clear_queue(&id).unwrap();
        assert_eq!(manager.runtime(&id).unwrap().queue.len(), 1);
        let released = manager.release_next_queued(&id, true).unwrap().unwrap();
        assert_eq!(released.id, callback.id);
        assert_eq!(
            manager.runtime(&id).unwrap().queue[0].status,
            crate::runtime::session::QueuedInputStatus::Claimed
        );
        manager.requeue_claimed_input(&id, &callback.id).unwrap();

        assert!(manager
            .consume_queued_input_by_id(&id, &callback.id)
            .unwrap()
            .is_some());
        assert!(manager
            .enqueue_runtime_callback(&id, "callback-once".into(), queued_user("done"))
            .unwrap()
            .is_none());
        let restored = SessionManager::new_in(dir.path());
        assert!(restored
            .enqueue_runtime_callback(&id, "callback-once".into(), queued_user("done"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn only_one_turn_can_claim_a_session_and_stop_preserves_queue() {
        let (_dir, manager) = test_manager();
        let id = manager.create("serialized");
        manager.mark_turn_started(&id).unwrap();
        assert!(manager.mark_turn_started(&id).is_err());
        manager
            .enqueue_input(
                &id,
                "client-2".into(),
                crate::runtime::session::QueuedInputDelivery::NextTurn,
                queued_user("second"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
        manager.set_queue_paused(&id, true).unwrap();
        let runtime = manager.runtime(&id).unwrap();
        assert!(runtime.paused);
        assert_eq!(runtime.queue.len(), 1);
        assert_eq!(
            runtime.queue[0].status,
            crate::runtime::session::QueuedInputStatus::Paused
        );
    }

    #[test]
    fn edited_queue_text_is_the_text_taken_for_execution() {
        let (_dir, manager) = test_manager();
        let id = manager.create("queue-edit-execution");
        let item = manager
            .enqueue_input(
                &id,
                "edit-client".into(),
                crate::runtime::session::QueuedInputDelivery::NextTurn,
                queued_user("old text"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        manager
            .replace_queue_item(&id, &item.id, Some("new text".into()), None, None)
            .unwrap();

        let taken = manager.take_next_queued_input(&id).unwrap().unwrap();
        assert_eq!(taken.id, item.id);
        assert_eq!(taken.message.content, "new text");
        assert!(manager.take_next_queued_input(&id).unwrap().is_none());
    }

    #[test]
    fn claimed_queue_input_is_only_removed_after_success_acknowledgement() {
        let (dir, manager) = test_manager();
        let id = manager.create("queue-claim-ack");
        manager.mark_turn_started(&id).unwrap();
        let item = manager
            .enqueue_input(
                &id,
                "claim-client".into(),
                crate::runtime::session::QueuedInputDelivery::NextTurn,
                queued_user("do not lose me"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        let claimed = manager.claim_next_queued_input(&id).unwrap().unwrap();
        assert_eq!(claimed.id, item.id);
        assert_eq!(manager.runtime(&id).unwrap().queue.len(), 1);
        assert_eq!(
            manager.runtime(&id).unwrap().queue[0].status,
            crate::runtime::session::QueuedInputStatus::Claimed
        );

        manager.mark_claimed_input_failed(&id, &item.id).unwrap();
        assert_eq!(
            manager.runtime(&id).unwrap().queue[0].status,
            crate::runtime::session::QueuedInputStatus::Failed
        );

        let restored = SessionManager::new_in(dir.path());
        assert_eq!(restored.runtime(&id).unwrap().queue.len(), 1);
        assert_eq!(
            restored.runtime(&id).unwrap().queue[0].message.content,
            "do not lose me"
        );
    }

    #[test]
    fn successful_claim_acknowledgement_consumes_exactly_one_input() {
        let (_dir, manager) = test_manager();
        let id = manager.create("queue-claim-success");
        let item = manager
            .enqueue_input(
                &id,
                "success-client".into(),
                crate::runtime::session::QueuedInputDelivery::NextTurn,
                queued_user("execute once"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        manager.claim_next_queued_input(&id).unwrap().unwrap();
        assert_eq!(
            manager
                .consume_queued_input_by_id(&id, &item.id)
                .unwrap()
                .unwrap()
                .id,
            item.id
        );
        assert!(manager.runtime(&id).unwrap().queue.is_empty());
    }

    #[test]
    fn queue_reordering_and_deletion_control_execution_order() {
        let (_dir, manager) = test_manager();
        let id = manager.create("queue-order-execution");
        let first = manager
            .enqueue_input(
                &id,
                "first-client".into(),
                crate::runtime::session::QueuedInputDelivery::NextTurn,
                queued_user("first"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
        let second = manager
            .enqueue_input(
                &id,
                "second-client".into(),
                crate::runtime::session::QueuedInputDelivery::NextTurn,
                queued_user("second"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        manager
            .replace_queue_item(&id, &second.id, None, Some(0), None)
            .unwrap();
        manager.delete_queue_item(&id, &first.id).unwrap();

        let taken = manager.take_next_queued_input(&id).unwrap().unwrap();
        assert_eq!(taken.id, second.id);
        assert_eq!(taken.sequence, 1);
        assert!(manager.take_next_queued_input(&id).unwrap().is_none());
    }

    #[test]
    fn final_turn_boundary_rechecks_queue_before_releasing_runner() {
        let (_dir, manager) = test_manager();
        let id = manager.create("queue-final-boundary");
        manager.mark_turn_started(&id).unwrap();

        // The normal boundary check sees an empty queue. A message then arrives
        // before settlement, which used to leave it queued forever in idle.
        assert!(manager.take_next_queued_input(&id).unwrap().is_none());
        let queued = manager
            .enqueue_input(
                &id,
                "boundary-client".into(),
                crate::runtime::session::QueuedInputDelivery::NextTurn,
                queued_user("arrived at settlement"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        let taken = manager
            .take_next_queued_input_or_settle(&id)
            .unwrap()
            .unwrap();
        assert_eq!(taken.id, queued.id);
        let still_running = manager.runtime(&id).unwrap();
        assert_eq!(still_running.phase, "running");
        assert!(still_running.active_turn_id.is_some());

        assert!(manager
            .take_next_queued_input_or_settle(&id)
            .unwrap()
            .is_none());
        let settled = manager.runtime(&id).unwrap();
        assert_eq!(settled.phase, "idle");
        assert!(settled.active_turn_id.is_none());
    }

    #[test]
    fn idle_durable_user_input_can_be_claimed_for_daemon_recovery() {
        let (_dir, manager) = test_manager();
        let id = manager.create("idle-queue-recovery");
        let queued = manager
            .enqueue_input(
                &id,
                "recovery-client".into(),
                crate::runtime::session::QueuedInputDelivery::Guidance,
                queued_user("recover me"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        // Guidance submitted after the old turn settled is a normal next turn,
        // and startup recovery can claim it without a Web client reconnect.
        assert_eq!(
            queued.delivery,
            crate::runtime::session::QueuedInputDelivery::NextTurn
        );
        assert!(manager
            .claim_queued_input_for_activation(&id, &queued.id)
            .unwrap());
        assert_eq!(
            manager.runtime(&id).unwrap().queue[0].status,
            crate::runtime::session::QueuedInputStatus::Claimed
        );
        assert!(!manager
            .claim_queued_input_for_activation(&id, &queued.id)
            .unwrap());
    }

    #[test]
    fn user_reordering_cannot_cross_runtime_callback_priority() {
        let (_dir, manager) = test_manager();
        let id = manager.create("queue-callback-fence");
        let user = manager
            .enqueue_input(
                &id,
                "user-client".into(),
                crate::runtime::session::QueuedInputDelivery::NextTurn,
                queued_user("user"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
        let callback = manager
            .enqueue_runtime_callback(&id, "callback-client".into(), queued_user("callback"))
            .unwrap()
            .unwrap();

        manager
            .replace_queue_item(&id, &user.id, None, Some(0), None)
            .unwrap();

        let runtime = manager.runtime(&id).unwrap();
        assert_eq!(runtime.queue[0].id, callback.id);
        assert_eq!(runtime.queue[1].id, user.id);
        assert_eq!(
            manager.take_next_queued_input(&id).unwrap().unwrap().id,
            callback.id
        );
    }

    #[test]
    fn claimed_idle_callback_is_not_taken_as_a_second_turn() {
        let (_dir, manager) = test_manager();
        let id = manager.create("callback-idle-claim");
        let callback = manager
            .enqueue_runtime_callback(&id, "callback-idle".into(), queued_user("callback"))
            .unwrap()
            .unwrap();

        assert!(manager
            .claim_runtime_callback_for_activation(&id, &callback.id)
            .unwrap());
        assert!(manager.take_next_queued_input(&id).unwrap().is_none());
        assert_eq!(
            manager
                .consume_queued_input_by_id(&id, &callback.id)
                .unwrap()
                .unwrap()
                .id,
            callback.id
        );
        assert!(manager.runtime(&id).unwrap().queue.is_empty());
    }

    #[test]
    fn callback_claim_restores_to_running_user_turn_without_duplication() {
        let (_dir, manager) = test_manager();
        let id = manager.create("callback-claim-conflict");
        let callback = manager
            .enqueue_runtime_callback(&id, "callback-conflict".into(), queued_user("callback"))
            .unwrap()
            .unwrap();
        assert!(manager
            .claim_runtime_callback_for_activation(&id, &callback.id)
            .unwrap());

        manager.mark_turn_started(&id).unwrap();
        manager
            .restore_claimed_runtime_callback(&id, &callback.id, false)
            .unwrap();

        let runtime = manager.runtime(&id).unwrap();
        assert_eq!(runtime.phase, "running");
        assert!(!runtime.paused);
        assert_eq!(runtime.queue[0].status, QueuedInputStatus::Queued);
        assert_eq!(
            manager.take_next_queued_input(&id).unwrap().unwrap().id,
            callback.id
        );
        assert!(manager.take_next_queued_input(&id).unwrap().is_none());
    }

    #[test]
    fn callback_activation_failure_restores_claim_as_paused() {
        let (_dir, manager) = test_manager();
        let id = manager.create("callback-claim-failure");
        let callback = manager
            .enqueue_runtime_callback(&id, "callback-failure".into(), queued_user("callback"))
            .unwrap()
            .unwrap();
        assert!(manager
            .claim_runtime_callback_for_activation(&id, &callback.id)
            .unwrap());

        manager
            .restore_claimed_runtime_callback(&id, &callback.id, true)
            .unwrap();

        let runtime = manager.runtime(&id).unwrap();
        assert!(runtime.paused);
        assert_eq!(runtime.phase, "paused");
        assert_eq!(runtime.queue[0].status, QueuedInputStatus::Paused);
        assert!(manager.take_next_queued_input(&id).unwrap().is_none());
    }

    #[test]
    fn concurrent_idle_callback_claim_has_one_owner() {
        let (_dir, manager) = test_manager();
        let id = manager.create("callback-claim-race");
        let callback = manager
            .enqueue_runtime_callback(&id, "callback-race".into(), queued_user("callback"))
            .unwrap()
            .unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let manager = manager.clone();
            let id = id.clone();
            let item_id = callback.id.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                manager
                    .claim_runtime_callback_for_activation(&id, &item_id)
                    .unwrap()
            }));
        }
        barrier.wait();
        let claims = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|claimed| *claimed)
            .count();

        assert_eq!(claims, 1);
        assert_eq!(
            manager.runtime(&id).unwrap().queue[0].status,
            QueuedInputStatus::Claimed
        );
    }

    #[tokio::test]
    async fn durable_delivery_conversion_updates_the_active_harness_projection() {
        let (_dir, manager) = test_manager();
        let id = manager.create("queue-delivery-conversion");
        manager.mark_turn_started(&id).unwrap();
        let control = crate::agent_core::harness::HarnessControl::default();
        control
            .begin(crate::agent_core::harness::HarnessPhase::Turn)
            .unwrap();
        let item = manager
            .enqueue_input(
                &id,
                "conversion-client".into(),
                crate::runtime::session::QueuedInputDelivery::NextTurn,
                queued_user("convert me"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        control
            .mutate_durable_queue(Some(&item.id), || {
                manager.replace_queue_item(
                    &id,
                    &item.id,
                    Some("guidance text".into()),
                    None,
                    Some(crate::runtime::session::QueuedInputDelivery::Guidance),
                )?;
                Ok(((), manager.runtime(&id).unwrap().queue))
            })
            .await
            .unwrap();
        assert_eq!(
            control.drain_steer().await.unwrap()[0].content,
            "guidance text"
        );

        // Once a guidance item has crossed the safe point, subsequent edits
        // are fenced instead of claiming success with different execution.
        assert!(control
            .mutate_durable_queue(Some(&item.id), || {
                manager.replace_queue_item(&id, &item.id, Some("too late".into()), None, None)?;
                Ok(((), manager.runtime(&id).unwrap().queue))
            })
            .await
            .is_err());

        let second = manager
            .enqueue_input(
                &id,
                "conversion-client-2".into(),
                crate::runtime::session::QueuedInputDelivery::Guidance,
                queued_user("back to next turn"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
        control
            .mutate_durable_queue(Some(&second.id), || {
                manager.replace_queue_item(
                    &id,
                    &second.id,
                    None,
                    None,
                    Some(crate::runtime::session::QueuedInputDelivery::NextTurn),
                )?;
                Ok(((), manager.runtime(&id).unwrap().queue))
            })
            .await
            .unwrap();
        assert!(control.drain_steer().await.unwrap().is_empty());
        assert_eq!(
            manager.take_next_queued_input(&id).unwrap().unwrap().id,
            second.id
        );

        let deleted = manager
            .enqueue_input(
                &id,
                "conversion-client-3".into(),
                crate::runtime::session::QueuedInputDelivery::Guidance,
                queued_user("delete before safe point"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
        control
            .mutate_durable_queue(None, || Ok(((), manager.runtime(&id).unwrap().queue)))
            .await
            .unwrap();
        control
            .mutate_durable_queue(Some(&deleted.id), || {
                manager.delete_queue_item(&id, &deleted.id)?;
                Ok(((), manager.runtime(&id).unwrap().queue))
            })
            .await
            .unwrap();
        assert!(control.drain_steer().await.unwrap().is_empty());
        assert!(!manager
            .runtime(&id)
            .unwrap()
            .queue
            .iter()
            .any(|item| item.id == deleted.id));
    }

    #[tokio::test]
    async fn durable_next_turn_has_no_second_harness_execution_copy() {
        let (_dir, manager) = test_manager();
        let id = manager.create("durable-next-turn-single-source");
        manager.mark_turn_started(&id).unwrap();
        let control = crate::agent_core::harness::HarnessControl::default();
        control
            .begin(crate::agent_core::harness::HarnessPhase::Turn)
            .unwrap();
        let queued = manager
            .enqueue_input(
                &id,
                "single-source-client".into(),
                crate::runtime::session::QueuedInputDelivery::NextTurn,
                queued_user("execute once"),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
        control
            .mutate_durable_queue(None, || Ok(((), manager.runtime(&id).unwrap().queue)))
            .await
            .unwrap();

        assert_eq!(
            manager.take_next_queued_input(&id).unwrap().unwrap().id,
            queued.id
        );
        let mut messages = Vec::new();
        let mut session = manager.get(&id).unwrap();
        assert!(control
            .consume_next_turn(&mut messages, &mut session)
            .await
            .unwrap()
            .is_none());
        assert!(messages.is_empty());
    }
}
