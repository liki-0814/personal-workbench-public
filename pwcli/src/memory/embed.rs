// FastEmbed MultilingualE5Small 封装：passage/query 前缀区分检索方向
use anyhow::Result;
#[cfg(any(feature = "embeddings", test))]
use std::path::{Path, PathBuf};

#[cfg(feature = "embeddings")]
use anyhow::Context;
#[cfg(feature = "embeddings")]
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
#[cfg(feature = "embeddings")]
use std::sync::Mutex;

#[cfg(feature = "embeddings")]
pub struct MemoryEmbedder {
    model: Mutex<TextEmbedding>,
}

#[cfg(not(feature = "embeddings"))]
pub struct MemoryEmbedder;

#[cfg(any(feature = "embeddings", test))]
fn fastembed_cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("cache").join("fastembed")
}

#[cfg(feature = "embeddings")]
impl MemoryEmbedder {
    /// 首次调用会下载模型权重（fastembed ort-download-binaries feature）
    pub fn new() -> Result<Self> {
        let cache_dir = fastembed_cache_dir(&crate::config::local_config::data_dir());
        std::fs::create_dir_all(&cache_dir)
            .with_context(|| format!("create FastEmbed cache directory {}", cache_dir.display()))?;
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::MultilingualE5Small)
                .with_cache_dir(cache_dir)
                .with_show_download_progress(false),
        )
        .context("init MultilingualE5Small embedder")?;
        Ok(Self {
            model: Mutex::new(model),
        })
    }

    /// 文档/记忆正文 embedding（passage 前缀）
    pub fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        let input = format!("passage: {}", text.trim());
        let mut model = self
            .model
            .lock()
            .map_err(|e| anyhow::anyhow!("embedder lock poisoned: {}", e))?;
        let embeddings = model.embed(vec![input], None).context("embed passage")?;
        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("embed returned empty result"))
    }

    /// 查询 embedding（query 前缀，与 passage 非对称检索）
    pub fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        let input = format!("query: {}", query.trim());
        let mut model = self
            .model
            .lock()
            .map_err(|e| anyhow::anyhow!("embedder lock poisoned: {}", e))?;
        let embeddings = model.embed(vec![input], None).context("embed query")?;
        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("embed returned empty result"))
    }
}

#[cfg(not(feature = "embeddings"))]
impl MemoryEmbedder {
    pub fn new() -> Result<Self> {
        anyhow::bail!("pwcli was built without vector embedding support")
    }

    pub fn embed_text(&self, _text: &str) -> Result<Vec<f32>> {
        anyhow::bail!("pwcli was built without vector embedding support")
    }

    pub fn embed_query(&self, _query: &str) -> Result<Vec<f32>> {
        anyhow::bail!("pwcli was built without vector embedding support")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_dir_is_scoped_to_configured_data_dir() {
        let data_dir = Path::new("/tmp/pwcli-data");
        assert_eq!(
            fastembed_cache_dir(data_dir),
            data_dir.join("cache").join("fastembed")
        );
    }
}
