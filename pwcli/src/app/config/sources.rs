/// 配置来源（按优先级从高到低）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    /// 环境变量（最高优先级）
    Env,
    /// 本地配置（.pwcli/config.local.json）
    Local,
    /// 项目配置（项目根目录 .pwcli.json）
    Project,
    /// 用户全局配置（~/.pwcli/config.json）
    User,
}

impl ConfigSource {
    fn priority_rank(&self) -> u8 {
        match self {
            ConfigSource::Env => 3,
            ConfigSource::Local => 2,
            ConfigSource::Project => 1,
            ConfigSource::User => 0,
        }
    }
}

impl PartialOrd for ConfigSource {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ConfigSource {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority_rank().cmp(&other.priority_rank())
    }
}

/// 配置条目（带来源追踪）
#[derive(Debug, Clone)]
pub struct ConfigEntry<T> {
    pub value: T,
    pub source: ConfigSource,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_source_ordering() {
        assert!(ConfigSource::Env > ConfigSource::Local);
        assert!(ConfigSource::Local > ConfigSource::Project);
        assert!(ConfigSource::Project > ConfigSource::User);
    }
}
