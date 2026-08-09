pub mod client;
pub mod config;
pub mod pool;
pub mod transfer;
pub mod tunnel;

use std::path::Path;
use std::sync::Arc;

use serde_json::json;

use crate::runtime::backend::BackendClient;
use crate::runtime::tools::progress;
use crate::runtime::tools::registry::{ToolImpact, ToolRegistry};

use self::pool::SshPool;
use self::tunnel::TunnelManager;

pub fn register(registry: &mut ToolRegistry, backend: Arc<BackendClient>) {
    let pool = Arc::new(SshPool::new(Arc::clone(&backend)));
    let tunnels = Arc::new(TunnelManager::new());

    register_execute(registry, Arc::clone(&pool));
    register_upload(registry, Arc::clone(&pool));
    register_download(registry, Arc::clone(&pool));
    register_tunnel(registry, Arc::clone(&pool), Arc::clone(&tunnels));
    register_list_servers(registry, Arc::clone(&backend));
}

fn register_execute(registry: &mut ToolRegistry, pool: Arc<SshPool>) {
    registry.register_with_impact(
        "ssh_execute",
        "在远程服务器上执行命令。通过 alias 指定服务器（可先用 ssh_list 查看已配置别名）。\
         自动复用连接池，首次连接后后续命令延迟极低。\
         支持合并多条命令（用 && 拼接）减少往返。\
         超时策略：显式传 timeout_secs 时同步等待到该时限，超时返回部分输出+错误由你决定下一步；\
         预计会超过 60s 的长命令请直接设 background=true。",
        json!({
            "type": "object",
            "properties": {
                "alias": { "type": "string", "description": "SSH 服务器别名" },
                "command": { "type": "string", "description": "要执行的 shell 命令" },
                "timeout_secs": { "type": "integer", "description": "同步等待的超时秒数（默认 30）。显式传入时命令会在前台等到该时限，超时返回部分输出；超过 60s 的需求建议改用 background=true" },
                "background": { "type": "boolean", "description": "设为 true 则后台执行，不阻塞对话；完成后会自动通知并汇报结果" }
            },
            "required": ["alias", "command"]
        }),
        ToolImpact::ExternalSideEffect,
        Box::new(move |args| {
            let pool = Arc::clone(&pool);
            let alias = args["alias"].as_str().unwrap_or("").to_string();
            let command = args["command"].as_str().unwrap_or("").to_string();
            let timeout = args["timeout_secs"].as_u64().unwrap_or(30);
            Box::pin(async move {
                if alias.is_empty() || command.is_empty() {
                    anyhow::bail!("alias 和 command 必填");
                }
                let cmd_preview = &command[..command.len().min(60)];
                progress::emit(&format!("🔗 SSH {} > {}", alias, cmd_preview));

                let handle_arc = pool.get_or_connect(&alias).await?;
                let result = client::exec(&handle_arc, &command, timeout).await?;

                if result.timed_out {
                    anyhow::bail!(
                        "命令超时 ({}s): {}\n\n[部分输出 stdout]\n{}\n[部分输出 stderr]\n{}\n\n\
                         命令在 {}s 内未完成。你可以：\
                         1) 调大 timeout_secs 重新执行；\
                         2) 设 background=true 转后台执行（完成后自动通知）；\
                         3) 优化命令（拆分/加过滤）后重试。",
                        timeout,
                        command,
                        if result.stdout.is_empty() { "（无）" } else { &result.stdout },
                        if result.stderr.is_empty() { "（无）" } else { &result.stderr },
                        timeout
                    );
                }

                let success = result.exit_code.map_or(result.stderr.is_empty(), |c| c == 0);
                Ok(serde_json::to_string_pretty(&json!({
                    "success": success,
                    "exit_code": result.exit_code,
                    "stdout": result.stdout,
                    "stderr": result.stderr,
                }))?)
            })
        }),
    );
}

fn register_upload(registry: &mut ToolRegistry, pool: Arc<SshPool>) {
    registry.register_with_impact(
        "ssh_upload",
        "通过 SFTP 上传本地文件到远程服务器。支持大文件实时进度。\
         预计耗时超过 60s 的大文件请直接设 background=true。",
        json!({
            "type": "object",
            "properties": {
                "alias": { "type": "string", "description": "SSH 服务器别名" },
                "local_path": { "type": "string", "description": "本地文件路径" },
                "remote_path": { "type": "string", "description": "远程目标路径" },
                "timeout_secs": { "type": "integer", "description": "同步等待的超时秒数（默认 300），超时返回错误由你决定下一步" },
                "background": { "type": "boolean", "description": "设为 true 则后台执行，不阻塞对话；完成后会自动通知并汇报结果" }
            },
            "required": ["alias", "local_path", "remote_path"]
        }),
        ToolImpact::ExternalSideEffect,
        Box::new(move |args| {
            let pool = Arc::clone(&pool);
            let alias = args["alias"].as_str().unwrap_or("").to_string();
            let local = args["local_path"].as_str().unwrap_or("").to_string();
            let remote = args["remote_path"].as_str().unwrap_or("").to_string();
            let timeout = args["timeout_secs"].as_u64().unwrap_or(300);
            Box::pin(async move {
                if alias.is_empty() || local.is_empty() || remote.is_empty() {
                    anyhow::bail!("alias、local_path、remote_path 必填");
                }
                progress::emit(&format!("⬆ 上传 {} → {}:{}", local, alias, remote));

                let handle_arc = pool.get_or_connect(&alias).await?;
                let transfer = transfer::upload(&handle_arc, Path::new(&local), &remote);
                let bytes = match tokio::time::timeout(
                    std::time::Duration::from_secs(timeout),
                    transfer,
                )
                .await
                {
                    Ok(result) => result?,
                    Err(_) => anyhow::bail!(
                        "上传超时 ({}s): {} → {}:{}\n\
                         你可以：1) 调大 timeout_secs 重新执行；2) 设 background=true 转后台执行（完成后自动通知）。",
                        timeout,
                        local,
                        alias,
                        remote
                    ),
                };

                Ok(format!("✅ 上传完成: {} → {}:{} ({} bytes)", local, alias, remote, bytes))
            })
        }),
    );
}

fn register_download(registry: &mut ToolRegistry, pool: Arc<SshPool>) {
    registry.register_with_impact(
        "ssh_download",
        "通过 SFTP 从远程服务器下载文件到本地。支持大文件实时进度。\
         预计耗时超过 60s 的大文件请直接设 background=true。",
        json!({
            "type": "object",
            "properties": {
                "alias": { "type": "string", "description": "SSH 服务器别名" },
                "remote_path": { "type": "string", "description": "远程文件路径" },
                "local_path": { "type": "string", "description": "本地目标路径" },
                "timeout_secs": { "type": "integer", "description": "同步等待的超时秒数（默认 300），超时返回错误由你决定下一步" },
                "background": { "type": "boolean", "description": "设为 true 则后台执行，不阻塞对话；完成后会自动通知并汇报结果" }
            },
            "required": ["alias", "remote_path", "local_path"]
        }),
        ToolImpact::ReversibleMutation,
        Box::new(move |args| {
            let pool = Arc::clone(&pool);
            let alias = args["alias"].as_str().unwrap_or("").to_string();
            let remote = args["remote_path"].as_str().unwrap_or("").to_string();
            let local = args["local_path"].as_str().unwrap_or("").to_string();
            let timeout = args["timeout_secs"].as_u64().unwrap_or(300);
            Box::pin(async move {
                if alias.is_empty() || remote.is_empty() || local.is_empty() {
                    anyhow::bail!("alias、remote_path、local_path 必填");
                }
                progress::emit(&format!("⬇ 下载 {}:{} → {}", alias, remote, local));

                let handle_arc = pool.get_or_connect(&alias).await?;
                let transfer = transfer::download(&handle_arc, &remote, Path::new(&local));
                let bytes = match tokio::time::timeout(
                    std::time::Duration::from_secs(timeout),
                    transfer,
                )
                .await
                {
                    Ok(result) => result?,
                    Err(_) => anyhow::bail!(
                        "下载超时 ({}s): {}:{} → {}\n\
                         你可以：1) 调大 timeout_secs 重新执行；2) 设 background=true 转后台执行（完成后自动通知）。",
                        timeout,
                        alias,
                        remote,
                        local
                    ),
                };

                Ok(format!("✅ 下载完成: {}:{} → {} ({} bytes)", alias, remote, local, bytes))
            })
        }),
    );
}

fn register_tunnel(registry: &mut ToolRegistry, pool: Arc<SshPool>, tunnels: Arc<TunnelManager>) {
    let t1 = Arc::clone(&tunnels);
    let p1 = Arc::clone(&pool);
    registry.register_with_impact(
        "ssh_tunnel",
        "SSH 隧道管理（本地端口转发）。\
         action=start: 启动隧道，访问远程内网服务（数据库、Web 服务等）。\
         action=stop: 停止指定隧道。\
         action=list: 列出所有活跃隧道。",
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["start", "stop", "list"], "description": "操作类型" },
                "alias": { "type": "string", "description": "SSH 服务器别名（start 时必填）" },
                "remote_host": { "type": "string", "description": "远程目标主机（默认 127.0.0.1）" },
                "remote_port": { "type": "integer", "description": "远程端口（start 时必填）" },
                "local_port": { "type": "integer", "description": "本地端口（可选，自动分配）" },
                "tunnel_id": { "type": "string", "description": "隧道 ID（stop 时必填）" }
            },
            "required": ["action"]
        }),
        ToolImpact::ExternalSideEffect,
        Box::new(move |args| {
            let tunnels = Arc::clone(&t1);
            let pool = Arc::clone(&p1);
            let args = args.clone();
            Box::pin(async move {
                let action = args["action"].as_str().unwrap_or("");
                match action {
                    "start" => {
                        let alias = args["alias"].as_str().unwrap_or("");
                        let remote_port = args["remote_port"].as_u64().unwrap_or(0) as u16;
                        if alias.is_empty() || remote_port == 0 {
                            anyhow::bail!("start 需要 alias 和 remote_port");
                        }
                        let remote_host = args["remote_host"].as_str().unwrap_or("127.0.0.1");
                        let local_port = args["local_port"].as_u64().map(|p| p as u16);

                        let handle_arc = pool.get_or_connect(alias).await?;
                        let info = tunnels
                            .start(handle_arc, alias, remote_host, remote_port, local_port)
                            .await?;

                        Ok(format!(
                            "✅ 隧道已启动: localhost:{} → {}:{}:{} (id={})",
                            info.local_port, alias, info.remote_host, info.remote_port, info.id
                        ))
                    }
                    "stop" => {
                        let id = args["tunnel_id"].as_str().unwrap_or("");
                        if id.is_empty() {
                            anyhow::bail!("stop 需要 tunnel_id");
                        }
                        if tunnels.stop(id).await {
                            Ok(format!("✅ 隧道 {} 已停止", id))
                        } else {
                            Ok(format!("⚠ 隧道 {} 不存在", id))
                        }
                    }
                    "list" => {
                        let list = tunnels.list().await;
                        if list.is_empty() {
                            return Ok("（无活跃隧道）".into());
                        }
                        let lines: Vec<String> = list
                            .iter()
                            .map(|t| {
                                format!(
                                    "  {} → localhost:{} → {}:{}",
                                    t.alias, t.local_port, t.remote_host, t.remote_port
                                )
                            })
                            .collect();
                        Ok(format!("活跃隧道 ({}):\n{}", list.len(), lines.join("\n")))
                    }
                    _ => anyhow::bail!("未知 action: {}", action),
                }
            })
        }),
    );
}

fn register_list_servers(registry: &mut ToolRegistry, backend: Arc<BackendClient>) {
    registry.register(
        "ssh_list_servers",
        "列出 config.json 中已配置的 SSH 服务器。配置修改统一在前端 Settings 完成。",
        json!({
            "type": "object",
            "properties": {}
        }),
        Box::new(move |_args| {
            let backend = Arc::clone(&backend);
            Box::pin(async move {
                let servers = config::load_servers(&backend).await?;
                if servers.is_empty() {
                    return Ok("（未配置 SSH 服务器。请在前端 Settings 中添加。）".into());
                }
                let lines: Vec<String> = servers
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let auth_type = match &s.auth {
                            config::AuthMethod::Key { .. } => "密钥",
                            config::AuthMethod::Password { .. } => "密码",
                        };
                        format!(
                            "{}. {} — {}@{}:{} [{}] {}",
                            i + 1,
                            s.alias,
                            s.user,
                            s.host,
                            s.port,
                            auth_type,
                            s.description,
                        )
                    })
                    .collect();
                Ok(format!(
                    "SSH 服务器 ({}):\n{}",
                    servers.len(),
                    lines.join("\n")
                ))
            })
        }),
    );
}
