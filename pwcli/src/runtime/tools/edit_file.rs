use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};
use tokio::sync::Mutex;
use unicode_normalization::UnicodeNormalization;

use super::fs_local::FsSandbox;
use super::registry::{ToolExecutionMode, ToolImpact, ToolOutput, ToolRegistry};

static MUTATION_LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Replacement {
    old_text: String,
    new_text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditArgs {
    path: String,
    #[serde(deserialize_with = "deserialize_edits")]
    edits: Vec<Replacement>,
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register_structured_with_impact(
        "edit",
        "Apply one or more unique, non-overlapping replacements to a text file. Every edit is matched against the original file.",
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "edits": {
                    "oneOf": [
                        { "type": "array", "minItems": 1, "items": {
                            "type": "object",
                            "properties": { "oldText": {"type":"string"}, "newText": {"type":"string"} },
                            "required": ["oldText", "newText"],
                            "additionalProperties": false
                        }},
                        { "type": "string", "description": "JSON-encoded edit array for model compatibility" }
                    ]
                }
            },
            "required": ["path", "edits"],
            "additionalProperties": false
        }),
        ToolExecutionMode::Sequential,
        ToolImpact::ReversibleMutation,
        Box::new(|value| {
            let value = value.clone();
            Box::pin(async move {
                let args: EditArgs = serde_json::from_value(value).context("Invalid edit arguments")?;
                let sandbox = FsSandbox::from_config()?;
                let path = sandbox.resolve(&args.path)?;
                edit_path(&path, &args.edits).await
            })
        }),
    );
}

pub async fn write_path(path: &Path, content: &str) -> Result<()> {
    let lock = mutation_lock(path).await?;
    let _guard = lock.lock().await;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, content).await?;
    Ok(())
}

async fn edit_path(path: &Path, edits: &[Replacement]) -> Result<ToolOutput> {
    if edits.is_empty() {
        anyhow::bail!("edits must contain at least one replacement");
    }
    let lock = mutation_lock(path).await?;
    let _guard = lock.lock().await;
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("Cannot read {}", path.display()))?;
    let (bom, body) = bytes
        .strip_prefix(&[0xEF, 0xBB, 0xBF])
        .map_or((false, bytes.as_slice()), |body| (true, body));
    let original = std::str::from_utf8(body).context("edit only supports UTF-8 text")?;
    let crlf = original.contains("\r\n");
    let normalized_original = original.replace("\r\n", "\n");
    let mut spans = Vec::with_capacity(edits.len());
    for edit in edits {
        if edit.old_text.is_empty() {
            anyhow::bail!("oldText must not be empty");
        }
        let old = edit.old_text.replace("\r\n", "\n");
        let span = find_unique_span(&normalized_original, &old)?;
        spans.push((span.0, span.1, edit.new_text.replace("\r\n", "\n")));
    }
    spans.sort_by_key(|span| span.0);
    for pair in spans.windows(2) {
        if pair[0].1 > pair[1].0 {
            anyhow::bail!("edits overlap; merge nearby replacements into one edit");
        }
    }
    let mut updated = normalized_original.clone();
    for (start, end, replacement) in spans.iter().rev() {
        updated.replace_range(*start..*end, replacement);
    }
    let first_changed_line = spans.first().map(|(start, _, _)| {
        normalized_original[..*start]
            .bytes()
            .filter(|b| *b == b'\n')
            .count()
            + 1
    });
    let restored = if crlf {
        updated.replace('\n', "\r\n")
    } else {
        updated.clone()
    };
    let mut output = Vec::with_capacity(restored.len() + usize::from(bom) * 3);
    if bom {
        output.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    output.extend_from_slice(restored.as_bytes());
    tokio::fs::write(path, output).await?;

    let diff = render_diff(&normalized_original, &updated);
    let patch = TextDiff::from_lines(&normalized_original, &updated)
        .unified_diff()
        .header(&path.display().to_string(), &path.display().to_string())
        .to_string();
    Ok(ToolOutput::with_details(
        format!("Updated {}", path.display()),
        json!({ "diff": diff, "patch": patch, "firstChangedLine": first_changed_line }),
    ))
}

async fn mutation_lock(path: &Path) -> Result<Arc<Mutex<()>>> {
    let key = if path.exists() {
        tokio::fs::canonicalize(path).await?
    } else {
        path.to_path_buf()
    };
    let mut locks = MUTATION_LOCKS.lock().await;
    Ok(Arc::clone(
        locks.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))),
    ))
}

fn find_unique_span(haystack: &str, needle: &str) -> Result<(usize, usize)> {
    let exact = haystack.match_indices(needle).collect::<Vec<_>>();
    match exact.as_slice() {
        [(start, _)] => return Ok((*start, *start + needle.len())),
        [_, _, ..] => anyhow::bail!("oldText is not unique in the original file"),
        [] => {}
    }
    let normalized_needle = fuzzy_normalize(needle);
    let (normalized_haystack, offsets) = fuzzy_normalize_with_offsets(haystack);
    let matches = normalized_haystack
        .match_indices(&normalized_needle)
        .filter_map(|(start, _)| {
            let end = start + normalized_needle.len();
            Some((offsets.get(start)?.0, offsets.get(end.checked_sub(1)?)?.1))
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [span] => Ok(*span),
        [] => anyhow::bail!("oldText was not found in the original file"),
        _ => anyhow::bail!("oldText fuzzy match is not unique in the original file"),
    }
}

fn fuzzy_normalize(value: &str) -> String {
    fuzzy_normalize_with_offsets(value).0
}

fn fuzzy_normalize_with_offsets(value: &str) -> (String, Vec<(usize, usize)>) {
    let mut normalized = String::new();
    let mut offsets = Vec::new();
    let chars = value.char_indices().collect::<Vec<_>>();
    for (index, (start, ch)) in chars.iter().copied().enumerate() {
        let end = chars
            .get(index + 1)
            .map_or(value.len(), |(offset, _)| *offset);
        if ch == '\n' {
            while normalized.ends_with([' ', '\t']) {
                normalized.pop();
                offsets.pop();
            }
        }
        for normalized_char in ch.to_string().nfkc() {
            let mapped = match normalized_char {
                '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
                '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
                '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
                '\u{00A0}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
                other => other,
            };
            let before = normalized.len();
            normalized.push(mapped);
            offsets.extend(std::iter::repeat_n((start, end), normalized.len() - before));
        }
    }
    while normalized.ends_with([' ', '\t']) {
        normalized.pop();
        offsets.pop();
    }
    (normalized, offsets)
}

fn render_diff(old: &str, new: &str) -> String {
    TextDiff::from_lines(old, new)
        .iter_all_changes()
        .map(|change| {
            let prefix = match change.tag() {
                ChangeTag::Delete => "-",
                ChangeTag::Insert => "+",
                ChangeTag::Equal => " ",
            };
            format!("{prefix}{change}")
        })
        .collect()
}

fn deserialize_edits<'de, D>(deserializer: D) -> Result<Vec<Replacement>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Array(_) => serde_json::from_value(value).map_err(serde::de::Error::custom),
        Value::String(encoded) => serde_json::from_str(&encoded).map_err(serde::de::Error::custom),
        _ => Err(serde::de::Error::custom(
            "edits must be an array or a JSON array string",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn edits_multiple_regions_and_preserves_crlf_bom() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.txt");
        tokio::fs::write(&path, b"\xEF\xBB\xBFalpha\r\nbeta\r\n")
            .await
            .unwrap();
        edit_path(
            &path,
            &[
                Replacement {
                    old_text: "alpha".into(),
                    new_text: "one".into(),
                },
                Replacement {
                    old_text: "beta".into(),
                    new_text: "two".into(),
                },
            ],
        )
        .await
        .unwrap();
        assert_eq!(
            tokio::fs::read(path).await.unwrap(),
            b"\xEF\xBB\xBFone\r\ntwo\r\n"
        );
    }

    #[test]
    fn fuzzy_matches_quotes_spaces_and_trailing_whitespace() {
        assert_eq!(
            find_unique_span("say “hello”\u{00a0}now   \n", "say \"hello\" now\n").unwrap(),
            (0, 24)
        );
    }

    #[test]
    fn rejects_ambiguous_and_overlapping_matches() {
        assert!(find_unique_span("x x", "x").is_err());
    }
}
