use anyhow::{Context, Result};
use base64::Engine;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::Value;

use crate::ai::llm::{ChatMessage, ImageAttachment};
use crate::runtime::image_generation::{
    ImageGenerationRequest, ImageGenerationService, ResolvedImageInput,
};
use crate::runtime::tools::context::ToolExecutionContext;
use crate::runtime::tools::progress;
use crate::runtime::visual_generation::{VisualIntent, VisualScene};

use super::*;

const PLANNER_PROMPT: &str = r#"You are the Planner in a publication-quality scientific illustration team. Convert the supplied scientific content and visual intent into an exact visual specification. Preserve every fact. For plots enumerate every series and map variables to x/y/color/marker channels. For diagrams define a single reading direction, semantic grouping, exact labels, arrows, colors, spacing, and exclusions. Reference images are style/layout examples only; never copy their facts. Do not include a figure title. Return only the detailed specification."#;
const STYLIST_PROMPT: &str = r#"You are the Stylist for a top-tier scientific publication. Improve only visual hierarchy, alignment, whitespace, restrained color, typography, line weights, legends, markers, and accessibility. Never change, omit, or invent semantics or numeric values. Return the complete revised visual specification only."#;
const STYLE_GUIDE: &str = include_str!("../../../resources/illustration/style-guide.md");
const CODE_PROMPT: &str = r#"Generate robust Python matplotlib code for the supplied plot specification. The canonical JSON input is already available as the global variable DATA; do not open files or embed substitute data. Use only matplotlib, numpy, math, statistics, json, or csv. Create at least one figure, render every supplied value faithfully, use the Agg-compatible API, avoid network/files/subprocesses, and return Python code only."#;
const CRITIC_PROMPT: &str = r#"You are a strict scientific-figure critic. Compare the image with the detailed description, original content, and visual intent. Accuracy and readability outrank aesthetics. Return JSON only: {"critic_suggestions":"No changes needed. or precise issues","revised_description":"No changes needed. or a complete corrected description"}. Never change numeric values or invent facts."#;
const EVAL_PROMPT: &str = r#"Evaluate the supplied scientific figure. Return JSON only with faithfulness, conciseness, readability, aesthetics (0-10 with rationale), redLines, and overall. Faithfulness and readability form Tier 1; use conciseness and aesthetics only as Tier 2."#;

pub struct IllustrationService {
    data_dir: std::path::PathBuf,
    references: ReferenceStore,
    runs: IllustrationRunStore,
}

impl IllustrationService {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self {
            data_dir: data_dir.to_owned(),
            references: ReferenceStore::new(data_dir),
            runs: IllustrationRunStore::new(data_dir),
        }
    }

    pub fn reference_store(&self) -> &ReferenceStore {
        &self.references
    }

    pub async fn execute(
        &self,
        context: &ToolExecutionContext,
        request: IllustrationRequest,
    ) -> Result<IllustrationRun> {
        let model = context
            .illustration_model
            .as_ref()
            .context("当前 Turn 没有可用的文本/代码模型")?;
        anyhow::ensure!(
            !request.visual_intent.trim().is_empty(),
            "visualIntent is required"
        );
        let resolved_mode = resolve_mode(&request);
        let session_id = context
            .chat_session_id
            .as_ref()
            .or(context.session_id.as_ref())
            .map(ToString::to_string)
            .unwrap_or_else(|| "default".into());
        let now = chrono::Utc::now().to_rfc3339();
        let mut run = IllustrationRun {
            schema_version: 1,
            id: uuid::Uuid::now_v7().to_string(),
            session_id: session_id.clone(),
            request: request.clone(),
            resolved_mode,
            status: IllustrationRunStatus::Running,
            stage: "routing".into(),
            model_id: model.model_id().into(),
            retrieved_reference_ids: vec![],
            candidates: vec![],
            selected_candidate_id: None,
            warnings: vec![],
            created_at: now.clone(),
            updated_at: now,
        };
        self.runs.save(&run).await?;
        if context.cancellation.is_cancelled() {
            return self.cancel(run).await;
        }

        if resolved_mode == IllustrationMode::Eval {
            return self.evaluate(context, run).await;
        }

        run.stage = "retrieving".into();
        progress::emit("🖼️ 正在检索内置与个人绘图参考…");
        let references = if request.retrieval == RetrievalMode::None {
            vec![]
        } else {
            self.references
                .retrieve(
                    resolved_mode,
                    &format!(
                        "{} {}",
                        request.visual_intent,
                        content_text(&request.content)
                    ),
                    10,
                )
                .unwrap_or_else(|error| {
                    run.warnings
                        .push(format!("参考库不可用，已降级为无检索模式：{error}"));
                    vec![]
                })
        };
        run.retrieved_reference_ids = references.iter().map(|r| r.entry.id.clone()).collect();
        if !model.supports_vision() && !references.is_empty() {
            run.warnings
                .push("当前模型不支持视觉输入；Planner 仅使用参考库的文字标签。".into());
        }
        self.runs.save(&run).await?;

        let base_run = std::sync::Arc::new(run.clone());
        let references = std::sync::Arc::new(references);
        let candidate_count = request.quality.candidates();
        let mut pending = futures::stream::iter(0..candidate_count)
            .map(|candidate_index| {
                let base_run = std::sync::Arc::clone(&base_run);
                let references = std::sync::Arc::clone(&references);
                async move {
                    progress::emit(&format!(
                        "🧭 正在规划候选 {}/{}…",
                        candidate_index + 1,
                        candidate_count
                    ));
                    (
                        candidate_index,
                        self.build_candidate(
                            context,
                            base_run.as_ref(),
                            references.as_slice(),
                            candidate_index,
                        )
                        .await,
                    )
                }
            })
            .buffer_unordered(candidate_count.min(4));
        while let Some((candidate_index, candidate)) = pending.next().await {
            if context.cancellation.is_cancelled() {
                return self.cancel(run).await;
            }
            run.stage = format!("candidate_{}_completed", candidate_index + 1);
            run.candidates
                .push(candidate.unwrap_or_else(|error| IllustrationCandidate {
                    id: format!("candidate-{}", candidate_index + 1),
                    revisions: vec![],
                    best_revision: None,
                    error: Some(error.to_string()),
                }));
            run.updated_at = chrono::Utc::now().to_rfc3339();
            self.runs.save(&run).await?;
        }
        run.candidates.sort_by(|left, right| left.id.cmp(&right.id));

        let selected = run
            .candidates
            .iter()
            .find(|c| c.best_revision.is_some())
            .map(|c| c.id.clone());
        run.selected_candidate_id = selected;
        run.stage = "completed".into();
        run.status = if run.selected_candidate_id.is_none() {
            IllustrationRunStatus::Failed
        } else if run.warnings.is_empty() {
            IllustrationRunStatus::Completed
        } else {
            IllustrationRunStatus::CompletedWithWarnings
        };
        run.updated_at = chrono::Utc::now().to_rfc3339();
        self.runs.save(&run).await?;
        if run.selected_candidate_id.is_none() {
            anyhow::bail!("all illustration candidates failed; run_id={}", run.id);
        }
        Ok(run)
    }

    async fn build_candidate(
        &self,
        context: &ToolExecutionContext,
        run: &IllustrationRun,
        references: &[RetrievedReference],
        candidate_index: usize,
    ) -> Result<IllustrationCandidate> {
        let model = context
            .illustration_model
            .as_ref()
            .context("illustration model missing")?;
        let mut planner_images = explicit_images(context, &run.request, 4 * 1024 * 1024)?;
        let used = planner_images
            .iter()
            .map(|image| image.data.len() * 3 / 4)
            .sum::<usize>();
        planner_images.extend(reference_images(
            references,
            (4 * 1024 * 1024usize).saturating_sub(used),
        ));
        let mut messages = vec![ChatMessage {
            role: "user".into(),
            content: planner_input(&run.request, references, candidate_index),
            images: if model.supports_vision() {
                planner_images
            } else {
                vec![]
            },
            generated_images: vec![],
            tool_calls: None,
            tool_call_id: None,
        }];
        let planned = model
            .complete(
                &messages,
                PLANNER_PROMPT,
                6000,
                Some(0.35 + candidate_index as f32 * 0.08),
            )
            .await?;
        messages = vec![plain_message(format!(
            "Style guide:\n{}\n\nOriginal content:\n{}\nVisual intent:\n{}\nPlanner specification:\n{}",
            STYLE_GUIDE,
            content_text(&run.request.content),
            run.request.visual_intent,
            planned
        ))];
        let mut description = model
            .complete(&messages, STYLIST_PROMPT, 6000, Some(0.2))
            .await?;
        let mut candidate = IllustrationCandidate {
            id: format!("candidate-{}", candidate_index + 1),
            revisions: vec![],
            best_revision: None,
            error: None,
        };

        for round in 0..=run.request.quality.critic_rounds() {
            let rendered = self.render(context, run, &description).await;
            match rendered {
                Ok((artifact_id, artifact_url, plot_code)) => {
                    let revision_index = candidate.revisions.len();
                    candidate.revisions.push(IllustrationRevision {
                        round,
                        description: description.clone(),
                        critic_suggestions: None,
                        artifact_id: Some(artifact_id.clone()),
                        artifact_url: Some(artifact_url.clone()),
                        plot_code,
                    });
                    candidate.best_revision = Some(revision_index);
                    if round == run.request.quality.critic_rounds() {
                        break;
                    }
                    if !model.supports_vision() {
                        break;
                    }
                    let image_path = crate::runtime::image_generation::artifact_path(
                        &self.data_dir,
                        &run.session_id,
                        &artifact_id,
                    )?;
                    let critique = critique(
                        model.as_ref(),
                        &run.request,
                        &description,
                        Some(&std::fs::read(image_path)?),
                        None,
                    )
                    .await?;
                    candidate.revisions[revision_index].critic_suggestions =
                        Some(critique.critic_suggestions.clone());
                    if is_no_change(&critique.critic_suggestions)
                        || is_no_change(&critique.revised_description)
                    {
                        break;
                    }
                    description = critique.revised_description;
                }
                Err(error) => {
                    if round == run.request.quality.critic_rounds() {
                        if candidate.best_revision.is_none() {
                            return Err(error);
                        }
                        break;
                    }
                    let critique = critique(
                        model.as_ref(),
                        &run.request,
                        &description,
                        None,
                        Some(&error.to_string()),
                    )
                    .await?;
                    if is_no_change(&critique.revised_description) {
                        if candidate.best_revision.is_none() {
                            return Err(error);
                        }
                        break;
                    }
                    description = critique.revised_description;
                }
            }
        }
        Ok(candidate)
    }

    async fn render(
        &self,
        context: &ToolExecutionContext,
        run: &IllustrationRun,
        description: &str,
    ) -> Result<(String, String, Option<String>)> {
        match run.resolved_mode {
            IllustrationMode::Plot => {
                progress::emit("📊 正在生成并隔离执行 matplotlib 程序…");
                let model = context
                    .illustration_model
                    .as_ref()
                    .context("illustration model missing")?;
                let code = model
                    .complete(
                        &[plain_message(format!(
                            "Raw DATA:\n{}\nVisual specification:\n{}",
                            content_text(&run.request.content),
                            description
                        ))],
                        CODE_PROMPT,
                        7000,
                        Some(0.1),
                    )
                    .await?;
                let rendered =
                    super::plot::render(&code, &run.request.content, &context.cancellation).await?;
                let asset = crate::runtime::image_generation::persist_external_asset(
                    &self.data_dir,
                    &run.session_id,
                    "matplotlib",
                    rendered.bytes,
                    "image/png",
                )
                .await?;
                progress::emit_image(&asset.url, &run.request.visual_intent);
                Ok((asset.id, asset.url, Some(rendered.code)))
            }
            IllustrationMode::Diagram | IllustrationMode::Polish | IllustrationMode::Refine => {
                progress::emit("🎨 正在通过现有生图配置渲染学术插图…");
                let resolved_images = resolve_images(context, &run.request)?;
                let intent = match run.resolved_mode {
                    IllustrationMode::Diagram => VisualIntent::Generate,
                    IllustrationMode::Refine | IllustrationMode::Polish => VisualIntent::Edit,
                    _ => VisualIntent::Generate,
                };
                let image_request = ImageGenerationRequest {
                    intent, scene: VisualScene::Scientific, prompt: description.into(), purpose: Some("publication-quality scientific illustration".into()), audience: Some("research paper readers".into()),
                    exact_text: run.request.constraints.exact_text.clone(), negative_prompt: Some("figure title, caption inside image, watermark, fabricated values, illegible text".into()),
                    aspect_ratio: run.request.aspect_ratio.clone(), image_refs: run.request.image_refs.clone(),
                    invariants: run.request.constraints.invariants.clone(), exclusions: vec!["figure caption inside the image".into(), "watermark".into()],
                    factual_constraints: run.request.constraints.factual_constraints.clone(), resolved_images, session_id: run.session_id.clone(),
                };
                let result = ImageGenerationService::new(&self.data_dir)
                    .generate(&image_request, context.active_model.as_ref())
                    .await?;
                let asset = result
                    .assets
                    .first()
                    .context("image generation returned no artifact")?;
                progress::emit_generated_image(
                    &asset.url,
                    &run.request.visual_intent,
                    &result.generated_image_record,
                );
                Ok((asset.id.clone(), asset.url.clone(), None))
            }
            _ => anyhow::bail!("mode {:?} cannot render", run.resolved_mode),
        }
    }

    async fn evaluate(
        &self,
        context: &ToolExecutionContext,
        mut run: IllustrationRun,
    ) -> Result<IllustrationRun> {
        let model = context
            .illustration_model
            .as_ref()
            .context("illustration model missing")?;
        let resolved = resolve_images(context, &run.request)?;
        let image = resolved
            .first()
            .context("eval requires at least one image reference")?;
        let message = ChatMessage {
            role: "user".into(),
            content: format!(
                "Original content:\n{}\nVisual intent:\n{}",
                content_text(&run.request.content),
                run.request.visual_intent
            ),
            images: vec![ImageAttachment {
                data: base64::engine::general_purpose::STANDARD.encode(&image.reference.bytes),
                media_type: image.reference.media_type.clone(),
            }],
            generated_images: vec![],
            tool_calls: None,
            tool_call_id: None,
        };
        let evaluation = model
            .complete_vision(&[message], EVAL_PROMPT, 3000)
            .await?
            .context("eval requires a configured vision model")?;
        run.candidates.push(IllustrationCandidate {
            id: "evaluation".into(),
            revisions: vec![IllustrationRevision {
                round: 0,
                description: evaluation,
                critic_suggestions: None,
                artifact_id: None,
                artifact_url: None,
                plot_code: None,
            }],
            best_revision: Some(0),
            error: None,
        });
        run.selected_candidate_id = Some("evaluation".into());
        run.stage = "completed".into();
        run.status = IllustrationRunStatus::Completed;
        run.updated_at = chrono::Utc::now().to_rfc3339();
        self.runs.save(&run).await?;
        Ok(run)
    }

    async fn cancel(&self, mut run: IllustrationRun) -> Result<IllustrationRun> {
        run.stage = "cancelled".into();
        run.status = IllustrationRunStatus::Cancelled;
        run.updated_at = chrono::Utc::now().to_rfc3339();
        self.runs.save(&run).await?;
        Ok(run)
    }
}

fn content_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string_pretty(value).unwrap_or_default())
}
fn plain_message(content: String) -> ChatMessage {
    ChatMessage {
        role: "user".into(),
        content,
        images: vec![],
        generated_images: vec![],
        tool_calls: None,
        tool_call_id: None,
    }
}
fn planner_input(
    request: &IllustrationRequest,
    references: &[RetrievedReference],
    variant: usize,
) -> String {
    let refs = references
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "{}. layout={} intent={} summary={}",
                i + 1,
                r.entry.layout,
                r.entry.visual_intent,
                r.entry.content_summary
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("Candidate variant: {}\nOriginal content:\n{}\nVisual intent:\n{}\nConstraints:\n{}\nRetrieved style/layout references:\n{}", variant+1, content_text(&request.content), request.visual_intent, serde_json::to_string(&request.constraints).unwrap_or_default(), refs)
}
fn reference_images(references: &[RetrievedReference], budget: usize) -> Vec<ImageAttachment> {
    let mut used = 0;
    references
        .iter()
        .filter_map(|r| {
            if used + r.bytes.len() > budget {
                return None;
            }
            used += r.bytes.len();
            Some(ImageAttachment {
                data: base64::engine::general_purpose::STANDARD.encode(&r.bytes),
                media_type: r.entry.media_type.clone(),
            })
        })
        .collect()
}
fn explicit_images(
    context: &ToolExecutionContext,
    request: &IllustrationRequest,
    budget: usize,
) -> Result<Vec<ImageAttachment>> {
    let resolved = resolve_images(context, request)?;
    let mut used = 0;
    let mut images = Vec::new();
    for input in resolved {
        anyhow::ensure!(
            used + input.reference.bytes.len() <= budget,
            "explicit image references exceed the 4 MiB Planner budget"
        );
        used += input.reference.bytes.len();
        images.push(ImageAttachment {
            data: base64::engine::general_purpose::STANDARD.encode(input.reference.bytes),
            media_type: input.reference.media_type,
        });
    }
    Ok(images)
}
fn resolve_images(
    context: &ToolExecutionContext,
    request: &IllustrationRequest,
) -> Result<Vec<ResolvedImageInput>> {
    if request.image_refs.is_empty() {
        return Ok(vec![]);
    }
    let registry = context
        .image_references
        .as_ref()
        .context("当前任务没有可用的图片引用")?;
    request
        .image_refs
        .iter()
        .map(|reference| {
            Ok(ResolvedImageInput {
                reference: registry.resolve(&reference.id)?,
                role: reference.role,
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct Critique {
    critic_suggestions: String,
    revised_description: String,
}
async fn critique(
    model: &dyn IllustrationModelPort,
    request: &IllustrationRequest,
    description: &str,
    image: Option<&[u8]>,
    failure: Option<&str>,
) -> Result<Critique> {
    let mut message=plain_message(format!("Detailed description:\n{}\nOriginal content:\n{}\nVisual intent:\n{}{}",description,content_text(&request.content),request.visual_intent,failure.map(|v|format!("\n[SYSTEM NOTICE] Rendering failed: {v}. Simplify the description and make the program robust.")).unwrap_or_default()));
    if let Some(bytes) = image {
        message.images.push(ImageAttachment {
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
            media_type: "image/png".into(),
        });
    }
    let response = if image.is_some() {
        model
            .complete_vision(&[message], CRITIC_PROMPT, 5000)
            .await?
            .context("vision critic unavailable")?
    } else {
        model
            .complete(&[message], CRITIC_PROMPT, 5000, Some(0.1))
            .await?
    };
    parse_json(&response).context("critic returned invalid JSON")
}
fn parse_json<T: for<'de> Deserialize<'de>>(value: &str) -> Result<T> {
    let trimmed = value.trim();
    let json = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .strip_suffix("```")
        .unwrap_or(trimmed)
        .trim();
    Ok(serde_json::from_str(json)?)
}
fn is_no_change(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("No changes needed.")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auto_routes_structured_data_to_plot() {
        let request = IllustrationRequest {
            mode: IllustrationMode::Auto,
            content: serde_json::json!([{"x":1}]),
            visual_intent: "trend".into(),
            image_refs: vec![],
            quality: Default::default(),
            retrieval: Default::default(),
            aspect_ratio: None,
            constraints: Default::default(),
        };
        assert_eq!(resolve_mode(&request), IllustrationMode::Plot);
    }
}
