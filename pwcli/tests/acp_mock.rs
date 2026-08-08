#![cfg(feature = "mock-acp")]

use std::time::Duration;

use pwcli::runtime::tools::code_agent::acp_runner;
use pwcli::runtime::tools::code_agent::{
    execute_code_agent, AgentTransportKind, CodeAgentArgs, SubAgentBackend,
};

struct MockBackend {
    bin: String,
    session_id: String,
}

impl SubAgentBackend for MockBackend {
    fn name(&self) -> &str {
        "mock"
    }

    fn bin_path(&self) -> &str {
        &self.bin
    }

    fn install_hint(&self) -> &str {
        "test fixture"
    }

    fn transport_kind(&self) -> AgentTransportKind {
        AgentTransportKind::Acp
    }

    fn acp_command(&self) -> Option<(String, Vec<String>)> {
        Some((self.bin.clone(), vec![self.session_id.clone()]))
    }
}

fn args(cwd: &std::path::Path, resume_session_id: Option<String>) -> CodeAgentArgs {
    CodeAgentArgs {
        backend: None,
        task: "exercise the mock ACP protocol".into(),
        cwd: cwd.to_string_lossy().into_owned(),
        mode: Some("edit".into()),
        effort: None,
        timeout_secs: Some(10),
        resume_session_id,
        model: None,
        context_window: None,
        permission_mode: Some("default".into()),
        spec: None,
        project_rules: None,
    }
}

async fn resolve_next_permission(session_id: &str, option_id: &str) -> bool {
    for _ in 0..500 {
        if let Some(permission) = acp_runner::pending_permissions()
            .into_iter()
            .find(|permission| permission.session_id == session_id)
        {
            acp_runner::resolve_permission(&permission.id, option_id).unwrap();
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn mock_acp_covers_permission_stream_usage_replay_resume_and_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = format!("mock-{}", uuid::Uuid::now_v7().simple());
    let backend = MockBackend {
        bin: env!("CARGO_BIN_EXE_pwcli-mock-acp").into(),
        session_id: session_id.clone(),
    };

    let first = tokio::spawn({
        let cwd = dir.path().to_path_buf();
        let root = cwd.clone();
        async move { execute_code_agent(args(&cwd, None), &backend, &root).await }
    });
    if !resolve_next_permission(&session_id, "allow").await {
        panic!("mock ACP turn ended before permission: {:?}", first.await);
    }
    let first = first.await.unwrap().unwrap();
    assert_eq!(first.session_id, session_id);
    assert_eq!(first.output, "mock approved");
    assert_eq!(first.usage.unwrap().input_tokens, 16);

    let events = acp_runner::events_after(Some(&session_id), 0);
    assert!(events.iter().any(|event| event.kind == "reasoning_chunk"));
    assert!(events.iter().any(|event| event.kind == "tool_call"));
    assert!(events
        .iter()
        .any(|event| event.kind == "permission_request"));
    assert!(events.iter().any(|event| event.kind == "usage_update"));
    let info = acp_runner::list_sessions()
        .into_iter()
        .find(|session| session.session_id == session_id)
        .unwrap();
    assert_eq!(info.context_tokens, 128);
    assert_eq!(info.context_window, 4096);
    assert_eq!(info.config_options[0]["category"], "model");
    assert_eq!(info.config_options[0]["options"][0]["value"], "mock-fast");

    acp_runner::cancel_session(&session_id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(acp_runner::events_after(Some(&session_id), 0)
        .iter()
        .any(|event| event.kind == "cancel_requested"));
    acp_runner::close_session(&session_id).await.unwrap();
    assert!(acp_runner::events_after(Some(&session_id), 0)
        .iter()
        .any(|event| event.kind == "session_closed"));

    let resumed_backend = MockBackend {
        bin: env!("CARGO_BIN_EXE_pwcli-mock-acp").into(),
        session_id: session_id.clone(),
    };
    let resumed = tokio::spawn({
        let cwd = dir.path().to_path_buf();
        let root = cwd.clone();
        let resume = session_id.clone();
        async move { execute_code_agent(args(&cwd, Some(resume)), &resumed_backend, &root).await }
    });
    if !resolve_next_permission(&session_id, "reject").await {
        panic!(
            "resumed mock ACP turn ended before permission: {:?}",
            resumed.await
        );
    }
    let resumed = resumed.await.unwrap().unwrap();
    assert_eq!(resumed.session_id, session_id);
    assert_eq!(resumed.output, "mock rejected");
    acp_runner::close_session(&session_id).await.unwrap();
}

#[tokio::test]
async fn mock_acp_keeps_parallel_sessions_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let first_id = format!("parallel-a-{}", uuid::Uuid::now_v7().simple());
    let second_id = format!("parallel-b-{}", uuid::Uuid::now_v7().simple());
    let run = |session_id: String| {
        let cwd = dir.path().to_path_buf();
        let root = cwd.clone();
        tokio::spawn(async move {
            let backend = MockBackend {
                bin: env!("CARGO_BIN_EXE_pwcli-mock-acp").into(),
                session_id,
            };
            let mut request = args(&cwd, None);
            request.permission_mode = Some("bypass".into());
            execute_code_agent(request, &backend, &root).await
        })
    };
    let (first, second) = tokio::join!(run(first_id.clone()), run(second_id.clone()));
    let first = first.unwrap().unwrap();
    let second = second.unwrap().unwrap();
    assert_eq!(first.session_id, first_id);
    assert_eq!(second.session_id, second_id);
    assert_eq!(first.output, "mock approved");
    assert_eq!(second.output, "mock approved");
    assert!(acp_runner::events_after(Some(&first_id), 0)
        .iter()
        .all(|event| event.session_id == first_id));
    assert!(acp_runner::events_after(Some(&second_id), 0)
        .iter()
        .all(|event| event.session_id == second_id));
    acp_runner::close_session(&first_id).await.unwrap();
    acp_runner::close_session(&second_id).await.unwrap();
}
