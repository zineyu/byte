//! Active-path reconstruction from persisted session entries.
//!
//! The active path is the ordered subset of a session's history that the
//! runtime uses when constructing the LLM context for the next model turn.
//! Compaction entries summarize older messages and take their place in the
//! active path while the original messages remain persisted.

use std::collections::{HashMap, HashSet};

use byte_protocol::{CompactionEntry, LlmMessage, MessageBody, MessageRole, SessionEntry};

/// Build the active conversation path as a list of LLM messages.
///
/// Walks the persisted entries from oldest to newest, including recent
/// messages and compaction entries while skipping messages that have been
/// summarized by a compaction entry. The returned vector is ordered from
/// oldest to newest so it can be used directly as LLM context history.
///
/// Messages whose identifiers fall inside a [`CompactionEntry::compacted_range`]
/// (inclusive of the endpoints) are omitted from the active path.
#[must_use]
pub fn build_active_path(entries: &[SessionEntry]) -> Vec<LlmMessage> {
    // Map each message ID to its chronological index among all messages.
    let mut message_positions: HashMap<String, usize> = HashMap::new();
    let mut ordered_message_ids: Vec<String> = Vec::new();
    for entry in entries {
        if let SessionEntry::Message(message) = entry {
            let _ = message_positions.insert(message.id.clone(), ordered_message_ids.len());
            ordered_message_ids.push(message.id.clone());
        }
    }

    // Collect all message IDs that fall inside any compaction range.
    let mut compacted_ids: HashSet<String> = HashSet::new();
    for entry in entries {
        if let SessionEntry::CompactionEntry(compaction) = entry {
            mark_compacted_messages(
                compaction,
                &message_positions,
                &ordered_message_ids,
                &mut compacted_ids,
            );
        }
    }

    // Build the non-compacted path in chronological order, tracking the original
    // position of each surviving message so compaction summaries can be inserted
    // at the correct location.
    let mut path: Vec<LlmMessage> = Vec::new();
    let mut original_positions: Vec<usize> = Vec::new();
    for entry in entries {
        if let SessionEntry::Message(message) = entry {
            if compacted_ids.contains(&message.id) {
                continue;
            }
            path.push(LlmMessage {
                role: message.role,
                body: message.body.clone(),
                tool_call_id: message.tool_call_id.clone(),
            });
            original_positions.push(message_positions[&message.id]);
        }
    }

    // Insert each compaction summary at the position of the oldest message in
    // its range.
    for entry in entries {
        if let SessionEntry::CompactionEntry(compaction) = entry {
            let oldest_position = match (
                message_positions.get(&compaction.compacted_range.first_message_id),
                message_positions.get(&compaction.compacted_range.last_message_id),
            ) {
                (Some(&a), Some(&b)) => a.min(b),
                _ => continue,
            };

            let insertion_index = original_positions
                .iter()
                .position(|&pos| pos >= oldest_position)
                .unwrap_or(path.len());

            path.insert(insertion_index, compaction_to_llm_message(compaction));
            original_positions.insert(insertion_index, oldest_position);
        }
    }

    path
}

/// Mark all message identifiers within a compaction range as skipped.
fn mark_compacted_messages(
    compaction: &CompactionEntry,
    positions: &HashMap<String, usize>,
    ordered_ids: &[String],
    skipped: &mut HashSet<String>,
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
        let _ = skipped.insert(id.clone());
    }
}

/// Convert a compaction entry into a summary [`LlmMessage`].
fn compaction_to_llm_message(compaction: &CompactionEntry) -> LlmMessage {
    LlmMessage {
        role: MessageRole::Summary,
        body: MessageBody::text(&compaction.summary),
        tool_call_id: None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use byte_protocol::{CompactionRange, Message, MessageBody, MessageRole};

    fn msg(id: &str, parent_id: Option<&str>, role: MessageRole, text: &str) -> SessionEntry {
        SessionEntry::Message(Message {
            id: id.into(),
            parent_id: parent_id.map(Into::into),
            role,
            tool_call_id: None,
            body: MessageBody::text(text),
        })
    }

    fn compaction(id: &str, first: &str, last: &str, summary: &str) -> SessionEntry {
        SessionEntry::CompactionEntry(CompactionEntry {
            id: id.into(),
            role: MessageRole::Summary,
            summary: summary.into(),
            compacted_range: CompactionRange {
                first_message_id: first.into(),
                last_message_id: last.into(),
            },
            created_at: "t".into(),
            run_id: "r1".into(),
        })
    }

    #[test]
    fn active_path_includes_recent_messages_and_skips_compacted() {
        let entries = vec![
            msg("m1", None, MessageRole::Developer, "hello"),
            msg("m2", Some("m1"), MessageRole::Assistant, "hi"),
            msg("m3", Some("m2"), MessageRole::Developer, "next"),
            compaction("ce1", "m1", "m2", "greeting"),
            msg("m4", Some("m3"), MessageRole::Assistant, "answer"),
        ];

        let path = build_active_path(&entries);

        assert_eq!(path.len(), 3);
        assert_eq!(path[0].role, MessageRole::Summary);
        assert_eq!(body_text(&path[0].body), "greeting");
        assert_eq!(path[1].role, MessageRole::Developer);
        assert_eq!(body_text(&path[1].body), "next");
        assert_eq!(path[2].role, MessageRole::Assistant);
        assert_eq!(body_text(&path[2].body), "answer");
    }

    #[test]
    fn active_path_compaction_summary_at_beginning() {
        let entries = vec![
            msg("m1", None, MessageRole::Developer, "first"),
            msg("m2", Some("m1"), MessageRole::Assistant, "second"),
            compaction("ce1", "m1", "m2", "summary"),
            msg("m3", Some("m2"), MessageRole::Developer, "third"),
        ];

        let path = build_active_path(&entries);

        assert_eq!(path.len(), 2);
        assert_eq!(path[0].role, MessageRole::Summary);
        assert_eq!(body_text(&path[0].body), "summary");
        assert_eq!(path[1].role, MessageRole::Developer);
        assert_eq!(body_text(&path[1].body), "third");
    }

    #[test]
    fn active_path_with_multiple_compactions_reconstructs_correctly() {
        let entries = vec![
            msg("m1", None, MessageRole::Developer, "a"),
            msg("m2", Some("m1"), MessageRole::Assistant, "b"),
            compaction("ce1", "m1", "m2", "ab"),
            msg("m3", Some("m2"), MessageRole::Developer, "c"),
            msg("m4", Some("m3"), MessageRole::Assistant, "d"),
            compaction("ce2", "m3", "m4", "cd"),
            msg("m5", Some("m4"), MessageRole::Developer, "e"),
        ];

        let path = build_active_path(&entries);

        assert_eq!(path.len(), 3);
        assert_eq!(body_text(&path[0].body), "ab");
        assert_eq!(body_text(&path[1].body), "cd");
        assert_eq!(body_text(&path[2].body), "e");
    }

    fn body_text(body: &MessageBody) -> &str {
        match &body.0[..] {
            [byte_protocol::MessageBlock::Text { text }] => text.as_str(),
            _ => "",
        }
    }
}
