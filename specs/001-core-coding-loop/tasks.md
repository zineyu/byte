# Tasks: Core Coding Loop

**Input**: Design documents from `specs/001-core-coding-loop/`

**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/`

**Tests**: Tests are required for runtime, protocol, persistence, and tool-loop changes per the constitution. This feature changes `RunExecutor` behavior and adds a regression demo, so all user stories include test tasks.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Confirm current workspace and dependencies are ready; no new crates or dependencies are introduced for this feature.

- [x] T001 Run `just verify` to confirm the current workspace passes all quality gates before making changes.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Inspect and confirm the existing loop foundation before modifying behavior. No new foundational code is required; this phase is verification-oriented and blocks all user stories.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [x] T002 Read `crates/byte-core/src/runner.rs` to confirm `RunExecutor::run_inner`, `consume_provider_stream`, and `execute_tool_call` structure and existing multi-turn behavior.
- [x] T003 Read `crates/byte-models/src/provider.rs` to confirm `ModelProvider` trait and `EchoProvider` test fixture contract.
- [x] T004 Read `crates/byte-tools/src/lib.rs` and `crates/byte-tools/src/run_command.rs` to confirm `ToolOutputStream`, `ToolStreamEvent`, and cancellation propagation contracts.
- [x] T005 Read `crates/byte-protocol/src/lib.rs` and `crates/byte-protocol/src/session.rs` to confirm `RuntimeEventKind`, `Message`, `MessageBlock`, `ToolCall`, `ToolResult`, and `LlmMessage` shapes are sufficient for the loop.

**Checkpoint**: Foundation understood — existing multi-turn loop, tool streaming, and cancellation contracts are clear.

---

## Phase 3: User Story 1 - Complete a Tool-Assisted Coding Request (Priority: P1) 🎯 MVP

**Goal**: Enable a single Run to read a file, edit a file, run a command, and produce a final assistant response, with all tool results fed back into subsequent Model Turns.

**Independent Test**: A new `byte-core` integration test starts a Run in a temporary workspace with a deterministic mock provider that requests `read_file`, then `apply_patch`, then `run_command`, then returns final text. The test asserts the Run succeeds, the workspace file is modified, the command ran, and the Session history contains the developer message, assistant tool calls, and correlated tool results in order.

### Tests for User Story 1

- [x] T006 [P] [US1] Add regression test `core_coding_loop_read_edit_command_demo` in `crates/byte-core/src/runner.rs` that uses a deterministic mock provider to exercise the full read → apply_patch → run_command → final response flow.
- [x] T007 [P] [US1] Add regression test `tool_results_feed_next_model_turn` in `crates/byte-core/src/runner.rs` that verifies each completed tool result is present and correlated in the next provider call's context.
- [x] T008 [P] [US1] Add regression test `multiple_tool_calls_in_one_message` in `crates/byte-core/src/runner.rs` that verifies sequential execution and correlation of multiple tool calls from a single assistant message.

### Implementation for User Story 1

- [x] T009 [US1] Extend or create a deterministic `ModelProvider` mock in `crates/byte-models/src/provider.rs` that returns a fixed sequence of tool calls (read_file → apply_patch → run_command) and a final assistant response based on the last tool result.
- [x] T010 [US1] Verify `RunExecutor::run_inner` in `crates/byte-core/src/runner.rs` correctly appends assistant tool calls and tool results to `turn_messages` and feeds them into the next Model Turn's context.
- [x] T011 [US1] Verify `LlmContextBuilder` in `crates/byte-core/src/llm_context.rs` includes prior `assistant` tool calls and matching `tool` results when building the provider context for each subsequent Model Turn.
- [x] T012 [US1] Register `apply_patch` and `run_command` in the test's `MvpToolRegistry` for the demo test, ensuring tool paths resolve against the temporary workspace root.
- [x] T013 [US1] Run `cargo test -p byte-core runner::` and `just verify rust` to confirm US1 tests pass.

**Checkpoint**: User Story 1 should be fully functional and testable independently. The read → edit → command demo succeeds end-to-end.

---

## Phase 4: User Story 2 - Cancel an Active Run Safely (Priority: P2)

**Goal**: Ensure cancellation does not leave a partial assistant message in durable Session history and allows a subsequent Run to start.

**Independent Test**: A new `byte-core` test starts a Run with a slow streaming provider, cancels it after at least one delta, and asserts that the Run reaches `Cancelled`, no partial assistant message is persisted, and a second `send_message` succeeds in the same Session.

### Tests for User Story 2

- [x] T014 [P] [US2] Add regression test `cancel_run_does_not_persist_partial_assistant_message` in `crates/byte-core/src/runner.rs` that verifies the Session history after cancellation contains only the developer message and any completed tool messages, with no partial assistant entry.
- [x] T015 [P] [US2] Add regression test `cancel_run_allows_next_run` in `crates/byte-core/src/runner.rs` that verifies a second `send_message` succeeds after the first Run is cancelled.
- [x] T016 [P] [US2] Add regression test `cancel_run_emits_single_terminal_cancelled_outcome` in `crates/byte-core/src/runner.rs` that verifies exactly one `RunCancelled` and one `RunFinished(Cancelled)` are emitted.

### Implementation for User Story 2

- [x] T017 [US2] Modify `RunExecutor::consume_provider_stream` in `crates/byte-core/src/runner.rs` to flush buffered deltas as `MessageDelta` events on cancellation but **not** call `store.append_message` for the partial assistant message.
- [x] T018 [US2] Ensure `RunExecutor::run` clears `active_run` and emits a single terminal outcome on the cancellation path.
- [x] T019 [US2] Update or remove existing assertions in `cancel_run_flushes_buffer_and_emits_ordered_events` and `cancel_run_then_send_message_starts_new_run` that expected partial assistant persistence, so they match the new semantics.
- [x] T020 [US2] Run `cargo test -p byte-core runner::` and `just verify rust` to confirm US2 tests pass.

**Checkpoint**: User Story 2 should be fully functional and testable independently. Cancellation leaves a clean Session history and permits a subsequent Run.

---

## Phase 5: User Story 3 - Prevent Conflicting Runs (Priority: P3)

**Goal**: Ensure one Session permits only one active Run at a time while allowing independent Sessions to run concurrently.

**Independent Test**: Existing tests already cover `concurrent_send_message_returns_busy` and `runs_on_different_sessions_execute_concurrently`. This phase adds focused regression tests for the new loop behavior and confirms the invariant still holds after the US1/US2 changes.

### Tests for User Story 3

- [x] T021 [P] [US3] Add regression test `same_session_busy_during_multi_tool_loop` in `crates/byte-core/src/runner.rs` that starts a multi-turn Run and asserts a second `send_message` to the same Session returns `Busy` without appending the rejected message to history.
- [x] T022 [P] [US3] Add regression test `cross_session_runs_remain_independent_during_loop` in `crates/byte-core/src/runner.rs` that starts multi-turn Runs in two different Sessions and asserts both proceed concurrently.
- [x] T023 [P] [US3] Add regression test `active_run_released_after_loop_error` in `crates/byte-core/src/runner.rs` that simulates a tool or provider error during a multi-turn Run and asserts `active_run` is released so a new Run can start.

### Implementation for User Story 3

- [x] T024 [US3] Verify `SessionRunner::send_message` in `crates/byte-core/src/runner.rs` still returns `RunnerError::Busy` and does not append the rejected message when a Run is active during the new multi-turn loop.
- [x] T025 [US3] Verify `RunExecutor::run` in `crates/byte-core/src/runner.rs` clears `active_run` on every terminal path (success, failure, cancellation) and after any internal error.
- [x] T026 [US3] Run `cargo test -p byte-core runner::` and `just verify rust` to confirm US3 tests pass.

**Checkpoint**: User Stories 1, 2, and 3 should all work independently. Same-Session concurrency is rejected, cross-Session concurrency works, and active-run state is always released.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, quality gates, and cross-story validation.

- [x] T027 [P] Update `specs/001-core-coding-loop/plan.md` and `specs/001-core-coding-loop/checklists/requirements.md` if any implementation details diverged from the plan.
- [x] T028 [P] Update inline code comments in `crates/byte-core/src/runner.rs` to explain the cancellation/persistence rule and the multi-turn flow if they are not already clear.
- [x] T029 [P] If the cancellation semantics change user-visible behavior materially, add or update a brief note in `docs/adr/` or `README.md` per the constitution's documentation fidelity requirement.
- [x] T030 Run `cargo fmt --all` and `cargo clippy --workspace` to confirm formatting and lints pass.
- [x] T031 Run `just verify` to confirm the full quality gate passes, including Rust tests, desktop typecheck/build, and `npx @google/design.md lint DESIGN.md`.
- [ ] T032 Run the manual validation steps (skipped: requires interactive desktop session) in `specs/001-core-coding-loop/quickstart.md` (optional but recommended): start the daemon and desktop, open a test workspace, and send a read → edit → command request.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — confirms the workspace is green.
- **Foundational (Phase 2)**: Depends on Setup — reads existing code and confirms contracts. Blocks all user stories.
- **User Stories (Phase 3–5)**: All depend on Foundational phase completion.
  - Execute in priority order (P1 → P2 → P3) because later stories may share test helpers or mock provider behavior with earlier stories.
  - T009 (mock provider) is a soft prerequisite for T006–T008; the mock can be built in T006–T008 directly if preferred, but sharing it avoids duplication.
- **Polish (Phase 6)**: Depends on all desired user stories being complete and green.

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Foundational. No dependencies on other stories. MVP scope.
- **User Story 2 (P2)**: Can start after Foundational. Depends on US1 only in that the mock provider may be reused; the cancellation logic is independent.
- **User Story 3 (P3)**: Can start after Foundational. Depends on US1 only in that multi-turn loop state exists; the concurrency invariant is independent.

### Within Each User Story

- Tests before implementation (or red-green) for the changed behavior.
- Update existing tests whose expectations are invalidated by the new cancellation semantics (T019) before declaring US2 done.
- Run the story's test subset before moving to the next story.

### Parallel Opportunities

- All Foundational inspection tasks (T002–T005) can run in parallel.
- Test tasks within a user story (T006–T008, T014–T016, T021–T023) can be written in parallel once the mock provider contract is clear.
- Polish tasks (T027–T032) can run in parallel after implementation is complete, except T031 which depends on T030.
- Different user stories can be worked on in parallel once the shared mock provider is stable, but sequential delivery is recommended for an MVP-first approach.

---

## Parallel Example: User Story 1

```bash
# After T009 deterministic mock provider is in place, launch tests in parallel:
- T006 [P] [US1] core_coding_loop_read_edit_command_demo in crates/byte-core/src/runner.rs
- T007 [P] [US1] tool_results_feed_next_model_turn in crates/byte-core/src/runner.rs
- T008 [P] [US1] multiple_tool_calls_in_one_message in crates/byte-core/src/runner.rs

# Then implementation/verification:
- T010 Verify run_inner appends turn_messages correctly
- T011 Verify LlmContextBuilder includes prior tool calls and results
- T012 Register tools for demo test
- T013 Run cargo test -p byte-core runner::
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup.
2. Complete Phase 2: Foundational inspection.
3. Complete Phase 3: User Story 1 — the read → edit → command demo test passes.
4. **STOP and VALIDATE**: `cargo test -p byte-core runner::` passes; demo is green.
5. Optionally demo to stakeholders before proceeding.

### Incremental Delivery

1. Setup + Foundational → Foundation ready.
2. User Story 1 → Test independently → MVP demo ready.
3. User Story 2 → Test independently → Cancellation semantics fixed.
4. User Story 3 → Test independently → Concurrency invariants verified under multi-turn loop.
5. Polish → Full `just verify` green.

### Parallel Team Strategy

With multiple developers:

1. Team completes Setup + Foundational together.
2. Developer A builds the deterministic mock provider (T009) and US1 tests.
3. Developer B updates cancellation semantics (T017–T019) and US2 tests.
4. Developer C adds US3 concurrency tests and verifies the invariant.
5. Final polish and full verification are done together.

---

## Notes

- [P] tasks = different files or independent concerns, no dependencies on incomplete tasks.
- [Story] label maps each task to the user story it serves.
- Each user story is independently testable; tests should fail before implementation and pass after.
- Prefer `jj desc` before coding and `jj squash` / `jj new` per the repository's `AGENTS.md` and `jujutsu` skill.
- No new protocol types, no new persisted formats, no frontend changes, and no new crate dependencies are required for this feature.
- Avoid vague tasks; every task names a concrete file or test.

---

## Phase 7: Convergence

- [x] T033 [P] Add regression test in `crates/byte-core/src/runner.rs` that asserts the `MessageCompleted` runtime event body contains the assistant text block followed by `ToolCall` blocks when a completed assistant message requests tools per FR-014 / contract `loop.md` (missing).
- [x] T034 [P] Add regression test in `crates/byte-core/src/runner.rs` that cancels a Run while a cancellable tool action is active and verifies the Run reaches a terminal `Cancelled` outcome without partial assistant persistence per US2/AC2 / contract `cancellation.md` (missing).
- [x] T035 [P] Add regression test in `crates/byte-core/src/runner.rs` for an unknown tool or invalid tool arguments, asserting the failure is represented as a `ToolOutputResult::error` in the next Model Turn per FR-006 / spec edge case (missing).
- [x] T036 [P] Add regression test in `crates/byte-core/src/runner.rs` for `ProviderError::InvalidResponse`, asserting the Run fails with a single terminal `RunFinished(Failed)` outcome per contract `loop.md` (missing).
