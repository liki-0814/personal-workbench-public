use crate::agent_core::contracts::ports::{
    BackgroundTaskPort, ImageEmitter, OpaqueToolContext, PermissionPort, ProgressEmitter,
    ToolExecutorPort,
};
#[cfg(test)]
use crate::agent_core::contracts::MessageRole;
use crate::agent_core::contracts::Session;
use crate::agent_core::graph::{GraphContext, GraphState};
use crate::agent_core::harness::{
    HarnessAuditSink, HarnessControl, HarnessFactory, HarnessInputs, HarnessPhase, HarnessProfile,
};
use crate::agent_core::hooks::HookRunner;
use crate::agent_core::middleware;
use crate::ai::llm::{ChatMessage, LlmClient, TokenUsage, ToolSchema};
use crate::ai::usage::UsageTracker;
use anyhow::Result;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// 单次 turn 内 LLM 往返轮数的 safety cap。
/// 正常情况下模型会自然终止（响应中无 tool_calls 即 break）；
/// 这个上限只用来兜底防止"模型永远返回 tool_call"的 runaway bug，
/// 让模型自然决定何时停止。
const MAX_TURNS: u32 = 100;

/// Summary of a single LLM turn (one user-input → assistant-output round-trip,
/// possibly involving multiple internal tool-use sub-rounds).
#[derive(Debug, Clone)]
pub struct TurnSummary {
    pub role: String,
    pub content: String,
    pub tool_calls: Option<Vec<ToolCallSummary>>,
    pub duration_ms: u64,
    pub ttft_ms: Option<u64>,
    pub token_usage: Option<TokenUsage>,
}

#[derive(Debug, Clone)]
pub struct ToolCallSummary {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// 用户对一次权限提示的决策
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Deny,
    /// 拒绝并附带原因（如黑名单命中）。会作为 tool_result 的内容回写给模型，
    /// 让它能据此调整下一步行为，而不是反复尝试同一条被拒命令。
    DenyWithReason(String),
}

/// 回调 trait 用于通知调用方工具执行事件（REPL 模式下用于 UI 输出）
pub trait ToolEventSink: Send + Sync {
    fn on_tool_call(&self, id: &str, name: &str, args: &str);
    fn on_tool_result(
        &self,
        id: &str,
        name: &str,
        result: &str,
        is_error: bool,
        failure: Option<&crate::agent_core::reliability::FailureEnvelope>,
    );
    /// Structured tool metadata for UI-native artifacts such as editable documents.
    fn on_tool_details(&self, _id: &str, _name: &str, _details: &serde_json::Value) {}
    /// Structured recovery progress for auto-retry and final recovery outcomes.
    fn on_tool_recovery(
        &self,
        _id: &str,
        _name: &str,
        _failure: &crate::agent_core::reliability::FailureEnvelope,
        _phase: &str,
    ) {
    }

    /// LLM 流式刚发出 ToolCallStart 时立即通知（args 还在累积，可能为空）。
    /// 默认空实现 —— REPL/oneshot sink 关心完整 args，从 `on_tool_call` 收即可；
    /// 仅 service `ChannelSink` 实装，把 trace chip 立刻流到前端 UI。
    fn on_tool_call_streaming(&self, _id: &str, _name: &str) {}
    /// LLM 流式 ToolCallDelta 增量。同上：仅 service 流式 trace 关心。
    fn on_tool_call_args_delta(&self, _id: &str, _delta: &str) {}
    fn on_permission_prompt(&self, name: &str, args: &str);
    fn on_permission_denied(&self, name: &str);
    fn on_decision_started(
        &self,
        _id: &str,
        _trigger: crate::agent_core::decision::DecisionTrigger,
        _risk: crate::agent_core::decision::DecisionRisk,
    ) {
    }
    fn on_decision_advisor(&self, _id: &str, _model: &str, _succeeded: bool) {}
    fn on_decision_resolved(
        &self,
        _id: &str,
        _verdict: &crate::agent_core::decision::DecisionVerdict,
    ) {
    }
    fn on_decision_escalated(
        &self,
        _id: &str,
        _verdict: &crate::agent_core::decision::DecisionVerdict,
    ) {
    }

    /// 每次 LLM 调用前——sink 可在此打印「思考中」占位、重置首字状态
    fn on_thinking_start(&self) {}
    /// AI 助手开始输出（流式首字符到达前）
    fn on_assistant_start(&self) {}
    /// A model-call boundary inside one agent turn. Service sinks use this to
    /// keep progress narration separate from the final answer.
    fn on_assistant_segment_start(&self, _round: u32) {}
    /// AI 助手文本增量
    fn on_text_delta(&self, _delta: &str) {}
    /// 扩展思考增量（thinking_delta / reasoning_content）
    fn on_thinking_delta(&self, _delta: &str) {}
    /// AI 助手输出结束（最后一个 delta 之后）
    fn on_assistant_end(&self) {}
    fn on_assistant_segment_end(&self, _round: u32, _has_tool_calls: bool) {}
    /// Current provider stream was discarded and will be sampled again.
    fn on_stream_retry(&self, _reason: &str) {}

    /// 一次 LLM 调用结束后的实际 token 用量。用于展示活动上下文，不能与整轮累计量混用。
    fn on_context_usage(&self, _usage: TokenUsage, _call_index: u32) {}

    /// 交互式询问用户是否允许此工具执行（Prompt 权限模式触发）
    /// 默认拒绝——非交互上下文（如 service 模式）安全兜底
    fn ask_permission<'a>(
        &'a self,
        _name: &'a str,
        _args: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PermissionDecision> + Send + 'a>> {
        Box::pin(async { PermissionDecision::Deny })
    }

    /// 给指定 tool_call_id 提供一个进度发射器（'static 闭包）。
    /// 长跑工具（如 code_agent）通过 [`crate::runtime::tools::progress::emit`] 调用它，
    /// 把"🔧 Read foo.rs"等进度行流式上抛到 sink。
    /// 默认 None —— REPL/oneshot sink 没有 channel，长跑工具本就只在 service 模式有意义。
    fn progress_emitter_for(&self, _id: &str) -> Option<ProgressEmitter> {
        None
    }

    /// 给指定 tool_call_id 提供一个图像发射器（'static 闭包）。
    /// `generate_image` 工具产出图片时调用 [`crate::runtime::tools::progress::emit_image`]，
    /// 走这个 emitter 上抛到 sink → SSE → 前端 generatedImages[]。
    /// 默认 None —— 仅 service 模式 ChannelSink 重写。
    fn image_emitter_for(&self, _id: &str) -> Option<ImageEmitter> {
        None
    }
}

/// No-op sink for when no real sink is provided. All callbacks are silent.
pub(crate) struct NoopSink;
impl ToolEventSink for NoopSink {
    fn on_tool_call(&self, _id: &str, _name: &str, _args: &str) {}
    fn on_tool_result(
        &self,
        _id: &str,
        _name: &str,
        _result: &str,
        _is_error: bool,
        _failure: Option<&crate::agent_core::reliability::FailureEnvelope>,
    ) {
    }
    fn on_permission_prompt(&self, _name: &str, _args: &str) {}
    fn on_permission_denied(&self, _name: &str) {}
}

#[derive(Debug, Clone, Copy)]
pub struct HarnessRunOptions {
    pub profile: HarnessProfile,
    pub max_rounds: Option<u32>,
    pub externalize_tool_outputs: bool,
    pub thinking: bool,
    pub yolo_mode: bool,
    pub unified_recovery: bool,
}

impl HarnessRunOptions {
    pub fn main(thinking: bool) -> Self {
        Self {
            profile: HarnessProfile::Main,
            max_rounds: None,
            externalize_tool_outputs: true,
            thinking,
            yolo_mode: false,
            unified_recovery: true,
        }
    }

    pub fn oneshot(yolo_mode: bool) -> Self {
        Self::oneshot_with_thinking(yolo_mode, false)
    }

    pub fn oneshot_with_thinking(yolo_mode: bool, thinking: bool) -> Self {
        Self {
            profile: HarnessProfile::Oneshot,
            max_rounds: None,
            externalize_tool_outputs: true,
            thinking,
            yolo_mode,
            unified_recovery: true,
        }
    }
}

pub struct AgentRunner<'a> {
    pub llm: &'a LlmClient,
    pub tool_registry: &'a dyn ToolExecutorPort,
    pub permission_engine: &'a dyn PermissionPort,
    pub hook_runner: &'a HookRunner,
    pub tool_schemas: &'a [ToolSchema],
    pub system_prompt: &'a str,
    pub run_options: HarnessRunOptions,
    /// 可选的事件回调，用于 REPL 模式的 UI 输出
    pub sink: Option<&'a dyn ToolEventSink>,
    /// 可选的取消信号：触发后流式 LLM 调用与 turn 循环会立即返回
    pub cancel_token: Option<&'a CancellationToken>,
    /// 后台任务管理器（service 模式注入，REPL 可选）
    pub background_tasks: Option<&'a dyn BackgroundTaskPort>,
    /// 当前 session_id（后台任务需要关联回 session）
    pub session_id: Option<String>,
    /// Arc 引用的 ToolRegistry，用于后台任务 clone 进 'static future
    /// Unified harness control plane for steering, follow-up, cancellation,
    /// and awaited lifecycle events. Callers that do not need interactive
    /// control can leave it unset without changing historical behavior.
    pub harness: Option<&'a HarnessControl>,
    /// Optional Harness-level MoA reviewer. The primary LLM remains the acting
    /// model; this reviewer is called only by explicit Decision graph nodes.
    pub decision_reviewer: Option<&'a dyn crate::agent_core::decision::DecisionReviewer>,
    /// Optional durable audit target. Absence still emits structured tracing.
    pub audit_sink: Option<&'a dyn HarnessAuditSink>,
    /// Explicit per-invocation tool context. Legacy callers may omit it while
    /// ToolRegistry maintains compatibility task-local scopes.
    pub tool_context: Option<OpaqueToolContext>,
    pub artifact_store: Option<&'a dyn crate::agent_core::contracts::ports::ArtifactStorePort>,
}

impl<'a> AgentRunner<'a> {
    /// 标准 Agent 多轮循环：delegates to AgentGraph.
    pub async fn run_turn(
        &self,
        messages: &mut Vec<ChatMessage>,
        session: &mut Session,
        usage_tracker: &mut UsageTracker,
    ) -> Result<Option<TurnSummary>> {
        let harness_cancel = if let Some(harness) = self.harness {
            Some(match harness.phase() {
                HarnessPhase::Idle => harness.begin(HarnessPhase::Turn)?,
                HarnessPhase::Turn => harness.cancellation_token(),
                phase => anyhow::bail!("agent harness is busy in phase {:?}", phase),
            })
        } else {
            None
        };
        let active_provider = self.llm.provider();
        let context_window = active_provider
            .current_model_context_window()
            .map(|window| window.min(u64::from(u32::MAX)) as u32)
            .unwrap_or_else(|| crate::ai::usage::context_window_for_model(&active_provider.model));
        let (steer_queue_mode, follow_up_queue_mode) =
            self.harness.map(HarnessControl::queue_modes).unwrap_or((
                crate::agent_core::harness::QueueMode::OneAtATime,
                crate::agent_core::harness::QueueMode::OneAtATime,
            ));
        let spec = crate::agent_core::harness::HarnessSpec::resolve(HarnessInputs {
            profile: self.run_options.profile,
            max_rounds: self.run_options.max_rounds.unwrap_or(MAX_TURNS),
            context_window,
            thinking: self.run_options.thinking,
            yolo_mode: self.run_options.yolo_mode,
            externalize_tool_outputs: self.run_options.externalize_tool_outputs,
            unified_recovery: self.run_options.unified_recovery,
            reviewer_enabled: self.decision_reviewer.is_some(),
            background_enabled: self.background_tasks.is_some(),
            steer_queue_mode: match steer_queue_mode {
                crate::agent_core::harness::QueueMode::OneAtATime => {
                    crate::agent_core::harness::spec::QueueModeSpec::OneAtATime
                }
                crate::agent_core::harness::QueueMode::All => {
                    crate::agent_core::harness::spec::QueueModeSpec::All
                }
            },
            follow_up_queue_mode: match follow_up_queue_mode {
                crate::agent_core::harness::QueueMode::OneAtATime => {
                    crate::agent_core::harness::spec::QueueModeSpec::OneAtATime
                }
                crate::agent_core::harness::QueueMode::All => {
                    crate::agent_core::harness::spec::QueueModeSpec::All
                }
            },
            tool_schemas: self.tool_schemas,
        })?;
        let built = HarnessFactory::build(&spec)?;
        if let Some(audit_sink) = self.audit_sink {
            if let Err(error) = audit_sink
                .record_spec(&built.spec, &built.fingerprint)
                .await
            {
                warn!(error = %error, "failed to persist HarnessSpec");
            }
        }
        let fingerprint = built.fingerprint.clone();
        let graph = built.graph;

        // 3. Build GraphState from current messages
        let mut state = GraphState::from_messages(messages.clone());

        // 4. Build GraphContext — temporarily move session into a Mutex
        let noop_sink = NoopSink;
        let sink: &dyn ToolEventSink = match self.sink {
            Some(s) => s,
            None => &noop_sink,
        };

        let default_cancel = CancellationToken::new();
        let cancel_token = self
            .cancel_token
            .or(harness_cancel.as_ref())
            .unwrap_or(&default_cancel);

        let graph_config = built.graph_config;

        // Move session into a Mutex for the graph context
        let taken_session = std::mem::replace(session, Session::new("_placeholder_"));
        let session_mutex = Mutex::new(taken_session);

        let ctx = GraphContext {
            llm: self.llm,
            tool_registry: self.tool_registry,
            permission_engine: self.permission_engine,
            hook_runner: self.hook_runner,
            tool_schemas: self.tool_schemas,
            system_prompt: self.system_prompt,
            config: &graph_config,
            sink,
            cancel_token,
            session: &session_mutex,
            background_tasks: self.background_tasks,
            session_id: self.session_id.as_deref(),
            harness: self.harness,
            decision_reviewer: self.decision_reviewer,
            harness_fingerprint: &fingerprint,
            audit_sink: self.audit_sink,
            tool_context: self.tool_context.clone(),
            artifact_store: self.artifact_store,
        };

        // 5. Run
        let mut run_result = graph.run_turn(&mut state, &ctx).await;
        if let Err(error) = &run_result {
            let message = error.to_string();
            if crate::ai::llm::retry::is_context_overflow(&message) {
                if let Some(harness) = self.harness {
                    harness
                        .events()
                        .emit(crate::agent_core::harness::HarnessEvent::AutoRetryStart {
                            attempt: 1,
                            max_attempts: 2,
                            delay_ms: 0,
                            error: message.clone(),
                        })
                        .await?;
                }
                let before = state.messages.len();
                let compactor = middleware::summarization::SummarizationMiddleware::new(0);
                let compacted =
                    middleware::AgentMiddleware::before_llm(&compactor, &mut state, &ctx)
                        .await
                        .is_ok()
                        && state.messages.len() < before;
                if compacted {
                    run_result = graph.run_turn(&mut state, &ctx).await;
                }
                if let Some(harness) = self.harness {
                    harness
                        .events()
                        .emit(crate::agent_core::harness::HarnessEvent::AutoRetryEnd {
                            attempt: 1,
                            success: run_result.is_ok(),
                            error: run_result.as_ref().err().map(ToString::to_string),
                        })
                        .await?;
                }
            }
        }

        // 6. Sync back: state.messages → messages, session_mutex → session
        *messages = state.messages;
        *session = session_mutex.into_inner();

        if let Some(harness) = self.harness {
            if let Err(settle_error) = harness.settle().await {
                if run_result.is_ok() {
                    return Err(settle_error);
                }
                warn!(error = %settle_error, "harness settle event failed after agent error");
            }
        }

        let summary = run_result?;

        // 7. Record usage
        if let Some(usage) = summary.token_usage {
            usage_tracker.record_turn(usage, &self.llm.provider().model);
        }

        Ok(Some(summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::permissions::PermissionEngine;
    use crate::runtime::tools::registry::ToolRegistry;
    use std::sync::{Arc, Mutex};

    /// 构造一段最简 OpenAI SSE 响应：纯文本，无工具调用
    fn sse_text_only(text: &str) -> String {
        format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            serde_json::json!({"choices":[{"delta":{"content": text},"finish_reason":null}]}),
            serde_json::json!({"choices":[{"delta":{},"finish_reason":"stop"}]})
        )
    }

    /// 构造一段 OpenAI SSE 响应：包含一个完整 tool_call
    fn sse_with_tool_call(id: &str, name: &str, args_json: &str) -> String {
        sse_with_tool_call_finish(id, name, args_json, "tool_calls")
    }

    fn sse_with_tool_call_finish(
        id: &str,
        name: &str,
        args_json: &str,
        finish_reason: &str,
    ) -> String {
        // 拆成 start（含 id+name）+ delta（args 全量一次给）+ done
        format!(
            "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            serde_json::json!({"choices":[{"delta":{"tool_calls":[{
                "index":0,"id":id,"type":"function","function":{"name":name,"arguments":""}
            }]},"finish_reason":null}]}),
            serde_json::json!({"choices":[{"delta":{"tool_calls":[{
                "index":0,"function":{"arguments": args_json}
            }]},"finish_reason":null}]}),
            serde_json::json!({"choices":[{"delta":{},"finish_reason":finish_reason}]})
        )
    }

    fn sse_with_two_tool_calls(first: (&str, &str), second: (&str, &str)) -> String {
        format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            serde_json::json!({"choices":[{"delta":{"tool_calls":[
                {"index":0,"id":first.0,"type":"function","function":{"name":first.1,"arguments":"{}"}},
                {"index":1,"id":second.0,"type":"function","function":{"name":second.1,"arguments":"{}"}}
            ]},"finish_reason":null}]}),
            serde_json::json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]})
        )
    }

    fn make_llm_client(server_url: String) -> LlmClient {
        LlmClient::with_provider(
            crate::ai::config::ProviderConfig {
                name: "t".into(),
                base_url: server_url.clone(),
                api_key: "sk".into(),
                protocol: "openai".into(),
                model: "gpt-4o".into(),
                models: vec![],
                use_proxy: None,
                compat_profile: None,
            },
            server_url,
        )
    }

    struct RecordingSink {
        calls: Arc<Mutex<Vec<(String, String)>>>,
        results: Arc<Mutex<Vec<(String, String, bool)>>>,
    }

    impl ToolEventSink for RecordingSink {
        fn on_tool_call(&self, _id: &str, n: &str, a: &str) {
            self.calls.lock().unwrap().push((n.into(), a.into()));
        }
        fn on_tool_result(
            &self,
            _id: &str,
            n: &str,
            r: &str,
            e: bool,
            _failure: Option<&crate::agent_core::reliability::FailureEnvelope>,
        ) {
            self.results.lock().unwrap().push((n.into(), r.into(), e));
        }
        fn on_permission_prompt(&self, _n: &str, _a: &str) {}
        fn on_permission_denied(&self, _n: &str) {}
    }

    #[tokio::test]
    async fn test_run_turn_no_tool_calls() {
        // LLM 直接给出文本回答，不调工具，循环 1 轮就返回
        let mut server = mockito::Server::new_async().await;
        let _llm = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_text_only("你好！"))
            .expect(1)
            .create();

        let llm = make_llm_client(server.url());
        let registry = ToolRegistry::new();
        let schemas = registry.to_schemas();

        let runner = AgentRunner {
            llm: &llm,
            tool_registry: &registry,
            permission_engine: &PermissionEngine::default_engine(),
            hook_runner: &HookRunner::new(),
            tool_schemas: &schemas,
            system_prompt: "你是助手",
            run_options: HarnessRunOptions::oneshot(false),
            sink: None,
            cancel_token: None,
            background_tasks: None,
            session_id: None,
            harness: None,
            decision_reviewer: None,
            audit_sink: None,
            tool_context: None,
            artifact_store: None,
        };

        let mut msgs = vec![ChatMessage {
            images: vec![],
            generated_images: vec![],
            role: "user".into(),
            content: "你好".into(),
            tool_calls: None,
            tool_call_id: None,
        }];
        let mut sess = Session::new("t");
        let mut usage = UsageTracker::new();

        let summary = runner
            .run_turn(&mut msgs, &mut sess, &mut usage)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summary.content, "你好！");
        // user + assistant
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].role, "assistant");
        assert!(msgs[1].tool_calls.is_none());
    }

    #[tokio::test]
    async fn test_run_turn_consumes_follow_up_before_settling() {
        let mut server = mockito::Server::new_async().await;
        let _first = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_text_only("first answer"))
            .expect(1)
            .create();
        let _second = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_text_only("follow-up answer"))
            .expect(1)
            .create();

        let llm = make_llm_client(server.url());
        let registry = ToolRegistry::new();
        let schemas = registry.to_schemas();
        let harness = HarnessControl::default();
        harness.begin(HarnessPhase::Turn).unwrap();
        harness
            .follow_up(ChatMessage {
                role: "user".into(),
                content: "one more thing".into(),
                images: vec![],
                generated_images: vec![],
                tool_calls: None,
                tool_call_id: None,
            })
            .await
            .unwrap();

        let runner = AgentRunner {
            llm: &llm,
            tool_registry: &registry,
            permission_engine: &PermissionEngine::default_engine(),
            hook_runner: &HookRunner::new(),
            tool_schemas: &schemas,
            system_prompt: "test",
            run_options: HarnessRunOptions::oneshot(true),
            sink: None,
            cancel_token: None,
            background_tasks: None,
            session_id: None,
            harness: Some(&harness),
            decision_reviewer: None,
            audit_sink: None,
            tool_context: None,
            artifact_store: None,
        };
        let mut messages = vec![ChatMessage {
            role: "user".into(),
            content: "start".into(),
            images: vec![],
            generated_images: vec![],
            tool_calls: None,
            tool_call_id: None,
        }];
        let mut session = Session::new("follow-up");
        let mut usage = UsageTracker::new();

        let summary = runner
            .run_turn(&mut messages, &mut session, &mut usage)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(summary.content, "follow-up answer");
        assert_eq!(harness.phase(), HarnessPhase::Idle);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[2].role, "user");
        assert_eq!(messages[2].content, "one more thing");
        assert_eq!(
            session.messages.last().unwrap().role,
            MessageRole::Assistant
        );
        assert!(session
            .messages
            .iter()
            .any(|message| message.role == MessageRole::User
                && message.text_content() == "one more thing"));
    }

    #[tokio::test]
    async fn test_run_turn_single_tool_call_then_text() {
        // LLM 第一次返回 tool_call，第二次返回文本结束
        let mut server = mockito::Server::new_async().await;
        let _todos_get = server
            .mock("GET", "/api/data/todos")
            .with_status(200)
            .with_body(r#"{"data":[{"id":"t1","title":"hello","status":"todo","type":"today"}]}"#)
            .create();

        let _call1 = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_with_tool_call(
                "tc_1",
                "data_crud",
                r#"{"domain":"todo","action":"query"}"#,
            ))
            .expect(1)
            .create();
        let _call2 = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_text_only("当前有 1 个待办：hello"))
            .expect(1)
            .create();

        let llm = make_llm_client(server.url());
        let backend = Arc::new(crate::runtime::backend::BackendClient::new(server.url()));
        let mut registry = ToolRegistry::new();
        crate::runtime::tools::data_crud::register(&mut registry, Arc::clone(&backend));
        let schemas = registry.to_schemas();

        let calls = Arc::new(Mutex::new(vec![]));
        let results = Arc::new(Mutex::new(vec![]));
        let sink = RecordingSink {
            calls: calls.clone(),
            results: results.clone(),
        };

        let runner = AgentRunner {
            llm: &llm,
            tool_registry: &registry,
            permission_engine: &PermissionEngine::default_engine(),
            hook_runner: &HookRunner::new(),
            tool_schemas: &schemas,
            system_prompt: "你是助手",
            run_options: HarnessRunOptions::oneshot(true),
            sink: Some(&sink),
            cancel_token: None,
            background_tasks: None,
            session_id: None,
            harness: None,
            decision_reviewer: None,
            audit_sink: None,
            tool_context: None,
            artifact_store: None,
        };

        let mut msgs = vec![ChatMessage {
            images: vec![],
            generated_images: vec![],
            role: "user".into(),
            content: "列出所有待办".into(),
            tool_calls: None,
            tool_call_id: None,
        }];
        let mut sess = Session::new("t");
        let mut usage = UsageTracker::new();

        let summary = runner
            .run_turn(&mut msgs, &mut sess, &mut usage)
            .await
            .unwrap()
            .unwrap();
        assert!(summary.content.contains("hello"));

        // user + assistant(tool_calls) + tool_result + assistant(text)
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[1].role, "assistant");
        assert!(msgs[1].tool_calls.is_some());
        assert_eq!(msgs[2].role, "tool");
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("tc_1"));
        assert_eq!(msgs[3].role, "assistant");

        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, "data_crud");
    }

    #[tokio::test]
    async fn test_run_turn_does_not_execute_length_truncated_tool_call() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let mut server = mockito::Server::new_async().await;
        let _call1 = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_with_tool_call_finish(
                "tc_truncated",
                "side_effect",
                r#"{\"value\":\"incomplete"#,
                "length",
            ))
            .expect(1)
            .create();
        let _call2 = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_text_only("recovered"))
            .expect(1)
            .create();

        let llm = make_llm_client(server.url());
        let executions = Arc::new(AtomicUsize::new(0));
        let executions_for_handler = Arc::clone(&executions);
        let registry = ToolRegistry::new();
        registry.register(
            "side_effect",
            "Must not run with incomplete arguments",
            serde_json::json!({
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"]
            }),
            Box::new(move |_| {
                let executions = Arc::clone(&executions_for_handler);
                Box::pin(async move {
                    executions.fetch_add(1, Ordering::SeqCst);
                    Ok("executed".to_string())
                })
            }),
        );
        let schemas = registry.to_schemas();
        let runner = AgentRunner {
            llm: &llm,
            tool_registry: &registry,
            permission_engine: &PermissionEngine::default_engine(),
            hook_runner: &HookRunner::new(),
            tool_schemas: &schemas,
            system_prompt: "test",
            run_options: HarnessRunOptions::oneshot(true),
            sink: None,
            cancel_token: None,
            background_tasks: None,
            session_id: None,
            harness: None,
            decision_reviewer: None,
            audit_sink: None,
            tool_context: None,
            artifact_store: None,
        };
        let mut messages = vec![ChatMessage {
            role: "user".into(),
            content: "run it".into(),
            images: vec![],
            generated_images: vec![],
            tool_calls: None,
            tool_call_id: None,
        }];
        let mut session = Session::new("truncated");
        let mut usage = UsageTracker::new();

        let summary = runner
            .run_turn(&mut messages, &mut session, &mut usage)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(summary.content, "recovered");
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        let tool_result = messages
            .iter()
            .find(|message| message.role == "tool")
            .expect("synthetic tool result");
        assert!(tool_result.content.contains("output token limit"));
    }

    #[tokio::test]
    async fn test_run_turn_executes_independent_tool_calls_in_parallel() {
        let mut server = mockito::Server::new_async().await;
        let _call1 = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_with_two_tool_calls(
                ("tc_first", "first"),
                ("tc_second", "second"),
            ))
            .expect(1)
            .create();
        let _call2 = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_text_only("done"))
            .expect(1)
            .create();

        let llm = make_llm_client(server.url());
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let registry = ToolRegistry::new();
        for name in ["first", "second"] {
            let barrier = Arc::clone(&barrier);
            registry.register(
                name,
                "parallel test tool",
                serde_json::json!({ "type": "object" }),
                Box::new(move |_| {
                    let barrier = Arc::clone(&barrier);
                    Box::pin(async move {
                        barrier.wait().await;
                        Ok("ok".to_string())
                    })
                }),
            );
        }
        let schemas = registry.to_schemas();
        let runner = AgentRunner {
            llm: &llm,
            tool_registry: &registry,
            permission_engine: &PermissionEngine::default_engine(),
            hook_runner: &HookRunner::new(),
            tool_schemas: &schemas,
            system_prompt: "test",
            run_options: HarnessRunOptions::oneshot(true),
            sink: None,
            cancel_token: None,
            background_tasks: None,
            session_id: None,
            harness: None,
            decision_reviewer: None,
            audit_sink: None,
            tool_context: None,
            artifact_store: None,
        };
        let mut messages = vec![ChatMessage {
            role: "user".into(),
            content: "run both".into(),
            images: vec![],
            generated_images: vec![],
            tool_calls: None,
            tool_call_id: None,
        }];
        let mut session = Session::new("parallel");
        let mut usage = UsageTracker::new();

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            runner.run_turn(&mut messages, &mut session, &mut usage),
        )
        .await
        .expect("parallel tools should reach the barrier")
        .unwrap();

        let tool_results: Vec<_> = messages
            .iter()
            .filter(|message| message.role == "tool")
            .collect();
        assert_eq!(tool_results.len(), 2);
        assert_eq!(tool_results[0].tool_call_id.as_deref(), Some("tc_first"));
        assert_eq!(tool_results[1].tool_call_id.as_deref(), Some("tc_second"));
    }

    #[tokio::test]
    async fn test_terminating_tool_ends_without_another_model_call() {
        let mut server = mockito::Server::new_async().await;
        let _call = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_with_tool_call("tc_quit", "quit", "{}"))
            .expect(1)
            .create();

        let llm = make_llm_client(server.url());
        let registry = ToolRegistry::new();
        registry.register_structured(
            "quit",
            "terminate the turn",
            serde_json::json!({ "type": "object" }),
            crate::runtime::tools::registry::ToolExecutionMode::Parallel,
            Box::new(|_| {
                Box::pin(async {
                    Ok(crate::runtime::tools::registry::ToolOutput::terminating(
                        "terminated",
                    ))
                })
            }),
        );
        let schemas = registry.to_schemas();
        let runner = AgentRunner {
            llm: &llm,
            tool_registry: &registry,
            permission_engine: &PermissionEngine::default_engine(),
            hook_runner: &HookRunner::new(),
            tool_schemas: &schemas,
            system_prompt: "test",
            run_options: HarnessRunOptions::oneshot(true),
            sink: None,
            cancel_token: None,
            background_tasks: None,
            session_id: None,
            harness: None,
            decision_reviewer: None,
            audit_sink: None,
            tool_context: None,
            artifact_store: None,
        };
        let mut messages = vec![ChatMessage {
            role: "user".into(),
            content: "quit".into(),
            images: vec![],
            generated_images: vec![],
            tool_calls: None,
            tool_call_id: None,
        }];
        let mut session = Session::new("terminate");
        let mut usage = UsageTracker::new();

        runner
            .run_turn(&mut messages, &mut session, &mut usage)
            .await
            .unwrap();

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[2].role, "tool");
        assert_eq!(messages[2].content, "terminated");
    }

    #[tokio::test]
    async fn test_run_turn_max_turns_runaway() {
        // LLM 永远返回 tool_call，循环应在 MAX_TURNS 后兜底返回
        let mut server = mockito::Server::new_async().await;
        let _ = server
            .mock("GET", "/api/data/todos")
            .with_status(200)
            .with_body(r#"{"data":[]}"#)
            .create();

        // 用一个 expect_at_least 让同一个 mock 接住所有请求
        let _llm = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_with_tool_call(
                "tc_runaway",
                "data_crud",
                r#"{"domain":"todo","action":"query"}"#,
            ))
            .expect_at_least(MAX_TURNS as usize)
            .create();

        let llm = make_llm_client(server.url());
        let backend = Arc::new(crate::runtime::backend::BackendClient::new(server.url()));
        let mut registry = ToolRegistry::new();
        crate::runtime::tools::data_crud::register(&mut registry, Arc::clone(&backend));
        let schemas = registry.to_schemas();

        let runner = AgentRunner {
            llm: &llm,
            tool_registry: &registry,
            permission_engine: &PermissionEngine::default_engine(),
            hook_runner: &HookRunner::new(),
            tool_schemas: &schemas,
            system_prompt: "你是助手",
            run_options: HarnessRunOptions::oneshot(true),
            sink: None,
            cancel_token: None,
            background_tasks: None,
            session_id: None,
            harness: None,
            decision_reviewer: None,
            audit_sink: None,
            tool_context: None,
            artifact_store: None,
        };

        let mut msgs = vec![ChatMessage {
            images: vec![],
            generated_images: vec![],
            role: "user".into(),
            content: "列出待办".into(),
            tool_calls: None,
            tool_call_id: None,
        }];
        let mut sess = Session::new("t");
        let mut usage = UsageTracker::new();

        // 不应 panic，应正常返回
        let _ = runner
            .run_turn(&mut msgs, &mut sess, &mut usage)
            .await
            .unwrap();
        // Now delegated to AgentGraph + LoopDetectionMiddleware (hard_limit=5).
        // Rounds 1-3: each produce assistant + tool_result (2 msgs).
        // Round 3 after_llm: 3rd hash repetition → sets pending_warning.
        // Round 4: before_llm injects warning user msg; then assistant + tool (3 msgs).
        //          after_llm: 4th repetition → sets another pending_warning.
        // Round 5: before_llm injects warning; then assistant only (2 msgs).
        //          after_llm: 5th repetition → ForceEnd, clear tool_calls.
        // Total: 1(user) + 3*2(r1-3) + 3(r4: warn+asst+tool) + 2(r5: warn+asst) = 12.
        assert!(
            msgs.len() >= 10 && msgs.len() <= 14,
            "LoopDetectionMiddleware should hard-stop near 5 repetitions, got {} messages",
            msgs.len()
        );
        // Verify loop detection actually stopped the loop (not MAX_TURNS)
        let assistant_count = msgs.iter().filter(|m| m.role == "assistant").count();
        assert!(
            assistant_count <= 6,
            "should have stopped well before MAX_TURNS, got {} assistant msgs",
            assistant_count
        );
    }
}
