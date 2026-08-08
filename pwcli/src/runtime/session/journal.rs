use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{ContentBlock, ConversationMessage, MessageRole};

pub const JOURNAL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalHeader {
    #[serde(rename = "type")]
    pub kind: String,
    pub version: u32,
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub cwd: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub timestamp: DateTime<Utc>,
    #[serde(flatten)]
    pub data: JournalEntryData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JournalEntryData {
    Message {
        message: ConversationMessage,
    },
    ModelChange {
        provider: String,
        model_id: String,
    },
    ThinkingLevelChange {
        thinking_level: String,
    },
    ActiveToolsChange {
        active_tool_names: Vec<String>,
    },
    Compaction {
        summary: String,
        first_kept_entry_id: Uuid,
        tokens_before: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
    },
    BranchSummary {
        summary: String,
        from_id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
    },
    Custom {
        custom_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Label {
        target_id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    SessionInfo {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Leaf {
        leaf_id: Option<Uuid>,
    },
}

#[derive(Debug)]
pub struct SessionJournal {
    path: PathBuf,
    header: JournalHeader,
    entries: Vec<JournalEntry>,
    by_id: HashMap<Uuid, usize>,
    leaf_id: Option<Uuid>,
}

impl SessionJournal {
    pub fn create(
        path: impl Into<PathBuf>,
        cwd: impl Into<PathBuf>,
        parent_session: Option<PathBuf>,
    ) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("create session journal directory {}", parent.display())
            })?;
        }
        let header = JournalHeader {
            kind: "session".to_string(),
            version: JOURNAL_VERSION,
            id: Uuid::now_v7(),
            timestamp: Utc::now(),
            cwd: cwd.into(),
            parent_session,
            metadata: serde_json::Map::new(),
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("create session journal {}", path.display()))?;
        writeln!(file, "{}", serde_json::to_string(&header)?)?;
        file.sync_data()?;
        Ok(Self {
            path,
            header,
            entries: Vec::new(),
            by_id: HashMap::new(),
            leaf_id: None,
        })
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let file = File::open(&path)
            .with_context(|| format!("open session journal {}", path.display()))?;
        let mut lines = BufReader::new(file).lines().enumerate();
        let (_, header_line) = lines.next().context("session journal is empty")?;
        let header: JournalHeader =
            serde_json::from_str(&header_line?).context("parse session journal header")?;
        if header.kind != "session" {
            bail!("invalid session journal header type: {}", header.kind);
        }
        if header.version != JOURNAL_VERSION {
            bail!(
                "unsupported session journal version {}, expected {}",
                header.version,
                JOURNAL_VERSION
            );
        }

        let raw = fs::read_to_string(&path)?;
        let has_trailing_newline = raw.ends_with('\n');
        let line_count = raw.lines().count();
        let mut entries = Vec::new();
        let mut by_id = HashMap::new();
        let mut leaf_id = None;
        for (line_index, line) in raw.lines().enumerate().skip(1) {
            if line.trim().is_empty() {
                continue;
            }
            let entry: JournalEntry = match serde_json::from_str(line) {
                Ok(entry) => entry,
                Err(_error) if line_index + 1 == line_count && !has_trailing_newline => {
                    break;
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("parse session journal line {}", line_index + 1));
                }
            };
            if by_id.insert(entry.id, entries.len()).is_some() {
                bail!("duplicate session journal entry id {}", entry.id);
            }
            match &entry.data {
                JournalEntryData::Leaf { leaf_id: explicit } => leaf_id = *explicit,
                _ => leaf_id = Some(entry.id),
            }
            entries.push(entry);
        }

        Ok(Self {
            path,
            header,
            entries,
            by_id,
            leaf_id,
        })
    }

    pub fn header(&self) -> &JournalHeader {
        &self.header
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }

    pub fn leaf_id(&self) -> Option<Uuid> {
        self.leaf_id
    }

    pub fn get_entry(&self, id: Uuid) -> Option<&JournalEntry> {
        self.by_id.get(&id).map(|index| &self.entries[*index])
    }

    pub fn append(&mut self, data: JournalEntryData) -> Result<Uuid> {
        let entry = JournalEntry {
            id: Uuid::now_v7(),
            parent_id: self.leaf_id,
            timestamp: Utc::now(),
            data,
        };
        self.append_entry(entry)
    }

    pub fn append_message(&mut self, message: ConversationMessage) -> Result<Uuid> {
        self.append(JournalEntryData::Message { message })
    }

    pub fn move_to(&mut self, leaf_id: Option<Uuid>) -> Result<Uuid> {
        if let Some(id) = leaf_id {
            if !self.by_id.contains_key(&id) {
                bail!("session journal entry {} not found", id);
            }
        }
        self.append(JournalEntryData::Leaf { leaf_id })
    }

    pub fn path_to_root(&self, leaf_id: Option<Uuid>) -> Result<Vec<&JournalEntry>> {
        let mut path = Vec::new();
        let mut current = leaf_id.or(self.leaf_id);
        while let Some(id) = current {
            let entry = self
                .get_entry(id)
                .with_context(|| format!("session journal entry {} not found", id))?;
            path.push(entry);
            current = entry.parent_id;
        }
        path.reverse();
        Ok(path)
    }

    pub fn active_messages(&self) -> Result<Vec<ConversationMessage>> {
        let mut messages: Vec<(Uuid, ConversationMessage)> = Vec::new();
        for entry in self.path_to_root(None)? {
            match &entry.data {
                JournalEntryData::Message { message } => messages.push((entry.id, message.clone())),
                JournalEntryData::Compaction {
                    summary,
                    first_kept_entry_id,
                    details,
                    ..
                } => {
                    let kept_from = messages
                        .iter()
                        .position(|(id, _)| id == first_kept_entry_id)
                        .unwrap_or(messages.len());
                    let kept = messages.split_off(kept_from);
                    let summary_message = details
                        .as_ref()
                        .and_then(|value| {
                            value
                                .get("summaryMessage")
                                .or_else(|| value.get("summary_message"))
                        })
                        .and_then(|value| serde_json::from_value(value.clone()).ok())
                        .unwrap_or_else(|| ConversationMessage {
                            id: format!("journal_compaction_{}", entry.id),
                            role: MessageRole::System,
                            content: vec![ContentBlock::Text {
                                text: summary.clone(),
                            }],
                            created_at: entry.timestamp,
                            parent_id: None,
                            model: None,
                            token_usage: None,
                        });
                    messages = vec![(entry.id, summary_message)];
                    messages.extend(kept);
                }
                JournalEntryData::BranchSummary { summary, .. } => {
                    messages.push((
                        entry.id,
                        ConversationMessage {
                            id: format!("msg_branch_summary_{}", entry.id),
                            role: MessageRole::System,
                            content: vec![ContentBlock::Text {
                                text: format!("【分支摘要】{summary}"),
                            }],
                            created_at: entry.timestamp,
                            parent_id: None,
                            model: None,
                            token_usage: None,
                        },
                    ));
                }
                _ => {}
            }
        }
        Ok(messages.into_iter().map(|(_, message)| message).collect())
    }

    pub fn fork(&self, path: impl Into<PathBuf>, leaf_id: Option<Uuid>) -> Result<Self> {
        let mut fork =
            SessionJournal::create(path, self.header.cwd.clone(), Some(self.path.clone()))?;
        let messages = if leaf_id.is_some() {
            let mut selected = Vec::new();
            for entry in self.path_to_root(leaf_id)? {
                if let JournalEntryData::Message { message } = &entry.data {
                    selected.push(message.clone());
                }
            }
            selected
        } else {
            self.active_messages()?
        };
        for message in messages {
            fork.append_message(message)?;
        }
        Ok(fork)
    }

    pub fn append_branch_summary(
        &mut self,
        summary: impl Into<String>,
        from_id: Uuid,
        details: Option<Value>,
    ) -> Result<Uuid> {
        if !self.by_id.contains_key(&from_id) {
            bail!("branch summary source {} not found", from_id);
        }
        self.append(JournalEntryData::BranchSummary {
            summary: summary.into(),
            from_id,
            details,
        })
    }

    fn append_entry(&mut self, entry: JournalEntry) -> Result<Uuid> {
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .with_context(|| format!("append session journal {}", self.path.display()))?;
        file.lock_exclusive()?;
        let write_result = (|| -> Result<()> {
            writeln!(file, "{}", serde_json::to_string(&entry)?)?;
            file.sync_data()?;
            Ok(())
        })();
        let unlock_result = file.unlock();
        write_result?;
        unlock_result?;

        let id = entry.id;
        match &entry.data {
            JournalEntryData::Leaf { leaf_id } => self.leaf_id = *leaf_id,
            _ => self.leaf_id = Some(id),
        }
        self.by_id.insert(id, self.entries.len());
        self.entries.push(entry);
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_reload_and_build_active_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut journal = SessionJournal::create(&path, dir.path(), None).unwrap();
        let first = journal
            .append_message(ConversationMessage::new_user("first"))
            .unwrap();
        let second = journal
            .append_message(ConversationMessage::new_assistant("second"))
            .unwrap();
        assert_eq!(journal.path_to_root(None).unwrap().len(), 2);

        journal.move_to(Some(first)).unwrap();
        let branch = journal
            .append_message(ConversationMessage::new_user("branch"))
            .unwrap();
        let reloaded = SessionJournal::open(&path).unwrap();
        assert_eq!(reloaded.leaf_id(), Some(branch));
        let active = reloaded.path_to_root(None).unwrap();
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].id, first);
        assert_eq!(active[1].id, branch);
        assert!(reloaded.get_entry(second).is_some());
    }

    #[test]
    fn ignores_partial_trailing_line_but_not_corruption_in_the_middle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut journal = SessionJournal::create(&path, dir.path(), None).unwrap();
        journal
            .append_message(ConversationMessage::new_user("safe"))
            .unwrap();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        write!(file, "{{\"type\":\"message\"").unwrap();
        file.flush().unwrap();
        assert_eq!(SessionJournal::open(&path).unwrap().entries().len(), 1);

        writeln!(file).unwrap();
        file.flush().unwrap();
        assert!(SessionJournal::open(&path).is_err());
    }

    #[test]
    fn move_to_rejects_unknown_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut journal = SessionJournal::create(&path, dir.path(), None).unwrap();
        assert!(journal.move_to(Some(Uuid::now_v7())).is_err());
    }

    #[test]
    fn branch_summary_is_materialized_on_target_branch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("branch-summary.jsonl");
        let mut journal = SessionJournal::create(&path, dir.path(), None).unwrap();
        let root = journal
            .append_message(ConversationMessage::new_user("root"))
            .unwrap();
        let from = journal
            .append_message(ConversationMessage::new_assistant("old branch"))
            .unwrap();
        journal.move_to(Some(root)).unwrap();
        journal
            .append_branch_summary("old branch summary", from, None)
            .unwrap();
        let active = journal.active_messages().unwrap();
        assert_eq!(active.len(), 2);
        assert!(active[1].text_content().contains("old branch summary"));
    }

    #[test]
    fn fork_clones_selected_context_and_tracks_parent_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.jsonl");
        let mut source = SessionJournal::create(&path, dir.path(), None).unwrap();
        source
            .append_message(ConversationMessage::new_user("one"))
            .unwrap();
        source
            .append_message(ConversationMessage::new_assistant("two"))
            .unwrap();
        let fork = source.fork(dir.path().join("fork.jsonl"), None).unwrap();
        assert_eq!(
            fork.header().parent_session.as_deref(),
            Some(path.as_path())
        );
        assert_eq!(fork.active_messages().unwrap().len(), 2);
    }
}
