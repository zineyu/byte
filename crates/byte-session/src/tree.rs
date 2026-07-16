//! In-memory tree node representation for a persisted session.

use byte_protocol::{ActivatedSkill, CompactionEntry, Message, SessionEntry};

/// A single node in a Session tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// Session header node.
    SessionHeader {
        /// File format version.
        version: u16,
        /// Session identifier.
        id: String,
        /// Workspace path.
        workspace: String,
        /// ISO 8601 creation timestamp.
        created_at: String,
    },
    /// Message node.
    Message(Message),
    /// Skill activation node.
    SkillActivated(ActivatedSkill),
    /// Compaction summary node.
    CompactionEntry(CompactionEntry),
}

impl Node {
    /// Convert a raw [`SessionEntry`] into a tree node.
    #[must_use]
    pub fn from_entry(entry: SessionEntry) -> Option<Self> {
        match entry {
            SessionEntry::Session {
                version,
                id,
                workspace,
                created_at,
                ..
            } => Some(Self::SessionHeader {
                version,
                id,
                workspace,
                created_at,
            }),
            SessionEntry::Message(message) => Some(Self::Message(message)),
            SessionEntry::SkillActivated(skill) => Some(Self::SkillActivated(skill)),
            SessionEntry::CompactionEntry(entry) => Some(Self::CompactionEntry(entry)),
        }
    }

    /// Convert this tree node back into a [`SessionEntry`], if possible.
    #[must_use]
    pub fn into_entry(self) -> Option<SessionEntry> {
        match self {
            Self::SessionHeader {
                version,
                id,
                workspace,
                created_at,
            } => Some(SessionEntry::Session {
                version,
                id,
                workspace,
                created_at,
            }),
            Self::Message(message) => Some(SessionEntry::Message(message)),
            Self::SkillActivated(skill) => Some(SessionEntry::SkillActivated(skill)),
            Self::CompactionEntry(entry) => Some(SessionEntry::CompactionEntry(entry)),
        }
    }

    /// Return the stable identifier of this node, if it has one.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::SessionHeader { id, .. } => Some(id),
            Self::Message(message) => Some(&message.id),
            Self::SkillActivated(_) => None,
            Self::CompactionEntry(entry) => Some(&entry.id),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use byte_protocol::{CompactionRange, MessageRole};

    fn sample_compaction_entry() -> CompactionEntry {
        CompactionEntry {
            id: "ce-1".into(),
            role: MessageRole::Summary,
            summary: "summary".into(),
            compacted_range: CompactionRange {
                first_message_id: "msg-1".into(),
                last_message_id: "msg-3".into(),
            },
            created_at: "2026-07-16T10:00:00Z".into(),
            run_id: "run-1".into(),
        }
    }

    #[test]
    fn node_from_compaction_entry_roundtrips() {
        let entry = sample_compaction_entry();
        let node = Node::from_entry(SessionEntry::CompactionEntry(entry.clone()))
            .expect("compaction entry converts to node");
        let roundtripped = node.into_entry().expect("node converts back to entry");
        assert_eq!(roundtripped, SessionEntry::CompactionEntry(entry));
    }

    #[test]
    fn node_id_returns_compaction_entry_id() {
        let entry = sample_compaction_entry();
        let node = Node::from_entry(SessionEntry::CompactionEntry(entry)).unwrap();
        assert_eq!(node.id(), Some("ce-1"));
    }
}
