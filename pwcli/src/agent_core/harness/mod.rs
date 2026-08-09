use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::agent_core::contracts::{
    QueuedInput, QueuedInputDelivery, QueuedInputPriority, QueuedInputSource, QueuedInputStatus,
};
use crate::agent_core::decision::{DecisionOutcome, DecisionRisk, DecisionTrigger};
use crate::ai::llm::ChatMessage;

pub mod audit;
pub mod spec;

pub use audit::{HarnessAuditSink, InterventionKind, MiddlewareHook, MiddlewareIntervention};
pub use spec::{
    BuiltHarness, HarnessFactory, HarnessFingerprint, HarnessInputs, HarnessProfile, HarnessSpec,
};

type EventFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
type EventHandler = Arc<dyn Fn(HarnessEvent) -> EventFuture + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessPhase {
    Idle,
    Turn,
    Compaction,
    BranchSummary,
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueMode {
    OneAtATime,
    All,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HarnessEvent {
    RuntimeUpdate {
        model: Option<String>,
        thinking_level: Option<crate::agent_core::contracts::ThinkingLevel>,
        active_tools: Option<Vec<String>>,
        resource_revision: u64,
    },
    BeforeTool {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    AfterTool {
        id: String,
        name: String,
        content: String,
        is_error: bool,
    },
    AutoRetryStart {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        error: String,
    },
    AutoRetryEnd {
        attempt: u32,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    DecisionStarted {
        id: String,
        trigger: DecisionTrigger,
        risk: DecisionRisk,
    },
    DecisionResolved {
        id: String,
        outcome: DecisionOutcome,
        confidence: f32,
        consensus: f32,
        rationale: String,
    },
    DecisionEscalated {
        id: String,
        rationale: String,
        options: Vec<crate::agent_core::decision::DecisionOption>,
    },
    QueueUpdate {
        steer_count: usize,
        follow_up_count: usize,
        next_turn_count: usize,
    },
    Abort {
        cleared_steer: usize,
        cleared_follow_up: usize,
    },
    Settled {
        next_turn_count: usize,
    },
}

pub struct HarnessEventBus {
    handlers: Mutex<Vec<EventHandler>>,
    history: Mutex<VecDeque<HarnessEventRecord>>,
    next_sequence: AtomicU64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessEventRecord {
    pub sequence: u64,
    pub event: HarnessEvent,
}

impl Default for HarnessEventBus {
    fn default() -> Self {
        Self {
            handlers: Mutex::new(Vec::new()),
            history: Mutex::new(VecDeque::new()),
            next_sequence: AtomicU64::new(1),
        }
    }
}

impl HarnessEventBus {
    pub fn subscribe<F, Fut>(&self, handler: F)
    where
        F: Fn(HarnessEvent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.handlers
            .lock()
            .expect("harness event handlers lock poisoned")
            .push(Arc::new(move |event| Box::pin(handler(event))));
    }

    pub async fn emit(&self, event: HarnessEvent) -> Result<()> {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        {
            let mut history = self.history.lock().expect("harness history lock poisoned");
            history.push_back(HarnessEventRecord {
                sequence,
                event: event.clone(),
            });
            while history.len() > 2048 {
                history.pop_front();
            }
        }
        let handlers = self
            .handlers
            .lock()
            .expect("harness event handlers lock poisoned")
            .clone();
        for handler in handlers {
            handler(event.clone()).await?;
        }
        Ok(())
    }

    pub fn replay_after(&self, sequence: u64) -> Vec<HarnessEventRecord> {
        self.history
            .lock()
            .expect("harness history lock poisoned")
            .iter()
            .filter(|record| record.sequence > sequence)
            .cloned()
            .collect()
    }
}

struct HarnessControlState {
    phase: HarnessPhase,
    steer: Vec<SteeringInput>,
    consumed_steer_ids: HashSet<String>,
    follow_up: Vec<ChatMessage>,
    next_turn: Vec<NextTurnInput>,
    steering_mode: QueueMode,
    follow_up_mode: QueueMode,
    cancel_token: CancellationToken,
    model: Option<String>,
    thinking_level: Option<crate::agent_core::contracts::ThinkingLevel>,
    active_tools: Option<Vec<String>>,
    resource_revision: u64,
}

#[derive(Debug, Clone)]
struct SteeringInput {
    message: ChatMessage,
    queue_item_id: Option<String>,
}

#[derive(Debug, Clone)]
struct NextTurnInput {
    message: ChatMessage,
    queue_item_id: Option<String>,
    source: QueuedInputSource,
    priority: QueuedInputPriority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumedNextTurn {
    pub queue_item_id: Option<String>,
    pub source: QueuedInputSource,
}

impl Default for HarnessControlState {
    fn default() -> Self {
        Self {
            phase: HarnessPhase::Idle,
            steer: Vec::new(),
            consumed_steer_ids: HashSet::new(),
            follow_up: Vec::new(),
            next_turn: Vec::new(),
            steering_mode: QueueMode::OneAtATime,
            follow_up_mode: QueueMode::OneAtATime,
            cancel_token: CancellationToken::new(),
            model: None,
            thinking_level: None,
            active_tools: None,
            resource_revision: 0,
        }
    }
}

#[derive(Default)]
pub struct HarnessControl {
    state: Mutex<HarnessControlState>,
    events: HarnessEventBus,
}

impl HarnessControl {
    pub fn events(&self) -> &HarnessEventBus {
        &self.events
    }

    pub fn phase(&self) -> HarnessPhase {
        self.state
            .lock()
            .expect("harness control lock poisoned")
            .phase
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.state
            .lock()
            .expect("harness control lock poisoned")
            .cancel_token
            .clone()
    }

    pub fn queue_modes(&self) -> (QueueMode, QueueMode) {
        let state = self.state.lock().expect("harness control lock poisoned");
        (state.steering_mode, state.follow_up_mode)
    }

    pub fn begin(&self, phase: HarnessPhase) -> Result<CancellationToken> {
        if phase == HarnessPhase::Idle {
            bail!("cannot begin an idle harness run");
        }
        let mut state = self.state.lock().expect("harness control lock poisoned");
        if state.phase != HarnessPhase::Idle {
            bail!("agent harness is busy in phase {:?}", state.phase);
        }
        state.phase = phase;
        state.consumed_steer_ids.clear();
        state.cancel_token = CancellationToken::new();
        Ok(state.cancel_token.clone())
    }

    pub async fn settle(&self) -> Result<()> {
        let next_turn_count = {
            let mut state = self.state.lock().expect("harness control lock poisoned");
            state.phase = HarnessPhase::Idle;
            state.next_turn.len()
        };
        self.events
            .emit(HarnessEvent::Settled { next_turn_count })
            .await
    }

    pub async fn steer(&self, message: ChatMessage) -> Result<()> {
        self.enqueue_steer(message, None).await
    }

    pub async fn queued_steer(&self, message: ChatMessage, queue_item_id: String) -> Result<()> {
        self.enqueue_steer(message, Some(queue_item_id)).await
    }

    async fn enqueue_steer(
        &self,
        message: ChatMessage,
        queue_item_id: Option<String>,
    ) -> Result<()> {
        require_user_message(&message)?;
        {
            let mut state = self.state.lock().expect("harness control lock poisoned");
            if state.phase == HarnessPhase::Idle {
                bail!("cannot steer while the agent harness is idle");
            }
            if queue_item_id.as_ref().is_some_and(|queue_item_id| {
                state.consumed_steer_ids.contains(queue_item_id)
                    || state
                        .steer
                        .iter()
                        .any(|item| item.queue_item_id.as_ref() == Some(queue_item_id))
            }) {
                return Ok(());
            }
            state.steer.push(SteeringInput {
                message,
                queue_item_id,
            });
        }
        self.emit_queue_update().await
    }

    pub async fn follow_up(&self, message: ChatMessage) -> Result<()> {
        require_user_message(&message)?;
        {
            let mut state = self.state.lock().expect("harness control lock poisoned");
            if state.phase == HarnessPhase::Idle {
                bail!("cannot queue a follow-up while the agent harness is idle");
            }
            state.follow_up.push(message);
        }
        self.emit_queue_update().await
    }

    pub async fn next_turn(&self, message: ChatMessage) -> Result<()> {
        self.enqueue_next_turn(
            message,
            None,
            QueuedInputSource::User,
            QueuedInputPriority::Normal,
        )
        .await
    }

    pub async fn queued_next_turn(
        &self,
        message: ChatMessage,
        queue_item_id: String,
    ) -> Result<()> {
        self.enqueue_next_turn(
            message,
            Some(queue_item_id),
            QueuedInputSource::User,
            QueuedInputPriority::Normal,
        )
        .await
    }

    pub async fn priority_next_turn(
        &self,
        message: ChatMessage,
        queue_item_id: String,
    ) -> Result<()> {
        self.enqueue_next_turn(
            message,
            Some(queue_item_id),
            QueuedInputSource::RuntimeCallback,
            QueuedInputPriority::High,
        )
        .await
    }

    async fn enqueue_next_turn(
        &self,
        message: ChatMessage,
        queue_item_id: Option<String>,
        source: QueuedInputSource,
        priority: QueuedInputPriority,
    ) -> Result<()> {
        require_user_message(&message)?;
        {
            let mut state = self.state.lock().expect("harness control lock poisoned");
            if queue_item_id.as_ref().is_some_and(|queue_item_id| {
                state
                    .next_turn
                    .iter()
                    .any(|item| item.queue_item_id.as_ref() == Some(queue_item_id))
            }) {
                return Ok(());
            }
            let item = NextTurnInput {
                message,
                queue_item_id,
                source,
                priority,
            };
            if priority == QueuedInputPriority::High {
                let index = state
                    .next_turn
                    .iter()
                    .position(|queued| queued.priority == QueuedInputPriority::Normal)
                    .unwrap_or(state.next_turn.len());
                state.next_turn.insert(index, item);
            } else {
                state.next_turn.push(item);
            }
        }
        self.emit_queue_update().await
    }

    pub async fn drain_steer(&self) -> Result<Vec<ChatMessage>> {
        let messages = {
            let mut state = self.state.lock().expect("harness control lock poisoned");
            let mode = state.steering_mode;
            drain_steering(&mut state, mode)
                .into_iter()
                .map(|item| item.message)
                .collect::<Vec<_>>()
        };
        if !messages.is_empty() {
            self.emit_queue_update().await?;
        }
        Ok(messages)
    }

    /// Run a durable queue mutation while fencing guidance consumption, then
    /// rebuild the active-turn guidance projection from the persisted queue.
    /// Normal next-turn inputs and runtime callbacks intentionally stay out of
    /// this in-memory projection: the session runtime remains their sole
    /// execution source.
    pub async fn mutate_durable_queue<T, F>(
        &self,
        queue_item_id: Option<&str>,
        mutation: F,
    ) -> Result<T>
    where
        F: FnOnce() -> Result<(T, Vec<QueuedInput>)>,
    {
        let value = {
            let mut state = self.state.lock().expect("harness control lock poisoned");
            if queue_item_id.is_some_and(|id| state.consumed_steer_ids.contains(id)) {
                bail!("queue item can no longer be edited");
            }
            let (value, queue) = mutation()?;
            sync_durable_guidance(&mut state, &queue);
            value
        };
        self.emit_queue_update().await?;
        Ok(value)
    }

    /// Drain guidance that missed the final safe point and reconcile it with
    /// the durable queue under the same Harness lock. This prevents a PATCH or
    /// DELETE from racing between the drain and the durable conversion.
    pub async fn drain_pending_guidance<F>(&self, reconcile: F) -> Result<Vec<ChatMessage>>
    where
        F: FnOnce(&[String]) -> Result<()>,
    {
        let messages = {
            let mut state = self.state.lock().expect("harness control lock poisoned");
            let drained = std::mem::take(&mut state.steer);
            let queue_item_ids = drained
                .iter()
                .filter_map(|item| item.queue_item_id.clone())
                .collect::<Vec<_>>();
            for queue_item_id in &queue_item_ids {
                state.consumed_steer_ids.insert(queue_item_id.clone());
            }
            if let Err(error) = reconcile(&queue_item_ids) {
                for queue_item_id in &queue_item_ids {
                    state.consumed_steer_ids.remove(queue_item_id);
                }
                state.steer = drained;
                return Err(error);
            }
            drained
                .into_iter()
                .filter(|item| item.queue_item_id.is_none())
                .map(|item| item.message)
                .collect::<Vec<_>>()
        };
        if !messages.is_empty() {
            self.emit_queue_update().await?;
        }
        Ok(messages)
    }

    pub async fn drain_follow_up(&self) -> Result<Vec<ChatMessage>> {
        let messages = {
            let mut state = self.state.lock().expect("harness control lock poisoned");
            let mode = state.follow_up_mode;
            drain(&mut state.follow_up, mode)
        };
        if !messages.is_empty() {
            self.emit_queue_update().await?;
        }
        Ok(messages)
    }

    pub async fn drain_next_turn(&self) -> Result<Vec<ChatMessage>> {
        let messages: Vec<ChatMessage> = {
            let mut state = self.state.lock().expect("harness control lock poisoned");
            drain_next_turn(&mut state.next_turn)
                .into_iter()
                .map(|item| item.message)
                .collect()
        };
        if !messages.is_empty() {
            self.emit_queue_update().await?;
        }
        Ok(messages)
    }

    pub async fn remove_next_turn_matching(&self, content: &str) -> Result<()> {
        {
            let mut state = self.state.lock().expect("harness control lock poisoned");
            if let Some(index) = state
                .next_turn
                .iter()
                .position(|item| item.message.content == content)
            {
                state.next_turn.remove(index);
            }
        }
        self.emit_queue_update().await
    }

    pub async fn remove_next_turn_by_id(&self, queue_item_id: &str) -> Result<()> {
        {
            let mut state = self.state.lock().expect("harness control lock poisoned");
            if let Some(index) = state
                .next_turn
                .iter()
                .position(|item| item.queue_item_id.as_deref() == Some(queue_item_id))
            {
                state.next_turn.remove(index);
            }
        }
        self.emit_queue_update().await
    }

    pub async fn clear_next_turn(&self) -> Result<()> {
        self.state
            .lock()
            .expect("harness control lock poisoned")
            .next_turn
            .retain(|item| item.source == QueuedInputSource::RuntimeCallback);
        self.emit_queue_update().await
    }

    /// Move queued next-turn messages into both the model input and the
    /// persisted conversation view. Drivers start another turn when this
    /// returns a non-zero count.
    pub async fn consume_next_turn(
        &self,
        messages: &mut Vec<ChatMessage>,
        session: &mut crate::agent_core::contracts::Session,
    ) -> Result<Option<ConsumedNextTurn>> {
        let queued = {
            let mut state = self.state.lock().expect("harness control lock poisoned");
            drain_next_turn(&mut state.next_turn).into_iter().next()
        };
        let Some(queued) = queued else {
            return Ok(None);
        };
        self.emit_queue_update().await?;
        let conversation_message = match queued.source {
            QueuedInputSource::User => {
                crate::agent_core::contracts::ConversationMessage::new_user(&queued.message.content)
            }
            QueuedInputSource::RuntimeCallback => {
                crate::agent_core::contracts::ConversationMessage::new_system(
                    &queued.message.content,
                )
            }
        };
        session.add_message(conversation_message);
        messages.push(queued.message);
        Ok(Some(ConsumedNextTurn {
            queue_item_id: queued.queue_item_id,
            source: queued.source,
        }))
    }

    pub fn set_steering_mode(&self, mode: QueueMode) {
        self.state
            .lock()
            .expect("harness control lock poisoned")
            .steering_mode = mode;
    }

    pub fn set_follow_up_mode(&self, mode: QueueMode) {
        self.state
            .lock()
            .expect("harness control lock poisoned")
            .follow_up_mode = mode;
    }

    pub async fn abort(&self) -> Result<()> {
        let (cleared_steer, cleared_follow_up) = {
            let mut state = self.state.lock().expect("harness control lock poisoned");
            state.cancel_token.cancel();
            let cleared = (state.steer.len(), state.follow_up.len());
            state.steer.clear();
            state.follow_up.clear();
            cleared
        };
        self.events
            .emit(HarnessEvent::Abort {
                cleared_steer,
                cleared_follow_up,
            })
            .await?;
        self.emit_queue_update().await
    }

    pub async fn update_runtime(
        &self,
        model: Option<String>,
        thinking_level: Option<crate::agent_core::contracts::ThinkingLevel>,
        active_tools: Option<Vec<String>>,
        reload_resources: bool,
    ) -> Result<()> {
        let event = {
            let mut state = self.state.lock().expect("harness control lock poisoned");
            if let Some(model) = model {
                state.model = Some(model);
            }
            if let Some(thinking_level) = thinking_level {
                state.thinking_level = Some(thinking_level);
            }
            if let Some(tools) = active_tools {
                state.active_tools = Some(tools);
            }
            if reload_resources {
                state.resource_revision += 1;
            }
            HarnessEvent::RuntimeUpdate {
                model: state.model.clone(),
                thinking_level: state.thinking_level,
                active_tools: state.active_tools.clone(),
                resource_revision: state.resource_revision,
            }
        };
        self.events.emit(event).await
    }

    pub fn requested_model(&self) -> Option<String> {
        self.state
            .lock()
            .expect("harness control lock poisoned")
            .model
            .clone()
    }

    pub fn requested_thinking_level(&self) -> Option<crate::agent_core::contracts::ThinkingLevel> {
        self.state
            .lock()
            .expect("harness control lock poisoned")
            .thinking_level
    }

    pub fn active_tool_names(&self) -> Option<Vec<String>> {
        self.state
            .lock()
            .expect("harness control lock poisoned")
            .active_tools
            .clone()
    }

    pub fn is_tool_active(&self, name: &str) -> bool {
        self.state
            .lock()
            .expect("harness control lock poisoned")
            .active_tools
            .as_ref()
            .map(|tools| tools.iter().any(|tool| tool == name))
            .unwrap_or(true)
    }

    async fn emit_queue_update(&self) -> Result<()> {
        let event = {
            let state = self.state.lock().expect("harness control lock poisoned");
            HarnessEvent::QueueUpdate {
                steer_count: state.steer.len(),
                follow_up_count: state.follow_up.len(),
                next_turn_count: state.next_turn.len(),
            }
        };
        self.events.emit(event).await
    }
}

fn require_user_message(message: &ChatMessage) -> Result<()> {
    if message.role != "user" {
        bail!("harness queues only accept user messages");
    }
    Ok(())
}

fn drain(queue: &mut Vec<ChatMessage>, mode: QueueMode) -> Vec<ChatMessage> {
    match mode {
        QueueMode::All => std::mem::take(queue),
        QueueMode::OneAtATime if queue.is_empty() => Vec::new(),
        QueueMode::OneAtATime => vec![queue.remove(0)],
    }
}

fn drain_steering(state: &mut HarnessControlState, mode: QueueMode) -> Vec<SteeringInput> {
    let drained = match mode {
        QueueMode::All => std::mem::take(&mut state.steer),
        QueueMode::OneAtATime if state.steer.is_empty() => Vec::new(),
        QueueMode::OneAtATime => vec![state.steer.remove(0)],
    };
    for item in &drained {
        if let Some(queue_item_id) = &item.queue_item_id {
            state.consumed_steer_ids.insert(queue_item_id.clone());
        }
    }
    drained
}

fn sync_durable_guidance(state: &mut HarnessControlState, queue: &[QueuedInput]) {
    state.steer.retain(|item| item.queue_item_id.is_none());
    for item in queue.iter().filter(|item| {
        item.source == QueuedInputSource::User
            && item.delivery == QueuedInputDelivery::Guidance
            && item.status == QueuedInputStatus::WaitingSafePoint
            && !state.consumed_steer_ids.contains(&item.id)
    }) {
        state.steer.push(SteeringInput {
            message: queued_execution_message(item),
            queue_item_id: Some(item.id.clone()),
        });
    }

    // Durable next-turn input is consumed directly from SessionManager. Drop
    // stale user projections left by an older daemon while preserving legacy
    // untagged controls and high-priority runtime callbacks until all callers
    // have migrated.
    state.next_turn.retain(|item| {
        item.queue_item_id.is_none() || item.source == QueuedInputSource::RuntimeCallback
    });
}

fn queued_execution_message(item: &QueuedInput) -> ChatMessage {
    let mut message = item.message.clone();
    message.images = item
        .image_urls
        .iter()
        .filter_map(|url| crate::ai::llm::ImageAttachment::from_url(url))
        .collect();
    for reference in &item.file_references {
        let title = reference
            .get("path")
            .or_else(|| reference.get("title"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("附件");
        let content = reference
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        message
            .content
            .push_str(&format!("\n\n[引用文件: {title}]\n{content}"));
    }
    message
}

fn drain_next_turn(queue: &mut Vec<NextTurnInput>) -> Vec<NextTurnInput> {
    if queue.is_empty() {
        Vec::new()
    } else {
        vec![queue.remove(0)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex as AsyncMutex;

    fn user(text: &str) -> ChatMessage {
        ChatMessage {
            role: "user".to_string(),
            content: text.to_string(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[tokio::test]
    async fn drains_one_steering_message_at_a_time() {
        let control = HarnessControl::default();
        control.begin(HarnessPhase::Turn).unwrap();
        control.steer(user("one")).await.unwrap();
        control.steer(user("two")).await.unwrap();
        assert_eq!(control.drain_steer().await.unwrap()[0].content, "one");
        assert_eq!(control.drain_steer().await.unwrap()[0].content, "two");
    }

    #[tokio::test]
    async fn abort_clears_active_queues_but_preserves_next_turn() {
        let control = HarnessControl::default();
        control.begin(HarnessPhase::Turn).unwrap();
        let token = control.cancellation_token();
        control.steer(user("steer")).await.unwrap();
        control.follow_up(user("follow")).await.unwrap();
        control.next_turn(user("next")).await.unwrap();
        control.abort().await.unwrap();
        assert!(token.is_cancelled());
        assert!(control.drain_steer().await.unwrap().is_empty());
        assert!(control.drain_follow_up().await.unwrap().is_empty());
        assert_eq!(control.drain_next_turn().await.unwrap()[0].content, "next");
    }

    #[tokio::test]
    async fn consume_next_turn_updates_runtime_and_session_once() {
        let control = HarnessControl::default();
        control.next_turn(user("queued follow-on")).await.unwrap();
        let mut messages = Vec::new();
        let mut session = crate::agent_core::contracts::Session::new("next-turn");

        assert_eq!(
            control
                .consume_next_turn(&mut messages, &mut session)
                .await
                .unwrap()
                .unwrap()
                .source,
            QueuedInputSource::User
        );
        assert_eq!(messages[0].content, "queued follow-on");
        assert_eq!(session.messages[0].text_content(), "queued follow-on");
        assert_eq!(
            control
                .consume_next_turn(&mut messages, &mut session)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn priority_next_turn_precedes_user_input_and_dedupes_by_queue_id() {
        let control = HarnessControl::default();
        control
            .queued_next_turn(user("user follow-on"), "queue-user".into())
            .await
            .unwrap();
        control
            .priority_next_turn(user("collaborator result"), "queue-callback".into())
            .await
            .unwrap();
        control
            .priority_next_turn(user("collaborator result"), "queue-callback".into())
            .await
            .unwrap();
        let mut messages = Vec::new();
        let mut session = crate::agent_core::contracts::Session::new("priority-next-turn");

        let consumed = control
            .consume_next_turn(&mut messages, &mut session)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(consumed.queue_item_id.as_deref(), Some("queue-callback"));
        assert_eq!(consumed.source, QueuedInputSource::RuntimeCallback);
        assert_eq!(messages[0].content, "collaborator result");
        assert_eq!(
            session.messages[0].role,
            crate::agent_core::contracts::MessageRole::System
        );

        let consumed = control
            .consume_next_turn(&mut messages, &mut session)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(consumed.queue_item_id.as_deref(), Some("queue-user"));
    }

    #[tokio::test]
    async fn settled_waits_for_async_subscribers() {
        let control = HarnessControl::default();
        let events = Arc::new(AsyncMutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        control.events().subscribe(move |event| {
            let captured = Arc::clone(&captured);
            async move {
                tokio::task::yield_now().await;
                captured.lock().await.push(event);
                Ok(())
            }
        });
        control.begin(HarnessPhase::Turn).unwrap();
        control.settle().await.unwrap();
        assert_eq!(events.lock().await.len(), 1);
        assert_eq!(control.phase(), HarnessPhase::Idle);
    }

    #[tokio::test]
    async fn event_history_replays_after_sequence() {
        let bus = HarnessEventBus::default();
        bus.emit(HarnessEvent::Settled { next_turn_count: 0 })
            .await
            .unwrap();
        bus.emit(HarnessEvent::Settled { next_turn_count: 1 })
            .await
            .unwrap();
        let replay = bus.replay_after(1);
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].sequence, 2);
    }

    #[tokio::test]
    async fn runtime_updates_are_visible_without_recreating_control() {
        let control = HarnessControl::default();
        control
            .update_runtime(
                Some("model-b".to_string()),
                Some(crate::agent_core::contracts::ThinkingLevel::High),
                Some(vec!["read_file".to_string()]),
                true,
            )
            .await
            .unwrap();
        assert_eq!(control.requested_model().as_deref(), Some("model-b"));
        assert_eq!(
            control.requested_thinking_level(),
            Some(crate::agent_core::contracts::ThinkingLevel::High)
        );
        assert!(control.is_tool_active("read_file"));
        assert!(!control.is_tool_active("run_command"));
    }
}
