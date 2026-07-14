//! Session view repository for reconstructing a [`SessionView`] from persisted entries.
//!
//! The repository separates "how session history is persisted" from "how it is
//! presented to the rest of the runtime". It reads raw [`SessionEntry`] records
//! from a [`SessionStore`] and rebuilds the active message path, including
//! workspace instructions from `AGENTS.md`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use byte_protocol::{Message, SessionEntry, SessionView};
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

    /// Load a normalized [`SessionView`] by following the active path from the
    /// most recent message back to the root.
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
    /// Returns an error if the header is missing or the active parent chain is
    /// broken.
    pub fn build_view(
        entries: Vec<SessionEntry>,
        session_id: &str,
        workspace_instructions: Option<String>,
        workspace_instructions_error: Option<String>,
    ) -> Result<SessionView, SessionViewError> {
        let workspace = extract_workspace(session_id, &entries)?;
        let mut messages_by_id: HashMap<String, Message> = HashMap::new();
        let mut message_order: Vec<String> = Vec::new();

        for entry in entries {
            match entry {
                SessionEntry::Message(message) => {
                    message_order.push(message.id.clone());
                    let _ = messages_by_id.insert(message.id.clone(), message);
                }
                SessionEntry::Session { .. } => {}
            }
        }

        let mut messages: Vec<Message> = Vec::new();
        if let Some(latest_id) = message_order.last().cloned() {
            let mut current: Option<String> = Some(latest_id);
            while let Some(id) = &current {
                let message = messages_by_id.get(id).cloned().ok_or_else(|| {
                    SessionViewError::BrokenChain {
                        session_id: session_id.to_owned(),
                    }
                })?;
                current.clone_from(&message.parent_id);
                messages.push(message);
            }
            messages.reverse();
        }

        Ok(SessionView {
            session_id: session_id.to_owned(),
            workspace: workspace.clone(),
            workspace_instructions,
            workspace_instructions_error,
            messages,
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
    use byte_protocol::{MessageBody, MessageRole};

    fn message_text(message: &Message) -> &str {
        match &message.body.0[..] {
            [byte_protocol::MessageBlock::Text { text }] => text.as_str(),
            _ => "",
        }
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
    fn build_view_broken_chain_returns_error() {
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

        let err = SessionViewRepository::build_view(entries, "session-1", None, None)
            .expect_err("broken chain should fail");

        assert!(
            matches!(err, SessionViewError::BrokenChain { session_id } if session_id == "session-1")
        );
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
