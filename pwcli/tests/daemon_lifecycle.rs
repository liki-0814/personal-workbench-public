use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};
use std::thread;
use std::time::Duration;

fn pwcli(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pwcli"))
        .args(args)
        .env("HOME", home)
        .output()
        .unwrap()
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn spawn_delayed_openai_sse() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let header_end = loop {
            let count = socket.read(&mut buffer).unwrap();
            assert!(count > 0, "mock LLM request ended before headers");
            request.extend_from_slice(&buffer[..count]);
            if let Some(position) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|value| value.trim().parse::<usize>().ok())
            })
            .unwrap_or_default();
        while request.len() - header_end < content_length {
            let count = socket.read(&mut buffer).unwrap();
            assert!(count > 0, "mock LLM request body ended early");
            request.extend_from_slice(&buffer[..count]);
        }

        thread::sleep(Duration::from_millis(400));
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"persisted after disconnect\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).unwrap();
    });
    (port, handle)
}

struct DaemonGuard<'a> {
    home: &'a Path,
}

impl Drop for DaemonGuard<'_> {
    fn drop(&mut self) {
        let _ = pwcli(self.home, &["daemon", "stop"]);
    }
}

#[test]
fn daemon_start_is_idempotent_and_stop_is_controlled() {
    let home = tempfile::tempdir().unwrap();
    let data_dir = home.path().join("data");
    let port = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    fs::create_dir_all(home.path().join(".pwcli")).unwrap();
    let config = serde_json::json!({
        "schemaVersion": 2,
        "server": {
            "backendPort": port,
            "dataDir": data_dir,
        },
        "tools": { "fsBase": home.path() },
        "providers": [{
            "name": "smoke",
            "base_url": "http://127.0.0.1:9",
            "api_key": "smoke-only",
            "protocol": "openai",
            "model": "smoke-model",
            "models": [{ "id": "smoke-model", "name": "Smoke" }]
        }],
        "active_provider": "smoke"
    });
    fs::write(
        home.path().join(".pwcli/config.json"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();

    let start = pwcli(home.path(), &["daemon", "start"]);
    assert!(start.status.success(), "{}", output_text(&start));
    let _guard = DaemonGuard { home: home.path() };

    let second_start = pwcli(home.path(), &["daemon", "start"]);
    assert!(
        second_start.status.success(),
        "{}",
        output_text(&second_start)
    );
    assert!(
        String::from_utf8_lossy(&second_start.stdout).contains("already running"),
        "{}",
        output_text(&second_start)
    );

    let status = pwcli(home.path(), &["daemon", "status", "--json"]);
    assert!(status.status.success(), "{}", output_text(&status));
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["running"], true);
    assert_eq!(status["healthy"], true);
    assert_eq!(status["httpAddress"], format!("http://127.0.0.1:{port}"));
    assert!(status["pid"].as_u64().is_some());

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let web = client
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .unwrap();
    assert!(web.status().is_success());
    assert!(web.text().unwrap().contains("<div id=\"root\"></div>"));
    let config = client
        .get(format!("http://127.0.0.1:{port}/api/local-config"))
        .send()
        .unwrap();
    assert!(config.status().is_success());
    assert_eq!(config.json::<serde_json::Value>().unwrap()["success"], true);

    let saved = client
        .put(format!("http://127.0.0.1:{port}/api/data/smoke"))
        .json(&serde_json::json!({ "value": { "native": true } }))
        .send()
        .unwrap();
    assert!(saved.status().is_success());
    let loaded = client
        .get(format!("http://127.0.0.1:{port}/api/data/smoke"))
        .send()
        .unwrap()
        .json::<serde_json::Value>()
        .unwrap();
    assert_eq!(loaded["data"]["native"], true);

    let jobs = client
        .get(format!("http://127.0.0.1:{port}/api/jobs"))
        .send()
        .unwrap();
    assert!(jobs.status().is_success());
    assert!(jobs.json::<serde_json::Value>().unwrap()["jobs"].is_array());

    let acp_sessions = client
        .get(format!("http://127.0.0.1:{port}/api/agent/acp/sessions"))
        .send()
        .unwrap();
    assert!(acp_sessions.status().is_success());
    assert!(acp_sessions.json::<serde_json::Value>().unwrap()["sessions"].is_array());

    let created_session = client
        .post(format!("http://127.0.0.1:{port}/api/agent/sessions"))
        .json(&serde_json::json!({
            "name": "tui-reconnect-smoke",
            "cwd": home.path(),
        }))
        .send()
        .unwrap()
        .json::<serde_json::Value>()
        .unwrap();
    let durable_session_id = created_session["id"].as_str().unwrap().to_string();
    let snapshot = client
        .get(format!(
            "http://127.0.0.1:{port}/api/agent/sessions/{durable_session_id}/snapshot"
        ))
        .send()
        .unwrap();
    assert!(snapshot.status().is_success());

    let missing_api = client
        .get(format!("http://127.0.0.1:{port}/api/does-not-exist"))
        .send()
        .unwrap();
    assert_eq!(missing_api.status(), reqwest::StatusCode::NOT_FOUND);
    assert!(missing_api
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json")));

    let stop = pwcli(home.path(), &["daemon", "stop"]);
    assert!(stop.status.success(), "{}", output_text(&stop));

    let restart = pwcli(home.path(), &["daemon", "start"]);
    assert!(restart.status.success(), "{}", output_text(&restart));
    let sessions = client
        .get(format!("http://127.0.0.1:{port}/api/agent/sessions"))
        .send()
        .unwrap()
        .json::<Vec<(String, String)>>()
        .unwrap();
    assert!(sessions
        .iter()
        .any(|(id, name)| id == &durable_session_id && name == "tui-reconnect-smoke"));
    let second_stop = pwcli(home.path(), &["daemon", "stop"]);
    assert!(
        second_stop.status.success(),
        "{}",
        output_text(&second_stop)
    );

    let stopped = pwcli(home.path(), &["daemon", "status", "--json"]);
    assert!(stopped.status.success(), "{}", output_text(&stopped));
    let stopped: serde_json::Value = serde_json::from_slice(&stopped.stdout).unwrap();
    assert_eq!(stopped["running"], false);
    assert_eq!(stopped["healthy"], false);
    assert!(!data_dir.join("run/daemon.json").exists());
}

#[test]
fn daemon_can_boot_before_provider_setup() {
    let home = tempfile::tempdir().unwrap();
    let data_dir = home.path().join("data");
    let port = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    fs::create_dir_all(home.path().join(".pwcli")).unwrap();
    fs::write(
        home.path().join(".pwcli/config.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": 2,
            "server": { "backendPort": port, "dataDir": data_dir }
        }))
        .unwrap(),
    )
    .unwrap();

    let start = pwcli(home.path(), &["daemon", "start"]);
    assert!(start.status.success(), "{}", output_text(&start));
    let _guard = DaemonGuard { home: home.path() };
    let status = pwcli(home.path(), &["daemon", "status", "--json"]);
    assert!(status.status.success(), "{}", output_text(&status));
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["healthy"], true);
}

#[test]
fn daemon_reloads_external_config_edits_into_frontend_projections() {
    let home = tempfile::tempdir().unwrap();
    let data_dir = home.path().join("data");
    let port = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config_path = home.path().join(".pwcli/config.json");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let config = |provider: &str, language: &str| {
        serde_json::json!({
            "schemaVersion": 2,
            "server": { "backendPort": port, "dataDir": data_dir },
            "ai": { "responseLanguage": language },
            "providers": [{
                "name": provider,
                "base_url": "http://127.0.0.1:9",
                "api_key": "smoke-only",
                "protocol": "openai",
                "model": "smoke-model",
                "models": [{ "id": "smoke-model", "name": "Smoke" }]
            }],
            "active_provider": provider
        })
    };
    fs::write(
        &config_path,
        serde_json::to_vec_pretty(&config("before", "zh-CN")).unwrap(),
    )
    .unwrap();

    let start = pwcli(home.path(), &["daemon", "start"]);
    assert!(start.status.success(), "{}", output_text(&start));
    let _guard = DaemonGuard { home: home.path() };
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    fs::write(
        &config_path,
        serde_json::to_vec_pretty(&config("after", "en")).unwrap(),
    )
    .unwrap();

    let mut reloaded = false;
    for _ in 0..30 {
        let local_config = client
            .get(format!("http://127.0.0.1:{port}/api/local-config"))
            .send()
            .unwrap()
            .json::<serde_json::Value>()
            .unwrap();
        let providers = client
            .get(format!("http://127.0.0.1:{port}/api/data/ai_providers"))
            .send()
            .unwrap()
            .json::<serde_json::Value>()
            .unwrap();
        reloaded = local_config["data"]["ai"]["responseLanguage"] == "en"
            && providers["data"][0]["name"] == "after";
        if reloaded {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    assert!(
        reloaded,
        "external config edit did not reach frontend projections"
    );
}

#[test]
fn daemon_finishes_and_persists_chat_after_sse_client_disconnects() {
    let home = tempfile::tempdir().unwrap();
    let data_dir = home.path().join("data");
    let daemon_port = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (llm_port, llm_thread) = spawn_delayed_openai_sse();
    fs::create_dir_all(home.path().join(".pwcli")).unwrap();
    fs::write(
        home.path().join(".pwcli/config.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": 2,
            "server": { "backendPort": daemon_port, "dataDir": data_dir },
            "tools": { "fsBase": home.path() },
            "features": { "autoMemoryExtract": false },
            "providers": [{
                "name": "disconnect-smoke",
                "base_url": format!("http://127.0.0.1:{llm_port}"),
                "api_key": "smoke-only",
                "protocol": "openai",
                "model": "smoke-model",
                "models": [{ "id": "smoke-model", "name": "Smoke" }]
            }],
            "active_provider": "disconnect-smoke"
        }))
        .unwrap(),
    )
    .unwrap();

    let start = pwcli(home.path(), &["daemon", "start"]);
    assert!(start.status.success(), "{}", output_text(&start));
    let _guard = DaemonGuard { home: home.path() };
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let session = client
        .post(format!("http://127.0.0.1:{daemon_port}/api/agent/sessions"))
        .json(&serde_json::json!({
            "name": "disconnect-smoke",
            "cwd": home.path(),
        }))
        .send()
        .unwrap()
        .json::<serde_json::Value>()
        .unwrap();
    let session_id = session["id"].as_str().unwrap();
    let body = serde_json::to_string(&serde_json::json!({
        "messages": [{ "role": "user", "content": "finish this", "images": [] }],
        "system_prompt": "Reply once.",
        "thinking": false
    }))
    .unwrap();
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", daemon_port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "POST /api/agent/sessions/{session_id}/stream HTTP/1.1\r\nHost: 127.0.0.1:{daemon_port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    stream.flush().unwrap();
    let mut first_event = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !String::from_utf8_lossy(&first_event).contains("first_token") {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "stream ended before first_token");
        first_event.extend_from_slice(&buffer[..count]);
    }
    drop(stream);

    let snapshot_url =
        format!("http://127.0.0.1:{daemon_port}/api/agent/sessions/{session_id}/snapshot");
    let mut persisted = false;
    for _ in 0..50 {
        let snapshot = client
            .get(&snapshot_url)
            .send()
            .unwrap()
            .json::<serde_json::Value>()
            .unwrap();
        persisted = snapshot["messages"].as_array().is_some_and(|messages| {
            messages.iter().any(|message| {
                message["role"] == "assistant" && message["content"] == "persisted after disconnect"
            })
        });
        if persisted {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    llm_thread.join().unwrap();
    assert!(
        persisted,
        "assistant result was not persisted after disconnect"
    );
}

#[test]
fn daemon_refuses_to_steal_an_occupied_port() {
    let home = tempfile::tempdir().unwrap();
    let data_dir = home.path().join("data");
    let occupied = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = occupied.local_addr().unwrap().port();
    fs::create_dir_all(home.path().join(".pwcli")).unwrap();
    fs::write(
        home.path().join(".pwcli/config.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": 2,
            "server": { "backendPort": port, "dataDir": data_dir }
        }))
        .unwrap(),
    )
    .unwrap();

    let start = pwcli(home.path(), &["daemon", "start"]);
    assert!(!start.status.success(), "{}", output_text(&start));
    assert!(
        String::from_utf8_lossy(&start.stderr).contains("exited"),
        "{}",
        output_text(&start)
    );
    assert_eq!(occupied.local_addr().unwrap().port(), port);

    let status = pwcli(home.path(), &["daemon", "status", "--json"]);
    assert!(status.status.success(), "{}", output_text(&status));
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["running"], false);
    assert_eq!(status["healthy"], false);
}
