# Feature Specification: Core Coding Loop

**Feature Branch**: `not-created (no before_specify hook configured)`

**Created**: 2026-07-16

**Status**: Draft

**Input**: User description: "https://github.com/zineyu/byte/issues/9 — Complete the Core Coding Loop so a Session Run can move through model output, tool calls, tool results, and continued model reasoning until completion or cancellation."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Complete a Tool-Assisted Coding Request (Priority: P1)

As a Developer, I want the Desktop Coding Agent to inspect my Code Workspace, modify a file, verify the change with a command, and continue reasoning until it gives me a final response, so that I can complete a useful coding task in one Conversation Turn without manually coordinating each Model Turn.

**Why this priority**: This is the Core Coding Loop and the minimum workflow that makes the product useful as a coding agent rather than a chat interface.

**Independent Test**: Start a Run with a controlled coding request that requires reading an existing file, editing that file, running a non-interactive verification command, and interpreting the result. The story passes when the Session records each action and result and the Desktop Coding Agent returns a final assistant response without client-side orchestration.

**Acceptance Scenarios**:

1. **Given** a trusted Code Workspace with a readable file and a passing verification command, **When** the Developer requests a change that requires inspection, editing, and verification, **Then** the Run completes the required tool actions, uses their results in later Model Turns, and returns a final assistant response.
2. **Given** a tool completes with a recoverable failure such as a failed patch, missing file, non-zero command exit, or timeout, **When** the Model Provider requests further reasoning, **Then** the failure result is available in the next Model Turn so the Desktop Coding Agent can recover, explain the problem, or choose another action.
3. **Given** an assistant response requests multiple tool calls, **When** those calls are processed, **Then** every result remains correlated with its originating call and is included in the continued reasoning in a deterministic order.

---

### User Story 2 - Cancel an Active Run Safely (Priority: P2)

As a Developer, I want to cancel an active Run, so that I can stop unwanted model reasoning or tool execution without corrupting the Session or preventing later work.

**Why this priority**: The Core Coding Loop may execute commands or make file changes in unrestricted local agent mode, so the Developer needs a reliable way to stop ongoing work.

**Independent Test**: Start a Run that remains active during model output or a long-running tool action, request cancellation, and verify that the Run reaches one cancelled outcome, preserves only coherent completed history, and allows a subsequent Run in the same Session.

**Acceptance Scenarios**:

1. **Given** a Run is waiting for or receiving Model Provider output, **When** the Developer cancels it, **Then** the Run stops, reports cancellation once, and does not persist an incomplete assistant message.
2. **Given** a cancellable tool action is active, **When** the Developer cancels the Run, **Then** cancellation is propagated to the action, the Run reaches a cancelled outcome, and completed Session history remains recoverable.
3. **Given** a Run has been cancelled, **When** the Developer sends a later message in the same Session, **Then** a new Run can start normally.

---

### User Story 3 - Prevent Conflicting Runs (Priority: P3)

As a Developer, I want each Session to allow only one active Run at a time, so that messages, tool results, file actions, and assistant responses cannot be interleaved into an ambiguous history.

**Why this priority**: Serializing work within a Session protects history coherence while still allowing independent Sessions to be used concurrently.

**Independent Test**: Start a long-running Run, submit a second message to the same Session, and start another Run in a different Session. The same-Session request must be rejected without altering history, while the other Session proceeds independently.

**Acceptance Scenarios**:

1. **Given** a Session already has an active Run, **When** another message is submitted to that Session, **Then** the new request is rejected as busy and the active Run is unchanged.
2. **Given** one Session has an active Run, **When** a message is submitted to a different Session, **Then** the second Session can start its own Run independently.
3. **Given** a Run ends successfully, fails, or is cancelled, **When** another message is submitted to that Session, **Then** the Session is no longer considered busy and can accept the new Run.

### Edge Cases

- The Model Provider returns a tool-call-only assistant message with no visible text.
- The Model Provider returns text and tool calls in the same completed assistant message.
- A requested tool name is unknown, its arguments are invalid, or execution cannot start.
- A tool starts but returns a failed outcome, including a non-zero command exit or timeout.
- Cancellation arrives before the first Model Turn, during streamed model output, during tool execution, between tool completion and result recording, or immediately before normal completion.
- The Model Provider or a tool fails after earlier completed messages or tool results have already been recorded.
- The Desktop Coding Agent repeatedly requests tools without reaching a final answer; this is accepted behavior in this iteration, and the system relies on the Model Provider and system prompts to terminate naturally. A future iteration may introduce an operational limit if needed.
- The client disconnects or restarts while a Run is active; durable Session history remains the recovery source, while transient progress may be lost.
- Multiple tool calls are returned together and one fails while others succeed.
- Relative file and command paths are requested while multiple Sessions are bound to different Code Workspaces.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The runtime MUST own the complete Core Coding Loop; the desktop client MUST only submit commands and display state, not decide when to call the Model Provider or execute tools.
- **FR-002**: A Run MUST support multiple Model Turns, continuing whenever a completed assistant message requests one or more tool calls and ending when a completed assistant message requests no further tools.
- **FR-003**: Each Model Turn MUST receive the relevant active Session history, including completed assistant tool calls and their correlated tool results from earlier turns in the same Run.
- **FR-004**: Completed assistant messages that request tools MUST be recorded as coherent messages containing both their text, when present, and their tool calls.
- **FR-005**: Every tool outcome MUST remain correlated with the tool call that produced it and MUST be recorded in the Session before subsequent reasoning depends on it.
- **FR-006**: Successful tool outcomes, completed failure outcomes, and failures that prevent tool execution from starting MUST be represented to the Model Provider in a form that permits continued reasoning.
- **FR-007**: The Run MUST process all tool calls from a completed assistant message in a deterministic order and MUST make all corresponding outcomes available before the next Model Turn begins.
- **FR-008**: A Session MUST permit at most one active Run; additional same-Session submissions MUST be rejected without changing the active Run or appending the rejected message to Session history.
- **FR-009**: Different Sessions MUST be able to have active Runs independently.
- **FR-010**: The Developer MUST be able to request cancellation of the active Run for a Session, including while waiting for model output or while a tool action is active.
- **FR-011**: Every accepted Run MUST reach exactly one terminal outcome: completed, failed, or cancelled.
- **FR-012**: Cancellation or failure MUST NOT leave a partial assistant message in durable Session history; already completed developer, assistant, and tool messages MUST remain coherent and recoverable.
- **FR-013**: After any terminal outcome or internal error, the Session MUST release its active-Run state so a later Run can start.
- **FR-014**: Runtime progress MUST expose ordered visibility of Run start, completed messages, tool start, tool output where available, tool finish, cancellation, errors, and the terminal Run outcome.
- **FR-015**: Relative tool operations MUST be resolved against the Code Workspace bound to the Session, without leaking workspace context between Sessions.
- **FR-016**: A representative demo Run MUST be able to read workspace files, modify a file, execute a non-interactive command, interpret the resulting output or failure, and produce a final assistant response.
- **FR-017**: Session history written by the loop MUST remain sufficient to reconstruct the completed active conversation path after the client or daemon is restarted.

### Security & Risk Requirements

- **SR-001**: The feature MUST operate only in a trusted local environment and within a Code Workspace intentionally opened by the Developer; remote daemon access is out of scope.
- **SR-002**: The feature MUST preserve the documented unrestricted local agent mode, in which model-requested file changes and commands may cause file damage, secret exposure, deletion, or network activity without runtime permission filtering.
- **SR-003**: Real secrets, API keys, and tokens MUST NOT be added to the Code Workspace or Session artifacts as part of the demo or verification scenarios.
- **SR-004**: Cancellation MUST be treated as best-effort interruption: actions completed before cancellation may have lasting effects, and the Run MUST report a coherent cancelled outcome rather than imply those effects were rolled back.

### Protocol & Compatibility Requirements

- **PR-001**: Run lifecycle, message, tool-call, tool-result, cancellation, and runtime-event data shared across process boundaries MUST use the project's canonical shared contracts and terminology.
- **PR-002**: The feature MUST preserve the loopback-only daemon connection restriction.
- **PR-003**: Existing Session history created before this feature MUST remain loadable; any persisted-format change MUST provide backward-compatible recovery or an explicit migration.
- **PR-004**: Runtime events MUST remain an ephemeral projection for live clients; durable Session history MUST remain the authority for recovery.

### Key Entities

- **Session**: A saved conversation and tool-action history for one Developer in one Code Workspace. It owns an ordered active conversation path and may have zero or one active Run.
- **Run**: One accepted execution attempt for a Conversation Turn. It has an identity, lifecycle state, cancellation state, and exactly one terminal outcome.
- **Conversation Turn**: One Developer message and the Desktop Coding Agent's resulting assistant response, potentially spanning multiple Model Turns and tool actions.
- **Model Turn**: One request to the Model Provider and its completed response within a Run. It may produce final assistant content, tool calls, or both.
- **Message**: A durable Session entry with a canonical role and a body of typed blocks. Relevant roles include developer, assistant, and tool.
- **Tool Call**: A model-requested action identified by a call identity, tool name, and arguments; it is part of a completed assistant message.
- **Tool Result**: The correlated completed outcome of a Tool Call, including useful output or a model-visible failure description.
- **Runtime Event**: An ordered, transient notification that lets clients observe Run and tool progress without owning orchestration.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% of representative read → edit → command demo Runs reach a final assistant response without the desktop client initiating intermediate Model Turns or tool actions.
- **SC-002**: In 100% of continuation tests, each completed tool result is present and correctly correlated in the next Model Turn's context.
- **SC-003**: In 100% of controlled same-Session concurrency tests, a second submission is rejected without altering the active Run or durable Session history.
- **SC-004**: In 100% of controlled cross-Session tests, an active Run in one Session does not prevent a Run from starting in another Session.
- **SC-005**: In controlled cancellation tests, 95% of cancellable Runs reach a terminal cancelled state within 2 seconds of the cancellation request, and 100% permit a subsequent Run after termination.
- **SC-006**: Across success, failure, cancellation, provider-error, and tool-error tests, 100% of accepted Runs emit exactly one terminal outcome and release the Session's active-Run state.
- **SC-007**: After restarting and reloading each tested Session, 100% of completed developer messages, assistant messages, tool calls, and tool results on the active path are recoverable without partial assistant content.
- **SC-008**: In a stakeholder-observed demo, a Developer can submit one coding request and understand the final outcome and any tool failures from the resulting Session history without manually repairing or correlating the conversation.

## Assumptions

- Issues #6, #7, and #8 are completed prerequisites that provide file inspection, patch/diff editing, and non-interactive command execution with visible tool lifecycles.
- The Developer intentionally runs the Desktop Coding Agent in a trusted local environment and accepts the documented risks of unrestricted local agent mode.
- Existing Model Providers can return completed assistant messages containing text, tool calls, or both, and can accept prior assistant tool calls plus correlated tool results in later Model Turns.
- Tool actions within one assistant response are processed deterministically before the next Model Turn; parallel tool execution is not required for this feature.
- A finite safety bound is not implemented in this iteration; the system relies on the Model Provider and system prompts to terminate the loop naturally. A future iteration may introduce a configurable or operational limit if needed.
- Out of scope are React-owned loop orchestration, interactive terminal sessions, new permission or sandbox systems, new tool categories, new provider families, new transports, background process management, incremental streaming of tool-call arguments, and hard safety limits on Model Turns.
