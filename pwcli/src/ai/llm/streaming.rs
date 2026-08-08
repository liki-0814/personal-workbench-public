use crate::ai::llm::models::*;
use futures::stream::Stream;

/// 将非流式响应当作单事件流返回
pub fn single_event_stream(response: AiResponse) -> impl Stream<Item = StreamEvent> {
    use futures::stream;
    let mut events = Vec::new();

    if let Some(tool_calls) = response.tool_calls {
        for tc in tool_calls {
            let id = tc.id.clone();
            let name = tc.function.name.clone();
            let args = tc.function.arguments;
            events.push(StreamEvent::ToolCallStart {
                id: id.clone(),
                name,
                thought_signature: tc.thought_signature.clone(),
            });
            events.push(StreamEvent::ToolCallDelta {
                id: id.clone(),
                arguments_delta: args,
            });
            events.push(StreamEvent::ToolCallEnd { id });
        }
    }

    if !response.content.is_empty() {
        events.push(StreamEvent::TextDelta(response.content));
    }

    events.push(StreamEvent::Done(response.usage));
    stream::iter(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_event_stream_text_only() {
        let response = AiResponse {
            content: "Hello".to_string(),
            tool_calls: None,
            usage: Some(TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        };

        let events: Vec<StreamEvent> = futures::executor::block_on(async {
            use futures::StreamExt;
            single_event_stream(response).collect().await
        });

        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], StreamEvent::TextDelta(t) if t == "Hello"));
        assert!(matches!(&events[1], StreamEvent::Done(Some(u)) if u.total_tokens == 15));
    }

    #[test]
    fn test_single_event_stream_with_tool_call() {
        let response = AiResponse {
            content: "".to_string(),
            tool_calls: Some(vec![ToolCall {
                id: "tc_1".to_string(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: "list_files".to_string(),
                    arguments: r#"{"path": "/tmp"}"#.to_string(),
                },
                thought_signature: None,
            }]),
            usage: None,
        };

        let events: Vec<StreamEvent> = futures::executor::block_on(async {
            use futures::StreamExt;
            single_event_stream(response).collect().await
        });

        assert_eq!(events.len(), 4);
        assert!(
            matches!(&events[0], StreamEvent::ToolCallStart { id, name, .. } if id == "tc_1" && name == "list_files")
        );
        assert!(
            matches!(&events[1], StreamEvent::ToolCallDelta { id, arguments_delta } if id == "tc_1" && arguments_delta == r#"{"path": "/tmp"}"#)
        );
        assert!(matches!(&events[2], StreamEvent::ToolCallEnd { id } if id == "tc_1"));
        assert!(matches!(&events[3], StreamEvent::Done(None)));
    }
}
