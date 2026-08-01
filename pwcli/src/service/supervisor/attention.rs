use serde_json::{json, Value};

use super::AttentionAction;

pub(super) fn normalized_detail(kind: &str, title: &str, detail: Value) -> Value {
    let mut detail = if detail.is_object() {
        detail
    } else {
        json!({})
    };
    let object = detail.as_object_mut().expect("attention detail object");
    let (risk, recommendation, fallback_actions): (&str, &str, &[(&str, &str)]) = match kind {
        "workspace_required" => (
            "选错目录可能读取或修改错误的项目。",
            "选择这个任务实际执行的项目目录；经常在此项目执行时绑定到任务。",
            &[("use_once", "仅本次使用"), ("bind_task", "绑定到任务")],
        ),
        "decision" => (
            "任务在缺少选择时无法安全继续。",
            "选择最符合目标的方案；不确定时先打开原会话。",
            &[("open_task", "打开原会话")],
        ),
        "learning_checkpoint" => (
            "这是你主动标记的学习任务；跳过不会阻止系统继续，但会失去一次高价值判断练习。",
            "先根据已有分析写下你的优先级与验收证据；不想参与时可按系统建议继续。",
            &[
                ("continue_recommended", "按建议继续"),
                ("open_task", "我先作答"),
            ],
        ),
        "permission" => (
            "允许会执行本地工具；拒绝会停止当前操作。",
            "仅在工具、参数和工作目录符合预期时允许本次执行。",
            &[("allow", "允许本次"), ("deny", "拒绝")],
        ),
        "review" => (
            "接受会把当前冻结 revision 合回；旧 revision 必须拒绝。",
            "先阅读 Review Packet，证据充分则接受，否则定向打回。",
            &[("open_review", "查看审阅包"), ("discard", "丢弃改动")],
        ),
        "task_failed" => (
            "目标尚未完成；盲目重复可能再次触发同一失败。",
            "优先重试当前步骤；确认原 CLI 不可用时再批准替换。",
            &[
                ("retry_step", "重试该步骤"),
                ("replace_backend", "改用推荐 CLI"),
            ],
        ),
        "background_failure" => (
            "后台工具没有产出可信结果。",
            "仅在任务标记为可重放时重试，否则回到原会话重新发起。",
            &[("retry", "重试"), ("ignore", "忽略")],
        ),
        "background_interrupted" => (
            "daemon 重启前未记录完成结果，外部副作用状态未知。",
            "回到原会话核对状态；确认安全后再手动重试。",
            &[("open_task", "回到原会话"), ("ignore", "忽略")],
        ),
        "merge_conflict" => (
            "自动合回无法唯一决定冲突内容。",
            "进入原会话按任务目标处理语义冲突，或放弃本次改动。",
            &[("open_task", "处理冲突"), ("ignore", "暂不处理")],
        ),
        _ => (
            "任务需要人工判断后才能继续。",
            "打开关联任务，根据证据选择继续或停止。",
            &[("open_task", "打开任务"), ("ignore", "忽略")],
        ),
    };
    object.entry("reason").or_insert_with(|| title.into());
    object.entry("risk").or_insert_with(|| risk.into());
    object
        .entry("recommendedAction")
        .or_insert_with(|| recommendation.into());
    object.entry("evidenceRefs").or_insert_with(|| json!([]));
    if !object.contains_key("actions") {
        let actions = if kind == "decision" {
            object
                .get("options")
                .and_then(Value::as_array)
                .map(|options| {
                    options
                        .iter()
                        .take(2)
                        .filter_map(|option| {
                            Some(json!({
                                "id": option.get("id")?.as_str()?,
                                "label": option.get("label")?.as_str()?,
                            }))
                        })
                        .collect::<Vec<_>>()
                })
                .filter(|actions| !actions.is_empty())
                .unwrap_or_else(|| {
                    fallback_actions
                        .iter()
                        .map(|(id, label)| json!({ "id": id, "label": label }))
                        .collect()
                })
        } else {
            fallback_actions
                .iter()
                .map(|(id, label)| json!({ "id": id, "label": label }))
                .collect()
        };
        object.insert("actions".into(), Value::Array(actions));
    }
    detail
}

pub(super) fn text(detail: &Value, key: &str) -> String {
    detail
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(super) fn actions(detail: &Value) -> Vec<AttentionAction> {
    detail
        .get("actions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|action| {
            Some(AttentionAction {
                id: action.get("id")?.as_str()?.to_string(),
                label: action.get("label")?.as_str()?.to_string(),
            })
        })
        .take(2)
        .collect()
}

pub(super) fn evidence(detail: &Value) -> Vec<String> {
    detail
        .get("evidenceRefs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}
