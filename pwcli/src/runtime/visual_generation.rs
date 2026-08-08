use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::ai::llm::models::{ChatMessage, ImageAttachment};
use crate::runtime::settings::local_config::ResponseLanguage;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum VisualIntent {
    #[default]
    Generate,
    Reference,
    Edit,
    Variant,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum VisualScene {
    #[default]
    Auto,
    Photo,
    Illustration,
    Poster,
    Infographic,
    Scientific,
    Product,
    UiMockup,
    Asset,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VisualStage {
    Briefing,
    ResolvingInputs,
    Routing,
    CompilingPrompt,
    Generating,
    Validating,
    Repairing,
    Ready,
    ReadyWithWarnings,
    Failed,
    Cancelled,
}

impl VisualStage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Briefing => "正在整理视觉需求",
            Self::ResolvingInputs => "正在解析参考图片",
            Self::Routing => "正在选择能力匹配的生图模型",
            Self::CompilingPrompt => "正在编译场景 Prompt",
            Self::Generating => "正在生成图片",
            Self::Validating => "正在进行视觉验收",
            Self::Repairing => "正在定点修复",
            Self::Ready => "图片已生成并通过验收",
            Self::ReadyWithWarnings => "图片已生成，存在验收提示",
            Self::Failed => "生图失败",
            Self::Cancelled => "生图已取消",
        }
    }
}

impl VisualScene {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Photo => "photo",
            Self::Illustration => "illustration",
            Self::Poster => "poster",
            Self::Infographic => "infographic",
            Self::Scientific => "scientific",
            Self::Product => "product",
            Self::UiMockup => "ui-mockup",
            Self::Asset => "asset",
        }
    }
}

pub fn compile_prompt(
    brief: &VisualBrief,
    aspect_ratio: &str,
    language: ResponseLanguage,
) -> String {
    let mut sections = Vec::new();
    if brief.purpose.is_some() || brief.audience.is_some() {
        sections.push(match language {
            ResponseLanguage::Chinese | ResponseLanguage::Auto => format!(
                "用途与受众：{}{}",
                brief.purpose.as_deref().unwrap_or("未指定用途"),
                brief
                    .audience
                    .as_deref()
                    .map(|value| format!("；面向{value}"))
                    .unwrap_or_default()
            ),
            ResponseLanguage::English => format!(
                "Purpose and audience: {}{}",
                brief.purpose.as_deref().unwrap_or("unspecified purpose"),
                brief
                    .audience
                    .as_deref()
                    .map(|value| format!("; for {value}"))
                    .unwrap_or_default()
            ),
        });
    }
    sections.push(match language {
        ResponseLanguage::Chinese | ResponseLanguage::Auto => {
            format!("用户主请求：{}", brief.prompt.trim())
        }
        ResponseLanguage::English => format!("User request: {}", brief.prompt.trim()),
    });
    if !brief.references.is_empty() {
        let references = brief
            .references
            .iter()
            .map(|reference| format!("{}={:?}", reference.id, reference.role))
            .collect::<Vec<_>>()
            .join(
                if matches!(language, ResponseLanguage::Chinese | ResponseLanguage::Auto) {
                    "；"
                } else {
                    "; "
                },
            );
        sections.push(match language {
            ResponseLanguage::Chinese | ResponseLanguage::Auto => {
                format!("输入图片角色：{references}")
            }
            ResponseLanguage::English => format!("Input image roles: {references}"),
        });
    }
    if !brief.exact_text.is_empty() {
        let exact_text = brief
            .exact_text
            .iter()
            .map(|value| format!("「{value}」"))
            .collect::<Vec<_>>()
            .join(
                if matches!(language, ResponseLanguage::Chinese | ResponseLanguage::Auto) {
                    "、"
                } else {
                    ", "
                },
            );
        sections.push(match language {
            ResponseLanguage::Chinese | ResponseLanguage::Auto => format!(
                "必须逐字呈现且不得增删的文字：{exact_text}。除这些文字外不要生成额外文字。"
            ),
            ResponseLanguage::English => format!(
                "Render this text exactly without additions or omissions: {exact_text}. Do not generate any other text."
            ),
        });
    }
    sections.push(match language {
        ResponseLanguage::Chinese | ResponseLanguage::Auto => format!(
            "构图与比例：画面宽高比 {aspect_ratio}。{}",
            scene_recipe(brief.scene, language)
        ),
        ResponseLanguage::English => format!(
            "Composition and aspect ratio: use {aspect_ratio}. {}",
            scene_recipe(brief.scene, language)
        ),
    });
    if !brief.invariants.is_empty() {
        sections.push(match language {
            ResponseLanguage::Chinese | ResponseLanguage::Auto => {
                format!("必须保持：{}", brief.invariants.join("；"))
            }
            ResponseLanguage::English => {
                format!("Must preserve: {}", brief.invariants.join("; "))
            }
        });
    }
    if !brief.factual_constraints.is_empty() {
        sections.push(match language {
            ResponseLanguage::Chinese | ResponseLanguage::Auto => format!(
                "事实约束（不得推断或改写）：{}",
                brief.factual_constraints.join("；")
            ),
            ResponseLanguage::English => format!(
                "Factual constraints (do not infer or rewrite): {}",
                brief.factual_constraints.join("; ")
            ),
        });
    }
    if !brief.exclusions.is_empty() {
        sections.push(match language {
            ResponseLanguage::Chinese | ResponseLanguage::Auto => {
                format!("禁止出现：{}", brief.exclusions.join("；"))
            }
            ResponseLanguage::English => {
                format!("Exclude: {}", brief.exclusions.join("; "))
            }
        });
    }
    if matches!(brief.intent, VisualIntent::Edit) {
        sections.push(match language {
            ResponseLanguage::Chinese | ResponseLanguage::Auto => "编辑原则：只修改用户明确指定的对象，其余主体、布局、身份特征、产品几何、文字和背景保持不变。".into(),
            ResponseLanguage::English => "Editing rule: modify only the objects explicitly specified by the user. Preserve all other subjects, layout, identity traits, product geometry, text, and background.".into(),
        });
    }
    sections.join("\n")
}

fn scene_recipe(scene: VisualScene, language: ResponseLanguage) -> &'static str {
    match (scene, language) {
        (VisualScene::Auto, ResponseLanguage::Chinese) => "建立清晰主体、稳定视觉层级、自然细节和统一质感，避免模板化与廉价 AI 观感。",
        (VisualScene::Photo, ResponseLanguage::Chinese) => "使用可信镜头语言、自然光影和空间关系，保留皮肤、织物、表面与环境的真实纹理。",
        (VisualScene::Illustration, ResponseLanguage::Chinese) => "统一媒介、线条、色彩与层次，所有对象保持同一插画语言和细节密度。",
        (VisualScene::Poster, ResponseLanguage::Chinese) => "先建立传播标题层级与阅读路径，控制留白、对齐和信息密度，文案必须准确。",
        (VisualScene::Infographic, ResponseLanguage::Chinese) => "用明确节点、关系、箭头、标签和单向阅读路径表达信息，不添加未提供的数据。",
        (VisualScene::Scientific, ResponseLanguage::Chinese) => "术语、结构、方向与因果关系必须服从事实约束；只画概念示意，不伪造实验数据、显微结果或医学证据。",
        (VisualScene::Product, ResponseLanguage::Chinese) => "严格保持产品几何、颜色、材质、标签和品牌识别，使用可信棚拍光线与接触阴影。",
        (VisualScene::UiMockup, ResponseLanguage::Chinese) => "使用真实产品信息层级、组件约束、一致间距和可操作状态，避免装饰性伪 UI。",
        (VisualScene::Asset, ResponseLanguage::Chinese) => "轮廓清晰、边缘干净、背景纯净，在小尺寸下仍可识别，不添加水印或多余装饰。",
        (VisualScene::Auto, ResponseLanguage::English) => "Create a clear subject, stable visual hierarchy, natural detail, and coherent texture. Avoid template-like or cheap AI aesthetics.",
        (VisualScene::Photo, ResponseLanguage::English) => "Use credible camera language, natural lighting, and spatial relationships. Preserve realistic skin, fabric, surface, and environmental textures.",
        (VisualScene::Illustration, ResponseLanguage::English) => "Keep the medium, line work, color, depth, visual language, and detail density consistent across all objects.",
        (VisualScene::Poster, ResponseLanguage::English) => "Establish title hierarchy and reading path first. Control whitespace, alignment, and information density, and render copy accurately.",
        (VisualScene::Infographic, ResponseLanguage::English) => "Express information with explicit nodes, relationships, arrows, labels, and a one-way reading path. Do not add unprovided data.",
        (VisualScene::Scientific, ResponseLanguage::English) => "Terms, structure, direction, and causal relationships must follow the factual constraints. Draw only a conceptual schematic; do not fabricate experimental data, microscopy results, or medical evidence.",
        (VisualScene::Product, ResponseLanguage::English) => "Strictly preserve product geometry, color, materials, labels, and brand identity. Use credible studio lighting and contact shadows.",
        (VisualScene::UiMockup, ResponseLanguage::English) => "Use realistic product information hierarchy, component constraints, consistent spacing, and actionable states. Avoid decorative fake UI.",
        (VisualScene::Asset, ResponseLanguage::English) => "Keep contours clear, edges clean, and the background uncluttered. Preserve recognizability at small sizes without watermarks or extra decoration.",
        (scene, ResponseLanguage::Auto) => scene_recipe(scene, ResponseLanguage::Chinese),
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ImageReferenceRole {
    EditTarget,
    Content,
    Style,
    Composition,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageReferenceRequest {
    pub id: String,
    pub role: ImageReferenceRole,
}

#[derive(Debug, Clone)]
pub struct ResolvedImageReference {
    pub id: String,
    pub source_id: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct ImageReferenceRegistry {
    entries: HashMap<String, ResolvedImageReference>,
}

impl ImageReferenceRegistry {
    pub fn resolve(&self, id: &str) -> Result<ResolvedImageReference> {
        self.entries
            .get(id)
            .cloned()
            .with_context(|| format!("图片引用 {id} 不存在、已过期或不属于当前会话"))
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 为一次 Agent turn 建立 task-local 图片引用，并只把不透明 ID 告知模型。
pub fn build_image_reference_registry(messages: &mut [ChatMessage]) -> Arc<ImageReferenceRegistry> {
    let mut registry = ImageReferenceRegistry::default();
    for (message_index, message) in messages.iter_mut().enumerate() {
        let mut annotations = Vec::new();
        register_attachments(
            &mut registry,
            &mut annotations,
            message_index,
            "upload",
            &message.images,
        );
        register_attachments(
            &mut registry,
            &mut annotations,
            message_index,
            "generated",
            &message.generated_images,
        );
        if !annotations.is_empty() {
            message
                .content
                .push_str("\n\n[当前会话可用图片引用（仅可原样传给 generate_image.imageRefs）]\n");
            message.content.push_str(&annotations.join("\n"));
        }
    }
    Arc::new(registry)
}

fn register_attachments(
    registry: &mut ImageReferenceRegistry,
    annotations: &mut Vec<String>,
    message_index: usize,
    kind: &str,
    attachments: &[ImageAttachment],
) {
    for (image_index, attachment) in attachments.iter().enumerate() {
        let id = format!("imgref_{message_index}_{kind}_{image_index}");
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&attachment.data) else {
            continue;
        };
        if bytes.is_empty() || bytes.len() > 20 * 1024 * 1024 {
            continue;
        }
        let Some(detected_mime) = crate::runtime::media::detect_raster_mime(&bytes) else {
            continue;
        };
        if attachment.media_type != detected_mime
            && !(attachment.media_type == "image/jpg" && detected_mime == "image/jpeg")
        {
            continue;
        }
        registry.entries.insert(
            id.clone(),
            ResolvedImageReference {
                id: id.clone(),
                source_id: format!("message:{message_index}:{kind}:{image_index}"),
                media_type: detected_mime.into(),
                bytes,
            },
        );
        annotations.push(format!("- {id}: {kind} image"));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VisualBrief {
    pub intent: VisualIntent,
    pub scene: VisualScene,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    #[serde(default)]
    pub exact_text: Vec<String>,
    #[serde(default)]
    pub references: Vec<ImageReferenceRequest>,
    #[serde(default)]
    pub invariants: Vec<String>,
    #[serde(default)]
    pub exclusions: Vec<String>,
    #[serde(default)]
    pub factual_constraints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum QaStatus {
    Passed,
    Repaired,
    NotChecked,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QaIssue {
    pub category: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageQaRecord {
    pub status: QaStatus,
    #[serde(default)]
    pub issues: Vec<QaIssue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedImageReference {
    pub role: ImageReferenceRole,
    pub source_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedImageRecord {
    pub id: String,
    pub url: String,
    pub status: String,
    pub intent: VisualIntent,
    pub scene: VisualScene,
    pub visual_brief: VisualBrief,
    pub final_prompt: String,
    pub aspect_ratio: String,
    pub model_id: String,
    pub provider_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub routing_reason: String,
    #[serde(default)]
    pub references: Vec<GeneratedImageReference>,
    pub qa: ImageQaRecord,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trips_with_kebab_case_enums() {
        let brief = VisualBrief {
            intent: VisualIntent::Edit,
            scene: VisualScene::UiMockup,
            prompt: "change the header".into(),
            purpose: None,
            audience: None,
            exact_text: vec!["Dashboard".into()],
            references: vec![],
            invariants: vec!["keep layout".into()],
            exclusions: vec![],
            factual_constraints: vec![],
        };
        let value = serde_json::to_value(&brief).unwrap();
        assert_eq!(value["intent"], "edit");
        assert_eq!(value["scene"], "ui-mockup");
        assert_eq!(serde_json::from_value::<VisualBrief>(value).unwrap(), brief);
    }

    #[test]
    fn registry_exposes_only_opaque_current_turn_references() {
        let image = ImageAttachment {
            data: base64::engine::general_purpose::STANDARD
                .encode(b"\x89PNG\r\n\x1a\nregistry-test"),
            media_type: "image/png".into(),
        };
        let mut messages = vec![ChatMessage {
            role: "user".into(),
            content: "use my image".into(),
            images: vec![image.clone()],
            generated_images: vec![image],
            tool_calls: None,
            tool_call_id: None,
        }];
        let registry = build_image_reference_registry(&mut messages);
        assert!(messages[0].content.contains("imgref_0_upload_0"));
        assert!(messages[0].content.contains("imgref_0_generated_0"));
        assert!(registry.resolve("imgref_0_upload_0").is_ok());
        assert!(registry.resolve("imgref_0_generated_0").is_ok());
        assert!(registry.resolve("imgref_9_upload_0").is_err());
    }

    #[test]
    fn scientific_recipe_preserves_text_and_fact_boundaries() {
        let brief = VisualBrief {
            intent: VisualIntent::Generate,
            scene: VisualScene::Scientific,
            prompt: "show the mechanism".into(),
            purpose: Some("paper figure".into()),
            audience: None,
            exact_text: vec!["GMV Nowcast".into()],
            references: vec![],
            invariants: vec![],
            exclusions: vec!["watermark".into()],
            factual_constraints: vec!["no future information".into()],
        };
        let prompt = compile_prompt(&brief, "16:9", ResponseLanguage::Chinese);
        assert!(prompt.contains("GMV Nowcast"));
        assert!(prompt.contains("不得推断或改写"));
        assert!(prompt.contains("不伪造实验数据"));
    }

    #[test]
    fn prompt_compiler_follows_response_language() {
        let brief = VisualBrief {
            intent: VisualIntent::Generate,
            scene: VisualScene::Infographic,
            prompt: "展示 GMV 模型".into(),
            purpose: Some("技术汇报".into()),
            audience: None,
            exact_text: vec![],
            references: vec![],
            invariants: vec![],
            exclusions: vec![],
            factual_constraints: vec![],
        };
        let chinese = compile_prompt(&brief, "16:9", ResponseLanguage::Chinese);
        assert!(chinese.contains("用户主请求"));
        assert!(!chinese.contains("User request:"));

        let english = compile_prompt(&brief, "16:9", ResponseLanguage::English);
        assert!(english.contains("User request:"));
        assert!(!english.contains("用户主请求"));
    }
}
