use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

/// Git 上下文信息
#[derive(Debug, Clone, Default)]
pub struct GitContext {
    pub is_git_repo: bool,
    pub branch: String,
    pub commit_sha: String,
    pub commit_message: String,
    pub remote_url: String,
    pub ahead: i32,
    pub behind: i32,
    pub has_uncommitted: bool,
    pub has_untracked: bool,
    pub modified_files: Vec<String>,
    pub untracked_files: Vec<String>,
    pub stash_count: i32,
}

impl GitContext {
    pub async fn from_path(path: &Path) -> Self {
        let mut ctx = Self::default();

        // 检查是否在 git 仓库中
        let check = Command::new("git")
            .arg("rev-parse")
            .arg("--git-dir")
            .current_dir(path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        match check {
            Ok(output) if output.status.success() => {
                ctx.is_git_repo = true;
            }
            _ => return ctx,
        }

        // 获取分支
        if let Ok(branch) = run_git(path, &["rev-parse", "--abbrev-ref", "HEAD"]).await {
            ctx.branch = branch.trim().to_string();
        } else if let Ok(branch) = run_git(path, &["symbolic-ref", "--short", "HEAD"]).await {
            // `rev-parse HEAD` fails before the first commit, but an unborn
            // repository still has a meaningful current branch.
            ctx.branch = branch.trim().to_string();
        }

        // 获取 commit SHA
        if let Ok(sha) = run_git(path, &["rev-parse", "HEAD"]).await {
            ctx.commit_sha = sha.trim().to_string();
        }

        // 获取 commit message
        if let Ok(msg) = run_git(path, &["log", "-1", "--pretty=%s"]).await {
            ctx.commit_message = msg.trim().to_string();
        }

        // 获取 remote URL
        if let Ok(url) = run_git(path, &["remote", "get-url", "origin"]).await {
            ctx.remote_url = url.trim().to_string();
        }

        // 获取 ahead/behind
        if let Ok(count) = run_git(
            path,
            &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
        )
        .await
        {
            let parts: Vec<&str> = count.trim().split('\t').collect();
            if parts.len() == 2 {
                ctx.ahead = parts[0].parse().unwrap_or(0);
                ctx.behind = parts[1].parse().unwrap_or(0);
            }
        }

        // 检查是否有未提交更改
        if let Ok(status) = run_git(path, &["status", "--porcelain"]).await {
            let lines: Vec<&str> = status.lines().collect();
            ctx.has_uncommitted = lines.iter().any(|l| {
                l.starts_with(' ')
                    || l.starts_with('M')
                    || l.starts_with('A')
                    || l.starts_with('D')
                    || l.starts_with('R')
                    || l.starts_with('C')
            });
            ctx.has_untracked = lines.iter().any(|l| l.starts_with("??"));
            ctx.modified_files = lines
                .iter()
                .filter(|l| l.starts_with('M') || l.starts_with(" M"))
                .map(|l| l[3..].to_string())
                .collect();
            ctx.untracked_files = lines
                .iter()
                .filter(|l| l.starts_with("??"))
                .map(|l| l[3..].to_string())
                .collect();
        }

        // 获取 stash 数量
        if let Ok(stash) = run_git(path, &["stash", "list"]).await {
            ctx.stash_count = stash.lines().count() as i32;
        }

        ctx
    }

    /// 获取当前目录的 Git 上下文
    pub async fn current() -> Self {
        Self::from_path(Path::new(".")).await
    }

    /// 格式化状态摘要
    pub fn status_summary(&self) -> String {
        if !self.is_git_repo {
            return "（非 Git 仓库）".to_string();
        }

        let mut parts = vec![format!(
            "📌 {} {}",
            self.branch,
            &self.commit_sha[..8.min(self.commit_sha.len())]
        )];

        if self.ahead > 0 {
            parts.push(format!("↑{}", self.ahead));
        }
        if self.behind > 0 {
            parts.push(format!("↓{}", self.behind));
        }
        if self.has_uncommitted {
            parts.push("*修改".to_string());
        }
        if self.has_untracked {
            parts.push("?未跟踪".to_string());
        }
        if self.stash_count > 0 {
            parts.push(format!("📦stash({})", self.stash_count));
        }

        parts.join(" ")
    }
}

async fn run_git(cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(anyhow::anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_git_repo() {
        let ctx = GitContext::default();
        assert_eq!(ctx.status_summary(), "（非 Git 仓库）");
    }

    #[tokio::test]
    async fn test_git_context_current() {
        let temp = tempfile::tempdir().unwrap();
        run_git(temp.path(), &["init", "-b", "test-branch"])
            .await
            .unwrap();

        let ctx = GitContext::from_path(temp.path()).await;
        assert!(ctx.is_git_repo);
        assert_eq!(ctx.branch, "test-branch");
    }
}
