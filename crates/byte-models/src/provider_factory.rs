use std::sync::Arc;

use thiserror::Error;

use crate::config::ModelProviderConfig;
use crate::openai::OpenAiCompatibleProvider;
use crate::provider::{EchoProvider, ModelProvider};

/// Errors that can occur when constructing a provider from configuration.
#[derive(Debug, Error)]
pub enum ProviderFactoryError {
    /// The configured provider name is not supported.
    #[error("unsupported provider '{provider}'")]
    UnsupportedProvider {
        /// The provider name read from configuration.
        provider: String,
    },
}

/// Create a model provider from a validated configuration.
///
/// The caller is responsible for loading and validating configuration;
/// this function only maps the provider name to its implementation.
///
/// # Errors
///
/// Returns an error if the configured provider name is not supported.
pub fn create_provider(
    config: ModelProviderConfig,
) -> Result<Arc<dyn ModelProvider>, ProviderFactoryError> {
    match config.provider.as_str() {
        "openai" | "openai-compatible" => Ok(Arc::new(OpenAiCompatibleProvider::new(config))),
        "echo" => Ok(Arc::new(EchoProvider {
            chunk_size: config.echo_chunk_size_or_default(),
            delay: config.echo_delay_or_default(),
        })),
        other => Err(ProviderFactoryError::UnsupportedProvider {
            provider: other.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use byte_protocol::{LlmMessage, MessageRole};
    use futures::StreamExt;

    fn dummy_config(provider: &str) -> ModelProviderConfig {
        ModelProviderConfig {
            provider: provider.to_owned(),
            base_url: "https://example.com/v1/".to_owned(),
            api_key: "sk-test".to_owned(),
            model: "gpt-test".to_owned(),
            echo_chunk_size: None,
            echo_delay_ms: None,
            context_budget: None,
        }
    }

    #[test]
    fn creates_openai_compatible_provider() {
        let config = dummy_config("openai");
        assert!(create_provider(config).is_ok());
    }

    #[test]
    fn creates_openai_compatible_provider_with_alias() {
        let config = dummy_config("openai-compatible");
        assert!(create_provider(config).is_ok());
    }

    #[tokio::test]
    async fn creates_echo_provider_that_streams_message() {
        let config = dummy_config("echo");
        let provider = create_provider(config).expect("echo provider should create");
        let messages = vec![LlmMessage::text(MessageRole::Developer, "hello")];

        let events: Vec<_> = provider
            .send_message(messages, vec![])
            .await
            .expect("echo provider should accept message")
            .collect()
            .await;

        assert!(!events.is_empty(), "echo provider should emit events");
    }

    #[test]
    fn rejects_unknown_provider() {
        let config = dummy_config("anthropic");
        match create_provider(config) {
            Ok(_) => panic!("unknown provider should fail"),
            Err(ProviderFactoryError::UnsupportedProvider { provider }) => {
                assert_eq!(provider, "anthropic");
            }
        }
    }
}
