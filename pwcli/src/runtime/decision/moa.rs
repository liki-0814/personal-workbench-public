use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::future::join_all;
use tokio::sync::Mutex;

use crate::agent_core::decision::{
    AdvisorResult, DecisionOption, DecisionOutcome, DecisionRequest, DecisionReviewer,
    DecisionTrigger, DecisionVerdict,
};
use crate::ai::config::ProviderConfig;
use crate::ai::llm::{ChatMessage, LlmClient, TokenUsage};
use crate::runtime::decision::config::{MoaConfig, MoaPreset};
use crate::runtime::decision::registry::{is_vision_model, resolve_model_ref};

const IMAGE_SYSTEM: &str = "Describe the supplied image accurately for other models. Transcribe visible text, code, numbers, charts, formulas, and UI state. Do not infer user intent.";
const DECISION_ADVISOR_SYSTEM: &str = "You are an independent advisor at an Agent Harness decision checkpoint. Review only the supplied decision, proposal, evidence, and relevant transcript. Treat explicit tool and skill documentation in that evidence or transcript as authoritative; do not invent undocumented incompatibilities. Identify concrete risks, contradictions, and a complete recommended course of action. Explanatory bridges are factual claims too: words such as therefore, because, expected, more fundamental, more robust, truly determines, or a claimed reason for an epistemic qualifier require evidence or an explicit bounded analysis label. At FinalReview, recommend revise for an error that materially changes the decision, an unsupported quantitative or qualitative factual claim, an unlabeled explanatory inference that changes interpretation, a conclusion that contradicts the document's own evidence boundary, corrupted replacement characters or mojibake, a missing required deliverable, or a fatal structure/layout issue. A placeholder, `待补`, evidence-gap warning, or qualitative direction is still a missing required deliverable when the original user explicitly requested the exact number, comparison, decomposition, or answer. Recommend GATHER_EVIDENCE unless the transcript proves the most direct authoritative structured source was actually attempted and inaccessible or the user accepted reduced scope; a failed PDF text extraction alone is not proof that a downloadable table or data file is unavailable. Equivalent terminology already defined by the evidence, minor citation wording, and optional polish are non-blocking warnings and should proceed. In round 2, reconsider your recommendation using the anonymous disagreements and candidate plans from round 1, then either revise it or explicitly defend it. The first non-empty line must be exactly `RECOMMENDATION: PROCEED`, `RECOMMENDATION: REVISE`, `RECOMMENDATION: GATHER_EVIDENCE`, or `RECOMMENDATION: ESCALATE`. Follow it with a concise rationale and operational instruction for the judge. Do not call tools, do not address the user, and do not reveal hidden chain-of-thought.";
const DECISION_JUDGE_SYSTEM: &str = "You are the judge at an Agent Harness decision checkpoint. Synthesize the independent advisor summaries into one operational verdict for the acting agent. Treat explicit tool and skill documentation in the decision evidence or transcript as authoritative over advisor speculation. At a PreAction checkpoint, `proceed` means execute the currently proposed tools; `gather_evidence` means reject the current tools and return to the agent to propose different evidence gathering. Never use `gather_evidence` merely because the current proposed tool itself gathers evidence: when that tool is the recommended safe next action, the outcome must be `proceed`. Explanatory bridges are factual claims too: words such as therefore, because, expected, more fundamental, more robust, truly determines, or a claimed reason for an epistemic qualifier require evidence or an explicit bounded analysis label. At FinalReview, use revise or gather_evidence for an error that materially changes the decision, an unsupported quantitative or qualitative factual claim, an unlabeled explanatory inference that changes interpretation, a conclusion that contradicts the document's own evidence boundary, corrupted replacement characters or mojibake, a missing required deliverable, or a fatal structure/layout issue. A placeholder, `待补`, evidence-gap warning, or qualitative direction does not satisfy an explicitly requested exact number, comparison, decomposition, or answer. Use GATHER_EVIDENCE unless the evidence proves the most direct authoritative structured source was attempted and inaccessible or the user accepted reduced scope; failed PDF text extraction alone is insufficient. Proceed with a warning for equivalent terminology already defined by the evidence, minor citation wording, or optional polish; do not spend another agent turn on editorial-only changes after a valid document has rendered. The consensus score must estimate agreement among advisors, not merely your confidence. When material disagreement remains, extract 2-4 mutually exclusive, complete end-to-end candidate plans in options. Return JSON only with keys: outcome (proceed|revise|gather_evidence|escalate), confidence (0..1), consensus (0..1), rationale (brief common ground and disagreements), instruction (brief), options (array of {id,label,description}). Never call tools and never include hidden chain-of-thought.";
const DECISION_JUDGE_REPAIR_SYSTEM: &str = "Act as the replacement judge for an Agent Harness decision whose primary judge response was invalid. Evaluate the complete judgeInput and advisor evidence yourself; invalidResponse and parseError are diagnostics, not the subject being judged. Return exactly one JSON object and no prose or markdown. Required keys: outcome (proceed|revise|gather_evidence|escalate), confidence (0..1), consensus (0..1), rationale (brief string), instruction (brief string), options (array of {id,label,description}; use [] when there is no material disagreement).";
const MAX_CONSENSUS_ROUNDS: u8 = 2;
const REQUIRED_CONSENSUS: f32 = 0.66;
const DECISION_PROVIDER_TIMEOUT: Duration = Duration::from_secs(180);
const DECISION_PROVIDER_ROUTE_TIMEOUT: Duration = Duration::from_secs(180);
const FINAL_REVIEW_SOURCE_EVIDENCE_CHARS: usize = 32000;
const PREACTION_SOURCE_EVIDENCE_CHARS: usize = 16000;

fn is_source_evidence_tool(name: &str) -> bool {
    matches!(
        name,
        "search_file_content"
            | "web_query"
            | "web_read"
            | "download_web_file"
            | "read"
            | "grep"
            | "find"
            | "read_pdf"
            | "read_artifact"
            | "bash"
            | "inspect_document_evidence"
    )
}

fn source_evidence_transcript(messages: &[ChatMessage]) -> String {
    let mut evidence_calls = HashMap::new();
    for message in messages {
        if let Some(calls) = &message.tool_calls {
            for call in calls {
                if is_source_evidence_tool(&call.function.name) {
                    evidence_calls.insert(call.id.as_str(), call.function.name.as_str());
                }
            }
        }
    }
    let mut evidence = String::new();
    for message in messages {
        let Some(tool_call_id) = message.tool_call_id.as_deref() else {
            continue;
        };
        let Some(tool_name) = evidence_calls.get(tool_call_id) else {
            continue;
        };
        evidence.push_str(&format!(
            "tool {tool_name}: {}\n",
            truncate_chars(&message.content, 6000)
        ));
    }
    // FinalReview must see the structured evidence that actually supports the
    // document, not only the first web result. Data-heavy runs commonly have a
    // news release first and authoritative XLSX/CSV extraction later. A small
    // head-only cap silently dropped those later tool results and caused the
    // reviewers to request evidence that the agent had already collected.
    truncate_chars(&evidence, 48000)
}

fn is_document_review_tool(name: &str) -> bool {
    matches!(
        name,
        "create_document"
            | "inspect_document"
            | "inspect_document_fragments"
            | "inspect_document_evidence"
            | "record_document_evidence"
            | "patch_document"
            | "attach_document_asset"
            | "inspect_document_layout"
            | "render_document"
    )
}

fn document_review_transcript(messages: &[ChatMessage]) -> String {
    let mut document_calls = HashMap::new();
    let mut entries = Vec::new();
    for message in messages {
        if let Some(calls) = &message.tool_calls {
            for call in calls {
                if is_document_review_tool(&call.function.name) {
                    document_calls.insert(call.id.clone(), call.function.name.clone());
                    entries.push(format!(
                        "call {}: {}\n",
                        call.function.name,
                        truncate_chars(&call.function.arguments, 12_000)
                    ));
                }
            }
        }
        let Some(tool_call_id) = message.tool_call_id.as_deref() else {
            continue;
        };
        let Some(tool_name) = document_calls.get(tool_call_id) else {
            continue;
        };
        entries.push(format!(
            "result {tool_name}: {}\n",
            truncate_chars(&message.content, 16_000)
        ));
    }

    let mut selected = Vec::new();
    let mut chars = 0usize;
    for entry in entries.into_iter().rev() {
        let remaining = 24_000usize.saturating_sub(chars);
        if remaining == 0 {
            break;
        }
        let entry = truncate_chars(&entry, remaining);
        chars += entry.chars().count();
        selected.push(entry);
    }
    selected.reverse();
    selected.concat()
}

fn original_task_contract(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .find(|message| message.role == "user")
        .map(|message| truncate_chars(&message.content, 6000).to_string())
        .unwrap_or_default()
}

fn localized_decision_system(base: &str) -> String {
    let language_rule = match crate::runtime::settings::local_config::get().ai.response_language {
        crate::runtime::settings::local_config::ResponseLanguage::Chinese
        | crate::runtime::settings::local_config::ResponseLanguage::Auto => {
            "Write every recommendation, rationale, and instruction in Simplified Chinese. Preserve code, identifiers, and enum values exactly."
        }
        crate::runtime::settings::local_config::ResponseLanguage::English => {
            "Write every recommendation, rationale, and instruction in English. Preserve code, identifiers, and enum values exactly."
        }
    };
    format!("{base} {language_rule}")
}

fn advisor_recommendation(summary: &str) -> Option<DecisionOutcome> {
    let marker = summary.lines().find(|line| !line.trim().is_empty())?.trim();
    match marker {
        "RECOMMENDATION: PROCEED" => Some(DecisionOutcome::Proceed),
        "RECOMMENDATION: REVISE" => Some(DecisionOutcome::Revise),
        "RECOMMENDATION: GATHER_EVIDENCE" => Some(DecisionOutcome::GatherEvidence),
        "RECOMMENDATION: ESCALATE" => Some(DecisionOutcome::Escalate),
        _ => None,
    }
}

fn fallback_verdict_from_advisors(
    advisors: &[AdvisorResult],
    judge_failure: &str,
) -> Option<DecisionVerdict> {
    let successful = advisors
        .iter()
        .filter(|advisor| advisor.succeeded)
        .collect::<Vec<_>>();
    if successful.is_empty() {
        return None;
    }
    let recommendations = successful
        .iter()
        .map(|advisor| advisor_recommendation(&advisor.summary))
        .collect::<Option<Vec<_>>>()?;
    let outcome = if recommendations.contains(&DecisionOutcome::Escalate) {
        DecisionOutcome::Escalate
    } else if recommendations.contains(&DecisionOutcome::GatherEvidence) {
        DecisionOutcome::GatherEvidence
    } else if recommendations.contains(&DecisionOutcome::Revise) {
        DecisionOutcome::Revise
    } else {
        DecisionOutcome::Proceed
    };
    let agreeing = recommendations
        .iter()
        .filter(|recommendation| **recommendation == outcome)
        .count();
    let consensus = agreeing as f32 / recommendations.len() as f32;
    let instruction = successful
        .iter()
        .map(|advisor| {
            advisor
                .summary
                .lines()
                .skip_while(|line| line.trim().is_empty())
                .skip(1)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|summary| !summary.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(DecisionVerdict {
        outcome,
        confidence: if consensus == 1.0 { 0.75 } else { 0.6 },
        consensus,
        rationale: format!(
            "Judge unavailable ({judge_failure}); applied conservative structured advisor fallback. {agreeing}/{} advisors explicitly recommended {outcome:?}.",
            recommendations.len()
        ),
        instruction: truncate_chars(&instruction, 1600),
        options: if consensus == 1.0 {
            Vec::new()
        } else {
            disagreement_options(advisors)
        },
        rounds: 1,
        advisors: Vec::new(),
        usage: TokenUsage::default(),
    })
}

#[derive(Clone)]
pub struct MoaRuntime {
    pub name: String,
    pub preset: MoaPreset,
    pub providers: Vec<ProviderConfig>,
    pub backend_url: String,
    grounding_cache: Arc<Mutex<HashMap<u64, String>>>,
    usage: Arc<Mutex<TokenUsage>>,
}

impl MoaRuntime {
    fn provider_for(
        &self,
        model: &crate::runtime::decision::config::MoaModelRef,
    ) -> Result<ProviderConfig> {
        resolve_model_ref(&self.providers, model).with_context(|| {
            format!(
                "MoA '{}' model not found: {}:{}",
                self.name, model.provider, model.model
            )
        })
    }

    async fn add_usage(&self, usage: Option<TokenUsage>) {
        if let Some(value) = usage {
            let mut total = self.usage.lock().await;
            total.prompt_tokens += value.prompt_tokens;
            total.completion_tokens += value.completion_tokens;
            total.total_tokens += value.total_tokens;
        }
    }

    fn configured_fallback_providers(&self, primary: &ProviderConfig) -> Vec<ProviderConfig> {
        let mut candidates = vec![primary.clone()];
        for model_ref in self
            .preset
            .advisors
            .iter()
            .chain(std::iter::once(&self.preset.judge))
        {
            if let Ok(provider) = self.provider_for(model_ref) {
                if !candidates.iter().any(|candidate| {
                    candidate.name == provider.name && candidate.model == provider.model
                }) {
                    candidates.push(provider);
                }
            }
        }
        candidates
    }

    pub async fn take_usage(&self) -> TokenUsage {
        std::mem::take(&mut *self.usage.lock().await)
    }

    async fn ground_images(&self, messages: &[ChatMessage]) -> Result<String> {
        let images = messages
            .iter()
            .flat_map(|m| m.images.iter().cloned())
            .collect::<Vec<_>>();
        if images.is_empty() {
            return Ok(String::new());
        }
        let mut hasher = DefaultHasher::new();
        for image in &images {
            image.media_type.hash(&mut hasher);
            image.data.hash(&mut hasher);
        }
        let cache_key = hasher.finish();
        if let Some(cached) = self.grounding_cache.lock().await.get(&cache_key).cloned() {
            return Ok(cached);
        }
        let model_ref = self
            .preset
            .image_describer
            .clone()
            .filter(|r| !r.model.is_empty())
            .or_else(|| {
                is_vision_model(&self.providers, &self.preset.judge.model)
                    .then(|| self.preset.judge.clone())
            })
            .or_else(|| {
                self.preset
                    .advisors
                    .iter()
                    .find(|r| is_vision_model(&self.providers, &r.model))
                    .cloned()
            })
            .or_else(|| {
                self.providers.iter().find_map(|provider| {
                    provider
                        .models
                        .iter()
                        .find(|model| is_vision_model(&self.providers, &model.id))
                        .map(|model| crate::runtime::decision::config::MoaModelRef {
                            provider: provider.name.clone(),
                            model: model.id.clone(),
                        })
                })
            });
        let Some(model_ref) = model_ref else {
            return Ok(String::new());
        };
        let provider = self.provider_for(&model_ref)?;
        let client = LlmClient::with_provider(provider.clone(), self.backend_url.clone());
        let prompt = ChatMessage {
            role: "user".into(),
            content: "Describe these images.".into(),
            images,
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        };
        let response =
            Box::pin(client.chat_with_sampling(&[prompt], Some(IMAGE_SYSTEM), Some(2000), None))
                .await
                .with_context(|| {
                    format!(
                        "MoA image describer failed (preset='{}', provider='{}', model='{}'; change it in 设置 → AI 模型 → 图片描述模型)",
                        self.name, model_ref.provider, model_ref.model
                    )
                })?;
        self.add_usage(response.usage).await;
        self.grounding_cache
            .lock()
            .await
            .insert(cache_key, response.content.clone());
        Ok(response.content)
    }

    fn decision_prompt(
        request: &DecisionRequest,
        messages: &[ChatMessage],
        grounding: &str,
    ) -> String {
        let final_review =
            request.trigger == crate::agent_core::decision::DecisionTrigger::FinalReview;
        let recent_limit = if final_review { 6 } else { 12 };
        let recent_chars = if final_review { 1000 } else { 2400 };
        let mut transcript = String::new();
        for message in messages
            .iter()
            .rev()
            .take(recent_limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            if message.role != "user" && message.role != "assistant" && message.role != "tool" {
                continue;
            }
            let content = truncate_chars(&message.content, recent_chars);
            transcript.push_str(&format!("{}: {}\n", message.role, content));
        }
        let tools = request
            .tool_calls
            .iter()
            .map(|call| {
                serde_json::json!({
                    "name": call.function.name,
                    "arguments": serde_json::from_str::<serde_json::Value>(&call.function.arguments)
                        .unwrap_or_else(|_| serde_json::Value::String(call.function.arguments.clone()))
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "decisionId": request.id,
            "trigger": request.trigger,
            "risk": request.risk,
            "question": request.question,
            "proposal": request.proposal,
            "evidence": request.evidence,
            "pendingTools": tools,
            "imageGrounding": grounding,
            "originalTaskContract": truncate_chars(&original_task_contract(messages), if final_review { 2500 } else { 6000 }),
            "sourceEvidence": truncate_chars(
                &source_evidence_transcript(messages),
                if final_review {
                    FINAL_REVIEW_SOURCE_EVIDENCE_CHARS
                } else {
                    PREACTION_SOURCE_EVIDENCE_CHARS
                },
            ),
            "documentEvidence": if final_review { document_review_transcript(messages) } else { String::new() },
            "recentTranscript": transcript,
        })
        .to_string()
    }

    async fn review_decision(
        &self,
        request: &DecisionRequest,
        messages: &[ChatMessage],
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<DecisionVerdict> {
        *self.usage.lock().await = TokenUsage::default();
        if !self.preset.enabled {
            return Ok(DecisionVerdict {
                outcome: DecisionOutcome::Proceed,
                confidence: 1.0,
                consensus: 1.0,
                rationale: "MoA preset is disabled".into(),
                instruction: String::new(),
                options: Vec::new(),
                rounds: 1,
                advisors: Vec::new(),
                usage: TokenUsage::default(),
            });
        }

        let grounding = tokio::select! {
            _ = cancel.cancelled() => anyhow::bail!("decision review cancelled"),
            result = self.ground_images(messages) => result?,
        };
        let prompt = Self::decision_prompt(request, messages, &grounding);
        if self.preset.advisors.is_empty() {
            anyhow::bail!("MoA preset has no advisor models");
        }
        let advisor_limit =
            if request.trigger == crate::agent_core::decision::DecisionTrigger::FinalReview {
                2
            } else {
                match request.risk {
                    crate::agent_core::decision::DecisionRisk::Low => {
                        self.preset.low_risk_advisors.unwrap_or(1)
                    }
                    crate::agent_core::decision::DecisionRisk::Elevated => {
                        self.preset.elevated_risk_advisors.unwrap_or(2)
                    }
                    crate::agent_core::decision::DecisionRisk::High => self
                        .preset
                        .high_risk_advisors
                        .unwrap_or(self.preset.advisors.len()),
                }
            }
            .clamp(1, self.preset.advisors.len());
        let decision = serde_json::from_str::<serde_json::Value>(&prompt).unwrap_or_default();
        let mut prior_verdict: Option<DecisionVerdict> = None;
        let mut final_verdict = None;
        for round in 1..=MAX_CONSENSUS_ROUNDS {
            let advisor_prompt = if let Some(prior) = prior_verdict.as_ref() {
                serde_json::json!({
                    "round": round,
                    "decision": decision,
                    "roundOneJudge": {
                        "rationale": prior.rationale,
                        "instruction": prior.instruction,
                        "consensus": prior.consensus,
                        "candidatePlans": prior.options,
                    },
                    "anonymousRoundOneRecommendations": prior.advisors.iter()
                        .filter(|item| item.succeeded)
                        .map(|item| item.summary.as_str())
                        .collect::<Vec<_>>(),
                    "instruction": "Reconsider the complete decision using the anonymous disagreements. Revise your end-to-end recommendation when persuaded; otherwise explicitly defend it. Do not identify or speculate about other advisors."
                })
                .to_string()
            } else {
                serde_json::json!({ "round": round, "decision": decision }).to_string()
            };
            let advisors = self
                .run_advisor_round(advisor_prompt, advisor_limit, cancel)
                .await;
            let successful = advisors.iter().filter(|item| item.succeeded).count();
            if successful == 0 {
                let failures = advisors
                    .iter()
                    .map(|item| format!("{}: {}", item.model, item.summary))
                    .collect::<Vec<_>>()
                    .join("; ");
                anyhow::bail!(
                    "all MoA advisors failed in round {round} (preset='{}'; change them in 设置 → AI 模型 → Advisors): {}",
                    self.name,
                    failures
                );
            }
            let mut verdict = self
                .judge_round(
                    request.trigger,
                    &decision,
                    &advisors,
                    round,
                    prior_verdict.as_ref(),
                    cancel,
                )
                .await?;
            verdict.rounds = round;
            verdict.advisors = advisors;
            if verdict.consensus > REQUIRED_CONSENSUS {
                final_verdict = Some(verdict);
                break;
            }
            if round == MAX_CONSENSUS_ROUNDS {
                resolve_low_consensus(request.trigger, &mut verdict);
                final_verdict = Some(verdict);
                break;
            }
            prior_verdict = Some(verdict);
        }

        let mut verdict = final_verdict.context("MoA produced no verdict")?;
        verdict.usage = self.take_usage().await;
        let verdict = verdict.normalized();
        if let Err(error) = append_decision_journal(&self.name, request, &verdict) {
            tracing::warn!(%error, decision_id = %request.id, "failed to append decision journal");
        }
        Ok(verdict)
    }

    async fn run_advisor_round(
        &self,
        prompt: String,
        advisor_limit: usize,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Vec<AdvisorResult> {
        let advisor_futures = self
            .preset
            .advisors
            .iter()
            .take(advisor_limit)
            .map(|model_ref| {
                let prompt = prompt.clone();
                let cancel = cancel.clone();
                async move {
                    let model_name = format!("{}:{}", model_ref.provider, model_ref.model);
                    let result = async {
                        let primary = self.provider_for(model_ref)?;
                        let advisor_messages = [ChatMessage {
                            role: "user".into(),
                            content: prompt,
                            images: Vec::new(),
                            generated_images: Vec::new(),
                            tool_calls: None,
                            tool_call_id: None,
                        }];
                        let advisor_system = localized_decision_system(DECISION_ADVISOR_SYSTEM);
                        let candidates = self.configured_fallback_providers(&primary);
                        let route_started = Instant::now();
                        let candidate_count = candidates.len();
                        let mut last_error = None;
                        for (index, provider) in candidates.iter().enumerate() {
                            let attempt_timeout = route_attempt_timeout(
                                route_started,
                                candidate_count.saturating_sub(index),
                            );
                            if attempt_timeout.is_zero() {
                                last_error = Some(anyhow::anyhow!(
                                    "advisor fallback route timed out after {}s",
                                    DECISION_PROVIDER_ROUTE_TIMEOUT.as_secs()
                                ));
                                break;
                            }
                            let client = LlmClient::with_provider(
                                provider.clone(),
                                self.backend_url.clone(),
                            );
                            let response = tokio::select! {
                                _ = cancel.cancelled() => anyhow::bail!("decision review cancelled"),
                                timed = tokio::time::timeout(
                                    attempt_timeout,
                                    client.chat_with_sampling(
                                        &advisor_messages,
                                        Some(&advisor_system),
                                        self.preset.advisor_max_tokens.filter(|value| *value > 0),
                                        self.preset.advisor_temperature,
                                    ),
                                ) => match timed {
                                    Ok(response) => response,
                                    Err(_) => Err(anyhow::anyhow!(
                                        "advisor provider timed out after {}s",
                                        attempt_timeout.as_secs()
                                    )),
                                },
                            };
                            match response {
                                Ok(response) => {
                                    self.add_usage(response.usage).await;
                                    if response.content.trim().is_empty() {
                                        last_error = Some(anyhow::anyhow!(
                                            "advisor returned an empty response"
                                        ));
                                        if index + 1 < candidates.len() {
                                            continue;
                                        }
                                        break;
                                    }
                                    return Ok::<(String, String), anyhow::Error>((
                                        format!("{}:{}", provider.name, provider.model),
                                        response.content,
                                    ));
                                }
                                Err(error) => {
                                    let retryable = crate::ai::llm::retry::classify(
                                        None,
                                        &error.to_string(),
                                    ) == crate::ai::llm::retry::ErrorClass::Retryable;
                                    last_error = Some(error);
                                    if !retryable || index + 1 == candidates.len() {
                                        break;
                                    }
                                }
                            }
                        }
                        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no advisor provider configured")))
                    }
                    .await;
                    match result {
                        Ok((model, summary)) => AdvisorResult {
                            model,
                            succeeded: true,
                            summary: truncate_chars(&summary, 4000),
                        },
                        Err(error) => AdvisorResult {
                            model: model_name,
                            succeeded: false,
                            summary: error.to_string(),
                        },
                    }
                }
            });
        join_all(advisor_futures).await
    }

    async fn judge_round(
        &self,
        trigger: DecisionTrigger,
        decision: &serde_json::Value,
        advisors: &[AdvisorResult],
        round: u8,
        prior: Option<&DecisionVerdict>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<DecisionVerdict> {
        let judge_provider = self.provider_for(&self.preset.judge)?;
        let judge_client =
            LlmClient::with_provider(judge_provider.clone(), self.backend_url.clone());
        let judge_prompt = serde_json::json!({
            "round": round,
            "decision": decision,
            "priorJudge": prior.map(|verdict| serde_json::json!({
                "rationale": verdict.rationale,
                "instruction": verdict.instruction,
                "consensus": verdict.consensus,
                "candidatePlans": verdict.options,
            })),
            "advisors": advisors.iter().filter(|item| item.succeeded).collect::<Vec<_>>(),
        })
        .to_string();
        let judge_messages = [ChatMessage {
            role: "user".into(),
            content: judge_prompt.clone(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        }];
        let judge_system = localized_decision_system(DECISION_JUDGE_SYSTEM);
        let response = tokio::select! {
            _ = cancel.cancelled() => anyhow::bail!("decision review cancelled"),
            response = tokio::time::timeout(
                DECISION_PROVIDER_TIMEOUT,
                judge_client.chat_with_sampling(
                    &judge_messages,
                    Some(&judge_system),
                    Some(1200),
                    self.preset.judge_temperature,
                ),
            ) => response.map_err(|_| anyhow::anyhow!(
                "judge provider timed out after {}s",
                DECISION_PROVIDER_TIMEOUT.as_secs()
            ))?.with_context(|| {
                format!(
                    "MoA judge failed (preset='{}', provider='{}', model='{}'; change it in 设置 → AI 模型 → Judge)",
                    self.name, self.preset.judge.provider, self.preset.judge.model
                )
            })?,
        };
        self.add_usage(response.usage).await;
        let mut verdict = match parse_judge_verdict(&response.content) {
            Ok(verdict) => verdict,
            Err(first_error) => {
                let repair_messages = [ChatMessage {
                    role: "user".into(),
                    content: judge_repair_prompt(
                        &judge_prompt,
                        &response.content,
                        &first_error.to_string(),
                    ),
                    images: Vec::new(),
                    generated_images: Vec::new(),
                    tool_calls: None,
                    tool_call_id: None,
                }];
                let repair_system = localized_decision_system(DECISION_JUDGE_REPAIR_SYSTEM);
                let mut last_error = first_error.to_string();
                let mut repaired_verdict = None;
                let repair_providers = self
                    .configured_fallback_providers(&judge_provider)
                    .into_iter()
                    .take(3)
                    .collect::<Vec<_>>();
                let route_started = Instant::now();
                let provider_count = repair_providers.len();
                for (index, provider) in repair_providers.into_iter().enumerate() {
                    let attempt_timeout =
                        route_attempt_timeout(route_started, provider_count.saturating_sub(index));
                    if attempt_timeout.is_zero() {
                        last_error = format!(
                            "judge repair fallback route timed out after {}s",
                            DECISION_PROVIDER_ROUTE_TIMEOUT.as_secs()
                        );
                        break;
                    }
                    let repair_label = format!("{}:{}", provider.name, provider.model);
                    let repair_client =
                        LlmClient::with_provider(provider, self.backend_url.clone());
                    let repaired = tokio::select! {
                        _ = cancel.cancelled() => anyhow::bail!("decision review cancelled"),
                        timed = tokio::time::timeout(
                            attempt_timeout,
                            repair_client.chat_with_sampling(
                                &repair_messages,
                                Some(&repair_system),
                                Some(600),
                                Some(0.0),
                            ),
                        ) => match timed {
                            Ok(response) => response,
                            Err(_) => Err(anyhow::anyhow!(
                                "judge repair provider timed out after {}s",
                                attempt_timeout.as_secs()
                            )),
                        },
                    };
                    match repaired {
                        Ok(response) => {
                            self.add_usage(response.usage).await;
                            match parse_judge_verdict(&response.content) {
                                Ok(verdict) => {
                                    repaired_verdict = Some(verdict);
                                    break;
                                }
                                Err(error) => {
                                    last_error = format!("{repair_label}: {error}");
                                }
                            }
                        }
                        Err(error) => {
                            last_error = format!("{repair_label}: {error}");
                        }
                    }
                }
                repaired_verdict.or_else(|| {
                    fallback_verdict_from_advisors(
                        advisors,
                        &format!(
                        "MoA judge JSON repair failed after configured fallback order (preset='{}', primary='{}:{}'): {}",
                        self.name,
                        self.preset.judge.provider,
                        self.preset.judge.model,
                        last_error
                        ),
                    )
                }).with_context(|| {
                    format!(
                        "MoA judge JSON repair failed and advisor markers were incomplete (preset='{}', primary='{}:{}'): {}",
                        self.name,
                        self.preset.judge.provider,
                        self.preset.judge.model,
                        last_error
                    )
                })?
            }
        };
        normalize_preaction_evidence_outcome(trigger, &mut verdict, advisors);
        Ok(verdict)
    }
}

fn normalize_preaction_evidence_outcome(
    trigger: DecisionTrigger,
    verdict: &mut DecisionVerdict,
    advisors: &[AdvisorResult],
) {
    if trigger != DecisionTrigger::PreAction || verdict.outcome != DecisionOutcome::GatherEvidence {
        return;
    }
    let recommendations = advisors
        .iter()
        .filter(|advisor| advisor.succeeded)
        .map(|advisor| advisor_recommendation(&advisor.summary))
        .collect::<Option<Vec<_>>>();
    let Some(recommendations) = recommendations else {
        return;
    };
    if recommendations.len() >= 2
        && recommendations
            .iter()
            .all(|outcome| *outcome == DecisionOutcome::Proceed)
    {
        verdict.outcome = DecisionOutcome::Proceed;
        verdict.instruction.clear();
        verdict.rationale.push_str(
            " 协议归一化：PreAction 的当前工具本身就是顾问一致建议执行的取证动作，\
             因此 outcome 从 gather_evidence 修正为 proceed。",
        );
    }
}

fn resolve_low_consensus(trigger: DecisionTrigger, verdict: &mut DecisionVerdict) {
    let original_rationale = verdict.rationale.clone();
    match (trigger, verdict.outcome) {
        (DecisionTrigger::AgentRequest, _) | (_, DecisionOutcome::Escalate) => {
            verdict.outcome = DecisionOutcome::Escalate;
            if verdict.options.len() < 2 {
                verdict.options = disagreement_options(&verdict.advisors);
            }
        }
        (DecisionTrigger::PreAction, DecisionOutcome::Proceed) => {
            verdict.outcome = DecisionOutcome::GatherEvidence;
            verdict.instruction = "不要执行当前高影响动作；先完成顾问要求的核验或改成风险更低、可验证的方案，再重新提交评审。".into();
            verdict.options.clear();
        }
        (DecisionTrigger::FinalReview, DecisionOutcome::Proceed) => {
            verdict.outcome = DecisionOutcome::Revise;
            verdict.instruction =
                "在结束前先解决顾问尚未形成共识的实质性证据或交付缺口，再重新进行最终审阅。".into();
            verdict.options.clear();
        }
        (DecisionTrigger::Recovery, DecisionOutcome::Proceed) => {
            verdict.outcome = DecisionOutcome::Revise;
            verdict.instruction = "采用更保守且不会重复原失败的恢复方案，再继续执行。".into();
            verdict.options.clear();
        }
        (_, DecisionOutcome::Revise | DecisionOutcome::GatherEvidence) => {
            verdict.options.clear();
        }
    }
    verdict.rationale = format!(
        "两轮评审后共识度为 {:.0}%，未超过 66%；已选择不执行有争议动作的保守路径。{}{}",
        verdict.consensus * 100.0,
        if original_rationale.is_empty() {
            ""
        } else {
            " "
        },
        original_rationale
    );
}

fn route_attempt_timeout(started: Instant, remaining_candidates: usize) -> Duration {
    if remaining_candidates == 0 {
        return Duration::ZERO;
    }
    let remaining = DECISION_PROVIDER_ROUTE_TIMEOUT.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Duration::ZERO;
    }
    let divisor = u32::try_from(remaining_candidates).unwrap_or(u32::MAX);
    (remaining / divisor)
        .max(Duration::from_secs(1))
        .min(DECISION_PROVIDER_TIMEOUT)
}

fn disagreement_options(advisors: &[AdvisorResult]) -> Vec<DecisionOption> {
    let mut seen = std::collections::HashSet::new();
    let mut options = advisors
        .iter()
        .filter(|advisor| advisor.succeeded && seen.insert(advisor.summary.trim().to_string()))
        .take(4)
        .enumerate()
        .map(|(index, advisor)| DecisionOption {
            id: format!("option_{}", index + 1),
            label: format!("方案 {}", index + 1),
            description: advisor.summary.clone(),
        })
        .collect::<Vec<_>>();
    if options.len() < 2 {
        options.push(DecisionOption {
            id: "gather_more_evidence".into(),
            label: "先补充证据再执行".into(),
            description: "暂停当前方案，先收集能消除核心分歧的证据，再按新证据重新评审并执行。"
                .into(),
        });
    }
    options
}

fn append_decision_journal(
    preset: &str,
    request: &DecisionRequest,
    verdict: &DecisionVerdict,
) -> Result<()> {
    use std::io::Write;

    let data_dir = crate::runtime::settings::local_config::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let path = data_dir.join("decision-journal.jsonl");
    let record = serde_json::json!({
        "schemaVersion": 1,
        "timestamp": chrono::Utc::now(),
        "decisionId": request.id,
        "preset": preset,
        "trigger": request.trigger,
        "risk": request.risk,
        "question": request.question,
        "scope": "session",
        "source": "moa",
        "outcome": verdict.outcome,
        "confidence": verdict.confidence,
        "consensus": verdict.consensus,
        "rounds": verdict.rounds,
        "rationale": verdict.rationale,
        "instruction": verdict.instruction,
        "options": verdict.options,
        "advisorCount": verdict.advisors.len(),
        "promotedToMemory": false
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    writeln!(file, "{record}")?;
    Ok(())
}

#[async_trait]
impl DecisionReviewer for MoaRuntime {
    async fn review(
        &self,
        request: &DecisionRequest,
        messages: &[ChatMessage],
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<DecisionVerdict> {
        self.review_decision(request, messages, cancel).await
    }
}

fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        value.chars().take(max).collect::<String>() + "…"
    }
}

fn extract_json_object(value: &str) -> Option<&str> {
    let start = value.find('{')?;
    let end = value.rfind('}')?;
    (start <= end).then(|| &value[start..=end])
}

fn parse_judge_verdict(value: &str) -> Result<DecisionVerdict> {
    let json = extract_json_object(value).context("MoA judge did not return a JSON object")?;
    let parsed = serde_json::from_str::<serde_json::Value>(json).or_else(|_| {
        serde_json::from_str::<serde_json::Value>(&remove_trailing_json_commas(json))
    })?;
    let normalized = normalize_judge_value(parsed)?;
    serde_json::from_value(normalized).context("invalid MoA judge verdict")
}

fn judge_repair_prompt(judge_prompt: &str, invalid_response: &str, parse_error: &str) -> String {
    serde_json::json!({
        "judgeInput": serde_json::from_str::<serde_json::Value>(judge_prompt)
            .unwrap_or_else(|_| serde_json::Value::String(judge_prompt.to_string())),
        "invalidResponse": truncate_chars(invalid_response, 6000),
        "parseError": parse_error,
        "instruction": "Judge the supplied decision yourself from judgeInput. The invalidResponse is diagnostic context only; do not merely describe or repair its syntax.",
    })
    .to_string()
}

fn normalize_judge_value(mut value: serde_json::Value) -> Result<serde_json::Value> {
    for key in ["verdict", "result", "judgment", "judgement"] {
        if value.get(key).is_some_and(serde_json::Value::is_object) {
            value = value[key].take();
            break;
        }
    }
    let object = value
        .as_object_mut()
        .context("MoA judge verdict must be an object")?;
    let raw_outcome = object
        .get("outcome")
        .and_then(serde_json::Value::as_str)
        .or_else(|| object.get("decision").and_then(serde_json::Value::as_str))
        .or_else(|| object.get("action").and_then(serde_json::Value::as_str))
        .context("MoA judge verdict is missing outcome")?;
    let normalized_outcome = match raw_outcome
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '-'], "_")
        .as_str()
    {
        "proceed" | "approve" | "approved" | "continue" | "execute" | "allow" => "proceed",
        "revise" | "retry" | "modify" | "change" => "revise",
        "gather_evidence" | "research" | "investigate" => "gather_evidence",
        "escalate" | "ask_user" | "pause" | "reject" | "deny" | "denied" => "escalate",
        other => anyhow::bail!("unknown MoA judge outcome: {other}"),
    };
    object.insert("outcome".into(), serde_json::json!(normalized_outcome));
    for key in ["confidence", "consensus"] {
        if let Some(number) = object.get(key).and_then(serde_json::Value::as_f64) {
            if number > 1.0 && number <= 100.0 {
                object.insert(key.into(), serde_json::json!(number / 100.0));
            }
        }
    }
    for key in ["options", "advisors"] {
        if object.get(key).is_some_and(|item| {
            item.as_array()
                .is_some_and(|items| items.iter().any(|entry| !entry.is_object()))
        }) {
            object.remove(key);
        }
    }
    Ok(value)
}

fn remove_trailing_json_commas(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let chars = value.chars().collect::<Vec<_>>();
    let mut in_string = false;
    let mut escaped = false;
    let mut index = 0;
    while index < chars.len() {
        let current = chars[index];
        if in_string {
            output.push(current);
            if escaped {
                escaped = false;
            } else if current == '\\' {
                escaped = true;
            } else if current == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if current == '"' {
            in_string = true;
            output.push(current);
            index += 1;
            continue;
        }
        if current == ',' {
            let mut lookahead = index + 1;
            while lookahead < chars.len() && chars[lookahead].is_whitespace() {
                lookahead += 1;
            }
            if lookahead < chars.len() && matches!(chars[lookahead], '}' | ']') {
                index += 1;
                continue;
            }
        }
        output.push(current);
        index += 1;
    }
    output
}

pub fn runtime_for_active_preset(
    config: &MoaConfig,
    providers: Vec<ProviderConfig>,
    backend_url: String,
) -> Result<Option<MoaRuntime>> {
    if config.active_preset.is_empty() {
        return Ok(None);
    }
    let preset = config
        .presets
        .get(&config.active_preset)
        .cloned()
        .with_context(|| format!("active MoA preset not found: {}", config.active_preset))?;
    if !preset.enabled {
        return Ok(None);
    }
    if preset.advisors.is_empty() {
        anyhow::bail!("active MoA preset requires at least one advisor");
    }
    if preset.judge.model.is_empty() {
        anyhow::bail!("active MoA preset requires a judge");
    }
    for temperature in [preset.advisor_temperature, preset.judge_temperature]
        .into_iter()
        .flatten()
    {
        if !(0.0..=2.0).contains(&temperature) {
            anyhow::bail!("MoA temperature must be between 0 and 2");
        }
    }
    Ok(Some(MoaRuntime {
        name: config.active_preset.clone(),
        preset,
        providers,
        backend_url,
        grounding_cache: Arc::new(Mutex::new(HashMap::new())),
        usage: Arc::new(Mutex::new(TokenUsage::default())),
    }))
}

pub async fn active_runtime(
    backend: &crate::runtime::backend::BackendClient,
    backend_url: String,
) -> Result<Option<MoaRuntime>> {
    let config = crate::runtime::decision::load_moa_config(backend).await;
    if config.active_preset.is_empty() {
        return Ok(None);
    }
    let providers = crate::runtime::decision::registry::load_providers(backend).await;
    runtime_for_active_preset(&config, providers, backend_url)
}

#[cfg(test)]
mod decision_tests {
    use super::*;

    #[test]
    fn decision_provider_attempts_are_time_bounded() {
        assert_eq!(DECISION_PROVIDER_TIMEOUT, Duration::from_secs(180));
        assert_eq!(DECISION_PROVIDER_ROUTE_TIMEOUT, Duration::from_secs(180));
        let shared = route_attempt_timeout(Instant::now(), 3);
        assert!(shared <= Duration::from_secs(60));
        assert!(shared >= Duration::from_secs(59));
    }

    #[test]
    fn advisor_fallback_requires_an_exact_first_line_marker() {
        assert_eq!(
            advisor_recommendation("RECOMMENDATION: PROCEED\nLooks good."),
            Some(DecisionOutcome::Proceed)
        );
        assert_eq!(
            advisor_recommendation("I recommend proceed."),
            None,
            "free-form prose must never become an automatic approval"
        );
    }

    #[test]
    fn advisor_fallback_only_proceeds_when_every_structured_advisor_proceeds() {
        let proceed = AdvisorResult {
            model: "peach".into(),
            succeeded: true,
            summary: "RECOMMENDATION: PROCEED\nNo material issue.".into(),
        };
        let revise = AdvisorResult {
            model: "claude".into(),
            succeeded: true,
            summary: "RECOMMENDATION: REVISE\nRemove unsupported claim.".into(),
        };
        let unanimous =
            fallback_verdict_from_advisors(&[proceed.clone(), proceed.clone()], "judge timeout")
                .expect("structured advisors should produce a fallback");
        assert_eq!(unanimous.outcome, DecisionOutcome::Proceed);
        assert_eq!(unanimous.consensus, 1.0);

        let disagreement = fallback_verdict_from_advisors(&[proceed, revise], "judge timeout")
            .expect("structured advisors should produce a fallback");
        assert_eq!(disagreement.outcome, DecisionOutcome::Revise);
        assert_eq!(disagreement.consensus, 0.5);
    }

    #[test]
    fn advisor_fallback_rejects_incomplete_structured_recommendations() {
        let marked = AdvisorResult {
            model: "peach".into(),
            succeeded: true,
            summary: "RECOMMENDATION: PROCEED\nNo material issue.".into(),
        };
        let unmarked = AdvisorResult {
            model: "k3".into(),
            succeeded: true,
            summary: "Looks acceptable.".into(),
        };
        assert!(fallback_verdict_from_advisors(&[marked, unmarked], "judge timeout").is_none());
    }

    fn test_provider(name: &str, model: &str) -> ProviderConfig {
        ProviderConfig {
            name: name.into(),
            base_url: "https://example.invalid".into(),
            api_key: "test".into(),
            protocol: "openai".into(),
            model: model.into(),
            models: vec![crate::ai::config::ModelEntry {
                id: model.into(),
                name: model.into(),
                enabled: Some(true),
                max_output: None,
                context_window: None,
                capabilities: None,
                request_params: None,
                thinking_params: None,
                deferred_tools_mode: None,
                ..Default::default()
            }],
            use_proxy: None,
            compat_profile: None,
        }
    }

    #[test]
    fn preserves_source_tool_results_outside_recent_transcript_window() {
        let mut messages = vec![ChatMessage {
            role: "assistant".into(),
            content: String::new(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: Some(vec![crate::ai::llm::ToolCall {
                id: "evidence-call".into(),
                kind: "function".into(),
                function: crate::ai::llm::FunctionCall {
                    name: "search_file_content".into(),
                    arguments: "{}".into(),
                },
                thought_signature: None,
            }]),
            tool_call_id: None,
        }];
        messages.push(ChatMessage {
            role: "tool".into(),
            content: "2030 sales share exceeds 40%".into(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: Some("evidence-call".into()),
        });
        messages.extend((0..12).map(|index| ChatMessage {
            role: "assistant".into(),
            content: format!("later message {index}"),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        }));

        let evidence = source_evidence_transcript(&messages);
        assert!(evidence.contains("search_file_content"));
        assert!(evidence.contains("2030 sales share exceeds 40%"));
    }

    #[test]
    fn preserves_later_structured_data_evidence_after_large_early_result() {
        let mut messages = Vec::new();
        for (index, (tool, content)) in [
            ("web_read", "early release ".repeat(350)),
            (
                "bash",
                "BEA Table 2 annual contribution: PCE 1.87, investment 0.73".to_string(),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let call_id = format!("evidence-{index}");
            messages.push(ChatMessage {
                role: "assistant".into(),
                content: String::new(),
                images: Vec::new(),
                generated_images: Vec::new(),
                tool_calls: Some(vec![crate::ai::llm::ToolCall {
                    id: call_id.clone(),
                    kind: "function".into(),
                    function: crate::ai::llm::FunctionCall {
                        name: tool.into(),
                        arguments: "{}".into(),
                    },
                    thought_signature: None,
                }]),
                tool_call_id: None,
            });
            messages.push(ChatMessage {
                role: "tool".into(),
                content,
                images: Vec::new(),
                generated_images: Vec::new(),
                tool_calls: None,
                tool_call_id: Some(call_id),
            });
        }

        let evidence = source_evidence_transcript(&messages);
        assert!(evidence.contains("early release"));
        assert!(evidence.contains("BEA Table 2 annual contribution"));
        assert!(evidence.contains("PCE 1.87"));
        assert!(evidence
            .find("BEA Table 2 annual contribution")
            .is_some_and(|position| position > 4000));
        assert!(evidence.len() < FINAL_REVIEW_SOURCE_EVIDENCE_CHARS);
    }

    #[test]
    fn final_review_treats_requested_placeholders_as_missing_and_sees_data_work() {
        assert!(DECISION_ADVISOR_SYSTEM.contains("is still a missing required deliverable"));
        assert!(DECISION_JUDGE_SYSTEM.contains("does not satisfy an explicitly requested"));
        assert!(is_source_evidence_tool("download_web_file"));
        assert!(is_source_evidence_tool("bash"));
        assert!(is_source_evidence_tool("read_pdf"));
    }

    #[test]
    fn preaction_does_not_confuse_executing_evidence_tools_with_rejecting_them() {
        let advisors = ["peach", "k3"]
            .into_iter()
            .map(|model| AdvisorResult {
                model: model.into(),
                succeeded: true,
                summary: "RECOMMENDATION: PROCEED\nExecute the read-only evidence tool.".into(),
            })
            .collect::<Vec<_>>();
        let mut verdict = DecisionVerdict {
            outcome: DecisionOutcome::GatherEvidence,
            confidence: 0.9,
            consensus: 0.95,
            rationale: "The proposed command gathers evidence.".into(),
            instruction: "Gather evidence.".into(),
            options: Vec::new(),
            rounds: 1,
            advisors: Vec::new(),
            usage: TokenUsage::default(),
        };

        normalize_preaction_evidence_outcome(DecisionTrigger::PreAction, &mut verdict, &advisors);
        assert_eq!(verdict.outcome, DecisionOutcome::Proceed);
        assert!(verdict.instruction.is_empty());

        verdict.outcome = DecisionOutcome::GatherEvidence;
        normalize_preaction_evidence_outcome(DecisionTrigger::FinalReview, &mut verdict, &advisors);
        assert_eq!(verdict.outcome, DecisionOutcome::GatherEvidence);
    }

    #[test]
    fn final_review_keeps_full_document_and_later_patch_evidence() {
        let long_document = format!(
            "{{\"content\":{{\"markdown\":\"{}UNSUPPORTED_TAIL\"}}}}",
            "正文".repeat(1200)
        );
        let messages = vec![
            ChatMessage {
                role: "assistant".into(),
                content: String::new(),
                images: Vec::new(),
                generated_images: Vec::new(),
                tool_calls: Some(vec![crate::ai::llm::ToolCall {
                    id: "inspect-1".into(),
                    kind: "function".into(),
                    function: crate::ai::llm::FunctionCall {
                        name: "inspect_document".into(),
                        arguments: r#"{"documentId":"doc-1"}"#.into(),
                    },
                    thought_signature: None,
                }]),
                tool_call_id: None,
            },
            ChatMessage {
                role: "tool".into(),
                content: long_document,
                images: Vec::new(),
                generated_images: Vec::new(),
                tool_calls: None,
                tool_call_id: Some("inspect-1".into()),
            },
            ChatMessage {
                role: "assistant".into(),
                content: String::new(),
                images: Vec::new(),
                generated_images: Vec::new(),
                tool_calls: Some(vec![crate::ai::llm::ToolCall {
                    id: "patch-1".into(),
                    kind: "function".into(),
                    function: crate::ai::llm::FunctionCall {
                        name: "patch_document".into(),
                        arguments: r#"{"documentId":"doc-1","expectedRevision":1,"patches":[{"op":"replaceText","path":"/markdown","matchText":"bad","value":"good"}]}"#.into(),
                    },
                    thought_signature: None,
                }]),
                tool_call_id: None,
            },
            ChatMessage {
                role: "tool".into(),
                content: "revision=2".into(),
                images: Vec::new(),
                generated_images: Vec::new(),
                tool_calls: None,
                tool_call_id: Some("patch-1".into()),
            },
        ];

        let evidence = document_review_transcript(&messages);
        assert!(evidence.contains("UNSUPPORTED_TAIL"));
        assert!(evidence.contains("replaceText"));
        assert!(evidence.contains("revision=2"));
    }

    #[test]
    fn fallback_routes_follow_the_active_preset_order() {
        let advisor_a = test_provider("provider-a", "advisor-a");
        let advisor_b = test_provider("provider-b", "advisor-b");
        let judge = test_provider("provider-j", "judge");
        let runtime = MoaRuntime {
            name: "test".into(),
            preset: MoaPreset {
                advisors: vec![
                    crate::runtime::decision::config::MoaModelRef {
                        provider: "provider-a".into(),
                        model: "advisor-a".into(),
                    },
                    crate::runtime::decision::config::MoaModelRef {
                        provider: "provider-b".into(),
                        model: "advisor-b".into(),
                    },
                ],
                judge: crate::runtime::decision::config::MoaModelRef {
                    provider: "provider-j".into(),
                    model: "judge".into(),
                },
                ..Default::default()
            },
            providers: vec![
                advisor_a.clone(),
                advisor_b,
                judge.clone(),
                test_provider("unused", "not-configured"),
            ],
            backend_url: String::new(),
            grounding_cache: Arc::new(Mutex::new(HashMap::new())),
            usage: Arc::new(Mutex::new(TokenUsage::default())),
        };

        let advisor_route = runtime
            .configured_fallback_providers(&advisor_a)
            .into_iter()
            .map(|provider| provider.model)
            .collect::<Vec<_>>();
        assert_eq!(
            advisor_route,
            vec![
                "advisor-a".to_string(),
                "advisor-b".to_string(),
                "judge".to_string(),
            ]
        );

        let judge_route = runtime
            .configured_fallback_providers(&judge)
            .into_iter()
            .map(|provider| provider.model)
            .collect::<Vec<_>>();
        assert_eq!(
            judge_route,
            vec![
                "judge".to_string(),
                "advisor-a".to_string(),
                "advisor-b".to_string(),
            ]
        );
    }

    #[test]
    fn extracts_json_from_fenced_or_prefixed_output() {
        assert_eq!(
            extract_json_object("```json\n{\"outcome\":\"proceed\"}\n```"),
            Some("{\"outcome\":\"proceed\"}")
        );
        assert_eq!(extract_json_object("no object"), None);
    }

    #[test]
    fn judge_repair_keeps_the_original_decision_context() {
        let prompt = judge_repair_prompt(
            r#"{"decision":{"trigger":"final_review","proposal":"deliver report"},"advisors":[{"summary":"proceed"}]}"#,
            "",
            "missing JSON",
        );
        let value: serde_json::Value = serde_json::from_str(&prompt).unwrap();
        assert_eq!(
            value["judgeInput"]["decision"]["proposal"],
            "deliver report"
        );
        assert_eq!(value["judgeInput"]["advisors"][0]["summary"], "proceed");
        assert!(value["instruction"]
            .as_str()
            .unwrap()
            .contains("judgeInput"));
    }

    #[test]
    fn parses_wrapped_alias_and_percent_scores() {
        let verdict = parse_judge_verdict(
            r#"```json
            {"verdict":{"decision":"APPROVE","confidence":90,"consensus":80}}
            ```"#,
        )
        .unwrap();
        assert_eq!(verdict.outcome, DecisionOutcome::Proceed);
        assert!((verdict.confidence - 0.9).abs() < f32::EPSILON);
        assert!((verdict.consensus - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn parses_trailing_commas_and_maps_reject_to_escalate() {
        let verdict =
            parse_judge_verdict(r#"{"outcome":"reject","rationale":"unsafe","options":[],}"#)
                .unwrap();
        assert_eq!(verdict.outcome, DecisionOutcome::Escalate);
    }

    #[test]
    fn unknown_outcome_is_not_silently_approved() {
        let error = parse_judge_verdict(r#"{"outcome":"maybe"}"#).unwrap_err();
        assert!(error.to_string().contains("unknown MoA judge outcome"));
    }

    #[test]
    fn low_consensus_pre_action_chooses_evidence_over_user_escalation() {
        let mut verdict = DecisionVerdict {
            outcome: DecisionOutcome::Proceed,
            confidence: 0.8,
            consensus: 0.6,
            rationale: "verify the source before plotting".into(),
            instruction: String::new(),
            options: vec![],
            rounds: 2,
            advisors: vec![],
            usage: TokenUsage::default(),
        };
        resolve_low_consensus(DecisionTrigger::PreAction, &mut verdict);
        assert_eq!(verdict.outcome, DecisionOutcome::GatherEvidence);
        assert!(verdict.instruction.contains("先完成顾问要求的核验"));
        assert!(verdict.options.is_empty());
    }

    #[test]
    fn explicit_agent_request_can_still_escalate_on_low_consensus() {
        let mut verdict = DecisionVerdict {
            outcome: DecisionOutcome::Proceed,
            confidence: 0.5,
            consensus: 0.4,
            rationale: String::new(),
            instruction: String::new(),
            options: vec![],
            rounds: 2,
            advisors: vec![],
            usage: TokenUsage::default(),
        };
        resolve_low_consensus(DecisionTrigger::AgentRequest, &mut verdict);
        assert_eq!(verdict.outcome, DecisionOutcome::Escalate);
    }
}
