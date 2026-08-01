use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let schema_files = [
        "collections.toml",
        "chat.toml",
        "folders.toml",
        "singletons.toml",
    ];

    let mut code = String::from("pub const SCHEMA_FILES: &[(&str, &str)] = &[\n");
    for f in &schema_files {
        let schema_path = Path::new("schemas").join(f);
        let schema_source = fs::read_to_string(&schema_path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", schema_path.display()));
        toml::from_str::<toml::Table>(&schema_source)
            .unwrap_or_else(|error| panic!("invalid schema {}: {error}", schema_path.display()));
        code.push_str(&format!(
            "    ({:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/schemas/{}\"))),\n",
            f, f
        ));
    }
    code.push_str("];\n");

    fs::write(Path::new(&out_dir).join("schemas_generated.rs"), code).unwrap();

    for f in &schema_files {
        println!("cargo:rerun-if-changed=schemas/{}", f);
    }

    emit_build_metadata();
    generate_web_assets(Path::new(&out_dir));
}

fn emit_build_metadata() {
    let commit = command_output("git", &["rev-parse", "--short=12", "HEAD"])
        .unwrap_or_else(|| "unknown".to_string());
    let dirty = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| !output.stdout.is_empty());
    let build_epoch = env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| {
            command_output("git", &["show", "-s", "--format=%ct", "HEAD"])
                .and_then(|value| value.parse::<u64>().ok())
        })
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default()
        });
    let build_date = DateTime::<Utc>::from_timestamp(build_epoch as i64, 0)
        .map(|value| value.to_rfc3339())
        .unwrap_or_else(|| build_epoch.to_string());
    let target = env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".to_string());
    let dirty_suffix = if dirty { " dirty" } else { "" };
    let version_info =
        format!("{version} (commit {commit}{dirty_suffix}, built {build_date}, target {target})");

    println!("cargo:rustc-env=PWCLI_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=PWCLI_BUILD_EPOCH={build_epoch}");
    println!("cargo:rustc-env=PWCLI_BUILD_DATE={build_date}");
    println!("cargo:rustc-env=PWCLI_BUILD_TARGET={target}");
    println!("cargo:rustc-env=PWCLI_VERSION_INFO={version_info}");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/index");
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn generate_web_assets(out_dir: &Path) {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let dist = Path::new(&manifest_dir).join("../dist");
    println!("cargo:rerun-if-changed={}", dist.display());

    let mut files = Vec::new();
    collect_files(&dist, &dist, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut code = String::from(
        "pub fn generated_asset(path: &str) -> Option<EmbeddedAsset> {\n    match path {\n",
    );
    for (relative, absolute) in files {
        let mime = mime_for(&relative);
        code.push_str(&format!(
            "        {:?} => Some(EmbeddedAsset {{ bytes: include_bytes!({:?}), content_type: {:?} }}),\n",
            relative,
            absolute.to_string_lossy(),
            mime,
        ));
    }
    code.push_str("        _ => None,\n    }\n}\n");
    fs::write(out_dir.join("web_assets_generated.rs"), code).unwrap();
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<(String, std::path::PathBuf)>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, files);
        } else if path.is_file() {
            if let Ok(relative) = path.strip_prefix(root) {
                files.push((relative.to_string_lossy().replace('\\', "/"), path));
            }
        }
    }
}

fn mime_for(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|value| value.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("map") => "application/json",
        _ => "application/octet-stream",
    }
}
