use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use base64::Engine;
use chrono::{DateTime, Utc};
use regex::Regex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const SCHEMA_VERSION: u32 = 1;
const MAX_HTML_BYTES: usize = 64 * 1024 * 1024;
const MAX_MARKDOWN_BYTES: usize = 16 * 1024 * 1024;
const MAX_EVIDENCE_BYTES: usize = 512 * 1024;
const MAX_EVIDENCE_ENTRIES: usize = 100;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DocumentKind {
    Html,
    Markdown,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DocumentRuntime {
    #[default]
    Static,
    Archify,
    Delegated,
}

impl DocumentKind {
    pub fn source_filename(self) -> &'static str {
        match self {
            Self::Html => "document.html",
            Self::Markdown => "document.md",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Markdown => "markdown",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "html" => Ok(Self::Html),
            "markdown" => Ok(Self::Markdown),
            _ => bail!("unsupported document kind: {value}"),
        }
    }
}

impl DocumentRuntime {
    fn as_str(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Archify => "archify",
            Self::Delegated => "delegated",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "static" => Ok(Self::Static),
            "archify" => Ok(Self::Archify),
            "delegated" => Ok(Self::Delegated),
            _ => bail!("unsupported document runtime: {value}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocumentOriginType {
    RuntimeTask,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentOrigin {
    #[serde(rename = "type")]
    pub origin_type: DocumentOriginType,
    pub task_id: String,
    pub attempt_id: String,
    pub batch_id: String,
    pub executor_id: String,
    pub agent_name: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskDeliverableRelation {
    Primary,
    Supporting,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskDeliverableState {
    Materializing,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskDeliverable {
    pub document_id: String,
    pub relation: TaskDeliverableRelation,
    pub state: TaskDeliverableState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentManifest {
    pub schema_version: u32,
    pub id: String,
    pub kind: DocumentKind,
    pub title: String,
    pub status: String,
    pub revision: u64,
    pub updated_at: DateTime<Utc>,
    pub qa_status: String,
    #[serde(default)]
    pub runtime: DocumentRuntime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<DocumentOrigin>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSummary {
    pub id: String,
    pub kind: DocumentKind,
    pub title: String,
    pub status: String,
    pub revision: u64,
    pub updated_at: DateTime<Utc>,
    pub qa_status: String,
    pub runtime: DocumentRuntime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<DocumentOrigin>,
}

impl From<DocumentManifest> for DocumentSummary {
    fn from(value: DocumentManifest) -> Self {
        Self {
            id: value.id,
            kind: value.kind,
            title: value.title,
            status: value.status,
            revision: value.revision,
            updated_at: value.updated_at,
            qa_status: value.qa_status,
            runtime: value.runtime,
            origin: value.origin,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentPayload {
    pub manifest: DocumentManifest,
    pub content: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceBasis {
    Source,
    UserInput,
    Calculation,
    Analysis,
    Assumption,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceEntry {
    pub claim: String,
    pub basis: EvidenceBasis,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limitations: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceLedger {
    pub schema_version: u32,
    pub document_id: String,
    pub document_revision: u64,
    pub updated_at: DateTime<Utc>,
    pub entries: Vec<EvidenceEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceLedgerView {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ledger: Option<EvidenceLedger>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDocument {
    pub kind: DocumentKind,
    pub title: String,
    #[serde(default)]
    pub content: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDocument {
    pub expected_revision: u64,
    #[serde(default)]
    pub title: Option<String>,
    pub content: Value,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub qa_status: Option<String>,
}

#[derive(Debug)]
pub struct RevisionConflict {
    pub expected: u64,
    pub current: u64,
}

impl std::fmt::Display for RevisionConflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "document revision conflict: expected {}, current {}",
            self.expected, self.current
        )
    }
}

impl std::error::Error for RevisionConflict {}

pub fn init(data_dir: &Path) -> Result<()> {
    fs::create_dir_all(documents_root(data_dir))?;
    let connection = open_connection(data_dir)?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS documents (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            title TEXT NOT NULL,
            status TEXT NOT NULL,
            revision INTEGER NOT NULL,
            updated_at TEXT NOT NULL,
            qa_status TEXT NOT NULL,
            runtime TEXT NOT NULL DEFAULT 'static',
            origin_json TEXT
        );",
    )?;
    ensure_column(
        &connection,
        "documents",
        "runtime",
        "TEXT NOT NULL DEFAULT 'static'",
    )?;
    ensure_column(&connection, "documents", "origin_json", "TEXT")?;
    Ok(())
}

pub fn list(data_dir: &Path) -> Result<Vec<DocumentSummary>> {
    init(data_dir)?;
    let connection = open_connection(data_dir)?;
    let mut statement = connection.prepare(
        "SELECT id, kind, title, status, revision, updated_at, qa_status, runtime, origin_json
         FROM documents ORDER BY updated_at DESC",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, Option<String>>(8)?,
        ))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(
            |(id, kind, title, status, revision, updated_at, qa_status, runtime, origin)| {
                Ok(DocumentSummary {
                    id,
                    kind: DocumentKind::parse(&kind)?,
                    title,
                    status,
                    revision: revision.try_into().unwrap_or_default(),
                    updated_at: updated_at.parse::<DateTime<Utc>>()?,
                    qa_status,
                    runtime: DocumentRuntime::parse(&runtime)?,
                    origin: origin
                        .map(|value| serde_json::from_str::<DocumentOrigin>(&value))
                        .transpose()?,
                })
            },
        )
        .collect()
}

pub fn create(data_dir: &Path, request: CreateDocument) -> Result<DocumentPayload> {
    init(data_dir)?;
    let title = non_empty_title(&request.title)?;
    let manifest = DocumentManifest {
        schema_version: SCHEMA_VERSION,
        id: Uuid::now_v7().simple().to_string(),
        kind: request.kind,
        title,
        status: "draft".into(),
        revision: 1,
        updated_at: Utc::now(),
        qa_status: "pending".into(),
        runtime: DocumentRuntime::Static,
        origin: None,
    };
    let content = normalize_content(
        request.kind,
        request
            .content
            .unwrap_or_else(|| default_content(request.kind)),
        false,
    )?;
    let directory = document_dir(data_dir, &manifest.id)?;
    fs::create_dir_all(directory.join("assets"))?;
    write_document_files(&directory, &manifest, &content)?;
    upsert_metadata(data_dir, &manifest)?;
    Ok(DocumentPayload { manifest, content })
}

pub fn read(data_dir: &Path, id: &str) -> Result<DocumentPayload> {
    let directory = document_dir(data_dir, id)?;
    let manifest: DocumentManifest = serde_json::from_slice(
        &fs::read(directory.join("manifest.json")).context("document not found")?,
    )
    .context("document manifest is invalid")?;
    let source = fs::read_to_string(directory.join(manifest.kind.source_filename()))
        .context("document source is missing")?;
    let content = normalize_content(
        manifest.kind,
        match manifest.kind {
            DocumentKind::Html => json!({ "html": source }),
            DocumentKind::Markdown => json!({ "markdown": source }),
        },
        manifest.runtime == DocumentRuntime::Archify,
    )?;
    Ok(DocumentPayload { manifest, content })
}

pub fn update(data_dir: &Path, id: &str, request: UpdateDocument) -> Result<DocumentPayload> {
    let current = read(data_dir, id)?;
    if current.manifest.runtime != DocumentRuntime::Static {
        bail!("generated documents are read-only; save a copy before editing");
    }
    if current.manifest.revision != request.expected_revision {
        return Err(RevisionConflict {
            expected: request.expected_revision,
            current: current.manifest.revision,
        }
        .into());
    }
    let mut manifest = current.manifest;
    if let Some(title) = request.title {
        manifest.title = non_empty_title(&title)?;
    }
    if let Some(status) = request.status {
        manifest.status = status;
    }
    let _ = request.qa_status;
    manifest.qa_status = "pending".into();
    manifest.revision += 1;
    manifest.updated_at = Utc::now();
    let content = normalize_content(manifest.kind, request.content, false)?;
    let directory = document_dir(data_dir, id)?;
    write_document_files(&directory, &manifest, &content)?;
    let _ = fs::remove_file(directory.join("layout.json"));
    upsert_metadata(data_dir, &manifest)?;
    Ok(DocumentPayload { manifest, content })
}

pub fn delete(data_dir: &Path, id: &str) -> Result<()> {
    let directory = document_dir(data_dir, id)?;
    if directory.exists() {
        fs::remove_dir_all(directory)?;
    }
    init(data_dir)?;
    open_connection(data_dir)?.execute("DELETE FROM documents WHERE id=?1", [id])?;
    Ok(())
}

pub fn read_evidence_ledger(data_dir: &Path, id: &str) -> Result<EvidenceLedgerView> {
    let document = read(data_dir, id)?;
    let path = document_dir(data_dir, id)?.join("evidence.json");
    if !path.is_file() {
        return Ok(EvidenceLedgerView {
            status: "missing",
            ledger: None,
        });
    }
    let ledger: EvidenceLedger =
        serde_json::from_slice(&fs::read(path)?).context("document evidence ledger is invalid")?;
    validate_evidence_ledger(&ledger)?;
    if ledger.document_id != id {
        bail!("document evidence ledger belongs to another document");
    }
    Ok(EvidenceLedgerView {
        status: if ledger.document_revision == document.manifest.revision {
            "current"
        } else {
            "stale"
        },
        ledger: Some(ledger),
    })
}

pub fn replace_evidence_ledger(
    data_dir: &Path,
    id: &str,
    expected_revision: u64,
    entries: Vec<EvidenceEntry>,
) -> Result<EvidenceLedger> {
    let document = read(data_dir, id)?;
    if document.manifest.revision != expected_revision {
        return Err(RevisionConflict {
            expected: expected_revision,
            current: document.manifest.revision,
        }
        .into());
    }
    let ledger = EvidenceLedger {
        schema_version: SCHEMA_VERSION,
        document_id: id.to_string(),
        document_revision: document.manifest.revision,
        updated_at: Utc::now(),
        entries,
    };
    validate_evidence_ledger(&ledger)?;
    atomic_write(
        &document_dir(data_dir, id)?.join("evidence.json"),
        &serde_json::to_vec_pretty(&ledger)?,
    )?;
    Ok(ledger)
}

pub fn store_asset(data_dir: &Path, id: &str, filename: &str, bytes: &[u8]) -> Result<String> {
    if bytes.is_empty() {
        bail!("asset is empty");
    }
    let extension = safe_image_extension(filename, bytes)?;
    let asset_id = format!("{}.{extension}", Uuid::new_v4().simple());
    let directory = document_dir(data_dir, id)?;
    let manifest: DocumentManifest = serde_json::from_slice(
        &fs::read(directory.join("manifest.json")).context("HTML document not found")?,
    )
    .context("document is not an HTML document")?;
    if manifest.kind != DocumentKind::Html {
        bail!("document is not an HTML document");
    }
    let assets = directory.join("assets");
    fs::create_dir_all(&assets)?;
    atomic_write(&assets.join(&asset_id), bytes)?;
    Ok(format!("assets/{asset_id}"))
}

pub fn resolve_asset(data_dir: &Path, id: &str, asset: &str) -> Result<PathBuf> {
    validate_id(id)?;
    if asset.is_empty() || asset.contains('/') || asset.contains('\\') || asset.contains("..") {
        bail!("invalid asset name");
    }
    let path = document_dir(data_dir, id)?.join("assets").join(asset);
    if !path.is_file() {
        bail!("asset not found");
    }
    Ok(path)
}

pub fn export_html(data_dir: &Path, id: &str) -> Result<(DocumentManifest, String)> {
    let payload = read(data_dir, id)?;
    let directory = document_dir(data_dir, id)?;
    let mut html = payload.content["html"]
        .as_str()
        .context("HTML document content requires /html")?
        .to_owned();
    let assets = directory.join("assets");
    if assets.is_dir() {
        for entry in fs::read_dir(assets)? {
            let entry = entry?;
            if !entry.path().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let bytes = fs::read(entry.path())?;
            let mime = match entry.path().extension().and_then(|value| value.to_str()) {
                Some("png") => "image/png",
                Some("webp") => "image/webp",
                _ => "image/jpeg",
            };
            let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
            html = html.replace(
                &format!("assets/{name}"),
                &format!("data:{mime};base64,{encoded}"),
            );
        }
    }
    Ok((payload.manifest, html))
}

pub fn import_html(data_dir: &Path, title: &str, bytes: &[u8]) -> Result<DocumentPayload> {
    if bytes.is_empty() || bytes.len() > MAX_HTML_BYTES {
        bail!("HTML file is empty or exceeds 64 MiB");
    }
    let html = std::str::from_utf8(bytes).context("HTML file must be UTF-8")?;
    create(
        data_dir,
        CreateDocument {
            kind: DocumentKind::Html,
            title: title.to_owned(),
            content: Some(json!({ "html": html })),
        },
    )
}

pub fn create_archify(data_dir: &Path, title: &str, html: String) -> Result<DocumentPayload> {
    init(data_dir)?;
    let manifest = DocumentManifest {
        schema_version: SCHEMA_VERSION,
        id: Uuid::now_v7().simple().to_string(),
        kind: DocumentKind::Html,
        title: non_empty_title(title)?,
        status: "ready".into(),
        revision: 1,
        updated_at: Utc::now(),
        qa_status: "passed".into(),
        runtime: DocumentRuntime::Archify,
        origin: None,
    };
    let content = normalize_content(DocumentKind::Html, json!({ "html": html }), true)?;
    let directory = document_dir(data_dir, &manifest.id)?;
    fs::create_dir_all(directory.join("assets"))?;
    write_document_files(&directory, &manifest, &content)?;
    upsert_metadata(data_dir, &manifest)?;
    Ok(DocumentPayload { manifest, content })
}

pub fn create_delegated_markdown(
    data_dir: &Path,
    title: &str,
    markdown: &str,
    origin: DocumentOrigin,
) -> Result<DocumentPayload> {
    validate_origin(&origin)?;
    let digest = format!("{:x}", Sha256::digest(origin.attempt_id.as_bytes()));
    let id = format!("taskdoc-{}", &digest[..32]);
    let directory = document_dir(data_dir, &id)?;
    if directory.exists() {
        let existing = read(data_dir, &id)?;
        if existing.manifest.origin.as_ref() == Some(&origin) {
            upsert_metadata(data_dir, &existing.manifest)?;
            return Ok(existing);
        }
        bail!("delegated document id collision");
    }

    init(data_dir)?;
    let manifest = DocumentManifest {
        schema_version: SCHEMA_VERSION,
        id,
        kind: DocumentKind::Markdown,
        title: non_empty_title(title)?,
        status: "ready".into(),
        revision: 1,
        updated_at: Utc::now(),
        qa_status: "passed".into(),
        runtime: DocumentRuntime::Delegated,
        origin: Some(origin),
    };
    let content = normalize_content(
        DocumentKind::Markdown,
        json!({ "markdown": markdown }),
        false,
    )?;
    let temporary = documents_root(data_dir).join(format!(
        ".{}-materializing-{}",
        manifest.id,
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(temporary.join("assets"))?;
    if let Err(error) = write_document_files(&temporary, &manifest, &content)
        .and_then(|()| fs::rename(&temporary, &directory).map_err(Into::into))
    {
        let _ = fs::remove_dir_all(&temporary);
        if let Ok(existing) = read(data_dir, &manifest.id) {
            if existing.manifest.origin == manifest.origin {
                upsert_metadata(data_dir, &existing.manifest)?;
                return Ok(existing);
            }
        }
        return Err(error);
    }
    upsert_metadata(data_dir, &manifest)?;
    Ok(DocumentPayload { manifest, content })
}

/// Materialize a delegated Markdown result without letting invalid executor
/// output poison the durable outbox forever. Origin validation still fails
/// closed because it is daemon-owned metadata; only executor-authored title
/// and Markdown are replaced by a bounded diagnostic document.
pub fn create_delegated_markdown_or_diagnostic(
    data_dir: &Path,
    title: &str,
    markdown: &str,
    origin: DocumentOrigin,
) -> Result<DocumentPayload> {
    validate_origin(&origin)?;
    let validation_error = non_empty_title(title)
        .and_then(|_| validate_markdown(markdown))
        .err();
    if let Some(error) = validation_error {
        let safe_title = title
            .replace('\0', "�")
            .trim()
            .chars()
            .take(120)
            .collect::<String>();
        let safe_title = if safe_title.is_empty() {
            "委派任务产出".to_string()
        } else {
            safe_title
        };
        let preview = markdown
            .replace('\0', "�")
            .chars()
            .take(4_000)
            .collect::<String>();
        let preview = if preview.trim().is_empty() {
            "（执行器未返回正文）".to_string()
        } else {
            preview
        };
        return create_delegated_markdown(
            data_dir,
            &format!("{safe_title}（产出诊断）"),
            &format!(
                "# 产出整理失败\n\n执行器返回的 Markdown 无法作为正式文档导入，系统已保留诊断信息。\n\n## 校验错误\n\n`{}`\n\n## 原始内容预览\n\n```text\n{}\n```",
                error.to_string().replace('`', "'"),
                preview.replace("```", "` ` `")
            ),
            origin,
        );
    }
    create_delegated_markdown(data_dir, title, markdown, origin)
}

fn normalize_content(
    kind: DocumentKind,
    value: Value,
    allow_archify_runtime: bool,
) -> Result<Value> {
    match kind {
        DocumentKind::Html => normalize_html_content(value, allow_archify_runtime),
        DocumentKind::Markdown => normalize_markdown_content(value),
    }
}

fn normalize_html_content(value: Value, allow_archify_runtime: bool) -> Result<Value> {
    let object = value
        .as_object()
        .context("HTML document content must be an object")?;
    if object.keys().any(|key| key != "html") {
        bail!("HTML document content only supports the /html field");
    }
    let html = object
        .get("html")
        .and_then(Value::as_str)
        .context("HTML document content requires /html")?;
    validate_html(html, allow_archify_runtime)?;
    Ok(json!({ "html": html }))
}

fn normalize_markdown_content(value: Value) -> Result<Value> {
    let object = value
        .as_object()
        .context("Markdown document content must be an object")?;
    if object.keys().any(|key| key != "markdown") {
        bail!("Markdown document content only supports the /markdown field");
    }
    let markdown = object
        .get("markdown")
        .and_then(Value::as_str)
        .context("Markdown document content requires /markdown")?;
    validate_markdown(markdown)?;
    Ok(json!({ "markdown": markdown }))
}

fn validate_html(html: &str, allow_archify_runtime: bool) -> Result<()> {
    if html.trim().is_empty() {
        bail!("HTML document is empty");
    }
    if html.len() > MAX_HTML_BYTES {
        bail!("HTML document exceeds 64 MiB");
    }
    if html.contains('\0') {
        bail!("HTML document contains a NUL byte");
    }
    let lower = html.to_ascii_lowercase();
    if !lower.contains("<html") || !lower.contains("<body") {
        bail!("HTML document must contain <html> and <body>");
    }
    if !allow_archify_runtime && unsafe_html_regex().is_match(html) {
        bail!("HTML document contains scripts, embedded frames, event handlers or a dangerous URL");
    }
    if allow_archify_runtime
        && (!html.contains("Built with Archify")
            || !html.contains("<meta name=\"generator\" content=\"archify ")
            || !html.contains("data-node-id="))
    {
        bail!("trusted Archify document is missing canonical runtime markers");
    }
    Ok(())
}

fn validate_markdown(markdown: &str) -> Result<()> {
    if markdown.trim().is_empty() {
        bail!("Markdown document is empty");
    }
    if markdown.len() > MAX_MARKDOWN_BYTES {
        bail!("Markdown document exceeds 16 MiB");
    }
    if markdown.contains('\0') {
        bail!("Markdown document contains a NUL byte");
    }
    Ok(())
}

fn validate_origin(origin: &DocumentOrigin) -> Result<()> {
    for (name, value) in [
        ("taskId", origin.task_id.as_str()),
        ("attemptId", origin.attempt_id.as_str()),
        ("batchId", origin.batch_id.as_str()),
        ("executorId", origin.executor_id.as_str()),
        ("agentName", origin.agent_name.as_str()),
    ] {
        let value = value.trim();
        if value.is_empty() || value.chars().count() > 256 || value.contains('\0') {
            bail!("document origin {name} must contain 1 to 256 characters");
        }
    }
    Ok(())
}

fn validate_evidence_ledger(ledger: &EvidenceLedger) -> Result<()> {
    if ledger.schema_version != SCHEMA_VERSION {
        bail!("unsupported document evidence schema version");
    }
    validate_id(&ledger.document_id)?;
    if ledger.document_revision == 0 {
        bail!("document evidence revision must be positive");
    }
    if ledger.entries.is_empty() || ledger.entries.len() > MAX_EVIDENCE_ENTRIES {
        bail!("document evidence ledger must contain 1 to {MAX_EVIDENCE_ENTRIES} entries");
    }
    for (index, entry) in ledger.entries.iter().enumerate() {
        validate_evidence_text(&entry.claim, 1000, index, "claim")?;
        validate_evidence_text(&entry.source, 1000, index, "source")?;
        for (field, value, maximum) in [
            ("locator", entry.locator.as_deref(), 500),
            ("excerpt", entry.excerpt.as_deref(), 2000),
            ("formula", entry.formula.as_deref(), 1000),
            ("limitations", entry.limitations.as_deref(), 1000),
        ] {
            if let Some(value) = value {
                validate_evidence_text(value, maximum, index, field)?;
            }
        }
        if entry.basis == EvidenceBasis::Calculation && entry.formula.is_none() {
            bail!("evidence entries/{index}/formula is required for a calculation");
        }
    }
    if serde_json::to_vec(ledger)?.len() > MAX_EVIDENCE_BYTES {
        bail!("document evidence ledger exceeds 512 KiB");
    }
    Ok(())
}

fn validate_evidence_text(value: &str, maximum: usize, index: usize, field: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > maximum {
        bail!("evidence entries/{index}/{field} must contain 1 to {maximum} characters");
    }
    Ok(())
}

fn unsafe_html_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"(?is)<\s*(?:script|iframe|object|embed)\b|\son[a-z]+\s*=|javascript\s*:|data\s*:\s*text/html"#,
        )
        .expect("valid unsafe HTML regex")
    })
}

fn default_content(kind: DocumentKind) -> Value {
    match kind {
        DocumentKind::Html => json!({
            "html": "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><style>body{font-family:system-ui,sans-serif;margin:0;padding:48px;color:#172033;background:#fff}main{max-width:960px;margin:auto}h1{font-size:42px;line-height:1.1}</style></head><body><main><h1>新建文档</h1><p>开始编辑，或让 AI 完善内容与版式。</p></main></body></html>"
        }),
        DocumentKind::Markdown => json!({
            "markdown": "# 新建文档\n\n开始整理内容。"
        }),
    }
}

fn open_connection(data_dir: &Path) -> Result<Connection> {
    fs::create_dir_all(data_dir)?;
    let connection = Connection::open(data_dir.join("data.db"))?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    Ok(connection)
}

fn upsert_metadata(data_dir: &Path, manifest: &DocumentManifest) -> Result<()> {
    init(data_dir)?;
    let origin_json = manifest
        .origin
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    open_connection(data_dir)?.execute(
        "INSERT INTO documents
            (id, kind, title, status, revision, updated_at, qa_status, runtime, origin_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
            kind=excluded.kind, title=excluded.title, status=excluded.status,
            revision=excluded.revision, updated_at=excluded.updated_at,
            qa_status=excluded.qa_status, runtime=excluded.runtime,
            origin_json=excluded.origin_json",
        params![
            manifest.id,
            manifest.kind.as_str(),
            manifest.title,
            manifest.status,
            i64::try_from(manifest.revision)?,
            manifest.updated_at.to_rfc3339(),
            manifest.qa_status,
            manifest.runtime.as_str(),
            origin_json,
        ],
    )?;
    Ok(())
}

fn write_document_files(
    directory: &Path,
    manifest: &DocumentManifest,
    content: &Value,
) -> Result<()> {
    atomic_write(
        &directory.join("manifest.json"),
        &serde_json::to_vec_pretty(manifest)?,
    )?;
    let (field, source) = match manifest.kind {
        DocumentKind::Html => ("html", content.get("html").and_then(Value::as_str)),
        DocumentKind::Markdown => ("markdown", content.get("markdown").and_then(Value::as_str)),
    };
    let source = source.with_context(|| format!("document content requires /{field}"))?;
    atomic_write(
        &directory.join(manifest.kind.source_filename()),
        source.as_bytes(),
    )
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|value| value == column) {
        connection.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn non_empty_title(value: &str) -> Result<String> {
    let title = value.trim();
    if title.is_empty() || title.chars().count() > 160 {
        bail!("document title must contain 1 to 160 characters");
    }
    Ok(title.to_owned())
}

fn document_dir(data_dir: &Path, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    Ok(documents_root(data_dir).join(id))
}

fn documents_root(data_dir: &Path) -> PathBuf {
    data_dir.join("artifacts").join("documents")
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 80
        || !id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        bail!("invalid document id");
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    {
        let mut file = File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}

fn safe_image_extension(filename: &str, bytes: &[u8]) -> Result<&'static str> {
    let lower = filename.to_ascii_lowercase();
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") || lower.ends_with(".png") {
        return Ok("png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) || lower.ends_with(".jpg") || lower.ends_with(".jpeg")
    {
        return Ok("jpg");
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") || lower.ends_with(".webp") {
        return Ok("webp");
    }
    bail!("only PNG, JPEG and WebP assets are supported");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_html() -> Value {
        json!({
            "html": "<!doctype html><html><head><style>@page{size:A4}</style></head><body><main><h1>测试</h1><img src=\"assets/chart.png\"></main></body></html>"
        })
    }

    fn sample_evidence() -> EvidenceEntry {
        EvidenceEntry {
            claim: "2024 was the warmest year in the observational record.".into(),
            basis: EvidenceBasis::Source,
            source: "WMO, State of the Global Climate 2024".into(),
            locator: Some("Key messages".into()),
            excerpt: Some("It was the warmest year in the 175-year observational record.".into()),
            formula: None,
            limitations: Some("Calendar-year observation, not long-term warming.".into()),
        }
    }

    #[test]
    fn creates_updates_and_reads_html_document() {
        let data = tempfile::tempdir().unwrap();
        let created = create(
            data.path(),
            CreateDocument {
                kind: DocumentKind::Html,
                title: "HTML report".into(),
                content: Some(sample_html()),
            },
        )
        .unwrap();
        assert_eq!(created.manifest.revision, 1);
        assert_eq!(created.content["html"], sample_html()["html"]);

        let updated = update(
            data.path(),
            &created.manifest.id,
            UpdateDocument {
                expected_revision: 1,
                title: None,
                content: json!({
                    "html": "<!doctype html><html><body><h1>修订</h1></body></html>"
                }),
                status: None,
                qa_status: Some("passed".into()),
            },
        )
        .unwrap();
        assert_eq!(updated.manifest.revision, 2);
        assert_eq!(updated.manifest.qa_status, "pending");
        assert!(
            read(data.path(), &created.manifest.id).unwrap().content["html"]
                .as_str()
                .unwrap()
                .contains("修订")
        );
    }

    #[test]
    fn rejects_old_kinds_and_content_for_the_wrong_kind() {
        assert!(serde_json::from_value::<CreateDocument>(json!({
            "kind": "report",
            "title": "legacy"
        }))
        .is_err());
        assert!(
            normalize_content(DocumentKind::Html, json!({ "markdown": "# legacy" }), false)
                .is_err()
        );
        assert!(normalize_content(
            DocumentKind::Html,
            json!({ "html": "<p>fragment</p>" }),
            false
        )
        .is_err());
        assert!(normalize_content(
            DocumentKind::Html,
            json!({
                "html": "<!doctype html><html><body><img src=x onerror=alert(1)></body></html>"
            }),
            false
        )
        .is_err());
    }

    #[test]
    fn creates_updates_reads_and_lists_markdown_document() {
        let data = tempfile::tempdir().unwrap();
        let created = create(
            data.path(),
            CreateDocument {
                kind: DocumentKind::Markdown,
                title: "Markdown report".into(),
                content: Some(json!({ "markdown": "# 结论\n\n第一版。" })),
            },
        )
        .unwrap();
        assert_eq!(created.manifest.kind, DocumentKind::Markdown);
        assert_eq!(created.content["markdown"], "# 结论\n\n第一版。");
        assert!(data
            .path()
            .join("artifacts/documents")
            .join(&created.manifest.id)
            .join("document.md")
            .is_file());

        let updated = update(
            data.path(),
            &created.manifest.id,
            UpdateDocument {
                expected_revision: 1,
                title: None,
                content: json!({ "markdown": "# 结论\n\n第二版。" }),
                status: Some("ready".into()),
                qa_status: None,
            },
        )
        .unwrap();
        assert_eq!(updated.manifest.revision, 2);
        assert_eq!(
            read(data.path(), &created.manifest.id).unwrap().content["markdown"],
            "# 结论\n\n第二版。"
        );

        let listed = list(data.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].kind, DocumentKind::Markdown);
        assert_eq!(listed[0].runtime, DocumentRuntime::Static);
    }

    #[test]
    fn materializes_delegated_markdown_idempotently_and_keeps_it_read_only() {
        let data = tempfile::tempdir().unwrap();
        let origin = DocumentOrigin {
            origin_type: DocumentOriginType::RuntimeTask,
            task_id: "task_1".into(),
            attempt_id: "attempt_1".into(),
            batch_id: "batch_1".into(),
            executor_id: "codex".into(),
            agent_name: "Alex".into(),
        };
        let first = create_delegated_markdown(
            data.path(),
            "任务结果",
            "# 任务结果\n\n已完成。",
            origin.clone(),
        )
        .unwrap();
        let replay = create_delegated_markdown(
            data.path(),
            "不会覆盖已有标题",
            "# 也不会覆盖正文",
            origin.clone(),
        )
        .unwrap();
        assert_eq!(first.manifest.id, replay.manifest.id);
        assert_eq!(replay.content, first.content);
        assert_eq!(first.manifest.runtime, DocumentRuntime::Delegated);
        assert_eq!(first.manifest.origin, Some(origin));
        assert_eq!(list(data.path()).unwrap()[0].origin, first.manifest.origin);

        let error = update(
            data.path(),
            &first.manifest.id,
            UpdateDocument {
                expected_revision: 1,
                title: None,
                content: json!({ "markdown": "# 非法修改" }),
                status: None,
                qa_status: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("read-only"));
    }

    #[test]
    fn invalid_delegated_markdown_becomes_a_diagnostic_document() {
        let data = tempfile::tempdir().unwrap();
        let payload = create_delegated_markdown_or_diagnostic(
            data.path(),
            "任务结果",
            " \0 ",
            DocumentOrigin {
                origin_type: DocumentOriginType::RuntimeTask,
                task_id: "task_invalid".into(),
                attempt_id: "attempt_invalid".into(),
                batch_id: "batch_invalid".into(),
                executor_id: "codex".into(),
                agent_name: "Alex".into(),
            },
        )
        .unwrap();

        assert!(payload.manifest.title.contains("产出诊断"));
        assert!(payload.content["markdown"]
            .as_str()
            .unwrap()
            .contains("产出整理失败"));
        assert_eq!(payload.manifest.runtime, DocumentRuntime::Delegated);
    }

    #[test]
    fn reads_legacy_manifest_without_runtime_metadata() {
        let manifest: DocumentManifest = serde_json::from_value(json!({
            "schemaVersion": 1,
            "id": "legacy-document",
            "kind": "html",
            "title": "Legacy",
            "status": "draft",
            "revision": 1,
            "updatedAt": "2026-01-01T00:00:00Z",
            "qaStatus": "pending"
        }))
        .unwrap();
        assert_eq!(manifest.runtime, DocumentRuntime::Static);
        assert!(manifest.origin.is_none());
    }

    #[test]
    fn archify_runtime_is_trusted_only_through_the_internal_creator() {
        let data = tempfile::tempdir().unwrap();
        let diagram: Value = serde_json::from_str(include_str!(
            "../resources/archify/examples/web-app.architecture.json"
        ))
        .unwrap();
        let html = crate::tools::archify::render_html(&diagram).unwrap();
        let created = create_archify(data.path(), "Architecture", html.clone()).unwrap();
        assert_eq!(created.manifest.runtime, DocumentRuntime::Archify);
        assert_eq!(
            read(data.path(), &created.manifest.id).unwrap().content["html"],
            html
        );
        let error = update(
            data.path(),
            &created.manifest.id,
            UpdateDocument {
                expected_revision: 1,
                title: None,
                content: json!({ "html": html }),
                status: None,
                qa_status: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("read-only"));
    }

    #[test]
    fn revision_conflicts_do_not_overwrite() {
        let data = tempfile::tempdir().unwrap();
        let created = create(
            data.path(),
            CreateDocument {
                kind: DocumentKind::Html,
                title: "Conflict".into(),
                content: Some(sample_html()),
            },
        )
        .unwrap();
        let error = update(
            data.path(),
            &created.manifest.id,
            UpdateDocument {
                expected_revision: 9,
                title: None,
                content: sample_html(),
                status: None,
                qa_status: None,
            },
        )
        .unwrap_err();
        assert!(error.downcast_ref::<RevisionConflict>().is_some());
    }

    #[test]
    fn evidence_ledger_is_current_then_stale_after_document_update() {
        let data = tempfile::tempdir().unwrap();
        let created = create(
            data.path(),
            CreateDocument {
                kind: DocumentKind::Html,
                title: "Evidence".into(),
                content: Some(sample_html()),
            },
        )
        .unwrap();
        assert_eq!(
            read_evidence_ledger(data.path(), &created.manifest.id)
                .unwrap()
                .status,
            "missing"
        );
        replace_evidence_ledger(
            data.path(),
            &created.manifest.id,
            1,
            vec![sample_evidence()],
        )
        .unwrap();
        let current = read_evidence_ledger(data.path(), &created.manifest.id).unwrap();
        assert_eq!(current.status, "current");
        assert_eq!(current.ledger.unwrap().entries, vec![sample_evidence()]);

        update(
            data.path(),
            &created.manifest.id,
            UpdateDocument {
                expected_revision: 1,
                title: None,
                content: json!({
                    "html": "<!doctype html><html><body><h1>Revision two</h1></body></html>"
                }),
                status: None,
                qa_status: None,
            },
        )
        .unwrap();
        assert_eq!(
            read_evidence_ledger(data.path(), &created.manifest.id)
                .unwrap()
                .status,
            "stale"
        );
    }

    #[test]
    fn evidence_ledger_checks_revision_and_calculation_formula() {
        let data = tempfile::tempdir().unwrap();
        let created = create(
            data.path(),
            CreateDocument {
                kind: DocumentKind::Html,
                title: "Evidence validation".into(),
                content: Some(sample_html()),
            },
        )
        .unwrap();
        let conflict = replace_evidence_ledger(
            data.path(),
            &created.manifest.id,
            9,
            vec![sample_evidence()],
        )
        .unwrap_err();
        assert!(conflict.downcast_ref::<RevisionConflict>().is_some());

        let mut calculation = sample_evidence();
        calculation.basis = EvidenceBasis::Calculation;
        calculation.formula = None;
        assert!(
            replace_evidence_ledger(data.path(), &created.manifest.id, 1, vec![calculation],)
                .unwrap_err()
                .to_string()
                .contains("formula is required")
        );
    }

    #[test]
    fn export_inlines_assets_and_imports_html() {
        let data = tempfile::tempdir().unwrap();
        let created = create(
            data.path(),
            CreateDocument {
                kind: DocumentKind::Html,
                title: "Package".into(),
                content: Some(sample_html()),
            },
        )
        .unwrap();
        let image = [
            0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 0, 0, 0,
        ];
        let asset = store_asset(data.path(), &created.manifest.id, "chart.png", &image).unwrap();
        assert_eq!(asset.split('/').next(), Some("assets"));
        let with_asset = update(
            data.path(),
            &created.manifest.id,
            UpdateDocument {
                expected_revision: 1,
                title: None,
                content: json!({
                    "html": sample_html()["html"].as_str().unwrap().replace("assets/chart.png", &asset)
                }),
                status: None,
                qa_status: None,
            },
        )
        .unwrap();
        let (_, exported) = export_html(data.path(), &with_asset.manifest.id).unwrap();
        assert!(exported.contains("data:image/png;base64,"));
        assert!(!exported.contains("assets/"));
        let imported = import_html(data.path(), "Imported", exported.as_bytes()).unwrap();
        assert_eq!(imported.manifest.kind, DocumentKind::Html);
        assert_ne!(imported.manifest.id, created.manifest.id);
        assert_eq!(imported.content["html"], exported);
    }
}
