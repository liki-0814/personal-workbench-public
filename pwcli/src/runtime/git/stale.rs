use std::path::Path;

/// 过期分支策略
#[derive(Debug, Clone)]
pub struct StaleBranchPolicy {
    /// 多少天无提交视为过期
    pub stale_days: u32,
    /// 是否自动标记
    pub auto_mark: bool,
    /// 保护分支列表（永不过期）
    pub protected_branches: Vec<String>,
}

impl Default for StaleBranchPolicy {
    fn default() -> Self {
        Self {
            stale_days: 30,
            auto_mark: true,
            protected_branches: vec![
                "main".to_string(),
                "master".to_string(),
                "develop".to_string(),
                "dev".to_string(),
            ],
        }
    }
}

/// 分支过期检测结果
#[derive(Debug, Clone)]
pub struct StaleBranchResult {
    pub branch: String,
    pub last_commit_date: String,
    pub days_since_commit: u32,
    pub commit_author: String,
    pub is_protected: bool,
    pub is_stale: bool,
}

/// 基础提交状态
#[derive(Debug, Clone)]
pub struct BaseCommitState {
    pub base_branch: String,
    pub base_sha: String,
    pub merge_base: String,
    pub commits_ahead: i32,
    pub commits_behind: i32,
}

impl StaleBranchPolicy {
    /// 检查分支是否受保护
    pub fn is_protected(&self, branch: &str) -> bool {
        self.protected_branches.iter().any(|b| b == branch)
    }

    /// 检测过期分支
    pub async fn detect_stale_branches(
        &self,
        repo_path: &Path,
    ) -> anyhow::Result<Vec<StaleBranchResult>> {
        let output = tokio::process::Command::new("git")
            .args([
                "for-each-ref",
                "--sort=-committerdate",
                "refs/heads/",
                "--format=%(refname:short)|%(committerdate:short)|%(authorname)",
            ])
            .current_dir(repo_path)
            .output()
            .await?;

        let mut results = Vec::new();
        let text = String::from_utf8_lossy(&output.stdout);

        for line in text.lines() {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() < 3 {
                continue;
            }

            let branch = parts[0].to_string();
            let date = parts[1].to_string();
            let author = parts[2].to_string();

            let days_since = days_since_date(&date).unwrap_or(0);
            let is_protected = self.is_protected(&branch);
            let is_stale = !is_protected && days_since >= self.stale_days;

            results.push(StaleBranchResult {
                branch,
                last_commit_date: date,
                days_since_commit: days_since,
                commit_author: author,
                is_protected,
                is_stale,
            });
        }

        Ok(results)
    }
}

/// 获取基础提交状态
pub async fn get_base_commit_state(
    repo_path: &Path,
    branch: &str,
    base_branch: &str,
) -> anyhow::Result<BaseCommitState> {
    let base_sha = run_git(repo_path, &["rev-parse", base_branch]).await?;
    let merge_base = run_git(repo_path, &["merge-base", branch, base_branch]).await?;

    let count = run_git(
        repo_path,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{}...{}", branch, base_branch),
        ],
    )
    .await?;

    let parts: Vec<&str> = count.trim().split('\t').collect();
    let ahead = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let behind = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);

    Ok(BaseCommitState {
        base_branch: base_branch.to_string(),
        base_sha: base_sha.trim().to_string(),
        merge_base: merge_base.trim().to_string(),
        commits_ahead: ahead,
        commits_behind: behind,
    })
}

fn days_since_date(date_str: &str) -> Option<u32> {
    let date = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d").ok()?;
    let today = chrono::Local::now().date_naive();
    let days = today.signed_duration_since(date).num_days();
    Some(days.max(0) as u32)
}

async fn run_git(cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
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
    fn test_protected_branch() {
        let policy = StaleBranchPolicy::default();
        assert!(policy.is_protected("main"));
        assert!(!policy.is_protected("feature/x"));
    }

    #[test]
    fn test_days_since_date() {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        assert_eq!(days_since_date(&date), Some(0));
    }

    #[test]
    fn test_stale_branch_result() {
        let result = StaleBranchResult {
            branch: "old-feature".to_string(),
            last_commit_date: "2024-01-01".to_string(),
            days_since_commit: 365,
            commit_author: "user".to_string(),
            is_protected: false,
            is_stale: true,
        };
        assert!(result.is_stale);
        assert!(!result.is_protected);
    }
}
