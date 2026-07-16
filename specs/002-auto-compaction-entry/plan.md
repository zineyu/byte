# Implementation Plan: Automatic Compaction Entry

**Branch**: `002-auto-compaction-entry` | **Date**: 2026-07-16 | **Spec**: [specs/002-auto-compaction-entry/spec.md](spec.md)

**Input**: Feature specification from `specs/002-auto-compaction-entry/spec.md`

## Summary

在 Session 的 active Message History 达到 Model Provider 配置的 Context Budget 90% 时，自动触发 Compaction：由 daemon 调用同一个 Model Provider 生成一段自然语言摘要，并把最老的连续消息块替换为一个持久化、可见的 Compaction Entry。后续 Model Turn 使用 Compaction Entry 代替被压缩的旧消息来构造 LLM 上下文，但原始消息仍保留在 Session 树中，可审计、可恢复。桌面端将 Compaction Entry 渲染为可展开的时间线节点。

## Technical Context

**Language/Version**: Rust (stable, workspace `crates/*`) + TypeScript/React (Tauri v2 desktop shell)

**Primary Dependencies**: 复用现有 crate 边界；新增协议类型与运行时事件全部落在 `byte-protocol`，compaction 逻辑落在 `byte-core`，持久化扩展落在 `byte-session`，模型调用复用 `byte-models`。

**Storage**: JSONL 树状 Session 持久化（`byte-session`），向后兼容旧 Session 格式。

**Testing**: `cargo test`；桌面端 `pnpm typecheck` / `pnpm build`。

**Target Platform**: 本地桌面（Tauri + 手动启动的 Rust daemon）。

**Project Type**: desktop-app with local daemon。

**Performance Goals**: Compaction 作为 Run 内同步步骤，应在一次普通模型调用量级时间内完成；用户可观察到 Run 处于“compaction”状态，但不能出现无响应卡死。

**Constraints**: 仅在本地可信环境运行；daemon 监听 `127.0.0.1`/`localhost`；桌面壳拒绝非本地 daemon 地址；API key 明文存储风险已接受。

**Scale/Scope**: 单用户、本地 Session；预期同一 Session 内 Compaction Entry 数量 < 100。

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- [x] **Shared protocol**: Cross-boundary types, JSON-RPC commands, runtime events, and session views are placed in or reused from `byte-protocol`; no redefinitions in `byte-daemon`, `apps/desktop/src-tauri`, or React.
- [x] **Dependency direction**: The proposed crate/module graph does not introduce reverse dependencies (`byte-daemon` → `byte-core` → `byte-tools`/`byte-skills`/`byte-models`/`byte-session`; all → `byte-protocol`).
- [x] **Daemon/client boundary**: Business logic, model/tool loops, and runtime state live in the Rust daemon; the desktop shell only connects, transports, and renders.
- [x] **Trusted-local risk**: Unrestricted file/command access is acknowledged, the feature runs only in a trusted local environment, and any new risk is documented in the README security notice or an ADR.
- [x] **Test coverage**: JSON-RPC framing, request/response correlation, event ordering, session persistence, tool execution, or skill changes include regression or contract tests.
- [x] **Domain language**: New terms, entities, or user-facing names match `CONTEXT.md` vocabulary; divergent synonyms are avoided.
- [x] **Documentation/ADR impact**: `README.md`, `AGENTS.md`, `DESIGN.md`, `CONTEXT.md`, or ADRs are updated if the feature changes architecture, UI, domain language, or workflow.
- [x] **Quality gates**: `just verify` is expected to pass after implementation.

**GATE STATUS**: PASS (post-design re-check completed on 2026-07-16)

### Post-Design Notes

- All new cross-boundary types (`CompactionEntry`, `CompactionRange`, and the three compaction runtime events) are placed in `byte-protocol`.
- No new JSON-RPC commands are added; compaction state is exposed via the existing `runtime_event` notification channel, preserving the thin-client boundary.
- `byte-core` owns the compaction decision and summarization coordination; `byte-session` owns persistence; `byte-models` owns the model call; the desktop shell only renders the events and persisted view.
- The `MessageRole::Summary` variant already exists in `byte-protocol`; we reuse it rather than introducing a new synonym.
- `CONTEXT.md` and the protocol glossary should be updated when the implementation adds the `CompactionEntry` term and the new runtime event kinds; this is tracked as part of implementation tasks.

## Project Structure

### Documentation (this feature)

```text
specs/002-auto-compaction-entry/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
└── tasks.md             # Phase 2 output (created by /speckit.tasks)
```

### Source Code (repository root)

```text
crates/
├── byte-protocol/src/
│   ├── types/          # CompactionEntry, CompactionEvent, CompactionStateView
│   └── events.rs       # runtime event enums
├── byte-core/src/
│   ├── session/        # active-path reconstruction, budget monitoring
│   └── compaction.rs   # compaction logic & summarizer coordination
├── byte-models/src/
│   └── provider.rs     # summarization call via existing provider abstraction
├── byte-session/src/
│   ├── tree.rs         # CompactionEntry node support
│   └── persistence.rs  # JSONL backward-compatible format
└── byte-daemon/src/
    └── rpc/            # expose compaction state via existing JSON-RPC surface

apps/desktop/
├── src/
│   ├── components/     # CompactionEntry timeline item
│   └── stores/         # handle compaction runtime events
└── src-tauri/src/
    └── lib.rs          # event forwarding (no new business logic)
```

**Structure Decision**: 新增协议类型进入 `byte-protocol`；运行时 compaction 决策进入 `byte-core`；持久化改动进入 `byte-session`；模型调用复用 `byte-models`；daemon 仅做连接/转发；桌面端仅做渲染。没有新增 crate，不引入反向依赖。

## Complexity Tracking

> **No Constitution Check violations require justification.**

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| — | — | — |
