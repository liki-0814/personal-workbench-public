use serde::{Deserialize, Serialize};
use serde_json::Value;

// ProviderConfig 定义在 config 模块，这里 re-export
pub use crate::config::ProviderConfig;

/// 图片附件
#[derive(Debug, Clone)]
pub struct ImageAttachment {
    pub data: String,       // base64 (不含 data:... 前缀)
    pub media_type: String, // e.g. "image/png"
}

impl ImageAttachment {
    /// 从 data URL ("data:image/png;base64,xxxxx") 解析
    pub fn from_data_url(url: &str) -> Option<Self> {
        let rest = url.strip_prefix("data:")?;
        let (meta, data) = rest.split_once(";base64,")?;
        Some(Self {
            media_type: meta.to_string(),
            data: data.to_string(),
        })
    }

    /// 从 server URL ("/api/images/xxx.png") 同步下载并转为 base64
    fn from_server_url(url: &str, backend_base: &str) -> Option<Self> {
        let full_url = if url.starts_with("http") {
            url.to_string()
        } else {
            format!("{}{}", backend_base, url)
        };
        let resp = reqwest::blocking::get(&full_url).ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/png")
            .to_string();
        let media_type = if content_type.contains(';') {
            content_type
                .split(';')
                .next()
                .unwrap_or("image/png")
                .trim()
                .to_string()
        } else {
            content_type
        };
        let bytes = resp.bytes().ok()?;
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Some(Self { media_type, data })
    }

    /// 解析图片 URL：支持 data URL 和 server URL
    pub fn from_url(url: &str) -> Option<Self> {
        if url.starts_with("data:") {
            Self::from_data_url(url)
        } else if url.starts_with("/api/") {
            let backend = crate::config::RuntimeConfig::load().backend_url;
            Self::from_server_url(url, &backend)
        } else {
            None
        }
    }
}

/// 统一消息格式（兼容 OpenAI / Anthropic）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// 前端发来的 images 可以是 data URL 或 server URL，反序列化时自动解析
    #[serde(default, deserialize_with = "deserialize_images", skip_serializing)]
    pub images: Vec<ImageAttachment>,
    /// 历史工具生图。仅用于为当前 turn 建立不透明图片引用，不发送给 LLM transport。
    #[serde(
        rename = "generatedImages",
        default,
        deserialize_with = "deserialize_images",
        skip_serializing
    )]
    pub generated_images: Vec<ImageAttachment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

fn deserialize_images<'de, D>(deserializer: D) -> Result<Vec<ImageAttachment>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let urls: Vec<String> = Vec::deserialize(deserializer).unwrap_or_default();
    Ok(urls
        .iter()
        .filter_map(|u| ImageAttachment::from_url(u))
        .collect())
}

/// 工具调用（OpenAI 格式，内部统一）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
    /// Gemini thought signature — must be echoed back in the next turn
    #[serde(
        rename = "thoughtSignature",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// 工具 Schema（OpenAI function 格式）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSchema {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Token 用量
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// 非流式 AI 响应
#[derive(Debug, Clone)]
pub struct AiResponse {
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Stop,
    ToolUse,
    Length,
    Other(String),
}

impl StopReason {
    pub fn from_provider(value: &str) -> Self {
        match value {
            "stop" | "end_turn" | "stop_sequence" => Self::Stop,
            "tool_calls" | "tool_use" => Self::ToolUse,
            "length" | "max_tokens" | "model_length" => Self::Length,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn is_length(&self) -> bool {
        matches!(self, Self::Length)
    }
}

/// 流式事件
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// 首 token 到达（用于 TTFT 计算）
    FirstToken,
    /// 文本增量
    TextDelta(String),
    /// 扩展思考增量（Anthropic thinking_delta / OpenAI 兼容 reasoning_content）
    ThinkingDelta(String),
    /// 工具调用开始
    ToolCallStart {
        id: String,
        name: String,
        thought_signature: Option<String>,
    },
    /// 工具调用参数增量
    ToolCallDelta { id: String, arguments_delta: String },
    /// 工具调用结束
    ToolCallEnd { id: String },
    /// 工具执行结果（pwcli 端执行后产出的结果，前端用于渲染 tool trace）
    ToolResult {
        id: String,
        name: String,
        result: String,
        is_error: bool,
        failure: Option<crate::reliability::FailureEnvelope>,
    },
    /// Structured recovery progress for a tool call (auto-retry / final outcome).
    ToolRecovery {
        id: String,
        name: String,
        failure: crate::reliability::FailureEnvelope,
        phase: String,
    },
    /// 工具执行期进度行（仅长跑工具如 code_agent 发出，
    /// 前端在对应 trace chip 内追加显示。每行已格式化好可直接渲染）
    ToolProgress { id: String, line: String },
    /// 工具产出的图片 URL（generate_image 工具用），
    /// 前端 push 到当前 assistant 消息的 generatedImages[] 内联渲染。
    ToolImage {
        id: String,
        url: String,
        alt: String,
        record: Option<Box<crate::visual_generation::GeneratedImageRecord>>,
    },
    /// A tool created or updated an editable report/slide document.
    ToolDocument {
        id: String,
        document: serde_json::Value,
    },
    /// A tool paused the turn for a structured user choice.
    ToolDecision {
        id: String,
        decision: serde_json::Value,
    },
    /// A code_agent native CLI session was attached to the durable
    /// collaboration surface.
    /// Provider 结束原因。先于 Done 发出，供 Harness 拒绝执行被截断的工具参数。
    ResponseStop(StopReason),
    /// 单次 LLM 调用的上下文用量。与 Done 中整个 Agent turn 的累计用量分离。
    ContextUsage { usage: TokenUsage, call_index: u32 },
    /// 完成事件（携带用量）
    Done(Option<TokenUsage>),
    /// 错误事件
    Error(String),
    /// Harness discarded a pathological partial response and is resampling.
    StreamReset { reason: String },
    DecisionStarted {
        id: String,
        trigger: String,
        risk: String,
    },
    DecisionAdvisor {
        id: String,
        model: String,
        status: String,
    },
    DecisionResolved {
        id: String,
        outcome: String,
        confidence: f32,
        consensus: f32,
        rationale: String,
    },
    DecisionEscalated {
        id: String,
        rationale: String,
        options: Vec<crate::fusion::DecisionOption>,
    },
}

/// LLM 请求体（内部使用）
#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub messages: Vec<ChatMessage>,
    pub system_prompt: Option<String>,
    pub tools: Option<Vec<ToolSchema>>,
    pub stream: bool,
    /// 强制工具调用: "auto" | "required" | "none" | {"type": "function", "function": {"name": "..."}}
    pub tool_choice: Option<String>,
    /// 启用扩展思考模式。各 provider 字段名不同，由具体客户端翻译：
    /// - Anthropic: `thinking: { type:"enabled", budget_tokens:1024 }`
    /// - OpenAI-compatible providers: `enable_thinking: true`
    pub thinking: bool,
    /// 覆盖默认 max_tokens（None 时由协议客户端用 DEFAULT_MAX_TOKENS）
    pub max_tokens: Option<u32>,
    /// Optional sampling temperature. None omits the field so the provider
    /// keeps its model default.
    pub temperature: Option<f32>,
}

/// Provider 协议枚举（wire name 使用 snake/kebab 兼容写法）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderProtocol {
    OpenAiChat,
    OpenAiResponses,
    OpenAiCodexResponses,
    AnthropicMessages,
    GoogleGenerative,
    GoogleAntigravity,
}

impl ProviderProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiChat => "openai_chat",
            Self::OpenAiResponses => "openai_responses",
            Self::OpenAiCodexResponses => "openai_codex_responses",
            Self::AnthropicMessages => "anthropic_messages",
            Self::GoogleGenerative => "google_generative",
            Self::GoogleAntigravity => "google_antigravity",
        }
    }

    /// Parse a configured protocol name. Unknown values return an error instead
    /// of silently falling back to OpenAI.
    pub fn parse(s: &str) -> Result<Self, String> {
        let normalized = s.trim().to_ascii_lowercase().replace('-', "_");
        match normalized.as_str() {
            // New canonical names
            "openai_chat" | "openai" | "openai_compatible" => Ok(Self::OpenAiChat),
            "openai_responses" | "responses" => Ok(Self::OpenAiResponses),
            "openai_codex_responses" | "codex_responses" => Ok(Self::OpenAiCodexResponses),
            "anthropic_messages" | "anthropic" => Ok(Self::AnthropicMessages),
            "google_generative" | "google" | "gemini" | "generative_language" => {
                Ok(Self::GoogleGenerative)
            }
            "google_antigravity" | "antigravity" | "cloud_code_assist" => {
                Ok(Self::GoogleAntigravity)
            }
            other if other.is_empty() => Err("protocol is required".into()),
            other => Err(format!("unsupported protocol '{other}'")),
        }
    }

    /// Legacy helper used by older call sites. Prefer `parse`.
    pub fn from_name(s: &str) -> Self {
        Self::parse(s).unwrap_or(Self::OpenAiChat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_event_variants() {
        let events = [
            StreamEvent::FirstToken,
            StreamEvent::TextDelta("hello".to_string()),
            StreamEvent::Done(None),
        ];
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn stop_reason_normalizes_provider_values() {
        assert_eq!(StopReason::from_provider("tool_calls"), StopReason::ToolUse);
        assert_eq!(StopReason::from_provider("max_tokens"), StopReason::Length);
        assert!(StopReason::from_provider("length").is_length());
    }

    #[test]
    fn test_provider_protocol_from_name() {
        assert_eq!(
            ProviderProtocol::parse("openai").unwrap(),
            ProviderProtocol::OpenAiChat
        );
        assert_eq!(
            ProviderProtocol::parse("openai-chat").unwrap(),
            ProviderProtocol::OpenAiChat
        );
        assert_eq!(
            ProviderProtocol::parse("anthropic").unwrap(),
            ProviderProtocol::AnthropicMessages
        );
        assert_eq!(
            ProviderProtocol::parse("openai_responses").unwrap(),
            ProviderProtocol::OpenAiResponses
        );
        assert_eq!(
            ProviderProtocol::parse("openai-codex-responses").unwrap(),
            ProviderProtocol::OpenAiCodexResponses
        );
        assert_eq!(
            ProviderProtocol::parse("google-generative").unwrap(),
            ProviderProtocol::GoogleGenerative
        );
        assert!(ProviderProtocol::parse("bedrock").is_err());
    }

    #[test]
    fn test_token_usage_default() {
        let usage = TokenUsage::default();
        assert_eq!(usage.prompt_tokens, 0);
        assert_eq!(usage.completion_tokens, 0);
        assert_eq!(usage.total_tokens, 0);
    }

    #[test]
    fn test_provider_config_serde() {
        let cfg = ProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "sk-test".to_string(),
            protocol: "openai".to_string(),
            model: "gpt-4".to_string(),
            models: Vec::new(),
            use_proxy: None,
            compat_profile: None,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let decoded: ProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, decoded);
    }
}
