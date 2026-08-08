// MemoryStore: per-user 文件型记忆存储；原子写 + fs2 独占锁 + 三道硬约束闸
use crate::runtime::memory::types::{MemoryEntry, MemoryIndex, MemoryIndexLine};
use crate::runtime::settings::RuntimeConfig;
use chrono::Local;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub const MAX_SUMMARY_CHARS: usize = 150;
pub const MAX_ENTRY_BYTES: usize = 16 * 1024;
pub const MAX_INDEX_BYTES: usize = 256 * 1024;
pub const MAX_PROFILE_BYTES: usize = 4 * 1024;
const MAX_PENDING_TURNS: usize = 20;

#[derive(Debug)]
pub enum MemoryError {
    SummaryTooLong { got: usize, max: usize },
    EntryTooLarge { got: usize, max: usize },
    IndexFull { got: usize, max: usize },
    ProfileTooLarge { got: usize, max: usize },
    Io(io::Error),
}

impl std::fmt::Display for MemoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryError::SummaryTooLong { got, max } => {
                write!(f, "summary too long: {} chars (max {})", got, max)
            }
            MemoryError::EntryTooLarge { got, max } => {
                write!(f, "entry too large: {} bytes (max {})", got, max)
            }
            MemoryError::IndexFull { got, max } => {
                write!(f, "index too large: {} bytes (max {})", got, max)
            }
            MemoryError::ProfileTooLarge { got, max } => {
                write!(f, "profile too large: {} bytes (max {})", got, max)
            }
            MemoryError::Io(e) => write!(f, "io error: {}", e),
        }
    }
}

impl std::error::Error for MemoryError {}

impl From<io::Error> for MemoryError {
    fn from(e: io::Error) -> Self {
        MemoryError::Io(e)
    }
}

pub type MemoryResult<T> = Result<T, MemoryError>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryMeta {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub last_compact_at: Option<i64>,
    #[serde(default)]
    pub failed_compact_count: u32,
    #[serde(default)]
    pub disabled_until: Option<i64>,
    #[serde(default)]
    pub last_auto_compact_date: Option<String>,
    #[serde(default)]
    pub failed_extract_count: u32,
    #[serde(default)]
    pub disabled_extract_until: Option<i64>,
    #[serde(default)]
    pub seen_content_hashes: Vec<String>,
    #[serde(default)]
    pub last_dream_at: Option<i64>,
    #[serde(default)]
    pub last_dream_project: Option<String>,
}

fn default_schema_version() -> u32 {
    crate::runtime::memory::types::CURRENT_SCHEMA_VERSION
}

pub struct MemoryStore {
    base_dir: PathBuf,
    lock_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMemoryTurn {
    pub text: String,
    pub queued_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PendingMemoryQueue {
    #[serde(default)]
    turns: Vec<PendingMemoryTurn>,
}

pub struct MemoryMaintenanceGuard {
    file: File,
}

#[cfg(test)]
std::thread_local! {
    static TEST_MEMORY_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[derive(Debug, Clone)]
pub struct ArchivedEntryRef {
    pub slug: String,
    pub archive_ts: String,
    pub path: PathBuf,
}

impl MemoryStore {
    pub fn new(user_slug: &str) -> MemoryResult<Self> {
        #[cfg(test)]
        let base = TEST_MEMORY_ROOT
            .with(|slot| slot.borrow().as_ref().map(|root| root.join(user_slug)))
            .unwrap_or_else(|| RuntimeConfig::config_dir().join("memory").join(user_slug));
        #[cfg(not(test))]
        let base = RuntimeConfig::config_dir().join("memory").join(user_slug);
        Self::new_with_dir(base)
    }

    /// 测试专用：当前线程的记忆根目录（`memory/<user_slug>` 的父路径）
    #[cfg(test)]
    pub fn set_test_memory_root(root: Option<PathBuf>) {
        TEST_MEMORY_ROOT.with(|slot| *slot.borrow_mut() = root);
    }

    // 测试入口：允许指向任意 base 目录（不依赖 ~/.pwcli）
    pub fn new_with_dir(base_dir: PathBuf) -> MemoryResult<Self> {
        fs::create_dir_all(base_dir.join("entries"))?;
        fs::create_dir_all(base_dir.join("archive"))?;
        let lock_path = base_dir.join(".lock");
        Ok(Self {
            base_dir,
            lock_path,
        })
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    fn index_path(&self) -> PathBuf {
        self.base_dir.join("MEMORY.md")
    }

    fn entry_path(&self, slug: &str) -> PathBuf {
        self.base_dir.join("entries").join(format!("{}.md", slug))
    }

    fn meta_path(&self) -> PathBuf {
        self.base_dir.join(".meta.json")
    }

    fn profile_path(&self) -> PathBuf {
        self.base_dir.join("PROFILE.md")
    }

    fn pending_path(&self) -> PathBuf {
        self.base_dir.join(".pending-extraction.json")
    }

    fn maintenance_lock_path(&self) -> PathBuf {
        self.base_dir.join(".maintenance.lock")
    }

    // 取独占锁：所有公开写 API 入口持有；返回的 LockGuard 在 drop 时自动释放
    fn acquire_lock(&self) -> MemoryResult<LockGuard> {
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(&self.lock_path)?;
        f.lock_exclusive()?;
        Ok(LockGuard { file: f })
    }

    pub fn try_acquire_maintenance(&self) -> MemoryResult<Option<MemoryMaintenanceGuard>> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(self.maintenance_lock_path())?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(MemoryMaintenanceGuard { file })),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn enqueue_pending_turn(&self, text: String, queued_at: i64) -> MemoryResult<usize> {
        let _guard = self.acquire_lock()?;
        let path = self.pending_path();
        let mut queue = if path.exists() {
            serde_json::from_slice::<PendingMemoryQueue>(&fs::read(&path)?).unwrap_or_default()
        } else {
            PendingMemoryQueue::default()
        };
        if !queue.turns.iter().any(|turn| turn.text == text) {
            queue.turns.push(PendingMemoryTurn { text, queued_at });
        }
        if queue.turns.len() > MAX_PENDING_TURNS {
            let overflow = queue.turns.len() - MAX_PENDING_TURNS;
            queue.turns.drain(..overflow);
        }
        let count = queue.turns.len();
        let body = serde_json::to_vec(&queue).map_err(|error| {
            MemoryError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                error.to_string(),
            ))
        })?;
        atomic_write(&path, &body)?;
        Ok(count)
    }

    pub fn read_pending_turns(&self) -> MemoryResult<Vec<PendingMemoryTurn>> {
        let path = self.pending_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let queue =
            serde_json::from_slice::<PendingMemoryQueue>(&fs::read(path)?).map_err(|error| {
                MemoryError::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    error.to_string(),
                ))
            })?;
        Ok(queue.turns)
    }

    pub fn acknowledge_pending_turns(&self, count: usize) -> MemoryResult<()> {
        let _guard = self.acquire_lock()?;
        let path = self.pending_path();
        if !path.exists() {
            return Ok(());
        }
        let mut queue =
            serde_json::from_slice::<PendingMemoryQueue>(&fs::read(&path)?).map_err(|error| {
                MemoryError::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    error.to_string(),
                ))
            })?;
        let drained = count.min(queue.turns.len());
        queue.turns.drain(..drained);
        if queue.turns.is_empty() {
            fs::remove_file(path)?;
        } else {
            let body = serde_json::to_vec(&queue).map_err(|error| {
                MemoryError::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    error.to_string(),
                ))
            })?;
            atomic_write(&path, &body)?;
        }
        Ok(())
    }

    pub fn read_index(&self) -> MemoryResult<MemoryIndex> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(MemoryIndex::default());
        }
        let raw = fs::read_to_string(&path)?;
        Ok(parse_index(&raw))
    }

    // Raw MEMORY.md text for prompt injection — skip parsing to keep formatting verbatim.
    pub fn read_index_raw(&self) -> MemoryResult<String> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(String::new());
        }
        Ok(fs::read_to_string(&path)?)
    }

    /// User profile markdown (optional `PROFILE.md` at memory base).
    pub fn read_profile(&self) -> MemoryResult<String> {
        let path = self.profile_path();
        if !path.exists() {
            return Ok(String::new());
        }
        Ok(fs::read_to_string(&path)?)
    }

    pub fn write_index_atomic(&self, idx: &MemoryIndex) -> MemoryResult<()> {
        let _g = self.acquire_lock()?;
        let body = serialize_index(idx);
        if body.len() > MAX_INDEX_BYTES {
            return Err(MemoryError::IndexFull {
                got: body.len(),
                max: MAX_INDEX_BYTES,
            });
        }
        atomic_write(&self.index_path(), body.as_bytes())
    }

    /// Write MEMORY.md verbatim (frontend raw editor); enforces `MAX_INDEX_BYTES`.
    pub fn write_index_raw(&self, content: &str) -> MemoryResult<()> {
        if content.len() > MAX_INDEX_BYTES {
            return Err(MemoryError::IndexFull {
                got: content.len(),
                max: MAX_INDEX_BYTES,
            });
        }
        let _g = self.acquire_lock()?;
        atomic_write(&self.index_path(), content.as_bytes())
    }

    /// Append or update a single index line under lock.
    pub fn upsert_index_line(&self, line: &MemoryIndexLine) -> MemoryResult<()> {
        let _g = self.acquire_lock()?;
        let mut idx = self.read_index()?;
        if let Some(existing) = idx.entries.iter_mut().find(|e| e.slug == line.slug) {
            existing.summary = line.summary.clone();
            existing.updated_at = line.updated_at;
        } else {
            idx.entries.push(line.clone());
        }
        let body = serialize_index(&idx);
        if body.len() > MAX_INDEX_BYTES {
            return Err(MemoryError::IndexFull {
                got: body.len(),
                max: MAX_INDEX_BYTES,
            });
        }
        atomic_write(&self.index_path(), body.as_bytes())
    }

    /// Build a digest of recent memories for prompt injection (newest first).
    pub fn build_digest(&self, max_bytes: usize, max_lines: usize) -> MemoryResult<String> {
        let idx = self.read_index()?;
        if idx.entries.is_empty() {
            return Ok(String::new());
        }
        let mut lines_with_ts: Vec<(i64, String)> = Vec::new();
        for line in &idx.entries {
            let updated_at = self
                .read_entry(&line.slug)
                .map(|e| e.updated_at)
                .unwrap_or(line.updated_at);
            let display = format!("- [{}]({}.md) — {}", line.slug, line.slug, line.summary);
            lines_with_ts.push((updated_at, display));
        }
        lines_with_ts.sort_by(|a, b| b.0.cmp(&a.0));
        let mut out = String::new();
        for (_, line) in lines_with_ts.into_iter().take(max_lines) {
            if !out.is_empty() && out.len() + line.len() + 1 > max_bytes {
                break;
            }
            if out.is_empty() && line.len() > max_bytes {
                break;
            }
            out.push_str(&line);
            out.push('\n');
            if out.len() > max_bytes {
                break;
            }
        }
        Ok(out.trim_end().to_string())
    }

    pub fn read_entry(&self, slug: &str) -> MemoryResult<MemoryEntry> {
        let entry = self.read_entry_raw(slug)?;
        if !entry.is_active() {
            return Err(MemoryError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("entry {} is soft-deleted", slug),
            )));
        }
        Ok(entry)
    }

    /// 读取条目（含软删 tombstone），供迁移/统计用。
    pub fn read_entry_raw(&self, slug: &str) -> MemoryResult<MemoryEntry> {
        let raw = fs::read_to_string(self.entry_path(slug))?;
        parse_entry(slug, &raw).ok_or_else(|| {
            MemoryError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "malformed entry frontmatter",
            ))
        })
    }

    pub fn write_entry_atomic(&self, entry: &MemoryEntry) -> MemoryResult<()> {
        let mut entry = entry.clone();
        entry.ensure_id();
        if entry.summary.chars().count() > MAX_SUMMARY_CHARS {
            return Err(MemoryError::SummaryTooLong {
                got: entry.summary.chars().count(),
                max: MAX_SUMMARY_CHARS,
            });
        }
        let serialized = serialize_entry(&entry);
        if serialized.len() > MAX_ENTRY_BYTES {
            return Err(MemoryError::EntryTooLarge {
                got: serialized.len(),
                max: MAX_ENTRY_BYTES,
            });
        }
        let _g = self.acquire_lock()?;
        atomic_write(&self.entry_path(&entry.slug), serialized.as_bytes())
    }

    pub fn delete_entry(&self, slug: &str) -> MemoryResult<()> {
        let _g = self.acquire_lock()?;
        let path = self.entry_path(slug);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        self.remove_slug_from_index(slug)
    }

    /// 软删：保留 tombstone 文件，从 MEMORY.md 移除。
    pub fn soft_delete_entry(&self, slug: &str, deleted_at: i64) -> MemoryResult<()> {
        let _g = self.acquire_lock()?;
        let path = self.entry_path(slug);
        if !path.exists() {
            return Err(MemoryError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("entry not found: {}", slug),
            )));
        }
        let mut entry = self.read_entry_raw(slug)?;
        entry.deleted_at = Some(deleted_at);
        entry.updated_at = deleted_at;
        entry.ensure_id();
        let serialized = serialize_entry(&entry);
        atomic_write(&path, serialized.as_bytes())?;
        self.remove_slug_from_index(slug)
    }

    fn remove_slug_from_index(&self, slug: &str) -> MemoryResult<()> {
        let idx_path = self.index_path();
        if idx_path.exists() {
            let raw = fs::read_to_string(&idx_path)?;
            let mut idx = parse_index(&raw);
            idx.entries.retain(|e| e.slug != slug);
            let body = serialize_index(&idx);
            atomic_write(&idx_path, body.as_bytes())?;
        }
        Ok(())
    }

    pub fn list_entries(&self) -> MemoryResult<Vec<String>> {
        self.list_active_entries()
    }

    pub fn active_entry_size(&self, slug: &str) -> MemoryResult<usize> {
        Ok(fs::metadata(self.entry_path(slug))?.len() as usize)
    }

    pub fn active_entries_total_size(&self) -> MemoryResult<usize> {
        self.list_active_entries()?
            .iter()
            .try_fold(0usize, |total, slug| {
                self.active_entry_size(slug)
                    .map(|size| total.saturating_add(size))
            })
    }

    /// entries/ 下所有 .md stem（含软删）
    pub fn list_all_entry_slugs(&self) -> MemoryResult<Vec<String>> {
        self.list_entry_stems_in_dir(&self.base_dir.join("entries"))
    }

    pub fn list_active_entries(&self) -> MemoryResult<Vec<String>> {
        let mut out = Vec::new();
        for slug in self.list_all_entry_slugs()? {
            match self.read_entry_raw(&slug) {
                Ok(e) if e.is_active() => out.push(slug),
                _ => {}
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn list_soft_deleted_entries(&self) -> MemoryResult<Vec<String>> {
        let mut out = Vec::new();
        for slug in self.list_all_entry_slugs()? {
            if let Ok(e) = self.read_entry_raw(&slug) {
                if e.deleted_at.is_some() {
                    out.push(slug);
                }
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn list_archived_entries(&self) -> MemoryResult<Vec<ArchivedEntryRef>> {
        let archive_root = self.base_dir.join("archive");
        let mut out = Vec::new();
        if !archive_root.exists() {
            return Ok(out);
        }
        for ts_entry in fs::read_dir(&archive_root)? {
            let ts_entry = ts_entry?;
            if !ts_entry.file_type()?.is_dir() {
                continue;
            }
            let ts = ts_entry.file_name().to_string_lossy().to_string();
            let entries_dir = ts_entry.path().join("entries");
            if !entries_dir.exists() {
                continue;
            }
            for slug in self.list_entry_stems_in_dir(&entries_dir)? {
                out.push(ArchivedEntryRef {
                    slug: slug.clone(),
                    archive_ts: ts.clone(),
                    path: entries_dir.join(format!("{}.md", slug)),
                });
            }
        }
        out.sort_by(|a, b| a.archive_ts.cmp(&b.archive_ts).then(a.slug.cmp(&b.slug)));
        Ok(out)
    }

    pub fn read_archived_entry(path: &Path, slug_hint: &str) -> MemoryResult<MemoryEntry> {
        let raw = fs::read_to_string(path)?;
        parse_entry(slug_hint, &raw).ok_or_else(|| {
            MemoryError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "malformed archived entry",
            ))
        })
    }

    pub fn count_vector_index(&self) -> Option<usize> {
        let db_path = self.base_dir.join("index.db");
        if !db_path.exists() {
            return None;
        }
        crate::runtime::memory::index::MemoryIndexDb::open(&self.base_dir)
            .ok()
            .and_then(|db| db.count_docs().ok())
    }

    /// Fast check: only reads first ~512 bytes of entry to detect `deleted_at:` in frontmatter.
    /// Avoids full-file parse for stats counting.
    pub fn entry_has_deleted_at(&self, slug: &str) -> bool {
        let path = self.entry_path(slug);
        let mut buf = [0u8; 512];
        let Ok(mut f) = File::open(&path) else {
            return false;
        };
        let Ok(n) = f.read(&mut buf) else {
            return false;
        };
        let header = String::from_utf8_lossy(&buf[..n]);
        header.contains("deleted_at:")
    }

    /// Count archived entries without building full ArchivedEntryRef vec.
    pub fn count_archived_entries(&self) -> MemoryResult<usize> {
        let archive_root = self.base_dir.join("archive");
        if !archive_root.exists() {
            return Ok(0);
        }
        let mut count = 0usize;
        for ts_entry in fs::read_dir(&archive_root)? {
            let ts_entry = ts_entry?;
            if !ts_entry.file_type()?.is_dir() {
                continue;
            }
            let entries_dir = ts_entry.path().join("entries");
            if !entries_dir.exists() {
                continue;
            }
            for entry in fs::read_dir(&entries_dir)? {
                let entry = entry?;
                if entry.path().extension().and_then(|s| s.to_str()) == Some("md") {
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    fn list_entry_stems_in_dir(&self, dir: &Path) -> MemoryResult<Vec<String>> {
        let mut out = Vec::new();
        if !dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("md") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    out.push(stem.to_string());
                }
            }
        }
        out.sort();
        Ok(out)
    }

    // 把 entries/ + MEMORY.md 整体快照到 archive/<ts>/，返回快照目录
    pub fn snapshot_to_archive(&self) -> MemoryResult<PathBuf> {
        let _g = self.acquire_lock()?;
        let ts = Local::now().format("%Y%m%dT%H%M%S").to_string();
        let dst = self.base_dir.join("archive").join(&ts);
        fs::create_dir_all(dst.join("entries"))?;
        // 拷贝 MEMORY.md
        let idx_src = self.index_path();
        if idx_src.exists() {
            fs::copy(&idx_src, dst.join("MEMORY.md"))?;
        }
        // 拷贝 entries/*.md
        let entries_src = self.base_dir.join("entries");
        if entries_src.exists() {
            for entry in fs::read_dir(&entries_src)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    let name = path.file_name().unwrap();
                    fs::copy(&path, dst.join("entries").join(name))?;
                }
            }
        }
        Ok(dst)
    }

    pub fn write_profile_atomic(&self, content: &str) -> MemoryResult<()> {
        if content.len() > MAX_PROFILE_BYTES {
            return Err(MemoryError::ProfileTooLarge {
                got: content.len(),
                max: MAX_PROFILE_BYTES,
            });
        }
        let _g = self.acquire_lock()?;
        atomic_write(&self.profile_path(), content.as_bytes())
    }

    /// 将指定 slug 的条目 rename 到 archive 快照目录，并从 MEMORY.md 移除对应行。
    pub fn move_entries_to_archive(&self, slugs: &[String], ts_dir: &Path) -> MemoryResult<usize> {
        if slugs.is_empty() {
            return Ok(0);
        }
        let _g = self.acquire_lock()?;
        fs::create_dir_all(ts_dir.join("entries"))?;
        let slug_set: std::collections::HashSet<&str> = slugs.iter().map(|s| s.as_str()).collect();
        let mut moved = 0usize;
        for slug in slugs {
            let src = self.entry_path(slug);
            if src.exists() {
                let dst = ts_dir.join("entries").join(format!("{}.md", slug));
                fs::rename(&src, &dst)?;
                moved += 1;
            }
        }
        let idx_path = self.index_path();
        if idx_path.exists() {
            let raw = fs::read_to_string(&idx_path)?;
            let mut idx = parse_index(&raw);
            idx.entries.retain(|e| !slug_set.contains(e.slug.as_str()));
            let body = serialize_index(&idx);
            atomic_write(&idx_path, body.as_bytes())?;
        }
        Ok(moved)
    }

    pub fn read_meta(&self) -> MemoryResult<MemoryMeta> {
        let path = self.meta_path();
        if !path.exists() {
            return Ok(MemoryMeta {
                schema_version: 0,
                ..Default::default()
            });
        }
        let raw = fs::read_to_string(&path)?;
        let meta: MemoryMeta = serde_json::from_str(&raw).map_err(|e| {
            MemoryError::Io(io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
        })?;
        Ok(meta)
    }

    pub fn write_meta_atomic(&self, meta: &MemoryMeta) -> MemoryResult<()> {
        let _g = self.acquire_lock()?;
        let body = serde_json::to_vec_pretty(meta).map_err(|e| {
            MemoryError::Io(io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
        })?;
        atomic_write(&self.meta_path(), &body)
    }
}

struct LockGuard {
    file: File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

impl Drop for MemoryMaintenanceGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

// tmp-write + rename：rename 是 POSIX 原子操作，崩溃时旧文件不变
fn atomic_write(target: &Path, bytes: &[u8]) -> MemoryResult<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = target.with_extension(format!(
        "{}.tmp",
        target
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("part")
    ));
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, target)?;
    Ok(())
}

// MEMORY.md 行格式：- [Title](slug.md) — summary
fn serialize_index(idx: &MemoryIndex) -> String {
    let mut s = String::new();
    for line in &idx.entries {
        s.push_str(&format!(
            "- [{}]({}.md) — {}\n",
            line.slug, line.slug, line.summary
        ));
    }
    s
}

fn parse_index(raw: &str) -> MemoryIndex {
    let mut entries = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("- [") {
            continue;
        }
        // - [<slug>](<slug>.md) — <summary>
        let (slug, rest) = match parse_index_line(trimmed) {
            Some(v) => v,
            None => continue,
        };
        entries.push(MemoryIndexLine {
            slug,
            summary: rest,
            updated_at: 0,
        });
    }
    MemoryIndex { entries }
}

fn parse_index_line(line: &str) -> Option<(String, String)> {
    // 形如 "- [slug](slug.md) — summary"
    let after_dash = line.strip_prefix("- [")?;
    let close_bracket = after_dash.find(']')?;
    let slug = &after_dash[..close_bracket];
    let after_paren = after_dash[close_bracket + 1..].strip_prefix("(")?;
    let close_paren = after_paren.find(')')?;
    let after_paren_close = &after_paren[close_paren + 1..];
    // separator 是 " — "（em dash）或 " - "
    let summary = after_paren_close
        .trim_start()
        .strip_prefix("—")
        .or_else(|| after_paren_close.trim_start().strip_prefix('-'))
        .unwrap_or(after_paren_close)
        .trim()
        .to_string();
    Some((slug.to_string(), summary))
}

// frontmatter 序列化：---\nslug:\nsummary:\ncreated_at:\nupdated_at:\n---\n<content>\n
fn serialize_entry(entry: &MemoryEntry) -> String {
    let mut s = String::new();
    s.push_str("---\n");
    s.push_str(&format!("slug: {}\n", entry.slug));
    if let Some(ref id) = entry.id {
        s.push_str(&format!("id: {}\n", id));
    }
    if let Some(ref prev) = entry.supersedes {
        s.push_str(&format!("supersedes: {}\n", prev));
    }
    if let Some(deleted_at) = entry.deleted_at {
        s.push_str(&format!("deleted_at: {}\n", deleted_at));
    }
    s.push_str(&format!("kind: {}\n", entry.kind));
    if !entry.tags.is_empty() {
        s.push_str(&format!(
            "tags_json: {}\n",
            serde_json::to_string(&entry.tags).unwrap_or_default()
        ));
    }
    if !entry.sources.is_empty() {
        s.push_str(&format!(
            "sources_json: {}\n",
            serde_json::to_string(&entry.sources).unwrap_or_default()
        ));
    }
    s.push_str(&format!("summary: {}\n", entry.summary));
    s.push_str(&format!("created_at: {}\n", entry.created_at));
    s.push_str(&format!("updated_at: {}\n", entry.updated_at));
    s.push_str("---\n");
    s.push_str(&entry.content);
    if !entry.content.ends_with('\n') {
        s.push('\n');
    }
    s
}

fn parse_entry(slug_hint: &str, raw: &str) -> Option<MemoryEntry> {
    let rest = raw.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let header = &rest[..end];
    let body = &rest[end + 5..];
    let mut slug = slug_hint.to_string();
    let mut summary = String::new();
    let mut created_at: i64 = 0;
    let mut updated_at: i64 = 0;
    let mut id: Option<String> = None;
    let mut supersedes: Option<String> = None;
    let mut deleted_at: Option<i64> = None;
    let mut kind = "knowledge".to_string();
    let mut tags = Vec::new();
    let mut sources = Vec::new();
    for line in header.lines() {
        let (k, v) = match line.split_once(':') {
            Some(kv) => kv,
            None => continue,
        };
        let v = v.trim();
        match k.trim() {
            "slug" => slug = v.to_string(),
            "summary" => summary = v.to_string(),
            "created_at" => created_at = v.parse().unwrap_or(0),
            "updated_at" => updated_at = v.parse().unwrap_or(0),
            "id" => id = Some(v.to_string()),
            "supersedes" => supersedes = Some(v.to_string()),
            "deleted_at" => deleted_at = v.parse().ok(),
            "kind" => kind = v.to_string(),
            "tags_json" => tags = serde_json::from_str(v).unwrap_or_default(),
            "sources_json" => sources = serde_json::from_str(v).unwrap_or_default(),
            _ => {}
        }
    }
    Some(MemoryEntry {
        slug,
        summary,
        content: body.trim_end_matches('\n').to_string(),
        created_at,
        updated_at,
        id,
        supersedes,
        deleted_at,
        kind,
        tags,
        sources,
    })
}
