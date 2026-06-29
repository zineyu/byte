use serde::{Deserialize, Serialize};

use crate::MessageRole;

/// A lightweight summary of a Session for listing in the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SessionSummary {
    /// Session identifier.
    pub session_id: String,
    /// Optional workspace path associated with the session.
    pub workspace: Option<String>,
    /// ISO 8601 timestamp of when the session was created.
    pub created_at: String,
}

/// A lightweight summary of a compaction entry for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CompactionSummary {
    /// Compaction entry identifier.
    pub id: String,
    /// Identifier of the parent message this compaction replaces.
    pub parent_id: String,
    /// Human-readable summary text.
    pub summary: String,
}

/// A normalized view of a Session for the React UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SessionView {
    /// Session identifier.
    pub session_id: String,
    /// Optional workspace path associated with the session.
    pub workspace: Option<String>,
    /// Messages in the session, in UI order.
    pub messages: Vec<SessionMessage>,
    /// Compaction entries in the session.
    pub compactions: Vec<CompactionSummary>,
}

/// A message inside a `SessionView`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SessionMessage {
    /// Message identifier.
    pub id: String,
    /// Parent message identifier, if any.
    pub parent_id: Option<String>,
    /// Role of the message sender.
    pub role: MessageRole,
    /// Rendered text content.
    pub content: String,
    /// Identifier of the answered tool call, if this is a tool result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Tool calls requested by the assistant, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<crate::ToolCall>>,
}

/// Persisted content of a session message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessageContent {
    /// Role of the message sender.
    pub role: MessageRole,
    /// Plain text content, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Tool calls requested by the assistant, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<crate::ToolCall>>,
}

impl SessionMessageContent {
    /// Create a plain text message content.
    pub fn text(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            text: Some(content.into()),
            tool_calls: None,
        }
    }

    /// Create assistant content that includes tool calls.
    pub fn with_tool_calls(
        role: MessageRole,
        content: impl Into<String>,
        tool_calls: Vec<crate::ToolCall>,
    ) -> Self {
        Self {
            role,
            text: Some(content.into()),
            tool_calls: Some(tool_calls),
        }
    }
}

/// A single persisted record inside a Session JSONL file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEntry {
    /// Session header record.
    Session {
        /// File format version.
        version: u16,
        /// Session identifier.
        id: String,
        /// Optional workspace path.
        workspace: Option<String>,
        /// ISO 8601 creation timestamp.
        created_at: String,
    },
    /// Message record.
    Message {
        /// Message identifier.
        id: String,
        /// Parent message identifier, if any.
        parent_id: Option<String>,
        /// Message content.
        message: SessionMessageContent,
    },
    /// Tool result record.
    ToolResult {
        /// Result record identifier.
        id: String,
        /// Parent message identifier.
        parent_id: String,
        /// Identifier of the tool call that produced this result.
        tool_call_id: String,
        /// Serialized tool output.
        content: String,
    },
    /// Compaction record.
    Compaction {
        /// Compaction entry identifier.
        id: String,
        /// Parent message identifier.
        parent_id: String,
        /// Human-readable summary text.
        summary: String,
    },
}

/// Parameters for creating a new session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewSessionParams {
    /// Optional workspace path for the new session.
    pub workspace: Option<String>,
}

/// Result returned after creating a new session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewSessionResult {
    /// Identifier of the newly created session.
    pub session_id: String,
}

/// Result returned when listing sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListSessionsResult {
    /// Summaries of the available sessions.
    pub sessions: Vec<SessionSummary>,
}

/// Parameters for loading a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadSessionParams {
    /// Session identifier to load.
    pub session_id: String,
}

/// Result returned after loading a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadSessionResult {
    /// Full view of the loaded session.
    pub session: SessionView,
}

/// Parameters for deleting a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteSessionParams {
    /// Session identifier to delete.
    pub session_id: String,
}

/// Result returned after deleting a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteSessionResult {
    /// Identifier of the deleted session.
    pub session_id: String,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::decode_json_line;

    #[test]
    fn session_header_roundtrips() {
        let entry = SessionEntry::Session {
            version: 1,
            id: "session-1".into(),
            workspace: Some("/home/dev/project".into()),
            created_at: "2026-06-24T12:00:00Z".into(),
        };

        let line = serde_json::to_string(&entry).expect("entry encodes");
        let decoded: SessionEntry = decode_json_line(&line).expect("entry decodes");

        assert_eq!(decoded, entry);
        let entry = SessionEntry::Message {
            id: "msg-1".into(),
            parent_id: Some("msg-0".into()),
            message: SessionMessageContent {
                role: MessageRole::Developer,
                text: Some("hello".into()),
                tool_calls: None,
            },
        };

        let line = serde_json::to_string(&entry).expect("entry encodes");
        let decoded: SessionEntry = decode_json_line(&line).expect("entry decodes");

        assert_eq!(decoded, entry);
    }

    #[test]
    fn session_view_roundtrips() {
        let view = SessionView {
            session_id: "session-1".into(),
            workspace: Some("/home/dev/project".into()),
            messages: vec![SessionMessage {
                id: "msg-1".into(),
                parent_id: None,
                role: MessageRole::Assistant,
                content: "hi".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            compactions: vec![CompactionSummary {
                id: "compact-1".into(),
                parent_id: "msg-1".into(),
                summary: "summary text".into(),
            }],
        };

        let value = serde_json::to_value(&view).expect("view encodes");
        let decoded: SessionView = serde_json::from_value(value).expect("view decodes");

        assert_eq!(decoded, view);
    }

    #[test]
    fn session_summary_roundtrips() {
        let summary = SessionSummary {
            session_id: "session-1".into(),
            workspace: Some("/home/dev/project".into()),
            created_at: "2026-06-24T12:00:00Z".into(),
        };

        let value = serde_json::to_value(&summary).expect("summary encodes");
        let decoded: SessionSummary = serde_json::from_value(value).expect("summary decodes");

        assert_eq!(decoded, summary);
    }

    #[test]
    fn message_content_with_tool_calls_roundtrips() {
        let content = SessionMessageContent::with_tool_calls(
            MessageRole::Assistant,
            String::new(),
            vec![crate::ToolCall {
                id: "call-1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "src/main.rs"}),
            }],
        );

        let line = serde_json::to_string(&content).expect("content encodes");
        let decoded: SessionMessageContent = decode_json_line(&line).expect("content decodes");

        assert_eq!(decoded, content);
    }

    #[test]
    fn tool_result_entry_roundtrips() {
        let entry = SessionEntry::ToolResult {
            id: "tr-1".into(),
            parent_id: "msg-1".into(),
            tool_call_id: "call-1".into(),
            content: "file contents".into(),
        };

        let line = serde_json::to_string(&entry).expect("entry encodes");
        let decoded: SessionEntry = decode_json_line(&line).expect("entry decodes");

        assert_eq!(decoded, entry);
    }
}
