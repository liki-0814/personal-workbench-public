use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum IllustrationMode {
    #[default]
    Auto,
    Diagram,
    Plot,
    Polish,
    Refine,
    Eval,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum IllustrationQuality {
    Fast,
    #[default]
    Balanced,
    Max,
}

impl IllustrationQuality {
    pub fn candidates(self) -> usize {
        match self {
            Self::Fast => 1,
            Self::Balanced => 2,
            Self::Max => 4,
        }
    }
    pub fn critic_rounds(self) -> usize {
        match self {
            Self::Fast => 1,
            Self::Balanced => 2,
            Self::Max => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalMode {
    #[default]
    Auto,
    None,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IllustrationConstraints {
    #[serde(default)]
    pub invariants: Vec<String>,
    #[serde(default)]
    pub exact_text: Vec<String>,
    #[serde(default)]
    pub factual_constraints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IllustrationRequest {
    #[serde(default)]
    pub mode: IllustrationMode,
    pub content: Value,
    pub visual_intent: String,
    #[serde(default)]
    pub image_refs: Vec<crate::runtime::visual_generation::ImageReferenceRequest>,
    #[serde(default)]
    pub quality: IllustrationQuality,
    #[serde(default)]
    pub retrieval: RetrievalMode,
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    #[serde(default)]
    pub constraints: IllustrationConstraints,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IllustrationRunStatus {
    Running,
    Completed,
    CompletedWithWarnings,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IllustrationRevision {
    pub round: usize,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub critic_suggestions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plot_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IllustrationCandidate {
    pub id: String,
    pub revisions: Vec<IllustrationRevision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_revision: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IllustrationRun {
    pub schema_version: u32,
    pub id: String,
    pub session_id: String,
    pub request: IllustrationRequest,
    pub resolved_mode: IllustrationMode,
    pub status: IllustrationRunStatus,
    pub stage: String,
    pub model_id: String,
    pub retrieved_reference_ids: Vec<String>,
    pub candidates: Vec<IllustrationCandidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_candidate_id: Option<String>,
    pub warnings: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferencePackManifest {
    pub schema_version: u32,
    pub id: String,
    pub version: String,
    pub license_notice: String,
    pub entries: Vec<ReferenceEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceEntry {
    pub id: String,
    pub kind: IllustrationMode,
    pub layout: String,
    pub content_summary: String,
    pub visual_intent: String,
    pub keywords: Vec<String>,
    pub image_path: String,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
    pub perceptual_hash: String,
    pub source: String,
    pub author: String,
    pub license: String,
    pub redistributable: bool,
    pub notice: String,
}

#[derive(Debug, Clone)]
pub struct RetrievedReference {
    pub entry: ReferenceEntry,
    pub bytes: Vec<u8>,
}
