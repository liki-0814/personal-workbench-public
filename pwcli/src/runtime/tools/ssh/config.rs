use crate::runtime::backend::BackendClient;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SshServer {
    pub id: String,
    pub alias: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    #[serde(default)]
    pub auth: AuthMethod,
    #[serde(default)]
    pub proxy_jump: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_env")]
    pub environment: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub forward_agent: bool,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

fn default_port() -> u16 {
    22
}
fn default_env() -> String {
    "development".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AuthMethod {
    #[serde(rename = "key")]
    Key {
        path: String,
        #[serde(default)]
        passphrase: Option<String>,
    },
    #[serde(rename = "password")]
    Password { value: String },
}

impl Default for AuthMethod {
    fn default() -> Self {
        AuthMethod::Key {
            path: "~/.ssh/id_rsa".into(),
            passphrase: None,
        }
    }
}

pub async fn load_servers(_backend: &BackendClient) -> Result<Vec<SshServer>> {
    Ok(crate::runtime::settings::local_config::get()
        .tools
        .ssh_servers
        .clone())
}

pub async fn find_server(backend: &BackendClient, alias: &str) -> Result<SshServer> {
    let servers = load_servers(backend).await?;
    servers
        .into_iter()
        .find(|s| s.alias == alias)
        .with_context(|| format!("SSH 服务器别名 '{}' 不存在", alias))
}
