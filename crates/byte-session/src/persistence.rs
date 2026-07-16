//! JSONL persistence helpers for session entries.
//!
//! This module centralizes parsing so that the session store can load files
//! written by newer or older versions of the daemon without failing on
//! unknown entry variants.

use byte_protocol::SessionEntry;

/// Parse a JSONL string into a list of [`SessionEntry`] records, silently
/// skipping records whose `type` field is not recognized.
///
/// # Errors
///
/// Returns an error if a recognized record cannot be deserialized as JSON or
/// as a valid [`SessionEntry`].
#[must_use = "returns the decoded entries; discarding the result may miss parse failures"]
pub fn decode_session_entries(input: &str) -> Result<Vec<SessionEntry>, serde_json::Error> {
    let mut entries = Vec::new();
    for line in input.lines().filter(|line| !line.trim().is_empty()) {
        if let Some(entry) = decode_session_entry(line)? {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// Parse a single JSON line, returning `Ok(None)` when the record type is
/// unknown so that callers can degrade gracefully.
///
/// # Errors
///
/// Returns an error if the line is not valid JSON or if a known record type
/// cannot be deserialized into a [`SessionEntry`].
fn decode_session_entry(line: &str) -> Result<Option<SessionEntry>, serde_json::Error> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    let value: serde_json::Value = serde_json::from_str(trimmed)?;
    let ty = value.get("type").and_then(|v| v.as_str());
    match ty {
        Some("session" | "message" | "skill_activated" | "compaction_entry") => {
            serde_json::from_value(value).map(Some)
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use byte_protocol::{CompactionEntry, CompactionRange, Message, MessageRole};

    #[test]
    fn decodes_known_entries_and_skips_unknown_variant() {
        let input = concat!(
            r#"{"type":"session","version":8,"id":"s1","workspace":"/w","created_at":"t"}"#,
            "\n",
            r#"{"type":"message","id":"m1","parentId":null,"role":"developer","body":[{"type":"text","text":"hi"}]}"#,
            "\n",
            r#"{"type":"future_variant","id":"x"}"#,
            "\n",
            r#"{"type":"compaction_entry","id":"ce-1","role":"summary","summary":"...","compactedRange":{"firstMessageId":"m1","lastMessageId":"m1"},"createdAt":"t","runId":"r1"}"#,
            "\n"
        );

        let entries = decode_session_entries(input).expect("entries decode");
        assert_eq!(entries.len(), 3);
        assert!(matches!(&entries[0],
            SessionEntry::Session { id, .. } if id == "s1"
        ));
        assert!(matches!(&entries[1],
            SessionEntry::Message(Message { id, .. }) if id == "m1"
        ));
        assert!(matches!(
            &entries[2],
            SessionEntry::CompactionEntry(CompactionEntry { id, .. }) if id == "ce-1"
        ));
    }

    #[test]
    fn backward_compatible_load_without_compaction_entries() {
        let input = concat!(
            r#"{"type":"session","version":8,"id":"s1","workspace":"/w","created_at":"t"}"#,
            "\n",
            r#"{"type":"message","id":"m1","parentId":null,"role":"developer","body":[{"type":"text","text":"hi"}]}"#,
            "\n"
        );

        let entries = decode_session_entries(input).expect("entries decode");
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .all(|e| !matches!(e, SessionEntry::CompactionEntry(_)))
        );
    }

    #[test]
    fn compaction_entry_roundtrips() {
        let entry = SessionEntry::CompactionEntry(CompactionEntry {
            id: "ce-1".into(),
            role: MessageRole::Summary,
            summary: "summary".into(),
            compacted_range: CompactionRange {
                first_message_id: "m1".into(),
                last_message_id: "m2".into(),
            },
            created_at: "t".into(),
            run_id: "r1".into(),
        });
        let line = byte_protocol::encode_json_line(&entry).expect("entry encodes");
        let decoded = decode_session_entries(&line).expect("entries decode");
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0], entry);
    }
}
