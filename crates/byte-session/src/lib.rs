use std::collections::HashMap;
use std::path::{Path, PathBuf};

use byte_protocol::{
    encode_json_line, MessageRole, SessionEntry, SessionMessage, SessionMessageContent,
    SessionSummary, SessionView,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session id contains invalid characters: {0}")]
    InvalidSessionId(String),
    #[error("session directory is invalid: {0}")]
    InvalidDirectory(String),
    #[error("session already exists: {0}")]
    AlreadyExists(String),
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("session {0} has no header")]
    MissingHeader(String),
    #[error("session {0} has a broken parent chain")]
    BrokenChain(String),
    #[error("session {0} is busy")]
    Busy(String),
    #[error("failed to read session file: {0}")]
    Read(#[from] std::io::Error),
    #[error("failed to serialize session entry: {0}")]
    Serialize(#[from] serde_json::Error),
}

impl From<byte_protocol::ProtocolError> for SessionError {
    fn from(error: byte_protocol::ProtocolError) -> Self {
        match error {
            byte_protocol::ProtocolError::Serialize(source) => SessionError::Serialize(source),
        }
    }
}

/// Persists Sessions as LF-delimited JSON records with stable entry IDs and
/// parent IDs forming a tree inside a single session file.
pub struct SessionStore {
    base_dir: PathBuf,
}

impl SessionStore {
    /// Create a store rooted at the given directory.
    pub fn new(base_dir: impl Into<PathBuf>) -> Result<Self, SessionError> {
        let base_dir = base_dir.into();
        if !base_dir.is_absolute() {
            return Err(SessionError::InvalidDirectory(
                "session base dir must be absolute".into(),
            ));
        }
        Ok(Self { base_dir })
    }

    /// Create a store using the default XDG data directory
    /// (`$XDG_DATA_HOME/byte/sessions`, falling back to `$HOME/.local/share/byte/sessions`).
    pub fn with_default_dir() -> Result<Self, SessionError> {
        Self::new(default_base_dir()?)
    }

    /// Ensure a session file exists with a valid header. The write is atomic
    /// via `create_new`; if the file already exists the call is idempotent.
    pub async fn new_session(
        &self,
        session_id: &str,
        workspace: Option<&str>,
    ) -> Result<(), SessionError> {
        let path = self.session_path(session_id)?;
        tokio::fs::create_dir_all(&self.base_dir).await?;

        let header = SessionEntry::Session {
            version: byte_protocol::PROTOCOL_VERSION,
            id: session_id.to_owned(),
            workspace: workspace.map(|s| s.to_owned()),
            created_at: now_epoch_millis(),
        };

        match self.atomic_write(&path, &header).await {
            Ok(()) => Ok(()),
            Err(SessionError::Read(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// Append a message entry to the session file and return its stable id.
    /// If `id` is `None`, a UUID is generated.
    pub async fn append_message(
        &self,
        session_id: &str,
        id: Option<&str>,
        parent_id: Option<&str>,
        role: MessageRole,
        content: impl Into<String>,
    ) -> Result<String, SessionError> {
        let path = self.session_path(session_id)?;
        let id = id
            .map(|s| s.to_owned())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let entry = SessionEntry::Message {
            id: id.clone(),
            parent_id: parent_id.map(|s| s.to_owned()),
            message: SessionMessageContent {
                role,
                content: content.into(),
            },
        };
        self.write_line(&path, &entry).await?;
        Ok(id)
    }

    /// Maximum session file size that will be loaded into memory (64 MiB).
    pub const MAX_SESSION_FILE_SIZE: u64 = 64 * 1024 * 1024;

    /// Load a normalized `SessionView` by following the active path from the
    /// most recent message back to the root.
    pub async fn load_session(&self, session_id: &str) -> Result<SessionView, SessionError> {
        let path = self.session_path(session_id)?;
        if !path.exists() {
            return Err(SessionError::NotFound(session_id.to_owned()));
        }

        let metadata = tokio::fs::metadata(&path).await?;
        if metadata.len() > Self::MAX_SESSION_FILE_SIZE {
            return Err(SessionError::Read(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "session file {} exceeds {} byte limit",
                    path.display(),
                    Self::MAX_SESSION_FILE_SIZE
                ),
            )));
        }

        let contents = tokio::fs::read_to_string(&path).await?;
        let entries: Vec<SessionEntry> =
            byte_protocol::decode_json_lines(&contents).map_err(SessionError::Serialize)?;

        reconstruct_view(session_id, entries)
    }

    /// List all sessions as lightweight summaries, ordered by `created_at` descending.
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>, SessionError> {
        tokio::fs::create_dir_all(&self.base_dir).await?;

        let mut summaries = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.base_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            let metadata = entry.metadata().await?;
            if !metadata.is_file() {
                continue;
            }

            let session_id = match path.file_stem().and_then(|s| s.to_str()) {
                Some(id) => id.to_owned(),
                None => continue,
            };

            let header = match read_session_header(&path).await {
                Ok(header) => header,
                Err(_) => continue,
            };

            if let SessionEntry::Session {
                id,
                workspace,
                created_at,
                ..
            } = header
            {
                if id == session_id {
                    summaries.push(SessionSummary {
                        session_id: id,
                        workspace,
                        created_at,
                    });
                }
            }
        }

        summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(summaries)
    }

    /// Delete the session file if it exists. Returns success even if the file
    /// is already gone.
    pub async fn delete_session(&self, session_id: &str) -> Result<(), SessionError> {
        let path = self.session_path(session_id)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(SessionError::Read(error)),
        }
    }

    fn session_path(&self, session_id: &str) -> Result<PathBuf, SessionError> {
        if session_id.is_empty() {
            return Err(SessionError::InvalidSessionId(
                "session id must not be empty".into(),
            ));
        }
        if session_id.len() > 128 {
            return Err(SessionError::InvalidSessionId(
                "session id must be 128 characters or fewer".into(),
            ));
        }
        if session_id.contains(['/', '\\']) || session_id.contains("..") {
            return Err(SessionError::InvalidSessionId(session_id.to_owned()));
        }
        if session_id.starts_with('.') {
            return Err(SessionError::InvalidSessionId(
                "session id must not start with a dot".into(),
            ));
        }
        if session_id.chars().any(|c| c.is_ascii_control()) {
            return Err(SessionError::InvalidSessionId(
                "session id contains control characters".into(),
            ));
        }
        Ok(self.base_dir.join(format!("{session_id}.jsonl")))
    }

    async fn write_line(&self, path: &Path, entry: &SessionEntry) -> Result<(), SessionError> {
        let line = encode_json_line(entry)?;
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?
            .write_all(line.as_bytes())
            .await?;
        Ok(())
    }

    /// Write `entry` to `path` atomically using `create_new`. Fails with
    /// `AlreadyExists` if the file already exists.
    async fn atomic_write(&self, path: &Path, entry: &SessionEntry) -> Result<(), SessionError> {
        let line = encode_json_line(entry)?;
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.flush().await?;
        Ok(())
    }
}

fn default_base_dir() -> Result<PathBuf, SessionError> {
    let data_dir = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| String::from("."));
            PathBuf::from(home).join(".local").join("share")
        });
    Ok(data_dir.join("byte").join("sessions"))
}

fn now_epoch_millis() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before Unix epoch");
    format!("{}.{:03}Z", now.as_secs(), now.subsec_millis())
}

async fn read_session_header(path: &Path) -> Result<SessionEntry, SessionError> {
    let file = tokio::fs::File::open(path).await?;
    let reader = tokio::io::BufReader::new(file);
    let mut lines = reader.lines();

    let first = lines
        .next_line()
        .await?
        .ok_or_else(|| SessionError::MissingHeader(path.display().to_string()))?;

    byte_protocol::decode_json_line::<SessionEntry>(first.trim_end_matches(['\r', '\n']))
        .map_err(SessionError::Serialize)
}

fn reconstruct_view(
    session_id: &str,
    entries: Vec<SessionEntry>,
) -> Result<SessionView, SessionError> {
    let mut workspace: Option<String> = None;
    let mut messages_by_id: HashMap<String, SessionMessage> = HashMap::new();
    let mut message_order: Vec<String> = Vec::new();

    for entry in entries {
        match entry {
            SessionEntry::Session {
                id, workspace: ws, ..
            } if id == session_id => {
                workspace = ws;
            }
            SessionEntry::Message {
                id,
                parent_id,
                message,
            } => {
                message_order.push(id.clone());
                messages_by_id.insert(
                    id.clone(),
                    SessionMessage {
                        id,
                        parent_id,
                        role: message.role,
                        content: message.content,
                    },
                );
            }
            _ => {}
        }
    }

    let mut messages: Vec<SessionMessage> = Vec::new();
    if let Some(latest_id) = message_order.last().cloned() {
        let mut current: Option<String> = Some(latest_id);
        while let Some(id) = current {
            let message = messages_by_id
                .get(&id)
                .cloned()
                .ok_or_else(|| SessionError::BrokenChain(session_id.to_owned()))?;
            current = message.parent_id.clone();
            messages.push(message);
        }
        messages.reverse();
    }

    Ok(SessionView {
        session_id: session_id.to_owned(),
        workspace,
        messages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> SessionStore {
        let dir = tempfile::tempdir().expect("temp dir");
        SessionStore::new(dir.path().to_path_buf()).expect("store creates")
    }

    #[tokio::test]
    async fn new_session_writes_header() {
        let store = temp_store();

        store
            .new_session("session-1", Some("/workspace"))
            .await
            .expect("new session");

        let path = store.session_path("session-1").unwrap();
        let contents = tokio::fs::read_to_string(&path).await.expect("read");
        let entry: SessionEntry = serde_json::from_str(contents.trim()).expect("parse");

        assert!(
            matches!(entry, SessionEntry::Session { id, workspace: Some(ws), version, .. } if id == "session-1" && ws == "/workspace" && version == byte_protocol::PROTOCOL_VERSION)
        );
    }

    #[tokio::test]
    async fn new_session_is_idempotent() {
        let store = temp_store();

        store.new_session("session-1", None).await.unwrap();
        store.new_session("session-1", None).await.unwrap();

        let view = store.load_session("session-1").await.unwrap();
        assert_eq!(view.messages.len(), 0);
    }

    #[tokio::test]
    async fn append_message_creates_entry_with_parent() {
        let store = temp_store();
        store.new_session("session-1", None).await.unwrap();

        let first_id = store
            .append_message("session-1", None, None, MessageRole::Developer, "hello")
            .await
            .expect("append first");

        let second_id = store
            .append_message(
                "session-1",
                None,
                Some(&first_id),
                MessageRole::Assistant,
                "hi",
            )
            .await
            .expect("append second");
        assert_ne!(first_id, second_id);

        let path = store.session_path("session-1").unwrap();
        let contents = tokio::fs::read_to_string(&path).await.expect("read");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 3);

        let second: SessionEntry = serde_json::from_str(lines[2]).unwrap();
        assert!(
            matches!(second, SessionEntry::Message { id, parent_id: Some(parent), message } if id == second_id && parent == first_id && message.role == MessageRole::Assistant && message.content == "hi")
        );
    }

    #[tokio::test]
    async fn load_session_reconstructs_active_path() {
        let store = temp_store();
        store
            .new_session("session-1", Some("/workspace"))
            .await
            .unwrap();

        let first_id = store
            .append_message("session-1", None, None, MessageRole::Developer, "hello")
            .await
            .unwrap();
        let second_id = store
            .append_message(
                "session-1",
                None,
                Some(&first_id),
                MessageRole::Assistant,
                "hi",
            )
            .await
            .unwrap();

        let view = store.load_session("session-1").await.unwrap();

        assert_eq!(view.session_id, "session-1");
        assert_eq!(view.workspace.as_deref(), Some("/workspace"));
        assert_eq!(view.messages.len(), 2);
        assert_eq!(view.messages[0].id, first_id);
        assert_eq!(view.messages[0].role, MessageRole::Developer);
        assert_eq!(view.messages[0].content, "hello");
        assert_eq!(view.messages[1].id, second_id);
        assert_eq!(view.messages[1].parent_id, Some(first_id));
        assert_eq!(view.messages[1].role, MessageRole::Assistant);
        assert_eq!(view.messages[1].content, "hi");
    }

    #[tokio::test]
    async fn load_missing_session_fails() {
        let store = temp_store();

        let err = store
            .load_session("missing")
            .await
            .expect_err("missing session should fail");

        assert!(matches!(err, SessionError::NotFound(id) if id == "missing"));
    }

    #[tokio::test]
    async fn list_sessions_returns_summaries_in_descending_created_order() {
        let store = temp_store();
        store
            .new_session("session-a", Some("/workspace/a"))
            .await
            .unwrap();
        // Small sleep to guarantee distinct created_at ordering.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        store
            .new_session("session-b", Some("/workspace/b"))
            .await
            .unwrap();

        let summaries = store.list_sessions().await.unwrap();

        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].session_id, "session-b");
        assert_eq!(summaries[0].workspace.as_deref(), Some("/workspace/b"));
        assert_eq!(summaries[1].session_id, "session-a");
        assert_eq!(summaries[1].workspace.as_deref(), Some("/workspace/a"));
    }

    #[tokio::test]
    async fn delete_session_removes_file() {
        let store = temp_store();
        store.new_session("session-1", None).await.unwrap();

        store.delete_session("session-1").await.unwrap();

        assert!(!store.session_path("session-1").unwrap().exists());
        assert!(matches!(
            store.load_session("session-1").await.unwrap_err(),
            SessionError::NotFound(id) if id == "session-1"
        ));
    }

    #[tokio::test]
    async fn delete_session_is_idempotent() {
        let store = temp_store();

        store.delete_session("never-created").await.unwrap();
    }

    #[test]
    fn rejects_path_traversal_session_id() {
        let store = temp_store();
        assert!(store.session_path("../escape").is_err());
        assert!(store.session_path("foo/bar").is_err());
    }
}
