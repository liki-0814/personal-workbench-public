use std::fmt;
use std::fs;
use std::path::PathBuf;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{
    fmt::{self as tfmt, format::Writer, time::FormatTime},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter, Layer,
};

/// 用本地时区（chrono::Local）格式化日志时间戳，替换 tracing 默认的 UTC 输出。
struct LocalTime;

impl FormatTime for LocalTime {
    fn format_time(&self, w: &mut Writer<'_>) -> fmt::Result {
        write!(
            w,
            "{}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f %:z")
        )
    }
}

const LOG_DIR_NAME: &str = "logs";
const MAX_LOG_AGE_DAYS: u64 = 7;

fn default_log_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".pwcli")
        .join(LOG_DIR_NAME)
}

pub fn init_logging() {
    init_logging_in(&default_log_dir());
}

pub fn init_logging_in(dir: &std::path::Path) {
    let dir = dir.to_path_buf();
    let _ = fs::create_dir_all(&dir);

    let file_appender = RollingFileAppender::new(Rotation::DAILY, &dir, "pwcli.log");

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("pwcli=info,tower_http=info"));

    let file_layer = tfmt::layer()
        .with_target(true)
        .with_ansi(false)
        .with_timer(LocalTime)
        .with_writer(file_appender);

    let stderr_layer = tfmt::layer()
        .with_target(false)
        .with_ansi(false)
        .with_timer(LocalTime)
        .with_writer(std::io::stderr)
        .with_filter(EnvFilter::new("pwcli=info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(stderr_layer)
        .init();

    cleanup_old_logs(&dir);
}

fn cleanup_old_logs(dir: &PathBuf) {
    let cutoff =
        std::time::SystemTime::now() - std::time::Duration::from_secs(MAX_LOG_AGE_DAYS * 86400);

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.starts_with("pwcli.log") {
            continue;
        }
        if let Ok(meta) = path.metadata() {
            if let Ok(modified) = meta.modified() {
                if modified < cutoff {
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }
}
