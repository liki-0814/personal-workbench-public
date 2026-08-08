//! pi 对齐的 `grep` / `find` 搜索工具。
//!
//! 基于 `ignore` crate 的 `WalkBuilder`，自动尊重 .gitignore/.ignore；
//! 两者都经 `FsSandbox::resolve` 约束在 `tools.fsBase` 沙箱内。

use serde_json::{json, Value};

use crate::runtime::tools::registry::{ToolExecutionMode, ToolImpact, ToolOutput, ToolRegistry};

const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_OUTPUT_CHARS: usize = 20_000;
const DEFAULT_GREP_LIMIT: u64 = 100;
const DEFAULT_FIND_LIMIT: u64 = 1000;

pub fn register(registry: &mut ToolRegistry) {
    registry.register_structured_with_impact(
        "grep",
        "在目录中按正则（或字面量）搜索文件内容，自动尊重 .gitignore。返回带路径和行号的匹配行，可带上下文行；适合先定位再用 read 精读。",
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "minLength": 1, "description": "正则表达式；literal=true 时按字面量匹配" },
                "path": { "type": "string", "description": "搜索目录或文件（可选，默认沙箱根目录）" },
                "glob": { "type": "string", "description": "文件名过滤 glob，如 '*.rs'（可选）" },
                "ignoreCase": { "type": "boolean", "description": "忽略大小写（可选）" },
                "literal": { "type": "boolean", "description": "把 pattern 当字面量而非正则（可选）" },
                "context": { "type": "integer", "minimum": 0, "maximum": 10, "description": "每个匹配前后的上下文行数（可选）" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500, "description": "最大匹配行数，默认 100" }
            },
            "required": ["pattern"],
            "additionalProperties": false
        }),
        ToolExecutionMode::Parallel,
        ToolImpact::Observe,
        Box::new(|args: &Value| {
            let pattern = args["pattern"].as_str().unwrap_or("").to_string();
            let path = args["path"].as_str().map(str::to_string);
            let glob = args["glob"].as_str().map(str::to_string);
            let ignore_case = args["ignoreCase"].as_bool().unwrap_or(false);
            let literal = args["literal"].as_bool().unwrap_or(false);
            let context = args["context"].as_u64().unwrap_or(0).min(10) as usize;
            let limit = args["limit"]
                .as_u64()
                .unwrap_or(DEFAULT_GREP_LIMIT)
                .clamp(1, 500) as usize;
            Box::pin(async move {
                if pattern.is_empty() {
                    anyhow::bail!("pattern is required");
                }
                let output = tokio::task::spawn_blocking(move || {
                    grep_files(&pattern, path.as_deref(), glob.as_deref(), ignore_case, literal, context, limit)
                })
                .await??;
                Ok(ToolOutput::text(output))
            })
        }),
    );

    registry.register_structured_with_impact(
        "find",
        "按 glob 模式查找文件路径（自动尊重 .gitignore），返回相对沙箱根的路径列表。示例：'**/*.rs'、'**/test_*.py'。",
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "minLength": 1, "description": "glob 模式，如 '**/*.rs' 或 '*.md'" },
                "path": { "type": "string", "description": "搜索目录（可选，默认沙箱根目录）" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 5000, "description": "最多返回的路径数，默认 1000" }
            },
            "required": ["pattern"],
            "additionalProperties": false
        }),
        ToolExecutionMode::Parallel,
        ToolImpact::Observe,
        Box::new(|args: &Value| {
            let pattern = args["pattern"].as_str().unwrap_or("").to_string();
            let path = args["path"].as_str().map(str::to_string);
            let limit = args["limit"]
                .as_u64()
                .unwrap_or(DEFAULT_FIND_LIMIT)
                .clamp(1, 5000) as usize;
            Box::pin(async move {
                if pattern.is_empty() {
                    anyhow::bail!("pattern is required");
                }
                let output = tokio::task::spawn_blocking(move || {
                    find_files(&pattern, path.as_deref(), limit)
                })
                .await??;
                Ok(ToolOutput::text(output))
            })
        }),
    );
}

fn resolve_search_root(path: Option<&str>) -> anyhow::Result<std::path::PathBuf> {
    let sandbox = crate::runtime::tools::fs_local::FsSandbox::from_config()?;
    let resolved = sandbox.resolve(path.unwrap_or("."))?;
    if !resolved.exists() {
        anyhow::bail!("path does not exist: {}", resolved.display());
    }
    Ok(resolved)
}

fn build_walker(root: &std::path::Path, glob: Option<&str>) -> anyhow::Result<ignore::Walk> {
    let mut builder = ignore::WalkBuilder::new(root);
    builder.hidden(false).git_ignore(true).git_global(true);
    if let Some(glob) = glob {
        let glob = glob.trim();
        if !glob.is_empty() {
            let mut overrides = ignore::overrides::OverrideBuilder::new(root);
            overrides
                .add(glob)
                .map_err(|error| anyhow::anyhow!("invalid glob '{glob}': {error}"))?;
            builder.overrides(
                overrides
                    .build()
                    .map_err(|error| anyhow::anyhow!("invalid glob '{glob}': {error}"))?,
            );
        }
    }
    Ok(builder.build())
}

fn grep_files(
    pattern: &str,
    path: Option<&str>,
    glob: Option<&str>,
    ignore_case: bool,
    literal: bool,
    context: usize,
    limit: usize,
) -> anyhow::Result<String> {
    let root = resolve_search_root(path)?;
    grep_files_in_root(pattern, &root, glob, ignore_case, literal, context, limit)
}

fn grep_files_in_root(
    pattern: &str,
    root: &std::path::Path,
    glob: Option<&str>,
    ignore_case: bool,
    literal: bool,
    context: usize,
    limit: usize,
) -> anyhow::Result<String> {
    let pattern_text = if literal {
        regex::escape(pattern)
    } else {
        pattern.to_string()
    };
    let regex = regex::RegexBuilder::new(&pattern_text)
        .case_insensitive(ignore_case)
        .build()
        .map_err(|error| anyhow::anyhow!("invalid regex '{pattern}': {error}"))?;

    // path 指向单个文件时直接搜该文件
    if root.is_file() {
        let mut lines = Vec::new();
        let mut total = 0usize;
        grep_one_file(&root, &root, &regex, context, limit, &mut lines, &mut total);
        return Ok(render_grep_output(&lines, total, limit));
    }

    let mut lines = Vec::new();
    let mut total = 0usize;
    for entry in build_walker(&root, glob)? {
        if total >= limit {
            break;
        }
        let Ok(entry) = entry else { continue };
        let file_path = entry.path();
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        grep_one_file(
            &root, file_path, &regex, context, limit, &mut lines, &mut total,
        );
    }
    Ok(render_grep_output(&lines, total, limit))
}

fn grep_one_file(
    root: &std::path::Path,
    file_path: &std::path::Path,
    regex: &regex::Regex,
    context: usize,
    limit: usize,
    lines: &mut Vec<String>,
    total: &mut usize,
) {
    if *total >= limit {
        return;
    }
    let Ok(metadata) = std::fs::metadata(file_path) else {
        return;
    };
    if metadata.len() > MAX_FILE_BYTES {
        return;
    }
    let Ok(bytes) = std::fs::read(file_path) else {
        return;
    };
    if bytes.contains(&0) {
        return;
    }
    let Ok(content) = String::from_utf8(bytes) else {
        return;
    };
    let rel = file_path
        .strip_prefix(root)
        .unwrap_or(file_path)
        .to_string_lossy();
    let file_lines: Vec<&str> = content.lines().collect();
    let mut printed = std::collections::BTreeSet::new();
    let mut last_group_end = 0usize;
    for (index, line) in file_lines.iter().enumerate() {
        if !regex.is_match(line) {
            continue;
        }
        if *total >= limit {
            return;
        }
        *total += 1;
        let start = index.saturating_sub(context);
        let end = (index + context + 1).min(file_lines.len());
        if context > 0 && !printed.is_empty() && start > last_group_end {
            lines.push("--".to_string());
        }
        for context_index in start..end {
            if printed.insert(context_index) {
                let separator = if context_index == index { ":" } else { "-" };
                lines.push(format!(
                    "{rel}{}{}: {}",
                    separator,
                    context_index + 1,
                    excerpt_line(file_lines[context_index], 300)
                ));
            }
        }
        last_group_end = end;
    }
}

fn render_grep_output(lines: &[String], total: usize, limit: usize) -> String {
    if lines.is_empty() {
        return "未找到匹配".to_string();
    }
    let mut output = lines.join("\n");
    if total >= limit {
        output.push_str(&format!(
            "\n[已达匹配上限 {limit} 条；请收窄 pattern、path 或 glob 后再搜]"
        ));
    }
    cap_output(output)
}

fn find_files(pattern: &str, path: Option<&str>, limit: usize) -> anyhow::Result<String> {
    let root = resolve_search_root(path)?;
    find_files_in_root(pattern, &root, limit)
}

fn find_files_in_root(
    pattern: &str,
    root: &std::path::Path,
    limit: usize,
) -> anyhow::Result<String> {
    let matcher = globset::GlobBuilder::new(pattern)
        .literal_separator(false)
        .build()
        .map_err(|error| anyhow::anyhow!("invalid glob '{pattern}': {error}"))?
        .compile_matcher();
    let basename_only = !pattern.contains('/');

    let mut matches = Vec::new();
    for entry in build_walker(&root, None)? {
        if matches.len() >= limit {
            break;
        }
        let Ok(entry) = entry else { continue };
        let file_path = entry.path();
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let rel = file_path.strip_prefix(&root).unwrap_or(file_path);
        // 无 '/' 的模式按文件名匹配（如 '*.rs'），带路径的模式按相对路径匹配
        let matched = matcher.is_match(rel)
            || (basename_only && rel.file_name().is_some_and(|name| matcher.is_match(name)));
        if matched {
            matches.push(rel.to_string_lossy().into_owned());
        }
    }
    if matches.is_empty() {
        return Ok("未找到匹配的文件".to_string());
    }
    let mut output = matches.join("\n");
    if matches.len() >= limit {
        output.push_str(&format!(
            "\n[已达上限 {limit} 条；请收窄 pattern 或 path 后再搜]"
        ));
    }
    Ok(cap_output(output))
}

fn excerpt_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        line.to_string()
    } else {
        format!("{}…", line.chars().take(max_chars).collect::<String>())
    }
}

fn cap_output(output: String) -> String {
    if output.chars().count() <= MAX_OUTPUT_CHARS {
        return output;
    }
    let mut capped = output.chars().take(MAX_OUTPUT_CHARS).collect::<String>();
    capped.push_str("\n[输出已截断；请收窄搜索范围]");
    capped
}

#[cfg(test)]
mod tests {
    use super::{
        cap_output, excerpt_line, find_files_in_root, grep_files_in_root, MAX_OUTPUT_CHARS,
    };

    #[test]
    fn excerpt_truncates_long_lines() {
        let long = "x".repeat(400);
        assert!(excerpt_line(&long, 300).ends_with('…'));
        assert_eq!(excerpt_line("short", 300), "short");
    }

    #[test]
    fn output_is_capped() {
        let big = "line\n".repeat(MAX_OUTPUT_CHARS);
        let capped = cap_output(big);
        assert!(capped.contains("[输出已截断；请收窄搜索范围]"));
    }

    #[test]
    fn find_and_grep_execute_against_a_real_temporary_tree() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(
            root.path().join("src/main.rs"),
            "fn main() { println!(\"needle\"); }",
        )
        .unwrap();
        std::fs::write(root.path().join("README.md"), "nothing here").unwrap();

        let found = find_files_in_root("**/*.rs", root.path(), 10).unwrap();
        assert!(found.contains("src/main.rs"), "{found}");
        let matches =
            grep_files_in_root("needle", root.path(), Some("*.rs"), false, true, 0, 10).unwrap();
        assert!(matches.contains("src/main.rs"), "{matches}");
        assert!(matches.contains("needle"), "{matches}");
    }
}
