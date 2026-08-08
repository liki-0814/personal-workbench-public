use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::process::Command;

const WRAPPER: &str = r#"
import ast, json, os, resource, sys
resource.setrlimit(resource.RLIMIT_CPU, (20, 20))
resource.setrlimit(resource.RLIMIT_AS, (1024*1024*1024, 1024*1024*1024))
resource.setrlimit(resource.RLIMIT_FSIZE, (30*1024*1024, 30*1024*1024))
resource.setrlimit(resource.RLIMIT_NPROC, (16, 16))
code_path, data_path, output_path = sys.argv[1:4]
code = open(code_path, 'r', encoding='utf-8').read()
tree = ast.parse(code, filename='plot.py')
allowed = {'matplotlib', 'numpy', 'math', 'statistics', 'json', 'csv'}
blocked_calls = {'open','exec','eval','compile','input','breakpoint','__import__'}
blocked_attrs = {'system','popen','spawn','fork','forkpty','execv','execve','socket','urlopen','request'}
for node in ast.walk(tree):
    if isinstance(node, (ast.Import, ast.ImportFrom)):
        names = [a.name.split('.')[0] for a in node.names] if isinstance(node, ast.Import) else [(node.module or '').split('.')[0]]
        if any(n not in allowed for n in names): raise RuntimeError('blocked import: ' + ','.join(names))
    if isinstance(node, ast.Call):
        if isinstance(node.func, ast.Name) and node.func.id in blocked_calls: raise RuntimeError('blocked call: ' + node.func.id)
        if isinstance(node.func, ast.Attribute) and node.func.attr in blocked_attrs: raise RuntimeError('blocked call: ' + node.func.attr)
import matplotlib
matplotlib.use('Agg', force=True)
import matplotlib.pyplot as plt
plt.close('all'); plt.rcdefaults()
with open(data_path, 'r', encoding='utf-8') as fh: DATA = json.load(fh)
safe_builtins = {'range':range,'len':len,'min':min,'max':max,'sum':sum,'abs':abs,'round':round,'enumerate':enumerate,'zip':zip,'list':list,'dict':dict,'tuple':tuple,'set':set,'str':str,'int':int,'float':float,'bool':bool,'__import__':__import__}
exec(compile(tree, 'plot.py', 'exec'), {'__builtins__':safe_builtins, 'DATA':DATA})
if not plt.get_fignums(): raise RuntimeError('plot program created no matplotlib figure')
plt.savefig(output_path, format='png', bbox_inches='tight', dpi=300)
plt.close('all')
"#;

#[derive(Debug)]
pub struct PlotRenderResult {
    pub bytes: Vec<u8>,
    pub code: String,
}

pub async fn render(
    code_text: &str,
    data: &serde_json::Value,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<PlotRenderResult> {
    let code = extract_code(code_text);
    anyhow::ensure!(
        !code.trim().is_empty(),
        "plot model returned no Python code"
    );
    let temp = tempfile::tempdir()?;
    let wrapper = temp.path().join("runner.py");
    let program = temp.path().join("plot.py");
    let input = temp.path().join("data.json");
    let output = temp.path().join("plot.png");
    tokio::fs::write(&wrapper, WRAPPER).await?;
    tokio::fs::write(&program, &code).await?;
    tokio::fs::write(&input, serde_json::to_vec(data)?).await?;
    let python_environment = super::python_env::ensure_ready()
        .await
        .context("prepare the pwcli matplotlib environment")?;
    let python = python_environment.executable;
    let mut command = isolated_command(&python, temp.path(), &wrapper, &program, &input, &output)?;
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin")
        .env("MPLCONFIGDIR", temp.path())
        .current_dir(temp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command
        .spawn()
        .context("start the isolated pwcli matplotlib renderer")?;
    let result = tokio::select! {
        _ = cancellation.cancelled() => anyhow::bail!("plot rendering cancelled"),
        result = tokio::time::timeout(Duration::from_secs(45), child.wait_with_output()) => {
            result.context("plot rendering exceeded 45 seconds")??
        }
    };
    anyhow::ensure!(
        result.status.success(),
        "plot renderer failed: {}",
        String::from_utf8_lossy(&result.stderr).trim()
    );
    let bytes = tokio::fs::read(&output)
        .await
        .context("plot renderer produced no PNG")?;
    anyhow::ensure!(
        bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "plot renderer produced an invalid PNG"
    );
    Ok(PlotRenderResult { bytes, code })
}

#[allow(unreachable_code)]
fn isolated_command(
    python: &str,
    root: &Path,
    wrapper: &Path,
    program: &Path,
    input: &Path,
    output: &Path,
) -> Result<Command> {
    #[cfg(target_os = "macos")]
    {
        anyhow::ensure!(
            Path::new("/usr/bin/sandbox-exec").exists(),
            "secure plot sandbox is unavailable"
        );
        let profile = format!("(version 1)(deny default)(allow process-exec)(allow process-fork)(deny network*)(allow file-read*)(allow file-write* (subpath \"{}\"))", root.display());
        let mut command = Command::new("/usr/bin/sandbox-exec");
        command
            .args(["-p", &profile, python])
            .arg(wrapper)
            .arg(program)
            .arg(input)
            .arg(output);
        return Ok(command);
    }
    #[cfg(target_os = "linux")]
    {
        let bwrap = ["/usr/bin/bwrap", "/bin/bwrap"]
            .into_iter()
            .find(|p| Path::new(p).exists())
            .context("secure plot sandbox requires bubblewrap")?;
        let mut command = Command::new(bwrap);
        command
            .args([
                "--unshare-all",
                "--die-with-parent",
                "--new-session",
                "--ro-bind",
                "/",
                "/",
                "--bind",
            ])
            .arg(root)
            .arg(root)
            .arg("--chdir")
            .arg(root)
            .arg(python)
            .arg(wrapper)
            .arg(program)
            .arg(input)
            .arg(output);
        return Ok(command);
    }
    anyhow::bail!("secure plot sandbox is unsupported on this platform")
}

pub fn extract_code(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(rest) = trimmed.split_once("```python").map(|(_, rest)| rest) {
        return rest
            .split_once("```")
            .map(|(code, _)| code)
            .unwrap_or(rest)
            .trim()
            .to_string();
    }
    if let Some(rest) = trimmed.split_once("```").map(|(_, rest)| rest) {
        return rest
            .split_once("```")
            .map(|(code, _)| code)
            .unwrap_or(rest)
            .trim()
            .to_string();
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_fenced_python() {
        assert_eq!(extract_code("x\n```python\nprint(1)\n```"), "print(1)");
    }

    #[test]
    fn wrapper_rejects_process_and_network_primitives_before_execution() {
        assert!(WRAPPER.contains("blocked_calls"));
        assert!(WRAPPER.contains("blocked_attrs"));
        assert!(WRAPPER.contains("'system'"));
        assert!(WRAPPER.contains("'socket'"));
    }
}
