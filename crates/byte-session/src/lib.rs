//! Persistent session storage for the byte agent.
//!
//! Sessions are stored as line-delimited JSON records where each entry has a
//! stable id and an optional parent id, forming a tree inside a single file.
//! The store supports creating sessions, appending messages and tool results,
//! listing summaries, reading raw entries, and deleting sessions.
//! View reconstruction (parent-chain walking, workspace instruction reading,
//! and `SessionView` assembly) lives in `byte-core`.
#![deny(rustdoc::broken_intra_doc_links)]

pub mod persistence;
pub mod tree;

use std::path::{Path, PathBuf};

use byte_protocol::{
    ActivatedSkill, CompactionEntry, Message, MessageBody, MessageRole, SessionEntry,
    SessionSummary, encode_json_line,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

/// Errors that can occur when interacting with the session store.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The session id contains invalid characters.
    #[error("session id contains invalid characters: {0}")]
    InvalidSessionId(String),
    /// The session directory is invalid.
    #[error("session directory is invalid: {0}")]
    InvalidDirectory(String),
    /// A session with the requested id already exists.
    #[error("session already exists: {0}")]
    AlreadyExists(String),
    /// The requested session could not be found.
    #[error("session not found: {0}")]
    NotFound(String),
    /// An I/O error occurred while reading or writing a session file.
    #[error("failed to read session file: {0}")]
    Read(#[from] std::io::Error),
    /// A session entry could not be serialized or deserialized.
    #[error("failed to serialize session entry: {0}")]
    Serialize(#[from] serde_json::Error),
}

impl From<byte_protocol::ProtocolError> for SessionError {
    /// Convert a protocol error into the session error it represents.
    fn from(error: byte_protocol::ProtocolError) -> Self {
        match error {
            byte_protocol::ProtocolError::Serialize(source) => Self::Serialize(source),
        }
    }
}

/// Persists Sessions as LF-delimited JSON records with stable entry IDs and
/// parent IDs forming a tree inside a single session file.
#[derive(Debug)]
pub struct SessionStore {
    /// Absolute directory containing the JSONL session files.
    base_dir: PathBuf,
}

impl SessionStore {
    /// Create a store rooted at the given directory.
    ///
    /// # Errors
    ///
    /// Returns an error if `base_dir` is not an absolute path.
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
    ///
    /// # Errors
    ///
    /// Returns an error if the default base directory is not absolute.
    pub fn with_default_dir() -> Result<Self, SessionError> {
        Self::new(default_base_dir())
    }

    /// Ensure a session file exists with a valid header. The write is atomic
    /// via `create_new`; if the file already exists the call is idempotent.
    ///
    /// # Errors
    ///
    /// Returns an error if the session id is invalid or the session file cannot
    /// be created.
    pub async fn new_session(&self, session_id: &str, workspace: &str) -> Result<(), SessionError> {
        let path = self.session_path(session_id)?;
        tokio::fs::create_dir_all(&self.base_dir).await?;

        let header = SessionEntry::Session {
            version: byte_protocol::PROTOCOL_VERSION,
            id: session_id.to_owned(),
            workspace: workspace.to_owned(),
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
    ///
    /// # Errors
    ///
    /// Returns an error if the session id is invalid or the entry cannot be
    /// written.
    pub async fn append_message(
        &self,
        session_id: &str,
        id: Option<&str>,
        parent_id: Option<&str>,
        role: MessageRole,
        body: MessageBody,
        tool_call_id: Option<&str>,
    ) -> Result<String, SessionError> {
        let path = self.session_path(session_id)?;
        let id = id.map_or_else(|| uuid::Uuid::new_v4().to_string(), ToOwned::to_owned);
        let entry = SessionEntry::Message(Message {
            id: id.clone(),
            parent_id: parent_id.map(ToOwned::to_owned),
            role,
            tool_call_id: tool_call_id.map(ToOwned::to_owned),
            body,
        });
        self.write_line(&path, &entry).await?;
        Ok(id)
    }

    /// Append a skill activation record to the session file.
    ///
    /// # Errors
    ///
    /// Returns an error if the session id is invalid or the entry cannot be
    /// written.
    pub async fn append_skill_activation(
        &self,
        session_id: &str,
        name: &str,
        content: &str,
    ) -> Result<(), SessionError> {
        let path = self.session_path(session_id)?;
        let entry = SessionEntry::SkillActivated(ActivatedSkill {
            name: name.to_owned(),
            content: content.to_owned(),
        });
        self.write_line(&path, &entry).await
    }

    /// Append a compaction entry to the session file and return its identifier.
    ///
    /// # Errors
    ///
    /// Returns an error if the session id is invalid or the entry cannot be
    /// written.
    pub async fn append_compaction_entry(
        &self,
        session_id: &str,
        entry: CompactionEntry,
    ) -> Result<String, SessionError> {
        let path = self.session_path(session_id)?;
        let id = entry.id.clone();
        self.write_line(&path, &SessionEntry::CompactionEntry(entry))
            .await?;
        Ok(id)
    }

    /// Maximum session file size that will be loaded into memory (64 MiB).
    pub const MAX_SESSION_FILE_SIZE: u64 = 64 * 1024 * 1024;

    /// Read all persisted entries for a session.
    ///
    /// This is the raw persistence read: it validates the file exists, enforces
    /// the size limit, parses the JSONL lines, and returns the decoded entries.
    /// It does not reconstruct the active path or read `AGENTS.md`.
    ///
    /// # Errors
    ///
    /// Returns an error if the session is not found, the file is too large, or
    /// the entries cannot be parsed.
    pub async fn read_entries(&self, session_id: &str) -> Result<Vec<SessionEntry>, SessionError> {
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
        persistence::decode_session_entries(&contents).map_err(SessionError::Serialize)
    }

    /// List all sessions as lightweight summaries, ordered by `created_at` descending.
    ///
    /// # Errors
    ///
    /// Returns an error if the session directory cannot be read.
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

            let Ok(header) = read_session_header(&path).await else {
                continue;
            };

            if let SessionEntry::Session {
                id,
                workspace,
                created_at,
                ..
            } = header
                && id == session_id
            {
                summaries.push(SessionSummary {
                    session_id: id,
                    workspace,
                    created_at,
                });
            }
        }

        summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(summaries)
    }

    /// Delete the session file if it exists. Returns success even if the file
    /// is already gone.
    ///
    /// # Errors
    ///
    /// Returns an error if the session id is invalid or the file cannot be
    /// removed for reasons other than not existing.
    pub async fn delete_session(&self, session_id: &str) -> Result<(), SessionError> {
        let path = self.session_path(session_id)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(SessionError::Read(error)),
        }
    }

    /// Returns the file path for `session_id` inside `base_dir`, validating the id.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::InvalidSessionId`] when `session_id` is empty, too long,
    /// contains path traversal characters, starts with a dot, or contains control characters.
    pub fn session_path(&self, session_id: &str) -> Result<PathBuf, SessionError> {
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

    /// Appends `entry` as a JSON line to `path`.
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

/// Returns the default base directory for session storage, honoring `XDG_DATA_HOME`.
fn default_base_dir() -> PathBuf {
    let data_dir = std::env::var("XDG_DATA_HOME").map_or_else(
        |_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| String::from("."));
            PathBuf::from(home).join(".local").join("share")
        },
        PathBuf::from,
    );
    data_dir.join("byte").join("sessions")
}

/// Returns the current time formatted as seconds.milliseconds UTC.
fn now_epoch_millis() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}Z", now.as_secs(), now.subsec_millis())
}

/// Reads and parses the first line of the session file at `path`.
async fn read_session_header(path: &Path) -> Result<SessionEntry, SessionError> {
    let file = tokio::fs::File::open(path).await?;
    let reader = tokio::io::BufReader::new(file);
    let mut lines = reader.lines();

    let first = lines.next_line().await?.ok_or_else(|| {
        SessionError::Read(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("session file {} is missing its header", path.display()),
        ))
    })?;

    byte_protocol::decode_json_line::<SessionEntry>(first.trim_end_matches(['\r', '\n']))
        .map_err(SessionError::Serialize)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn message_text(message: &Message) -> &str {
        match &message.body.0[..] {
            [byte_protocol::MessageBlock::Text { text }] => text.as_str(),
            _ => "",
        }
    }

    fn temp_store() -> SessionStore {
        let dir = tempfile::tempdir().expect("temp dir");
        SessionStore::new(dir.path().to_path_buf()).expect("store creates")
    }

    #[tokio::test]
    async fn new_session_writes_header() {
        let store = temp_store();

        store
            .new_session("session-1", "/workspace")
            .await
            .expect("new session");

        let path = store.session_path("session-1").unwrap();
        let contents = tokio::fs::read_to_string(&path).await.expect("read");
        let entry: SessionEntry = serde_json::from_str(contents.trim()).expect("parse");

        assert!(
            matches!(entry, SessionEntry::Session { id, workspace: ws, version, .. } if id == "session-1" && ws == "/workspace" && version == byte_protocol::PROTOCOL_VERSION)
        );
    }

    #[tokio::test]
    async fn new_session_is_idempotent() {
        let store = temp_store();

        store.new_session("session-1", "/workspace").await.unwrap();
        store.new_session("session-1", "/workspace").await.unwrap();

        let entries = store.read_entries("session-1").await.unwrap();
        assert_eq!(entries.len(), 1);
        assert!(matches!(&entries[0], SessionEntry::Session { id, .. } if id == "session-1"));
    }

    #[tokio::test]
    async fn append_message_creates_entry_with_parent() {
        let store = temp_store();
        store.new_session("session-1", "/workspace").await.unwrap();

        let first_id = store
            .append_message(
                "session-1",
                None,
                None,
                MessageRole::Developer,
                MessageBody::text("hello"),
                None,
            )
            .await
            .expect("append first");

        let second_id = store
            .append_message(
                "session-1",
                None,
                Some(&first_id),
                MessageRole::Assistant,
                MessageBody::text("hi"),
                None,
            )
            .await
            .expect("append second");
        assert_ne!(first_id, second_id);

        let path = store.session_path("session-1").unwrap();
        let contents = tokio::fs::read_to_string(&path).await.expect("read");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 3);

        let second: SessionEntry = serde_json::from_str(lines[2]).expect("parse second entry");
        assert!(matches!(
            &second,
            SessionEntry::Message(Message {
                id,
                parent_id: Some(parent),
                role: MessageRole::Assistant,
                tool_call_id: None,
                ..
            }) if id == &second_id && parent == &first_id
        ));
        if let SessionEntry::Message(message) = &second {
            assert_eq!(message_text(message), "hi");
        }
    }

    #[tokio::test]
    async fn append_skill_activation_writes_skill_entry() {
        let store = temp_store();
        store.new_session("session-1", "/workspace").await.unwrap();

        store
            .append_skill_activation("session-1", "review", "Review carefully.")
            .await
            .expect("append skill activation");

        let entries = store.read_entries("session-1").await.unwrap();
        assert_eq!(entries.len(), 2);
        assert!(
            matches!(
                &entries[1],
                SessionEntry::SkillActivated(ActivatedSkill { name, content }) if name == "review" && content == "Review carefully."
            ),
            "second entry should be skill activation"
        );
    }

    #[tokio::test]
    async fn read_entries_returns_header_and_messages() {
        let store = temp_store();
        store.new_session("session-1", "/workspace").await.unwrap();

        let first_id = store
            .append_message(
                "session-1",
                None,
                None,
                MessageRole::Developer,
                MessageBody::text("hello"),
                None,
            )
            .await
            .unwrap();
        let _second_id = store
            .append_message(
                "session-1",
                None,
                Some(&first_id),
                MessageRole::Assistant,
                MessageBody::text("hi"),
                None,
            )
            .await
            .unwrap();

        let entries = store.read_entries("session-1").await.unwrap();
        assert_eq!(entries.len(), 3);
        assert!(matches!(
            &entries[0],
            SessionEntry::Session { id, workspace, .. } if id == "session-1" && workspace == "/workspace"
        ));
        assert!(matches!(
            &entries[1],
            SessionEntry::Message(Message { id, role: MessageRole::Developer, .. }) if id == &first_id
        ));
    }

    #[tokio::test]
    async fn read_entries_missing_session_returns_not_found() {
        let store = temp_store();

        let err = store
            .read_entries("missing")
            .await
            .expect_err("missing session should fail");

        assert!(matches!(err, SessionError::NotFound(id) if id == "missing"));
    }

    #[tokio::test]
    async fn read_entries_rejects_oversized_file() {
        let store = temp_store();
        store.new_session("session-1", "/workspace").await.unwrap();

        // Write enough data to exceed the limit.
        let huge_payload = "x".repeat(64 * 1024 * 1024 + 1);
        let path = store.session_path("session-1").unwrap();
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .unwrap()
            .write_all(huge_payload.as_bytes())
            .await
            .unwrap();

        let err = store
            .read_entries("session-1")
            .await
            .expect_err("oversized file should fail");
        assert!(matches!(err, SessionError::Read(_)));
    }

    #[tokio::test]
    async fn read_entries_parses_existing_session_header() {
        let store = temp_store();
        store.new_session("session-1", "/workspace").await.unwrap();

        let entries = store.read_entries("session-1").await.unwrap();
        assert_eq!(entries.len(), 1);
        assert!(matches!(
            &entries[0],
            SessionEntry::Session {
                id,
                workspace,
                version,
                ..
            } if id == "session-1" && workspace == "/workspace" && *version == byte_protocol::PROTOCOL_VERSION
        ));
    }

    #[tokio::test]
    async fn list_sessions_returns_summaries_in_descending_created_order() {
        let store = temp_store();
        store
            .new_session("session-a", "/workspace/a")
            .await
            .unwrap();
        // Small sleep to guarantee distinct created_at ordering.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        store
            .new_session("session-b", "/workspace/b")
            .await
            .unwrap();

        let summaries = store.list_sessions().await.unwrap();

        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].session_id, "session-b");
        assert_eq!(summaries[0].workspace, "/workspace/b");
        assert_eq!(summaries[1].session_id, "session-a");
        assert_eq!(summaries[1].workspace, "/workspace/a");
    }

    #[tokio::test]
    async fn delete_session_removes_file() {
        let store = temp_store();
        store.new_session("session-1", "/workspace").await.unwrap();

        store.delete_session("session-1").await.unwrap();

        assert!(!store.session_path("session-1").unwrap().exists());
        assert!(matches!(
            store.read_entries("session-1").await.unwrap_err(),
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
