use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 分支锁碰撞信息
#[derive(Debug, Clone)]
pub struct BranchLockCollision {
    pub branch: String,
    pub locked_by: String,
    pub lock_time: String,
    pub reason: String,
}

/// 分支锁管理器
#[derive(Debug, Clone)]
pub struct BranchLockManager {
    locks: Arc<Mutex<HashMap<String, BranchLockEntry>>>,
}

#[derive(Debug, Clone)]
struct BranchLockEntry {
    locked_by: String,
    lock_time: String,
    reason: String,
}

impl BranchLockManager {
    pub fn new() -> Self {
        Self {
            locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 锁定分支
    pub fn lock(&self, branch: &str, by: &str, reason: &str) -> Result<(), BranchLockCollision> {
        let mut locks = self.locks.lock().unwrap();

        if let Some(entry) = locks.get(branch) {
            return Err(BranchLockCollision {
                branch: branch.to_string(),
                locked_by: entry.locked_by.clone(),
                lock_time: entry.lock_time.clone(),
                reason: entry.reason.clone(),
            });
        }

        locks.insert(
            branch.to_string(),
            BranchLockEntry {
                locked_by: by.to_string(),
                lock_time: chrono::Local::now().to_rfc3339(),
                reason: reason.to_string(),
            },
        );

        Ok(())
    }

    /// 解锁分支
    pub fn unlock(&self, branch: &str, by: &str) -> bool {
        let mut locks = self.locks.lock().unwrap();

        if let Some(entry) = locks.get(branch) {
            if entry.locked_by == by {
                locks.remove(branch);
                return true;
            }
        }

        false
    }

    /// 强制解锁（管理员用）
    pub fn force_unlock(&self, branch: &str) -> bool {
        let mut locks = self.locks.lock().unwrap();
        locks.remove(branch).is_some()
    }

    /// 检查分支是否被锁定
    pub fn is_locked(&self, branch: &str) -> Option<BranchLockCollision> {
        let locks = self.locks.lock().unwrap();

        locks.get(branch).map(|entry| BranchLockCollision {
            branch: branch.to_string(),
            locked_by: entry.locked_by.clone(),
            lock_time: entry.lock_time.clone(),
            reason: entry.reason.clone(),
        })
    }

    /// 列出所有锁
    pub fn list_locks(&self) -> Vec<BranchLockCollision> {
        let locks = self.locks.lock().unwrap();

        locks
            .iter()
            .map(|(branch, entry)| BranchLockCollision {
                branch: branch.clone(),
                locked_by: entry.locked_by.clone(),
                lock_time: entry.lock_time.clone(),
                reason: entry.reason.clone(),
            })
            .collect()
    }

    /// 检测分支锁碰撞
    pub fn detect_collisions(&self, branches: &[String]) -> Vec<BranchLockCollision> {
        let mut collisions = Vec::new();

        for branch in branches {
            if let Some(collision) = self.is_locked(branch) {
                collisions.push(collision);
            }
        }

        collisions
    }
}

impl Default for BranchLockManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock_unlock() {
        let mgr = BranchLockManager::new();
        assert!(mgr.lock("main", "user1", "编辑中").is_ok());
        assert!(mgr.is_locked("main").is_some());
        assert!(mgr.unlock("main", "user1"));
        assert!(mgr.is_locked("main").is_none());
    }

    #[test]
    fn test_lock_collision() {
        let mgr = BranchLockManager::new();
        mgr.lock("main", "user1", "编辑中").unwrap();
        let result = mgr.lock("main", "user2", "也要编辑");
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_user_unlock() {
        let mgr = BranchLockManager::new();
        mgr.lock("main", "user1", "编辑中").unwrap();
        assert!(!mgr.unlock("main", "user2"));
    }

    #[test]
    fn test_force_unlock() {
        let mgr = BranchLockManager::new();
        mgr.lock("main", "user1", "编辑中").unwrap();
        assert!(mgr.force_unlock("main"));
        assert!(mgr.is_locked("main").is_none());
    }

    #[test]
    fn test_list_locks() {
        let mgr = BranchLockManager::new();
        mgr.lock("main", "user1", "编辑中").unwrap();
        mgr.lock("dev", "user2", "测试").unwrap();
        let locks = mgr.list_locks();
        assert_eq!(locks.len(), 2);
    }

    #[test]
    fn test_detect_collisions() {
        let mgr = BranchLockManager::new();
        mgr.lock("main", "user1", "编辑中").unwrap();
        let collisions = mgr.detect_collisions(&["main".to_string(), "dev".to_string()]);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].branch, "main");
    }
}
