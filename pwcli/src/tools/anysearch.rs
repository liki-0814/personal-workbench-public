//! Native AnySearch MCP client.
//!
//! AnySearch is the primary public-web search/extraction provider. The API key
//! is optional and comes exclusively from `~/.pwcli/config.json` at
//! `tools.anySearch.apiKey`; an empty key uses AnySearch's anonymous quota.

use anyhow::Context;
use serde_json::{json, Value};

use crate::tools::web::shared_http_client;

const ANYSEARCH_ENDPOINT: &str = "https://api.anysearch.com/mcp";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchItem {
    pub title: String,
    pub url: String,
    pub description: String,
}

async fn call_tool(name: &str, arguments: Value) -> anyhow::Result<String> {
    let client = shared_http_client()?;
    let api_key = crate::config::local_config::get()
        .tools
        .any_search
        .api_key
        .trim()
        .to_string();
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    });
    let mut request = client
        .post(ANYSEARCH_ENDPOINT)
        .timeout(std::time::Duration::from_secs(30))
        .header("Accept", "application/json, text/event-stream")
        .header("User-Agent", "pwcli-anysearch/1.0")
        .json(&payload);
    if !api_key.is_empty() {
        request = request.bearer_auth(api_key);
    }

    let response = request.send().await.context("AnySearch 请求失败")?;
    let status = response.status();
    let body = response.text().await.context("读取 AnySearch 响应失败")?;
    if !status.is_success() {
        anyhow::bail!("AnySearch HTTP {}: {}", status, truncate_error(&body));
    }
    let value: Value = serde_json::from_str(&body).context("AnySearch 返回了无效 JSON")?;
    extract_text_response(&value)
}

fn truncate_error(text: &str) -> String {
    text.chars().take(500).collect()
}

fn extract_text_response(value: &Value) -> anyhow::Result<String> {
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("未知错误");
        anyhow::bail!("AnySearch: {message}");
    }
    value
        .pointer("/result/content")
        .and_then(Value::as_array)
        .and_then(|content| {
            content.iter().find_map(|item| {
                (item.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| item.get("text").and_then(Value::as_str))
                    .flatten()
            })
        })
        .map(str::to_string)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("AnySearch 响应中没有文本内容"))
}

fn site_scoped_query(query: &str, site: Option<&str>) -> String {
    let Some(site) = site.map(str::trim).filter(|site| !site.is_empty()) else {
        return query.to_string();
    };
    let host = site
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    format!("site:{host} {query}")
}

fn search_arguments(
    query: &str,
    max_results: usize,
    site: Option<&str>,
    domain: Option<&str>,
    sub_domain: Option<&str>,
    sub_domain_params: Option<&Value>,
) -> Value {
    let mut args = json!({
        "query": site_scoped_query(query, site),
        "max_results": max_results.clamp(1, 10),
    });
    if let Some(domain) = domain.map(str::trim).filter(|domain| !domain.is_empty()) {
        args["domain"] = Value::String(domain.to_string());
    }
    if let Some(sub_domain) = sub_domain
        .map(str::trim)
        .filter(|sub_domain| !sub_domain.is_empty())
    {
        args["sub_domain"] = Value::String(sub_domain.to_string());
    }
    if let Some(params) = sub_domain_params {
        args["sub_domain_params"] = params.clone();
    }
    args
}

pub(crate) async fn search_markdown(
    query: &str,
    max_results: usize,
    site: Option<&str>,
    domain: Option<&str>,
) -> anyhow::Result<String> {
    search_markdown_with_options(query, max_results, site, domain, None, None).await
}

pub(crate) async fn search_markdown_with_options(
    query: &str,
    max_results: usize,
    site: Option<&str>,
    domain: Option<&str>,
    sub_domain: Option<&str>,
    sub_domain_params: Option<&Value>,
) -> anyhow::Result<String> {
    call_tool(
        "search",
        search_arguments(
            query,
            max_results,
            site,
            domain,
            sub_domain,
            sub_domain_params,
        ),
    )
    .await
}

pub(crate) async fn batch_search_markdown(
    queries: &[String],
    max_results: usize,
    site: Option<&str>,
    domain: Option<&str>,
    sub_domain: Option<&str>,
    sub_domain_params: Option<&Value>,
) -> anyhow::Result<String> {
    call_tool(
        "batch_search",
        batch_search_arguments(
            queries,
            max_results,
            site,
            domain,
            sub_domain,
            sub_domain_params,
        )?,
    )
    .await
}

fn batch_search_arguments(
    queries: &[String],
    max_results: usize,
    site: Option<&str>,
    domain: Option<&str>,
    sub_domain: Option<&str>,
    sub_domain_params: Option<&Value>,
) -> anyhow::Result<Value> {
    if queries.is_empty() || queries.len() > 5 {
        anyhow::bail!("AnySearch batch_search 只支持 1-5 条查询");
    }
    let queries = queries
        .iter()
        .map(|query| {
            search_arguments(
                query,
                max_results,
                site,
                domain,
                sub_domain,
                sub_domain_params,
            )
        })
        .collect::<Vec<_>>();
    Ok(json!({ "queries": queries }))
}

pub(crate) async fn get_sub_domains(domains: &[String]) -> anyhow::Result<String> {
    call_tool("get_sub_domains", sub_domains_arguments(domains)?).await
}

fn sub_domains_arguments(domains: &[String]) -> anyhow::Result<Value> {
    let domains = domains
        .iter()
        .map(|domain| domain.trim())
        .filter(|domain| !domain.is_empty())
        .collect::<Vec<_>>();
    match domains.as_slice() {
        [] => anyhow::bail!("至少需要一个 AnySearch domain"),
        [domain] => Ok(json!({ "domain": domain })),
        _ => Ok(json!({ "domains": domains })),
    }
}

pub(crate) async fn search_items(
    query: &str,
    max_results: usize,
    site: Option<&str>,
    domain: Option<&str>,
) -> anyhow::Result<Vec<SearchItem>> {
    let markdown = search_markdown(query, max_results, site, domain).await?;
    let items = parse_search_items(&markdown);
    if items.is_empty() {
        anyhow::bail!("AnySearch 未返回可解析的搜索结果");
    }
    Ok(items)
}

pub(crate) async fn extract(url: &str) -> anyhow::Result<String> {
    call_tool("extract", json!({ "url": url })).await
}

fn parse_search_items(markdown: &str) -> Vec<SearchItem> {
    let mut items = Vec::new();
    let mut title = String::new();
    let mut url = String::new();
    let mut description = String::new();

    let flush = |items: &mut Vec<SearchItem>,
                 title: &mut String,
                 url: &mut String,
                 description: &mut String| {
        if !url.is_empty() {
            items.push(SearchItem {
                title: if title.is_empty() {
                    url.clone()
                } else {
                    std::mem::take(title)
                },
                url: std::mem::take(url),
                description: std::mem::take(description),
            });
        }
        title.clear();
        description.clear();
    };

    for line in markdown.lines().map(str::trim) {
        if let Some(raw_title) = line.strip_prefix("### ") {
            flush(&mut items, &mut title, &mut url, &mut description);
            title = raw_title
                .split_once(". ")
                .map(|(_, value)| value)
                .unwrap_or(raw_title)
                .trim_matches('*')
                .to_string();
        } else if let Some(raw_url) = line.strip_prefix("- **URL**:") {
            url = raw_url.trim().trim_matches(['<', '>']).to_string();
        } else if !url.is_empty() && description.is_empty() {
            if let Some(raw_description) = line.strip_prefix("- ") {
                if !raw_description.starts_with("**") {
                    description = raw_description.trim().to_string();
                }
            }
        }
    }
    flush(&mut items, &mut title, &mut url, &mut description);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_mcp_text_content() {
        let value = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "content": [{ "type": "text", "text": "ok" }] }
        });
        assert_eq!(extract_text_response(&value).unwrap(), "ok");
    }

    #[test]
    fn parses_anysearch_markdown_results() {
        let markdown = r#"## Search Results
### 1. First result
- **URL**: https://example.com/one
- First description

### 2. Second result
- **URL**: https://example.com/two
- Second description"#;
        let items = parse_search_items(markdown);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "First result");
        assert_eq!(items[0].url, "https://example.com/one");
        assert_eq!(items[1].description, "Second description");
    }

    #[test]
    fn builds_vertical_search_arguments() {
        let params = json!({"type": "stock", "symbol": "AAPL", "cn_code": ""});
        let args = search_arguments(
            "AAPL",
            20,
            None,
            Some("finance"),
            Some("finance.quote"),
            Some(&params),
        );
        assert_eq!(args["max_results"], 10);
        assert_eq!(args["domain"], "finance");
        assert_eq!(args["sub_domain"], "finance.quote");
        assert_eq!(args["sub_domain_params"], params);
    }

    #[test]
    fn builds_batch_search_arguments() {
        let queries = vec!["AAPL".to_string(), "MSFT".to_string()];
        let args = batch_search_arguments(
            &queries,
            3,
            None,
            Some("finance"),
            Some("finance.quote"),
            None,
        )
        .unwrap();
        assert_eq!(args["queries"].as_array().unwrap().len(), 2);
        assert_eq!(args["queries"][0]["query"], "AAPL");
        assert_eq!(args["queries"][1]["max_results"], 3);
        assert_eq!(args["queries"][1]["sub_domain"], "finance.quote");
    }

    #[test]
    fn builds_single_and_multiple_domain_arguments() {
        assert_eq!(
            sub_domains_arguments(&["finance".into()]).unwrap(),
            json!({"domain": "finance"})
        );
        assert_eq!(
            sub_domains_arguments(&["finance".into(), "health".into()]).unwrap(),
            json!({"domains": ["finance", "health"]})
        );
    }
}
