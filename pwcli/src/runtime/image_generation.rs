use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::runtime::settings::local_config::{GenImageModelSection, GenImageSection};
use crate::runtime::visual_generation::{
    compile_prompt, GeneratedImageRecord, GeneratedImageReference, ImageQaRecord,
    ImageReferenceRequest, ImageReferenceRole, QaStatus, ResolvedImageReference, VisualBrief,
    VisualIntent, VisualScene, VisualStage,
};

const MAX_PROMPT_CHARS: usize = 20_000;
const MAX_RESPONSE_BYTES: usize = 85 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 30 * 1024 * 1024;
const QA_UPSTREAM_MAX_BASE64_BYTES: usize = 5 * 1024 * 1024;
const QA_BASE64_HEADROOM_BYTES: usize = 256 * 1024;
const QA_TARGET_BASE64_BYTES: usize = QA_UPSTREAM_MAX_BASE64_BYTES - QA_BASE64_HEADROOM_BYTES;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenerationRequest {
    #[serde(default)]
    pub intent: VisualIntent,
    #[serde(default)]
    pub scene: VisualScene,
    pub prompt: String,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub audience: Option<String>,
    #[serde(default)]
    pub exact_text: Vec<String>,
    #[serde(default)]
    pub negative_prompt: Option<String>,
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    #[serde(default)]
    pub image_refs: Vec<ImageReferenceRequest>,
    #[serde(default)]
    pub invariants: Vec<String>,
    #[serde(default)]
    pub exclusions: Vec<String>,
    #[serde(default)]
    pub factual_constraints: Vec<String>,
    #[serde(skip)]
    pub resolved_images: Vec<ResolvedImageInput>,
    pub session_id: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedImageInput {
    pub reference: ResolvedImageReference,
    pub role: ImageReferenceRole,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedImageAsset {
    pub id: String,
    pub url: String,
    pub mime: String,
    pub byte_size: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenerationResult {
    pub image_urls: Vec<String>,
    pub assets: Vec<GeneratedImageAsset>,
    pub result_text: String,
    pub model: String,
    pub provider_name: String,
    pub generated_image_record: GeneratedImageRecord,
}

pub struct ImageGenerationService {
    artifacts_root: PathBuf,
}

impl ImageGenerationService {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            artifacts_root: data_dir.join("artifacts"),
        }
    }

    pub async fn generate(
        &self,
        request: &ImageGenerationRequest,
        acting_model: Option<&crate::ai::llm::model_context::ActiveModelContext>,
    ) -> Result<ImageGenerationResult> {
        validate_request(request)?;
        let config = crate::runtime::settings::local_config::get()
            .tools
            .gen_image;
        // An explicitly configured gen-image model list always takes
        // precedence over auto-discovery.
        if config.enabled && config.models.iter().any(|model| model.enabled) {
            return self
                .generate_configured(request, &config, acting_model)
                .await;
        }
        // Auto-linkage: derive image-generation models from connected AI
        // providers, so no separate gen-image configuration is required.
        let derived = derive_provider_image_models().await;
        anyhow::ensure!(
            !derived.is_empty(),
            "没有可用的生图模型：请在设置 → 系统集成中手动配置，或连接带有生图模型（模型 id 含 image/imagine）的 Provider 并在模型目录中采纳"
        );
        let section = GenImageSection {
            enabled: true,
            default_model: config.default_model.clone(),
            models: derived,
        };
        self.generate_configured(request, &section, acting_model)
            .await
    }

    async fn generate_configured(
        &self,
        request: &ImageGenerationRequest,
        config: &GenImageSection,
        acting_model: Option<&crate::ai::llm::model_context::ActiveModelContext>,
    ) -> Result<ImageGenerationResult> {
        let response_language = crate::runtime::settings::local_config::get()
            .ai
            .response_language;
        emit_stage(VisualStage::Briefing);
        let brief = VisualBrief {
            intent: request.intent,
            scene: request.scene,
            prompt: request.prompt.clone(),
            purpose: request.purpose.clone(),
            audience: request.audience.clone(),
            exact_text: request.exact_text.clone(),
            references: request.image_refs.clone(),
            invariants: request.invariants.clone(),
            exclusions: request.exclusions.clone(),
            factual_constraints: request.factual_constraints.clone(),
        };
        emit_stage(VisualStage::ResolvingInputs);
        let aspect_ratio = normalized_aspect_ratio(request).to_string();
        emit_stage(VisualStage::CompilingPrompt);
        let final_prompt = compile_prompt(&brief, &aspect_ratio, response_language);
        let mut compiled_request = request.clone();
        compiled_request.prompt = final_prompt.clone();
        emit_stage(VisualStage::Routing);
        let selection = select_configured_model(config, request)?;
        let model = selection.model;
        emit_stage(VisualStage::Generating);
        let initial = generate_candidate(model, &compiled_request).await?;
        emit_stage(VisualStage::Validating);
        let initial_qa = run_vision_qa(
            &brief,
            &initial.bytes,
            initial.mime,
            response_language,
            acting_model,
        )
        .await;
        let (candidate, qa, final_model, routing_reason) = match initial_qa {
            Ok(Some(decision)) => {
                let qa = qa_record_from_decision(decision, false);
                (initial, qa, model, selection.reason)
            }
            Ok(None) => (
                initial,
                ImageQaRecord {
                    status: QaStatus::NotChecked,
                    issues: vec![crate::runtime::visual_generation::QaIssue {
                        category: "artifact".into(),
                        description: "未配置可用的 vision 模型，图片未自动验收".into(),
                    }],
                    repair_prompt: None,
                },
                model,
                selection.reason,
            ),
            Err(error) => (
                initial,
                ImageQaRecord {
                    status: QaStatus::Warning,
                    issues: vec![crate::runtime::visual_generation::QaIssue {
                        category: "artifact".into(),
                        description: format!("自动验收失败：{error}"),
                    }],
                    repair_prompt: None,
                },
                model,
                selection.reason,
            ),
        };
        let result_text = candidate.result_text.clone();
        let asset = persist_asset(
            &self.artifacts_root,
            &request.session_id,
            &final_model.model,
            candidate.bytes,
            candidate.mime,
            &request.resolved_images,
        )
        .await?;
        let record = GeneratedImageRecord {
            id: asset.id.clone(),
            url: asset.url.clone(),
            status: if matches!(qa.status, QaStatus::Passed | QaStatus::Repaired) {
                "ready".into()
            } else {
                "ready_with_warnings".into()
            },
            intent: request.intent,
            scene: request.scene,
            visual_brief: brief,
            final_prompt,
            aspect_ratio,
            model_id: final_model.model.clone(),
            provider_name: final_model.name.clone(),
            routing_reason,
            references: request
                .resolved_images
                .iter()
                .map(|input| GeneratedImageReference {
                    role: input.role,
                    source_id: input.reference.source_id.clone(),
                })
                .collect(),
            qa,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        write_generation_record(&self.artifacts_root, &request.session_id, &record).await?;
        emit_stage(if record.status == "ready" {
            VisualStage::Ready
        } else {
            VisualStage::ReadyWithWarnings
        });
        Ok(ImageGenerationResult {
            image_urls: vec![asset.url.clone()],
            assets: vec![asset],
            result_text,
            model: final_model.model.clone(),
            provider_name: final_model.name.clone(),
            generated_image_record: record,
        })
    }
}

struct GeneratedCandidate {
    bytes: Vec<u8>,
    mime: &'static str,
    result_text: String,
}

async fn generate_candidate(
    model: &GenImageModelSection,
    request: &ImageGenerationRequest,
) -> Result<GeneratedCandidate> {
    let upstream = call_configured_model(model, request).await?;
    let result_text = extract_text(&upstream);
    let image = if model.protocol == "gemini-generate-content" {
        extract_final_gemini_images(&upstream)
    } else {
        deep_extract_images(&upstream, 0)
    }
    .into_iter()
    .next()
    .context("image provider returned no image")?;
    let (bytes, mime) = decode_or_fetch_image(&image).await?;
    Ok(GeneratedCandidate {
        bytes,
        mime,
        result_text,
    })
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum QaVerdict {
    Pass,
    Repair,
    Warning,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QaDecision {
    verdict: QaVerdict,
    #[serde(default)]
    issues: Vec<crate::runtime::visual_generation::QaIssue>,
    #[serde(default)]
    repair_prompt: Option<String>,
}

fn qa_record_from_decision(decision: QaDecision, repaired: bool) -> ImageQaRecord {
    let status = match decision.verdict {
        QaVerdict::Pass if repaired => QaStatus::Repaired,
        QaVerdict::Pass => QaStatus::Passed,
        QaVerdict::Repair | QaVerdict::Warning => QaStatus::Warning,
    };
    ImageQaRecord {
        status,
        issues: decision.issues,
        repair_prompt: decision.repair_prompt,
    }
}

async fn run_vision_qa(
    brief: &VisualBrief,
    bytes: &[u8],
    mime: &str,
    response_language: crate::runtime::settings::local_config::ResponseLanguage,
    acting_model: Option<&crate::ai::llm::model_context::ActiveModelContext>,
) -> Result<Option<QaDecision>> {
    let Some(provider) = select_qa_provider(acting_model) else {
        return Ok(None);
    };
    let runtime = crate::runtime::settings::RuntimeConfig::load();
    let client = crate::ai::llm::LlmClient::with_provider(provider, runtime.backend_url);
    let qa_bytes = bytes.to_vec();
    let qa_mime = mime.to_string();
    let (qa_bytes, qa_mime) =
        tokio::task::spawn_blocking(move || prepare_qa_image(&qa_bytes, &qa_mime))
            .await
            .context("vision QA image transcoder stopped unexpectedly")??;
    let message = crate::ai::llm::ChatMessage {
        role: "user".into(),
        content: match response_language {
            crate::runtime::settings::local_config::ResponseLanguage::Chinese
            | crate::runtime::settings::local_config::ResponseLanguage::Auto => format!(
                "视觉需求：\n{}\n\n检查附带的生成图片，只返回 JSON。",
                serde_json::to_string(brief)?
            ),
            crate::runtime::settings::local_config::ResponseLanguage::English => format!(
                "Visual brief:\n{}\n\nInspect the attached generated image and return JSON only.",
                serde_json::to_string(brief)?
            ),
        },
        images: vec![crate::ai::llm::ImageAttachment {
            data: base64::engine::general_purpose::STANDARD.encode(qa_bytes),
            media_type: qa_mime,
        }],
        generated_images: vec![],
        tool_calls: None,
        tool_call_id: None,
    };
    let response = client
        .chat_with_sampling(
            &[message],
            Some(qa_system_prompt(response_language)),
            Some(1200),
            None,
        )
        .await?;
    parse_qa_decision(&response.content).map(Some)
}

fn prepare_qa_image(bytes: &[u8], mime: &str) -> Result<(Vec<u8>, String)> {
    prepare_qa_image_with_limit(bytes, mime, QA_TARGET_BASE64_BYTES)
}

fn prepare_qa_image_with_limit(
    bytes: &[u8],
    mime: &str,
    target_base64_bytes: usize,
) -> Result<(Vec<u8>, String)> {
    if base64_encoded_len(bytes.len()) <= target_base64_bytes {
        return Ok((bytes.to_vec(), mime.to_string()));
    }

    use image::codecs::jpeg::JpegEncoder;

    let source = image::load_from_memory(bytes).context("vision QA image could not be decoded")?;
    let attempts = [
        (4096, 90),
        (4096, 80),
        (3072, 85),
        (2560, 82),
        (2048, 80),
        (1536, 75),
        (1280, 70),
        (1024, 65),
    ];
    for (max_dimension, quality) in attempts {
        let image = if source.width() > max_dimension || source.height() > max_dimension {
            source.resize(
                max_dimension,
                max_dimension,
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            source.clone()
        }
        .into_rgb8();
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, quality).encode(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )?;
        if base64_encoded_len(encoded.len()) <= target_base64_bytes {
            return Ok((encoded, "image/jpeg".into()));
        }
    }

    anyhow::bail!("vision QA proxy image remains above the provider's 5MB limit")
}

fn base64_encoded_len(byte_len: usize) -> usize {
    byte_len.div_ceil(3).saturating_mul(4)
}

fn qa_system_prompt(
    language: crate::runtime::settings::local_config::ResponseLanguage,
) -> &'static str {
    match language {
        crate::runtime::settings::local_config::ResponseLanguage::Chinese
        | crate::runtime::settings::local_config::ResponseLanguage::Auto => QA_SYSTEM_PROMPT_ZH,
        crate::runtime::settings::local_config::ResponseLanguage::English => QA_SYSTEM_PROMPT_EN,
    }
}

const QA_SYSTEM_PROMPT_ZH: &str = r#"你是严格的视觉验收系统。将图片与提供的 VisualBrief 对比，检查内容、精确文字、构图、参考图漂移、科学完整性、视觉质量和生成缺陷。不得推断 Brief 中不存在的事实。只返回一个 JSON 对象：{"verdict":"pass|repair|warning","issues":[{"category":"content|text|composition|reference-drift|scientific|visual-quality|artifact","description":"中文问题说明"}],"repairPrompt":"可选的简短中文定点修复要求"}。仅当问题可通过一次图片编辑或重新生成修复时使用 repair；非阻塞的不确定性使用 warning。所有 description 和 repairPrompt 必须使用简体中文。"#;

const QA_SYSTEM_PROMPT_EN: &str = r#"You are a strict visual QA system. Compare the image against the supplied VisualBrief. Check content, exact text, composition, reference drift, scientific integrity, visual quality, and artifacts. Never infer facts absent from the brief. Return exactly one JSON object: {"verdict":"pass|repair|warning","issues":[{"category":"content|text|composition|reference-drift|scientific|visual-quality|artifact","description":"..."}],"repairPrompt":"optional concise targeted fix"}. Use repair only for a concrete issue that one image edit or regeneration can fix. Use warning for non-blocking uncertainty. Write every description and repairPrompt in English."#;

fn parse_qa_decision(content: &str) -> Result<QaDecision> {
    let trimmed = content.trim();
    let json = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .strip_suffix("```")
        .unwrap_or(trimmed)
        .trim();
    let decision: QaDecision =
        serde_json::from_str(json).context("vision QA returned invalid JSON")?;
    const CATEGORIES: &[&str] = &[
        "content",
        "text",
        "composition",
        "reference-drift",
        "scientific",
        "visual-quality",
        "artifact",
    ];
    anyhow::ensure!(
        decision
            .issues
            .iter()
            .all(|issue| CATEGORIES.contains(&issue.category.as_str())
                && !issue.description.trim().is_empty()),
        "vision QA returned an invalid issue"
    );
    Ok(decision)
}

fn select_qa_provider(
    acting_model: Option<&crate::ai::llm::model_context::ActiveModelContext>,
) -> Option<crate::ai::config::ProviderConfig> {
    let runtime = crate::runtime::settings::RuntimeConfig::load();
    let providers = runtime.providers.unwrap_or_default();
    let local = crate::runtime::settings::local_config::get();
    if let Some(model_ref) = local
        .ai
        .moa
        .as_ref()
        .and_then(|config| config.presets.get(&config.active_preset))
        .and_then(|preset| preset.image_describer.as_ref())
    {
        if let Some(provider) =
            crate::runtime::decision::registry::resolve_model_ref(&providers, model_ref)
        {
            if crate::runtime::decision::registry::is_vision_model(&providers, &provider.model) {
                return Some(provider);
            }
        }
    }
    if let Some(acting_model) = acting_model.filter(|model| model.supports_vision) {
        if !acting_model.model_id.is_empty() {
            let model_ref = crate::runtime::decision::config::MoaModelRef {
                provider: acting_model.provider_id.clone().unwrap_or_default(),
                model: acting_model.model_id.clone(),
            };
            if let Some(provider) =
                crate::runtime::decision::registry::resolve_model_ref(&providers, &model_ref)
            {
                return Some(provider);
            }
        }
    }
    providers.iter().find_map(|provider| {
        provider
            .models
            .iter()
            .find(|model| {
                model.enabled != Some(false)
                    && model
                        .capabilities
                        .as_ref()
                        .and_then(|capabilities| capabilities.vision)
                        == Some(true)
            })
            .map(|model| {
                let mut selected = provider.clone();
                selected.model = model.id.clone();
                selected
            })
    })
}

fn emit_stage(stage: VisualStage) {
    crate::runtime::tools::progress::emit(&format!("🎨 {}", stage.label()));
}

fn validate_request(request: &ImageGenerationRequest) -> Result<()> {
    let prompt = request.prompt.trim();
    anyhow::ensure!(!prompt.is_empty(), "prompt is required");
    anyhow::ensure!(
        prompt.chars().count() <= MAX_PROMPT_CHARS,
        "prompt exceeds 20000 characters"
    );
    validate_owner_id(&request.session_id)?;
    if matches!(
        request.intent,
        VisualIntent::Reference | VisualIntent::Edit | VisualIntent::Variant
    ) {
        anyhow::ensure!(
            !request.resolved_images.is_empty(),
            "intent {:?} requires at least one valid image reference",
            request.intent
        );
    }
    Ok(())
}

fn validate_owner_id(value: &str) -> Result<()> {
    anyhow::ensure!(!value.is_empty() && value.len() <= 128, "invalid sessionId");
    anyhow::ensure!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "invalid sessionId"
    );
    Ok(())
}

pub(crate) fn is_image_model(model: &Value) -> bool {
    if model
        .get("capabilities")
        .and_then(|value| value.get("image"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return true;
    }
    let id = model.get("id").and_then(Value::as_str).unwrap_or_default();
    crate::ai::config::is_image_generation_model_id(id)
}

fn entry_is_image_model(model: &crate::ai::config::ModelEntry) -> bool {
    model
        .capabilities
        .as_ref()
        .and_then(|capabilities| capabilities.image)
        == Some(true)
        || crate::ai::config::is_image_generation_model_id(&model.id)
}

/// Auto-linkage between image generation and connected AI providers: any
/// provider that carries enabled image-generation models and resolvable
/// credentials becomes immediately usable, with no separate gen-image
/// configuration required.  Only OpenAI-compatible transports expose the
/// `/images/generations` endpoint this pipeline speaks.
async fn derive_provider_image_models() -> Vec<GenImageModelSection> {
    let runtime = crate::runtime::settings::RuntimeConfig::load();
    let providers = runtime.providers.unwrap_or_default();
    let auth = crate::ai::provider::AuthManager::default();
    let mut sections = Vec::new();
    for provider in providers {
        if !provider.protocol.starts_with("openai") {
            continue;
        }
        let image_models: Vec<&crate::ai::config::ModelEntry> = provider
            .models
            .iter()
            .filter(|model| model.enabled != Some(false) && entry_is_image_model(model))
            .collect();
        if image_models.is_empty() {
            continue;
        }
        let Ok(resolved) = auth.resolve(&provider).await else {
            continue;
        };
        if resolved.api_key.trim().is_empty() {
            continue;
        }
        let kind = crate::ai::provider::provider_kind(&provider);
        let url = format!(
            "{}/images/generations",
            provider.base_url.trim_end_matches('/')
        );
        for model in image_models {
            sections.push(GenImageModelSection {
                id: format!("{}:{}", provider.name, model.id),
                name: provider.name.clone(),
                enabled: true,
                protocol: if kind == crate::ai::provider::ProviderKind::QwenTokenPlanCn {
                    "qwen-openai-images".to_string()
                } else {
                    "openai-images".to_string()
                },
                url: url.clone(),
                api_key: resolved.api_key.clone(),
                model: model.id.clone(),
                ..Default::default()
            });
        }
    }
    sections
}

struct ModelSelection<'a> {
    model: &'a GenImageModelSection,
    reason: String,
}

fn select_configured_model<'a>(
    config: &'a GenImageSection,
    request: &ImageGenerationRequest,
) -> Result<ModelSelection<'a>> {
    let default = if config.default_model.trim().is_empty() {
        config.models.iter().find(|model| model.enabled)
    } else {
        config
            .models
            .iter()
            .find(|model| model.id == config.default_model || model.model == config.default_model)
    }
    .context("未配置可用的默认 gen-image 模型；请在设置 → 系统集成中完成配置")?;
    let (model, reason) = if supports_request(default, request.intent) {
        (
            default,
            format!(
                "默认模型 {} 支持 {:?}，优先使用",
                default.id, request.intent
            ),
        )
    } else {
        let fallback = config
            .models
            .iter()
            .filter(|model| model.enabled && supports_request(model, request.intent))
            .max_by_key(|model| {
                model
                    .capabilities
                    .preferred_scenes
                    .iter()
                    .any(|scene| scene == request.scene.as_str())
            })
            .with_context(|| {
                format!(
                    "默认 gen-image 模型 {} 不支持 {:?}，且没有其他已启用的能力匹配模型",
                    default.id, request.intent
                )
            })?;
        (
            fallback,
            format!(
                "默认模型 {} 不支持 {:?}，路由到能力匹配模型 {}",
                default.id, request.intent, fallback.id
            ),
        )
    };
    anyhow::ensure!(model.enabled, "gen-image 模型 {} 未启用", model.id);
    anyhow::ensure!(
        matches!(
            model.protocol.as_str(),
            "openai-images" | "qwen-openai-images" | "gemini-generate-content"
        ),
        "gen-image 模型 {} 使用了不支持的协议 {}",
        model.id,
        model.protocol
    );
    anyhow::ensure!(
        !model.url.trim().is_empty(),
        "gen-image 模型 {} 缺少 URL",
        model.id
    );
    anyhow::ensure!(
        !model.api_key.trim().is_empty(),
        "gen-image 模型 {} 缺少 API Key",
        model.id
    );
    anyhow::ensure!(
        !model.model.trim().is_empty(),
        "gen-image 模型 {} 缺少上游模型名",
        model.id
    );
    Ok(ModelSelection { model, reason })
}

fn supports_request(model: &GenImageModelSection, intent: VisualIntent) -> bool {
    let standard_openai_edit = !model.edit_url.trim().is_empty()
        || model
            .url
            .trim_end_matches('/')
            .ends_with("/images/generations");
    let (generate_default, reference_default, edit_default) = match model.protocol.as_str() {
        "gemini-generate-content" => (true, true, true),
        "openai-images" => (true, standard_openai_edit, standard_openai_edit),
        "qwen-openai-images" => (true, false, false),
        _ => (false, false, false),
    };
    match intent {
        VisualIntent::Generate => model.capabilities.generate.unwrap_or(generate_default),
        VisualIntent::Reference | VisualIntent::Variant => {
            model.capabilities.reference.unwrap_or(reference_default)
        }
        VisualIntent::Edit => model.capabilities.edit.unwrap_or(edit_default),
    }
}

fn normalized_aspect_ratio(request: &ImageGenerationRequest) -> &'static str {
    const RATIOS: &[(&str, f64)] = &[
        ("1:1", 1.0),
        ("2:3", 2.0 / 3.0),
        ("3:2", 3.0 / 2.0),
        ("3:4", 3.0 / 4.0),
        ("4:3", 4.0 / 3.0),
        ("4:5", 4.0 / 5.0),
        ("5:4", 5.0 / 4.0),
        ("9:16", 9.0 / 16.0),
        ("16:9", 16.0 / 9.0),
        ("21:9", 21.0 / 9.0),
    ];
    if let Some(requested) = request.aspect_ratio.as_deref() {
        if let Some((name, _)) = RATIOS.iter().find(|(name, _)| *name == requested) {
            return name;
        }
    }
    let target = 1.0;
    RATIOS
        .iter()
        .min_by(|(_, left), (_, right)| {
            (*left / target)
                .ln()
                .abs()
                .total_cmp(&(*right / target).ln().abs())
        })
        .map(|(name, _)| *name)
        .unwrap_or("1:1")
}

fn qwen_size(request: &ImageGenerationRequest) -> String {
    let recommended = match normalized_aspect_ratio(request) {
        "1:1" => Some("1536*1536"),
        "2:3" => Some("1024*1536"),
        "3:2" => Some("1536*1024"),
        "3:4" => Some("1080*1440"),
        "4:3" => Some("1440*1080"),
        "9:16" => Some("1080*1920"),
        "16:9" => Some("1920*1080"),
        "21:9" => Some("2048*872"),
        _ => None,
    };
    if let Some(size) = recommended {
        return size.to_string();
    }
    let (ratio_width, ratio_height) = normalized_aspect_ratio(request)
        .split_once(':')
        .and_then(|(width, height)| Some((width.parse::<f64>().ok()?, height.parse::<f64>().ok()?)))
        .unwrap_or((1.0, 1.0));
    let (width, height) = if ratio_width >= ratio_height {
        (
            2048,
            ((2048.0 * ratio_height / ratio_width) / 32.0).round() as u32 * 32,
        )
    } else {
        (
            ((2048.0 * ratio_width / ratio_height) / 32.0).round() as u32 * 32,
            2048,
        )
    };
    format!("{width}*{height}")
}

async fn call_configured_model(
    model: &GenImageModelSection,
    request: &ImageGenerationRequest,
) -> Result<Value> {
    if !request.resolved_images.is_empty()
        && matches!(
            model.protocol.as_str(),
            "openai-images" | "qwen-openai-images"
        )
    {
        return call_openai_edit(model, request).await;
    }
    let (url, body, auth) = match model.protocol.as_str() {
        "openai-images" => (
            model.url.replace("{model}", &model.model),
            openai_images_body(model, request, false),
            AuthStyle::Bearer,
        ),
        "qwen-openai-images" => (
            model.url.replace("{model}", &model.model),
            openai_images_body(model, request, true),
            AuthStyle::Bearer,
        ),
        "gemini-generate-content" => (
            model.url.replace("{model}", &model.model),
            configured_gemini_body(request),
            AuthStyle::GoogleApiKey,
        ),
        protocol => anyhow::bail!("unsupported gen-image protocol {protocol}"),
    };
    fetch_json(url, &model.api_key, body, auth).await
}

fn openai_images_body(
    model: &GenImageModelSection,
    request: &ImageGenerationRequest,
    qwen_size_format: bool,
) -> Value {
    let size = if qwen_size_format {
        qwen_size(request)
    } else {
        openai_size(request).into()
    };
    json!({
        "model": model.model,
        "prompt": if qwen_size_format { "" } else { request.prompt.as_str() },
        "n": 1,
        "size": if qwen_size_format { size } else { size.replace('*', "x") },
        "extendParams": {
            "prompt_extend": true,
            "watermark": false,
            "messages": [{ "role": "user", "content": [{ "text": request.prompt }] }]
        }
    })
}

fn openai_size(request: &ImageGenerationRequest) -> &'static str {
    let ratio = normalized_aspect_ratio(request);
    let (width, height) = ratio
        .split_once(':')
        .and_then(|(width, height)| Some((width.parse::<u32>().ok()?, height.parse::<u32>().ok()?)))
        .unwrap_or((1, 1));
    match width.cmp(&height) {
        std::cmp::Ordering::Greater => "1536x1024",
        std::cmp::Ordering::Less => "1024x1536",
        std::cmp::Ordering::Equal => "1024x1024",
    }
}

fn configured_gemini_body(request: &ImageGenerationRequest) -> Value {
    let mut parts = vec![json!({ "text": request.prompt })];
    parts.extend(request.resolved_images.iter().map(|input| {
        json!({
            "inlineData": {
                "mimeType": input.reference.media_type,
                "data": base64::engine::general_purpose::STANDARD.encode(&input.reference.bytes)
            }
        })
    }));
    json!({
        "contents": [{
            "role": "user",
            "parts": parts
        }],
        "generationConfig": {
            "responseModalities": ["TEXT", "IMAGE"],
            "imageConfig": {
                "aspectRatio": normalized_aspect_ratio(request),
                "imageSize": "4K"
            }
        }
    })
}

async fn call_openai_edit(
    model: &GenImageModelSection,
    request: &ImageGenerationRequest,
) -> Result<Value> {
    let edit_url = if !model.edit_url.trim().is_empty() {
        model.edit_url.replace("{model}", &model.model)
    } else if model.url.ends_with("/generations") {
        format!("{}/edits", model.url.trim_end_matches("/generations"))
    } else {
        anyhow::bail!(
            "gen-image 模型 {} 需要显式 editUrl 才能处理参考图或编辑",
            model.id
        );
    };
    let mut form = reqwest::multipart::Form::new()
        .text("model", model.model.clone())
        .text("prompt", request.prompt.clone())
        .text("n", "1")
        .text("size", qwen_size(request).replace('*', "x"));
    for (index, input) in request.resolved_images.iter().enumerate() {
        let extension = extension_for_mime(&input.reference.media_type);
        let part = reqwest::multipart::Part::bytes(input.reference.bytes.clone())
            .file_name(format!("image-{index}.{extension}"))
            .mime_str(&input.reference.media_type)?;
        form = form.part(if index == 0 { "image" } else { "image[]" }, part);
    }
    fetch_multipart_json(edit_url, &model.api_key, form).await
}

#[derive(Clone, Copy)]
enum AuthStyle {
    Bearer,
    GoogleApiKey,
}

async fn fetch_json(url: String, api_key: &str, body: Value, auth: AuthStyle) -> Result<Value> {
    let request = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()?
        .post(url)
        .json(&body);
    let response = match auth {
        AuthStyle::Bearer => request.bearer_auth(api_key),
        AuthStyle::GoogleApiKey => request
            .bearer_auth(api_key)
            .header("x-goog-api-key", api_key),
    }
    .send()
    .await?;
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        anyhow::bail!("image provider response exceeds 85MB");
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        anyhow::ensure!(
            bytes.len().saturating_add(chunk.len()) <= MAX_RESPONSE_BYTES,
            "image provider response exceeds 85MB"
        );
        bytes.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(bytes).context("image provider returned non-UTF-8 JSON")?;
    if !status.is_success() {
        anyhow::bail!("upstream {status}: {}", truncate(&text, 2000));
    }
    serde_json::from_str(&text).context("image provider returned invalid JSON")
}

async fn fetch_multipart_json(
    url: String,
    api_key: &str,
    form: reqwest::multipart::Form,
) -> Result<Value> {
    let response = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()?
        .post(url)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await?;
    parse_json_response(response).await
}

async fn parse_json_response(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        anyhow::bail!("image provider response exceeds 85MB");
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        anyhow::ensure!(
            bytes.len().saturating_add(chunk.len()) <= MAX_RESPONSE_BYTES,
            "image provider response exceeds 85MB"
        );
        bytes.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(bytes).context("image provider returned non-UTF-8 JSON")?;
    if !status.is_success() {
        anyhow::bail!("upstream {status}: {}", truncate(&text, 2000));
    }
    serde_json::from_str(&text).context("image provider returned invalid JSON")
}

async fn decode_or_fetch_image(source: &str) -> Result<(Vec<u8>, &'static str)> {
    let (bytes, declared_mime) = if let Some(data) = source.strip_prefix("data:") {
        let (metadata, encoded) = data.split_once(',').context("invalid image data URL")?;
        anyhow::ensure!(
            metadata.ends_with(";base64"),
            "image data URL must be base64"
        );
        let mime = metadata.trim_end_matches(";base64");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .context("invalid image base64")?;
        (bytes, Some(mime))
    } else if source.starts_with("https://") {
        let image = crate::runtime::media::fetch_remote_image(source)
            .await
            .map_err(|cause| anyhow::anyhow!(cause.message))?;
        return Ok((image.bytes, image.mime));
    } else {
        anyhow::bail!("image provider returned an unsupported URL")
    };
    let detected = crate::runtime::media::detect_raster_mime(&bytes)
        .context("generated image has invalid magic bytes")?;
    if let Some(declared) = declared_mime {
        anyhow::ensure!(
            declared == detected || (declared == "image/jpg" && detected == "image/jpeg"),
            "generated image MIME does not match its content"
        );
    }
    let dimensions = raster_dimensions(&bytes)?;
    if bytes.len() <= MAX_IMAGE_BYTES && dimensions.0 <= 4096 && dimensions.1 <= 4096 {
        return Ok((bytes, detected));
    }
    let encoded = tokio::task::spawn_blocking(move || transcode_lossless_webp(&bytes, None))
        .await
        .context("generated image transcoder stopped unexpectedly")??;
    anyhow::ensure!(
        encoded.len() <= MAX_IMAGE_BYTES,
        "generated image remains above 30MB after lossless WebP transcoding"
    );
    Ok((encoded, "image/webp"))
}

fn raster_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    use image::{ImageDecoder, ImageReader};

    let reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .context("generated image format is not decodable")?;
    Ok(reader.into_decoder()?.dimensions())
}

fn transcode_lossless_webp(bytes: &[u8], export_dimensions: Option<(u32, u32)>) -> Result<Vec<u8>> {
    use image::{codecs::webp::WebPEncoder, ImageDecoder, ImageEncoder, ImageReader};

    let reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .context("generated image format is not decodable")?;
    let decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    let mut image = image::DynamicImage::from_decoder(decoder)
        .context("generated image pixels could not be decoded")?;
    let (export_width, export_height) = export_dimensions.unwrap_or((4096, 4096));
    if width > export_width || height > export_height {
        image = image.resize(
            export_width,
            export_height,
            image::imageops::FilterType::Lanczos3,
        );
    }
    let image = image.into_rgba8();
    let width = image.width();
    let height = image.height();
    let mut encoded = Vec::new();
    WebPEncoder::new_lossless(&mut encoded).write_image(
        image.as_raw(),
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(encoded)
}

async fn persist_asset(
    artifacts_root: &Path,
    session_id: &str,
    model_id: &str,
    bytes: Vec<u8>,
    mime: &'static str,
    inputs: &[ResolvedImageInput],
) -> Result<GeneratedImageAsset> {
    let id = uuid::Uuid::now_v7().to_string();
    let session_dir = artifacts_root.join("images").join(session_id);
    let staging = session_dir.join(format!(".staging-{id}"));
    let destination = session_dir.join(&id);
    tokio::fs::create_dir_all(&staging).await?;
    set_directory_permissions(&session_dir).await?;
    set_directory_permissions(&staging).await?;
    let extension = extension_for_mime(mime);
    let image_path = staging.join(format!("image.{extension}"));
    tokio::fs::write(&image_path, &bytes).await?;
    set_file_permissions(&image_path).await?;
    if !inputs.is_empty() {
        let inputs_dir = staging.join("inputs");
        tokio::fs::create_dir_all(&inputs_dir).await?;
        set_directory_permissions(&inputs_dir).await?;
        for (index, input) in inputs.iter().enumerate() {
            let extension = extension_for_mime(&input.reference.media_type);
            let path = inputs_dir.join(format!("{index}.{extension}"));
            tokio::fs::write(&path, &input.reference.bytes).await?;
            set_file_permissions(&path).await?;
        }
    }
    let manifest = json!({
        "schemaVersion": 1,
        "id": id,
        "sessionId": session_id,
        "modelId": model_id,
        "mime": mime,
        "byteSize": bytes.len(),
        "inputs": inputs.iter().map(|input| json!({
            "referenceId": input.reference.id,
            "sourceId": input.reference.source_id,
            "role": input.role,
        })).collect::<Vec<_>>(),
        "createdAt": chrono::Utc::now().to_rfc3339(),
    });
    let manifest_path = staging.join("manifest.json");
    tokio::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?).await?;
    set_file_permissions(&manifest_path).await?;
    tokio::fs::rename(&staging, &destination).await?;
    Ok(GeneratedImageAsset {
        id: id.clone(),
        url: format!("/api/image-artifacts/{session_id}/{id}"),
        mime: mime.to_string(),
        byte_size: bytes.len(),
    })
}

/// Persist a raster produced by a trusted runtime renderer (for example the
/// isolated matplotlib adapter) in the same artifact namespace as model-made
/// images, so Agent, CLI and Web all render it identically.
pub async fn persist_external_asset(
    data_dir: &Path,
    session_id: &str,
    renderer_id: &str,
    bytes: Vec<u8>,
    mime: &'static str,
) -> Result<GeneratedImageAsset> {
    validate_owner_id(session_id)?;
    persist_asset(
        &data_dir.join("artifacts"),
        session_id,
        renderer_id,
        bytes,
        mime,
        &[],
    )
    .await
}

async fn write_generation_record(
    artifacts_root: &Path,
    session_id: &str,
    record: &GeneratedImageRecord,
) -> Result<()> {
    let directory = artifacts_root
        .join("images")
        .join(session_id)
        .join(&record.id);
    let manifest_path = directory.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&tokio::fs::read(&manifest_path).await?)?;
    manifest["generationRecord"] = serde_json::to_value(record)?;
    let staging = directory.join(".manifest.json.tmp");
    tokio::fs::write(&staging, serde_json::to_vec_pretty(&manifest)?).await?;
    set_file_permissions(&staging).await?;
    tokio::fs::rename(staging, manifest_path).await?;
    Ok(())
}

#[cfg(unix)]
async fn set_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await?;
    Ok(())
}

#[cfg(not(unix))]
async fn set_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
async fn set_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    Ok(())
}

#[cfg(not(unix))]
async fn set_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

pub fn artifact_path(data_dir: &Path, session_id: &str, artifact_id: &str) -> Result<PathBuf> {
    validate_owner_id(session_id)?;
    validate_owner_id(artifact_id)?;
    let directory = data_dir
        .join("artifacts")
        .join("images")
        .join(session_id)
        .join(artifact_id);
    for extension in ["png", "jpg", "gif", "webp"] {
        let candidate = directory.join(format!("image.{extension}"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    anyhow::bail!("image artifact not found")
}

pub async fn delete_session_artifacts(data_dir: &Path, session_id: &str) -> Result<()> {
    validate_owner_id(session_id)?;
    let directory = data_dir.join("artifacts").join("images").join(session_id);
    match tokio::fs::remove_dir_all(directory).await {
        Ok(()) => Ok(()),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(cause) => Err(cause.into()),
    }
}

fn extension_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "png",
    }
}

fn deep_extract_images(value: &Value, depth: usize) -> Vec<String> {
    if depth > 10 {
        return Vec::new();
    }
    match value {
        Value::String(value)
            if value.starts_with("data:image") || value.starts_with("https://") =>
        {
            vec![value.clone()]
        }
        Value::String(value)
            if value.len() > 100
                && value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "+/=".contains(character)
                }) =>
        {
            vec![format!("data:image/png;base64,{value}")]
        }
        Value::Array(values) => values
            .iter()
            .flat_map(|value| deep_extract_images(value, depth + 1))
            .collect(),
        Value::Object(values) => values
            .iter()
            .filter(|(key, _)| key.as_str() != "text")
            .flat_map(|(_, value)| deep_extract_images(value, depth + 1))
            .collect(),
        _ => Vec::new(),
    }
}

fn extract_final_gemini_images(value: &Value) -> Vec<String> {
    value
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|part| part.get("thought").and_then(Value::as_bool) != Some(true))
        .filter_map(|part| {
            let inline = part.get("inlineData").or_else(|| part.get("inline_data"))?;
            let data = inline.get("data")?.as_str()?;
            let mime = inline
                .get("mimeType")
                .or_else(|| inline.get("mime_type"))
                .and_then(Value::as_str)
                .unwrap_or("image/png");
            Some(format!("data:{mime};base64,{data}"))
        })
        .collect()
}

fn extract_text(value: &Value) -> String {
    let gemini = value
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<String>();
    if !gemini.trim().is_empty() {
        return gemini.trim().to_string();
    }
    let text = value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/output/text").and_then(Value::as_str))
        .unwrap_or("图像生成完成")
        .trim()
        .to_string();
    if text.is_empty() {
        "图像生成完成".to_string()
    } else {
        text
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_request() {
        let request = ImageGenerationRequest {
            prompt: "scientific figure".into(),
            negative_prompt: None,
            aspect_ratio: None,
            session_id: "session_1".into(),
            ..Default::default()
        };
        assert!(validate_request(&request).is_ok());
        let invalid = ImageGenerationRequest {
            prompt: String::new(),
            ..request
        };
        assert!(validate_request(&invalid).is_err());
    }

    #[test]
    fn recognizes_case_insensitive_qwen_alias_and_explicit_image_capability() {
        assert!(is_image_model(
            &json!({ "id": "Qwen-image-3.0-DogFooding" })
        ));
        assert!(is_image_model(
            &json!({ "id": "custom-renderer", "capabilities": { "image": true } })
        ));
    }

    #[test]
    fn selects_only_the_configured_default_model() {
        let model = GenImageModelSection {
            id: "qwen".into(),
            name: "Qwen".into(),
            enabled: true,
            protocol: "qwen-openai-images".into(),
            url: "https://configured.example/images/generations".into(),
            api_key: "secret".into(),
            model: "qwen-image-2.0-pro".into(),
            ..Default::default()
        };
        let config = GenImageSection {
            enabled: true,
            default_model: "qwen".into(),
            models: vec![model],
        };
        let request = ImageGenerationRequest::default();
        assert_eq!(
            select_configured_model(&config, &request).unwrap().model.id,
            "qwen"
        );
        let missing = GenImageSection {
            default_model: "missing".into(),
            ..config
        };
        assert!(select_configured_model(&missing, &request).is_err());
    }

    #[test]
    fn qwen_openai_images_uses_extended_messages_and_recommended_sizes() {
        let provider = GenImageModelSection {
            id: "qwen".into(),
            name: "Qwen".into(),
            enabled: true,
            protocol: "qwen-openai-images".into(),
            url: "https://configured.example/images/generations".into(),
            api_key: "secret".into(),
            model: "qwen-image-2.0-pro".into(),
            ..Default::default()
        };
        let request = ImageGenerationRequest {
            prompt: "scientific cell".into(),
            negative_prompt: Some("blur".into()),
            aspect_ratio: Some("16:9".into()),
            session_id: "session_1".into(),
            ..Default::default()
        };
        let body = openai_images_body(&provider, &request, true);
        assert_eq!(body["model"], "qwen-image-2.0-pro");
        assert_eq!(
            body["extendParams"]["messages"][0]["content"][0]["text"],
            "scientific cell"
        );
        assert_eq!(body["prompt"], "");
        assert_eq!(body["size"], "1920*1080");
        assert_eq!(
            openai_images_body(&provider, &request, false)["size"],
            "1536x1024"
        );
        assert_eq!(
            openai_images_body(&provider, &request, false)["prompt"],
            "scientific cell"
        );
        for (ratio, expected) in [
            ("1:1", "1536*1536"),
            ("2:3", "1024*1536"),
            ("3:2", "1536*1024"),
            ("3:4", "1080*1440"),
            ("4:3", "1440*1080"),
            ("9:16", "1080*1920"),
            ("16:9", "1920*1080"),
            ("21:9", "2048*872"),
        ] {
            let request = ImageGenerationRequest {
                aspect_ratio: Some(ratio.into()),
                ..Default::default()
            };
            assert_eq!(
                openai_images_body(&provider, &request, true)["size"],
                expected
            );
        }
    }

    #[test]
    fn configured_gemini_uses_4k_and_skips_thought_images() {
        let request = ImageGenerationRequest {
            prompt: "research figure".into(),
            negative_prompt: None,
            aspect_ratio: Some("21:9".into()),
            session_id: "session_1".into(),
            ..Default::default()
        };
        let body = configured_gemini_body(&request);
        assert_eq!(
            body["generationConfig"]["imageConfig"]["aspectRatio"],
            "21:9"
        );
        assert_eq!(body["generationConfig"]["imageConfig"]["imageSize"], "4K");
        let response = json!({
            "candidates": [{ "content": { "parts": [
                { "thought": true, "inlineData": { "mimeType": "image/png", "data": "thinking" } },
                { "inlineData": { "mimeType": "image/png", "data": "final" } }
            ] } }]
        });
        assert_eq!(
            extract_final_gemini_images(&response),
            vec!["data:image/png;base64,final"]
        );
    }

    #[test]
    fn routes_default_first_then_falls_back_for_edit_capability() {
        let default = GenImageModelSection {
            id: "plain".into(),
            enabled: true,
            protocol: "qwen-openai-images".into(),
            url: "https://example.test/generations".into(),
            api_key: "secret".into(),
            model: "plain-model".into(),
            ..Default::default()
        };
        let editor = GenImageModelSection {
            id: "editor".into(),
            enabled: true,
            protocol: "gemini-generate-content".into(),
            url: "https://example.test/{model}".into(),
            api_key: "secret".into(),
            model: "editor-model".into(),
            ..Default::default()
        };
        let config = GenImageSection {
            enabled: true,
            default_model: "plain".into(),
            models: vec![default, editor],
        };
        let request = ImageGenerationRequest {
            intent: VisualIntent::Edit,
            ..Default::default()
        };
        assert_eq!(
            select_configured_model(&config, &request).unwrap().model.id,
            "editor"
        );
    }

    #[test]
    fn gemini_encodes_resolved_image_inputs() {
        let input = ResolvedImageInput {
            reference: ResolvedImageReference {
                id: "imgref_0_upload_0".into(),
                source_id: "message:0:upload:0".into(),
                media_type: "image/png".into(),
                bytes: b"pixels".to_vec(),
            },
            role: ImageReferenceRole::Content,
        };
        let request = ImageGenerationRequest {
            prompt: "keep the product".into(),
            session_id: "session_1".into(),
            resolved_images: vec![input],
            ..Default::default()
        };
        let gemini = configured_gemini_body(&request);
        assert_eq!(
            gemini["contents"][0]["parts"][1]["inlineData"]["mimeType"],
            "image/png"
        );
    }

    #[test]
    fn qa_json_is_strict_and_repair_is_only_reported_as_a_warning() {
        let repair = parse_qa_decision(
            r#"```json
{"verdict":"repair","issues":[{"category":"text","description":"标题乱码"}],"repairPrompt":"只修复标题"}
```"#,
        )
        .unwrap();
        assert_eq!(repair.verdict, QaVerdict::Repair);
        assert_eq!(
            qa_record_from_decision(repair, false).status,
            QaStatus::Warning
        );
        assert!(parse_qa_decision(
            r#"{"verdict":"warning","issues":[{"category":"unknown","description":"x"}]}"#
        )
        .is_err());
    }

    #[test]
    fn oversized_qa_image_is_transcoded_below_the_payload_limit() {
        use image::{codecs::png::PngEncoder, ImageEncoder};

        let width = 256;
        let height = 256;
        let mut state = 1_u32;
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..width * height {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            pixels.extend_from_slice(&[
                (state >> 24) as u8,
                (state >> 16) as u8,
                (state >> 8) as u8,
                255,
            ]);
        }
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
            .unwrap();

        let limit = 256 * 1024;
        assert!(base64_encoded_len(png.len()) > limit);
        let (proxy, mime) = prepare_qa_image_with_limit(&png, "image/png", limit).unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(base64_encoded_len(proxy.len()) <= limit);
    }

    #[test]
    fn lossless_webp_transcode_preserves_dimensions_and_valid_magic() {
        use image::{codecs::png::PngEncoder, ImageEncoder};

        let pixels = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&pixels, 2, 2, image::ExtendedColorType::Rgba8)
            .unwrap();

        let webp = transcode_lossless_webp(&png, None).unwrap();
        assert_eq!(
            crate::runtime::media::detect_raster_mime(&webp),
            Some("image/webp")
        );
        let decoded = image::load_from_memory(&webp).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));

        let downscaled = transcode_lossless_webp(&png, Some((1, 1))).unwrap();
        let decoded = image::load_from_memory(&downscaled).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1, 1));
    }

    #[test]
    fn oversized_generated_image_is_downscaled_to_4k_without_changing_aspect_ratio() {
        use image::{codecs::png::PngEncoder, ImageEncoder};

        let width = 4100;
        let height = 10;
        let pixels = vec![255; width as usize * height as usize * 4];
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
            .unwrap();

        let webp = transcode_lossless_webp(&png, None).unwrap();
        let decoded = image::load_from_memory(&webp).unwrap();
        assert_eq!(decoded.width(), 4096);
        assert_eq!(decoded.height(), 10);
    }

    #[test]
    fn extracts_images_without_text_urls() {
        let value = json!({
            "text": "https://not-an-image.example",
            "data": [{"url": "https://cdn.example/figure.png"}]
        });
        assert_eq!(
            deep_extract_images(&value, 0),
            vec!["https://cdn.example/figure.png"]
        );
    }

    #[tokio::test]
    async fn deletes_only_the_requested_session_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("artifacts/images/session_a/asset");
        let second = temp.path().join("artifacts/images/session_b/asset");
        tokio::fs::create_dir_all(&first).await.unwrap();
        tokio::fs::create_dir_all(&second).await.unwrap();

        delete_session_artifacts(temp.path(), "session_a")
            .await
            .unwrap();

        assert!(!first.exists());
        assert!(second.exists());
        assert!(delete_session_artifacts(temp.path(), "../escape")
            .await
            .is_err());
    }
}
