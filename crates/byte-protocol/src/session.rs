use serde::{Deserialize, Serialize};

use crate::MessageRole;

/// A normalized view of a Session for the React UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionView {
    pub session_id: String,
    pub workspace: Option<String>,
    pub messages: Vec<SessionMessage>,
}
/// A message inside a `SessionView`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionMessage {
    pub id: String,
    pub parent_id: Option<String>,
    pub role: MessageRole,
    pub content: String,
}

/// Raw content stored inside a `message` session entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessageContent {
    pub role: MessageRole,
    pub content: String,
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
    pub session_id: String,
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewSessionResult {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadSessionParams {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadSessionResult {
    pub session: SessionView,
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
    }

    #[test]
    fn message_entry_roundtrips() {
        let entry = SessionEntry::Message {
            id: "msg-1".into(),
            parent_id: Some("msg-0".into()),
            message: SessionMessageContent {
                role: MessageRole::Developer,
                content: "hello".into(),
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
            }],
        };

        let value = serde_json::to_value(&view).expect("view encodes");
        let decoded: SessionView = serde_json::from_value(value).expect("view decodes");

        assert_eq!(decoded, view);
    }
}
