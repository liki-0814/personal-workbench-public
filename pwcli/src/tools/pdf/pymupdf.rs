//! 本地 PyMuPDF fallback。需 `python3 -c "import fitz"`。
//!
//! 流程：
//! 1. 远程 URL → 下载到 `<out_dir>/source.pdf`；本地路径直接用
//! 2. 调 python3 子进程跑内嵌脚本：
//!    - 逐页 `page.get_text()` 拼 markdown，stdout 输出
//!    - 逐页 `page.get_images()` 提取图片存 `<out_dir>/images/p{n}_{j}.png`
//! 3. 写 `<out_dir>/full.md`，删 `source.pdf`（URL 模式）

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::tools::progress;
use crate::tools::web::shared_http_client;

static PYMUPDF_AVAILABLE: OnceLock<bool> = OnceLock::new();

const PYMUPDF_SCRIPT: &str = r#"
import sys, os, json
try:
    import fitz  # PyMuPDF
except Exception as e:
    sys.stderr.write(f"PyMuPDF not installed: {e}\n")
    sys.exit(2)

pdf_path = sys.argv[1]
images_dir = sys.argv[2]
os.makedirs(images_dir, exist_ok=True)

doc = fitz.open(pdf_path)
md_parts = []
for i, page in enumerate(doc):
    md_parts.append(f"\n\n## Page {i+1}\n\n")
    md_parts.append(page.get_text())
    try:
        for j, img in enumerate(page.get_images(full=True)):
            xref = img[0]
            base = doc.extract_image(xref)
            ext = base.get("ext", "png")
            data = base.get("image")
            if not data:
                continue
            out_path = os.path.join(images_dir, f"p{i+1}_{j+1}.{ext}")
            with open(out_path, "wb") as f:
                f.write(data)
            md_parts.append(f"\n\n![](images/p{i+1}_{j+1}.{ext})\n")
    except Exception as e:
        sys.stderr.write(f"image extract page={i+1} err={e}\n")

sys.stdout.write("".join(md_parts))
"#;

/// 检测 `python3 -c "import fitz"`，结果 OnceLock 缓存。
pub fn check_pymupdf_available() -> bool {
    *PYMUPDF_AVAILABLE.get_or_init(|| {
        let output = std::process::Command::new("python3")
            .arg("-c")
            .arg("import fitz")
            .output();
        match output {
            Ok(o) => o.status.success(),
            Err(_) => false,
        }
    })
}

/// 入口。`url_or_path` 既支持 http/https URL 也支持本地路径。
pub async fn fetch_pdf_via_pymupdf(url_or_path: &str, out_dir: &Path) -> Result<String> {
    if !check_pymupdf_available() {
        anyhow::bail!("PyMuPDF 不可用（需要 `pip install pymupdf`）");
    }

    let is_remote = url_or_path.starts_with("http://") || url_or_path.starts_with("https://");
    let (pdf_path, downloaded_temp): (PathBuf, Option<PathBuf>) = if is_remote {
        progress::emit("⬇ 下载 PDF…");
        let dest = out_dir.join("source.pdf");
        let client = shared_http_client()?;
        let bytes = client
            .get(url_or_path)
            .send()
            .await
            .with_context(|| format!("GET {} 失败", url_or_path))?
            .bytes()
            .await
            .context("下载 PDF 字节失败")?;
        let mut f = tokio::fs::File::create(&dest)
            .await
            .with_context(|| format!("创建 {} 失败", dest.display()))?;
        f.write_all(&bytes).await?;
        f.flush().await?;
        (dest.clone(), Some(dest))
    } else {
        let p = PathBuf::from(url_or_path);
        if !p.exists() {
            anyhow::bail!("本地 PDF 不存在: {}", url_or_path);
        }
        (p, None)
    };

    progress::emit("📖 PyMuPDF 解析中…");
    let images_dir = out_dir.join("images");
    tokio::fs::create_dir_all(&images_dir).await.ok();

    let output = Command::new("python3")
        .arg("-c")
        .arg(PYMUPDF_SCRIPT)
        .arg(&pdf_path)
        .arg(&images_dir)
        .output()
        .await
        .context("python3 调用失败")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("PyMuPDF 失败: {}", stderr);
    }
    let md = String::from_utf8_lossy(&output.stdout).to_string();
    if md.trim().is_empty() {
        return Err(anyhow!("PyMuPDF 输出为空"));
    }

    let full_md = out_dir.join("full.md");
    tokio::fs::write(&full_md, &md).await.ok();

    // URL 模式：清理临时源 PDF
    if let Some(tmp) = downloaded_temp {
        let _ = tokio::fs::remove_file(&tmp).await;
    }

    Ok(md)
}
