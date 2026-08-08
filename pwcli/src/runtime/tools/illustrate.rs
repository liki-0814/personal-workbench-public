use serde_json::{json, Value};

use crate::runtime::illustration::{IllustrationRequest, IllustrationService};
use crate::runtime::tools::registry::{ToolExecutionMode, ToolImpact, ToolOutput, ToolRegistry};

pub fn register(registry: &ToolRegistry) {
    registry.register_contextual_structured_with_impact(
        "illustrate",
        "生成、编辑或评估学术插图与真实数据图。统一执行参考检索、规划、风格增强、渲染和多轮视觉验收；统计图始终使用隔离 matplotlib，不使用生图模型。",
        json!({
            "type": "object",
            "properties": {
                "mode": { "type": "string", "enum": ["auto", "diagram", "plot", "polish", "refine", "eval"], "default": "auto" },
                "content": { "description": "方法文本，或 plot 使用的结构化原始数据" },
                "visualIntent": { "type": "string", "minLength": 1 },
                "imageRefs": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string", "pattern": "^imgref_" },
                            "role": { "type": "string", "enum": ["edit-target", "content", "style", "composition"] }
                        },
                        "required": ["id", "role"], "additionalProperties": false
                    }
                },
                "quality": { "type": "string", "enum": ["fast", "balanced", "max"], "default": "balanced" },
                "retrieval": { "type": "string", "enum": ["auto", "none"], "default": "auto" },
                "aspectRatio": { "type": "string", "enum": ["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9"] },
                "constraints": {
                    "type": "object",
                    "properties": {
                        "invariants": { "type": "array", "items": { "type": "string" } },
                        "exactText": { "type": "array", "items": { "type": "string" } },
                        "factualConstraints": { "type": "array", "items": { "type": "string" } }
                    }, "additionalProperties": false
                }
            },
            "required": ["content", "visualIntent"], "additionalProperties": false
        }),
        ToolExecutionMode::Sequential,
        ToolImpact::ReversibleMutation,
        Box::new(|context, args: &Value| {
            let request = serde_json::from_value::<IllustrationRequest>(args.clone());
            let context = context.clone();
            Box::pin(async move {
                let request = request.map_err(|error| anyhow::anyhow!("illustrate 参数无效：{error}"))?;
                let data_dir = crate::runtime::settings::local_config::data_dir();
                let run = IllustrationService::new(&data_dir).execute(&context, request).await?;
                let selected = run.selected_candidate_id.as_deref().unwrap_or("none");
                Ok(ToolOutput {
                    content: format!("学术绘图工作流完成：run_id={} mode={:?} selected={} status={:?}。完整 Planner/Stylist/Critic 演化记录已持久化。", run.id, run.resolved_mode, selected, run.status),
                    terminate: false,
                    details: Some(serde_json::to_value(&run)?),
                    added_tool_names: Vec::new(),
                })
            })
        }),
    );
}
