use serde::{Deserialize, Serialize};

use crate::MessageRole;

/// A lightweight summary of a Session for listing in the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SessionSummary {
    /// Session identifier.
    pub session_id: String,
    /// Workspace path associated with the session.
    pub workspace: String,
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
    /// Workspace path associated with the session.
    pub workspace: String,
    /// Raw content of the workspace's AGENTS.md instruction file, if found.
    pub workspace_instructions: Option<String>,
    /// Human-readable warning if the workspace's AGENTS.md exists but could not be read.
    pub workspace_instructions_error: Option<String>,
    /// Messages in the session, in UI order.
    pub messages: Vec<Message>,
    /// Compaction entries in the session.
    pub compactions: Vec<CompactionSummary>,
}

/// A persisted message node, also used as the runtime view of a session history node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct Message {
    /// Message identifier.
    pub id: String,
    /// Parent message identifier, if any.
    pub parent_id: Option<String>,
    /// Role of the message sender.
    pub role: MessageRole,
    /// Message body content.
    pub body: MessageBody,
}

/// The body of a [`Message`]: a list of typed blocks.
///
/// Serializes as a JSON array because of `#[serde(transparent)]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(transparent)]
#[ts(export, rename_all = "camelCase")]
pub struct MessageBody(pub Vec<MessageBlock>);

impl MessageBody {
    /// Create a body containing a single text block.
    pub fn text(content: impl Into<String>) -> Self {
        Self(vec![MessageBlock::Text {
            text: content.into(),
        }])
    }
}

/// A single block inside a [`MessageBody`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub enum MessageBlock {
    /// Plain text content.
    Text {
        /// The text value.
        text: String,
    },
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
        /// Workspace path.
        workspace: String,
        /// ISO 8601 creation timestamp.
        created_at: String,
    },
    /// Message record.
    Message(Message),
    /// Tool result record.
    ///
    /// Deprecated in favour of `Message` entries with `role = Tool`; kept for
    /// migration during Slice 1.
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
    ///
    /// Deprecated in favour of `Message` entries with `role = Summary`; kept
    /// for migration during Slice 1.
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
    /// Workspace path for the new session.
    pub workspace: String,
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
            workspace: "/home/dev/project".into(),
            created_at: "2026-06-24T12:00:00Z".into(),
        };

        let line = serde_json::to_string(&entry).expect("entry encodes");
        let decoded: SessionEntry = decode_json_line(&line).expect("entry decodes");

        assert_eq!(decoded, entry);
    }

    #[test]
    fn message_entry_roundtrips() {
        let entry = SessionEntry::Message(Message {
            id: "msg-1".into(),
            parent_id: Some("msg-0".into()),
            role: MessageRole::Developer,
            body: MessageBody::text("hello"),
        });

        let line = serde_json::to_string(&entry).expect("entry encodes");
        let decoded: SessionEntry = decode_json_line(&line).expect("entry decodes");

        assert_eq!(decoded, entry);
    }

    #[test]
    fn message_body_serializes_as_array() {
        let body = MessageBody::text("hello");
        let value = serde_json::to_value(&body).expect("body encodes");
        assert_eq!(
            value,
            serde_json::json!([{ "type": "text", "text": "hello" }])
        );
    }

    #[test]
    fn session_view_roundtrips() {
        let view = SessionView {
            session_id: "session-1".into(),
            workspace: "/home/dev/project".into(),
            workspace_instructions: Some("follow these instructions".into()),
            workspace_instructions_error: None,
            messages: vec![Message {
                id: "msg-1".into(),
                parent_id: None,
                role: MessageRole::Assistant,
                body: MessageBody::text("hi"),
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
            workspace: "/home/dev/project".into(),
            created_at: "2026-06-24T12:00:00Z".into(),
        };

        let value = serde_json::to_value(&summary).expect("summary encodes");
        let decoded: SessionSummary = serde_json::from_value(value).expect("summary decodes");

        assert_eq!(decoded, summary);
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
