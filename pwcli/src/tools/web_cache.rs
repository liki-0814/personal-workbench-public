//! 会话级网页抓取缓存（同一 session 内相同 URL/mode/offset 复用结果）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

pub const WEB_CACHE_TTL: Duration = Duration::from_secs(600);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WebCacheKey {
    pub session_id: String,
    pub mode: String,
    pub url: String,
    pub offset: usize,
    pub selector: Option<String>,
}

impl WebCacheKey {
    pub fn new(
        session_id: impl Into<String>,
        mode: impl Into<String>,
        url: impl Into<String>,
        offset: usize,
        selector: Option<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            mode: mode.into(),
            url: url.into(),
            offset,
            selector,
        }
    }
}

struct WebCacheEntry {
    content: String,
    created_at: Instant,
}

#[derive(Clone, Default)]
pub struct WebFetchCache {
    inner: Arc<RwLock<HashMap<WebCacheKey, WebCacheEntry>>>,
}

impl WebFetchCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get(&self, key: &WebCacheKey) -> Option<String> {
        let guard = self.inner.read().await;
        guard.get(key).and_then(|entry| {
            if entry.created_at.elapsed() <= WEB_CACHE_TTL {
                Some(entry.content.clone())
            } else {
                None
            }
        })
    }

    pub async fn put(&self, key: WebCacheKey, content: String) {
        let mut guard = self.inner.write().await;
        guard.insert(
            key,
            WebCacheEntry {
                content,
                created_at: Instant::now(),
            },
        );
    }

    pub async fn clear_session(&self, session_id: &str) {
        let mut guard = self.inner.write().await;
        guard.retain(|k, _| k.session_id != session_id);
    }

    pub async fn prune_expired(&self) {
        let mut guard = self.inner.write().await;
        guard.retain(|_, entry| entry.created_at.elapsed() <= WEB_CACHE_TTL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cache_hit_within_ttl() {
        let cache = WebFetchCache::new();
        let key = WebCacheKey::new("s1", "read", "https://example.com", 0, None);
        cache.put(key.clone(), "hello".to_string()).await;
        assert_eq!(cache.get(&key).await, Some("hello".to_string()));
    }

    #[tokio::test]
    async fn clear_session_removes_entries() {
        let cache = WebFetchCache::new();
        let key = WebCacheKey::new("s1", "read", "https://example.com", 0, None);
        cache.put(key.clone(), "x".to_string()).await;
        cache.clear_session("s1").await;
        assert!(cache.get(&key).await.is_none());
    }
}
