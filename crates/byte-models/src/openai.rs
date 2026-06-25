use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestSystemMessageContent, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent, CreateChatCompletionRequestArgs,
};
use async_openai::Client;
use async_trait::async_trait;
use byte_protocol::{MessageRole, RunMessage};
use futures::StreamExt;

use crate::config::ModelProviderConfig;
use crate::provider::{ModelProvider, ProviderError, ProviderEvent, ProviderStream};

/// An OpenAI-compatible provider using `async-openai` under the hood.
pub struct OpenAiCompatibleProvider {
    client: Client<OpenAIConfig>,
    model: String,
}

impl OpenAiCompatibleProvider {
    pub fn new(config: ModelProviderConfig) -> Self {
        let openai_config = OpenAIConfig::new()
            .with_api_key(config.api_key)
            .with_api_base(config.base_url);

        Self {
            client: Client::with_config(openai_config),
            model: config.model,
        }
    }
}

#[async_trait]
impl ModelProvider for OpenAiCompatibleProvider {
    async fn send_message(
        &self,
        messages: Vec<RunMessage>,
    ) -> Result<ProviderStream, ProviderError> {
        let system_message = ChatCompletionRequestMessage::System(
            ChatCompletionRequestSystemMessage {
                content: ChatCompletionRequestSystemMessageContent::Text(
                    "You are Byte Agent's local coding assistant. This is a single-turn text response; do not use tools.".to_owned(),
                ),
                name: None,
            },
        );

        let chat_messages: Vec<ChatCompletionRequestMessage> = messages
            .into_iter()
            .map(|m| match m.role {
                MessageRole::Developer => {
                    ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                        content: ChatCompletionRequestUserMessageContent::Text(m.content),
                        name: None,
                    })
                }
                MessageRole::Assistant => {
                    ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                        content: ChatCompletionRequestUserMessageContent::Text(m.content),
                        name: None,
                    })
                }
            })
            .collect();

        let request = CreateChatCompletionRequestArgs::default()
            .model(&self.model)
            .messages(
                [system_message]
                    .into_iter()
                    .chain(chat_messages)
                    .collect::<Vec<_>>(),
            )
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
        let message_id_for_deltas = message_id.clone();
        let stream = response.filter_map(move |result| {
            let message_id = message_id_for_deltas.clone();
            async move {
                match result {
                    Ok(chunk) => {
                        let deltas: Vec<String> = chunk
                            .choices
                            .into_iter()
                            .filter_map(|choice| choice.delta.content)
                            .collect();

                        if deltas.is_empty() {
                            None
                        } else {
                            Some(Ok(ProviderEvent::TextDelta {
                                message_id,
                                delta: deltas.join(""),
                            }))
                        }
                    }
                    Err(error) => Some(Err(ProviderError::Request(sanitize_error(&error)))),
                }
            }
        });

        let stream = async_stream::try_stream! {
            yield ProviderEvent::MessageStarted { message_id: message_id.clone() };

            for await event in stream {
                yield event?;
            }

            yield ProviderEvent::MessageCompleted { message_id };
        };

        Ok(Box::pin(stream))
    }
}

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

    #[test]
    fn sanitize_error_strips_api_key_mentions() {
        let error = std::fmt::Error;
        assert_eq!(
            sanitize_error(&error),
            "an error occurred when formatting an argument"
        );
    }
}
