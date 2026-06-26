use serde::{Deserialize, Serialize};

use crate::MessageRole;

/// A lightweight summary of a Session for listing in the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SessionSummary {
    pub session_id: String,
    pub workspace: Option<String>,
    pub created_at: String,
}

/// A lightweight summary of a compaction entry for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CompactionSummary {
    pub id: String,
    pub parent_id: String,
    pub summary: String,
}

/// A normalized view of a Session for the React UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SessionView {
    pub session_id: String,
    pub workspace: Option<String>,
    pub messages: Vec<SessionMessage>,
    pub compactions: Vec<CompactionSummary>,
}

/// A message inside a `SessionView`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SessionMessage {
    pub id: String,
    pub parent_id: Option<String>,
    pub role: MessageRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<crate::ToolCall>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessageContent {
    pub role: MessageRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
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
    Session {
        version: u16,
        id: String,
        workspace: Option<String>,
        created_at: String,
    },
    Message {
        id: String,
        parent_id: Option<String>,
        message: SessionMessageContent,
    },
    ToolResult {
        id: String,
        parent_id: String,
        tool_call_id: String,
        content: String,
    },
    Compaction {
        id: String,
        parent_id: String,
        summary: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewSessionParams {
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewSessionResult {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListSessionsResult {
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadSessionParams {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadSessionResult {
    pub session: SessionView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteSessionParams {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteSessionResult {
    pub session_id: String,
}

#[cfg(test)]
mod tests {
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
        let decoded: SessionMessageContent =
            crate::decode_json_line(&line).expect("content decodes");

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
        let decoded: SessionEntry = crate::decode_json_line(&line).expect("entry decodes");

        assert_eq!(decoded, entry);
    }
}
