use serde_json::Value;

use super::domain_schema::DomainSchema;

pub fn format_results(domain: &str, items: &[Value], schema: &DomainSchema) -> String {
    match domain {
        "todo" => format_todos(items),
        _ => format_generic(items, schema),
    }
}

fn format_todos(items: &[Value]) -> String {
    if items.is_empty() {
        return "（暂无任务）".to_string();
    }
    items
        .iter()
        .map(|t| {
            let completed = t
                .get("completed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let title = t.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let priority = t.get("priority").and_then(|v| v.as_str()).unwrap_or("");
            let due = t.get("dueDate").and_then(|v| v.as_str()).unwrap_or("");
            let sub_tasks = t.get("subTasks").and_then(|v| v.as_array());
            let mut line = format!("{} {}", if completed { "✅" } else { "⬜" }, title);
            if !priority.is_empty() {
                line.push_str(&format!(" [{}]", priority));
            }
            if !due.is_empty() {
                line.push_str(&format!(" (截止: {})", due));
            }
            if let Some(subs) = sub_tasks {
                let done = subs
                    .iter()
                    .filter(|s| {
                        s.get("completed")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                    })
                    .count();
                if !subs.is_empty() {
                    line.push_str(&format!(" | {}/{} 子任务", done, subs.len()));
                }
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_generic(items: &[Value], schema: &DomainSchema) -> String {
    let mut lines = vec![format!("找到 {} 条记录", items.len())];
    let display = &schema.display_field;
    for item in items.iter().take(20) {
        if let Some(label) = item.get(display).and_then(|v| v.as_str()) {
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
            lines.push(format!("  - {} ({})", label, id));
        } else if let Some(content) = item.get("content").and_then(|v| v.as_str()) {
            let preview: String = content.chars().take(100).collect();
            if content.chars().count() > 100 {
                lines.push(format!("  - {}...", preview));
            } else {
                lines.push(format!("  - {}", preview));
            }
        } else {
            lines.push(format!(
                "  - {}",
                serde_json::to_string(item).unwrap_or_default()
            ));
        }
    }
    if items.len() > 20 {
        lines.push(format!("  ... 还有 {} 条", items.len() - 20));
    }
    lines.join("\n")
}
