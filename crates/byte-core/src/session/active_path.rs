//! Active-path reconstruction from persisted session entries.
//!
//! The active path is the ordered subset of a session's history that the
//! runtime uses when constructing the LLM context for the next model turn.
//! Compaction entries summarize older messages and take their place in the
//! active path while the original messages remain persisted.
//!
//! Activated skills are injected as synthetic user-role messages at the
//! position of their [`SessionEntry::SkillActivated`] record. Because
//! compaction ranges only reference persisted message ids, these synthetic
//! messages can never be compacted, which keeps activated skill content in
//! the model context permanently without putting it in the system prompt
//! (see ADR 0021). Tool result messages of `activate_skill` calls are
//! replaced with a short pointer so the skill content is not duplicated.

use std::collections::{HashMap, HashSet};

use byte_protocol::{
    ActivatedSkill, CompactionEntry, LlmMessage, MessageBlock, MessageBody, MessageRole,
    SessionEntry,
};

/// Build the active conversation path as a list of LLM messages.
///
/// Walks the persisted entries from oldest to newest, including recent
/// messages and compaction entries while skipping messages that have been
/// summarized by a compaction entry. The returned vector is ordered from
/// oldest to newest so it can be used directly as LLM context history.
///
/// Messages whose identifiers fall inside a [`CompactionEntry::compacted_range`]
/// (inclusive of the endpoints) are omitted from the active path.
///
/// [`SessionEntry::SkillActivated`] entries become synthetic user-role
/// messages at their stream position; when a skill was activated multiple
/// times, the latest content snapshot wins. Tool result messages belonging
/// to `activate_skill` calls are replaced with a short pointer because the
/// synthetic skill message already carries the full content.
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

    // Map `activate_skill` tool call ids to the activated skill name so the
    // matching tool result messages can be replaced with a short pointer.
    let activate_skill_calls = collect_activate_skill_calls(entries);

    // Names of skills with a persisted activation record; only these
    // activations provide the synthetic content message.
    let activated_names: HashSet<&str> = entries
        .iter()
        .filter_map(|entry| match entry {
            SessionEntry::SkillActivated(skill) => Some(skill.name.as_str()),
            _ => None,
        })
        .collect();

    // Build the non-compacted path in chronological order, tracking the original
    // position of each surviving message so compaction summaries can be inserted
    // at the correct location.
    let mut path: Vec<LlmMessage> = Vec::new();
    let mut original_positions: Vec<usize> = Vec::new();
    let mut skill_message_index: HashMap<String, usize> = HashMap::new();
    let mut messages_seen = 0usize;
    for entry in entries {
        match entry {
            SessionEntry::Message(message) => {
                messages_seen += 1;
                if compacted_ids.contains(&message.id) {
                    continue;
                }
                if let Some(skill_name) = message
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| activate_skill_calls.get(id))
                    && activated_names.contains(skill_name.as_str())
                {
                    // Keep the tool result so the assistant tool call stays
                    // paired, but drop the duplicated full content.
                    path.push(LlmMessage {
                        role: message.role,
                        body: MessageBody::text(format!(
                            "Skill `{skill_name}` activated; its instructions are provided in the conversation."
                        )),
                        tool_call_id: message.tool_call_id.clone(),
                    });
                    original_positions.push(message_positions[&message.id]);
                    continue;
                }
                path.push(LlmMessage {
                    role: message.role,
                    body: message.body.clone(),
                    tool_call_id: message.tool_call_id.clone(),
                });
                original_positions.push(message_positions[&message.id]);
            }
            SessionEntry::SkillActivated(skill) => {
                if let Some(&index) = skill_message_index.get(&skill.name) {
                    // Refresh the content snapshot in place; the latest
                    // activation wins, matching RunnerPool recovery semantics.
                    path[index] = skill_to_llm_message(skill);
                } else {
                    let _ = skill_message_index.insert(skill.name.clone(), path.len());
                    path.push(skill_to_llm_message(skill));
                    // Position the synthetic message after all messages seen
                    // so far and before any later message.
                    original_positions.push(messages_seen);
                }
            }
            _ => {}
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

/// Map each `activate_skill` tool call id to the skill name it activates.
fn collect_activate_skill_calls(entries: &[SessionEntry]) -> HashMap<String, String> {
    let mut calls: HashMap<String, String> = HashMap::new();
    for entry in entries {
        if let SessionEntry::Message(message) = entry {
            for block in &message.body.0 {
                if let MessageBlock::ToolCall(call) = block
                    && call.name == "activate_skill"
                    && let Some(name) = call.arguments.get("name").and_then(|v| v.as_str())
                {
                    let _ = calls.insert(call.id.clone(), name.to_owned());
                }
            }
        }
    }
    calls
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

/// Convert an activated skill record into a synthetic user-role
/// [`LlmMessage`] carrying the full skill instructions.
fn skill_to_llm_message(skill: &ActivatedSkill) -> LlmMessage {
    LlmMessage {
        role: MessageRole::Developer,
        body: MessageBody::text(format!(
            "Skill `{}` has been activated. Follow these instructions:\n\n{}",
            skill.name, skill.content
        )),
        tool_call_id: None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use byte_protocol::{
        ActivatedSkill, CompactionRange, Message, MessageBlock, MessageBody, MessageRole, ToolCall,
    };

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
            [MessageBlock::Text { text }] => text.as_str(),
            _ => "",
        }
    }

    fn skill(name: &str, content: &str) -> SessionEntry {
        SessionEntry::SkillActivated(ActivatedSkill {
            name: name.into(),
            content: content.into(),
        })
    }

    fn tool_call_msg(
        id: &str,
        parent_id: Option<&str>,
        call_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> SessionEntry {
        SessionEntry::Message(Message {
            id: id.into(),
            parent_id: parent_id.map(Into::into),
            role: MessageRole::Assistant,
            tool_call_id: None,
            body: MessageBody(vec![MessageBlock::ToolCall(ToolCall {
                id: call_id.into(),
                name: tool_name.into(),
                arguments,
            })]),
        })
    }

    fn tool_result_msg(
        id: &str,
        parent_id: Option<&str>,
        call_id: &str,
        text: &str,
    ) -> SessionEntry {
        SessionEntry::Message(Message {
            id: id.into(),
            parent_id: parent_id.map(Into::into),
            role: MessageRole::Tool,
            tool_call_id: Some(call_id.into()),
            body: MessageBody::text(text),
        })
    }

    #[test]
    fn skill_activated_entry_becomes_synthetic_developer_message_in_position() {
        let entries = vec![
            msg("m1", None, MessageRole::Developer, "hello"),
            skill("review", "Review carefully."),
            msg("m2", Some("m1"), MessageRole::Developer, "next"),
        ];

        let path = build_active_path(&entries);

        assert_eq!(path.len(), 3);
        assert_eq!(body_text(&path[0].body), "hello");
        assert_eq!(path[1].role, MessageRole::Developer);
        assert!(body_text(&path[1].body).contains("Skill `review` has been activated"));
        assert!(body_text(&path[1].body).contains("Review carefully."));
        assert_eq!(body_text(&path[2].body), "next");
    }

    #[test]
    fn repeated_skill_activation_keeps_latest_snapshot_only() {
        let entries = vec![
            msg("m1", None, MessageRole::Developer, "hello"),
            skill("review", "First version."),
            msg("m2", Some("m1"), MessageRole::Developer, "next"),
            skill("review", "Updated version."),
        ];

        let path = build_active_path(&entries);

        let skill_messages: Vec<_> = path
            .iter()
            .filter(|message| body_text(&message.body).contains("Skill `review`"))
            .collect();
        assert_eq!(skill_messages.len(), 1);
        assert!(body_text(&skill_messages[0].body).contains("Updated version."));
        assert!(!body_text(&skill_messages[0].body).contains("First version."));
    }

    #[test]
    fn activate_skill_tool_result_is_replaced_with_pointer() {
        let full_content = r#"{"name":"review","description":"d","content":"Review carefully."}"#;
        let entries = vec![
            msg("m1", None, MessageRole::Developer, "hello"),
            tool_call_msg(
                "m2",
                Some("m1"),
                "call-1",
                "activate_skill",
                serde_json::json!({"name": "review"}),
            ),
            tool_result_msg("m3", Some("m2"), "call-1", full_content),
            skill("review", "Review carefully."),
        ];

        let path = build_active_path(&entries);

        assert_eq!(path.len(), 4);
        // Tool result keeps its pairing but drops the duplicated full content.
        assert_eq!(path[2].role, MessageRole::Tool);
        assert_eq!(path[2].tool_call_id, Some("call-1".into()));
        assert!(body_text(&path[2].body).contains("Skill `review` activated"));
        assert!(!body_text(&path[2].body).contains("Review carefully."));
        // The synthetic message carries the content exactly once.
        let occurrences = path
            .iter()
            .filter(|message| body_text(&message.body).contains("Review carefully."))
            .count();
        assert_eq!(occurrences, 1);
    }

    #[test]
    fn activate_skill_tool_result_without_activation_record_keeps_original_body() {
        let entries = vec![
            msg("m1", None, MessageRole::Developer, "hello"),
            tool_call_msg(
                "m2",
                Some("m1"),
                "call-1",
                "activate_skill",
                serde_json::json!({"name": "missing"}),
            ),
            tool_result_msg("m3", Some("m2"), "call-1", "error: skill not found"),
        ];

        let path = build_active_path(&entries);

        assert_eq!(path.len(), 3);
        assert_eq!(body_text(&path[2].body), "error: skill not found");
    }

    #[test]
    fn skill_content_survives_compaction_of_surrounding_messages() {
        let entries = vec![
            msg("m1", None, MessageRole::Developer, "hello"),
            skill("review", "Review carefully."),
            msg("m2", Some("m1"), MessageRole::Assistant, "hi"),
            msg("m3", Some("m2"), MessageRole::Developer, "next"),
            msg("m4", Some("m3"), MessageRole::Assistant, "answer"),
            compaction("ce1", "m1", "m4", "everything so far"),
        ];

        let path = build_active_path(&entries);

        let skill_messages: Vec<_> = path
            .iter()
            .filter(|message| body_text(&message.body).contains("Review carefully."))
            .collect();
        assert_eq!(
            skill_messages.len(),
            1,
            "activated skill content must survive compaction"
        );
    }
}
