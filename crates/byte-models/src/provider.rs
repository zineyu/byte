use std::pin::Pin;

use async_trait::async_trait;
use byte_protocol::{MessageRole, RunMessage};
use futures::Stream;

pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send>>;

/// An event emitted by a model provider during a streaming generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEvent {
    /// A new assistant message has begun.
    MessageStarted { message_id: String },
    /// Additional text content for the active assistant message.
    TextDelta { message_id: String, delta: String },
    /// The active assistant message is complete.
    /// When the model requested tool calls, they are included here.
    MessageCompleted {
        message_id: String,
        tool_calls: Option<Vec<byte_protocol::ToolCall>>,
    },
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

#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Send a single conversation turn to the model and receive incremental
    /// assistant events. The returned stream must yield exactly one
    /// `MessageStarted`/`MessageCompleted` pair, with zero or more `TextDelta`
    /// events in between. When `tools` is non-empty, the model may request tool
    /// calls via `MessageCompleted::tool_calls`.
    async fn send_message(
        &self,
        messages: Vec<RunMessage>,
        tools: Vec<byte_protocol::ToolDefinition>,
    ) -> Result<ProviderStream, ProviderError>;
}
/// A mock provider that echoes the final developer message back in chunks.
#[derive(Debug, Clone, Copy)]
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
        tools: Vec<byte_protocol::ToolDefinition>,
    ) -> Result<ProviderStream, ProviderError> {
        let has_read_file = tools.iter().any(|tool| tool.name == "read_file");
        let has_write_file = tools.iter().any(|tool| tool.name == "write_file");
        let has_apply_patch = tools.iter().any(|tool| tool.name == "apply_patch");
        let last_was_tool = messages
            .last()
            .is_some_and(|message| message.role == MessageRole::Tool);

        if !last_was_tool {
            let last_user_message = messages
                .iter()
                .rev()
                .find(|message| message.role == MessageRole::Developer)
                .map_or("", |message| message.content.as_str());

            if has_write_file && is_write_file_intent(last_user_message) {
                let tool_call = byte_protocol::ToolCall {
                    id: "echo-call-1".into(),
                    name: "write_file".into(),
                    arguments: serde_json::json!({
                        "path": "hello.txt",
                        "content": "Hello, world!"
                    }),
                };
                return Ok(tool_call_stream(tool_call, self.delay));
            }

            if has_apply_patch && is_apply_patch_intent(last_user_message) {
                let tool_call = byte_protocol::ToolCall {
                    id: "echo-call-1".into(),
                    name: "apply_patch".into(),
                    arguments: serde_json::json!({
                        "path": "src/lib.rs",
                        "patch": [
                            {"search": "fn old_one() {}", "replace": "fn new_one() {}"},
                            {"search": "fn old_two() {}", "replace": "fn new_two() {}"}
                        ]
                    }),
                };
                return Ok(tool_call_stream(tool_call, self.delay));
            }

            if has_read_file {
                let tool_call = byte_protocol::ToolCall {
                    id: "echo-call-1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "main.rs"}),
                };
                return Ok(tool_call_stream(tool_call, self.delay));
            }
        }

        let last = messages
            .into_iter()
            .filter(|m| m.role == MessageRole::Developer)
            .map(|m| m.content)
            .next_back()
            .unwrap_or_default();

        let content = format!("Echo: {last}");
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
            yield ProviderEvent::MessageCompleted { message_id, tool_calls: None };
        };

        Ok(Box::pin(stream))
    }
}

fn is_write_file_intent(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    if has_negation(&lower) {
        return false;
    }
    message.contains("创建") || message.contains("写入") || lower.contains("write")
}

fn is_apply_patch_intent(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    if has_negation(&lower) {
        return false;
    }
    message.contains("替换") || lower.contains("patch") || lower.contains("apply_patch")
}

fn has_negation(message: &str) -> bool {
    // Very small guard against obvious negated commands. This is only a test
    // mock, so exhaustive NLP is out of scope. Match whole English words and
    // common Chinese negation phrases to avoid false positives like "notify",
    // "note", or "创建一个不小的文件".
    has_english_negation(message) || has_chinese_negation(message)
}

fn has_english_negation(message: &str) -> bool {
    const NEGATIONS: &[&str] = &["no", "not", "never", "don't", "dont", "cannot", "cant"];
    message
        .split(|c: char| !c.is_alphabetic() && c != '\'')
        .map(str::to_ascii_lowercase)
        .any(|word| NEGATIONS.contains(&word.as_str()))
}

fn has_chinese_negation(message: &str) -> bool {
    const NEGATIONS: &[&str] = &[
        "不要",
        "别",
        "不想",
        "不用",
        "不需要",
        "不能",
        "不会",
        "没有",
        "请勿",
    ];
    NEGATIONS.iter().any(|neg| message.contains(neg))
}

fn tool_call_stream(
    tool_call: byte_protocol::ToolCall,
    delay: std::time::Duration,
) -> ProviderStream {
    let message_id = uuid::Uuid::new_v4().to_string();
    Box::pin(async_stream::try_stream! {
        yield ProviderEvent::MessageStarted { message_id: message_id.clone() };
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        yield ProviderEvent::MessageCompleted {
            message_id,
            tool_calls: Some(vec![tool_call]),
        };
    })
}
#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn echo_provider_streams_developer_message_back() {
        let provider = EchoProvider {
            chunk_size: 3,
            ..Default::default()
        };
        let messages = vec![RunMessage::text(MessageRole::Developer, "hello")];

        let stream = provider
            .send_message(messages, vec![])
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
            Ok(ProviderEvent::MessageCompleted {
                tool_calls: None,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn echo_provider_handles_empty_messages() {
        let provider = EchoProvider {
            chunk_size: 5,
            ..Default::default()
        };
        let stream = provider
            .send_message(vec![], vec![])
            .await
            .expect("stream starts");
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
            Ok(ProviderEvent::MessageCompleted {
                tool_calls: None,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn echo_provider_returns_mock_read_file_tool_call() {
        let provider = EchoProvider::default();
        let messages = vec![RunMessage::text(MessageRole::Developer, "读一下 main.rs")];
        let tools = vec![byte_protocol::ToolDefinition {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: serde_json::json!({"type": "object"}),
        }];

        let stream = provider
            .send_message(messages, tools)
            .await
            .expect("stream starts");
        let events: Vec<_> = stream.collect().await;

        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            Ok(ProviderEvent::MessageStarted { .. })
        ));
        assert!(
            matches!(
                &events[1],
                Ok(ProviderEvent::MessageCompleted { tool_calls: Some(calls), .. })
                if calls.len() == 1 && calls[0].name == "read_file"
            ),
            "expected a read_file tool call, got {:?}",
            events[1]
        );
    }

    #[tokio::test]
    async fn echo_provider_does_not_loop_tool_calls() {
        let provider = EchoProvider::default();
        let messages = vec![
            RunMessage::text(MessageRole::Developer, "读一下 main.rs"),
            RunMessage::tool_result("echo-call-1", "file contents"),
        ];
        let tools = vec![byte_protocol::ToolDefinition {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: serde_json::json!({"type": "object"}),
        }];

        let stream = provider
            .send_message(messages, tools)
            .await
            .expect("stream starts");
        let events: Vec<_> = stream.collect().await;

        assert!(
            matches!(
                &events.last().unwrap(),
                Ok(ProviderEvent::MessageCompleted {
                    tool_calls: None,
                    ..
                })
            ),
            "should echo after receiving a tool result"
        );
    }

    #[test]
    fn detects_write_file_intent() {
        assert!(is_write_file_intent("创建一个文件"));
        assert!(is_write_file_intent("write hello.txt"));
    }

    #[test]
    fn rejects_negated_write_file_intent() {
        assert!(!is_write_file_intent("不要创建文件"));
        assert!(!is_write_file_intent("don't write the file"));
        assert!(!is_write_file_intent("do not write the file"));
    }

    #[test]
    fn accepts_write_file_intent_with_false_positive_negation_words() {
        assert!(!has_english_negation("notify me and write a file"));
        assert!(is_write_file_intent("notify me and write a file"));
        assert!(is_write_file_intent("note this and write it down"));
        assert!(is_write_file_intent("now write the file"));
        assert!(is_write_file_intent("创建一个不小的文件"));
    }

    #[test]
    fn detects_apply_patch_intent() {
        assert!(is_apply_patch_intent("替换这段代码"));
        assert!(is_apply_patch_intent("apply the patch"));
    }

    #[test]
    fn rejects_negated_apply_patch_intent() {
        assert!(!is_apply_patch_intent("不要替换"));
        assert!(!is_apply_patch_intent("do not patch"));
    }

    #[test]
    fn accepts_apply_patch_intent_with_false_positive_negation_words() {
        assert!(is_apply_patch_intent("notify me and apply the patch"));
        assert!(is_apply_patch_intent("note this and patch it"));
        assert!(is_apply_patch_intent("apply the patch now"));
        assert!(is_apply_patch_intent("替换不小的这段代码"));
    }
}
