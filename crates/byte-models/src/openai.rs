use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCallChunk,
    ChatCompletionMessageToolCalls, ChatCompletionRequestAssistantMessage,
    ChatCompletionRequestAssistantMessageContent, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessage, ChatCompletionRequestSystemMessageContent,
    ChatCompletionRequestToolMessage, ChatCompletionRequestToolMessageContent,
    ChatCompletionRequestUserMessage, ChatCompletionRequestUserMessageContent,
    ChatCompletionResponseStream, ChatCompletionTool, ChatCompletionTools,
    CreateChatCompletionRequestArgs, FunctionCall, FunctionObject,
};
use async_trait::async_trait;
use byte_protocol::{LlmMessage, MessageBlock, MessageBody, MessageRole, ToolCall};

use crate::config::ModelProviderConfig;
use crate::provider::{ModelProvider, ProviderError, ProviderEvent, ProviderStream};

/// Concatenate all text blocks in a [`MessageBody`] into a single string.
fn body_text(body: &MessageBody) -> String {
    let mut text = String::new();
    for block in &body.0 {
        if let MessageBlock::Text { text: t } = block {
            text.push_str(t);
        }
    }
    text
}

/// Collect every [`ToolCall`] block in a [`MessageBody`].
fn body_tool_calls(body: &MessageBody) -> Vec<ToolCall> {
    body.0
        .iter()
        .filter_map(|block| {
            if let MessageBlock::ToolCall(call) = block {
                Some(call.clone())
            } else {
                None
            }
        })
        .collect()
}

/// Remove any trailing slash from a base URL.
fn normalize_base_url(url: &str) -> String {
    url.trim_end_matches('/').to_owned()
}

/// An OpenAI-compatible provider using `async-openai` under the hood.
#[derive(Debug)]
pub struct OpenAiCompatibleProvider {
    /// Underlying async-openai HTTP client.
    client: Client<OpenAIConfig>,
    /// Model name sent with each chat completion request.
    model: String,
}

impl OpenAiCompatibleProvider {
    /// Create a new provider from the supplied configuration.
    #[must_use]
    pub fn new(config: ModelProviderConfig) -> Self {
        let openai_config = OpenAIConfig::new()
            .with_api_key(config.api_key)
            .with_api_base(normalize_base_url(&config.base_url));

        Self {
            client: Client::with_config(openai_config),
            model: config.model,
        }
    }
}

#[async_trait]
impl ModelProvider for OpenAiCompatibleProvider {
    #[allow(deprecated)]
    async fn send_message(
        &self,
        messages: Vec<LlmMessage>,
        tools: Vec<byte_protocol::ToolDefinition>,
    ) -> Result<ProviderStream, ProviderError> {
        let chat_messages: Vec<ChatCompletionRequestMessage> = messages
            .into_iter()
            .map(|m| {
                let text = body_text(&m.body);
                match m.role {
                    MessageRole::System => {
                        ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
                            content: ChatCompletionRequestSystemMessageContent::Text(text),
                            name: None,
                        })
                    }
                    MessageRole::Developer => {
                        ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                            content: ChatCompletionRequestUserMessageContent::Text(text),
                            name: None,
                        })
                    }
                    MessageRole::Assistant => {
                        let tool_calls = body_tool_calls(&m.body);
                        ChatCompletionRequestMessage::Assistant(
                            ChatCompletionRequestAssistantMessage {
                                content: Some(ChatCompletionRequestAssistantMessageContent::Text(
                                    text,
                                )),
                                name: None,
                                tool_calls: if tool_calls.is_empty() {
                                    None
                                } else {
                                    Some(
                                        tool_calls
                                            .into_iter()
                                            .map(|call| {
                                                ChatCompletionMessageToolCalls::Function(
                                                    ChatCompletionMessageToolCall {
                                                        id: call.id,
                                                        function: FunctionCall {
                                                            name: call.name,
                                                            arguments: call.arguments.to_string(),
                                                        },
                                                    },
                                                )
                                            })
                                            .collect(),
                                    )
                                },
                                function_call: None,
                                refusal: None,
                                audio: None,
                            },
                        )
                    }
                    MessageRole::Tool => {
                        ChatCompletionRequestMessage::Tool(ChatCompletionRequestToolMessage {
                            content: ChatCompletionRequestToolMessageContent::Text(text),
                            tool_call_id: m.tool_call_id.unwrap_or_default(),
                        })
                    }
                    MessageRole::Summary => {
                        // Summary messages are converted to system messages by
                        // `LlmContextBuilder` before reaching the provider; treat
                        // any remaining Summary role as system for safety.
                        ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
                            content: ChatCompletionRequestSystemMessageContent::Text(text),
                            name: None,
                        })
                    }
                }
            })
            .collect();
        let chat_tools: Vec<ChatCompletionTools> = tools
            .into_iter()
            .map(|tool| {
                ChatCompletionTools::Function(ChatCompletionTool {
                    function: FunctionObject {
                        name: tool.name,
                        description: Some(tool.description),
                        parameters: Some(tool.parameters),
                        strict: None,
                    },
                })
            })
            .collect();
        let request = CreateChatCompletionRequestArgs::default()
            .model(&self.model)
            .messages(chat_messages)
            .tools(chat_tools)
            .stream(true)
            .build()
            .map_err(|e| ProviderError::Configuration(e.to_string()))?;

        let response = self
            .client
            .chat()
            .create_stream(request)
            .await
            .map_err(|e| {
                ProviderError::Request(format!(
                    "failed to start chat stream: {}",
                    sanitize_error(&e)
                ))
            })?;

        let message_id = uuid::Uuid::new_v4().to_string();

        Ok(build_provider_stream(response, message_id))
    }
}

/// Build a provider stream from an underlying OpenAI-compatible SSE stream.
///
/// Errors from the underlying stream are propagated as `ProviderError::Request`
/// items and terminate the stream; no `MessageCompleted` event is emitted after
/// an error.
fn build_provider_stream(
    response: ChatCompletionResponseStream,
    message_id: String,
) -> ProviderStream {
    let stream = async_stream::try_stream! {
        yield ProviderEvent::MessageStarted { message_id: message_id.clone() };

        let mut accumulator: Vec<ChatCompletionMessageToolCallChunk> = Vec::new();
        for await result in response {
            let chunk = result.map_err(|error| {
                ProviderError::Request(format!(
                    "chat stream error: {}",
                    sanitize_error(&error)
                ))
            })?;

            let deltas: Vec<String> = chunk
                .choices
                .iter()
                .filter_map(|choice| choice.delta.content.clone())
                .collect();

            let tool_call_chunks: Vec<ChatCompletionMessageToolCallChunk> = chunk
                .choices
                .into_iter()
                .filter_map(|choice| choice.delta.tool_calls)
                .flatten()
                .collect();

            if !deltas.is_empty() {
                yield ProviderEvent::TextDelta {
                    message_id: message_id.clone(),
                    delta: deltas.join(""),
                };
            }
            accumulator.extend(tool_call_chunks);
        }

        let tool_calls = if accumulator.is_empty() {
            None
        } else {
            Some(combine_tool_call_chunks(accumulator))
        };
        yield ProviderEvent::MessageCompleted {
            message_id,
            tool_calls,
        };
    };

    Box::pin(stream)
}

/// Merge streamed tool-call fragments into complete `ToolCall` values.
fn combine_tool_call_chunks(chunks: Vec<ChatCompletionMessageToolCallChunk>) -> Vec<ToolCall> {
    let mut by_index: std::collections::BTreeMap<u32, (Option<String>, Option<String>, String)> =
        std::collections::BTreeMap::new();

    for chunk in chunks {
        let entry = by_index.entry(chunk.index).or_default();
        if let Some(id) = chunk.id {
            entry.0 = Some(id);
        }
        if let Some(function) = chunk.function {
            if let Some(name) = function.name {
                entry.1 = Some(name);
            }
            if let Some(args) = function.arguments {
                entry.2.push_str(&args);
            }
        }
    }

    by_index
        .into_values()
        .filter_map(|(id, name, args)| {
            let name = name?;
            let id = id.unwrap_or_else(|| name.clone());
            let arguments = serde_json::from_str(&args)
                .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
            Some(ToolCall {
                id,
                name,
                arguments,
            })
        })
        .collect()
}

/// Redact sensitive authentication details from an error message.
fn sanitize_error(error: &impl std::fmt::Display) -> String {
    // Do not expose API keys, authorization headers, or raw request bodies.
    let text = error.to_string();
    if text.to_lowercase().contains("api key") || text.to_lowercase().contains("authorization") {
        "provider authentication failed".to_owned()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_openai::error::OpenAIError;
    use async_openai::types::chat::{
        ChatChoiceStream, ChatCompletionStreamResponseDelta, CreateChatCompletionStreamResponse,
    };
    use futures::StreamExt;

    #[test]
    fn sanitize_error_strips_api_key_mentions() {
        assert_eq!(
            sanitize_error(&"Invalid API key provided"),
            "provider authentication failed"
        );
    }

    #[test]
    fn normalize_base_url_removes_trailing_slash() {
        assert_eq!(
            normalize_base_url("https://api.openai.com/v1/"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_base_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1"
        );
    }

    #[allow(deprecated)]
    fn text_chunk(content: &str) -> CreateChatCompletionStreamResponse {
        CreateChatCompletionStreamResponse {
            id: "chatcmpl-test".to_owned(),
            choices: vec![ChatChoiceStream {
                index: 0,
                delta: ChatCompletionStreamResponseDelta {
                    content: Some(content.to_owned()),
                    function_call: None,
                    tool_calls: None,
                    role: None,
                    refusal: None,
                },
                finish_reason: None,
                logprobs: None,
            }],
            created: 0,
            model: "gpt-test".to_owned(),
            service_tier: None,
            system_fingerprint: None,
            object: "chat.completion.chunk".to_owned(),
            usage: None,
        }
    }

    #[tokio::test]
    async fn provider_stream_yields_message_completed_on_success() {
        let response: ChatCompletionResponseStream = Box::pin(futures::stream::iter(vec![
            Ok(text_chunk("Hello,")),
            Ok(text_chunk(" world!")),
        ]));

        let events: Vec<_> = build_provider_stream(response, "msg-1".to_owned())
            .collect()
            .await;
        assert_eq!(events.len(), 4);
        assert!(matches!(
            events[0],
            Ok(ProviderEvent::MessageStarted { .. })
        ));
        assert!(
            matches!(
                &events[1],
                Ok(ProviderEvent::TextDelta { delta, .. }) if delta == "Hello,"
            ),
            "expected first text delta, got {:?}",
            events[1]
        );
        assert!(
            matches!(
                &events[2],
                Ok(ProviderEvent::TextDelta { delta, .. }) if delta == " world!"
            ),
            "expected second text delta, got {:?}",
            events[2]
        );
        assert!(
            matches!(
                &events[3],
                Ok(ProviderEvent::MessageCompleted {
                    tool_calls: None,
                    ..
                })
            ),
            "expected MessageCompleted, got {:?}",
            events[3]
        );
    }

    #[test]
    fn body_text_concatenates_all_text_blocks() {
        let body = MessageBody(vec![
            MessageBlock::Text {
                text: "Hello,".into(),
            },
            MessageBlock::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "main.rs"}),
            }),
            MessageBlock::Text {
                text: " world!".into(),
            },
        ]);
        assert_eq!(body_text(&body), "Hello, world!");
    }

    #[test]
    fn body_tool_calls_extracts_tool_call_blocks() {
        let call = ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "main.rs"}),
        };
        let body = MessageBody(vec![
            MessageBlock::Text {
                text: String::new(),
            },
            MessageBlock::ToolCall(call.clone()),
        ]);
        let calls = body_tool_calls(&body);
        assert_eq!(calls, vec![call]);
    }

    #[tokio::test]
    async fn provider_stream_propagates_underlying_error() {
        let response: ChatCompletionResponseStream = Box::pin(futures::stream::iter(vec![
            Ok(text_chunk("before")),
            Err(OpenAIError::InvalidArgument("mock stream error".to_owned())),
            Ok(text_chunk("after")),
        ]));

        let events: Vec<_> = build_provider_stream(response, "msg-2".to_owned())
            .collect()
            .await;

        assert_eq!(events.len(), 3);
        assert!(matches!(
            events[0],
            Ok(ProviderEvent::MessageStarted { .. })
        ));
        assert!(
            matches!(
                &events[1],
                Ok(ProviderEvent::TextDelta { delta, .. }) if delta == "before"
            ),
            "expected text delta before error, got {:?}",
            events[1]
        );
        assert!(
            matches!(
                &events[2],
                Err(ProviderError::Request(msg)) if msg.contains("mock stream error")
            ),
            "expected propagated request error, got {:?}",
            events[2]
        );
    }
}
