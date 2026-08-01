use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::graph::{AgentGraph, GraphConfig};
use crate::llm::ToolSchema;
use crate::middleware;

pub const HARNESS_SCHEMA_VERSION: u32 = 1;
const MIDDLEWARE_REVISION: u32 = 1;
const POLICY_REVISION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessProfile {
    Main,
    Oneshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphPolicySpec {
    pub max_rounds: u32,
    pub context_window: u32,
    pub thinking: bool,
    pub yolo_mode: bool,
    pub allow_tool_output_externalization: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MiddlewareSpec {
    ToolOutputBudget {
        revision: u32,
        externalize_threshold: usize,
        fallback_truncate: usize,
        preview_head: usize,
        preview_tail: usize,
    },
    DanglingToolCall {
        revision: u32,
    },
    LoopDetection {
        revision: u32,
        repeat_warn_threshold: u32,
        repeat_hard_limit: u32,
        window_size: usize,
        tool_frequency_warn: u32,
        tool_frequency_hard: u32,
        max_document_evidence_searches: u32,
    },
    XmlToolCall {
        revision: u32,
    },
    Summarization {
        revision: u32,
        token_threshold: u32,
        keep_recent_tokens: u32,
    },
    SubAgentLimit {
        revision: u32,
        max_concurrent: usize,
    },
}

impl MiddlewareSpec {
    pub fn name(&self) -> &'static str {
        match self {
            Self::ToolOutputBudget { .. } => "tool_output_budget",
            Self::DanglingToolCall { .. } => "dangling_tool_call",
            Self::LoopDetection { .. } => "loop_detection",
            Self::XmlToolCall { .. } => "xml_tool_call",
            Self::Summarization { .. } => "summarization",
            Self::SubAgentLimit { .. } => "subagent_limit",
        }
    }

    pub fn revision(&self) -> u32 {
        match self {
            Self::ToolOutputBudget { revision, .. }
            | Self::DanglingToolCall { revision }
            | Self::LoopDetection { revision, .. }
            | Self::XmlToolCall { revision }
            | Self::Summarization { revision, .. }
            | Self::SubAgentLimit { revision, .. } => *revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionPolicySpec {
    pub revision: u32,
    pub reviewer_enabled: bool,
    pub final_review_round_threshold: u32,
    pub final_review_tool_threshold: u32,
    pub timeout_seconds: u64,
    pub required_consensus_basis_points: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundPolicySpec {
    pub revision: u32,
    pub enabled: bool,
    pub eligible_tools: Vec<String>,
    pub promotion_timeout_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueModeSpec {
    OneAtATime,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuePolicySpec {
    pub revision: u32,
    pub steer: QueueModeSpec,
    pub follow_up: QueueModeSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessSpec {
    pub schema_version: u32,
    pub profile: HarnessProfile,
    pub graph: GraphPolicySpec,
    pub middleware: Vec<MiddlewareSpec>,
    pub decision: DecisionPolicySpec,
    pub background: BackgroundPolicySpec,
    pub queue: QueuePolicySpec,
    pub toolset_fingerprint: String,
}

pub struct HarnessInputs<'a> {
    pub profile: HarnessProfile,
    pub max_rounds: u32,
    pub context_window: u32,
    pub thinking: bool,
    pub yolo_mode: bool,
    pub externalize_tool_outputs: bool,
    pub reviewer_enabled: bool,
    pub background_enabled: bool,
    pub steer_queue_mode: QueueModeSpec,
    pub follow_up_queue_mode: QueueModeSpec,
    pub tool_schemas: &'a [ToolSchema],
}

impl HarnessSpec {
    pub fn resolve(inputs: HarnessInputs<'_>) -> Result<Self> {
        let toolset_fingerprint = toolset_fingerprint(inputs.tool_schemas)?;
        let tool_output = MiddlewareSpec::ToolOutputBudget {
            revision: MIDDLEWARE_REVISION,
            externalize_threshold: 12_000,
            fallback_truncate: 8_000,
            preview_head: 2_000,
            preview_tail: 1_000,
        };
        let dangling = MiddlewareSpec::DanglingToolCall {
            revision: MIDDLEWARE_REVISION,
        };
        let loop_detection = MiddlewareSpec::LoopDetection {
            revision: MIDDLEWARE_REVISION,
            repeat_warn_threshold: 3,
            repeat_hard_limit: 5,
            window_size: 20,
            tool_frequency_warn: 30,
            tool_frequency_hard: 50,
            max_document_evidence_searches: 12,
        };
        let xml = MiddlewareSpec::XmlToolCall {
            revision: MIDDLEWARE_REVISION,
        };

        let mut middleware = Vec::with_capacity(6);
        if inputs.externalize_tool_outputs {
            middleware.push(tool_output);
        }
        middleware.extend([dangling, loop_detection, xml]);
        middleware.push(MiddlewareSpec::Summarization {
            revision: MIDDLEWARE_REVISION,
            token_threshold: inputs.context_window.saturating_mul(70) / 100,
            keep_recent_tokens: 20_000,
        });
        middleware.push(MiddlewareSpec::SubAgentLimit {
            revision: MIDDLEWARE_REVISION,
            max_concurrent: 3,
        });

        Ok(Self {
            schema_version: HARNESS_SCHEMA_VERSION,
            profile: inputs.profile,
            graph: GraphPolicySpec {
                max_rounds: inputs.max_rounds,
                context_window: inputs.context_window,
                thinking: inputs.thinking,
                yolo_mode: inputs.yolo_mode,
                allow_tool_output_externalization: inputs.externalize_tool_outputs,
            },
            middleware,
            decision: DecisionPolicySpec {
                revision: POLICY_REVISION,
                reviewer_enabled: inputs.reviewer_enabled,
                final_review_round_threshold: 6,
                final_review_tool_threshold: 4,
                timeout_seconds: 420,
                required_consensus_basis_points: 6_600,
            },
            background: BackgroundPolicySpec {
                revision: POLICY_REVISION,
                enabled: inputs.background_enabled,
                eligible_tools: if inputs.background_enabled {
                    ["code_agent", "ssh_download", "ssh_execute", "ssh_upload"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect()
                } else {
                    Vec::new()
                },
                promotion_timeout_seconds: 60,
            },
            queue: QueuePolicySpec {
                revision: POLICY_REVISION,
                steer: inputs.steer_queue_mode,
                follow_up: inputs.follow_up_queue_mode,
            },
            toolset_fingerprint,
        })
    }

    pub fn fingerprint(&self) -> Result<HarnessFingerprint> {
        let value = serde_json::to_value(self).context("serialize HarnessSpec")?;
        Ok(HarnessFingerprint(format!(
            "harness:v1:sha256:{}",
            sha256_hex(&canonical_json_bytes(&value)?)
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HarnessFingerprint(pub String);

impl std::fmt::Display for HarnessFingerprint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

pub struct BuiltHarness {
    pub graph: AgentGraph,
    pub graph_config: GraphConfig,
    pub spec: HarnessSpec,
    pub fingerprint: HarnessFingerprint,
}

pub struct HarnessFactory;

impl HarnessFactory {
    pub fn build(spec: &HarnessSpec) -> Result<BuiltHarness> {
        let mut middlewares: Vec<Box<dyn middleware::AgentMiddleware>> =
            Vec::with_capacity(spec.middleware.len());
        for item in &spec.middleware {
            let middleware: Box<dyn middleware::AgentMiddleware> = match item {
                MiddlewareSpec::ToolOutputBudget {
                    externalize_threshold,
                    fallback_truncate,
                    preview_head,
                    preview_tail,
                    ..
                } => Box::new(
                    middleware::tool_output_budget::ToolOutputBudgetMiddleware::from_config(
                        middleware::tool_output_budget::ToolOutputBudgetConfig {
                            externalize_threshold: *externalize_threshold,
                            fallback_truncate: *fallback_truncate,
                            preview_head: *preview_head,
                            preview_tail: *preview_tail,
                        },
                    ),
                ),
                MiddlewareSpec::DanglingToolCall { .. } => {
                    Box::new(middleware::dangling_tool_call::DanglingToolCallMiddleware::new())
                }
                MiddlewareSpec::LoopDetection {
                    repeat_warn_threshold,
                    repeat_hard_limit,
                    window_size,
                    tool_frequency_warn,
                    tool_frequency_hard,
                    ..
                } => Box::new(
                    middleware::loop_detection::LoopDetectionMiddleware::with_limits(
                        *repeat_warn_threshold,
                        *repeat_hard_limit,
                        *window_size,
                        *tool_frequency_warn,
                        *tool_frequency_hard,
                    ),
                ),
                MiddlewareSpec::XmlToolCall { .. } => {
                    Box::new(middleware::xml_tool_call::XmlToolCallMiddleware::new())
                }
                MiddlewareSpec::Summarization {
                    token_threshold, ..
                } => Box::new(middleware::summarization::SummarizationMiddleware::new(
                    *token_threshold,
                )),
                MiddlewareSpec::SubAgentLimit { max_concurrent, .. } => Box::new(
                    middleware::subagent_limit::SubAgentLimitMiddleware::new(*max_concurrent),
                ),
            };
            middlewares.push(middleware);
        }

        let graph_config = GraphConfig {
            max_rounds: spec.graph.max_rounds,
            thinking: spec.graph.thinking,
            yolo_mode: spec.graph.yolo_mode,
            decision_review_round_threshold: spec.decision.final_review_round_threshold,
            decision_review_tool_threshold: spec.decision.final_review_tool_threshold,
            decision_timeout_seconds: spec.decision.timeout_seconds,
            required_consensus_basis_points: spec.decision.required_consensus_basis_points,
            background_promotion_timeout_seconds: spec.background.promotion_timeout_seconds,
            background_eligible_tools: spec.background.eligible_tools.clone(),
        };
        let graph = AgentGraph::builder()
            .config(graph_config.clone())
            .middlewares(middlewares)
            .build();
        Ok(BuiltHarness {
            graph,
            graph_config,
            spec: spec.clone(),
            fingerprint: spec.fingerprint()?,
        })
    }
}

pub fn toolset_fingerprint(schemas: &[ToolSchema]) -> Result<String> {
    let mut tools = schemas
        .iter()
        .map(|schema| {
            Ok((
                schema.function.name.clone(),
                serde_json::to_value(schema).context("serialize tool schema")?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    tools.sort_by(|left, right| left.0.cmp(&right.0));
    let value = Value::Array(tools.into_iter().map(|(_, value)| value).collect());
    Ok(format!(
        "toolset:v1:sha256:{}",
        sha256_hex(&canonical_json_bytes(&value)?)
    ))
}

fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>> {
    serde_json::to_vec(&canonicalize(value)).context("serialize canonical JSON")
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        _ => value.clone(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::llm::FunctionSchema;

    fn schema(name: &str, parameters: Value) -> ToolSchema {
        ToolSchema {
            kind: "function".into(),
            function: FunctionSchema {
                name: name.into(),
                description: format!("{name} description"),
                parameters,
            },
        }
    }

    fn inputs<'a>(schemas: &'a [ToolSchema]) -> HarnessInputs<'a> {
        HarnessInputs {
            profile: HarnessProfile::Main,
            max_rounds: 100,
            context_window: 100_000,
            thinking: false,
            yolo_mode: false,
            externalize_tool_outputs: true,
            reviewer_enabled: true,
            background_enabled: true,
            steer_queue_mode: QueueModeSpec::OneAtATime,
            follow_up_queue_mode: QueueModeSpec::OneAtATime,
            tool_schemas: schemas,
        }
    }

    #[test]
    fn canonical_object_order_is_stable() {
        let first = vec![schema("read", json!({"type":"object","a":1,"b":2}))];
        let second = vec![schema("read", json!({"b":2,"a":1,"type":"object"}))];
        assert_eq!(
            HarnessSpec::resolve(inputs(&first))
                .unwrap()
                .fingerprint()
                .unwrap(),
            HarnessSpec::resolve(inputs(&second))
                .unwrap()
                .fingerprint()
                .unwrap()
        );
    }

    #[test]
    fn tool_order_is_stable_but_toolset_changes_are_visible() {
        let read = schema("read", json!({"type":"object"}));
        let write = schema("write", json!({"type":"object"}));
        assert_eq!(
            toolset_fingerprint(&[read.clone(), write.clone()]).unwrap(),
            toolset_fingerprint(&[write.clone(), read.clone()]).unwrap()
        );
        assert_ne!(
            toolset_fingerprint(&[read]).unwrap(),
            toolset_fingerprint(&[write]).unwrap()
        );
    }

    #[test]
    fn middleware_order_changes_fingerprint() {
        let schemas = vec![schema("read", json!({"type":"object"}))];
        let mut spec = HarnessSpec::resolve(inputs(&schemas)).unwrap();
        let original = spec.fingerprint().unwrap();
        spec.middleware.swap(0, 1);
        assert_ne!(original, spec.fingerprint().unwrap());
    }
}
