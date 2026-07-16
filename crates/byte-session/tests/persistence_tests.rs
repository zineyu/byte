//! Integration tests for session persistence, including compaction entries.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use tokio::io::AsyncWriteExt;

use byte_protocol::{CompactionEntry, CompactionRange, MessageBody, MessageRole, SessionEntry};
use byte_session::SessionStore;
use tempfile::tempdir;

fn temp_store() -> SessionStore {
    let dir = tempdir().expect("temp dir");
    SessionStore::new(dir.path().to_path_buf()).expect("store creates")
}

#[tokio::test]
async fn compaction_entry_roundtrips_through_store() {
    let store = temp_store();
    store.new_session("s1", "/workspace").await.unwrap();

    let entry = CompactionEntry {
        id: "ce-1".into(),
        role: MessageRole::Summary,
        summary: "summary text".into(),
        compacted_range: CompactionRange {
            first_message_id: "m1".into(),
            last_message_id: "m3".into(),
        },
        created_at: "t".into(),
        run_id: "r1".into(),
    };
    let id = store
        .append_compaction_entry("s1", entry.clone())
        .await
        .expect("append compaction entry");

    assert_eq!(id, "ce-1");

    let entries = store.read_entries("s1").await.unwrap();
    assert_eq!(entries.len(), 2);
    assert!(
        matches!(
            &entries[1],
            SessionEntry::CompactionEntry(CompactionEntry { id, summary, .. })
            if id == "ce-1" && summary == "summary text"
        ),
        "second entry should be the compaction entry"
    );
}

#[tokio::test]
async fn old_session_without_compaction_entries_loads_successfully() {
    let store = temp_store();
    store.new_session("s1", "/workspace").await.unwrap();
    let _ = store
        .append_message(
            "s1",
            Some("m1"),
            None,
            MessageRole::Developer,
            MessageBody::text("hello"),
            None,
        )
        .await
        .unwrap();

    let entries = store.read_entries("s1").await.unwrap();
    assert_eq!(entries.len(), 2);
    assert!(matches!(
        &entries[0],
        SessionEntry::Session { id, .. } if id == "s1"
    ));
    assert!(matches!(
        &entries[1],
        SessionEntry::Message(msg) if msg.id == "m1"
    ));
}

#[tokio::test]
async fn unknown_session_entry_variant_is_ignored() {
    let store = temp_store();
    store.new_session("s1", "/workspace").await.unwrap();
    let _ = store
        .append_message(
            "s1",
            Some("m1"),
            None,
            MessageRole::Developer,
            MessageBody::text("hello"),
            None,
        )
        .await
        .unwrap();

    // Manually append an unrecognized future record type.
    let path = store.session_path("s1").unwrap();
    tokio::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .await
        .unwrap()
        .write_all(b"{\"type\":\"future_variant\",\"id\":\"x\"}\n")
        .await
        .unwrap();

    let entries = store.read_entries("s1").await.unwrap();
    assert_eq!(entries.len(), 2);
    assert!(
        entries
            .iter()
            .all(|e| !matches!(e, SessionEntry::CompactionEntry(_)))
    );
}
