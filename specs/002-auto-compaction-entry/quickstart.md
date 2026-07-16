# Quickstart: Validate Automatic Compaction Entry

**Feature**: specs/002-auto-compaction-entry  
**Date**: 2026-07-16

## Prerequisites

- Development environment entered via `devenv shell` or `direnv allow`.
- Daemon can be started manually with `just start-daemon`.
- Desktop can be started with `just start-desktop`.
- A Model Provider is configured in `~/.config/byte/config.toml`.

## Build & Verify

```bash
just verify rust        # cargo fmt, clippy, test
just verify desktop     # pnpm install, typecheck, build
just verify             # full gate including repo checks
```

## Validation Scenarios

### Scenario 1: Long Session passes the Context Budget

**Goal**: Confirm that a Session can continue after the active path reaches 90% of the context budget.

1. Start the daemon: `just start-daemon`
2. Start the desktop: `just start-desktop` in another terminal
3. Connect the desktop to `127.0.0.1:8787`.
4. Open a new Session in a workspace.
5. Send enough messages to the agent that the accumulated active path reaches the configured context budget (use a small model context window or a test harness to speed this up).

**Expected outcome**:
- The Run does not fail with a context-length error.
- A `CompactionStarted` event is emitted, followed by `CompactionCompleted`.
- The Run finishes with `RunStatus::Succeeded`.
- A Compaction Entry appears in the timeline.

**Contract references**: [runtime-events.md](contracts/runtime-events.md), [data-model.md](data-model.md#state-transitions)

---

### Scenario 2: Compaction Entry is Visible and Expandable

**Goal**: Confirm UI rendering of Compaction Entries.

1. After Scenario 1, locate the Compaction Entry in the Session timeline.
2. Verify it is visually distinct from regular messages.
3. Click/activate the expand control.

**Expected outcome**:
- The expanded view shows the summary text and the range of compacted messages.
- The original messages are accessible from the expanded view (inline or via navigation).
- Collapsing the entry returns to the compact summary view.

**Contract references**: [ui-contract.md](contracts/ui-contract.md)

---

### Scenario 3: Original History is Preserved After Reload

**Goal**: Confirm that compaction does not delete original messages.

1. After Scenario 1, note the IDs of the compacted messages.
2. Close the desktop or restart the daemon.
3. Reconnect and load the same Session.

**Expected outcome**:
- The Compaction Entry is still present in the timeline.
- All original messages are still present in the Session tree.
- The active path is reconstructed correctly, including the Compaction Entry and recent messages.

**Contract references**: [data-model.md](data-model.md#active-path-reconstruction), [protocol-types.md](contracts/protocol-types.md#backward-compatibility)

---

### Scenario 4: Single Large Message Exceeds Budget

**Goal**: Confirm graceful failure without infinite compaction loops.

1. Create a new Session.
2. Send a single message or trigger a tool result whose size exceeds the configured context budget.

**Expected outcome**:
- A `CompactionStarted` event is NOT emitted (there is nothing to compact).
- The Run fails with a clear error message.
- No infinite loop occurs; the Run ends with `RunStatus::Failed`.

**Contract references**: [data-model.md](data-model.md#failure-transitions), [runtime-events.md](contracts/runtime-events.md#compaction-failure)

---

### Scenario 5: Multiple Compaction Entries in One Session

**Goal**: Confirm multiple Compaction Entries can exist and reload correctly.

1. Continue the Session from Scenario 1 until the active path reaches 90% of the budget a second time.

**Expected outcome**:
- A second `CompactionStarted` / `CompactionCompleted` pair is emitted.
- Two distinct Compaction Entries appear in the timeline.
- The Session reloads successfully with both Compaction Entries and all original messages.

**Contract references**: [data-model.md](data-model.md#relationships), FR-007 in [spec.md](spec.md)

## Success Criteria Mapping

| Scenario | Success Criteria |
|----------|------------------|
| 1 | SC-001, SC-003 |
| 2 | SC-004, SC-007 |
| 3 | SC-002, SC-005 |
| 4 | SC-006 |
| 5 | SC-005 |

## Cleanup

- Stop the desktop process.
- Stop the daemon with `Ctrl-C`.
- Delete any test Sessions if desired through the UI or by removing the session storage directory.

## Automated Test Coverage

The manual scenarios above require a live daemon and desktop session. Their core behaviors are covered by the automated test suite:

| Scenario | Automated coverage |
|----------|--------------------|
| 1. Long session past budget | `compaction_triggered_when_budget_exceeded` (`crates/byte-core/src/runner.rs`) |
| 2. Visible/expandable Compaction Entry | Frontend tests: `apps/desktop/src/store/reducer.test.ts` (compaction events), `apps/desktop/src/store/selectors.test.ts` (timeline items); component: `apps/desktop/src/CompactionEntry.tsx` |
| 3. Original history preserved | `build_view_original_messages_remain_after_compaction` (`crates/byte-core/src/session_view_repository.rs`) |
| 4. Single large message exceeds budget | `single_message_exceeds_budget_returns_error` (`crates/byte-core/src/compaction.rs`) |
| 5. Multiple Compaction Entries | `build_view_reloads_session_with_multiple_compaction_entries` (`crates/byte-core/src/session_view_repository.rs`) |
