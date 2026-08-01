use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{json, Value};

include!(concat!(env!("OUT_DIR"), "/schemas_generated.rs"));

#[derive(Debug, Clone)]
pub struct DomainSchema {
    pub name: String,
    pub storage_key: String,
    pub collection: bool,
    pub catalog: bool,
    pub display_field: String,
    pub description: String,
    pub fields: IndexMap<String, FieldDef>,
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub field_type: FieldType,
    pub required: bool,
    pub default: Option<Value>,
    pub auto: Option<AutoFill>,
    pub values: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    String,
    Number,
    Bool,
    Date,
    Datetime,
    Array,
    Object,
    Enum,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoFill {
    Now,
    Today,
}

pub struct DomainRegistry(Vec<DomainSchema>);

// Raw TOML deserialization types
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawField {
    #[serde(rename = "type")]
    field_type: FieldType,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    default: Option<toml::Value>,
    #[serde(default)]
    auto: Option<AutoFill>,
    #[serde(default)]
    values: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDomain {
    storage_key: String,
    #[serde(default = "default_true")]
    collection: bool,
    #[serde(default)]
    catalog: bool,
    #[serde(default = "default_display_field")]
    display_field: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    fields: IndexMap<String, RawField>,
}

fn default_true() -> bool {
    true
}
fn default_display_field() -> String {
    "title".to_string()
}

impl DomainRegistry {
    pub fn load() -> Self {
        let mut schemas = Vec::new();
        for (file, toml_content) in SCHEMA_FILES {
            let table: IndexMap<String, RawDomain> = toml::from_str(toml_content)
                .unwrap_or_else(|error| panic!("failed to parse domain schema {file}: {error}"));
            for (name, raw) in table {
                schemas.push(DomainSchema::from_raw(name, raw));
            }
        }
        Self(schemas)
    }

    pub fn get(&self, name: &str) -> Option<&DomainSchema> {
        self.0.iter().find(|s| s.name == name)
    }

    pub fn catalog_domains(&self) -> Vec<(&str, &str)> {
        self.0
            .iter()
            .filter(|s| s.catalog)
            .map(|s| (s.name.as_str(), s.storage_key.as_str()))
            .collect()
    }

    pub fn build_tool_description(&self) -> String {
        let domain_list = self
            .0
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        let field_hints: Vec<String> = self
            .0
            .iter()
            .map(|s| {
                let fields: Vec<String> = s
                    .fields
                    .iter()
                    .map(|(name, f)| {
                        let mut desc = format!("{}: {}", name, f.field_type.as_str());
                        if let Some(vals) = &f.values {
                            desc.push_str(&format!("({})", vals.join("|")));
                        }
                        if f.required {
                            desc.push_str("，必填");
                        }
                        if let Some(default) = &f.default {
                            desc.push_str(&format!("，默认 {}", default));
                        }
                        desc
                    })
                    .collect();
                let heading = if s.description.is_empty() {
                    s.name.clone()
                } else {
                    format!("{}（{}）", s.name, s.description)
                };
                if fields.is_empty() {
                    format!("- {}", heading)
                } else {
                    format!("- {} 字段：{}", heading, fields.join(", "))
                }
            })
            .collect();

        format!(
            "通用数据操作接口。通过指定 domain、action、target_id 和 payload 来创建/读取/更新/删除任何领域数据。\n\
可用领域：{}\n\
{}\n\
操作类型：create, read, update, delete, query, batch\n\
- create: payload 包含新对象字段（id 可选，自动生成）。尽可能填满结构化字段。\n\
- read/query: payload 作为过滤条件\n\
- update: target_id 必填，payload 只包含修改的字段\n\
- delete: target_id 必填，payload 留空\n\
- batch: payload.operations 包含多个子操作\n\
嵌套数据用 target_id = \"parentId/childId\"",
            domain_list,
            field_hints.join("\n")
        )
    }

    pub fn build_tool_schema(&self) -> Value {
        let domain_enum: Vec<Value> = self
            .0
            .iter()
            .map(|s| Value::String(s.name.clone()))
            .collect();

        json!({
            "type": "object",
            "properties": {
                "domain": {
                    "type": "string",
                    "enum": domain_enum,
                    "description": "数据领域"
                },
                "action": {
                    "type": "string",
                    "enum": ["create", "read", "update", "delete", "query", "batch"],
                    "description": "操作类型"
                },
                "target_id": {
                    "type": "string",
                    "description": "目标记录 ID。create 时留空；update/delete 时必填。"
                },
                "payload": {
                    "type": "object",
                    "description": "操作数据。create 填字段；update 只填修改字段；delete 留空；batch 填 {operations: [...]}。"
                }
            },
            "required": ["domain", "action"]
        })
    }
}

impl DomainSchema {
    fn from_raw(name: String, raw: RawDomain) -> Self {
        let fields = raw
            .fields
            .into_iter()
            .map(|(field_name, raw_field)| {
                let field_def = FieldDef {
                    field_type: raw_field.field_type,
                    required: raw_field.required,
                    default: raw_field.default.map(toml_to_json),
                    auto: raw_field.auto,
                    values: raw_field.values,
                };
                (field_name, field_def)
            })
            .collect();

        Self {
            name,
            storage_key: raw.storage_key,
            collection: raw.collection,
            catalog: raw.catalog,
            display_field: raw.display_field,
            description: raw.description,
            fields,
        }
    }

    pub fn build_defaults(&self) -> Value {
        let mut map = serde_json::Map::new();
        for (name, field) in &self.fields {
            if let Some(auto) = &field.auto {
                match auto {
                    AutoFill::Now => {
                        map.insert(
                            name.clone(),
                            Value::String(
                                chrono::Utc::now()
                                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                            ),
                        );
                    }
                    AutoFill::Today => {
                        map.insert(
                            name.clone(),
                            Value::String(chrono::Local::now().format("%Y-%m-%d").to_string()),
                        );
                    }
                }
            } else if let Some(default) = &field.default {
                map.insert(name.clone(), default.clone());
            }
        }
        Value::Object(map)
    }

    pub fn validate(&self, item: &Value) -> Option<String> {
        self.validate_fields(item, true)
    }

    pub fn validate_patch(&self, patch: &Value) -> Option<String> {
        self.validate_fields(patch, false)
    }

    fn validate_fields(&self, item: &Value, require_missing: bool) -> Option<String> {
        let Some(map) = item.as_object() else {
            return Some(format!("{} 必须是对象", self.name));
        };
        for (name, field) in &self.fields {
            let val = map.get(name);
            if field.required && (require_missing || val.is_some()) {
                let empty = match val {
                    None => true,
                    Some(Value::Null) => true,
                    Some(Value::String(s)) => s.trim().is_empty(),
                    _ => false,
                };
                if empty {
                    return Some(format!("{} 的 {} 不能为空", self.name, name));
                }
            }
            let Some(value) = val else {
                continue;
            };
            if !field.field_type.matches(value) {
                return Some(format!(
                    "{} 的 {} 类型不合法，期望 {}",
                    self.name,
                    name,
                    field.field_type.as_str()
                ));
            }
            if let Some(values) = &field.values {
                if let Value::String(v) = value {
                    if !v.is_empty() && !values.contains(v) {
                        return Some(format!(
                            "{} 的 {} 值 \"{}\" 不合法，可选：{}",
                            self.name,
                            name,
                            v,
                            values.join(", ")
                        ));
                    }
                }
            }
        }
        None
    }
}

impl FieldType {
    fn as_str(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Bool => "bool",
            Self::Date => "date",
            Self::Datetime => "datetime",
            Self::Array => "array",
            Self::Object => "object",
            Self::Enum => "enum",
        }
    }

    fn matches(&self, value: &Value) -> bool {
        match self {
            Self::String | Self::Date | Self::Datetime | Self::Enum => value.is_string(),
            Self::Number => value.is_number(),
            Self::Bool => value.is_boolean(),
            Self::Array => value.is_array(),
            Self::Object => value.is_object(),
        }
    }
}

fn toml_to_json(v: toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => json!(i),
        toml::Value::Float(f) => json!(f),
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Array(arr) => Value::Array(arr.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => {
            let map: serde_json::Map<String, Value> =
                t.into_iter().map(|(k, v)| (k, toml_to_json(v))).collect();
            Value::Object(map)
        }
        toml::Value::Datetime(dt) => Value::String(dt.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_all_schemas() {
        let registry = DomainRegistry::load();
        assert!(registry.get("todo").is_some());
        assert!(registry.get("chat_session").is_some());
        assert!(registry.get("chat_folder").is_some());
        assert!(registry.get("theme").is_some());
    }

    #[test]
    fn todo_schema_has_fields() {
        let registry = DomainRegistry::load();
        let todo = registry.get("todo").unwrap();
        assert!(todo.collection);
        assert!(todo.catalog);
        assert_eq!(todo.storage_key, "todos");
        assert!(todo.fields.contains_key("title"));
        assert!(todo.fields["title"].required);
        assert_eq!(todo.fields["status"].field_type, FieldType::Enum);
        assert_eq!(todo.fields["progress"].field_type, FieldType::Number);
        let todo_types: Vec<&str> = todo.fields["type"]
            .values
            .as_ref()
            .unwrap()
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(todo_types, ["today", "week", "longterm"]);
    }

    #[test]
    fn singleton_has_no_fields() {
        let registry = DomainRegistry::load();
        let theme = registry.get("theme").unwrap();
        assert!(!theme.collection);
        assert!(theme.fields.is_empty());
    }

    #[test]
    fn validate_required_field() {
        let registry = DomainRegistry::load();
        let todo = registry.get("todo").unwrap();
        let item = json!({"title": ""});
        assert!(todo.validate(&item).is_some());
        let item = json!({"title": "buy milk"});
        assert!(todo.validate(&item).is_none());
    }

    #[test]
    fn validate_enum_field() {
        let registry = DomainRegistry::load();
        let todo = registry.get("todo").unwrap();
        let item = json!({"title": "test", "priority": "urgent"});
        let err = todo.validate(&item);
        assert!(err.is_some());
        assert!(err.unwrap().contains("不合法"));
    }

    #[test]
    fn validate_declared_field_types() {
        let registry = DomainRegistry::load();
        let todo = registry.get("todo").unwrap();
        assert!(todo
            .validate(&json!({"title": "test", "completed": "yes"}))
            .unwrap()
            .contains("期望 bool"));
        assert!(todo
            .validate(&json!({"title": "test", "subTasks": {}}))
            .unwrap()
            .contains("期望 array"));
    }

    #[test]
    fn validate_patch_checks_present_fields_only() {
        let registry = DomainRegistry::load();
        let todo = registry.get("todo").unwrap();
        assert!(todo.validate_patch(&json!({"priority": "high"})).is_none());
        assert!(todo.validate_patch(&json!({"futureField": 1})).is_none());
        assert!(todo.validate_patch(&json!({"title": ""})).is_some());
        assert!(todo
            .validate_patch(&json!({"priority": "urgent"}))
            .is_some());
    }

    #[test]
    fn build_defaults_has_auto_now() {
        let registry = DomainRegistry::load();
        let todo = registry.get("todo").unwrap();
        let defaults = todo.build_defaults();
        assert!(defaults.get("createdAt").is_some());
        assert_eq!(defaults.get("completed"), Some(&Value::Bool(false)));
        assert_eq!(defaults.get("progress"), Some(&json!(0)));
    }

    #[test]
    fn corrected_domain_contracts_match_persisted_models() {
        let registry = DomainRegistry::load();

        let goal = registry.get("goal").unwrap();
        assert_eq!(goal.build_defaults().get("status"), Some(&json!("active")));

        let pomodoro = registry.get("pomodoro_record").unwrap();
        assert_eq!(pomodoro.display_field, "todoTitle");
        for field in ["mode", "duration", "startTime", "endTime"] {
            assert!(pomodoro.fields[field].required);
        }
        assert!(!pomodoro.fields.contains_key("createdAt"));
        assert!(registry.get("ssh_server").is_none());
    }

    #[test]
    fn schema_defaults_match_declared_types() {
        let registry = DomainRegistry::load();
        for schema in &registry.0 {
            for (name, field) in &schema.fields {
                if let Some(default) = &field.default {
                    assert!(
                        field.field_type.matches(default),
                        "{}.{} has invalid default {}",
                        schema.name,
                        name,
                        default
                    );
                }
            }
        }
    }

    #[test]
    fn rejects_unknown_field_type_and_auto_fill() {
        let unknown_type = r#"
[sample]
storage_key = "sample"

[sample.fields.value]
type = "bol"
"#;
        assert!(toml::from_str::<IndexMap<String, RawDomain>>(unknown_type).is_err());

        let unknown_auto = r#"
[sample]
storage_key = "sample"

[sample.fields.value]
type = "datetime"
auto = "todoy"
"#;
        assert!(toml::from_str::<IndexMap<String, RawDomain>>(unknown_auto).is_err());
    }

    #[test]
    fn tool_description_includes_domain_description_and_types() {
        let description = DomainRegistry::load().build_tool_description();
        assert!(description.contains("todo（待办任务）"));
        assert!(description.contains("title: string，必填"));
        assert!(description.contains("type: enum(today|week|longterm)"));
    }

    #[test]
    fn catalog_domains_subset() {
        let registry = DomainRegistry::load();
        let catalog = registry.catalog_domains();
        assert!(catalog.iter().any(|(name, _)| *name == "todo"));
        assert!(!catalog.iter().any(|(name, _)| *name == "theme"));
        assert!(!catalog.iter().any(|(name, _)| *name == "chat_session"));
    }
}
