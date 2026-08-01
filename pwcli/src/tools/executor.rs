use crate::permissions::{PermissionEngine, PermissionOutcome};
use crate::tools::registry::ToolRegistry;
use anyhow::Result;
use serde_json::Value;

/// 带权限检查的工具执行器
pub struct PermissionedExecutor {
    registry: ToolRegistry,
    permissions: PermissionEngine,
}

impl PermissionedExecutor {
    pub fn new(registry: ToolRegistry, permissions: PermissionEngine) -> Self {
        Self {
            registry,
            permissions,
        }
    }

    pub async fn execute(&self, name: &str, args: &Value) -> Result<String> {
        match self.permissions.check(name) {
            PermissionOutcome::Allow => self.registry.execute(name, args).await,
            PermissionOutcome::Deny => {
                anyhow::bail!("Permission denied: tool '{}' is blocked by policy", name)
            }
            PermissionOutcome::Prompt => {
                // Phase 2 中简化为自动允许（TUI 交互在 Phase 3 实现）
                // 实际应用中这里会暂停等待用户输入
                self.registry.execute(name, args).await
            }
        }
    }

    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    pub fn permissions(&self) -> &PermissionEngine {
        &self.permissions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::PermissionPolicy;

    #[tokio::test]
    async fn test_executor_allow() {
        let registry = crate::tools::registry::ToolRegistry::new();
        registry.register(
            "echo",
            "Echo",
            serde_json::json!({}),
            Box::new(|args| {
                let msg = args["msg"].as_str().unwrap_or("").to_string();
                Box::pin(async move { Ok(msg) })
            }),
        );

        let policy = PermissionPolicy::default();
        let engine = PermissionEngine::new(policy);
        let executor = PermissionedExecutor::new(registry, engine);

        let result = executor
            .execute("echo", &serde_json::json!({"msg": "hi"}))
            .await;
        assert_eq!(result.unwrap(), "hi");
    }
}
