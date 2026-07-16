# Contract: Runtime Events for Compaction

**Crate**: `crates/byte-protocol`  
**Transport**: JSON-RPC notifications via `runtime_event` method  
**Consumer**: Desktop shell forwards events to React; daemon owns emission

## Event Kinds Added to `RuntimeEventKind`

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum RuntimeEventKind {
    // ... existing variants ...

    /// Compaction started during a run.
    CompactionStarted {
        /// Run identifier.
        run_id: String,
        /// Session identifier.
        session_id: String,
        /// Identifiers of the messages being compacted.
        compacted_range: CompactionRange,
    },

    /// Compaction finished successfully and a Compaction Entry was persisted.
    CompactionCompleted {
        /// Run identifier.
        run_id: String,
        /// Session identifier.
        session_id: String,
        /// The newly created Compaction Entry identifier.
        compaction_entry_id: String,
        /// Summary text produced by the Model Provider.
        summary: String,
        /// Identifiers of the messages that were compacted.
        compacted_range: CompactionRange,
    },

    /// Compaction failed.
    CompactionFailed {
        /// Run identifier.
        run_id: String,
        /// Session identifier.
        session_id: String,
        /// Human-readable error message.
        error: String,
    },
}
```

## Event Sequences

### Normal Compaction

```json
{ "sequence": 20, "type": "run_started", "session_id": "s-1", "run_id": "r-1" }
{ "sequence": 21, "type": "compaction_started", "run_id": "r-1", "session_id": "s-1", "compacted_range": { "firstMessageId": "m-1", "lastMessageId": "m-10" } }
{ "sequence": 22, "type": "compaction_completed", "run_id": "r-1", "session_id": "s-1", "compaction_entry_id": "ce-1", "summary": "...", "compacted_range": { "firstMessageId": "m-1", "lastMessageId": "m-10" } }
{ "sequence": 23, "type": "message_started", "run_id": "r-1", "message_id": "m-11", "role": "assistant" }
// ...
{ "sequence": 30, "type": "run_finished", "run_id": "r-1", "status": "succeeded" }
```

### Compaction Failure

```json
{ "sequence": 21, "type": "compaction_started", "run_id": "r-1", "session_id": "s-1", "compacted_range": { "firstMessageId": "m-1", "lastMessageId": "m-10" } }
{ "sequence": 22, "type": "compaction_failed", "run_id": "r-1", "session_id": "s-1", "error": "compaction did not reduce context below budget" }
{ "sequence": 23, "type": "run_finished", "run_id": "r-1", "status": "failed", "error": "compaction did not reduce context below budget" }
```

> **Field casing**: `RuntimeEventKind` uses `#[serde(tag = "type", rename_all = "snake_case")]`, which renames the variant tags only; struct-variant fields keep their declared snake_case names (`run_id`, `session_id`, `compaction_entry_id`, `compacted_range`). The nested `CompactionRange` is a standalone struct with `rename_all = "camelCase"`, so its fields stay `firstMessageId` / `lastMessageId`.

## Ordering Guarantees

- `CompactionStarted` for a Run is emitted before any `MessageStarted` event that depends on the resulting Compaction Entry.
- `CompactionCompleted` or `CompactionFailed` is emitted before the Run finishes or before the next Model Turn is requested.
- Events are delivered through the same JSON-RPC notification channel as all other runtime events; consumers MUST handle them in sequence order.

## UI Consumer Contract

- The desktop shell MUST forward all `Compaction*` events to React without modification.
- React MAY use `CompactionStarted` to show a spinner or inline loading state on the timeline.
- React MUST use `CompactionCompleted` to insert or update the Compaction Entry timeline item.
- React MUST use `CompactionFailed` to display an error and keep the original messages visible.
