# Tasks: Automatic Compaction Entry

**Input**: Design documents from `specs/002-auto-compaction-entry/`

**Prerequisites**: plan.md, spec.md, data-model.md, contracts/, research.md, quickstart.md

**Tests**: Tests are required for protocol, runtime, persistence, and session changes. UI-only changes are validated via typecheck/build.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Maps task to specific user story (US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Prepare documentation and project conventions before implementation

- [x] T001 Review and confirm existing workspace/crate layout matches `plan.md` project structure
- [x] T002 [P] Add `CompactionEntry` domain definition to `CONTEXT.md` and `docs/protocol/glossary.md`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core protocol, persistence, and runtime primitives that MUST be complete before any user story can be implemented

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

### Protocol Types

- [x] T003 [P] Add `CompactionEntry` struct to `crates/byte-protocol/src/session.rs` (or `crates/byte-protocol/src/types/compaction.rs` if created)
- [x] T004 [P] Add `CompactionRange` struct to `crates/byte-protocol/src/session.rs` (or `crates/byte-protocol/src/types/compaction.rs`)
- [x] T005 [P] Add `CompactionStarted`, `CompactionCompleted`, `CompactionFailed` variants to `RuntimeEventKind` in `crates/byte-protocol/src/lib.rs`
- [x] T006 [P] Add `SessionEntry::CompactionEntry` variant to `crates/byte-protocol/src/session.rs` (and serialization tests)
- [x] T007 [P] Add `MessageRole::Summary` roundtrip tests for the new compaction event kinds in `crates/byte-protocol/src/lib.rs`

### Persistence Foundation

- [x] T008 Add `CompactionEntry` node support in `crates/byte-session/src/tree.rs`
- [x] T009 Implement backward-compatible `SessionEntry` deserialization in `crates/byte-session/src/persistence.rs` (ignore/ degrade unknown variants)
- [x] T010 [P] Add persistence roundtrip tests for `SessionEntry::CompactionEntry` in `crates/byte-session/tests/`

### Runtime Foundation

- [x] T011 Implement active-path reconstruction with `CompactionEntry` skipping in `crates/byte-core/src/session/active_path.rs` (or equivalent module)
- [x] T012 Implement active-path token estimator in `crates/byte-core/src/session/budget.rs` (or equivalent module)
- [x] T013 [P] Add unit tests for active-path reconstruction and budget estimation in `crates/byte-core/tests/`

**Checkpoint**: Foundation ready — user story implementation can now begin in parallel

---

## Phase 3: User Story 1 - Continue a Long Coding Session Past the Context Budget (Priority: P1) 🎯 MVP

**Goal**: Automatically detect when a Session approaches the Context Budget and create a Compaction Entry so the Run can continue.

**Independent Test**: Send enough messages to push the active path to ≥90% of the configured context budget; verify the Run succeeds, a Compaction Entry is persisted, and the Session remains coherent.

### Tests for User Story 1 ⚠️

> Tests are mandatory for runtime, protocol, persistence, tool, or bug-fix changes.

- [x] T014 [P] [US1] Add contract tests for new `CompactionEntry` and compaction events in `crates/byte-protocol/tests/contract_tests.rs` (or equivalent)
- [x] T015 [P] [US1] Add unit tests for compaction decision and range selection in `crates/byte-core/tests/compaction_tests.rs`
- [x] T016 [P] [US1] Add integration test for full compaction flow in `crates/byte-core/tests/compaction_integration.rs`

### Implementation for User Story 1

- [x] T017 [P] [US1] Implement compaction trigger logic (90% budget detection) in `crates/byte-core/src/compaction.rs`
- [x] T018 [P] [US1] Implement oldest-contiguous-block selection algorithm in `crates/byte-core/src/compaction.rs`
- [x] T019 [US1] Implement summarization prompt and provider call in `crates/byte-core/src/compaction.rs` using `byte-models` provider abstraction
- [x] T020 [US1] Integrate compaction into the Run lifecycle in `crates/byte-core/src/runner.rs` (or equivalent) so the Run waits for compaction before the next Model Turn
- [x] T021 [US1] Emit `CompactionStarted`, `CompactionCompleted`, `CompactionFailed` runtime events from `crates/byte-daemon/src/events.rs` (or equivalent)
- [x] T022 [US1] Add `compaction_entry_written` event ordering test in `crates/byte-daemon/tests/event_ordering_tests.rs`

**Checkpoint**: User Story 1 should be fully functional and testable independently

---

## Phase 4: User Story 2 - Inspect What Was Compacted (Priority: P2)

**Goal**: Render Compaction Entries as visible, expandable timeline items in the Desktop Coding Agent UI.

**Independent Test**: After a Session has been compacted, open the UI, locate the Compaction Entry, and verify it expands to show the summary and compacted range while remaining visually distinct from regular messages.

### Tests for User Story 2

- [x] T023 [P] [US2] Add desktop typecheck/build test via `pnpm typecheck` and `pnpm build` in `apps/desktop/`

### Implementation for User Story 2

- [x] T024 [P] [US2] Create `CompactionEntry` React component in `apps/desktop/src/components/CompactionEntry.tsx`
- [x] T025 [P] [US2] Implement expand/collapse state and summary/range rendering in `apps/desktop/src/components/CompactionEntry.tsx`
- [x] T026 [US2] Wire `CompactionEntry` into the Session timeline renderer in `apps/desktop/src/components/SessionTimeline.tsx` (or equivalent)
- [x] T027 [US2] Handle `CompactionStarted`, `CompactionCompleted`, `CompactionFailed` events in the frontend store at `apps/desktop/src/stores/sessionStore.ts` (or equivalent)
- [x] T028 [US2] Apply DESIGN.md tokens for Compaction Entry visuals; update `DESIGN.md` if new tokens are introduced

**Checkpoint**: User Stories 1 AND 2 should both work independently

---

## Phase 5: User Story 3 - Preserve Original History After Compaction (Priority: P3)

**Goal**: Ensure original messages remain recoverable after compaction and Sessions with multiple Compaction Entries reload correctly.

**Independent Test**: Compact a Session, reload it from persisted storage, and verify every original message is still present and the active path is reconstructed correctly.

### Tests for User Story 3 ⚠️

> Tests are mandatory for runtime, protocol, persistence, tool, or bug-fix changes.

- [x] T029 [P] [US3] Add persistence test that original messages remain after compaction in `crates/byte-session/tests/`
- [x] T030 [P] [US3] Add backward compatibility test for old Session format without `CompactionEntry` in `crates/byte-session/tests/`
- [x] T031 [P] [US3] Add multi-compaction Session reload test in `crates/byte-session/tests/`
- [x] T032 [P] [US3] Add edge-case tests for single large message, empty prior history, and cancellation during compaction in `crates/byte-core/tests/`

### Implementation for User Story 3

- [x] T033 [US3] Verify and finalize original-message preservation in `crates/byte-session/src/tree.rs`
- [x] T034 [US3] Verify and finalize multi-compaction active path reconstruction in `crates/byte-core/src/session/active_path.rs`
- [x] T035 [US3] Implement graceful failure paths for empty prior history and single-message-exceeds-budget cases in `crates/byte-core/src/compaction.rs`
- [x] T036 [US3] Implement cancellation handling during compaction in `crates/byte-core/src/runner.rs` (or equivalent)

**Checkpoint**: All user stories should now be independently functional

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Final documentation, validation, and quality gates

- [x] T037 [P] Update `README.md` and `AGENTS.md` if compaction introduces new workflow steps (e.g., no new steps expected; verify and document if any)
- [x] T038 Update `DESIGN.md` with final Compaction Entry visual tokens and prose if UI changes were made
- [x] T039 Run quickstart validation scenarios from `specs/002-auto-compaction-entry/quickstart.md`
- [x] T040 Run `cargo fmt` and `cargo clippy` across affected crates
- [x] T041 Run `just verify` full quality gate
- [x] T042 [P] Final code review and cleanup for `byte-protocol`, `byte-session`, `byte-core`, and `apps/desktop` changes

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Stories (Phase 3–5)**: All depend on Foundational phase completion
  - User stories can then proceed in parallel (if staffed)
  - Or sequentially in priority order (P1 → P2 → P3)
- **Polish (Phase 6)**: Depends on all desired user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Foundational (Phase 2) — no dependencies on other stories
- **User Story 2 (P2)**: Can start after Foundational (Phase 2) and ideally after US1 core events are emitted — independently testable via mocked events
- **User Story 3 (P3)**: Can start after Foundational (Phase 2) — primarily validates persistence and edge cases

### Within Each User Story

- Models/protocol types before services
- Services before integration
- Core implementation before tests that depend on it
- Story complete before moving to next priority

### Parallel Opportunities

- All Foundational tasks marked [P] can run in parallel (within Phase 2)
- Once Foundational phase completes, US1, US2, and US3 can start in parallel (if team capacity allows)
- All tests for a user story marked [P] can run in parallel
- UI tasks in US2 can run in parallel with backend tasks in US1/US3 once event types are stable

---

## Parallel Example: User Story 1

```bash
# Launch all tests for User Story 1 together:
Task: "T014 [P] [US1] Add contract tests for new CompactionEntry and compaction events"
Task: "T015 [P] [US1] Add unit tests for compaction decision and range selection"
Task: "T016 [P] [US1] Add integration test for full compaction flow"

# Launch foundational implementation tasks together:
Task: "T017 [P] [US1] Implement compaction trigger logic in crates/byte-core/src/compaction.rs"
Task: "T018 [P] [US1] Implement oldest-contiguous-block selection algorithm"
Task: "T019 [US1] Implement summarization prompt and provider call"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: User Story 1
4. **STOP and VALIDATE**: Test User Story 1 independently using `quickstart.md` Scenario 1
5. Demo the MVP: a long Session can exceed the original Context Budget without failing

### Incremental Delivery

1. Setup + Foundational → Foundation ready
2. Add User Story 1 → Test independently → Deploy/Demo (MVP!)
3. Add User Story 2 → Test independently → UI timeline rendering complete
4. Add User Story 3 → Test independently → Persistence and edge cases hardened
5. Each story adds value without breaking previous stories

### Parallel Team Strategy

With multiple developers:

1. Team completes Setup + Foundational together
2. Once Foundational is done:
   - Developer A: User Story 1 (compaction logic + events)
   - Developer B: User Story 2 (UI timeline + event handling)
   - Developer C: User Story 3 (persistence + edge cases)
3. Stories complete and integrate independently; final Polish phase merges

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Verify tests fail before implementing (red-green where applicable)
- Commit after each task or logical group using `jj`
- Stop at any checkpoint to validate a story independently
- Avoid: vague tasks, same file conflicts, cross-story dependencies that break independence
