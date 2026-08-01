/// Lane 事件：Git 工作流状态变更
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneEvent {
    /// 分支创建
    BranchCreated { name: String, base: String },
    /// 提交推送
    CommitPushed { sha: String, message: String },
    /// PR 创建/更新
    PullRequestUpdated { number: i32, status: PrStatus },
    /// 代码审查请求
    ReviewRequested { reviewer: String },
    /// 审查完成
    ReviewApproved { reviewer: String },
    /// 合并完成
    MergeCompleted { sha: String, method: MergeMethod },
    /// 发布
    Shipped {
        version: String,
        provenance: ShipProvenance,
    },
    /// 回滚
    RolledBack { from: String, to: String },
    /// 冲突
    ConflictDetected { branch: String, file: String },
    /// 同步完成
    Synced { commits: i32 },
}

/// PR 状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrStatus {
    Draft,
    Open,
    ChangesRequested,
    Approved,
    Merged,
    Closed,
}

/// 合并方法
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

/// 发布来源
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShipProvenance {
    LocalBranch,
    PullRequest { number: i32 },
    DirectPush,
    Tag { name: String },
}

impl LaneEvent {
    /// 事件描述
    pub fn description(&self) -> String {
        match self {
            LaneEvent::BranchCreated { name, base } => {
                format!("🌿 创建分支 {} (基于 {})", name, base)
            }
            LaneEvent::CommitPushed { sha, message } => {
                format!("📝 推送提交 {}: {}", &sha[..8.min(sha.len())], message)
            }
            LaneEvent::PullRequestUpdated { number, status } => {
                format!("🔀 PR #{} 状态更新为 {:?}", number, status)
            }
            LaneEvent::ReviewRequested { reviewer } => {
                format!("👀 请求 {} 审查", reviewer)
            }
            LaneEvent::ReviewApproved { reviewer } => {
                format!("✅ {} 批准了代码", reviewer)
            }
            LaneEvent::MergeCompleted { sha, method } => {
                format!("🔀 合并完成 ({:?}) → {}", method, &sha[..8.min(sha.len())])
            }
            LaneEvent::Shipped {
                version,
                provenance,
            } => {
                format!("🚀 发布 {} ({:?})", version, provenance)
            }
            LaneEvent::RolledBack { from, to } => {
                format!("↩️  回滚 {} → {}", from, to)
            }
            LaneEvent::ConflictDetected { branch, file } => {
                format!("⚠️  分支 {} 冲突: {}", branch, file)
            }
            LaneEvent::Synced { commits } => {
                format!("🔄 同步了 {} 个提交", commits)
            }
        }
    }

    /// 是否是需要用户关注的紧急事件
    pub fn is_urgent(&self) -> bool {
        matches!(
            self,
            LaneEvent::ConflictDetected { .. }
                | LaneEvent::RolledBack { .. }
                | LaneEvent::ReviewRequested { .. }
        )
    }

    /// 事件分类
    pub fn category(&self) -> LaneEventCategory {
        match self {
            LaneEvent::BranchCreated { .. } => LaneEventCategory::Branch,
            LaneEvent::CommitPushed { .. } => LaneEventCategory::Commit,
            LaneEvent::PullRequestUpdated { .. } => LaneEventCategory::Review,
            LaneEvent::ReviewRequested { .. } => LaneEventCategory::Review,
            LaneEvent::ReviewApproved { .. } => LaneEventCategory::Review,
            LaneEvent::MergeCompleted { .. } => LaneEventCategory::Merge,
            LaneEvent::Shipped { .. } => LaneEventCategory::Deploy,
            LaneEvent::RolledBack { .. } => LaneEventCategory::Deploy,
            LaneEvent::ConflictDetected { .. } => LaneEventCategory::Conflict,
            LaneEvent::Synced { .. } => LaneEventCategory::Sync,
        }
    }
}

/// 事件分类
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneEventCategory {
    Branch,
    Commit,
    Review,
    Merge,
    Deploy,
    Conflict,
    Sync,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_description() {
        let event = LaneEvent::BranchCreated {
            name: "feature/x".to_string(),
            base: "main".to_string(),
        };
        assert!(event.description().contains("feature/x"));
    }

    #[test]
    fn test_is_urgent() {
        assert!(LaneEvent::ConflictDetected {
            branch: "main".to_string(),
            file: "a.rs".to_string(),
        }
        .is_urgent());
        assert!(!LaneEvent::Synced { commits: 3 }.is_urgent());
    }

    #[test]
    fn test_category() {
        assert_eq!(
            LaneEvent::CommitPushed {
                sha: "abc".to_string(),
                message: "fix".to_string(),
            }
            .category(),
            LaneEventCategory::Commit
        );
    }
}
