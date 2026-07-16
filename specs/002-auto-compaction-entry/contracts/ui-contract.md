# Contract: UI / Desktop Shell for Compaction

**Frontend**: `apps/desktop` (React + Tauri v2)  
**Tauri layer**: `apps/desktop/src-tauri` forwards runtime events; implements no compaction logic

## TypeScript Bindings

`ts_rs` generates the following types from `byte-protocol`:

```typescript
type CompactionEntry = {
  id: string;
  role: "summary";
  summary: string;
  compactedRange: {
    firstMessageId: string;
    lastMessageId: string;
  };
  createdAt: string;
  runId: string;
};

type CompactionStartedEvent = {
  type: "compaction_started";
  run_id: string;
  session_id: string;
  compacted_range: CompactionEntry["compactedRange"];
};

type CompactionCompletedEvent = {
  type: "compaction_completed";
  run_id: string;
  session_id: string;
  compaction_entry_id: string;
  summary: string;
  compacted_range: CompactionEntry["compactedRange"];
};

type CompactionFailedEvent = {
  type: "compaction_failed";
  run_id: string;
  session_id: string;
  error: string;
};
```

## Timeline Rendering Contract

- The timeline MUST display a `CompactionEntry` as a distinct item, visually different from `Developer`, `Assistant`, and `Tool` messages.
- The collapsed state MUST show the summary text (truncated if necessary) and an expand affordance.
- The expanded state MUST show the full summary and the range of compacted messages (e.g., "Messages 1–12 summarized" or clickable message IDs).
- The original compacted messages MUST remain reachable from the expanded state, either by inline expansion or by navigation.
- Colors, fonts, radii, and spacing MUST come from `DESIGN.md`; any new token MUST be added to `DESIGN.md`.

## Tauri Command Surface

No new Tauri commands are required. The existing command that loads a Session (`load_session`) returns a `SessionView` that already includes `Message` nodes with `role: "summary"`. The `SessionView` also carries a `compactionEntries` array so the UI can resolve each summary message's compacted message range (`firstMessageId` / `lastMessageId`) without a separate fetch.

If a future iteration exposes manual compaction triggers, a new Tauri command would be added, but that is out of scope for this feature.

## Event Handling Contract

```typescript
function onRuntimeEvent(event: RuntimeEvent) {
  switch (event.type) {
    case "compaction_started":
      showCompactionLoading(event.run_id, event.compacted_range);
      break;
    case "compaction_completed":
      insertCompactionEntry(event);
      break;
    case "compaction_failed":
      showCompactionError(event.run_id, event.error);
      break;
  }
}
```

> **Field casing**: the `compaction_*` events are `RuntimeEventKind` struct variants and keep snake_case field names on the wire (`run_id`, `session_id`, `compaction_entry_id`, `compacted_range`), matching the ts-rs generated `RuntimeEventKind.ts`. Only the nested `CompactionRange` value uses camelCase (`firstMessageId` / `lastMessageId`).

## Accessibility Notes

- The expand/collapse control MUST be keyboard focusable and activate on Enter/Space.
- Screen readers SHOULD announce "Compaction summary" or similar before the summary text.

## Localization Notes

- This MVP is English-only; any user-facing labels like "Compaction summary" or "X messages summarized" are rendered in English. Localization is out of scope.
