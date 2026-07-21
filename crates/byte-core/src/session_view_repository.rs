//! Session view repository for reconstructing a [`SessionView`] from persisted entries.
//!
//! The repository separates "how session history is persisted" from "how it is
//! presented to the rest of the runtime". It reads raw [`SessionEntry`] records
//! from a [`SessionStore`] and rebuilds the active message path, including
//! workspace instructions from `AGENTS.md`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use byte_protocol::{
    CompactionEntry, Message, MessageBody, MessageRole, SessionEntry, SessionView,
};
use byte_session::SessionStore;

/// Errors that can occur when reconstructing a [`SessionView`].
#[derive(Debug, thiserror::Error)]
pub enum SessionViewError {
    /// The session entries do not contain a header for the requested session.
    #[error("session {session_id} has no header")]
    MissingHeader {
        /// Identifier of the session whose header is missing.
        session_id: String,
    },
    /// The session entries form a broken parent chain.
    #[error("session {session_id} has a broken parent chain")]
    BrokenChain {
        /// Identifier of the session with the broken chain.
        session_id: String,
    },
    /// An error originating from the session store.
    #[error(transparent)]
    Store(#[from] byte_session::SessionError),
}

/// Reconstructs [`SessionView`] instances from persisted session entries.
#[derive(Debug)]
pub struct SessionViewRepository {
    /// Backing persistent session store used to read raw entries.
    store: Arc<SessionStore>,
}

impl SessionViewRepository {
    /// Create a new repository backed by the given session store.
    #[must_use]
    pub const fn new(store: Arc<SessionStore>) -> Self {
        Self { store }
    }

    /// Load a normalized [`SessionView`] by rebuilding the chronological active
    /// path, replacing each compacted block with its summary entry.
    ///
    /// # Errors
    ///
    /// Returns an error if the session cannot be read, the header is missing,
    /// or the entries cannot be reconstructed.
    pub async fn load_session(&self, session_id: &str) -> Result<SessionView, SessionViewError> {
        let entries = self.store.read_entries(session_id).await?;
        let workspace = extract_workspace(session_id, &entries)?;
        let (workspace_instructions, workspace_instructions_error) =
            read_workspace_instructions(&workspace).await;
        Self::build_view(
            entries,
            session_id,
            workspace_instructions,
            workspace_instructions_error,
        )
    }

    /// Build a [`SessionView`] from raw entries without touching the file system.
    ///
    /// This function is the pure core of [`Self::load_session`] and is intended
    /// for unit testing with in-memory entry lists.
    ///
    /// # Errors
    ///
    /// Returns an error if the header is missing or the entries cannot be
    /// reconstructed.
    #[allow(clippy::needless_pass_by_value)]
    pub fn build_view(
        entries: Vec<SessionEntry>,
        session_id: &str,
        workspace_instructions: Option<String>,
        workspace_instructions_error: Option<String>,
    ) -> Result<SessionView, SessionViewError> {
        let workspace = extract_workspace(session_id, &entries)?;

        // Collect all messages in chronological order and map each message ID to
        // its position in that order.
        let mut ordered_messages: Vec<Message> = Vec::new();
        let mut message_ids: Vec<String> = Vec::new();
        let mut message_positions: HashMap<String, usize> = HashMap::new();
        let mut compaction_entries: Vec<CompactionEntry> = Vec::new();

        for entry in &entries {
            match entry {
                SessionEntry::Message(message) => {
                    let position = ordered_messages.len();
                    let _ = message_positions.insert(message.id.clone(), position);
                    ordered_messages.push(message.clone());
                    message_ids.push(message.id.clone());
                }
                SessionEntry::CompactionEntry(compaction) => {
                    compaction_entries.push(compaction.clone());
                }
                // Session headers, skill activations, and unknown variants from
                // newer protocol versions do not contribute to the message view.
                _ => {}
            }
        }

        // Mark every message that falls inside a compaction range as skipped.
        let mut compacted_ids: HashSet<String> = HashSet::new();
        for compaction in &compaction_entries {
            mark_compacted_messages(
                compaction,
                &message_positions,
                &message_ids,
                &mut compacted_ids,
            );
        }

        // Walk the chronological message list and build the active view. When a
        // compaction range starts, replace its entire block with a single
        // summary message whose parent is the message before the block.
        let mut messages: Vec<Message> = Vec::new();
        let mut index = 0;
        while index < ordered_messages.len() {
            if let Some(compaction) = compaction_entries.iter().find(|compaction| {
                let first = message_positions.get(&compaction.compacted_range.first_message_id);
                let last = message_positions.get(&compaction.compacted_range.last_message_id);
                match (first, last) {
                    (Some(&a), Some(&b)) => a.min(b) == index,
                    _ => false,
                }
            }) {
                let first_position = message_positions
                    [&compaction.compacted_range.first_message_id]
                    .min(message_positions[&compaction.compacted_range.last_message_id]);
                let last_position = message_positions[&compaction.compacted_range.first_message_id]
                    .max(message_positions[&compaction.compacted_range.last_message_id]);
                let parent_id = ordered_messages[first_position].parent_id.clone();

                messages.push(Message {
                    id: compaction.id.clone(),
                    parent_id,
                    role: MessageRole::Summary,
                    tool_call_id: None,
                    body: MessageBody::text(&compaction.summary),
                });

                index = last_position + 1;
            } else if compacted_ids.contains(&ordered_messages[index].id) {
                // A message that is compacted by an entry whose range starts
                // earlier should have been skipped as part of that range; this
                // guards against malformed overlapping ranges.
                index += 1;
            } else {
                messages.push(ordered_messages[index].clone());
                index += 1;
            }
        }

        Ok(SessionView {
            session_id: session_id.to_owned(),
            workspace: workspace.clone(),
            workspace_instructions,
            workspace_instructions_error,
            messages,
            compaction_entries,
        })
    }
}

/// Extracts the workspace path from the session header.
fn extract_workspace(
    session_id: &str,
    entries: &[SessionEntry],
) -> Result<String, SessionViewError> {
    entries
        .iter()
        .find_map(|entry| match entry {
            SessionEntry::Session { id, workspace, .. } if id == session_id => {
                Some(workspace.clone())
            }
            _ => None,
        })
        .ok_or_else(|| SessionViewError::MissingHeader {
            session_id: session_id.to_owned(),
        })
}

/// Mark all message identifiers within a compaction range as compacted.
fn mark_compacted_messages(
    compaction: &CompactionEntry,
    positions: &HashMap<String, usize>,
    ordered_ids: &[String],
    compacted_ids: &mut HashSet<String>,
) {
    let first = positions.get(&compaction.compacted_range.first_message_id);
    let last = positions.get(&compaction.compacted_range.last_message_id);
    let (first_index, last_index) = match (first, last) {
        (Some(&a), Some(&b)) => (a.min(b), a.max(b)),
        _ => return,
    };

    if last_index >= ordered_ids.len() {
        return;
    }

    for id in &ordered_ids[first_index..=last_index] {
        let _ = compacted_ids.insert(id.clone());
    }
}

/// Reads the workspace instruction file (`AGENTS.md`) from `workspace`.
///
/// Returns `(Some(content), None)` when the file is readable, `(None, None)`
/// when it does not exist, and `(None, Some(error))` when it exists but cannot
/// be read.
async fn read_workspace_instructions(workspace: &str) -> (Option<String>, Option<String>) {
    let path = Path::new(workspace).join("AGENTS.md");
    match tokio::fs::metadata(&path).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, None),
        Err(error) => (
            None,
            Some(format!("无法读取 Workspace Instructions: {error}")),
        ),
        Ok(metadata) => {
            if !metadata.is_file() {
                return (None, Some("AGENTS.md 不是文件".to_owned()));
            }
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => (Some(content), None),
                Err(error) => (
                    None,
                    Some(format!("无法读取 Workspace Instructions: {error}")),
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use byte_protocol::{CompactionEntry, CompactionRange, MessageBody, MessageRole};
    use byte_session::SessionStore;
    use std::sync::Arc;

    fn message_text(message: &Message) -> &str {
        match &message.body.0[..] {
            [byte_protocol::MessageBlock::Text { text }] => text.as_str(),
            _ => "",
        }
    }

    #[test]
    fn build_view_includes_compaction_entry_as_summary() {
        let m1 = Message {
            id: "msg-1".into(),
            parent_id: None,
            role: MessageRole::Developer,
            tool_call_id: None,
            body: MessageBody::text("hello"),
        };
        let m2 = Message {
            id: "msg-2".into(),
            parent_id: Some("msg-1".into()),
            role: MessageRole::Assistant,
            tool_call_id: None,
            body: MessageBody::text("hi"),
        };
        let m3 = Message {
            id: "msg-3".into(),
            parent_id: Some("msg-2".into()),
            role: MessageRole::Developer,
            tool_call_id: None,
            body: MessageBody::text("next"),
        };
        let ce = CompactionEntry {
            id: "ce-1".into(),
            role: MessageRole::Summary,
            summary: "greeting exchange".into(),
            compacted_range: CompactionRange {
                first_message_id: "msg-1".into(),
                last_message_id: "msg-2".into(),
            },
            created_at: "2026-01-01T00:00:00Z".into(),
            run_id: "run-1".into(),
        };

        let entries = vec![
            SessionEntry::Session {
                version: byte_protocol::PROTOCOL_VERSION,
                id: "session-1".into(),
                workspace: "/workspace".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
            },
            SessionEntry::Message(m1),
            SessionEntry::Message(m2),
            SessionEntry::CompactionEntry(ce),
            SessionEntry::Message(m3),
        ];

        let view = SessionViewRepository::build_view(entries, "session-1", None, None)
            .expect("build view");

        assert_eq!(view.messages.len(), 2);
        assert!(
            view.messages
                .iter()
                .any(|m| m.role == MessageRole::Summary && m.id == "ce-1")
        );
        let summary = view
            .messages
            .iter()
            .find(|m| m.role == MessageRole::Summary)
            .expect("summary present");
        assert_eq!(summary.body, MessageBody::text("greeting exchange"));
        assert_eq!(summary.parent_id, None);

        let last = view
            .messages
            .iter()
            .find(|m| m.id == "msg-3")
            .expect("msg-3 present");
        assert_eq!(last.parent_id, Some("msg-2".into()));
    }

    #[test]
    fn build_view_reconstructs_active_path() {
        let entries = vec![
            SessionEntry::Session {
                version: byte_protocol::PROTOCOL_VERSION,
                id: "session-1".into(),
                workspace: "/workspace".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
            },
            SessionEntry::Message(Message {
                id: "msg-1".into(),
                parent_id: None,
                role: MessageRole::Developer,
                tool_call_id: None,
                body: MessageBody::text("hello"),
            }),
            SessionEntry::Message(Message {
                id: "msg-2".into(),
                parent_id: Some("msg-1".into()),
                role: MessageRole::Assistant,
                tool_call_id: None,
                body: MessageBody::text("hi"),
            }),
        ];

        let view = SessionViewRepository::build_view(entries, "session-1", None, None)
            .expect("build view");

        assert_eq!(view.session_id, "session-1");
        assert_eq!(view.workspace, "/workspace");
        assert_eq!(view.messages.len(), 2);
        assert_eq!(view.messages[0].id, "msg-1");
        assert_eq!(view.messages[0].role, MessageRole::Developer);
        assert_eq!(message_text(&view.messages[0]), "hello");
        assert_eq!(view.messages[1].id, "msg-2");
        assert_eq!(view.messages[1].parent_id, Some("msg-1".into()));
        assert_eq!(view.messages[1].role, MessageRole::Assistant);
        assert_eq!(message_text(&view.messages[1]), "hi");
    }

    #[test]
    fn build_view_preserves_tool_message() {
        let entries = vec![
            SessionEntry::Session {
                version: byte_protocol::PROTOCOL_VERSION,
                id: "session-1".into(),
                workspace: "/workspace".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
            },
            SessionEntry::Message(Message {
                id: "msg-1".into(),
                parent_id: None,
                role: MessageRole::Developer,
                tool_call_id: None,
                body: MessageBody::text("read main.rs"),
            }),
            SessionEntry::Message(Message {
                id: "msg-2".into(),
                parent_id: Some("msg-1".into()),
                role: MessageRole::Assistant,
                tool_call_id: None,
                body: MessageBody::text(""),
            }),
            SessionEntry::Message(Message {
                id: "msg-3".into(),
                parent_id: Some("msg-2".into()),
                role: MessageRole::Tool,
                tool_call_id: Some("tc-1".into()),
                body: MessageBody::text("fn main() {}"),
            }),
        ];

        let view = SessionViewRepository::build_view(entries, "session-1", None, None)
            .expect("build view");

        assert_eq!(view.messages.len(), 3);
        assert_eq!(view.messages[0].role, MessageRole::Developer);
        assert_eq!(view.messages[1].role, MessageRole::Assistant);
        assert_eq!(view.messages[2].role, MessageRole::Tool);
        assert_eq!(view.messages[2].tool_call_id, Some("tc-1".into()));
        assert_eq!(message_text(&view.messages[2]), "fn main() {}");
    }

    #[test]
    fn build_view_reconstructs_summary_on_active_path() {
        let entries = vec![
            SessionEntry::Session {
                version: byte_protocol::PROTOCOL_VERSION,
                id: "session-1".into(),
                workspace: "/workspace".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
            },
            SessionEntry::Message(Message {
                id: "msg-1".into(),
                parent_id: None,
                role: MessageRole::Developer,
                tool_call_id: None,
                body: MessageBody::text("hello"),
            }),
            SessionEntry::Message(Message {
                id: "msg-2".into(),
                parent_id: Some("msg-1".into()),
                role: MessageRole::Assistant,
                tool_call_id: None,
                body: MessageBody::text("hi"),
            }),
            SessionEntry::Message(Message {
                id: "summary-1".into(),
                parent_id: Some("msg-2".into()),
                role: MessageRole::Summary,
                tool_call_id: None,
                body: MessageBody::text("assistant message compacted"),
            }),
        ];

        let view = SessionViewRepository::build_view(entries, "session-1", None, None)
            .expect("build view");

        assert_eq!(view.messages.len(), 3);
        let summary_message = view
            .messages
            .iter()
            .find(|m| m.id == "summary-1")
            .expect("summary message present");
        assert_eq!(summary_message.parent_id, Some("msg-2".into()));
        assert_eq!(summary_message.role, MessageRole::Summary);
        assert_eq!(
            summary_message.body,
            MessageBody::text("assistant message compacted")
        );
    }

    #[test]
    fn build_view_missing_header_returns_error() {
        let entries = vec![SessionEntry::Message(Message {
            id: "msg-1".into(),
            parent_id: None,
            role: MessageRole::Developer,
            tool_call_id: None,
            body: MessageBody::text("hello"),
        })];

        let err = SessionViewRepository::build_view(entries, "session-1", None, None)
            .expect_err("missing header should fail");

        assert!(
            matches!(err, SessionViewError::MissingHeader { session_id } if session_id == "session-1")
        );
    }

    #[test]
    fn build_view_with_broken_parent_chain_still_includes_messages() {
        let entries = vec![
            SessionEntry::Session {
                version: byte_protocol::PROTOCOL_VERSION,
                id: "session-1".into(),
                workspace: "/workspace".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
            },
            SessionEntry::Message(Message {
                id: "msg-1".into(),
                parent_id: Some("missing".into()),
                role: MessageRole::Developer,
                tool_call_id: None,
                body: MessageBody::text("hello"),
            }),
        ];

        let view = SessionViewRepository::build_view(entries, "session-1", None, None)
            .expect("build view");

        assert_eq!(view.messages.len(), 1);
        assert_eq!(view.messages[0].id, "msg-1");
    }

    #[test]
    fn build_view_includes_workspace_instructions() {
        let entries = vec![SessionEntry::Session {
            version: byte_protocol::PROTOCOL_VERSION,
            id: "session-1".into(),
            workspace: "/workspace".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        }];

        let view = SessionViewRepository::build_view(
            entries,
            "session-1",
            Some("Always use Rust.\n".into()),
            None,
        )
        .expect("build view");

        assert_eq!(
            view.workspace_instructions.as_deref(),
            Some("Always use Rust.\n")
        );
        assert_eq!(view.workspace_instructions_error, None);
    }

    #[tokio::test]
    async fn build_view_original_messages_remain_after_compaction() {
        let store = Arc::new(
            SessionStore::new(tempfile::tempdir().unwrap().path().to_path_buf())
                .expect("store creates"),
        );
        store
            .new_session("session-1", "/workspace")
            .await
            .expect("new session");

        let m1 = store
            .append_message(
                "session-1",
                None,
                None,
                MessageRole::Developer,
                MessageBody::text("hello"),
                None,
            )
            .await
            .expect("append m1");
        let m2 = store
            .append_message(
                "session-1",
                None,
                Some(&m1),
                MessageRole::Assistant,
                MessageBody::text("hi"),
                None,
            )
            .await
            .expect("append m2");
        let m3 = store
            .append_message(
                "session-1",
                None,
                Some(&m2),
                MessageRole::Developer,
                MessageBody::text("next"),
                None,
            )
            .await
            .expect("append m3");
        let m4 = store
            .append_message(
                "session-1",
                None,
                Some(&m3),
                MessageRole::Assistant,
                MessageBody::text("answer"),
                None,
            )
            .await
            .expect("append m4");

        let _ = store
            .append_compaction_entry(
                "session-1",
                CompactionEntry {
                    id: "ce-1".into(),
                    role: MessageRole::Summary,
                    summary: "greeting exchange".into(),
                    compacted_range: CompactionRange {
                        first_message_id: m1.clone(),
                        last_message_id: m2.clone(),
                    },
                    created_at: "2026-01-01T00:00:00Z".into(),
                    run_id: "run-1".into(),
                },
            )
            .await
            .expect("append compaction");

        let persisted = store.read_entries("session-1").await.expect("read entries");
        let persisted_message_ids: Vec<String> = persisted
            .iter()
            .filter_map(|entry| match entry {
                SessionEntry::Message(message) => Some(message.id.clone()),
                _ => None,
            })
            .collect();
        assert!(persisted_message_ids.contains(&m1));
        assert!(persisted_message_ids.contains(&m2));
        assert!(persisted_message_ids.contains(&m3));
        assert!(persisted_message_ids.contains(&m4));

        let repo = SessionViewRepository::new(store);
        let view = repo.load_session("session-1").await.expect("load session");
        assert_eq!(view.messages.len(), 3, "summary + m3 + m4");
        assert_eq!(view.messages[0].id, "ce-1");
        assert_eq!(view.messages[0].role, MessageRole::Summary);
        assert_eq!(view.messages[1].id, m3);
        assert_eq!(view.messages[2].id, m4);
        assert_eq!(view.messages[2].parent_id, Some(m3));
    }

    #[test]
    fn build_view_reloads_session_with_multiple_compaction_entries() {
        let m1 = Message {
            id: "msg-1".into(),
            parent_id: None,
            role: MessageRole::Developer,
            tool_call_id: None,
            body: MessageBody::text("first"),
        };
        let m2 = Message {
            id: "msg-2".into(),
            parent_id: Some("msg-1".into()),
            role: MessageRole::Assistant,
            tool_call_id: None,
            body: MessageBody::text("second"),
        };
        let m3 = Message {
            id: "msg-3".into(),
            parent_id: Some("msg-2".into()),
            role: MessageRole::Developer,
            tool_call_id: None,
            body: MessageBody::text("third"),
        };
        let m4 = Message {
            id: "msg-4".into(),
            parent_id: Some("msg-3".into()),
            role: MessageRole::Assistant,
            tool_call_id: None,
            body: MessageBody::text("fourth"),
        };
        let m5 = Message {
            id: "msg-5".into(),
            parent_id: Some("msg-4".into()),
            role: MessageRole::Developer,
            tool_call_id: None,
            body: MessageBody::text("recent"),
        };
        let ce1 = CompactionEntry {
            id: "ce-1".into(),
            role: MessageRole::Summary,
            summary: "first block".into(),
            compacted_range: CompactionRange {
                first_message_id: "msg-1".into(),
                last_message_id: "msg-2".into(),
            },
            created_at: "2026-01-01T00:00:00Z".into(),
            run_id: "run-1".into(),
        };
        let ce2 = CompactionEntry {
            id: "ce-2".into(),
            role: MessageRole::Summary,
            summary: "second block".into(),
            compacted_range: CompactionRange {
                first_message_id: "msg-3".into(),
                last_message_id: "msg-4".into(),
            },
            created_at: "2026-01-01T00:00:00Z".into(),
            run_id: "run-1".into(),
        };

        let entries = vec![
            SessionEntry::Session {
                version: byte_protocol::PROTOCOL_VERSION,
                id: "session-1".into(),
                workspace: "/workspace".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
            },
            SessionEntry::Message(m1),
            SessionEntry::Message(m2),
            SessionEntry::CompactionEntry(ce1),
            SessionEntry::Message(m3),
            SessionEntry::Message(m4),
            SessionEntry::CompactionEntry(ce2),
            SessionEntry::Message(m5),
        ];

        let view = SessionViewRepository::build_view(entries, "session-1", None, None)
            .expect("build view");

        assert_eq!(view.messages.len(), 3, "two summaries + recent message");
        assert_eq!(view.messages[0].id, "ce-1");
        assert_eq!(view.messages[0].role, MessageRole::Summary);
        assert_eq!(message_text(&view.messages[0]), "first block");
        assert_eq!(view.messages[1].id, "ce-2");
        assert_eq!(view.messages[1].role, MessageRole::Summary);
        assert_eq!(message_text(&view.messages[1]), "second block");
        assert_eq!(view.messages[2].id, "msg-5");
        assert_eq!(view.messages[2].role, MessageRole::Developer);
        assert_eq!(message_text(&view.messages[2]), "recent");
    }

    #[test]
    fn build_view_backward_compatible_without_compaction_entries() {
        let entries = vec![
            SessionEntry::Session {
                version: byte_protocol::PROTOCOL_VERSION,
                id: "session-1".into(),
                workspace: "/workspace".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
            },
            SessionEntry::Message(Message {
                id: "msg-1".into(),
                parent_id: None,
                role: MessageRole::Developer,
                tool_call_id: None,
                body: MessageBody::text("hello"),
            }),
            SessionEntry::Message(Message {
                id: "msg-2".into(),
                parent_id: Some("msg-1".into()),
                role: MessageRole::Assistant,
                tool_call_id: None,
                body: MessageBody::text("hi"),
            }),
            SessionEntry::SkillActivated(byte_protocol::ActivatedSkill {
                name: "read_file".into(),
                content: String::new(),
            }),
        ];

        let view = SessionViewRepository::build_view(entries, "session-1", None, None)
            .expect("build view");

        assert_eq!(view.messages.len(), 2);
        assert_eq!(view.messages[0].id, "msg-1");
        assert_eq!(view.messages[1].id, "msg-2");
    }

    #[tokio::test]
    async fn load_session_reads_agents_md() {
        let workspace = tempfile::tempdir().expect("temp workspace");
        let workspace_path = workspace.path().to_str().unwrap();
        let agents_path = workspace.path().join("AGENTS.md");
        tokio::fs::write(&agents_path, "Always use Rust.\n")
            .await
            .expect("write AGENTS.md");

        let store = Arc::new(
            SessionStore::new(tempfile::tempdir().unwrap().path().to_path_buf())
                .expect("store creates"),
        );
        store
            .new_session("session-1", workspace_path)
            .await
            .expect("new session");
        let repo = SessionViewRepository::new(store);

        let view = repo.load_session("session-1").await.expect("load session");

        assert_eq!(view.workspace, workspace_path);
        assert_eq!(
            view.workspace_instructions.as_deref(),
            Some("Always use Rust.\n")
        );
        assert_eq!(view.workspace_instructions_error, None);
    }

    #[tokio::test]
    async fn load_session_reports_error_when_agents_md_is_unreadable() {
        let workspace = tempfile::tempdir().expect("temp workspace");
        let workspace_path = workspace.path().to_str().unwrap();
        let agents_path = workspace.path().join("AGENTS.md");
        tokio::fs::create_dir(&agents_path)
            .await
            .expect("create AGENTS.md directory");

        let store = Arc::new(
            SessionStore::new(tempfile::tempdir().unwrap().path().to_path_buf())
                .expect("store creates"),
        );
        store
            .new_session("session-1", workspace_path)
            .await
            .expect("new session");
        let repo = SessionViewRepository::new(store);

        let view = repo.load_session("session-1").await.expect("load session");

        assert_eq!(view.workspace_instructions, None);
        assert!(view.workspace_instructions_error.is_some());
    }

    #[tokio::test]
    async fn load_session_missing_session_returns_store_error() {
        let store = Arc::new(
            SessionStore::new(tempfile::tempdir().unwrap().path().to_path_buf())
                .expect("store creates"),
        );
        let repo = SessionViewRepository::new(store);

        let err = repo
            .load_session("missing")
            .await
            .expect_err("missing session should fail");

        assert!(
            matches!(err, SessionViewError::Store(byte_session::SessionError::NotFound(id)) if id == "missing")
        );
    }
}
