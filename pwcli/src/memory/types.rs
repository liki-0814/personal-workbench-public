// 用户记忆条目与索引行；schema v2 增加 id / supersedes / deleted_at
use serde::{Deserialize, Serialize};

use crate::memory::id::new_entry_id;

pub const CURRENT_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySource {
    pub kind: String,
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryEntry {
    pub slug: String,
    pub summary: String,
    pub content: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// 稳定 UUID（schema v2）；旧条目 migrate 后补齐
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 本条取代的前序 slug（ADD-only 分配 foo_2 时指向 foo）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    /// 软删时间戳；有值则不在活跃索引/检索中
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<i64>,
    /// preference / fact / decision / project / knowledge.
    #[serde(default = "default_memory_kind")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<MemorySource>,
}

impl MemoryEntry {
    pub fn is_active(&self) -> bool {
        self.deleted_at.is_none()
    }

    pub fn new_active(
        slug: String,
        summary: String,
        content: String,
        now: i64,
        supersedes: Option<String>,
    ) -> Self {
        Self {
            slug,
            summary,
            content,
            created_at: now,
            updated_at: now,
            id: Some(new_entry_id()),
            supersedes,
            deleted_at: None,
            kind: default_memory_kind(),
            tags: Vec::new(),
            sources: Vec::new(),
        }
    }

    pub fn ensure_id(&mut self) {
        if self.id.is_none() {
            self.id = Some(new_entry_id());
        }
    }

    pub fn with_source(mut self, source: MemorySource) -> Self {
        self.sources.push(source);
        self
    }
}

fn default_memory_kind() -> String {
    "knowledge".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MemoryIndex {
    #[serde(default)]
    pub entries: Vec<MemoryIndexLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryIndexLine {
    pub slug: String,
    pub summary: String,
    pub updated_at: i64,
}
