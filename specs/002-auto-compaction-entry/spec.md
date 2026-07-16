# Feature Specification: Automatic Compaction Entry

**Feature Branch**: `002-auto-compaction-entry`

**Created**: 2026-07-16

**Status**: Draft

**Input**: User description: "https://github.com/zineyu/byte/issues/10 — When a long Session approaches the context budget, automatically create a visible Compaction Entry and use it for subsequent context construction without deleting original history."

## Clarifications

### Session 2026-07-16

- Q: 自动 Compaction 应在 Context Budget 达到多少时触发？ → A: 90% 触发（在达到硬上限前预留缓冲）。
- Q: Compaction Entry 的摘要内容应采用哪种生成策略？ → A: 模型生成摘要（由 daemon 调用配置好的 Model Provider 对选中的旧消息进行总结）。
- Q: Compaction Entry 应包含哪些核心元数据？ → A: 仅摘要和范围（保存模型生成的摘要文本以及被 compact 的消息范围）。
- Q: Compaction 与当前 Run 的协作方式应如何设计？ → A: 同步阻塞 Run（compaction 作为当前 Run 的一部分完成，Run 等待 compaction 结束后才继续下一个 Model Turn）。
- Q: 触发 Compaction 时，应选择哪些消息进行压缩？ → A: 最老的连续消息块（从 active path 最早的消息开始，连续压缩直到整体低于 90% 预算）。

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Continue a Long Coding Session Past the Context Budget (Priority: P1)

As a Developer, I want the Desktop Coding Agent to keep helping me after a long Session approaches the Model Provider context budget, so that I can continue an extended coding task without losing the thread of the conversation or starting over.

**Why this priority**: This is the primary value of the feature. Without automatic compaction, long Sessions hit a hard context limit and the Core Coding Loop becomes unusable for non-trivial tasks.

**Independent Test**: Open a Session, run enough Conversation Turns that the accumulated Message History approaches the configured context budget, and verify that a new Run can still request and receive a Model Provider response. The story passes when the Run completes and the Session history remains coherent and recoverable.

**Acceptance Scenarios**:

1. **Given** a Session whose active Message History is close to the configured context budget, **When** the Developer sends a new message that starts a Run, **Then** the runtime creates a Compaction Entry before the next Model Turn so the Run can continue.
2. **Given** a Run is in progress and the active Message History grows to approach the context budget, **When** the next Model Turn is requested, **Then** the runtime may create a Compaction Entry after the preceding Model Turn so the Run can continue.
3. **Given** a Compaction Entry has been created, **When** subsequent Model Turns in the same Session are executed, **Then** the Compaction Entry is used in place of the older active-path messages it summarizes, and those older messages remain available for inspection.

---

### User Story 2 - Inspect What Was Compacted (Priority: P2)

As a Developer, I want Compaction Entries to appear as visible, expandable timeline items in the Session view, so that I can understand what part of the conversation was summarized and retain confidence in the Desktop Coding Agent's continued reasoning.

**Why this priority**: Visibility preserves trust and auditability. A hidden summary cache would make the Session feel unreliable or opaque.

**Independent Test**: After a Session has been compacted, open the Desktop Coding Agent interface and locate the Compaction Entry in the timeline. Verify that it can be expanded to reveal the summary content and that the original messages it replaces are still accessible.

**Acceptance Scenarios**:

1. **Given** a Session contains one or more Compaction Entries, **When** the Developer views the Session timeline, **Then** each Compaction Entry is rendered as a distinct timeline item that is clearly identifiable from regular Messages.
2. **Given** a Compaction Entry is visible in the timeline, **When** the Developer expands it, **Then** the summary content and the range of compacted messages are displayed.
3. **Given** a Compaction Entry is visible, **When** the Developer collapses it, **Then** the timeline shows only the compacted summary, preserving screen space and focus.

---

### User Story 3 - Preserve Original History After Compaction (Priority: P3)

As a Developer, I want the original Messages and tool results to remain stored after compaction, so that I can review, audit, or re-expand earlier parts of the conversation without losing fidelity.

**Why this priority**: Original history is the source of truth for Session recovery and user trust. Compaction is a context-optimization technique, not a deletion mechanism.

**Independent Test**: Compact a Session, then reload it from persisted storage and verify that every original developer, assistant, and tool Message that existed before compaction is still recoverable, even though later Model Turns use the Compaction Entry for context construction.

**Acceptance Scenarios**:

1. **Given** a Session has been compacted, **When** the Session is reloaded from persisted storage, **Then** the original compacted messages are still present in the Session tree.
2. **Given** a Compaction Entry exists in an active Session, **When** the Developer navigates to the compacted region, **Then** the full original Messages and tool results can be viewed.
3. **Given** compaction has been triggered multiple times within the same Session, **When** the Session is reloaded, **Then** each Compaction Entry and the original messages between them remain recoverable.

### Edge Cases

- The context budget is reached during the first Model Turn of a Run, leaving no prior active-path history to summarize.
- A Model Turn is so large that even a single assistant message or tool result exceeds the configured budget; compaction cannot help, and the Run must fail gracefully.
- The summary of a Compaction Entry is itself large enough that the active path still exceeds the budget after compaction; the runtime must avoid infinite compaction loops.
- A Compaction Entry is created immediately before the Developer sends a new message, then another is created after the assistant response; multiple Compaction Entries may exist in the same Session.
- The Developer cancels a Run while compaction is in progress; the Session must remain coherent and allow a later Run to start.
- The Model Provider returns a response that references a tool call whose details were compacted; the runtime must still correlate the tool result correctly.
- The persisted Session is loaded on a client that did not observe compaction happening live; the Compaction Entry must still be visible and usable.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The runtime MUST detect when a Session's active Message History is at or above 90% of the configured context budget before or after a Model Turn, and MUST trigger compaction only when necessary.
- **FR-002**: The runtime MUST create a Compaction Entry as a durable, visible Session entry that summarizes the older active-path messages being replaced.
- **FR-003**: Each Compaction Entry MUST preserve enough context from the compacted messages to allow continued reasoning in subsequent Model Turns, including key developer intent, assistant decisions, and tool outcomes.
- **FR-004**: Compaction MUST NOT delete or remove the original Messages and tool results; they MUST remain part of the persisted Session tree.
- **FR-005**: For subsequent Model Turns, the runtime MUST use the Compaction Entry in place of the older active-path messages it summarizes when constructing the context sent to the Model Provider.
- **FR-006**: The Desktop Coding Agent UI MUST render Compaction Entries as expandable timeline items that are visually distinguishable from regular Messages.
- **FR-007**: The runtime MUST support multiple Compaction Entries within the same Session, with later Compaction Entries summarizing older active-path messages that follow earlier Compaction Entries.
- **FR-008**: Compaction MUST be deterministic and reproducible: the same Session state and context budget MUST produce the same Compaction Entry and active path.
- **FR-009**: After compaction, the Session MUST remain loadable and the active conversation path MUST remain reconstructable for future Runs.
- **FR-010**: Compaction MUST gracefully handle the case where the context budget is exceeded by a single large message or tool result by failing the Run with a clear error rather than entering an infinite compaction loop.
- **FR-011**: Compaction MUST complete within the active Run; the Run MUST wait for the Compaction Entry to be created before the next Model Turn begins.

### Security & Risk Requirements

- **SR-001**: The feature MUST operate only in a trusted local environment and within a Code Workspace intentionally opened by the Developer; compaction is not a mechanism for sharing or exposing Session data.
- **SR-002**: Compaction MUST preserve the original Message History without loss so that the Developer retains an audit trail of model decisions, tool calls, and file changes.
- **SR-003**: A Compaction Entry MUST summarize existing conversation content only; it MUST NOT introduce new instructions, system prompts, or behavior that was not derived from the original messages.
- **SR-004**: Any new file access required for compaction summaries or storage MUST be documented as part of the unrestricted local agent mode risk acceptance.

### Protocol & Compatibility Requirements

- **PR-001**: The Compaction Entry type, any new runtime events related to compaction, and any protocol methods used to expose compaction state MUST use the project's shared protocol types and terminology and MUST be documented in the protocol glossary.
- **PR-002**: The loopback-only daemon connection restriction MUST be preserved; compaction state and commands MUST NOT be exposed to non-local clients.
- **PR-003**: Any change to the Session persistence format introduced by Compaction Entries MUST include a backward-compatible recovery strategy or an explicit migration for Sessions created before this feature.
- **PR-004**: Runtime events related to compaction MUST remain an ephemeral projection for live clients; durable Session history MUST remain the authority for reconstructing the active path after restart.

### Key Entities

- **Session**: A saved conversation and tool-action history for one Developer in one Code Workspace. It owns an ordered active conversation path and may contain zero or more Compaction Entries.
- **Compaction Entry**: A visible Session entry containing a natural-language summary of older conversation history and the range of messages it replaces, used for continued context construction. It is a durable, persisted entity and is rendered as a distinct timeline item.
- **Message History**: The complete record of messages and tool actions within a Session, spanning its persisted form, the runtime view, and the LLM context used for a Run.
- **Active Path**: The ordered subset of Message History that the runtime considers when constructing the context for the next Model Turn. After compaction, the Active Path includes recent messages and Compaction Entries in place of the older messages they summarize.
- **Context Budget**: The maximum amount of conversation context the configured Model Provider can accept for a single Model Turn. The runtime monitors this budget to decide when compaction is needed.
- **Run**: One accepted execution attempt for a Conversation Turn. Compaction may be triggered during a Run to keep the Session within the Context Budget.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% of representative long-Session tests that approach the configured Context Budget complete a new Run without manual deletion or restart.
- **SC-002**: In 100% of compaction tests, the Compaction Entry is written as a visible, durable Session entry and the original compacted Messages remain recoverable after reload.
- **SC-003**: In 100% of continued-reasoning tests after compaction, subsequent Model Turns use the Compaction Entry in place of the summarized older messages while still producing coherent assistant responses.
- **SC-004**: In 100% of UI tests, Compaction Entries appear as expandable timeline items that clearly distinguish them from regular Messages.
- **SC-005**: In 100% of multi-compaction tests, a Session with several Compaction Entries reloads correctly and each active path remains reconstructable.
- **SC-006**: In 100% of unbounded-context tests where a single message or result exceeds the Context Budget, the Run fails gracefully with a clear error and no infinite compaction loop occurs.
- **SC-007**: In a stakeholder-observed demo, a Developer can carry on a long coding conversation beyond the original Context Budget and understand which parts of the Session have been compacted.

## Assumptions

- Issue #9 (Core Coding Loop) is completed so that a Session supports multi-turn Runs and durable Message History.
- The context budget is derived from the active Model Provider configuration and is known to the runtime before a Model Turn is sent.
- Compaction is triggered when the active Message History reaches 90% of the configured context budget, leaving buffer for the Compaction Entry itself and the next Model Turn.
- Compaction Entry summaries are generated by invoking the configured Model Provider to summarize the selected older active-path messages, preserving key developer intent, assistant decisions, and tool outcomes.
- When triggered, compaction selects the oldest contiguous block of active-path messages and summarizes them into a single Compaction Entry such that the resulting active path falls below the 90% budget threshold.
- Summarization preserves the essential intent of developer requests, assistant explanations, and tool outcomes without reproducing every verbatim detail.
- A single Compaction Event creates exactly one Compaction Entry; multiple Compaction Events in the same Session produce multiple Compaction Entries.
- Out of scope are: manual compaction triggers, user-editable summaries, compaction across multiple Sessions, compaction that removes original Messages from storage, and policy-based selective compaction (e.g., compact only tool results).
