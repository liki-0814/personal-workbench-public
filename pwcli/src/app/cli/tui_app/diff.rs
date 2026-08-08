use std::process::Stdio;

use anyhow::{Context, Result};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tokio::process::Command;

#[derive(Debug, Clone, Default)]
pub struct DiffDocument {
    pub lines: Vec<String>,
    pub files: usize,
    pub additions: usize,
    pub deletions: usize,
}

impl DiffDocument {
    pub fn parse(text: &str) -> Self {
        let lines: Vec<String> = text.lines().map(ToOwned::to_owned).collect();
        let files = lines
            .iter()
            .filter(|line| line.starts_with("diff --git "))
            .count();
        let additions = lines
            .iter()
            .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
            .count();
        let deletions = lines
            .iter()
            .filter(|line| line.starts_with('-') && !line.starts_with("---"))
            .count();
        Self {
            lines,
            files,
            additions,
            deletions,
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "{} files · +{} / -{}",
            self.files, self.additions, self.deletions
        )
    }

    pub fn styled_lines(&self) -> Vec<Line<'static>> {
        self.lines
            .iter()
            .map(|line| {
                let style = if line.starts_with("diff --git ") {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else if line.starts_with("@@") {
                    Style::default().fg(Color::Magenta)
                } else if line.starts_with('+') && !line.starts_with("+++") {
                    Style::default().fg(Color::Green)
                } else if line.starts_with('-') && !line.starts_with("---") {
                    Style::default().fg(Color::Red)
                } else if line.starts_with("---") || line.starts_with("+++") {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                };
                Line::from(Span::styled(line.clone(), style))
            })
            .collect()
    }
}

pub async fn load_worktree() -> Result<DiffDocument> {
    let output = Command::new("git")
        .args(["diff", "--no-ext-diff", "--unified=3", "HEAD", "--"])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("run git diff")?;
    if !output.status.success() {
        anyhow::bail!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let mut patch = output.stdout;
    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("list untracked files")?;
    if !untracked.status.success() {
        anyhow::bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&untracked.stderr).trim()
        );
    }
    for path in untracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|p| !p.is_empty())
    {
        let path = String::from_utf8_lossy(path);
        let addition = Command::new("git")
            .args([
                "diff",
                "--no-index",
                "--no-ext-diff",
                "--unified=3",
                "--",
                "/dev/null",
                path.as_ref(),
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await
            .with_context(|| format!("diff untracked file {path}"))?;
        // `git diff --no-index` returns 1 when differences were found.
        if addition.status.code() != Some(0) && addition.status.code() != Some(1) {
            anyhow::bail!(
                "git diff failed for {path}: {}",
                String::from_utf8_lossy(&addition.stderr).trim()
            );
        }
        patch.extend_from_slice(&addition.stdout);
    }
    Ok(DiffDocument::parse(&String::from_utf8_lossy(&patch)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_diff_summary_without_counting_headers() {
        let document = DiffDocument::parse(
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1,2 @@\n-old\n+new\n+more\n",
        );
        assert_eq!(document.files, 1);
        assert_eq!(document.additions, 2);
        assert_eq!(document.deletions, 1);
        assert_eq!(document.summary(), "1 files · +2 / -1");
    }
}
