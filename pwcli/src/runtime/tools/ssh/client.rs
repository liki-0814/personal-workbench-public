use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use russh::keys::{load_secret_key, PrivateKeyWithHashAlg};
use russh::{client, ChannelMsg, Disconnect};

use super::config::{AuthMethod, SshServer};

pub struct Handler;

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

pub type SshHandle = client::Handle<Handler>;

pub async fn connect(server: &SshServer) -> Result<SshHandle> {
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(600)),
        keepalive_interval: Some(Duration::from_secs(60)),
        ..<_>::default()
    });

    let addr = (server.host.as_str(), server.port);
    eprintln!(
        "[ssh] 连接 {}@{}:{}...",
        server.user, server.host, server.port
    );
    let mut session = client::connect(config, addr, Handler)
        .await
        .map_err(|e| {
            eprintln!("[ssh] 连接失败: {:?}", e);
            e
        })
        .with_context(|| format!("SSH 连接 {}:{} 失败", server.host, server.port))?;

    match &server.auth {
        AuthMethod::Password { value } => {
            eprintln!("[ssh] 密码认证 {}@{}...", server.user, server.host);
            let res = session
                .authenticate_password(&server.user, value)
                .await
                .map_err(|e| {
                    eprintln!("[ssh] 密码认证失败: {:?}", e);
                    e
                })
                .context("密码认证传输失败")?;
            if !res.success() {
                eprintln!("[ssh] 密码被拒绝: {}@{}", server.user, server.host);
                anyhow::bail!("密码认证被拒绝: {}@{}", server.user, server.host);
            }
            eprintln!("[ssh] 认证成功");
        }
        AuthMethod::Key { path, passphrase } => {
            let expanded = shellexpand::tilde(path);
            let key = load_secret_key(expanded.as_ref(), passphrase.as_deref())
                .with_context(|| format!("加载密钥 {} 失败", path))?;
            let res = session
                .authenticate_publickey(
                    &server.user,
                    PrivateKeyWithHashAlg::new(
                        Arc::new(key),
                        session
                            .best_supported_rsa_hash()
                            .await
                            .ok()
                            .flatten()
                            .flatten(),
                    ),
                )
                .await
                .context("密钥认证传输失败")?;
            if !res.success() {
                anyhow::bail!("密钥认证被拒绝: {}@{}", server.user, server.host);
            }
        }
    }

    Ok(session)
}

pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<u32>,
    /// 命令在 `timeout_secs` 内未结束；stdout/stderr 保留已收到的部分输出。
    pub timed_out: bool,
}

pub async fn exec(
    handle: &tokio::sync::Mutex<SshHandle>,
    command: &str,
    timeout_secs: u64,
) -> Result<ExecResult> {
    let mut channel = {
        let h = handle.lock().await;
        h.channel_open_session()
            .await
            .context("打开 SSH channel 失败")?
    };

    channel
        .exec(true, command)
        .await
        .context("发送 exec 命令失败")?;

    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();
    let mut exit_code: Option<u32> = None;

    let result = tokio::time::timeout(Duration::from_secs(timeout_secs), async {
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => stdout_buf.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, ext: 1 } => stderr_buf.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => exit_code = Some(exit_status),
                ChannelMsg::Close | ChannelMsg::Eof => break,
                _ => {}
            }
        }
    })
    .await;

    // 超时不吞掉已收到的输出：带回部分结果，由上层（工具 handler）组织成
    // 带指导信息的错误，交回模型决定重试/加大超时/转后台。
    let timed_out = result.is_err();
    if timed_out {
        // 尽力关闭 channel，忽略远端已断开等错误
        let _ = channel.close().await;
    }

    Ok(ExecResult {
        stdout: String::from_utf8_lossy(&stdout_buf).into(),
        stderr: String::from_utf8_lossy(&stderr_buf).into(),
        exit_code,
        timed_out,
    })
}

pub async fn disconnect(handle: &SshHandle) -> Result<()> {
    handle
        .disconnect(Disconnect::ByApplication, "", "")
        .await
        .ok();
    Ok(())
}
