//! 工具执行期的会话 / 网页缓存 task-local 上下文。

use std::sync::Arc;

use crate::tools::web_cache::WebFetchCache;
use crate::visual_generation::ImageReferenceRegistry;

tokio::task_local! {
    static CURRENT_WEB_CACHE: Option<Arc<WebFetchCache>>;
    static CURRENT_SESSION_ID: Option<String>;
    static CURRENT_CHAT_SESSION_ID: Option<String>;
    static CURRENT_WORK_ITEM_ID: Option<String>;
    static CURRENT_IMAGE_REFERENCES: Option<Arc<ImageReferenceRegistry>>;
}

pub fn image_references() -> Option<Arc<ImageReferenceRegistry>> {
    CURRENT_IMAGE_REFERENCES
        .try_with(|registry| registry.clone())
        .ok()
        .flatten()
}

pub async fn with_image_references<F, T>(registry: Option<Arc<ImageReferenceRegistry>>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_IMAGE_REFERENCES.scope(registry, fut).await
}

pub fn web_cache() -> Option<Arc<WebFetchCache>> {
    CURRENT_WEB_CACHE.try_with(|c| c.clone()).ok().flatten()
}

pub fn session_id() -> Option<String> {
    CURRENT_SESSION_ID.try_with(|s| s.clone()).ok().flatten()
}

pub fn chat_session_id() -> Option<String> {
    CURRENT_CHAT_SESSION_ID
        .try_with(|session_id| session_id.clone())
        .ok()
        .flatten()
}

pub fn work_item_id() -> Option<String> {
    CURRENT_WORK_ITEM_ID
        .try_with(|work_item_id| work_item_id.clone())
        .ok()
        .flatten()
}

pub async fn with_work_item<F, T>(work_item_id: Option<String>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_WORK_ITEM_ID.scope(work_item_id, fut).await
}

pub async fn with_chat_session<F, T>(chat_session_id: Option<String>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_CHAT_SESSION_ID.scope(chat_session_id, fut).await
}

pub async fn with_web_context<F, T>(
    cache: Option<Arc<WebFetchCache>>,
    session_id: Option<String>,
    fut: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_WEB_CACHE
        .scope(cache, CURRENT_SESSION_ID.scope(session_id, fut))
        .await
}
