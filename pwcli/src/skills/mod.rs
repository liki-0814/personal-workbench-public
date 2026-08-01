use serde_json::Value;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};
use std::time::SystemTime;

use crate::tools::registry::ToolRegistry;

const BUILTIN_ARCHIFY_SKILL: &str = include_str!("../../resources/skills/archify/SKILL.md");

/// 一个 skill = 一个 `~/.agents/skills/<name>/SKILL.md` 文件解析后的结构
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub command: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Global skills cache with mtime-based auto-reload (5s polling)
// ─────────────────────────────────────────────────────────────────────────────

static SKILLS_CACHE: OnceLock<RwLock<Vec<Skill>>> = OnceLock::new();
static SKILLS_WATCHER_STARTED: OnceLock<()> = OnceLock::new();

/// 获取当前 skills（从缓存读取，5s mtime 轮询自动刷新）。
pub fn get_skills() -> Vec<Skill> {
    let cache = SKILLS_CACHE.get_or_init(|| RwLock::new(load_skills_from_disk()));
    cache.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 初始化 skills 缓存并启动 mtime watcher。
/// 在 service 启动或 REPL 初始化时调用一次。
pub fn init() {
    let _ = SKILLS_CACHE.get_or_init(|| RwLock::new(load_skills_from_disk()));
    start_skills_watcher();
}

fn start_skills_watcher() {
    if SKILLS_WATCHER_STARTED.set(()).is_err() {
        return; // already started
    }
    tokio::spawn(async {
        let dir = skills_dir();
        let mut last_mtime = dir_mtime(&dir);
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let current = dir_mtime(&dir);
            if current != last_mtime {
                last_mtime = current;
                reload();
            }
        }
    });
}

fn reload() {
    let fresh = load_skills_from_disk();
    if let Some(cache) = SKILLS_CACHE.get() {
        if let Ok(mut guard) = cache.write() {
            *guard = fresh;
        }
    }
}

/// 获取 skills 目录的最新修改时间（递归检查子目录 SKILL.md）。
fn dir_mtime(dir: &std::path::Path) -> Option<SystemTime> {
    let mut latest: Option<SystemTime> = std::fs::metadata(dir).ok()?.modified().ok();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if let Ok(meta) = std::fs::metadata(&skill_file) {
                    if let Ok(mt) = meta.modified() {
                        latest = Some(latest.map_or(mt, |l| l.max(mt)));
                    }
                }
                // Also check directory mtime (new file added)
                if let Ok(meta) = std::fs::metadata(&path) {
                    if let Ok(mt) = meta.modified() {
                        latest = Some(latest.map_or(mt, |l| l.max(mt)));
                    }
                }
            }
        }
    }
    latest
}

fn skills_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".agents").join("skills"))
        .unwrap_or_else(|| PathBuf::from(".agents/skills"))
}

// ─────────────────────────────────────────────────────────────────────────────
// Disk loading (internal)
// ─────────────────────────────────────────────────────────────────────────────

/// 从 `~/.agents/skills/` 目录加载所有 skill（同步 IO）。
/// 外部调用者应使用 `get_skills()` 走缓存。
pub fn load_skills() -> Vec<Skill> {
    get_skills()
}

fn load_skills_from_disk() -> Vec<Skill> {
    let dir = skills_dir();
    let mut skills = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let skill_file = path.join("SKILL.md");
            if !skill_file.exists() {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&skill_file) {
                if let Some(skill) = parse_skill(&content) {
                    skills.push(skill);
                }
            }
        }
    }
    if !skills.iter().any(|skill| skill.name == "archify") {
        if let Some(skill) = parse_skill(BUILTIN_ARCHIFY_SKILL) {
            skills.push(skill);
        }
    }
    skills
}

// ─────────────────────────────────────────────────────────────────────────────
// Parsing
// ─────────────────────────────────────────────────────────────────────────────

/// 解析 SKILL.md: YAML frontmatter (--- ... ---) + markdown body
fn parse_skill(content: &str) -> Option<Skill> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_first = &trimmed[3..];
    let end_idx = after_first.find("\n---")?;
    let frontmatter = &after_first[..end_idx];
    let body = after_first[end_idx + 4..].trim().to_string();

    let name = extract_yaml_field(frontmatter, "name")?;
    let description = extract_yaml_field(frontmatter, "description").unwrap_or_default();
    let command = extract_yaml_field(frontmatter, "command");

    Some(Skill {
        name,
        description,
        body,
        command,
    })
}

/// Resolve a relative resource against installed skill roots. A result is
/// returned only when exactly one skill contains the requested file.
pub fn resolve_resource_path(path: &str) -> Option<PathBuf> {
    resolve_resource_path_from(&get_skills(), path, &skills_dir())
}

fn resolve_resource_path_from(
    skills: &[Skill],
    path: &str,
    skills_base: &std::path::Path,
) -> Option<PathBuf> {
    let relative = std::path::Path::new(path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return None;
    }

    let base = std::fs::canonicalize(skills_base).ok()?;
    let mut matches = skills.iter().filter_map(|skill| {
        let root = std::fs::canonicalize(base.join(&skill.name)).ok()?;
        if !root.starts_with(&base) {
            return None;
        }
        let candidate = std::fs::canonicalize(root.join(relative)).ok()?;
        (candidate.starts_with(&root) && candidate.is_file()).then_some(candidate)
    });
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn skill_instructions(skill: &Skill) -> String {
    skill_instructions_from(skill, &skills_dir(), "~/.agents/skills")
}

fn skill_instructions_from(
    skill: &Skill,
    skills_base: &std::path::Path,
    resource_root_label: &str,
) -> String {
    if !skills_base.join(&skill.name).is_dir() {
        return format!(
            "Built-in skill compiled into pwcli. It has no external resource directory and must not invoke repository-relative scripts.\n\n{}",
            skill.body
        );
    }
    format!(
        "Skill resource root: {}/{}\nResolve every relative file referenced below against this directory. Use this fixed prefix when calling read_file.\n\n{}",
        resource_root_label.trim_end_matches('/'), skill.name,
        skill.body
    )
}

/// 简易 YAML 字段提取（支持单行和多行 > 格式）
fn extract_yaml_field(yaml: &str, key: &str) -> Option<String> {
    let prefix = format!("{}:", key);
    for (i, line) in yaml.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(&prefix) {
            let value = trimmed[prefix.len()..].trim();
            if value.is_empty() || value == ">" || value == "|" {
                // 多行值：收集后续缩进行
                let mut parts = Vec::new();
                for next_line in yaml.lines().skip(i + 1) {
                    if next_line.starts_with(' ') || next_line.starts_with('\t') {
                        parts.push(next_line.trim());
                    } else {
                        break;
                    }
                }
                let joined = parts.join(" ");
                return if joined.is_empty() {
                    None
                } else {
                    Some(joined)
                };
            }
            let v = if (value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\''))
            {
                value[1..value.len() - 1].to_string()
            } else {
                value.to_string()
            };
            return if v.is_empty() { None } else { Some(v) };
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// System prompt injection
// ─────────────────────────────────────────────────────────────────────────────

/// 构建 skill 列表摘要（注入到 system prompt 末尾）
pub fn build_skill_listing(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n## 可用 Skills\n以下 skill 可通过 use_skill 工具调用（传入 skill 名称即可获取完整内容）：\n\n");
    for s in skills {
        let desc_short: String = s.description.chars().take(200).collect();
        out.push_str(&format!("- **{}**: {}\n", s.name, desc_short));
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Tool registration
// ─────────────────────────────────────────────────────────────────────────────

/// 把 skill 注册为工具：
/// 1. `use_skill` 通用工具 — AI 传 skill name，从全局缓存读取最新 body
/// 2. 有 `command` 字段的 skill 额外注册为独立可执行工具
pub fn register_skill_tools(registry: &mut ToolRegistry, skills: Vec<Skill>) {
    if skills.is_empty() {
        return;
    }

    // use_skill 从全局缓存读取（不再从闭包快照），新增/修改的 skill 自动可见
    registry.register(
        "use_skill",
        "调用一个 skill：传入 skill 名称，返回该 skill 的完整指导内容。调用后请按内容约束行动。",
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "skill 名称（从可用 Skills 列表中选择）" },
                "args": { "type": "string", "description": "可选参数，传递给 skill 的上下文" }
            },
            "required": ["name"]
        }),
        Box::new(move |args: &Value| {
            let name = args["name"].as_str().unwrap_or("").to_string();
            Box::pin(async move {
                let current_skills = get_skills();
                let skill = current_skills.iter().find(|s| s.name == name);
                match skill {
                    Some(s) => Ok(skill_instructions(s)),
                    None => {
                        let available: Vec<_> = current_skills.iter().map(|s| s.name.as_str()).collect();
                        Ok(format!("未找到 skill \"{}\"。可用: {}", name, available.join(", ")))
                    }
                }
            })
        }),
    );

    // 有 command 的 skill 额外注册为独立工具
    for skill in skills {
        if let Some(cmd_template) = skill.command {
            let name = format!("skill_{}", skill.name.replace('-', "_"));
            let desc = format!(
                "[Skill] {} — 执行: {}",
                skill.description.chars().take(100).collect::<String>(),
                &cmd_template
            );
            let cmd = cmd_template.clone();
            registry.register_with_impact(
                name,
                desc,
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "传递给命令的参数" }
                    },
                    "required": ["query"]
                }),
                crate::tools::registry::ToolImpact::ExternalSideEffect,
                Box::new(move |args: &Value| {
                    let query = args["query"].as_str().unwrap_or("").to_string();
                    let final_cmd = cmd.replace("{query}", &query);
                    Box::pin(async move {
                        let input = crate::bash::BashCommandInput::new(final_cmd).with_timeout(30);
                        let sandbox = crate::bash::SandboxConfig::default();
                        let output = crate::bash::execute_bash(input, &sandbox).await?;
                        Ok(output.combined_output())
                    })
                }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skill_basic() {
        let content = r#"---
name: test-skill
description: A test skill
command: echo "{query}"
---

# Test Skill

Body content here."#;
        let skill = parse_skill(content).unwrap();
        assert_eq!(skill.name, "test-skill");
        assert_eq!(skill.description, "A test skill");
        assert_eq!(skill.command, Some("echo \"{query}\"".to_string()));
        assert!(skill.body.contains("Body content here."));
    }

    #[test]
    fn test_parse_skill_multiline_description() {
        let content = r#"---
name: multi
description: >
  This is a long description
  spanning multiple lines
---

Body"#;
        let skill = parse_skill(content).unwrap();
        assert_eq!(skill.name, "multi");
        assert!(skill.description.contains("This is a long description"));
        assert!(skill.description.contains("spanning multiple lines"));
    }

    #[test]
    fn test_parse_skill_no_command() {
        let content = r#"---
name: prompt-only
description: Just a prompt skill
---

Instructions here."#;
        let skill = parse_skill(content).unwrap();
        assert_eq!(skill.name, "prompt-only");
        assert!(skill.command.is_none());
    }

    #[test]
    fn builtin_archify_skill_is_self_contained() {
        let skill = parse_skill(BUILTIN_ARCHIFY_SKILL).expect("built-in Archify skill");
        assert_eq!(skill.name, "archify");
        assert!(skill.description.contains("architecture"));
        assert!(skill.body.contains("create_document"));
        assert!(skill.body.contains("generate_image"));
        assert!(!skill.body.contains("node bin/archify"));
    }

    #[test]
    fn resolves_unique_relative_resource_inside_skill_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("doneai");
        std::fs::create_dir_all(root.join("references/core")).unwrap();
        let expected = root.join("references/core/polling-and-errors.md");
        std::fs::write(&expected, "content").unwrap();
        let skill = Skill {
            name: "doneai".to_string(),
            description: String::new(),
            body: String::new(),
            command: None,
        };

        assert_eq!(
            resolve_resource_path_from(
                &[skill],
                "references/core/polling-and-errors.md",
                temp.path()
            ),
            Some(std::fs::canonicalize(expected).unwrap())
        );
        let instructions = skill_instructions_from(
            &Skill {
                name: "doneai".to_string(),
                description: String::new(),
                body: "instructions".to_string(),
                command: None,
            },
            temp.path(),
            &temp.path().to_string_lossy(),
        );
        assert!(instructions.contains(&format!("Skill resource root: {}", root.display())));
    }

    #[test]
    fn rejects_resource_traversal_and_ambiguous_matches() {
        let temp = tempfile::tempdir().unwrap();
        let make_skill = |name: &str| {
            let root = temp.path().join(name);
            std::fs::create_dir_all(root.join("references")).unwrap();
            std::fs::write(root.join("references/shared.md"), name).unwrap();
            Skill {
                name: name.to_string(),
                description: String::new(),
                body: String::new(),
                command: None,
            }
        };
        let skills = [make_skill("one"), make_skill("two")];

        assert!(resolve_resource_path_from(&skills, "../outside.md", temp.path()).is_none());
        assert!(resolve_resource_path_from(&skills, "references/shared.md", temp.path()).is_none());
    }
}
