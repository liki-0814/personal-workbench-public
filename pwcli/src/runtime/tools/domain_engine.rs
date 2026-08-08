use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::runtime::backend::BackendClient;

use super::domain_format::format_results;
use super::domain_hooks::{generate_domain_id, get_hook};
use super::domain_schema::{DomainRegistry, DomainSchema};

static REGISTRY: OnceLock<DomainRegistry> = OnceLock::new();

pub fn registry() -> &'static DomainRegistry {
    REGISTRY.get_or_init(DomainRegistry::load)
}

pub async fn execute(backend: &BackendClient, args: &Value) -> Result<String> {
    let domain = args["domain"].as_str().unwrap_or("").to_string();
    let action = args["action"].as_str().unwrap_or("").to_string();
    let target_id = args["target_id"].as_str().unwrap_or("").to_string();
    let payload = args
        .get("payload")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));

    if domain.is_empty() || action.is_empty() {
        return Err(anyhow::anyhow!("domain 和 action 必填"));
    }
    if action == "batch" {
        return execute_batch(backend, &payload).await;
    }
    execute_single(backend, &domain, &action, &target_id, &payload).await
}

async fn execute_single(
    backend: &BackendClient,
    domain: &str,
    action: &str,
    target_id: &str,
    payload: &Value,
) -> Result<String> {
    let reg = registry();
    let schema = reg
        .get(domain)
        .ok_or_else(|| anyhow::anyhow!("未知领域: {}", domain))?;

    let current = backend
        .get_data(&schema.storage_key)
        .await
        .unwrap_or_else(|_| {
            if schema.collection {
                json!([])
            } else {
                Value::Null
            }
        });

    if !schema.collection {
        return execute_singleton(backend, schema, action, payload, current).await;
    }

    let mut items = match current {
        Value::Array(arr) => arr,
        _ => vec![],
    };

    match action {
        "create" => do_create(backend, schema, payload, &mut items).await,
        "update" => do_update(backend, schema, target_id, payload, &mut items).await,
        "delete" => do_delete(backend, schema, target_id, &mut items).await,
        "read" | "query" => do_query(schema, payload, &items),
        _ => Err(anyhow::anyhow!("未知操作: {}", action)),
    }
}

async fn execute_singleton(
    backend: &BackendClient,
    schema: &DomainSchema,
    action: &str,
    payload: &Value,
    current: Value,
) -> Result<String> {
    match action {
        "read" | "query" => Ok(format!(
            "{} 当前值\n{}",
            schema.name,
            serde_json::to_string_pretty(&current).unwrap_or_default()
        )),
        "create" | "update" => {
            let next = merge_value_payload(current, payload);
            backend
                .set_data(&schema.storage_key, &next)
                .await
                .with_context(|| format!("保存 {} 失败", schema.storage_key))?;
            Ok(format!("已更新 {}", schema.name))
        }
        "delete" => {
            backend
                .delete_data(&schema.storage_key)
                .await
                .with_context(|| format!("删除 {} 失败", schema.storage_key))?;
            Ok(format!("已删除 {}", schema.name))
        }
        _ => Err(anyhow::anyhow!("未知操作: {}", action)),
    }
}

async fn execute_batch(backend: &BackendClient, payload: &Value) -> Result<String> {
    let operations = payload
        .get("operations")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("batch 操作需要 payload.operations 数组"))?;

    if operations.is_empty() {
        return Ok("batch 操作列表为空".to_string());
    }

    let mut results = Vec::new();
    for (i, op) in operations.iter().enumerate() {
        let domain = op["domain"].as_str().unwrap_or("").to_string();
        let action = op["action"].as_str().unwrap_or("").to_string();
        let target_id = op["target_id"].as_str().unwrap_or("").to_string();
        let op_payload = op
            .get("payload")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));

        if domain.is_empty() || action.is_empty() {
            results.push(format!("[{}] 跳过：domain 或 action 为空", i + 1));
            continue;
        }

        let result = execute_single(backend, &domain, &action, &target_id, &op_payload).await;
        match result {
            Ok(r) => results.push(format!("[{}] {}.{}: {}", i + 1, domain, action, r)),
            Err(e) => results.push(format!("[{}] {}.{} 错误: {}", i + 1, domain, action, e)),
        }
    }

    Ok(format!(
        "批量操作完成（{} 项）：\n{}",
        operations.len(),
        results.join("\n")
    ))
}

async fn do_create(
    backend: &BackendClient,
    schema: &DomainSchema,
    payload: &Value,
    items: &mut Vec<Value>,
) -> Result<String> {
    let mut merged = payload.clone();

    // Apply hook (folder normalize, todo enrich, etc.)
    if let Some(hook) = get_hook(&schema.name) {
        hook.on_create(&mut merged, schema);
    }

    // Merge schema defaults (only fills missing fields)
    let defaults = schema.build_defaults();
    merge_defaults(&mut merged, &defaults);

    // Validate
    if let Some(err) = schema.validate(&merged) {
        return Err(anyhow::anyhow!("{}", err));
    }

    // Ensure id
    let id = merged
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| generate_domain_id(&schema.name));

    if let Value::Object(ref mut map) = merged {
        map.insert("id".to_string(), json!(id));
    }

    items.push(merged);
    backend
        .set_data(&schema.storage_key, &json!(items))
        .await
        .with_context(|| format!("保存 {} 失败", schema.storage_key))?;

    Ok(format!("已创建 {}「{}」", schema.name, id))
}

async fn do_update(
    backend: &BackendClient,
    schema: &DomainSchema,
    target_id: &str,
    payload: &Value,
    items: &mut Vec<Value>,
) -> Result<String> {
    if target_id.is_empty() {
        return Err(anyhow::anyhow!("update 操作需要 target_id"));
    }

    let (parent_idx, child_idx) =
        find_index(items, target_id).ok_or_else(|| anyhow::anyhow!("未找到: {}", target_id))?;

    let mut normalized = payload.clone();
    if let Some(hook) = get_hook(&schema.name) {
        hook.on_before_update(&mut normalized);
    }
    if let Some(err) = schema.validate_patch(&normalized) {
        return Err(anyhow::anyhow!("{}", err));
    }
    let payload = &normalized;

    if let Some(child_idx) = child_idx {
        let parent = items
            .get_mut(parent_idx)
            .ok_or_else(|| anyhow::anyhow!("parent not found"))?;
        let parent_map = match parent {
            Value::Object(m) => m,
            _ => return Err(anyhow::anyhow!("parent type invalid")),
        };
        let mut children = match parent_map.get("links") {
            Some(Value::Array(arr)) => arr.clone(),
            _ => return Err(anyhow::anyhow!("links array invalid")),
        };
        let child = children
            .get_mut(child_idx)
            .ok_or_else(|| anyhow::anyhow!("child not found"))?;
        let child_map = match child {
            Value::Object(m) => m,
            _ => return Err(anyhow::anyhow!("child type invalid")),
        };
        if let Value::Object(p) = payload {
            for (k, v) in p {
                if k == "id" {
                    continue;
                }
                child_map.insert(k.clone(), v.clone());
            }
        }
        parent_map.insert("links".to_string(), Value::Array(children));
    } else {
        let item = items
            .get_mut(parent_idx)
            .ok_or_else(|| anyhow::anyhow!("item not found"))?;
        let item_map = match item {
            Value::Object(m) => m,
            _ => return Err(anyhow::anyhow!("item type invalid")),
        };
        if let Value::Object(p) = payload {
            for (k, v) in p {
                if k == "id" {
                    continue;
                }
                item_map.insert(k.clone(), v.clone());
            }
        }
        if item_map.contains_key("updatedAt") {
            item_map.insert(
                "updatedAt".to_string(),
                json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
            );
        }
    }

    backend
        .set_data(&schema.storage_key, &json!(items))
        .await
        .with_context(|| format!("保存 {} 失败", schema.storage_key))?;

    Ok(format!("已更新 {} {}", schema.name, target_id))
}

async fn do_delete(
    backend: &BackendClient,
    schema: &DomainSchema,
    target_id: &str,
    items: &mut Vec<Value>,
) -> Result<String> {
    if target_id.is_empty() {
        return Err(anyhow::anyhow!("delete 操作需要 target_id"));
    }

    let (parent_idx, child_idx) =
        find_index(items, target_id).ok_or_else(|| anyhow::anyhow!("未找到: {}", target_id))?;

    if let Some(child_idx) = child_idx {
        let parent = items
            .get_mut(parent_idx)
            .ok_or_else(|| anyhow::anyhow!("parent not found"))?;
        let parent_map = match parent {
            Value::Object(m) => m,
            _ => return Err(anyhow::anyhow!("parent type invalid")),
        };
        let mut children = match parent_map.get("links") {
            Some(Value::Array(arr)) => arr.clone(),
            _ => return Err(anyhow::anyhow!("links array invalid")),
        };
        if child_idx < children.len() {
            children.remove(child_idx);
        }
        parent_map.insert("links".to_string(), Value::Array(children));
    } else {
        if parent_idx < items.len() {
            items.remove(parent_idx);
        }
    }

    backend
        .set_data(&schema.storage_key, &json!(items))
        .await
        .with_context(|| format!("保存 {} 失败", schema.storage_key))?;

    Ok(format!("已删除 {} {}", schema.name, target_id))
}

fn do_query(schema: &DomainSchema, payload: &Value, items: &[Value]) -> Result<String> {
    let results = filter_items(items, payload);
    if results.is_empty() {
        return Ok(format!("（暂无 {} 数据）", schema.name));
    }
    Ok(format_results(&schema.name, &results, schema))
}

fn find_index(items: &[Value], target_id: &str) -> Option<(usize, Option<usize>)> {
    let parts: Vec<&str> = target_id.split('/').collect();
    if parts.len() == 1 {
        if let Some(idx) = items
            .iter()
            .position(|x| x.get("id").and_then(|v| v.as_str()) == Some(target_id))
        {
            return Some((idx, None));
        }
        if let Some(idx) = items
            .iter()
            .position(|x| x.get("title").and_then(|v| v.as_str()) == Some(target_id))
        {
            return Some((idx, None));
        }
        return None;
    }
    if parts.len() == 2 {
        let parent_idx = items
            .iter()
            .position(|x| x.get("id").and_then(|v| v.as_str()) == Some(parts[0]))?;
        let parent = items.get(parent_idx)?;
        let child_array = parent.get("links").and_then(|v| v.as_array())?;
        let child_idx = child_array
            .iter()
            .position(|x| x.get("id").and_then(|v| v.as_str()) == Some(parts[1]))?;
        return Some((parent_idx, Some(child_idx)));
    }
    None
}

fn filter_items(items: &[Value], filter: &Value) -> Vec<Value> {
    let filter_obj = match filter.as_object() {
        Some(o) if !o.is_empty() => o,
        _ => return items.to_vec(),
    };
    items
        .iter()
        .filter(|item| {
            let record = match item.as_object() {
                Some(o) => o,
                None => return false,
            };
            for (key, value) in filter_obj {
                if record.get(key) != Some(value) {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect()
}

fn merge_value_payload(current: Value, payload: &Value) -> Value {
    if let Some(value) = payload.get("value") {
        return value.clone();
    }
    match (current, payload) {
        (Value::Object(mut current_map), Value::Object(payload_map)) => {
            for (k, v) in payload_map {
                current_map.insert(k.clone(), v.clone());
            }
            Value::Object(current_map)
        }
        (_, next) => next.clone(),
    }
}

fn merge_defaults(base: &mut Value, defaults: &Value) {
    if let (Value::Object(base_map), Value::Object(def_map)) = (base, defaults) {
        for (k, v) in def_map {
            if !base_map.contains_key(k) {
                base_map.insert(k.clone(), v.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_update_patch_does_not_mutate_existing_item() {
        let backend = BackendClient::new("http://127.0.0.1:1");
        let schema = registry().get("todo").unwrap();
        let original = json!({
            "id": "todo-1",
            "title": "test",
            "type": "today",
            "completed": false
        });
        let mut items = vec![original.clone()];

        let result = do_update(
            &backend,
            schema,
            "todo-1",
            &json!({"completed": "yes"}),
            &mut items,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(items, vec![original]);
    }
}
