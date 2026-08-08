use std::backtrace::Backtrace;
use std::fs;
use std::path::PathBuf;

const KEEP_CRASHES: usize = 5;

fn crash_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".pwcli")
        .join("crashes")
}

pub fn latest_crash() -> Option<PathBuf> {
    let mut entries = fs::read_dir(crash_dir())
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("log"))
        .collect::<Vec<_>>();
    entries.sort();
    entries.pop()
}

pub fn mark_seen(path: &std::path::Path) {
    let _ = fs::rename(path, path.with_extension("seen"));
}

pub fn install_handler() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let dir = crash_dir();
        if fs::create_dir_all(&dir).is_ok() {
            let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
            let path = dir.join(format!("crash-{timestamp}-{}.log", std::process::id()));
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                .unwrap_or("non-string panic payload");
            let location = info
                .location()
                .map(ToString::to_string)
                .unwrap_or_else(|| "unknown".into());
            let dump = format!(
                "timestamp={}\npid={}\nlocation={}\npanic={}\nbacktrace:\n{}\n",
                chrono::Utc::now().to_rfc3339(),
                std::process::id(),
                location,
                payload,
                Backtrace::force_capture(),
            );
            let _ = fs::write(path, dump);
            rotate();
        }
        previous(info);
    }));
}

fn rotate() {
    let Ok(entries) = fs::read_dir(crash_dir()) else {
        return;
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    let remove_count = paths.len().saturating_sub(KEEP_CRASHES);
    for path in paths.into_iter().take(remove_count) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_is_bounded() {
        assert_eq!(KEEP_CRASHES, 5);
    }
}
