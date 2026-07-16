# Research: Core Coding Loop

**Feature**: Core Coding Loop
**Date**: 2026-07-16

## Unknowns Resolved

### 1. How to test the full read → edit → command demo deterministically

- **Decision**: Use an extended deterministic `ModelProvider` mock (pattern in `byte-models/src/provider.rs`) that returns a fixed sequence: read_file request, apply_patch request, run_command request, final assistant text.
- **Rationale**: The existing test suite uses `EchoProvider` for deterministic loop tests. A real provider call would be non-deterministic and require external credentials. The mock is sufficient to verify the orchestration contract.
- **Alternatives considered**: Manual end-to-end test with the OpenAI-compatible provider; rejected as a regression test because it is not reproducible in CI and requires secrets.

### 2. Whether to add a hard safety limit on Model Turns

- **Decision**: No hard safety limit is added in this iteration.
- **Rationale**: The user explicitly removed this requirement. The design keeps the loop unbounded; future iterations may introduce provider/prompt-level controls or a soft operational limit if needed.
- **Alternatives considered**: Adding a configurable `max_turns` counter in `RunExecutor::run_inner`; rejected per user decision.

### 3. Whether to persist partial assistant messages on cancellation

- **Decision**: Do not persist partial assistant messages on cancellation.
- **Rationale**: The feature specification explicitly requires that cancellation not leave partial assistant messages in durable Session history (FR-012). The current implementation persists partial content; it must be changed so that only completed messages and tool results are durable.
- **Alternatives considered**: Keeping the current behavior and documenting it as a known exception; rejected because it conflicts with a mandatory requirement.

### 4. Whether new protocol or persisted types are needed

- **Decision**: No new types are needed. The existing `Message`, `MessageBlock`, `LlmMessage`, `ToolCall`, `ToolResult`, and `RuntimeEventKind` types already support the loop.
- **Rationale**: Reusing existing types avoids backward-compatibility risks and respects the constitution's shared-protocol gate.
- **Alternatives considered**: Adding a dedicated `Run` persisted entity or a turn counter in the Session header; rejected because the loop state is transient and should not be persisted.

## Research Sources

- `crates/byte-core/src/runner.rs` — current `RunExecutor` loop, cancellation, and persistence behavior.
- `crates/byte-core/src/llm_context.rs` — context construction and system prompt rebuilding.
- `crates/byte-models/src/provider.rs` — `ModelProvider` trait and `EchoProvider` test fixture.
- `crates/byte-tools/src/lib.rs` — `ToolStreamEvent` and `ToolOutputResult` contracts.
- `specs/001-core-coding-loop/spec.md` — acceptance criteria and success criteria.
- `CONTEXT.md` and `docs/protocol/glossary.md` — canonical domain terminology.
- `.specify/memory/constitution.md` — project architecture and quality gates.
