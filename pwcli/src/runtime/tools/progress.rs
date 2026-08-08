//! 工具执行期进度上抛通道。
//!
//! 设计：
//! - 多数工具（data_crud / filesystem / bash / memory / ...）执行就是一次 await，没有
//!   "中间状态"可发——保留它们 `ToolHandler` 现有签名不变。
//! - 个别长跑工具（例如原生 CLI 子 agent）需要把协议事件露给前端。
//!   让这部分工具通过 [`emit`] 主动发，由 [`ToolRegistry::execute_with_progress`]
//!   在调用 handler 前注入 task-local emitter；不需要进度的工具读不到 emitter，直接 no-op。
//!
//! 路径：ACP notification → `progress::emit("ACP tool: Read foo.rs")`
//! → task-local 拿到 sink emitter → `StreamEvent::ToolProgress { id, line }` → SSE
//! → 前端 trace chip 进度区。

/// 进度发射器。每条上抛的"进度行"都会调一次。
/// `'static` 是 task_local 的硬要求，所以闭包不能借用 sink；
/// service 模式下闭包持 `tx.clone()` + 当前 tool_call_id 的 String。
pub use crate::agent_core::contracts::ports::{ImageEmitter, ProgressEmitter, TextDeltaEmitter};

tokio::task_local! {
    static CURRENT_PROGRESS: Option<ProgressEmitter>;
    static CURRENT_IMAGE: Option<ImageEmitter>;
    static CURRENT_TEXT_DELTA: Option<TextDeltaEmitter>;
}

/// 工具内部调用：上抛一行进度。
/// 在 [`ToolRegistry::execute_with_emitters`] scope 外（如直接走 `execute`）会静默 no-op。
pub fn emit(line: &str) {
    let _ = CURRENT_PROGRESS.try_with(|p| {
        if let Some(emitter) = p.as_ref() {
            emitter(line);
        }
    });
}

/// 工具内部调用：上抛一张生成的图（base64 data URL 或 https URL）。
pub fn emit_image(url: &str, alt: &str) {
    let _ = CURRENT_IMAGE.try_with(|p| {
        if let Some(emitter) = p.as_ref() {
            emitter(url, alt, None);
        }
    });
}

pub fn emit_generated_image(
    url: &str,
    alt: &str,
    record: &crate::runtime::visual_generation::GeneratedImageRecord,
) {
    let serialized = serde_json::to_value(record).ok();
    let _ = CURRENT_IMAGE.try_with(|emitter| {
        if let Some(emitter) = emitter.as_ref() {
            emitter(url, alt, serialized.as_ref());
        }
    });
}

/// 合成阶段 LLM 流式调用：每个 token delta 调一次。scope 外为 no-op。
pub fn emit_text_delta(delta: &str) {
    let _ = CURRENT_TEXT_DELTA.try_with(|p| {
        if let Some(emitter) = p.as_ref() {
            emitter(delta);
        }
    });
}

/// 拿到当前 task-local progress emitter 的克隆（Arc 引用计数 +1）。
/// 用于把 emitter 显式传到 [`tokio::spawn`] 起的新 task —— 新 task 的 task_local 是空的，
/// 必须在新 task 里再调一次 [`with_emitters`] 重建 scope，否则 emit 静默 no-op。
pub fn current() -> Option<ProgressEmitter> {
    CURRENT_PROGRESS.try_with(|p| p.clone()).ok().flatten()
}

/// Image emitter 同款 helper。
pub fn current_image() -> Option<ImageEmitter> {
    CURRENT_IMAGE.try_with(|p| p.clone()).ok().flatten()
}

pub fn current_text_delta() -> Option<TextDeltaEmitter> {
    CURRENT_TEXT_DELTA.try_with(|p| p.clone()).ok().flatten()
}

/// 在 progress + image + text_delta emitter scope 内执行 future。registry 用，工具代码不直接调。
pub async fn with_emitters<F, T>(
    progress: Option<ProgressEmitter>,
    image: Option<ImageEmitter>,
    fut: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_PROGRESS
        .scope(
            progress,
            CURRENT_IMAGE.scope(image, CURRENT_TEXT_DELTA.scope(None, fut)),
        )
        .await
}

/// 扩展版：同时注入 text_delta emitter，供长任务工具流式回传进度。
pub async fn with_all_emitters<F, T>(
    progress: Option<ProgressEmitter>,
    image: Option<ImageEmitter>,
    text_delta: Option<TextDeltaEmitter>,
    fut: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_PROGRESS
        .scope(
            progress,
            CURRENT_IMAGE.scope(image, CURRENT_TEXT_DELTA.scope(text_delta, fut)),
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[tokio::test]
    async fn emit_outside_scope_is_noop() {
        // 不在任何 scope 里调 emit，不应 panic
        emit("hello");
    }

    #[tokio::test]
    async fn emit_inside_scope_reaches_emitter() {
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
        let captured2 = Arc::clone(&captured);
        let emitter: ProgressEmitter = Arc::new(move |line: &str| {
            captured2.lock().unwrap().push(line.to_string());
        });

        with_emitters(Some(emitter), None, async {
            emit("first");
            emit("second");
        })
        .await;

        let lines = captured.lock().unwrap();
        assert_eq!(*lines, vec!["first".to_string(), "second".to_string()]);
    }

    #[tokio::test]
    async fn emit_with_none_emitter_is_noop() {
        with_emitters(None, None, async {
            emit("ignored");
        })
        .await;
    }

    #[tokio::test]
    async fn emit_image_inside_scope() {
        let captured: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(vec![]));
        let captured2 = Arc::clone(&captured);
        let emitter: ImageEmitter = Arc::new(move |url: &str, alt: &str, _record| {
            captured2
                .lock()
                .unwrap()
                .push((url.to_string(), alt.to_string()));
        });
        with_emitters(None, Some(emitter), async {
            emit_image("data:image/png;base64,xxx", "a cat");
        })
        .await;
        let items = captured.lock().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, "data:image/png;base64,xxx");
        assert_eq!(items[0].1, "a cat");
    }
}
