use agent_client_protocol::schema::{
    ContentBlock, ContentChunk, InitializeRequest, InitializeResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PermissionOption,
    PermissionOptionKind, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectOption, SessionNotification, SessionUpdate, StopReason, TextContent,
    ToolCall, ToolCallUpdate, ToolCallUpdateFields, Usage, UsageUpdate,
};
use agent_client_protocol::{Agent, Client, ConnectionTo};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let session_name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "mock-session".to_string());
    let transport = agent_client_protocol::ByteStreams::new(
        tokio::io::stdout().compat_write(),
        tokio::io::stdin().compat(),
    );
    Agent
        .builder()
        .on_receive_request(
            async |request: InitializeRequest, responder, _cx| {
                responder.respond(InitializeResponse::new(request.protocol_version))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: NewSessionRequest, responder, _cx| {
                responder.respond(
                    NewSessionResponse::new(session_name.clone())
                        .config_options(mock_config_options()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |_request: LoadSessionRequest, responder, _cx| {
                responder.respond(LoadSessionResponse::new().config_options(mock_config_options()))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |request: PromptRequest, responder, cx: ConnectionTo<Client>| {
                let session_id = request.session_id.clone();
                let connection = cx.clone();
                cx.spawn(async move {
                    connection.send_notification(SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::Text(
                            TextContent::new("mock reasoning"),
                        ))),
                    ))?;
                    connection.send_notification(SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::ToolCall(ToolCall::new("mock-tool", "inspect workspace")),
                    ))?;
                    let permission = connection
                        .send_request(RequestPermissionRequest::new(
                            session_id.clone(),
                            ToolCallUpdate::new("mock-tool", ToolCallUpdateFields::new()),
                            vec![
                                PermissionOption::new(
                                    "allow",
                                    "Allow once",
                                    PermissionOptionKind::AllowOnce,
                                ),
                                PermissionOption::new(
                                    "reject",
                                    "Reject",
                                    PermissionOptionKind::RejectOnce,
                                ),
                            ],
                        ))
                        .block_task()
                        .await?;
                    let approved = matches!(
                        permission.outcome,
                        RequestPermissionOutcome::Selected(ref selected)
                            if selected.option_id.to_string() == "allow"
                    );
                    connection.send_notification(SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                            TextContent::new(if approved {
                                "mock approved"
                            } else {
                                "mock rejected"
                            }),
                        ))),
                    ))?;
                    connection.send_notification(SessionNotification::new(
                        session_id,
                        SessionUpdate::UsageUpdate(UsageUpdate::new(128, 4096)),
                    ))?;
                    responder.respond(
                        PromptResponse::new(StopReason::EndTurn).usage(Usage::new(24, 16, 8)),
                    )?;
                    Ok(())
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, |_connection| async move {
            std::future::pending::<()>().await;
            #[allow(unreachable_code)]
            Ok(())
        })
        .await?;
    Ok(())
}

fn mock_config_options() -> Vec<SessionConfigOption> {
    vec![SessionConfigOption::select(
        "model",
        "Model",
        "mock-fast",
        vec![
            SessionConfigSelectOption::new("mock-fast", "Mock Fast"),
            SessionConfigSelectOption::new("mock-deep", "Mock Deep"),
        ],
    )
    .category(SessionConfigOptionCategory::Model)]
}
