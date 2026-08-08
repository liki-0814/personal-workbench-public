use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct DataWrap {
    pub data: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AllDataResponse {
    pub data: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DirEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DirListResponse {
    pub entries: Vec<DirEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileContent {
    pub content: Option<String>,
    /// Daemon 路由 `/api/fs/read` 返回 camelCase `isBinary`；这里强制 rename 对齐。
    /// 缺这个 rename 会导致 reqwest .json() 反序列化失败 → "error decoding response body"，
    /// 从外面看就是 read_file 工具调用神秘报错。
    #[serde(rename = "isBinary")]
    pub is_binary: bool,
    #[serde(default)]
    pub meta: Option<FileMeta>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileMeta {
    pub size: u64,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileSearchResponse {
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub size: u64,
    pub modified: String,
    pub created: String,
}

#[derive(Debug, Clone)]
pub struct BackendClient {
    client: Client,
    base_url: String,
}

impl BackendClient {
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: crate::ai::http::default_client(crate::ai::http::ClientProfile::Backend),
            base_url: base_url.into(),
        }
    }

    pub async fn get_data(&self, key: &str) -> Result<Value> {
        let url = format!("{}/api/data/{}", self.base_url, key);
        let res = self.client.get(&url).send().await?;
        if !res.status().is_success() {
            anyhow::bail!(
                "API error {}: {}",
                res.status(),
                res.text().await.unwrap_or_default()
            );
        }
        let body: DataWrap = res.json().await?;
        Ok(body.data)
    }

    pub async fn set_data(&self, key: &str, value: &Value) -> Result<()> {
        let url = format!("{}/api/data/{}", self.base_url, key);
        let res = self
            .client
            .put(&url)
            .json(&serde_json::json!({ "value": value }))
            .send()
            .await?;
        if !res.status().is_success() {
            anyhow::bail!(
                "API error {}: {}",
                res.status(),
                res.text().await.unwrap_or_default()
            );
        }
        Ok(())
    }

    pub async fn delete_data(&self, key: &str) -> Result<()> {
        let url = format!("{}/api/data/{}", self.base_url, key);
        let res = self.client.delete(&url).send().await?;
        if !res.status().is_success() {
            anyhow::bail!(
                "API error {}: {}",
                res.status(),
                res.text().await.unwrap_or_default()
            );
        }
        Ok(())
    }

    pub async fn get_all_data(&self) -> Result<HashMap<String, String>> {
        let url = format!("{}/api/data", self.base_url);
        let res = self.client.get(&url).send().await?;
        if !res.status().is_success() {
            anyhow::bail!(
                "API error {}: {}",
                res.status(),
                res.text().await.unwrap_or_default()
            );
        }
        let body: AllDataResponse = res.json().await?;
        Ok(body.data)
    }

    pub async fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let url = format!("{}/api/fs/list", self.base_url);
        let res = self
            .client
            .get(&url)
            .query(&[("path", path)])
            .send()
            .await?;
        if !res.status().is_success() {
            anyhow::bail!(
                "API error {}: {}",
                res.status(),
                res.text().await.unwrap_or_default()
            );
        }
        let body: DirListResponse = res.json().await?;
        Ok(body.entries)
    }

    pub async fn read_file(&self, path: &str) -> Result<FileContent> {
        let url = format!("{}/api/fs/read", self.base_url);
        let res = self
            .client
            .get(&url)
            .query(&[("path", path)])
            .send()
            .await?;
        if !res.status().is_success() {
            anyhow::bail!(
                "API error {}: {}",
                res.status(),
                res.text().await.unwrap_or_default()
            );
        }
        let body: FileContent = res.json().await?;
        Ok(body)
    }

    pub async fn search_files(&self, query: &str, dir_path: Option<&str>) -> Result<Vec<String>> {
        let mut query_pairs = vec![("q", query)];
        if let Some(path) = dir_path {
            query_pairs.push(("path", path));
        }
        let url = format!("{}/api/fs/search", self.base_url);
        let res = self.client.get(&url).query(&query_pairs).send().await?;
        if !res.status().is_success() {
            anyhow::bail!(
                "API error {}: {}",
                res.status(),
                res.text().await.unwrap_or_default()
            );
        }
        let body: FileSearchResponse = res.json().await?;
        Ok(body.files)
    }

    pub async fn get_file_info(&self, path: &str) -> Result<FileInfo> {
        let url = format!("{}/api/fs/info", self.base_url);
        let res = self
            .client
            .get(&url)
            .query(&[("path", path)])
            .send()
            .await?;
        if !res.status().is_success() {
            anyhow::bail!(
                "API error {}: {}",
                res.status(),
                res.text().await.unwrap_or_default()
            );
        }
        let body: FileInfo = res.json().await?;
        Ok(body)
    }

    pub async fn jobs_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value> {
        let url = format!("{}/api/jobs{}", self.base_url, path);
        let res = match method {
            "POST" => {
                let mut req = self.client.post(&url);
                if let Some(b) = body {
                    req = req.json(b);
                }
                req.send().await?
            }
            "PUT" => {
                let mut req = self.client.put(&url);
                if let Some(b) = body {
                    req = req.json(b);
                }
                req.send().await?
            }
            "DELETE" => self.client.delete(&url).send().await?,
            _ => self.client.get(&url).send().await?,
        };
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("Jobs API error {}: {}", status, text);
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    #[tokio::test]
    async fn test_get_data() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/api/data/test_key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"foo":"bar"}}"#)
            .create();

        let client = BackendClient::new(server.url());
        let result = client.get_data("test_key").await.unwrap();
        assert_eq!(result["foo"], "bar");
        mock.assert();
    }

    #[tokio::test]
    async fn test_list_dir() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/api/fs/list")
            .match_query(mockito::Matcher::UrlEncoded("path".into(), "/tmp".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"success":true,"entries":[{"name":"foo.txt","type":"file","size":123}]}"#,
            )
            .create();

        let client = BackendClient::new(server.url());
        let entries = client.list_dir("/tmp").await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "foo.txt");
        assert_eq!(entries[0].kind, "file");
        mock.assert();
    }

    #[tokio::test]
    async fn test_search_files() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/api/fs/search")
            .match_query(mockito::Matcher::AllOf(vec![mockito::Matcher::UrlEncoded(
                "q".into(),
                "test".into(),
            )]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"files":["/a.txt","/b.txt"]}"#)
            .create();

        let client = BackendClient::new(server.url());
        let files = client.search_files("test", None).await.unwrap();
        assert_eq!(files.len(), 2);
        mock.assert();
    }
}
