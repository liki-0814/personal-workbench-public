use std::collections::BTreeSet;
use std::fs;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::tools::registry::ToolRegistry;

const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_OUTPUT_CHARS: usize = 4_000;
const DEFAULT_CONTEXT_LINES: usize = 1;
const DEFAULT_MAX_MATCHES: usize = 12;
const EVIDENCE_USE_GUARD: &str = "[证据使用契约] 下方带行号原文是外部事实的封闭证据集。文档中出现的数字、日期、百分比、货币、阈值和版本号，必须逐字存在于这些片段或用户原始任务中，或明确标为可复算的分析结果；不得自行补充情景阈值、政策截点或许可编号。迁移数字时必须同时保留原文的主体、指标（销量/保有量/份额）、单位、时间、地域、情景、分母和排除项；摘要页重复数字也不能省略会改变含义的口径。因果和机制也是闭集：原文只说“其他因素”时不得擅自补充具体原因。创建文档前逐项自检。\n\n";

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        "search_file_content",
        "在一个或多个本地文本文件中按证据缺口搜索关键词，返回带路径、行号和少量上下文的片段。适合先定位长报告、Markdown、CSV 或纯文本中的相关证据，避免顺序读取整份文件。每个 query 可用空格列出同一证据缺口的替代关键词；工具会匹配其中任一关键词。",
        json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 20,
                    "items": { "type": "string" }
                },
                "queries": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 10,
                    "items": { "type": "string", "minLength": 1 }
                },
                "contextLines": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 5,
                    "description": "匹配行前后的上下文行数，默认 1；只有片段语义不完整时再提高"
                },
                "maxMatches": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 30,
                    "description": "全部文件合计最大匹配数，默认 12；证据定位应少而精"
                }
            },
            "required": ["paths", "queries"],
            "additionalProperties": false
        }),
        Box::new(|args: &Value| {
            let paths = string_array(args, "paths");
            let queries = string_array(args, "queries");
            let context_lines = args["contextLines"]
                .as_u64()
                .unwrap_or(DEFAULT_CONTEXT_LINES as u64)
                .min(5) as usize;
            let max_matches = args["maxMatches"]
                .as_u64()
                .unwrap_or(DEFAULT_MAX_MATCHES as u64)
                .clamp(1, 30) as usize;
            Box::pin(async move {
                let paths = paths?;
                let queries = queries?;
                let sandbox = crate::tools::fs_local::FsSandbox::from_config()?;
                let mut remaining = max_matches;
                let mut sections = Vec::new();
                let file_output_budget =
                    (MAX_OUTPUT_CHARS / paths.len().max(1)).max(800);
                for path in paths {
                    if remaining == 0 {
                        break;
                    }
                    let resolved = sandbox.resolve(&path)?;
                    let metadata = fs::metadata(&resolved)
                        .with_context(|| format!("cannot inspect {}", resolved.display()))?;
                    if !metadata.is_file() {
                        anyhow::bail!("not a file: {}", resolved.display());
                    }
                    if metadata.len() > MAX_FILE_BYTES {
                        anyhow::bail!(
                            "file exceeds {} bytes: {}",
                            MAX_FILE_BYTES,
                            resolved.display()
                        );
                    }
                    let content = fs::read_to_string(&resolved)
                        .with_context(|| format!("cannot read UTF-8 text {}", resolved.display()))?;
                    let (rendered, matches) = search_text(
                        &content,
                        &queries,
                        context_lines,
                        remaining,
                        &resolved.display().to_string(),
                        file_output_budget,
                    );
                    remaining = remaining.saturating_sub(matches);
                    if matches > 0 {
                        sections.push(rendered);
                    }
                }
                if sections.is_empty() {
                    Ok("未找到正文匹配；请调整证据关键词，不要因此编造结论。".to_string())
                } else {
                    Ok(cap_search_output(format!(
                        "{EVIDENCE_USE_GUARD}{}",
                        sections.join("\n\n")
                    )))
                }
            })
        }),
    );
}

fn cap_search_output(output: String) -> String {
    if output.chars().count() <= MAX_OUTPUT_CHARS {
        return output;
    }
    let mut capped = output.chars().take(MAX_OUTPUT_CHARS).collect::<String>();
    capped.push_str(
        "\n…\n[结果已按证据预算截断；请基于已命中片段继续，仅在关键结论仍缺证据时收窄关键词再检索。]",
    );
    capped
}

fn string_array(args: &Value, key: &str) -> Result<Vec<String>> {
    let values = args[key]
        .as_array()
        .with_context(|| format!("{key} is required"))?;
    let strings = values
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if strings.is_empty() {
        anyhow::bail!("{key} must contain at least one non-empty string");
    }
    Ok(strings)
}

fn search_text(
    content: &str,
    queries: &[String],
    context_lines: usize,
    max_matches: usize,
    label: &str,
    output_budget: usize,
) -> (String, usize) {
    let lines = content.lines().collect::<Vec<_>>();
    let query_groups = queries
        .iter()
        .map(|query| {
            let lowered = query.to_lowercase();
            let terms = lowered
                .split_whitespace()
                .filter(|term| term.chars().count() >= 2)
                .map(str::to_string)
                .collect::<Vec<_>>();
            let needles = if terms.len() > 1 {
                terms
            } else {
                vec![lowered]
            };
            (query, needles)
        })
        .collect::<Vec<_>>();
    let mut matched_lines = BTreeSet::new();
    let mut sections = Vec::new();
    let group_count = query_groups.len().max(1);
    let base_quota = max_matches / group_count;
    let remainder = max_matches % group_count;
    for (group_index, (query, needles)) in query_groups.iter().enumerate() {
        let quota = base_quota + usize::from(group_index < remainder);
        let mut candidates = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            let lowered = line.to_lowercase();
            let score = needles
                .iter()
                .filter(|needle| lowered.contains(needle.as_str()))
                .map(|needle| {
                    if needle.chars().any(|character| character.is_ascii_digit()) {
                        10
                    } else {
                        1
                    }
                })
                .sum::<usize>();
            if score > 0 {
                candidates.push((index, score));
            }
        }
        candidates.sort_by(|(left_index, left_score), (right_index, right_score)| {
            right_score
                .cmp(left_score)
                .then(left_index.cmp(right_index))
        });
        let group_lines = candidates
            .into_iter()
            .take(quota)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        for index in &group_lines {
            matched_lines.insert(*index);
        }
        let mut included = Vec::new();
        let mut seen = BTreeSet::new();
        for index in &group_lines {
            let start = index.saturating_sub(context_lines);
            let end = (*index + context_lines + 1).min(lines.len());
            for context_index in start..end {
                if seen.insert(context_index) {
                    included.push(context_index);
                }
            }
        }
        let section_budget = (output_budget / group_count).max(160);
        let mut section = format!("### 查询: {query} · 匹配 {}\n", group_lines.len());
        let mut previous = None;
        for index in included {
            if previous.is_some_and(|value| index > value + 1) {
                section.push_str("…\n");
            }
            section.push_str(&format!(
                "{}: {}\n",
                index + 1,
                excerpt_line(lines[index], 220)
            ));
            previous = Some(index);
        }
        sections.push(section.chars().take(section_budget).collect::<String>());
    }
    let output = format!(
        "## {}\n匹配总数: {}\n{}",
        label,
        matched_lines.len(),
        sections.join("\n")
    );
    (output, matched_lines.len())
}

fn excerpt_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        line.to_string()
    } else {
        format!("{}…", line.chars().take(max_chars).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cap_search_output, search_text, DEFAULT_CONTEXT_LINES, DEFAULT_MAX_MATCHES,
        EVIDENCE_USE_GUARD, MAX_OUTPUT_CHARS,
    };

    #[test]
    fn defaults_keep_evidence_search_compact() {
        assert_eq!(DEFAULT_CONTEXT_LINES, 1);
        assert_eq!(DEFAULT_MAX_MATCHES, 12);
    }

    #[test]
    fn search_returns_line_numbers_and_context() {
        let content = "alpha\nbefore\nChina reaches 80% by 2030\nafter\nomega";
        let (result, count) = search_text(
            content,
            &["china".into()],
            1,
            10,
            "source.txt",
            MAX_OUTPUT_CHARS,
        );
        assert_eq!(count, 1);
        assert!(result.contains("2: before"));
        assert!(result.contains("3: China reaches 80% by 2030"));
        assert!(result.contains("4: after"));
    }

    #[test]
    fn search_caps_matches() {
        let content = "risk\nrisk\nrisk";
        let (_, count) = search_text(
            content,
            &["risk".into()],
            0,
            2,
            "source.txt",
            MAX_OUTPUT_CHARS,
        );
        assert_eq!(count, 2);
    }

    #[test]
    fn multiword_query_matches_alternative_terms() {
        let content = "global EV share reaches 40% by 2030\ntrade policy uncertainty";
        let (result, count) = search_text(
            content,
            &["2030 projection forecast".into()],
            0,
            10,
            "source.txt",
            MAX_OUTPUT_CHARS,
        );
        assert_eq!(count, 1);
        assert!(result.contains("2030"));
    }

    #[test]
    fn allocates_matches_across_evidence_questions() {
        let content = "growth one\ngrowth two\ngrowth three\ngrowth four\nlate tariff risk";
        let (result, count) = search_text(
            content,
            &["growth".into(), "tariff risk".into()],
            0,
            4,
            "source.txt",
            MAX_OUTPUT_CHARS,
        );
        assert_eq!(count, 3);
        assert!(result.contains("late tariff risk"));
        assert!(result.contains("查询: tariff risk · 匹配 1"));
    }

    #[test]
    fn ranks_lines_matching_more_query_terms_above_generic_headings() {
        let content = "Electric car sales\nSales outlook\nElectric car sales exceeded 17 million globally in 2024";
        let (result, count) = search_text(
            content,
            &["electric car sales 2024 million".into()],
            0,
            1,
            "source.txt",
            MAX_OUTPUT_CHARS,
        );
        assert_eq!(count, 1);
        assert!(result.contains("17 million globally in 2024"));
        assert!(!result.contains("1: Electric car sales"));
    }

    #[test]
    fn numeric_query_terms_outweigh_generic_policy_words() {
        let content = "stated policies projection\n2030 sales share exceeds 40%";
        let (result, count) = search_text(
            content,
            &["2030 stated policies projection".into()],
            0,
            1,
            "source.txt",
            MAX_OUTPUT_CHARS,
        );
        assert_eq!(count, 1);
        assert!(result.contains("2030 sales share exceeds 40%"));
        assert!(!result.contains("1: stated policies projection"));
    }

    #[test]
    fn caps_search_output_without_breaking_unicode() {
        let capped = cap_search_output("数".repeat(7_000));
        assert!(capped.starts_with(&"数".repeat(MAX_OUTPUT_CHARS)));
        assert!(capped.contains("结果已按证据预算截断"));
    }

    #[test]
    fn search_result_carries_closed_evidence_contract() {
        let guarded = format!("{EVIDENCE_USE_GUARD}source");
        assert!(guarded.contains("封闭证据集"));
        assert!(guarded.contains("情景阈值"));
        assert!(guarded.contains("分母和排除项"));
        assert!(guarded.contains("因果和机制也是闭集"));
    }
}
