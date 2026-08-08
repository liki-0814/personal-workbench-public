use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use super::client::SshHandle;
use crate::runtime::tools::progress;

pub async fn upload(handle: &Arc<Mutex<SshHandle>>, local: &Path, remote: &str) -> Result<u64> {
    let metadata = tokio::fs::metadata(local)
        .await
        .with_context(|| format!("本地文件不存在: {}", local.display()))?;
    let total = metadata.len();

    let sftp = open_sftp(handle).await?;

    let content = tokio::fs::read(local)
        .await
        .with_context(|| format!("读取 {} 失败", local.display()))?;

    let mut file = sftp
        .open_with_flags(
            remote,
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        )
        .await
        .with_context(|| format!("远程打开 {} 失败", remote))?;

    let chunk_size: usize = 128 * 1024;
    let mut written: u64 = 0;
    for chunk in content.chunks(chunk_size) {
        file.write_all(chunk).await.context("SFTP 写入失败")?;
        written += chunk.len() as u64;
        if total > chunk_size as u64 {
            let pct = (written as f64 / total as f64 * 100.0) as u32;
            progress::emit(&format!("⬆ 上传 {}%  ({}/{})", pct, written, total));
        }
    }

    file.flush().await.ok();
    file.shutdown().await.ok();

    Ok(written)
}

pub async fn download(handle: &Arc<Mutex<SshHandle>>, remote: &str, local: &Path) -> Result<u64> {
    let sftp = open_sftp(handle).await?;

    let meta = sftp
        .metadata(remote)
        .await
        .with_context(|| format!("远程文件不存在: {}", remote))?;
    let total = meta.size.unwrap_or(0);

    let mut file = sftp
        .open_with_flags(remote, OpenFlags::READ)
        .await
        .with_context(|| format!("远程打开 {} 失败", remote))?;

    if let Some(parent) = local.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }

    let mut local_file = tokio::fs::File::create(local)
        .await
        .with_context(|| format!("创建本地文件 {} 失败", local.display()))?;

    let mut buf = vec![0u8; 128 * 1024];
    let mut downloaded: u64 = 0;

    loop {
        let n = file.read(&mut buf).await.context("SFTP 读取失败")?;
        if n == 0 {
            break;
        }
        local_file
            .write_all(&buf[..n])
            .await
            .context("写入本地文件失败")?;
        downloaded += n as u64;
        if total > 128 * 1024 {
            let pct = if total > 0 {
                (downloaded as f64 / total as f64 * 100.0) as u32
            } else {
                0
            };
            progress::emit(&format!("⬇ 下载 {}%  ({}/{})", pct, downloaded, total));
        }
    }

    Ok(downloaded)
}

async fn open_sftp(handle: &Arc<Mutex<SshHandle>>) -> Result<SftpSession> {
    let h = handle.lock().await;
    let channel = h
        .channel_open_session()
        .await
        .context("打开 SFTP channel 失败")?;
    drop(h);
    channel
        .request_subsystem(true, "sftp")
        .await
        .context("请求 SFTP 子系统失败")?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .context("初始化 SFTP 会话失败")?;
    Ok(sftp)
}
