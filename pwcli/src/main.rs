use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::Read;
use tokio::sync::mpsc;

use pwcli::app::cli::repl_ui;
use pwcli::app::cli::tui_app::{self, MenuSources, StatusUpdate, UiEvent, UserSubmission};
use pwcli::app::config::wizard::{run_config_user, run_config_wizard};
use pwcli::app::config::RuntimeConfig;
use pwcli::runtime::git::GitContext;

/// Personal Workbench CLI — interactive AI agent + slash-command REPL.
#[derive(Parser)]
#[command(
    name = "pwcli",
    version = env!("PWCLI_VERSION_INFO"),
    about,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// Run a single prompt non-interactively, print the assistant's reply, then exit.
    /// Useful for scripts, cron, and pipelines: `echo foo | pwcli -p "summarize stdin"`.
    #[arg(short = 'p', long = "prompt", value_name = "PROMPT")]
    prompt: Option<String>,

    /// Interactive terminal layout. Fullscreen is the default; inline keeps
    /// the terminal's native scrollback for compatibility.
    #[arg(long, value_enum, default_value_t = tui_app::TuiMode::Fullscreen)]
    tui_mode: tui_app::TuiMode,

    /// Ensure the daemon is running, then open the Web client.
    #[arg(long)]
    web: bool,

    /// Print the Web URL without opening a browser.
    #[arg(long, requires = "web")]
    no_open: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Capture a durable Inbox work item and queue its first Agent input.
    Capture {
        /// Prompt text. When omitted, read it from stdin.
        prompt: Option<String>,
        /// Workspace to bind to the durable Agent session.
        #[arg(long)]
        cwd: Option<std::path::PathBuf>,
        /// Preferred executor for a later delegated start.
        #[arg(long, value_parser = ["auto", "pwcli", "codex", "qoder", "kimi"])]
        executor: Option<String>,
        /// Open the Workbench after capture.
        #[arg(long)]
        open: bool,
        /// Output format.
        #[arg(short = 'f', long = "format", default_value = "text", value_parser = ["text", "json"])]
        format: String,
    },
    /// Internal entrypoint used by the daemon for an isolated delegated task.
    #[command(hide = true)]
    TaskWorker {
        #[arg(long)]
        descriptor: std::path::PathBuf,
    },
    /// Ensure the daemon is running, then open the Web client.
    Web {
        /// Print the Web URL without opening a browser.
        #[arg(long)]
        no_open: bool,
    },
    /// Manage the persistent local daemon.
    Daemon {
        #[command(subcommand)]
        sub: pwcli::app::platform::daemon::DaemonCommand,
    },
    /// Check configuration, storage, daemon health and native ACP CLIs.
    Doctor(pwcli::app::platform::daemon::DoctorArgs),
    /// Run the interactive configuration wizard (provider, API key, model).
    Config {
        #[command(subcommand)]
        sub: Option<ConfigSub>,
    },
    /// Login, inspect, or logout AI provider credentials.
    Auth {
        #[command(subcommand)]
        sub: AuthSub,
    },
    /// Manage named Mixture-of-Agents presets.
    Moa {
        #[command(subcommand)]
        sub: MoaSub,
    },
    /// Manage the durable memory repository and import Markdown knowledge.
    Memory {
        #[command(subcommand)]
        sub: MemorySub,
    },
    /// Generate, edit, evaluate, or manage references for scientific illustrations.
    Illustrate {
        #[command(subcommand)]
        sub: Option<IllustrateSub>,
        /// Input text/JSON file. Omit to read stdin.
        #[arg(long)]
        input: Option<std::path::PathBuf>,
        #[arg(long, default_value = "auto", value_parser = ["auto", "diagram", "plot", "polish", "refine", "eval"])]
        mode: String,
        #[arg(long)]
        intent: Option<String>,
        #[arg(long, default_value = "balanced", value_parser = ["fast", "balanced", "max"])]
        quality: String,
        #[arg(long, default_value = "auto", value_parser = ["auto", "none"])]
        retrieval: String,
        #[arg(long)]
        aspect_ratio: Option<String>,
    },
    /// Run newline-delimited JSON RPC over stdin/stdout.
    Rpc,
}

#[derive(Subcommand)]
enum MoaSub {
    /// List configured presets.
    List,
    /// Create or update a preset using a numbered interactive model picker.
    Configure {
        name: Option<String>,
        #[arg(long)]
        default: bool,
    },
    /// Delete a preset.
    Delete { name: String },
}

#[derive(Subcommand)]
enum ConfigSub {
    /// Set user identity non-interactively (writes config.user.{email,slug,id}).
    User {
        #[arg(long)]
        email: String,
    },
}

#[derive(Subcommand)]
enum AuthSub {
    /// Authenticate a configured provider.
    Login {
        provider_id: String,
        /// Codex login method; other OAuth providers always use device code.
        #[arg(long, value_parser = ["device", "browser"], default_value = "device")]
        method: String,
        /// API key for API-key providers. Omit to enter it interactively.
        #[arg(long)]
        api_key: Option<String>,
    },
    /// Print credential status for configured providers.
    Status,
    /// Remove a provider credential.
    Logout { provider_id: String },
}

#[derive(Subcommand)]
enum MemorySub {
    ImportMarkdown { path: std::path::PathBuf },
    List,
    Search { query: String },
}

#[derive(Subcommand)]
enum IllustrateSub {
    /// Inspect and manage illustration reference libraries.
    Refs {
        #[command(subcommand)]
        sub: IllustrationRefsSub,
    },
}

#[derive(Subcommand)]
enum IllustrationRefsSub {
    Status,
    List,
    ResetStarter,
    Import {
        path: std::path::PathBuf,
        #[arg(long, value_parser = ["diagram", "plot"])]
        kind: String,
        #[arg(long)]
        layout: String,
        #[arg(long)]
        intent: String,
        #[arg(
            long,
            default_value = "User-provided scientific illustration reference"
        )]
        summary: String,
    },
    /// Import only diagram/ref.json and plot/ref.json from a local PaperBananaBench tree.
    ImportPack {
        path: std::path::PathBuf,
    },
    Remove {
        id: String,
    },
}

fn print_banner(git_ctx: &GitContext, config: &RuntimeConfig) {
    use repl_ui::{BLUE, BOLD, CYAN, DIM, RESET};

    let mascot = ["  /\\_/\\  ", " ( o.o ) ", "  > ^ <  "];

    let version = env!("CARGO_PKG_VERSION");
    let model = config
        .active_provider()
        .map(|p| format!("{} · {}", p.name, p.model))
        .unwrap_or_else(|| "未配置模型".to_string());
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| ".".to_string());

    let info_lines = [
        format!("{}{}pwcli {}{}", BOLD, CYAN, version, RESET),
        format!("{}{}{}", DIM, model, RESET),
        format!("{}{}{}", DIM, cwd, RESET),
    ];

    let max_art_width = mascot
        .iter()
        .map(|l| repl_ui::visible_len(l))
        .max()
        .unwrap_or(0);
    let padding = 4;

    for (i, art_line) in mascot.iter().enumerate() {
        let info = info_lines.get(i).map(|s| s.as_str()).unwrap_or("");
        let art_visible = repl_ui::visible_len(art_line);
        let spaces = max_art_width.saturating_sub(art_visible) + padding;
        println!(
            "{}{}{}{:spaces$}{}",
            BLUE,
            art_line,
            RESET,
            "",
            info,
            spaces = spaces
        );
    }

    for info in info_lines.iter().skip(mascot.len()) {
        println!("{:width$}{}", "", info, width = max_art_width + padding);
    }

    if git_ctx.is_git_repo {
        println!("{}{}{}", DIM, git_ctx.status_summary(), RESET);
    }
    println!();
}

#[tokio::main]
async fn main() -> Result<()> {
    let previous_crash = pwcli::app::platform::crash::latest_crash();
    pwcli::app::platform::crash::install_handler();
    if let Some(path) = previous_crash {
        eprintln!("上次运行异常退出，崩溃信息：{}", path.display());
        pwcli::app::platform::crash::mark_seen(&path);
    }
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Capture {
            prompt,
            cwd,
            executor,
            open,
            format,
        }) => {
            let prompt = match prompt {
                Some(prompt) => prompt,
                None => {
                    let mut prompt = String::new();
                    std::io::stdin().read_to_string(&mut prompt)?;
                    prompt
                }
            };
            if prompt.trim().is_empty() {
                anyhow::bail!("capture prompt must not be empty");
            }
            let cwd = cwd.unwrap_or(std::env::current_dir()?);
            let (captured, daemon_url) =
                pwcli::app::cli::tui_client::capture_work_item(prompt, &cwd, executor.as_deref())
                    .await?;
            if format == "json" {
                println!("{}", serde_json::to_string_pretty(&captured)?);
            } else {
                println!("captured work item {}", captured.work_item_id);
                println!("session: {}", captured.session_id);
            }
            if open {
                open_capture_url(&format!(
                    "{}/?agentSessionId={}",
                    daemon_url.trim_end_matches('/'),
                    captured.session_id
                ))?;
            }
            return Ok(());
        }
        Some(Command::TaskWorker { descriptor }) => {
            pwcli::app::config::local_config::init().await;
            return pwcli::runtime::task::run_worker(descriptor).await;
        }
        Some(Command::Web { no_open }) => {
            pwcli::app::config::local_config::init().await;
            return pwcli::app::platform::daemon::web(no_open).await;
        }
        Some(Command::Daemon { sub }) => {
            pwcli::app::config::local_config::init().await;
            return pwcli::app::platform::daemon::execute(sub).await;
        }
        Some(Command::Doctor(args)) => {
            pwcli::app::config::local_config::init().await;
            return pwcli::app::platform::daemon::doctor(args).await;
        }
        Some(Command::Config { sub }) => match sub {
            Some(ConfigSub::User { email }) => return run_config_user(email).await,
            None => return run_config_wizard().await,
        },
        Some(Command::Auth { sub }) => {
            pwcli::app::config::local_config::init().await;
            return run_auth_command(sub).await;
        }
        Some(Command::Moa { sub }) => {
            pwcli::app::config::local_config::init().await;
            return run_moa_command(sub).await;
        }
        Some(Command::Memory { sub }) => {
            pwcli::app::config::local_config::init().await;
            return run_memory_command(sub).await;
        }
        Some(Command::Illustrate {
            sub,
            input,
            mode,
            intent,
            quality,
            retrieval,
            aspect_ratio,
        }) => {
            pwcli::app::config::local_config::init().await;
            if let Some(IllustrateSub::Refs { sub }) = sub {
                return run_illustration_refs(sub);
            }
            let intent = intent.context("--intent is required when generating an illustration")?;
            let raw = match input {
                Some(path) => std::fs::read_to_string(&path)
                    .with_context(|| format!("read {}", path.display()))?,
                None => {
                    let mut value = String::new();
                    std::io::stdin().read_to_string(&mut value)?;
                    value
                }
            };
            anyhow::ensure!(
                !raw.trim().is_empty(),
                "illustration input must not be empty"
            );
            let resolved_mode = parse_illustration_mode(&mode)?;
            let content = if resolved_mode == pwcli::runtime::illustration::IllustrationMode::Plot
                || raw.trim_start().starts_with(['{', '['])
            {
                serde_json::from_str(&raw)
                    .context("plot/JSON illustration input is invalid JSON")?
            } else {
                serde_json::Value::String(raw)
            };
            let request = pwcli::runtime::illustration::IllustrationRequest {
                mode: resolved_mode,
                content,
                visual_intent: intent,
                image_refs: vec![],
                quality: match quality.as_str() {
                    "fast" => pwcli::runtime::illustration::IllustrationQuality::Fast,
                    "max" => pwcli::runtime::illustration::IllustrationQuality::Max,
                    _ => pwcli::runtime::illustration::IllustrationQuality::Balanced,
                },
                retrieval: if retrieval == "none" {
                    pwcli::runtime::illustration::RetrievalMode::None
                } else {
                    pwcli::runtime::illustration::RetrievalMode::Auto
                },
                aspect_ratio,
                constraints: Default::default(),
            };
            let factory = pwcli::app::composition::RuntimeFactory::load_local().await?;
            let context = factory
                .illustration_context(std::env::current_dir()?)
                .await?;
            let data_dir = pwcli::app::config::local_config::data_dir();
            let run = pwcli::runtime::illustration::IllustrationService::new(&data_dir)
                .execute(&context, request)
                .await?;
            println!("{}", serde_json::to_string_pretty(&run)?);
            return Ok(());
        }
        Some(Command::Rpc) => {
            pwcli::app::config::local_config::init().await;
            return pwcli::app::cli::rpc::run_stdio().await;
        }
        None => {} // fall through to REPL or oneshot
    }

    if cli.web {
        pwcli::app::config::local_config::init().await;
        return pwcli::app::platform::daemon::web(cli.no_open).await;
    }

    // -p / --prompt: non-interactive single-shot mode (no TUI).
    if let Some(prompt) = cli.prompt {
        return pwcli::app::cli::tui_client::run_daemon_oneshot(prompt).await;
    }

    // The interactive TUI is a client. The daemon owns sessions, tools,
    // background work and ACP subprocesses across TUI/Web disconnects.
    pwcli::app::config::local_config::init().await;
    pwcli::app::platform::daemon::start().await?;
    let daemon_status = pwcli::app::platform::daemon::status().await;
    let daemon_sessions =
        pwcli::app::cli::tui_client::fetch_daemon_sessions(&daemon_status.http_address)
            .await
            .unwrap_or_default();
    let config = RuntimeConfig::load();
    let git_ctx = GitContext::current().await;
    print_banner(&git_ctx, &config);
    let (input_tx, input_rx) = mpsc::channel::<UserSubmission>(64);
    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UiEvent>();
    let window_tokens = config
        .active_provider()
        .map(|provider| pwcli::ai::usage::context_window_for_model(&provider.model))
        .unwrap_or(32_000);
    let client_handle = tokio::spawn(pwcli::app::cli::tui_client::run_daemon_client_task(
        input_rx,
        ui_tx.clone(),
        daemon_status.http_address.clone(),
        window_tokens,
    ));
    let initial_status = StatusUpdate {
        model: Some(format!(
            "daemon {}",
            daemon_status
                .version
                .as_deref()
                .unwrap_or(env!("CARGO_PKG_VERSION"))
        )),
        yolo: Some(
            pwcli::runtime::permissions::current_agent_permission_mode()
                == pwcli::runtime::permissions::AgentPermissionMode::Full,
        ),
        permission_mode: Some(
            pwcli::runtime::permissions::current_agent_permission_mode()
                .as_str()
                .to_string(),
        ),
        session_name: None,
        active_provider: config.active_provider.clone(),
    };
    let moa_config = pwcli::app::config::local_config::get()
        .ai
        .moa
        .unwrap_or_default();
    let mut menu_providers: Vec<String> = config
        .providers
        .as_ref()
        .map(|ps| ps.iter().map(|p| p.name.clone()).collect())
        .unwrap_or_default();
    let mut models_by_provider: std::collections::HashMap<String, Vec<String>> = config
        .providers
        .as_ref()
        .map(|ps| {
            ps.iter()
                .map(|p| {
                    (
                        p.name.clone(),
                        p.models.iter().map(|m| m.id.clone()).collect(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if !moa_config.presets.is_empty() {
        menu_providers.push("moa".into());
        models_by_provider.insert("moa".into(), moa_config.presets.keys().cloned().collect());
    }
    let menu_sources = MenuSources {
        providers: menu_providers,
        models_by_provider,
        active_provider: config.active_provider.clone().unwrap_or_default(),
        sessions: daemon_sessions,
    };

    let result = tui_app::run(
        input_tx.clone(),
        ui_rx,
        initial_status,
        window_tokens,
        menu_sources,
        cli.tui_mode,
    )
    .await;
    // `/exit` only disconnects this client. The daemon and its work continue.
    let _ = input_tx.try_send(UserSubmission::Shutdown);
    drop(input_tx);
    let mut client_handle = client_handle;
    if tokio::time::timeout(Duration::from_millis(500), &mut client_handle)
        .await
        .is_err()
    {
        client_handle.abort();
        let _ = client_handle.await;
    }

    result
}

fn parse_illustration_mode(value: &str) -> Result<pwcli::runtime::illustration::IllustrationMode> {
    use pwcli::runtime::illustration::IllustrationMode;
    Ok(match value {
        "diagram" => IllustrationMode::Diagram,
        "plot" => IllustrationMode::Plot,
        "polish" => IllustrationMode::Polish,
        "refine" => IllustrationMode::Refine,
        "eval" => IllustrationMode::Eval,
        "auto" => IllustrationMode::Auto,
        _ => anyhow::bail!("invalid illustration mode"),
    })
}

fn run_illustration_refs(command: IllustrationRefsSub) -> Result<()> {
    let data_dir = pwcli::app::config::local_config::data_dir();
    let store = pwcli::runtime::illustration::ReferenceStore::new(&data_dir);
    match command {
        IllustrationRefsSub::Status => println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "references": store.status()?,
                "python": pwcli::runtime::illustration::python_env::status()
            }))?
        ),
        IllustrationRefsSub::List => {
            println!("{}", serde_json::to_string_pretty(&store.list_personal()?)?)
        }
        IllustrationRefsSub::ResetStarter => {
            store.reset_starter()?;
            println!("starter reference pack restored");
        }
        IllustrationRefsSub::Import {
            path,
            kind,
            layout,
            intent,
            summary,
        } => {
            let entry = store.import_personal(
                &path,
                parse_illustration_mode(&kind)?,
                &layout,
                &intent,
                &summary,
            )?;
            println!("{}", serde_json::to_string_pretty(&entry)?);
        }
        IllustrationRefsSub::ImportPack { path } => {
            let count = store.import_paperbanana_reference_pool(&path)?;
            println!(
                "imported {count} PaperBanana reference-pool images (test/GT outside ref.json ignored)"
            );
        }
        IllustrationRefsSub::Remove { id } => {
            store.remove_personal(&id)?;
            println!("removed {id}");
        }
    }
    Ok(())
}

fn open_capture_url(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open").arg(url).status();
    #[cfg(target_os = "linux")]
    let status = std::process::Command::new("xdg-open").arg(url).status();
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .status();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let status: std::io::Result<std::process::ExitStatus> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "opening a browser is unsupported on this target",
    ));
    let status = status?;
    if !status.success() {
        anyhow::bail!("system browser command exited with {status}");
    }
    Ok(())
}

async fn run_auth_command(sub: AuthSub) -> Result<()> {
    use pwcli::ai::provider::{
        provider_kind, AuthFlowMethod, AuthFlowState, AuthManager, ProviderKind,
    };

    let config = RuntimeConfig::load();
    let providers = config.providers.as_deref().unwrap_or_default();
    let find_provider = |id: &str| {
        providers
            .iter()
            .find(|provider| provider.name == id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("provider `{id}` is not configured"))
    };
    let manager = AuthManager::default();
    match sub {
        AuthSub::Status => {
            if providers.is_empty() {
                println!("No providers configured.");
                return Ok(());
            }
            for provider in providers {
                let status = manager.status(provider).await?;
                println!("{}\t{}\t{}", provider.name, status.method, status.status);
            }
        }
        AuthSub::Logout { provider_id } => {
            let provider = find_provider(&provider_id)?;
            manager.logout(&provider)?;
            println!("Logged out {}", provider.name);
        }
        AuthSub::Login {
            provider_id,
            method,
            api_key,
        } => {
            let provider = find_provider(&provider_id)?;
            let kind = provider_kind(&provider);
            if matches!(kind, ProviderKind::Custom | ProviderKind::QwenTokenPlanCn) {
                let key = match api_key {
                    Some(value) => value,
                    None => {
                        eprint!("API key: ");
                        use std::io::Write;
                        std::io::stderr().flush()?;
                        let mut value = String::new();
                        std::io::stdin().read_line(&mut value)?;
                        value.trim().to_string()
                    }
                };
                manager.set_api_key(&provider, key)?;
                println!("Authenticated {} with API key", provider.name);
                return Ok(());
            }
            let method = if method == "browser" {
                AuthFlowMethod::Browser
            } else {
                AuthFlowMethod::DeviceCode
            };
            let flow = manager.start_flow(&provider, method).await?;
            if let Some(url) = flow.verification_uri.as_deref() {
                println!("Open: {url}");
                if method == AuthFlowMethod::Browser {
                    if let Err(error) = open_capture_url(url) {
                        eprintln!("Could not open browser automatically: {error}");
                    }
                }
            }
            if let Some(code) = flow.user_code.as_deref() {
                println!("Code: {code}");
            }
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let snapshot = manager
                    .flow(&flow.flow_id)
                    .await
                    .context("authentication flow disappeared")?;
                match snapshot.state {
                    AuthFlowState::Pending => continue,
                    AuthFlowState::Connected => {
                        println!("Authenticated {}", provider.name);
                        break;
                    }
                    AuthFlowState::Failed => anyhow::bail!(
                        "authentication failed: {}",
                        snapshot.error.unwrap_or_else(|| "unknown error".into())
                    ),
                    AuthFlowState::Expired => anyhow::bail!("authentication flow expired"),
                    AuthFlowState::Cancelled => anyhow::bail!("authentication was cancelled"),
                }
            }
        }
    }
    Ok(())
}

async fn run_memory_command(sub: MemorySub) -> Result<()> {
    let config = RuntimeConfig::load();
    let user_slug = config
        .user
        .as_ref()
        .and_then(|user| user.slug.clone())
        .unwrap_or_else(|| "local".to_string());
    let store = pwcli::runtime::memory::MemoryStore::new(&user_slug)?;
    match sub {
        MemorySub::ImportMarkdown { path } => {
            let report = pwcli::runtime::memory::import_markdown_tree(&store, &path)?;
            println!(
                "files={} imported={} updated={} unchanged={} skipped={}",
                report.files_scanned,
                report.chunks_imported,
                report.chunks_updated,
                report.chunks_unchanged,
                report.files_skipped
            );
        }
        MemorySub::List => print!("{}", store.read_index_raw()?),
        MemorySub::Search { query } => {
            let hits = pwcli::runtime::memory::hybrid_search(
                &store,
                pwcli::runtime::memory::HybridSearchOptions {
                    query,
                    max_results: 10,
                    exclude_slugs: std::collections::HashSet::new(),
                    candidate_top_n: Some(20),
                    include_archived: true,
                },
            )?;
            for hit in hits {
                println!("{}\t{:.6}\t{}", hit.slug, hit.score, hit.summary);
            }
        }
    }
    Ok(())
}

async fn run_moa_command(command: MoaSub) -> Result<()> {
    use pwcli::runtime::decision::config::{MoaModelRef, MoaPreset};
    let mut local = pwcli::app::config::local_config::get();
    let mut moa = local.ai.moa.clone().unwrap_or_default();
    match command {
        MoaSub::List => {
            if moa.presets.is_empty() {
                println!("No MoA presets configured.");
            }
            for (name, preset) in &moa.presets {
                let marker = if name == &moa.active_preset { "*" } else { " " };
                println!(
                    "{marker} {name}: {} advisor(s) → judge {}:{}{}",
                    preset.advisors.len(),
                    preset.judge.provider,
                    preset.judge.model,
                    if preset.enabled { "" } else { " (disabled)" }
                );
            }
        }
        MoaSub::Delete { name } => {
            if moa.presets.remove(&name).is_none() {
                anyhow::bail!("MoA preset not found: {name}");
            }
            let fallback = moa.presets.keys().next().cloned().unwrap_or_default();
            if moa.active_preset == name {
                moa.active_preset = fallback;
            }
            local.ai.moa = Some(moa);
            pwcli::app::config::local_config::save(&local).await?;
            println!("Deleted MoA preset '{name}'.");
        }
        MoaSub::Configure { name, default } => {
            let runtime = RuntimeConfig::load();
            let models = runtime
                .providers
                .unwrap_or_default()
                .into_iter()
                .flat_map(|provider| {
                    provider
                        .models
                        .into_iter()
                        .filter(|model| model.enabled != Some(false))
                        .map(move |model| MoaModelRef {
                            provider: provider.name.clone(),
                            model: model.id,
                        })
                })
                .collect::<Vec<_>>();
            if models.is_empty() {
                anyhow::bail!("No provider models configured");
            }
            for (index, model) in models.iter().enumerate() {
                println!("{:>3}. {}:{}", index + 1, model.provider, model.model);
            }
            let preset_name = name.unwrap_or_else(|| {
                if moa.active_preset.is_empty() {
                    "fusion".into()
                } else {
                    moa.active_preset.clone()
                }
            });
            if !is_valid_preset_name(&preset_name) {
                anyhow::bail!("Invalid preset name: {preset_name}");
            }
            let judge_index = prompt_line("Judge number")?
                .parse::<usize>()
                .ok()
                .filter(|value| *value > 0 && *value <= models.len())
                .ok_or_else(|| anyhow::anyhow!("Invalid judge number"))?
                - 1;
            let advisors = prompt_line("Advisor numbers (comma separated)")?
                .split(',')
                .filter_map(|value| value.trim().parse::<usize>().ok())
                .filter(|value| *value > 0 && *value <= models.len())
                .map(|value| models[value - 1].clone())
                .collect::<Vec<_>>();
            if advisors.is_empty() {
                anyhow::bail!("At least one advisor model is required");
            }
            let existing = moa.presets.get(&preset_name);
            let preset = MoaPreset {
                enabled: true,
                advisors,
                judge: models[judge_index].clone(),
                image_describer: existing.and_then(|value| value.image_describer.clone()),
                advisor_max_tokens: existing.and_then(|value| value.advisor_max_tokens),
                advisor_temperature: existing.and_then(|value| value.advisor_temperature),
                judge_temperature: existing.and_then(|value| value.judge_temperature),
                low_risk_advisors: existing.and_then(|value| value.low_risk_advisors),
                elevated_risk_advisors: existing.and_then(|value| value.elevated_risk_advisors),
                high_risk_advisors: existing.and_then(|value| value.high_risk_advisors),
            };
            moa.presets.insert(preset_name.clone(), preset);
            if moa.active_preset.is_empty() || default {
                moa.active_preset = preset_name.clone();
            }
            local.ai.moa = Some(moa);
            pwcli::app::config::local_config::save(&local).await?;
            println!("Configured MoA preset '{preset_name}'.");
        }
    }
    Ok(())
}

fn prompt_line(label: &str) -> Result<String> {
    use std::io::{self, Write};
    print!("{label}: ");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(value.trim().to_string())
}

fn is_valid_preset_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_lowercase()
                || ch.is_ascii_digit()
                || (index > 0 && matches!(ch, '.' | '_' | '-'))
        })
}
