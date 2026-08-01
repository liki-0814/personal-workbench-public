//! MinerU API token 解析。
//!
//! 设了 token 走 Precision (`/api/v4`)：高质量、需 token；
//! 没设走 Agent (`/api/v1`)：免 token、限速、质量略低。
//! 仅 PDF 工具用此 helper，未来其他 MinerU 端点要复用直接调即可。
//!
//! Source of truth: `~/.pwcli/config.json` → `ai.mineruToken`.

/// 返回非空 token；空字符串视为未配置。
pub fn get_mineru_token() -> Option<String> {
    let from_cfg = crate::config::local_config::get().ai.mineru_token;
    (!from_cfg.is_empty()).then_some(from_cfg)
}
