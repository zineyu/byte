use std::pin::Pin;

use async_trait::async_trait;
use byte_protocol::{MessageRole, RunMessage};
use futures::Stream;

/// An event emitted by a model provider during a streaming generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEvent {
    /// A new assistant message has begun.
    MessageStarted { message_id: String },
    /// Additional text content for the active assistant message.
    TextDelta { message_id: String, delta: String },
    /// The active assistant message is complete.
    MessageCompleted { message_id: String },
}

/// Errors that can occur when invoking a model provider.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider is not configured: {0}")]
    Configuration(String),
    #[error("provider request failed: {0}")]
    Request(String),
    #[error("provider response is invalid: {0}")]
    InvalidResponse(String),
}

pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send>>;

/// Byte's model-provider boundary. Callers depend on this trait, not on any
/// particular third-party SDK.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Send a single conversation turn to the model and receive incremental
    /// assistant events. The returned stream must yield exactly one
    /// `MessageStarted`/`MessageCompleted` pair, with zero or more `TextDelta`
    /// events in between.
    async fn send_message(
        &self,
        messages: Vec<RunMessage>,
    ) -> Result<ProviderStream, ProviderError>;
}

/// A mock provider that echoes the final developer message back in chunks.
pub struct EchoProvider {
    pub chunk_size: usize,
    pub delay: std::time::Duration,
}

impl Default for EchoProvider {
    fn default() -> Self {
        Self {
            chunk_size: 5,
            delay: std::time::Duration::ZERO,
        }
    }
}

#[async_trait]
impl ModelProvider for EchoProvider {
    async fn send_message(
        &self,
        messages: Vec<RunMessage>,
    ) -> Result<ProviderStream, ProviderError> {
        let last = messages
            .into_iter()
            .filter(|m| m.role == MessageRole::Developer)
            .map(|m| m.content)
            .next_back()
            .unwrap_or_default();

        let content = format!("Echo: {}", last);
        let chunks: Vec<String> = content
            .chars()
            .collect::<Vec<_>>()
            .chunks(self.chunk_size)
            .map(|chunk| chunk.iter().collect())
            .collect();

        let delay = self.delay;
        let stream = async_stream::try_stream! {
            let message_id = uuid::Uuid::new_v4().to_string();
            yield ProviderEvent::MessageStarted { message_id: message_id.clone() };
            for chunk in chunks {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                yield ProviderEvent::TextDelta { message_id: message_id.clone(), delta: chunk };
            }
            yield ProviderEvent::MessageCompleted { message_id };
        };

        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn echo_provider_streams_developer_message_back() {
        let provider = EchoProvider {
            chunk_size: 3,
            ..Default::default()
        };
        let messages = vec![RunMessage {
            role: MessageRole::Developer,
            content: "hello".to_owned(),
        }];

        let stream = provider
            .send_message(messages)
            .await
            .expect("stream starts");
        let events: Vec<_> = stream.collect().await;

        assert_eq!(events.len(), 6);
        assert!(matches!(
            &events[0],
            Ok(ProviderEvent::MessageStarted { .. })
        ));
        assert!(matches!(
            &events[1],
            Ok(ProviderEvent::TextDelta { delta, .. }) if delta == "Ech"
        ));
        assert!(matches!(
            &events[2],
            Ok(ProviderEvent::TextDelta { delta, .. }) if delta == "o: "
        ));
        assert!(matches!(
            &events[3],
            Ok(ProviderEvent::TextDelta { delta, .. }) if delta == "hel"
        ));
        assert!(matches!(
            &events[4],
            Ok(ProviderEvent::TextDelta { delta, .. }) if delta == "lo"
        ));
        assert!(matches!(
            &events[5],
            Ok(ProviderEvent::MessageCompleted { .. })
        ));
    }

    #[tokio::test]
    async fn echo_provider_handles_empty_messages() {
        let provider = EchoProvider {
            chunk_size: 5,
            ..Default::default()
        };
        let stream = provider.send_message(vec![]).await.expect("stream starts");
        let events: Vec<_> = stream.collect().await;

        assert_eq!(events.len(), 4);
        assert!(matches!(
            &events[0],
            Ok(ProviderEvent::MessageStarted { .. })
        ));
        assert!(matches!(
            &events[1],
            Ok(ProviderEvent::TextDelta { delta, .. }) if delta == "Echo:"
        ));
        assert!(matches!(
            &events[2],
            Ok(ProviderEvent::TextDelta { delta, .. }) if delta == " "
        ));
        assert!(matches!(
            &events[3],
            Ok(ProviderEvent::MessageCompleted { .. })
        ));
    }
}
