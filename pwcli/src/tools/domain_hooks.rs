use serde_json::{json, Value};

use super::domain_schema::DomainSchema;

pub trait DomainHook: Send + Sync {
    fn on_create(&self, _item: &mut Value, _schema: &DomainSchema) {}
    fn on_before_update(&self, _item: &mut Value) {}
}

pub fn get_hook(domain: &str) -> Option<&'static dyn DomainHook> {
    match domain {
        "todo" => Some(&TodoHook),
        "chat_folder" => Some(&FolderHook),
        _ => None,
    }
}

// ─── Todo Hook ───

struct TodoHook;

impl DomainHook for TodoHook {
    fn on_create(&self, item: &mut Value, _schema: &DomainSchema) {
        enrich_todo_create(item);
    }
}

fn enrich_todo_create(item: &mut Value) {
    let Value::Object(map) = item else { return };
    let title = map
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let type_is_today = map
        .get("type")
        .and_then(|v| v.as_str())
        .map(|t| t == "today")
        .unwrap_or(true);

    if !map.contains_key("type") {
        map.insert("type".to_string(), json!("today"));
    }
    if !map.contains_key("priority") {
        map.insert("priority".to_string(), json!("medium"));
    }
    if !map.contains_key("notes") {
        map.insert("notes".to_string(), json!(default_todo_notes(&title)));
    }
    if !map.contains_key("dueDate") && type_is_today {
        map.insert(
            "dueDate".to_string(),
            json!(chrono::Local::now().format("%Y-%m-%d").to_string()),
        );
    }

    let needs_steps = !matches!(map.get("subTasks"), Some(Value::Array(arr)) if !arr.is_empty());
    if needs_steps {
        map.insert(
            "subTasks".to_string(),
            Value::Array(infer_todo_steps(&title)),
        );
        return;
    }

    if let Some(Value::Array(sub_tasks)) = map.get_mut("subTasks") {
        for sub_task in sub_tasks {
            let Value::Object(sub_map) = sub_task else {
                continue;
            };
            if !sub_map.contains_key("id") {
                sub_map.insert("id".to_string(), json!(generate_id("subtask")));
            }
            if !sub_map.contains_key("completed") {
                sub_map.insert("completed".to_string(), json!(false));
            }
        }
    }
}

fn infer_todo_steps(title: &str) -> Vec<Value> {
    let complex_keywords = [
        "分析", "排查", "优化", "方案", "调研", "设计", "复盘", "故障", "问题", "丢失", "模型",
        "实验", "上线", "重构",
    ];
    let steps: Vec<&str> = if complex_keywords.iter().any(|k| title.contains(k)) {
        vec![
            "明确目标、背景和影响范围",
            "收集相关数据、日志和现象证据",
            "定位原因并验证关键假设",
            "制定优化方案和实施步骤",
            "沉淀结论、风险和后续行动项",
        ]
    } else {
        vec!["明确完成标准", "执行任务", "检查结果并收尾"]
    };

    steps
        .into_iter()
        .map(|title| {
            json!({
                "id": generate_id("subtask"),
                "title": title,
                "completed": false
            })
        })
        .collect()
}

fn default_todo_notes(title: &str) -> String {
    let complex_keywords = [
        "分析", "排查", "优化", "方案", "调研", "设计", "复盘", "故障", "问题", "丢失", "模型",
    ];
    if complex_keywords.iter().any(|k| title.contains(k)) {
        format!(
            "围绕「{}」梳理背景、现象、根因、优化方案和后续行动。",
            title
        )
    } else {
        String::new()
    }
}

// ─── Folder Hook ───

struct FolderHook;

impl DomainHook for FolderHook {
    fn on_create(&self, item: &mut Value, _schema: &DomainSchema) {
        normalize_folder_payload(item);
    }
    fn on_before_update(&self, item: &mut Value) {
        normalize_folder_payload(item);
    }
}

fn normalize_folder_payload(item: &mut Value) {
    let Value::Object(map) = item else { return };
    if map.contains_key("title") && !map.contains_key("name") {
        if let Some(title) = map.remove("title") {
            map.insert("name".to_string(), title);
        }
    }
}

// ─── Helpers ───

fn generate_id(domain: &str) -> String {
    use rand::Rng;
    let ts = chrono::Utc::now().timestamp_millis();
    let rand_part = format!("{:05x}", rand::thread_rng().gen::<u32>());
    format!("{}-{}-{}", domain, ts, &rand_part[..5.min(rand_part.len())])
}

pub fn generate_domain_id(domain: &str) -> String {
    generate_id(domain)
}
