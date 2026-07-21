use serde::{Deserialize, Serialize};

use crate::ActivatedSkill;

use crate::{MessageRole, ToolCall};

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
    /// Compaction entries present in the session, keyed by their message ID in the view.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compaction_entries: Vec<CompactionEntry>,
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
    /// Identifier of the tool call this message answers, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
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
    /// A tool call requested by the model.
    ToolCall(ToolCall),
}

/// A block-level delta used during streaming to update a single [`MessageBlock`]
/// without replacing the whole [`MessageBody`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub enum BlockDelta {
    /// Incremental plain text update.
    TextDelta {
        /// The text fragment to append.
        delta: String,
    },
    /// Incremental tool-call update (reserved for future streaming).
    ToolCallDelta {
        /// Optional tool-call identifier fragment.
        id: Option<String>,
        /// Optional tool name fragment.
        name: Option<String>,
        /// Optional arguments JSON fragment.
        arguments_delta: Option<String>,
    },
}

impl From<String> for BlockDelta {
    fn from(delta: String) -> Self {
        Self::TextDelta { delta }
    }
}

impl From<&str> for BlockDelta {
    fn from(delta: &str) -> Self {
        Self::TextDelta {
            delta: delta.to_owned(),
        }
    }
}

/// Identifies a contiguous block of active-path messages that a compaction
/// entry summarizes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CompactionRange {
    /// First message in the compacted block.
    pub first_message_id: String,
    /// Last message in the compacted block.
    pub last_message_id: String,
}

/// A durable, persisted summary node that replaces a contiguous block of
/// older active-path messages when constructing LLM context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CompactionEntry {
    /// Stable identifier, unique within the session.
    pub id: String,
    /// Fixed role for LLM context reconstruction. Always `MessageRole::Summary`.
    pub role: MessageRole,
    /// Natural-language summary generated by the model provider.
    pub summary: String,
    /// Identifies the oldest contiguous block of active-path messages summarized.
    pub compacted_range: CompactionRange,
    /// ISO 8601 timestamp of creation.
    pub created_at: String,
    /// The run during which the compaction entry was created.
    pub run_id: String,
}

/// A single persisted record inside a Session JSONL file.
///
/// Marked `#[non_exhaustive]` so new record variants can be added without
/// breaking downstream `match` expressions; readers must ignore unknown
/// variants, matching the forward-compatible decode policy in
/// `byte_session::persistence`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
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
    /// Skill activation record.
    SkillActivated(ActivatedSkill),
    /// Compaction summary record.
    CompactionEntry(CompactionEntry),
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
            tool_call_id: None,
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
            messages: vec![
                Message {
                    id: "msg-1".into(),
                    parent_id: None,
                    role: MessageRole::Assistant,
                    tool_call_id: None,
                    body: MessageBody::text("hi"),
                },
                Message {
                    id: "msg-2".into(),
                    parent_id: Some("msg-1".into()),
                    role: MessageRole::Summary,
                    tool_call_id: None,
                    body: MessageBody::text("summary text"),
                },
            ],
            compaction_entries: vec![],
        };

        let value = serde_json::to_value(&view).expect("view encodes");
        let decoded: SessionView = serde_json::from_value(value).expect("view decodes");

        assert_eq!(decoded, view);
    }

    #[test]
    fn compaction_entry_roundtrips() {
        let entry = CompactionEntry {
            id: "ce-1".into(),
            role: MessageRole::Summary,
            summary: "the developer asked for tests".into(),
            compacted_range: CompactionRange {
                first_message_id: "msg-1".into(),
                last_message_id: "msg-3".into(),
            },
            created_at: "2026-07-16T10:00:00Z".into(),
            run_id: "run-1".into(),
        };

        let line = serde_json::to_string(&SessionEntry::CompactionEntry(entry.clone()))
            .expect("entry encodes");
        let decoded: SessionEntry = decode_json_line(&line).expect("entry decodes");

        assert_eq!(
            decoded,
            SessionEntry::CompactionEntry(entry),
            "compaction entry should roundtrip"
        );
    }

    #[test]
    fn compaction_entry_serializes_expected_shape() {
        let entry = CompactionEntry {
            id: "ce-1".into(),
            role: MessageRole::Summary,
            summary: "the developer asked for tests".into(),
            compacted_range: CompactionRange {
                first_message_id: "msg-1".into(),
                last_message_id: "msg-3".into(),
            },
            created_at: "2026-07-16T10:00:00Z".into(),
            run_id: "run-1".into(),
        };

        let value =
            serde_json::to_value(SessionEntry::CompactionEntry(entry)).expect("entry encodes");
        assert_eq!(
            value,
            serde_json::json!({
                "type": "compaction_entry",
                "id": "ce-1",
                "role": "summary",
                "summary": "the developer asked for tests",
                "compactedRange": {
                    "firstMessageId": "msg-1",
                    "lastMessageId": "msg-3"
                },
                "createdAt": "2026-07-16T10:00:00Z",
                "runId": "run-1"
            })
        );
    }
}
