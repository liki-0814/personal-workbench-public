use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, RwLock};

use super::client::SshHandle;

#[derive(Debug, Clone)]
pub struct TunnelInfo {
    pub id: String,
    pub alias: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
}

pub struct TunnelManager {
    tunnels: RwLock<HashMap<String, (TunnelInfo, tokio::task::JoinHandle<()>)>>,
}

impl Default for TunnelManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TunnelManager {
    pub fn new() -> Self {
        Self {
            tunnels: RwLock::new(HashMap::new()),
        }
    }

    pub async fn start(
        &self,
        handle: Arc<Mutex<SshHandle>>,
        alias: &str,
        remote_host: &str,
        remote_port: u16,
        local_port: Option<u16>,
    ) -> Result<TunnelInfo> {
        let listener = if let Some(p) = local_port {
            TcpListener::bind(format!("127.0.0.1:{}", p))
                .await
                .with_context(|| format!("绑定本地端口 {} 失败", p))?
        } else {
            TcpListener::bind("127.0.0.1:0")
                .await
                .context("绑定随机端口失败")?
        };

        let actual_port = listener.local_addr()?.port();
        let id = format!("{}-{}", alias, actual_port);

        let info = TunnelInfo {
            id: id.clone(),
            alias: alias.to_string(),
            local_port: actual_port,
            remote_host: remote_host.to_string(),
            remote_port,
        };

        let rh = remote_host.to_string();
        let rp = remote_port;
        let task = tokio::spawn(async move {
            run_tunnel(listener, handle, &rh, rp).await;
        });

        self.tunnels
            .write()
            .await
            .insert(id.clone(), (info.clone(), task));

        Ok(info)
    }

    pub async fn stop(&self, id: &str) -> bool {
        let mut tunnels = self.tunnels.write().await;
        if let Some((_, task)) = tunnels.remove(id) {
            task.abort();
            true
        } else {
            false
        }
    }

    pub async fn list(&self) -> Vec<TunnelInfo> {
        let tunnels = self.tunnels.read().await;
        tunnels.values().map(|(info, _)| info.clone()).collect()
    }
}

async fn run_tunnel(
    listener: TcpListener,
    handle: Arc<Mutex<SshHandle>>,
    remote_host: &str,
    remote_port: u16,
) {
    loop {
        let Ok((mut stream, peer)) = listener.accept().await else {
            break;
        };

        let rh = remote_host.to_string();
        let h = Arc::clone(&handle);
        tokio::spawn(async move {
            let channel = {
                let locked = h.lock().await;
                match locked
                    .channel_open_direct_tcpip(
                        rh,
                        remote_port.into(),
                        peer.ip().to_string(),
                        peer.port().into(),
                    )
                    .await
                {
                    Ok(c) => c,
                    Err(_) => return,
                }
            };

            let mut ssh_stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut ssh_stream).await;
        });
    }
}
