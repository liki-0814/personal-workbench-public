use uuid::Uuid;

/// 新记忆条目分配 UUID v4（schema v2 主键，slug 仍为人类可读键）。
pub fn new_entry_id() -> String {
    Uuid::new_v4().to_string()
}
